"""Backend interface of the bridge and a model-free fake for tests.

A backend works on NumPy arrays in C order ``[z, y, x]`` and boxes
``[[z0, z1], [y0, y1], [x0, x1]]`` (half-open). Interactions return the box
of the voxels that changed in the target mask, or ``None``.
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import List, Optional, Tuple

import numpy as np

ZyxBox = List[List[int]]


def slicer(b: ZyxBox) -> tuple:
    return tuple(slice(lo, hi) for lo, hi in b)


def diff_box(before: np.ndarray, after: np.ndarray) -> Optional[ZyxBox]:
    """Box around the voxels that differ, or ``None``."""
    changed = np.argwhere(before != after)
    if changed.size == 0:
        return None
    lo, hi = changed.min(axis=0), changed.max(axis=0) + 1
    return [[int(a), int(b)] for a, b in zip(lo, hi)]


class BackendSession(ABC):
    """One volume and one target mask."""

    @abstractmethod
    def point(self, zyx: Tuple[int, int, int], positive: bool) -> Optional[ZyxBox]: ...

    @abstractmethod
    def box(self, bbox: ZyxBox, positive: bool) -> Optional[ZyxBox]: ...

    @abstractmethod
    def scribble(self, mask: np.ndarray, bbox: ZyxBox, positive: bool) -> Optional[ZyxBox]: ...

    @abstractmethod
    def lasso(self, mask: np.ndarray, bbox: ZyxBox, positive: bool) -> Optional[ZyxBox]: ...

    @abstractmethod
    def undo(self) -> Tuple[bool, Optional[ZyxBox]]: ...

    @abstractmethod
    def reset(self) -> None: ...

    @abstractmethod
    def mask(self, bbox: ZyxBox) -> np.ndarray: ...

    @abstractmethod
    def is_empty(self) -> bool: ...

    def close(self) -> None:
        """Releases resources (called when the protocol session ends)."""


class Backend(ABC):
    """An engine able to open interactive sessions."""

    #: Sessions held at once; older ones are closed to make room.
    max_sessions: int = 8

    @abstractmethod
    def info(self) -> dict:
        """``name``, ``version``, ``vendor``, ``device``, ``prompts`` (list of
        ``point``/``box``/``scribble``/``lasso``), ``planar_boxes_only``,
        ``undo``, ``modalities``, ``research_only``, ``license``,
        ``max_voxels``."""

    @abstractmethod
    def open(self, volume: np.ndarray, spacing: tuple, modality: str) -> BackendSession:
        """Starts a session on ``volume`` (float32, ``[z, y, x]``)."""


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


class FakeSession(BackendSession):
    def __init__(self, volume: np.ndarray, tolerance: float):
        self.volume = volume
        self.tolerance = tolerance
        self.target = np.zeros(volume.shape, dtype=np.uint8)
        self.previous: Optional[np.ndarray] = None

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
            "research_only": False,
            "license": "MIT — region growing, no model",
            "max_voxels": 50_000_000,
        }

    def open(self, volume, spacing, modality):
        span = float(volume.max() - volume.min()) or 1.0
        return FakeSession(volume, self.tolerance_fraction * span)
