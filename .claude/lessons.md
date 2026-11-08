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

### L17 — Worked on a harness-named branch in a stale clone, outside the workflow (2026-10, region tool, 16.6)

- **Symptom:** The cloud session handed over the branch
  `claude/loving-franklin-4eg2ly`, cut from `main`. The local clone had no
  `origin/develop`, so `.claude/`, the hooks, `ferrum-workflow` and
  `CLAUDE.md`'s workflow section were not on disk. The change (a
  noise-robust region tool) was built and pushed with no spec, no
  criteria, no `preflight.sh` and no verification table, and a duplicate
  branch skill was written. A UI test also hit L9 again (the Region
  settings made the panel taller than the 1 400 px test window).
- **Root cause:** `main`-only clone taken as "there is no `develop`"; no
  `git fetch origin develop` before the first decision.
- **Rule:** At the start of every task, run `git fetch origin develop`
  and read `.claude/` from it before anything else. A branch name set by
  the harness is renamed to `<prefix>/<topic>` and rebased onto
  `origin/develop` before the first commit. Re-run `preflight.sh` with
  lavapipe installed (`apt-get install mesa-vulkan-drivers libvulkan1`);
  without a Vulkan driver the GPU tests are skipped, unlike CI.
- **Guard:** `ferrum-change` §1 (step on harness-named branches),
  session-start hook H (fresh `origin/develop`, branch status).

### L18 — A voxel-based filter passed on a 1 mm phantom and failed on real CT (2026-10, region tool, 16.6)

- **Symptom:** The region tool's smoothing and opening were counted in
  voxels. Phantom tests (1 mm cubes) gave Dice 0.97–0.999. On MSD liver CT
  (0.7 × 0.7 × 5 mm) the opening of one voxel cut nothing: the liver leaked
  into kidney, muscle and spleen (Dice 0.38–0.78), and a bridge 4 mm wide
  and one slice thick looked "thick" in voxels. A second field run showed
  one click taking 35–70 s on a 0.6 × 0.6 × 0.8 mm study (60 M voxel box).
- **Root cause:** Isotropic synthetic data hid the unit (L4 is the same
  family). No timing was taken on a realistic volume size.
- **Rule:** Sizes of filters and morphology are in millimetres, never
  voxels. Every phantom for such a feature has anisotropic spacing and a
  realistic volume size, and the test says how long it may take. Run a
  field test on real cases before saying a feature "works".
- **Guard:** `region::tests::the_opening_works_in_millimetres_on_thick_slices`;
  the field-test record in `docs/testing.md`; `ferrum-verify` (field level).

### L19 — Pushed after reading only the first lines of `preflight.sh` (2026-10-07, region tool, 16.6)

- **Symptom:** Commit `6b4ec7f` was pushed while the preflight output had
  been cut by `head`; its failing app test (a block-based opening shrank a
  12 mm ball from 799 to 633 voxels) showed up as a red `check` and
  `coverage` in CI. The next commit fixed it.
- **Rule:** Before every push, the last line of `preflight.sh` must read
  `all checks passed`; never filter its output to a prefix.
- **Guard:** `ferrum-change` §4 (read the whole result); prefer
  `preflight.sh | tail -3`.

### L20 — A default tuned on the liver was useless on the lung (2026-10, region tool, 16.6-n)

- **Symptom:** After the liver field test the default tolerance was capped
  at 40 HU. On MSD lung CT that grew nothing (aerated parenchyma varies by
  hundreds of HU; ±200…±400 gives Dice 0.92–0.97 for one lung). It was
  found only because the user asked for the lungs.
- **Rule:** A default that depends on tissue must be checked on every
  tissue the feature claims (liver, lung, …) before it is called "works";
  prefer a value derived from the data (here: the clicked value) to one
  constant. Say what the reference is (the lung set has no human lung mask:
  a classical reference was used and named as such).
- **Guard:** `workflow.rs::the_tolerance_comes_from_the_clicked_value_until_the_user_sets_it`;
  the lung row in `docs/testing.md`.

### L21 — The benchmark used a smooth ball; real masks were 30 to 100 times slower (2026-10, radiomics, 18.12-c)

- **Symptom:** The design budget was "`clinical` ≤ 2 s for 100 000 voxels"
  and the PR 6 benchmark said 607 ms. The field test on the MSD spleen took
  3 s for a mask of 83 000 voxels and 87 s for 893 000. The cause was the
  first convex hull: every point against every face. A ball has few hull
  faces; a real mask cut to a box (frayed rim, pieces everywhere, hundreds
  of cavities) has many. Reproduced with a blobby synthetic mask: 16.7 s for
  101 752 voxels.
- **Rule:** A performance number is for the *shape* it was measured on.
  Benchmark an algorithm whose cost depends on the shape (hull, flood fill,
  mesh) on a smooth shape, a frayed one and a space-filling blobby one, and
  at 10 times the stated size, before writing the number into the spec.
- **Guard:** `crates/ferrum-radiomics/benches/profiles.rs` (ball, ragged,
  blobby, at about 100 000 and 1 000 000 voxels); the old hull stays in the
  tests as a reference (`the_fast_hull_equals_the_reference_on_lattice_clouds_…`);
  `docs/radiomics.md` §3.2 names the shapes.

### L22 — A field-test script proved on an isolated phantom failed on tissue that touches the organ (2026-10, radiomics field test)

- **Symptom:** The first field-test script grew a spleen with the spleen's
  own value range. On real CT every case hit the size limit (the liver,
  kidney and muscle have the same HU), so no test ran. The dry run on a
  phantom had passed because the phantom's cube was isolated (and L14 had
  already named this leak).
- **Rule:** A script for the user's machine is rehearsed on a fixture that
  contains the confounder the real data has, not only the clean case. For
  a region grown from an organ's own range: soft tissue of the same value
  around it. Cut such regions to a box around the organ instead of
  narrowing the range.
- **Guard:** `ferrum-field-test` §2 (rehearse on a leaky fixture); the
  script keeps a high limit and `restrict_to_box`. Related: a local branch
  named `origin/develop` made the short name ambiguous, so scripts use full
  ref names (`refs/remotes/origin/develop`) and check out detached.

### L23 — DICOM volumes were never brought into LPS; every phantom was axial (2026-10, field check of Stage 20)

- **Symptom:** The DICOM reader kept the stored voxel order and only
  recorded the direction, while renders, plane labels and tools assume
  `+i` left, `+j` posterior, `+k` superior. A sagittal or coronal MR was
  shown in the wrong planes, and a series with rows towards the patient's
  right was mirrored, labelled R/L as an axial CT. It shipped for many
  stages: every synthetic DICOM in the tests was axial with row `+x`, and
  probe and measurements were right (they go through the direction), so
  nothing numeric failed. NIfTI was reoriented all along.
- **Rule:** Every reader brings volumes into the canonical frame, and its
  tests include sagittal, coronal, reversed-row, feet-first and oblique
  inputs, compared against an independent reader by patient position, not
  only by voxel index.
- **Guard:** `assemble::tests::dicom_grids_are_brought_into_lps`,
  `dicom_integration.rs::a_sagittal_series_is_stored_in_lps`,
  `dicom_export.rs::a_segmentation_of_a_sagittal_series_lies_on_the_lps_grid`;
  the field check repacks a real NIfTI as sagittal and coronal DICOM and
  requires the same MPR (W8).

### L24 — The fake repository carried what real files lack (2026-10, field check of 20.2)

- **Symptom:** The volume switch passed every use-case test, and the
  field check on real files found two bugs at once. A 4D PET NIfTI asked
  for its image type again after each switch (the new volume's metadata
  had no modality, and the declaration lived on the old one). A Canon
  Enhanced DTI went black at b = 1500, because the file stores one window
  for all volumes, chosen for b = 0, and the switch used it. The fake
  repository of the tests set `modality: "CT"` and no stored window, the
  two things real MR and PET files differ in.
- **Rule:** A fake repository for a use case varies what real sources
  vary: no modality (NIfTI), a stored window shared by all volumes, and
  values on different scales per volume. Run the use case on at least one
  real file of each kind before the PR is called done.
- **Guard:** `crates/ferrum-app/tests/volumes.rs::the_image_type_said_once_holds_for_every_volume`,
  `a_window_stored_for_the_series_does_not_hide_the_other_volumes`; the
  Stage 20 field check (`dcm_qa` sets, OpenNeuro PET) with screenshots.

### L25 — Ported FERRUM's rules to another repository from a stale clone (2026-10, medical-harness bootstrap)

- **Symptom:** The new `medical-harness` repository was set up with
  development rules "adapted from FERRUM". They were adapted from a
  checkout on an old session branch. It had no `.claude/` workflow:
  `ferrum-workflow` and the other skills, `ferrum_hooks.py`,
  `lessons.md`. The harness got three ad-hoc hooks instead. Ruff and
  review agents were also planned against that old state.
- **Root cause:** The files were read from the working tree of the
  session's branch, not from `origin/develop`. The session-start hook
  (H), which reports how far behind `origin/develop` a branch is, did
  not exist on that old branch, so nothing warned.
- **Why it was missed:** L3 and L17 cover designing and committing in
  FERRUM itself. They did not cover reading FERRUM as the source for
  another repository.
- **Fix:** The harness workflow is re-ported from `origin/develop`, and
  this branch starts from a fresh `origin/develop`.
- **Guard:** `new-branch` and `ferrum-change` §1 (fresh `origin/develop`)
  now also apply to reading. Before porting or quoting FERRUM rules
  elsewhere, run `git fetch origin develop` and read with `git show
  origin/develop:<path>`, or from a branch cut from it.
- **Rule:** The source of truth for FERRUM's rules is `origin/develop`,
  never a session branch.

### L26 — Hook J read heredoc bodies and redirections as git arguments (2026-10, review agents and hooks)

- **Symptom:** On its first real use, the new branch-name hook J
  blocked two harmless commands in this session:
  - `git push … 2>&1 | tail`: `2>&1` was read as a branch name;
  - writing a skill file through a heredoc: an example line inside it,
    `git switch -c <type>/<description> origin/develop`, was parsed as a
    command.
- **Root cause:** `segments()` in `.claude/hooks/ferrum_hooks.py` split
  the whole command text at newlines, including here-document bodies,
  and kept redirection tokens as arguments. Hooks A and B had the same
  blind spot, but it never mattered to them.
- **Why it was missed:** Every hook test used a one-line command without
  redirections or heredocs, unlike the commands sessions actually run.
- **Fix:** `without_heredocs()` and `without_redirections()` run before
  the command is split (#66).
- **Guard:** `test_redirections_are_not_branch_names` and
  `test_heredoc_bodies_are_data` in `.claude/hooks/test_hooks.py`.
- **Rule:** Hook tests include the command shapes sessions actually
  use: pipes, redirections, heredocs, `&&` chains, `git -C`.

### L27 — A released file lock stayed held by a child another thread forked (2026-10, Stage 23 PR 2 CI)

- **Symptom:** On CI, `leases_share_the_capacity_through_files` found its
  group "busy" right after the only holder had left: a zero-timeout
  attempt gave a timeout with no holders. Locally it passed every time.
- **Root cause:** `flock` belongs to the open file description, not to
  the descriptor. Dropping a `File` only closes our descriptor. When
  another thread forks a child at that moment (the parallel process
  tests, the bridge's workers, the agent's subprocesses), the child holds
  a copy of every descriptor until it `exec`s, so the lock outlives our
  `drop`.
- **Why it was missed:** The race needs a fork in the same few
  microseconds. The tests that fork ran next to it only by chance, on CI's
  slower runner.
- **Fix:** Locks are unlocked explicitly before the file is closed
  (`Held` guard for `admission.lock`, `Lease::drop`, stale checks).
- **Guard:** `locks_released_while_other_threads_spawn_processes` in
  `crates/ferrum-gpu-lock/tests/processes.rs` fails three times out of
  three without the fix.
- **Rule:** Never rely on closing a file to release a `flock`/`File::lock`
  in a process that may spawn children: unlock explicitly. A test that
  passes locally and fails once on CI is a race to reproduce, not a flake.
- **Same family (Stage 23 PR 2 and 3):** a child holder that tried its
  lease once (`Duration::ZERO`) lost to the parent's `holders()` poll,
  which takes the admission lock briefly. It failed 1 run in 15–20 under
  load: first in `gpu_sharing.rs`, then in `ferrum-gpu-lock/tests/processes.rs`.
  A test's child holder waits for its lease; only a test of the refusal
  itself tries once.

### L28 — Processes a test started outlived it and hung the run (2026-10, Stage 23 PR 3)

- **Symptom:** A mutation check of `gpu hold` (the fix reverted) never
  finished. The file it was to restore stayed mutated, and the next build
  tested the wrong code. Earlier, `pkill -f "ferrum-cli gpu hold"` killed
  the shell that ran it, because its own command line matched.
- **Root cause:** The failing tests left their `sh` loops running
  (`while true`). Their stdout was the pipe into `grep`, so the pipeline
  waited for them forever, and the restore step after it never ran.
- **Why it was missed:** The tests passed with the fix, so the loops
  always ended. Only a failing run left them behind.
- **Guard:** Scripts that tests run loop for a bounded time (about 20 s,
  `crates/ferrum-cli/tests/cli.rs`), and every wait for a child has a
  timeout (`wait_or_kill`). For a mutation check, build first, run the
  test binary under `timeout`, and restore in a separate step. Find stray
  processes with `ps` and kill them by pid, never with `pkill -f`.

### L29 — A caption edit rewrote whole files: PowerShell 5.1 and UTF-8 (2026-11, Stage 25 PR 1)

- **Symptom:** While writing Stage 25 PR 1 on Windows, `.md` files and one
  test file were edited with `Get-Content -Raw … | Set-Content -Encoding
  utf8`. `dev_plan.md` and `docs/view-host.md` came back with a byte-order
  mark and CP1251 mojibake (`Stages 1–13` became `Stages 1вЂ“13`), and a
  test comment lost three dash characters. `git diff --stat` showed 421
  changed lines in a file that had gained 77.
- **Root cause:** Windows PowerShell 5.1 reads a BOM-less file through the
  ANSI code page (`Get-Content` default) and writes UTF-8 *with* a BOM, so
  every `–`, `—`, `≤` or `×` round-trips through CP1251; a second pass
  double-encodes it.
- **Why it was missed:** `git diff --stat` was read only after `git add`,
  and the corruption looks plausible in a console whose code page matches
  the mojibake.
- **Fix:** the files were restored from their original bytes
  (`text.encode("cp1251").decode("utf-8")`, BOM stripped) and the staged
  diff then showed additions only.
- **Rule:** Edit repository text with the editor tool, never by rewriting a
  whole file through a shell. On Windows use PowerShell 7
  (`-Encoding utf8NoBOM`) or Python with `newline="\n"`. Read
  `git diff --cached --numstat` before every commit: a small edit must
  show a small count, and one changed line must show one insertion, not a
  rewrite of the file.
- **Guard:** this rule and the `git diff --cached --numstat` step in the
  delivery checklist (`ferrum-change` §2).

### L30 — An iframe into a harness panel: the secure context said no (2026-11, Stage 26)

- **Symptom:** The DeepSeek Harness plugin showed a black panel. The host
  answered every route, the page drew correctly in a normal browser, and the
  plugin's own smoke script passed.
- **Root cause:** The Desktop window is loaded from the `dsh-app://` scheme,
  which Chromium treats as a secure context; such a page does not render
  `http://` content in a frame, so the iframe stayed empty. Serving
  `http://127.0.0.1` is not a substitute for rendering in the panel. The same
  rule kills a cross-origin `import()` of a `http://` module.
- **Why it was missed:** The page was verified in a plain browser tab, where
  the iframe works, so every check that ran said "the server is fine". No
  check ran inside the harness window, and the harness reports nothing there.
- **Fix:** The panel renders the study itself (`crates/ferrum-wasm`, WebGPU
  in a `<canvas>`); the host page is not framed at all. PR #91 replaced #90.
- **Guard:** `docs/webgpu-panel.md` records the constraint with the
  measurement; the plugin smoke script checks the bundle's registrations and
  never claims the image was seen; the PR body carries the "not verified"
  row when the panel was not opened in a real harness window
  (`ferrum-verify` §4).
- **Rule:** For a harness integration, verify inside the harness's own
  renderer, not in a browser tab: a secure-context host (`dsh-app://`,
  `app://`, `tauri://`) refuses what a plain page allows. When a frame stays
  black while the server answers, suspect the embedding policy before the
  code.

### L31 — A registered tab type that no menu could offer (2026-11, Stage 26)

- **Symptom:** After the panel rendered, the person could not open it in the
  right sidebar: the strip's "+" menu had no FERRUM entry, and the header
  button always opened the panel in the centre column.
- **Root cause:** Two independent defects in the plugin's client half.
  1. `sidebarRightTabs.register` accepted the type, but the "+" menu is built
     from the type's `guide` entries, and the registration passed `guide: []`.
     A type with no guide entry is registered and offered nowhere.
  2. The header button called `sidebarRight.openTab`, which needs a focused
     pane (`require()`); outside one it throws, and the plugin's own fallback
     took over — the centre column. The built-in Browser uses
     `commandTarget(element)` + `openTabFromTarget(kind, target)`, which binds
     the tab to the session.
- **Why it was missed:** The smoke script checked that the slots were
  registered, not that the type could be reached from the UI; and the first
  "fix" guessed at the API instead of reading how the shipped Browser plugin
  registers its own tab.
- **Fix:** A guide entry with title, description and icon; the header button
  opens through `openTabFromTarget` and reveals a collapsed pane
  (`isExpanded`/`toggleExpanded`), guarded so a build without those methods
  still opens the tab. Commit `bee370b`.
- **Guard:** `smoke-load.cjs` asserts the guide entry, its title, the session
  binding, that a collapsed pane is revealed and that an open pane is not
  collapsed; the fallback is asserted separately.
- **Rule:** When a plugin extends a harness's own UI, read the shipped plugin
  that owns that surface (`dsh-client-ui-sidebar-browser`) and copy its
  registration path field by field. A registration that succeeds is not a
  registration that is reachable, and a test must assert reachability, not
  just the call.

### L32 — Chasing an API contract by reading minified bundles (2026-11, Stage 26)

- **Symptom:** Three rounds were spent on the right-sidebar tab, each ending
  in a new guess about the slot contract. One guess (declaring
  `dsh.client.inject: ["@deepseek-ai/dsh-client-ui-sidebar-right"]` in the
  plugin manifest) dropped the plugin out of the client graph entirely,
  because that package is not a dependency of the profile.
- **Root cause:** The harness's client code is bundled and minified; reading
  it gives fragments, not contracts. Meanwhile the harness ships tools that
  answer directly: `boot()` composes the profile, `prepareProfileEntries`
  reports what it would disable, the host prints activation warnings to
  stderr, and the served page embeds the whole client graph.
- **Why it was missed:** Debugging started from the docs and the bundle text
  instead of from the running system's own diagnostics.
- **Fix:** The working method, recorded as W9.
- **Guard:** W9 states the order of investigation for a harness integration;
  `integrations/dsh/plugin/lib/index.js` records its activation so "the row
  never loaded" and "the panel is not visible" are distinguishable.
- **Rule:** With a bundled host, do not infer a contract from minified
  source; drive the host's own boot, composition and graph, and read what it
  says.

### L33 — A red test suite accepted as "it was already red" (2026-11, Stage 26)

- **Symptom:** `cargo test --workspace` failed in six places and
  `test_hooks.py` in three, all on this Windows machine. Each was checked
  against `origin/develop`, found to fail there too, written into the PR as
  "pre-existing", and the work went on. The user stopped it: a commit must
  not leave the suite red.
- **Root cause:** The checks were platform-blind. They were written and are
  run in CI on Linux, where every case below happens to be true:
  - `guard_edit` built its repository path with `os.path.relpath`, which
    returns backslashes on Windows, while the ADR pattern and
    `git cat-file` expect `/` — so a merged ADR could be rewritten with no
    hook complaint;
  - `session_start` sized `target/` with `du`, a Unix tool; on Windows
    `FileNotFoundError` escaped and the whole session-start message
    disappeared;
  - the hook tests wrote fixtures without `encoding`, so the locale code page
    met the hook's UTF-8 reader;
  - three tests hard-coded Unix roots (`/data/in`, `/run/user/1000/…`);
  - `mcp.rs` replaced the workspace root in JSON, where a Windows separator
    is `\\`, so the replacement never matched;
  - `package.rs` compared `SKILL.md` with `\n` against a CRLF checkout;
  - two helpers sat behind a `#[cfg(unix)]` test but were not gated
    themselves, which `-D warnings` reports on Windows.
- **Why it was missed:** "Red on develop too" answered "is this my
  regression" and was taken as "not my problem". It answers neither of the
  questions that matter: does the project pass on the machine the work runs
  on, and does CI cover that platform?
- **Fix:** Commit `1035d28`: paths normalised to `/`, a portable size walk,
  fixtures written as UTF-8, roots built from the running platform, both JSON
  spellings replaced, line endings normalised, the helpers gated like their
  user.
- **Guard:** the hook tests run before a change that touches hooks
  (`CLAUDE.md`, *Hooks*), and the delivery checklist expects the suite to be
  green, not "as green as before" (`ferrum-change` §4). The new L33 rule
  states what to do with a pre-existing failure.
- **Rule:** A failing test is a defect of the repository, not of the
  platform it was noticed on: fix it, or exclude it on purpose with a
  written reason inside the test. Never hand a branch where the suite is
  red, and never let "it failed before my change" stand as an answer — at
  minimum say in the same breath which platform CI covers and which it does
  not.

### L34 — Three red CI pushes that a local run would have caught (2026-11, Stage 26)

- **Symptom:** The same branch was pushed three times in a row, and CI failed
  each time within seconds — twice in `python-lint` (four ruff findings, then
  a missing trailing newline and a misplaced `noqa`), and once earlier on the
  manifest. Nothing in the branch was wrong in principle: every finding was
  mechanical and fixed in a minute. What it cost was three CI runs, three
  waits of six minutes on `check`, and three rounds of context.
- **Root cause:** The checks were run *after* the push and read from the CI
  log. Locally, only the tests of the crates that had changed had been run,
  and `python-lint` was not run at all — the hook that would have run ruff
  printed "ruff is not installed" and that line was not acted on.
- **Why it was missed:** A Windows machine was treated as a reason to skip
  the Linux-shaped checks. It is not: WSL runs the same Ubuntu toolchain CI
  uses, and lavapipe was already installed there.
- **Fix:** `preflight.sh` runs in WSL on this machine; the script itself was
  broken by a CRLF working copy (`/usr/bin/env: 'bash\r': Permission
  denied`), which `.gitattributes` (`*.sh eol=lf`) and
  `core.autocrlf=false` now prevent, and its schema check compares content
  rather than bytes. ruff 0.16.9, the version CI pins, is installed locally.
- **Guard:** the rule is part of the development rules, not only of this
  file: `CLAUDE.md` has the section *Every push is preceded by a local run —
  a rule, not advice* with the checklist (preflight, WSL on Windows, the
  checks outside it, push once, a red suite is a defect), and
  `ferrum-workflow`, `ferrum-change` §4 and `ferrum-verify` §1 each point at
  it where a push is decided.
- **Rule:** Push once, after the checks pass locally. A CI minute spent on a
  finding `ruff check` or `cargo fmt` would have shown at once is a minute
  the project cannot pay for; on Windows that means WSL, not skipping.

### L35 — A native `<select>` popup does not inherit the page's colours (2026-11, Stage 26)

- **Symptom:** In the harness panel the 3D mode drop-down showed its selected
  value, but opening it revealed white text on a white popup: the options
  were there and could not be read. The user found it at once.
- **Root cause:** The `<select>` carried `color: inherit`, so in the harness's
  dark theme the options computed to `rgb(230,231,234)` over a *transparent*
  background. A native popup is not painted by the page: the platform paints
  the list with its own palette, which in a light system theme is white, and
  the inherited near-white text landed on it. Measured with
  `getComputedStyle` on both variants: before, option text `rgb(230,231,234)`
  on `transparent`; after, `rgb(29,31,38)` on `rgb(255,255,255)`.
- **Why it was missed:** The closed state renders correctly, so every look at
  the toolbar said "fine"; the defect only exists while the popup is open,
  and no check painted one.
- **Fix:** The list and its options state their own colours, and
  `color-scheme: light` makes the popup follow them.
- **Guard:** `integrations/dsh/tests/test_panel.py::test_the_mode_dropdown_states_its_own_colours`
  requires a style for both the list and the options with a foreground and a
  background, and checks the four modes are still offered; it fails when the
  colour is removed from the option style. `ferrum-visual-check` gains the
  question for native popups.
- **Rule:** A native popup (`<select>`, `<datalist>`, a context menu) is
  drawn by the platform, not by the page: give it explicit colours and a
  `color-scheme`, and never rely on `inherit` across that boundary.

### L36 — A stage declared L that grew to XL, and the hard stop nobody took (2026-11, Stage 26)

- **Symptom:** `dev_plan.md` records Stage 26 as **Size: L** and its
  implementation table has an empty `PR` column. The branch it produced,
  `feat/webgpu-panel`, is **+5386 / −33 across 55 files** — class XL by the
  budget in `ferrum-size/calibration.md` (L is ≤ 3500 lines, ≤ 40 files; XL
  must be "split into M/L PRs"). It ran for days, was pushed eleven times,
  and was only reviewed after the user asked whether the review step had been
  skipped.
- **Root cause:** Size was estimated once, before the work, from the design;
  nothing re-measured it as the work grew. The branch's own commits show the
  natural split that was available and never taken: `ferrum-wasm` (5 files),
  `ferrum-view` (11), the DeepSeek Harness panel (10), plus the docs.
- **Why the stop was missed:** `ferrum-workflow` §2 names "grow beyond size M
  during implementation" as a **hard stop, even in autonomous mode**. The
  check is written as a rule but has no trigger: no hook and no checklist item
  asks for the diff size, so nothing fires when a branch passes a budget. The
  estimate was treated as a formality instead of a commitment.
- **What it cost:** one PR that cannot be reviewed in one sitting (which is
  how two tests that could not fail, an unauthenticated CORS header and two
  weakened tests reached the branch), a duplicated open PR (#89 carried the
  first 25 files of the same branch and was merged into this one), and two
  review agents asked to judge 55 files.
- **Guard:** measured, not remembered —
  `git diff --shortstat origin/develop...HEAD` before a push. `ferrum-size`
  §5 should be checked against the actual numbers whenever a stage closes,
  and the implementation table's `PR` column must be filled before the first
  push, not after.
- **Rule:** The size estimate is a commitment, not a formality. Re-measure
  with `git diff --shortstat` before each push; when a stage crosses its class
  (M → L, L → XL), stop and split — the commit boundaries usually show where.
  A stage that ends XL has produced work nobody can review properly, whatever
  its tests say.

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

### W7 — Prototype on real cases in Python, then port (2026-10, region tool, 16.6)

- **What:** After the field test showed the first design failing, the
  fix was explored in `numpy`/`scipy` on four public MSD liver cases (a
  grid of tolerance, smoothing and opening, 20 combinations in minutes),
  then ported to Rust and re-run through the same field script.
- **Outcome:** The prototype found the one change that mattered
  (millimetre-based opening: mean Dice 0.74 → 0.92) before any Rust was
  written. Self-testing the field script on a phantom found a real bug
  (a seed outside the tolerance was reported as "too large").
- **Keep:** For an algorithm that depends on data, try the parameter grid
  on real cases in a scripting language first; self-test every field
  script on a phantom before sending it.

### W8 — Repack one real volume in several formats and compare (2026-10, DICOM orientation)

- **What:** A public T1 (OpenNeuro, CC0) was repacked with pydicom as a
  sagittal and a coronal DICOM series with its real, slightly oblique
  geometry, and FERRUM's renders and probes of all three were compared with
  the NIfTI's.
- **Outcome:** It showed the orientation bug at once (correlation 0.64 and
  wrong planes before, 0.9999 after) and caught a mistake in the repacking
  itself (an axis-aligned assumption on an oblique affine), because the
  probes disagreed with the NIfTI before any FERRUM change.
- **Keep:** When no real data of a format can be downloaded, repack real
  data of another format with an independent library, verify the packing
  against that library first, then compare FERRUM across the formats.

### W9 — Building a harness plugin: probe the host, then copy the shipped plugin (2026-11, Stage 26)

- **What:** The order that finally worked for the DeepSeek Harness panel,
  after three rounds of guessing:
  1. **Prove the platform first.** A probe crate plus a stand page
     (`crates/ferrum-wasm`, `integrations/dsh/panel/stand.html`) answered
     "can the browser draw with our renderer at all" before any plugin code:
     144 fps, WebGPU, one afternoon. Every later failure was then known not
     to be the renderer.
  2. **Ask the host's own boots, not its source.** The runtime is asar-
     wrapped, so `ELECTRON_RUN_AS_NODE=1 "<Harness>.exe" <script>` runs the
     harness's own `boot()`/`composeEntries()`/`loadProfileDirectory()` and
     prints the entry tree it would mount; `reportSkippedBundles` and the
     activation audit write to stderr, which the Electron window drops — run
     the host from a console and read it. A profile copy in a temp directory
     makes this safe.
  3. **Read the graph the harness serves.** `GET /` embeds `__DSH_BOOT__`
     with every client bundle, its phase and its `inject` list: it answers
     "is my bundle there, and does it come after the package it needs".
  4. **Copy the shipped plugin that owns the surface.** For a right-sidebar
     tab: `dsh-client-ui-sidebar-browser` (`sidebarRightTabs.register` with a
     `guide` entry, `sidebar.right.pane.tab` + `.title` keyed by the type id,
     `commandTarget` + `openTabFromTarget`). For a header action:
     `dsh-client-ui-jobs`. The contract is the shipped code, not a doc.
  5. **Make the host half record that it applied.** One line appended to a
     file tells "the row never loaded" from "the panel is not visible"; a
     harness reports activation failures where nobody looks.
  6. **Verify in the harness's renderer.** The panel ran in a browser tab
     for hours and stayed black inside the harness (L30).
- **Outcome:** Steps 2 and 4 ended three rounds of guessing in minutes each;
  the tab, the token and the WebGPU canvas all work in the harness now.
- **Keep:** For any host that mounts plugins (a harness, an IDE, a window
  manager), the order is: probe the platform in isolation, ask the host's own
  boot what it mounts, read the served graph, copy the shipped plugin that
  owns the surface, leave a trace in the host half, then verify inside the
  host's own renderer.

### W10 — One renderer, two hosts: the wasm binding as a thin layer (2026-11, Stage 26)

- **What:** `crates/ferrum-wasm` does not re-implement anything: it takes a
  `<canvas>`, gets the WebGPU adapter, and calls `VolumeRenderer`,
  `FrameParams`, `OrbitCamera` and `RenderMode` exactly as the desktop does.
  `ferrum-domain`, `ferrum-processing` and `ferrum-app` compiled for
  `wasm32` with no source change at all.
- **Outcome:** The browser and the desktop cannot drift: a shader or frame
  model change reaches both at once, and the rendering rule of `CLAUDE.md`
  still has one implementation. The whole crate is behind
  `#![cfg(target_arch = "wasm32")]`, so the native workspace build is
  unaffected.
- **Keep:** When a second host must draw the same thing, bind the existing
  renderer rather than port it; treat "the dependency compiles for the other
  target unchanged" as the design constraint, and check it before writing
  the binding.
