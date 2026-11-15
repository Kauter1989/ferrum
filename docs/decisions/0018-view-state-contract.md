# ADR 0018 — The view-state contract: a workspace artifact the agent writes and the clients apply

- **Status:** Proposed (G2 of Stage 28)
- **Date:** 2026-11-15

## Context

[ADR 0014](0014-generative-ui-view-host.md) decided that a harness mounts a
FERRUM component and the agent learns what the person did without sitting in
the render loop. [ADR 0017](0017-view-host-crate.md) built the streaming host
with a 2D `ferrum-view/1` state document; Stage 24 PR 2 (#109) gave the
embeddable widget its events as Rust types. [#70](https://github.com/Kauter1989/ferrum/issues/70)
asks for the rest: a versioned **ViewState** an agent can set (component,
plane, slice, window, overlays, camera, selected segment, tool, source hash),
the events in JSON (`point_picked`, `roi_drawn`, `slice_changed`,
`window_changed`, `camera_changed`), agent commands with CLI = MCP, a stale
source hash refused with `source_changed`, and no command that creates a
decision.

The constraint that shapes the design: `ferrum-agent` depends on neither
`ferrum-app` nor `ferrum-view` (CLAUDE.md layering). An agent command therefore
cannot call a `Viewer` use case or the host directly. The issue also says
**in process first** — the embeddable widget — and streaming to other hosts can
come later.

## Decision

1. **The view state is a workspace artifact, not a live object the agent
   holds.** `ferrum-io` persists `view.json` (the state) and `view-events.json`
   (the event log) beside `segments.json`, atomically, and notifies change
   through the existing mechanism (`changes::Touched::ViewState` /
   `ViewEvents`, `WorkspaceChange::ViewStateChanged`, `WorkspaceWatcher`,
   `subscribe`). The agent commands read and write the artifact; the live
   clients (the `ferrum-ui` widget and the `ferrum-view` host) watch it and
   apply it. This keeps the layering: the agent links only `ferrum-io` and
   `ferrum-domain`, as `decisions` already does.

2. **The `ViewState`/`ViewEvent` value types live in `ferrum-domain`; the apply
   use case lives in `ferrum-app`.** `Viewer::apply_view_state` is the only
   place the state becomes use cases (`set_view_mode`, `set_slice_index`,
   `set_window`, `set_segments_shown`, `set_segment_style`, `select_segment`,
   `tool`, `volume.camera`); `Viewer::view_state` reads it back. The host's
   existing 2D `state::ViewState` becomes a projection of the domain type; the
   host keeps its current routes until Stage 29.

3. **The contract is `ferrum-view/1`, extended additively.** The state document
   gains `component`, `overlays`, `selected_segment`, `tool`, `camera`,
   `source_hash`, `revision`; a 2D-only client keeps working. The events gain
   `roi_drawn` and `camera_changed` and take the JSON form of the widget's
   Rust events. The wire names are the issue's (`slice`/`mpr`/`volume`,
   snake_case tools); a spec page (`docs/view-state.md`) and conformance
   fixtures pin them, shared by the agent's schema and the host's parser.

4. **In process first; the host honours the 2D subset now.** The widget applies
   the full state. The HTTP host applies the artifact for its existing 2D
   controls and reports fields it cannot yet honour in an `unhonoured` list
   (not silently dropped). Streaming the full state to the host and the browser
   is Stage 29.

5. **The state binds to the study's `source_sha256`.** `view set` takes the
   `source_hash` the host read with the state and refuses a stale one with
   `source_changed`, applying nothing — the rule `decisions::record` already
   uses (16.8). `view state` returns the current hash.

6. **The artifact is a suggestion applied once per revision; the person takes
   over.** A client applies the artifact when `ViewStateChanged` arrives, then
   the person's input wins and is recorded as events; the next `view set` is a
   new revision. Nothing locks the view and no mode is entered. No command
   creates a decision — those stay with `ferrum_agent::decisions`.

7. **Privacy is structural.** The state and events hold numbers, labels, the
   source hash and the tool/component names only — no path, UID, name or header
   value. The privacy scan covers the artifact, the event log and the command
   envelopes (extends 25.5-a).

## Consequences

- An agent and any number of live clients agree on the view through one
  artifact, in one process or across processes, with the change notification
  the workspace already has; no new transport is introduced.
- The agent command set grows by three (`view set/state/events`), a contract
  surface: schemas, `commands.json`/`commands.md`, contract tests and CLI = MCP
  follow the change matrix.
- A selected-segment accessor is added to `SegmentationState`/`Viewer` (none
  exists today); the apply use case reports which fields changed so a set of
  the current state records no event.
- The host's 2D `ViewState` and the domain type coexist until Stage 29
  consolidates them; the duplication is bounded and named.
- The event log is persisted, so `view events` reads the same sequence the host
  serves; a client that produces events writes the log (the host keeps its
  in-memory ring for `/api/events`).
- Stage 29 streams the full state to other hosts and hardens the protocol
  (rotating token, an ADR first); this ADR does not decide that.
