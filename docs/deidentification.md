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

## 2. Clean Pixel Data (PR 2, criterion 31.3)

- **Region masking:** the `BurnedInAnnotation` flag (`YES`) and a configured
  rectangular region mask replace the burned-in pixels (a solid fill). A test
  with burned-in text in a known region shows the region replaced and the rest
  of the image unchanged.
- **Masked renders:** `view slice --masked` and the host frame route apply the
  region mask before drawing. Masking is a pixel transform, not image formation
  (the rendering rule is untouched); a visual test fixes a masked render of a
  phantom with burned-in text.
- **OCR bridge (optional):** an out-of-process OCR check behind
  `ferrum-engine/1` (a new bridge kind, no OCR in Rust, no weights bundled).
  Without a bridge the step is skipped and reported, never silently passed. The
  bridge reports detected text regions; the mask covers them.

## 3. Oracles and integration (PR 3, criteria 31.4–31.5)

- **Oracle diff test** (Python layer, CI `python-lint`/pytest): de-identify the
  same synthetic study with FERRUM and with **pydicom/deid** (MIT) and the
  **RSNA/CTP** profile; pin the expected agreement, name and justify differences.
  Generate DICOM at run time; commit no patient or volume data.
- **Export integration:** `export bundle --deidentify basic [--option …]` writes
  a de-identified DICOM export through the profile engine.
- **Agent integration:** the command gains the option with a JSON Schema,
  `commands.json`, docs and contract tests (CLI = MCP); the privacy scan passes.

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
- A masked render of burned-in text verified visually (31.3-c).
- The privacy scan over every output passes (31.5-a).
- Full preflight (WSL Ubuntu-24.04) before every push; the rendering rule
  untouched (masking is a pixel transform).

## Out of scope

A DIMSE router (#76); OCR weights or inference in Rust; de-identification of
non-DICOM formats (NIfTI has no standard profile — its sidecar pseudonymisation
is separate); any `ferrum-view/1` contract change beyond the masked-render flag.
