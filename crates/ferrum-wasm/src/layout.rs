//! The MPR layout: where each of the four views goes on the canvas.
//!
//! Pure geometry, testable on the host: `ferrum-wasm` runs only on wasm32, but
//! the arithmetic that decides where a slice lands is where the bugs were
//! (a slice stretched over a square quarter, a slice drawn over its
//! neighbours). Keeping it out of the renderer lets a test call it with real
//! canvas sizes and plane shapes.

use glam::Vec2;

/// The four quarters of the MPR layout, as `(min, max)` in canvas pixels:
/// axial top-left, coronal top-right, sagittal bottom-left, 3D bottom-right.
///
/// The quarters tile the canvas exactly: their areas add up to the canvas, and
/// no two overlap.
pub fn quarters(size: Vec2) -> [(Vec2, Vec2); 4] {
    let (half_w, half_h) = (size.x / 2.0, size.y / 2.0);
    [
        (Vec2::ZERO, Vec2::new(half_w, half_h)),
        (Vec2::new(half_w, 0.0), Vec2::new(size.x, half_h)),
        (Vec2::new(0.0, half_h), Vec2::new(half_w, size.y)),
        (Vec2::new(half_w, half_h), Vec2::new(size.x, size.y)),
    ]
}

/// The largest box inside `rect`, centred in it, with the pixel shape of a
/// plane `plane` pixels wide and `plane` high.
///
/// The shader stretches the image over the rectangle it is given, so a
/// 512 × 256 coronal slice in a square quarter would come out with its
/// anatomy stretched to twice its height: the box must keep the plane's
/// aspect. A plane of zero height takes the box as it is.
pub fn fit(rect: (Vec2, Vec2), plane: (u32, u32)) -> (Vec2, Vec2) {
    let (min, max) = rect;
    let (box_w, box_h) = (max.x - min.x, max.y - min.y);
    let aspect = plane.0 as f32 / plane.1.max(1) as f32;
    // A box wider than the plane is limited by the height, and one narrower
    // than the plane by the width. Sizing by the other side overflows the
    // quarter: a 2:1 plane in a 1.6:1 quarter would come out 400 wide in a
    // 320-wide quarter, and be clipped by the scissor instead of fitted.
    let (w, h) =
        if box_w / box_h.max(1.0) > aspect { (box_h * aspect, box_h) } else { (box_w, box_w / aspect.max(1e-3)) };
    let centre = (min + max) / 2.0;
    (centre - Vec2::new(w, h) / 2.0, centre + Vec2::new(w, h) / 2.0)
}

// The tests live in `tests/layout.rs`: this module is compiled there directly,
// so the cases run on the host even though the crate itself is wasm-only.
