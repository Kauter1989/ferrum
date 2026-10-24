"""A backend that runs in a child process, so it can be unloaded completely.

PyTorch cannot give back a CUDA context inside a process: emptying its cache
frees tensors, not the context or the weights (about 2 GB for nnInteractive).
:class:`WorkerBackend` therefore builds the real backend in a child process
(``multiprocessing`` with ``spawn``) and forwards every call over a pipe.
:meth:`WorkerBackend.unload` ends the child, which returns all of its GPU
memory to the driver; the next session starts it again (Stage 23,
``docs/gpu-sharing.md`` §3.5).

Volumes and mask regions travel as NumPy arrays (pickled); the backend's
manifest is read once from the first child and kept.
"""

from __future__ import annotations

import logging
import multiprocessing as mp
import shutil
import subprocess
import threading
from collections.abc import Callable
from typing import Any

from .backends import Backend, BackendSession

log = logging.getLogger("ferrum_bridges")


def _serve(factory: Callable[..., Backend], args: tuple, conn) -> None:
    """Child process: builds the backend and answers calls until the pipe closes."""
    backend = factory(*args)
    sessions: dict = {}
    next_id = 0
    while True:
        try:
            op, sid, name, call_args = conn.recv()
        except EOFError:
            return
        try:
            if op == "attr":
                result = getattr(backend, name)
            elif op == "backend" and name == "open":
                next_id += 1
                sessions[next_id] = backend.open(*call_args[0], **call_args[1])
                result: Any = next_id
            elif op == "backend":
                result = getattr(backend, name)(*call_args[0], **call_args[1])
            elif name == "close":
                session = sessions.pop(sid, None)
                result = session.close() if session is not None else None
            else:
                result = getattr(sessions[sid], name)(*call_args[0], **call_args[1])
            conn.send(("ok", result))
        except Exception as e:  # sent back and raised in the parent
            conn.send(("err", e))


class _Child:
    """One running child process and its end of the pipe."""

    def __init__(self, factory, args):
        ctx = mp.get_context("spawn")
        self.conn, child_conn = ctx.Pipe()
        self.process = ctx.Process(target=_serve, args=(factory, args, child_conn), daemon=True)
        self.process.start()
        child_conn.close()
        self.lock = threading.Lock()

    def call(self, op: str, sid: int, name: str, *args, **kwargs):
        with self.lock:
            self.conn.send((op, sid, name, (args, kwargs)))
            status, value = self.conn.recv()
        if status == "err":
            raise value
        return value

    def stop(self) -> None:
        try:
            self.conn.close()
        finally:
            self.process.join(timeout=10)
            if self.process.is_alive():
                self.process.kill()
                self.process.join(timeout=5)


class WorkerSession(BackendSession):
    """A session living in the child; every call is forwarded."""

    def __init__(self, owner: WorkerBackend, child: _Child, sid: int):
        self.owner, self.child, self.sid = owner, child, sid

    def _call(self, name, *args, **kwargs):
        return self.child.call("session", self.sid, name, *args, **kwargs)

    def point(self, zyx, positive):
        return self._call("point", zyx, positive)

    def box(self, bbox, positive):
        return self._call("box", bbox, positive)

    def scribble(self, mask, bbox, positive):
        return self._call("scribble", mask, bbox, positive)

    def lasso(self, mask, bbox, positive):
        return self._call("lasso", mask, bbox, positive)

    def undo(self):
        return self._call("undo")

    def reset(self):
        return self._call("reset")

    def mask(self, bbox):
        return self._call("mask", bbox)

    def is_empty(self):
        return self._call("is_empty")

    def start_job(self, labels):
        return self._call("start_job", labels)

    def job_status(self, job):
        return self._call("job_status", job)

    def cancel_job(self, job):
        return self._call("cancel_job", job)

    def label_map(self):
        return self._call("label_map")

    def close(self):
        if self.child.process.is_alive():
            self._call("close")
        self.owner.closed(self)


class WorkerBackend(Backend):
    """``factory(*args)`` built and run in a child process; see the module
    documentation. ``factory`` must be importable (a class or a top-level
    function), as ``spawn`` pickles it."""

    def __init__(self, factory: Callable[..., Backend], *args):
        self.factory, self.args = factory, args
        self.child: _Child | None = None
        self.open_sessions = 0
        self._info: dict | None = None
        self.lock = threading.Lock()
        self._ensure()  # load at start, as the in-process backends do
        self.max_sessions = self._ensure().call("attr", 0, "max_sessions")

    def _ensure(self) -> _Child:
        with self.lock:
            if self.child is None or not self.child.process.is_alive():
                log.info("starting the backend worker")
                self.child = _Child(self.factory, self.args)
                self._info = None
            return self.child

    def info(self) -> dict:
        if self._info is None:
            self._info = dict(self._ensure().call("backend", 0, "info"))
        info = dict(self._info)
        info["unload"] = True
        resident = self.resident_mib()
        if resident is not None:
            info["resident_mib"] = resident
        return info

    def open(self, volume, spacing, modality, origin=None, direction=None):
        child = self._ensure()
        sid = child.call("backend", 0, "open", volume, spacing, modality, origin=origin, direction=direction)
        with self.lock:
            self.open_sessions += 1
        return WorkerSession(self, child, sid)

    def closed(self, _session: WorkerSession) -> None:
        with self.lock:
            self.open_sessions = max(0, self.open_sessions - 1)

    def can_unload(self) -> bool:
        return True

    def unload(self) -> None:
        with self.lock:
            if self.open_sessions:
                raise RuntimeError("sessions are open")
            child, self.child = self.child, None
        if child is not None:
            log.info("unloading the backend worker (pid %s)", child.process.pid)
            child.stop()

    def pid(self) -> int | None:
        """Process id of the running child, if any."""
        child = self.child
        return child.process.pid if child is not None and child.process.is_alive() else None

    def resident_mib(self) -> int | None:
        """GPU memory of the child as ``nvidia-smi`` reports it; 0 when no
        child runs; ``None`` without ``nvidia-smi``."""
        pid = self.pid()
        if pid is None:
            return 0
        return gpu_memory_of(pid)


def gpu_memory_of(pid: int) -> int | None:
    """MiB of GPU memory used by ``pid`` per ``nvidia-smi``; ``None`` when it
    cannot be measured."""
    smi = shutil.which("nvidia-smi")
    if smi is None:
        return None
    try:
        out = subprocess.run(  # noqa: S603 - a fixed argument list, no shell
            [smi, "--query-compute-apps=pid,used_memory", "--format=csv,noheader,nounits"],
            capture_output=True,
            text=True,
            timeout=5,
            check=True,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return None
    total = 0
    for line in out.splitlines():
        parts = [p.strip() for p in line.split(",")]
        if len(parts) == 2 and parts[0] == str(pid) and parts[1].isdigit():
            total += int(parts[1])
    return total


__all__ = ["WorkerBackend", "WorkerSession", "gpu_memory_of"]
