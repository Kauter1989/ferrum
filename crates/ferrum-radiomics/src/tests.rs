//! Tests of [`compute`] on a phantom with known answers.

use ferrum_domain::{Dims3, MaskRegion, Volume};
use glam::{UVec3, Vec3};

use super::*;

/// 12 x 12 x 12 voxels of 1 x 1 x 2 mm; the value is `100 * k - 600` HU, so
/// each axial slice is constant and the answers are known by hand.
fn phantom() -> Volume {
    let dims = Dims3 { x: 12, y: 12, z: 12 };
    let values: Vec<f32> = (0..dims.voxel_count()).map(|n| 100.0 * (n as u32 / 144) as f32 - 600.0).collect();
    Volume::from_physical(dims, Vec3::new(1.0, 1.0, 2.0), &values).unwrap()
}

/// The box 2..6 x 3..7 x 4..8 (4 x 4 x 4 voxels; slices 4..=7 hold -200, -100, 0, 100).
fn block() -> MaskRegion {
    let (lo, hi) = (UVec3::new(2, 3, 4), UVec3::new(6, 7, 8));
    MaskRegion::from_grid(Dims3 { x: 12, y: 12, z: 12 }, |n| {
        let p = UVec3::new(n as u32 % 12, (n as u32 / 12) % 12, n as u32 / 144);
        p.cmpge(lo).all() && p.cmplt(hi).all()
    })
    .unwrap()
}

fn run(unit: ValueUnit) -> Result<MetricsResult, MetricsError> {
    let (v, r) = (phantom(), block());
    compute(&Input { region: &r, volume: &v, unit }, &QUICK, &Params::default())
}

fn near(r: &MetricsResult, id: &str, expected: f64) {
    let got = r.number(id).unwrap_or_else(|| panic!("{id} has no value"));
    // the phantom is stored in 16 bits: 1100 HU / 65535 per step
    assert!((got - expected).abs() < 0.02, "{id}: {got} != {expected}");
}

// covers 18.1-c
#[test]
fn the_result_names_the_profile_the_unit_and_the_parameters() {
    let r = run(ValueUnit::Hu).unwrap();
    assert_eq!((r.profile, r.profile_version), ("quick", "0.1"));
    assert_eq!(r.unit, ValueUnit::Hu);
    assert_eq!(r.params.preprocessing.name(), "none");
    let ids: Vec<_> = r.features.iter().map(|f| f.feature.id).collect();
    assert_eq!(ids, QUICK.features, "values come in the order of the profile");
    assert_eq!(r.get("intensity.mean").unwrap().unit, "HU");
    assert_eq!(r.get("size.volume_ml").unwrap().unit, "mL");
    assert_eq!(r.get("size.voxels").unwrap().unit, "");
}

// covers 18.2-a, 18.2-b
#[test]
fn size_features_of_a_block_with_known_answers() {
    let r = run(ValueUnit::Hu).unwrap();
    assert_eq!(r.number("size.voxels"), Some(64.0));
    // 64 voxels of 1 x 1 x 2 mm
    assert_eq!(r.number("size.volume_mm3"), Some(128.0));
    assert_eq!(r.number("size.volume_ml"), Some(0.128));
    assert_eq!(
        [r.number("size.extent_i_mm"), r.number("size.extent_j_mm"), r.number("size.extent_k_mm")],
        [Some(4.0), Some(4.0), Some(8.0)]
    );
    // centroid index (3.5, 4.5, 5.5) -> mm (3.5, 4.5, 11.0) with the identity geometry
    assert_eq!(
        [r.number("size.centroid_x_mm"), r.number("size.centroid_y_mm"), r.number("size.centroid_z_mm")],
        [Some(3.5), Some(4.5), Some(11.0)]
    );
    // all slices of a block tie; the first one wins (1-based): k 4 -> 5, j 3 -> 4, i 2 -> 3
    assert_eq!(
        [
            r.number("size.largest_slice_axial"),
            r.number("size.largest_slice_coronal"),
            r.number("size.largest_slice_sagittal")
        ],
        [Some(5.0), Some(4.0), Some(3.0)]
    );
    // a 4 x 4 voxel slice: the diagonal is 3 * sqrt(2) between voxel centres
    near(&r, "size.long_axis_mm", 3.0 * 2.0_f64.sqrt());
}

// covers 18.3-a, 18.3-b, 18.3-c
#[test]
fn intensity_features_of_a_block_with_known_answers() {
    let r = run(ValueUnit::Hu).unwrap();
    // 16 voxels each of -200, -100, 0, 100 HU
    for (id, expected) in [
        ("intensity.mean", -50.0),
        ("intensity.std", 12500.0_f64.sqrt()),
        ("intensity.min", -200.0),
        ("intensity.max", 100.0),
        ("intensity.range", 300.0),
        ("intensity.p5", -200.0),
        ("intensity.p25", -100.0),
        ("intensity.p50", 0.0),
        ("intensity.p75", 0.0),
        ("intensity.p95", 100.0),
        ("intensity.iqr", 100.0),
    ] {
        near(&r, id, expected);
    }
}

// covers 18.3-e
#[test]
fn an_unknown_unit_refuses_the_intensity_features_instead_of_guessing() {
    for modality in ["", "MR"] {
        match run(ValueUnit::Unknown { modality: modality.into() }) {
            Err(MetricsError::UnitUnknown { modality: m, features }) => {
                assert_eq!(m, modality);
                assert!(features.contains(&"intensity.mean") && !features.iter().any(|f| f.starts_with("size.")));
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn an_empty_region_is_an_error_not_a_row_of_zeros() {
    let v = phantom();
    let empty = MaskRegion::empty(ferrum_domain::segmentation::VoxelBox::new(UVec3::ZERO, UVec3::splat(3)));
    let input = Input { region: &empty, volume: &v, unit: ValueUnit::Hu };
    assert_eq!(compute(&input, &QUICK, &Params::default()), Err(MetricsError::EmptyRegion));
}

#[test]
fn the_same_input_gives_the_same_result_bit_for_bit() {
    assert_eq!(run(ValueUnit::Hu), run(ValueUnit::Hu));
}

#[test]
fn a_region_that_is_not_tight_gives_the_same_numbers() {
    // the same block in a loose box: sizes must not depend on the crop
    let (v, tight) = (phantom(), block());
    let loose = tight.expanded(ferrum_domain::segmentation::VoxelBox::new(UVec3::ZERO, UVec3::splat(12)));
    let (a, b) = (
        compute(&Input { region: &tight, volume: &v, unit: ValueUnit::Hu }, &QUICK, &Params::default()).unwrap(),
        compute(&Input { region: &loose, volume: &v, unit: ValueUnit::Hu }, &QUICK, &Params::default()).unwrap(),
    );
    assert_eq!(a, b);
}

fn clinical(region: &MaskRegion, volume: &Volume) -> MetricsResult {
    compute(&Input { region, volume, unit: ValueUnit::Hu }, &CLINICAL, &Params::default()).unwrap()
}

/// A grid of 12 x 12 x 12 voxels (1 x 1 x 2 mm), all zero.
fn empty_volume() -> Volume {
    let dims = Dims3 { x: 12, y: 12, z: 12 };
    Volume::from_physical(dims, Vec3::new(1.0, 1.0, 2.0), &vec![0.0; dims.voxel_count()]).unwrap()
}

fn cells(cells: &[(u32, u32, u32)]) -> MaskRegion {
    MaskRegion::from_grid(Dims3 { x: 12, y: 12, z: 12 }, |n| {
        let p = (n as u32 % 12, (n as u32 / 12) % 12, n as u32 / 144);
        cells.contains(&p)
    })
    .unwrap()
}

fn num(r: &MetricsResult, id: &str) -> f64 {
    r.number(id).unwrap_or_else(|| panic!("{id}: {:?}", r.get(id)))
}

// covers 18.5-b
#[test]
fn the_shape_figures_of_a_single_voxel_follow_from_its_octahedron() {
    // 1 x 1 x 1 mm voxels: the surface is an octahedron of area sqrt(3) and volume 1/6
    let v = Volume::from_physical(Dims3 { x: 3, y: 3, z: 3 }, Vec3::ONE, &[0.0; 27]).unwrap();
    let one = MaskRegion::from_grid(Dims3 { x: 3, y: 3, z: 3 }, |n| n == 13).unwrap();
    let r = clinical(&one, &v);
    let (area, volume) = (3.0_f64.sqrt(), 1.0 / 6.0);
    let pi = std::f64::consts::PI;
    let near = |id: &str, expected: f64| {
        assert!((num(&r, id) - expected).abs() < 1e-12, "{id}: {} != {expected}", num(&r, id))
    };
    near("shape.surface_area_mm2", area);
    near("shape.mesh_volume_ml", volume / 1000.0);
    near("shape.surface_to_volume_per_mm", area / volume);
    // (36 pi V^2)^(1/3) = pi^(1/3) / 1 for V = 1/6
    near("shape.sphericity", pi.cbrt() / area);
    near("shape.compactness_2", pi / area.powi(3));
    near("shape.compactness_1", volume / (pi.sqrt() * area.powf(1.5)));
    near("shape.spherical_disproportion", area / pi.cbrt());
    let sphericity = num(&r, "shape.sphericity");
    assert!(
        (sphericity.powi(3) - num(&r, "shape.compactness_2")).abs() < 1e-12,
        "compactness 2 is the cube of the sphericity"
    );
    assert!((sphericity * num(&r, "shape.spherical_disproportion") - 1.0).abs() < 1e-12);
}

// covers 18.6-b, 18.6-d
#[test]
fn the_axes_of_a_block_are_known_by_hand_and_a_tie_has_no_direction() {
    let (v, r) = (phantom(), block());
    let c = clinical(&r, &v);
    // 4 x 4 x 4 voxels of 1 x 1 x 2 mm: index variance (4^2 - 1) / 12 = 1.25; along k times 2^2
    let (major, minor) = (4.0 * 5.0_f64.sqrt(), 4.0 * 1.25_f64.sqrt());
    assert!((num(&c, "shape.major_axis_mm") - major).abs() < 1e-9);
    assert!((num(&c, "shape.minor_axis_mm") - minor).abs() < 1e-9);
    assert!((num(&c, "shape.least_axis_mm") - minor).abs() < 1e-9);
    assert!((num(&c, "shape.elongation") - 0.5).abs() < 1e-9 && (num(&c, "shape.flatness") - 0.5).abs() < 1e-9);
    // the long axis is k: (0, 0, 1) in LPS; the other two are equally long, so they have no direction
    assert_eq!(
        [num(&c, "shape.major_axis_dir_x"), num(&c, "shape.major_axis_dir_y"), num(&c, "shape.major_axis_dir_z")]
            .map(f64::abs),
        [0.0, 0.0, 1.0]
    );
    for id in ["shape.minor_axis_dir_x", "shape.least_axis_dir_z"] {
        match &c.get(id).unwrap().value {
            Value::Null { reason } => assert!(reason.contains("not unique"), "{reason}"),
            other => panic!("{id}: {other:?}"),
        }
    }
}

// covers 18.5-d, 18.6-c
#[test]
fn a_point_a_line_and_a_plate_never_give_nan() {
    let v = empty_volume();
    let line: Vec<_> = (0..7).map(|i| (i, 3, 4)).collect();
    let plate: Vec<_> = (0..6).flat_map(|i| (0..6).map(move |j| (i, j, 2))).collect();
    for (name, body) in [("point", vec![(3, 4, 5)]), ("line", line), ("plate", plate)] {
        let r = clinical(&cells(&body), &v);
        for f in &r.features {
            if let Value::Number(x) = f.value {
                assert!(x.is_finite(), "{name}: {} is {x}", f.feature.id);
            }
        }
        let sphericity = num(&r, "shape.sphericity");
        assert!(sphericity > 0.0 && sphericity <= 1.0, "{name}: {sphericity}");
        assert!(num(&r, "shape.surface_area_mm2") > 0.0 && num(&r, "shape.mesh_volume_ml") > 0.0, "{name}");
    }
    // a point has no extent: the ratios are undefined and say why
    let point = clinical(&cells(&[(3, 4, 5)]), &v);
    for id in ["shape.elongation", "shape.flatness"] {
        match &point.get(id).unwrap().value {
            Value::Null { reason } => assert!(reason.contains("no extent"), "{reason}"),
            other => panic!("{id}: {other:?}"),
        }
    }
    // a plate is flat: no extent along its normal, which has a unique direction
    let plate: Vec<_> = (0..6).flat_map(|i| (0..6).map(move |j| (i, j, 2))).collect();
    let plate = clinical(&cells(&plate), &v);
    assert_eq!((num(&plate, "shape.least_axis_mm"), num(&plate, "shape.flatness")), (0.0, 0.0));
    assert_eq!(num(&plate, "shape.least_axis_dir_z"), 1.0);
}

// covers 18.5-b
#[test]
fn clinical_adds_to_quick_without_changing_what_quick_says() {
    let (v, r) = (phantom(), block());
    let quick = run(ValueUnit::Hu).unwrap();
    let c = clinical(&r, &v);
    for f in &quick.features {
        assert_eq!(c.get(f.feature.id), Some(f), "{} differs between the profiles", f.feature.id);
    }
    assert_eq!((c.profile, c.profile_version), ("clinical", "0.3"));
    assert!(c.features.len() > quick.features.len());
    assert!(quick.warnings.is_empty(), "quick does not look at the connectivity");
    assert_eq!(c.get("shape.surface_area_mm2").unwrap().unit, "mm2");
    // shape does not need the unit, so quick's refusal is the only reason clinical is refused too
    assert!(matches!(
        compute(
            &Input { region: &r, volume: &v, unit: ValueUnit::Unknown { modality: String::new() } },
            &CLINICAL,
            &Params::default()
        ),
        Err(MetricsError::UnitUnknown { .. })
    ));
}

/// A box of voxels as a list of cells.
fn cuboid(lo: [u32; 3], hi: [u32; 3]) -> Vec<(u32, u32, u32)> {
    (lo[2]..hi[2]).flat_map(|k| (lo[1]..hi[1]).flat_map(move |j| (lo[0]..hi[0]).map(move |i| (i, j, k)))).collect()
}

// covers 18.2-c
#[test]
fn the_3d_diameter_of_the_block_is_its_space_diagonal() {
    let r = clinical(&block(), &phantom());
    // 4 x 4 x 4 voxels of 1 x 1 x 2 mm: the centres span 3, 3 and 3 steps
    assert!((num(&r, "size.max_diameter_mm") - 54f64.sqrt()).abs() < 1e-9);
    // the axial long axis is shorter: the 3D diameter is the larger of the two
    assert!(num(&r, "size.max_diameter_mm") > num(&r, "size.long_axis_mm"));
}

// covers 18.7-a, 18.7-b
#[test]
fn a_box_is_its_own_hull() {
    let r = clinical(&block(), &phantom());
    assert!((num(&r, "shape.solidity") - 1.0).abs() < 1e-9);
    assert!((num(&r, "shape.convexity") - 1.0).abs() < 1e-9);
    assert!((num(&r, "shape.hull_volume_ml") - num(&r, "shape.mesh_volume_ml")).abs() < 1e-12);
    assert!((num(&r, "shape.hull_area_mm2") - num(&r, "shape.surface_area_mm2")).abs() < 1e-9);
    assert_eq!(r.get("shape.hull_volume_ml").unwrap().unit, "mL");
    assert_eq!(r.get("shape.hull_area_mm2").unwrap().unit, "mm2");
}

// covers 18.7-a, 18.7-b
#[test]
fn two_voxels_apart_have_a_hull_known_by_hand() {
    // voxels of 1 x 1 x 2 mm at i = 2 and i = 5. Each is an octahedron with
    // half axes 0.5, 0.5 and 1 mm (volume 1/3 mm³, area 3 mm²). The hull is
    // a prism of length 3 whose cross-section is the diamond (area 1 mm²,
    // perimeter 4 * sqrt(1.25) mm), closed by two half octahedra.
    let r = clinical(&cells(&[(2, 5, 5), (5, 5, 5)]), &phantom());
    let hull_volume = 3.0 + 1.0 / 3.0;
    assert!((num(&r, "shape.mesh_volume_ml") * 1000.0 - 2.0 / 3.0).abs() < 1e-12);
    assert!((num(&r, "shape.hull_volume_ml") * 1000.0 - hull_volume).abs() < 1e-12);
    assert!((num(&r, "shape.solidity") - 0.2).abs() < 1e-12);
    assert!((num(&r, "shape.hull_area_mm2") - (12.0 * 1.25f64.sqrt() + 3.0)).abs() < 1e-12);
    // two pieces: the hull surface is larger than the surface of the pieces
    assert!((num(&r, "shape.convexity") - (5f64.sqrt() + 0.5)).abs() < 1e-12);
    assert_eq!(num(&r, "size.max_diameter_mm"), 3.0);
    assert_eq!(num(&r, "topology.components"), 2.0);
}

// covers 18.8-a, 18.8-b, 18.8-c, 18.8-d
#[test]
fn a_hollow_box_reports_its_cavity_and_warns() {
    let v = phantom();
    let mut hollow = cuboid([2, 2, 2], [8, 8, 8]);
    hollow.retain(|c| !cuboid([4, 4, 4], [6, 6, 6]).contains(c));
    let r = clinical(&cells(&hollow), &v);
    assert_eq!(num(&r, "topology.components"), 1.0);
    assert_eq!(num(&r, "topology.largest_component_share"), 1.0);
    assert_eq!(num(&r, "topology.holes"), 1.0);
    // 8 voxels of 1 x 1 x 2 mm
    assert_eq!(num(&r, "topology.hole_volume_mm3"), 16.0);
    assert_eq!(num(&r, "topology.euler_number"), 2.0);
    assert_eq!(num(&r, "topology.border_sides"), 0.0);
    assert_eq!(r.warnings.len(), 1);
    assert!(r.warnings[0].starts_with("has_holes: 1 cavities of 8 voxels"), "{:?}", r.warnings);
    // the cavity surface is inside the mesh: its volume is that of the box minus that of the cavity
    let outer = clinical(&cells(&cuboid([2, 2, 2], [8, 8, 8])), &v);
    let inner = clinical(&cells(&cuboid([4, 4, 4], [6, 6, 6])), &v);
    let expected = num(&outer, "shape.mesh_volume_ml") - num(&inner, "shape.mesh_volume_ml");
    assert!((num(&r, "shape.mesh_volume_ml") - expected).abs() < 1e-12);
    assert!((num(&r, "shape.solidity") - expected / num(&outer, "shape.mesh_volume_ml")).abs() < 1e-12);
}

// covers 18.8-d
#[test]
fn a_piece_far_away_and_the_image_border_each_give_their_warning() {
    let v = phantom();
    let mut cut = cuboid([0, 3, 3], [4, 6, 6]);
    cut.extend(cuboid([7, 7, 7], [10, 9, 9]));
    let r = clinical(&cells(&cut), &v);
    assert_eq!((num(&r, "topology.components"), num(&r, "topology.border_sides")), (2.0, 1.0));
    let codes: Vec<&str> = r.warnings.iter().map(|w| w.split(':').next().unwrap()).collect();
    assert_eq!(codes, ["fragmented", "touches_border"]);
    // the mask is not touched by a warning: the same segment measured again is identical
    assert_eq!(clinical(&cells(&cut), &v), r);
}

// covers 18.9-a, 18.9-c, 18.10-a
#[test]
fn the_distribution_of_the_block_is_known_by_hand_and_the_bin_width_is_recorded() {
    // four slices of 16 voxels at -200, -100, 0 and 100 HU
    let (v, r) = (phantom(), block());
    let c = clinical(&r, &v);
    let unit = "HU";
    let near = |id: &str, expected: f64| {
        let got = num(&c, id);
        // the phantom is stored in 16 bits: 0.02 HU per step, and energy adds that up
        let tol = 0.05 + 1e-4 * expected.abs();
        assert!((got - expected).abs() < tol, "{id}: {got} != {expected}");
    };
    // mean -50, deviations -150 -50 50 150: m2 = 12500, m3 = 0, m4 = 256 250 000
    near("intensity.skewness", 0.0);
    near("intensity.kurtosis", 256_250_000.0 / 156_250_000.0 - 3.0);
    // energy 16 * (40000 + 10000 + 0 + 10000)
    near("intensity.energy", 960_000.0 * (1.0 + 0.0));
    near("intensity.rms", 15_000f64.sqrt());
    near("intensity.mad", 100.0);
    near("intensity.rmad", 100.0);
    // four bins of a quarter each at width 25
    near("intensity.entropy", 2.0);
    near("intensity.uniformity", 0.25);
    // range -200..100: V10 counts >= -170 (three slices), V90 counts >= 70 (one)
    near("intensity.ivh_v10", 0.75);
    near("intensity.ivh_v90", 0.25);
    // 7th largest of 64 is 100, 58th largest is -200
    near("intensity.ivh_i10", 100.0);
    near("intensity.ivh_i90", -200.0);
    assert_eq!(c.get("intensity.energy").unwrap().unit, format!("{unit}2"));
    assert_eq!(c.get("intensity.entropy").unwrap().unit, "bits");
    assert_eq!(c.get("intensity.uniformity").unwrap().unit, "");
    assert_eq!(c.get("intensity.rms").unwrap().unit, "HU");
    assert_eq!(c.bin_width, Some(25.0));
    // quick has no histogram feature, so it records no bin width and carries no curves
    let q = run(ValueUnit::Hu).unwrap();
    assert!(q.bin_width.is_none() && q.ivh.is_none() && q.hu_ranges.is_none());
}

// covers 18.9-c
#[test]
fn a_different_bin_width_changes_the_histogram_and_is_recorded() {
    let (v, r) = (phantom(), block());
    let params = Params { bin_width: 500.0, ..Params::default() };
    let c = compute(&Input { region: &r, volume: &v, unit: ValueUnit::Hu }, &CLINICAL, &params).unwrap();
    // width 500: -200 and -100 share [-500, 0), 100 is in [0, 500), and the 0 HU slice
    // (stored as a hair below or above 0) joins either: entropy of 3 : 1 or of 2 : 2
    let h = num(&c, "intensity.entropy");
    assert!((h - 0.811_278).abs() < 1e-5 || (h - 1.0).abs() < 1e-12, "{h}");
    assert_eq!(c.bin_width, Some(500.0));
    assert_eq!(c.params.bin_width, 500.0);
    // the moments do not depend on the bin width
    let d = clinical(&r, &v);
    assert_eq!(num(&c, "intensity.energy"), num(&d, "intensity.energy"));
    assert_eq!(num(&c, "intensity.ivh_v10"), num(&d, "intensity.ivh_v10"));
}

// covers 18.9-d
#[test]
fn a_constant_segment_has_no_skewness_or_kurtosis_and_says_why() {
    // one slice of the block is constant
    let v = phantom();
    let slice = cells(&cuboid([2, 3, 5], [6, 7, 6]));
    let c = clinical(&slice, &v);
    for id in ["intensity.skewness", "intensity.kurtosis"] {
        match &c.get(id).unwrap().value {
            Value::Null { reason } => assert!(reason.contains("all voxel values are equal"), "{reason}"),
            other => panic!("{id}: {other:?}"),
        }
    }
    assert!((num(&c, "intensity.entropy")).abs() < 1e-12);
    assert!((num(&c, "intensity.uniformity") - 1.0).abs() < 1e-12);
    assert!((num(&c, "intensity.mad")).abs() < 1e-3);
}

// covers 18.10-a, 18.10-c
#[test]
fn the_curve_starts_at_one_never_rises_and_has_101_points() {
    let (v, r) = (phantom(), block());
    let c = clinical(&r, &v);
    let curve = c.ivh.as_ref().expect("clinical carries the curve");
    assert_eq!((curve.intensity.len(), curve.volume_fraction.len()), (101, 101));
    assert_eq!(curve.volume_fraction[0], 1.0);
    assert!((curve.intensity[0] + 200.0).abs() < 0.05 && (curve.intensity[100] - 100.0).abs() < 0.05);
    assert!(curve.volume_fraction.windows(2).all(|w| w[1] <= w[0]));
    assert_eq!(curve.volume_fraction[10], num(&c, "intensity.ivh_v10"));
    assert_eq!(curve.volume_fraction[90], num(&c, "intensity.ivh_v90"));
}

// covers 18.11-a, 18.11-c, 18.11-d
#[test]
fn hu_ranges_are_recorded_and_the_shares_with_the_outside_sum_to_one() {
    let (v, r) = (phantom(), block());
    let given = vec![HuRange::new(-250.0, -150.0), HuRange::new(-150.0, 50.0), HuRange::new(50.0, 150.0)];
    let params = Params { hu_ranges: Some(given.clone()), ..Params::default() };
    let c = compute(&Input { region: &r, volume: &v, unit: ValueUnit::Hu }, &CLINICAL, &params).unwrap();
    let shares = c.hu_ranges.as_ref().unwrap();
    assert!(!shares.is_default);
    assert_eq!(shares.ranges.iter().map(|s| s.range).collect::<Vec<_>>(), given, "ranges come back as given");
    assert_eq!(shares.ranges.iter().map(|s| s.voxels).collect::<Vec<_>>(), [16, 32, 16]);
    assert_eq!(shares.outside, 0.0);
    // the defaults are marked as defaults, and still sum to one with the outside
    let d = clinical(&r, &v);
    let defaults = d.hu_ranges.as_ref().unwrap();
    assert!(defaults.is_default);
    assert_eq!(defaults.ranges.len(), DEFAULT_HU_RANGES.len());
    let sum: f64 = defaults.ranges.iter().map(|s| s.fraction).sum::<f64>() + defaults.outside;
    assert!((sum - 1.0).abs() < 1e-12, "{sum}");
}

// covers 18.11-b
#[test]
fn bad_parameters_are_refused_before_anything_is_computed() {
    let (v, r) = (phantom(), block());
    let run = |params: Params| compute(&Input { region: &r, volume: &v, unit: ValueUnit::Hu }, &CLINICAL, &params);
    let overlap =
        Params { hu_ranges: Some(vec![HuRange::new(0.0, 100.0), HuRange::new(50.0, 150.0)]), ..Params::default() };
    assert!(matches!(run(overlap), Err(MetricsError::BadParameter(m)) if m.contains("overlap")));
    assert!(matches!(run(Params { bin_width: 0.0, ..Params::default() }), Err(MetricsError::BadParameter(_))));
    // even a profile that does not use them refuses a nonsense parameter
    let quick = compute(
        &Input { region: &r, volume: &v, unit: ValueUnit::Hu },
        &QUICK,
        &Params { bin_width: -1.0, ..Params::default() },
    );
    assert!(matches!(quick, Err(MetricsError::BadParameter(_))));
}

// covers 18.3-e
#[test]
fn the_ranges_and_the_curve_need_the_unit_like_the_other_intensity_features() {
    let (v, r) = (phantom(), block());
    let unknown = ValueUnit::Unknown { modality: "MR".into() };
    let e = compute(&Input { region: &r, volume: &v, unit: unknown }, &CLINICAL, &Params::default()).unwrap_err();
    match e {
        MetricsError::UnitUnknown { features, .. } => {
            assert!(features.contains(&"intensity.entropy") && features.contains(&"intensity.ivh_i10"));
        }
        other => panic!("{other:?}"),
    }
}
