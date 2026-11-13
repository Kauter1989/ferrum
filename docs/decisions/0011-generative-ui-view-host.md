# ADR 0011 — Interactive view host for generative UI

- **Status:** Draft
- **Date:** 2026-10-05

## Context

In an agent harness the interface is generated: the orchestrating model
decides which component to show next to its text. FERRUM is already usable
by an agent ([ADR 0008](0008-agent-skill.md)), but only as a sequence of
calls. The agent renders a PNG, a person (or the harness) turns a click into
`r-0003:x,y`, and the agent calls `probe`. Every interaction passes through
the model. That is too slow and too costly for the work FERRUM is for:
scrolling a stack, changing the window, moving an MPR crosshair, rotating a
volume, reviewing a segment.

The goal of this ADR is **interactive work inside a generated interface**:
the agent mounts a FERRUM component in the conversation, the person works
in it directly and smoothly, and the agent learns about the outcome (a
picked point, a drawn region, a decision) without being in the render loop.

What already exists and is reused:

- All use cases live in `ferrum-app`; adapters stay thin.
- `ferrum-agent/1` commands with JSON Schemas, over a CLI and MCP.
- Workspaces hold the shared state: annotations, segments, provenance,
  audit log. The desktop app opens the same workspace.
- Renders have an exact pixel-to-voxel and pixel-to-patient-mm mapping.
- The CPU reference renderer reproduces the GPU image; parity is tested in
  `gpu_parity.rs`. `ferrum-app` and `ferrum-render` (without `gpu`) have no
  wgpu or egui dependency.
- Agents propose, people confirm. Values come from voxels. No patient
  identifiers leave FERRUM by default.

Constraints specific to this problem:

- The harness may run on a laptop with a GPU, in a browser, on a server
  without a GPU, or on a phone. One rendering path does not fit all.
- Pixel data of a study is sensitive. Which pixels go to the *model* and
  which only to the *person* must be a decision of the operator.
- A click on "confirm" must not be something an agent can produce.

## Decision

1. **A component, not a window.** FERRUM is offered to a harness as a small
   catalogue of interactive components (initially `SliceView`, `MprView`,
   `VolumeView`, `Montage`, `SegmentReview`, `MeasureTable`, `Compare`). The
   agent mounts one with a command; the harness shows it where the
   conversation needs it. The desktop app stays the full-featured client of
   the same state.

2. **One state document, `ferrum-view/1`.** A component is described by a
   `ViewState` (component, workspace, plane, slice or point, window,
   overlays, camera, selected segment, tool). It is a plain JSON value
   with a JSON Schema, versioned like `ferrum-agent/1`.
   - The agent sends a `ViewState` (`view open`, `view set`) and the
     component follows it.
   - The component sends back *events*, not frames: `point_picked`,
     `window_changed`, `slice_changed`, `roi_drawn`, `camera_changed`,
     `decision`. Points are always in the existing point forms, with the
     render id and the pixel, so they resolve through the same mapping.
   - Events are coalesced by the host; the agent receives the latest state
     and meaningful events (picks, ROI, decisions), not every mouse move.
   - Both directions are new commands in `ferrum-agent` and are served by
     the CLI and MCP with identical JSON, as for every command.

3. **A view host in a new crate, over `ferrum-app`.** The new crate
   `ferrum-view` owns sessions: it keeps a loaded volume, the `ViewState`
   and the frame producer alive across calls, which per-call CLI use cannot
   do. It depends on `ferrum-app`, `ferrum-agent` (commands), `ferrum-io`
   and `ferrum-render` (no `gpu` feature unless the server opts in). It
   contains no business logic: every operation is a `Viewer` use case. The
   MCP server (`ferrum-cli mcp`) gains a live-session mode that uses it.

4. **Two rendering backends behind one frame interface; the host picks
   one per session.** The choice depends on where a usable GPU is.

   | Backend | Where pixels are made | Chosen when |
   |---|---|---|
   | **Local (WASM + wgpu/WebGPU)** | In the person's browser or webview, from the volume downloaded to the client | The client has WebGPU, the volume fits the client budget, and the operator allows the volume to leave the server |
   | **Streaming** | On the host (GPU if present, otherwise the CPU reference renderer); frames go to a thin client | The client has no WebGPU or is weak, the volume is large, or the operator forbids sending the volume to the client |

   Both implement the same trait (`FrameSource`: given a `ViewState`,
   produce a frame and its pixel mapping). The selection is automatic and
   can be overridden:
   - capability negotiation at mount time (client reports WebGPU, adapter
     limits, memory, network round-trip; host reports GPU, volume size,
     policy);
   - local is preferred when allowed and capable, because interaction then
     costs no round trip; streaming is the fallback and the default for
     policy-restricted data;
   - the backend can switch during a session (for example local →
     streaming if the device is lost) without changing the `ViewState`.

   Because the CPU reference renderer reproduces the GPU image, a streamed
   frame and a locally rendered frame agree, and a CPU-only server is a
   supported streaming host, not a degraded one.

5. **The rendering rule extends, it does not fork.** The WASM client uses
   `volume.wgsl` unchanged and the same image-formation model. The parity
   suite gains a case that compares the WASM/WebGPU output to the CPU
   reference; no third implementation is allowed. Any change to the shader
   or `cpu/raycast.rs` is still mirrored and covered by `gpu_parity.rs`.

6. **Streaming details.**
   - Transport: a WebSocket (or the harness's own channel) carrying JPEG or
     WebP/AV1 frames for interaction and a lossless PNG after the
     interaction settles, so that what stays on screen is exact. Each frame
     carries its mapping, so a pick is resolved on the host against the
     frame the person saw.
   - Interaction renders may use reduced resolution (the existing dynamic
     resolution for 3D); the settled frame is full quality.
   - Back-pressure: the host renders only the latest `ViewState`; stale
     requests are dropped.

7. **Local client details.**
   - `ferrum-domain` and the parts of `ferrum-app` the client needs must
     build for `wasm32-unknown-unknown` without `ferrum-io`; the client
     receives a compact volume (decoded, canonical LPS, with its geometry),
     not DICOM files, so no reader runs in the browser and nothing parses
     files outside `ferrum-io`.
   - The client computes only what the component needs to draw. Values
     shown as numbers still come from the host through `probe`, `stats` and
     `measure` (decision 9), so measurement code exists once.

8. **Humans decide through their own channel.** `decision` events
   (confirm, reject) are produced only by a person's action in the
   component and are delivered to the host over a channel the agent has no
   command for. The host writes them to the workspace (provenance, audit
   log) and tells the agent the outcome. An agent cannot synthesise a
   `decision`. This replaces reliance on the operator switch for
   "harness review" in the generative setting; the existing switch keeps
   its meaning for CLI-only harnesses.

9. **Values come from voxels, also here.** The component may show a cursor
   readout, but its source is a host call (`probe`, `profile`) or, in
   local mode, a lookup in the client's copy of the same voxels, labelled
   with its unit. Pixel colours are never the source of a number. Agents
   still take numbers from tools, not from frames.

10. **Privacy and egress are explicit policy.** The operator configuration
    gains a `view` section with, at least:
    - `volume_to_client`: allow or forbid the local backend;
    - `frames_to_model`: whether rendered frames or only events and numbers
      are returned to the model. Default: only events and numbers; a frame
      goes to the model only when the agent explicitly asks for a render
      (the existing `view slice` path), as today;
    - `bind`: host and port. Default is loopback with a per-session token;
      remote exposure is opt-in and requires TLS in front.
    Frames, state and events contain no identifiers; the output scan in
    `tests/privacy.rs` covers them. Overlays and labels are numbers and
    plane letters only, as for renders.

11. **Stale state is detected, not trusted.** Each `ViewState` carries the
    workspace source hash (the existing `source_changed` check). If the
    series changed under a session, the host stops and reports
    `source_changed`.

12. **Optional engines stay optional.** AI tools in a component are visible
    but disabled without an engine and show the engine's licence
    (`research_only`), exactly as in the desktop app.

## Alternatives considered

- **Only the desktop app as the interactive client** (open the workspace
  window beside the harness). Cheap and useful as a first step, but it
  leaves the generated interface and breaks on remote, browser and mobile
  harnesses. It remains the full client and the fallback of last resort.
- **Streaming only.** Simplest and policy-friendly, but each interaction
  pays a round trip, and a GPU server is needed per active session for
  3D. Kept as one of two backends.
- **WASM only.** Best latency, but excludes devices without WebGPU, forces
  the volume onto the client, and does not suit restrictive data policies.
  Kept as one of two backends.
- **Returning a frame to the model on every interaction.** Gives the model
  full awareness, but costs tokens, latency and data egress, and invites
  reading numbers from pixels. Rejected: the model receives events and
  numbers, and asks for a render when it needs to look.
- **A new rendering implementation in JavaScript** (WebGL, three.js).
  Rejected: a third image-formation model that parity tests would have to
  chase.

## Consequences

- The layering note in CLAUDE.md gains `ferrum-view`: it depends on
  `ferrum-app`, `ferrum-agent`, `ferrum-io` and `ferrum-render`, and
  nothing depends on it except `ferrum-cli`. `docs/architecture.md` is
  updated when this is implemented.
- `ferrum-agent/1` gains view commands; each gets a JSON Schema in
  `schema.rs`, an entry in `skills/ferrum/reference/commands.md`, a
  regenerated `skills/ferrum/schemas/commands.json`, and contract tests;
  CLI and MCP must give identical JSON. `docs/agent-cli.md` gains a section.
- `ferrum-view/1` (state and events) gets a spec page and conformance
  fixtures so other harnesses can implement a client.
- `ferrum-domain` (and the used part of `ferrum-app`) must stay free of
  anything that blocks `wasm32` builds; CI gets a wasm build job.
- `ferrum-render` gains the WebGPU parity test and the `FrameSource`
  trait; the WGSL and the CPU model are unchanged.
- A session keeps a volume in memory; the host needs limits (sessions,
  memory, idle timeout) and, for streaming with a GPU, the same GPU-group
  lock as engines.
- The MCP server stops being strictly stateless between calls. Sessions
  are explicit, have ids and expire; closing the harness conversation
  closes them.
- Medical-device status is unchanged: all results remain proposals for
  qualified review.

## Open questions

- Which harness UI mechanism carries the component (an MCP UI resource in
  a sandboxed frame, a harness-specific widget API, or a plain URL)? The
  host should serve a self-contained page, so that several mechanisms work.
- Frame codec for streaming (WebP vs AV1 vs H.264 over WebRTC) and the
  latency target. Initial target: scroll and window changes under 50 ms on
  loopback.
- Volume transfer for the local backend: compression, tiling or a
  downsampled copy for interaction with a full-resolution copy on demand.
- Per-session authentication in harnesses where the browser and the host
  are on different machines.
- Multi-user sessions (two people in one component). Out of scope for the
  first version.
- Whether `ferrum-view` should also serve the desktop app's review queue,
  to keep one implementation of decisions.

## Staging (proposal)

1. `ferrum-view/1` spec, schemas, the state/event commands, and a
   streaming host with a minimal web client; loopback only; decision
   channel; privacy tests.
2. Workspace live-reload in the desktop app, so the person can move
   between the generated component and the full client.
3. The WASM + WebGPU client behind the same interface, capability
   negotiation, parity test, wasm CI job.
4. Remaining components (`VolumeView`, `Compare`, `SegmentReview`) and
   remote exposure with TLS guidance.
