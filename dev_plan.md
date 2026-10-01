# Development plan

This is the working plan for FERRUM. Each stage groups features
as user stories with acceptance criteria. Status legend:
✅ done · 🚧 in progress · 📋 planned.

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

## Stage 14 — Extensibility and AI segmentation (demo) 📋

FERRUM stays a visualisation core with extension points
([ADR 0007](docs/decisions/0007-extensibility-and-engine-protocol.md)).
Segmentation engines run out of process and speak the
[FERRUM Engine Protocol](docs/engine-protocol.md). There is no
inference in Rust for now.

| # | User story | Acceptance criteria |
|---|---|---|
| 14.1 | As an **integrator**, I want a documented engine protocol so that I can plug any segmentation engine into FERRUM without changing its code. | `docs/engine-protocol.md` (`ferrum-engine/1`) and ADR 0007. ✅ |
| 14.2 | As a **radiologist**, I want segments shown over the image and in 3D so that I can review a segmentation. | Volume geometry (origin, direction). `LabelMap`, segments with name, colour, visibility, opacity and volume in ml. 2D fill and outline and 3D rendering, with CPU-reference parity. NIfTI label-map import and export. ✅ |
| 14.3 | As a **developer**, I want an engine port with a network client and a mock so that engines are swappable and testable without a GPU. | `SegmentationEngine` / `InteractiveSession` ports; `ferrum-engines` with `HttpEngine` and `MockEngine`; tests against the mock. |
| 14.4 | As a **radiologist**, I want AI tools in a collapsible panel that work only when an engine is connected so that I always know what is available. | *AI segmentation* section: connection status and URL, prompt tools (point ±, box, scribble, lasso), Accept, Reset and Undo. The tools are disabled with a hint when no engine is connected, and a *Research use only* badge appears when the engine reports it. |
| 14.5 | As a **researcher**, I want a reproducible nnInteractive demo so that I can try interactive AI segmentation on my own GPU machine. | `bridges/nninteractive` (FastAPI) with Docker and `docs/ai-demo.md` covering requirements, start-up, SSH tunnel and a walkthrough on the demo CT. A protocol conformance test runs against the bridge. |
| 14.6 | As an **integrator**, I want bridges for MONAI Label and TotalSegmentator so that automatic segmentation and active learning become available. | Later stage. |

---

## Stage 15 — FERRUM as an agent skill 📋

AI agents in a medical harness use FERRUM as a skill: a skill package and
a headless tool, `ferrum-cli`, over the same use cases as the desktop
app. Agents propose and clinicians confirm
([ADR 0008](docs/decisions/0008-agent-skill.md),
[specification](docs/agent-skill.md)).

| # | User story | Acceptance criteria |
|---|---|---|
| 15.1 | As an **integrator**, I want a documented skill design so that I can plan FERRUM into my agent harness. | ADR 0008 and `docs/agent-skill.md`: commands, envelope, workspace, privacy, review, evaluation. ✅ |
| 15.2 | As a **clinician**, I want to know who created each annotation and segment and whether it was confirmed so that I never mistake a proposal for a finding. | `Provenance` (author human/agent/engine, status proposed/confirmed/rejected, time) in the domain. `ferrum-annotations` v2, segment metadata JSON, `ferrum-workspace` v1 read/write. |
| 15.3 | As an **agent developer**, I want a headless command line with JSON output so that a skill can drive FERRUM from scripts. | `ferrum-agent` + `ferrum-cli`: study, view (slice, MPR, montage, 3D with pixel mapping), probe, stats, profile, measure, annotate, segment (incl. threshold region growing), export. CPU renderer without a GPU. Operator configuration, audit log. Contract tests on phantoms and JSON Schema validation. |
| 15.4 | As a **harness developer**, I want an MCP server so that the tools plug into MCP-capable harnesses without glue code. | `ferrum-cli mcp` (stdio) serving the same commands; renders as image content; identical JSON to the CLI in a scripted session. |
| 15.5 | As a **harness developer**, I want an installable skill package so that agents know when and how to use FERRUM safely. | `skills/ferrum/` (`SKILL.md`, references, schemas) and a plugin manifest in the release archives. Evaluations on phantoms with known answers. A scan proves that outputs contain no identifiers. |
| 15.6 | As a **clinician**, I want to review an agent's work in the viewer so that I can accept, edit or reject it. | *File → Open workspace*; review queue with author and status; decisions written to the workspace and the audit log. |
| 15.7 | As a **harness developer**, I want AI segmentation and standard exports in the skill so that results flow to PACS and reporting. | `segment interactive` / `segment auto` through `ferrum-engine/1` (after 14.3–14.5); DICOM SEG and SR (TID 1500) export. |

---

## Next stages 📋

Future stages are added here as they are planned (e.g. "Stage 15 — …"),
with user stories and acceptance criteria in the same format.
