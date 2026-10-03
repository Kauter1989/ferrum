# Segmentation scenarios

How an AI agent segments a study with FERRUM and an AI engine, step by
step, with the commands it runs. The scenarios cover organ volumetry,
lesions by prompts, detection, quantification, corrections, follow-up,
MR and dataset pre-labelling, on one 12 GB GPU.

This is the playbook. The design and its reasons are in
[agent-segmentation.md](agent-segmentation.md) and
[ADR 0010](decisions/0010-agent-segmentation.md). Every option is in the
command reference [agent-cli.md](agent-cli.md). The agent-facing
version is in the skill
([`skills/ferrum/reference/segmentation.md`](../skills/ferrum/reference/segmentation.md)).

| # | Scenario | Engines | Verified on real data |
|---|---|---|---|
| [S0](#s0--preflight-and-engine-choice) | Preflight and engine choice | all | ✅ |
| [S1](#s1--organ-volumetry) | Organ volumetry | TotalSegmentator `total` (cross-check: MONAI Label) | ✅ Dice 0.954 (spleen) |
| [S2](#s2--lesion-or-structure-by-prompts) | Lesion or structure by prompts | nnInteractive (MONAI SAM2 / DeepGrow) | ✅ Dice 0.937 (spleen) |
| [S3](#s3--detect-then-refine) | Detect, then refine | TotalSegmentator sub-task → nnInteractive | components only |
| [S4](#s4--fluid-and-tissue-quantification) | Fluid and tissue quantification | TotalSegmentator sub-tasks | — |
| [S5](#s5--correct-a-segment) | Correct a segment | nnInteractive | partly (see S5) |
| [S6](#s6--follow-up-comparison) | Follow-up comparison | TotalSegmentator `--fast` + nnInteractive | — |
| [S7](#s7--mr-structures) | MR structures | TotalSegmentator `total_mr`, MONAI, nnInteractive | — |
| [S8](#s8--pre-labelling-a-dataset) | Pre-labelling a dataset | automatic engines; review in the desktop app | — |

The scenarios marked "verified" ran end to end through `ferrum-cli` and
the bridges, on an RTX 3080 12 GB under WSL 2. The data was the CT
`spleen_10` from the Medical Segmentation Decathlon (Task09), compared
with its ground truth. The rest are covered by phantom tests with the
mock engine (`crates/ferrum-agent/tests/segmentation.rs`) but have not
yet run with a real model.

## Rules for every scenario

- **Results are proposals.** Every engine segment is saved with status
  *proposed*, author `engine` and `requested_by` the agent. Only a person
  confirms it (desktop app, *Confirm / Reject*). The agent may refine,
  rename and delete its own proposals, never a person's segment.
- **Numbers come from voxels.** Volumes, diameters, Dice and HU come
  from FERRUM's commands, never from the vision model. The vision model
  only finds locations and flags leaks.
- **Checks are reported, not hidden.** Every engine result carries
  `checks`. The agent names every failed check in its answer (see
  [Quality checks](#quality-checks)).
- **Licences are shown.** nnInteractive weights are CC BY-NC-SA 4.0,
  so its results are *research use only*. The envelope warns about it,
  and so does the agent.
- **One heavy engine at a time.** On a 12 GB card the engines take turns
  (`gpu_groups`). The agent never runs two engine commands in parallel.
  On `engine_unavailable` with "retry in N s", it waits.
- **Stop instead of guessing.** If the agent cannot find the structure,
  or the series does not cover it, it says so and does not prompt
  blindly.

## Set-up

Start the bridges ([bridges/README.md](../bridges/README.md),
[ai-demo.md](ai-demo.md)):

```bash
cd bridges
docker compose --profile interactive up -d    # nnInteractive          127.0.0.1:8765
docker compose --profile automatic up -d      # TotalSegmentator total 127.0.0.1:8766, sub-task 8768
# docker compose --profile mixed up -d        # TotalSegmentator --fast + MONAI Label 8767
```

Operator configuration (`FERRUM_AGENT_CONFIG=ferrum-agent.toml`):

```toml
[data]
read_roots = ["/data"]
workspace_root = "/work/ferrum"

[network]
engines = ["http://127.0.0.1:8765", "http://127.0.0.1:8766", "http://127.0.0.1:8768"]
gpu_groups = { gpu0 = ["http://127.0.0.1:8765", "http://127.0.0.1:8766", "http://127.0.0.1:8768"] }

[limits]
engine_job_timeout_s = 900
max_prompts_per_object = 8
roi_margin_mm = 48
allow_research_only = true      # false: nnInteractive is forbidden
```

The examples below use `NNI=http://127.0.0.1:8765` and
`TS=http://127.0.0.1:8766`. Every command answers with a
`ferrum-agent/1` JSON envelope. Over MCP, the same commands are the tools
`ferrum_segment_auto`, `ferrum_segment_interactive` and so on, and give
the same JSON.

Points are written as `v:i,j,k` (voxel), `mm:x,y,z` (LPS millimetres) or
`r-0007:412,318` (a pixel of an earlier render). Prompts start with `+`
(include) or `-` (exclude).

## S0 — Preflight and engine choice

```bash
ferrum-cli study open -w ct1 /data/ct.nii.gz --modality CT   # NIfTI carries no modality
ferrum-cli study info -w ct1                                  # modality, HU, dims, spacing, warnings
ferrum-cli engine list                                        # reachable, modes, labels, research_only, GPU group
```

- Stop on irregular spacing, the wrong modality or a series that does
  not cover the region. Mention strongly anisotropic voxels (e.g.
  5 mm slices).
- For NIfTI the agent declares the modality from the task or the user
  (`--modality CT`). Without it, values are not taken as HU.
- Choose the engine:

| Task | First choice | Fallback | Refuse when |
|---|---|---|---|
| Named organ or structure in CT | TotalSegmentator `total` | MONAI `segmentation`, then nnInteractive | no automatic engine knows the label and there is no location |
| Named organ in MR | TotalSegmentator `total_mr` | nnInteractive | multi-channel input needed |
| A lesion at a known location | nnInteractive | MONAI SAM2 box, DeepGrow | no location and no detection task |
| "All nodules / effusion / …" | the TotalSegmentator sub-task | — | no bridge serves the sub-task |
| A structure no model knows | nnInteractive | `segment threshold` | — |

If only a research-only engine can do the task, the agent asks the user
first.

## S1 — Organ volumetry

*"Volumes of the liver, spleen and kidneys."*

```bash
ferrum-cli view montage -w ct1 --plane coronal --step 10 --window soft_tissue   # coverage
ferrum-cli segment auto -w ct1 --engine $TS --modality CT --agent a1 \
    --label liver --label spleen --label kidney_left --label kidney_right
ferrum-cli segment shape -w ct1 2                     # per segment: ml, extent, slices, long axis
ferrum-cli stats -w ct1 --segment 2                   # HU median, IQR, percentiles
ferrum-cli view montage -w ct1 --plane axial --step 10 --window soft_tissue \
    --overlay segments --segment-style outline        # leaks, for the vision model
ferrum-cli export bundle -w ct1 --format ferrum --format dicom
```

- Read the `checks` of each segment:
  - `components`: one dominant part;
  - `border`: the organ is cut off by the field of view, so its volume is
    a lower bound;
  - `laterality`: `kidney_left` lies on the patient's left.
- Report HU statistics as values only. The agent does not interpret them.
- **Cross-check (optional):** open a second workspace on the same series
  and run MONAI Label `segmentation` with `--name-prefix monai/`, then
  `segment compare -w ct1 2 5 --b-workspace ct1-monai`. Dice below 0.90
  or a volume difference above 10 % is reported as *engines disagree*,
  with both values. The agent does not pick one.
- **Report:** organ, volume in ml, method (engine, task, version), failed
  checks, items to review.

Measured: 4 organs in 52 s (2 organs in 45 s, ~2.2 GB GPU memory). Spleen
256.4 ml vs. 253.1 ml ground truth, Dice 0.954.

## S2 — Lesion or structure by prompts

*"Segment the lesion in segment 4 of the liver, axial slice 87."*

```bash
ferrum-cli view slice -w ct1 --plane axial --slice-number 87 --window soft_tissue   # → r-0007
ferrum-cli probe -w ct1 r-0007:412,318                                              # value contrast
ferrum-cli segment interactive -w ct1 --engine $NNI --modality CT --agent a1 \
    --name "Lesion 1" +r-0007:412,318 --min-ml 0.1 --max-ml 200                     # → segment 7
ferrum-cli segment shape -w ct1 7                                                   # largest slice, ends
ferrum-cli view slice -w ct1 --plane axial --slice-number 90 --overlay segments     # → r-0009
ferrum-cli segment interactive -w ct1 --engine $NNI --segment 7 --append -r-0009:301,255   # remove a leak
ferrum-cli segment interactive -w ct1 --engine $NNI --segment 7 --undo                     # take it back
ferrum-cli segment shape -w ct1 7                                                   # long and short axis
ferrum-cli stats -w ct1 --segment 7
```

1. **Locate.** Use renders and `probe` or `profile` across the lesion.
   If it cannot be found, stop.
2. **First prompt.** Use a point at the centre. For low contrast, a box
   on the largest slice works better: `+box:r-0007:380,290:r-0007:445,350`.
3. **Refine** the same object (`--segment 7`):
   - an exclude point on a leak;
   - an include point on a miss, preferably on another slice.

   Stop when `checks.stability` reports Dice ≥ 0.95 between the last two
   revisions, or at `max_prompts_per_object`, and say which. Each call
   replays the stored prompts, so the result does not depend on a live
   engine session.
4. **Measure** from voxels: `segment shape` gives the volume, the
   longest axial diameter and its perpendicular, with their end points.
5. **Export** and flag *research use only*. The prompts are stored with
   the segment (`engine_inputs.json`).

Only a region around the prompts is uploaded (their box + 48 mm). If the
object reaches the edge of that region, the `roi` check fails: repeat
with `--roi MIN MAX` or `--whole-volume`. On the test series the ROI also
gave the right object. The same point sent with the whole volume gave a
30 ml fragment instead of ~250 ml.

Measured: ~1 s per prompt, refinements included; ~5.4 GB GPU memory over
the baseline. Spleen from one point, one refinement and an undo: 270.3 ml vs.
253.1 ml ground truth, Dice 0.937. Against TotalSegmentator: Dice 0.945,
HD95 5.0 mm.

## S3 — Detect, then refine

*"Find and measure all lung nodules."*

```bash
ferrum-cli segment auto -w ct1 --engine http://127.0.0.1:8768 --name-prefix candidate/   # lung_nodules
ferrum-cli segment components -w ct1 3 --min-ml 0.01 --split    # one segment per candidate
ferrum-cli segment shape -w ct1 12                              # box, centre, largest slice
ferrum-cli view mpr -w ct1 v:251,198,156 --window lung          # plausible or doubtful?
ferrum-cli segment interactive -w ct1 --engine $NNI --name "Nodule 1" \
    +box:v:240,188,156:v:262,208,156 +v:251,198,156
ferrum-cli segment compare -w ct1 12 20                         # candidate vs. refined
```

- Check first that the slices are thin (≤ 2.5 mm). Otherwise warn that
  small nodules are missed.
- Refine at most 20 candidates. List the rest without refining them.
- The vision model labels each candidate *plausible* or *doubtful*
  (vessel, scar, artefact) and gives a reason. Doubtful candidates are
  kept, with the reason.
- A refined volume more than 3× the candidate's means a leak. Do one
  exclude round, then report it.
- **Report:** a table with nodule, location (slice, mm, lobe if `total`
  ran), volume, long and short axis, and flags.

## S4 — Fluid and tissue quantification

- **Effusion:** run TotalSegmentator `pleural_pericard_effusion` as in
  S1, per side by laterality. A volume below 5 ml is reported as "not
  reliably measurable", not as a number.
- **Tissue at a vertebral level** (e.g. L3):
  1. Find `vertebrae_L3` with `total --fast`.
  2. Take its axial slice range from `segment shape`.
  3. Measure the slab: `stats -w ct1 --segment 9 --box v:0,0,41 v:511,511,44`.
- Licensed tasks (`tissue_types`) only with the operator's licence. The
  research badge is shown.

## S5 — Correct a segment

*"The threshold spleen leaks into the stomach; fix it."*

```bash
ferrum-cli segment threshold -w ct1 --seed v:381,282,31 --min 70 --max 130 --max-ml 600 --name "Spleen"
ferrum-cli segment shape -w ct1 5                                       # before
ferrum-cli segment interactive -w ct1 --engine $NNI --from-segment 5 -v:400,250,31
ferrum-cli segment compare -w ct1 5 5 --b-workspace ct1-before          # change in ml and Dice
ferrum-cli segment edit -w ct1 5 --op keep_largest                      # deterministic clean-up
```

- `--from-segment` seeds the engine with lassos made from the mask on its
  largest axial, coronal and sagittal slices. It then applies the agent's
  points and writes the result into the same segment, now an engine
  proposal. Objects made by `segment interactive` are refined directly
  (`--segment 5 --append …`).
- Keep the "before" state: a copy in a second workspace on the same
  series, or at least its `segment shape`. The summary says what changed
  and where.
- `segment edit` (`keep_largest`, `fill_holes`, `restrict_to_box`,
  `remove_small`) is for organs. It is never used to drop parts of a
  lesion.
- A person's segment is never changed (`forbidden`). The agent describes
  the error with its location and measurements instead.

On the test series a threshold from the spleen's own 5th–95th
percentiles leaked into the liver and stomach. `--max-ml 600` stopped it
(`limit`), as intended. The agent then narrows the range or goes
straight to nnInteractive (S2). The `--from-segment` step is covered by
phantom tests; it has not yet run with a real model.

## S6 — Follow-up comparison

FERRUM has no registration. The correspondence between time points is
anatomical, and the reviewer confirms it.

1. Open both studies in two workspaces (`base`, `fu`).
2. Segment the lesion in `base` (S2).
3. Describe its position: vertebral level and organ from `total --fast`,
   plus the offset in mm from the organ's centroid (`segment shape`).
4. In `fu`, render the same region, locate the lesion, and segment it
   with the same prompt types.
5. Report:
   - both volumes and diameters, with their methods;
   - the change;
   - *correspondence proposed by the agent*.

   A lesion that is not found is reported as "not found by the agent",
   never as "resolved".

## S7 — MR structures

- Use `total_mr` for organs, MONAI `prostate_mri_anatomy` for T2
  prostate zones, and nnInteractive for anything else.
- `--modality MR`. Values have no physical unit: report volumes and shape,
  and intensities only as "signal, arbitrary units".

## S8 — Pre-labelling a dataset

- One workspace per study, with the same engine, task and version for
  all. Check `engine info` before each study, and stop the batch if any
  of them changes.
- Automatic engines and deterministic checks only, without vision-based
  refinement, so the proposals are reproducible.
- The agent's summary per study lists the failed checks, so annotators
  start with the doubtful cases.
- Annotators review the studies in the desktop app (*Confirm all*,
  corrections recorded). The dataset is exported with only confirmed
  labels, so it never contains unreviewed model output.

## Quality checks

Every command that creates or changes a segment returns `data.checks`.
`checks.failed` lists the failures, and each failure also comes as a
warning.

| Check | Fails when | The agent… |
|---|---|---|
| `empty` | no voxel | reports "the engine found nothing" |
| `size` | outside `--min-ml` / `--max-ml` | refines (S2) or reports |
| `components` | the largest 26-connected part < 90 % | `keep_largest` for organs only; reports it for lesions |
| `border` | touches the volume edge | reports "cut off by the field of view; lower bound" |
| `roi` | reaches an inner face of the uploaded region | repeats with `--roi` or `--whole-volume` |
| `laterality` | `_left`/`_right` name on the wrong side of the midline | reports it, never renames |
| `overlap` | > 1 % of the result belongs to other segments | reports both segments |
| `stability` | Dice to the previous revision < 0.95 | reports "not converged" |

## Display for review

Renders draw segments as closed outlines by default. For a reviewer,
outlines plus a translucent fill are often easier to read:

```bash
ferrum-cli view slice -w ct1 --plane axial --slice-number 32 \
    --overlay segments --segment-style fill_outline --segment-opacity 0.35
```

In the desktop app, the *Segments* panel offers *Outline / Fill / Both*
and a *Fill opacity* slider.

## GPU budget (12 GB)

| Step | Time | GPU memory over a ~2.3 GB baseline |
|---|---|---|
| TotalSegmentator `total`, 2–4 organs | 45–52 s | ~2.2 GB, freed after the job |
| nnInteractive, per prompt (ROI or whole volume) | ~1 s | ~5.4 GB while the session is open |
| shape, components, compare, stats, renders | 0.2–0.5 s | none (CPU) |

Measured on an RTX 3080 12 GB (WSL 2) with
`scripts/benchmark_engines.py`. Details and rules are in
[agent-segmentation.md §2](agent-segmentation.md#2-gpu-budget-on-a-12-gb-rtx-3080--3080-ti).
