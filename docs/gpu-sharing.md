# GPU sharing with an external model server (Stage 23)

Design of Stage 23 in [dev_plan.md](../dev_plan.md)
([#68](https://github.com/Kauter1989/ferrum/issues/68)). Decision record:
[ADR 0015](decisions/0015-gpu-lease-protocol.md).

## 1. Context

Medical Harness runs a local language model (`llama-server`) on the same
12 GB RTX 3080 Ti as FERRUM's engines. Today:

| What | Where | Gap |
|---|---|---|
| GPU groups: one engine call of a group at a time, across processes | before this stage: `GpuLock` in `crates/ferrum-agent/src/sessions.rs`, used by `segment auto` (`commands/engine.rs`) and `segment interactive` (`commands/interactive.rs`) | lock file in the shared temporary folder (`/tmp/ferrum-gpu-<group>.lock`); undocumented; a mutex with no notion of size; the wait has no progress |
| Desktop AI tools | `crates/ferrum-app/src/viewer/ai.rs` (worker thread), wired in `crates/ferrum/src/ui/ai_panel.rs` | take no GPU lock at all |
| nnInteractive bridge frees the image on close | `NnInteractiveBackend.release` (`bridges/ferrum_bridges/nninteractive.py`) | not measured; weights and CUDA context stay (≈ 2 GB); nothing can unload them |
| Engine manifest | `GET /v1/info` (`docs/engine-protocol.md` §3) | says nothing about GPU memory |
| Benchmark | `scripts/benchmark_engines.py` | cannot run next to an external holder |

Answers at G1 (2026-10-23): the recommended answers to Q1–Q5 and Q7; the
card is an RTX 3080 Ti 12 GB. The harness's model (Q6) is measured in the
field run rather than assumed.

## 2. Constitution check

| Rule | Holds? |
|---|---|
| Layering `ferrum` → `ferrum-app` → `ferrum-domain` ← `ferrum-io` | yes. The new crate `ferrum-gpu-lock` is a leaf with no FERRUM dependency, used by `ferrum-engines` and `ferrum-agent`. `ferrum-app` learns about waiting only through a port in `ferrum-domain` (§3.4), so it still has no wgpu, egui or engine client. CLAUDE.md's layering section gains one line (ADR 0015). |
| Engines only in `ferrum-engines`; files only in `ferrum-io` | yes. The unload call and the gated engine live in `ferrum-engines`. Lock files are not data files; they are read only by `ferrum-gpu-lock`. |
| No inference in Rust; engines out of process | yes; the bridge's model moves into a child process of the bridge, still outside FERRUM. |
| Agents propose, people confirm | not affected. `engine unload` frees memory only, never touches results. |
| No identifiers leave FERRUM | yes: a holder record holds a holder name, a process id, a lease and a time, never a study path, UID or name (23.2-e). |
| No patient or volume data in the repository | yes; tests use the mock engine and synthetic volumes. |
| Rendering rule | not affected. |
| Security | the lock directory is per user and checked (23.1-c). `gpu hold` runs a command, so it exists in the CLI only and is **never** an MCP tool. |

## 3. Design

### 3.1 `ferrum-gpu-lock` (new crate)

A small crate, MIT OR Apache-2.0, with no FERRUM dependency. Its only
dependency is the safe `rustix` on Unix (the effective user id; the
workspace denies `unsafe`, so `libc` is not used; already in `Cargo.lock`). It implements the protocol `ferrum-gpu-lock/1` (§4.1), so the
harness, written in Rust, uses it directly, and a process in another
language follows §4.1.

```rust
pub struct LockDir { /* checked per-user directory */ }
impl LockDir {
    pub fn default_for_user() -> Result<Self, LockError>;   // §3.2
    pub fn at(path: impl Into<PathBuf>) -> Result<Self, LockError>;
}

pub enum Kind { Resident, Engine }          // a model server, or one engine call
pub struct Request<'a> {
    pub group: &'a str,
    pub holder: &'a str,                    // "llama-server", "ferrum: nnInteractive"
    pub kind: Kind,
    pub lease_mib: Option<u64>,             // None: the whole group
    pub capacity_mib: Option<u64>,          // None: one holder at a time (today's rule)
}
pub struct Wait<'a> { pub group: &'a str, pub held: &'a [HolderInfo], pub waited: Duration }

impl LockDir {
    /// Blocks until admitted, `timeout` elapsed, or `on_wait` returns false (cancel).
    pub fn acquire(&self, req: &Request, timeout: Duration,
                   on_wait: &mut dyn FnMut(&Wait) -> bool) -> Result<Lease, LockError>;
    pub fn holders(&self, group: &str) -> Result<Vec<HolderInfo>, LockError>;
}
pub struct Lease { /* releases on drop */ }
```

**Admission (one decision, under the group's admission lock, which is
only tried, so a process that keeps it cannot outlast the timeout):**
1. List `holder-*.lock`. A file whose OS lock can be taken belongs to a
   dead holder: delete it (stale). An OS lock is released by the kernel
   when its process dies, so `SIGKILL` cannot leave the group locked.
2. With the live holders `H` and the request `R`:
   - `capacity` unknown → admit only when `H` is empty (today's mutex,
     23.1-f);
   - `R` is an `Engine` and some holder in `H` is an `Engine` → wait (rule
     1 of agent-segmentation §2: FERRUM runs one engine call at a time);
   - a holder in `H` has no lease (whole group) → wait;
   - `R` has no lease, or its lease exceeds the capacity → admit only when
     `H` is empty, with a warning when the lease exceeds the capacity
     (23.2-d);
   - otherwise admit when `Σ lease(H) + lease(R) ≤ capacity`.
3. On admission, create `holder-<pid>-<random>.json`, take its OS lock,
   write the record, and release the admission lock. Otherwise release
   the admission lock, call `on_wait`, sleep 200 ms and retry.

A live holder file that cannot be parsed counts as a holder with no
lease (the safe reading). Records over 4 KiB are not read.

### 3.2 Lock directory (23.1-c)

| Platform | Directory |
|---|---|
| any | `network.gpu_lock_dir` when set |
| Linux | `$XDG_RUNTIME_DIR/ferrum/gpu`; without it `~/.cache/ferrum/gpu` (Q7) |
| macOS | `~/Library/Caches/ferrum/gpu` |
| Windows | `%LOCALAPPDATA%\ferrum\gpu` (per-user ACLs by default) |

Each group has its own subdirectory, named like today's file name
(characters other than ASCII letters, digits and `-` become `_`).
Directories are created with mode 0700. On Unix every directory from
`ferrum/` down and every holder file is checked with `symlink_metadata`.
FERRUM refuses, naming the path, any of them that:
- is a symlink;
- is owned by a user other than the effective user;
- for a directory, can be written by group or others.

FERRUM never falls back to `/tmp`. The ownership check is a function of
`(uid, mode, file type)`, so it is unit-tested without root. On Windows
the check is skipped and documented.

### 3.3 Configuration and expected peaks (23.2)

`[network]` in the operator configuration gains three optional keys. The
desktop app reads the same file (§3.4).

```toml
[network]
gpu_groups = { card = ["http://127.0.0.1:8765", "http://127.0.0.1:8766"] }
gpu_capacity_mib = { card = 11264 }                     # (new) leave headroom for the desktop's textures
gpu_peaks_mib = { "http://127.0.0.1:8765" = 6000, "http://127.0.0.1:8766" = 4500 }   # (new) per engine
gpu_lock_dir = "/run/user/1000/ferrum/gpu"              # (new) optional
```

The `[network]` GPU keys are parsed once, in `ferrum_engines::gpu::GpuSettings`.
`ferrum-agent`'s `AgentConfig` delegates them to it, and the desktop loads
only `[network]` from `$FERRUM_AGENT_CONFIG`. `ferrum-engines` takes the
`toml` dependency, which the workspace already has.

**Expected peak of an engine** = the larger of `gpu_peaks_mib[url]` and the
engine's `limits.expected_peak_mib`; when neither exists, the call takes
the whole group. A peak is the engine **process's whole GPU footprint
during a call** (weights, CUDA context and working memory), the number
`nvidia-smi --query-compute-apps` shows. An idle bridge's resident
memory is not leased. A holder that needs that room unloads idle bridges
(§3.5) and checks the free memory itself (NVML), as the protocol doc
says.

### 3.4 Who takes a lease

- **Agent** (`segment auto`, `segment interactive`): `GpuLock` is
  replaced by `ferrum_agent::sessions::gpu_lease`, over
  `ferrum_engines::gpu::GpuSettings::acquire`. The lease is held for the
  call, and the engine session is closed before the lease is released.
  MCP progress shows the wait ("waiting for GPU group card: llama-server
  (pid 4242) holds 6144 MiB; this call needs 6000 MiB"). The timeout gives
  `engine_unavailable` naming the holders, their leases and the lock
  directory (23.1-e). The holder name is `ferrum-agent: <engine name>`, cut
  to 128 characters with control characters replaced, as every holder
  name is. Engine sessions of a group are still not cached between MCP
  calls.
- **Desktop:** `ferrum_engines::gpu::GatedEngine` wraps any
  `SegmentationEngine`. It takes a lease in `open_session`, and in
  `start_job` for automatic jobs, and keeps it until the session is
  dropped, because an open interactive session holds the image on the GPU.
  - Waiting is reported through a new port in `ferrum-domain`:
    `trait EngineObserver { fn waiting(&self, note: &str) -> bool }`,
    which returns `false` to cancel.
  - `SegmentationEngine` gains `fn observe(&self, _: Arc<dyn EngineObserver>) {}`
    with a default empty body, so other engines are unchanged.
    `Viewer::connect_engine` registers an observer that turns the wait
    into `Event::Waiting(note)` and reads the existing cancel flag.
  - The AI panel shows "Waiting for GPU: held by …" with Cancel (23.3).
    `ai_panel.rs` wraps the `HttpEngine` in `GatedEngine` when the URL is
    in a group.
  - An open interactive session keeps its lease while the person
    works, so it can block an external holder; the session closes when the
    person switches tools or the study, as today.

### 3.5 Unload and resident memory (23.4)

- **Protocol (additive to v1):**
  - `capabilities.unload: true`;
  - `POST /v1/unload` returns 204, or 503 `busy` with `Retry-After` while
    a session or job is active;
  - `limits.expected_peak_mib` and top-level `resident_mib` in `GET /v1/info`,
    both optional.
- **Bridges:** a `WorkerBackend` wraps a backend in a child process
  (`multiprocessing` with `spawn`).
  - Every backend call is forwarded over a pipe. Volumes and mask
    regions travel as NumPy arrays: ≤ 32 MB for an 8 M-voxel ROI, which is
    measured in the field.
  - `unload()` ends the child, which frees all of its GPU memory
    including the CUDA context. The next `open` starts it again
    (reload time measured).
  - The manifest is cached from the first start.
  - **nnInteractive** runs in the worker.
  - **TotalSegmentator** already runs each job in a child process, so
    `unload` is a no-op that reports success.
  - **MONAI Label** is a client of another server: `unload: false`.
  - `resident_mib` is the child's memory from
    `nvidia-smi --query-compute-apps` when available, otherwise from
    `torch.cuda.memory_reserved`. It is advisory (point 7 of #68).
- **Agent:** `engine unload { engine?, group? }`. It calls each allowed
  engine (all of the group when `group` is given) and returns
  `[{engine, result: unloaded|busy|unsupported|unreachable, resident_mib?}]`.
  It never cancels a job. `engine list` adds `resident_mib` and
  `unload` per engine.
- **Release on idle (23.4-a):** already in `release()`; the stage adds a
  bridge test with a fake torch for both the `DELETE` and the TTL path,
  and the field measurement.

### 3.6 `ferrum-cli gpu hold` (23.5-d)

`ferrum-cli gpu hold --group G [--lease-mib N] [--name NAME] -- <command> [args…]`
loads the operator configuration and takes a `Resident` lease. It starts
the command, forwards Ctrl-C to it, and releases the lease when the
command exits, returning the command's exit code. The logic lives in
`ferrum_agent::gpu::hold`; `ferrum-cli` only parses arguments. It is not
an agent command: it is in neither the MCP tool list nor `commands.json`,
and a test checks that it is absent from both.

As built: the command runs in its own process group (Unix), so Ctrl-C at
the terminal reaches only `gpu hold`. `gpu hold` passes Ctrl-C or SIGTERM
on once as SIGINT and releases the lease only after the command has
exited; a Ctrl-C during the wait stops the wait. A command in its own
process group must not read the terminal (a model server does not).
`--timeout-s` defaults to `limits.engine_job_timeout_s`.

### 3.7 Benchmark and recipe (23.5)

- `scripts/benchmark_engines.py` gains these options:
  - `--external-holder "<command>"` starts the holder under
    `ferrum-cli gpu hold`;
  - `--lease-mib N`;
  - `--wait-ready <url>` polls the holder's health URL (`llama-server`
    `/health`) before the steps start.
- Each step records the card's peak, the holder's memory (by pid), and
  the time the call waited.
- The recipe for 12 GB goes to `docs/gpu-sharing.md` §5 after the field
  run.

## 4. Contracts

| Contract | Change | Also update (`ferrum-change` §2) |
|---|---|---|
| `ferrum-gpu-lock/1` (new) | §4.1 | this doc; ADR 0015; `docs/architecture.md`; CLAUDE.md layering line |
| `ferrum-engine/1` | `capabilities.unload`, `POST /v1/unload`, `limits.expected_peak_mib`, `resident_mib` (all optional) | `docs/engine-protocol.md`; conformance suite (unload refused while busy, 204 when idle); bridges and `pytest` |
| Agent commands | `engine unload` (new); `engine list` adds `resident_mib`, `unload`; GPU wait errors name holders | `schema.rs`, `commands.json`, `skills/ferrum/reference/commands.md`, `docs/agent-cli.md`, contract tests, CLI parse test, privacy scan |
| Operator configuration | `gpu_capacity_mib`, `gpu_peaks_mib`, `gpu_lock_dir` | `docs/agent-cli.md` (configuration), config tests |
| CLI only | `gpu hold` | `docs/agent-cli.md`, CLI parse test, "not in MCP" test |
| Desktop UI | waiting state and Cancel in the AI panel | `cargo test -p ferrum --test ui`; use case in `Viewer` |
| `ferrum-domain` port | `EngineObserver`, `SegmentationEngine::observe` (default no-op) | `docs/architecture.md` ports table |

### 4.1 Protocol `ferrum-gpu-lock/1`

A process outside FERRUM that is not written in Rust follows this text.
A Rust process uses the crate `ferrum-gpu-lock`.

**Files.** `<lock dir>` is §3.2, and `<group>` is the group name with
characters other than ASCII letters, digits and `-` replaced by `_`.

```
<lock dir>/<group>/admission.lock        taken (exclusive) only while one holder is admitted
<lock dir>/<group>/holder-<id>.lock      exclusive OS lock, held for the holder's lifetime
<lock dir>/<group>/holder-<id>.json      the holder's record (never locked, so it can be read on Windows)
```

`<id>` is unique on the machine; the crate uses `<pid>-<nanoseconds>-<counter>`.

**Record** (one JSON object; the crate writes one line; readers accept any
whitespace):

```json
{"protocol":"ferrum-gpu-lock/1","holder":"llama-server","pid":4242,"kind":"resident","lease_mib":6144,"since_unix_s":1792789468}
```

- `kind` is `resident` (a process that stays on the card, such as a model
  server) or `engine` (one engine call).
- `lease_mib` is the GPU memory held, or `null` for the whole group.
- The record names no study, path or patient.

**OS lock.** `flock(fd, LOCK_EX)` on Unix, and `LockFileEx` with
`LOCKFILE_EXCLUSIVE_LOCK` on Windows. This is what Rust's
`File::lock`/`try_lock` use, and Python's `fcntl.flock`.

**To join a group:**
1. Create the directories if missing (mode 0700). Refuse a directory or
   file that is a symlink, that is owned by another user, or (for a
   directory) that group or others can write.
2. Open `admission.lock` (create, mode 0600) and take its lock (blocking).
3. For every `holder-*.lock`, try its lock without blocking:
   - if the lock is free, its holder is dead: release it, then delete
     the `.json` and the `.lock`;
   - if it is taken, read the `.json` (at most 4 KiB). A record that
     cannot be read counts as `lease_mib: null`.
4. Decide with the live holders and the capacity of the group (§3.1,
   [`decide`](../crates/ferrum-gpu-lock/src/lib.rs)).
5. **Admitted:** create `holder-<id>.lock` (create-new, mode 0600), take
   its lock, write `holder-<id>.json` (create-new, mode 0600), then
   release `admission.lock`.
6. **Not admitted:** release `admission.lock`, wait about 200 ms, and go
   back to step 2 until the timeout.

**To leave:** delete the `.json`, close the `.lock` (which releases the
OS lock), then delete the `.lock`. A crash at any point leaves a lock
that is free, which the next admission cleans up.

**Python sketch of the lease a model server holds:**

```python
import fcntl, json, os, time
d = os.path.join(os.environ["XDG_RUNTIME_DIR"], "ferrum", "gpu", "card")
os.umask(0o077)                              # every level must be 0700: makedirs' mode is the leaf's only
os.makedirs(d, exist_ok=True)
with open(os.path.join(d, "admission.lock"), "a+") as adm:
    fcntl.flock(adm, fcntl.LOCK_EX)          # steps 3–4 omitted: assume the group admits us
    # hold the admission lock only to decide, never while the server runs
    hid = f"{os.getpid()}-{time.time_ns()}-0"
    lock = open(os.path.join(d, f"holder-{hid}.lock"), "x")
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    with open(os.path.join(d, f"holder-{hid}.json"), "x") as rec:
        json.dump({"protocol": "ferrum-gpu-lock/1", "holder": "llama-server", "pid": os.getpid(),
                   "kind": "resident", "lease_mib": 6144, "since_unix_s": int(time.time())}, rec)
# … keep `lock` open while the server runs
```

Records and lock files must be regular files. Open them with
`O_NOFOLLOW | O_NONBLOCK`, and check the opened handle rather than the
path. A record that is anything else counts as holding the whole group.

**Not supported:**
- processes of one user that see different lock directories. A process
  started without `XDG_RUNTIME_DIR` (cron, `sudo -u`, some SSH sessions, a
  container) uses `~/.cache/ferrum/gpu`; one started with it uses
  `/run/user/<uid>/ferrum/gpu` (on WSLg, GUI apps get
  `/mnt/wslg/runtime-dir`). Each would be admitted alone. Set
  `network.gpu_lock_dir` for every participant when in doubt; FERRUM's
  wait and timeout messages name the directory in use;
- network file systems;
- a container and its host sharing a group, unless both see the same
  directory as the same user.

## 5. Risks and assumptions

- **The peaks are estimates until measured (L13).** The values in §3.3
  come from agent-segmentation §2: nnInteractive about 5.4 GB above a
  baseline that included the desktop, TotalSegmentator `total` about
  2.2 GB for two organs. PR 2's field test measures per-process
  footprints, and the recipe uses those.
- **The lease is cooperative.** A process that does not take a lease
  (another user's job, a game) is invisible. The capacity leaves
  headroom, and the docs tell holders to check NVML before loading.
- **Starvation:** a waiting engine call can be passed by short holders
  that keep arriving. A FIFO ticket queue is left for later (out of
  scope). The timeout bounds the wait.
- **The desktop's interactive session holds its lease while open.** A
  person idling in the AI tool blocks a model server that needs the room.
  Mitigation: the lease is released when the session closes; an idle
  timeout is left for later if the field run shows the need.
- **Worker process overhead:** pickling the ROI per `open`, and mask
  regions per prompt. Estimated below 50 ms for an 8 M-voxel ROI; PR 2's
  field test measures it against the current 0.9–1.2 s per prompt. If it
  is too slow: shared memory (`multiprocessing.shared_memory`).
- **The CUDA context of the child:** ending the child frees it. This was
  verified in principle (the TotalSegmentator bridge already relies on
  it), but not yet for nnInteractive.
- **Windows:** the lock and directory code compile there (release
  builds); CI runs Linux only. The Windows path is checked in the field
  only if the user runs the desktop natively.

## 6. Validation plan

| Criterion | Verified by | Level |
|---|---|---|
| [23.1-a] | `docs/gpu-sharing.md` §4.1 review; `ferrum-gpu-lock` doc tests of the record format | test, manual |
| [23.1-b] | `ferrum-gpu-lock` builds with only `rustix`; `cargo tree -p ferrum-gpu-lock` check in a test; `ferrum-agent` has no `GpuLock` left | test |
| [23.1-c] | `dir_check_refuses_other_owner_symlink_and_group_writable` (unit, injected metadata); `refuses_world_writable_dir` (real dir, 0777); `default_dir_never_tmp` | test |
| [23.1-d] | `two_processes_holder_then_segment_auto` (holder child process, mock engine, wait then success ≤ 1 s after release); `killed_holder_is_stale` (`SIGKILL`) | test |
| [23.1-e] | `wait_timeout_names_holder` (envelope); MCP progress notification during the wait (`mcp.rs` test) | test |
| [23.1-f] | `no_capacity_is_a_mutex` (external + engine) | test |
| [23.2-a] | `engine_calls_stay_one_at_a_time_under_capacity` | test |
| [23.2-b] | `peak_from_manifest_raises_never_lowers`; `no_peak_takes_whole_group` | test |
| [23.2-c] | `lease_6000_blocks_7000_admits_4000` (capacity 12 000) | test |
| [23.2-d] | `peak_over_capacity_runs_alone_with_warning` | test |
| [23.2-e] | `gpu_sharing.rs::lock_records_name_no_study` (no workspace or source path in the records of an engine call); holder names are engine names or operator names, cut and cleaned (`record.rs::holder_names_are_cut_and_cleaned`) | test |
| [23.3-a] | `gated_engine_takes_lease_on_open_and_job` (ferrum-engines); desktop config load test | test |
| [23.3-b] | `viewer_reports_waiting_and_cancel_sends_nothing` (ferrum-app, mock engine + external holder) | test |
| [23.3-c] | UI test `ai_panel_shows_waiting_for_gpu`; a person checks the panel with a real holder | test, manual |
| [23.4-a] | `pytest` `test_release_on_delete_and_ttl` (fake torch); `nvidia-smi --query-compute-apps` before/after on the user's card | test, field |
| [23.4-b] | conformance `unload_refused_while_busy`, `unload_idle`; `pytest` worker unload; field: 0 MiB after unload, reload time | test, field |
| [23.4-c] | contract test `engine_unload_reports_per_engine` (mock server with and without `unload`); CLI = MCP JSON | test |
| [23.4-d] | contract test `engine_list_shows_resident_mib`; doc review | test, manual |
| [23.5-a] | `benchmark_engines.py --dry-run` with a fake holder in `pytest` | test |
| [23.5-b] | field run: 20 calls, no OOM, table in agent-segmentation §2 | field |
| [23.5-c] | review of §2 rule 2 and ADR 0015 | manual |
| [23.5-d] | `gpu_hold_runs_command_and_releases` (CLI test with `sh -c 'exit 3'` → exit 3, lease gone); `gpu_hold_not_an_mcp_tool` | test |

## 7. Size and PR split

Contract surfaces: the new lock protocol, `ferrum-engine/1`, the agent
command set, the bridges' Python API and the desktop UI. Five surfaces, so
the stage is **XL**, split into three PRs of size L, one new surface each
where possible. The estimates are compared with #63 (L, 739 lines) and
#64 (M, 968 lines).

| PR | Tasks | Estimate (code + tests + docs) | Surfaces | Class | Field test after |
|---|---|---|---|---|---|
| 1 | 23.T1–T3 | ~900 + 350 + 250 ≈ 1 500 lines, ~16 files | lock protocol, config | L (low) | — (CPU and processes only; CI proves it) |
| 2 | 23.T4–T6 | ~700 (Rust 300, Python 400) + 300 + 200 ≈ 1 200 lines, ~14 files | `ferrum-engine/1`, bridges API, agent command | L | yes: release, unload, reload time, worker overhead, per-process peaks |
| 3 | 23.T7–T9 | ~650 + 250 + 300 ≈ 1 200 lines, ~14 files | desktop UI (+ CLI only) | L (low) | yes: the success metric with `llama-server` |

## 8. Tasks

| Task | Criteria | Tests first | Touches (matrix rows) | Depends on | PR |
|---|---|---|---|---|---|
| 23.T1 `ferrum-gpu-lock`: directory checks, holder records, admission, stale holders | 23.1-a/b/c/f, 23.2-a/c/d | `dir_check_*`, `killed_holder_is_stale`, `lease_6000_blocks_7000_admits_4000` | architecture or decision (ADR 0015, architecture.md, CLAUDE.md) | — | 1 |
| 23.T2 `GpuSettings` in `ferrum-engines`; `AgentConfig` delegates; peaks | 23.2-b | `peak_from_manifest_raises_never_lowers`, config tests | agent outputs (config docs) | T1 | 1 |
| 23.T3 Agent on the gate: wait with progress, timeout naming holders | 23.1-d/e, 23.2-e | `two_processes_holder_then_segment_auto`, `wait_timeout_names_holder` | agent command (errors), agent outputs (privacy) | T2 | 1 |
| 23.T4 Protocol additions + conformance | 23.4-b/d (protocol) | `unload_refused_while_busy`, `unload_idle` | engine protocol | T1 | 2 |
| 23.T5 Bridges: `WorkerBackend`, unload, `resident_mib`, release tests | 23.4-a/b | `test_release_on_delete_and_ttl`, worker unload test | engine protocol (bridges) | T4 | 2 |
| 23.T6 `engine unload`, `engine list` fields | 23.4-c/d | `engine_unload_reports_per_engine` | agent command | T4 | 2 |
| 23.T7 Domain port + `GatedEngine` + `Viewer` waiting/cancel | 23.3-a/b | `gated_engine_takes_lease_on_open_and_job`, `viewer_reports_waiting_and_cancel_sends_nothing` | desktop UI (use case) | T2 | 3 |
| 23.T8 AI panel waiting state; `gpu hold` | 23.3-c, 23.5-d | `ai_panel_shows_waiting_for_gpu`, `gpu_hold_runs_command_and_releases`, `gpu_hold_not_an_mcp_tool` | desktop UI, CLI | T7, T3 | 3 |
| 23.T9 Benchmark holder options; field run; recipe; agent-segmentation §2 | 23.5-a/b/c | `pytest` dry run | user-visible behaviour (CHANGELOG, docs) | T5, T8 | 3 |
