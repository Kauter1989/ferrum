"""MONAI Label backend (Project MONAI, https://github.com/Project-MONAI/MONAILabel).

Written against the REST API of MONAI Label 0.8. The bridge is a client of
a running MONAI Label server (any app: radiology, monaibundle, …); it
serves that server's models to FERRUM:

- an **interactive** model (``deepedit``, ``deepgrow`` or ``annotation``
  types, e.g. DeepEdit, DeepGrow, SAM2) answers include/exclude clicks;
  ``annotation`` models (SAM2) also take a box;
- a **segmentation** model runs as an automatic job and returns a label
  map with the model's labels.

Requests: the volume is written as a NIfTI image with the patient geometry
(as for TotalSegmentator) and uploaded once as a MONAI Label *session*
(``PUT /session/``). Each click re-runs ``POST /infer/{model}`` on the
session with the whole click history in index space ``[x, y, z]`` of that
image; MONAI Label models are stateless between requests, so undo replays
the history without the last prompt.
"""

from __future__ import annotations

import gzip
import json
import os
from typing import List, Optional

import numpy as np

from .backends import Backend, BackendSession, JobRunner, Unsupported, diff_box, slicer
from .totalsegmentator import affine_ras

#: MONAI Label infer types that answer clicks.
INTERACTIVE_TYPES = ("deepedit", "deepgrow", "annotation")


def label_values(labels) -> dict:
    """MONAI Label ``labels`` (dict name→value or list of names) → name→value,
    without the background."""
    if isinstance(labels, dict):
        pairs = {str(k): int(v) for k, v in labels.items()}
    else:
        pairs = {str(name): i + 1 for i, name in enumerate(labels or [])}
    return {k: v for k, v in pairs.items() if v != 0 and k.lower() != "background"}


def nifti_bytes(volume: np.ndarray, affine: np.ndarray) -> bytes:
    """``[z, y, x]`` volume → gzip NIfTI bytes with data ordered ``x, y, z``."""
    import nibabel as nib

    img = nib.Nifti1Image(volume.transpose(2, 1, 0).astype(np.float32), affine)
    return gzip.compress(img.to_bytes(), compresslevel=1)


def read_label_image(data: bytes, shape: tuple) -> np.ndarray:
    """NIfTI (plain or gzip) result → ``uint16`` labels ``[z, y, x]`` of ``shape``."""
    import nibabel as nib

    if data[:2] == b"\x1f\x8b":
        data = gzip.decompress(data)
    img = nib.Nifti1Image.from_bytes(data)
    arr = np.asanyarray(img.dataobj)
    arr = arr.reshape(arr.shape[:3]) if arr.ndim > 3 and all(s == 1 for s in arr.shape[3:]) else arr
    labels = np.rint(arr).astype(np.uint16).transpose(2, 1, 0)
    if labels.shape != shape:
        raise RuntimeError(f"MONAI Label returned a {labels.shape} result for a {shape} volume")
    return labels


class MonaiLabelSession(BackendSession):
    def __init__(self, backend: "MonaiLabelBackend", volume: np.ndarray, affine: np.ndarray):
        self.backend = backend
        self.shape = volume.shape
        self.target = np.zeros(volume.shape, dtype=np.uint8)
        self.history: List[tuple] = []  # ("point", [x, y, z], positive) | ("box", roi)
        self.runner = JobRunner()
        files = {"files": ("volume.nii.gz", nifti_bytes(volume, affine), "application/gzip")}
        r = backend.request("PUT", "/session/", files=files)
        self.session_id = r.json()["session_id"]

    # ------------------------------------------------------------ interactive
    def _params(self, history) -> dict:
        b = self.backend
        fg = [p for kind, p, *pos in history if kind == "point" and pos[0]]
        bg = [p for kind, p, *pos in history if kind == "point" and not pos[0]]
        params = {"foreground": fg, "background": bg}
        if b.model_type == "deepedit" and b.label:
            params.update({b.label: fg, "label": b.label})
        rois = [p for kind, p, *_ in history if kind == "box"]
        if rois:
            params["roi"] = rois[-1]
        return params

    def _infer(self, history) -> np.ndarray:
        if not history:
            return np.zeros(self.shape, dtype=np.uint8)
        b = self.backend
        labels = b.infer(b.model, self.session_id, self._params(history), self.shape)
        value = b.model_labels(b.model).get(b.label) if b.label else None
        return (labels == value if value is not None and b.model_type == "deepedit" else labels > 0).astype(np.uint8)

    def _apply(self, history) -> Optional[list]:
        new = self._infer(history)
        self.history = history
        changed = diff_box(self.target, new)
        self.target = new
        return changed

    def point(self, zyx, positive):
        z, y, x = (int(c) for c in zyx)
        return self._apply(self.history + [("point", [x, y, z], bool(positive))])

    def box(self, bbox, positive):
        if self.backend.model_type != "annotation":
            raise Unsupported("box prompts")
        if not positive:
            raise Unsupported("exclude boxes")
        (z0, z1), (y0, y1), (x0, x1) = bbox
        return self._apply(self.history + [("box", [x0, y0, x1, y1, z0, z1])])

    def undo(self):
        if not self.history:
            return False, None
        return True, self._apply(self.history[:-1])

    def reset(self):
        self.history = []
        self.target = np.zeros(self.shape, dtype=np.uint8)

    def mask(self, bbox):
        return self.target[slicer(bbox)]

    def is_empty(self):
        return not self.target.any()

    # ------------------------------------------------------------ automatic
    def start_job(self, labels):
        b = self.backend
        names = b.model_labels(b.auto_model)
        unknown = [n for n in (labels or []) if n not in names]
        if unknown:
            raise ValueError(f"unknown labels for model {b.auto_model}: {unknown[:5]}")
        keep = [names[n] for n in labels] if labels else None

        def run(progress):
            progress(0.1, f"MONAI Label {b.auto_model}")
            out = b.infer(b.auto_model, self.session_id, {}, self.shape)
            if keep is not None:
                out[~np.isin(out, keep)] = 0
            return out

        return self.runner.start(run, f"queued: MONAI Label {b.auto_model}")

    def job_status(self, job):
        return self.runner.status(job)

    def cancel_job(self, job):
        self.runner.cancel(job)

    def label_map(self):
        if self.runner.result is None:
            raise ValueError("no finished job")
        return self.runner.result

    def close(self):
        try:
            self.backend.request("DELETE", f"/session/{self.session_id}")
        except Exception:  # the server may have expired it already
            pass


class MonaiLabelBackend(Backend):
    """A client of one MONAI Label server.

    ``model`` (interactive) and ``auto_model`` (automatic) default to the
    first model of a matching type in the server's ``/info/``; pass ``""``
    to switch a mode off. ``client`` is an ``httpx.Client`` (injected in
    tests)."""

    max_sessions = 4

    def __init__(self, server: str = "http://127.0.0.1:8000", model: Optional[str] = None,
                 auto_model: Optional[str] = None, label: Optional[str] = None,
                 token: Optional[str] = None, research_only: bool = True, timeout: float = 600.0, client=None):
        if client is None:
            import httpx

            headers = {"Authorization": f"Bearer {token}"} if token else {}
            client = httpx.Client(base_url=server.rstrip("/"), headers=headers, timeout=timeout)
        self.server = server
        self.client = client
        self.research_only = research_only
        self.server_info = self.request("GET", "/info/").json()
        models = self.server_info.get("models", {})
        self.model = self._pick(model, INTERACTIVE_TYPES)
        self.auto_model = self._pick(auto_model, ("segmentation",))
        if not self.model and not self.auto_model:
            raise RuntimeError(f"no interactive or segmentation model on {server}: {sorted(models)}")
        self.model_type = models[self.model].get("type", "") if self.model else ""
        if self.model_type == "deepedit" and not label:
            label = next(iter(self.model_labels(self.model)), None)
        self.label = label

    def _pick(self, name: Optional[str], types: tuple) -> Optional[str]:
        models = self.server_info.get("models", {})
        if name == "":
            return None
        if name is not None:
            if name not in models:
                raise RuntimeError(f"model {name!r} is not on the server: {sorted(models)}")
            return name
        return next((k for k, m in models.items() if m.get("type") in types), None)

    def request(self, method: str, path: str, **kwargs):
        r = self.client.request(method, path, **kwargs)
        if r.status_code >= 400:
            raise RuntimeError(f"MONAI Label {method} {path}: HTTP {r.status_code} {r.text[:200]}")
        return r

    def model_labels(self, model: Optional[str]) -> dict:
        if not model:
            return {}
        return label_values(self.server_info["models"][model].get("labels", []))

    def infer(self, model: str, session_id: str, params: dict, shape: tuple) -> np.ndarray:
        r = self.request("POST", f"/infer/{model}", params={"session_id": session_id, "output": "image"},
                         data={"params": json.dumps(params)})
        return read_label_image(r.content, shape)

    def info(self) -> dict:
        auto = self.model_labels(self.auto_model)
        prompts = ["point", "box"] if self.model_type == "annotation" else ["point"] if self.model else []
        used = ", ".join(m for m in (self.model, self.auto_model) if m)
        return {
            "name": f"MONAI Label ({self.server_info.get('name', 'app')})",
            "version": f"{self.server_info.get('version', '')} [{used}]".strip(),
            "vendor": "Project MONAI (bridge: FERRUM)",
            "device": self.server,
            "interactive": bool(self.model),
            "prompts": prompts,
            "planar_boxes_only": False,
            "undo": bool(self.model),
            # inference runs on the MONAI Label server, whose GPU kernels the bridge cannot pin down
            "deterministic": False,
            "automatic": bool(self.auto_model),
            "labels": [{"value": v, "name": k} for k, v in sorted(auto.items(), key=lambda kv: kv[1])],
            "modalities": [],
            "research_only": self.research_only,
            "license": "MONAI Label: Apache-2.0; each model's weights carry their own licence (see the app)",
            "max_voxels": int(os.environ.get("FERRUM_BRIDGE_MAX_VOXELS", "0")),
        }

    def open(self, volume, spacing, modality, origin=None, direction=None):
        origin = origin or (0.0, 0.0, 0.0)
        direction = direction or ((1, 0, 0), (0, 1, 0), (0, 0, 1))
        return MonaiLabelSession(self, volume, affine_ras(spacing, origin, direction))
