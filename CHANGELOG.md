# Changelog

All notable changes to FERRUM. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/).

## [0.2.2] — 2026-10-02

First release archived on Zenodo, which gives FERRUM a citable DOI.

### Changed

- README: the release badge reads the latest release from GitHub again
  now that the repository is public, and a DOI badge links to the Zenodo
  archive.

## [0.2.1] — 2026-10-02

A clear path to segmentation in the desktop app, and every tool offered
only in the view modes where it works.

### Added

- **Region tool:** built-in segmentation without an engine. A click on a
  slice fills the connected region whose values lie within ± a tolerance
  of the clicked voxel (default a tenth of the window width); a size
  limit refuses regions that leak into neighbouring tissue.
- **Segment toolbar group** in the 2D and MPR views: the region tool and
  the AI tools, which stay disabled until an engine is connected and say
  why in their tooltip.
- **Segmentation guide:** the *Segmentation* section shows the next step
  (choose a tool, draw on a slice, check, accept), and the slice view
  shows a one-line hint while a segmentation tool is active.

### Changed

- Settings panel tabs follow the view mode: *Image* in 2D, *Volume* in
  3D, both in MPR. The *AI segmentation* section became *Segmentation*,
  with the AI tools moved to the toolbar.
- **Ctrl+Z** undoes what the current view edits: an eraser stroke in 3D,
  the last segmentation edit or AI prompt in 2D and MPR.
- Region growing lives in the domain layer and is shared by the desktop
  app and the agent's `segment threshold`.

### Fixed

- Segmentation could be started from the 3D view (AI tools and automatic
  runs in the *Volume* tab); segments are now created on slices only.
- The volume eraser stayed active in the MPR view's 3D cell, without a
  control showing it; it now works in the 3D view only.
- **Ctrl+Z** in the 2D view undid invisible eraser strokes in 3D.
- *Segments → Add* created an empty segment that nothing could fill; it
  is removed.

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

[0.2.2]: https://github.com/Kauter1989/ferrum/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/Kauter1989/ferrum/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/Kauter1989/ferrum/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Kauter1989/ferrum/releases/tag/v0.1.0
