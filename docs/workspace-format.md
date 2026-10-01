# Workspace and result formats

FERRUM keeps the results of working on a series in plain files: JSON and
NIfTI, readable by any tool. This page specifies them:

- `ferrum-annotations` v2: measurements and notes;
- `ferrum-segments` v1: metadata of a label map;
- `ferrum-workspace` v1: a directory that ties results to their source
  data;
- provenance: who created an item and whether a person confirmed it.

They are implemented in `ferrum-io` (`annotations.rs`, `segments.rs`,
`workspace.rs`). The agent skill ([agent-skill.md](agent-skill.md)) and,
later, the desktop review queue build on them.

## 1. Provenance

Every annotation and segment carries a provenance object. Results of
agents and segmentation engines are **proposals** until a person reviews
them, so a proposal is never mistaken for a finding.

```json
{
  "author": { "kind": "engine", "name": "TotalSegmentator", "version": "2.18.0 (total)", "research_only": false },
  "status": "proposed",
  "created": "2026-10-01T12:00:00Z",
  "reviewed_by": null,
  "reviewed": null
}
```

| Field | Values |
|---|---|
| `author.kind` | `human` (drawn in the viewer), `agent` (with `id` from the harness, or `null`), `engine` (with `name`, `version`, `research_only`) |
| `status` | `proposed`, `confirmed` or `rejected` |
| `created` | RFC 3339 UTC time, or `null` if unknown |
| `reviewed_by` | who confirmed or rejected the item, or `null` (the viewer does not ask for a name) |
| `reviewed` | when, or `null` |

Rules:
- Items drawn in the viewer are `human`, `confirmed`.
- AI results start as `engine`, `proposed`:
  - **Accept** in the *AI segmentation* panel confirms the current object;
  - **Confirm**, **Reject** and **Reopen** in the *Segments* and
    *Annotations* lists review any item.
  - The author stays the engine after review.
- A missing or `null` provenance means `human`, `confirmed`, which is
  what files written before provenance existed describe.

## 2. `ferrum-annotations` v2

Written by *Export annotations* and into workspaces. Version 2 adds
`provenance` to each annotation; readers accept versions 1 and 2.

```json
{
  "format": "ferrum-annotations",
  "version": 2,
  "generator": "FERRUM 0.2.0",
  "source": { "name": "lung_053", "path": "/data/lung_053" },
  "study": { "study_instance_uid": "…", "series_instance_uid": "…", "modality": "CT", "…": "…" },
  "volume": { "dims": [512, 512, 252], "spacing_mm": [0.78, 0.78, 1.25], "frame": "LPS voxel grid: …" },
  "coordinates": { "…": "explanations of the coordinate fields" },
  "annotations": [
    {
      "id": 0, "name": "Nodule A", "type": "Distance",
      "plane": "axial", "slice_index": 119, "slice_number": 120,
      "value": 14.2, "unit": "mm", "text": null,
      "points_mm": [[120.4, 98.0], [131.1, 107.3]],
      "points_voxel": [[153.9, 125.1, 119.0], [167.6, 137.1, 119.0]],
      "provenance": { "author": { "kind": "human" }, "status": "confirmed", "created": "2026-10-01T12:00:00Z", "reviewed_by": null, "reviewed": null }
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `type` | `Distance` (2 points), `Angle` (3: arm, vertex, arm), `Area` (polygon, ≥ 3), `Rectangle` (2 corners), `Text` (1 point + `text`) |
| `plane`, `slice_index` | slice the annotation is drawn on; `slice_index` is 0-based, `slice_number` 1-based as shown in the viewer |
| `points_mm` | in-plane millimetres from the top-left corner of the displayed slice, x right, y down. **Readers use these.** |
| `points_voxel` | the same points as continuous voxel coordinates `(i, j, k)`; informative |
| `value`, `unit` | measured value in `mm`, `deg` or `mm2`; informative (recomputed from the points) |

When reading:
- ids, names, slices, geometry and provenance are restored;
- a reader given the volume size rejects a document for another size;
- later annotations get ids above the highest id read.

## 3. `ferrum-segments` v1

A sidecar of a NIfTI label map (`uint8`, one value per segment). It keeps
what NIfTI cannot hold.

```json
{
  "format": "ferrum-segments",
  "version": 1,
  "generator": "FERRUM 0.2.0",
  "segments": [
    { "label": 1, "name": "liver", "color": [230, 85, 75], "visible": true, "opacity": 0.5,
      "voxels": 18234, "volume_ml": 412.7, "provenance": { "…": "…" } }
  ]
}
```

| Field | Meaning |
|---|---|
| `label` | value in the label map, `1..=255`, unique |
| `name`, `color`, `visible`, `opacity` | display settings; defaults: `Segment {label}`, palette colour, `true`, `0.5` |
| `voxels`, `volume_ml` | informative; readers ignore them |
| `provenance` | see §1 |

Labels present in the label map but missing from the sidecar get default
segments.

## 4. `ferrum-workspace` v1

A directory of results for one series. It lies next to the source data,
never inside it, and is created by the agent interface (`study open`) or,
later, by the viewer.

```text
ws/ct1/
├── workspace.json    # manifest (below)
├── annotations.json  # ferrum-annotations v2 (absent when there are none)
├── segments.nii.gz   # label map on the volume grid, with the patient geometry
├── segments.json     # ferrum-segments v1
├── renders/          # images written by the agent interface
└── audit.jsonl       # one JSON object per line
```

```json
{
  "format": "ferrum-workspace",
  "version": 1,
  "generator": "FERRUM 0.2.0",
  "created": "2026-10-01T12:00:00Z",
  "source": {
    "path": "/data/incoming/series-17",
    "series_id": "1.2.840.…",
    "format": "DICOM",
    "files": [ { "path": "IM0001.dcm", "size": 526336, "sha256": "9f2c…" } ]
  }
}
```

**Sources:**
- Source files are referenced and hashed (SHA-256), never copied or
  modified.
- File paths are relative to `source.path` when they lie below it, and
  absolute otherwise.
- Before working on a workspace, its sources are verified. A missing file,
  or one whose size or hash differs, fails with `source_changed`, so
  results are never shown on data they were not made for.

**Writing:**
- Files are replaced atomically: written as `.partial-<name>` next to the
  target, then renamed.
- Saving an empty annotation set or segmentation removes its files.

**Audit log:** `audit.jsonl` is append-only.
- Each line is a JSON object; a `time` field (RFC 3339) is added first
  when missing.
- The agent interface writes one line per command (command, parameters,
  outputs, hashes). Review decisions are logged there too.

**Privacy:**
- The manifest holds paths and hashes only.
- `annotations.json` copies the study identification (UIDs, date,
  description) from the source header, as the annotation export does.
- Keep workspaces where the source data may live.
