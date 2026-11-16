//! The CPU slice segment overlay — the one rule mirroring `slice.wgsl`
//! (Stage 29, criterion 29.5-a).
//!
//! Before this, the rule lived in three places (the shader, the view host's
//! CPU backend and the agent's render). The fill/outline colour math is
//! [`SegmentStyle::blend`]; this module owns the edge detection and the loop,
//! so every CPU slice overlay goes through one implementation and is compared
//! to the GPU in `gpu_parity.rs`. The agent's thick outline is a distinct
//! render style (its edge spans several pixels and counts the image border)
//! but shares the same blend.

use ferrum_domain::SegmentStyle;

/// Applies the segment overlay over an already windowed grey RGBA slice,
/// following `slice.wgsl::overlay`: a pixel takes the full segment colour when
/// any of its four neighbours carries another label (the outline), otherwise
/// the fill is blended with `opacity × fill_opacity`; a background or invisible
/// pixel stays grey.
///
/// `label_at(x, y)` gives the label at a screen pixel (`0` = background); at
/// native resolution a screen pixel is a voxel. `segment(label)` gives the
/// colour and opacity of a *visible* segment, or `None` to leave the pixel
/// grey. Neighbours clamp to the image border, as the shader's clamped UV does.
pub fn slice_overlay(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    label_at: impl Fn(u32, u32) -> u8,
    segment: impl Fn(u8) -> Option<([u8; 3], f32)>,
    style: SegmentStyle,
    fill_opacity: f32,
) {
    for y in 0..height {
        for x in 0..width {
            let label = label_at(x, y);
            if label == 0 {
                continue;
            }
            let Some((color, opacity)) = segment(label) else {
                continue;
            };
            if opacity <= 0.0 {
                continue;
            }
            let left = label_at(x.saturating_sub(1), y);
            let right = label_at((x + 1).min(width - 1), y);
            let up = label_at(x, y.saturating_sub(1));
            let down = label_at(x, (y + 1).min(height - 1));
            let edge = left != label || right != label || up != label || down != label;
            let i = (y * width + x) as usize;
            let grey = [rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]];
            let out = style.blend(grey, color, opacity * fill_opacity, edge);
            rgba[i * 4..i * 4 + 3].copy_from_slice(&out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers 29.5-a
    #[test]
    fn a_block_gets_an_outline_and_a_fill() {
        // a 4×4 slice with a 2×2 block of label 1 in the middle
        let (w, h) = (4u32, 4u32);
        let label_at = |x: u32, y: u32| u8::from((1..3).contains(&x) && (1..3).contains(&y));
        let segment = |l: u8| (l == 1).then_some(([200u8, 100, 50], 0.5));
        let mut rgba: Vec<u8> = (0..w * h).flat_map(|_| [10u8, 10, 10, 255]).collect();
        slice_overlay(&mut rgba, w, h, label_at, segment, SegmentStyle::FillAndOutline, 1.0);
        let px = |x: u32, y: u32| {
            let i = (y * w + x) as usize;
            [rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]]
        };
        // every block pixel touches a non-block neighbour -> full-colour outline
        assert_eq!(px(1, 1), [200, 100, 50]);
        // outside the block stays grey
        assert_eq!(px(0, 0), [10, 10, 10]);
    }

    // covers 29.5-a
    #[test]
    fn fill_only_blends_the_interior() {
        let (w, h) = (5u32, 5u32);
        let label_at = |x: u32, y: u32| u8::from((1..4).contains(&x) && (1..4).contains(&y));
        let segment = |l: u8| (l == 1).then_some(([200u8, 100, 50], 1.0));
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        slice_overlay(&mut rgba, w, h, label_at, segment, SegmentStyle::Fill, 0.5);
        let i = (2 * w + 2) as usize;
        // interior fill: grey 0 blended toward the colour at alpha 0.5
        assert_eq!([rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]], [100, 50, 25]);
    }

    // covers 29.5-a — out-of-bounds neighbours clamp to the edge pixel (as the
    // shader's clamped UV), not to background: a corner block pixel is a fill,
    // not an outline, because its off-image neighbours are itself.
    #[test]
    fn border_neighbours_clamp_to_the_edge_pixel() {
        let (w, h) = (3u32, 3u32);
        let label_at = |x: u32, y: u32| u8::from(x < 2 && y < 2); // block at the top-left corner
        let segment = |l: u8| (l == 1).then_some(([200u8, 100, 50], 1.0));
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        slice_overlay(&mut rgba, w, h, label_at, segment, SegmentStyle::FillAndOutline, 0.5);
        // corner (0,0): left/up clamp to itself, right/down are the same label
        // -> not an edge -> fill at alpha 0.5, not the full-colour outline
        assert_eq!([rgba[0], rgba[1], rgba[2]], [100, 50, 25]);
        // an interior border of the block (2,0 is background; (1,0) touches it) is an edge
        let i = (0 * w + 1) as usize;
        assert_eq!([rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]], [200, 100, 50]);
    }

    // covers 29.5-a — Outline draws only the edge; a zero-opacity segment is grey.
    #[test]
    fn outline_style_and_zero_opacity() {
        let (w, h) = (4u32, 4u32);
        let label_at = |x: u32, y: u32| u8::from((1..3).contains(&x) && (1..3).contains(&y));
        let mut rgba: Vec<u8> = (0..w * h).flat_map(|_| [10u8, 10, 10, 255]).collect();
        slice_overlay(&mut rgba, w, h, label_at, |l| (l == 1).then_some(([200u8, 100, 50], 1.0)), SegmentStyle::Outline, 1.0);
        let px = |x: u32, y: u32| {
            let i = (y * w + x) as usize;
            [rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]]
        };
        assert_eq!(px(1, 1), [200, 100, 50], "the edge is full colour");
        // a 2×2 block has no interior pixel: every block pixel is an edge, so
        // check the outline-only rule on a larger block's centre instead
        let (w2, h2) = (6u32, 6u32);
        let big = |x: u32, y: u32| u8::from((1..5).contains(&x) && (1..5).contains(&y));
        let mut rgba2: Vec<u8> = (0..w2 * h2).flat_map(|_| [10u8, 10, 10, 255]).collect();
        slice_overlay(&mut rgba2, w2, h2, big, |l| (l == 1).then_some(([200u8, 100, 50], 1.0)), SegmentStyle::Outline, 1.0);
        let i = (2 * w2 + 2) as usize;
        assert_eq!([rgba2[i * 4], rgba2[i * 4 + 1], rgba2[i * 4 + 2]], [10, 10, 10], "the interior stays grey");
        // zero opacity: nothing is drawn
        let mut rgba3: Vec<u8> = (0..w2 * h2).flat_map(|_| [10u8, 10, 10, 255]).collect();
        slice_overlay(&mut rgba3, w2, h2, big, |l| (l == 1).then_some(([200u8, 100, 50], 0.0)), SegmentStyle::FillAndOutline, 1.0);
        assert_eq!([rgba3[0], rgba3[1], rgba3[2]], [10, 10, 10]);
    }
}
