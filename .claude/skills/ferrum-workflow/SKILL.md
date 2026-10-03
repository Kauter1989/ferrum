---
name: ferrum-workflow
description: Entry point for any FERRUM work request — a stage, a feature, a fix, docs. Picks the track (stage / feature / small) and the mode (gated or autonomous), then runs the phases specify → clarify → plan → size → tasks → analyze → implement → verify → deliver → retro with the right skills and approval gates. Use at the start of every task in this repository, and when the user asks to work "autonomously".
---

# FERRUM development workflow

The workflow adapts GitHub Spec Kit (constitution → specify → clarify →
plan → tasks → analyze → implement) to FERRUM. It adds what Spec Kit
lacks and what cost us most:
- sizing against the model's capacity;
- verification on real data;
- a lessons loop.

**Constitution:**
- `CLAUDE.md` (principles, layering, conventions);
- `docs/vision.md`;
- ADRs in `docs/decisions/`;
- `.claude/lessons.md`.

It changes only through a new ADR or a lesson, with the user's consent.
Every phase checks against it.

## 1. Pick the track

| Track | When | Size (skill `ferrum-size`) |
|---|---|---|
| **Stage** | a new stage, architecture, a contract surface (agent command set, engine protocol, workspace format, shaders), or several features | L or XL; split into PRs |
| **Feature** | one command or option, one UI tool, a bug with a known cause, a bridge change within the protocol | M (one PR) |
| **Small** | docs, metadata, an obvious one-place fix, a release | S |

When unsure, size first (`ferrum-size`) and let the size decide. A
"feature" that crosses two contract surfaces is a stage.

## 2. Pick the mode

| Mode | Tracks | Gates |
|---|---|---|
| **Gated** (default for stages) | stage, feature | G1 spec, G2 design (stage only), G3 merge |
| **Autonomous** (when the user says so, or for a feature they handed over) | feature, small | G3 merge only |

**Autonomous means no questions until the PR is open and green.**
- Ambiguities are resolved by the safest reasonable assumption. Each
  assumption is written into the spec's *Clarifications* as `assumed:`
  and repeated in the PR body under *Assumptions*.
- **Hard stops, even in autonomous mode.** Stop and ask when the work
  would:
  - violate or amend the constitution;
  - change a contract surface (then it is a stage);
  - touch patient data, privacy, licences or the `research_only`
    handling;
  - weaken a safety rule (agents propose, people confirm);
  - grow beyond size M during implementation.
- **Merging always waits for the user's word (G3).** So do pushes to
  `develop` or `main`, and releases.

## 3. Phases

| # | Phase | Skill | Artifact | Stage | Feature | Small |
|---|---|---|---|---|---|---|
| 1 | Specify | `ferrum-specify` | stage section in `dev_plan.md`, criteria with IDs | ✓ | ✓ (short) | — |
| 2 | Clarify | `ferrum-specify` | *Clarifications* in the spec | ✓ → **G1** | ✓ (gated: **G1**; autonomous: `assumed:`) | — |
| 3 | Plan | `ferrum-plan` | `docs/<topic>.md` and ADRs | ✓ | ✓ (a section in the PR body, or in the design doc of its stage) | — |
| 4 | Size | `ferrum-size` | size and PR split in the plan | ✓ → **G2** | ✓ (must be M) | — |
| 5 | Tasks | `ferrum-plan` | task table N.1…N.k | ✓ | ✓ (checklist in the PR) | — |
| 6 | Analyze | `ferrum-analyze` | coverage report (`analyze.py`) | ✓ | ✓ | — |
| 7 | Implement | `ferrum-change` | commits on a feature branch | ✓ | ✓ | ✓ |
| 8 | Verify | `ferrum-verify` | verification table in the PR | ✓ | ✓ | preflight only |
| 9 | Deliver | `ferrum-change` §7 | PR, CI green → **G3** | ✓ | ✓ | ✓ |
| 10 | Retro | `ferrum-lessons`, `ferrum-size` §5 | lessons, sizing calibration | ✓ | ✓ when something happened | — |

Release is separate (`ferrum-release`), on the user's decision.

## 4. How a request flows

**Gated stage:**
1. Specify and clarify, then present the spec in chat → G1.
2. Plan, size and tasks, then present the design and the PR split →
   G2.
3. Per PR of the split: analyze, implement, verify, deliver → G3.
4. Retro after the stage.

**Gated feature:** one message with the spec (criteria with IDs),
questions, a plan sketch, size M and tasks → G1. Then analyze,
implement, verify, deliver → G3.

**Autonomous feature:**
1. Specify, writing `assumed:` instead of questions.
2. Plan, size (must be M), tasks, analyze, implement, verify.
3. Deliver, then report with the PR link, assumptions and the
   verification table → G3.

Retro follows if something happened.

**Small:** implement, verify (preflight), deliver → G3.

## 5. Status reports

At each gate, and at the end of autonomous work, report:
- phase reached;
- artifacts (links);
- acceptance criteria covered and how (test, visual, field);
- what has *not* been verified;
- open assumptions;
- what the user needs to decide.

Never present an unverified criterion as done.
