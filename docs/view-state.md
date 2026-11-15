# View state and agent commands (`ferrum-view/1`, Stage 28)

Design for [Stage 28](../dev_plan.md) / [#70](https://github.com/Kauter1989/ferrum/issues/70):
an agent drives what the person sees — open the lung window, go to slice 142,
show segment 3, draw the long axis — without sitting in the render loop, and
the person takes over at any time. G2 design; the contract decision is
[ADR 0018](decisions/0018-view-state-contract.md).

## 1. What exists, what is added

Exists:

- The HTTP view host serves a 2D state, input and events
  ([view-host.md](view-host.md), `crates/ferrum-view/src/state.rs`):
  `ViewState { axis, index, window, segments, rev, probe }`, `Input`
  (slice/step/plane/window/preset/segments/probe), events `slice_changed`,
  `plane_changed`, `window_changed`, `segments_changed`, `point_picked`.
- The embeddable widget returns the frame's events as Rust types
  (`ferrum_ui::ViewerEvent`: `PointPicked`, `RoiDrawn`, `SliceChanged`,
  `WindowChanged`, `CameraChanged`, #109).
- Every use case lives in `ferrum-app::Viewer`: `set_view_mode`,
  `set_slice_index`, `step_slice`, `set_window`, `apply_window_preset`,
  `set_segments_shown`, `set_segment_style`, `adopt_segments`; the public
  state `slices: SliceState` (`axis`, `indices`, `views`, `window`),
  `volume: VolumeViewState` (`camera: OrbitCamera`, `settings`, `clip`),
  `tool: ToolKind`, `probe`.
- The workspace store persists artifacts atomically and notifies change
  (`Workspace::save_segments`/`save_annotations`, `changes::publish`,
  `WorkspaceWatcher`, `subscribe`); the decisions API binds to the study's
  `source_sha256` and refuses a stale one with `source_changed`
  (`ferrum_agent::decisions`).

Added by this stage:

- The versioned **ViewState** JSON contract (the full state, not only the 2D
  subset) and the JSON form of the events, with a spec page (§3) and
  conformance fixtures (§9).
- A **workspace artifact** that holds the view state and the event log, so an
  agent and a live client agree without sharing a process (§4).
- A `Viewer` **apply use case** and the accessors it needs (§5).
- Agent commands `view set`, `view state`, `view events` (§7).

## 2. Layering

`ferrum-agent` depends on neither `ferrum-app` nor `ferrum-view` (CLAUDE.md).
So the view state is **data in the workspace**, and the live clients apply it:

```
ferrum-agent (view set/state/events) ── writes/reads ──▶ ferrum-io workspace artifact (view.json, view-events.json)
                                                              │  changes::publish / WorkspaceWatcher
                                                              ▼
ferrum-ui widget ─┐                                   ferrum-view host ─┐
                  ├─ apply via ─▶ ferrum-app::Viewer::apply_view_state ◀─┘
                                  (ViewState type in ferrum-domain)
```

- `ferrum-domain` owns the `ViewState`/`ViewEvent` value types (pure data, no
  I/O), shared by the agent, the app and the clients.
- `ferrum-io` owns persistence (`Workspace::save_view_state` /
  `load_view_state`, the event log) and the change notification, exactly like
  `segments.json`.
- `ferrum-app` owns `Viewer::apply_view_state` (the only place the state
  becomes use cases) and `Viewer::view_state` (read it back).
- `ferrum-agent` commands read/write the artifact through `ferrum-io`; they
  never link the app or the host.
- `ferrum-ui` (widget) and `ferrum-view` (host) watch the artifact and apply
  it; they record the events it produces.

**In process first** (the issue): the widget applies the full state. The HTTP
host adopts the artifact for its existing 2D controls in this stage and
reports fields it cannot yet honour; streaming the full state (camera, 3D,
overlay control, selected segment, tool) to the host and the browser is
[Stage 29](../dev_plan.md).

## 3. The contract (`ferrum-view/1`)

The state document keeps `format: "ferrum-view/1"` and gains fields; a client
that knows only the 2D subset keeps working (the new fields are additive).

### 3.1 ViewState

```json
{
  "format": "ferrum-view/1",
  "revision": 7,
  "source_hash": "<sha256 of the workspace source, as decisions binds it>",
  "component": "slice",            // "slice" | "mpr" | "volume"
  "plane": "axial",                // "axial" | "coronal" | "sagittal"
  "slice_index": 142,              // 0-based, clamped to the volume
  "window": { "center": -600, "width": 1500 },
  "overlays": { "segments_shown": true, "fill_opacity": 0.4, "style": "fill_outline" },
  "selected_segment": 3,           // label, or null
  "tool": "probe",                 // see §3.3
  "camera": {                      // present for "mpr"/"volume"
    "orientation": [0, 0, 0, 1],   // quaternion x,y,z,w
    "distance": 2.5,
    "target": [0, 0, 0],           // model space
    "fov_y": 0.785
  },
  "probe": { "voxel": [251,198,142], "patient_mm": [12.4,-40.1,88.1], "value": 55, "unit": "HU" }
}
```

- `component` maps to `ViewMode`: `slice`→`Slice2d`, `mpr`→`Mpr`,
  `volume`→`Volume3d` (the wire names of the issue; the desktop labels are
  "2D"/"MPR"/"3D").
- `plane`/`slice_index` map to `slices.axis` / `slices.indices`.
- `window` maps to `slices.window` (physical units; `HU` only for a CT).
- `overlays` maps to `set_segments_shown`, `fill_opacity`, `set_segment_style`.
- `selected_segment` is a label of the segmentation set, or `null` (§5 gap).
- `tool` maps to `Viewer::tool` (§3.3).
- `camera` maps to `volume.camera: OrbitCamera` (`orientation`, `distance`,
  `target`, `fov_y`).
- `probe` is read-only (the last pick); a `view set` ignores it.
- `revision` increases on every applied change; `source_hash` binds the state
  to the study (§6).

A state with no `component` is `slice` (backward compatible). Unknown fields
are refused by `view set` (a contract error names the field); a client reading
a state ignores fields it does not know.

### 3.2 Events

```json
{ "format": "ferrum-view/1",
  "events": [
    { "seq": 1, "kind": "slice_changed", "plane": "axial", "slice_index": 142 },
    { "seq": 2, "kind": "window_changed", "center": -600, "width": 1500 },
    { "seq": 3, "kind": "camera_changed", "orientation": [...], "distance": 2.5 },
    { "seq": 4, "kind": "roi_drawn", "item": { "kind": "segment", "label": 3 } },
    { "seq": 5, "kind": "point_picked", "voxel": [...], "patient_mm": [...], "value": 55, "unit": "HU" }
  ],
  "next": 5, "dropped": 0 }
```

Kinds: `point_picked`, `roi_drawn`, `slice_changed`, `window_changed`,
`camera_changed`, and the host's existing `plane_changed`, `segments_changed`.
`roi_drawn` carries the item (annotation id or segment label) and its revision,
as the widget's `RoiDrawn` does; it is **not** a decision. `seq` increases; the
log keeps the last 512 and counts `dropped` (as the host does today). The
widget's Rust `ViewerEvent` (§1) gets exactly this JSON form (Stage 24 Q3).

### 3.3 Tools

`pan`, `window_level`, `probe`, `distance`, `angle`, `area`, `rect`, `text`,
`move`, `delete`, `ai_point`, `ai_box`, `ai_scribble`, `ai_lasso`, `region`
(`ToolKind`, snake_case). A `view set` may name any of them; setting a tool
never draws anything by itself.

## 4. The workspace artifact

Two files beside `segments.json`, owned by `ferrum-io`:

- **`view.json`** — `{ "format", "revision", "source_hash", "state": { …§3.1… } }`.
  Written atomically (`partial_path` + rename) by
  `Workspace::save_view_state`, read by `load_view_state`; a save publishes
  `changes::Touched::ViewState` (a new variant). An absent file means the
  default state (component `slice`, axial, the volume's window).
- **`view-events.json`** — the event log `{ "format", "next", "dropped",
  "events": [ …§3.2… ] }`, capped at 512. The live client that produces an
  event appends it here (atomic write, `Touched::ViewEvents`); `view events`
  reads the same sequence the host serves. A client keeps its in-memory ring
  for the HTTP `/api/events` and persists it so an agent reads the same `seq`.

The change mechanism gains `WorkspaceChange::ViewStateChanged { revision }`
(no identifiers — a revision number only), reported by `WorkspaceWatcher` and
`subscribe` like the segment changes. A client applies the artifact when this
change arrives (or on its next poll), never on a timer.

The privacy rules of [view-host.md §5](view-host.md) hold: the artifact carries
numbers, labels and the source hash only — no path, UID, name or header value.
The source hash is the study's `source_sha256` (a hash, not a name).

## 5. The apply use case

`Viewer::apply_view_state(&mut self, state: &ViewState) -> Applied` (ferrum-app)
maps each field to the existing use cases:

| Field | Use case |
|---|---|
| `component` | `set_view_mode` |
| `plane` | `slices.axis = …` |
| `slice_index` | `set_slice_index(axis, i)` (clamped) |
| `window` | `set_window` |
| `overlays.segments_shown` | `set_segments_shown` |
| `overlays.fill_opacity` / `style` | `segments.fill_opacity` / `set_segment_style` |
| `selected_segment` | `select_segment(label)` — **new accessor** (gap) |
| `tool` | `tool = …` |
| `camera` | `volume.camera = OrbitCamera { … }` |

`Viewer::view_state(&self) -> ViewState` reads it back (the source hash comes
from the workspace session). `Applied` reports which fields changed, so the
caller records exactly the events that happened (a `view set` of the current
window records nothing).

**Gaps to add** (the design names them; implementation fills them):
- `SegmentationState.selected: Option<u8>` + `Viewer::select_segment` /
  `selected_segment` (no selected-segment state exists today).
- A `ViewState`/`ViewEvent` value type in `ferrum-domain` (the host's
  `state::ViewState` becomes a projection of it; the host keeps its 2D
  `ViewState` for the existing routes until Stage 29).

**Person takes over.** The artifact is a suggestion applied once per revision:
a client applies it when `ViewStateChanged` arrives, then the person's input
wins and is recorded as events; the agent's next `view set` is a new revision.
Nothing locks the view, no mode is entered (criterion 28.2-a). A `view set`
that names a field the host cannot honour yet (camera/3D in this stage) is
reported in the answer's `unhonoured` list, not silently dropped (28.2-b).

## 6. Binding to the source (`source_changed`)

`ViewState.source_hash` is the study's `source_sha256` (the value
`ferrum_agent::decisions` binds to, `Study::load`). `view set` takes the
`source_hash` the host read with the state; if it differs from the current
study's, the command refuses with `source_changed` and applies nothing — the
same rule as `decisions::record` (16.8). `view state` returns the current
`source_hash` so the host can pass it back. A workspace whose sources changed
under a long-lived client is already refused by `Study::load`/`verify_sources`.

## 7. Agent commands

Three commands, JSON in / `ferrum-agent/1` envelope out, registered in
`schema.rs` and the `COMMANDS` registry, shared by CLI and MCP (a contract test
asserts identical JSON), `commands.json`/`commands.md` regenerated:

- **`view set -w W --state <json>`** (or `--component/--plane/--slice/--window/
  --show-segments/--select/--tool/--camera` flags) — validates the state
  against the schema, checks `source_hash` (§6), writes `view.json` with
  `revision + 1`, returns the applied state and the `unhonoured` list. No
  decision is created (a test asserts review state is untouched, 28.3-c).
- **`view state -w W`** — reads `view.json` (or the default), returns the state
  with the current `source_hash` and `revision`.
- **`view events -w W [--since <seq>]`** — reads `view-events.json`, returns the
  events after `seq` with `next`/`dropped`.

Contract tests (28.3-b): set a state, read back the events it produced; a stale
`source_hash` is `source_changed`; an unknown field is a contract error; CLI and
MCP give identical JSON. The commands are agent commands (unlike `gpu hold` /
`view serve`, which are CLI-only): driving the view is the agent's job here.

## 8. Privacy

The privacy scan (`crates/ferrum-agent/tests/privacy.rs`) is extended to the
ViewState artifact, the event log and the three command envelopes: a study in a
patient-named folder leaks neither the name nor the path (28.5-a, extends
25.5-a). The state and events hold numbers, labels, the source hash and the
tool/component names only.

## 9. Conformance fixtures

`crates/ferrum-view/tests/fixtures/` (or `ferrum-agent/tests/fixtures/`) holds
valid and invalid ViewState/event documents: a full state, a 2D-only state
(backward compatible), an unknown field, a bad enum, a stale `source_hash`,
each event kind. A conformance test parses every fixture and asserts the
outcome; the agent's schema and the host's parser share the fixtures so they
cannot drift.

## 10. PR split and verification

Size L, two PRs:

1. **The contract and the in-process path** — `ViewState`/`ViewEvent` types
   (domain), the schema + spec page + fixtures, the artifact (io) + change
   notification, `apply_view_state`/`view_state` + the selected-segment
   accessor (app), the widget applies the artifact and emits JSON events,
   privacy. Criteria 28.1, 28.2-a, 28.5.
2. **The agent commands** — `view set/state/events`, schemas, contract tests
   (set → events, stale hash, CLI = MCP, no decision), the host applies the 2D
   subset + `source_changed`, docs/CHANGELOG. Criteria 28.2-b, 28.3, 28.4.

Verification: unit tests on the apply use case (each field, clamping, the
`Applied` diff); the conformance fixtures; the contract tests; the privacy
scan; a widget UI test (an applied state is reflected within one frame and the
person takes over); `gpu_parity.rs` unchanged (no image-formation change).
Success metric: in the example host, `view set` (lung window, slice 142, show
segment 3) is reflected within one frame, the events read back match, and the
person scrolls and takes over.

## 11. Out of scope (Stage 29)

Streaming the full ViewState (camera/3D/overlays/selected/tool) to the HTTP
host and the browser; the browser panel's segments/overlay, transfer-function
editor and measurements; compressed/16-bit volume transfer; the rotating token
(an ADR first); the CPU overlay consolidation; WebGPU test automation. Decisions
through the view stay with `ferrum_agent::decisions`; remote exposure/TLS and
several viewers per host remain out of scope.