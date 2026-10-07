---
name: new-branch
description: Create a git-flow branch with a standard name (feat/, fix/, docs/, refactor/, chore/, test/, perf/, hotfix/, release/) before starting any change. Use at the start of every feature, fix, docs or refactor task, and whenever work on a different topic begins, so each topic gets its own branch.
---

# Create a git-flow branch

FERRUM follows git flow (CLAUDE.md, CONTRIBUTING.md): `develop` is the
integration branch, work happens on short-lived topic branches that are
merged back by pull request.

## When to use

- At the start of every task that changes files, **before the first edit**.
- When the work moves to a **different topic** (a bug found while building a
  feature, a docs update, a refactor): create a separate branch for it
  instead of stacking unrelated commits. Prefer many small branches, one
  topic each.
- If the session or the user names a branch to develop on, use that name
  and still rename it with this skill's convention only when asked.

## Name

`<type>/<short-kebab-description>`

| Type | Use for | Base |
|------|---------|------|
| `feat/` | new behaviour for users or API | `develop` |
| `fix/` | bug fixes | `develop` |
| `docs/` | documentation only | `develop` |
| `refactor/` | no behaviour change | `develop` |
| `perf/` | speed or memory | `develop` |
| `test/` | tests only | `develop` |
| `chore/` | tooling, CI, dependencies, releases of metadata | `develop` |
| `hotfix/` | urgent fix of a released version | `main` |
| `release/` | release preparation, e.g. `release/0.4.0` | `develop` |

Rules for the description: lowercase ASCII, words joined by `-`, 2–5 words,
what the change is (not who or when), no issue-tracker noise
(`feat/robust-region-growing`, `fix/slice-scroll-off-by-one`). Add the issue
number as a prefix when one exists (`fix/123-slice-scroll`). Only
`feat/ fix/ docs/ refactor/ perf/ test/ chore/ hotfix/ release/` are valid
prefixes.

## Steps

1. Check the tree: `git status --short`. Uncommitted work belonging to
   another topic goes into its own commit or stash first; never carry it
   over silently.
2. Pick the type from the table and write the description.
3. Find the base: `develop` if it exists locally or on `origin`, otherwise
   the default branch (`main`); `main` for `hotfix/`.
   ```bash
   git fetch origin
   base=develop; git rev-parse --verify -q origin/develop >/dev/null || base=main
   git switch -c <type>/<description> "origin/$base"
   ```
4. Commit in small steps with clear messages; push with
   `git push -u origin <branch>`.
5. Do **not** open a pull request unless asked.

## Renaming a branch

```bash
git branch -m <old> <new>
git push -u origin <new>
git push origin --delete <old>   # only if the user asked for the rename
```
Check first that no open pull request uses the old name (a rename closes
it); tell the user when one exists.
