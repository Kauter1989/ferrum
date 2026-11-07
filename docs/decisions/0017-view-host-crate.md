# ADR 0017 — `ferrum-view`: a local view host with a streaming backend

- **Status:** Accepted
- **Date:** 2026-11-03

## Context

[ADR 0014](0014-generative-ui-view-host.md) decided that a harness mounts a
FERRUM component and the person works in it directly. It left the first
implementation open: a host crate, two rendering backends (local WebGPU in
the browser and streaming from the host), a `ferrum-view/1` state document,
and the transport.

The first harness is the DeepSeek Harness (Electron, its GUI is a web page on
loopback). Its client plugins are browser bundles, so the harness cannot link
Rust: FERRUM's process must be separate. ADR 0014's staging puts the
streaming host first, the WASM client third.

## Decision

1. **A crate `ferrum-view`, not a binary inside the CLI.** It owns the
   session (a `Viewer`, its workspace and the frame source) and the HTTP
   server; it depends on `ferrum-app`, `ferrum-io` and `ferrum-render` with
   its `gpu` feature, and nothing depends on it except `ferrum-cli`.
   `ferrum-cli view serve` only parses arguments, prints the URL and token
   and blocks — the command is CLI-only, never an MCP tool, as `gpu hold`
   already is.

2. **The streaming backend is the host's GPU or the CPU reference.** The GPU
   path uses the same `VolumeRenderer`, `slice.wgsl` and `Viewer::slice_params`
   as the desktop; the CPU path uses the same slice oracle as `gpu_parity.rs`.
   No third image-formation model is introduced (ADR 0014 decision 5). The
   WASM/WebGPU backend stays a later stage behind the same frame interface.

3. **Frames at native resolution.** One pixel per voxel of the plane, nearest
   sampling, no letterbox: a pixel and a voxel then map one to one through the
   sidecar-style mapping, the two backends are comparable pixel by pixel, and
   a 512×512 frame is small enough for loopback. Zoom is the client's
   business in this stage.

4. **Pull, not push.** The client asks for the state and the frame; the host
   renders only when the state changed since the last frame. This keeps one
   code path (plain HTTP, no WebSocket), makes "render the latest state" the
   natural behaviour, and lets a harness use an `<img>` if it wants no
   JavaScript of its own.

5. **One self-contained page and one JSON protocol.** `GET /` is a single
   HTML document with inline CSS and JavaScript and no other origin; the same
   routes serve a harness that draws its own controls. A harness needs no
   build step and no knowledge of egui.

6. **Loopback, token, no third party.** The host binds `127.0.0.1`, and every
   request needs the token (query, bearer header or the page's `HttpOnly`
   cookie). State, events and the page carry no path, UID, name or header
   value; a test scans them.

7. **The agent command set does not change.** In this stage the agent learns
   what happened by asking for the events later (Stage 26 gives them a
   command); the view never records a decision — that stays with
   `ferrum_agent::decisions` (16.7).

## Alternatives considered

- **A separate binary crate `ferrum-view-server`.** More deployable, but it
  adds a second entry point with the same argument parsing, and the CLI is
  where every other operator tool lives (`gpu hold`, `engine serve`).
- **WebSocket frames pushed at a fixed rate.** Lower latency for a drag, but
  a second protocol, a new dependency and no benefit over loopback for
  frames that are already rendered on demand; the client can poll as fast as
  it likes.
- **JPEG frames.** Smaller, but lossy: the parity tests, a person measuring
  on screen and the mapping all benefit from exact pixels, and loopback makes
  the size irrelevant.
- **Rendering at a display size with letterbox and zoom.** Closer to the
  desktop's slice view, but it couples the host to each client's layout, and
  the mapping needs the image rectangle; native resolution plus a client
  scale is simpler and exact.
- **A WASM client now.** Best latency and no frames on the wire, but it puts
  the volume on the client (a policy question), needs a wasm CI job and
  leaves server-side rendering unsolved; ADR 0014 stages it third.

## Consequences

- New crate in the workspace; `docs/architecture.md`, the layering note in
  `CLAUDE.md` and the CHANGELOG gain it.
- `ferrum-app` gains `window()`, `set_window()` and `slice_index()` (a host
  and any embedder need to read and set the window) and `adopt_segments()`
  (a long-lived viewer follows a workspace another process changes).
- `ferrum-cli` builds wgpu now, because the host renders on the GPU; the
  agent and MCP paths are unchanged.
- `docs/view-host.md` specifies `ferrum-view/1`; other harnesses can
  implement a client against it.
- MPR, the 3D view, WASM and remote exposure stay open (Stage 26).
- A session holds a volume in memory and renders when asked; the operator
  starts and stops it with the harness, so no idle limits are needed yet.
