# ADR 0005 — Repository renamed to `ferrum`

- **Status:** Accepted
- **Date:** 2026-10-01

## Context

ADR 0004 renamed the product, crates and binary to FERRUM but kept the
GitHub repository name, `dicom_renderer`.

## Decision

The repository is renamed to
[`Kauter1989/ferrum`](https://github.com/Kauter1989/ferrum). This
supersedes the last bullet of ADR 0004's decision. Links in the README,
the About dialog and the workspace manifest point to the new address.

## Consequences

GitHub redirects the old URLs, so existing clones and links keep working.
Clones should still update their remote:
`git remote set-url origin https://github.com/Kauter1989/ferrum`.
