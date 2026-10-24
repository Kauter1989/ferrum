# ADR 0015 — GPU leases shared with processes outside FERRUM

- **Status:** Accepted (G2, 2026-10-23)
- **Date:** 2026-10-23

## Context

[ADR 0010](0010-agent-segmentation.md) introduced GPU groups: engines
that share a card run one call at a time, through an advisory file lock
in the temporary folder. Medical Harness now keeps a language model
(`llama-server`) resident on the same 12 GB card
([#68](https://github.com/Kauter1989/ferrum/issues/68), Stage 23). The
lock as built:
- cannot be joined by another process, because its location and format
  are undocumented;
- sits in a directory every local user can write;
- is a mutex, so it cannot let an engine run next to a model when both
  fit;
- is not taken by the desktop app.

Design: [docs/gpu-sharing.md](../gpu-sharing.md).

## Decision

1. **A protocol and a crate.** `ferrum-gpu-lock/1` is a documented
   protocol. The new crate `ferrum-gpu-lock` (MIT OR Apache-2.0, no FERRUM
   dependency; only the safe `rustix` on Unix, for the effective user id)
   implements it for FERRUM and for Rust
   hosts. It is a leaf below `ferrum-engines` and `ferrum-agent`.
2. **Leases, not a mutex.**
   - Each holder holds an OS lock on its own lock file, next to an
     unlocked record that declares a lease in MiB. When it dies the kernel
     drops its lock, so the record becomes stale.
   - A group with a configured capacity admits holders while their
     leases fit. A group without one keeps today's one-holder rule.
   - FERRUM's own engine calls of a group still run one at a time.
3. **An engine's expected peak** is the larger of the operator's value
   and the engine's manifest (`limits.expected_peak_mib`). The engine can
   raise it, never lower it. Without either, a call takes the whole group.
4. **Per-user lock directory,** checked for owner, permissions and
   symlinks. FERRUM never uses the shared temporary folder for it.
5. **Unload is part of `ferrum-engine/1`** (`POST /v1/unload`, optional,
   refused while busy). Bridges run their model in a child process so
   that unloading frees the CUDA context. Resident memory reported by an
   engine is advisory; holders check free memory themselves.
6. **The desktop takes leases too.** A `GatedEngine` decorator in
   `ferrum-engines` takes the lease. `ferrum-app` learns about waiting
   only through a new `ferrum-domain` port (`EngineObserver`), so its
   layering is unchanged.
7. **`gpu hold` is a CLI-only operator tool.** It starts a command, so it
   is never exposed over MCP.

## Consequences

- The layering section of CLAUDE.md and `docs/architecture.md` gain the
  crate `ferrum-gpu-lock` and the port `EngineObserver`.
- Lock files move from `/tmp/ferrum-gpu-<group>.lock` to the per-user
  directory. A FERRUM before this change and one after it do not see each
  other's lock. The release notes say to upgrade every FERRUM process on a
  machine together.
- `ferrum-engine/1` gains optional fields and an optional endpoint. Old
  bridges keep working with `unload: false` and no peak, so their calls
  take the whole group.
- Admission is cooperative: a process that takes no lease is invisible.
  The capacity keeps headroom, and the docs say so.
- Not decided here: a fair (FIFO) queue for waiting holders, and leases
  across several GPUs or machines.
