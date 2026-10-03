---
name: ferrum-size
description: Phase 4 of the FERRUM workflow — estimate the size of a stage or feature against what the current model reliably delivers in one PR and one session, classify it S/M/L/XL, and split XL work into PRs. Calibrated on the repository's merged PRs (history.sh, calibration.md) and recalibrated after every merge. Use after the plan's tasks are drafted, when choosing between feature and stage track, and when work grows during implementation.
---

# Sizing against the model

A PR must fit two limits:
1. **What the model delivers well in one session:** without losing
   context to compaction, without skipped checks, without bugs slipping
   to the field test.
2. **What a person can review.**

Stage 17 went in as one 7 000-line PR. It needed a context compaction,
and its review unit was too large (see `calibration.md`). Sizing makes
the split a planned decision.

## 1. Estimate each task

For each task of the plan:
1. Find the closest merged PR (`history.sh`, or the table in
   `calibration.md`).
2. Estimate **code lines** (production Rust, WGSL and Python) from that
   analogue. Then add:
   - tests: 30 % of the code (the range is 20–40 %); more when the task
     is safety- or geometry-heavy;
   - docs and schemas: 10–25 %;
   - generated files: count them separately and do not weigh them (for
     example `commands.json`).
3. Count the **files** and the **contract surfaces** it touches (list in
   `calibration.md`).
4. Add 25 % for unknowns when the area is new to the codebase.

Sum per task, then per candidate PR.

## 2. Classify

Use the budgets table in `calibration.md`; the strictest column decides.
- A task that alone exceeds L is too coarse: go back to `ferrum-plan`
  and cut it.
- In **autonomous mode** only M (or S) is allowed. Anything larger goes
  gated.

## 3. Split into PRs

- **Each PR is a set of whole tasks** that merges on its own with CI
  green and leaves `develop` releasable. A PR is never half a feature
  behind a flag nobody tests.
- **Size:** aim for M per PR, L at most.
- **Contract surfaces:** at most one new one per PR when possible. The
  protocol or schema change goes first, with its consumers in the same
  PR or the next.
- **Order** by dependencies: domain → io/engines → app → agent/CLI → UI
  → docs and benchmark.
- **Field tests:** schedule one after each PR that touches engines,
  rendering or formats. Do not save them for the end of the stage (the
  L1 lesson from #39).

Write the result into the plan's §7:

```markdown
| PR | Tasks | Est. lines (code/tests/docs) | Files | Surfaces | Class | Field test after |
|---|---|---|---|---|---|---|
| 1 | N.3 | 900/300/150 | 18 | — | M | no |
```

## 4. Watch during implementation

Re-estimate when one of these happens:
- the diff passes the estimate by 30 %;
- a new contract surface appears;
- the session had a context compaction.

Then:
- **Gated:** tell the user and propose moving the remaining tasks to the
  next PR.
- **Autonomous:** stop at the M limit, deliver what is complete as the
  PR, and list the remaining tasks in its body.

## 5. Recalibrate after the merge (retro)

Append a row to the *Log* of `calibration.md`:
- estimated class and lines, actual lines and files (`history.sh 1`);
- context compactions, CI rounds and bugs found after the merge.

Lower a budget by one step when two PRs in a row of that class needed a
compaction or let a bug through. Raise it only after five clean PRs at
the limit. When the model changes, start one class lower and
recalibrate.
