# Quantitative segment profiles (Stage 18)

Design for [Stage 18](../dev_plan.md) (`quick` and `clinical` profiles).
IBSI preprocessing, texture features, comparison and change over time
are [Stage 19](../dev_plan.md); they are designed when Stage 19 is
confirmed again after Stage 18 is released (see §7). Decision:
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
| Ports | Yes. No file access outside `ferrum-io`; no engine involved. The desktop app saves a declared modality through the port in `ferrum-domain`: `ResultStore` got `modality` and `set_modality` (default methods), `ferrum-io::WorkspaceStore` implements them with the manifest (PR 2). |
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
  size.rs         volume, box, centroid in mm, extents, axial axes
  diameter.rs     maximum 3D diameter between voxel centres
  mesh.rs         marching cubes: area, mesh volume
  pca.rs          3×3 symmetric eigen-decomposition (Jacobi), axes, elongation, flatness
  hull.rs         3D convex hull and farthest pair (solidity and convexity are formed in shape.rs)
  topology.rs     pieces, cavities, Euler number, warnings (thresholds as constants)
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
  interpolation; the `radiomics-ibsi` profile (Stage 19) will use it and
  bump its version. Mixing them silently is a lesson waiting to happen.
- **No threads in reductions** (Phases 1–3), so that results are
  byte-identical on every machine.
- Rounding is the adapters' job and is shared by the CLI and MCP, so
  their JSON is identical.

### 3.2 Algorithms

| Feature | Method | Risk |
|---|---|---|
| Volume, extent, centroid (mm) | voxel count × voxel volume; centroid of voxel centres mapped through the grid's direction and origin | none |
| Maximum 3D diameter | farthest pair among the hull vertices of the end voxels of the lines along the longest side of the box (a distance along a line is largest at an end, so no other voxel can be an end of the farthest pair); voxel centres, as the existing long axis; IBSI and pyradiomics use mesh vertices, so the field test expects small differences; a flat or straight set has no 3D hull and is searched pairwise | hull robustness |
| Surface area, mesh volume | marching cubes at iso 0.5 on the mask padded by one voxel; the surface of each of the 256 corner patterns is built from the face loops (no memorised table; ambiguous faces separate the inside corners, so voxels that touch only at an edge or a corner are separate pieces), each loop a fan around its centroid and wound outwards; vertices scaled by the spacing; area as the sum of triangle areas | closed and outward-wound for every pattern, checked by invariants; a binary mask's mesh is rougher than the smooth shape (a ball: +7 %, see §5) |
| Compactness 1 and 2, sphericity, spherical disproportion, surface-to-volume | IBSI formulas on the mesh area and mesh volume | none |
| PCA axes, elongation, flatness | population covariance of the voxel centres in patient mm (computed on indices, then carried through direction and spacing); Jacobi eigenvalues; axis length 4√λ (IBSI); a direction is `Null` when two axes are equally long or the segment has no extent along it; a flat or thin body has zero for the lengths with no extent (exactly), and `Null` ratios only for a single voxel |
| Convex hull, solidity, convexity | incremental hull on the deduplicated mesh vertices (the six axis extremes first, so interior points cost a few face tests); no new dependency; solidity = mesh volume / hull volume, convexity = hull area / mesh area | coplanar and tiny inputs → `Null` with a reason |
| Components, largest share | `ferrum_domain::analysis::components` | none |
| Cavities | `ferrum_domain::analysis::fill_holes` marks the background not 6-connected to the border of the box; its 6-connected pieces are the cavities | cost follows the box |
| Warnings | `fragmented` (largest piece below 95 % of the voxels), `has_holes` (one cavity or more), `touches_border` (one image side or more); thresholds are in the definition texts; they never change the mask | none |
| Euler number | Euler characteristic of the cubical complex of the 26-connected foreground (vertices − edges + faces − cubes) | verified on a cube (1), a cube with a cavity (2) and a torus (0) |
| First-order | mean, population std, min, max, nearest-rank percentiles, range, IQR; skewness, excess kurtosis, energy, RMS, MAD, rMAD, entropy, uniformity (IBSI definitions) | constant region → `Null` |
| Histogram | fixed bin width (default 25 HU for CT), recorded in the result; bins `[k·w, (k+1)·w)` anchored at 0 HU, so segments are binned alike whatever their minimum (pyradiomics also anchors at a multiple of the width; IBSI's default anchors at the ROI minimum, so entropy can differ by a few hundredths) | the anchor changes entropy and uniformity; named in the definition text and in the field test |
| IVH | on the voxel values, not binned: V_x = share of voxels ≥ min + x % of the range; I_x = the ⌈x·N/100⌉-th largest value (the highest value that x % of the volume reaches, as in dose-volume histograms); the curve is V at 0, 1, … 100 % of the range | definitions checked by hand on a ramp and on 15 voxels (rounding up) |
| HU-range shares | half-open ranges `[lo, hi)`, two binary searches on the sorted values; overlapping, empty or non-finite ranges are `bad_request`; touching ranges are fine; the share outside all ranges is reported so that shares sum to 1; defaults (−1000, −500, −100, 0, 100, 300, 1000) are documented and marked as defaults, not as clinical thresholds | none |

Performance is an estimate (≤ 2 s for the `clinical` profile on a 100 000
voxel segment) until PR 6 measures it with a benchmark in
`crates/ferrum-radiomics/benches`.

### 3.3 Profiles

`quick`: size, first-order. `clinical`: `quick` plus shape, hull,
topology and distribution; it is registered with its first additional
features (PR 3), so that no profile is an alias of another. Both are `0.x` until Phase 3 is released and
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

- **Marching cubes.** A wrong table gives wrong area quietly, so there
  is none: each pattern's surface is built from the face loops, and the
  invariants (every edge used once in each direction, so closed and
  outward-wound; Euler characteristic 2 for a ball; exact area and volume
  of a single voxel, an octahedron) are tested on random masks that
  contain every pattern. Measured (L13): the area of a ball of radius
  10 mm is 7.4 % above 4πr² at 1 mm voxels and 6.6 % at 0.5 mm, so it does
  not converge to 1; the sphericity of a voxelised ball is about 0.93.
  The triangulation of ambiguous cubes is this crate's own; IBSI's
  reference differs in the details, so conformance (Stage 19) is checked
  against its phantom, not assumed.
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
| 18.2-c | `diameter.rs` (`the_diameter_of_a_box_is_its_space_diagonal`, `the_diameter_equals_a_brute_force_search_on_random_blobs`, `flat_thin_and_single_voxel_segments_have_a_diameter`), `hull.rs::farthest_pair_equals_brute_force_and_handles_flat_sets`, `tests.rs::the_3d_diameter_of_the_block_is_its_space_diagonal`, `crates/ferrum-agent/tests/metrics.rs::the_clinical_profile_adds_diameter_hull_and_connectivity` | test |
| 18.2-d | `metrics.rs::axes_and_extents_equal_segment_shape` | test |
| 18.2-e | `size.rs::dims_of_one_and_anisotropy` (each axis) | test |
| 18.2-f | `metrics.rs::empty_or_unknown_segment_is_not_found` | test |
| 18.3-a | `intensity.rs::first_order_matches_stats` (the existing phantom: p5 6, p50 51, std 29.15) | test |
| 18.3-b | `intensity.rs::range_and_iqr_exact` | test |
| 18.3-c | `metrics.rs::unit_is_reported_and_unknown_warns` (NIfTI, DICOM) | test |
| 18.3-d | `feature.rs::definitions_name_std_and_percentile_method` | test |
| 18.3-e | `metrics.rs::unknown_modality_is_refused_with_a_hint` (NIfTI) | test |
| 18.3-f | `crates/ferrum-app/tests/metrics.rs` (`an_image_without_a_modality_asks_and_remembers_the_answer`, `an_image_with_its_own_modality_keeps_it`, `the_declaration_is_kept_in_the_workspace_and_taken_from_it`), `ferrum-io` `workspace_store_keeps_the_declared_modality_in_the_manifest`, `ui.rs::a_nifti_file_without_a_modality_asks_what_it_is`; the dialog looked at in a render and, on a real NIfTI, by a person | test, manual |
| 18.4-a | `commands.json` freshness (`crates/ferrum-cli/tests/package.rs`), contract test | test |
| 18.4-b | `cli.rs::segment_metrics_parses` and one `--help` run | test |
| 18.4-c | `crates/ferrum-agent/tests/mcp.rs` parity case | test |
| 18.4-d | `crates/ferrum-agent/tests/privacy.rs` | test |
| 18.4-e | `crates/ferrum-app/tests/metrics.rs::a_segment_is_measured_in_the_background_…`, `…marked_out_of_date…`, `ui.rs::the_metrics_card_shows_values_with_units_and_says_when_a_segment_is_a_proposal`, `metrics_card.rs::every_row_carries_the_definition_of_its_feature`; the card looked at in a render; a person opens it on a real study | test, manual |
| 18.4-f | `ui.rs::the_metrics_card_explains_an_unknown_image_type_and_recovers_when_it_is_set`, `metrics.rs::an_unknown_image_type_explains_instead_of_guessing` | test |
| 18.4-g | `metrics.rs::describe_adds_definitions` | test |
| 18.5-a | `mesh.rs` (`the_surface_is_closed_and_wound_outwards_for_every_kind_of_cube`, `a_ball_is_a_closed_surface_of_euler_characteristic_two`, `a_single_voxel_is_an_octahedron_with_exact_area_and_volume`, `the_area_of_a_ball_exceeds_the_analytic_value_by_the_known_margin_at_every_resolution`, `voxels_that_touch_only_at_an_edge_or_a_corner_are_separate_closed_pieces`) | test |
| 18.5-b | `tests.rs::the_shape_figures_of_a_single_voxel_follow_from_its_octahedron`, `clinical_adds_to_quick_without_changing_what_quick_says`, `metrics.rs::the_clinical_profile_adds_the_shape_…` | test |
| 18.5-c | `mesh.rs::spacing_scales_the_mesh_exactly`, the ball at two resolutions in `the_area_of_a_ball_exceeds_…` | test |
| 18.5-d | `tests.rs::a_point_a_line_and_a_plate_never_give_nan` | test |
| 18.6-a | `pca.rs::an_ellipsoid_has_the_axes_of_its_semi_axes` (5, 12, 8 mm, 1 and 2 mm slices) | test |
| 18.6-b | `pca.rs::an_ellipsoid_has_the_axes_…`, `tests.rs::the_axes_of_a_block_are_known_by_hand_and_a_tie_has_no_direction` | test |
| 18.6-c | `pca.rs::a_line_a_plane_and_a_point_are_undefined_where_…`, `tests.rs::a_point_a_line_and_a_plate_never_give_nan` | test |
| 18.6-d | `pca.rs` (`directions_are_unit_vectors_in_lps_with_a_fixed_sign`, `the_sign_of_a_direction_is_fixed_by_its_largest_component`, `an_object_turned_by_45_degrees_…`), `metrics.rs` (a null direction says why) | test |
| 18.7-a | `hull.rs` (`volume_and_area_of_a_box`, `every_point_is_inside_or_on_the_hull_and_the_surface_is_closed`, `volume_equals_the_brute_force_hull_of_a_small_cloud`, `interior_and_repeated_points_change_nothing`, `flat_or_tiny_input_has_no_hull`), `tests.rs::a_box_is_its_own_hull` | test |
| 18.7-b | `tests.rs` (`a_box_is_its_own_hull`, `two_voxels_apart_have_a_hull_known_by_hand`), `hull.rs::the_hull_of_an_l_shaped_mesh_is_known_by_hand` (11/3, derived with the mixed area in the test comment and checked against an independent facet-polygon computation outside the repository) | test |
| 18.7-c | `hull.rs::cost_follows_the_surface` (3 000 interior points leave 8 vertices) | test |
| 18.8-a | `topology.rs::pieces_touching_at_an_edge_or_a_corner_are_one_piece_and_far_pieces_are_not` (pieces come from `ferrum_domain::analysis::components`, the same call as `segment components`) | test |
| 18.8-b | `topology.rs` (`a_box_with_a_cavity_has_one_hole_of_eight_voxels_and_euler_number_two`, `separate_cavities_are_counted_and_a_tunnel_is_not_a_cavity`), `tests.rs::a_hollow_box_reports_its_cavity_and_warns` | test |
| 18.8-c | `topology.rs` (box 1, box with a cavity 2, ring 0, two voxels sharing a corner 1) | test |
| 18.8-d | `topology.rs` (`a_segment_at_the_edge_of_the_image_touches_the_border`, `warnings_leave_the_mask_alone`), `tests.rs::a_piece_far_away_and_the_image_border_each_give_their_warning` | test |
| 18.9-a | `distribution.rs` (`ten_voxels_give_the_hand_computed_moments_and_deviations`, `…entropy_and_uniformity_for_the_bin_width`, `bins_are_anchored_at_zero_…`, `a_symmetric_set_has_no_skew_…`), `tests.rs::the_distribution_of_the_block_is_known_by_hand_…`, `feature.rs::the_distribution_features_are_registered_…` | test |
| 18.9-b | `distribution.rs::ten_voxels_give_the_hand_computed_…` (sum 60, m2 27.2, m3 246.6, m4 4010, energy 632, MAD 3.6, rMAD 1.5; bins of 5 and of 1) | test |
| 18.9-c | `tests.rs` (`…bin_width_is_recorded`, `a_different_bin_width_changes_the_histogram_and_is_recorded`), `params.rs::parameters_are_checked_…`, `crates/ferrum-agent/tests/metrics.rs::the_clinical_profile_adds_the_distribution_…` | test |
| 18.9-d | `distribution.rs::a_constant_region_has_no_skewness_…`, `tests.rs::a_constant_segment_has_no_skewness_or_kurtosis_and_says_why`, the agent contract test | test |
| 18.10-a | `distribution.rs` (`a_linear_ramp_gives_the_exact_volume_and_intensity_figures`, `the_ten_voxel_set_gives_the_ranked_intensities`, `a_fraction_of_the_volume_that_is_not_a_whole_number_of_voxels_rounds_up`), `tests.rs::the_curve_starts_at_one_never_rises_and_has_101_points` | test |
| 18.10-b | `distribution.rs::a_linear_ramp_gives_the_exact_volume_and_intensity_figures` (0..=99 HU: V10 0.9, V90 0.1, I10 90, I90 10) | test |
| 18.10-c | `distribution.rs::the_curve_never_rises_whatever_the_values` (pseudo-random sets of 1 to 400 values, fixed seed; not proptest) | test |
| 18.11-a | `distribution.rs::range_shares_are_half_open_and_sum_to_one_with_the_outside`, `tests.rs::hu_ranges_are_recorded_and_the_shares_with_the_outside_sum_to_one` | test |
| 18.11-b | `params.rs::parameters_are_checked_and_ranges_may_touch_but_not_overlap`, `tests.rs::bad_parameters_are_refused_…`, `crates/ferrum-agent/tests/metrics.rs::nonsense_parameters_are_bad_requests_with_a_hint` | test |
| 18.11-c | `ui.rs::hu_ranges_come_from_the_interface_fields`; defaults documented | test, manual |
| 18.11-d | `tests.rs::hu_ranges_are_recorded_…`, the agent contract test (ranges as given, `default` flag, note) | test |
| 18.12-a | `ui.rs::clinical_card_groups_by_family_with_tooltips` | test, manual |
| 18.12-b | as 18.4-a to 18.4-d for `clinical`: `commands.json` freshness, `cli.rs::segment_metrics_flags_become_parameters` (`--bin-width`, `--hu-range`), `mcp.rs` parity, `privacy.rs` | test |
| 18.12-c | `crates/ferrum-radiomics/benches/profiles.rs`, the number goes into the docs | test |
| Stage success: values agree with pyradiomics on MSD Task09 spleen | a field script (not committed) run on the user's machine, differences explained in the PR | field |

Every criterion of Stage 18 has at least one row.

## 7. Size and PR split

Estimates use the closest merged PRs in `.claude/skills/ferrum-size/calibration.md`
(#28 `feat/agent-views` 1 109 lines M; #30 `feat/review-workspace`
784 M; #34 `feat/segmentation-workflow` 1 175 M; #32 `feat/dicom-seg-sr`
1 631 L). Lines are code / tests / docs; generated `commands.json` is not
weighed. All are estimates until measured.

Stage 18 is **XL** as one piece (≈ 7 500 lines) and is split
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

### Stage 19

IBSI preprocessing, texture features, comparison and change over time
were Phases 4–6 of Stage 18. On 2026-10-07 they moved to Stage 19, so
that Stage 18 closes after its Phase 3 and gets a clean analyze report.
Stage 19 has its own spec confirmation, design and size.

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
