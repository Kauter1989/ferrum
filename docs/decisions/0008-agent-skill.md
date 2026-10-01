# ADR 0008 — FERRUM as an agent skill

- **Status:** Accepted
- **Date:** 2026-10-01

## Context

FERRUM should be reusable not only by people at a desktop but also by AI
agents. A typical target is a medical agent *harness*: an orchestrating
model with a set of skills, for example study retrieval (PACS/DICOMweb),
imaging (FERRUM), reporting, guidelines and FHIR. The harness decides
which skill to use; the skill must do its job reliably, explain itself
and stay safe.

An agent skill is a folder with instructions (`SKILL.md`, loaded on
demand) and the tools those instructions call. The instructions are only
as good as the tools. For FERRUM the tools are the use cases that the
desktop app already has: open a study, show slices and 3D views, read
values, measure, annotate, segment and export.

The constraints are specific to medicine:

- Studies contain protected health information (PHI).
- Agents must not act as a medical device: they propose, a clinician
  decides.
- Results must be reproducible and traceable to their input.
- Vision models read images, but numbers read off an image are
  unreliable. Measurements must come from the voxels.

## Decision

1. **One command layer, two transports.** A new crate `ferrum-agent`
   exposes the `Viewer` use cases as typed, versioned commands
   (`ferrum-agent/1`) with JSON input and output and generated JSON
   Schemas. A new binary `ferrum-cli` serves them as a command-line tool
   (`ferrum-cli <command> --json`) and as an MCP server over stdio
   (`ferrum-cli mcp`). It has no windowing or egui dependency, so it runs
   in containers and on servers. It renders with the headless GPU path
   when available and with the CPU reference renderer otherwise, so
   images can be reproduced.

2. **A skill package in the repository.** `skills/ferrum/` holds
   `SKILL.md` and reference pages that are loaded on demand. Release
   archives ship the package with `ferrum-cli`, together with a plugin
   manifest for harnesses that install skills and MCP servers as one
   unit. The package documents workflows, coordinates, output formats and
   safety rules; it contains no code beyond calls to `ferrum-cli`.

3. **Workspaces connect agents and people.** Every agent session works
   in a workspace directory: references to the source data with hashes,
   annotations, segments, renders and an audit log. The desktop app opens
   the same workspace, so a clinician sees exactly what the agent did.

4. **Agents propose, people confirm.** Annotations and segments carry
   provenance: the author (human, agent or engine), status (`proposed`,
   `confirmed`, `rejected`) and time. Anything created through the skill
   is `proposed`. Only the desktop app's review queue, or a harness
   approval step that names a human, confirms it.

5. **Private and offline by default.**
   - Outputs carry no patient identifiers.
   - Source data is read-only, and writes stay inside the workspace.
   - There is no network access except to engine URLs on an allowlist.
   - These settings come from an operator configuration file that tool
     calls cannot change.

6. **Measurements come from voxels.** Every render returns the mapping
   from image pixels to voxels and patient millimetres. The skill tells
   the agent to locate structures on images but to obtain values only
   from `probe`, `measure` and `stats`. All numbers come with units and
   voxel-size uncertainty.

## Consequences

- Two new crates (`ferrum-agent`, `ferrum-cli`) and a skill package. The
  layering does not change: both crates sit on top of `ferrum-app`, like
  the desktop app.
- The domain gains provenance and review status for annotations and
  segments. The annotation export moves to `ferrum-annotations` v2, and a
  workspace format (`ferrum-workspace` v1) is added.
- The desktop app gains a workspace review queue.
- The command contract becomes a public API. Breaking changes need
  `ferrum-agent/2`, like the engine protocol.
- Skill behaviour is tested with evaluations on synthetic phantoms with
  known answers, which can run in CI without patient data.
- DICOM SEG and Structured Report exports become more valuable, because
  harnesses pass results to PACS and reporting skills. They are planned
  after the first version.
