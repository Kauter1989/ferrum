# dicom_renderer — MRI Viewer in Rust

High-performance desktop viewer for volumetric medical images (DICOM,
NIfTI): 2D slices, MPR and GPU volume rendering with transfer function
editing, clipping, measurements and a volume eraser. Rewrite of the
non-segmentation functionality of the
[EPAM MRI Viewer](https://github.com/epam/mriviewer) web application.

![MPR view](docs/images/mpr.png)

## Run

```bash
cargo run --release -p mri-viewer -- /path/to/dicom-folder
# or: make run ARGS=/path/to/dicom-folder
```

Drop folders/files onto the window or use **Open folder** (Ctrl+O).
Requires a GPU with Vulkan, Metal, DX12 or GL support.

## Features

| Area | Features |
|---|---|
| Data | DICOM Part 10 (all common transfer syntaxes incl. JPEG, JPEG 2000, RLE, deflate; multi-frame; rescale; MONOCHROME1), series grouping and spatial ordering, NIfTI-1 read/write, parallel loading with progress & cancel |
| 2D | axial/coronal/sagittal slices sampled on the GPU, window/level (drag, presets, auto), pan/zoom, nearest/linear, voxel probe |
| MPR | 2×2 layout, colour-coded crosshairs, right-click navigation |
| Measurements | distance, angle, free-form area, rectangle, text; move/delete/clear — all in millimetres |
| 3D | tissue, isosurface, MIP and transfer-function DVR; opacity, brightness, quality; ambient occlusion |
| Transfer function | editor over a log-scaled histogram, add/move/remove points, per-point colour, presets |
| Clipping | view-aligned cut, clip box, oblique plane, cut-surface slice overlay |
| Eraser | cylinder brush (radius/depth), undo, restore |
| Processing | Gaussian smoothing, Sobel edges |
| Output | screenshots (PNG), NIfTI export, DICOM attribute viewer |

Keyboard: F2/F3/F4 — 2D/3D/MPR, arrows/PgUp/PgDn or wheel — slice,
Ctrl+wheel — zoom, Ctrl+Z — undo erase.

## Workspace

| Crate | Layer |
|---|---|
| `crates/mri-domain` | domain model and ports |
| `crates/mri-processing` | parallel volume algorithms |
| `crates/mri-io` | DICOM / NIfTI repositories |
| `crates/mri-render` | WGSL/wgpu renderer + CPU reference |
| `crates/mri-app` | application layer (state, use cases, tools, jobs) |
| `crates/mri-viewer` | egui desktop application |

See [architecture](docs/architecture.md),
[test suite](docs/testing.md) and
[ADRs](docs/decisions/).

## Develop

```bash
cargo test --workspace                     # all tests (GPU tests skip without adapter)
MRI_REQUIRE_GPU=1 cargo test --workspace   # as CI (install mesa-vulkan-drivers)
cargo clippy --workspace --all-targets -- -D warnings
cargo bench --workspace
cargo run --release -p mri-render --example snapshot -- <input> <out_dir>
MRI_SAMPLE_DICOM=/path/to/series cargo test -p mri-io   # also test on a real DICOM series
```

On Linux the window needs `libxkbcommon-x11` and a Vulkan/GL driver; for
headless GPU tests install Mesa lavapipe (`mesa-vulkan-drivers`).

## License

MIT — see [LICENSE](LICENSE).

