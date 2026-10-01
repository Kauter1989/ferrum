# ADR 0006 — Releases can be cut from the Actions tab

- **Status:** Accepted
- **Date:** 2026-10-01

## Context

ADR 0003 publishes a release when a `v*` tag is pushed. Some environments
used to work on the repository (for example automated agents behind a git
proxy) may push branches but not tags.

## Decision

`.github/workflows/release.yml` also accepts a manual run with a `tag`
input. The workflow builds the three archives and a single `publish` job
creates the tag on the selected branch's head together with the GitHub
release. A manual run without `tag` is a dry run that only keeps the
archives as workflow artifacts. Pushing a `v*` tag keeps working as before.

Releases are cut from `main`; development continues on `develop`.

## Consequences

The release is created by one job after all builds succeed, so a failing
platform no longer leaves a partially populated release.
