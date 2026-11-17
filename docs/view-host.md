# Local view host (`ferrum-view/1`) — design and protocol

Stage 25 ([#70](https://github.com/Kauter1989/ferrum/issues/70)). Spec and
criteria: `dev_plan.md` Stage 25. Decisions:
[ADR 0014](decisions/0014-generative-ui-view-host.md) (the view host as a
component) and [ADR 0017](decisions/0017-view-host-crate.md) (the crate and
the streaming backend).

## 1. Context

A harness shows FERRUM next to its conversation: a person scrolls a study,
changes the window, reads a value, and the agent learns what happened
without being in the render loop. Two things were missing for that:

1. a process that keeps a `Viewer` alive across requests (a CLI call cannot),
   and
2. a way for a GUI written in another language — the DeepSeek Harness is an
   Electron app — to show FERRUM's pixels.

Everything else existed: use cases in `ferrum-app`, `Viewer::slice_params`,
offscreen GPU rendering with readback in `ferrum-render`
(`render_slice_image`), the CPU oracle behind it, the `GpuSink` port and the
workspace store with change notifications.

## 2. Decision in one paragraph

`ferrum-view` holds one workspace, draws the view components on the local GPU
(wgpu, headless) or, when no adapter exists, with the CPU reference path, and
serves them over loopback with a small self-contained page and a JSON state.
A harness either embeds the page in an iframe or draws `/api/frame.png`
itself; both speak the same routes. The state is the full `ferrum-view/1`
ViewState (Stage 29): component, camera, overlays, selected segment and tool
included, applied through `Viewer::apply_view_state`. A slice frame is at the
volume's native resolution — one pixel per voxel — so a pixel and a voxel map
one to one; the 3D and MPR frames are at a fixed size and carry a projection
mapping. The two backends are comparable pixel by pixel.

## 3. Layering

```
ferrum-cli → ferrum-view → ferrum-app → ferrum-domain
                        ↘ ferrum-render (gpu), ferrum-processing   ← ferrum-io (workspace)
```

- `ferrum-view` is the only crate that knows about HTTP and the token.
- Every operation is a `Viewer` use case; the crate holds no business logic.
  The GPU backend implements the `GpuSink` port (whose signature names
  `ferrum-processing`'s `AmbientOcclusion`); `Viewer::sync_gpu` decides what
  to upload, the sink only forwards — the same glue as `ferrum-ui`.
- The image-formation model stays where it is: `slice.wgsl` / `volume.wgsl`
  on the GPU and `SliceImage` / `cpu/raycast.rs` on the CPU. `ferrum-view`
  adds no third renderer (ADR 0014 decision 5); the MPR compositor is a
  display blit shared by both backends, not image formation.
- `ferrum-agent` does not depend on `ferrum-view`; the agent command set is
  unchanged in this stage.

## 4. Protocol

Base URL: `http://127.0.0.1:<port>`, printed by `ferrum-cli view serve` on
stdout as one JSON object before the first request:

```json
{"url":"http://127.0.0.1:41234","token":"8f3c…","workspace":"/data/ws/ct1","backend":"gpu"}
```

### 4.1 Token

Every request needs the token, in any of:

- `?token=<token>` (the page gets it in its URL and passes it on),
- `Authorization: Bearer <token>`,
- the cookie `ferrum-view=<token>`, set by `GET /` with `HttpOnly` and
  `SameSite=Strict`.

A missing or wrong token is `401` with `{"error":"token required"}` and
nothing else. The token never appears in a log line, in the workspace or in
a frame. It is 32 hex digits from the operating system's random source.

The token is **short-lived and rotates** ([ADR 0019](decisions/0019-view-host-rotating-token.md)):
it is valid for 15 minutes (`expires_in` in the boot line and the refresh
answer). `POST /api/token/refresh`, authorized by a live token, returns the next
one — `{"token":"<32 hex>","expires_in":900}` — and re-sets the cookie. The
previous token stays valid for a 60-second grace window, so requests in flight
and a panel mid-refresh do not `401`; at most two tokens are live at once, both
compared in constant time. An expired token authorizes nothing and cannot
refresh. The panel refreshes proactively (at ~80% of the TTL); the boot token
printed by `view serve` only starts the session. A harness that draws
`/api/frame.png` with the boot token must refresh too, or reconnect after the
TTL.

### 4.2 Routes

| Method | Path | Answer |
|---|---|---|
| `GET` | `/`, `/index.html` | the self-contained page (`text/html`), sets the cookie |
| `GET` | `/api/state` | the state document below, `application/json` |
| `GET` | `/api/frame.png` | the state component's frame (`slice` at native resolution, `volume`/`mpr` at 512×512), `image/png`; header `X-Ferrum-Rev` |
| `GET` | `/api/events?since=<seq>` | events with `seq` greater than `since` |
| `GET` | `/api/annotations` | the measurements the tool machine committed, with their values (4.6, 29.2-c) |
| `POST` | `/api/input` | one input object; answers with the new state |
| `POST` | `/api/token/refresh` | a fresh token `{"token","expires_in"}`; re-sets the cookie (4.1) |
| `GET` | `/api/volume` | the voxels as little-endian `u16` storage by default (`?storage=f32` for physical `f32`), `application/octet-stream`; the grid, spacing, range and storage form travel in `X-Ferrum-Volume` as JSON (26.3-a, 29.3-a). The client reconstructs `min + stored × (max − min) / 65535` |
| `GET` | `/api/volume.json` | the same description as `application/json`, with `sha256` of the voxels |
| `GET` | `/api/labels` | the segment label map as `u8` (one per voxel), `application/octet-stream`; grid and segment metadata in `X-Ferrum-Labels` (29.2-a). A study without segmentation gives a zero grid |
| `GET` | `/api/labels.json` | the same description as `application/json`, with `sha256` |
| `GET` | `/panel/<file>` | the browser renderer module (`ferrum_wasm.js`, `ferrum_wasm_bg.wasm`); **no token** (26.3-b) |

Every `GET` answer other than `/panel/<file>` needs the token. `/panel/<file>`
serves code, not data — a browser cannot attach a token to a module import —
so it needs none, and the route serves only those two literal file names from
fixed directories: anything else is `404`, never a path on disk.

An unknown path is `404`, a known path with the wrong method is `405`, a body
over 64 KiB is `413`, an unparseable or invalid input is `400` — in every
case with `{"error": "…"}` and no state change. Every answer has
`Cache-Control: no-store`, and only a successful (`2xx`) answer carries
`Access-Control-Allow-Origin: *`, so a page on another origin can read an
authorised answer but cannot use a refusal to discover that a host is running.

### 4.3 State

The full `ferrum-view/1` ViewState (Stage 28's contract, streamed since
Stage 29): the same fields the workspace artifact and the agent's `view set`
carry, serialised by the one JSON contract of `ferrum-io`, plus the host's
revision, slice count, segment list, volume description and last frame.

```json
{
  "format": "ferrum-view/1",
  "rev": 7,
  "component": "mpr",
  "plane": "axial",
  "slice_index": 119,
  "slice_count": 252,
  "window": { "center": 40.0, "width": 400.0 },
  "overlays": { "segments_shown": true, "fill_opacity": 1.0, "style": "fill_outline" },
  "selected_segment": 1,
  "tool": "pan",
  "camera": {
    "orientation": [0.0, 0.0, 0.0, 1.0],
    "distance": 1.6,
    "target": [0.0, 0.0, 0.0],
    "fov_y": 1.0472
  },
  "segments": {
    "shown": true,
    "items": [
      { "label": 1, "name": "liver", "color": [230, 85, 75], "visible": true, "opacity": 0.5, "voxels": 18234 }
    ]
  },
  "volume": {
    "dims": [512, 512, 252],
    "spacing_mm": [0.78, 0.78, 1.25],
    "value_unit": "HU",
    "value_range": [-1024.0, 3071.0]
  },
  "probe": { "voxel": [251, 198, 87], "patient_mm": [12.4, -40.1, 118.0], "value": 55.0, "unit": "HU" },
  "frame": {
    "rev": 7,
    "size": [512, 512],
    "mapping": { "kind": "mpr", "size": [512, 512], "quarters": ["…see §5"] }
  }
}
```

- `component` is `slice`, `mpr` or `volume`. `camera` is present for `mpr`
  and `volume` (null for `slice`); `orientation` is the quaternion
  `x, y, z, w`, `target` model space (the volume box's longest side is 1),
  `fov_y` radians.
- `overlays.style` is `outline`, `fill` or `fill_outline`; `fill_opacity`
  multiplies each segment's opacity on slices. `selected_segment` is a label
  or null; `tool` is one of the contract's wire names (`pan`, `window_level`,
  `probe`, `distance`, `angle`, `area`, `rect`, `text`, `move`, `delete`,
  `ai_point`, `ai_box`, `ai_scribble`, `ai_lasso`, `region`).
- `rev` increases on every change; a frame carries the revision it was drawn
  for, so a client can tell a stale image from a fresh one.
- `frame` is present once a frame was drawn; its mapping depends on the
  component (§5): the 2D affine for a slice, a projection for the 3D view,
  per-quarter mappings for MPR.
- `value_unit` is `HU` for a CT and `raw` otherwise: a NIfTI without a
  declared modality has no unit.
- No path, UID, patient name or header value appears here (25.5-a).
- `segments.items` holds labels, names, colours, visibility, opacity and
  voxel counts; the overlay is drawn when `segments.shown` is true
  (`overlays.segments_shown` is the same flag).
- There is no `unhonoured` list: the host honours every field of the
  contract (Stage 29; the 2D subset of Stage 28 is gone).

### 4.4 Input

`POST /api/input` takes exactly one of:

| Object | Effect |
|---|---|
| `{"slice": 12}` | show slice 12 (0-based, clamped) |
| `{"step": ±n}` | move by `n` slices; one wheel notch is ±1 |
| `{"plane": "axial"}` | `axial`, `coronal` or `sagittal` |
| `{"window": {"center": -600, "width": 1500}}` | set the window (width > 0) |
| `{"preset": "lung"}` | `full_range`, `brain`, `soft_tissue`, `lung`, `bone` |
| `{"segments": true}` | show or hide the overlay |
| `{"probe": {"x": 300, "y": 210}}` | read the voxel under that frame pixel of the current component (§5) |
| `{"component": "mpr"}` | `slice`, `mpr` or `volume` |
| `{"camera": {"orbit": [0.3, -0.2]}}` | orbit by radians around the camera's up/right axes |
| `{"camera": {"zoom": 1.0}}` | multiply the distance by `exp(-amount)` (positive zooms in) |
| `{"camera": {"target": [0.1, 0, 0]}}` | move the orbit target (model space) |
| `{"overlays": {"fill_opacity": 0.4}}` | set the fill opacity (0..1) and/or `style` (`outline`, `fill`, `fill_outline`); untouched fields stay |
| `{"selected_segment": 1}` | select a segment by label; `null` clears |
| `{"tool": "distance"}` | activate a tool by its wire name |
| `{"segment": {"label": 1, "visible": false}}` | per-segment visibility and/or `opacity` (0..1) |
| `{"draw": {"tool": "distance", "points": [[4, 4], [7, 6]]}}` | draw an annotation on the active plane's slice (§5) |

A camera object holds exactly one of `orbit`, `zoom`, `target`. Every input
becomes a partial ViewState applied by `Viewer::apply_view_state` (the full
applier): a field equal to the current one produces no event, the slice index
is clamped, and the person's next input wins — nothing is locked.

The answer is the new state document, so a client can update its controls and
the frame in one round trip.

### 4.5 Events

```json
{
  "format": "ferrum-view/1",
  "events": [
    { "seq": 1, "kind": "slice_changed", "plane": "axial", "slice_index": 120 },
    { "seq": 2, "kind": "camera_changed",
      "camera": { "orientation": [0, 0, 0, 1], "distance": 1.6, "target": [0, 0, 0], "fov_y": 1.0472 } },
    { "seq": 3, "kind": "roi_drawn", "item": { "kind": "annotation", "id": 0 } },
    { "seq": 4, "kind": "point_picked", "voxel": [251, 198, 120],
      "patient_mm": [12.4, -40.1, 88.1], "value": 55.0, "unit": "HU" }
  ],
  "next": 4,
  "dropped": 0
}
```

Kinds: `slice_changed`, `plane_changed`, `window_changed`,
`segments_changed`, `camera_changed`, `roi_drawn`, `point_picked` — the
domain `ViewEvent`'s own, persisted to the workspace's `view-events.json`
without a round trip, so the agent's `view events` reads the same sequence.
`camera_changed` carries the new camera; `roi_drawn` names the committed
annotation (`{"kind": "annotation", "id"}`) or segment
(`{"kind": "segment", "label"}`). Sequence numbers increase; the host keeps
the last 512 events and counts the dropped ones. Events hold numbers and
labels only, and `point_picked` carries the same voxel and value `probe`
gives. No event is a decision.

### 4.6 Annotations

`GET /api/annotations` serves the measurements the host's tool machine
committed (the `draw` input, §4.4), so a panel that sends `draw` can display
the value the host computed — distance in mm, angle in degrees, area in mm²
(Stage 29, criterion 29.2-c). The route refreshes the workspace first, so
annotations an agent wrote are adopted the way segments are.

```json
{
  "format": "ferrum-view/1",
  "annotations": [
    { "id": 0, "name": "Distance 1", "kind": "Distance", "plane": "axial",
      "slice_index": 119, "value": 12.3, "unit": "mm", "label": "12.3 mm",
      "points_mm": [[40.5, 60.5], [52.5, 63.5]] }
  ]
}
```

- `kind` is the domain's own (`Distance`, `Angle`, `Rectangle`, `Area`,
  `Text`); `value`/`unit`/`label` come from `Annotation::value/unit/label`
  (`mm`, `deg`, `mm2`; `label` prints areas in cm²). `value` is null for text.
- `points_mm` are in-plane millimetres from the top-left corner of the
  displayed slice (x right, y down) — the same frame the `draw` input's
  pixels map into, so a client can draw the shape over its own slice.
- `roi_drawn` names the committed annotation's `id`; this document resolves
  it to a value. Numbers and labels only: no path, UID or header value
  (25.5-a).

## 5. Frames

`GET /api/frame.png` draws the state's `component`:

- **`slice`** — native resolution: one pixel per voxel of the plane, no
  letterbox, nearest sampling, opaque background. The client scales the
  image. The mapping is the 2D affine: `pixel_to_voxel` and
  `pixel_to_patient_mm` take `(x, y, 1)` to voxel and to patient millimetres,
  exactly as a render's sidecar does ([agent-cli.md](agent-cli.md)), with
  `plane`, `slice_index`, `size`, `pixel_mm` and `orientation`.
- **`volume`** — the 3D view at a fixed 512×512 (the CPU reference's time
  grows with the pixel count). GPU: `render_volume_image`; CPU:
  `Viewer::render_volume_cpu` (the one raycaster oracle `volume.wgsl`
  mirrors). Jitter is off on both sides, so the frames are deterministic and
  comparable. The mapping is a **projection**, not an affine:

  ```json
  {
    "kind": "projection",
    "size": [512, 512],
    "camera": { "orientation": [0, 0, 0, 1], "distance": 1.6, "target": [0, 0, 0], "fov_y": 1.0472 },
    "pick_ray": { "origin": [0, 0, -1.6], "direction": [0, 0, 1] },
    "pick": { "voxel": [251, 198, 87], "patient_mm": [12.4, -40.1, 118.0], "value": 55.0, "unit": "HU" }
  }
  ```

  A client resolves any pixel `(x, y)` to a ray with the camera at
  `ndc = (2(x+0.5)/w − 1, 1 − 2(y+0.5)/h)`, `aspect = w/h` (the domain
  `OrbitCamera::ray`); `pick_ray` is that ray through the frame's centre and
  `pick` the voxel it hits (null when it hits nothing), so a client has an
  anchor without doing the math.
- **`mpr`** — the four quarters in one 512×512 composite, laid out like the
  desktop grid: axial top-left, 3D top-right, coronal bottom-left, sagittal
  bottom-right. Each slice quarter is drawn at native resolution and fitted
  into its quarter keeping the plane's aspect (integer scale, nearest blit,
  opaque black letterbox — never stretched); the 3D quarter fills its rect.
  Both backends tile with the one shared compositor; no quarter adds image
  formation. The mapping names every quarter:

  ```json
  {
    "kind": "mpr",
    "size": [512, 512],
    "quarters": [
      { "component": "slice", "plane": "axial", "rect": [0, 0, 256, 256],
        "image_rect": [0, 32, 256, 224], "mapping": { "…the slice affine" } },
      { "component": "volume", "rect": [256, 0, 512, 256], "mapping": { "…the projection" } }
    ]
  }
  ```

  `rect` is the quarter in composite pixels; `image_rect` is where the
  native image lands inside it. A composite pixel maps to a native slice
  pixel by `(p − image_rect.min) × native_size / image_rect.size`.

Mechanics:

- A frame is redrawn only when the state changed since the last one, so a
  scroll that coalesces several notches renders once.
- The GPU backend pushes the viewer's data through the `GpuSink` port
  (`Viewer::sync_gpu` decides what changed — volume, transfer function,
  occupancy, mask, AO, labels, segment colours — the sink forwards, as in
  `ferrum-ui`), then renders and reads back.
- The CPU backend draws `SliceImage::extract` + `WindowLevel::apply` (the
  same oracle `gpu_parity.rs` uses) and applies the one segment-overlay rule
  (`ferrum_render::cpu::slice_overlay`, mirroring `slice.wgsl`): a pixel in full
  segment colour where a four-neighbour label differs, otherwise the fill
  blended with `opacity × fill_opacity`. The 3D view goes through
  `Viewer::render_volume_cpu`, the same scene the GPU holds.
- The two backends are compared in `crates/ferrum-view/tests/view_host.rs`
  (mean deviation and the four outline sides) for the slice, the volume and
  the MPR composite, which also pins the CPU slice output to the oracle
  exactly and the MPR quarters to exact pixels on the block phantom.

Probe and draw:

- `{"probe": {x, y}}` reads the voxel under a frame pixel of the current
  component: the slice affine on `slice`, `Viewer::pick` on `volume` (the
  pick lands on the rendered surface, which trilinear sampling places
  between voxels; the probe names the voxel one step behind it along the
  ray, so a click on a structure reads that structure's value), and the
  quarter's own mapping on `mpr`. A pixel outside the image — or a ray that
  hits nothing — is `400`.
- `{"draw": {tool, points}}` drives the real tool machine
  (`Viewer::slice_input`) on the active plane's slice: `distance`/`rect`
  take two pixels (press, release), `angle` three, `area` the polygon's
  corners (the host clicks the first point again to close). Points are
  pixels of that plane's native-resolution slice. A commit records
  `roi_drawn` and saves the annotation into the workspace; a degenerate
  draw (below the tool's minimum size) commits nothing. The measurement
  tools draw in the `slice` component; the page's other components keep
  probe/orbit behaviour.

## 6. Privacy and limits

- Loopback only; the host binds `127.0.0.1` and has no option to bind
  elsewhere in this stage.
- The token is the only credential; a wrong one changes nothing.
- Documents hold voxel indices, patient millimetres, window values, segment
  labels and names, and annotation values and names. A test scans the state,
  the events, the annotations and the page for the study's file name and
  folder and finds none of them.
- Bodies are capped at 64 KiB; the event log at 512 entries; the page is one
  static document.
- The host sends no request to any other origin.

## 7. How a harness mounts it

Two ways, both documented by the same routes:

1. **Embed the page.** An `<iframe src="http://127.0.0.1:PORT/?token=…">`
   gets a working viewer in one line in a normal browser page. In the
   DeepSeek Harness desktop window the iframe stays black: the window is
   a secure context and Chromium refuses `http://` content there (L30) —
   the harness panel renders the study itself through WebGPU instead
   (`integrations/dsh/`, Stage 26).
2. **Draw it yourself.** `GET /api/frame.png` is an `<img>` source; input
   goes back with `POST /api/input`. The mapping block in the state resolves
   a pixel to a voxel without guessing.

`integrations/dsh/` holds the working example for the DeepSeek Harness
(criterion 25.6, the second PR of this stage).

## 8. Size and PR split

Size L, two PRs:

1. the crate, the command, the page, the protocol and its tests (this PR);
2. the DeepSeek Harness plugin under `integrations/` and the demo steps.

## 9. What is not here

The MPR crosshair navigation (clicking one plane moves the other two — the
desktop's `navigate_to`; the host's MPR scrolls the active plane); the
transfer function in the ViewState contract (panel-local, Stage 29's design
§5); decisions through the view (they stay with `ferrum_agent::decisions`);
remote exposure and TLS; several viewers per host; an agent command for the
events. The full state, the MPR grid and the 3D view themselves are here
since Stage 29 PR 2 (29.1).


