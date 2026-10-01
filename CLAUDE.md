# CLAUDE.md — FERRUM

Reusable visualisation core for volumetric medical images (DICOM/NIfTI)
in Rust (Cargo workspace): a desktop viewer, ports for data sources and
segmentation engines, and an agent skill (planned). Read
[docs/vision.md](docs/vision.md) before changing anything architectural.

## Principles — keep them

- FERRUM owns visualisation and measurement. Everything else goes through a
  port (`VolumeRepository`, `SegmentationEngine`, later `Exporter`), with
  implementations composed at compile time.
- Every use case lives in `ferrum-app`. The desktop UI, embedding
  applications and the agent tool (`ferrum-agent` / `ferrum-cli`) are thin
  adapters over it.
- AI engines run out of process behind `ferrum-engine/1`. There is no
  inference in Rust, and no engine or weights are bundled.
- AI tools stay visible but disabled without an engine. Engine licences
  (`research_only`) are shown.
- Agents and engines propose; only people confirm. Values come from voxels.
- No patient identifiers leave FERRUM by default. Never commit patient
  data.

## Layering — never violate

```
ferrum (presentation) → ferrum-app (application) → ferrum-domain ← ferrum-io (data)
                                       ↘ ferrum-processing, ferrum-render (infrastructure)
```

- `ferrum-domain` has no I/O, no GPU, no UI dependencies; it defines ports
  (`VolumeRepository`).
- `ferrum-app` must compile without wgpu and egui (`ferrum-render` is used with
  `default-features = false`); it talks to the GPU through `GpuSink`.
- `ferrum-io` implements `VolumeRepository`; nothing else parses files.
- `ferrum-engines` implements `SegmentationEngine` (`ferrum-engine/1`
  client, mock, reference server); nothing else talks to engines. Protocol
  changes update `docs/engine-protocol.md` and the conformance suite.
- `ferrum-agent` holds the agent commands (JSON in, `ferrum-agent/1`
  envelope out) and depends only on `ferrum-domain` and `ferrum-io`;
  `ferrum-cli` only parses arguments. New commands update
  `docs/agent-cli.md` and the contract tests in `crates/ferrum-agent/tests`.
- No business logic in `ferrum` widgets — add a use case to `Viewer`.
- DICOM tags only through `dicom_dictionary_std::tags` constants — never raw
  tag literals.

## Rendering rule

`crates/ferrum-render/src/shaders/volume.wgsl` and
`crates/ferrum-render/src/cpu/raycast.rs` implement the same image-formation
model. Any change to one must be mirrored in the other and covered by
`crates/ferrum-render/tests/gpu_parity.rs`.

## Commands

```bash
cargo test --workspace
FERRUM_REQUIRE_GPU=1 cargo test --workspace     # CI mode (lavapipe)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo bench --workspace
make coverage                                # coverage report; CI floor in Makefile
cargo run --release -- <path>                 # the app is the default member
```

## Conventions

- Keep functions within the complexity budget in `clippy.toml` (cognitive
  complexity 25, 120 lines, nesting 6); split them instead of `allow`.
  Coverage must not drop below `COVERAGE_FLOOR` (docs/quality.md).
- Every public item has a doc comment (`missing_docs` is a workspace lint).
- No `unwrap()`/`expect()` outside tests (clippy lints).
- Never commit patient data or volume data (not even synthetic): tests
  generate DICOM/NIfTI at run time (`crates/ferrum-io/tests/common`); a real
  series can be supplied with `FERRUM_SAMPLE_DICOM=<dir>`. The only images in
  the repository are README screenshots in `docs/images/`, rendered by
  `examples/showcase.rs` from public, de-identified data with attribution
  (the start-screen image comes from the UI test suite and contains no data).
- Volumes are always in the canonical LPS frame (`+x` left, `+y`
  posterior, `+z` superior); new readers must reorient into it.
- Architecture changes update `docs/architecture.md`; decisions go to
  `docs/decisions/` as new ADRs (existing ADRs are append-only).
- Branches: `develop` is the integration branch; use `feat/`, `fix/`,
  `docs/`, `refactor/` prefixes.
