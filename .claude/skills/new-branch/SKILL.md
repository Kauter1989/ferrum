---
name: new-branch
description: Create a git-flow branch with a standard name (feat/, fix/, docs/, refactor/, chore/, test/, perf/, hotfix/, release/) before starting any change. Use at the start of every feature, fix, docs or refactor task, and whenever work on a different topic begins, so each topic gets its own branch.
---

# Create a git-flow branch

FERRUM follows git flow (CLAUDE.md, CONTRIBUTING.md): `develop` is the
integration branch, work happens on short-lived topic branches that are
merged back into `develop` by pull request. The delivery flow after the
branch exists is in the `ferrum-change` skill.

## When to use

- At the start of every task that changes files, **before the first edit**.
- When the work moves to a **different topic** (a bug found while building a
  feature, a docs update, a refactor): create a separate branch for it
  instead of stacking unrelated commits. Prefer many small branches, one
  topic each.
- If the session names a branch to develop on, use it; check that it was
  cut from `develop` (`git merge-base --is-ancestor origin/develop HEAD`)
  and rebase it onto `develop` if not.

## Name

`<type>/<short-kebab-description>`

| Type | Use for |
|------|---------|
| `feat/` | new behaviour for users or API |
| `fix/` | bug fixes |
| `docs/` | documentation only |
| `refactor/` | no behaviour change |
| `perf/` | speed or memory |
| `test/` | tests only |
| `chore/` | tooling, CI, dependencies, releases of metadata |
| `hotfix/` | urgent fix of a released version |
| `release/` | release preparation, e.g. `release/0.4.0` |

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
3. **Every branch starts from a fresh `origin/develop`**, whatever its type
   (hotfixes and releases included). Never branch from `main`, from another
   topic branch, or from a stale local `develop`; never commit on `develop`
   or `main`.
   ```bash
   git fetch origin develop
   git switch -c <type>/<description> origin/develop
   ```
   If a shallow or single-branch clone has no `origin/develop`, fetch it
   first (`git fetch origin develop`) instead of falling back to `main`.
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
