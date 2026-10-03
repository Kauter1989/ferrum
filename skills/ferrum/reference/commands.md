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
| `stats` | one of `box {min, max}`, `sphere {center, radius_mm}`, `segment` (optionally with `box`: the part inside it), `annotation` | voxels, volume ml, mean, std, min, max, percentiles |
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
| `segment list`, `segment rename`, `segment delete` | `label`, `name` | segments with ml and provenance; you may change your own and engine segments you asked for |

## Measuring segments
| Command | Key parameters | Returns |
|---|---|---|
| `segment shape` | `segment` | ml; extent in mm; slice ranges and largest slices; axial long and short axis with end points and slice number; border contact; components; laterality for names with left/right |
| `segment components` | `segment`, `min_ml?`, `split?` | components (largest first) with centroid, box, ml; `split` makes a segment of each further one |
| `segment compare` | `a`, `b`, `b_workspace?` | Dice, Jaccard, volumes and difference, HD95, Hausdorff, centroid distance |
| `segment edit` | `segment`, `op` (`keep_largest`, `fill_holes`, `restrict_to_box` + `box`, `remove_small` + `min_ml`) | ml before and after |

## Segmentation engines
| Command | Key parameters | Returns |
|---|---|---|
| `engine list` | — | every allowed engine: reachable, name, modes, `research_only`, `gpu_group` |
| `engine info` | `engine?` | capabilities (prompts, labels), `deterministic`, `research_only`, licence |
| `segment interactive` | `prompts` (`point`, `box`, `scribble`/`lasso` with `points` on one slice), `name?`, `segment?` + `append?`/`undo?`, `from_segment?`, `roi?`/`whole_volume?`, `min_ml?`/`max_ml?`, `modality?`, `agent?`, `engine?` | the segment (new or refined), `revision`, `roi`, `checks` |
| `segment auto` | `labels?`, `name_prefix?`, `modality?`, `agent?`, `engine?` | one segment per structure found, each with `checks` |

Engines are separate programs (e.g. nnInteractive, TotalSegmentator, MONAI
Label bridges). The operator decides which ones you may use. Say "research
use only" when the engine is marked so. Workflows: `segmentation.md`.

## Review and hand-off
| Command | Key parameters | Returns |
|---|---|---|
| `review list` | — | proposed annotations and segments |
| `review confirm`, `review reject` | `annotation` or `segment`, `by` | only if the operator allows harness review |
| `export bundle` | `formats`: `ferrum` (default), `dicom` | `export/`: report.json; annotations.json, segments.nii.gz + .json (ferrum); segmentation.dcm + measurements.dcm (dicom); SHA-256 each |

## Examples
```bash
ferrum-cli study open -w ct1 /data/incoming/lung_053.nii.gz
ferrum-cli view montage -w ct1 --plane axial --window lung
ferrum-cli view slice -w ct1 --plane axial --slice-number 120 --window lung
ferrum-cli probe -w ct1 r-0002:412,318
ferrum-cli measure distance -w ct1 r-0002:400,310 r-0002:431,322
ferrum-cli segment threshold -w ct1 --seed r-0002:412,318 --min -100 --max 200 --max-ml 50 --name "Nodule"
ferrum-cli engine info
ferrum-cli segment interactive -w ct1 --name "Nodule" --agent run-1 +r-0002:412,318
ferrum-cli segment interactive -w ct1 --segment 3 --append -r-0002:430,340
ferrum-cli segment interactive -w ct1 --segment 3 --undo
ferrum-cli segment interactive -w ct1 --name "Lesion" "lasso:r-0002:400,300;r-0002:430,300;r-0002:430,330"
ferrum-cli segment shape -w ct1 3
ferrum-cli segment auto -w ct1 --label liver --label spleen --name-prefix ts/
ferrum-cli segment compare -w ct1 4 9
ferrum-cli export bundle -w ct1
```
