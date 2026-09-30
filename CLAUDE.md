# CLAUDE.md — dicom_renderer

Rust desktop viewer for DICOM/NIfTI volumes (Cargo workspace).

## Layering — never violate

```
mri-viewer (presentation) → mri-app (application) → mri-domain ← mri-io (data)
                                   ↘ mri-processing, mri-render (infrastructure)
```

- `mri-domain` has no I/O, no GPU, no UI dependencies; it defines ports
  (`VolumeRepository`).
- `mri-app` must compile without wgpu and egui (`mri-render` is used with
  `default-features = false`); it talks to the GPU through `GpuSink`.
- `mri-io` implements `VolumeRepository`; nothing else parses files.
- No business logic in `mri-viewer` widgets — add a use case to `Viewer`.
- DICOM tags only through `dicom_dictionary_std::tags` constants — never raw
  tag literals.

## Rendering rule

`crates/mri-render/src/shaders/volume.wgsl` and
`crates/mri-render/src/cpu/raycast.rs` implement the same image-formation
model. Any change to one must be mirrored in the other and covered by
`crates/mri-render/tests/gpu_parity.rs`.

## Commands

```bash
cargo test --workspace
MRI_REQUIRE_GPU=1 cargo test --workspace     # CI mode (lavapipe)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo bench --workspace
cargo run --release -p mri-viewer -- <path>
```

## Conventions

- Every public item has a doc comment (`missing_docs` is a workspace lint).
- No `unwrap()`/`expect()` outside tests (clippy lints).
- Never commit patient data or volume data (not even synthetic): tests
  generate DICOM/NIfTI at run time (`crates/mri-io/tests/common`); a real
  series can be supplied with `MRI_SAMPLE_DICOM=<dir>`. The only images in
  the repository are README screenshots in `docs/images/`, rendered by
  `examples/showcase.rs` from public, de-identified data with attribution
  (the start-screen image comes from the UI test suite and contains no data).
- Volumes are always in the canonical LPS frame (`+x` left, `+y`
  posterior, `+z` superior); new readers must reorient into it.
- Architecture changes update `docs/architecture.md`; decisions go to
  `docs/decisions/` as new ADRs (existing ADRs are append-only).
- Branches: `develop` is the integration branch; use `feat/`, `fix/`,
  `docs/`, `refactor/` prefixes.
