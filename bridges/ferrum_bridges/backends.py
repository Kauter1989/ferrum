"""Backend interface of the bridges and a model-free fake for tests.

A backend works on NumPy arrays in C order ``[z, y, x]`` and boxes
``[[z0, z1], [y0, y1], [x0, x1]]`` (half-open). Interactions return the box
of the voxels that changed in the target mask, or ``None``. Automatic
backends also run jobs that produce a ``uint16`` label map.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import List, Optional, Tuple

import threading
import uuid

import numpy as np

ZyxBox = List[List[int]]


class Unsupported(Exception):
    """The backend does not offer this operation."""


def slicer(b: ZyxBox) -> tuple:
    return tuple(slice(lo, hi) for lo, hi in b)


def diff_box(before: np.ndarray, after: np.ndarray) -> Optional[ZyxBox]:
    """Box around the voxels that differ, or ``None``."""
    changed = np.argwhere(before != after)
    if changed.size == 0:
        return None
    lo, hi = changed.min(axis=0), changed.max(axis=0) + 1
    return [[int(a), int(b)] for a, b in zip(lo, hi)]


class BackendSession:
    """One volume; interactive backends keep one target mask, automatic
    backends run jobs. Every operation is optional: the default raises
    :class:`Unsupported`, which the protocol maps to the right status."""

    def point(self, zyx: Tuple[int, int, int], positive: bool) -> Optional[ZyxBox]:
        raise Unsupported("point prompts")

    def box(self, bbox: ZyxBox, positive: bool) -> Optional[ZyxBox]:
        raise Unsupported("box prompts")

    def scribble(self, mask: np.ndarray, bbox: ZyxBox, positive: bool) -> Optional[ZyxBox]:
        raise Unsupported("scribble prompts")

    def lasso(self, mask: np.ndarray, bbox: ZyxBox, positive: bool) -> Optional[ZyxBox]:
        raise Unsupported("lasso prompts")

    def undo(self) -> Tuple[bool, Optional[ZyxBox]]:
        raise Unsupported("undo")

    def reset(self) -> None:
        """Clears the target mask (no-op for automatic-only backends)."""

    def mask(self, bbox: ZyxBox) -> np.ndarray:
        raise Unsupported("masks")

    def is_empty(self) -> bool:
        return True

    def start_job(self, labels: Optional[List[str]]) -> str:
        """Starts automatic segmentation of ``labels`` (``None`` = all)."""
        raise Unsupported("automatic segmentation")

    def job_status(self, job: str) -> dict:
        """``{"state", "progress", "message"}``; raises ``KeyError`` if unknown."""
        raise Unsupported("automatic segmentation")

    def cancel_job(self, job: str) -> None:
        raise Unsupported("automatic segmentation")

    def label_map(self) -> np.ndarray:
        """``uint16`` labels ``[z, y, x]`` of the last finished job."""
        raise Unsupported("automatic segmentation")

    def close(self) -> None:
        """Releases resources (called when the protocol session ends)."""


class JobRunner:
    """Runs one function at a time in a thread and tracks job states.

    Cancellation is cooperative: a cancelled job is reported ``cancelled``
    at once and its result is discarded when it finishes."""

    def __init__(self):
        self.jobs: dict = {}
        self.result: Optional[np.ndarray] = None
        self.lock = threading.Lock()

    def start(self, fn, message: str) -> str:
        job = uuid.uuid4().hex[:16]
        with self.lock:
            if any(j["state"] in ("queued", "running") for j in self.jobs.values()):
                raise RuntimeError("a job is already running")
            self.jobs[job] = {"state": "running", "progress": 0.05, "message": message}

        def run():
            try:
                result = fn(lambda p, m=None: self._progress(job, p, m))
                with self.lock:
                    if self.jobs[job]["state"] == "running":
                        self.result = result
                        self.jobs[job].update(state="done", progress=1.0)
            except Exception as e:  # reported to the client as a failed job
                with self.lock:
                    self.jobs[job].update(state="failed", message=f"{type(e).__name__}: {e}")

        threading.Thread(target=run, name=f"job-{job}", daemon=True).start()
        return job

    def _progress(self, job: str, progress: float, message: Optional[str]) -> bool:
        """Records progress; returns ``False`` once the job was cancelled, so
        long work can stop early."""
        with self.lock:
            if self.jobs[job]["state"] != "running":
                return False
            self.jobs[job]["progress"] = max(0.0, min(1.0, progress))
            if message:
                self.jobs[job]["message"] = message
            return True

    def status(self, job: str) -> dict:
        with self.lock:
            return dict(self.jobs[job])

    def cancel(self, job: str) -> None:
        with self.lock:
            entry = self.jobs[job]
            if entry["state"] in ("queued", "running"):
                entry.update(state="cancelled")

    def wait(self, job: str, timeout: float = 30.0) -> dict:
        """Blocks until the job leaves the running state (tests)."""
        import time

        end = time.monotonic() + timeout
        while self.status(job)["state"] in ("queued", "running") and time.monotonic() < end:
            time.sleep(0.01)
        return self.status(job)


class Backend(ABC):
    """An engine able to open interactive sessions."""

    #: Sessions held at once; older ones are closed to make room.
    max_sessions: int = 8

    @abstractmethod
    def info(self) -> dict:
        """``name``, ``version``, ``vendor``, ``device``, ``interactive``,
        ``prompts`` (list of ``point``/``box``/``scribble``/``lasso``),
        ``planar_boxes_only``, ``undo``, ``automatic``, ``labels`` (list of
        ``{value, name, color?}``), ``modalities``, ``research_only``,
        ``license``, ``max_voxels``."""

    @abstractmethod
    def open(self, volume: np.ndarray, spacing: tuple, modality: str, origin=None, direction=None) -> BackendSession:
        """Starts a session on ``volume`` (float32, ``[z, y, x]``). ``spacing``,
        ``origin`` and ``direction`` (rows = LPS unit vectors of ``i, j, k``)
        describe the grid in patient space, as sent by FERRUM."""


# --------------------------------------------------------------------------- fake


def grow(volume: np.ndarray, seed: Tuple[int, int, int], tolerance: float, limit: Optional[ZyxBox] = None) -> np.ndarray:
    """6-connected region of voxels within ``tolerance`` of the seed value."""
    region = np.zeros(volume.shape, dtype=bool)
    allowed = np.abs(volume - volume[seed]) <= tolerance
    if limit is not None:
        inside = np.zeros(volume.shape, dtype=bool)
        inside[slicer(limit)] = True
        allowed &= inside
    if not allowed[seed]:
        return region
    region[seed] = True
    while True:
        grown = region.copy()
        for axis in range(3):
            for step in (1, -1):
                shifted = np.roll(region, step, axis=axis)
                edge = [slice(None)] * 3
                edge[axis] = slice(0, 1) if step == 1 else slice(-1, None)
                shifted[tuple(edge)] = False
                grown |= shifted
        grown &= allowed
        if np.array_equal(grown, region):
            return region
        region = grown


#: Labels of the fake backend's automatic mode (intensity bands).
FAKE_LABELS = [
    {"value": 1, "name": "intermediate", "color": [80, 175, 95]},
    {"value": 2, "name": "bright"},
]


class FakeSession(BackendSession):
    def __init__(self, volume: np.ndarray, tolerance: float):
        self.volume = volume
        self.tolerance = tolerance
        self.target = np.zeros(volume.shape, dtype=np.uint8)
        self.previous: Optional[np.ndarray] = None
        self.runner = JobRunner()

    def start_job(self, labels):
        names = {l["name"]: l["value"] for l in FAKE_LABELS}
        unknown = [n for n in (labels or []) if n not in names]
        if unknown:
            raise ValueError(f"unknown labels: {unknown}")
        wanted = [names[n] for n in labels] if labels else list(names.values())

        def classify(progress):
            lo, hi = float(self.volume.min()), float(self.volume.max())
            third = (hi - lo) / 3.0 or 1.0
            out = np.zeros(self.volume.shape, dtype=np.uint16)
            out[self.volume > lo + third] = 1
            out[self.volume > lo + 2 * third] = 2
            progress(0.5, "classifying")
            out[~np.isin(out, wanted)] = 0
            return out

        return self.runner.start(classify, "fake job")

    def job_status(self, job):
        return self.runner.status(job)

    def cancel_job(self, job):
        self.runner.cancel(job)

    def label_map(self):
        if self.runner.result is None:
            raise ValueError("no finished job")
        return self.runner.result

    def _apply(self, new: np.ndarray) -> Optional[ZyxBox]:
        self.previous = self.target
        self.target = new
        return diff_box(self.previous, new)

    def point(self, zyx, positive):
        new = self.target.copy()
        if positive:
            new[grow(self.volume, zyx, self.tolerance)] = 1
        elif new[zyx]:
            new[grow(new.astype(np.float32), zyx, 0.0)] = 0
        return self._apply(new)

    def box(self, bbox, positive):
        new = self.target.copy()
        if positive:
            centre = tuple((lo + hi - 1) // 2 for lo, hi in bbox)
            new[grow(self.volume, centre, self.tolerance, bbox)] = 1
        else:
            new[slicer(bbox)] = 0
        return self._apply(new)

    def _mask_prompt(self, mask, bbox, positive):
        new = self.target.copy()
        region = new[slicer(bbox)]
        region[mask != 0] = 1 if positive else 0
        return self._apply(new)

    def scribble(self, mask, bbox, positive):
        return self._mask_prompt(mask, bbox, positive)

    def lasso(self, mask, bbox, positive):
        return self._mask_prompt(mask, bbox, positive)

    def undo(self):
        if self.previous is None:
            return False, None
        changed = diff_box(self.target, self.previous)
        self.target, self.previous = self.previous, None
        return True, changed

    def reset(self):
        self.target = np.zeros_like(self.target)
        self.previous = None

    def mask(self, bbox):
        return self.target[slicer(bbox)]

    def is_empty(self):
        return not self.target.any()


class FakeBackend(Backend):
    """Region growing without a model: exercises the protocol in tests and CI."""

    def __init__(self, tolerance_fraction: float = 0.08):
        self.tolerance_fraction = tolerance_fraction

    def info(self) -> dict:
        return {
            "name": "FERRUM fake engine (bridge test backend)",
            "version": "1",
            "vendor": "FERRUM",
            "device": "cpu",
            "prompts": ["point", "box", "scribble", "lasso"],
            "planar_boxes_only": False,
            "undo": True,
            "interactive": True,
            "automatic": True,
            "labels": FAKE_LABELS,
            "research_only": False,
            "license": "MIT — region growing, no model",
            "max_voxels": 50_000_000,
        }

    def open(self, volume, spacing, modality, origin=None, direction=None):
        span = float(volume.max() - volume.min()) or 1.0
        return FakeSession(volume, self.tolerance_fraction * span)
