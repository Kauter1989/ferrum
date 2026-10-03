---
name: ferrum-specify
description: Phases 1–2 of the FERRUM workflow — write the specification of a stage or feature (what and why, user stories, acceptance criteria with IDs, success metrics, out of scope) in dev_plan.md, check the quality of every criterion, and clarify open points with the user (or record assumptions in autonomous mode). Use after ferrum-workflow picked the track, before any design or code.
---

# Specify and clarify

The spec says **what** and **why**, never how. It lives in `dev_plan.md`
as the stage's section (artifact choice "a": no extra folders).

## 1. Start on a fresh base (L3)

```bash
git fetch origin develop
git checkout -b <prefix>/<topic> origin/develop
```

Read what already exists:
- `dev_plan.md`;
- `CHANGELOG.md` `[Unreleased]`;
- the docs of the area;
- the code on this base.

The spec must not describe as new what already exists.

## 2. Write the section

```markdown
## Stage N — <title>

<Goal and context: who needs it, what problem, what exists today. 3–6 sentences.>

| # | User story | Acceptance criteria |
|---|---|---|
| N.1 | As a **<role>**, I want <capability> so that <benefit>. | [N.1-a] <criterion>; [N.1-b] <criterion>; … |

**Success metrics:** <numbers measured on real data or a benchmark, e.g. Dice ≥ 0.90 on MSD Task09 spleen; ≤ 2 s per prompt on a 12 GB GPU>.
**Out of scope:** <what this stage does not do>.

**Clarifications:**
- Q: <question> — A: <answer from the user> (date)
- assumed: <assumption made in autonomous mode, and why it is the safe choice>
```

The rules:
- **Roles:** clinician, reviewer, operator, agent developer, harness
  developer, annotator.
- **IDs:** `[N.k-x]` with a letter per criterion. They are permanent.
  Tests cite them (`// covers 18.2-b`), and `ferrum-analyze` checks the
  coverage.
- **When a criterion is done,** add the ✅ note after it, as earlier
  stages do. Never delete a criterion; strike it through (`~~…~~`) with
  a reason.
- **Features** inside an existing stage get a new row there (`N.k`).
  Small fixes get no spec.

## 3. Check every criterion (Spec Kit's "checklist")

Go through each criterion. Rewrite it until every applicable answer is
yes.

| Question | Why (lesson) |
|---|---|
| Is it measurable by a test or a field run, with an exact expected value? | W2 |
| For anything visible: is the invariant stated (closed on all sides, at 1:1 and zoomed, visible on dark and bright, orientation)? | L1 |
| Are the negative paths specified (`forbidden`, `limit`, empty result, engine unavailable)? | L7, L14 |
| Are system-made and user-given inputs distinguished (limits, undo)? | L6 |
| Does it hold for NIfTI and DICOM, anisotropic voxels, a dimension of 1? | L4, L10 |
| Does it say what a person must confirm (agents propose)? | constitution |
| Does it keep identifiers in FERRUM (privacy)? | constitution |
| If it names hardware or performance, is the number measured or marked as an estimate? | L13 |

A criterion such as "segments are drawn with outlines" fails this check.
"[15.3-c] the outline of a segment is closed on all four sides and 1–3
px wide at 1:1 and 8×" passes.

## 4. Clarify

- Mark every open point in the draft with `[?]`.
- **Gated mode:** ask all `[?]` questions in one message. Include the
  draft section and your recommended answer for each question. Record
  the answers under *Clarifications*. The user's approval of the spec is
  **gate G1**.
- **Autonomous mode:** resolve each `[?]` with the safest reasonable
  choice and record it as `assumed:`.
  - Stop and ask anyway on the hard stops of `ferrum-workflow` §2.
- Never guess facts that the user can state: hardware, datasets,
  licences, who uses the feature (L13).

## 5. Hand over

The spec and its clarifications are committed on the branch with the
design (`ferrum-plan`). For a feature they are committed with the code.
