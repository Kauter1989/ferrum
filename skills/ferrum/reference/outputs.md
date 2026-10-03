# Outputs

## Envelope
```json
{ "api": "ferrum-agent/1", "ok": true, "data": { }, "warnings": [ ],
  "provenance": { "ferrum": "FERRUM 0.3.0", "command": "probe", "params": { }, "source_sha256": "…", "renderer": "cpu", "time": "…" } }
{ "api": "ferrum-agent/1", "ok": false, "error": { "code": "out_of_volume", "message": "…", "hint": "…" } }
```

| Error code | Meaning |
|---|---|
| `bad_request` | invalid parameters; the message names the parameter |
| `no_study` | no study open in the workspace |
| `not_found` | unknown series, annotation, segment or render |
| `out_of_volume` | point, slice or pixel outside the data |
| `source_changed` | the source files changed since the workspace was made |
| `forbidden` | not allowed by the operator, or not the agent's own item |
| `limit` | size or count limit |
| `internal` | anything else |

Over MCP the same envelope is the text and the `structuredContent` of
the tool result. Renders add the PNG as image content, and errors set
`isError`.

## Render sidecar (`renders/r-NNNN.json`)
- **slice:**
  - `plane`, `slice_number`, `size`, `pixel_mm`, `window`;
  - `pixel_to_voxel` and `pixel_to_patient_mm` (affine maps of
    `(x, y, 1)`);
  - `orientation` letters at the image edges.
- **montage / mpr:** `tiles[]` with the same fields per tile plus
  `origin`.
- **volume:** `view`, `rendering`, no mapping.

## Workspace
`workspace.json` (source paths and SHA-256), `annotations.json`
(`ferrum-annotations` v2), `segments.nii.gz` + `segments.json`,
`renders/`, `audit.jsonl`. The format is in FERRUM's
`docs/workspace-format.md`.

## Export bundle (`export/`)
`report.json` (`ferrum-report` v1) holds:
- study description, measurements and segments with volumes;
- `confirmed` per item and `unconfirmed_items`;
- a disclaimer.

Next to it, by `formats`:
- `ferrum` (default): `annotations.json`, `segments.nii.gz`,
  `segments.json`;
- `dicom`: `segmentation.dcm` (DICOM SEG) and `measurements.dcm` (SR,
  TID 1500) for a PACS. Rejected items are left out, proposed ones are
  marked, and angles and text notes are not exported (a warning says
  what was left out).

The envelope lists every file with its SHA-256.
