---
name: new-branch
description: Name and create a git-flow topic branch from a fresh origin/develop (feat/, fix/, docs/, refactor/, chore/, test/, perf/). Used by ferrum-change §1; use it alone only to rename or split a branch. The workflow itself starts with ferrum-workflow.
---

# New branch

This skill only names and cuts the branch. **Do not start a task here:**
every task starts with `ferrum-workflow` (track, mode, phases), and the
delivery steps are in `ferrum-change`.

- **Every branch starts from a fresh `origin/develop`**, of any type
  (L3). Never from `main`, another topic branch or a stale local
  `develop`; never commit on `develop` or `main` (L2).
  ```bash
  git fetch origin develop
  git switch -c <type>/<description> origin/develop
  ```
- **Name:** `<type>/<2–5 lowercase words joined by ->`, what the change is
  (`feat/robust-region-growing`, `fix/slice-scroll-off-by-one`); issue
  number first when there is one. Types: `feat` `fix` `docs` `refactor`
  `perf` `test` `chore` (and `release/<x.y.z>` for `ferrum-release`).
- **One topic per branch.** Work on another topic starts another branch;
  unrelated changes in the tree are committed or stashed first.
- **A branch the harness named** (`claude/…`) is renamed
  (`git branch -m`) and rebased onto `origin/develop` before the first
  commit (L17). Renaming the remote branch needs `git push origin
  --delete <old>` — ask the user if the push is refused.
- Pushing a topic branch is fine; opening a PR is `ferrum-change` §7.
