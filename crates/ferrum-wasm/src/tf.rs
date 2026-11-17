//! The transfer-function editor's data: the wire form of the control points
//! and the histogram the editor draws behind the curve.
//!
//! Pure data, testable on the host: `ferrum-wasm` runs only on wasm32, but the
//! shape of what crosses to the panel is where a silent drift would hide (a
//! swapped colour channel, a histogram that is not normalised). The edits
//! themselves are the domain's [`TransferFunction`] API — this module only
//! shapes the read side, and the binding re-uploads the baked lookup table
//! after every edit.

use ferrum_domain::{storage_histogram, TransferFunction, Volume};

/// Floats per control point in [`points`]: position, opacity, red, green, blue.
pub const POINT_STRIDE: usize = 5;

/// The control points as a flat `f32` array, [`POINT_STRIDE`] floats each:
/// position and opacity in `[0, 1]`, then the colour's red, green and blue in
/// `[0, 1]`. Sorted by position, as the domain keeps them; the first point is
/// at `0` and the last at `1`.
pub fn points(tf: &TransferFunction) -> Vec<f32> {
    let mut out = Vec::with_capacity(tf.points().len() * POINT_STRIDE);
    for p in tf.points() {
        out.extend_from_slice(&[p.position, p.opacity, p.color.r, p.color.g, p.color.b]);
    }
    out
}

/// The volume's storage histogram: [`TransferFunction::LUT_SIZE`] uniform bins
/// over the normalised `u16` storage, each count divided by the largest so the
/// editor can draw it as a background in `[0, 1]`. A volume with no samples
/// gives all zeros.
pub fn histogram(volume: &Volume) -> Vec<f32> {
    let counts = storage_histogram(volume.data(), TransferFunction::LUT_SIZE);
    let max = counts.iter().copied().max().unwrap_or(0);
    if max == 0 {
        return vec![0.0; counts.len()];
    }
    counts.iter().map(|&c| c as f32 / max as f32).collect()
}

// The tests live in `tests/tf_editor.rs`: this module is compiled there
// directly, so the cases run on the host even though the crate is wasm-only.
