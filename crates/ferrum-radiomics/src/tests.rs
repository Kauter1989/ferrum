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
    assert_eq!((c.profile, c.profile_version), ("clinical", "0.1"));
    assert!(c.features.len() > quick.features.len());
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
