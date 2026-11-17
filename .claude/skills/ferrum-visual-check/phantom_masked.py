#!/usr/bin/env python3
"""Render a phantom with burned-in text through ferrum-cli and check the mask.

Clean Pixel Data (Stage 31, criterion 31.3-c). The phantom is a 64 x 48 x 6
CT-like DICOM series with a bright rectangular "text" block (20 x 12 voxels,
off-centre) on slices 3..5 and BurnedInAnnotation = YES. Its slices are
rendered at 1:1 and 8x, unmasked and masked, and checked pixel by pixel:
- the masked region holds the solid fill, the unmasked render shows the text;
- every pixel outside the region is identical in both renders;
- with no --mask the flag masks the whole image (the conservative default);
- an unflagged NIfTI phantom reports "nothing to mask", never silently passes.

The PNGs stay in the output directory: open them (Read tool) and look.

Usage (repo root, after `cargo build -p ferrum-cli`):
    python3 .claude/skills/ferrum-visual-check/phantom_masked.py [--cli target/debug/ferrum-cli] [--out DIR]
Exit code 0 when every check passes. Needs Pillow for the checks; without it,
only the PNGs are written. The DICOM phantom is generated at run time with a
minimal explicit-VR writer (no pydicom, no committed data).
"""

import argparse
import json
import os
import struct
import subprocess
import sys
import tempfile
import time

DIMS = (64, 48, 6)  # i, j, k
RECT = (14, 10, 34, 22)  # i0, j0, i1, j1 (exclusive): the burned-in "text"
TEXT_K = (2, 3, 4)  # 0-based slices carrying the text
SLICE = 4  # 1-based axial slice through the text
SIZES = (64, 512)  # 1:1 and 8x
WINDOW = "500,1000"  # bounds [0, 1000]: fill 0 -> black, text 1000 -> white
TEXT_VALUE = 1000

CT_STORAGE = "1.2.840.10008.5.1.4.1.1.2"
EXPLICIT_LE = "1.2.840.10008.1.2.1"
SERIES_UID = "1.2.826.0.1.3680043.8.498.555"
STUDY_UID = "1.2.826.0.1.3680043.8.498.555.0"


def background(i: int, j: int) -> int:
    """A non-uniform dark background, so a black fill is visible."""
    return (i + 2 * j) % 256


def stored(i: int, j: int, k: int) -> int:
    inside = RECT[0] <= i < RECT[2] and RECT[1] <= j < RECT[3] and k in TEXT_K
    return TEXT_VALUE if inside else background(i, j)


def _pad(b: bytes) -> bytes:
    return b + b"\0" if len(b) % 2 else b


def _elem(tag: tuple, vr: str, value: bytes) -> bytes:
    head = struct.pack("<HH", *tag) + vr.encode("ascii")
    if vr in ("OB", "OW", "OF", "SQ", "UN", "UT"):
        return head + b"\0\0" + struct.pack("<I", len(value)) + value
    return head + struct.pack("<H", len(value)) + value


def _str(tag: tuple, vr: str, s: str) -> bytes:
    return _elem(tag, vr, _pad(s.encode("ascii")))


def _us(tag: tuple, v: int) -> bytes:
    return _elem(tag, "US", struct.pack("<H", v))


def write_dicom_slice(path: str, k: int, flag: str | None) -> None:
    """One axial 1 mm slice, stored = physical (slope 1, intercept 0)."""
    nx, ny, _ = DIMS
    pixels = b"".join(struct.pack("<H", stored(i, j, k)) for j in range(ny) for i in range(nx))
    sop = f"{SERIES_UID}.{k + 1}"
    meta = [
        _str((0x0002, 0x0002), "UI", CT_STORAGE),
        _str((0x0002, 0x0003), "UI", sop),
        _str((0x0002, 0x0010), "UI", EXPLICIT_LE),
        _str((0x0002, 0x0012), "UI", "1.2.826.0.1.3680043.10.543.1"),
        _str((0x0002, 0x0013), "SH", "FERRUM_VIS"),
    ]
    meta_bytes = b"".join(meta)
    ds = [
        _str((0x0008, 0x0016), "UI", CT_STORAGE),
        _str((0x0008, 0x0018), "UI", sop),
        _str((0x0008, 0x0060), "CS", "CT"),
        _str((0x0020, 0x000D), "UI", STUDY_UID),
        _str((0x0020, 0x000E), "UI", SERIES_UID),
        _str((0x0020, 0x0013), "IS", str(k + 1)),
        _str((0x0020, 0x0032), "DS", f"0\\0\\{k}"),
        _str((0x0020, 0x0037), "DS", "1\\0\\0\\0\\1\\0"),
        _str((0x0028, 0x0030), "DS", "1\\1"),
        _str((0x0018, 0x0050), "DS", "1"),
        _us((0x0028, 0x0002), 1),
        _str((0x0028, 0x0004), "CS", "MONOCHROME2"),
        _us((0x0028, 0x0010), ny),
        _us((0x0028, 0x0011), nx),
        _us((0x0028, 0x0100), 16),
        _us((0x0028, 0x0101), 16),
        _us((0x0028, 0x0102), 15),
        _us((0x0028, 0x0103), 0),
        _str((0x0028, 0x1052), "DS", "0"),
        _str((0x0028, 0x1053), "DS", "1"),
    ]
    if flag is not None:
        ds.append(_str((0x0028, 0x0301), "CS", flag))
    ds.append(_elem((0x7FE0, 0x0010), "OW", pixels))
    group_len = _elem((0x0002, 0x0000), "UL", struct.pack("<I", len(meta_bytes)))
    with open(path, "wb") as f:
        f.write(b"\0" * 128 + b"DICM" + group_len + meta_bytes + b"".join(ds))


def write_nifti(path: str) -> None:
    """The same phantom as NIfTI (no BurnedInAnnotation flag exists there)."""
    nx, ny, nz = DIMS
    h = bytearray(348)
    struct.pack_into("<i", h, 0, 348)
    struct.pack_into("<8h", h, 40, 3, nx, ny, nz, 1, 1, 1, 1)
    struct.pack_into("<h", h, 70, 4)  # int16
    struct.pack_into("<h", h, 72, 16)
    struct.pack_into("<8f", h, 76, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0)
    struct.pack_into("<f", h, 108, 352)
    struct.pack_into("<f", h, 112, 1.0)
    struct.pack_into("<h", h, 254, 1)  # sform
    for row, r in enumerate(([-1, 0, 0, 0], [0, -1, 0, 0], [0, 0, 1, 0])):
        struct.pack_into("<4f", h, 280 + 16 * row, *r)
    h[344:348] = b"n+1\0"
    vals = [stored(i, j, k) for k in range(nz) for j in range(ny) for i in range(nx)]
    data = bytes(h) + b"\0" * 4 + struct.pack(f"<{len(vals)}h", *vals)
    import gzip

    with open(path, "wb") as f:
        f.write(gzip.compress(data))


def cli(binary: str, config: str, *args: str) -> dict:
    env = dict(os.environ, FERRUM_AGENT_CONFIG=config)
    out = subprocess.run([binary, *args], capture_output=True, text=True, env=env)
    if not out.stdout.strip():
        sys.exit(f"ferrum-cli {' '.join(args)} printed no JSON:\n{out.stderr}")
    envelope = json.loads(out.stdout)
    if not envelope.get("ok"):
        sys.exit(f"ferrum-cli {' '.join(args)} failed: {json.dumps(envelope.get('error'))}")
    return envelope["data"]


def check_mask(base, masked, scale: float, name: str) -> list:
    """The region holds the fill, every other pixel is unchanged."""
    if base.size != masked.size:
        return [f"{name}: size mismatch {base.size} vs {masked.size}"]
    bpx, mpx = base.load(), masked.load()
    w, h = base.size
    x0, y0 = int(RECT[0] * scale), int(RECT[1] * scale)
    x1, y1 = int(RECT[2] * scale), int(RECT[3] * scale)
    errors = []
    region = outside = 0
    for y in range(h):
        for x in range(w):
            inside = x0 <= x < x1 and y0 <= y < y1
            if inside:
                region += 1
                if mpx[x, y][:3] != (0, 0, 0):
                    errors.append(f"{name}: masked pixel ({x}, {y}) is {mpx[x, y][:3]}, expected the fill (0,0,0)")
                if bpx[x, y][:3] != (255, 255, 255):
                    errors.append(f"{name}: unmasked ({x}, {y}) is {bpx[x, y][:3]}, expected text (255,255,255)")
            else:
                outside += 1
                if mpx[x, y] != bpx[x, y]:
                    errors.append(f"{name}: pixel ({x}, {y}) outside the region changed: {bpx[x, y]} -> {mpx[x, y]}")
            if len(errors) > 8:
                return errors
    print(f"  {name}: {region} region pixels filled, {outside} pixels unchanged")
    return errors


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--cli", default="target/debug/ferrum-cli")
    p.add_argument("--out", default=None, help="output directory (default: a temp dir)")
    a = p.parse_args()
    out = a.out or tempfile.mkdtemp(prefix="ferrum-masked-")
    data, ws = os.path.join(out, "data"), os.path.join(out, "ws")
    os.makedirs(data, exist_ok=True)
    os.makedirs(ws, exist_ok=True)
    config = os.path.join(out, "agent.toml")
    with open(config, "w") as f:
        f.write(f'[data]\nread_roots = ["{data}"]\nworkspace_root = "{ws}"\n')
    dicom_dir = os.path.join(data, "burned")
    os.makedirs(dicom_dir, exist_ok=True)
    for k in range(DIMS[2]):
        write_dicom_slice(os.path.join(dicom_dir, f"img_{k:03}.dcm"), k, "YES")
    nii = os.path.join(data, "phantom.nii.gz")
    write_nifti(nii)

    w = os.path.join(ws, "burned")
    cli(a.cli, config, "study", "open", "-w", w, dicom_dir)
    wn = os.path.join(ws, "nifti")
    cli(a.cli, config, "study", "open", "-w", wn, nii, "--modality", "CT")
    print(f"PNGs in {out}")

    try:
        from PIL import Image
    except ImportError:
        Image = None
        print("Pillow missing: PNGs written, pixels not checked (pip install pillow)")

    errors, renders = [], []
    mask_args = ["--mask", f"v:{RECT[0]},{RECT[1]},{SLICE - 1}", f"v:{RECT[2] - 1},{RECT[3] - 1},{SLICE - 1}"]
    for size in SIZES:
        scale = size / DIMS[0]
        common = ["-w", w, "--plane", "axial", "--slice-number", str(SLICE), "--window", WINDOW, "--size", str(size)]
        base = cli(a.cli, config, "view", "slice", *common)
        masked = cli(a.cli, config, "view", "slice", *common, "--masked", *mask_args)
        name = f"masked@{size}px"
        renders += [(f"base@{size}px", base["image"]), (name, masked["image"])]
        if masked.get("mask", {}).get("applied") is not True or masked["mask"]["source"] != "regions":
            errors.append(f"{name}: sidecar mask block wrong: {masked.get('mask')}")
        if Image is not None:
            errors += check_mask(
                Image.open(base["image"]).convert("RGB"), Image.open(masked["image"]).convert("RGB"), scale, name
            )

    # the flag's conservative default: no region given -> the whole image
    whole = cli(
        a.cli,
        config,
        "view",
        "slice",
        "-w",
        w,
        "--plane",
        "axial",
        "--slice-number",
        str(SLICE),
        "--window",
        WINDOW,
        "--size",
        "64",
        "--masked",
    )
    renders.append(("flag-default@64px", whole["image"]))
    if whole.get("mask", {}).get("source") != "burned_in" or whole["mask"]["applied"] is not True:
        errors.append(f"flag default: sidecar mask block wrong: {whole.get('mask')}")
    if Image is not None:
        px = Image.open(whole["image"]).convert("RGB").load()
        if any(px[x, y] != (0, 0, 0) for y in range(48) for x in range(64)):
            errors.append("flag default: a pixel does not hold the fill")

    # an unflagged study: nothing to mask, reported (never silently passed)
    common_n = ["-w", wn, "--plane", "axial", "--slice-number", str(SLICE), "--window", WINDOW, "--size", "64"]
    base_n = cli(a.cli, config, "view", "slice", *common_n)
    masked_n = cli(a.cli, config, "view", "slice", *common_n, "--masked")
    renders.append(("unflagged-masked@64px", masked_n["image"]))
    if masked_n.get("mask", {}).get("applied") is not False or masked_n["mask"]["source"] != "none":
        errors.append(f"unflagged: sidecar mask block wrong: {masked_n.get('mask')}")
    if Image is not None and Image.open(base_n["image"]).tobytes() != Image.open(masked_n["image"]).tobytes():
        errors.append("unflagged: the image changed although nothing was masked")

    print("\nrenders to look at:")
    for name, path in renders:
        print(f"  {name}: {path}")
    if errors:
        print("\nFAILED:")
        for e in errors:
            print(f"  {e}")
        return 1
    if Image is None:
        print("\nnothing checked")
        return 0
    record_pass()
    print("\nall masked-render checks passed (marker written for the pre-push hook)")
    return 0


def record_pass() -> None:
    """Write the marker the pre-push hook (E2) compares with the render sources."""
    here = os.path.dirname(os.path.abspath(__file__))
    hooks = os.path.join(here, "..", "..", "hooks", "ferrum_hooks.py")
    root = os.path.abspath(os.path.join(here, "..", "..", ".."))
    if not os.path.exists(hooks):
        return
    import importlib.util

    spec = importlib.util.spec_from_file_location("ferrum_hooks", hooks)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    with open(os.path.join(mod.state_dir(root), "visual-check.json"), "w", encoding="utf-8") as f:
        json.dump({"hash": mod.visual_hash(root), "time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}, f)


if __name__ == "__main__":
    # covers 31.3-c
    sys.exit(main())
