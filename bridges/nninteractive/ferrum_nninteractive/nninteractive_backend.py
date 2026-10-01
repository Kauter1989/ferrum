"""nnInteractive backend (MIC-DKFZ, https://github.com/MIC-DKFZ/nnInteractive).

Written against ``nninteractive`` 2.6. One inference session and one copy
of the network are kept on the GPU; the bridge serves one FERRUM session at
a time (a new session replaces the previous one), which is what an
interactive single-user demo needs.

The nnInteractive code is Apache-2.0; the model weights are distributed
separately under their own licence (CC BY-NC-SA 4.0 for the official
checkpoints), reported to FERRUM, which then marks results *Research use
only*.
"""

from __future__ import annotations

import os
from typing import Optional

import numpy as np

from .backends import Backend, BackendSession, ZyxBox, slicer


def _research_only(license_text: str) -> bool:
    text = (license_text or "").upper()
    return "NC" in text.replace("-", " ").split() or "NON-COMMERCIAL" in text or "!!MISSING!!" in text


class NnInteractiveSession(BackendSession):
    def __init__(self, session, target):
        self.s = session
        self.target = target

    @staticmethod
    def _box(b) -> Optional[ZyxBox]:
        return None if b is None else [[int(lo), int(hi)] for lo, hi in b]

    def point(self, zyx, positive):
        return self._box(self.s.add_point_interaction(tuple(int(c) for c in zyx), include_interaction=positive))

    def box(self, bbox, positive):
        return self._box(self.s.add_bbox_interaction(bbox, include_interaction=positive))

    def scribble(self, mask, bbox, positive):
        return self._box(self.s.add_scribble_interaction(mask, include_interaction=positive, interaction_bbox=bbox))

    def lasso(self, mask, bbox, positive):
        return self._box(self.s.add_lasso_interaction(mask, include_interaction=positive, interaction_bbox=bbox))

    def undo(self):
        if not self.s.undo():
            return False, None
        # nnInteractive records the region restored by undo for its own remote client
        changed = getattr(self.s, "_last_paste_bbox", None)
        if changed is None:
            changed = [[0, n] for n in self.target.shape]
        return True, self._box(changed)

    def reset(self):
        self.s.reset_interactions()

    def mask(self, bbox):
        region = self.target[slicer(bbox)]
        return region.cpu().numpy() if hasattr(region, "cpu") else np.asarray(region)

    def is_empty(self):
        return not bool(self.target.any())

    def close(self):
        self.s.reset_interactions()


class NnInteractiveBackend(Backend):
    """nnInteractive on one GPU (or CPU, very slowly)."""

    max_sessions = 1

    def __init__(
        self,
        device: str = "cuda:0",
        model_id: Optional[str] = None,
        model_dir: Optional[str] = None,
        torch_compile: bool = False,
    ):
        import torch
        from nnInteractive.inference.inference_session import nnInteractiveInferenceSession

        if model_dir is None:
            from nnInteractive.model_management import ensure_model_available, get_default_model_id

            model_id = model_id or get_default_model_id()
            model_dir = str(ensure_model_available(model_id))
        self.model = model_id or os.path.basename(os.path.normpath(model_dir))
        self.torch = torch
        self.device = torch.device(device)
        self.session = nnInteractiveInferenceSession(
            device=self.device,
            use_torch_compile=torch_compile,
            verbose=False,
            torch_n_threads=os.cpu_count() or 4,
            do_autozoom=True,
            enable_undo=True,
        )
        self.session.initialize_from_trained_model_folder(model_dir)
        self.license = self.session.license or ""
        supported = getattr(self.session, "supported_interactions", {}) or {}
        self.prompts = [
            name
            for name, keys in (
                ("point", ["points"]),
                ("box", ["bbox2d", "bbox3d"]),
                ("scribble", ["scribble"]),
                ("lasso", ["lasso"]),
            )
            if any(supported.get(k, False) for k in keys)
        ]
        self.planar_only = not supported.get("bbox3d", False)

    def info(self) -> dict:
        if self.device.type == "cuda":
            device = f"{self.device} {self.torch.cuda.get_device_name(self.device)}"
        else:
            device = str(self.device)
        try:
            from importlib.metadata import version

            pkg = version("nninteractive")
        except Exception:  # pragma: no cover - metadata missing in odd installs
            pkg = "unknown"
        return {
            "name": "nnInteractive",
            "version": f"{pkg} ({self.model})",
            "vendor": "MIC-DKFZ (bridge: FERRUM)",
            "device": device,
            "prompts": self.prompts,
            "planar_boxes_only": self.planar_only,
            "undo": bool(getattr(self.session, "supports_undo", False)),
            "modalities": [],
            "research_only": _research_only(self.license),
            "license": f"Model weights: {self.license}" if self.license else "Model weights: licence unknown",
            "max_voxels": 0,
        }

    def open(self, volume, spacing, modality):
        # nnInteractive works in voxel space; spacing is accepted but unused
        self.session.set_image(volume[None], {"spacing": list(spacing)[::-1]})  # [z, y, x]
        target = self.torch.zeros(volume.shape, dtype=self.torch.uint8)
        self.session.set_target_buffer(target)
        return NnInteractiveSession(self.session, target)
