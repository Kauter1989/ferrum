# ADR 0007 — Extensibility and the FERRUM Engine Protocol

- **Status:** Accepted
- **Date:** 2026-10-01

## Context

FERRUM is meant to be the visualisation core of a wider ecosystem of
medical-imaging tools, used in both commercial and research settings. It
should not try to be a universal solution. It must offer clear extension
points so that other projects can plug in data sources, exporters and
analysis engines.

The first such extension is AI segmentation. The candidate engines
(nnInteractive, MONAI Label, TotalSegmentator) are Python/PyTorch
programs with different APIs and licences:

- nnInteractive has a remote mode, but its wire protocol is an internal,
  undocumented Python client–server pair.
- MONAI Label has a documented REST API built around its own data store
  and apps.
- TotalSegmentator is a command-line tool and a Python library.

Inference inside the Rust process is out of scope for now. Interactive
AI segmentation starts as a demo.

## Decision

1. **Extension through ports, wired at compile time.** Extension points
   are traits in `ferrum-domain`. The application composes the
   implementations at start-up; there is no dynamic loading of native
   plugins, because Rust has no stable ABI. The ports are:
   - `VolumeRepository` (exists): data sources.
   - `SegmentationEngine` and `InteractiveSession` (new): segmentation
     engines.
   - `Exporter` (later): annotation, label-map and report exporters.

2. **Out-of-process engines speak an open protocol.** Engines run as
   separate processes and talk to FERRUM over the **FERRUM Engine
   Protocol** (`ferrum-engine/1`), a small HTTP API specified in
   [docs/engine-protocol.md](../engine-protocol.md). FERRUM ships exactly
   one network client, `HttpEngine`, plus an in-process `MockEngine` for
   tests.

   Each third-party engine gets a thin **bridge** in `bridges/<engine>/`
   that translates the protocol to the engine's own API. The first bridge
   is nnInteractive, for the demo; MONAI Label and TotalSegmentator
   follow. Anyone can implement the protocol in any language without
   changing FERRUM.

3. **Data stays minimal and standard.** The protocol carries voxel
   arrays and geometry only, never DICOM headers or patient identifiers.
   Results come back on the same voxel grid. Files exchanged with other
   tools use standard formats: NIfTI for volumes and label maps, JSON for
   annotations, and DICOM SEG later.

4. **Licences are visible.** An engine reports whether it is for research
   use only (for example, nnInteractive weights are CC BY-NC-SA 4.0).
   FERRUM shows this next to the tools. The FERRUM code stays MIT and
   does not include any engine or weights.

5. **The UI degrades gracefully.** The *AI segmentation* section of the
   settings panel is always visible and collapsible. Without a connected
   engine its tools are disabled, with a hint on how to connect one.

## Consequences

- The domain gains volume geometry (origin and direction), label maps,
  segments and prompts. Rendering gains a segment overlay in 2D and 3D,
  mirrored in the CPU reference.
- A new crate, `ferrum-engines`, holds `HttpEngine` and `MockEngine`.
  Bridges live outside the Rust workspace, in `bridges/`.
- Engines can be swapped without code changes: a commercial deployment
  plugs in a commercially licensed engine through the same protocol.
- A second hop (FERRUM → bridge → engine) adds a little latency. On a
  local machine or LAN it is small compared with model inference, and
  volumes are uploaded once per session.
- Protocol changes are versioned (`/v1`). Additions that old clients
  can ignore stay within v1.
