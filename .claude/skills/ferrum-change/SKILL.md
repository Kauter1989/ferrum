---
name: ferrum-change
description: Implement and deliver phases of the FERRUM workflow (ferrum-workflow) — branching from a fresh develop, the "what else must change" matrix (schemas, docs, contract tests, render parity), tests that can fail and cite their criteria, local checks that mirror CI (preflight.sh), and the PR flow. Use for every code or docs change, from a one-line fix to one PR of a stage; small changes use it alone.
---

# Changing FERRUM

Follow the steps in order. Each step names the lesson it comes from
(`.claude/lessons.md`, L = failure, W = what worked).

This is phase 7 (implement) and phase 9 (deliver) of `ferrum-workflow`.
For a stage or a feature, the spec, plan, size and tasks come first
(`ferrum-specify`, `ferrum-plan`, `ferrum-size`, `ferrum-analyze`). A
small change starts here.

## 1. Start

1. Read `.claude/lessons.md`. Note every lesson that touches the area you
   will change, and keep its rule in mind.
2. Fetch and branch from the fresh base (L3):
   ```bash
   git fetch origin develop
   git checkout -b feat/<topic> origin/develop    # or fix/, docs/, refactor/, chore/
   ```
   Never commit on `develop` or `main` (L2).
3. For a design or a stage, first check what already exists on the fresh
   base:
   - `dev_plan.md`;
   - `CHANGELOG.md` (`[Unreleased]`);
   - the highest number in `docs/decisions/`.
4. Ask the user for facts you would otherwise assume: hardware, data,
   which engine, licence constraints (L13).

## 2. Plan the full change

Find every row of the matrix your change touches, and plan its whole
"also update" column before writing code.

| You change… | Also update | Checked by |
|---|---|---|
| An agent command or its parameters (`crates/ferrum-agent`) | `schema.rs`; `ferrum-cli schema > skills/ferrum/schemas/commands.json`; `skills/ferrum/reference/commands.md`; `docs/agent-cli.md`; a contract test in `crates/ferrum-agent/tests`; a CLI parse test in `crates/ferrum-cli/src/cli.rs` (L8) | `crates/ferrum-cli/tests/package.rs`, contract tests |
| Agent outputs (new fields, files) | the identifier scan must still pass | `crates/ferrum-agent/tests/privacy.rs` |
| Anything that draws pixels (slice, overlay, outline, volume) | `slice.wgsl`/`volume.wgsl` **and** the CPU path (`cpu/raycast.rs`, `ferrum-agent` `view.rs`); a parity test; skill `ferrum-visual-check` (L1) | `crates/ferrum-render/tests/gpu_parity.rs`, exact-geometry tests |
| The engine protocol | `docs/engine-protocol.md`, the conformance suite, the bridges and their tests | `crates/ferrum-engines/tests/conformance.rs`, `pytest` in `bridges/` |
| Workspace files or manifests | `docs/workspace-format.md` (with the version example) | unit tests in `crates/ferrum-io/src/workspace.rs` |
| Segment mutation rules | `changeable_segment`; a test that expects `forbidden` for a person's or confirmed segment (L7) | `crates/ferrum-agent/tests/segmentation.rs` |
| Inputs the system generates for the user (seeds, defaults) | keep them apart from user inputs in limits and undo (L6) | |
| An input format reader | reorient into LPS; what metadata is missing (modality, units)? (L10) | `crates/ferrum-io/tests` |
| The desktop UI | use cases in `ferrum-app` (`Viewer`), not in widgets; `cargo test -p ferrum --test ui` (L9) | UI tests |
| The architecture or a decision | `docs/architecture.md`; a new ADR (existing ADRs are append-only) | review |
| User-visible behaviour | `CHANGELOG.md` `[Unreleased]`; README if a feature row or a link changes | review |

Layering: `ferrum` → `ferrum-app` → `ferrum-domain` ← `ferrum-io`;
`ferrum-app` without wgpu/egui; `ferrum-agent` without the `gpu` feature;
DICOM tags only via `dicom_dictionary_std::tags`.

## 3. Write tests that can fail

- **Exact expected values on known phantoms**, not "> 0" or "is some"
  (W2, L1). Examples: pixel counts of an outline, ml of a box, Dice of
  shifted balls, HD95 from brute force.
- **Size the phantom so the feature changes the outcome** (L5): the ROI
  must cut, the limit must trigger, the border must be touched.
- **Edge cases of geometry** (L4): anisotropic spacing, dims of 1 on each
  axis, both NIfTI and DICOM, left/right asymmetry.
- **Safety cases first** (L7): the `forbidden` and `limit` paths.
- **Break the code on purpose once** to see the new test fail. Then
  restore it.
- **Cite the criterion** the test verifies with a comment on the test:
  `// covers 18.2-b` (`# covers …` in Python). `ferrum-analyze` reads
  these comments.

## 4. Check locally — the same as CI

Run `.claude/skills/ferrum-change/preflight.sh` from the repo root. It
runs, in CI order:
1. `cargo fmt --all -- --check`;
2. `cargo clippy --workspace --all-targets -- -D warnings`;
3. the schema freshness check (`ferrum-cli schema` against
   `skills/ferrum/schemas/commands.json`);
4. `FERRUM_REQUIRE_GPU=1 cargo test --workspace -- --test-threads=4`
   (lavapipe: `mesa-vulkan-drivers`);
5. with `--coverage`, `make coverage-ci` (floor in the `Makefile`).

Notes:
- **Hooks catch some of this before you do** (`CLAUDE.md`, *Hooks*):
  - `rustfmt` runs after each edit;
  - the push is blocked on fmt, a stale schema or a missing visual
    check;
  - commits on `develop`/`main` and data files are blocked.

  The hooks do not run clippy or the tests: `preflight.sh` does.
- **Clippy traps seen before:**
  - `is_multiple_of`;
  - `needless_range_loop`;
  - `type_complexity` (add a type alias);
  - `identity_op`;
  - complexity budget: cognitive 25, 120 lines, nesting 6 — split the
    function, never `allow`.
- **No `unwrap()`/`expect()` outside tests.** Every public item has a doc
  comment.
- **Disk:** the cloud allowance is small (L15). Remove
  `target/llvm-cov-target` and `target/release` when not needed.

## 5. Look at the result yourself

- **Rendering or overlays:** run skill `ferrum-visual-check`.
- **Agent commands:** run the command once with the debug build against
  a synthetic study, and read the JSON (W5).
- **Engines, real data formats, performance:** plan a field test with
  skill `ferrum-field-test` (W1). Say in the PR which parts have and
  have not run on real data.

## 6. Re-read the diff adversarially

Ask of every hunk:
- what input breaks it;
- which side or axis it ignores;
- what the docs now say that is no longer true.

Fix what you find before pushing. Never claim a check ran when it
did not.

## 7. Deliver

1. Commit with a descriptive message. End it with the attribution lines
   the session requires.
2. Push the feature branch: `git push -u origin <branch>`. Pushing a
   feature branch is allowed. Never push to `develop` or `main` (L2).
3. Open a PR into `develop`:
   - gated mode: when the user wants one;
   - autonomous mode: as the end of the work.

   The body holds the analyze report, the verification table
   (`ferrum-verify`) and, in autonomous mode, the *Assumptions*. Then
   watch CI and reviews, and drive the PR to green.
4. Merge only when the user says so. Use a merge commit, as in the
   repo's history.

## 8. Record (retro)

- When the work produced a bug, a wasted CI or test round, or an
  approach that clearly paid off, add an entry with skill
  `ferrum-lessons` in the same branch.
- After the merge, append the sizing row (`ferrum-size` §5).
