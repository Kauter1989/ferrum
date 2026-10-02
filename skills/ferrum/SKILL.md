---
name: ferrum-imaging
description: View and measure CT, MR and other volumetric medical images (DICOM or NIfTI) with FERRUM — render slices, montages, MPR and 3D, read values (Hounsfield units for CT), measure distances, angles, areas and volumes, segment structures by threshold, and export results for clinical review. Use when a task needs to look at or quantify a medical image. Not for diagnosis.
---

# FERRUM imaging skill

Tools: the `ferrum_*` MCP tools (`ferrum-cli mcp`) or the `ferrum-cli`
command line. Both return the same JSON envelope:
- check `ok`;
- read `warnings`;
- on errors, follow `error.hint`.

Parameters are checked against JSON Schemas (`ferrum-cli schema`).

## Workflow
1. **Find the series:** `study scan` with the user's paths. Pick the
   series that fits the task (modality, size, description). Ask if
   several fit.
2. **Open it:** `study open` with a workspace name and the path (plus
   `series` if needed). Read `study info`: modality, `value_unit`,
   spacing, slice counts, warnings.
3. **Look:**
   - `view montage` to find where things are;
   - `view slice`, `view mpr` for detail;
   - `view volume` for 3D context.

   Choose a window for the task (`lung`, `soft_tissue`, `bone`, `brain`,
   or `{center, width}`).
4. **Measure with tools, never by eye:**
   - point at what you saw with `{"render": "r-0003", "pixel": [x, y]}`;
   - use `probe`, `stats`, `profile`, `measure distance|angle|area` and
     `segment threshold` (probe the seed first, set `max_ml`).
5. **Record:** name what you create (`annotate add` with `name`,
   `segment threshold` with `name`).
6. **Hand over:** `export bundle`, then `review list`. End your answer
   with the items that need a person's review and the workspace path. A
   clinician reviews them in the FERRUM desktop app (*Open workspace* →
   *Review*).

## Rules
- FERRUM is not a medical device. Never state a diagnosis or a
  likelihood of disease. Describe location and measured values:
  - plane, slice number, patient mm;
  - values, sizes, volumes.
- Report every number with its unit and method, e.g. "16.0 mm, distance
  between two edge points, ± 2 mm (one voxel spacing)".
- Values are in HU only when `value_unit` is `HU`; NIfTI files have no
  unit.
- Everything you create is *proposed* until a clinician confirms it. Say
  so.
- Results from engines marked `research_only` are for research use only.
- If the data looks wrong, stop and say so. Examples: wrong modality for
  the task, a region that is not covered, strongly anisotropic voxels.
- Patient identifiers are withheld by design. Never try to obtain them.
- You may change or delete only what you created. Confirm or reject
  items only if the operator allows it, and always in the name of a
  person.

## References (read when needed)
- `reference/commands.md`: every command with key parameters and
  examples.
- `reference/coordinates.md`: voxels, slice numbers, patient mm, render
  pixels.
- `reference/outputs.md`: envelopes, render sidecars, workspace and
  export files.
- `reference/safety.md`: privacy, provenance, review, operator settings.
- `schemas/commands.json`: JSON Schemas of all parameters.
