# ADR 0009 — Open source under MIT OR Apache-2.0

- **Status:** Accepted
- **Date:** 2026-10-02

## Context

The repository was private and licensed under MIT. It is about to become
public. FERRUM is meant to be embedded in commercial and research products
([vision](../vision.md)); its maintainer earns from integration work
(engines, PACS connectors, embedding, agent harnesses, support), not from
licence sales. Medical imaging is a patent-heavy field, and MIT carries no
explicit patent grant. All commits so far are the maintainer's, so the
licence can still change without other contributors' consent.

## Decision

- Source code is licensed under **MIT OR Apache-2.0** at the user's
  option, the convention of the Rust ecosystem. Apache-2.0 adds an
  explicit patent licence; MIT remains for those who need it.
  `LICENSE` becomes `LICENSE-MIT` and `LICENSE-APACHE`; manifests, the
  release archives and the README follow.
- Contributions are accepted under the same dual licence with a
  Developer Certificate of Origin sign-off (`CONTRIBUTING.md`); no CLA,
  because no dual licensing with a proprietary licence is planned.
- Customer-specific integrations live in separate, private crates and
  repositories that plug into the existing ports; the public core stays
  complete and free.
- The licence covers the code, not the name: forks are asked to use
  another name.
- `cargo deny check licenses` keeps every dependency permissive (CI job
  `licenses`, `deny.toml`).
- Copyleft (GPL/AGPL with a commercial licence) was rejected: it would
  block the embedding that FERRUM exists for.

## Consequences

Anyone, including competitors, may use and ship FERRUM commercially. The
maintainer's advantage is expertise, the protocols and the name.
Changing the licence later requires the consent of every contributor.
Screenshots in `docs/images/` remain CC BY-SA 4.0 as derivatives of their
source data.
