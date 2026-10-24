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
| `study scan` | `paths` | series: pseudonymised id, format, modality, dims, description; one entry per volume of a multi-volume series, with `group` (its series) and `volume` (`number`, `count`, `label`, `complete`, `b_value`, `echo_time_ms`, `echo_number`, `temporal_position`, `frame_time_s`) |
| `study open` | `workspace`, `path`, `series?` (a series or volume id; another volume of the workspace's series switches to it, keeping segments and annotations), `modality?` (for NIfTI, e.g. `CT`) | dims, spacing, LPS origin and direction, value unit, slice counts, window presets; `warnings` when the series could not be shown as stored (one volume of several, irregular or assumed spacing) |
| `study info` | `workspace` | the same, plus annotation and segment counts with pending reviews; `volume` and `volumes` of a multi-volume series (value provenance names the `volume`) |

## Looking
| Command | Key parameters | Returns |
|---|---|---|
| `view slice` | `plane`, `slice_number` or `at`, `window?` (default: the series' own, else 0.5–99.5 % of the values), `size?`, `overlays?` (`segments`), `segment_style?` (`outline`, `fill`, `fill_outline`), `segment_opacity?` | PNG + sidecar with `pixel_to_voxel`, `pixel_to_patient_mm` |
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
| `segment shape` | `segment`, `solid_min` | ml; extent in mm; slice ranges and largest slices; axial long and short axis with end points and slice number; border contact; components; laterality for names with left/right; with `solid_min`, `solid`: the same axes, ml and share of the voxels ≥ that value (you choose the threshold; report it as a measurement, never as a category) |
| `segment distance` | `segment`, `to` | shortest distance in mm from `segment` to the surface of `to`, the two closest points, `inside`, `overlap_voxels`, `uncertainty_mm`; measure distances between structures with this, not with picked points |
| `segment metrics` | `segment`, `profile?` (`quick` or `clinical`), `bin_width?` (HU, default 25), `hu_ranges?` (`[[min, max], …]`, half-open, not overlapping), `describe?` | profile name and version; unit; per feature `value`, `unit` (and `reason` if undefined): size (voxels, mm³, ml, extent per grid axis, centroid in LPS mm, axial long and short axis, largest slices) and first-order HU statistics (mean, std, min, max, range, p5–p95, IQR); `clinical` adds the shape (surface area, compactness, sphericity, principal axes with elongation, flatness and directions, a direction is `null` with a reason when two axes are equally long), the convex hull (volume, area, solidity, convexity), the maximum 3D diameter, and the connectivity (pieces, largest-piece share, cavities and their volume, Euler number, image sides touched; `warnings` name `fragmented`, `has_holes`, `touches_border`) and the HU distribution (skewness and kurtosis, energy, RMS, deviations, histogram entropy and uniformity on `bin_width`, V10/V90/I10/I90 and the `ivh` curve, `hu_ranges` shares with the share outside; the defaults are marked `default` and are not clinical thresholds); `describe` adds each definition. Needs a CT study: declare NIfTI with `study open --modality CT` |
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
