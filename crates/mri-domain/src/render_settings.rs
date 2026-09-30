//! User-controllable parameters of the 3D renderer.

use crate::color::Rgb;
use crate::transfer::TransferFunction;

/// 3D rendering technique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RenderMode {
    /// Emission/absorption rendering of a tissue band followed by a shaded
    /// isosurface ("Volume render" of the original viewer).
    #[default]
    Tissue,
    /// Shaded first-hit isosurface.
    Isosurface,
    /// Maximum intensity projection.
    Mip,
    /// Direct volume rendering driven by the editable transfer function.
    TransferFunction,
}

impl RenderMode {
    /// All modes in display order.
    pub const ALL: [RenderMode; 4] =
        [RenderMode::Tissue, RenderMode::Isosurface, RenderMode::Mip, RenderMode::TransferFunction];

    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            RenderMode::Tissue => "Tissue",
            RenderMode::Isosurface => "Isosurface",
            RenderMode::Mip => "MIP",
            RenderMode::TransferFunction => "Transfer function",
        }
    }
}

/// Thresholds of the [`RenderMode::Tissue`] technique, in normalised
/// intensity. `low..high` is a triangular opacity band; rays stop at the
/// first sample above `surface` and shade an isosurface there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TissueThresholds {
    /// Start of the translucent band.
    pub low: f32,
    /// End of the translucent band.
    pub high: f32,
    /// Opaque surface threshold.
    pub surface: f32,
}

impl TissueThresholds {
    /// Creates thresholds, sorting them so `low <= high <= surface`.
    pub fn new(a: f32, b: f32, c: f32) -> Self {
        let mut v = [a.clamp(0.0, 1.0), b.clamp(0.0, 1.0), c.clamp(0.0, 1.0)];
        v.sort_by(f32::total_cmp);
        Self { low: v[0], high: v[1], surface: v[2] }
    }

    /// Triangular opacity of the tissue band at intensity `v` (peak `1` in
    /// the middle of the band, `0` outside).
    pub fn band_opacity(&self, v: f32) -> f32 {
        let w = self.high - self.low;
        if w <= 0.0 {
            return 0.0;
        }
        ((v - self.low).min(self.high - v) / w).max(0.0) * 2.0
    }
}

impl Default for TissueThresholds {
    fn default() -> Self {
        // Values of the original viewer's RGB sliders (0.09, 0.3, 0.46).
        Self::new(0.09, 0.3, 0.46)
    }
}

/// Complete set of 3D rendering parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSettings {
    /// Active technique.
    pub mode: RenderMode,
    /// Global opacity multiplier in `[0, 1]`.
    pub opacity: f32,
    /// Brightness in `[0, 1]`.
    pub brightness: f32,
    /// Quality in `[0, 1]`, controls the ray-marching step size.
    pub quality: f32,
    /// Isosurface threshold (normalised intensity).
    pub iso_threshold: f32,
    /// Tissue technique thresholds.
    pub tissue: TissueThresholds,
    /// Opacity of the raw slice drawn on a cut surface, `[0, 1]`.
    pub cut_surface_opacity: f32,
    /// Whether ambient occlusion shading is enabled.
    pub ambient_occlusion: bool,
    /// Colour of the tissue band at its low end.
    pub tissue_color_low: Rgb,
    /// Colour of the tissue band at its high end.
    pub tissue_color_high: Rgb,
    /// Surface colour when facing the light.
    pub surface_color_lit: Rgb,
    /// Surface colour when facing away from the light.
    pub surface_color_shadow: Rgb,
    /// Use empty-space skipping acceleration.
    pub empty_space_skipping: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            mode: RenderMode::default(),
            opacity: 0.53,
            brightness: 0.56,
            quality: 0.35,
            iso_threshold: 0.46,
            tissue: TissueThresholds::default(),
            cut_surface_opacity: 0.0,
            ambient_occlusion: false,
            tissue_color_low: Rgb::new(0.1, 0.0, 0.0),
            tissue_color_high: Rgb::new(1.0, 0.0, 0.0),
            surface_color_lit: Rgb::new(1.0, 0.902, 0.773),
            surface_color_shadow: Rgb::new(0.5, 0.4, 0.3),
            empty_space_skipping: true,
        }
    }
}

impl RenderSettings {
    /// Ray-marching step in model units (model box longest side = 1):
    /// `1 / (100 + 700 * quality)`, i.e. 100…800 samples per box length.
    pub fn step_size(&self) -> f32 {
        1.0 / (100.0 + 700.0 * self.quality.clamp(0.0, 1.0))
    }

    /// Reference step for opacity correction (the step at quality 0.5).
    pub const REFERENCE_STEP: f32 = 1.0 / 450.0;

    /// Returns `true` if a region whose normalised values span `[min, max]`
    /// can contribute to the image in the current mode. Regions for which
    /// this is `false` may be skipped by the ray marcher (empty-space
    /// skipping) without changing the result.
    pub fn range_visible(&self, min: f32, max: f32, tf: &TransferFunction) -> bool {
        match self.mode {
            RenderMode::Isosurface => max > self.iso_threshold,
            RenderMode::Tissue => max > self.tissue.low,
            RenderMode::Mip => max > 0.0,
            RenderMode::TransferFunction => tf.max_opacity_in(min, max) > 0.0,
        }
    }

    /// The lowest normalised intensity that can contribute to the image in
    /// the current mode; used by empty-space skipping.
    pub fn visibility_threshold(&self) -> f32 {
        match self.mode {
            RenderMode::Tissue => self.tissue.low.min(self.tissue.surface),
            RenderMode::Isosurface => self.iso_threshold,
            RenderMode::Mip | RenderMode::TransferFunction => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_size_matches_legacy_formula() {
        let mut s = RenderSettings::default();
        s.quality = 0.0;
        assert!((s.step_size() - 0.01).abs() < 1e-7);
        s.quality = 1.0;
        assert!((s.step_size() - 1.0 / 800.0).abs() < 1e-7);
        s.quality = 5.0;
        assert!((s.step_size() - 1.0 / 800.0).abs() < 1e-7);
    }

    #[test]
    fn thresholds_are_sorted() {
        let t = TissueThresholds::new(0.5, 0.1, 0.3);
        assert_eq!((t.low, t.high, t.surface), (0.1, 0.3, 0.5));
    }

    #[test]
    fn band_opacity_is_triangle() {
        let t = TissueThresholds::new(0.2, 0.4, 0.9);
        assert_eq!(t.band_opacity(0.1), 0.0);
        assert!((t.band_opacity(0.3) - 1.0).abs() < 1e-6);
        assert!((t.band_opacity(0.25) - 0.5).abs() < 1e-6);
        assert_eq!(t.band_opacity(0.5), 0.0);
        let degenerate = TissueThresholds::new(0.3, 0.3, 0.3);
        assert_eq!(degenerate.band_opacity(0.3), 0.0);
    }

    #[test]
    fn visibility_threshold_per_mode() {
        let mut s = RenderSettings::default();
        s.mode = RenderMode::Isosurface;
        assert_eq!(s.visibility_threshold(), s.iso_threshold);
        s.mode = RenderMode::Mip;
        assert_eq!(s.visibility_threshold(), 0.0);
        s.mode = RenderMode::Tissue;
        assert_eq!(s.visibility_threshold(), s.tissue.low);
    }
}
