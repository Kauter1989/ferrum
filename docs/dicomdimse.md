# DIMSE client — C-FIND/C-MOVE/C-ECHO VolumeRepository (Stage 32, PR 2)

- **Spec:** `dev_plan.md` Stage 32, criteria 32.3-a…b (#76)
- **Decision:** [ADR 0021](decisions/0021-pacs-client.md)
- **Companion:** [dicomweb.md](dicomweb.md) (PR 1, QIDO/WADO)
- **Status:** PR 2 implements 32.T2

FERRUM opens studies a site keeps in a **classic PACS** (no DICOMweb) over the
DICOM upper-layer protocol (DIMSE). The client is a `VolumeRepository`
implementation in `ferrum-io`
(`crates/ferrum-io/src/dicom/dimse/`, `DicomDimseRepository`) built on
[`dicom-ul`](https://crates.io/crates/dicom-ul) 0.10 (dicom-rs, MIT OR
Apache-2.0). It queries with **C-FIND**, verifies with **C-ECHO**, and
retrieves with **C-MOVE**, receiving the move's sub-operations through a
**C-STORE SCP**.

## 1. Repository fit — a second PACS repository beside DICOMweb

`VolumeRepository` (`crates/ferrum-domain/src/repository.rs`) is unchanged.
`DicomDimseRepository` is a separate implementation beside
`DicomWebRepository`, not a method on it, for three reasons:

- **Different transport, same port.** DICOMweb is HTTP/JSON (`dicom-web`,
  async); DIMSE is the binary upper-layer protocol (`dicom-ul`, sync). They
  share no client code, but both express a PACS query as a path-shaped URL and
  feed the **same** assembly pipeline (§3). One port serves files, DICOMweb and
  DIMSE, so `CompositeRepository`, the viewer, the view host and the CLI open a
  classic PACS study with no app-layer change.
- **Different descriptor namespace.** A DIMSE descriptor's id is
  `{SeriesInstanceUID}#d:{host[:port]}` (the `#d:` suffix keeps it apart from
  DICOMweb's `#w:` and the file readers' geometry suffix); `format` is
  `"DIMSE"` (= `name()`), so the composite's format dispatch routes
  `load`/`volumes` back to this repository.
- **The send-off boundary is per-repository.** C-MOVE/C-STORE are host-only
  (§5); keeping them in their own repository makes that boundary explicit.

`CompositeRepository::default()` gains `DicomDimseRepository::new()` after the
DICOMweb repository. The repositories coexist because each declines what is
not its own: a path without a `dicom://` scheme is `NothingFound` here (a file
path or a DICOMweb URL), and a `dicom://` URL is `NothingFound` for the file
readers.

### URL grammar

```text
dicom://host[:port]/CALLED-AE[?keyword=value&…]
```

- `host[:port]` — the PACS node's TCP address; port **104** when omitted.
- `CALLED-AE` — the PACS application entity title (the called AE); `ANY-SCP`
  when omitted.
- Query parameters are C-FIND matching attributes from the **same**
  keyword→tag table as DICOMweb (`PatientID`, `StudyInstanceUID`,
  `SeriesInstanceUID`, `StudyDate`, `Modality`, …, via
  `dicom_dictionary_std::tags`). The URL helpers (`keyword_tag`,
  `percent_decode`, `split_authority`, `safe_url`) are shared with
  `dicom/web.rs` — one implementation (L23).

The calling AE title, the C-STORE SCP address and the move destination are
**configured on the repository**, never parsed from a URL (they are this
node's identity, not the query): `with_calling_ae_title`,
`with_store_scp_addr`, `with_move_destination`. **No userinfo** — credentials
in a URL would leak into logs and error strings; DIMSE user identity is a
follow-up.

### Descriptors

- `id`: `{SeriesInstanceUID}#d:{host[:port]}` — stable, unique per server, no
  patient identifiers.
- `sources`: one entry, the retrieval URL
  `dicom://host[:port]/CALLED-AE?StudyInstanceUID={study}&SeriesInstanceUID={series}`.
  `load` parses it back with the same URL parser; the descriptor is
  self-contained, so a later load needs no re-query.
- `dims`: Columns × Rows × NumberOfSeriesRelatedInstances when C-FIND returns
  them; `0` for what the server did not return.
- `description`: series description (or "Unnamed series"), instance count and
  modality — series-level text only, never patient name/ID.

## 2. C-FIND flow (`scan`) and C-ECHO (`echo`)

`scan` resolves every URL to **series-level** match identifiers:

1. `StudyInstanceUID` among the filters → one **series-level** C-FIND
   (QueryRetrieveLevel = SERIES) within that study.
2. Otherwise → a **study-level** C-FIND (level = STUDY), then a series-level
   C-FIND per returned study. (The Study Root Information Model requires a
   study UID to query series; this is the classic two-level query.)

Each C-FIND proposes the *Study Root Query/Retrieve Information Model – FIND*
abstract syntax with Explicit/Implicit VR Little Endian, sends the command
(always Implicit VR LE, PS3.7) plus the identifier data set in the negotiated
syntax, and collects the pending (0xFF00/0xFF01) match identifiers until the
success status (0x0000). Each match becomes a descriptor; matches without a
SeriesInstanceUID are skipped. All URLs empty → `NothingFound`. Cancellation
is checked between URLs and between matches.

`echo(target)` is a standalone **read-only** verification: it establishes an
association proposing the *Verification* SOP Class, exchanges a C-ECHO and
checks the success status. It is not part of `scan`/`load`; a host (or, in
PR 3, an agent) calls it to test reachability.

## 3. C-MOVE flow (`load`) — the shared LPS pipeline

1. Parse `sources[0]` into address + called AE + study UID + series UID.
2. Bind a [`StoreScp`] C-STORE SCP (§4) and start its accept loop.
3. Establish a C-MOVE association (*Study Root … – MOVE*), send the C-MOVE-RQ
   naming the **move destination** AE title plus a series-level identifier
   (StudyInstanceUID + SeriesInstanceUID).
4. The PACS opens a **new** association to the SCP and sends the instances as
   C-STORE sub-operations; the SCP collects them. The C-MOVE association
   reports pending sub-operation counts until success.
5. Stop the SCP, take the received instances (whole Part 10 objects in
   memory), build `SliceHeader`s (`SliceHeader::from_object`), group with
   `dicom/series.rs`, keep the requested series, order, and
   `assemble::assemble_with(headers, ordered, progress, decode)` where
   `decode` decodes frames from the received objects
   (`assemble::decode_object_frames`). This is the **same** function the file
   reader and the DICOMweb reader call: ordering, the Modality LUT, the
   lossless HU grid and the `to_canonical` LPS reorientation are one
   implementation for every source (L23) — a retrieved study is
   voxel-identical to the same study opened from files.
6. Metadata: `header::describe` + `header::study_info` of the first instance
   plus Slices / Slice spacing / Ordering attributes, as the file reader adds
   them; `source` is the retrieval URL.

A move that reports success but delivers no instances is `NothingFound`.
`volumes()` keeps the port default (the whole series), as DICOMweb PR 1.

## 4. C-STORE SCP (`StoreScp`)

`StoreScp::bind(addr, ae_title, receive_dir)` binds a TCP listener (port `0`
for an ephemeral port; `local_addr()` reports the bound address). `spawn()`
runs the accept loop on a background thread; `ScpHandle::finish()` stops it
and returns the received instances. For each storage association it:

- negotiates with `promiscuous` mode (accepts the storage SOP classes the move
  source proposes) and the transfer-syntax registry's syntaxes;
- answers C-ECHO-RQ (the source may verify) and receives C-STORE-RQ command +
  data set, rebuilding the Part 10 file meta information;
- keeps each instance **in memory** and, when `receive_dir` is set, also
  writes it there as `{SOPInstanceUID}.dcm` — a persistent storage SCP, e.g.
  into a workspace receive area (32.3-a "receives the study into the
  workspace");
- sends a C-STORE-RSP (success) per instance.

**The move destination must be reachable.** C-MOVE names a destination AE
title; the PACS resolves it to a network address from its own configuration.
A host therefore configures the PACS's move-destination AE with the SCP's
bound address (a fixed `with_store_scp_addr` port, or the ephemeral
`local_addr()`). The mock PACS in the tests is told the address directly.

## 5. Send-off boundary — host-only (32.3-b)

C-MOVE and the C-STORE SCP **send data off the workstation** and open a
network listener: per [ADR 0021 §5](decisions/0021-pacs-client.md) they are
**host-only**, called from a person's explicit action in a host, and are
**never registered as agent commands**. `DicomDimseRepository::load` (C-MOVE)
is therefore a host operation. C-ECHO (`echo`) and C-FIND (`scan`) are
**read-only** and may become agent commands with pseudonymised output in
PR 3 (32.4); PR 2 registers no agent commands at all. The boundary is
enforced at the agent command layer (PR 3 wires only `echo`/`scan`), not by
the repository, which a host composes freely.

## 6. Timeouts, errors, privacy

**Timeouts.** `dicom-ul`'s sync API uses blocking sockets; the repository sets
`connection_timeout`, `read_timeout` and `write_timeout` to one configured
deadline (default 30 s, `with_timeout`). An expired deadline is a
`RepositoryError::Io`, never a hang. The C-STORE SCP socket has its own 30 s
deadline.

**Error mapping** — every failure is a `RepositoryError`, never a panic
(`command::map_dimse_error`):

| Condition | `dicom-ul` / boundary | `RepositoryError` |
|---|---|---|
| DNS/connect/refused | `Connect`, `ToAddress` | `Io` (connection) |
| association rejected | `Rejected` (A-ASSOCIATE-RJ) | `Io` (rejected) |
| association aborted | `Aborted`, mid-message A-ABORT | `Io` (aborted) |
| deadline expired | socket timeout | `Io` (timed out) |
| no accepted presentation context | `NoAcceptedPresentationContexts` | `Io` |
| connection closed / wire error | `ConnectionClosed`, `Wire*`, `*Pdu` | `Io` |
| unexpected PDU / malformed command or data | `UnexpectedPdu`, `ReadError` | `Corrupt` |
| C-FIND/C-MOVE/C-ECHO failure status | DIMSE status ≠ 0 | `Io` (operation + status) |
| empty query / move with no instances | — | `NothingFound` |
| cancelled | progress sink | `Cancelled` |

Messages name the host and the failure kind. **Query values never appear in
errors** (a `PatientID` filter is an identifier): the URL is stripped to
scheme + authority + path before it is quoted (`safe_url`), and the raw
`dicom-ul` error Display is dropped (it could carry PDU detail).

**Privacy.** Retrieved pixel data stays **in memory** by default; the client
writes no patient data to disk unless a host opts into a receive directory
(`with_receive_dir`), and then only on the local workstation. Descriptors'
public text carries series-level information only. `SeriesMetadata`
identifiers follow the file readers' rule (internal to FERRUM, pseudonymised
in agent outputs, PR 3).

## 7. Dependencies

| Crate | Version | Why | Licence |
|---|---|---|---|
| `dicom-ul` | 0.10 (resolves 0.10.0) | DIMSE association + PDU layer; **default features** (sync TCP, no TLS) | MIT OR Apache-2.0 |
| `dicom-encoding` | 0.10 | `TransferSyntax` / `TransferSyntaxIndex` (now a direct dep) | MIT OR Apache-2.0 |
| `dicom-transfer-syntax-registry` | 0.10 | transfer-syntax lookup (now a direct dep) | MIT OR Apache-2.0 |

`dicom-ul` 0.10.0 is the release compatible with the workspace's dicom-rs
0.10.0 object types (0.10.1 would bump `dicom-encoding`/`-transfer-syntax-registry`
to 0.10.1; cargo keeps 0.10.0 to match the lock). The DIMSE service commands
(C-ECHO/C-FIND/C-MOVE/C-STORE) are built in `dimse/command.rs` from
`dicom-object` command sets — `dicom-ul` ships the association/PDU layer only;
the published `dicom-echoscu`/`-findscu`/`-movescu`/`-storescp` crates are
binary-only (`has_lib = false`) and served as the protocol reference.

**No new native crypto.** `dicom-ul`'s default features pull neither rustls nor
tokio, so **no `aws-lc-rs`** is added (unlike `dicom-web`'s reqwest). DIMSE TLS
(the `sync-tls` feature, which would add rustls/aws-lc) is a follow-up, not
pursued here. The workspace's WSL-only build constraint (L52, dicomweb.md §6)
is unchanged: `web.rs` still pulls aws-lc-rs, so all cargo operations run in
WSL.

## 8. Validation plan (PR 2)

Mock PACS = a `dicom-ul` server on `127.0.0.1:0` in test threads, speaking
C-ECHO/C-FIND/C-MOVE; for C-MOVE it opens a C-STORE SCU association back to the
repository's SCP and sends the run-time synthetic phantom (`tests/common`, no
committed data). Integration tests (`crates/ferrum-io/tests/dicomdimse.rs`):

| Test | Covers |
|---|---|
| C-ECHO ok; failure status → `Io`; connection refused → `Io` | 32.3-a |
| C-FIND series-level → descriptors (no patient id); global study→series query; empty → `NothingFound`; file paths → `NothingFound` | 32.3-a |
| C-MOVE retrieve + assemble, voxel-for-voxel equal to the file reader (axial); absolute phantom pixel check | 32.3-a |
| C-MOVE into a configured receive dir (4 Part 10 files); default writes nothing | 32.3-a, 32.3-b |
| a sagittal series comes out in LPS, equal to the file reader (L23) | 32.3-a |
| move with no instances → `NothingFound`; move failure status (0xA801) → `Io` | 32.3-a |
| association reject → `Io`; slow server + short deadline → `Io` (no hang); move abort → `Io` | 32.3-a |
| query values never leak into errors; malformed URL names the keyword not the value | 32.3-b |
| cancellation → `Cancelled`; composite opens files and `dicom://` URLs | 32.3-a |

Integration against a real Orthanc (the pinned phantom of 32.1-b, STOW,
highdicom) is **PR 4**; agent query/retrieve commands and the privacy scan are
**PR 3**.

## 9. PR 3 as built — the agent surface (32.4)

PR 3 wires the read-only DIMSE operations into the agent: `pacs query` calls
`DicomDimseRepository::scan` (C-FIND) for a `dicom://` URL, and `pacs echo`
calls `DicomDimseRepository::echo` (C-ECHO). Output is pseudonymised and the
URL is checked against the operator's PACS allowlist (`network.pacs`) before
any association — the SSRF boundary ([agent-cli.md](agent-cli.md) §PACS).

**C-MOVE and the C-STORE SCP stay host-only.** `pacs retrieve` uses WADO-RS
(DICOMweb) only and refuses a `dicom://` URL with a hint that DIMSE retrieval
is a host operation; no agent command calls `load` (C-MOVE) or binds a
`StoreScp`. The send-off boundary of §5 is enforced at the agent command
layer: the command set contains no STOW/C-MOVE/C-STORE (a test checks the
command list, the MCP tools and `commands.json`). Tests:
`ferrum-agent/tests/pacs.rs`.
