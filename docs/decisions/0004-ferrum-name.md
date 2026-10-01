# ADR 0004 — Project name FERRUM

- **Status:** Accepted
- **Date:** 2026-10-01

## Context

The application took the repository's name, `dicom_renderer` (ADR 0003),
while its library crates kept the `mri-` prefix from the monorepo era.
Neither name says what the project is: the viewer handles CT, PET and
other modalities as well as MRI, and it does more than render DICOM.

## Decision

- The product is called **FERRUM**, with the tagline *"FERRUM.
  High-performance medical imaging."* *Ferrum* is Latin for iron, a nod to
  Rust.
- Crates are renamed to `ferrum-domain`, `ferrum-processing`, `ferrum-io`,
  `ferrum-render` and `ferrum-app`. The application crate and its binary
  are `ferrum` (`crates/ferrum`).
- Environment variables use the `FERRUM_` prefix: `FERRUM_REQUIRE_GPU`,
  `FERRUM_SAMPLE_DICOM` and `FERRUM_SNAPSHOT_DIR`.
- Release archives are named `ferrum-<tag>-<target>`. The per-user
  configuration directory is `<config dir>/ferrum`.
- The GitHub repository keeps its name, `dicom_renderer`.

## Consequences

ADRs 0001–0003 keep the names that were current when they were written.
The "recent studies" list stored under the previous configuration
directory is not migrated.
