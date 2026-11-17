//! The transfer-function editor's wire data, on the host (criterion 29.2-b).
//!
//! `ferrum-wasm` builds only for wasm32, but the shape of what crosses to the
//! panel is testable here: the control points as five floats each (a swapped
//! channel or a lost endpoint would redraw the wrong anatomy), and the
//! histogram the editor draws behind the curve. The edits themselves are the
//! domain's `TransferFunction` API, tested in `ferrum-domain`; these cases
//! check that an edit reaches the wire form the panel re-reads.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[path = "../src/tf.rs"]
mod tf;

use ferrum_domain::{ControlPoint, Dims3, Rgb, TransferFunction, Volume};
use glam::Vec3;

fn rebuild(flat: &[f32]) -> TransferFunction {
    let points: Vec<ControlPoint> =
        flat.chunks(tf::POINT_STRIDE).map(|c| ControlPoint::new(c[0], Rgb::new(c[2], c[3], c[4]), c[1])).collect();
    TransferFunction::new(points).unwrap()
}

// covers 29.2-b — the points cross as five floats each and rebuild the same
// function: position, opacity, then red, green, blue, all in [0, 1].
#[test]
fn the_points_round_trip_through_the_flat_wire_form() {
    let original = TransferFunction::legacy_default();
    let flat = tf::points(&original);
    assert_eq!(flat.len(), original.points().len() * tf::POINT_STRIDE);
    assert_eq!(rebuild(&flat), original, "the wire form loses or swaps a field");
    // The endpoints stay at 0 and 1, and every float is a unit value.
    assert_eq!(flat[0], 0.0);
    assert_eq!(flat[flat.len() - tf::POINT_STRIDE], 1.0);
    assert!(flat.iter().all(|v| (0.0..=1.0).contains(v)), "a value left the unit square: {flat:?}");
}

// covers 29.2-b — every edit the binding offers reaches the wire form the
// panel re-reads after the edit (move, insert, remove, recolour).
#[test]
fn an_edit_changes_the_wire_form() {
    let mut edited = TransferFunction::linear_ramp();
    let index = edited.insert_point(0.5);
    let flat = tf::points(&edited);
    assert_eq!(flat.len(), 3 * tf::POINT_STRIDE, "the inserted point is on the wire");
    assert_eq!(flat[index * tf::POINT_STRIDE], 0.5);

    edited.move_point(index, 0.5, 0.2).unwrap();
    let flat = tf::points(&edited);
    assert!((flat[index * tf::POINT_STRIDE + 1] - 0.2).abs() < 1e-6, "the move reached the opacity");

    edited.set_color(index, Rgb::new(1.0, 0.0, 0.0)).unwrap();
    let flat = tf::points(&edited);
    let c = &flat[index * tf::POINT_STRIDE + 2..index * tf::POINT_STRIDE + 5];
    assert_eq!(c, &[1.0, 0.0, 0.0], "the colour channels are in r, g, b order");

    edited.remove_point(index).unwrap();
    assert_eq!(rebuild(&tf::points(&edited)), TransferFunction::linear_ramp());
}

// covers 29.2-b — the histogram is the storage histogram, normalised so the
// largest bin is 1. The phantom's two values land in the first and last bin:
// 0.3 is off the power-of-two storage grid, so the range stays the data range
// (on-grid data would widen it, `IntensityRange::for_data`, and the max would
// sit below the last bin).
#[test]
fn the_histogram_normalises_the_storage_counts() {
    let values = [-1000.0, -1000.0, -1000.0, -1000.0, 0.3, 0.3, 0.3, 0.3];
    let volume = Volume::from_physical(Dims3::new(2, 2, 2), Vec3::ONE, &values).unwrap();
    let h = tf::histogram(&volume);
    assert_eq!(h.len(), TransferFunction::LUT_SIZE);
    assert_eq!(h[0], 1.0, "the min value fills the first bin");
    assert_eq!(h[255], 1.0, "the max value fills the last bin");
    assert!(h[1..255].iter().all(|&v| v == 0.0), "a two-value phantom has no middle bins: {h:?}");
}

// covers 29.2-b — a ramp spreads over the bins, stays in [0, 1], and a
// single-value volume (the empty box the binding starts with) does not divide
// by zero.
#[test]
fn the_histogram_stays_in_the_unit_range() {
    let values: Vec<f32> = (0..64u16).map(|i| f32::from(i) * 10.0).collect();
    let volume = Volume::from_physical(Dims3::new(4, 4, 4), Vec3::ONE, &values).unwrap();
    let h = tf::histogram(&volume);
    assert_eq!(h.len(), TransferFunction::LUT_SIZE);
    assert!(h.iter().all(|&v| (0.0..=1.0).contains(&v)), "a bin left [0, 1]: {h:?}");
    assert_eq!(h.iter().copied().fold(0.0f32, f32::max), 1.0, "the largest bin is 1");
    assert!(h.iter().filter(|&&v| v > 0.0).count() > 8, "a ramp spreads over the bins");

    let flat = Volume::from_physical(Dims3::new(1, 1, 1), Vec3::ONE, &[0.0]).unwrap();
    let h = tf::histogram(&flat);
    assert_eq!(h.len(), TransferFunction::LUT_SIZE);
    assert!(h.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "a single value broke the histogram");
}
