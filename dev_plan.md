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

## Stage 17 — Agent segmentation scenarios 📋

An AI agent segments with the engines that already connect to FERRUM
(nnInteractive, TotalSegmentator, MONAI Label) in reviewed, repeatable
scenarios — organ volumetry, lesions by prompts, detect-then-refine,
corrections, follow-up, dataset pre-labelling — on a single 12 GB GPU
(RTX 3080 Ti). Design: [docs/agent-segmentation.md](docs/agent-segmentation.md)
([ADR 0010](docs/decisions/0010-agent-segmentation.md), proposed).

| # | User story | Acceptance criteria |
|---|---|---|
| 17.1 | As an **agent developer**, I want to refine an engine segment with further prompts so that an agent can correct a leak or a miss instead of starting again. | `segment interactive` with `segment` and `append`; the prompt history stored with the segment and replayed by the command line, cached by the MCP server (identical JSON); `requested_by` lets the agent rename, refine and delete engine segments it requested, never a person's. |
| 17.2 | As an **operator**, I want engine calls sized for one 12 GB GPU so that nnInteractive and TotalSegmentator share the card without running out of memory. | ROI uploads for interactive sessions (default: prompts' box + 48 mm); `network.gpu_groups` serialises calls across processes; `engine list`; `name_prefix` and `modality` for engine commands; job progress over MCP. |
| 17.3 | As a **clinician**, I want every engine result checked by fixed rules so that an agent cannot hide a leak, a cut-off organ or a left/right mix-up. | `segment shape`, `components`, `compare`, `edit`; `stats` on a segment within a box; `checks` (empty, size, components, border, laterality, overlap, stability, research flag) on every engine result, with warnings; phantom tests with known answers. |
| 17.4 | As an **operator**, I want the bridges to free GPU memory when idle so that engines can take turns on one card. | TotalSegmentator jobs in a child process; nnInteractive drops its image when the last session closes; `deterministic` in `info` and a replay test in the conformance suite; compose profiles `interactive`, `automatic`, `mixed`. |
| 17.5 | As a **harness developer**, I want the skill to teach the segmentation scenarios so that agents use engines safely. | `skills/ferrum/reference/segmentation.md` and a `SKILL.md` section; evaluations with the mock engine (segment and measure, find all objects, remove a leak). |
| 17.6 | As an **operator**, I want measured VRAM and times on an RTX 3080 Ti so that I can plan the hardware. | Benchmark per engine and scenario; the table in the design filled in. |
| 17.7 | As an **agent developer**, I want to correct an existing segment with an engine so that imported or proposed labels can be fixed. | `from_segment` (lasso seeds from the mask) and scribble/lasso prompts in the agent; phantom test. |
