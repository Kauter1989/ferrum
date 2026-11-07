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

`ferrum-view` holds one workspace, draws slices on the local GPU (wgpu,
headless) or, when no adapter exists, with the CPU reference path, and serves
them over loopback with a small self-contained page and a JSON state. A
harness either embeds the page in an iframe or draws `/api/frame.png` itself;
both speak the same routes. Frames are at the volume's native resolution —
one pixel per voxel — so a pixel and a voxel map one to one and the two
backends are comparable pixel by pixel.

## 3. Layering

```
ferrum-cli → ferrum-view → ferrum-app → ferrum-domain
                        ↘ ferrum-render (gpu)   ← ferrum-io (workspace)
```

- `ferrum-view` is the only crate that knows about HTTP and the token.
- Every operation is a `Viewer` use case; the crate holds no business logic.
- The image-formation model stays where it is: `slice.wgsl` on the GPU and
  `SliceImage` / `cpu/raycast.rs` on the CPU. `ferrum-view` adds no third
  renderer (ADR 0014 decision 5).
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

### 4.2 Routes

| Method | Path | Answer |
|---|---|---|
| `GET` | `/`, `/index.html` | the self-contained page (`text/html`), sets the cookie |
| `GET` | `/api/state` | the state document below, `application/json` |
| `GET` | `/api/frame.png` | the slice at native resolution, `image/png`; header `X-Ferrum-Rev` |
| `GET` | `/api/events?since=<seq>` | events with `seq` greater than `since` |
| `POST` | `/api/input` | one input object; answers with the new state |
| `GET` | `/api/volume` | the voxels as little-endian `f32`, `application/octet-stream`; the grid, spacing and range travel in `X-Ferrum-Volume` as JSON (26.3-a) |
| `GET` | `/api/volume.json` | the same description as `application/json`, with `sha256` of the voxels |
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

```json
{
  "format": "ferrum-view/1",
  "rev": 7,
  "view": "slice",
  "plane": "axial",
  "slice_index": 119,
  "slice_count": 252,
  "window": { "center": 40.0, "width": 400.0 },
  "segments": {
    "shown": true,
    "items": [
      { "label": 1, "name": "liver", "visible": true, "opacity": 0.5, "voxels": 18234 }
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
    "mapping": {
      "plane": "axial",
      "slice_index": 119,
      "size": [512, 512],
      "pixel_mm": 0.78,
      "pixel_to_voxel": [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 119.0]],
      "pixel_to_patient_mm": [[0.78, 0.0, -199.0], [0.0, 0.78, -199.0], [0.0, 0.0, 148.0]],
      "orientation": { "left": "R", "right": "L", "top": "A", "bottom": "P" }
    }
  }
}
```

- `rev` increases on every change; a frame carries the revision it was drawn
  for, so a client can tell a stale image from a fresh one.
- `frame` is present once a frame was drawn; the mapping takes `(x, y, 1)`
  to voxel and to patient millimetres, exactly as a render's sidecar does
  ([agent-cli.md](agent-cli.md)).
- `value_unit` is `HU` for a CT and `raw` otherwise: a NIfTI without a
  declared modality has no unit.
- No path, UID, patient name or header value appears here (25.5-a).
- `segments.items` holds labels, names, visibility, opacity and voxel
  counts; the overlay is drawn when `segments.shown` is true.

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
| `{"probe": {"x": 300, "y": 210}}` | read the voxel under that frame pixel |

The answer is the new state document, so a client can update its controls and
the frame in one round trip.

### 4.5 Events

```json
{
  "format": "ferrum-view/1",
  "events": [
    { "seq": 1, "kind": "slice_changed", "plane": "axial", "slice_index": 120 },
    { "seq": 2, "kind": "point_picked", "voxel": [251, 198, 120],
      "patient_mm": [12.4, -40.1, 88.1], "value": 55.0, "unit": "HU" }
  ],
  "next": 2,
  "dropped": 0
}
```

Kinds: `slice_changed`, `plane_changed`, `window_changed`,
`segments_changed`, `point_picked`. Sequence numbers increase; the host keeps
the last 512 events and counts the dropped ones. Events hold numbers and
labels only, and `point_picked` carries the same voxel and value `probe`
gives.

## 5. Frames

- Native resolution: one pixel per voxel of the plane, no letterbox, nearest
  sampling, opaque background. The client scales the image.
- A frame is redrawn only when the state changed since the last one, so a
  scroll that coalesces several notches renders once.
- The GPU backend uploads the volume once per dataset revision and the label
  map once per segmentation revision, then renders and reads back.
- The CPU backend draws `SliceImage::extract` + `WindowLevel::apply` (the
  same oracle `gpu_parity.rs` uses) and repeats the overlay rule of
  `slice.wgsl`: a pixel in full segment colour where a four-neighbour label
  differs, otherwise the fill blended with `opacity × fill_opacity`.
- The two backends are compared in `crates/ferrum-view/tests/view_host.rs`
  (mean deviation and the four outline sides), which also pins the CPU
  output to the oracle exactly.

## 6. Privacy and limits

- Loopback only; the host binds `127.0.0.1` and has no option to bind
  elsewhere in this stage.
- The token is the only credential; a wrong one changes nothing.
- Documents hold voxel indices, patient millimetres, window values, segment
  labels and names. A test scans the state, the events and the page for the
  study's file name and folder and finds none of them.
- Bodies are capped at 64 KiB; the event log at 512 entries; the page is one
  static document.
- The host sends no request to any other origin.

## 7. How a harness mounts it

Two ways, both documented by the same routes:

1. **Embed the page.** An `<iframe src="http://127.0.0.1:PORT/?token=…">`
   gets a working viewer in one line. The DeepSeek Harness has no CSP that
   would refuse it, and the page makes no cross-origin request.
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

MPR crosshair, the 3D volume view and the other `ferrum-view/1` components
(Stage 26, on the same interfaces); decisions through the view (they stay
with `ferrum_agent::decisions`); a WASM/WebGPU client; remote exposure and
TLS; several viewers per host; an agent command for the events.


