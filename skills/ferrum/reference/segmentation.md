# Segmentation with engines

Parameters are written as in the MCP tools (`segment: L`); on the command
line they are flags (`--segment L`, see `commands.md`).

Engines are separate programs behind `ferrum-engine/1`: nnInteractive
(prompts), TotalSegmentator (automatic, one task per bridge), MONAI Label
(both). The operator decides which ones you may use (`engine list`). Their
results are **proposals** made by the engine at your request; a clinician
confirms them. Design: FERRUM's `docs/agent-segmentation.md`.

## 0. Preflight (every task)
1. `study info`: modality, `value_unit`, spacing, warnings. Stop on the
   wrong modality, missing coverage or strongly irregular slices.
2. `engine list`, then `engine info` for candidates: prompts, labels,
   modalities, `research_only`, `deterministic`, `gpu_group`.
3. Choose:

| Task | First choice | Fallback |
|---|---|---|
| A named organ or structure | automatic engine whose `labels` contain it (`segment auto --labels …`) | interactive engine at a located point |
| A lesion at a known place | interactive engine (`segment interactive`) | `segment threshold` |
| "All nodules / effusion / bleed" | the automatic sub-task, then refine each object interactively | — |

If only a research-only engine can do the task, tell the user before you
use it. `engine_unavailable` with "busy" or "GPU group" means wait and
retry; do not switch engines silently.

## 1. Organs (automatic)
1. `segment auto` with the `labels` you need (and `name_prefix` when a
   second engine will run, e.g. `monai/`).
2. Read `checks` of every segment (see §4). A segment touching the border
   is a lower bound; a left/right segment on the wrong side is reported,
   never renamed.
3. Optional cross-check: run a second engine **in a second workspace**
   on the same series (a voxel holds one segment), then
   `segment compare` with `a`, `b` and `b_workspace`. Dice < 0.90 or a
   volume difference > 10 %: report both values, do not choose.

## 2. A lesion (prompts)
1. Find it on renders (`view slice`, `view mpr`); check it with `probe`
   or `profile`. Never prompt an engine before you have seen the target.
2. First call: one include point at its centre, or a box on the slice
   where it is largest (both corners on that slice).
3. Look at its largest slice and its ends with `--overlay segments`
   (`segment shape` gives the slice numbers).
4. Refine **the same segment**: `segment interactive` with `segment: L`,
   `append: true` and an exclude point on a leak or an include point on a
   miss. `undo: true` drops the last prompt.
5. Stop when `checks.stability` ≥ 0.95, or after 8 prompts: then report
   "not converged".
6. Measure: `segment shape` (volume, long and short axis with end points
   and slice number) and `stats` with `segment: L`.

## 3. Find all, then refine
1. `segment auto` with the detection sub-task (e.g. `lung_nodules`).
2. `segment components` with `segment: L`, `min_ml: 0.01`, `split: true`:
   one segment per object, largest first.
3. For each object (at most 20): `view mpr` through its centroid; then
   `segment interactive` with a box on its largest slice and a centre
   point; compare with the candidate (`segment compare`). A refined volume
   over 3 × the candidate's is a leak: one exclude round, else report it.

## 4. Checks
Every engine result has `checks` and a warning per failed check:

| Check | Meaning | What you do |
|---|---|---|
| `empty` | the engine found nothing | report it |
| `size` | outside `min_ml`/`max_ml` you gave | refine or report |
| `components` | the largest part holds < 90 % | `segment edit` with `op: keep_largest` for organs only, never for lesions |
| `border` | touches the edge of the volume | report the volume as a lower bound |
| `roi` | touches the edge of the region sent to the engine | repeat with a larger `roi` or `whole_volume` |
| `laterality` | named left/right but lies on the other side | report it |
| `overlap` | voxels the engine marked belong to another segment (kept there) | report both segments |
| `stability` | the last refinement changed the object (Dice < 0.95) | refine further or report "not converged" |

## 5. Corrections
- `segment edit` (`keep_largest`, `fill_holes`, `restrict_to_box`,
  `remove_small`) works on segments you made or asked an engine for.
- `segment interactive` with `from_segment: L` redoes one of your
  segments with the engine, seeded with lassos from its mask; then refine
  as in §2.
- A person's segment is never changed; describe the problem instead.

## 6. Answer
- Every number with unit and method (e.g. "longest axial diameter 16.0 mm
  between voxel centres on axial slice 16, ± 1 mm").
- Name the engine, its version and "research use only" when marked.
- List failed checks and the items to review (`review list`).
