# ADR 0013 — Volumes of a multi-volume series: one entry each, workspace bound to the series

- **Status:** Accepted (G2, 2026-10-14)
- **Date:** 2026-10-14

## Context

MR and PET series often hold several volumes: echoes, diffusion b-values,
dynamic phases, PET frames. Since 2.7 FERRUM shows the first and warns.
A person or an agent has to choose which volume to read (Stage 20.2–20.3).
The user decided (G1, 2026-10-14): one entry per volume, volumes named by
b-value, echo time or phase, and switching in the desktop app without
reloading the study.

## Decision

1. **A volume is a series descriptor.** `SeriesDescriptor` gets an optional
   `VolumeInfo` (group, index, count, label, b-value, echo time and number,
   temporal position, frame time, completeness).
   `VolumeRepository::volumes(series)` expands a series into its volumes;
   `scan` keeps returning whole series, so ids, workspaces and callers
   that do not ask for volumes are unchanged. `study scan` lists one
   entry per volume, each with the pseudonymised `group` of its series.
   Single-volume series are unchanged.
2. **Volumes are told apart by the fields that differ at one position**
   (temporal position, echo number or time, b-value, trigger time, frame
   reference time, acquisition number), else by rank at the position.
   NIfTI volumes are named from BIDS sidecars when present.
3. **A workspace belongs to the series, not to one volume.** The manifest
   keeps the group as `series_id` and the current volume as
   `source.volume`. Opening another volume of the same series in the same
   workspace switches it; segments and annotations stay, because the
   volumes of a complete series share the grid. The manifest is written as
   version 2 only when `source.volume` is set; version 1 opens as volume 1.
4. **Values name their volume.** Provenance of value commands and metrics
   results carries the volume.

## Consequences

- Existing calls keep working: a source with one multi-volume series still
  opens without `series` (volume 1, with a warning).
- Segments drawn on one volume can be measured on another (for example an
  ADC map against a b = 1000 segment), with the volume named in the
  result.
- An older FERRUM refuses a version 2 workspace instead of silently opening
  the wrong volume.
- A volume missing slices cannot share a workspace with the complete ones;
  it is listed and opened on its own.

## Alternatives considered

- **Volumes inside one scan entry** (`volumes: [...]` and `study open
  --volume`): fewer entries, but the user chose one entry per volume, and
  every tool already takes a series id.
- **A workspace per volume:** no format change, but segments could not
  follow a switch, which the user asked for in the desktop app.
