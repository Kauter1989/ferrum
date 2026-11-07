//! The MPR geometry, on the host.
//!
//! `ferrum-wasm` builds only for wasm32, but the arithmetic that decides where
//! a slice lands is where the visual bugs were: a slice stretched over a
//! square quarter, or drawn over its neighbours. `layout.rs` holds that
//! arithmetic and is compiled here directly, so the cases below run in CI
//! without a browser and fail when the geometry is wrong.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

#[path = "../src/layout.rs"]
mod layout;

use glam::Vec2;

fn area(r: (Vec2, Vec2)) -> f32 {
    (r.1.x - r.0.x) * (r.1.y - r.0.y)
}

// covers 26.2-a
#[test]
fn the_quarters_tile_the_canvas_and_never_overlap() {
    let size = Vec2::new(320.0, 200.0);
    let q = layout::quarters(size);

    assert_eq!(q[0].0, Vec2::ZERO, "the first quarter does not start at the origin");
    let total: f32 = q.iter().map(|r| area(*r)).sum();
    assert!((total - size.x * size.y).abs() < 0.5, "the quarters cover {total}, the canvas is 80000");

    // Each quarter is non-empty.
    for (i, r) in q.iter().enumerate() {
        assert!(area(*r) > 0.0, "quarter {i} is empty: {r:?}");
    }
    // No two quarters share an area: an overlap would paint one view over
    // another, which is the defect the scissor rects work around.
    for (i, a) in q.iter().enumerate() {
        for b in q.iter().skip(i + 1) {
            let w = a.1.x.min(b.1.x) - a.0.x.max(b.0.x);
            let h = a.1.y.min(b.1.y) - a.0.y.max(b.0.y);
            assert!(w <= 0.0 || h <= 0.0, "two quarters overlap: {a:?} and {b:?}");
        }
    }
}

// covers 26.2-a
#[test]
fn a_wide_plane_is_limited_by_its_quarter_s_width() {
    let quarter = (Vec2::ZERO, Vec2::new(320.0, 200.0));
    let (min, max) = layout::fit(quarter, (512, 256));
    let (w, h) = (max.x - min.x, max.y - min.y);

    // A 2:1 plane is wider than a 1.6:1 quarter, so the width is the limit:
    // 320 wide and 160 high. Sizing by the 200 height instead would give 400
    // wide, overflowing the quarter the scissor clips it to.
    assert!((w - 320.0).abs() < 1e-3, "a 2:1 plane fills the 320-wide quarter, got {w}");
    assert!((h - 160.0).abs() < 1e-3, "a 2:1 plane 320 wide is 160 high, got {h}");
    // Centred vertically: the margin above equals the margin below.
    assert!((min.y - (200.0 - max.y)).abs() < 1e-3, "the fitted slice is not centred vertically");
}

// covers 26.2-a
#[test]
fn a_tall_plane_is_limited_by_its_quarter_s_height() {
    let quarter = (Vec2::ZERO, Vec2::new(320.0, 200.0));
    let (min, max) = layout::fit(quarter, (100, 400));
    let (w, h) = (max.x - min.x, max.y - min.y);

    // A 1:4 plane is narrower than the quarter, so the height limits: 50 wide
    // and 200 high. Sizing by the width would give 1280 high.
    assert!((h - 200.0).abs() < 1e-3, "a 1:4 plane fills the 200-high quarter, got {h}");
    assert!((w - 50.0).abs() < 1e-3, "a 1:4 plane 200 high is 50 wide, got {w}");
    // Centred horizontally: the margin left equals the margin right.
    assert!((min.x - (320.0 - max.x)).abs() < 1e-3, "the fitted slice is not centred horizontally");
}

// covers 26.2-a
#[test]
fn a_square_plane_fills_the_shorter_side_of_its_quarter() {
    let quarter = (Vec2::ZERO, Vec2::new(320.0, 200.0));
    let (min, max) = layout::fit(quarter, (256, 256));
    let (w, h) = (max.x - min.x, max.y - min.y);

    assert!((w - h).abs() < 1e-3, "a square plane came out {w} by {h}");
    assert!((h - 200.0).abs() < 1e-3, "a square plane fills the 200-high quarter, got {h}");
    // A square in a wide quarter fills the height and is centred horizontally.
    assert!(min.y.abs() < 1e-3, "a square plane leaves no vertical margin");
    assert!((min.x - (320.0 - max.x)).abs() < 1e-3, "a square plane is not centred horizontally");
}

// covers 26.2-a
#[test]
fn every_fitted_slice_stays_inside_its_quarter() {
    // The invariant behind all the cases above: whatever the plane's shape, the
    // fitted box never leaves the quarter. The bug this guards against is a
    // slice sized by the wrong side, which overflows and is clipped.
    let quarter = (Vec2::ZERO, Vec2::new(320.0, 200.0));
    for (pw, ph) in [(512, 512), (512, 256), (256, 512), (512, 100), (100, 512), (64, 64), (1024, 24), (24, 1024)] {
        let (min, max) = layout::fit(quarter, (pw, ph));
        let (w, h) = (max.x - min.x, max.y - min.y);
        assert!(w <= 320.0 + 1e-3 && h <= 200.0 + 1e-3, "a {pw}x{ph} plane fitted to {w} x {h} overflows 320x200");
        assert!((w / h - pw as f32 / ph.max(1) as f32).abs() < 1e-2, "the {pw}x{ph} plane lost its aspect: {w} x {h}");
        assert!(min.x >= -1e-3 && min.y >= -1e-3, "the fitted slice starts outside the quarter: {min:?}");
    }
}

// covers 26.2-a
#[test]
fn a_plane_of_zero_height_does_not_divide_by_zero() {
    let (min, max) = layout::fit((Vec2::ZERO, Vec2::new(100.0, 100.0)), (10, 0));

    assert!(min.x.is_finite() && max.x.is_finite(), "a zero-height plane gave a non-finite rect");
    assert!((max.x - min.x) > 0.0, "a zero-height plane collapsed the image");
}
