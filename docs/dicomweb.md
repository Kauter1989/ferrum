# DICOMweb client — QIDO/WADO VolumeRepository (Stage 32, PR 1)

- **Spec:** `dev_plan.md` Stage 32, criteria 32.1-a…c (#76)
- **Decision:** [ADR 0021](decisions/0021-pacs-client.md)
- **Status:** PRs 1–3 (32.T1–T3) merged (#136, #137, #138); PR 4 implements
  32.T4 (Orthanc-in-Docker CI integration + highdicom validation, §10), closing
  #76

FERRUM opens studies a site keeps in its PACS. The client is a
`VolumeRepository` implementation in `ferrum-io`
(`crates/ferrum-io/src/dicom/web.rs`, `DicomWebRepository`) built on
[`dicom-web`](https://crates.io/crates/dicom-web) 0.6 (dicom-rs, MIT OR
Apache-2.0). It is read-only: query (QIDO-RS) and retrieve (WADO-RS).
Send-off (STOW-RS, C-MOVE, C-STORE) is host-only and arrives in later PRs
behind the boundary of ADR 0021 §5 — PR 1 contains none of it.

## 1. Repository fit — the port stays path-based

`VolumeRepository` (`crates/ferrum-domain/src/repository.rs`) is unchanged:

```text
scan(paths: &[PathBuf]) -> Vec<SeriesDescriptor>     paths are PACS URLs
load(series: &SeriesDescriptor) -> LoadedSeries      sources[0] is the retrieval URL
volumes(series) -> Vec<SeriesDescriptor>             default: the whole series (PR 1)
```

A `PathBuf` is a string container on every platform; the repository
interprets it, never touches the file system with it. This keeps one port
for files and PACS, so `CompositeRepository`, the desktop viewer, the view
host and the CLI open a PACS study with no app-layer change.

### URL grammar

```text
dicomweb://host[:port]/service-root[?keyword=value&…]     → http://host[:port]/service-root
dicomwebs://host[:port]/service-root[?keyword=value&…]    → https://host[:port]/service-root
```

`service-root` is the DICOMweb base of the server, e.g. Orthanc's
`/dicom-web` (`http://localhost:8042/dicom-web`). The scheme is replaced
by `http`/`https`; the rest is the base URL `dicom-web` expects.

Query parameters are QIDO matching attributes. A fixed keyword→tag table
(only DICOM keywords, via `dicom_dictionary_std::tags`):

| Keyword | Tag | Keyword | Tag |
|---|---|---|---|
| `PatientID` | (0010,0020) | `StudyDate` | (0008,0020) |
| `PatientName` | (0010,0010) | `Modality` | (0008,0060) |
| `AccessionNumber` | (0008,0050) | `StudyDescription` | (0008,1030) |
| `StudyInstanceUID` | (0020,000D) | `SeriesDescription` | (0008,103E) |
| `SeriesInstanceUID` | (0020,000E) | `SeriesNumber` | (0020,0011) |

Reserved control parameters: `limit`, `offset` (QIDO pagination, passed to
`with_limit`/`with_offset`). An unknown keyword is `Unsupported`, naming the
keyword — not its value. Values are passed through percent-decoding once
(the URL layer) and then verbatim to QIDO; no fuzzy matching in PR 1.

**No userinfo.** `dicomweb://user:pass@host/…` is refused: credentials in a
URL leak into logs, error strings and recent-files lists. Auth is configured
on the repository (§4).

### Descriptors

- `id`: `{SeriesInstanceUID}#w:{host[:port]}` — stable, unique per server,
  no patient identifiers. (The file readers use
  `{SeriesInstanceUID}#{cols}x{rows}#{orientation}`; the `#w:` suffix keeps
  the two namespaces apart.)
- `format`: `"DICOMweb"` — equals `name()`, so `CompositeRepository`'s
  format dispatch routes `load`/`volumes` back to this repository.
- `sources`: one entry, the WADO-RS retrieval URL in the same scheme:
  `dicomweb://host/service-root/studies/{StudyInstanceUID}/series/{SeriesInstanceUID}`.
  `load` parses it back; the descriptor is self-contained, so a later load
  (e.g. a volume switch or a reopened series picker) needs no re-query.
- `dims`: Rows × Columns × NumberOfSeriesRelatedInstances when QIDO returns
  them (requested via `includefield`); `0` for what the server did not
  return — the exact grid is known after `load`.
- `description`: series description (or "Unnamed series"), instance count
  and modality — series-level text only, never patient name/ID.

### Composite behaviour

`CompositeRepository::default()` gains `DicomWebRepository::new()` after the
file readers. The repositories coexist because each declines what is not
its own:

- file readers: a URL is not a file → `collect_files` finds nothing →
  `NothingFound`;
- `DicomWebRepository`: a path without a `dicomweb(s)://` scheme →
  `NothingFound` (it is a file path, not a malformed PACS query).

A malformed PACS URL (missing host, bad port, userinfo, unknown keyword) is
`Unsupported` with a message that names the problem and the host only.

## 2. QIDO flow (`scan`)

For each URL, one series-level QIDO-RS request:

1. `StudyInstanceUID` among the filters →
   `client.query_series_in_study(uid)` with the remaining filters;
   otherwise `client.query_series()` with all filters. (The study-scoped
   endpoint is what classic PACS index efficiently; the global one serves
   patient/date queries.)
2. `includefield`: Modality, SeriesDescription, SeriesNumber,
   NumberOfSeriesRelatedInstances, Rows, Columns, StudyInstanceUID,
   SeriesInstanceUID — what a descriptor needs without retrieving pixels.
3. `limit`/`offset` when given.
4. The response is `Vec<InMemDicomObject>` (DICOM JSON, parsed by
   `dicom-web`); each object becomes one descriptor. Objects without a
   SeriesInstanceUID are skipped (logged) — they cannot be retrieved.

All URLs empty → `RepositoryError::NothingFound` (32.1-c). Progress is
reported per URL; cancellation is checked between URLs and mapped to
`Cancelled`.

## 3. WADO flow (`load`)

1. Parse `sources[0]` back into base URL + study UID + series UID.
2. `client.retrieve_series(study, series).run()` — WADO-RS series-level
   retrieval: one `multipart/related; type="application/dicom"` response
   whose parts are whole Part 10 objects (metadata **and** pixel data).
   PR 1 deliberately retrieves whole instances rather than
   `…/metadata` + `…/frames/{list}`: the parts are exactly what the file
   reader decodes, so no transfer-syntax re-encapsulation is needed and the
   pixel path is identical. Per-frame retrieval (bandwidth optimisation for
   huge multi-frame objects) is a follow-up.
3. Collect the stream into `Vec<FileDicomObject<InMemDicomObject>>`,
   reporting progress per instance and checking cancellation. An empty
   stream → `NothingFound`.
4. `SliceHeader::from_object` per instance (the same extractor the file
   reader uses; the synthetic per-instance URL is the error-reporting
   "path"), then `series::group` → keep the group of the requested series →
   `series::order` (position/instance-number ordering, warnings).
5. `assemble::assemble_with(headers, ordered, progress, decode)` where
   `decode` decodes frames **from the retrieved objects**
   (`dicom_pixeldata::PixelDecoder`, Modality LUT applied, colour →
   luminance). The file reader's `assemble` calls the same function with a
   decode-from-disk closure. One implementation of ordering, the lossless
   HU grid (`IntensityRange`) and `to_canonical` LPS reorientation serves
   both sources (L23): a retrieved study is voxel-identical to the same
   study opened from files.
6. Metadata: `header::describe` + `header::study_info` of the first
   instance, plus Slices / Slice spacing / Ordering attributes as the file
   reader adds them; `source` is the retrieval URL. `StudyInfo` and the
   patient identifiers stay inside FERRUM exactly as for files (the privacy
   scan reads them; agent outputs pseudonymise, PR 3).

`volumes()` keeps the port default (the whole series) in PR 1; multi-volume
partitioning over PACS needs the same metadata pass and is a follow-up.

## 4. Auth, timeouts, errors (32.1-c)

**Auth.** `DicomWebRepository::new()` is anonymous;
`.with_basic_auth(user, pass)` and `.with_bearer_token(token)` configure the
Authorization header (basic wins when both are set — `dicom-web`'s rule).
Credentials live only in the repository instance; they appear in no URL,
descriptor, error string or log line. A 401/403 is reported as an
authentication failure naming the host and status.

**Timeouts.** `dicom-web` 0.6 builds its `reqwest::Client` internally with
no timeout configuration and does not expose it. The repository therefore
wraps every request future in `tokio::time::timeout` (default 30 s,
`.with_timeout(Duration)`); on expiry the future is dropped (reqwest aborts
the request) and the error is `Io("… timed out after …")`. The deadline
covers the whole request including the multipart body stream.

**Runtime.** `dicom-web` is async; the port is sync. Each `scan`/`load`
builds a current-thread tokio runtime (`enable_all`) and blocks on it.
FERRUM's hosts (eframe, tiny_http, stdio MCP) are synchronous; the
repository must not be called from inside an ambient tokio runtime
(`block_on` would panic) — a documented constraint, checked by construction
in every current host.

**Error mapping** — every failure is a `RepositoryError`, never a panic:

| Condition | `dicom-web` / boundary | `RepositoryError` |
|---|---|---|
| DNS/connect/refused/reset | `RequestFailed` (`is_connect`) | `Io` |
| deadline expired | `tokio::time::timeout` | `Io` (timed out) |
| HTTP 401/403 | `HttpStatusFailure` | `Io` (authentication) |
| other HTTP ≥ 400 | `HttpStatusFailure` | `Io` (status) |
| response not DICOM JSON (e.g. an HTML error page) | `UnexpectedContentType`, `MissingContentTypeHeader`, `ContentTypeParseFailed` | `Corrupt` |
| JSON body not DICOM JSON | `DeserializationFailed` | `Corrupt` |
| multipart broken / not `related` / part not DICOM | `MultipartReaderFailed`, `UnexpectedMultipartType`, `DicomReaderFailed` | `Corrupt` |
| empty multipart | `EmptyResponse` | `NothingFound` |
| QIDO returns `[]`; WADO stream empty; series vanished | — | `NothingFound` |
| instance missing Rows/Columns, pixel decode fails | `IoError` via the shared pipeline | `Corrupt` / `Unsupported` (existing `From<IoError>`) |
| cancelled | progress sink | `Cancelled` |

Messages name the host and the failure kind. **Query values never appear in
errors** (a `PatientID` filter is an identifier): the URL is stripped to
scheme + authority + path before it is quoted, and the query is dropped.

## 5. Privacy

- Retrieved pixel data stays **in memory**; the client writes no patient
  data to disk (the rejected spooling alternative would have).
- Descriptors' public text (`description`, `id`) carries series-level
  information only — no patient name, ID, birth date or accession number.
- Error strings drop the query part of URLs and never quote credentials.
- `SeriesMetadata.study`/patient identifiers follow the file readers' rule:
  internal to FERRUM, read by the privacy scan, pseudonymised in agent
  outputs (PR 3).
- Send-off (STOW/C-MOVE/C-STORE) is host-only, never an agent command
  (ADR 0021 §5); PR 1 implements no send-off.
- Orthanc's DICOMweb plugin is **AGPL**: a separate Docker service in CI
  (PR 4), never linked. `dicom-web` is MIT OR Apache-2.0; its tree passes
  `cargo deny check licenses` (reqwest/rustls/aws-lc-rs are
  ISC/Apache-2.0/MIT/BSD-family).

## 6. Dependencies

| Crate | Version | Why | Licence |
|---|---|---|---|
| `dicom-web` | 0.6 | QIDO/WADO client on dicom-rs 0.10 (0.5 is on 0.9 — incompatible object types; 0.7 exists but the spec pins 0.6) | MIT OR Apache-2.0 |
| `tokio` | 1, features `rt`, `net`, `time`, default-features off | per-operation runtime + request deadline | MIT |
| `futures-util` | 0.3, `std` | consume the WADO multipart stream | MIT OR Apache-2.0 |
| `aws-lc-rs` | 1, feature `prebuilt-nasm` | reqwest's rustls provider; prebuilt NASM objects keep Windows builds working without NASM | ISC AND (Apache-2.0 OR ISC) |

Transitive additions (reqwest, hyper, h2, rustls-platform-verifier, …) are
licence-checked by `cargo deny check licenses` in CI. `tiny_http` (already a
workspace dependency) serves the mock PACS in tests — no new async test
stack.

**Native build.** `dicom-web`'s reqwest uses rustls with the **aws-lc-rs**
provider, so `aws-lc-sys` compiles AWS-LC from C/assembly on every platform
(the workspace's `ureq` uses rustls-with-ring; this adds a second, larger
native crypto build). On Linux/WSL and CI it builds cleanly. On the dev
Windows machine the `aws-lc-sys` C build fails under cargo with MSVC
`D8050` while the *identical* `cl.exe` command succeeds from a `cmd` prompt
— Kaspersky interfering with cargo's child processes (the L39 family, which
already blocks `cargo test` there), on an old MSVC 14.33 toolchain. Build
and test in WSL (as the rest of the project does); a Windows desktop build
likely needs the build directories excluded from Kaspersky or a newer MSVC
(neither verified here). The `prebuilt-nasm` feature removes the separate
NASM requirement for the assembly step. This is an environment constraint,
not a client defect; it is recorded here so the dependency's cost is
visible. **Decision (2026-11-18, user):** accept the WSL-only build constraint
(consistent with L39); a ring-based transport (fork `dicom-web` or a ureq-based
client) to remove `aws-lc-rs` and restore the Windows build is a possible
follow-up, not pursued now.

## 7. PR split (Stage 32, size XL → four PRs)

| PR | What | Criteria |
|---|---|---|
| **1 (this)** | `DicomWebRepository` (QIDO scan, WADO load, LPS, auth, timeouts, errors), composite integration, mock-HTTP tests incl. pixel match | 32.1-a…c |
| 2 | DIMSE C-FIND/C-MOVE/C-ECHO (`dicom-ul`), C-STORE SCP into the workspace; send-off host-only | 32.3-a…b |
| 3 | STOW-RS of de-identified SEG/SR + agent query/retrieve commands (schemas, `commands.json`, docs, contract tests, privacy scan) | 32.2-a…b, 32.4-a…b |
| 4 | Orthanc-in-Docker CI integration: query → retrieve → open, pinned-phantom pixel match against the real PACS, STOW validated by highdicom; graceful skip without Docker | 32.5-a…b |

32.1-b's "pinned phantom retrieved from Orthanc" closes in PR 4; PR 1
proves the pixel path against a mock PACS serving the same Part 10 bytes
the file reader opens, voxel by voxel.

## 8. Validation plan (PR 1)

Mock PACS = `tiny_http` on `127.0.0.1:0` in a test thread, serving canned
QIDO JSON and WADO multipart built from the run-time synthetic phantom
(`tests/common`, no committed data). Integration tests
(`crates/ferrum-io/tests/dicomweb.rs`):

| Test | Covers |
|---|---|
| scan parses QIDO series into descriptors (filters, includefield seen by the server) | 32.1-a |
| load retrieves, orders and reorients; voxel-for-voxel equal to the file reader on the same phantom (axial) | 32.1-a, 32.1-b (mock) |
| a sagittal series comes out in LPS, equal to the file reader (L23) | 32.1-a |
| empty QIDO result → `NothingFound`; empty WADO stream → `NothingFound` | 32.1-c |
| connection refused, HTTP 500, 401 → `Io`, no panic | 32.1-c |
| HTML instead of DICOM JSON → `Corrupt` | 32.1-c |
| slow server + short deadline → `Io` (timed out) | 32.1-c |
| basic/bearer Authorization header sent; wrong credentials → 401 → `Io` | 32.1-c |
| file paths → `NothingFound`; malformed URLs → `Unsupported`; composite opens both files and URLs | 32.1-a |
| cancellation → `Cancelled` | 32.1-c |

Unit tests in `web.rs`: URL parsing (schemes, port, filters, reserved
parameters, userinfo refusal, query stripped from errors), descriptor id.

Integration against a real Orthanc (the pinned phantom of 32.1-b, STOW,
highdicom) is PR 4.

## 9. PR 3 as built — STOW-RS send-off and the agent surface (32.2, 32.4)

### STOW-RS, host-only (`dicom/stow.rs`)

`DicomWebStow` stores DICOM objects to a PACS over `dicom-web`'s STOW API
(`store_instances`, POST `{service-root}/studies`). It is **host-only**:
called from a person's explicit action in a host, never registered as an
agent command (ADR 0021 §5, criterion 32.2-b). It shares the read-only
repository's request deadline and error mapping (`web::block_on_pacs`,
`web::map_web_error`, L23) and the same auth posture (basic/bearer on the
client, never in a URL).

- `store(target, objects, progress)` sends Part 10 objects;
  `store_files(target, paths, progress)` is the **export → STOW hand-off**:
  the Stage 31 exporter ([`deidentification.md`](deidentification.md)) writes
  the de-identified `segmentation.dcm` / `measurements.dcm`
  (`DicomExportOptions::deid`), and the host sends those files. The deid
  profile is the sole privacy gate — what leaves FERRUM is already
  de-identified; STOW adds no identifiers.
- The response is checked **per instance** (PS3.18 §10.5.3, Store Instances
  Response Module, Annex I): `StowReport` lists each referenced instance as
  `Success` (Referenced SOP Sequence, 0008,1190), `Warning` (0008,1199, with
  Warning Reason 0008,1196) or `Failure` (Failed SOP Sequence 0008,1198, with
  Failure Reason 0008,1197). `all_stored()` is false when any instance
  failed. (The 0.10 DICOM dictionary names tag 0008,1190 `RETRIEVE_URL`; its
  value is the STOW success sequence — the code uses the dictionary constant
  and comments the quirk.)
- A non-success HTTP status (e.g. 409, one or more instances rejected) is a
  `RepositoryError::Io` naming the host and status; `dicom-web` 0.6 does not
  expose the 409 body, so per-instance failure detail is available on the
  200/202 response path. An empty input is `NothingFound`; a slow PACS times
  out (`Io`). Tests: `tests/dicomstow.rs` (mock PACS — a de-identified export
  arrives with no identifiers, per-instance success/warning/failure,
  409/auth/timeout). Real Orthanc + highdicom validation is PR 4.

### Agent query/retrieve/verify (`ferrum-agent`, 32.4)

The agent gains three **read-only** commands, thin over the repositories:
`pacs query` (QIDO-RS / C-FIND), `pacs retrieve` (WADO-RS into a workspace)
and `pacs echo` (C-ECHO). Send-off (STOW/C-MOVE/C-STORE) is absent from the
command set; `pacs retrieve` refuses a `dicom://` URL (C-MOVE is host-only).
A PACS-sourced workspace records the URL as its source and keeps no local
patient files (`Workspace::create_remote`); each load re-retrieves in memory
(ADR 0021 §2). Query output is pseudonymised; the SSRF allowlist
(`network.pacs`) and the privacy scan are described in
[agent-cli.md](agent-cli.md) §PACS. Tests: `ferrum-agent/tests/pacs.rs`
(mock PACS, CLI = MCP, pseudonymisation, SSRF refusals).

## 10. PR 4 as built — Orthanc-in-Docker CI + highdicom (32.5, closes #76)

PR 4 checks the client against a **real PACS**, not a mock: query → retrieve →
open with a voxel-for-voxel pixel match, and a STOW'd SEG validated by
highdicom. It closes the Orthanc clause of 32.1-b ("a pinned phantom retrieved
from Orthanc") and criteria 32.5-a…b.

### Orthanc as a separate AGPL service

Orthanc's DICOMweb plugin is **AGPL**, so Orthanc runs as a **separate Docker
service**, reached over HTTP only — never linked into FERRUM (ADR 0021 §6,
constitution licences; FERRUM stays MIT OR Apache-2.0). The CI job `orthanc`
(`.github/workflows/ci.yml`) starts the pinned image
`orthancteam/orthanc:26.11.1` (Orthanc 1.13.1, DICOMweb plugin 1.24) as a
GitHub Actions **service container**. The plugin is bundled and enabled by
default; the job sets `DICOM_WEB_PLUGIN_ENABLED=true`,
`ORTHANC__DICOM_WEB__ROOT=/dicom-web/`, authentication on with the default
`orthanc`/`orthanc` user (`ORTHANC__REGISTERED_USERS`). The job runs on the
runner host, so the service port maps to `localhost:8042` and the DICOMweb
endpoint is `http://localhost:8042/dicom-web` (`FERRUM_ORTHANC_URL`). A curl
loop waits for `/dicom-web/system` before the tests run. highdicom (MIT) is
installed in the job's Python layer; it is a test dependency, never linked into
Rust.

### Rust integration (`crates/ferrum-io/tests/orthanc.rs`)

| Test | Covers |
|---|---|
| `query_retrieve_open_matches_the_phantom_voxel_for_voxel` — STOW a run-time CT phantom to Orthanc, QIDO-query it, WADO-retrieve and open it; the volume equals the file reader's voxel for voxel (absolute phantom gradient through the Modality LUT), and the composite (viewer path) opens it identically | 32.5-a, 32.1-b |
| `stow_of_a_deidentified_seg_reports_the_success_sequence` — STOW a de-identified SEG/SR export; `StowReport` reports both instances as `Success` | 32.5-a, 32.2-a |
| `an_absent_study_is_nothing_found` — a query matching nothing is `NothingFound`, never a panic | 32.5-a, 32.1-c |

**STOW tag confirmation (the PR 3 quirk).** `stow::parse_response` reads the
success sequence only from tag **(0008,1190)**, which the dicom-rs 0.10
dictionary names `RETRIEVE_URL` (§9). A green `count(Success) == 2` against a
real Orthanc confirms Orthanc reports its stored instances under (0008,1190):
were it another tag, the success count would be 0 and the test would fail. The
Python layer independently reads the raw STOW JSON's `00081190` sequence. No
`parse_response` change was needed — the tag is confirmed, not fixed. (Confirmed
by CI; the dev machine has no Docker, so locally the test skips — see below.)

### Python highdicom validation (`scripts/test_orthanc.py`)

A self-contained round-trip, in the `test_deid_oracle.py` pattern (venv, skip
without deps): generate a CT phantom with identifiers (pydicom) → STOW it to
Orthanc → `ferrum-cli` opens it, segments a blob and exports a de-identified
SEG/SR (`export bundle --deidentify basic`, Stage 31) → STOW the export back →
**retrieve both objects over WADO-RS** → validate what comes back with
highdicom: the SEG as a `Segmentation` (binary, contiguous segment numbers,
labels/algorithm, non-empty frames, orthonormal geometry, referenced source
images) and the SR as a `Comprehensive3DSR` with a TID 1500 measurement report
(segment volume measured). A third test asserts none of the source identifiers
leaks into the retrieved SEG and that `PatientIdentityRemoved == YES`. The STOW
response's `00081190` success sequence is checked to reference both exported
SOP Instance UIDs.

### Graceful skip (32.5-b) and run-time data

Both layers skip when no PACS is reachable. Rust: `FERRUM_ORTHANC_URL` wins
(CI sets it); otherwise a 500 ms TCP probe of `127.0.0.1:8042` decides — absent
Orthanc, each test prints a skip note and returns (passes). Python:
`pytest.importorskip` for pydicom/highdicom/requests/numpy, a `ferrum-cli`
lookup, and an Orthanc `/system` probe; an explicitly configured but unreachable
`FERRUM_ORTHANC_URL` is a failure (CI must catch a broken service), the default
probe skips. All DICOM is generated at run time (`tests/common`, pydicom); **no
patient or volume data is committed**.

**Local verification.** The dev machine's Docker Desktop does not start (and
WSL has no Docker Engine), so the integration runs in CI, where the service
container is up. Locally the skip behaviour is verified: the three Rust tests
pass by skipping (`cargo test -p ferrum-io --test orthanc`, no PACS) and the
Python module skips (`pytest scripts/test_orthanc.py`, no pydicom/highdicom).
The pixel-match, STOW-tag and highdicom assertions are exercised by the CI
`orthanc` job against the real PACS.