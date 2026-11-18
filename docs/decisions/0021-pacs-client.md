# 0021 — PACS client: DICOMweb/DIMSE in ferrum-io, send-off host-only

- **Status:** accepted (G1 spec approved 2026-11-16; G2 design)
- **Date:** 2026-11-18
- **Issue:** [#76](https://github.com/Kauter1989/ferrum/issues/76) (P2)
- **Spec:** `dev_plan.md` Stage 32; **design:** [dicomweb.md](../dicomweb.md)

## Context

Real sites keep studies in a PACS. Medical Harness needs to open a study from
a PACS (Demo 8 hand-off) and hand SEG/SR results back. FERRUM's data port
(`VolumeRepository`) is path-based: `scan(paths)` finds series, `load(series)`
retrieves one. The Rust ecosystem's DICOMweb client is `dicom-web` (dicom-rs,
MIT OR Apache-2.0); DIMSE is `dicom-ul`. Two constraints shape the design:

- **Layering.** `ferrum-io` is the only data crate; the domain port carries no
  I/O. A PACS client is a data source, so it is a `VolumeRepository`
  implementation beside the file readers — not a new port, not an app-layer
  service.
- **Privacy and licences.** Query/retrieve is read-only and may become agent
  commands with pseudonymised output (Stage 32.4). Send-off operations
  (STOW-RS, C-MOVE, C-STORE) move data off the workstation and must never be
  agent commands. Orthanc, the test PACS, ships its DICOMweb plugin under
  **AGPL**: it runs as a separate service, never linked into FERRUM
  (constitution licences; FERRUM stays MIT OR Apache-2.0).

## Decision

1. **The client lives in `ferrum-io` as `DicomWebRepository`, a
   `VolumeRepository` implementation; the trait is unchanged.** A PACS query
   is expressed as a path-shaped URL, so the existing port, the composite
   repository, the viewer and the CLI open a PACS study with no app-layer
   change:

   - `dicomweb://host[:port]/service-root?PatientID=…&StudyInstanceUID=…`
     (HTTP) and `dicomwebs://…` (HTTPS) are PACS queries; the query parameters
     are QIDO matching attributes from a fixed keyword→tag table.
   - A path that is not a `dicomweb(s)://` URL is `NothingFound` for this
     repository, so it composes safely with the file readers in
     `CompositeRepository` (which tries every repository and tolerates
     `NothingFound`). A malformed PACS URL is `Unsupported`, naming the
     problem — never the query values.
   - `SeriesDescriptor.sources` carries the WADO-RS retrieval URL of the
     series; `load` reads it back. The descriptor `id` is
     `{SeriesInstanceUID}#w:{host}` — stable, unique per server, and free of
     patient identifiers. No new field on the port is needed: the URL is the
     series' "file list".

2. **Retrieved data is assembled by the existing file-reader pipeline, in
   memory.** WADO-RS series retrieval returns whole Part 10 objects
   (metadata + pixel data in one multipart response). `load` builds
   `SliceHeader`s from them (`SliceHeader::from_object`, already used by the
   file reader), groups and orders them with `dicom/series.rs`, and decodes
   through one shared assembly function (`dicom/assemble.rs`) parameterised by
   a frame decoder: files decode from disk, PACS objects from memory
   (`dicom_pixeldata::PixelDecoder` works on both). Ordering, the Modality
   LUT, the lossless HU grid and the reorientation into the canonical LPS
   frame are therefore **the same code** for files and PACS — a retrieved
   study cannot drift from a downloaded one (L23). The alternative — spooling
   retrieved instances to a temporary directory and re-scanning it — was
   rejected: it writes patient data to disk and leaves descriptors pointing
   at files that a later volume switch would find deleted.

3. **`dicom-web` 0.6 is the HTTP client; the sync port is bridged with a
   per-operation current-thread tokio runtime.** `dicom-web` 0.6 is the
   release built on dicom-rs 0.10 (the workspace's version; 0.5 uses 0.9 and
   would duplicate the object types). Its API is async (reqwest); each
   `scan`/`load` operation blocks on its own small runtime. `dicom-web`
   exposes no timeout configuration, so every request is wrapped in a
   deadline at this boundary (`tokio::time::timeout`, default 30 s,
   configurable); an expired deadline drops the request and is reported as
   `RepositoryError::Io`. All `DicomWebError` variants map to
   `RepositoryError` (connection/HTTP/auth/timeout → `Io`, non-DICOM or
   malformed responses → `Corrupt`, empty result → `NothingFound`); nothing
   panics on a hostile server.

4. **Auth is basic or bearer, configured on the repository, never parsed
   from the URL.** `with_basic_auth(user, pass)` / `with_bearer_token(token)`
   feed `dicom-web`'s Authorization header. Credentials in a URL would leak
   into logs, error strings and recent-files lists, so the URL grammar has no
   userinfo part. Error messages name the host and the failure kind, never
   query values (a `PatientID` filter is an identifier) and never credentials.
   Host configuration of credentials arrives with the agent/host integration
   (PR 3); PR 1 exposes the repository API.

5. **Send-off is host-only; query/retrieve is the agent surface.** STOW-RS
   (32.2), C-MOVE and the C-STORE SCP (32.3) send data off the workstation:
   they are called only from a person's explicit action in a host, never
   registered as agent commands. Agent commands (PR 3) cover query and
   retrieve only — QIDO/WADO and C-FIND/C-ECHO — with pseudonymised ids and a
   passing privacy scan (32.4). PR 1 contains no send-off code at all;
   `dicom-web`'s STOW capability stays unused until the host boundary exists.

6. **Orthanc is a separate AGPL service, never linked.** Integration tests
   (PR 4) run Orthanc with its DICOMweb plugin in Docker as the test PACS;
   FERRUM talks to it over HTTP only. `dicom-web` and its tree are
   licence-checked (`cargo deny check licenses`): MIT OR Apache-2.0 and
   compatible permissive licences. Its reqwest uses rustls with the aws-lc-rs
   provider (`ISC AND (Apache-2.0 OR ISC)`); the `prebuilt-nasm` feature keeps
   Windows builds working without a NASM install. highdicom (MIT) validates
   STOW results in the Python test layer.

## Consequences

- `ferrum dicomweb://host/dicom-web?PatientID=…` opens a PACS study through
  the existing viewer; the composite repository gains `DicomWebRepository`
  by default. File opening is unchanged (file readers return `NothingFound`
  for URLs, the web reader for file paths).
- One assembly pipeline serves files and PACS; the assemble refactor is a
  decoder parameter, covered by the existing DICOM suite plus new mock-HTTP
  tests (scan, load, reorientation, pixel match against the file reader,
  errors, auth, timeout, cancellation).
- New dependencies: `dicom-web` 0.6, `tokio` (rt/net/time), `futures-util`,
  and transitively reqwest/hyper/rustls/aws-lc-rs. The tokio runtime is
  created per operation: FERRUM's hosts are synchronous, and the repository
  must not be driven from inside an ambient tokio runtime (documented).
- Retrieved pixel data stays in memory; no patient data is written to disk
  by the client, and no identifier appears in descriptors' public text or
  error strings beyond what the file readers already carry in metadata.
- Multi-volume switching over PACS (`volumes()` partitioning) and per-frame
  WADO retrieval (bandwidth optimisation) are follow-ups, not PR 1; the
  default `volumes()` (whole series) applies.
- Out of scope: a PACS server; WADO-URI; TLS termination beyond the client's
  HTTPS support; any rendering change (the client feeds the existing
  reader/viewer).