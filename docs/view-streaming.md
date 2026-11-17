# The view everywhere: full streaming, the browser panel, hardening (Stage 29)

Design for [Stage 29](../dev_plan.md) (closes the out-of-scope of Stages 25–26
and the #91 follow-ups). G2 design; the rotating-token decision is
[ADR 0019](decisions/0019-view-host-rotating-token.md). Stage 28 gave the
`ferrum-view/1` ViewState contract and the in-process apply
([view-state.md](view-state.md), ADR 0018); this stage streams the full state to
other hosts, makes the browser panel a full client, and hardens the protocol
and the rendering rule.

## 1. Two consumers, one contract

There are two ways a harness shows FERRUM, and they share the ViewState:

1. **Host-rendered** (`<iframe>` of the host page, or `GET /api/frame.png` as an
   `<img>`). The host draws the pixels. Today it draws only a 2D slice; Stage 29
   adds the MPR grid and the 3D view, the camera, the overlay controls, the
   selected segment and the tool, and the `camera_changed`/`roi_drawn` events
   (29.1). This closes Stage 25's MPR/3D deferral.
2. **Browser-rendered** (the DeepSeek Harness panel, `ferrum-wasm` + WebGPU).
   The panel takes the voxels once and draws every frame on the person's GPU
   (144 fps measured, [webgpu-panel.md](webgpu-panel.md)). Stage 29 adds
   segments/overlay, the transfer-function editor and measurements (29.2), and
   synchronizes the panel with the host's state and events.

The contract is the same `ferrum-view/1` ViewState (Stage 28). The host applies
it with `Viewer::apply_view_state` (the full applier already exists); the panel
applies it to its local renderer. Neither adds image-formation code: the GPU
uses `slice.wgsl`/`volume.wgsl`, the CPU uses the one oracle (ADR 0014 d5).

## 2. The host streams the full state (29.1)

The host's 2D `state::ViewState` becomes a projection of the domain
`ferrum_domain::ViewState`; `apply_view_2d` is replaced by
`Viewer::apply_view_state` (the `unhonoured` list disappears — the host honours
everything). Concretely:

- **State** (`state_json`): `component` (`slice`/`mpr`/`volume`), `camera`
  (for mpr/volume), `overlays` (`segments_shown`, `fill_opacity`, `style`),
  `selected_segment`, `tool`, plus the existing plane/slice/window/probe. The
  `segments.items` list stays.
- **Input** (`POST /api/input`): the existing slice/step/plane/window/preset/
  segments/probe, plus `component`, `camera` (orbit/zoom/target), `overlays`
  (fill_opacity/style), `selected_segment`, `tool`, and per-segment
  visibility/opacity. One input object still sets one thing; the answer is the
  new state.
- **Events**: `camera_changed` and `roi_drawn` join the existing kinds. The
  host emits `roi_drawn` when a tool commits an annotation (the measurement
  path, §5); the domain `ViewEvent` already carries both.
- **Frames**: `FrameSource::render` grows from `(viewer, axis)` to the
  component. `slice` keeps the native-resolution 2D frame. `volume` renders the
  3D view (GPU `render_volume_image`, CPU `CpuRaycaster::render`) at a fixed
  output size; `mpr` renders the four quarters (axial/coronal/sagittal/3D) into
  one composite frame. The 3D mapping is a projection (camera + viewport), not
  the 2D affine `pixel_to_voxel`; the sidecar carries the camera and the
  `Viewer::pick` ray so a client can resolve a pixel. `Viewer::frame_params`
  and `Viewer::pick` already exist.
- **Parity** (29.1-b): `gpu_parity.rs` already covers the 3D modes and the
  slice overlay (GPU vs analytic). Add host-level CPU-vs-GPU cases for the
  volume and MPR frames (the `view_host.rs` mean-deviation pattern), on the one
  overlay rule of §6.

The page (`page.html`) gains the component switch, the camera drag in 3D, the
overlay controls and the tool selector; it stays self-contained.

**As built (PR 2):** the state is read live from `Viewer::view_state()`
through `ferrum_io::state_to_json` (no second copy in the host); every input
is a partial state applied by `Viewer::apply_view_state`, and the events are
the domain `ViewEvent`s, persisted without a JSON round trip. The 3D and MPR
frames are 512×512; the MPR grid mirrors the desktop's quarters (axial
top-left, 3D top-right, coronal bottom-left, sagittal bottom-right), each
slice quarter aspect-fit at an integer scale and blitted nearest by one
shared compositor. The GPU backend uploads through the `GpuSink` port
(`Viewer::sync_gpu`), the CPU 3D frame through the new
`Viewer::render_volume_cpu` use case (the one raycaster oracle, jitter off).
`roi_drawn` comes from a `draw` input that drives `Viewer::slice_input`
(distance/rect/angle/area) and saves the committed annotation with
`Viewer::save_workspace`. A 3D probe names the voxel one step behind the
pick along the ray (the trilinear surface sits between voxels). Host-level
CPU-vs-GPU parity for the volume and MPR frames lives in `view_host.rs`
(mean deviation + exact quarter geometry); `gpu_parity.rs` already pins the
underlying 3D modes and the slice overlay.

## 3. A practical wire (29.3)

A 512×512×252 study crosses as ~264 MB of `f32` today. Two changes:

- **16-bit transfer**: the volume's storage is already `u16`; serve it as
  little-endian `u16` with the rescale `slope`/`intercept` in the
  `X-Ferrum-Volume` header, and the client reconstructs the physical value
  (`storage × slope + intercept`). This halves the wire before compression and
  loses nothing (the storage is the source of truth). `GET /api/volume` keeps
  the `f32` form for a client that asks for it (`Accept: application/x-f32`),
  so the change is backward compatible.
- **Compression**: gzip the body (`flate2`, already in the workspace) when the
  client sends `Accept-Encoding: gzip`; the header describes the uncompressed
  grid. A CT compresses well; the panel decompresses in the fetch layer.
  **As built (PR 3):** the `u16` form alone halves the wire and satisfies
  29.3-a ("compressed *or* 16-bit"); gzip on top is deferred as a follow-up
  (it needs the test HTTP client's decompression handled).
- **Labels**: a new `GET /api/labels` serves the segment label map (`u8`, one
  byte per voxel, same grid) with the segment metadata, so the panel can draw
  the overlay (§5). It is cached per segmentation revision like the volume.

`/api/volume.json` stops recomputing the whole buffer + sha256 for the header
(it reads the cached description).

## 4. A rotating token (29.4, ADR 0019)

The token is static for the life of the process today. ADR 0019 decides a
short-lived, rotating token: the host issues a token with a TTL, a
`POST /api/token/refresh` (authorized by the current token) returns the next
one, and the panel refreshes before expiry. The initial token printed by
`view serve` still boots the session; the cookie is re-set on refresh. Loopback
only, no TLS (out of scope). The mechanism, TTL and grace window are the ADR's;
this design only fixes the route and the panel's refresh loop.

## 5. The browser panel is a full client (29.2)

The panel (`ferrum-wasm` + `integrations/dsh/plugin/lib/client.js`) renders
locally; Stage 29 wires what it lacks:

- **Segments/overlay (29.2-a)**: the renderer already has `update_labels` /
  `set_segment_colors`; the binding hardcodes `segments:false`. Add
  `set_labels(bytes)` + `set_segments(shown, fill_opacity, style)` +
  per-segment visibility/opacity, fed by `GET /api/labels`. The overlay uses the
  same `slice.wgsl`/`volume.wgsl` rule (no new image formation).
- **Transfer-function editor (29.2-b)**: add a `set_transfer_function(points)`
  binding (the domain edit-API — `move_point`/`set_color`/`insert_point`/
  `remove_point`/`hit_test` — already exists) and a JS editor in `client.js`
  (draggable points over a histogram, per-point colour; the desktop
  `tf_editor.rs` is the reference). The histogram is computed in wasm from the
  volume (`storage_histogram`). The TF stays **panel-local** in this stage: the
  ViewState contract does not carry it (adding it is a contract change deferred
  unless the person asks); the panel's TF is its own render setting.
- **Measurements (29.2-c)**: the tool machine lives in `ferrum-app`
  (`ToolController`, `Viewer::slice_input`). **Decision: run the tools in the
  panel** by adding `ferrum-app` to `ferrum-wasm` (the app compiles without
  wgpu/egui). The panel builds the `SliceContext` from its local volume/view/
  window and drives `ToolController` locally, so a drag previews at frame rate
  with no round trip. The committed annotation travels to the workspace (the
  host's `POST /api/input` tool path or the annotations artifact) and surfaces
  as `roi_drawn`. The panel draws annotations as a 2D overlay (canvas2d/DOM over
  the WebGPU canvas) using `Annotation::label()` and the mm→screen math of the
  layout. **Risk to verify first:** `ferrum-app`'s transitive `rayon` use must
  build for `wasm32` (the panel does not need parallel loading); if it blocks,
  the fallback is host-side tools via a new `Input::Tool` variant. The first
  panel PR verifies the wasm build before committing to the local path.
- **Sync**: after Connect the panel reads `/api/state`, applies it, polls
  `/api/events` (and the artifact watcher's effect through the host), and sends
  the person's input back with `POST /api/input`, so the agent and the panel
  agree on the view. Plane switch / window drag / probe in 2D and the MPR
  crosshair + per-plane scroll come with this sync.

## 6. One overlay rule; a tested WebGPU path (29.5)

- **CPU overlay consolidation (29.5-a)**: the 2D slice-overlay rule exists in
  three places — `slice.wgsl` (GPU), `ferrum-view/src/cpu.rs` (host CPU), and
  `ferrum-agent/src/commands/view.rs` (agent CPU, intentionally a thick
  outline). Extract one CPU slice-overlay primitive (edge detection + the
  domain `SegmentStyle::blend`) into `ferrum-render` (cpu), use it for the host
  CPU backend and a CPU slice renderer, and add a CPU-vs-GPU slice parity case
  to `gpu_parity.rs`. The agent's thick outline stays a distinct render style
  but shares the edge/blend primitive. After this, the slice rule is "one rule
  + parity", like the volume rule.
- **WebGPU automation (29.5-b)**: headless Chromium has no WebGPU adapter on
  this machine; a headed window does ([webgpu-panel.md](webgpu-panel.md)). Add
  an automated headed-browser test (Playwright/Chromium) that loads the panel,
  renders a phantom, and asserts pixels/fps — a field-style check run on the
  GPU machine, not a headless CI screenshot. It is gated behind an env flag so
  CI without a headed browser skips it (as `gpu_parity.rs` skips without an
  adapter).

## 7. PR split and sequencing (XL)

Seven PRs, ordered by dependency; the independent hardening/wire PRs can
interleave:

| PR | What | Criteria | Depends on |
|---|---|---|---|
| 1 | one CPU slice-overlay rule + CPU slice renderer + slice parity | 29.5-a | — |
| 2 | host full ViewState + MPR/3D frames + camera/roi events + parity | 29.1-a/b | 1 |
| 3 | 16-bit + gzip volume transfer + `/api/labels` | 29.3-a | — |
| 4 | rotating token (ADR 0019) | 29.4-a | — |
| 5 | panel segments/overlay + host sync (verify wasm build of app) | 29.2-a | 2, 3 |
| 6 | panel TF editor + measurements | 29.2-b/c | 5 |
| 7 | headed-browser WebGPU automation | 29.5-b | 5, 6 |

Each PR carries its own preflight, review and G3 report; merging waits for the
user's word per PR. PR 1 is foundational (the rendering rule), so it goes
first; PRs 3 and 4 are independent and can land while PR 2 is in review.

## 8. Verification

- `gpu_parity.rs`: the new CPU-vs-GPU slice case (PR 1) and the host volume/MPR
  cases (PR 2); the rendering rule mirrored in `volume.wgsl`/`cpu/raycast.rs`.
- `view_host.rs`: full state/input/events round trips, the rotating token
  (expiry + refresh), the 16-bit/gzip volume and labels, privacy (no
  identifiers in the new documents).
- `ferrum-wasm` host tests (`layout.rs`, `mode_names.rs`) plus the wasm build of
  `ferrum-app` (PR 5); `test_panel.py`, `smoke-load.cjs`, `smoke.js` updated.
- The headed-browser WebGPU test (PR 7), field-run on the GPU machine.
- Privacy: the new endpoints/documents hold numbers and labels only; the
  identifier scan covers them.

## 9. Out of scope

Remote exposure and TLS; several viewers per host; the TF in the ViewState
contract (panel-local unless asked); decisions through the view (they stay with
`ferrum_agent::decisions`). The "tests that cannot fail" left by #91 fold into
29.5 or a small track.