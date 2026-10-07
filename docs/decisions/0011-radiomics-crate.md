# ADR 0011 — A radiomics crate for quantitative segment profiles

- **Status:** Proposed (accepted when gate G2 of Stage 18 passes)
- **Date:** 2026-10-07

## Context

Stage 18 adds quantitative description of a segment: size, shape and
HU-intensity features, grouped in versioned profiles, later texture
features and IBSI conformance (see [the stage](../../dev_plan.md) and
[the design](../radiomics.md)).

Today the building blocks are scattered:
- mask measurements (components, shape, agreement) are in
  `ferrum-domain/src/analysis.rs`, next to `MaskRegion`;
- first-order statistics (`summarise`) are in `ferrum-agent`, so the
  desktop app cannot use them and a second copy would drift;
- nothing computes surface area, PCA axes, hull, holes, histograms or
  texture matrices.

The planned work is numerically heavy (marching cubes, convex hull,
eigenvalues, texture matrices over 13 directions) and grows over six
phases. `ferrum-domain` must stay small and free of heavy algorithms.
`ferrum-agent` may depend only on `ferrum-domain`, `ferrum-io`,
`ferrum-engines` and `ferrum-render`, and `ferrum-processing` is not on
that list.

## Decision

1. **A new crate `ferrum-radiomics`** holds the feature registry, the
   profiles, the result type and every computation. It depends on
   `ferrum-domain` (for `MaskRegion`, `Volume`, spacing) and on plain
   numeric crates only: no I/O, no GPU, no UI, no `ferrum-io`.
2. **Dependency rule change** (an amendment of the layering in
   `CLAUDE.md`): `ferrum-agent` and `ferrum-app` may depend on
   `ferrum-radiomics`. The diagram becomes
   `ferrum-app → ferrum-domain ← ferrum-radiomics`, with the agent
   adapter using it too. Nothing else changes.
3. **One implementation per measure.** Where `ferrum-domain` already has
   a measure (components, extent, long and short axis, Dice, HD95),
   `ferrum-radiomics` calls it; it does not copy it. Where `ferrum-agent`
   has one (`summarise`), it moves to `ferrum-radiomics` and the agent
   calls it, with unchanged JSON.
4. **Profiles are versioned and recorded.** A result carries the profile
   name and version and every parameter, so numbers stay comparable.
   Profiles are `0.x` while a stage is in progress and `1` when released.
5. **Deterministic by construction:** fixed operation order, no data
   races in reductions, same input and parameters give byte-identical
   JSON.
6. **Feature definitions live once**, in the registry (id, family, unit,
   definition text, version), and are shown by the UI tooltips, the CLI
   and MCP, and the docs.

## Consequences

- `CLAUDE.md` (layering) and `docs/architecture.md` change in the first
  PR of Stage 18, together with the crate (so the docs never describe a
  crate that does not exist).
- The workspace gets one member and `ferrum-agent` and `ferrum-app` get
  one dependency each. `cargo deny check licenses` must stay green; any
  new numeric dependency needs its licence checked first.
- `ferrum-app` still compiles without wgpu and egui.
- Texture features and IBSI preprocessing (Stage 19)
  have a place to live without touching the domain.

## Alternatives considered

- **Extend `ferrum-domain/src/analysis.rs`.** No change to the
  constitution, but the domain would absorb thousands of lines of
  numerics and, later, texture matrices. Rejected by the user
  (2026-10-07).
- **Put it in `ferrum-processing`.** Infrastructure crate with rayon and
  filters, but `ferrum-agent` may not depend on it, and adding that
  dependency pulls in resampling and filter code the agent does not need.
- **Compute in an engine (pyradiomics over `ferrum-engine/1`).** The
  protocol is for segmentation, and a Python dependency would break the
  offline, deterministic CLI. pyradiomics is used only as a reference in
  the field test.
