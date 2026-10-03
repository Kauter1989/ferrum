# Lessons — what went wrong, what worked

The project's working memory for development sessions. Read it before
starting a change. Add an entry whenever a bug is found, a push or CI
round is wasted, or an approach clearly pays off (skill
`ferrum-lessons`).

Every failure entry names a **guard**: a test, a check in a skill or a
rule that would have caught it. An entry without a guard is unfinished.

Entries are append-only. When a lesson is superseded, add a new entry
that says so instead of editing the old one.

## Failures

### L1 — Segment outlines drawn on two sides only (2026-10, Stage 15.3 → fixed in 0.3.0)

- **Symptom:** In agent renders (`view slice`, `montage`, `mpr`), organs
  had outlines on the right and bottom edges only. The user found it on
  real CT renders; it had shipped for two stages.
- **Root cause:** `on_edge` in `crates/ferrum-agent/src/commands/view.rs`
  compared a pixel only with its right and lower neighbour.
  - A one-sided neighbourhood is a classic edge-detection shortcut;
    it is invisible in a test that only counts "some outline pixels".
  - Secondary issues:
    - a one-pixel line almost vanished at 8× zoom;
    - a green outline on a white background could not be seen.
- **Why it was missed:**
  - The tests asserted that outline pixels exist, not where they are or
    how many there should be.
  - Nobody looked at a render of a real study before the field test.
  - The phantoms were symmetric blobs, so a one-sided outline still
    "looked like" an outline in the counts.
- **Fix:** `on_edge` checks all four directions up to a zoom-dependent
  width (1–3 px, a third of a voxel on screen).
- **Guards:**
  - `slice_renders_draw_closed_outlines_or_translucent_fills`
    (`crates/ferrum-agent/tests/segmentation.rs`): exactly 20 outline
    pixels for a known square at 1:1, the 48²−42² ring at 8×;
  - `slice_overlay_fills_and_outlines_segments`
    (`crates/ferrum-render/tests/gpu_parity.rs`);
  - skill `ferrum-visual-check`.
- **Rule:**
  - Pixel output is tested with exact geometry on a known phantom, at
    1:1 and zoomed, on all four sides.
  - Before claiming a visual change works, open the rendered PNG
    yourself.

### L2 — Pushed straight to `develop` (2026-10, Stage 17 design)

- **Symptom:** A design commit went to `develop` without a PR. The user
  objected.
- **Rule:**
  - Work on `feat/`, `fix/`, `docs/`, `refactor/` or `chore/` branches.
  - `develop` and `main` change only through merged PRs.
  - Pushing a feature branch is fine. Merging needs the user's word.
- **Guard:** skill `ferrum-change`, step "Deliver".

### L3 — Design written against a stale clone (2026-10, Stage 17 design)

- **Symptom:** The push was rejected. Remote `develop` already had
  Stage 15.7 (engine commands, DICOM SEG/SR), and the design described
  missing features that already existed. The design doc and the ADR
  number had to be redone.
- **Rule:** `git fetch origin develop` and branch from
  `origin/develop` before reading the code for a design. Check the
  highest ADR number and `dev_plan.md` on the fresh base.
- **Guard:** skill `ferrum-change`, step "Start".

### L4 — Distance transform mixed up axes when one axis had size 1 (2026-10, Stage 17.3)

- **Symptom:** Caught by a unit test before commit: for a mask one
  column wide (`sx == 1`), the second axis pass ran with the first
  axis's base offsets, so distances (and HD95) were wrong.
- **Root cause:** The base offset of each line was chosen by comparing
  strides/sizes, which coincide when a dimension is 1.
- **Rule:** Dispatch on the axis index, never on a size or stride that
  can coincide. Test geometry code with anisotropic spacing and with
  dims of 1 on each axis.
- **Guard:** `distance_transform_is_exact`
  (`crates/ferrum-domain/src/analysis.rs`), case "a line of one column".

### L5 — A test that could not fail: ROI larger than the phantom (2026-10, Stage 17.2)

- **Symptom:** The ROI tests passed, but the default margin (48 mm)
  covered the whole phantom. Cropping was never exercised.
- **Rule:** Each test must be able to fail. Size phantoms so that the
  feature under test changes the outcome: the ROI must cut, the limit
  must trigger, the border must be touched or not.
  - Sanity-check: break the code on purpose once and see the test fail.
- **Guard:** the `roi` check and `regions_of_interest_are_cut_around_the_prompts`.

### L6 — System-made prompts counted against the user's limit (2026-10, Stage 17.7)

- **Symptom:** `--from-segment` lasso seeds used up
  `max_prompts_per_object`, so refinements failed with `limit`.
  `--undo` could remove the seeds.
- **Rule:**
  - Keep inputs the system generates apart from inputs the user or
    agent gives (`seeds` vs `prompts` in `engine_inputs.json`).
  - Limits and undo apply to the latter only.
- **Guard:** `lasso_seeds_do_not_count_against_the_prompt_limit`.

### L7 — An agent could overwrite a segment a person confirmed (2026-10, Stage 17.1)

- **Symptom:** Found in self-review: refinement rights were checked by
  `requested_by` only, not by status.
- **Rule:** Every command that mutates a segment checks provenance
  through one function (`changeable_segment`). A person's work and
  anything a person confirmed is `forbidden` for agents. Write the
  "forbidden" test together with the feature, not after.
- **Guard:** tests in `crates/ferrum-agent/tests/segmentation.rs` that
  expect `forbidden`.

### L8 — CLI flags flattened into the wrong subcommand (2026-10, 0.3.0)

- **Symptom:** `--segment-style` landed on `view volume` instead of
  `view slice`. The schema and the docs were right; the CLI was wrong.
- **Rule:** Every new flag gets a parse test in `crates/ferrum-cli`
  (`call(&["ferrum-cli", …])`) and one run of `ferrum-cli <cmd> --help`.
  The CLI and MCP must produce identical JSON.
- **Guard:** the CLI unit tests in `crates/ferrum-cli/src/cli.rs`.

### L9 — UI test broke because a button moved off-screen (2026-10, 0.3.0)

- **Symptom:** Two new rows in the Segments panel pushed *Confirm* to
  y = 1402 in a 1400 px test window. The UI test failed; the app itself
  was fine.
- **Rule:** When adding UI rows, run `cargo test -p ferrum --test ui`.
  On a layout failure, check whether the test window is too small
  before changing behaviour.

### L10 — NIfTI CT values were not treated as HU (2026-10, field test)

- **Symptom:** On a real NIfTI CT, `study open` warned "not in Hounsfield
  units", and window presets and stats were off. NIfTI has no modality.
- **Fix:** `study open --modality CT`, kept in the workspace manifest.
- **Rule:** Every input format: ask what metadata it lacks (modality,
  units, orientation, patient frame) and how the user supplies it.
  Test with NIfTI and DICOM both.
- **Guard:** `nifti_studies_can_be_declared_ct`.

### L11 — Field-test script ran a stale binary (2026-10, field test)

- **Symptom:** With `--skip-setup`, the script kept the old `ferrum-cli`.
  It rejected `--modality`, wrote the error to stderr only, and every
  later step failed with `no_study`.
  - A run of the user's GPU time was wasted.
  - The report showed `ok=` with no hint.
- **Rule:**
  - Field-test scripts always fetch the branch and rebuild FERRUM
    (incremental, cheap); "skip setup" skips only the heavy engine
    installs.
  - When a command prints no JSON, show its stderr in the report.
- **Guard:** skill `ferrum-field-test`.

### L12 — Environment failures on the user's machine (2026-10, field test)

- **Symptoms:**
  - DNS failures in WSL (`Temporary failure resolving`);
  - `operator torchvision::nms does not exist` (torch and torchvision
    from different indexes);
  - `nvidia-smi` missing inside a container.
- **Rule:**
  - A field-test script starts with preflight checks (network, GPU,
    disk, RAM) that stop with a clear message.
  - It installs torch and torchvision from the same index in one
    command and verifies the import.
  - Optional tools degrade, they do not abort.
- **Guard:** skill `ferrum-field-test`.

### L13 — Design numbers assumed, not measured (2026-10, Stage 17.6)

- **Symptoms:**
  - The design assumed an RTX 3080 Ti; the machine was an RTX 3080
    12 GB.
  - It claimed the ROI keeps nnInteractive "far below 12 GB". The
    measurement showed the same ~5.4 GB peak with and without the ROI.
    The real gain was the right object (whole volume: a 30 ml
    fragment).
- **Rule:**
  - Mark guidance as guidance until measured.
  - Ask for the exact hardware.
  - After a measurement, correct the doc even when it contradicts the
    design.

### L14 — Threshold seeded from an organ's own percentiles leaks (2026-10, field test S5)

- **Symptom:** `segment threshold` with the spleen's p5–p95 HU leaked
  into the liver and stomach. `--max-ml` stopped it, as designed.
- **Rule:** Scenario scripts and skills must expect `limit`, try a
  narrower range (p25–p75) once, then fall back to an engine. This is
  not a bug in FERRUM.

### L15 — Disk full during the coverage build (2026-10, Stage 17)

- **Symptom:** "No space left on device" while running tests after
  `make coverage`.
- **Rule:** Before a full build in a cloud session, delete
  `target/llvm-cov-target` and `target/release` if they are not
  needed; check `du -sh target`.

### L16 — A whole stage in one 7 000-line PR (2026-10, Stage 17, #39)

- **Symptom:** PR #39 changed 75 files and 7 000 lines.
  - The session needed a context compaction.
  - The PR was too large to review.
  - The only field test came after all the code, so its findings (L1,
    L10) landed as late fix commits.
- **Root cause:** Nothing sized the stage before implementation. The
  task table (17.1–17.7) existed but was not mapped to PRs.
- **Rule:** Size every stage against the model's budget before coding,
  and split it into M/L PRs, each with its own field test where
  relevant.
- **Guard:** skill `ferrum-size` (budgets and the log in
  `calibration.md`), gate G2 of `ferrum-workflow`.

## What worked

### W1 — Field test on the user's GPU with a throw-away script (2026-10, Stage 17)

- **What:** A script, attached and not committed, built the branch,
  installed the engines, downloaded a public labelled CT (MSD Task09),
  ran the scenarios and wrote `steps.txt` and `benchmark.md`.
- **Outcome:** It found L1, L10 and L14, gave Dice against ground truth
  (0.954 and 0.937) and the VRAM table. Nothing else found L1.
- **Keep:**
  - one field test per feature that touches engines, rendering or real
    data formats;
  - ask for `steps.txt`, logs and PNGs back.

### W2 — Exact-geometry phantom tests (2026-10, 0.3.0)

- **What:** Known shapes (a square, a box, shifted balls) with exact
  expected numbers: pixel counts, ml, Dice and HD95 from brute force.
- **Keep:** prefer an exact expected value over "> 0" or "is some".

### W3 — Contract tests through the public envelope (2026-10, Stage 17)

- **What:** Tests in `crates/ferrum-agent/tests` drive the commands with
  JSON, through the CLI and MCP, against the mock engine.
- **Outcome:** Refactors inside commands were safe; CLI/MCP parity was
  checked for free.

### W4 — Release through PRs, published by the workflow (2026-10, 0.3.0)

- **Steps:**
  1. A bump PR into `develop`.
  2. A `develop` → `main` PR with a merge commit "Release X.Y.Z (#N)".
  3. `release.yml` dispatched on `main` with `tag: vX.Y.Z`.
- **Keep:** skill `ferrum-release`.

### W5 — Reproducing a user-reported failure locally before answering (2026-10, field test)

- **What:** For the `ok=` report, a synthetic NIfTI with the current
  build showed that the code was fine. The cause was the stale binary
  (L11).
- **Keep:** reproduce with the current branch first, then decide whether
  the bug is in the code or in the environment.

### W6 — Mechanical rules moved into hooks (2026-10, workflow)

- **What:** Hooks enforce the rules a script can check:
  - protected branches (L2);
  - no data commits;
  - append-only ADRs;
  - asking before a merge or release;
  - fmt and schema freshness before a push;
  - a fresh phantom check for slice/overlay changes (L1);
  - rustfmt after edits;
  - matrix reminders (L1, L7, L8, L9, L10);
  - the session-start status (L3, L15);
  - the compaction log (L16).

  The tests are in `.claude/hooks/test_hooks.py`.
- **Outcome:** A live `git push --dry-run origin develop` was blocked
  in the session that installed the hooks.
- **Keep:** When a lesson's rule is mechanical, add a hook and a test
  for it. Leave judgement to the skills.
