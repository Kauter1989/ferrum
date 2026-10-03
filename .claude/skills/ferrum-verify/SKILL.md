---
name: ferrum-verify
description: Phase 8 of the FERRUM workflow — verify an implementation against its acceptance criteria on three levels (CI checks, phantom/visual, field test on the user's machine) and write the verification table for the PR, stating plainly what has not been verified. Use after implementation, before delivering a PR, and when field-test results arrive.
---

# Verify

Verification is against the **criteria** of the spec, not against "the
tests pass". Each criterion ends up with a level and an outcome.

## 1. Level 1 — CI checks (always)

```bash
.claude/skills/ferrum-change/preflight.sh            # fmt, clippy, schema, tests with GPU (lavapipe)
python3 .claude/skills/ferrum-analyze/analyze.py --stage <N>
```

- Both must pass.
- Add `--coverage` when you added a lot of code: the floor is in the
  `Makefile`.
- A GPU test that skipped is not a pass (`FERRUM_REQUIRE_GPU=1`).

## 2. Level 2 — phantom and visual (anything visible or numeric)

- **Pixels:** skill `ferrum-visual-check`. Run `phantom_render.py`, then
  open the PNGs.
- **Numbers** (ml, mm, HU, Dice): run the command once on a synthetic
  study with a known answer and read the JSON (W5).
- **Desktop UI:** `cargo test -p ferrum --test ui`. Name what a person
  should check by hand (`manual` row).

## 3. Level 3 — field test (engines, rendering, formats, performance)

Skill `ferrum-field-test`:
- Prepare the script per PR, not per stage (`ferrum-size` §3).
- Compare the results with the **success metrics** of the spec.
- Turn each finding into a test or a lesson before the merge.

If the user cannot run it now, the PR says so. The affected criteria
stay "not verified on real data".

## 4. The verification table (PR body)

```markdown
## Verification

| Criterion | Level | Evidence | Outcome |
|---|---|---|---|
| [18.1-a] | test | `segmentation.rs::outline_is_closed_at_any_zoom` | ✅ |
| [18.1-b] | visual | `phantom_render.py` (all checks), looked at outline@64px, fill_outline@512px | ✅ |
| [18.3-a] | field | — | ⏳ not run yet: needs the user's GPU |

Not verified: <list>. Assumptions (autonomous mode): <list>.
```

**Honesty rules:**
- Never mark ✅ for a level that did not run.
- "Looked at" names the images.
- A skipped GPU test is ⏳, not ✅.

## 5. After verification

- Fix findings before delivering. Each fix gets its test and, when it
  was a real slip, a lesson (`ferrum-lessons`).
- Deliver with `ferrum-change` §7, and drive CI to green.
- Merging waits for the user (G3).
