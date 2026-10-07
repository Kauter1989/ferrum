# Quantitative segment profiles (Stage 18)

Design for Phases 1–3 of [Stage 18](../dev_plan.md) (`quick` and
`clinical` profiles). Phases 4–6 (IBSI preprocessing, texture features,
comparison and change over time) are designed when they are confirmed
again after Phase 3 (see §7). Decision:
[ADR 0011](decisions/0011-radiomics-crate.md).

## 1. Context

What exists on `develop` (0.3.0) and stays as it is:

| Today | Where |
|---|---|
| Voxel count, `volume_ml` | `ferrum-domain/src/segmentation.rs` (`SegmentationSet::voxel_count`, `volume_ml`) |
| Mask crop, components (26-connected), shape (extent, largest slices, axial long and short axis, border contact), agreement (Dice, Jaccard, HD95, Hausdorff, centroid distance), edits | `ferrum-domain/src/analysis.rs` (`MaskRegion`, `components`, `shape`, `compare`) |
| Agent commands over them | `segment shape`, `segment components`, `segment compare`, `segment edit` (`ferrum-agent/src/commands/masks.rs`) |
| First-order statistics (count, mean, std, min, max, p5–p95) of a box, sphere, segment or area annotation | `ferrum-agent/src/commands/inspect.rs` (`stats`, `summarise`), nearest-rank percentiles |
| Value unit of a study: `HU` only when the modality is CT | `ferrum-agent/src/study.rs` (`Study::value_unit`) |
| Modality declared for a study that carries none (NIfTI) | `ferrum-agent/src/study.rs` (`declare_modality`), saved in the workspace manifest; `study open --modality` |
| Desktop app: segments panel, `Viewer` use cases for segments | `ferrum/src/ui/segments_panel.rs`, `ferrum-app/src/viewer/segments.rs` |

What is missing: a shared implementation for the desktop app, surface
area and compactness measures, PCA axes, convex hull, holes and Euler
number, distribution measures, intensity-volume histogram, HU-range
shares, profiles, and any way for the desktop app to declare a modality.
The desktop reader sets an empty modality for NIfTI (`ferrum-io/src/nifti.rs`).

## 2. Constitution check

| Point | Answer |
|---|---|
| Layering | Yes, after ADR 0011: `ferrum-radiomics` depends on `ferrum-domain` only; `ferrum-app` and `ferrum-agent` use it. `ferrum-app` still compiles without wgpu and egui. |
| Ports | Yes. No file access outside `ferrum-io`; no engine involved. If the desktop app needs to save a declared modality, the port in `ferrum-domain` (`ResultStore`) is extended, and `ferrum-io` implements it (to be confirmed in PR 2, see §5). |
| No inference in Rust | Yes; classical image analysis only. |
| Agents propose, people confirm; values come from voxels | Yes. Metrics only read; warnings never change a mask. A result for a segment that is not confirmed carries its provenance. |
| Identifiers | Yes. Results hold numbers, ids and units; the identifier scan (`tests/privacy.rs`) covers the new command. |
| Rendering rule | Not touched (no pixels). The desktop card is UI text. |
| DICOM tags | Not touched. |

## 3. Design

### 3.1 The crate

```
ferrum-radiomics/src/
  lib.rs          compute(), re-exports
  feature.rs      Feature { id, family, unit, definition, since, needs_unit }, the registry
  profile.rs      Profile { name, version, features }, `quick`, `clinical`
  result.rs       MetricsResult, Value (Number | Null { reason }), Warning
  params.rs       Params { bin_width, hu_ranges, … }, recorded in the result
  intensity.rs    first-order, histogram, IVH, HU-range shares
  size.rs         volume, box, centroid in mm, extents, 3D diameter
  mesh.rs         marching cubes: area, mesh volume
  pca.rs          3×3 symmetric eigen-decomposition (Jacobi), axes, elongation, flatness
  hull.rs         3D convex hull, farthest pair, solidity, convexity
  topology.rs     cavities, Euler number, warnings
```

Entry point:

```rust
pub fn compute(input: &Input, profile: &Profile, params: &Params)
    -> Result<MetricsResult, MetricsError>;

pub struct Input<'a> {
    pub region: &'a MaskRegion,   // from ferrum-domain
    pub volume: &'a Volume,       // values and spacing
    pub unit: ValueUnit,          // Hu | Unknown { modality }
}
```

- `Value::Null { reason }` stands for a measure that is undefined (a
  plane, a constant region); NaN and infinity never leave the crate.
- A feature with `needs_unit` (every intensity feature) with
  `ValueUnit::Unknown` gives `MetricsError::UnitUnknown`, which the
  adapters report as `bad_request` with the hint to declare the modality
  (18.3-e). Geometry is available without it through `segment shape`.
- **One implementation per measure:** extent, long and short axis,
  component count and largest share call `ferrum_domain::analysis`;
  `summarise` moves here from the agent, and the agent's `stats` calls it
  (same JSON, regression test 18.3-a).
- **Percentiles** are nearest-rank, as `stats` has them today
  (`round((n−1)·q)`), and the definition text says so. IBSI uses linear
  interpolation; the `radiomics-ibsi` profile (Phase 4) will use it and
  bump its version. Mixing them silently is a lesson waiting to happen.
- **No threads in reductions** (Phases 1–3), so that results are
  byte-identical on every machine.
- Rounding is the adapters' job and is shared by the CLI and MCP, so
  their JSON is identical.

### 3.2 Algorithms

| Feature | Method | Risk |
|---|---|---|
| Volume, extent, centroid (mm) | voxel count × voxel volume; centroid of voxel centres mapped through the grid's direction and origin | none |
| Maximum 3D diameter | farthest pair among the hull vertices of the boundary voxel centres (voxel centres, as the existing long axis; IBSI and pyradiomics use mesh vertices, so the field test expects small differences) | hull robustness |
| Surface area, mesh volume | marching cubes at iso 0.5 on the mask padded by one voxel; vertices scaled by the spacing; area as the sum of triangle areas | table errors; checked by mesh invariants (every edge shared by exactly two triangles, Euler characteristic 2 for a ball) |
| Compactness 1 and 2, sphericity, spherical disproportion, surface-to-volume | IBSI formulas on the mesh area and mesh volume | none |
| PCA axes, elongation, flatness | covariance of voxel centres in mm; Jacobi eigenvalues; axis length 4√λ (IBSI) | degenerate sets → `Null` |
| Convex hull, solidity, convexity | incremental hull on the mesh vertices (not on every voxel); no new dependency | coplanar and tiny inputs → `Null` with a reason |
| Components, largest share | `ferrum_domain::analysis::components` | none |
| Cavities | flood fill of the background (6-connected) from the border of the padded box; the unreached background is the cavities | cost follows the box |
| Euler number | Euler characteristic of the cubical complex of the 26-connected foreground (vertices − edges + faces − cubes) | verified on a cube (1), a cube with a cavity (2) and a torus (0) |
| First-order | mean, population std, min, max, nearest-rank percentiles, range, IQR; skewness, excess kurtosis, energy, RMS, MAD, rMAD, entropy, uniformity (IBSI definitions) | constant region → `Null` |
| Histogram | fixed bin width (default 25 HU for CT), recorded | none |
| IVH | V10, V90, I10, I90 and the curve on the discretised values (IBSI definitions) | definitions checked by hand on a ramp |
| HU-range shares | half-open ranges `[lo, hi)`; overlapping ranges are `bad_request`; defaults are documented and marked as defaults | none |

Performance is an estimate (≤ 2 s for the `clinical` profile on a 100 000
voxel segment) until PR 6 measures it with a benchmark in
`crates/ferrum-radiomics/benches`.

### 3.3 Profiles

`quick`: size, first-order. `clinical`: `quick` plus shape, hull,
topology and distribution. Both are `0.x` until Phase 3 is released and
`1` afterwards; the result always names profile and version. A feature
added to a profile is a version bump.

### 3.4 Use cases and adapters

- `Viewer::segment_metrics(label, profile, params)` in `ferrum-app`
  (synchronous, for tests and embedding apps) and its background-job
  variant for the panel, through the existing job queue
  (`ferrum-app/src/jobs.rs`).
- `Viewer::declare_modality(modality)` and `Viewer::needs_modality()` so
  that the app can ask (18.3-f). The value is written to the metadata of
  the dataset and to the workspace manifest, as the CLI does.
- Agent command `segment metrics`: parameters `workspace`, `segment`,
  `profile` (default `quick`), `bin_width`, `hu_ranges`, `describe`;
  result
  `{ segment, profile: {name, version}, unit, params, features: { id: { value | null, unit, reason?, definition? } }, warnings }`.
  `segment` is the existing `segment_json`, which carries the provenance.
  CLI: `ferrum-cli segment metrics -w W <label> [--profile P] [--describe]`.
- Segments panel: a metrics card per selected segment, grouped by family,
  value and unit, definition tooltip on every feature, *proposed* label
  for an unconfirmed segment, and the reason when there is nothing to
  show. In Phase 3 it gets the bin-width and HU-range fields (the person
  edits them; 18.11-c).
- NIfTI without a modality: the desktop app asks when the file is opened
  (a small dialog: CT, MR, PT, other code), shows the choice and keeps it.

## 4. Contracts and "also update" rows

| Change | Also update (`ferrum-change` §2) |
|---|---|
| New agent command `segment metrics` | `schema.rs`; `ferrum-cli schema > skills/ferrum/schemas/commands.json`; `skills/ferrum/reference/commands.md`; `docs/agent-cli.md`; a contract test in `crates/ferrum-agent/tests`; a CLI parse test in `crates/ferrum-cli/src/cli.rs` (L8); identifier scan (`tests/privacy.rs`) |
| `stats` now calls the shared maths | the existing `stats` contract and unit tests stay unchanged and must pass |
| New crate and dependency rule | `CLAUDE.md` layering, `docs/architecture.md`, workspace `Cargo.toml`, `cargo deny check licenses` |
| Desktop UI: card, modality dialog | use cases in `Viewer`, not in widgets; `cargo test -p ferrum --test ui` (L9) |
| Possibly the `ResultStore` port | `ferrum-domain` trait, `ferrum-io` implementation and its tests |
| User-visible behaviour | `CHANGELOG.md` `[Unreleased]`; README feature row |

No change to the engine protocol, workspace file formats, shaders or the
CPU renderer.

## 5. Risks and assumptions

- **Marching cubes table.** A wrong table gives wrong area quietly.
  Mitigation: the mesh invariants above, tested on a ball, a box and a
  torus, and a comparison with the analytic area of a sphere within 3 %
  (a tolerance to be set by measurement, L13).
- **Convex hull robustness** on coplanar or very small inputs. Mitigation:
  property tests against brute force (every input point inside or on the
  hull, volume against a reference), and `Null` with a reason on
  degenerate input. Estimated at ~400 lines; not measured.
- **Percentile method.** Nearest-rank now, linear interpolation under
  IBSI later. Mitigation: the definition text names the method and the
  profile version changes with it.
- **pyradiomics differs on purpose in places** (3D diameter on mesh
  vertices, its own percentile and bin choices). The field test compares
  with explained tolerances, not with equality.
- **Port change.** Saving a declared modality from the desktop app may
  need `ResultStore` to grow a method. To be checked at the start of
  PR 2; if it does, that becomes the PR's one extra surface.
- **Panel blocking.** A large segment may take seconds. The panel uses
  the job queue; the 2 s budget is an estimate.
- **Assumption:** only CT is supported. For other modalities the
  geometry commands still work and the intensity features are refused.

## 6. Validation plan

Levels: `test` (CI), `manual` (a person in the desktop app), `field`
(the user's machine, public data: MSD Task09 spleen; pyradiomics as the
reference). No criterion draws pixels in the renderer, so there is no
`visual` row; the desktop card is checked by the UI test and by hand.

| Criterion | Verified by | Level |
|---|---|---|
| 18.1-a | `feature.rs::ids_are_unique_and_documented` (planned) | test |
| 18.1-b | `profile.rs::unknown_profile_lists_known_ones` (planned) | test |
| 18.1-c | `crates/ferrum-agent/tests/metrics.rs::result_carries_profile_params_and_provenance` (planned) | test |
| 18.1-d | review of ADR 0011, `CLAUDE.md`, `docs/architecture.md` | manual |
| 18.2-a | `size.rs::volume_of_a_box_with_anisotropic_spacing` (4×5×6, 0.7×0.7×2.5) | test |
| 18.2-b | `size.rs::extent_and_centroid_in_patient_mm_with_direction` | test |
| 18.2-c | `hull.rs::max_diameter_equals_brute_force` (box diagonal, random blob) | test |
| 18.2-d | `metrics.rs::axes_and_extents_equal_segment_shape` | test |
| 18.2-e | `size.rs::dims_of_one_and_anisotropy` (each axis) | test |
| 18.2-f | `metrics.rs::empty_or_unknown_segment_is_not_found` | test |
| 18.3-a | `intensity.rs::first_order_matches_stats` (the existing phantom: p5 6, p50 51, std 29.15) | test |
| 18.3-b | `intensity.rs::range_and_iqr_exact` | test |
| 18.3-c | `metrics.rs::unit_is_reported_and_unknown_warns` (NIfTI, DICOM) | test |
| 18.3-d | `feature.rs::definitions_name_std_and_percentile_method` | test |
| 18.3-e | `metrics.rs::unknown_modality_is_refused_with_a_hint` (NIfTI) | test |
| 18.3-f | `crates/ferrum-app/tests/use_cases.rs::declare_modality_is_kept` and the dialog | test, manual |
| 18.4-a | `commands.json` freshness (`crates/ferrum-cli/tests/package.rs`), contract test | test |
| 18.4-b | `cli.rs::segment_metrics_parses` and one `--help` run | test |
| 18.4-c | `crates/ferrum-agent/tests/mcp.rs` parity case | test |
| 18.4-d | `crates/ferrum-agent/tests/privacy.rs` | test |
| 18.4-e | `crates/ferrum/tests/ui.rs::metrics_card_shows_values_and_proposed_label`; a person opens the card on a real study | test, manual |
| 18.4-f | `ui.rs::metrics_card_explains_missing_segment` | test |
| 18.4-g | `metrics.rs::describe_adds_definitions` | test |
| 18.5-a | `mesh.rs::sphere_area_within_3_percent`; closed-mesh invariants | test |
| 18.5-b | `mesh.rs::compactness_and_sphericity_formulas` (hand-computed cube) | test |
| 18.5-c | `mesh.rs::scale_invariance_between_1mm_and_half_mm` | test |
| 18.5-d | `mesh.rs::one_voxel_thick_object_is_null_with_reason` | test |
| 18.6-a | `pca.rs::ellipsoid_axes_within_3_percent` (5, 8, 12 mm) | test |
| 18.6-b | `pca.rs::elongation_and_flatness_are_axis_ratios` | test |
| 18.6-c | `pca.rs::line_plane_and_point_are_null` | test |
| 18.6-d | `pca.rs::orientation_is_sign_normalised_and_deterministic` | test |
| 18.7-a | `hull.rs::volume_area_solidity_convexity` | test |
| 18.7-b | `hull.rs::box_is_solid_and_l_shape_by_hand` | test |
| 18.7-c | `hull.rs::cost_follows_the_surface` (vertex count of the input is checked) | test |
| 18.8-a | `topology.rs::components_equal_segment_components` | test |
| 18.8-b | `topology.rs::box_with_a_2x2x2_cavity_has_one_hole_of_8_voxels` | test |
| 18.8-c | `topology.rs::euler_of_cube_hollow_cube_and_torus` (1, 2, 0) | test |
| 18.8-d | `topology.rs::warnings_do_not_change_the_mask` | test |
| 18.9-a | `intensity.rs::distribution_measures_follow_ibsi_definitions` | test |
| 18.9-b | `intensity.rs::ten_voxel_set_exact` | test |
| 18.9-c | `metrics.rs::bin_width_is_recorded` | test |
| 18.9-d | `intensity.rs::constant_region_has_null_skewness_and_kurtosis` | test |
| 18.10-a | `intensity.rs::ivh_v10_v90_i10_i90` | test |
| 18.10-b | `intensity.rs::ivh_of_a_linear_ramp_is_exact` | test |
| 18.10-c | `intensity.rs::ivh_curve_never_increases` (proptest) | test |
| 18.11-a | `intensity.rs::range_fractions_sum_to_one` | test |
| 18.11-b | `metrics.rs::overlapping_ranges_are_bad_request` | test |
| 18.11-c | `ui.rs::hu_ranges_come_from_the_interface_fields`; defaults documented | test, manual |
| 18.11-d | `metrics.rs::ranges_are_recorded` | test |
| 18.12-a | `ui.rs::clinical_card_groups_by_family_with_tooltips` | test, manual |
| 18.12-b | as 18.4-a to 18.4-d for `clinical` | test |
| 18.12-c | `crates/ferrum-radiomics/benches/profiles.rs`, the number goes into the docs | test |
| Stage success: values agree with pyradiomics on MSD Task09 spleen | a field script (not committed) run on the user's machine, differences explained in the PR | field |

Every criterion of Phases 1–3 has at least one row.

## 7. Size and PR split

Estimates use the closest merged PRs in `.claude/skills/ferrum-size/calibration.md`
(#28 `feat/agent-views` 1 109 lines M; #30 `feat/review-workspace`
784 M; #34 `feat/segmentation-workflow` 1 175 M; #32 `feat/dicom-seg-sr`
1 631 L). Lines are code / tests / docs; generated `commands.json` is not
weighed. All are estimates until measured.

Stage 18 (Phases 1–3) is **XL** as one piece (≈ 7 500 lines) and is split
into six PRs, each M, each leaving `develop` releasable.

| PR | Tasks | Est. lines (code/tests/docs) | Files | Surfaces | Class | Field test after |
|---|---|---|---|---|---|---|
| 1 | T1 crate, registry, profiles, first-order, size; `segment metrics` quick | 900 / 350 / 150 | ~22 | agent command set | M | yes (CLI, CPU only, MSD spleen NIfTI and DICOM) |
| 2 | T2 desktop: modality dialog and metrics card | 600 / 250 / 60 | ~12 | desktop UI (+ port if needed) | M | yes (the user opens a NIfTI) |
| 3 | T3 mesh shape: area, compactness, sphericity, PCA | 900 / 350 / 100 | ~12 | — | M | no |
| 4 | T4 hull, 3D diameter, solidity; T5 holes, Euler, warnings | 900 / 350 / 80 | ~14 | — | M | no |
| 5 | T6 distribution, IVH, HU ranges; `clinical` v1 in CLI/MCP | 850 / 350 / 120 | ~14 | agent command parameters | M | yes (pyradiomics comparison) |
| 6 | T7 clinical card (families, tooltips, bin and range fields), benchmark, docs | 500 / 200 / 150 | ~12 | desktop UI | M | yes (the user, UI) |

Re-estimate when a PR passes its estimate by 30 %, a new contract surface
appears, or the session needs a compaction (`ferrum-size` §4).

### Phases 4–6

They are not part of this design. The analyze script counts every
criterion of the stage, so Stage 18 could not reach a clean report until
Phases 4–6 are done. **Proposal (a decision for G2):** move Phases 4–6 to
a new **Stage 19** (renumbered 19.1–19.10), so that Stage 18 closes after
Phase 3 and Stage 19 gets its own spec confirmation, design and size.

## 8. Tasks

| Task | Criteria | Tests first | Touches (matrix rows) | Depends on | PR |
|---|---|---|---|---|---|
| 18.T1 Crate, registry, profiles, result, first-order, size, `segment metrics --profile quick` | 18.1-a, 18.1-b, 18.1-c, 18.1-d, 18.2-a, 18.2-b, 18.2-d, 18.2-e, 18.2-f, 18.3-a, 18.3-b, 18.3-c, 18.3-d, 18.3-e, 18.4-a, 18.4-b, 18.4-c, 18.4-d, 18.4-g | `first_order_matches_stats`, contract test CLI vs MCP | agent command, new crate and rule, privacy scan, docs | — | 1 |
| 18.T2 Desktop modality dialog and metrics card | 18.3-f, 18.4-e, 18.4-f | `declare_modality_is_kept`, UI test | desktop UI, port, use cases | T1 | 2 |
| 18.T3 Mesh, area, compactness, sphericity, PCA | 18.5-a, 18.5-b, 18.5-c, 18.5-d, 18.6-a, 18.6-b, 18.6-c, 18.6-d | closed-mesh invariants, sphere area | agent command (profile content), docs | T1 | 3 |
| 18.T4 Convex hull, 3D diameter, solidity | 18.2-c, 18.7-a, 18.7-b, 18.7-c | `max_diameter_equals_brute_force` | agent command (profile content), docs | T1 | 4 |
| 18.T5 Holes, Euler, warnings | 18.8-a, 18.8-b, 18.8-c, 18.8-d | cavity and Euler phantoms; warnings do not mutate | agent command (profile content), docs | T1 | 4 |
| 18.T6 Distribution, IVH, HU ranges, `clinical` v1 in CLI/MCP | 18.9-a, 18.9-b, 18.9-c, 18.9-d, 18.10-a, 18.10-b, 18.10-c, 18.11-a, 18.11-b, 18.11-d, 18.12-b | ten-voxel set, ramp IVH, overlapping ranges | agent command parameters, schema, docs | T1 | 5 |
| 18.T7 Clinical card, HU-range fields, benchmark | 18.11-c, 18.12-a, 18.12-c | UI test, benchmark | desktop UI, docs | T2, T6 | 6 |

Safety tests (`forbidden`, `limit`) do not apply: no task mutates a
segment. The empty and unknown-segment paths are in T1.
