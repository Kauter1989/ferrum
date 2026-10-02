# Changelog

All notable changes to FERRUM. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/).

## [0.2.0] — 2026-10-02

FERRUM becomes a visualisation core with open interfaces for segmentation
engines and AI agents, and goes open source.

### Added

- **Annotations:** annotation list with names, slice navigation and JSON
  export with study identification and one-based slice numbers.
- **Segments:** patient geometry on every volume, label maps and segments,
  2D and 3D overlay with CPU parity, NIfTI label-map import and export.
- **Segmentation engines:** the `SegmentationEngine` port and the FERRUM
  Engine Protocol `ferrum-engine/1` (client, mock engine, reference server,
  conformance suite); AI segmentation panel; automatic segmentation jobs;
  bridges for nnInteractive, TotalSegmentator and MONAI Label.
- **Provenance and review:** author and status (proposed, confirmed,
  rejected) on annotations and segments; workspace and result formats with
  SHA-256 of sources and an audit log; review queue in the desktop app
  (*File → Open workspace*).
- **Agent skill (`ferrum-agent/1`):** headless `ferrum-cli` with study,
  view (slice, montage, MPR, 3D), probe, stats, profile, measure, annotate,
  segment, review, engine and export commands; JSON Schemas; MCP server
  (`ferrum-cli mcp`); skill package and plugin in `skills/`; evaluations
  on phantoms with known answers; identifier scan of all outputs.
- **Export:** DICOM SEG and DICOM SR (TID 1500) for hand-off to PACS and
  reporting.
- **Quality:** CI gates for line coverage (floor in the `Makefile`) and a
  complexity budget (`clippy.toml`).
- **Project:** `CONTRIBUTING.md` (DCO sign-off), `SECURITY.md`,
  `CODE_OF_CONDUCT.md`, `CITATION.cff`; dependency licence check
  (`cargo deny check licenses`) in CI.

### Changed

- Licence: **MIT OR Apache-2.0** (was MIT); see ADR 0009.
- The slice slider shows one-based slice numbers.
- Release archives also contain `ferrum-cli`, the agent skill package and
  a plugin folder.

## [0.1.0] — 2026-10-01

First release of the desktop viewer: DICOM and NIfTI loading in the LPS
frame, 2D slices and MPR, GPU volume rendering (tissue, isosurface, MIP,
transfer functions) with a CPU reference renderer and parity tests,
measurements, clipping, eraser and headless snapshots.

[0.2.0]: https://github.com/Kauter1989/ferrum/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Kauter1989/ferrum/releases/tag/v0.1.0
