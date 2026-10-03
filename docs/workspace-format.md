# Workspace and result formats

FERRUM keeps the results of working on a series in plain files: JSON and
NIfTI, readable by any tool. This page specifies them:

- `ferrum-annotations` v2: measurements and notes;
- `ferrum-segments` v1: metadata of a label map;
- `ferrum-workspace` v1: a directory that ties results to their source
  data;
- provenance: who created an item and whether a person confirmed it;
- DICOM export: Segmentation and a measurement report for PACS.

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
| `requested_by` | optional: who asked the author for the item, e.g. `{ "kind": "agent", "id": "run-1" }` for an engine segment the agent asked for; written only when set |

Rules:
- Items drawn in the viewer (including regions of the region tool) are
  `human`, `confirmed`.
- AI results start as `engine`, `proposed`:
  - **Accept** under *Segmentation → AI engine* confirms the current object;
  - **Confirm**, **Reject** and **Reopen** in the *Segments* and
    *Annotations* lists review any item.
  - The author stays the engine after review.
- A missing or `null` provenance means `human`, `confirmed`, which is
  what files written before provenance existed describe.
- Agents may change their own items and engine segments they requested
  (`requested_by.kind = agent`) until a person confirms them; a change
  sets the item back to `proposed`.

## 2. `ferrum-annotations` v2

Written by *Export annotations* and into workspaces. Version 2 adds
`provenance` to each annotation; readers accept versions 1 and 2.

```json
{
  "format": "ferrum-annotations",
  "version": 2,
  "generator": "FERRUM 0.2.3",
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
  "generator": "FERRUM 0.2.3",
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
├── engine_inputs.json  # ferrum-engine-inputs v1 (absent when there are none)
├── renders/          # images written by the agent interface
└── audit.jsonl       # one JSON object per line
```

```json
{
  "format": "ferrum-workspace",
  "version": 1,
  "generator": "FERRUM 0.2.3",
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

**Engine inputs:** `engine_inputs.json` (`ferrum-engine-inputs` v1)
keeps the prompts behind each interactive engine segment, so the agent
can refine the object later by replaying them
([agent-segmentation.md](agent-segmentation.md)).

```json
{ "format": "ferrum-engine-inputs", "version": 1, "generator": "FERRUM 0.2.3",
  "objects": [ { "label": 7, "created": "2026-10-03T10:00:00Z",
                 "engine": "http://127.0.0.1:8765", "engine_name": "nnInteractive", "engine_version": "2.6.0 (nnInteractive_v1.0)",
                 "roi": { "min": [180, 140, 60], "max": [330, 290, 120] }, "revision": 2, "seeds": 0,
                 "prompts": [ { "type": "point", "positive": true, "voxel": [251, 198, 87] },
                              { "type": "box", "positive": false, "min": [200, 150, 87], "max": [300, 260, 88] },
                              { "type": "lasso", "positive": true, "min": [210, 160, 87], "max": [290, 250, 88], "runs": [0, 12, 80, 9] } ] } ] }
```

- Coordinates are voxels of the full grid; boxes are half-open.
- Scribble and lasso masks are `runs`: (start, length) pairs of the inside
  voxels of their box, `i` fastest.
- `roi` is the region uploaded to the engine; `seeds` counts leading
  lasso prompts derived from an earlier mask.
- An entry belongs to the segment with the same `label` and provenance
  `created` time; entries of deleted or replaced segments are ignored and
  dropped on the next save.

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

## 5. DICOM export

`export bundle` with the `dicom` format writes two DICOM objects next to
the JSON report, so results can be stored in a PACS and read by DICOM
viewers. Implemented in `ferrum-io` (`dicom/export.rs`).

| File | Object |
|---|---|
| `segmentation.dcm` | Segmentation (SEG, `1.2.840.10008.5.1.4.1.1.66.4`), binary |
| `measurements.dcm` | Comprehensive 3D SR (`1.2.840.10008.5.1.4.1.1.88.34`), TID 1500 *Imaging Measurement Report* |

**Segmentation:**
- One segment per FERRUM segment with voxels, numbered from 1 in label
  order; the FERRUM label is in the segment description.
- One frame per segment and slice that holds it, on the volume grid:
  rows along `j`, columns along `i`, 1-bit pixels.
- Shared groups: pixel spacing and orientation; per-frame groups:
  position, segment number and, when source UIDs are kept, the source
  image of the slice (Derivation Image).
- Algorithm type from the author: `MANUAL` (human), `SEMIAUTOMATIC`
  (agent) or `AUTOMATIC` (engine, with its name and version).
- Category and type are generic (*Anatomical Structure*, *Tissue*): FERRUM
  does not know what a segment is.
- Display colour as CIELab.

**Measurement report** (TID 1500):
- Observer: device *FERRUM*; language en-US; procedure *Imaging
  procedure*.
- One Measurement Group (TID 1501) per distance, area or rectangle: a
  *Length* (mm) or *Area* (mm²) inferred from a 3D polyline or polygon in
  patient coordinates on the frame of reference of the segmentation.
  Angles and text notes are not exported.
- One Volumetric ROI group (TID 1411) per exported segment: *Referenced
  Segment* in the SEG, the source series, and *Volume* in ml.
- Every group has a tracking identifier (the item's name) and a stable
  tracking UID.
- `VerificationFlag` is `UNVERIFIED`; `CompletionFlag` is `PARTIAL` while
  any item is unconfirmed.

**Review status:**
- Rejected items are left out of both objects.
- Proposed items are exported and marked: in the segment description
  (`proposed by engine …; unconfirmed`) and, in the report, by a code in
  FERRUM's private scheme `99FERRUM`:

| Concept | Values |
|---|---|
| `(review-status, 99FERRUM, "Review status")` | `proposed`, `confirmed`, `rejected` |
| `(provenance, 99FERRUM, "Provenance")` | text: author and reviewer |

**Privacy:** what is copied from the source follows the operator's
configuration (`ferrum-agent.toml`):

| Setting | Off (default) | On |
|---|---|---|
| `expose_identifiers` | patient name and ID empty, `PatientIdentityRemoved` = `YES` | patient name, ID, sex, accession number, study ID copied |
| `expose_dates` | study date and time empty | copied (and the birth date, with identifiers) |
| `pseudonymise_uids` | study, series and frame-of-reference UIDs replaced by salted hashes (`2.25.…`); source images not referenced | source UIDs kept; frames reference their source images |

Sources that are not DICOM (NIfTI) get a new study and frame of
reference; patient attributes stay empty.

**Validation:** the objects are read back with
[highdicom](https://github.com/ImagingDataCommons/highdicom) in CI
(`scripts/validate_dicom_export.py`).
