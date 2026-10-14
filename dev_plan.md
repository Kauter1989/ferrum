# Development plan

This is the working plan for FERRUM, a reusable visualisation core for
medical imaging ([vision](docs/vision.md)). Stages 1–13 built the viewer.
Stage 14 opens it to AI segmentation engines, and Stage 15 makes it an
agent skill.

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

**2.7 — Out of scope:** splitting a series into selectable volumes (echo, b-value, phase), per-frame geometry of Enhanced multi-frame DICOM, resampling irregular series, NIfTI-2, PET units and SUV (planned as separate stages).

**2.7 — Clarifications:**
- assumed: a series with several images per position shows volume 1 with a warning instead of being refused. It matches the NIfTI 4D behaviour of 2.4 and keeps DWI and multi-echo series readable; refusing would block reading altogether.
- assumed: the irregular-gap tolerance is max(0.01 mm, 2 % of the median gap): above the rounding noise of Image Position (Patient), below a missing slice (100 %).
- assumed: the desktop shows the warnings in a dismissible window after loading and in the **Info** tab; no new dialog flow.
- assumed: the agent's default window changes only for data without a stored window; the schema and the parameters stay the same.

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

Future stages are added here as they are planned (e.g. "Stage 15 — …"),
with user stories and acceptance criteria in the same format.

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

## Stage 18 — Segmentation post-processing: quantitative profiles 📋

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
| 6 | clinical card, HU-range fields, benchmark | 🚧 in review |

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
