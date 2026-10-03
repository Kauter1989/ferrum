---
name: ferrum-release
description: Release FERRUM — version bump, CHANGELOG, develop → main and the GitHub release with archives. Use when the user asks to release, to merge develop into main, or to publish a version/tag. Follows the repository's release history (Release X.Y.Z (#N) merge commits on main, release.yml dispatched on main).
---

# Releasing FERRUM

This is how 0.2.x and 0.3.0 were released (W4 in `.claude/lessons.md`).
Every step that changes `develop`, `main` or the public releases needs
the user's go-ahead. A request such as "merge into main" or "release
it" counts as one for the whole sequence.

## 1. Choose the version (semver, 0.x)

| What changed since the last tag | Bump |
|---|---|
| New features (commands, stages, UI tools) | minor: 0.3.0 → 0.4.0 |
| Only fixes, docs, metadata | patch: 0.3.0 → 0.3.1 |

Read `CHANGELOG.md` `[Unreleased]` and `git log origin/main..origin/develop`
to decide. If unsure, propose a version to the user.

## 2. Bump PR into `develop`

```bash
git fetch origin develop main
git checkout -b chore/release-X.Y.Z origin/develop
git grep -n "<old version>" -- ':!Cargo.lock' ':!CHANGELOG.md'
```

Replace the old version in:
- `Cargo.toml` (workspace `version`), then let `cargo check` update
  `Cargo.lock`;
- `skills/plugin/plugin.json`;
- `CITATION.cff`: `version` and `date-released`;
- the format examples: `docs/agent-cli.md`, `docs/workspace-format.md`,
  `skills/ferrum/reference/outputs.md`.

Then update `CHANGELOG.md`:
- add a heading `## [X.Y.Z] — YYYY-MM-DD` below an empty
  `## [Unreleased]`;
- add a compare link `[X.Y.Z]: …/compare/vOLD...vX.Y.Z` at the bottom.

Check, commit `chore: release X.Y.Z`, push, and open a PR into
`develop`:
- check with `.claude/skills/ferrum-change/preflight.sh --quick` plus
  `cargo test -p ferrum-cli -p ferrum-agent` (version strings appear in
  outputs);
- the PR title is "Release X.Y.Z: version bump".

When CI is green, merge it with a merge commit.

## 3. PR `develop` → `main`

- **Title:** "Release X.Y.Z".
- **Body:** the user-visible changes grouped as in the CHANGELOG, and
  the PRs it contains.
- **Merge:** when CI on `develop`'s head is green, merge with a merge
  commit titled `Release X.Y.Z (#N)`, as in the history of `main`.

## 4. Publish

Dispatch `.github/workflows/release.yml` on `main` with the input
`tag: vX.Y.Z`:
- the workflow creates the tag on `main`'s head;
- it builds the viewer, `ferrum-cli`, the skill package and the plugin
  folder;
- it publishes the GitHub release, and Zenodo archives it from there;
- an empty `tag` is a dry run: the archives are built but nothing is
  published.

Follow the run to the end (it takes 4–15 minutes). Then confirm:
- the release `vX.Y.Z` exists and has its archives;
- the tag points at the `Release X.Y.Z (#N)` commit.

## 5. Afterwards

- Merged PRs are finished. New work starts on a fresh branch from
  `develop`.
- If anything in the sequence went wrong, record it with skill
  `ferrum-lessons` (for example, Zenodo metadata in 0.2.3).
