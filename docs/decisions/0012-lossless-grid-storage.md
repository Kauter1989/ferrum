# ADR 0012 — Voxel values on a grid are stored exactly

- **Status:** Accepted (2026-10-14; the user chose option 2 after the Stage 18 field test)
- **Date:** 2026-10-14

## Context

A volume stores each voxel as a `u16` over a physical range
(`IntensityRange`, `0 ↦ min`, `65535 ↦ max`). The readers set the range to
the minimum and maximum of the data, so the step is `(max − min) / 65535`.
For a CT of −1024 to 3071 HU that is 0.0625 HU, but not *exactly* 1/16:
a whole Hounsfield unit does not land on a step. `probe` of a voxel the
file holds as 55 HU returned 55.01; a minimum of −25 became −24.9866; a
threshold at `--min 100` could miss the voxels of exactly 100 HU.

The Stage 18 field test (MSD spleen, FERRUM `clinical` against
pyradiomics on the same masks) showed it: mean, minimum, maximum and
energy differed by 0.01 to 0.07 %, and pyradiomics run on an image
quantised the same way agreed with FERRUM to 0.00 %. The error is at most
half a step (about 0.03 HU), small against CT noise, but FERRUM's rule is
that values come from voxels, and a measurement tool should return the
number the file holds.

## Decision

When every value of the data is a multiple of the storage step above the
minimum, the volume is stored **exactly**:

- the step is the smallest power of two not below `span / 65535`
  (`IntensityRange::grid_step`), so whole units (step ≤ 1), halves,
  quarters and so on are whole multiples of it;
- the range is `min .. min + 65535 · step` (`IntensityRange::on_grid`),
  which reaches up to twice as far above the data as the data are wide;
- values are converted in double precision, so a stored `n` reads back as
  `min + n · step` to the last bit.

Data that are not on the grid (float images, a span of more than 65 535
whole units) keep the data range and the old precision.
`IntensityRange::for_data` makes the choice; the NIfTI and DICOM readers
and `Volume::from_physical` call it.

## Consequences

- CT in HU (the field case) and PET-like data with a slope of one half
  read back exactly: `probe`, `stats`, `segment threshold` and
  `segment metrics` give the numbers of the file.
- The range of a volume (`study info` `value_range`, the window and
  transfer-function limits) can be wider than the data: for −1024 to 3071
  it is −1024 to 3071.9375, for −1000 to 1500 it is −1000 to 3095.9375.
  Windows and presets are in physical units and are unchanged; the
  normalised value of a voxel is smaller (the data fill 60 to 100 % of
  `[0, 1]`), which costs nothing visible at 16 bits.
- A volume whose data are not on the grid is unchanged, so nothing that
  works today changes for MR in arbitrary units.
- Memory is unchanged: still one `u16` per voxel.
- Rejected: a second, exact copy for measurement (option 3: it doubles
  the memory of a volume); leaving the quantisation and documenting it
  (option 1: a threshold at a whole unit stays unreliable).
