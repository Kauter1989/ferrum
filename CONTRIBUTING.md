# Contributing to FERRUM

Thank you for helping. FERRUM is a visualisation core for medical images,
so correctness, privacy and a clean architecture matter more than speed of
merging. Please read [docs/vision.md](docs/vision.md) and
[docs/architecture.md](docs/architecture.md) before a larger change, and
open an issue first to discuss new features or architectural changes.

## Ground rules

- **Never commit patient data or volume data**, not even synthetic or
  de-identified. Tests generate DICOM and NIfTI at run time
  (`crates/ferrum-io/tests/common`); a real series can be supplied locally
  with `FERRUM_SAMPLE_DICOM=<dir>`.
- Keep the layering: `ferrum` → `ferrum-app` → `ferrum-domain` ←
  `ferrum-io`; business logic belongs in `ferrum-app`, not in widgets.
- Changes to `crates/ferrum-render/src/shaders/volume.wgsl` must be
  mirrored in `crates/ferrum-render/src/cpu/raycast.rs` and covered by
  `crates/ferrum-render/tests/gpu_parity.rs`.
- Protocol changes (`ferrum-engine/1`, `ferrum-agent/1`) update their
  documents, schemas and contract or conformance tests.
- Every public item has a doc comment; no `unwrap()`/`expect()` outside
  tests; stay within the complexity budget in `clippy.toml`.

The full list is in [CLAUDE.md](CLAUDE.md), which applies to people and
AI assistants alike.

## Workflow

1. Branch from `develop` with a `feat/`, `fix/`, `docs/` or `refactor/`
   prefix.
2. Before opening a pull request against `develop`, run:

   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

   Coverage must not drop below the floor in the `Makefile`
   (see [docs/quality.md](docs/quality.md)).
3. Describe what changed and how it was verified. Architecture changes
   update `docs/architecture.md`; decisions go to `docs/decisions/` as a
   new ADR (existing ADRs are append-only).

## Sign-off (Developer Certificate of Origin)

Every commit must be signed off to certify that you wrote the change or
otherwise have the right to submit it under the project's licence, as
stated in the [Developer Certificate of Origin](https://developercertificate.org/):

```bash
git commit -s -m "fix: …"
```

This adds a `Signed-off-by: Your Name <you@example.com>` line.

## Licence of contributions

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in FERRUM by you, as defined in the Apache-2.0
license, shall be dual licensed under the MIT license and the Apache
License, Version 2.0, without any additional terms or conditions.

## Questions and commercial work

Use GitHub issues for bugs and questions. For integrations and support,
write to [research@vchukanov.ru](mailto:research@vchukanov.ru).
