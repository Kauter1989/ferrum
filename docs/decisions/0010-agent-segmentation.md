# ADR 0010 — Agent segmentation scenarios with engines

- **Status:** Proposed
- **Date:** 2026-10-03

## Context

The agent skill (ADR 0008) has `engine info`, `segment interactive` and
`segment auto` over `ferrum-engine/1` (Stage 15.7). Three engines already
connect to FERRUM through bridges: nnInteractive (interactive), TotalSegmentator
(automatic) and MONAI Label (both). The target workstation has a single
12 GB GPU (RTX 3080 Ti), on which nnInteractive and the full
TotalSegmentator model do not fit at the same time.

Today every `segment interactive` call uploads the whole volume and
creates a new segment, so an agent cannot refine an object, and it may
not delete the engine segments of failed attempts. The command line
starts a new process per call, so it cannot hold an engine session, and
the command line and MCP must give identical JSON. Agents also need
deterministic checks of engine results, because a vision model cannot
be trusted to judge a mask, and numbers must come from voxels.

## Decision

1. **GPU groups in the operator configuration.** Allowed engine URLs that
   share a GPU are listed in `network.gpu_groups`; FERRUM runs one call
   of a group at a time, across processes.
2. **The prompt history is the session.** The prompts of an interactive
   object are stored in the workspace with the segment. The command line
   replays them on a fresh session; the MCP server keeps the engine
   session as a cache of the same history. Engines report whether replay
   is deterministic. An agent may refine, rename and delete engine
   segments it requested (`requested_by`), never a person's.
3. **Interactive sessions upload a region of interest** (prompts'
   bounding box plus a margin) by default, which fits a 12 GB GPU and
   needs no protocol change.
4. **Deterministic quality checks** (empty, size, components, border,
   laterality, overlap, stability, research flag) are part of the
   command results, with new mask-analysis commands (`segment shape`,
   `components`, `compare`, `edit`).
5. `ferrum-agent` may depend on `ferrum-processing`, which brings no GPU
   or UI dependencies.

The scenarios and command details are in
[docs/agent-segmentation.md](../agent-segmentation.md).

## Consequences

- The layering note in CLAUDE.md changes when this is implemented:
  `ferrum-agent` also depends on `ferrum-processing`.
- `ferrum-workspace` v1 and `ferrum-agent/1` gain fields and commands;
  nothing existing changes.
- The bridges change: TotalSegmentator jobs run in a child process, the
  nnInteractive bridge frees GPU memory when idle, and `info` gains
  `deterministic`. The conformance suite gains a replay test.
- The command line pays a volume upload per interactive call; the ROI
  keeps that small. Long-running agents should use MCP.
