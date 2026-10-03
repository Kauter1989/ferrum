---
name: ferrum-analyze
description: Phase 6 of the FERRUM workflow — a read-only consistency check before and after implementation. Every acceptance criterion maps to a task and a test or validation row (analyze.py), every task's matrix rows are planned, and spec, design and code do not contradict each other. Use after tasks are drafted (before coding), and again before opening the PR.
---

# Analyze

Analyze changes nothing; it reports. It runs **twice**:
1. **Before implementation.** The validation plan covers every
   criterion, and the tasks cover the matrix. Holes found now are cheap.
2. **Before the PR.** The tests that the plan promised exist and cite
   their criteria.

## 1. Criterion coverage (mechanical)

```bash
python3 .claude/skills/ferrum-analyze/analyze.py --stage <N>
```

For each `[N.k-x]` in the stage section of `dev_plan.md`, it lists:
- **tests:** code files that cite the ID. The convention is a comment on
  the test: `// covers 18.2-b`, or `# covers 18.2-b` in Python.
- **other levels:** validation-table rows in `docs/*.md` that name the
  ID with `visual`, `field` or `manual`.

It also flags:
- criteria whose task has no row;
- duplicate IDs;
- IDs cited in code that the spec does not define.

Before implementation, uncovered criteria are expected for `test`. Then
check the design's validation table instead (`ferrum-plan` §1.6).
Before the PR, the exit code must be 0.

## 2. Judgement checks (by reading)

| Check | How |
|---|---|
| Spec ↔ design | Each criterion has a validation row. The design adds no behaviour that the spec lacks (or the spec is updated). |
| Design ↔ constitution | The constitution check of the plan has no unexplained "no". |
| Tasks ↔ matrix | For every "Touches" entry, the follow-ups of `ferrum-change` §2 are in the task (schema, docs, contract and CLI-parse tests, parity, `forbidden` test). |
| Visible criteria | Each has an exact-geometry test **and** a `visual` row (L1). |
| Test strength | Planned tests assert exact values, and their phantoms are sized so that the feature changes the outcome (L5, W2). |
| Docs ↔ code (before PR) | `docs/agent-cli.md`, `commands.md`, the workspace formats and the README say what the code now does. Look for stale numbers and stale "pending" notes. |
| Size (before PR) | The diff is within the planned class (`ferrum-size` §4). |

## 3. Output

A short report: the `analyze.py` table, then the findings of §2, each
with its fix. Act on it:
- **Gated mode:** findings that change the spec or design go to the
  user. Fix the rest yourself.
- **Autonomous mode:** fix what stays within the approved scope. Record
  the rest under *Assumptions* in the PR, or stop on a hard stop.

Before the PR, the report goes into the PR body. `ferrum-verify`
completes it.
