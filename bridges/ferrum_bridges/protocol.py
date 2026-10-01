"""FERRUM Engine Protocol (``ferrum-engine/1``) server on top of a backend.

Shared by every bridge in ``bridges/`` (nnInteractive, TotalSegmentator,
MONAI Label).

The protocol is specified in ``docs/engine-protocol.md``. This module is
engine independent: it parses requests, validates prompts against the
uploaded grid, keeps sessions and revisions, and maps errors to the
protocol's ``{"error": {"code", "message"}}`` bodies. The engine itself is a
:class:`~ferrum_bridges.backends.Backend`.

Axis conventions: the protocol uses voxel indices ``(i, j, k)`` with ``i``
fastest. Backends receive NumPy arrays in C order ``[k, j, i]`` (``z, y, x``)
and boxes as ``[[z0, z1], [y0, y1], [x0, x1]]`` (half-open), which is what
nnInteractive expects for an image of shape ``[1, z, y, x]``.
"""

from __future__ import annotations

import base64
import gzip
import json
import logging
import threading
import time
import uuid
from dataclasses import dataclass, field
from typing import Optional

import numpy as np
from fastapi import FastAPI, Request, Response
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from starlette.exceptions import HTTPException as StarletteHTTPException

from .backends import Backend, BackendSession, Unsupported, ZyxBox

PROTOCOL = "ferrum-engine/1"
log = logging.getLogger("ferrum_bridges")

DTYPES = {"int16": "<i2", "uint16": "<u2", "float32": "<f4"}


class ProtocolError(Exception):
    """An error with an HTTP status and a protocol error code."""

    def __init__(self, status: int, code: str, message: str, retry_after: Optional[int] = None):
        super().__init__(message)
        self.status = status
        self.code = code
        self.message = message
        self.retry_after = retry_after


def bad_request(message: str) -> ProtocolError:
    return ProtocolError(400, "bad_request", message)


@dataclass
class Session:
    """One protocol session: the declared grid and, after upload, the backend session."""

    dims: tuple  # (nx, ny, nz)
    dtype: str
    spacing: tuple
    modality: str
    origin: tuple = (0.0, 0.0, 0.0)
    direction: tuple = ((1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (0.0, 0.0, 1.0))
    backend: Optional[BackendSession] = None
    revision: int = 0
    last_used: float = field(default_factory=time.monotonic)

    def zyx_shape(self) -> tuple:
        nx, ny, nz = self.dims
        return (nz, ny, nx)


def to_zyx_box(lo: list, hi: list) -> ZyxBox:
    """Protocol half-open box (i, j, k) → backend box [[z0, z1], [y0, y1], [x0, x1]]."""
    return [[lo[2], hi[2]], [lo[1], hi[1]], [lo[0], hi[0]]]


def from_zyx_box(b: Optional[ZyxBox]) -> Optional[dict]:
    if b is None:
        return None
    (z0, z1), (y0, y1), (x0, x1) = b
    return {"min": [int(x0), int(y0), int(z0)], "max": [int(x1), int(y1), int(z1)]}


def _ints(value, n: int, what: str) -> list:
    if not isinstance(value, list) or len(value) != n or not all(isinstance(v, int) and v >= 0 for v in value):
        raise bad_request(f"{what}: expected {n} non-negative integers")
    return value


def _check_box(lo: list, hi: list, dims: tuple) -> None:
    if any(a >= b for a, b in zip(lo, hi)) or any(b > d for b, d in zip(hi, dims)):
        raise bad_request(f"box {lo}..{hi} is empty or outside the volume {list(dims)}")


class Engine:
    """Protocol state machine around a backend (one lock: engines run one request at a time)."""

    def __init__(self, backend: Backend, session_ttl_s: int = 3600):
        self.backend = backend
        self.ttl = session_ttl_s
        self.sessions: dict = {}
        self.jobs: dict = {}  # job id -> session id
        self.lock = threading.Lock()

    # ------------------------------------------------------------------ helpers
    def info(self) -> dict:
        b = self.backend.info()
        return {
            "protocol": PROTOCOL,
            "name": b["name"],
            "version": b.get("version", ""),
            "vendor": b.get("vendor", ""),
            "device": b.get("device", ""),
            "capabilities": {
                "interactive": b.get("interactive", True),
                "automatic": b.get("automatic", False),
                "prompts": b.get("prompts", []),
                "planar_boxes_only": b.get("planar_boxes_only", False),
                "undo": b.get("undo", False),
            },
            "modalities": b.get("modalities", []),
            "labels": b.get("labels", []),
            "research_only": b.get("research_only", False),
            "license": b.get("license", ""),
            "limits": {"max_voxels": b.get("max_voxels", 0), "session_ttl_s": self.ttl},
        }

    def _expire(self) -> None:
        now = time.monotonic()
        for sid in [s for s, e in self.sessions.items() if now - e.last_used > self.ttl]:
            log.info("session %s expired", sid)
            self._close(sid)

    def _close(self, sid: str) -> None:
        self.jobs = {j: s for j, s in self.jobs.items() if s != sid}
        entry = self.sessions.pop(sid, None)
        if entry is not None and entry.backend is not None:
            entry.backend.close()

    def _get(self, sid: str, need_volume: bool = True) -> Session:
        self._expire()
        entry = self.sessions.get(sid)
        if entry is None:
            raise ProtocolError(404, "not_found", f"unknown or expired session {sid}")
        entry.last_used = time.monotonic()
        if need_volume and entry.backend is None:
            raise ProtocolError(409, "no_volume", "upload the volume first")
        return entry

    # ------------------------------------------------------------------ endpoints
    def create(self, body: dict) -> dict:
        dims = tuple(_ints(body.get("dims"), 3, "dims"))
        if 0 in dims:
            raise bad_request("dims must be non-zero")
        dtype = body.get("dtype")
        if dtype not in DTYPES:
            raise bad_request("dtype must be int16, uint16 or float32")
        spacing = body.get("spacing") or [1.0, 1.0, 1.0]
        max_voxels = self.backend.info().get("max_voxels", 0)
        voxels = dims[0] * dims[1] * dims[2]
        if max_voxels and voxels > max_voxels:
            raise ProtocolError(413, "too_large", f"{voxels} voxels > {max_voxels}")
        self._expire()
        # free the least recently used sessions beyond the backend's capacity
        while len(self.sessions) >= self.backend.max_sessions:
            oldest = min(self.sessions, key=lambda s: self.sessions[s].last_used)
            log.info("closing session %s to make room", oldest)
            self._close(oldest)
        origin = body.get("origin") or [0.0, 0.0, 0.0]
        direction = body.get("direction") or [[1, 0, 0], [0, 1, 0], [0, 0, 1]]
        if len(origin) != 3 or len(direction) != 3 or any(len(r) != 3 for r in direction):
            raise bad_request("origin must have 3 numbers and direction 3 rows of 3")
        sid = uuid.uuid4().hex
        self.sessions[sid] = Session(
            dims,
            dtype,
            tuple(spacing),
            str(body.get("modality") or ""),
            tuple(float(v) for v in origin),
            tuple(tuple(float(v) for v in r) for r in direction),
        )
        return {"session_id": sid, "expires_in_s": self.ttl}

    def upload(self, sid: str, data: bytes) -> None:
        entry = self._get(sid, need_volume=False)
        n = entry.dims[0] * entry.dims[1] * entry.dims[2]
        dt = np.dtype(DTYPES[entry.dtype])
        if len(data) != n * dt.itemsize:
            raise bad_request(f"expected {n * dt.itemsize} bytes of {entry.dtype}, got {len(data)}")
        volume = np.frombuffer(data, dtype=dt).reshape(entry.zyx_shape()).astype(np.float32)
        if entry.backend is not None:
            entry.backend.close()
        entry.backend = self.backend.open(
            volume, entry.spacing, entry.modality, origin=entry.origin, direction=entry.direction
        )
        entry.revision = 0

    def _result(self, entry: Session, changed: Optional[ZyxBox]) -> dict:
        if changed is not None:
            entry.revision += 1
        return {"revision": entry.revision, "changed": from_zyx_box(changed), "empty": entry.backend.is_empty()}

    def prompt(self, sid: str, body: dict) -> dict:
        entry = self._get(sid)
        kind = body.get("type")
        info = self.backend.info()
        if kind not in ("point", "box", "scribble", "lasso"):
            raise bad_request(f"unknown prompt type {kind!r}")
        if kind not in info.get("prompts", []):
            raise ProtocolError(422, "unsupported_prompt", f"{kind} prompts are not supported")
        positive = bool(body.get("positive", True))
        dims = entry.dims
        b = entry.backend
        if kind == "point":
            v = _ints(body.get("voxel"), 3, "voxel")
            if any(c >= d for c, d in zip(v, dims)):
                raise bad_request(f"point {v} is outside the volume {list(dims)}")
            return self._result(entry, b.point((v[2], v[1], v[0]), positive))
        lo, hi = _ints(body.get("min"), 3, "min"), _ints(body.get("max"), 3, "max")
        _check_box(lo, hi, dims)
        zyx = to_zyx_box(lo, hi)
        if kind == "box":
            if info.get("planar_boxes_only") and all(h - l > 1 for l, h in zip(lo, hi)):
                raise ProtocolError(422, "unsupported_prompt", "only planar boxes (one voxel thick) are supported")
            return self._result(entry, b.box(zyx, positive))
        try:
            raw = base64.b64decode(body.get("mask") or "", validate=True)
        except ValueError as e:
            raise bad_request(f"mask: {e}") from e
        shape = tuple(h - l for l, h in zyx)
        if len(raw) != shape[0] * shape[1] * shape[2]:
            raise bad_request(f"mask has {len(raw)} values, the box has {shape[0] * shape[1] * shape[2]} voxels")
        mask = (np.frombuffer(raw, dtype=np.uint8).reshape(shape) != 0).astype(np.uint8)
        fn = b.scribble if kind == "scribble" else b.lasso
        return self._result(entry, fn(mask, zyx, positive))

    def mask(self, sid: str, box: Optional[str]) -> tuple:
        entry = self._get(sid)
        if box is None:
            lo, hi = [0, 0, 0], list(entry.dims)
        else:
            try:
                n = [int(x) for x in box.split(",")]
            except ValueError as e:
                raise bad_request("box: expected 6 integers") from e
            if len(n) != 6 or any(x < 0 for x in n):
                raise bad_request("box: expected 6 non-negative integers")
            lo, hi = n[:3], n[3:]
        _check_box(lo, hi, entry.dims)
        data = entry.backend.mask(to_zyx_box(lo, hi))
        return np.ascontiguousarray(data, dtype=np.uint8).tobytes(), entry.revision

    def undo(self, sid: str) -> dict:
        entry = self._get(sid)
        if not self.backend.info().get("undo", False):
            raise ProtocolError(422, "unsupported_prompt", "undo is not supported")
        done, changed = entry.backend.undo()
        return self._result(entry, changed if done else None)

    # ---------------------------------------------------------------- automatic
    def _automatic(self) -> None:
        if not self.backend.info().get("automatic", False):
            raise ProtocolError(404, "not_found", "this engine has no automatic segmentation")

    def segment(self, sid: str, body: dict) -> dict:
        self._automatic()
        entry = self._get(sid)
        labels = body.get("labels")
        if labels is not None and (not isinstance(labels, list) or not all(isinstance(l, str) for l in labels)):
            raise bad_request("labels must be an array of strings or null")
        try:
            job = entry.backend.start_job(labels)
        except ValueError as e:
            raise bad_request(str(e)) from e
        except RuntimeError as e:
            raise ProtocolError(503, "busy", str(e), retry_after=5) from e
        self.jobs[job] = sid
        return {"job_id": job}

    def _job_session(self, job: str) -> Session:
        self._automatic()
        sid = self.jobs.get(job)
        if sid is None:
            raise ProtocolError(404, "not_found", f"unknown job {job}")
        return self._get(sid)

    def job_status(self, job: str) -> dict:
        s = self._job_session(job).backend.job_status(job)
        return {"state": s["state"], "progress": float(s.get("progress", 0.0)), "message": s.get("message", "")}

    def cancel_job(self, job: str) -> None:
        self._job_session(job).backend.cancel_job(job)

    def label_map(self, sid: str) -> bytes:
        self._automatic()
        entry = self._get(sid)
        try:
            values = entry.backend.label_map()
        except ValueError as e:
            raise ProtocolError(409, "no_result", str(e)) from e
        if tuple(values.shape) != entry.zyx_shape():
            raise ProtocolError(500, "internal", f"label map shape {values.shape} != volume {entry.zyx_shape()}")
        return np.ascontiguousarray(values, dtype="<u2").tobytes()

    def reset(self, sid: str) -> None:
        entry = self._get(sid)
        entry.backend.reset()
        entry.revision += 1

    def delete(self, sid: str) -> None:
        self._get(sid, need_volume=False)
        self._close(sid)


def create_app(backend: Backend, token: Optional[str] = None, session_ttl_s: int = 3600) -> FastAPI:
    """FastAPI application serving ``ferrum-engine/1`` for ``backend``."""
    app = FastAPI(title="FERRUM engine bridge", docs_url=None, redoc_url=None, openapi_url=None)
    engine = Engine(backend, session_ttl_s)
    app.state.engine = engine

    def error(e: ProtocolError) -> JSONResponse:
        headers = {"Retry-After": str(e.retry_after)} if e.retry_after else None
        return JSONResponse({"error": {"code": e.code, "message": e.message}}, status_code=e.status, headers=headers)

    @app.exception_handler(ProtocolError)
    async def protocol_error(_req: Request, e: ProtocolError):
        return error(e)

    @app.exception_handler(StarletteHTTPException)
    async def http_error(_req: Request, e: StarletteHTTPException):
        code = {404: "not_found", 405: "bad_request"}.get(e.status_code, "internal")
        return error(ProtocolError(e.status_code, code, str(e.detail)))

    @app.exception_handler(RequestValidationError)
    async def validation_error(_req: Request, e: RequestValidationError):
        return error(bad_request(str(e)))

    @app.exception_handler(Unsupported)
    async def unsupported(_req: Request, e: Unsupported):
        return error(ProtocolError(422, "unsupported_prompt", f"{e} not supported"))

    @app.exception_handler(Exception)
    async def internal_error(_req: Request, e: Exception):
        log.exception("engine failure")
        return error(ProtocolError(500, "internal", f"{type(e).__name__}: {e}"))

    @app.middleware("http")
    async def auth(request: Request, call_next):
        if token and request.headers.get("authorization") != f"Bearer {token}":
            return error(ProtocolError(401, "unauthorized", "missing or wrong token"))
        return await call_next(request)

    async def body_bytes(request: Request) -> bytes:
        data = await request.body()
        if request.headers.get("content-encoding", "").lower() == "gzip":
            try:
                data = gzip.decompress(data)
            except OSError as e:
                raise bad_request(f"invalid gzip body: {e}") from e
        return data

    async def body_json(request: Request) -> dict:
        try:
            value = json.loads(await body_bytes(request) or b"{}")
        except ValueError as e:
            raise bad_request(f"invalid JSON: {e}") from e
        if not isinstance(value, dict):
            raise bad_request("expected a JSON object")
        return value

    def locked(fn, *args):
        with engine.lock:
            return fn(*args)

    @app.get("/v1/info")
    def info():
        return locked(engine.info)

    @app.post("/v1/sessions", status_code=201)
    async def create(request: Request):
        body = await body_json(request)
        return JSONResponse(locked(engine.create, body), status_code=201)

    @app.put("/v1/sessions/{sid}/volume", status_code=204)
    async def upload(sid: str, request: Request):
        data = await body_bytes(request)
        locked(engine.upload, sid, data)
        return Response(status_code=204)

    @app.post("/v1/sessions/{sid}/prompts")
    async def prompt(sid: str, request: Request):
        body = await body_json(request)
        return locked(engine.prompt, sid, body)

    @app.get("/v1/sessions/{sid}/mask")
    def mask(sid: str, request: Request, box: Optional[str] = None):
        data, revision = locked(engine.mask, sid, box)
        headers = {"X-Ferrum-Revision": str(revision)}
        if len(data) > 1024 and "gzip" in request.headers.get("accept-encoding", ""):
            data = gzip.compress(data, compresslevel=1)
            headers["Content-Encoding"] = "gzip"
        return Response(data, media_type="application/octet-stream", headers=headers)

    @app.post("/v1/sessions/{sid}/undo")
    def undo(sid: str):
        return locked(engine.undo, sid)

    @app.post("/v1/sessions/{sid}/reset", status_code=204)
    def reset(sid: str):
        locked(engine.reset, sid)
        return Response(status_code=204)

    @app.post("/v1/sessions/{sid}/segment", status_code=202)
    async def segment(sid: str, request: Request):
        body = await body_json(request)
        return JSONResponse(locked(engine.segment, sid, body), status_code=202)

    @app.get("/v1/jobs/{job}")
    def job_status(job: str):
        return locked(engine.job_status, job)

    @app.delete("/v1/jobs/{job}", status_code=204)
    def cancel_job(job: str):
        locked(engine.cancel_job, job)
        return Response(status_code=204)

    @app.get("/v1/sessions/{sid}/labelmap")
    def label_map(sid: str, request: Request):
        data = locked(engine.label_map, sid)
        headers = {}
        if len(data) > 1024 and "gzip" in request.headers.get("accept-encoding", ""):
            data = gzip.compress(data, compresslevel=1)
            headers["Content-Encoding"] = "gzip"
        return Response(data, media_type="application/octet-stream", headers=headers)

    @app.delete("/v1/sessions/{sid}", status_code=204)
    def delete(sid: str):
        locked(engine.delete, sid)
        return Response(status_code=204)

    return app
