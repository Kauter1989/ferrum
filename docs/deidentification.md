# DICOM de-identification — design (Stage 31, #75)

- **Spec:** `dev_plan.md` Stage 31 (criteria 31.1–31.5). **ADR:**
  [0020](decisions/0020-deidentification-profile.md). **Mode:** gated stage
  (G1 approved 2026-11-17; G2 this design; per-PR G3).

FERRUM gains a standard de-identification: the PS3.15 Annex E **Basic
Application Level Confidentiality Profile** with its options, **Clean Pixel
Data** (burned-in text), and test oracles against established anonymisers. It
extends the export's existing pseudonymisation (`dicom/export.rs`) into a
recognisable standard profile.

## 1. The profile engine (PR 1, criteria 31.1–31.2)

### Where it lives

`crates/ferrum-io/src/dicom/deid.rs` — a transform on a
`dicom_object::InMemDicomObj` (the crate already depends on `dicom_object` and
`dicom_dictionary_std`). `ferrum-io` is the only DICOM parser; the
de-identification is a DICOM transform beside the reader and the export. A
`ferrum-app` use case (`Viewer::deidentify_export` or a free function over the
repository) wires it; the CLI/agent and the view host stay thin.

### The table is data, not code

`crates/ferrum-io/src/dicom/deid_table.csv` holds one row per attribute of
PS3.15 Table E.1-1:

```
tag,keyword,action,option
(0010,0010),PatientName,D,
(0010,0020),PatientID,Z,
(0010,0030),PatientBirthDate,Z,
(0008,0020),StudyDate,R,retain_dates
(0008,0018),SOPInstanceUID,U,
(0008,1030),StudyDescription,C,clean_descriptors
(0010,1010),PatientAge,K,retain_characteristics
...
```

- `action` is the standard's: `D` replace with a non-empty consistent value,
  `Z` zero-length, `X` remove, `U` consistent pseudonymous UID, `C` clean
  descriptors, `K` keep, `R`/`Z/D` option-dependent.
- `option` names the option that governs an option-dependent row (empty for a
  fixed row).
- The table is **sourced from the standard once** (PR 1 task: transcribe Table
  E.1-1, cross-check against deid's Basic Profile data) and committed. A
  generator reads it into a static structure; a test fails when the table and
  the generator disagree, and a pinned fixture fixes the action of every row.

### Applying the profile

`deidentify(obj: &mut InMemDicomObj, profile: &Profile, uids: &UidMap)`:

- For each table row present in the object, apply the action:
  - `D` → a consistent non-empty placeholder (e.g. `PatientName` → `ANONYMOUS`,
    `PatientID` → a salted hash);
  - `Z` → zero-length value;
  - `X` → remove the element;
  - `U` → `uid_from` salted hash (the export's derivation), consistent per UID
    within the run via `UidMap`;
  - `C` → clean free text (search for identifier patterns — names, IDs, dates —
    and replace; conservative, never leaves a recognisable identifier);
  - `K` → keep;
  - `R`/`Z/D` → per the option (e.g. dates shifted by a consistent per-patient
    offset under *retain longitudinal dates modified*, else zero-length).
- Add the de-identification method attributes the standard requires
  (`DeidentificationMethod`, `PatientIdentityRemoved` = `YES`, the profile and
  options coded).
- Keep the pixel data and the attributes the profile keeps.

### Options (31.2)

`ProfileOptions` flags: `retain_dates_modified`, `retain_uids`,
`clean_descriptors`, `retain_device_identity`, `retain_institution_identity`,
`retain_patient_characteristics`. Each changes exactly the rows the table marks
option-dependent (pinned by a fixture per option). Default: no options (the most
conservative). An unknown option is refused.

### Tests

- A synthetic DICOM carrying **every** Table E.1-1 attribute (generated at run
  time, `crates/ferrum-io/tests/common`); the output matches the profile's action
  for each row (a pinned fixture).
- UID consistency within a run; `retain_uids` keeps them.
- Each option changes exactly its rows.
- The de-identified object reads back through FERRUM's own reader.

## 2. Clean Pixel Data (PR 2, criterion 31.3) — as built

Burned-in annotation — names, IDs and dates rendered into the pixels —
survives every attribute-level transform of §1. Clean Pixel Data masks it.
**Masking is a pixel transform on the data, never image formation**: the
renderers draw a masked copy exactly as they draw the original, so the
rendering rule (`slice.wgsl` ↔ `cpu/slice.rs`, `volume.wgsl` ↔
`cpu/raycast.rs`) is untouched.

### Region masking of the pixel data (31.3-a)

`crates/ferrum-io/src/dicom/clean_pixel.rs` — a DICOM transform beside the
reader, the export and `deid.rs`:

- `PixelRegion` (clean_pixel.rs:40) is a 0-based, half-open rectangle in
  stored pixel coordinates (`x` along columns, `y` along rows), clipped to
  the image by `clamped`.
- `clean_pixel_data(obj, spec)` (clean_pixel.rs:122) replaces the pixels of
  the configured regions with a solid fill (`CleanPixelSpec::fill`, a stored
  bit pattern; default `0`), the rest of the image unchanged. With no
  configured region and `BurnedInAnnotation = YES` it masks the **whole
  image** — the conservative default, since the text's location is unknown
  (ADR 0020 decision 6). With no region and no flag it is a reported no-op
  (`masked: false`). After masking, the flag is set to `NO`.
- Supported pixel data: native monochrome 8/16-bit and RGB 8-bit (planar
  configuration 0 or 1), single- or multi-frame. Encapsulated (compressed)
  data and other layouts are refused with `CleanPixelError::Unsupported` —
  a region that cannot be masked is an error, never a silent pass. On error
  the object is left as it was.
- `sources_burned_in(paths)` (clean_pixel.rs:154) reads the flag from a
  series' files (headers up to the pixel data) for the render-side default.
- Tests: `crates/ferrum-io/tests/clean_pixel.rs` (16) — a phantom with a
  bright "text" block in a known region, checked pixel by pixel (region
  replaced, rest unchanged), the flag default, clipping, 8/16-bit, RGB both
  planar modes, multi-frame, the refusals, and a masked file read back
  through FERRUM's own reader.

### Masked renders (31.3-c)

The loaded volume is in the canonical LPS frame, so the render-side mask is
a pure voxel transform on that grid:

- `Volume::masked_copy(boxes, fill)` (`crates/ferrum-domain/src/volume.rs:302`)
  returns a copy with the voxels of the half-open `VoxelBox`es replaced by
  the fill (clipped to the grid; geometry, spacing and range kept). It lives
  in `ferrum-domain` because `ferrum-app` must not depend on `ferrum-io`
  (layering): the DICOM-object transform is in `ferrum-io`, the pure voxel
  transform is in the domain both share. `MASK_FILL = 0` is the range
  minimum — black/neutral under a typical window.
- `Viewer::render_masked(boxes, fill, draw)`
  (`crates/ferrum-app/src/viewer.rs:817`) is the use case: it installs the
  masked dataset for the duration of `draw` (a fresh revision, so the GPU
  sink uploads it) and restores the original afterwards.
- Agent: `view slice --masked [--mask MIN MAX]…`
  (`crates/ferrum-agent/src/commands/view.rs`, `mask_plan`:268). Explicit
  `masks` boxes (the `stats` box convention) mask; else the flag masks the
  whole volume; else nothing. The sidecar's `mask` block always reports
  `requested/applied/source/boxes/fill` — never silently passed.
- CLI: `--masked` and repeatable `--mask MIN MAX` (implies `--masked`),
  `crates/ferrum-cli/src/cli.rs`; parse tests (L8).
- View host: `/api/frame.png?masked=1[&box=i0,j0,k0,i1,j1,k1…]`
  (`crates/ferrum-view/src/session.rs:270` `frame_masked`,
  `server.rs` `masked_frame`). Both backends draw the masked copy; the
  `X-Ferrum-Mask` header reports the effective boxes; the frame is cached
  per revision + mask, the unmasked frame stays untouched. This is the only
  `ferrum-view/1` change (the masked-render flag, within the §Out of scope
  allowance).
- Tests: `crates/ferrum-agent/tests/masked_render.rs` (4, pixel-exact region
  vs the rest), `crates/ferrum-app/tests/masked_render.rs` (restore
  invariant), `crates/ferrum-view/tests/view_host.rs` (5, incl. GPU/CPU
  masked parity). Visual: `.claude/skills/ferrum-visual-check/phantom_masked.py`
  (1:1 and 8×, flag default, unflagged skip; PNGs opened and looked at).

### OCR bridge (31.3-b, optional)

`crates/ferrum-engines/src/ocr.rs` — a new bridge kind of `ferrum-engine/1`
(`docs/engine-protocol.md` §7): `capabilities.ocr` and `POST /v1/ocr` (PNG
bytes → `{ "regions": [{x,y,w,h}] }` in image pixel coordinates). No OCR in
Rust, no weights bundled (constitution).

- `OcrBridge::info` verifies the protocol and the capability; `detect` sends
  the render and parses the regions. `check(bridge, png)` returns
  `OcrOutcome::Skipped` (no bridge), `Detected` (regions, possibly none) or
  `Failed` (unreachable / wrong protocol / bad answer) — each with a report
  JSON, **never silently passed**.
- Agent: `[network] ocr_bridge` config; `view slice --ocr` runs the check on
  the unmasked render and writes the sidecar `ocr` block. With `--masked`,
  detected regions extend the mask: `ocr_region_box`
  (`crates/ferrum-agent/src/commands/view.rs:347`) maps each region's
  bounding box through the render's pixel-to-voxel affine, rounded outwards
  (conservative), on the render's slice; the mask `source` is then `ocr`.
- Tests: `crates/ferrum-engines/tests/ocr.rs` (7) and
  `crates/ferrum-agent/tests/ocr_bridge.rs` (4), against a mock bridge.
- **Follow-up:** a production OCR bridge (the Python `bridges/` package or a
  third-party server). This PR delivers the interface and the skip/report
  behaviour; the engine itself is out of scope (no weights bundled).

## 3. Oracles and integration (PR 3, criteria 31.4–31.5)

- **Oracle diff test** (Python layer, CI `python-lint`/pytest): de-identify the
  same synthetic study with FERRUM and with **pydicom/deid** (MIT) and the
  **RSNA/CTP** profile; pin the expected agreement, name and justify differences.
  Generate DICOM at run time; commit no patient or volume data.
- **Export integration:** `export bundle --deidentify basic [--option …]` writes
  a de-identified DICOM export through the profile engine.
- **Agent integration:** the command gains the option with a JSON Schema,
  `commands.json`, docs and contract tests (CLI = MCP); the privacy scan passes.

### As built (PR 3)

- **Variant A — the orchestration lives in `ferrum-io`** (ADR 0018: the agent
  does not link `ferrum-app`; the `Exporter` port is planned, today the DICOM
  SEG/SR export is called directly, the existing pattern in
  `ferrum-agent/commands/export.rs`). `DicomExportOptions.deid:
  Option<ProfileOptions>` carries the profile; `DicomExporter::new` overrides
  the copy flags (the objects are built with the source's own identifiers,
  dates and UIDs) and `write_object` de-identifies each object before writing
  (`deidentify_file`), the profile the **sole privacy gate**. The agent/CLI
  stay thin: `export bundle --deidentify basic [--deid-option …]` parses the
  options into `ProfileOptions` and passes them down; an unknown profile or
  option is refused.
- **File Meta Information replacement** (E.1.1 item 7) is `deidentify_file`'s
  responsibility (deid.rs): the media storage SOP class/instance UIDs follow
  the de-identified data set, the implementation identity becomes FERRUM's
  (a stable `2.25` derivation), the source's application entity titles and
  private meta information are removed (the titles kept only under *Retain
  Device Identity*); the transfer syntax and meta version are kept.
- **D-on-sequence fix:** the engine's `D` branch for a sequence used to collapse
  it to one empty item, which destroyed an exported SR `ContentSequence`
  (`(0040,A730)=D`). PS3.15 Note 2.1 — the action applies to the sequence *and
  all of its contents* — so `D` now recurses, keeping the structure
  (`apply_sequence`, deid.rs). Regression: a D sequence keeps every item and
  de-identifies its contents; the export test checks the SR ContentSequence
  survives.
- **Oracle diff** (`scripts/test_deid_oracle.py`, `# covers 31.4-a/b`): a
  synthetic DICOM series (run time, no patient data) is de-identified by FERRUM
  (`export bundle --deidentify basic`) and by **pydicom/deid** with a recipe
  expressing the Basic Profile core actions; the test pins the agreement on the
  core identifiers (PatientName anonymised, PatientID/BirthDate/Accession/
  Institution/ReferringPhysician removed) and names the documented difference
  (FERRUM's default also zeroes StudyDate and removes PatientAge, which a
  minimal deid recipe keeps). Skips without `deid` or a built `ferrum-cli`. The
  **RSNA/CTP** anonymizer (Java) is a best-effort follow-up; the deid oracle
  covers the core.

## PR split (size L/XL, confirmed at G2)

| PR | What | Criteria |
|---|---|---|
| 1 | Basic Profile engine + Table E.1-1 data + generator + options | 31.1–31.2 |
| 2 | Clean Pixel Data masking + masked renders (+ OCR bridge) | 31.3 |
| 3 | pydicom/deid + CTP oracles; export/agent integration | 31.4–31.5 |

PR 1 is foundational (the engine + table); PR 2 builds masking on it; PR 3 wires
the oracles and the export/agent surface.

## Validation plan

- Every Table E.1-1 row's action pinned by a fixture (31.1-b).
- The pydicom/deid + CTP diff test green (31.4-a).
- A masked render of burned-in text verified visually (31.3-c) — **done**:
  `phantom_masked.py` (1:1 and 8×, region replaced pixel-exact, rest
  unchanged) plus the Rust pixel-exact and GPU/CPU parity tests.
- The privacy scan over every output passes (31.5-a).
- Full preflight (WSL Ubuntu-24.04) before every push; the rendering rule
  untouched (masking is a pixel transform).

## Out of scope

A DIMSE router (#76); OCR weights or inference in Rust; a **production OCR
bridge** (PR 2 delivers the bridge interface and the skip/report behaviour;
the engine itself is a follow-up); de-identification of non-DICOM formats
(NIfTI has no standard profile — its sidecar pseudonymisation is separate);
any `ferrum-view/1` contract change beyond the masked-render flag.
