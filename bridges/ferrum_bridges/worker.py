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
import time
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
            conn.send(("err", _portable(e)))


def _portable(e: Exception) -> Exception:
    """``e`` if it survives pickling both ways, else a ``RuntimeError`` with its text
    (an exception holding a tensor or a lock, or with an unusual ``__init__``)."""
    import pickle

    try:
        pickle.loads(pickle.dumps(e))  # noqa: S301 - our own bytes, round-tripped in this process
        return e
    except Exception:
        return RuntimeError(f"{type(e).__name__}: {e}")


class WorkerStopped(RuntimeError):
    """The backend worker process ended (crash or unload) during a call."""


class _Child:
    """One running child process and its end of the pipe."""

    def __init__(self, factory, args):
        ctx = mp.get_context("spawn")
        self.conn, child_conn = ctx.Pipe()
        # not a daemon: a backend may start processes of its own (TotalSegmentator jobs); the pipe's
        # EOF ends the child when the bridge dies, and stop() joins or kills it
        self.process = ctx.Process(target=_serve, args=(factory, args, child_conn))
        self.process.start()
        child_conn.close()
        self.lock = threading.Lock()
        self.dead = False

    def alive(self) -> bool:
        return not self.dead and self.process.is_alive()

    def call(self, op: str, sid: int, name: str, *args, **kwargs):
        with self.lock:
            if self.dead:
                raise WorkerStopped("the backend worker stopped; the next session starts it again")
            try:
                self.conn.send((op, sid, name, (args, kwargs)))
                status, value = self.conn.recv()
            except (EOFError, BrokenPipeError, ConnectionResetError, OSError) as e:
                self.dead = True
                code = self.process.exitcode
                raise WorkerStopped(f"the backend worker stopped (exit code {code})") from e
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
        self.done = False

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
        if self.done:
            return
        self.done = True
        try:
            if self.child.alive():
                self._call("close")
        finally:
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
        self._resident: tuple = (0.0, None)  # (when measured, (pid, MiB))
        self.lock = threading.Lock()
        self._ensure()  # load at start, as the in-process backends do
        self.max_sessions = self._ensure().call("attr", 0, "max_sessions")

    def _ensure(self) -> _Child:
        with self.lock:
            if self.child is None or not self.child.alive():
                log.info("starting the backend worker")
                self.child = _Child(self.factory, self.args)
                self._info = None
            return self.child

    def info(self) -> dict:
        if self._info is None:
            self._info = dict(self._ensure().call("backend", 0, "info"))
        # the protocol reads the manifest on every prompt: no measurement here (see resident_mib)
        return dict(self._info)

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
        return child.process.pid if child is not None and child.alive() else None

    #: Seconds a measurement of the child's GPU memory is reused: ``info`` is
    #: called under the protocol's lock, and ``nvidia-smi`` can take a second.
    RESIDENT_TTL_S = 10.0

    def resident_mib(self) -> int | None:
        """GPU memory of the child as ``nvidia-smi`` reports it; 0 when no
        child runs; ``None`` when it cannot be measured."""
        pid = self.pid()
        if pid is None:
            return 0
        when, value = self._resident
        if time.monotonic() - when < self.RESIDENT_TTL_S and value is not None and value[0] == pid:
            return value[1]
        mib = gpu_memory_of(pid)
        self._resident = (time.monotonic(), (pid, mib))
        return mib


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
    rows = [[p.strip() for p in line.split(",")] for line in out.splitlines()]
    mine = [r[1] for r in rows if len(r) == 2 and r[0] == str(pid)]
    # not listed (another pid namespace, WSL) or "[N/A]": unknown, not zero
    if not mine or not all(m.isdigit() for m in mine):
        return None
    return sum(int(m) for m in mine)


__all__ = ["WorkerBackend", "WorkerSession", "gpu_memory_of"]
