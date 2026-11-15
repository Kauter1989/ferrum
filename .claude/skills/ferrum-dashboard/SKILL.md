---
name: ferrum-dashboard
description: Generate and publish the FERRUM development dashboard (Stage 30) — a self-contained page of the plan's implementation state and the crate architecture, synchronised with dev_plan.md and coloured by lifecycle (released / develop / progress / planned). Use during deliver to snapshot the plan state for the G3 report, after a merge (phase 11) and after a release to publish to gh-pages, when hook H notes the dashboard is stale, or when the user asks to update or publish the dashboard. Generation is local (seconds, no CI minutes); publication pushes the gh-pages branch that GitHub Pages serves.
---

# Development dashboard

The dashboard is a **derived view**: every number comes from git and the
repository, never from hand entry. `dev_plan.md` stays the single source
of truth; the dashboard cannot drift from it by construction. Design:
[docs/dashboard.md](../../../docs/dashboard.md).

## 1. When

| Moment | Action |
|---|---|
| Deliver (`ferrum-change` §7), before the G3 report | generate from the branch HEAD; put the plan-state summary into the G3 report |
| After a merge (phase 11, `ferrum-report`) | generate from `origin/develop`; publish |
| After a release (`ferrum-release` §4) | generate from `origin/develop` (the new tag is on `main`); publish — the lifecycle colours shift |
| Hook H notes staleness, or the user asks | generate from `origin/develop`; publish |

## 2. Generate

```bash
python scripts/dashboard/generate.py --ref HEAD --out target/dashboard-site
```

Writes `data.json` (dashboard/1) and the self-contained `index.html`.
For a G3 snapshot use the branch HEAD; for publication use
`--ref origin/develop` after `git fetch origin`. The generator reads
committed state only and is deterministic (git-derived stamp, no wall
clock); unknown plan shapes warn and mark `unknown`, never default
silently (L37).

## 3. Verify before claiming it works

- `python .claude/skills/ferrum-analyze/analyze.py --stage 30` exits 0.
- `python -m pytest tests/dashboard -q` green.
- The page: no external subresources, light and dark themes, four
  lifecycle colours each with a text badge, footer naming the release
  tag, the target SHA and the generation date. **Open it in a browser and
  look** (the manual rows of docs/dashboard.md §6); a page claimed
  verified but not opened is a false claim (L33).
- No machine paths in the output — the test scans for them; the page
  quotes plan text and repo-relative paths only.

## 4. Publish (no CI minutes)

```bash
git fetch origin
python scripts/dashboard/publish.py --ref origin/develop
```

`publish.py` generates the site and pushes it to `gh-pages` — an orphan
branch holding only the site plus `.nojekyll` — through a throw-away
detached worktree, so the caller's tree and branch are never touched.
GitHub Pages serves `gh-pages` from the branch; no Actions minutes are
spent (the 10 builds/hour soft limit is ample for a few publications a
day). First time only: Settings → Pages → Source: Deploy from a branch →
`gh-pages` / `(root)`.

The push is exempt from the local cargo gate (hook E): CI never runs on
`gh-pages`, so the gate would judge a push CI does not see. The branch
holds only generated text; a force-push keeps it a pure publication
target. Never publish patient data or machine paths — the site holds
only plan and code metadata.

## 5. What it shows

- **Plan**: a card per stage — lifecycle colour + the plan's own heading
  badge, a segmented criterion-count bar, stories and expandable criteria
  with test/PR links pinned to the target SHA. Filters (lifecycle, status)
  and search narrow the view.
- **Architecture**: the crate table and an interactive dependency graph;
  crates changed since the release are marked; every layering violation
  is named (dependencies point inward, the per-crate prohibitions).
- **Roadmap**, the committed feature reports, warnings, and a sources
  footer (tag, SHAs, commands).

## 6. Rules

- The dashboard decides nothing: merging stays G3, publication is a push
  the skill performs after the merge or release.
- Regenerate, never hand-edit `data.json` or the published site.
- A stale dashboard is a defect of discipline, not of the tool: hook H
  reminds at session start; publish after every merge and every release.
- Colour is the lifecycle; the badge is the plan's status; coverage is
  the test/field evidence. Three axes, never collapsed into one.
