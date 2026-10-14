# Multi-volume series (Stage 20.2–20.4)

Design of choosing a volume of an MR or PET series that holds several:
echoes, diffusion b-values, dynamic phases, PET frames. Spec:
[`dev_plan.md`, Stage 20](../dev_plan.md#stage-20--multi-volume-mr-and-pet-series-);
decision: [ADR 0013](decisions/0013-multi-volume-series.md).

## 1. Context

What exists (0.3.x, after #58–#61):

| Piece | Where | Today |
|---|---|---|
| Several images per position | `ferrum-io/src/dicom/series.rs` (`order_by_position`, `StackKey`) | the first image at every position is shown; a warning says one volume of k is shown (2.7-a/b) |
| Enhanced per-frame keys | `ferrum-io/src/dicom/frames.rs` | Temporal Position Index, Effective Echo Time, b-value per frame (20.1) |
| 4D NIfTI | `ferrum-io/src/nifti.rs` (`read_values`) | volume 1 of N, with a warning (2.7-e) |
| Series descriptor | `ferrum-domain/src/repository.rs` (`SeriesDescriptor`) | one per series; `id`, `dims`, `sources` |
| Agent | `ferrum-agent/src/study.rs` (`Study::open`, `choose_series`) | a workspace holds one `series_id`; another series is refused |
| Desktop | `ferrum-app/src/viewer.rs` (`series_choice`, `load_series`); `ferrum/src/app.rs` (`series_dialog`) | a picker when a scan finds several series |
| Workspace | `ferrum-io/src/workspace.rs`, [format](workspace-format.md#4-ferrum-workspace-v1) | `source.series_id`, version 1 |

The user's answers (G1, 2026-10-14): **one entry per volume**; volumes are
**named by b-value, echo time or phase**; the desktop **switches volumes
without reloading** the study.

## 2. Constitution check

| Point | Answer |
|---|---|
| Layering | Yes. Partitioning in `ferrum-io`; `VolumeInfo` in `ferrum-domain`; switching is a `Viewer` use case in `ferrum-app`; the widget only calls it. |
| Ports | Yes. Files (DICOM, NIfTI, BIDS sidecars) are read only in `ferrum-io`. |
| No inference in Rust | Not touched. |
| Agents propose, people confirm; values from voxels | Yes. Every value command and metrics result names the volume its voxels came from (20.3-d); a switch marks metrics of another volume out of date (20.2-c). |
| No identifiers leave FERRUM | Yes. Group and volume ids are pseudonymised like series ids; labels hold only b, TE, phase and frame time. Privacy scan extended (20.3-e). |
| Rendering rule | Not touched: a switch uploads another volume through the existing `GpuSink`. |

## 3. Design

### 3.1 Domain

```rust
/// Which volume of a multi-volume series a descriptor stands for.
pub struct VolumeInfo {
    pub group: String,          // id of the series all volumes belong to
    pub index: u32,             // 0-based, in the order below
    pub count: u32,
    pub label: String,          // "b = 1000", "TE = 80 ms", "phase 3/20", "frame 3/33, t = 60 s", "volume 2/5"
    pub b_value: Option<f64>,
    pub echo_time_ms: Option<f64>,
    pub echo_number: Option<i64>,
    pub temporal_position: Option<i64>,
    pub frame_time_s: Option<f64>,
    pub complete: bool,         // one image at every position of the group
}
// SeriesDescriptor gets `volume: Option<VolumeInfo>` (None: single-volume series, unchanged).
```

### 3.2 Partitioning (ferrum-io)

DICOM, after grouping by `SeriesKey` and building the positioned images
(`series.rs`):

1. Group images by position (as `order_by_position` does).
2. If no position holds more than one image: one descriptor, `volume: None`
   (unchanged).
3. Otherwise find the **distinguishing fields**: those of Temporal
   Position, Echo Number, Effective Echo Time, b-value, Trigger Time,
   Frame Reference Time (new, classic dynamic PET) and Acquisition Number
   whose values differ between images at one position. The volume key of
   an image is the tuple of those fields; each distinct key is a volume,
   ordered by the key (`StackKey::order`).
4. If no field differs, the k-th image at each position (by Instance
   Number, then file order) forms volume k (`volume k/N`).
5. A volume missing a position is `complete: false`: listed, opened with a
   warning, not switchable from a complete one (different grid).

`VolumeRepository::scan` keeps returning whole series (the same ids as
before, so existing workspaces and callers are unchanged);
`VolumeRepository::volumes(series)` returns one descriptor per volume, or
the series itself. The volume id is `<series id>#v=<key>`, e.g. `…#v=b1000`, `…#v=t3`,
`…#v=n2`. Loading a descriptor reads only that volume's files (frames for
an Enhanced file).

NIfTI 4D: one descriptor per volume (`#v=n<k>`). Labels from BIDS
sidecars next to the file when present: `<name>.bval` (b-values) and
`<name>.json` `FrameTimesStart` (PET), else `volume k/N`. The reader
decodes only the chosen volume (offset `k × voxels`).

### 3.3 Agent

- `study scan`: one entry per volume, with `volume` (§3.1, without
  `group`) and `group` (pseudonymised). Single-volume entries are
  unchanged.
- `study open`: `series` takes a volume id or a group id. A group id, or
  no `series` when the source holds one multi-volume series and nothing
  else, opens volume 1 with a warning naming the count and how to choose.
  Two distinct series still need `series` (unchanged).
- Workspace bound to the **series**: `source.series_id` is the group id,
  `source.volume` the current volume. `study open` of another volume of
  the same series in that workspace **switches** the volume (audit entry
  `volume`); segments, annotations and engine inputs stay (same grid).
  Another series is refused as today.
- `study info`: `volume` (current) and `volumes` (id, label, complete)
  of the series.
- Provenance of value commands (`probe`, `stats`, `profile`,
  `measure`, `segment metrics`, `view …`) gains `volume` (label and id)
  when the series has several.

### 3.4 Desktop (ferrum-app use cases, ferrum widgets)

- The series picker groups descriptors by `volume.group`: one row per
  series ("DWI — 3 volumes"). Choosing it opens volume 1.
- `Viewer::volumes()` → the volumes of the open series;
  `Viewer::switch_volume(index)` → a `JobQueue` load, and on completion
  replaces the dataset's voxels while keeping slices, zoom, view mode,
  segments, annotations and the workspace. The window follows the new
  volume's auto window unless the person changed it. Metrics of another
  volume are marked out of date.
- A cache of loaded volumes (budget: 1 GiB of voxels, LRU) makes switching
  back immediate.
- UI: in the **Image** tab, a "Volume" row: a combo box with the labels and
  ‹ › buttons; incomplete volumes disabled with the reason.

## 4. Contracts

| Contract | Change | Also update (`ferrum-change` §2) |
|---|---|---|
| Agent output | `study scan` entries, `study open`/`info` fields, provenance `volume` | `schema.rs` (`series` description), `commands.json`, `docs/agent-cli.md`, `skills/ferrum/reference/commands.md`, contract tests, CLI = MCP, privacy scan |
| Workspace | `source.volume`; manifest version 2 only when set; version 1 opens as volume 1 | `docs/workspace-format.md`, unit tests in `workspace.rs` |
| Domain API | `SeriesDescriptor::volume`, `VolumeInfo` | `docs/architecture.md` |
| UI | Volume row, grouped picker | `cargo test -p ferrum --test ui` (L9) |

## 5. Risks and assumptions

- **Classic DWI from some vendors** stores the b-value only in private
  tags. Reading them would need raw tag literals, which `CLAUDE.md`
  forbids; such series are named `volume k/N`. A later change can add a
  vendor dictionary.
- **Memory:** the cache budget (1 GiB) is an estimate, not measured (L13);
  the field test reports the switch time and memory for dynamic PET
  (33 × 200 × 200 × 111).
- **Real multi-volume DICOM** cannot be downloaded here (TCIA and NEMA
  blocked). The field check repacks OpenNeuro DWI and dynamic PET NIfTI as
  DICOM with real b-values and frame times (W8), and asks the user for a
  scanner series.
- **Annotation slices** stay valid across volumes, because volumes of a
  complete series share the grid.

## 6. Validation plan

| Criterion | Verified by | Level |
|---|---|---|
| [20.4-a] | `series.rs` tests: distinguishing fields; echoes, b-values, phases; rank fallback (planned) | test |
| [20.4-b] | `dicom_integration.rs`: dynamic PET with Frame Reference Time (planned) | test |
| [20.4-c] | `nifti.rs`: 4D with `.bval` and PET `.json` sidecars (planned) | test |
| [20.4-d] | `dicom_integration.rs`: a volume's load decodes only its files (planned) | test |
| [20.2-a] | `use_cases.rs` volumes and labels; `ui.rs` Volume row (planned) | test, manual |
| [20.2-b] | `use_cases.rs::switching_keeps_the_view` (planned); field: switch time on dynamic PET | test, field |
| [20.2-c] | `use_cases.rs` metrics out of date and named volume (planned) | test |
| [20.2-d] | `ui.rs` grouped picker (planned) | test |
| [20.2-e] | `use_cases.rs` incomplete volume disabled (planned) | test |
| [20.3-a…e] | contract tests in `crates/ferrum-agent/tests` (scan, open, switch, info, provenance), `mcp.rs`, `privacy.rs` (planned) | test |
| [20.3-f] | `workspace.rs` v1 and v2 manifests (planned) | test |
| all | repacked DWI and dynamic PET (W8), desktop screenshots | field, visual |

## 7. Size and PR split

| PR | Tasks | Estimate | Class |
|---|---|---|---|
| 1 | 20.4 partitioning and loading (domain, io), design and ADR | ~550 code, ~400 tests, 12 files, 0 external contracts | M |
| 2 | 20.3 agent and workspace | ~400 code, ~350 tests, ~200 docs, 16 files, 2 contracts (agent output, workspace) | L |
| 3 | 20.2 desktop: switching, cache, UI | ~450 code, ~300 tests, 9 files | M |

A field check follows PR 1 (volumes and labels on repacked data) and PR 3
(switching in the desktop app, time and memory).

## 8. Tasks

| Task | Criteria | Tests first | Touches (matrix rows) | Depends on | PR |
|---|---|---|---|---|---|
| 20.4.1 `VolumeInfo`, partition DICOM by distinguishing fields, Frame Reference Time, load one volume | 20.4-a, 20.4-b, 20.4-d | `series.rs`, `dicom_integration.rs` | input format reader | — | 1 |
| 20.4.2 NIfTI 4D descriptors, BIDS sidecars, decode one volume | 20.4-c | `nifti.rs` | input format reader | 20.4.1 | 1 |
| 20.3.1 workspace `source.volume`, version 2 | 20.3-f | `workspace.rs` | workspace files | 20.4.1 | 2 |
| 20.3.2 scan, open, switch, info, provenance | 20.3-a…d | contract tests | agent command, agent outputs | 20.3.1 | 2 |
| 20.3.3 schema, docs, MCP, privacy | 20.3-e | `mcp.rs`, `privacy.rs`, `package.rs` | agent command, agent outputs | 20.3.2 | 2 |
| 20.2.1 `Viewer::volumes`, `switch_volume`, cache, metrics out of date | 20.2-b, 20.2-c, 20.2-e | `use_cases.rs` | desktop UI (use case) | 20.4.1 | 3 |
| 20.2.2 Volume row, grouped picker | 20.2-a, 20.2-d | `ui.rs` | desktop UI | 20.2.1 | 3 |
