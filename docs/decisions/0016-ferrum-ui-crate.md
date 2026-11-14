# ADR 0016 — The viewer as a library crate (`ferrum-ui`)

- **Status:** Accepted (Stage 24, G2 2026-10-28)
- **Date:** 2026-10-25

## Context

Medical Harness shows a conversation and FERRUM's viewer in one egui/wgpu
window ([#69](https://github.com/Kauter1989/ferrum/issues/69)). The views
and panels live in the `ferrum` binary, next to an app shell with its
own window and file dialogs. [ADR 0014](0014-generative-ui-view-host.md)
(draft) plans a view host with state and events; its first, in-process
step needs the viewer as a component a host can place.

## Decision

1. A new presentation crate `ferrum-ui` holds the views (2D, MPR, 3D),
   the panels, the theme and the GPU bridge, and exports `ViewerWidget`,
   `SidePanel`, `install` and `ViewerEvent`
   ([design](../embeddable-viewer.md)).
2. It depends on `ferrum-app`, `ferrum-domain`, `ferrum-render` (`gpu`),
   `ferrum-processing`, `ferrum-engines`, `ferrum-radiomics`, egui,
   egui-wgpu and wgpu. It has no window, menu or file dialog: not
   `eframe`, not `rfd`. File actions are returned to the host as
   requests.
3. `ferrum` (the desktop app) becomes a shell over `ferrum-ui`. Nothing
   else in the workspace depends on `ferrum-ui`.
4. Events are derived from `Viewer` state after each frame, so every rule
   stays in `ferrum-app`. No event is a decision.
5. egui, egui-wgpu and wgpu are re-exported; a host uses FERRUM's
   versions.

## Alternatives considered

- **The host embeds `ViewerApp`.** It brings the shell: header, file
  dialogs, start screen. Rejected.
- **Streaming frames to the host** (ADR 0014 decision 4). Needed for
  browsers and phones, not for a native egui host on the same machine,
  where it only adds latency. Left to a later stage.
- **Events raised inside the tools.** Every tool would need event
  plumbing, and `ferrum-app` would carry presentation concerns. Rejected
  in favour of the state diff.

## Consequences

- The CLAUDE.md layering line and `docs/architecture.md` gain
  `ferrum-ui`.
- The rendering rule is unchanged: shaders and the CPU renderer stay in
  `ferrum-render`.
- `ferrum-view/1` ([#70](https://github.com/Kauter1989/ferrum/issues/70))
  gives the events a JSON form and adds agent commands that set the
  widget's state.
