# ADR 0001 — Rewrite the viewer core and UI in Rust (`rust-viewer/`)

- **Status:** Accepted
- **Date:** 2026-09-30
- **Scope:** monorepo-wide (new `rust-viewer/` sub-project; `web/` is kept as-is)

## Context

The React/Three.js viewer in `web/` mixes rendering, state and UI inside
large components and singletons (e.g. `Graphics2d.jsx`, 844 lines;
`VolumeRenderer3d.js`, 1470 lines). Views mutate renderer objects stored in
the Redux store, algorithms are duplicated per orientation, and most logic
cannot be tested without a browser. Performance is bounded by WebGL1-era
workarounds (3D textures emulated as 2D tile maps, several render-to-texture
passes per frame, 8-bit volumes).

The goal is maximum performance and a maintainable, testable architecture
for all non-segmentation functionality: DICOM loading, 2D/MPR viewing,
volume rendering, transfer functions, clipping and user tools.

## Decision

1. Implement a new Cargo workspace `rust-viewer/` with one crate per layer:

   | Crate | Layer | Depends on |
   |---|---|---|
   | `mri-domain` | Domain: entities, value objects, rules, ports | `glam`, `thiserror` |
   | `mri-processing` | Domain services: parallel algorithms | domain, `rayon` |
   | `mri-io` | Data: `VolumeRepository` for DICOM/NIfTI | domain, `dicom-rs` |
   | `mri-render` | Infrastructure: WGSL/wgpu renderer + CPU reference | domain, processing |
   | `mri-app` | Application: state, use cases, jobs, tools | domain, processing, render (pure part) |
   | `mri-viewer` | Presentation: egui/eframe desktop app | all of the above |

   The dependency rule `Presentation → Domain ← Data` is enforced by crate
   boundaries: `mri-domain` defines the `VolumeRepository` port implemented
   by `mri-io`; `mri-app` defines the `GpuSink` port implemented by the
   presentation layer; `mri-app` compiles without wgpu or egui.

2. Rendering uses wgpu (Vulkan/Metal/DX12/GL) with a **single-pass** ray
   caster: rays are built from the inverse view-projection matrix and
   clipped analytically against the volume box and up to eight half-spaces
   (clip box, oblique plane, view-aligned cut). Render modes are
   pipeline-overridable constants (branch-free specialised shaders).
   Volumes are uploaded once as 16-bit 3D textures (R16Unorm, R16Float
   fallback); 2D slices are sampled from the same texture on the GPU, so
   slice/window changes cost one uniform update.

3. Performance features: empty-space skipping over a min/max brick grid
   (classification uses the domain rule `RenderSettings::range_visible`,
   so skipping cannot change the image), dynamic resolution while
   interacting, precomputed volumetric ambient occlusion, incremental
   eraser-mask uploads, rayon-parallel DICOM decoding and processing.

4. A CPU reference ray caster in `mri-render::cpu` implements the same
   image-formation model as the WGSL shader. It is the test oracle for GPU
   output, the picker for interactive tools (eraser), and a software
   fallback.

5. Segmentation models (TensorFlow.js brain/lung segmentation, ROI
   palettes) are intentionally **out of scope**.

6. The legacy web viewer is not modified. The long-lived integration branch
   for this work is `rust_viewer`.

## Consequences

- Positive: a strict layering with ports makes every use case testable
  headlessly (≈200 automated tests including GPU/CPU parity on lavapipe);
  loading and rendering are an order of magnitude faster than the web
  version; the DICOM layer can later back `mri-core` for visionOS.
- Negative: the desktop app is a new deliverable to package; the web
  deployment remains the React app until a WebGPU build of the Rust viewer
  is added (eframe/wgpu support it, but `openjp2`-based JPEG 2000 decoding
  must be feature-gated for wasm).
- Follow-ups: WASM/WebGPU build, packaging (MSI/DMG/AppImage), PACS/DICOMweb
  repository implementing `VolumeRepository`.
