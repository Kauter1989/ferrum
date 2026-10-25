# Embeddable viewer widget (`ferrum-ui`) — design

Stage 24, [#69](https://github.com/Kauter1989/ferrum/issues/69). Spec and
criteria: `dev_plan.md` Stage 24 (G1 ✅ 2026-10-25). Decision:
[ADR 0016](decisions/0016-ferrum-ui-crate.md).

## 1. Context

The viewer exists only inside the `ferrum` binary. `crates/ferrum/src`
holds two kinds of code:

| Kind | Files | Depends on |
|---|---|---|
| **Views and panels** | `ui/slice_view.rs`, `ui/volume_view.rs`, `ui/tf_editor.rs`, `ui/panels.rs`, `ui/segments_panel.rs`, `ui/segmentation_panel.rs`, `ui/ai_panel.rs`, `ui/review_panel.rs`, `ui/metrics_card.rs`, `ui/widgets.rs`, `ui/theme.rs`, `gpu_bridge.rs`; the toolbar and the viewport layout (`main_area`) in `app.rs` | `Viewer`, `ferrum-render` (`gpu`), `ferrum-processing`, `ferrum-engines`, `ferrum-radiomics`, egui, egui-wgpu, wgpu |
| **App shell** | the rest of `app.rs` (header, start screen, studies sidebar, status bar, dialogs, shortcuts, drag and drop, screenshots), `ui/recent.rs`, `main.rs` | the above, plus `eframe`, `rfd` |

The views and panels already depend only on `Viewer` and the GPU bridge.
Nothing in them opens files or windows; the three file actions of the
panels (export annotations, import and export labels) are flags that
`app.rs` reads and turns into `rfd` dialogs. So the split is a move,
not a rewrite.

## 2. Constitution check

| Principle | How it holds |
|---|---|
| Use cases in `ferrum-app` | The widget calls `Viewer` methods only. Events are read from `Viewer` state after the frame (§3.4), not computed in the widget. |
| Layering | `ferrum-ui` is presentation: it depends on `ferrum-app` and the infrastructure crates, nothing depends on it except `ferrum` and hosts. `ferrum-app` still builds without egui and wgpu. |
| Rendering rule | The shaders and `cpu/raycast.rs` stay in `ferrum-render`; `gpu_bridge.rs` moves unchanged. `gpu_parity.rs` is untouched. |
| Engines optional, licences shown | The AI widget moves with its licence line and `research_only` badge (24.5). |
| Agents propose, people confirm | No event is a decision (24.2-c). The review widget keeps Confirm/Reject; a host's own channel is `ferrum_agent::decisions` (16.7). |
| No identifiers leave FERRUM | Events hold voxel indices, millimetres, window values, segment labels and annotation ids, nothing from the DICOM header. A unit test serialises every event kind and runs the identifier scan over it. |
| Quality | `missing_docs`, no `unwrap`, the complexity budget apply to the new crate (workspace lints). Coverage counts it. |

## 3. Design

### 3.1 Crate layout

```
crates/ferrum-ui/
  src/lib.rs            re-exports egui, egui_wgpu; install(); ViewerWidget; ViewerEvent
  src/widget.rs         ViewerWidget: viewport layout (2D / MPR / 3D), toolbar, events
  src/events.rs         ViewerEvent, Point, Snapshot (before/after diff)
  src/gpu_bridge.rs     moved from ferrum
  src/views/            slice_view.rs, volume_view.rs, tf_editor.rs (moved)
  src/panels/           panels.rs (the tabbed settings panel), segments, segmentation,
                        ai, review, metrics_card (moved)
  src/theme.rs, widgets.rs (moved)
  examples/embed_host.rs
  tests/embed.rs
```

`ferrum` keeps `app.rs` (shell only), `ui/recent.rs` and `main.rs`, and
depends on `ferrum-ui`. Its `ui` module re-exports what its tests use, so
`crates/ferrum/tests/ui.rs` needs no change beyond paths (24.3-b).

Dependencies of `ferrum-ui`: `ferrum-app`, `ferrum-domain`,
`ferrum-render` (`gpu`), `ferrum-processing`, `ferrum-engines`,
`ferrum-radiomics`, `egui`, `egui-wgpu`, `wgpu`, `egui-phosphor`, `glam`,
`log`. Not `eframe`, not `rfd` (24.1-a; a test reads `cargo metadata`).
`eframe` is a dev-dependency for the example only.

### 3.2 Public API

```rust
/// Once per window, with the host's render state (24.1-b).
pub fn install(render_state: &egui_wgpu::RenderState);

pub struct ViewerWidget { /* view states, GPU sync, last snapshot, options */ }

impl ViewerWidget {
    pub fn new() -> Self;
    /// The desktop's tool bar above the views (default on) (24.1-c).
    pub fn with_toolbar(self, on: bool) -> Self;
    /// Draws the views of `viewer` into `ui`, polls the viewer, syncs the
    /// GPU when `render_state` is given, and returns this frame's events.
    pub fn show(&mut self, ui: &mut egui::Ui, viewer: &mut Viewer,
                render_state: Option<&egui_wgpu::RenderState>) -> Vec<ViewerEvent>;
}

/// The tabbed settings panel of the desktop (display, segments, AI,
/// review, metrics); each part is also a function of its own module.
pub struct SidePanel { /* PanelState */ }
impl SidePanel {
    pub fn show(&mut self, ui: &mut egui::Ui, viewer: &mut Viewer) -> Vec<PanelRequest>;
}

/// What a panel needs from the host: file dialogs stay with the host.
pub enum PanelRequest { ExportAnnotations, ImportLabels, ExportLabels }
```

`ferrum_ui::theme::install(ctx)` applies FERRUM's dark theme; the widget
calls it on first use unless the host opts out (`with_theme(false)`), so a
host can keep its own style.

Versions: `ferrum_ui::egui`, `ferrum_ui::egui_wgpu` and `ferrum_ui::wgpu`
re-export the versions FERRUM is built with; the crate docs name them
(Q4). A host on other versions gets a compile error, not a runtime one.

### 3.3 Viewer per window

One `Viewer` and one installed `VolumeRenderer` per window (out of
scope: several viewers). `install` registers the renderer in the egui-wgpu
callback resources, as the desktop does today. `show` without a render
state skips the GPU sync and draws the views' placeholders, so layout and
input work in CPU-only tests (24.1-b).

### 3.4 Events (24.2)

Events are derived from `Viewer` state, not raised inside the tools:
`show` takes a `Snapshot` before drawing and compares it with the state
after. This keeps every rule in `ferrum-app` and needs no event plumbing
through the tools.

| Event | Snapshot field | Emitted when |
|---|---|---|
| `PointPicked { point }` | `viewer.probe` (voxel) | the probe reading changed to a new voxel (Probe tool click) |
| `RoiDrawn { item }` | annotation ids; segment labels and mask revisions, by authorship | a new annotation or segment whose author is the person appeared, or the person's segment's mask changed (`item` is `Annotation(id)` or `Segment(label)`) |
| `SliceChanged { indices }` | `slices.indices` | the indices differ; once per frame with all three |
| `WindowChanged { center, width }` | `slices.window` | the window differs from the last one emitted **and** no pointer button is down (end of a drag) |
| `CameraChanged` | `volume.camera` | as for the window: after the drag or on a preset |

`Point` carries `voxel: [u32; 3]` and `patient_mm: [f64; 3]`
(`Volume::voxel_to_patient_f64` of the voxel centre), the forms `probe`
resolves (24.2-b). Events derive `serde`-free `Debug, Clone, PartialEq`;
#70 gives them a JSON form.

Engine proposals are not `RoiDrawn`: the author is the engine. A
decision is not an event (24.2-c).

### 3.5 Desktop after the move (24.3)

`ViewerApp::show` keeps the header, sidebar, status bar, dialogs and
start screen, and calls `ViewerWidget::show` in the central panel and
`SidePanel::show` in the right panel; `PanelRequest`s become the `rfd`
dialogs it opens today. The toolbar comes from the widget
(`with_toolbar(true)`), so it looks as before. The desktop ignores the
events.

### 3.6 Example host (24.4)

`examples/embed_host.rs` (eframe): a left panel with an event log and
three buttons that call `Viewer` use cases (next slice with
`set_slice_index`, lung window with `apply_window_preset(WindowPreset::Lung)`,
3D view with `set_view_mode`), the widget in the centre, the AI
widget below the log. It opens a file given on the command line or a
synthetic phantom. Its layout lives in `pub fn host_ui(...)`, which
`tests/embed.rs` includes with `#[path]` and drives with `egui_kittest`
(24.4-b, 24.5-b).

## 4. Contracts

| Contract | Change | Also update |
|---|---|---|
| New crate `ferrum-ui` | public API §3.2 | `docs/architecture.md` (crate graph, presentation layer), CLAUDE.md layering line, ADR 0016, CHANGELOG |
| `ferrum` | shell only | none beyond paths in its tests |
| `ferrum-agent/1`, `ferrum-engine/1`, workspace format | unchanged | — |

## 5. Risks and assumptions

- **Moving 4 000 lines can break behaviour unnoticed.** PR 1 is a pure
  move: no logic changes, the UI tests unchanged in what they check, and
  the PNG renders of `every_view_mode_renders_the_volume` compared before
  and after.
- **egui version lock.** A host must use FERRUM's egui; documented, and a
  compile error if not.
- **Event derivation misses a change made and undone within one frame.**
  Acceptable: the host sees the state it would see on screen.
- **`RoiDrawn` for an edited segment** fires on each mask revision of the
  person's own segment; a brush stroke gives one event per stroke (the
  mask revision changes once per stroke).

## 6. Validation plan

| Criterion | How | Level |
|---|---|---|
| 24.1-a | `ferrum_ui_has_no_shell_dependencies` (reads `cargo metadata`: no `eframe`, no `rfd` in normal deps) | test |
| 24.1-b | `the_widget_lays_out_without_a_gpu` (egui_kittest, no render state); the desktop's GPU renders on lavapipe | test |
| 24.1-c | `toolbar_and_panels_are_optional` | test |
| 24.2-a | `events_are_coalesced_per_frame` (scroll 5 slices in one frame: one `SliceChanged`; window drag: one `WindowChanged` after release) | test |
| 24.2-b | `a_pick_gives_the_voxel_probe_gives` | test |
| 24.2-c | `no_event_is_a_decision` (confirm in the review widget: no event; engine proposal: no `RoiDrawn`) | test |
| 24.3-a/b | the desktop UI tests pass; `gpu_parity.rs` passes; before/after PNGs of the view-mode renders identical | test, visual |
| 24.3-c | the diff has no change in `ferrum-render/src/shaders` or `cpu/` | review |
| 24.4-a/b | `the_example_host_receives_a_pick` | test |
| 24.5-a/b | `the_embedded_ai_widget_shows_licence_and_badge` | test |
| events privacy | `events_hold_no_identifiers` (identifier scan over every event's `Debug` text, on a study with a synthetic name) | test |
| success metric | example run on the user's machine (a real CT, a pick printed) | field, optional |

## 7. Size and PR split

About 4 000 moved lines plus ~800 new (widget, events, example, tests):
L. Two PRs:
1. **Move** (T1–T3): the crate, the desktop on it, no new behaviour.
2. **Embedding** (T4–T7): events, example host, badge test, docs.

## 8. Tasks

| Task | Criteria | PR |
|---|---|---|
| 24.T1 crate `ferrum-ui`; move views, panels, theme, widgets, GPU bridge | 24.1-a, 24.3-c | 1 |
| 24.T2 `ViewerWidget` (layout, toolbar, GPU sync), `SidePanel`, `PanelRequest` | 24.1-b/c | 1 |
| 24.T3 desktop on `ferrum-ui`; UI tests and parity unchanged; before/after renders | 24.3-a/b | 1 |
| 24.T4 `Snapshot` and events | 24.2-a/b/c | 2 |
| 24.T5 example host and its UI test | 24.4-a/b | 2 |
| 24.T6 licence and badge test in the embedded layout | 24.5-a/b | 2 |
| 24.T7 docs: architecture, CLAUDE.md, ADR 0016, CHANGELOG; events privacy test | — | 2 |
