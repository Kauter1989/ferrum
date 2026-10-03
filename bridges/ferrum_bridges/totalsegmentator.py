"""TotalSegmentator backend (Wasserthal et al., https://github.com/wasserth/TotalSegmentator).

Written against ``totalsegmentator`` 2.18. Automatic only: a job runs one
TotalSegmentator *task* (default ``total``: 117 CT structures) on the
uploaded volume and returns a multilabel map whose values are the task's
class indices.

The volume arrives in FERRUM's canonical frame (LPS, ``[z, y, x]`` arrays)
with its patient geometry; the backend builds a NIfTI image with the
matching RAS affine, so TotalSegmentator sees the patient orientation and
spacing it was trained for, and maps the result back onto the grid.

Privacy: TotalSegmentator sends anonymous usage statistics by default.
The backend switches them off before the first run, so the bridge talks to
no server except for the one-time weight download.

Licences: TotalSegmentator's code is Apache-2.0. Some tasks need a licence
(free for non-commercial use); those are reported to FERRUM as
*Research use only*.

GPU memory: each job runs in a child process (``spawn``), so all of its
GPU memory returns to the driver when the job ends and another engine on
the same card (e.g. nnInteractive on a 12 GB GPU) can run next; cancelling
a job terminates the process at once. ``--in-process`` keeps the old
behaviour (faster start, the memory stays in PyTorch's cache).
"""

from __future__ import annotations

import multiprocessing
import os
import tempfile
import time
from typing import Optional

import numpy as np

from .backends import Backend, BackendSession, JobRunner

LPS_TO_RAS = np.diag([-1.0, -1.0, 1.0])


def affine_ras(spacing: tuple, origin: tuple, direction: list) -> np.ndarray:
    """4×4 voxel→RAS affine of a grid indexed ``(i, j, k)``.

    ``direction`` holds the LPS unit vectors of the ``i``, ``j`` and ``k``
    axes as rows (as sent in the protocol)."""
    d = np.asarray(direction, dtype=float).T  # columns = axis directions
    a = np.eye(4)
    a[:3, :3] = LPS_TO_RAS @ d @ np.diag(np.asarray(spacing, dtype=float))
    a[:3, 3] = LPS_TO_RAS @ np.asarray(origin, dtype=float)
    return a


class TotalSegmentatorSession(BackendSession):
    def __init__(self, backend: "TotalSegmentatorBackend", volume: np.ndarray, spacing, origin, direction):
        self.backend = backend
        self.volume = volume
        self.affine = affine_ras(spacing, origin, direction)
        self.runner = JobRunner()

    def start_job(self, labels):
        classes = self.backend.classes
        names = {v: k for k, v in classes.items()}
        unknown = [n for n in (labels or []) if n not in names]
        if unknown:
            raise ValueError(f"unknown structures for task {self.backend.task}: {unknown[:5]}")
        roi_subset = list(labels) if labels and self.backend.task in ("total", "total_mr") else None
        keep = {names[n] for n in labels} if labels else None

        def run(progress):
            import nibabel as nib

            progress(0.1, f"TotalSegmentator {self.backend.task}{' (fast)' if self.backend.fast else ''}")
            image = nib.Nifti1Image(self.volume.transpose(2, 1, 0).astype(np.float32), self.affine)
            seg = self.backend.run_job(image, roi_subset, progress)
            if seg is None:  # cancelled
                return None
            data = np.asanyarray(seg.dataobj).astype(np.uint16).transpose(2, 1, 0)
            if data.shape != self.volume.shape:
                raise RuntimeError(f"result shape {data.shape} differs from the volume {self.volume.shape}")
            if keep is not None:
                data[~np.isin(data, list(keep))] = 0
            return data

        return self.runner.start(run, f"queued: TotalSegmentator {self.backend.task}")

    def job_status(self, job):
        return self.runner.status(job)

    def cancel_job(self, job):
        self.runner.cancel(job)

    def label_map(self):
        if self.runner.result is None:
            raise ValueError("no finished job")
        return self.runner.result


class TotalSegmentatorBackend(Backend):
    """TotalSegmentator on one GPU (or CPU, slowly); one job at a time."""

    max_sessions = 2
    #: Jobs run in a child process (set by ``__init__``; stubs that skip it run in process).
    isolate = False

    def __init__(
        self,
        task: str = "total",
        device: str = "gpu",
        fast: bool = False,
        license_number: Optional[str] = None,
        isolate: bool = True,
    ):
        from totalsegmentator.config import set_config_key, setup_totalseg
        from totalsegmentator.registry import get_task_classes, requires_license, task_modality

        setup_totalseg()
        set_config_key("send_usage_stats", False)  # no telemetry from a medical bridge
        self.task = task
        self.device = device
        self.fast = fast
        self.license_number = license_number
        self.classes = {int(k): str(v) for k, v in get_task_classes(task).items()}
        self.licensed = requires_license(task)
        self.modality = task_modality(task)
        self.isolate = isolate

    def options(self, roi_subset) -> dict:
        """Keyword arguments of ``totalsegmentator()`` besides input and output."""
        return dict(
            ml=True,
            task=self.task,
            fast=self.fast,
            roi_subset=roi_subset,
            device=self.device,
            license_number=self.license_number,
            quiet=True,
            skip_saving=True,
        )

    def run(self, image, roi_subset):
        from totalsegmentator.python_api import totalsegmentator

        return totalsegmentator(image, None, **self.options(roi_subset))

    def run_job(self, image, roi_subset, progress):
        """Runs a job (in a child process unless ``isolate`` is off); ``None``
        if it was cancelled."""
        if not self.isolate:
            return self.run(image, roi_subset)
        return run_in_child(self.options(roi_subset), image, progress, f"TotalSegmentator {self.task}")

    def info(self) -> dict:
        try:
            from importlib.metadata import version

            pkg = version("TotalSegmentator")
        except Exception:  # pragma: no cover - metadata missing in odd installs
            pkg = "unknown"
        if self.licensed:
            licence = f"Task '{self.task}' needs a TotalSegmentator licence (free for non-commercial use)"
        else:
            licence = "TotalSegmentator: Apache-2.0"
        return {
            "name": "TotalSegmentator",
            "version": f"{pkg} ({self.task}{', fast' if self.fast else ''})",
            "vendor": "Wasserthal et al. (bridge: FERRUM)",
            "device": self.device,
            "interactive": False,
            "prompts": [],
            "automatic": True,
            "labels": [{"value": k, "name": v} for k, v in sorted(self.classes.items())],
            "modalities": [self.modality],
            "research_only": self.licensed,
            "license": licence,
            "max_voxels": int(os.environ.get("FERRUM_BRIDGE_MAX_VOXELS", "0")),
        }

    def open(self, volume, spacing, modality, origin=None, direction=None):
        origin = origin or (0.0, 0.0, 0.0)
        direction = direction or ((1, 0, 0), (0, 1, 0), (0, 0, 1))
        return TotalSegmentatorSession(self, volume, spacing, origin, direction)


def _child(options: dict, image_path: str, out_path: str) -> None:
    """Child process of one job: TotalSegmentator on a NIfTI file, the label
    array saved as ``.npy``."""
    import nibabel as nib
    from totalsegmentator.config import set_config_key
    from totalsegmentator.python_api import totalsegmentator

    set_config_key("send_usage_stats", False)
    seg = totalsegmentator(nib.load(image_path), None, **options)
    np.save(out_path, np.asanyarray(seg.dataobj).astype(np.uint16))


def run_in_child(options: dict, image, progress, what: str, poll_s: float = 0.25):
    """Runs :func:`_child` in a spawned process, reporting the elapsed time;
    terminates it when ``progress`` reports a cancellation and returns
    ``None`` then."""
    import nibabel as nib

    ctx = multiprocessing.get_context("spawn")
    with tempfile.TemporaryDirectory(prefix="ferrum-totalseg-") as tmp:
        src, out = os.path.join(tmp, "image.nii.gz"), os.path.join(tmp, "labels.npy")
        nib.save(image, src)
        proc = ctx.Process(target=_child, args=(options, src, out), name="totalsegmentator-job")
        proc.start()
        start = time.monotonic()
        while proc.exitcode is None:
            proc.join(poll_s)
            if proc.exitcode is None and not progress(0.1, f"{what}: running ({time.monotonic() - start:.0f} s)"):
                proc.terminate()
                proc.join(10)
                return None
        if proc.exitcode != 0 or not os.path.exists(out):
            raise RuntimeError(f"{what} failed in its process (exit code {proc.exitcode}); see the bridge log")
        data = np.load(out)
    return nib.Nifti1Image(data, image.affine)
