# Development dashboard

Design for [Stage 30](../dev_plan.md) — a self-contained, interactive
dashboard of the plan's implementation state and the crate architecture,
synchronised with `dev_plan.md` and published on GitHub Pages without
spending CI minutes.

## 1. Context

What exists today:

- `dev_plan.md` — the single source of truth: stages with ✅/🚧/📋
  headings, stories `N.k`, criteria `[N.k-x]` with inline ✅ and PR
  references, an Implementation table per stage, a Roadmap with drafts.
- `.claude/skills/ferrum-analyze/analyze.py` — mechanical criterion
  coverage: `covers N.k-x` citations in code, `visual`/`field`/`manual`
  rows in docs validation tables.
- `docs/architecture.md` — the mermaid crate graph and the responsibility
  table; `CLAUDE.md` — the declared layering.
- `docs/<feature>/*-report.html` — per-PR reports on one self-contained
  template (palette, light/dark, inline SVG, code map pinned to a SHA).
- `.github/workflows/ci.yml` — the `python-lint` job (ruff + hook tests).

Nothing aggregates these into one view, and nothing distinguishes what
the latest release holds from what `develop` holds unreleased. The
dashboard is that aggregate — a **derived view**: every number comes from
git and the plan, never from hand entry (the `ferrum-report` §3 discipline).

## 2. Constitution check

| Point | Answer |
|---|---|
| Layering (`ferrum` → `ferrum-app` → `ferrum-domain` ← `ferrum-io`, …) | no crate is changed; the tooling lives in `scripts/`, `tests/`, `.claude/` — outside the crate graph |
| Ports: files only in `ferrum-io`, engines only in `ferrum-engines` | the generator parses no volume files and talks to no engine; it reads repo metadata (plan, git, Cargo.toml) |
| No inference in Rust; engines out of process | not touched |
| Agents and engines propose, people confirm | the dashboard reports; it decides nothing. Merging stays G3, publication is a push the skill performs after the merge |
| No identifiers leave FERRUM; no patient/volume data | the dashboard holds only plan text, crate names and repo-relative paths; a test scans the output for machine paths (30.1-e); no volume data exists in its inputs |
| Rendering rule (GPU/CPU mirrored) | no image formation; the page is HTML, not a render |

No ADR is needed: the architecture does not change. The workflow
integration (skill, hook exceptions, CLAUDE.md table row) was approved by
the user in chat on 2026-11-15 and lands in PR 2.

## 3. Design

### 3.1 Components

```
scripts/dashboard/generate.py     the data engine (Python stdlib + git + cargo)
scripts/dashboard/page.py         PR 2: the renderer (aggregate → HTML fragments)
scripts/dashboard/template.html   PR 2: the page skeleton (inline CSS, placeholders)
tests/dashboard/                  pytest fixtures and structural tests
site output (default target/dashboard-site/):
    data.json                     dashboard/1 — the machine-readable aggregate
    index.html                    PR 2: the self-contained page
.claude/skills/ferrum-dashboard/  PR 3: when and how the workflow runs it
gh-pages branch                   PR 3: the published site (Pages from branch)
```

Data flow (one local run, seconds, no CI):

```
git describe --tags --abbrev=0 origin/main ─→ release tag
git show <tag>:dev_plan.md ────────────────→ plan at release
git show <ref>:dev_plan.md ────────────────→ plan at target (HEAD / origin/develop)
parse both ─→ done@tag, done@target ───────→ lifecycle buckets (§3.3)
git grep covers-citations at <ref> ────────→ coverage per criterion (analyze convention)
git log --grep PR merges + merge-base ─────→ PR-ancestry refinement (§3.3)
cargo metadata --no-deps (or Cargo.toml) ──→ crate nodes, edges, descriptions
git diff --name-only <tag>..<ref> crates/ ─→ changed-since-release marks
layering constant (CLAUDE.md) ─────────────→ named violations
git ls-tree docs/*-report.html ────────────→ report index
→ data.json (sorted, git-derived stamp); PR 2: + template → index.html
```

CLI:

```bash
python scripts/dashboard/generate.py [--root .] [--ref HEAD] \
    [--release-ref origin/main] [--out target/dashboard-site]
```

`--ref` is any committed revision (the feature branch for the G3
snapshot, `origin/develop` for publication). The generator reads
**committed state only** — never the working tree — so two runs on the
same inputs are byte-identical (30.1-e). The wall clock is not an input:
`meta.generated_at` is the target commit's date.

### 3.2 Contract: `dashboard/1` (data.json)

Internal contract (not an agent/engine/workspace surface). Versioned by
the `schema` field; a breaking change bumps it and updates this section.

```json
{
  "schema": "dashboard/1",
  "meta": {
    "generated_at": "2026-11-15T22:54:09+03:00",
    "release_tag": "v0.4.0", "release_sha": "…",
    "target_ref": "origin/develop", "target_sha": "…",
    "repo_url": "https://github.com/Kauter1989/ferrum",
    "warnings": ["stage 24: heading 🚧 lags criterion 24.4-a (PR #109 merged)"]
  },
  "stages": [{
    "id": "23", "title": "GPU sharing …", "heading_status": "done",
    "bucket": "develop",
    "counts": {"released": 4, "develop": 3, "progress": 0, "planned": 0, "unknown": 0},
    "stories": [{
      "id": "23.1", "text": "As a …",
      "criteria": [{
        "id": "23.1-a", "text": "…", "done": true, "prs": [111],
        "bucket": "released",
        "tests": ["crates/ferrum-gpu-lock/tests/leases.rs"],
        "levels": ["field"]
      }]
    }]
  }],
  "roadmap": {"next": [{"n": 1, "what": "…", "kind": "…", "why": "…"}],
               "backlog": ["…"], "drafts": [{"id": "21", "title": "…", "stories": ["…"]}]},
  "crates": [{"name": "ferrum-domain", "description": "…",
               "deps": [], "changed_since_release": false, "violations": []}],
  "layering_violations": [{"from": "…", "to": "…", "rule": "…"}],
  "reports": [{"path": "docs/webgpu-panel/pr-91-report.html", "label": "pr-91"}]
}
```

Story-level units (early stages without criterion IDs) carry
`"criteria": []` and a story-level `bucket`/`done` instead; the stage
counts then count stories. Links are not stored: the renderer builds
them from `meta.repo_url` and `meta.target_sha`. `index.html` renders the
data into the HTML with `data-*` attributes (`data-stage`, `data-bucket`,
`data-crate`, `data-changed`) — PR 3's interactivity reads the DOM, so the
page carries no duplicated JSON payload; `data.json` is written alongside
for machine consumers.

### 3.3 Lifecycle rules (exact)

- `done(item, rev)` = inline ✅ on the criterion, OR the stage heading at
  `rev` is ✅ (and the item is not struck through), OR a PR cited on the
  item has its merge commit (`Merge pull request #N ` or `(#N)`) an
  ancestor of `rev` (`git merge-base --is-ancestor`).
- Criterion bucket: `released` if done at the release tag; else `develop`
  if done at the target; else `progress` if the target heading is 🚧;
  else `planned` if 📋; else `unknown` (with a warning).
- Stage bucket priority over its items:
  `unknown` > `progress` > `develop` > `released` > `planned`;
  `counts` holds the exact per-bucket item counts (the segmented bar).
- A heading that lags its criteria (🚧 while every criterion is done)
  produces a `meta.warnings` entry — the dashboard surfaces plan hygiene,
  it does not silently rewrite the plan.
- No release tag reachable → `release_tag: null`, every done item is
  `develop`, one warning. The page still renders.

### 3.4 Layering model

The constitution's rule is "dependencies point inward" plus specific
prohibitions — not the mermaid's drawn edges as a closed whitelist (the
first real run proved it: 10 legitimate inward edges, e.g.
`ferrum-app → ferrum-radiomics`, which CLAUDE.md itself names, would be
flagged). The generator encodes:

- `TIERS` (inner → outer): domain 0; gpu-lock 1; processing/render/io/
  engines/radiomics 2; app 3; ui/view/agent 4; ferrum/cli/wasm 5. An edge
  is allowed when it points strictly inward, or is a documented same-tier
  pair (`SAME_TIER_ALLOWED`: engines → gpu-lock, render → processing).
- `ferrum-app` must not depend on wgpu/egui (`APP_FORBIDDEN`).
- `ferrum-gpu-lock` depends on no other FERRUM crate.
- A crate missing from `TIERS` produces a warning, not a silent pass.

Every violating edge is named in `crates[].violations` and
`layering_violations`. The model is reviewed against CLAUDE.md in the
PR's code map; tests pin an outward edge and a forbidden dependency.

### 3.5 Page (PR 2)

One skeleton (`template.html`), the report palette (`--ok`/`--accent`/
`--warn`/`--muted`, light + dark via `prefers-color-scheme`, L35), sections:

1. header: title, meta pills (release tag, target SHA, date), warning count;
2. plan: one `<details>` card per stage — summary holds the number,
   title, heading badge, lifecycle badge and the segmented bar; inside,
   the stories and their criteria (text, badge, coverage links pinned to
   the target SHA, PR links);
3. architecture: the crate table (description, deps, changed mark,
   violations) — the interactive SVG graph is PR 2 (30.3);
4. roadmap: Next table, backlog, draft stages;
5. reports: links to the committed HTML reports (GitHub blob, pinned SHA);
6. footer: sources (tag, SHAs, commands), warnings in full, schema id.

All plan text is HTML-escaped. Native `<details>` gives expand/collapse
without JS. Every colour carries a text badge (30.2-b). PR 3 adds the
filter bar, search and the SVG graph into the `<!--INTERACTIVE-->`
placeholder, reading the DOM's `data-*` attributes.

## 4. Contracts and "also update" rows

| Surface | Change | Also update |
|---|---|---|
| `dashboard/1` (new, internal) | §3.2 | this doc; the page; the tests |
| CI `python-lint` | pytest step for `tests/dashboard` | `ci.yml` (30.1-g) |
| workflow (PR 3) | skill `ferrum-dashboard`; hooks J/E exception for `gh-pages`, G matrix row, H freshness | `CLAUDE.md` skill table, `ferrum-change` §2 matrix, `ferrum-release` checklist, `test_hooks.py` |
| user-visible | new tool | `CHANGELOG.md` `[Unreleased]` |

No agent command, engine protocol, workspace format, shader or desktop UI
surface is touched.

## 5. Risks and assumptions

- **Markdown parsing is the main risk** (the plan is prose). Mitigation:
  exact-fixture tests for every shape the real plan holds (ID-less early
  stages, letter stages, struck criteria, drafts, lagging headings,
  Implementation tables); unknown shapes warn and mark `unknown`, never
  default silently (L37). The first run on the real plan found two
  defects, both fixed and pinned by tests: the mermaid read as a closed
  whitelist flagged 10 legitimate inward edges (§3.4), and Stage 26's
  Implementation row 26.4 parsed as a story (header recognition).
- **Concurrent plan edits**: Stage 28 work edits `dev_plan.md` in another
  session. Stage 30 is appended before the Roadmap — a separate region;
  at merge, check the CHANGELOG placement too (L43).
- **No browser for the agent**: the visible criteria get structural tests
  (exact HTML assertions) plus `manual` rows the user runs (open the
  page, both themes). The PR states plainly that the agent did not open
  a browser (ferrum-verify honesty).
- **Preflight platform**: WSL Ubuntu-24.04 (L45), bash via an LF script
  file; the Windows host answers the GPU half if needed.
- **Pages from a branch**: 10 builds/hour soft limit — ample for a few
  publications a day; `.nojekyll` serves the site as-is.
- Estimates marked as such: generation ≤ 5 s and page ≤ 300 KB are
  budgets, measured before the PR (L13).

## 6. Validation plan

| Criterion | Verified by | Level |
|---|---|---|
| [30.1-a] | `tests/dashboard/test_generate.py::test_plan_parses_to_the_exact_fixture`, `…struck_criteria…`, `…letter_and_draft_stages…`, `…idless_stages…`, `…implementation_tables_are_not_stories…` (planned) | test |
| [30.1-b] | `test_lifecycle_buckets_on_a_fixture_repository`, `test_lagging_heading_criterion_is_develop`, `test_stage_bucket_priority_and_counts` (planned) | test |
| [30.1-c] | `test_coverage_follows_the_analyze_convention` (planned) | test |
| [30.1-d] | `test_crate_graph_and_layering_violations`, `test_fallback_parses_the_workspace_toml`, `test_changed_crates_are_marked` (planned) | test |
| [30.1-e] | `test_output_is_byte_identical_across_runs`, `test_output_holds_no_machine_paths` (planned) | test |
| [30.1-f] | `test_unknown_marker_warns_and_marks_unknown`, `test_empty_criteria_cell_warns` (planned) | test |
| [30.1-g] | `test_ci_runs_the_dashboard_tests` (planned); `ci.yml` python-lint step runs `tests/dashboard`; CI green on the PR; `analyze.py --stage 30` exit 0 locally | test |
| [30.2-a] | `tests/dashboard/test_page.py::test_page_is_self_contained` (PR 2) | test |
| [30.2-a] | open `index.html` from `file://` in a browser: page renders, network panel empty | manual |
| [30.2-b] | `test_four_colours_and_text_badges_both_themes` (PR 2) | test |
| [30.2-b] | emulate light and dark: chips legible, badges present | manual |
| [30.2-c] | `test_stage_cards_counts_links_and_footer`, `test_page_carries_data_attributes`, `test_plan_text_is_html_escaped`, `test_warnings_are_shown` (PR 2) | test |
| [30.2-c] | expand a stage, follow a criterion's test link (pinned SHA) | manual |
| [30.2-d] | `test_crate_table_marks_and_violations` (PR 2) | test |
| [30.2-d] | read the crate table: changed marks and violations visible | manual |

## 7. Size and PR split (ferrum-size)

| PR | Tasks | Est. lines (code/tests/docs) | Files | Surfaces | Class | Field test after |
|---|---|---|---|---|---|---|
| 1 | 30.T1, T3 | 590/540/330 | 7 | dashboard/1 (internal) | M | no (no engines/rendering/formats) |
| 2 | 30.T2 | 280/110/40 | 5 | — | S | no |
| 3 | 30.T4–T5 | 700/250/200 | ~12 | — (workflow) | M | no |

Autonomous mode allows M per PR. The whole feature measured 1844 lines
(L), so it was split at the module boundary — data engine first (the
dashboard/1 contract and its producer), the page consumer next, the
workflow integration last (L36; ferrum-size §3). Re-measure with
`git diff --shortstat origin/develop...HEAD` before each push.

## 8. Tasks

See the Implementation table in `dev_plan.md` (Stage 30). Order:
30.T1 (generator, tests first) → 30.T3 (CI step) in PR 1; 30.T2 (page)
in PR 2; 30.T4 → 30.T5 in PR 3.
