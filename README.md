# dicom_renderer

Fast desktop viewer for volumetric medical images — CT and MRI in DICOM or
NIfTI — written in Rust. It shows 2D slices, multiplanar reconstruction
(MPR) and GPU volume rendering, with interactive transfer functions,
clipping, measurements and a volume eraser.

![MPR layout of a chest CT: axial, coronal and sagittal slices in the lung window with a 3D transfer-function rendering](docs/images/mpr.png)

| | |
|---|---|
| ![Chest CT, transfer-function rendering of soft tissue and bone](docs/images/volume_soft_tissue.png) | ![Lungs and pulmonary vessels with the anterior chest wall clipped away](docs/images/volume_lung_vessels.png) |
| Transfer-function DVR, *CT soft tissue + bone* preset | *CT lung vessels* preset, anterior chest wall removed with the clip box |
| ![Rib cage as a shaded isosurface with ambient occlusion](docs/images/volume_bone.png) | ![Maximum intensity projection of the chest](docs/images/volume_mip.png) |
| Isosurface at 300 HU with ambient occlusion; scanner table clipped | Maximum intensity projection |

![Axial slice in the lung window with distance, angle, area and text annotations](docs/images/slice_measurements.png)

<sub>Screenshots show the real application rendering a public chest CT — see [Sample data](#sample-data).</sub>

---

## Contents

- [Features](#features)
- [Quick start](#quick-start)
- [Using the viewer](#using-the-viewer)
- [Rendering](#rendering)
- [Performance](#performance)
- [Architecture](#architecture)
- [Development](#development)
- [Testing](#testing)
- [Project layout](#project-layout)
- [Sample data](#sample-data)
- [License](#license)

## Features

**Data**
- DICOM Part 10: explicit/implicit VR, little/big endian, JPEG, JPEG 2000,
  RLE and deflate transfer syntaxes, multi-frame files, rescale
  slope/intercept, signed pixels, MONOCHROME1, colour images.
- Scans folders recursively and groups slices into series. Slices are
  sorted by their position along the plane normal, with fallbacks to
  Instance Number and Slice Location.
- NIfTI-1 (`.nii`, `.nii.gz`) reading and export; `sform`/`qform` orientation is
  honoured and every volume is brought into the DICOM patient frame (LPS),
  so NIfTI and DICOM data display identically.
- Parallel loading with progress reporting and cancellation. You pick the
  series when a folder contains more than one.

**2D and MPR**
- Axial, coronal and sagittal slices rendered on the GPU straight from the
  3D texture.
- Window/level by mouse drag, numeric input, presets (brain, soft tissue,
  lung, bone) or automatic percentiles.
- Pan, zoom around the cursor, and nearest or linear sampling.
- MPR layout: three slice views plus the 3D view. Crosshairs are
  colour-coded by plane; right-click jumps all views to a point.
- Voxel probe that reads the physical value (e.g. Hounsfield units).

**Measurements** — distance, angle, free-form area, rectangle and text
notes, all in millimetres (anisotropic voxels are handled). You can move,
delete and clear them.

**3D rendering**
- Four techniques:
  - *Tissue*: a translucent band followed by a shaded surface.
  - *Isosurface*.
  - *Maximum intensity projection*.
  - *Transfer-function* direct volume rendering.
- Opacity, brightness and quality (sampling rate) controls.
- Optional ambient occlusion.
- Trackball rotation, pan, zoom, reset, and one-click anatomical views
  (anterior, posterior, left, right, superior, inferior).

**Transfer function editor** — control points drawn over a log-scaled
intensity histogram. Drag points, double-click to add, right-click to
remove, and pick a colour per point. Generic presets plus CT presets
defined in Hounsfield units (*soft tissue + bone*, *lung vessels*, *bone*)
that adapt to the data range of the loaded scan.

**Clipping**
- View-aligned cut ("virtual knife").
- Axis-aligned clip box.
- Oblique plane with adjustable azimuth, elevation and offset.
- Optional overlay of the raw slice on the cut surface.

**Volume eraser** — a cylindrical brush with adjustable radius and depth,
undo and full restore.

**Processing** — Gaussian smoothing and Sobel edge filters.

**Output** — PNG screenshots, NIfTI export, and a viewer for the series'
DICOM attributes.

## Quick start

Requirements:
- A recent stable Rust toolchain (pinned by `rust-toolchain.toml`).
- A GPU with Vulkan, Metal, DirectX 12 or OpenGL support.
- On Linux: `libxkbcommon-x11` and a Vulkan/GL driver.

```bash
git clone https://github.com/Kauter1989/dicom_renderer
cd dicom_renderer
cargo run --release -p mri-viewer -- /path/to/dicom-folder
# or
make run ARGS=/path/to/dicom-folder
```

You can pass folders, DICOM files or NIfTI files on the command line, drop
them onto the window, or use **Open folder** / **Open files**.

## Using the viewer

| Action | 2D / MPR views | 3D view |
|---|---|---|
| Left drag | active tool (pan, window/level, measure…) | rotate (or erase in eraser mode) |
| Right / middle drag | — | pan |
| Mouse wheel | next/previous slice | zoom |
| Ctrl + wheel | zoom around cursor | — |
| Double click | reset pan/zoom (pan tool) | reset camera |
| Right click | MPR: move crosshair to this point | — |

Keyboard: **F2 / F3 / F4** switch between 2D, 3D and MPR.
**↑ ↓ / PgUp PgDn** change the slice. **Ctrl+O** opens a folder.
**Ctrl+Z** undoes the last erase.

## Rendering

All rendering runs on the GPU through [wgpu](https://wgpu.rs), which means
Vulkan, Metal, DX12 or GL depending on the platform. The volume is uploaded
once as a 16-bit 3D texture (`R16Unorm`, with an `R16Float` fallback).
Volumes larger than the device limit are downsampled automatically.

The ray-casting algorithms are a WGSL port of the author's own WebGL shaders
from an earlier web viewer:
- tissue band plus isosurface compositing;
- bisection refinement of isosurface hits;
- gradient-based Phong shading;
- MIP;
- transfer-function integration.

The port reworks the pipeline for performance:

1. **Single pass.** One full-screen triangle per frame. Each pixel builds
   its ray from the inverse view-projection matrix and intersects it
   analytically with the volume box and up to eight clipping half-spaces.
   There are no back-face or front-face render targets.
2. **Specialised pipelines.** The render technique is a pipeline-overridable
   constant, so each mode compiles to a branch-free shader.
3. **Empty-space skipping.** A min/max brick grid (8³ voxels per brick) is
   classified against the current mode and transfer function, and the ray
   jumps over empty bricks. Samples stay snapped to the same grid, so the
   image is identical with and without skipping.
4. **Dynamic resolution.** The 3D view renders at reduced resolution while
   you rotate it and at full resolution when you stop.
5. **Precomputed ambient occlusion.** Volumetric obscurance is computed once
   per threshold with separable box filters, instead of tracing occlusion
   rays per pixel.
6. **Correct anisotropy.** Normals and step sizes account for non-cubic
   voxels.
7. **Jittered ray starts.** Each pixel's starting offset is jittered to
   remove wood-grain artefacts.

The eraser mask is uploaded incrementally: only the region a stroke changed
is sent to the GPU.

A **CPU reference ray caster** implements exactly the same image-formation
model. It is the test oracle for GPU output, the picker for interactive
tools, and a software fallback.

## Performance

Measured on a 4-core machine using the *software* rasteriser llvmpipe
(a real GPU is one to two orders of magnitude faster at rendering):

| Operation | Result |
|---|---|
| Load a 256×256×128 DICOM series | ≈ 50 ms |
| Scan DICOM headers (128 files) | ≈ 17 ms |
| Histogram of a 256³ volume | 1.5 Gvoxel/s |
| Min/max brick grid, 256³ | 2 Gvoxel/s |
| Ambient occlusion, 256³ | ≈ 48 ms |
| Isosurface frame, 256², empty-space skipping off → on | 23.8 → 4.6 ms |

Reproduce these numbers with `cargo bench --workspace`.

## Architecture

Clean architecture with one crate per layer. Dependencies point inward:
the data layer implements ports defined by the domain, and the application
layer compiles without any GPU or UI library.

```mermaid
flowchart LR
    V["mri-viewer<br/>egui UI"] --> A["mri-app<br/>state · use cases · tools · jobs"]
    V --> R["mri-render<br/>WGSL / wgpu · CPU reference"]
    V --> IO["mri-io<br/>DICOM · NIfTI"]
    A --> D["mri-domain<br/>entities · rules · ports"]
    A --> P["mri-processing<br/>parallel algorithms"]
    A -->|"frame model, picking"| R
    R --> D
    P --> D
    IO -->|"implements VolumeRepository"| D
```

| Crate | Responsibility |
|---|---|
| `mri-domain` | Volume, window/level, transfer function, render settings, clipping, camera, slice geometry, annotations, eraser mask. Also the `VolumeRepository` port. No I/O. |
| `mri-processing` | Histogram, brick grid, ambient occlusion, filters and resampling, parallelised with rayon |
| `mri-io` | DICOM and NIfTI repositories (dicom-rs): scanning, series grouping, slice ordering, parallel decoding |
| `mri-render` | Frame model shared by GPU and CPU, WGSL shaders, the wgpu renderer (feature `gpu`) and the CPU ray caster |
| `mri-app` | The `Viewer` facade: loading jobs, slice and 3D state, 2D tool state machines, eraser with undo, and GPU synchronisation through the `GpuSink` port |
| `mri-viewer` | eframe/egui application: panels, widgets, paint callbacks, dialogs |

Details: [docs/architecture.md](docs/architecture.md) ·
decisions: [docs/decisions/](docs/decisions/).

## Development

```bash
make test        # all tests; GPU tests are skipped without an adapter
make test-gpu    # same, but a missing GPU adapter fails the run (CI mode)
make lint        # rustfmt --check + clippy -D warnings
make bench       # criterion benchmarks
make snapshot ARGS="<input> <out_dir>"   # headless PNG renders of every mode
make showcase ARGS="<input> <out_dir>"   # regenerate the README screenshots
```

Conventions:
- Every public item has a doc comment.
- No `unwrap`/`expect` outside tests.
- Changes to `volume.wgsl` must be mirrored in the CPU ray caster.
- Image data is never committed.

See [CLAUDE.md](CLAUDE.md) for the full rules.

To run GPU tests without a graphics card, install Mesa's software Vulkan
driver (`mesa-vulkan-drivers` on Debian/Ubuntu).

## Testing

About 180 tests run headlessly with `cargo test --workspace`:

| Level | What is checked |
|---|---|
| Unit | domain maths, windowing, transfer functions, clipping, camera, measurements, processing algorithms |
| Property-based | invariants over random inputs, e.g. that empty-space skipping can never hide visible material |
| Data layer | DICOM files generated at test time: transfer syntaxes, rescale, signed data, MONOCHROME1, multi-frame, several series, corrupt files; NIfTI round-trip |
| Shaders | WGSL validated with naga, and uniform layouts matched to the Rust structs |
| GPU parity | every render mode and feature, GPU image compared to the CPU reference |
| Application | use cases with an in-memory repository and a recording GPU sink |
| UI | the real app driven with egui_kittest, plus full-window renders of 2D, 3D and MPR |

To also test on a real DICOM series:
`MRI_SAMPLE_DICOM=/path/to/series cargo test -p mri-io`.
More in [docs/testing.md](docs/testing.md).

## Project layout

```
.
├── crates/
│   ├── mri-domain/       # domain model and ports
│   ├── mri-processing/   # parallel volume algorithms
│   ├── mri-io/           # DICOM / NIfTI repositories
│   ├── mri-render/       # shaders, GPU renderer, CPU reference
│   ├── mri-app/          # application layer
│   └── mri-viewer/       # desktop application (binary: mri-viewer)
├── docs/                 # architecture, testing, ADRs
├── .github/workflows/    # CI: fmt, clippy, tests on lavapipe, release build
└── Makefile
```

## Sample data

The screenshots are rendered from case `lung_053` of the **Medical
Segmentation Decathlon** lung task (*Task06_Lung*), a public, de-identified
chest CT collection originating from The Cancer Imaging Archive, licensed
under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/):

> M. Antonelli, A. Reinke, S. Bakas et al. *The Medical Segmentation
> Decathlon.* Nature Communications 13, 4128 (2022).

The screenshots in `docs/images/` are derivative works of that data and are
distributed under the same CC BY-SA 4.0 licence. No image data is stored in
this repository. To reproduce them, download `Task06_Lung.tar` from the
Decathlon, extract `imagesTr/lung_053.nii.gz`, and run:

```bash
make showcase ARGS="path/to/lung_053.nii.gz docs/images"
```

## License

Source code: MIT — see [LICENSE](LICENSE).
Screenshots in `docs/images/`: CC BY-SA 4.0 (see [Sample data](#sample-data)).
