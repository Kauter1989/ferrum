---
name: ferrum-visual-check
description: Verify any FERRUM change that affects pixels — slice or volume rendering, segment overlays, outlines, fills, colours, windowing, zoom, agent renders (view slice/montage/mpr/volume) or desktop shaders. Use before committing such a change and whenever a render "looks off". Runs a phantom with known geometry through ferrum-cli, checks closed outlines and fill area at 1:1 and zoomed, and requires looking at the PNGs. Born from the two-sided outline bug (lesson L1).
---

# Visual check for rendering changes

**Why this exists.** Segment outlines in agent renders were drawn on the
right and bottom edges only, and this shipped for two stages (lesson L1
in `.claude/lessons.md`). The tests only checked that *some* outline
pixels existed, and nobody looked at a real render. Pixel output needs
exact geometry and a person's (or your) eyes.

## 1. Run the phantom check

```bash
cargo build -q -p ferrum-cli
python3 .claude/skills/ferrum-visual-check/phantom_render.py --out <scratch>/visual
```

- **The phantom:** a 20 × 12 voxel rectangle, off-centre and not square.
- **The renders:** `view slice` in `outline`, `fill` and `fill_outline`,
  at 1:1 (64 px) and 8× (512 px).
- **The checks:**
  - every side of the outline is closed (≥ 90 % of the side's length);
  - opposite sides have equal counts;
  - the interior is not painted in outline style;
  - the fill covers the rectangle's area (±20 %).
- **Proof that it can fail:** with the old two-neighbour `on_edge`, it
  reports `left side has 1 of 12 pixels — outline not closed`.

Exit code 0 means every check passed. On failure, fix the code, not the
thresholds.

## 2. Look at the renders yourself

Open at least the `outline@64px` and `fill_outline@512px` PNGs with the
Read tool. Look for:
- **Closed rings:** gaps, doubled lines, an outline that is missing on
  one side.
- **Thickness vs zoom:** 1 px at 1:1, 2–3 px at 8×. A line one pixel wide
  vanishes when zoomed.
- **Contrast:** the colour must be visible on both dark and bright
  tissue. Try a bright window, `--window 0,50`, as well.
- **Orientation:**
  - axial images have the patient's left on screen right (radiological
    view; the render's `orientation` field reads `R` left, `L` right);
  - the rectangle sits off-centre, so a flip shows.

Say in your report which images you looked at. Do not claim a visual
result you did not see.

## 3. Keep CPU and GPU paths in step

Each image has a GPU and a CPU path, and they must agree:

| Path | Code | Parity test |
|---|---|---|
| Desktop slice view (GPU) | `crates/ferrum-render/src/shaders/slice.wgsl` | `crates/ferrum-render/tests/gpu_parity.rs` (`slice_overlay_fills_and_outlines_segments`) |
| Agent renders (CPU) | `crates/ferrum-agent/src/commands/view.rs` (`render_pixels`, `on_edge`) and `SegmentStyle::blend` in `ferrum-domain` | `crates/ferrum-agent/tests/segmentation.rs` (`slice_renders_draw_closed_outlines_or_translucent_fills`) |
| Volume rendering | `shaders/volume.wgsl` ↔ `cpu/raycast.rs` (CLAUDE.md rendering rule) | `gpu_parity.rs` |

- A change on one side is mirrored on the other in the same commit.
- Run the parity tests with `FERRUM_REQUIRE_GPU=1` (lavapipe). Without
  it they skip silently, and a skipped test proves nothing.

## 4. Turn every visual bug into an exact test

When a render bug is found (by you, a reviewer or a field test):
1. Reproduce it on the smallest phantom that shows it.
2. Add a test with **exact** numbers, for example "20 outline pixels at
   1:1" or "the 48² − 42² ring at 8×". Do not add a test that only says
   "outline pixels > 0".
3. Fix the code. The test fails before the fix and passes after it.
4. Record the bug with skill `ferrum-lessons`. Extend
   `phantom_render.py` if the bug class is not covered yet: a new style,
   plane, zoom or window.

## 5. Real data

The phantom proves geometry, not appearance on anatomy. For overlay,
colour or window changes, ask for renders of a real study in the next
field test (skill `ferrum-field-test`). Request at least one axial slice
through organs in each style, at the size an agent uses (768 px).
