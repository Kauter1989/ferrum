---
name: ferrum-plan
description: Phases 3 and 5 of the FERRUM workflow — turn an approved spec into a design (docs/<topic>.md and ADRs) with a constitution check, contracts, risks and a validation plan that maps every acceptance criterion to how it is verified, then break it into tasks. Use after the spec passed gate G1 (or, in autonomous mode, after specify), before sizing and code.
---

# Plan and tasks

The design says **how**. It lives in `docs/<topic>.md`, for example
`docs/agent-segmentation.md`. Decisions get new ADRs in
`docs/decisions/`; take the next free number on a fresh base (L3).

For a **feature**, the same sections can be short. They go in the PR
body or in a section of the stage's existing design doc.

## 1. Design document sections

1. **Context:** a link to the stage in `dev_plan.md`, and what exists
   today (with file paths).
2. **Constitution check.** Answer each point; a "no" is a hard stop that
   needs an ADR and the user:
   - layering: `ferrum` → `ferrum-app` → `ferrum-domain` ← `ferrum-io`;
     `ferrum-app` without wgpu/egui; `ferrum-agent` without the `gpu`
     feature;
   - ports: files only in `ferrum-io`, engines only in
     `ferrum-engines`;
   - no inference in Rust; engines out of process;
   - agents and engines propose, people confirm; values come from
     voxels;
   - no identifiers leave FERRUM; no patient or volume data in the
     repo;
   - the rendering rule: GPU and CPU paths mirrored, with parity tests.
3. **Design:** components, data flow, and the use cases in `ferrum-app`.
   Use a diagram where it helps.
4. **Contracts:**
   - JSON Schemas of commands (`schema.rs`);
   - `ferrum-engine/1` messages;
   - workspace files and their versions;
   - UI changes.

   For each, name the "also update" rows from `ferrum-change` §2.
5. **Risks and assumptions:** what may not work, what is estimated and
   not measured (L13), and the fallbacks.
6. **Validation plan:** a table with one row per acceptance criterion:

   | Criterion | Verified by | Level |
   |---|---|---|
   | [18.1-a] | `segmentation.rs::outline_is_closed_at_any_zoom` (planned) | test |
   | [18.1-b] | `phantom_render.py` + look at PNGs | visual |
   | [18.3-a] | Dice on MSD Task09 spleen, `scripts/benchmark_engines.py` | field |

   - **Levels:**
     - `test` (CI);
     - `visual` (`ferrum-visual-check`);
     - `field` (`ferrum-field-test`, the user's machine);
     - `manual` (a person in the desktop app; name what they check).
   - Every criterion has at least one row. A visible criterion has a
     `test` row with exact geometry **and** a `visual` row (L1).
7. **Size and PR split:** from `ferrum-size`.
8. **Tasks:** the table below.

Gated stage: present the design, size and split in chat. The user's
approval is **gate G2**.

## 2. Tasks

```markdown
| Task | Criteria | Tests first | Touches (matrix rows) | Depends on | PR |
|---|---|---|---|---|---|
| N.1 Refinement from stored prompts | N.1-a, N.1-b | contract test replay CLI vs MCP | agent command, workspace format | — | 1 |
```

- **A task is a vertical slice** that can be merged on its own. It
  closes named criteria and starts with its test.
- **"Touches" uses the matrix rows of `ferrum-change` §2,** so analyze
  can check the follow-up work: schemas, docs, parity, contract tests.
- **Order:** domain → io/engines → app → agent/CLI → UI → docs. Safety
  tests (`forbidden`, `limit`) go in the first task that mutates
  anything (L7).
- **Copy the table** into the stage's "Implementation" section and keep
  the state column (✅) current as PRs merge. Stage 17 is the example.
