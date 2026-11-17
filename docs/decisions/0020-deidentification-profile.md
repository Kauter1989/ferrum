# 0020 — DICOM de-identification: PS3.15 Basic Profile and Clean Pixel Data

- **Status:** accepted (G1 spec approved 2026-11-17; G2 design)
- **Date:** 2026-11-17
- **Issue:** [#75](https://github.com/Kauter1989/ferrum/issues/75) (P2)
- **Spec:** `dev_plan.md` Stage 31; **design:** [deidentification.md](../deidentification.md)

## Context

Medical Harness needs identifier-free inputs for remote vision models and for
exports to research or another site. FERRUM already pseudonymises its agent
outputs and its DICOM export UIDs (`dicom/export.rs`: `copy_identifiers`,
`copy_dates`, `keep_uids`, salted UID hashes). That is a FERRUM-specific
pseudonymisation, not a standard de-identification: it does not follow the
PS3.15 Annex E Basic Application Level Confidentiality Profile, does not clean
free-text descriptors, and does not mask burned-in pixel text. A standard
profile is needed so an export is recognisably de-identified to another site,
and so a render sent to a vision model carries no burned-in identifiers.

## Decision

1. **The transform lives in `ferrum-io`.** A new `dicom/deid.rs` applies the
   profile to a `dicom_object` object. `ferrum-io` is the only crate that parses
   DICOM (constitution); the de-identification is a DICOM transform, so it lives
   beside the reader and the export. A use case in `ferrum-app` wires it for the
   export and the agent; no business logic in the CLI/agent or the view host.

2. **The Basic Profile rules are generated from a committed Table E.1-1 data
   file**, not written by hand. `crates/ferrum-io/src/dicom/deid_table.csv`
   holds one row per attribute of PS3.15 Table E.1-1: tag, keyword, action, and
   the option that governs an option-dependent row. A build-time or test-time
   generator reads it; a test fails when the table and the generator disagree.
   The actions are the standard's: `D` (replace with a non-empty consistent
   value), `Z` (zero-length), `X` (remove), `U` (consistent pseudonymous UID),
   `C` (clean descriptors), `K` (keep), `R`/`Z/D` (option-dependent). The table
   is sourced from the standard once and verified against the oracles (decision
   4); it is data, not code, so a profile correction is a data edit.

3. **UIDs reuse the export's salted derivation.** `U` rows map study / series /
   SOP / frame-of-reference UIDs consistently within one run through the same
   `uid_from` salted hash the export already uses; `retain UIDs` keeps them.
   This keeps one UID derivation, not a second.

4. **Test oracles are pydicom/deid (MIT) and the RSNA/CTP anonymizer profile.**
   A Python diff test de-identifies the same synthetic study with FERRUM and
   with each oracle and pins the expected agreement (differences named and
   justified). The oracles run in the Python test layer, generate their DICOM at
   run time, and commit no patient or volume data.

5. **Clean Pixel Data is region masking first; OCR is an optional bridge.**
   Known burned-in regions (the `BurnedInAnnotation` flag and a configured
   rectangular mask) are masked in the pixel data. An OCR check runs **behind a
   bridge** (`ferrum-engine/1`-style, out of process; no OCR in Rust, no weights
   bundled, constitution); without a bridge the step is skipped and reported,
   never silently passed. Renders can be requested "masked" (`view slice
   --masked`, the host frame route) — masking is a pixel transform, not image
   formation, so the rendering rule is untouched.

6. **The profile is conservative by default.** The default is the Basic Profile
   with no options (the most conservative); each option (*retain longitudinal
   dates modified*, *retain UIDs*, *clean descriptors*, *retain device identity*,
   *retain institution identity*, *retain patient characteristics*) changes
   exactly the rows Table E.1-1 marks option-dependent. An unknown option is
   refused.

## Consequences

- An export can be de-identified to a recognised standard (`export bundle
  --deidentify basic [--option …]`); the agent command set gains the option with
  a schema, `commands.json`, docs and contract tests (CLI = MCP), and the privacy
  scan passes.
- The de-identification is reproducible and checked against established
  anonymisers, not asserted.
- A new bridge kind (OCR) follows the engine protocol; it is optional and never
  bundled.
- The Table E.1-1 data file is a committed artefact sourced from the standard;
  keeping it current is a data maintenance task, pinned by the oracle diff test.
- Out of scope: a DIMSE router (#76); OCR weights or inference in Rust;
  de-identification of non-DICOM formats (NIfTI has no standard profile).
