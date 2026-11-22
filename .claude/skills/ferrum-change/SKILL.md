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
   If the session handed you a branch (`claude/…`), rename it to
   `<prefix>/<topic>` and rebase it onto `origin/develop` before the first
   commit (L17). A clone without `origin/develop` is not "a repo without
   `develop`": fetch it first.
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
| The plan, architecture or a crate manifest (`dev_plan.md`, `docs/architecture.md`, `crates/*/Cargo.toml`) | regenerate the dashboard (`python scripts/dashboard/generate.py --ref HEAD`); publish after the merge (skill `ferrum-dashboard`) | hook G reminder |

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
- **Validate wire-format parsing against a real peer** (L54): a mock built
  from the same assumption as the code is self-consistent, not independent —
  it passed while the STOW success tag was wrong. Capture a real response as
  the fixture, or run the integration against the real service (CI service
  container / local Docker) before G3. A local graceful-skip is not validation;
  the G3 report must show the real-service result.
- **Cite the criterion** the test verifies with a comment on the test:
  `// covers 18.2-b` (`# covers …` in Python). `ferrum-analyze` reads
  these comments.

## 4. Check locally — the same as CI

The rule and its checklist live in `CLAUDE.md` (*Every push is preceded by a
local run*): `preflight.sh`, in WSL on Windows, plus the checks it does not
cover. Read them there; this section adds only what the command list cannot
say.

**What the command is** — `.claude/skills/ferrum-change/preflight.sh` from the
repo root, and its last line must read `all checks passed` (L19). It runs, in
CI order:
1. `cargo fmt --all -- --check`;
2. `cargo clippy --workspace --all-targets -- -D warnings`;
3. the schema freshness check (`ferrum-cli schema` against
   `skills/ferrum/schemas/commands.json`);
4. `FERRUM_REQUIRE_GPU=1 cargo test --workspace -- --test-threads=4`
   (lavapipe: `mesa-vulkan-drivers`);
5. with `--coverage`, `make coverage-ci` (floor in the `Makefile`).

**Why the order is fix → run → push once** (L34): every CI round costs the
project minutes it does not have, and a red push spends them on what a local
run answers for free. CI is then watched for the Linux-specific answers a
local run cannot give — not for the first look at the code.

**The platform split on Windows** — two runs answer two different questions:
- **WSL, without `FERRUM_REQUIRE_GPU`**: the Linux answers — hooks, ruff,
  fmt, clippy, schema, every non-GPU test and the UI tests;
- **the host (PowerShell)**: the Windows answers and, for now, the GPU
  renderer tests (`ferrum-render`'s parity suite, 14 tests, green).

The GPU half needs a Mesa new enough for wgpu 30. CI runs `ubuntu-latest`
(Mesa 24+); a WSL on Ubuntu 22.04 has Mesa 23.2 through `jammy-updates`, and
there the lavapipe driver segfaults in the first GPU parity test (`SIGSEGV` in
`gpu_parity`). The tests are not wrong, the driver is old: run the rest
without `FERRUM_REQUIRE_GPU` and take the GPU answer from the host. The two
things that make the WSL run work at all are fixed in the repository: the
working copy holds LF in shell scripts and data (`.gitattributes`,
`core.autocrlf=false`), and the schema check compares content, not line
endings.

A push made with only one of the two is a push whose other half CI will judge;
say which half was checked.

**A red suite is a defect, never a background** (L33). CI runs on Linux; a
change delivered from Windows or macOS may meet tests that only ever pass on
one platform. When a test fails before the change as well:
- fix it in the same branch — a path built with `os.path.relpath`, a Unix
  tool such as `du`, a fixture written without `encoding`, a root hard-coded
  as `/data`, a JSON compare that ignores `\\` — these are repository
  defects, and each one hides the next real failure;
- if it genuinely cannot be fixed here (a GPU that removes the device, a
  harness bug), say so in the PR in one sentence with the name of the test
  and the reason, and still run everything else green.

Never write "it failed before my change" and stop there.

Windows specifics (0.4.0 prep): a test exe that fails to *launch*
(os error 2/5/32) is an environment question first — antivirus
quarantines freshly linked binaries (L39); check the file exists and run
a surviving build directly. A GPU-test failure on a loaded desktop is
verified on a second backend (`WGPU_BACKEND=vulkan`) before it is called
a defect (L40). Never compare file content through the default codepage
— decode explicitly on both sides (L41, L29).

Notes:
- **Hooks catch some of this before you do** (`CLAUDE.md`, *Hooks*):
  - `rustfmt` runs after each edit of a `.rs` file, and `ruff format`
    plus safe fixes after each edit of a `.py` file (`ruff.toml`);
  - the push is blocked on fmt, a stale schema, a missing visual
    check, or ruff findings in changed Python files;
  - branch names that break skill `new-branch` are blocked;
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
- **Disk — a gate, not advice** (L53, supersedes L15). A full drive holding the
  WSL `ext4.vhdx` causes I/O errors, a read-only filesystem, rustc `SIGBUS` and
  `Wsl/Service/CreateInstance/E_FAIL` — it masquerades as a broken WSL. So:
  - `preflight.sh` **assesses free space on the build drive before building and
    logs it** to `.claude/state/build-space.log`; it refuses below ~10 GB.
  - Build into a **unique per-branch+date target dir** with
    `ferrum-build.sh` (sets `CARGO_TARGET_DIR=.builds/<branch>-<YYYYMMDD>` and
    runs the preflight), so artifacts never pile up in one shared `target/`.
    The committed `.cargo/config.toml` sets a **project-relative**
    `target-dir = ".builds"` (no absolute path — the repo clones cleanly on any
    machine), and `ferrum-build.sh` defaults `CARGO_HOME` to a project-relative
    `.cargo-home/` unless set locally, keeping the registry off the system
    drive too. Both `.builds/` and `.cargo-home/` are gitignored.
  - **Clean the branch's build dir when the branch merges** (deliver, §7);
    `ferrum-clean-builds.sh` previews merged/old `.builds/*` dirs and removes
    them only with `--apply` (regenerable artifacts only, never source/data).
  - On WSL `E_FAIL`/SIGBUS/read-only fs, **check the VHDX drive free space
    first** (L56), before services, drivers, or reboot.
- **Destructive git/file operations — assess first** (L55). Before any
  `reset --hard`, branch delete, or bulk delete: gather the full picture
  (`git status`, `git reflog`, `git worktree list`, which commits would be lost
  and whether they are merged/approved, whether the branch is checked out in
  another worktree). If it could lose **new, approved** code, STOP and propose
  an alternative (a detached worktree, a `backup-<name>` branch first,
  cherry-pick, stash). Verification worktrees are a **detached HEAD of an
  independent clone** (`git worktree add --detach <path> <ref>`) — never reset a
  branch checked out elsewhere (a shared-worktree reset moves the branch under
  the working tree and drops commits).

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

Then run the review agents (`.claude/agents/`) on the branch:
- `code-reviewer` for every PR;
- `security-reviewer` when the change touches:
  - readers;
  - `ferrum-agent`, `ferrum-cli` or the MCP server;
  - engines or bridges;
  - workspaces or exports;
  - configuration or dependencies.

Fix their blocking findings, or answer them in the PR body under
*Review*.

Run review subagents so partial results survive their death (L42):
instruct them to send findings in batches via `send_message`, not only
in the final answer; on a quota death, retry the agent after ~30 s
(bounded attempts) and keep the batches already received.

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
4. **Write the G3 report and post it to the PR.** Before asking for the
   merge, produce the HTML report on the final template's structure
   (`ferrum-report` §0 and §4.5) with a **code map**: each architectural
   claim linked to the `file:lines` that implement it, links pinned to the
   PR head SHA, and the SHA stated in the report. Post it as a PR comment;
   do not commit it, the code is not merged yet. Check every code-map entry
   after the last rebase and the last `cargo fmt` — a line number that moved
   is worse than none.
5. Ask for G3 with the report link and one paragraph: what the change is,
   what was verified, what was not, what is open. A merge request without
   the report is not a G3 request.
6. Merge only when the user says so. Use a merge commit, as in the
   repo's history. After the merge, regenerate the report from the merged
   tree as phase 11 (`ferrum-report`, `docs/<feature>/`).
7. **Clean the branch's build dir after the merge** (L53): remove
   `.builds/<branch>-*` (or run `ferrum-clean-builds.sh --apply`, which prunes
   merged/old branch dirs), so regenerable artifacts never fill the build
   drive. Delete only build artifacts — never source, patient data, or approved
   code, and assess consequences first (L55).

## 8. Record (retro)

- When the work produced a bug, a wasted CI or test round, or an
  approach that clearly paid off, add an entry with skill
  `ferrum-lessons` in the same branch.
- After the merge, append the sizing row (`ferrum-size` §5).
