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
