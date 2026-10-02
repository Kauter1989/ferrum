# Commands

Each command is an MCP tool named `ferrum_<command>` (spaces become
underscores) and a `ferrum-cli` sub-command. The parameters are listed in
`../schemas/commands.json`. On the command line, points are written:
- `v:i,j,k` for a voxel;
- `mm:x,y,z` for patient millimetres;
- `r-0001:x,y` for a pixel of an earlier render.

## Study
| Command | Key parameters | Returns |
|---|---|---|
| `study scan` | `paths` | series: pseudonymised id, format, modality, dims, description |
| `study open` | `workspace`, `path`, `series?` | dims, spacing, LPS origin and direction, value unit, slice counts, window presets |
| `study info` | `workspace` | the same, plus annotation and segment counts with pending reviews |

## Looking
| Command | Key parameters | Returns |
|---|---|---|
| `view slice` | `plane`, `slice_number` or `at`, `window?`, `size?`, `overlays?` | PNG + sidecar with `pixel_to_voxel`, `pixel_to_patient_mm` |
| `view montage` | `plane`, `from?`, `to?`, `step?`, `columns?`, `window?` | grid of labelled slices, one mapping per tile |
| `view mpr` | `at`, `window?` | axial, coronal and sagittal through a point, crosshair |
| `view volume` | `mode` (`mip`, `isosurface` + `threshold`, `transfer_function` + `preset`), `view` | 3D render; no pixel mapping |

## Measuring
| Command | Key parameters | Returns |
|---|---|---|
| `probe` | `point` | value of the nearest voxel, unit, point in all forms |
| `stats` | one of `box {min, max}`, `sphere {center, radius_mm}`, `segment`, `annotation` | voxels, volume ml, mean, std, min, max, percentiles |
| `profile` | `from`, `to`, `samples?` | values along the line with distances |
| `measure distance` | `points` (2) | mm, uncertainty ± one voxel spacing |
| `measure angle` | `points` (3, vertex in the middle) | degrees |
| `measure area` | `points` (≥ 3, planar) | mm² |

## Recording
| Command | Key parameters | Returns |
|---|---|---|
| `annotate add` | `kind` (`distance`, `angle`, `area`, `rectangle`, `text`), `plane`, `points` on one slice, `name?`, `text?`, `agent?` | the annotation, proposed |
| `annotate list`, `annotate rename`, `annotate delete` | `id`, `name` | annotations with provenance; only your own can change |
| `segment threshold` | `seed`, `min`, `max`, `max_ml?`, `name?`, `agent?` | a proposed segment with voxels and ml |
| `segment list`, `segment rename`, `segment delete` | `label`, `name` | segments with ml and provenance; only your own can change |

## Review and hand-off
| Command | Key parameters | Returns |
|---|---|---|
| `review list` | — | proposed annotations and segments |
| `review confirm`, `review reject` | `annotation` or `segment`, `by` | only if the operator allows harness review |
| `export bundle` | — | `export/`: report.json, annotations.json, segments.nii.gz + .json, SHA-256 each |

## Examples
```bash
ferrum-cli study open -w ct1 /data/incoming/lung_053.nii.gz
ferrum-cli view montage -w ct1 --plane axial --window lung
ferrum-cli view slice -w ct1 --plane axial --slice-number 120 --window lung
ferrum-cli probe -w ct1 r-0002:412,318
ferrum-cli measure distance -w ct1 r-0002:400,310 r-0002:431,322
ferrum-cli segment threshold -w ct1 --seed r-0002:412,318 --min -100 --max 200 --max-ml 50 --name "Nodule"
ferrum-cli export bundle -w ct1
```
