# Changelog

All notable changes to FERRUM. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- GPU groups can now be shared with processes outside FERRUM, such as a local model server on the same card ([design](docs/gpu-sharing.md), [ADR 0015](docs/decisions/0015-gpu-lease-protocol.md)). An engine call takes a lease of the new protocol `ferrum-gpu-lock/1` (crate `ferrum-gpu-lock`). With `network.gpu_capacity_mib` and `network.gpu_peaks_mib`, it runs next to other holders when its expected peak fits; otherwise it waits, and MCP progress names the holders. A group that stays full gives `engine_unavailable` naming them. **Upgrade note:** the lock moved from the shared temporary folder (`/tmp/ferrum-gpu-<group>.lock`) to a per-user directory (`$XDG_RUNTIME_DIR/ferrum/gpu`, else `~/.cache/ferrum/gpu`, or `network.gpu_lock_dir`), which is refused when another user owns it or can write it. Upgrade every FERRUM process on a machine together: an older one does not see the new lock.

### Fixed

- DICOM series that were not acquired as axial slices with rows towards the patient's left (sagittal and coronal MR, a reversed row direction, slices ordered towards the feet) were shown in their stored order: the "axial" view of a sagittal series showed sagittal anatomy and the image could be mirrored, while the labels read R/L as for an axial CT. DICOM volumes are now brought into the canonical LPS grid like NIfTI; coordinates, probe and measurements are unchanged. A DICOM SEG of such a series lies on the LPS grid and references the source series, with per-frame source images only where the planes coincide. Annotations saved before this version on a series whose grid only flipped (not resized) stay on the old side; label maps are moved correctly.
- Enhanced multi-frame DICOM (current MR and PET scanners) was stacked in file order with the spacing from the header and its origin at zero, because position, orientation and pixel spacing live in the functional groups. The loader now reads them per frame: frames are placed by their own Image Position, pixel spacing and slice thickness come from the Pixel Measures, frames at one position are told apart by Temporal Position Index, Effective Echo Time and b-value, and several Enhanced files of one series are ordered together. Each file is decoded once instead of once per frame.
- A DICOM series with several images at one slice position (MR echoes, diffusion b-values, dynamic phases, PET frames) was silently stacked from whichever file came first at each position, which could mix two volumes. The loader now takes the first image at every position by Temporal Position Identifier, Echo Number, b-value, Acquisition Number, Trigger Time and Instance Number, and says that one volume of several is shown.
- The loaders now say what they could not show as stored: a 4D NIfTI file (volume 1 of N), irregular slice gaps (a missing slice), slices ordered without image positions or frames of a multi-frame file (spacing taken from the header), and a missing pixel spacing (1 mm assumed). The warnings appear once in the desktop app after loading and stay under **Info**; the agent's `study open` and `study info` carry them in `warnings`.
- Agent renders of a series that stores no window (NIfTI, most MR and PET) used the full value range, so a few bright voxels left the image dark. They now use the same 0.5–99.5 % window as the desktop app, from one implementation in `ferrum-domain`.
- Voxel values on a grid are now stored exactly ([ADR 0012](docs/decisions/0012-lossless-grid-storage.md)). A CT in whole Hounsfield units (and a series with a slope of one half) reads back as the file holds it: `probe` of a 55 HU voxel returned 55.01, a minimum of −25 was −24.9866, and a threshold at `--min 100` could miss the voxels of exactly 100 HU (found by the Stage 18 field test). The range of such a volume (`study info` `value_range`, window limits) can be up to twice as wide as the data; windows and presets are unchanged. Float images and other data not on a grid are stored as before.
- `clinical` took 3 s for a real mask of 83 000 voxels and 87 s for 893 000 (found in the field test on the MSD spleen): the convex hull tested every point against every face. It now keeps the points outside each face (Quickhull); a blobby mask of 100 000 voxels takes 0.19 s instead of 16.7 s, a mask of a million voxels 1.3 to 2.5 s. Volume, area and the 3D diameter are unchanged.

### Added

- Desktop: a series of several volumes (echoes, b-values, phases, PET frames) opens on volume 1 and says so once; a **Volume** row in the Image panel lists every volume by name (`b = 1000`, `echo 2, TE = 7.65 ms`, `frame 3/33, t = 60 s`) with ‹ › to step. Switching replaces only the voxels: slice position, zoom, view mode, segments and annotations stay, the image type you gave a NIfTI series holds for all its volumes, and the automatic window of the new volume is used unless you set one (a window the file stores for all volumes, chosen for the first, would hide the others). A volume loaded before comes back at once (cache of 1 GiB of voxels). Metrics name the volume they were measured on and are marked out of date after a switch; a volume that misses slices is listed but disabled, with the reason. A workspace reopens on the volume last shown.
- Agent: a series of several volumes (echoes, b-values, phases, PET frames) is listed by `study scan` one entry per volume, with its label (`b = 1000`, `echo 2, TE = 40 ms`, `phase 3/20`), keys and `group`. `study open` takes a volume id; a series opens volume 1 with a warning saying how to choose. A workspace belongs to the series: opening another volume of it switches the volume and keeps segments and annotations ([ADR 0013](docs/decisions/0013-multi-volume-series.md)). `study info` lists the volumes, and the provenance of every value names the volume. The workspace manifest records the open volume (`source.volume`, version 2, only for such a series). Classic DICOM multi-echo volumes are now named with their Echo Time.
- The readers split a series that holds several volumes into its volumes (`VolumeRepository::volumes`): DICOM by the fields that differ at one slice position (temporal position, echo number and time, b-value, trigger time, PET Frame Reference Time, acquisition number), else by rank, numbering repeats such as diffusion directions (`b = 1000, 2/32`) and naming the derived trace image (`b = 1000, trace (isotropic)`) and magnitude and phase; Enhanced files by frame; 4D NIfTI by volume, named from BIDS sidecars (`.bval`, PET `.json` `FrameTimesStart`). Volumes are named `b = 1000`, `TE = 80 ms`, `phase 3/20`, `frame 3/33, t = 60 s` or `volume 2/5`, and loading one reads only its images. The agent and the desktop app use this in the next steps (Stage 20.2–20.3); scanning and opening are unchanged.
- Agent: `segment metrics` (`--profile quick`, `--describe`) gives the size
  and first-order HU statistics of a segment in one call, from a new pure
  crate `ferrum-radiomics` ([design](docs/radiomics.md),
  [ADR 0011](docs/decisions/0011-radiomics-crate.md)). Every feature has
  a stable id, a unit and a definition; the result names the profile and
  its version and the parameters. Intensity features need a CT study:
  NIfTI without a declared modality is refused with a hint. `stats` now
  uses the same statistics, with unchanged output.
- Agent: `segment metrics --profile clinical` (0.1) adds the shape of a segment: area and volume of a marching-cubes surface mesh (built without a lookup table, closed and outward-wound), surface-to-volume ratio, compactness 1 and 2, sphericity, spherical disproportion, and the principal axes (lengths, elongation, flatness, directions in LPS). A mesh of a binary mask is rougher than the smooth shape: a ball comes out about 7 % larger in area.
- Agent: `clinical` (0.2) adds the maximum 3D diameter between voxel centres, the convex hull of the mesh (volume, area, solidity, convexity) and the connectivity of a segment: 26-connected pieces and the share of the largest, 6-connected cavities and their volume, the Euler number and the image sides touched. The envelope warns `fragmented`, `has_holes` and `touches_border`; a warning never changes the mask.
- Agent: `clinical` (0.3) adds the distribution of the HU values: skewness, excess kurtosis, energy, RMS, mean and robust mean absolute deviation, histogram entropy and uniformity (`--bin-width`, default 25 HU, bins anchored at 0 HU and recorded), the intensity-volume histogram (V10, V90, I10, I90 and a 101-point curve) and the share of voxels in HU ranges (`--hu-range MIN,MAX`, half-open and non-overlapping, plus the share outside; built-in ranges are marked as defaults, not clinical thresholds).
- Desktop: the metrics card now shows the `clinical` profile in the groups Size, Shape, Connectivity and Intensity, with the warnings, the share of voxels by HU range, the intensity-volume histogram, and fields for the histogram bin width and the HU ranges (applied with a button; nonsense is refused with the reason). Measured: `clinical` takes 53 ms for a ball of 103 000 voxels and 187 ms for a frayed, blobby mask of 102 000 voxels on a 512 × 512 × 300 CT (`cargo bench -p ferrum-radiomics`).
- Desktop: a ruler button on each segment opens a metrics card (size and
  HU statistics, units, a definition on hover, a note while the segment is
  only a proposal, "out of date" when the segment changed). A NIfTI file,
  which says nothing about what it is, asks for its image type when it is
  opened; the answer is kept in the workspace, as `study open --modality`
  does, and a workspace's modality is applied when it is opened.

### Changed

- Desktop: the region tool chooses its tolerance from the clicked value until you set it: 300 HU on aerated lung, a tenth of the window width (at most 40) elsewhere; *Automatic tolerance* in the panel. On MSD lung CT one click now grows one lung (Dice 0.92–0.97 against a classical reference); the old default grew nothing. (Stage 16, 16.6-n)
- Desktop: the region tool segments whole organs on noisy images. The
  reference value is the median of the smoothed values around the seed,
  voxels are compared after smoothing, contacts with neighbouring organs
  are cut by an opening measured in millimetres (so thick and thin slices
  are treated alike), and vessels and holes inside the region are filled.
  New settings: smoothing (mm), bridge cutting (mm), hole filling; the
  default size limit is now 8 000 ml (up to 20 000 ml) and the default
  tolerance at most 40. On MSD liver CT the best Dice rose from 0.46–0.78
  to 0.88–0.96. The click runs as a background job (up to 25 s on the
  finest studies) with a *Cancel* button; the app stays responsive.
  (Stage 16, 16.6)

## [0.3.0] — 2026-10-03

Stage 17: agent segmentation scenarios with nnInteractive,
TotalSegmentator and MONAI Label on one GPU
([scenarios](docs/segmentation-scenarios.md), [design](docs/agent-segmentation.md)).

### Added

- Agent: refine an engine object with further prompts (`segment
  interactive --segment L --append`, `--undo`), its prompts kept in the
  workspace (`engine_inputs.json`) and replayed; `--from-segment` redoes
  an agent segment seeded with lassos from its mask; lasso and scribble
  prompts as points on one slice.
- Agent: only a region around the prompts is uploaded to interactive
  engines (`--roi`, `--whole-volume`); GPU groups run engines that share a
  card one at a time; `engine list`; `segment auto --name-prefix`;
  `--modality` for NIfTI; MCP progress notifications for automatic jobs
  and engine sessions kept between MCP calls.
- Agent: `segment shape` (extent, slices, axial long and short axis,
  laterality), `segment components` (with `--split`), `segment compare`
  (Dice, HD95, also across workspaces), `segment edit` (keep largest,
  fill holes, restrict to a box, remove small parts), `stats` of a
  segment within a box, and quality checks on every engine result.
- Provenance `requested_by`: agents may change engine segments they asked
  for; a change sets them back to proposed.
- Operator settings `gpu_groups`, `max_prompts_per_object`,
  `roi_margin_mm`, `allow_research_only`.
- Engine protocol: `capabilities.deterministic`, checked by a replay round
  of the conformance suite.
- Bridges: TotalSegmentator jobs in a child process (memory freed,
  cancel ends the job); nnInteractive frees the image when a session
  closes and offers `--deterministic`; `bridges/compose.yml` with
  profiles for one GPU and a MONAI Label bridge image.
- Segment display on slices: outline, translucent fill or both, with a
  fill opacity set at display time — in the desktop app (*Segments → On
  slices*, *Fill opacity*) and in agent renders (`segment_style`,
  `segment_opacity`).
- `study open --modality CT` declares the modality of a NIfTI study, so
  its values are taken as Hounsfield units; kept in the workspace.
- Skill: `reference/segmentation.md`, engine evaluations with
  `ferrum-cli eval engine`; `scripts/benchmark_engines.py` for time and
  GPU memory per scenario; measured on an RTX 3080 12 GB
  (docs/agent-segmentation.md §2).
- `docs/segmentation-scenarios.md`: the segmentation scenarios S0–S8 as
  a step-by-step playbook with commands, linked from the README.

### Fixed

- Agent renders drew segment outlines only on the right and lower edges
  of a segment; outlines are now closed on all sides.

## [0.2.3] — 2026-10-02

### Fixed

- Zenodo archiving: Zenodo could not read the citation metadata of
  v0.2.2 ("Citation metadata load failed"). `.zenodo.json` now gives it
  the metadata explicitly; `CITATION.cff` stays for GitHub's *Cite this
  repository*.

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

[0.3.0]: https://github.com/Kauter1989/ferrum/compare/v0.2.3...v0.3.0
[0.2.3]: https://github.com/Kauter1989/ferrum/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/Kauter1989/ferrum/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/Kauter1989/ferrum/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/Kauter1989/ferrum/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Kauter1989/ferrum/releases/tag/v0.1.0
