# Sizing calibration

The size budgets of `ferrum-size` come from this log. After every merged
feature PR, append one row to the table at the end, and adjust the
budgets when the outcomes say so (§5 of the skill).

## Budgets (current)

Valid for the model that did the work below (Claude Opus 5.5 class, one
cloud session per PR). For another model, start one class lower and
recalibrate after two PRs.

| Class | Lines changed (+/−, all files) | Files | Contract surfaces | Track | PRs |
|---|---|---|---|---|---|
| S | ≤ 300 | ≤ 8 | 0 | small | 1 |
| M | ≤ 1 500 | ≤ 25 | ≤ 1 | feature (autonomous allowed) | 1 |
| L | ≤ 3 500 | ≤ 40 | ≤ 2 | stage, gated | 1, or split if natural |
| XL | more | more | > 2 | stage, gated | split into M/L PRs |

**Contract surfaces:**
- agent command set and schemas;
- `ferrum-engine/1`;
- workspace formats;
- shaders and the CPU renderers;
- desktop UI layout;
- bridges' Python API.

## History (merged PRs into develop, from `history.sh`)

Lines are additions plus deletions. "Outcome" records what the size cost:
- context compactions in the session;
- CI rounds after the first push;
- bugs found after merge or in the field.

| PR | Branch | Files | Lines | Code | Tests | Docs | Class | Outcome |
|---|---|---|---|---|---|---|---|---|
| #17 | feat/engine-port | 17 | 2 244 | 1 759 | 225 | 34 | L | — |
| #19 | feat/ai-panel | 18 | 1 288 | 965 | 259 | 60 | M | — |
| #20 | feat/nninteractive-bridge | 19 | 1 290 | 749 | 214 | 224 | M | — |
| #21 | feat/automatic-segmentation | 15 | 992 | 700 | 265 | 27 | M | — |
| #22 | feat/totalsegmentator-bridge | 26 | 1 364 | 793 | 197 | 132 | M | — |
| #23 | feat/monailabel-bridge | 10 | 517 | 275 | 185 | 53 | M | — |
| #25 | feat/provenance-workspace | 30 | 1 945 | 1 593 | 79 | 200 | L | — |
| #26 | feat/agent-cli | 29 | 3 597 | 2 755 | 449 | 241 | XL (borderline) | — |
| #27 | feat/agent-mcp | 20 | 1 111 | 800 | 202 | 107 | M | — |
| #28 | feat/agent-views | 22 | 1 109 | 884 | 157 | 66 | M | — |
| #29 | feat/agent-skill-package | 31 | 2 957 | 489 | 330 | 349 | L (mostly generated schema) | — |
| #30 | feat/review-workspace | 21 | 784 | 516 | 228 | 40 | M | — |
| #31 | feat/agent-engines | 21 | 1 045 | 454 | 206 | 74 | M | — |
| #32 | feat/dicom-seg-sr | 22 | 1 631 | 1 184 | 310 | 115 | L (small) | — |
| #34 | feat/segmentation-workflow | 27 | 1 175 | 780 | 284 | 111 | M | — |
| #39 | feat/agent-segmentation (Stage 17) | 75 | 6 998 | 4 220 | 866 | 1 101 | **XL, not split** | ≥ 1 context compaction; after the field test 1 fix commit (outline bug L1 from 15.3, NIfTI modality L10, with display styles) and 2 docs commits (benchmark, scenarios) |

**Observations:**
- The median feature PR is ~1 100–1 300 lines in ~20 files. These PRs
  merged without recorded trouble. Earlier outcomes were not logged
  ("—"); log them from now on.
- Tests are 20–40 % of code lines, and docs 5–25 %. Use these ratios
  when estimating.
- Stage 17 as one 7 000-line PR fit in one session only with a context
  compaction.
  - Its review unit was too large to read.
  - Its field test came after all the code.

  With the split rule it would have been 3–4 PRs:
  1. analysis and checks;
  2. refinement, ROI and GPU groups;
  3. bridges;
  4. skill, docs and the benchmark.

  Each would have had its own field test.

## Log (append after each merged PR)

| Date | PR | Estimated class / lines | Actual lines / files | Compactions | CI rounds | Post-merge bugs | Note |
|---|---|---|---|---|---|---|---|
| 2026-10-03 | #39 | not estimated | 6 998 / 75 | ≥ 1 | 0 red | 0 so far (L1 pre-dated it) | budget derived from this |
