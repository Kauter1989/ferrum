# ADR 0002 — Standalone repository

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

ADR 0001 introduced the Rust viewer as the `rust-viewer/` sub-project of the
`mriviewer` monorepo. The rest of that monorepo (React web viewer, visionOS
app, segmentation models) is not needed for its further development.

## Decision

The workspace moves to its own repository, `dicom_renderer`, with the Cargo
workspace at the repository root and `develop` as the integration branch.
Only files required to build, test and develop the viewer are carried over:
sources, `Cargo.lock`, toolchain/format/lint configuration, documentation,
CI and the Makefile. The repository is MIT-licensed.

The real DICOM sample previously read from the monorepo's web e2e test data
is no longer referenced by path; the real-data integration test runs when
`MRI_SAMPLE_DICOM` points to a series and is skipped otherwise.

## Consequences

- No coupling to the legacy code base; CI builds only this workspace.
- Paths in ADR 0001 (`rust-viewer/`, `web/`) refer to the monorepo layout
  at the time of that decision.
