#!/usr/bin/env python3
"""Render a known phantom through ferrum-cli and check the overlay geometry.

The phantom is a 64 x 48 x 9 CT-like volume with one rectangular segment
(20 x 12 voxels, off-centre, not square). Its slices are rendered with every
segment style at 1:1 and zoomed, and the outline is checked:
- every side of the rectangle has outline pixels (lesson L1: outlines drawn
  on two sides only shipped for two stages);
- opposite sides have the same number of outline pixels;
- the fill covers about the rectangle's area.

The PNGs stay in the output directory: open them (Read tool) and look.

Usage (repo root, after `cargo build -p ferrum-cli`):
    python3 .claude/skills/ferrum-visual-check/phantom_render.py [--cli target/debug/ferrum-cli] [--out DIR]
Exit code 0 when every check passes. Needs Pillow for the checks; without it,
only the PNGs are written.
"""

import argparse
import gzip
import json
import os
import struct
import subprocess
import sys
import tempfile

DIMS = (64, 48, 9)            # i, j, k
RECT = (14, 10, 34, 22)       # i0, j0, i1, j1 (exclusive): 20 x 12 voxels
SLICE = 5                     # 1-based axial slice through the rectangle
SIZES = (64, 512)             # 1:1 and 8x


def write_nifti(path: str) -> None:
    """Background 0 HU, rectangle 100 HU on slices 3..7, 1 mm voxels, LPS."""
    nx, ny, nz = DIMS
    h = bytearray(348)
    struct.pack_into("<i", h, 0, 348)
    struct.pack_into("<8h", h, 40, 3, nx, ny, nz, 1, 1, 1, 1)
    struct.pack_into("<h", h, 70, 4)          # int16
    struct.pack_into("<h", h, 72, 16)
    struct.pack_into("<8f", h, 76, 1, 1, 1, 1, 1, 1, 1, 1)
    struct.pack_into("<f", h, 108, 352)
    struct.pack_into("<f", h, 112, 1.0)
    struct.pack_into("<h", h, 254, 1)         # sform
    for row, r in enumerate(([-1, 0, 0, 0], [0, -1, 0, 0], [0, 0, 1, 0])):
        struct.pack_into("<4f", h, 280 + 16 * row, *r)
    h[344:348] = b"n+1\0"
    vals = []
    for k in range(nz):
        for j in range(ny):
            for i in range(nx):
                inside = RECT[0] <= i < RECT[2] and RECT[1] <= j < RECT[3] and 2 <= k <= 6
                vals.append(100 if inside else 0)
    data = bytes(h) + b"\0" * 4 + struct.pack(f"<{len(vals)}h", *vals)
    with open(path, "wb") as f:
        f.write(gzip.compress(data))


def cli(binary: str, config: str, *args: str) -> dict:
    env = dict(os.environ, FERRUM_AGENT_CONFIG=config)
    out = subprocess.run([binary, *args], capture_output=True, text=True, env=env)
    if not out.stdout.strip():
        sys.exit(f"ferrum-cli {' '.join(args)} printed no JSON:\n{out.stderr}")
    env_ = json.loads(out.stdout)
    if not env_.get("ok"):
        sys.exit(f"ferrum-cli {' '.join(args)} failed: {json.dumps(env_.get('error'))}")
    return env_["data"]


def check_outline(img, color, name: str) -> list:
    """Outline pixels on all four sides of their bounding box, symmetric counts."""
    w, h = img.size
    px = img.load()
    hits = [(x, y) for y in range(h) for x in range(w) if px[x, y][:3] == color]
    if not hits:
        return [f"{name}: no outline pixels in colour {color}"]
    x0, x1 = min(x for x, _ in hits), max(x for x, _ in hits)
    y0, y1 = min(y for _, y in hits), max(y for _, y in hits)
    sides = {
        "left": sum(1 for x, _ in hits if x == x0),
        "right": sum(1 for x, _ in hits if x == x1),
        "top": sum(1 for _, y in hits if y == y0),
        "bottom": sum(1 for _, y in hits if y == y1),
    }
    errors = []
    # each side's outer line must run (almost) the full length of the box
    for side, n in sides.items():
        full = (y1 - y0 + 1) if side in ("left", "right") else (x1 - x0 + 1)
        if n < 0.9 * full:
            errors.append(f"{name}: {side} side has {n} of {full} pixels — outline not closed")
    if sides["left"] != sides["right"] or sides["top"] != sides["bottom"]:
        errors.append(f"{name}: asymmetric outline {sides}")
    # the interior must not be painted in outline style
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    if px[cx, cy][:3] == color:
        errors.append(f"{name}: centre pixel painted in outline style")
    print(f"  {name}: box x {x0}..{x1}, y {y0}..{y1}, sides {sides}")
    return errors


def check_fill(img, grey_bg, name: str, scale: float) -> list:
    """Pixels that differ from the background cover about the rectangle's area."""
    w, h = img.size
    px = img.load()
    changed = sum(1 for y in range(h) for x in range(w) if px[x, y][:3] != grey_bg)
    area = (RECT[2] - RECT[0]) * (RECT[3] - RECT[1]) * scale * scale
    print(f"  {name}: {changed} changed pixels, rectangle area {area:.0f}")
    if not 0.8 * area <= changed <= 1.25 * area:
        return [f"{name}: {changed} changed pixels, expected about {area:.0f}"]
    return []


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("--cli", default="target/debug/ferrum-cli")
    p.add_argument("--out", default=None, help="output directory (default: a temp dir)")
    a = p.parse_args()
    out = a.out or tempfile.mkdtemp(prefix="ferrum-visual-")
    data, ws = os.path.join(out, "data"), os.path.join(out, "ws")
    os.makedirs(data, exist_ok=True)
    os.makedirs(ws, exist_ok=True)
    config = os.path.join(out, "agent.toml")
    with open(config, "w") as f:
        f.write(f'[data]\nread_roots = ["{data}"]\nworkspace_root = "{ws}"\n')
    nii = os.path.join(data, "phantom.nii.gz")
    write_nifti(nii)

    w = os.path.join(ws, "p")
    cli(a.cli, config, "study", "open", "-w", w, nii, "--modality", "CT")
    seed = f"v:{(RECT[0] + RECT[2]) // 2},{(RECT[1] + RECT[3]) // 2},{SLICE - 1}"
    seg = cli(a.cli, config, "segment", "threshold", "-w", w, "--seed", seed,
              "--min", "50", "--max", "150", "--name", "rect")["segment"]
    color = tuple(seg["color"][:3])
    print(f"segment {seg['label']} colour {color}, PNGs in {out}")

    try:
        from PIL import Image
    except ImportError:
        Image = None
        print("Pillow missing: PNGs written, geometry not checked (pip install pillow)")

    errors, renders = [], []
    for size in SIZES:
        scale = size / DIMS[0]
        base = cli(a.cli, config, "view", "slice", "-w", w, "--plane", "axial",
                   "--slice-number", str(SLICE), "--window", "0,400", "--size", str(size))["image"]
        for style in ("outline", "fill", "fill_outline"):
            r = cli(a.cli, config, "view", "slice", "-w", w, "--plane", "axial",
                    "--slice-number", str(SLICE), "--window", "0,400", "--size", str(size),
                    "--overlay", "segments", "--segment-style", style, "--segment-opacity", "0.4")
            name = f"{style}@{size}px"
            renders.append((name, r["image"]))
            if Image is None:
                continue
            img = Image.open(r["image"]).convert("RGB")
            if style != "fill":
                errors += check_outline(img, color, name)
            if style == "fill":
                bg = Image.open(base).convert("RGB").load()[0, 0][:3]
                errors += check_fill(img, bg, name, scale)

    print("\nrenders to look at:")
    for name, path in renders:
        print(f"  {name}: {path}")
    if errors:
        print("\nFAILED:")
        for e in errors:
            print(f"  {e}")
        return 1
    print("\nall overlay checks passed" if Image else "\nnothing checked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
