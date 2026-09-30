# Test suite

The test suite is designed along the layers of the architecture: the lower
the layer, the more exhaustive and the faster the tests. Everything runs
headlessly with `cargo test --workspace` (≈ 180 tests, < 1 minute after
compilation); GPU tests use any Vulkan/Metal/DX12 adapter, including the
software rasteriser *lavapipe* used in CI.

```bash
make test               # all tests; GPU tests skip if no adapter exists
make test-gpu           # same, but a missing adapter is a failure (CI)
make lint               # rustfmt + clippy -D warnings
make bench              # criterion benchmarks
```

## Test pyramid

```mermaid
flowchart TB
    UI["UI end-to-end · egui_kittest<br/>3 tests: navigation + full-window wgpu renders"]
    GPU["GPU parity · wgpu vs CPU reference<br/>11 tests: every mode, clipping, mask, AO, ESS, slices"]
    INT["Integration · use cases &amp; data layer<br/>26 tests: fake repository, recording GPU sink, synthetic + real DICOM"]
    PROP["Property-based · proptest<br/>11 invariants × 256 cases"]
    UNIT["Unit tests · pure functions<br/>~120 tests in domain, processing, io, render, app"]
    UI --> GPU --> INT --> PROP --> UNIT
```

| Layer | Location | What is verified |
|---|---|---|
| Domain unit | `mri-domain/src/**` `#[cfg(test)]` | volume storage round-trip, trilinear sampling = GPU filtering, window/level & LUT, transfer-function validation/editing/baking, clipping half-spaces, camera maths, slice extraction & view mapping, measurements (distance, angle, shoelace area), eraser mask & undo |
| Domain properties | `mri-domain/tests/properties.rs` | TF samples bounded, `max_opacity_in` dominates samples (soundness of empty-space skipping), TF edits keep invariants, window monotonic, storage error bounded, ray/box interval inside box, clipping never extends a segment, camera orbit keeps distance, slice coordinates in unit cube, view mapping round-trip, trilinear sample in range |
| Processing | `mri-processing/src/**` | histogram counts/percentiles/peaks, brick ranges bound every trilinear sample, AO on analytic half-space, box filter, Gaussian mass conservation, Sobel edge response, resampling factors |
| Data layer | `mri-io/tests/dicom_integration.rs`, `src/**` | synthetic DICOM (explicit/implicit VR LE, big endian, signed + rescale, MONOCHROME1, multi-frame, missing positions, multiple series, corrupt files), progress & cancellation, NIfTI round-trip (plain/gzip), optional **real series** given by `MRI_SAMPLE_DICOM=<dir>` |
| Shaders | `mri-render/tests/shader_validation.rs` | WGSL parses and validates with naga (no GPU needed); `MODE` override exists; uniform block sizes equal the Rust `#[repr(C)]` structs |
| GPU parity | `mri-render/tests/gpu_parity.rs` | GPU image vs CPU reference (mean abs. difference < 2.5/255, < 2 % outliers) for tissue, isosurface, MIP, TF-DVR, clip box + view cut + cut surface, oblique plane, eraser mask, ambient occlusion; empty-space skipping leaves the image unchanged in every mode; GPU slice rendering equals domain slice extraction ±2/255; oversized volumes downsample to fit the device |
| CPU renderer | `mri-render/src/cpu/raycast.rs` | shading sanity, picking, mask effect, TF lookup |
| Application | `mri-app/tests/use_cases.rs`, `src/tools.rs` | load flow (auto-load, series choice, errors), GPU synchronisation uploads only what changed (volume, LUT, occupancy, full vs partial mask, AO), eraser + undo + reset, background AO, filters replace dataset, slice navigation, windowing, MPR navigation, measurements in mm from screen input, text annotation flow, probe, slice rect fitting, reload resets state; every 2D tool state machine |
| UI | `dicom_renderer/tests/ui.rs` | the real `ViewerApp` driven through the accessibility tree (welcome screen, mode/tool/render-mode switching, info dialog) and full-window wgpu renders of 2D, 3D (4 modes) and MPR asserting the views are drawn |

## Oracles

- **CPU reference renderer** — `CpuRaycaster` mirrors `volume.wgsl`
  function by function (same sampling, jitter hash, compositing, empty-space
  skipping) and is the oracle for GPU output.
- **Domain extraction** — `SliceImage::extract` + `WindowLevel::apply` is the
  oracle for the GPU slice shader.
- **Analytic phantoms** — spheres, half-spaces and ramps with known answers
  (e.g. AO of a half-space, isosurface hit position of a sphere).
- **Synthetic DICOM generator** — `mri-io/tests/common/mod.rs` writes
  series at test time with configurable transfer syntax, rescale, sign,
  photometric interpretation, frames and geometry; no image data is
  committed.

## Benchmarks

| Bench | Crate | Measures |
|---|---|---|
| `processing` | `mri-processing` | histogram, brick grid, AO, downsampling on 256³; Gaussian/Sobel on 96³ |
| `dicom_loading` | `mri-io` | header scan and full load of a 256×256×128 series |
| `render` | `mri-render` | CPU reference per mode; GPU per mode with/without empty-space skipping |

Reference numbers (4-core container, llvmpipe software GPU):
histogram 1.5 Gvoxel/s, bricks 2 Gvoxel/s, AO 256³ in 48 ms, DICOM
128-slice load in 50 ms; empty-space skipping speeds up GPU rendering
1.5–5× (isosurface 23.8 → 4.6 ms at 256² on llvmpipe).

## Adding tests

- New domain rule → unit test next to it, plus a property if it has an
  invariant.
- New shader feature → implement it in `cpu/raycast.rs` too and add a
  parity case in `gpu_parity.rs`.
- New use case → exercise it through `Viewer` in `mri-app/tests/use_cases.rs`
  with the fake repository and recording sink.
- New widget → kittest test in `dicom_renderer/tests/ui.rs`.
