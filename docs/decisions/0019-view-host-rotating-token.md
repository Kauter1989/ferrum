# ADR 0019 — A short-lived, rotating token for the view host

- **Status:** Proposed (G2 of Stage 29)
- **Date:** 2026-11-16

## Context

[ADR 0017](0017-view-host-crate.md) made the token the only credential of
`ferrum-view/1`: 32 hex digits from the OS random source, accepted in the query
string, an `Authorization: Bearer` header, or an `HttpOnly`/`SameSite=Strict`
cookie, compared in constant time, never logged. It is **static for the life of
the process** (`new_token` once per `ViewServer::start`). A token that leaks —
into a URL, a referrer, a crash dump, a shared screen — stays valid until the
operator restarts the host. Stage 29 (29.4-a) asks for a short-lived, rotating
token; the spec requires an ADR first.

The host is loopback-only and serves one workspace; the client is the person's
own browser panel (or an `<iframe>`/`<img>` on the same machine). There is no
TLS and no remote exposure (out of scope), so the token is a local capability,
not a network credential. Rotation must not break a long-lived panel that the
person leaves open.

## Decision

1. **A token carries a TTL.** The host issues a token valid for **15 minutes**
   (`TOKEN_TTL = 900 s`). The initial token printed by `ferrum-cli view serve`
   is the first such token; it boots the session exactly as today.

2. **Refresh rotates.** `POST /api/token/refresh`, authorized by the current
   token, returns the next one: `{"token":"<32 hex>","expires_in":900}`. It
   re-sets the `ferrum-view` cookie (`HttpOnly`, `SameSite=Strict`). An expired
   or wrong token is `401` and refreshes nothing — an attacker without a live
   token cannot rotate.

3. **A grace window overlaps the rotation.** The previous token stays valid for
   **60 s** after a new one is issued (`TOKEN_GRACE = 60 s`), so requests already
   in flight (and a panel mid-refresh) do not `401`. At most two tokens are live
   at once; both are compared in constant time.

4. **The client refreshes proactively.** The panel schedules a refresh at ~80 %
   of `expires_in` (12 min) and retries on a `401` by re-reading the boot token
   only if the operator re-runs `view serve`. The token stays in `localStorage`
   as today; it is never written to the workspace, a log line or a frame.

5. **Nothing else changes.** Loopback only; the query/Bearer/cookie forms stay;
   `/panel/<file>` still needs no token (it serves code, not data); the privacy
   and body/event limits stay. The TTL and grace are constants, not operator
   configuration, in this stage.

## Consequences

- A leaked token expires within 15 minutes (and within 60 s of the next
  rotation), bounding the window a local leak is useful; the static-token
  restart-to-revoke behaviour is gone.
- The protocol gains one route (`POST /api/token/refresh`) and an `expires_in`
  field; `docs/view-host.md` §4.1 and the panel's connect/refresh loop change,
  with tests for expiry, refresh, the grace overlap and a wrong-token refusal.
- A panel left open keeps working (it refreshes itself); a harness that only
  draws `/api/frame.png` with the boot token must refresh too, or reconnect
  after the TTL — documented in the mount guide.
- The TTL/grace are a local-policy choice, not a network-security claim: without
  TLS the token still crosses loopback in the clear, and remote exposure remains
  out of scope. A later stage that adds TLS or remote access revisits this ADR.
