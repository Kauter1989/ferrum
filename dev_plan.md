# Development plan

This is the working plan for FERRUM, a reusable visualisation core for
medical imaging ([vision](docs/vision.md)). Stages 1–13 built the viewer.
Stage 14 opens it to AI segmentation engines, and Stage 15 makes it an
agent skill. Stages 16–20 add annotation datasets, agent segmentation
scenarios, quantitative profiles of segments and multi-volume MR and PET
series. What comes next, in order, is in the [Roadmap](#roadmap) at the
end.

Each stage groups features as user stories with acceptance criteria.
Status legend: ✅ done · 🚧 in progress · 📋 planned.

New stages are appended at the end. A finished stage is kept as a record
of what was delivered and how it is verified.

Personas:
- **Radiologist**: reads studies and needs speed, correct orientation and
  reliable windowing.
- **Clinician/surgeon**: explores anatomy in 3D for planning and for
  explaining findings to a patient.
- **Researcher**: works with NIfTI and public datasets and needs exports
  and reproducible renders.
- **Developer**: maintains and extends the code base.
- **Integrator**: builds FERRUM into a commercial or research product,
  or connects a segmentation engine.
- **Agent / harness developer**: lets AI agents use FERRUM as a skill in
  a medical harness.

---

## Stage 1 — Foundation and architecture ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 1.1 | As a **developer**, I want a layered workspace (domain, processing, io, render, app, viewer) so that I can change the UI, file formats or GPU backend independently. | `ferrum-domain` has no I/O/GPU/UI dependencies. `ferrum-app` compiles without wgpu and egui. Dependencies point inward (see `docs/architecture.md`). |
| 1.2 | As a **developer**, I want storage and GPU access behind ports (`VolumeRepository`, `GpuSink`) so that use cases can be tested without files or a graphics card. | Application tests run against an in-memory repository and a recording GPU sink. |
| 1.3 | As a **developer**, I want CI that checks formatting, lints, tests on a software GPU and a release build so that regressions are caught before merge. | `.github/workflows/ci.yml` runs fmt, clippy `-D warnings`, `FERRUM_REQUIRE_GPU=1` tests on lavapipe, and a release build. |
| 1.4 | As a **developer**, I want recorded architecture decisions so that the reasons behind the design survive. | ADRs in `docs/decisions/` (Rust + wgpu + egui; canonical LPS frame). |

## Stage 2 — Loading data ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 2.1 | As a **radiologist**, I want to open a DICOM folder and have it split into series so that I can pick the study I need. | Recursive scan, grouping by Series Instance UID, and a series picker when a folder holds more than one series. |
| 2.2 | As a **radiologist**, I want slices ordered by their real position so that anatomy is never shuffled. | Sorted by projection on the plane normal, with fallbacks to Instance Number and Slice Location. Covered by tests. |
| 2.3 | As a **radiologist**, I want compressed and unusual DICOM to open so that I don't have to convert files. | Explicit/implicit VR, big endian, JPEG, JPEG 2000, RLE, deflate, multi-frame, signed pixels, rescale slope/intercept, MONOCHROME1 and colour. Each case is covered by a test that generates the file at run time. |
| 2.4 | As a **researcher**, I want to open `.nii` / `.nii.gz` so that I can inspect public datasets. | NIfTI-1 reader with gzip. `sform`/`qform` are honoured and the volume is reoriented to LPS. A 4D file shows its first volume. |
| 2.5 | As a **radiologist**, I want loading to be fast and cancellable, with progress, so that large studies never freeze the app. | Parallel decode (rayon), a progress overlay and a cancel button. A 512×512×252 CT loads in about 0.34 s on 4 CPU cores. |
| 2.6 | As a **user**, I want to drop files onto the window or pass them on the command line so that opening a study takes one gesture. | Drag-and-drop overlay, CLI arguments, **Open folder** / **Open files**, and Ctrl+O. |
| 2.7 | As a **clinician** reading MR or PET, I want the loader to tell me when a series could not be shown as stored (several images per slice position, a 4D file, irregular or unknown slice spacing) so that I never measure a mixed or distorted volume without knowing. | [2.7-a] A DICOM series with more than one image at a slice position (echoes, b-values, phases, frames) loads one volume, never a mix: at every position the image that sorts first by Temporal Position Identifier, Echo Number, Diffusion b-value, Acquisition Number, Trigger Time, Instance Number, file order; this holds for files in any order and for 2 echoes interleaved by Instance Number. ✅ (PR 1) [2.7-b] Such a load warns with the number of images per position (`k`, or `min–max` when positions differ) and the number of positions, and says that one volume of several is shown. ✅ (PR 1) [2.7-c] When a gap between neighbouring positions differs from the median gap by more than max(0.01 mm, 2 % of the median), the load warns with the smallest and largest gap and the median used for the slice spacing; a regular series (also 1 mm or 5 mm, any orientation) gives no warning. ✅ (PR 1) [2.7-d] A series ordered without image positions (Instance Number, Slice Location, file order), or a multi-frame file whose per-frame positions are not read, warns that the slice spacing comes from the header (with the value); a series of several multi-frame files warns that only the first frame of each is read; a series without Pixel Spacing warns that 1 mm is assumed. ✅ (PR 1) [2.7-e] A NIfTI file with more than one volume (`dim[4..7]`) warns that volume 1 of N is shown; a 3D file gives no warning. ✅ (PR 1) [2.7-f] The agent's `study open` and `study info` carry every load warning in the envelope `warnings`, identical over CLI and MCP; the warnings hold no identifiers (privacy scan). ✅ (PR 1) [2.7-g] The desktop app shows the load warnings once after loading (a window the person dismisses) and keeps them in the series information. ✅ (PR 1) [2.7-h] Without a window stored in the data and without `window`, agent renders use the same percentile window (0.5–99.5 %) as the desktop app, from one implementation in `ferrum-domain`; a stored DICOM window or an explicit `window` is used as before. ✅ (PR 2) |
| 2.8 | As a **clinician** reading a sagittal or coronal MR (or any DICOM series not acquired head-to-feet axially), I want its planes shown as what they are, with true left and right, so that I never read a mirrored or mislabelled image. | [2.8-a] A DICOM series (classic or Enhanced) is stored in the canonical LPS grid, as NIfTI already is: its voxel axes are permuted and flipped onto the closest of `+x` left, `+y` posterior, `+z` superior; an oblique remainder stays in the direction; every voxel keeps its value at its patient position (exact for sagittal, coronal, axial with reversed rows, normal towards the feet, and 20° oblique). ✅ [2.8-b] The axial view of a sagittal series shows axial anatomy, and the screen's left is the patient's right in every view, as for an axial CT. ✅ [2.8-c] Probe, measurements and patient coordinates give the same millimetres as before for the same anatomical point. ✅ [2.8-d] A DICOM SEG of such a series lies on the LPS grid; a frame references a source image only when it lies in that image's plane, otherwise the whole source series is referenced without per-frame derivation. ✅ |

**2.7 — Out of scope:** splitting a series into selectable volumes (echo, b-value, phase), per-frame geometry of Enhanced multi-frame DICOM, resampling irregular series, NIfTI-2, PET units and SUV (planned as separate stages).

**2.7 — Clarifications:**
- assumed: a series with several images per position shows volume 1 with a warning instead of being refused. It matches the NIfTI 4D behaviour of 2.4 and keeps DWI and multi-echo series readable; refusing would block reading altogether.
- assumed: the irregular-gap tolerance is max(0.01 mm, 2 % of the median gap): above the rounding noise of Image Position (Patient), below a missing slice (100 %).
- assumed: the desktop shows the warnings in a dismissible window after loading and in the **Info** tab; no new dialog flow.
- assumed: the agent's default window changes only for data without a stored window; the schema and the parameters stay the same.
- assumed (2.8): workspaces made before 2.8 on such series are not migrated. Label maps are stored with their geometry and land on the new grid; annotations are stored per plane and slice, so when the grid's size changed they are refused with the existing size message, and on a grid that only flipped they stay on the old side (said in the changelog). A migration would need a workspace format change.

## Stage 3 — 2D slices and MPR ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 3.1 | As a **radiologist**, I want axial, coronal and sagittal slices rendered straight from the volume so that switching planes is instant. | Slices are sampled on the GPU from the 3D texture. A slice change takes about 4 ms. Nearest or linear sampling. |
| 3.2 | As a **radiologist**, I want window/level by drag, by numeric input and by preset so that I can read soft tissue, lung and bone. | Mouse drag, L/W fields, presets (full range, brain, soft tissue, lung, bone) and auto percentiles. |
| 3.3 | As a **radiologist**, I want pan and zoom around the cursor so that I can inspect details. | Ctrl+wheel zooms at the cursor. Double-click resets the view. |
| 3.4 | As a **radiologist**, I want an MPR layout with linked crosshairs so that I can localise a finding in three planes. | 2×2 layout (three planes + 3D). Crosshairs are colour-coded per plane. Right-click moves every plane to that point. |
| 3.5 | As a **radiologist**, I want to see correct patient orientation so that I never mix up left and right. | All data is in LPS. Edge labels show R/L, A/P, S/I, verified against NIfTI and DICOM of the same patient. |
| 3.6 | As a **radiologist**, I want a voxel probe so that I can read physical values (HU). | The probe shows voxel indices and the rescaled value. |

## Stage 4 — Measurements ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 4.1 | As a **radiologist**, I want distance, angle, area, rectangle and text tools so that I can document findings. | Results are in mm, mm² and degrees and account for anisotropic voxels. Tool state machines are unit-tested. |
| 4.2 | As a **radiologist**, I want to move, delete and clear annotations so that I can fix mistakes. | Move and delete tools, plus "Clear all". Annotations are bound to their slice. |

## Stage 5 — 3D volume rendering ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 5.1 | As a **clinician**, I want tissue, isosurface, MIP and transfer-function rendering so that I can choose the best view of the anatomy. | Four techniques in one WGSL ray caster, specialised through a pipeline-overridable constant. |
| 5.2 | As a **clinician**, I want smooth interaction on large studies so that exploring in 3D feels natural. | Empty-space skipping over a brick grid, lower resolution while rotating, and jittered ray starts. The isosurface on the 512×512×252 CT runs at about 20 fps even on a software GPU. |
| 5.3 | As a **clinician**, I want shaded surfaces with ambient occlusion so that depth is easy to read. | Gradient Phong shading, bisection refinement of surface hits, and a precomputed AO volume. |
| 5.4 | As a **clinician**, I want to rotate, pan, zoom and jump to standard views so that I can orient myself quickly. | Trackball, pan, zoom and reset. One-click A/P/L/R/S/I views. An orientation gizmo. |
| 5.5 | As a **developer**, I want a CPU reference ray caster so that GPU output can be verified pixel by pixel. | The CPU ray caster mirrors `volume.wgsl`. GPU parity tests cover every mode and feature. |
| 5.6 | As a **user**, I want volumes larger than my GPU's limit to still open so that the app never fails on big data. | Automatic box downsampling to `max_texture_dimension_3d`. `R16Float` fallback when `R16Unorm` is not filterable. |

## Stage 6 — Transfer functions ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 6.1 | As a **clinician**, I want to edit the transfer function over the histogram so that I can highlight the structures I need. | Log-scaled histogram. Drag points, double-click to add, right-click to remove, and a per-point colour picker. |
| 6.2 | As a **radiologist**, I want CT presets in Hounsfield units so that typical views need one click. | *Soft tissue + bone*, *lung vessels* and *bone* presets that adapt to the data range. |

## Stage 7 — Clipping and editing ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 7.1 | As a **surgeon**, I want a clip box, a view-aligned cut and an oblique plane so that I can expose internal structures. | Up to 8 analytic half-spaces. Optional raw-slice overlay on the cut surface. Reset button. |
| 7.2 | As a **surgeon**, I want to erase occluding anatomy (e.g. the scanner table) with a brush so that I can present a clean view. | Cylindrical brush with radius and depth in mm. Undo (Ctrl+Z) and restore all. Incremental mask upload to the GPU. |

## Stage 8 — Processing and output ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 8.1 | As a **researcher**, I want Gaussian smoothing and Sobel edges so that I can reduce noise or emphasise boundaries. | Filters run as background jobs, and the UI stays responsive. |
| 8.2 | As a **researcher**, I want NIfTI export and PNG screenshots so that I can reuse results elsewhere. | NIfTI writer with a correct `sform`, round-trip tested. PNG screenshot of the window. |
| 8.3 | As a **radiologist**, I want to see the DICOM attributes of the series so that I can check acquisition parameters. | An Info window lists the series attributes. |

## Stage 9 — Futuristic interface ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 9.1 | As a **user**, I want the image to take the whole screen, with controls in floating docks, so that nothing competes with the anatomy. | Full-bleed dark canvas. Glass docks: modes and planes on the left, tools in the centre, file actions on the right. Status HUD at the bottom. |
| 9.2 | As a **user**, I want a collapsible settings panel with only the relevant controls so that the screen stays uncluttered. | **Tab** toggles the panel. Slices and Volume tabs in MPR. Collapsible sections. |
| 9.3 | As a **radiologist**, I want HUD read-outs in the views (plane, slice number, W/L, zoom, orientation) so that I don't have to look away from the image. | Corner brackets in the plane colour. Monospace HUD labels. A slice scrubber on the view edge. |
| 9.4 | As a **user**, I want a start screen with drag-and-drop and recent studies so that I can reopen my work in one click. | The recent list persists in the user config directory (8 entries). UI tests cover the start screen. |
| 9.5 | As a **user**, I want icon buttons with tooltips and accessible names so that the UI is both compact and testable. | Every dock button has a tooltip and an accessible label. `egui_kittest` drives the real app by those labels. |

## Stage 10 — Quality and documentation ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 10.1 | As a **developer**, I want a layered test suite so that every change is checked at the right level. | About 180 tests: unit, property-based, data layer, shader validation, GPU parity, application and UI. See `docs/testing.md`. |
| 10.2 | As a **developer**, I want reproducible benchmarks and screenshots so that performance and visuals can be compared over time. | `make bench`, `make snapshot` (timings per technique) and `make showcase` (README images from public data). |
| 10.3 | As a **visitor**, I want a README with real-data screenshots, limits and performance numbers so that I can judge the project quickly. | README: gallery from the MSD lung CT, supported modalities and grid limits, performance numbers with the test hardware, and references to the visualization literature. |

## Stage 11 — FERRUM brand and calm workspace UI ✅

The visual language of Stage 9 (floating glass docks, coral and cyan) is
replaced by a docked workstation layout in calm navy tones.

| # | User story | Acceptance criteria |
|---|---|---|
| 11.1 | As a **visitor**, I want the project to have a clear name and tagline so that I remember what it is. | The product is **FERRUM**, "High-performance medical imaging". Crates are `ferrum-*`, the binary is `ferrum`, and environment variables use the `FERRUM_` prefix (ADR 0004). The logo is the "Fe" tile of the periodic table. |
| 11.2 | As a **radiologist**, I want a calm interface that does not compete with the images during long reading sessions. | Navy surfaces, one blue accent, light-grey read-outs over a black canvas. No glow or saturated decorations. |
| 11.3 | As a **radiologist**, I want a predictable workstation layout so that every control is always in the same place. | The header holds the study, the 2D/3D/MPR switch and file actions. The toolbar holds the tools of the current mode. The right panel has *Image*, *Volume* and *Details* tabs, and a status bar sits at the bottom. |
| 11.4 | As a **radiologist**, I want to switch between recent studies without a file dialog. | A *Studies* sidebar shows the current study and the recently opened ones. One click opens a study. |
| 11.5 | As a **radiologist**, I want window and level as sliders with exact values, besides presets. | *Window* and *Level* rows with a slider and an editable value. Presets and *Auto* are chips. |
| 11.6 | As a **radiologist**, I want the series attributes next to the image, not in a pop-up. | **Info** opens the *Details* tab: dimensions, spacing, intensity range and DICOM attributes. |

## Stage 12 — Quality metrics ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 12.1 | As a **developer**, I want test coverage measured on every pull request so that untested code is visible and coverage never silently drops. | `cargo-llvm-cov` job in CI that counts all test levels. lcov and HTML reports are kept as artifacts, a summary is shown on the run page, and the job fails below the floor (87 %; baseline 88.5 % of lines). `make coverage` produces the report locally. |
| 12.2 | As a **developer**, I want a complexity budget so that functions stay small and readable. | Clippy enforces cognitive complexity ≤ 25, ≤ 120 lines per function and nesting ≤ 6 (`clippy.toml`). Functions that exceeded it were split: the slice view, the start screen, the series picker and NIfTI reorientation. |
| 12.3 | As a **maintainer**, I want the release to be cut without pushing tags so that it works from any environment. | `release.yml` accepts a manual run with a `tag` input, which creates the tag and the release (ADR 0006). |

## Stage 13 — Annotation workflow ✅

| # | User story | Acceptance criteria |
|---|---|---|
| 13.1 | As a **radiologist**, I want to see all annotations of the study in one list so that I can review my findings. | In the 2D view, the *Image* tab lists every annotation with type, value, plane and slice number. The list is not shown in 3D/MPR. Each row can be deleted. |
| 13.2 | As a **radiologist**, I want to jump to the slice of an annotation so that I can find it again instantly. | Each annotation stores its plane and slice. Clicking the row (or its arrow) switches the 2D view to that plane and slice. Rows on the current slice are highlighted. |
| 13.3 | As a **radiologist**, I want to name annotations so that a measurement says what it measures. | Default names `"<Type> <n>"`, editable in the list. The name is shown on the image (`Name: value`), and empty names are rejected. |
| 13.4 | As a **researcher**, I want to export annotations to JSON so that I can analyse them elsewhere and link them to the study. | `ferrum-annotations` v1 document with the source file or folder, DICOM study and series UIDs, date, time and descriptions, the volume grid, and for every annotation its name, type, plane, slice, value with unit, text, points in mm and in voxel coordinates. Covered by io, application and UI tests. |
| 13.6 | As a **radiologist**, I want one slice numbering everywhere so that numbers never disagree. | Slice numbers are one-based in the slider, on the image, in the annotation list and in the export (`slice_number`; `slice_index` stays zero-based for tools). |
| 13.5 | As a **clinician**, I want only clinically useful tools so that the interface stays focused. | Smoothing and edge filters are hidden from the UI: they changed the data irreversibly. The processing code stays available for future use. |

## Stage 14 — Extensibility and AI segmentation (demo) ✅

FERRUM stays a visualisation core with extension points
([ADR 0007](docs/decisions/0007-extensibility-and-engine-protocol.md)).
Segmentation engines run out of process and speak the
[FERRUM Engine Protocol](docs/engine-protocol.md). There is no
inference in Rust for now.

| # | User story | Acceptance criteria |
|---|---|---|
| 14.1 | As an **integrator**, I want a documented engine protocol so that I can plug any segmentation engine into FERRUM without changing its code. | `docs/engine-protocol.md` (`ferrum-engine/1`) and ADR 0007. ✅ |
| 14.2 | As a **radiologist**, I want segments shown over the image and in 3D so that I can review a segmentation. | Volume geometry (origin, direction). `LabelMap`, segments with name, colour, visibility, opacity and volume in ml. 2D fill and outline and 3D rendering, with CPU-reference parity. NIfTI label-map import and export. ✅ |
| 14.3 | As a **developer**, I want an engine port with a network client and a mock so that engines are swappable and testable without a GPU. | `SegmentationEngine` / `InteractiveSession` ports; `ferrum-engines` with `HttpEngine` and `MockEngine`; tests against the mock. ✅ Also a reference server and a conformance suite runnable against any engine URL. |
| 14.4 | As a **radiologist**, I want AI tools in a collapsible panel that work only when an engine is connected so that I always know what is available. | *AI segmentation* section: connection status and URL, prompt tools (point ±, box, scribble, lasso), Accept, Reset and Undo. The tools are disabled with a hint when no engine is connected, and a *Research use only* badge appears when the engine reports it. ✅ Discard replaces Reset; undo is offered when the engine supports it. |
| 14.5 | As a **researcher**, I want a reproducible nnInteractive demo so that I can try interactive AI segmentation on my own GPU machine. | `bridges/` (`ferrum-bridge nninteractive`, FastAPI) with Docker and `docs/ai-demo.md` covering requirements, start-up, SSH tunnel and a walkthrough on the demo CT. A protocol conformance test runs against the bridge. ✅ The CI runs it against the bridge with a model-free backend; the GPU walkthrough is manual. |
| 14.6 | As an **integrator**, I want bridges for MONAI Label and TotalSegmentator so that automatic segmentation and active learning become available. | Automatic segmentation in FERRUM: jobs with progress and cancel, structure selection, one named segment per structure found; covered by the mock engine and the conformance suite. ✅ TotalSegmentator bridge (`ferrum-bridge totalsegmentator`, Docker, telemetry off, geometry-correct NIfTI) ✅; MONAI Label bridge (`ferrum-bridge monailabel`: DeepEdit/DeepGrow/SAM2 clicks, SAM2 boxes, segmentation models as jobs; tested against a fake MONAI Label server and the conformance suite) ✅ |

---

## Stage 15 — FERRUM as an agent skill ✅

AI agents in a medical harness use FERRUM as a skill: a skill package and
a headless tool, `ferrum-cli`, over the same use cases as the desktop
app. Agents propose and clinicians confirm
([ADR 0008](docs/decisions/0008-agent-skill.md),
[specification](docs/agent-skill.md)).

| # | User story | Acceptance criteria |
|---|---|---|
| 15.1 | As an **integrator**, I want a documented skill design so that I can plan FERRUM into my agent harness. | ADR 0008 and `docs/agent-skill.md`: commands, envelope, workspace, privacy, review, evaluation. ✅ |
| 15.2 | As a **clinician**, I want to know who created each annotation and segment and whether it was confirmed so that I never mistake a proposal for a finding. | `Provenance` (author human/agent/engine, status proposed/confirmed/rejected, time) in the domain. `ferrum-annotations` v2, segment metadata JSON, `ferrum-workspace` v1 read/write. ✅ Formats in [docs/workspace-format.md](docs/workspace-format.md): v1 annotations still read; sources hashed with SHA-256 (`source_changed`); atomic writes; audit log. AI results start as engine proposals, Accept confirms; the Segments and Annotations lists show the author with Confirm / Reject / Reopen. |
| 15.3 | As an **agent developer**, I want a headless command line with JSON output so that a skill can drive FERRUM from scripts. | `ferrum-agent` + `ferrum-cli`: study, view (slice, MPR, montage, 3D with pixel mapping), probe, stats, profile, measure, annotate, segment (incl. threshold region growing), export. CPU renderer without a GPU. Operator configuration, audit log. Contract tests on phantoms and JSON Schema validation. ✅ ([docs/agent-cli.md](docs/agent-cli.md)): study scan/open/info; view slice, montage, MPR (PNG + per-tile pixel→voxel→patient mapping, labels, crosshair, segment outlines) and volume (CPU ray caster, six viewpoints; 3D pixels are deliberately not mapped to voxels, measurements come from slices); probe, stats (box, sphere, segment, area annotation), profile, measure distance/angle/area; annotate; segment threshold; review; export bundle (report, annotations, label map, SHA-256, unconfirmed items marked). Operator configuration (read roots, workspace root, pseudonymised UIDs, limits, harness review), audit log, JSON Schemas checked on every call, contract tests on phantoms with known answers. |
| 15.4 | As a **harness developer**, I want an MCP server so that the tools plug into MCP-capable harnesses without glue code. | `ferrum-cli mcp` (stdio) serving the same commands; renders as image content; identical JSON to the CLI in a scripted session. ✅ JSON-RPC over stdio (2025-06-18, 2025-03-26, 2024-11-05), tools with input schemas and hints, envelope as text and structured content, PNG as image content, command errors as tool errors, safety instructions; series cached in memory (invalidated when sources change), annotations and segments re-read on every call. |
| 15.5 | As a **harness developer**, I want an installable skill package so that agents know when and how to use FERRUM safely. | `skills/ferrum/` (`SKILL.md`, references, schemas) and a plugin manifest in the release archives. Evaluations on phantoms with known answers. A scan proves that outputs contain no identifiers. ✅ `skills/ferrum/` (SKILL.md, four reference pages, generated schemas kept in sync by a test), plugin folder in the release archives (manifest, MCP configuration, bundled `ferrum-cli`). Evaluations: five tasks with known answers on a phantom, `ferrum-cli eval tasks|phantoms|grade` (correct, unit, from tools, no diagnostic wording, review), reference solutions in the tests. Identifier scan over every envelope, render and export file of a full session on a CT with patient data. |
| 15.6 | As a **clinician**, I want to review an agent's work in the viewer so that I can accept, edit or reject it. | *File → Open workspace*; review queue with author and status; decisions written to the workspace and the audit log. ✅ *Open workspace* (toolbar): source hashes checked, the workspace's series loaded, its annotations and segments attached. *Review* section: proposals with author, reviewer name, Confirm / Reject; decisions from the queue or the Annotations / Segments lists are logged and saved at once; *Save to workspace* for other edits. `ResultStore` port in the domain, `WorkspaceStore` in `ferrum-io`. |
| 15.7 | As a **harness developer**, I want AI segmentation and standard exports in the skill so that results flow to PACS and reporting. | `segment interactive` / `segment auto` through `ferrum-engine/1` (after 14.3–14.5); DICOM SEG and SR (TID 1500) export. ✅ Engine commands: `engine info`, `segment interactive` (point and box prompts), `segment auto` (jobs with timeout) through `ferrum-engine/1`; engines allow-listed by the operator (loopback only without configuration), token from the environment, results proposed by the engine with `research_only`; tested against the reference server with the mock engine. DICOM export: `export bundle` with `formats: ["dicom"]` writes a binary Segmentation and a Comprehensive 3D SR (TID 1500: lengths and areas with 3D coordinates, segment volumes referencing the SEG), with review status in private codes, rejected items left out and patient data, dates and UIDs per the operator's privacy settings; validated with highdicom in CI. |

---

---

## Next stages 📋

Stages planned but not started, and stages in progress. Their order and
the backlog of smaller tasks are in the [Roadmap](#roadmap) at the end.

## Stage 16 — Annotated datasets 📋

FERRUM as a fast annotation tool for building labelled datasets: people
draw and correct, engines propose, and the export holds only what a
person reviewed. Review status doubles as label quality control: each
label records which engine proposed it and who confirmed it.

| # | User story | Acceptance criteria |
|---|---|---|
| 16.1 | As an **annotator**, I want to confirm or reject all pending proposals at once so that an automatic segmentation with dozens of structures does not need one click per segment. | *Confirm all* in the Review section (optionally filtered by engine or kind); one audit-log entry per item and a single workspace save; a matching use case in `ferrum-app`. |
| 16.2 | As a **researcher**, I want to export only confirmed items so that a dataset never contains unreviewed model output. | `SegmentationSet::confirmed_only` / `AnnotationSet::confirmed_only` in the domain; an *Only confirmed* option for the label-map and annotation exports in the desktop app; `only_confirmed` for `export bundle` (schema, CLI flag, docs, contract tests, identical CLI and MCP output). |
| 16.3 | As an **annotator**, I want a manual correction of an engine's segment to be recorded so that the dataset shows which labels a person edited. | Defined behaviour (and tests) for the provenance of a segment edited with the eraser or brush: the author or status reflects the human edit. |
| 16.4 | As a **dataset curator**, I want an annotation mode in which engine results count as confirmed so that review can happen in a separate QA step outside FERRUM. | Operator setting, off by default; exported provenance still names the engine. |
| 16.5 | As an **annotator**, I want one obvious way to create a segment, offered only where it works, so that I do not get lost between view modes and panels. | Every tool and setting is offered only in the view modes where it acts (tools on slices: 2D and MPR; eraser: 3D; panel tabs per mode), enforced in `ferrum-app`; no segmentation from the 3D view; a *Segment* toolbar group with a built-in region tool that needs no engine and the AI tools (disabled with the reason until an engine connects); a step-by-step guide in the panel and in the view; no empty "Add segment". ✅ |
| 16.6 | As an **annotator**, I want the region tool to segment a whole organ (liver, lung) on noisy CT, not only a small lesion, so that one click gives a usable starting segment. | [16.6-a] The reference value is the median of the smoothed values around the seed, so a noisy seed voxel does not decide (a 200 HU outlier at the seed of a 60 HU organ still yields ≥ 3 500 of the 4 096 voxels); [16.6-b] with ±40 HU noise a 16³ organ phantom is segmented to 3 500–4 300 voxels and a vessel crossing it is filled, also when it leaves the organ through its surface; [16.6-c] a 1 mm bridge to a second structure is cut by an opening of 1.5 mm and leaks without it; [16.6-d] a lesion 3 mm wide survives an opening of 3 mm unchanged (27 voxels); [16.6-e] smoothing of 1 mm does not shrink a sharp-edged ball (895 voxels with and without smoothing, opening and hole filling; strong smoothing recovers only the outermost boundary layer and shrinks small structures — stated in the panel hint); [16.6-i] smoothing and opening are in millimetres: on 5 mm slices a bridge 4 mm wide and one slice thick is cut by an opening of 3 mm and leaks without it; [16.6-k] when the click is not in the core left by the opening (a thin spot), the core nearest to the click survives, never a larger organ elsewhere; [16.6-l] the default tolerance is a tenth of the window width, at most 40 (the automatic window of a whole CT is over 1 000 wide and would leak on the first click); [16.6-m] the click grows the region on a worker thread: the click returns at once (`Viewer::region_pending`), the segment appears when the job finishes; a second click meanwhile is refused with a message and starts no job; *Cancel* (or clearing the segmentation, or opening another study) stops it and no segment is created even if the job had just finished; voxels labelled since the click are never overwritten; the cancel flag is checked every 65 536 grown voxels and between the later steps; [16.6-n] the tolerance is chosen at every click until the user sets it: 300 for an aerated seed (below −500 in a CT-like volume), otherwise as in 16.6-l; the panel shows the value used and a checkbox *Automatic tolerance*; moving the slider switches it off (on MSD lung CT the default of 40 grew nothing and ±200…±400 gives Dice 0.92–0.97 against the lung of the click; field test, `docs/testing.md`); [16.6-j] on real CT the default settings give Dice ≥ 0.90 against the reference on MSD Task03 liver cases (field test; measured on four cases: 0.951, 0.961, 0.958 and 0.883 — the last because the reference includes a hypodense tumour at the surface; see `docs/testing.md`); [16.6-f] the failures are told apart: seed outside the grid, seed already in a segment, region larger than the limit (message names the limit, the tolerance and the opening); the default limit is 8 000 ml (a lung is about 6 000 ml); [16.6-h] a seed on an edge or a vessel is never rejected (the region always contains the seed; "too large" is reported only for the size limit); [16.6-g] the panel offers smoothing (0–6 mm), bridge cutting (0–15 mm), hole filling and a size limit up to 20 000 ml. |
| 16.7 | As a **harness developer** ([#71](https://github.com/Kauter1989/ferrum/issues/71)), I want a person's review decision recorded through a library API, bound to what was shown, so that only the host's decision channel decides and every decision can be checked. | [16.7-a] `ferrum_agent::decisions::revision` gives an item's SHA-256 (64 hex digits) in the fixed encoding `ferrum-review-revision/1` over the source hash, the open volume and the item's content (annotation: kind, plane, slice, points, text, value, name; segment: name and voxels), stable while the item does not change; review state and display properties are not part of it; a pinned test fixes the encoding. ✅ [16.7-b] `record` appends an audit entry with `source: host`, `by`, `revision`, the host's `shown_sha256`, `source_sha256` and the volume, then writes confirm or reject for the named person into the workspace like the desktop review queue; a workspace lock serialises hosts. ✅ [16.7-c] A stale revision (the item's name or voxels changed since it was shown) is refused with `source_changed`, and an item someone has decided on already with `forbidden`; nothing is recorded. An empty, multi-line or bidi-formatted name, a hash that is not 64 hex digits, an unknown item or a workspace outside the root is refused. ✅ [16.7-d] It is not an agent command: no command or schema takes it, and `review confirm`/`reject` still need the operator switch. ✅ |
| 16.8 | As a **harness developer** ([#72](https://github.com/Kauter1989/ferrum/issues/72)), I want to be told when the agent, the viewer or the review panel changes a workspace so that each of them shows the others' changes at once instead of reading everything again. | [16.8-a] `ferrum_io::workspace::changes` gives typed changes: a segment added, removed, revised (name, colour, visibility, opacity or its voxels) or reviewed (with its new status); an annotation added, removed, revised or reviewed; the open volume switched (with its key, e.g. `b1000`, or `other` when the manifest holds a key FERRUM does not generate). [16.8-b] In process, `subscribe(root)` returns a receiver; every save through `Workspace` (agent commands, the viewer, `ferrum_agent::decisions`) sends its changes before the save returns, so a subscriber sees a change made through the agent API in the same event-loop tick. [16.8-c] For changes made by another process, `WorkspaceWatcher::poll` compares the files with the last state it read and gives the same changes; it reads a file again only when its size, modification time or inode changed; a file that cannot be read (not a regular file, too large, malformed) is reported as such and does not stop the other files. [16.8-d] Changes hold labels, ids, statuses and the volume key only: no names, texts, UIDs or paths. A test scans their text for the study's identifiers. [16.8-e] The agent command set does not change. |

**16.8 — clarifications (feature, autonomous; `assumed:`):**
- assumed: a polling watcher (`WorkspaceWatcher::poll`), not OS file notifications, so no new dependency and the same behaviour on Linux, Windows and WSL network drives; the host chooses the interval.
- assumed: a segment's voxels count as revised when its voxels in the label map change; the label map is read again only when its file changed.
- assumed: "reviewed" carries the new status (`proposed`, `confirmed`, `rejected`) but not the reviewer's name, which is a person's name.

**16.6 — success metric:** one click on a liver or a lung of a real CT gives a segment a person only has to trim (Dice against a reference ≥ 0.90 — met on 3 of 4 MSD liver cases, 0.883 on the fourth; lung: one click grows one lung, Dice 0.95–0.97 on 3 of 4 MSD lung cases against a classical, not a human, reference, once the tolerance is ±200 or more — 16.6-n). **Out of scope:** the agent command `segment threshold` (unchanged), an adaptive reference value that follows gradients inside the organ, tolerance chosen automatically, and separating the lung from the trachea.
**16.6 — clarifications:**
- Q: Does the first user report ("region growing does nothing on a click in 2D") have a code cause? — A: not found; the tool worked on phantoms and failed on real noisy data by leaking or by a rejected seed (2026-10).
- assumed: the user's data are CT in HU; the defaults (smoothing 1, opening 1, fill holes on, limit 8 000 ml) are safe for liver and lung and are editable in the panel.

## Stage 17 — Agent segmentation scenarios ✅

An AI agent segments with the engines that already connect to FERRUM
(nnInteractive, TotalSegmentator, MONAI Label) in reviewed, repeatable
scenarios — organ volumetry, lesions by prompts, detect-then-refine,
corrections, follow-up, dataset pre-labelling — on a single 12 GB GPU
(RTX 3080 / 3080 Ti). Design: [docs/agent-segmentation.md](docs/agent-segmentation.md)
([ADR 0010](docs/decisions/0010-agent-segmentation.md)).

| # | User story | Acceptance criteria |
|---|---|---|
| 17.1 | As an **agent developer**, I want to refine an engine segment with further prompts so that an agent can correct a leak or a miss instead of starting again. | `segment interactive` with `segment` and `append`; the prompt history stored with the segment and replayed by the command line, cached by the MCP server (identical JSON); `requested_by` lets the agent rename, refine and delete engine segments it requested, never a person's. ✅ `engine_inputs.json` (`ferrum-engine-inputs` v1); undo; replay tested CLI against MCP. |
| 17.2 | As an **operator**, I want engine calls sized for one 12 GB GPU so that nnInteractive and TotalSegmentator share the card without running out of memory. | ROI uploads for interactive sessions (default: prompts' box + 48 mm); `network.gpu_groups` serialises calls across processes; `engine list`; `name_prefix` and `modality` for engine commands; job progress over MCP. ✅ default ROI = prompts' box + 48 mm with a `roi` check; GPU groups through an advisory file lock; `allow_research_only`, `max_prompts_per_object`. |
| 17.3 | As a **clinician**, I want every engine result checked by fixed rules so that an agent cannot hide a leak, a cut-off organ or a left/right mix-up. | `segment shape`, `components`, `compare`, `edit`; `stats` on a segment within a box; `checks` (empty, size, components, border, laterality, overlap, stability, research flag) on every engine result, with warnings; phantom tests with known answers. ✅ analysis in `ferrum-domain` (`analysis.rs`: components, shape with axial long/short axis, exact distance transform, HD95). |
| 17.4 | As an **operator**, I want the bridges to free GPU memory when idle so that engines can take turns on one card. | TotalSegmentator jobs in a child process; nnInteractive drops its image when the last session closes; `deterministic` in `info` and a replay test in the conformance suite; compose profiles `interactive`, `automatic`, `mixed`. ✅ bridge and conformance tests; `bridges/compose.yml`. |
| 17.5 | As a **harness developer**, I want the skill to teach the segmentation scenarios so that agents use engines safely. | `skills/ferrum/reference/segmentation.md` and a `SKILL.md` section; evaluations with the mock engine (segment and measure, find all objects, remove a leak). ✅ three engine tasks; `ferrum-cli eval engine` serves the mock engine; the grader checks required commands. |
| 17.6 | As an **operator**, I want measured VRAM and times on a 12 GB card so that I can plan the hardware. | Benchmark per engine and scenario; the table in the design filled in. ✅ `scripts/benchmark_engines.py`; measured on an RTX 3080 12 GB (WSL 2): nnInteractive ~1 s per prompt, ~5.4 GB; TotalSegmentator 45–52 s, ~2.2 GB for two organs. |
| 17.7 | As an **agent developer**, I want to correct an existing segment with an engine so that imported or proposed labels can be fixed. | `from_segment` (lasso seeds from the mask) and scribble/lasso prompts in the agent; phantom test. ✅ lassos and scribbles as points on one slice; seeds do not count against the prompt limit. |
| 17.8 | As a **clinician** preparing the measurements staging relies on ([#73](https://github.com/Kauter1989/ferrum/issues/73)), I want the distance from a lesion to a surface and the solid part of a part-solid lesion measured from voxels so that they are reproducible instead of picked by hand. | [17.8-a] `segment distance` gives the shortest distance in mm between the centre of a voxel of `segment` and the centre of a voxel on the outer surface of `to` (`to` with its enclosed holes filled, so a lesion inside a lung segment is measured to the lung's border; a surface voxel has a face neighbour outside it or lies on the volume border), exact, with the two voxels (voxel and LPS mm), `inside` (`segment` within `to` with its holes filled) and `uncertainty_mm` = the largest voxel spacing; two boxes 6 voxels apart along x at 0.8 mm give 4.8 mm; a core in a hole of a lesion, one 2 mm slice from its outer surface, gives 2.0 mm and `inside`; in the domain, a 2-slice gap at 2.5 mm gives 7.5 mm, a sphere inside a sphere gives the brute-force value, and a mask containing the other's surface gives 0. ✅ [17.8-b] An unknown segment is `not_found`; a missing `to` or the same segment twice is `bad_request`. ✅ [17.8-c] `segment shape` with `solid_min` adds `solid`: `min_value`, voxels, ml, share and the axial long and short axis of the segment's voxels at or above it, on their own largest axial slice; there is no default threshold and no category in the output; a threshold above every value gives 0 voxels and no axes; values not known to be HU warn. ✅ [17.8-d] Schema, `commands.json`, docs, skill reference, CLI parse test and contract tests; CLI = MCP; the privacy scan passes. ✅ |

## Stage 18 — Segmentation post-processing: quantitative profiles ✅

Quantitative description of a segment: geometry, shape and HU intensity.
The user picks a **profile**, not individual metrics; a result always
carries the profile name, its version and every preprocessing parameter,
so numbers are comparable and reproducible. Values come from voxels;
agents and engines propose, people confirm (a result for an unconfirmed
segment says so, as the export bundle does).

Design: [docs/radiomics.md](docs/radiomics.md)
([ADR 0011](docs/decisions/0011-radiomics-crate.md)).

**What exists today (0.3.0, Stage 17)** and stays, with unchanged JSON:
`segment shape` (voxels, box, centroid, extent, largest slices, axial long
and short axis, border contact, component count and largest share),
`segment components`, `segment compare` (Dice, Jaccard, HD95, Hausdorff,
centroid distance; `ferrum-domain/src/analysis.rs`), `stats` (mean, std,
min, max, p5–p95 of a box, sphere, segment or area annotation; the maths
sits in `ferrum-agent`, so the desktop app cannot use it), `volume_ml`,
and DICOM SR (TID 1500) export. This stage builds on them: one
implementation shared by UI, CLI and MCP, and what is missing on top.

**Design decisions** (recorded in a new ADR, 0011, before Phase 1):
- Profiles: `quick` and `clinical` (`radiomics-ibsi` comes with Stage 19). Raw values are always
  returned; there are no composite indices.
- Naming and definitions follow IBSI feature families (`shape`,
  `intensity`, `histogram`, `ivh`, later `texture_*`). Every feature has a
  stable id, a unit and a definition text.
- A new use case `Viewer::segment_metrics` in `ferrum-app`; the Segments
  panel, `ferrum-cli` / MCP command `segment metrics` and embedding apps
  are thin adapters. CLI and MCP give identical JSON.
- The computation lives in a new crate `ferrum-radiomics` that depends
  only on `ferrum-domain` (no I/O, GPU or UI). `ferrum-agent` may depend on
  it; CLAUDE.md and `docs/architecture.md` are updated with ADR 0011
  (the user consented to this constitution change, 2026-10-07).
- This stage is Phases 1–3 (`quick` and `clinical`). IBSI preprocessing,
  texture features, comparison and change over time moved to Stage 19
  (below); it is confirmed again (G1) after this stage is released.
- First release scope: CT (HU). MR and PET normalisation are out.

### Phase 1 — Profile framework and basic metrics (`quick`)

| # | User story | Acceptance criteria |
|---|---|---|
| 18.1 | As a **developer**, I want a profile and feature registry so that new metrics plug in without touching the adapters. | [18.1-a] A feature has a stable id, family, unit, definition text and the version it appeared in; ids are unique, and a test fails when a registered feature has no definition text. ✅ (PR 1) [18.1-b] A profile is a versioned list of feature ids; `quick` and `clinical` are registered; an unknown profile gives `bad_request` that lists the known ones. ✅ for `quick` (PR 1); `clinical` is registered in PR 3 [18.1-c] A result carries profile name and version, the preprocessing parameters (even if "none"), per feature a value and unit, and the provenance (author, status) of the segment. ✅ (PR 1) [18.1-d] ADR 0011 written; layering docs updated as decided in Q1. ✅ (PR 1) |
| 18.2 | As a **radiologist**, I want size measures of a segment so that I can describe a finding. | [18.2-a] Volume in mm³ and mL equals voxel count × voxel volume; exact on a 4×5×6 box at spacing 0.7×0.7×2.5. ✅ (PR 1) [18.2-b] Box extent in mm and centroid in patient (LPS) mm, on a grid with a non-identity direction and origin. ✅ (PR 1) [18.2-c] Maximum 3D diameter (largest distance between voxel centres): exact on a box (its space diagonal) and equal to a brute-force search on a random blob. ✅ (PR 4) [18.2-d] Axial long and short axis, extents, largest slices equal `segment shape` (one implementation, tests for both). ✅ (PR 1) [18.2-e] Holds with anisotropic spacing and with a dimension of 1 on each axis (L4). ✅ (PR 1) [18.2-f] An empty or unknown segment gives `not_found` with a hint, never NaN or a crash. ✅ (PR 1) |
| 18.3 | As a **radiologist**, I want first-order HU statistics so that I can characterise density. | [18.3-a] Mean, std (population), min, max, p5, p25, p50, p75, p95 equal the current `stats` output on the existing phantom (regression test). ✅ (PR 1) [18.3-b] Range and IQR added, exact on a hand-computed set. ✅ (PR 1) [18.3-c] Values are rescaled voxel values with their unit; a NIfTI study without a declared modality reports unit `unknown` and a warning (L10). ✅ (PR 1) [18.3-d] The definition text states population std and the percentile method. ✅ (PR 1) [18.3-e] HU-based features of a study whose modality is unknown (NIfTI) are refused with `bad_request` and a hint to declare it (`--modality CT`); no value is computed with a guessed unit. ✅ (PR 1) [18.3-f] The desktop app asks for the modality (and so the value unit) when it opens a NIfTI file that does not declare one, shows it afterwards, and keeps it in the workspace manifest like the CLI does. ✅ (PR 2) |
| 18.4 | As a **user**, I want the `quick` profile in the desktop app, CLI and MCP so that I get the same numbers everywhere. | [18.4-a] `segment metrics --profile quick`: schema in `schema.rs`, `skills/ferrum/schemas/commands.json` regenerated, `skills/ferrum/reference/commands.md` and `docs/agent-cli.md` updated. ✅ (PR 1) [18.4-b] CLI parse test and one `--help` run (L8). ✅ (PR 1) [18.4-c] CLI and MCP JSON are identical (contract test). ✅ (PR 1) [18.4-d] The output passes the identifier scan (`tests/privacy.rs`). ✅ (PR 1) [18.4-e] Segments panel card shows value, unit and definition tooltip per feature, and a *proposed* label for an unconfirmed segment; `cargo test -p ferrum --test ui` passes (L9). ✅ (PR 2) [18.4-f] With no card open the panel says how to open one; when the image type is not known the card says so and leads to the question instead of showing numbers. ✅ (PR 2) [18.4-g] `--describe` (CLI) and `describe` (MCP) add the definition text of every feature to the result; without it the result has none. ✅ (PR 1) |

### Phase 2 — Shape and morphology (`clinical`, part 1)

| # | User story | Acceptance criteria |
|---|---|---|
| 18.5 | As a **radiologist**, I want surface area and compactness measures so that I can judge shape. | [18.5-a] Mesh-based surface area (marching cubes, anisotropic spacing), closed and wound outwards for every cube pattern; a single voxel is an octahedron (area √3, volume 1/6 mm³ exactly). A mesh of a binary mask is rougher than a smooth surface: measured on a ball of radius 10 mm, the area is 7.4 % above 4πr² at 1 mm voxels and 6.6 % at 0.5 mm (not converging to 1), the volume within 1 %. ✅ (PR 3) [18.5-b] Surface-to-volume ratio, compactness 1 and 2, sphericity, spherical disproportion, with the formulas in the definition text. ✅ (PR 3) [18.5-c] The same ball voxelised at 1 mm and at 0.5 mm gives areas within 2 % of each other (measured 0.7 %); a change of spacing scales the mesh exactly. ✅ (PR 3) [18.5-d] A single voxel, a line and a plate give finite values (a sphericity in (0, 1]), never NaN. ✅ (PR 3) |
| 18.6 | As a **radiologist**, I want PCA axes, elongation and flatness. | [18.6-a] Axis lengths 4√λ from the eigenvalues of the population covariance of the voxel centres in patient mm (IBSI definition; 4a/√5 for an ellipsoid with semi-axis a); an ellipsoid phantom with semi-axes 5, 12, 8 mm within 3 % at 1 mm and at 2 mm slices. ✅ (PR 3) [18.6-b] Elongation and flatness are ratios of those axes. ✅ (PR 3) [18.6-c] A line, a plate and a single voxel: axes with no extent are exactly zero (not noise) and the ratios are 0; a single voxel has no extent at all and its elongation and flatness are `null` with a reason (L4). ✅ (PR 3) [18.6-d] Orientation vectors in LPS (nine features), sign-normalised (the largest component positive, a tie by the first axis) so that the result is deterministic; a direction is `null` with a reason when two axes are equally long or the body has no extent along it. ✅ (PR 3) |
| 18.7 | As a **radiologist**, I want convex-hull measures and solidity. | [18.7-a] Hull volume and area, solidity (mesh volume / hull volume) and convexity (hull area / mesh area; at most 1 for a connected body, larger for separate pieces). ✅ (PR 4) [18.7-b] A box gives solidity 1 and convexity 1; two voxels three steps apart give solidity 0.2, a hull volume of 10/3 and a convexity of √5 + ½ by hand; the hull of an L-shaped phantom is 11/3, derived by hand in the test (amended: the L has no closed form for solidity because its mesh volume is not hand-derivable, so the hull volume is the hand value). ✅ (PR 4) [18.7-c] Hull on mesh vertices, not on every voxel, so the cost follows the surface (the interior points of a cloud do not become vertices). ✅ (PR 4) |
| 18.8 | As a **QA reviewer**, I want connectivity checks so that I notice a broken or holey segment. | [18.8-a] Component count and largest-component share equal `segment components`. ✅ (PR 4) [18.8-b] Holes: number and volume of cavities not connected to the border (6-connected); a box with a 2×2×2 cavity gives 1 hole of 8 voxels. ✅ (PR 4) [18.8-c] Euler number (26-connected foreground: 1 for a box, 2 with a cavity, 0 for a ring). ✅ (PR 4) [18.8-d] `warnings` (`fragmented`, `has_holes`, `touches_border`) with their thresholds in the definition text; warnings never change the mask — edits stay with `segment edit`. Thresholds: `fragmented` below 95 % of the voxels in the largest piece, `has_holes` from one cavity, `touches_border` from one image side. ✅ (PR 4) |

### Phase 3 — HU distribution (`clinical`, part 2)

| # | User story | Acceptance criteria |
|---|---|---|
| 18.9 | As a **radiologist**, I want distribution measures. | [18.9-a] Skewness, excess kurtosis, energy, RMS, MAD, rMAD (population versions, on the voxel values), entropy and uniformity (on a fixed-bin-width histogram, bins anchored at 0 HU: `[k·w, (k+1)·w)`; IBSI anchors at the ROI minimum, so entropy can differ from a strict IBSI tool and the field test will show how much). ✅ (PR 5) [18.9-b] A hand-computed 10-voxel set gives the exact values. ✅ (PR 5) [18.9-c] Bin width is a parameter, defaults to 25 HU for CT, and is recorded in the result. ✅ (PR 5) [18.9-d] A constant region gives `null` for skewness and kurtosis, not NaN. ✅ (PR 5) |
| 18.10 | As a **radiologist**, I want intensity-volume histogram metrics. | [18.10-a] V10, V90 (fraction of the volume at or above 10 % / 90 % of the way from the lowest to the highest value), I10, I90 (the highest value that at least 10 % / 90 % of the volume reaches: the ⌈x·N/100⌉-th largest) and the curve (101 samples), all on the voxel values, not binned. ✅ (PR 5) [18.10-b] A linear-ramp phantom gives exact values. ✅ (PR 5) [18.10-c] The curve never increases (property test). ✅ (PR 5) |
| 18.11 | As a **radiologist**, I want the share of voxels in HU ranges. | [18.11-a] Configurable half-open ranges; the fractions plus the share outside all ranges sum to 1 within 1e-12. ✅ (PR 5) [18.11-b] Overlapping ranges give `bad_request`. ✅ (PR 5) [18.11-c] The ranges are a parameter. In the desktop app they are taken from the interface fields (the person edits them); an agent passes its own ranges or leaves them out and gets the documented defaults, which are marked as defaults and not as clinical advice (agent half ✅ PR 5; the interface fields ✅ PR 6: bin width and a list of ranges, applied with a button, validated with the reason shown). [18.11-d] The ranges used are recorded in the result. ✅ (PR 5) |
| 18.12 | As a **user**, I want the `clinical` profile everywhere. | [18.12-a] The card groups features by family (Size, Shape, Connectivity, Intensity) with units and definition tooltips, shows the warnings, the share of voxels by HU range and the intensity-volume histogram. ✅ (PR 6) [18.12-b] CLI, MCP, schema, docs and contract tests as in 18.4. ✅ (PR 5) `clinical` stays `0.x` (now 0.3) until the field test against pyradiomics has been read; it becomes `1` with the release of Phase 3. [18.12-c] The benchmark of the profile on a 512×512×300 CT with a segment of 100 000 voxels is recorded. ✅ (PR 6) Re-done after the field test: the first benchmark (a smooth ball, 607 ms) hid that real, frayed masks took 16.7 s (blobby box fill) to 87 s (893 000 voxels, field test) with the first convex hull; with Quickhull `clinical` takes 53 ms (ball), 187 ms (blobby, 101 752 voxels) and 1.3 to 2.5 s for masks of about a million voxels; see `docs/radiomics.md` §3.2. |
| 18.13 | As an **analyst**, I want the voxel values of a CT in HU read back exactly so that a measurement is the number the file holds (found by the field test: a voxel of 55 HU read as 55.01). [ADR 0012](docs/decisions/0012-lossless-grid-storage.md) | [18.13-a] Data whose values are all multiples of the storage step above the minimum (whole HU of a CT, halves of a slope-one-half series) are stored and read back exactly, from NIfTI, DICOM (rescale applied, MONOCHROME1 inverted) and `Volume::from_physical`. ✅ [18.13-b] Data that are not on the grid (float images, a span of more than 65 535 steps, a constant image, no finite value) keep the data range and the old precision. ✅ [18.13-c] The range is the data range or at most twice as wide, never narrower; windows and presets (physical units) are unchanged and the GPU parity tests pass unchanged. ✅ (the whole suite including the GPU parity tests on lavapipe passes unchanged) [18.13-d] `probe`, `stats` and `segment metrics` return the whole units of the file (a minimum of −25 is −25). ✅ [18.13-e] A threshold at a whole unit includes the voxels of exactly that unit. ✅ [18.13-f] Field: pyradiomics agrees with FERRUM on the "exact" intensity features to 1e-6 on the MSD spleen (after the next field run). |

**Implementation** (Phases 1–3; sizing and tasks in
[docs/radiomics.md](docs/radiomics.md) §7–8):

| PR | Content | State |
|---|---|---|
| 1 | `ferrum-radiomics`, registry, `quick`, `segment metrics` (3D diameter moved to PR 4) | ✅ [#50](https://github.com/Kauter1989/ferrum/pull/50) |
| 2 | desktop modality dialog and metrics card | ✅ [#51](https://github.com/Kauter1989/ferrum/pull/51) |
| 3 | mesh shape: area, compactness, sphericity, PCA; profile `clinical` 0.1 | ✅ [#52](https://github.com/Kauter1989/ferrum/pull/52) |
| 4 | convex hull, 3D diameter, holes, Euler, warnings; profile `clinical` 0.2 | ✅ [#53](https://github.com/Kauter1989/ferrum/pull/53) |
| 5 | distribution, IVH, HU ranges; `clinical` 0.3 complete in CLI and MCP | ✅ [#54](https://github.com/Kauter1989/ferrum/pull/54) |
| 6 | clinical card, HU-range fields, benchmark | ✅ [#55](https://github.com/Kauter1989/ferrum/pull/55) |
| — | field-test fixes (Quickhull, HU ranges) | ✅ [#56](https://github.com/Kauter1989/ferrum/pull/56) |
| — | lossless grid storage (18.13, [ADR 0012](docs/decisions/0012-lossless-grid-storage.md)) | ✅ [#57](https://github.com/Kauter1989/ferrum/pull/57) |

Open: 18.13-f (pyradiomics against FERRUM on the "exact" intensity
features, MSD spleen) waits for the next field run on the user's machine.

**Success metrics:**
- Every feature of Phases 1–3 matches an analytic or hand-computed value on a phantom, exactly or within the stated tolerance.
- Field test (Phases 1–3): the values for a public segmented CT agree with a reference implementation (pyradiomics) on the same mask within the stated tolerances; differences are explained in the PR.
- Performance: the `clinical` profile ≤ 2 s on the 100 000-voxel segment above. ✅ Measured: 53 to 228 ms for 100 000 voxels on a ball, a ragged ball and a blobby box fill; 1.3 to 2.5 s for a million voxels (PR 6 and the field-test fix).

**Out of scope:** GUI localisation (a separate task, added to the plan later); IBSI preprocessing, texture features, extra comparison measures and change over time (Stage 19); MR and PET normalisation, SUV; automatic registration; classification, prediction and feature selection; composite indices (irregularity, homogeneity, density); GPU computation; bundling pyradiomics (it is only a field-test reference).

**Clarifications:**
- Q: profiles or single metrics? — A: profiles (variant D), IBSI names, tiered delivery (2026-10-03).
- Q: how many phases? — A: six (2026-10-03).
- Q: are explanations of the texture matrices needed? — A: mandatory, a tooltip at least (2026-10-03).
- Q1: where does the computation live? — A: a new crate `ferrum-radiomics`; the constitution change (ferrum-agent dependency rule) is consented (2026-10-07).
- Q2: which phases does this stage promise? — A: 1–3 now; the rest moved to Stage 19 and is confirmed again afterwards (2026-10-07).
- Q3: NIfTI without a modality? — A: HU-based features are refused until the modality is declared, and the format must be clarified when the data is loaded, in the app too (2026-10-07).
- Q4: HU ranges and localisation? — A: GUI localisation is a separate task, to be added to the plan later. Ranges for the skill are formed separately or left as documented defaults; in the GUI they come from the interface (2026-10-07).
- Q5: field test data? — A: MSD Task09 spleen with pyradiomics as the reference (2026-10-07).

## Stage 19 — Reproducible radiomics and comparison 📋

The second half of the quantitative profiles started in Stage 18: the
`radiomics-ibsi` profile (explicit preprocessing, IBSI conformance,
stability), texture features with explanations in the interface, and
comparison and change over time. It builds on `ferrum-radiomics`
([ADR 0011](docs/decisions/0011-radiomics-crate.md),
[docs/radiomics.md](docs/radiomics.md)) and starts only after Stage 18
(its Phases 1–3) is released. The spec is confirmed again (G1) before its
design.

What exists today (0.3.0, Stage 17) and stays: `segment compare` (Dice,
Jaccard, HD95, Hausdorff, centroid distance) and the DICOM SR (TID 1500)
export. The rows below moved here from Stage 18 on 2026-10-07 (the user's
decision), so that Stage 18 closes after its Phase 3.

### Phase 1 — Reproducible radiomics (`radiomics-ibsi`, preprocessing)

| # | User story | Acceptance criteria |
|---|---|---|
| 19.1 | As a **researcher**, I want explicit preprocessing so that results compare across centres. | [19.1-a] Isotropic resampling (voxel size parameter), HU re-segmentation range and outlier exclusion. [19.1-b] Upsampling a linear ramp by 2 keeps its values at the original positions. [19.1-c] All parameters are stored in the result with a SHA-256 `params_hash`. [19.1-d] The same input and parameters give byte-identical JSON. [19.1-e] The loaded volume and the label map are never modified. |
| 19.2 | As a **researcher**, I want IBSI conformance checks. | [19.2-a] The IBSI digital phantom is built in the test (no data file committed). [19.2-b] Per implemented feature a reference value and a tolerance from the IBSI reference manual. [19.2-c] `docs/radiomics-conformance.md` lists every feature as conformant or not. |
| 19.3 | As a **researcher**, I want to know how stable a metric is against boundary changes. | [19.3-a] Optional 1-voxel erosion and dilation of the mask; per feature the spread is reported. [19.3-b] The perturbation is deterministic. |

### Phase 2 — Texture features (`radiomics-ibsi`, part 2)

| # | User story | Acceptance criteria |
|---|---|---|
| 19.4 | As a **researcher**, I want GLCM and GLRLM features. | [19.4-a] 13-direction 3D matrices; the IBSI aggregation method is selectable and recorded. [19.4-b] Reference values on the IBSI phantom as in 19.2. |
| 19.5 | As a **researcher**, I want GLSZM, GLDM and NGTDM features. | [19.5-a] Same conformance approach as 19.2. [19.5-b] Within the complexity budget in `clippy.toml`; a benchmark is recorded (budget measured, L13). |
| 19.6 | As a **researcher**, I want optional filters (LoG, wavelet). | [19.6-a] Parameters recorded; off by default. [19.6-b] Conformance where IBSI gives references. |
| 19.7 | As a **user**, I want every texture matrix family explained where I see it so that I know what a number means and why it is computed. | Mandatory for 19.4–19.5 (no texture feature ships without it). [19.7-a] Each family has a tooltip in the UI, at least on its group header and per feature where practical. [19.7-b] A tooltip gives the full name, what the matrix counts, what it says about tissue and a caveat. [19.7-c] The texts live once, in the feature registry, so UI, CLI/MCP (`description` field, in the schema) and docs show identical wording; a test fails when a family or feature has no text. [19.7-d] Content: **GLCM** (Gray Level Co-occurrence Matrix) — how often pairs of voxels with given grey levels sit at a given distance and direction; contrast, homogeneity, correlation. **GLRLM** (Gray Level Run Length Matrix) — lengths of runs of equal grey level along a direction; coarse versus fine texture. **GLSZM** (Gray Level Size Zone Matrix) — sizes of connected zones of equal grey level; zone heterogeneity, direction-independent. **GLDM** (Gray Level Dependence Matrix) — how many neighbours share a voxel's grey level within a tolerance; local uniformity. **NGTDM** (Neighbouring Gray Tone Difference Matrix) — difference between a voxel and the mean of its neighbourhood; coarseness, busyness, complexity. [19.7-e] Each text says that values depend on bin width and preprocessing (recorded in the result) and are for research, not diagnosis. [19.7-f] Texts are in English; GUI localisation is a separate task (out of scope here). Who reviews the texts is decided when Phase 5 is confirmed. |

### Phase 3 — Comparison and change over time

| # | User story | Acceptance criteria |
|---|---|---|
| 19.8 | As a **clinician**, I want to compare two segmentations so that I can check an agent or engine against a person. | Dice, Jaccard, HD95, Hausdorff and centroid distance exist (`segment compare`). [19.8-a] Added: mean surface distance and volume difference in mL and %; checked against a brute-force search on shifted balls (as in Stage 17). [19.8-b] The desktop app shows the agreement next to Confirm / Reject for a segment that is not confirmed, against a reference segment the person picks. [19.8-c] Nothing is confirmed automatically. |
| 19.9 | As a **clinician**, I want the change of a segment between two studies. | [19.9-a] Volume and HU change, with both studies' provenance. [19.9-b] The result says "not registered" and claims no alignment. [19.9-c] Segments are matched by a pair the person chooses. [19.9-d] A modality or unit mismatch gives `bad_request`. [19.9-e] No identifiers leave FERRUM (pseudonymised UIDs as configured). |
| 19.10 | As an **integrator**, I want metrics in exports. | [19.10-a] The export bundle includes the metrics result with profile, version and parameters. [19.10-b] Unconfirmed items are marked, as in the bundle today. [19.10-c] The identifier scan passes. [19.10-d] The DICOM SR (TID 1500) export (exists) carries the volume and the metrics it can express; the rest stays in `report.json`. |

**Success metrics:**
- Every implemented feature is within the IBSI tolerance on the digital phantom, or is listed as not conformant.
- Texture and preprocessing values for a public segmented CT agree with pyradiomics on the same mask within the stated tolerances; differences are explained.

**Out of scope:** GUI localisation (a separate task); MR and PET normalisation, SUV; automatic registration; classification, prediction and feature selection; composite indices.

**Clarifications:**
- Q: are explanations of the texture matrices needed? — A: mandatory, a tooltip at least (2026-10-03).
- Q: what happens to Phases 4–6 of Stage 18? — A: they moved to this stage; the spec is confirmed again before the design (2026-10-07).
- open: who reviews the texture texts (19.7).

## Stage 20 — Multi-volume MR and PET series ✅

MR and PET series often hold several volumes: echoes, diffusion
b-values, dynamic phases, PET frames. Since 2.7 the loader shows one
volume and says so. It still reads Enhanced multi-frame files (the format
of current MR and PET scanners) without their per-frame geometry: frames
are stacked in file order with the spacing from the header. This stage
reads that geometry (20.1, a loader fix without a contract change), then
lets a person or an agent choose which volume to show (20.2–20.3, a
change of the agent's command set: gate G1 before design).

| # | User story | Acceptance criteria |
|---|---|---|
| 20.1 | As a **clinician** reading an Enhanced MR or PET file, I want its frames placed where the scanner put them so that distances and volumes are right. | [20.1-a] An Enhanced multi-frame file is ordered by the per-frame Image Position (Patient) (Plane Position Sequence) along the normal of its Image Orientation (Plane Orientation Sequence, shared or per frame); the slice spacing is the median gap between positions, the origin is the first plane's position. Holds for frames stored in any order. ✅ [20.1-b] Pixel spacing and slice thickness come from the Pixel Measures Sequence (shared or per frame) when the top level has none. ✅ [20.1-c] Frames at one position (several temporal positions, echoes or b-values) give one volume, chosen like 2.7-a by Temporal Position Index, Echo Numbers or Effective Echo Time, Diffusion b-value, and frame order; the load warns as in 2.7-b. ✅ [20.1-d] Per-frame rescale (Pixel Value Transformation Sequence) is applied to every frame. ✅ [20.1-e] Such a file loads without the 2.7-d warning about file order; a multi-frame file without per-frame positions keeps it. Several Enhanced files of one series are ordered together by their frames' positions. ✅ [20.1-f] The series key (orientation) is read from the functional groups, so frames of one Enhanced file are one series. ✅ |
| 20.2 | As a **clinician**, I want to choose which volume of a multi-volume series I see (echo, b-value, phase) and switch without reopening the study, so that I can read the one I need. | [20.2-a] Opening a multi-volume series (DICOM or 4D NIfTI) shows volume 1 and a Volume row listing every volume by name: `b = 1000`, `TE = 80 ms` (or `echo 2`), `phase 3/20`, `frame 3/33, t = 60 s`; `volume 2/5` when nothing names it. ✅ (PR 3) [20.2-b] Switching keeps the slice position, zoom, view mode, segments and annotations; the window follows the new volume's auto window unless the person set one; the load runs in the background, and a volume loaded before switches immediately (cache of 1 GiB of voxels, least recently used out). ✅ (PR 3) [20.2-c] After a switch, segment metrics of another volume are marked out of date; every metrics result names its volume. ✅ (PR 3) [20.2-d] The series picker lists series, not volumes: one multi-volume series opens without a picker. ✅ (PR 3) [20.2-e] A volume missing slices is listed but disabled, with the reason. ✅ (PR 3) |
| 20.3 | As an **agent developer**, I want `study scan` and `study open` to list and open one volume of a multi-volume series so that an agent can work on a b = 1000 or a late phase. | [20.3-a] `study scan` lists one entry per volume with `volume` (index, count, label, and b-value, echo time, echo number, temporal position, frame time when known) and `group` (pseudonymised id of the series); single-volume entries are unchanged. ✅ (PR 2) [20.3-b] `study open` with a volume id opens that volume; with the group id, or without `series` when the source holds one multi-volume series and nothing else, it opens volume 1 and warns with the number of volumes; two distinct series still need `series`. ✅ (PR 2) [20.3-c] A workspace belongs to the series: opening another volume of the same series in it switches the volume (audit entry), keeps segments and annotations; another series is refused as today. ✅ (PR 2) [20.3-d] `study info` names the current volume and lists the series' volumes with their ids and completeness; the provenance of `probe`, `stats`, `profile`, `measure`, `segment metrics` and `view` names the volume. ✅ (PR 2) [20.3-e] CLI and MCP give identical JSON; schema, `commands.json`, docs and contract tests are updated; the privacy scan passes. ✅ (PR 2) [20.3-f] The workspace manifest records `source.volume` (version 2, written only for a multi-volume series); a version 1 manifest opens as volume 1. ✅ (PR 2) |
| 20.4 | As a **clinician**, I want each volume of a series to hold exactly its own images so that a volume never mixes echoes, b-values or phases. | [20.4-a] DICOM images at each position are split into volumes by the fields that differ between images at one position (Complex Image Component, Temporal Position, Echo Numbers, Effective Echo Time, b-value, Trigger Time, Frame Reference Time, Acquisition Number, and whether a diffusion image is the derived isotropic trace: Diffusion Directionality `ISOTROPIC`, or a zero gradient direction at b > 0); each volume holds one image per position; when no field differs, the k-th image at each position (by Instance Number, then file order) forms volume k; a key that still holds several images at one position (diffusion directions at one b-value) is split the same way and numbered (`b = 1000, 2/32`). ✅ (PR 1) [20.4-b] A classic dynamic PET series is split by Frame Reference Time and named `frame k/N, t = … s`. ✅ (PR 1) [20.4-c] A 4D NIfTI gives one entry per volume, named from BIDS sidecars next to it when present (`.bval`; PET `.json` `FrameTimesStart`). ✅ (PR 1) [20.4-d] Loading one volume decodes only its images (frames of an Enhanced file), and its warnings no longer say that one volume of several is shown. ✅ (PR 1) |

**Success metrics:** an Enhanced MR or PET series from the user's scanner (field test) loads with the spacing and origin that the scanner's own viewer shows, within 0.01 mm.

**Out of scope:** 4D viewing (playing phases), registration between volumes, ADC or other derived maps, NIfTI-2, PET units and SUV (a separate stage).

**Clarifications:**
- assumed (20.1): a file with per-frame positions is ordered like a series of single-frame files (2.2) and goes through the same choice of one image per position (2.7-a); no new warning kind.
- Q: one scan entry per volume, or volumes inside one entry? — A: one entry per volume (2026-10-14).
- Q: which attributes name a volume? — A: b-value, echo time, phase (2026-10-14).
- Q: should the desktop app switch volumes without reloading? — A: yes (2026-10-14).
- Q: design (G2)? — A: approved (2026-10-14).
- assumed (20.2–20.4): a workspace belongs to the series, not one volume (ADR 0013); vendor-private b-value tags are not read (raw tag literals are forbidden), such volumes are named `volume k/N`.

**Design:** [docs/multi-volume.md](docs/multi-volume.md), [ADR 0013](docs/decisions/0013-multi-volume-series.md).

### Implementation

| Task | Criteria | PR | State |
|---|---|---|---|
| 20.1 Enhanced per-frame geometry | 20.1-a…f | [#60](https://github.com/Kauter1989/ferrum/pull/60) | ✅ |
| 20.4.1–20.4.2 partition volumes (DICOM, NIfTI) | 20.4-a…d | [#62](https://github.com/Kauter1989/ferrum/pull/62) | ✅ |
| 20.3.1–20.3.3 agent and workspace | 20.3-a…f | [#63](https://github.com/Kauter1989/ferrum/pull/63) | ✅ |
| 20.2.1–20.2.2 desktop switching | 20.2-a…e | [#64](https://github.com/Kauter1989/ferrum/pull/64) | ✅ |
| fix: DICOM brought into LPS (2.8), found by the 20.1 field check | 2.8 | [#61](https://github.com/Kauter1989/ferrum/pull/61) | ✅ |

**Field check (2026-10-15 and 2026-10-17, public data):** volumes named
and matched against dcm2niix (r = 1.0) on the `dcm_qa` sets: Siemens
multi-echo, Philips and Canon DTI (classic and Enhanced, with the trace
image), Philips magnitude and phase, Philips fMRI phases; a 33-frame
OpenNeuro PET NIfTI named from its BIDS sidecar. Agent: scan, open, switch
and provenance on the same data. Desktop: switching in 0.01 s (DICOM) and
1.1–1.3 s per new PET frame (the `.nii.gz` is decompressed), 0 s from the
cache. The desktop check found two bugs, fixed in #64 (lesson L24).

**Open (field, user's machine):** the success metric (an Enhanced series
from the user's scanner within 0.01 mm of the scanner's viewer), and the
switch time and GPU memory on a real GPU.

---

## Stage 23 — GPU sharing with an external model server 🚧 (G1 ✅, G2 ✅; PR 3 of 3)

Medical Harness runs its language model locally as a `llama-server`
sidecar on the same 12 GB card as FERRUM's engines
([#68](https://github.com/Kauter1989/ferrum/issues/68); P0 for the
harness's Demo 2: live segmentation next to a resident model). What exists:
- `network.gpu_groups` runs one engine call of a group at a time through
  an advisory file lock (`GpuLock` in `ferrum-agent/src/sessions.rs`).
  The file is in the shared temporary folder (`/tmp/ferrum-gpu-<group>.lock`),
  so another local user can pre-create or hold it. The format is
  undocumented, and the lock is a mutex that cannot tell a resident model
  from a heavy engine.
- The desktop app's AI tools (`ferrum-app/src/viewer/ai.rs`) do not take
  the lock at all.
- The nnInteractive bridge already frees the image and empties PyTorch's
  cache when a session closes (`NnInteractiveBackend.release`), but nobody
  has measured it. §2 of [agent-segmentation.md](docs/agent-segmentation.md)
  still lists that as to do. The weights and the CUDA context stay loaded
  (part of the ~2.3 GB baseline), and nothing can unload them.

This stage lets a process outside FERRUM hold part of the card, makes
FERRUM wait for room instead of running out of memory, and lets idle
bridges give their memory back. It touches the engine protocol, the agent
command set, the bridges and the desktop app, so it is gated and split
into PRs.

| # | User story | Acceptance criteria |
|---|---|---|
| 23.1 | As a **harness developer**, I want a documented GPU-group lock that my own process can take so that my model server and FERRUM's engines never use the card at the same time beyond its size. | [23.1-a] `docs/gpu-sharing.md` specifies the protocol `ferrum-gpu-lock/1`: directory, file names per group and holder, the OS lock used (`flock` on Unix, `LockFileEx` on Windows), the holder record (holder name, process id, leased MiB, start time; no study data), how to acquire and release, the timeout, and stale holders. [23.1-b] A permissive crate `ferrum-gpu-lock` (MIT OR Apache-2.0, no FERRUM crate, only `rustix` on Unix for the user id) implements it, and `ferrum-agent` uses it instead of its own `GpuLock`. [23.1-c] The lock directory is per user: `$XDG_RUNTIME_DIR/ferrum/gpu` on Linux, `%LOCALAPPDATA%\ferrum\gpu` on Windows, or `network.gpu_lock_dir`; it is created with mode 0700 (without `XDG_RUNTIME_DIR`: `~/.cache/ferrum/gpu`). A directory or lock file owned by another user, or a directory that group or others can write, is refused with an error that names the path; FERRUM never falls back to `/tmp`. [23.1-d] Two-process test: an external holder holds the group; `segment auto` (mock engine) waits and succeeds within 1 s after the holder releases. When the holder is killed (`SIGKILL`) instead, the call goes ahead within 1 s. [23.1-e] A wait longer than `engine_job_timeout_s` ends with `engine_unavailable`, naming the holder and its lease; over MCP the wait reports progress ("waiting for GPU group card: llama-server holds 6 144 MiB"). [23.1-f] A group without a capacity (23.2-d) behaves as today: one holder at a time, external holders included. |
| 23.2 | As an **operator**, I want a holder to declare how much of the card it holds so that FERRUM runs an engine next to a resident model when it fits, and waits when it does not. | [23.2-a] A holder takes a lease of N MiB. An engine call of the group goes ahead only when the leases of the live holders plus its expected peak fit in the group's capacity; otherwise it waits as in 23.1-d/e. FERRUM's own engine calls of a group still run one at a time (rule 1 of §2). [23.2-b] The expected peak of an engine comes from the operator (`network.gpu_peaks_mib`) and from the engine's `GET /v1/info` (`limits.expected_peak_mib`, optional, additive to `ferrum-engine/1`). The engine's value may raise the operator's but never lower it (the effective peak is the larger). An engine with neither takes the whole group. [23.2-c] Test: capacity 12 000 MiB with a 6 000 MiB lease. A call with a 7 000 MiB peak waits, then goes ahead once the lease is released; a call with a 4 000 MiB peak goes ahead at once. [23.2-d] The capacity of a group is set by the operator (`network.gpu_capacity_mib`). An expected peak larger than the capacity runs only when the group has no other holder, with a warning. [23.2-e] Lock records and outputs pass the identifier scan; no study path, UID or name is written to the lock directory. |
| 23.3 | As a **clinician** using the desktop app's AI tools, I want them to take the same GPU group as the agent so that the viewer never starts an engine next to a model that leaves no room. | [23.3-a] The desktop app reads the operator's `[network]` GPU settings from the same configuration file as the agent and takes the lease for an engine of a group before every request that may use the GPU. [23.3-b] While it waits, the AI panel shows "Waiting for GPU: <holder> (pid N) holds M MiB (group G)" with Cancel. Cancel releases nothing it does not hold, and the request is not sent. [23.3-c] UI test: an external holder makes the panel show the waiting state, and releasing it lets the request run. |
| 23.4 | As an **operator**, I want idle bridges to give their GPU memory back, completely when asked, and to see what each bridge holds, so that a 12 GB card has room for TotalSegmentator `total` (about 10 GB guided) next to a resident model. | [23.4-a] After the last session of the nnInteractive bridge closes (`DELETE` or TTL expiry), the bridge's GPU memory falls to its weights-only level ± 200 MiB, measured with `nvidia-smi --query-compute-apps` on the user's card (field). A bridge unit test checks that both paths drop the image and empty the cache. [23.4-b] A bridge that supports it reports `capabilities.unload: true` and serves `POST /v1/unload` (additive to `ferrum-engine/1`, in the conformance suite). The endpoint is refused with `busy` while a session or job is active. Afterwards the bridge holds no GPU memory at all (no CUDA context, measured), and its next session reloads the model; the reload time is measured and documented. [23.4-c] `engine unload [--engine URL \| --group G]` asks every idle bridge (of the group) to unload. The output lists, per engine, `unloaded`, `busy` or `unsupported`; it never stops a running job. CLI and MCP give identical JSON, and the schema, `commands.json`, docs and contract tests are updated. [23.4-d] `GET /v1/info` may report `resident_mib` (optional), and `engine list` shows it per engine marked as reported by the engine. The docs say that a holder checks free memory itself (NVML) instead of trusting it (point 7 of #68). |
| 23.5 | As an **operator**, I want a measured recipe for a 12 GB card with an LLM sidecar so that I can size the model and the engines without trying. | [23.5-a] `scripts/benchmark_engines.py --external-holder "<command>" --lease-mib N` starts the holder, samples GPU memory during every step, and reports the combined peak and the time each call waited. [23.5-b] Field run on the user's card with the harness's `llama-server` and nnInteractive plus TotalSegmentator: 20 engine calls in Demo 2's order without CUDA out-of-memory errors. The table goes to §2 of agent-segmentation.md and the recipe to `docs/gpu-sharing.md`; numbers are measured or marked as estimates (L13). [23.5-c] Rule 2 of §2 is updated to what the bridge does, and a new ADR (0015) records the lock protocol, the crate and the lease. [23.5-d] An operator without the harness can wrap a sidecar: `ferrum-cli gpu hold --group G --lease-mib N -- <command>` holds the lease while the command runs and releases it when the command exits; it is never an MCP tool. |

**Success metrics:** on the user's RTX 3080 Ti 12 GB, with `llama-server`
holding the harness's model (its VRAM measured in the run), Demo 2's sequence (interactive lesion, refinement, `total` on two
organs) runs 20 engine calls with no CUDA out-of-memory errors. Idle
nnInteractive returns to its weights-only level within 2 s of the last
`DELETE`.

**Out of scope:** scheduling across machines or several GPUs per group,
MIG/MPS, AMD/ROCm and Apple GPUs (the lease is still honoured, but
nothing is measured there), NVML inside FERRUM, starting or stopping
`llama-server` (its lifecycle stays with the harness), preempting a
running engine job.

**Clarifications:**
- Q1: a crate or only a documented protocol? — A: a crate and the protocol document (2026-10-23).
- Q2: leases for external holders, FERRUM's own calls still one at a time? — A: yes (2026-10-23).
- Q3: full unload through a worker child process in the bridge, refused while busy, through `engine unload`? — A: yes (2026-10-23).
- Q4: does the desktop app take part in this stage? — A: yes, in PR 3 (2026-10-23).
- Q5: `ferrum-cli gpu hold` for operators without the harness? — A: yes (2026-10-23).
- Q6: hardware? — A: RTX 3080 Ti 12 GB (2026-10-23). The harness's model, quantisation and context were not given. They are recorded with their measured VRAM in the field run rather than assumed (L13).
- Q7: lock directory without `XDG_RUNTIME_DIR`? — A: `~/.cache/ferrum/gpu`, 0700, owner checked; never `/tmp` (2026-10-23).

**Design:** [docs/gpu-sharing.md](docs/gpu-sharing.md), [ADR 0015](docs/decisions/0015-gpu-lease-protocol.md) (G2 ✅ 2026-10-23).

### Implementation

| Task | Criteria | PR | State |
|---|---|---|---|
| 23.T1 `ferrum-gpu-lock`: directory checks, holder records, admission, stale holders | 23.1-a/b/c/f, 23.2-a/c/d | 1 | ✅ |
| 23.T2 `GpuSettings` in `ferrum-engines`; `AgentConfig` delegates; peaks | 23.2-b (the manifest field in PR 2) | 1, 2 | ✅ |
| 23.T3 agent on the lease: wait with progress, timeout naming holders | 23.1-d/e, 23.2-e | 1 | ✅ |
| 23.T4 protocol additions and conformance | 23.4-b/d | 2 | ✅ |
| 23.T5 bridges: worker process, unload, `resident_mib`, release tests | 23.4-a/b (tests; the measurements on the card are the field test) | 2 | ✅ |
| 23.T6 `engine unload`, `engine list` fields | 23.4-c/d | 2 | ✅ |
| 23.T7 domain port, `GatedEngine`, `Viewer` waiting and cancel | 23.3-a/b | 3 | ✅ |
| 23.T8 AI panel waiting state; `gpu hold` | 23.3-c, 23.5-d | 3 | ✅ |
| 23.T9 benchmark holder options; field run; recipe | 23.5-a/b/c | 3 | options ✅ (dry run); field run and recipe pending |

**Size:** XL, five contract surfaces. It is split into three PRs of size L
([design §7–8](docs/gpu-sharing.md#7-size-and-pr-split)):
1. the lock crate, configuration and agent;
2. the protocol, the bridges and `engine unload`, then a field test;
3. the desktop, `gpu hold` and the benchmark, then the field run of the
   success metric.

---

## Stage 24 — Embeddable viewer widget (`ferrum-ui`) 🚧 (G1 ✅, G2 ✅; PR 1 of 2)

Medical Harness shows a conversation and FERRUM's viewer in one egui/wgpu
window ([#69](https://github.com/Kauter1989/ferrum/issues/69); P1 for the
harness's Demo 4, a live viewer). Until then the harness shows FERRUM
renders as images. What exists:
- The views (`slice_view.rs`, `volume_view.rs`), the panels (segments,
  segmentation and AI, review, metrics card, transfer function) and the
  GPU bridge (`gpu_bridge.rs`) live in the `ferrum` binary crate, mixed
  with the app shell in `app.rs`: header, toolbar, start screen, recent
  files, file dialogs (`rfd`), drag and drop, screenshots and keyboard
  shortcuts.
- Every use case is already a `Viewer` method in `ferrum-app`, and the
  views draw through `GpuSink` and egui-wgpu paint callbacks.
- The view state that an agent could set (`ferrum-view/1`,
  [#70](https://github.com/Kauter1989/ferrum/issues/70)) does not exist
  yet. It is the next stage and builds on this one.

This stage moves the viewer into a library crate that another egui/wgpu
application places in a panel, and makes the desktop app its first user.

| # | User story | Acceptance criteria |
|---|---|---|
| 24.1 | As a **harness developer**, I want to place FERRUM's viewer in a panel of my own egui/wgpu window so that the person works with the study next to the conversation, rendered on the GPU without streaming. | [24.1-a] A new crate `ferrum-ui` exports `ViewerWidget`, which draws the 2D, MPR and 3D views of a `Viewer` with their overlays, measurements and segments into any `egui::Ui` it is given, with no window, menus or file dialogs of its own. Its dependencies include neither `eframe` nor `rfd`. [24.1-b] A host installs the GPU resources once with the `egui_wgpu::RenderState` it already has (`ferrum_ui::install`). Without a render state (a CPU-only test), the widget still lays out and handles input. [24.1-c] The widget has the desktop's toolbar of view tools (modes, tools, zoom) as an option the host can turn off, and the panels (segments, AI segmentation, review, metrics) as separate widgets the host may place or leave out. |
| 24.2 | As a **harness developer**, I want the widget to tell me what the person did so that my agent learns the outcome without being in the render loop. | [24.2-a] Each frame, `ViewerWidget::show` returns the events of that frame: `PointPicked`, `RoiDrawn` (an annotation or a segment the person drew), `SliceChanged`, `WindowChanged` and `CameraChanged`. Events are coalesced: one `SliceChanged` per frame at most, and a drag gives one `WindowChanged` when it ends. [24.2-b] Points are given in FERRUM's point forms (`voxel` and `patient_mm`), the same values the agent's `probe` resolves. [24.2-c] No event is a decision: confirm and reject stay in the review widget, and the host's decision channel is the library API of 16.7. |
| 24.3 | As a **FERRUM maintainer**, I want the desktop app to be a thin user of the same crate so that the rendering rule and the UI tests stay in one place. | [24.3-a] `ferrum` (the app) keeps only the shell: header, start screen, recent files, file and save dialogs, drag and drop, screenshots, shortcuts. It draws the views and panels through `ferrum-ui`. [24.3-b] The UI tests (`crates/ferrum/tests/ui.rs`) and `gpu_parity.rs` pass unchanged in what they check; tests of the views and panels may move to `ferrum-ui`. [24.3-c] The shaders stay in `ferrum-render`; `ferrum-ui` adds no image-formation code, so the rendering rule is unchanged. |
| 24.4 | As a **harness developer**, I want an example host so that I can copy a working setup. | [24.4-a] `crates/ferrum-ui/examples/embed_host.rs` opens a window with its own side panel (an event log and a few buttons that call `Viewer` use cases, such as "next slice" and "lung window") next to the widget, and prints the events it receives. [24.4-b] A UI test drives the example's layout with `egui_kittest`: a click in the axial view gives one `PointPicked` whose voxel matches `probe` for that point. |
| 24.5 | As a **clinician**, I want the licence and the research-use badge of an engine to stay visible when FERRUM is embedded so that I always know a result came from a research-only engine. | [24.5-a] The AI segmentation widget shows the engine's licence and the `research_only` badge as the desktop does; a segment proposed by an engine keeps its "Proposed by engine" label in the segments widget. [24.5-b] A UI test checks both inside the embedded layout of the example. |

**Success metrics:** the example host shows a CT with the views, takes a
point pick and prints it within one frame. The desktop app looks and
behaves as before: all UI tests and `gpu_parity.rs` pass on lavapipe.

**Out of scope:** the `ferrum-view/1` state and agent commands (#70, next
stage), streaming and WASM backends (ADR 0014 decisions 4–7), several
viewers in one window (one `Viewer` per window; the GPU resources are
installed once), file loading inside the widget (the host opens studies
through `Viewer` or its own code), theming beyond FERRUM's dark theme.

**Clarifications (G1 ✅ 2026-10-25):**
- Q1: crate name `ferrum-ui`, as in the issue? — A: yes (2026-10-25).
- Q2: panels as separate widgets the host may place (24.1-c), rather than one fixed layout? — A: yes; the host decides its layout (2026-10-25).
- Q3: events in this stage, before `ferrum-view/1`? — A: yes, as Rust types; #70 gives them their JSON schema (2026-10-25).
- Q4: egui and wgpu versions: the host must use the same versions as FERRUM (they are re-exported as `ferrum_ui::egui` and `ferrum_ui::egui_wgpu`)? — A: yes, and the versions are named in the crate docs (2026-10-25).
- Q5: mode — gated (G1 spec, G2 design), as for a stage? — A: yes (2026-10-25).

**Design:** [docs/embeddable-viewer.md](docs/embeddable-viewer.md), [ADR 0016](docs/decisions/0016-ferrum-ui-crate.md) (G2 ✅ 2026-10-28: «работай автономно по плану далее»; CI on GitHub is unavailable until 1 November, so PRs are checked locally with `preflight.sh`). Size L, two PRs: the move, then events and the example host.

### Implementation

| Task | Criteria | PR | Status |
|---|---|---|---|
| 24.T1 crate `ferrum-ui`; views, panels, theme, widgets, GPU bridge moved | 24.1-a, 24.3-c | 1 | ✅ |
| 24.T2 `ViewerWidget` (layout, tool bar, polling, GPU sync), `panels::side_panel`, `PanelRequest` | 24.1-b/c | 1 | ✅ |
| 24.T3 desktop on `ferrum-ui`; UI tests and parity unchanged; renders before and after compared | 24.3-a/b | 1 | ✅ |
| 24.T4 events | 24.2-a/b/c | 2 | |
| 24.T5 example host and its UI test | 24.4-a/b | 2 | |
| 24.T6 licence and badge test in the embedded layout | 24.5-a/b | 2 | |
| 24.T7 docs, events privacy test | — | 2 | |

## Roadmap

Updated 2026-10-19, after Stage 20. Order is a proposal; each stage
starts with its spec at gate G1. Sizes follow `ferrum-size`.

### Next

| # | What | Kind | Why now |
|---|---|---|---|
| 1 | Release 0.4.0: Stages 18 and 20, loader warnings (2.7), DICOM in LPS (2.8), lossless HU (18.13) since 0.3.0 | release (`ferrum-release`) | `develop` holds a large unreleased change set; the user decides |
| 2 | Field test on the user's machine: Stage 20 on a scanner's Enhanced MR or PET (success metric, 0.01 mm), switch time and VRAM on the GPU; 18.13-f against pyradiomics | field test (`ferrum-field-test`) | the open criteria of Stages 18 and 20 can only be closed there |
| 3 | Stage 21 — PET quantification (below) | stage, L | from the MR/PET assessment (2026-10-14): without units and SUV a PET study has no usable numbers |
| 4 | Stage 22 — PET/CT fusion (below) | stage, L, new ADR | the main way PET is read; needs a second volume in `Dataset` and `GpuSink` |
| 5 | Stage 16 — the rest: 16.1 confirm all, 16.2 export only confirmed, 16.3 provenance of a corrected segment, 16.4 annotation mode | stage, M per story | open since Stage 16; 16.5–16.6 are done |
| 6 | Stage 19 — reproducible radiomics and comparison (IBSI, textures, change over time) | stage, XL, G1 again | waits for the release of Stage 18 (its own rule); MR normalisation joins it |
| 7 | Interactive view host for generative UI ([ADR 0014](docs/decisions/0014-generative-ui-view-host.md), draft): the agent mounts a FERRUM view in the conversation and the person works in it directly | stage, size after G1/G2 | a new direction (merged in #67); its priority against 3–6 is the user's call |
| 8 | Stage 23 — GPU sharing with an external model server (above, [#68](https://github.com/Kauter1989/ferrum/issues/68)) | stage, XL, 3 PRs | P0 for Medical Harness's Demo 2 (a resident local model on the same 12 GB card); its order against 1–7 is the user's call |

### Backlog (feature or small, any time)

- **Faster PET frame switch:** a 4D `.nii.gz` is decompressed from the
  start for every frame (1.1 s for 586 MB); decompress once into a
  temporary cache, or read uncompressed `.nii` directly.
- **GPU texture reuse:** a volume switch creates a new 3D texture; reuse
  it when dims and format match (measure on a real GPU first, L13).
- **Volume count in the series picker:** the scan descriptor does not carry
  it; reading volumes per series during the scan costs time on large
  folders.
- **Siemens mosaic:** classic Siemens EPI/DWI stores a slice grid in one
  image; it is read as one large slice (field note of 20.4).
- **Vendor b-values:** classic DWI that stores b in private tags is named
  `volume k/N`; a vendor dictionary would name it (raw tag literals stay
  forbidden, so it needs a named-constant table).
- **Window presets per modality:** the HU presets are offered for MR and
  PET too (from the MR/PET assessment); hide them or offer percentile and
  SUV windows (part of Stage 21 if not done before).
- **GUI localisation:** recorded as a separate task in Stage 18.

### Stage 21 — PET quantification 📋 (draft; criteria at G1)

| # | User story (draft) |
|---|---|
| 21.1 | As a **nuclear medicine physician**, I want PET values in Bq/ml and SUV (body weight; later lean body mass) from the DICOM PET module (units, decay correction, radiopharmaceutical dose and time, half-life, patient weight), refused with the reason when data are missing, so that numbers mean what they say. |
| 21.2 | As a **physician**, I want SUVmax, SUVpeak (1 cm³ sphere), SUVmean, MTV and TLG of a segment, and thresholds at 41 % of SUVmax and at SUV ≥ 2.5, so that a PET report needs no other tool. |
| 21.3 | As a **reader**, I want PET colour maps (inverted grey, hot/inferno) and SUV windows (0–5, 0–10, 0–15) in slices and MIP, with HU presets hidden for PET and MR; a shader change with CPU parity and a visual check (L1). |
| 21.4 | As an **agent developer**, I want `value_unit` = `SUVbw` or `Bq/ml` in the agent with the PET metrics in `segment metrics`, identical over CLI and MCP, and identifiers (weight, dose times) kept out of outputs unless allowed. |

Out of scope (draft): kinetic modelling, Patlak, partial-volume correction,
SUV for non-DICOM PET without a sidecar that names the units.

### Stage 22 — PET/CT fusion 📋 (draft; criteria and ADR at G1/G2)

| # | User story (draft) |
|---|---|
| 22.1 | As a **reader**, I want a PET volume shown in colour over a CT of the same study on the common LPS geometry, with opacity and its own window, marked "not registered" (no alignment is claimed, as in 19.9-b). |
| 22.2 | As a **reader**, I want the probe and the segment metrics to read the volume I choose (CT in HU, PET in SUV) under the same cursor. |
| 22.3 | As an **agent developer**, I want `view slice` and `view mpr` to render the overlay so that an agent sees what the person sees. |

Architecture (to decide in an ADR): a second volume in `Dataset` and
`GpuSink`, resampling to the base grid, and the rendering rule (shader and
CPU raycaster both).
