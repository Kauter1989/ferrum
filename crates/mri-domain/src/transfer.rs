//! Piecewise-linear 1D transfer function mapping normalised intensity to
//! colour and opacity.

use thiserror::Error;

use crate::color::{unit_to_u8, Rgb, Rgba8};
use crate::volume::IntensityRange;

/// A control point of a [`TransferFunction`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlPoint {
    /// Normalised intensity in `[0, 1]`.
    pub position: f32,
    /// Colour at this position.
    pub color: Rgb,
    /// Opacity in `[0, 1]`.
    pub opacity: f32,
}

impl ControlPoint {
    /// Creates a control point.
    pub const fn new(position: f32, color: Rgb, opacity: f32) -> Self {
        Self { position, color, opacity }
    }
}

/// CT presets defined in Hounsfield units, mapped onto the data range of
/// the loaded volume by [`TransferFunction::ct_preset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtPreset {
    /// Skin and muscle as translucent shells, bone opaque.
    SoftTissueBone,
    /// Lung parenchyma faint, pulmonary vessels and airway walls red, bone white.
    LungVessels,
    /// Only bone.
    Bone,
}

impl CtPreset {
    /// All presets.
    pub const ALL: [CtPreset; 3] = [CtPreset::SoftTissueBone, CtPreset::LungVessels, CtPreset::Bone];

    /// Label.
    pub fn label(&self) -> &'static str {
        match self {
            CtPreset::SoftTissueBone => "CT soft tissue + bone",
            CtPreset::LungVessels => "CT lung vessels",
            CtPreset::Bone => "CT bone",
        }
    }

    /// Control points `(HU, colour, opacity)`.
    fn points(&self) -> Vec<(f32, Rgb, f32)> {
        let c = Rgb::new;
        match self {
            CtPreset::SoftTissueBone => vec![
                (-700.0, c(0.0, 0.0, 0.0), 0.0),
                (-450.0, c(0.75, 0.45, 0.30), 0.0),
                (-150.0, c(0.93, 0.66, 0.50), 0.020),
                (20.0, c(0.80, 0.30, 0.22), 0.010),
                (80.0, c(0.85, 0.28, 0.20), 0.060),
                (180.0, c(0.95, 0.85, 0.70), 0.010),
                (300.0, c(1.0, 0.95, 0.85), 0.55),
                (1500.0, c(1.0, 1.0, 1.0), 0.85),
            ],
            CtPreset::LungVessels => vec![
                (-950.0, c(0.0, 0.0, 0.0), 0.0),
                (-850.0, c(0.55, 0.60, 0.75), 0.004),
                (-500.0, c(0.85, 0.55, 0.55), 0.015),
                (-300.0, c(0.95, 0.35, 0.30), 0.10),
                (-120.0, c(0.95, 0.70, 0.55), 0.015),
                (20.0, c(0.85, 0.15, 0.12), 0.12),
                (100.0, c(0.85, 0.12, 0.10), 0.30),
                (180.0, c(0.95, 0.85, 0.70), 0.05),
                (300.0, c(1.0, 0.96, 0.88), 0.60),
                (1500.0, c(1.0, 1.0, 1.0), 0.85),
            ],
            CtPreset::Bone => vec![
                (150.0, c(0.0, 0.0, 0.0), 0.0),
                (250.0, c(0.95, 0.80, 0.65), 0.15),
                (450.0, c(1.0, 0.96, 0.88), 0.80),
                (1500.0, c(1.0, 1.0, 1.0), 0.95),
            ],
        }
    }
}

/// Validation errors of transfer function edits.
#[derive(Debug, Error, PartialEq)]
pub enum TransferFunctionError {
    /// Fewer than two control points.
    #[error("a transfer function needs at least two control points")]
    TooFewPoints,
    /// Control points are not sorted by position.
    #[error("control points must be sorted by position")]
    Unsorted,
    /// The first/last control point is not at 0/1.
    #[error("the first and last control points must lie at 0 and 1")]
    OpenEnds,
    /// A value is outside `[0, 1]`.
    #[error("positions and opacities must lie in [0, 1]")]
    OutOfRange,
    /// The index does not refer to an existing control point.
    #[error("control point index {0} is out of range")]
    BadIndex(usize),
    /// The operation would remove an endpoint.
    #[error("endpoints cannot be removed")]
    EndpointRemoval,
}

/// Piecewise-linear transfer function with at least two control points, the
/// first at position `0` and the last at `1`.
#[derive(Debug, Clone, PartialEq)]
pub struct TransferFunction {
    points: Vec<ControlPoint>,
}

impl TransferFunction {
    /// Number of entries of the lookup table uploaded to the GPU.
    pub const LUT_SIZE: usize = 256;

    /// Creates a validated transfer function.
    pub fn new(points: Vec<ControlPoint>) -> Result<Self, TransferFunctionError> {
        if points.len() < 2 {
            return Err(TransferFunctionError::TooFewPoints);
        }
        let in_unit = |v: f32| (0.0..=1.0).contains(&v);
        if points.iter().any(|p| !in_unit(p.position) || !in_unit(p.opacity)) {
            return Err(TransferFunctionError::OutOfRange);
        }
        if points.windows(2).any(|w| w[0].position > w[1].position) {
            return Err(TransferFunctionError::Unsorted);
        }
        let first = points.first().map(|p| p.position);
        let last = points.last().map(|p| p.position);
        if first != Some(0.0) || last != Some(1.0) {
            return Err(TransferFunctionError::OpenEnds);
        }
        Ok(Self { points })
    }

    /// The default preset inherited from the original web viewer
    /// (ten handles tuned for head MRI / CT).
    pub fn legacy_default() -> Self {
        const XS: [f32; 10] = [0.0, 22.0, 40.0, 55.0, 61.0, 115.0, 118.0, 125.0, 160.0, 255.0];
        const YS: [f32; 10] = [0.0, 0.0, 0.3, 0.12, 0.0, 0.0, 0.4, 0.8, 0.95, 1.0];
        const COLORS: [(u8, u8, u8); 10] = [
            (0, 0, 0),
            (255, 128, 64),
            (255, 0, 0),
            (128, 64, 64),
            (128, 0, 0),
            (64, 64, 64),
            (128, 128, 128),
            (192, 192, 192),
            (255, 255, 255),
            (255, 255, 255),
        ];
        let points = XS
            .iter()
            .zip(YS)
            .zip(COLORS)
            .map(|((x, y), (r, g, b))| ControlPoint::new(x / 255.0, Rgb::from_u8(r, g, b), y))
            .collect();
        Self { points }
    }

    /// A grey ramp with linearly increasing opacity.
    pub fn linear_ramp() -> Self {
        Self { points: vec![ControlPoint::new(0.0, Rgb::BLACK, 0.0), ControlPoint::new(1.0, Rgb::WHITE, 1.0)] }
    }

    /// Bone-like preset: transparent soft tissue, opaque ivory above `start`.
    pub fn bone(start: f32) -> Self {
        let s = start.clamp(0.01, 0.98);
        Self {
            points: vec![
                ControlPoint::new(0.0, Rgb::BLACK, 0.0),
                ControlPoint::new(s, Rgb::new(0.9, 0.5, 0.3), 0.0),
                ControlPoint::new((s + 0.05).min(0.99), Rgb::new(1.0, 0.95, 0.85), 0.8),
                ControlPoint::new(1.0, Rgb::WHITE, 1.0),
            ],
        }
    }

    /// Builds a CT preset for a volume whose storage covers `range` (HU).
    /// Control points outside the range are clipped away.
    pub fn ct_preset(preset: CtPreset, range: IntensityRange) -> Self {
        let pts = preset.points();
        let first = pts.first().map(|p| (p.1, p.2)).unwrap_or((Rgb::BLACK, 0.0));
        let last = pts.last().map(|p| (p.1, p.2)).unwrap_or((Rgb::WHITE, 1.0));
        let mut points = vec![ControlPoint::new(0.0, first.0, first.1)];
        let mut prev = 0.0f32;
        for (hu, color, opacity) in pts {
            let x = (hu - range.min) / range.span();
            if x > prev && x < 1.0 {
                points.push(ControlPoint::new(x, color, opacity));
                prev = x;
            }
        }
        // Colour/opacity at the upper end of the data range.
        let tf = Self { points: points.clone() };
        let end = if prev < 1.0 { last } else { tf.sample(1.0) };
        points.push(ControlPoint::new(1.0, end.0, end.1));
        Self { points }
    }

    /// Control points (sorted by position).
    pub fn points(&self) -> &[ControlPoint] {
        &self.points
    }

    /// Evaluates colour and opacity at normalised intensity `x`.
    pub fn sample(&self, x: f32) -> (Rgb, f32) {
        let x = x.clamp(0.0, 1.0);
        // First point whose position is >= x; guaranteed to exist since the
        // last point is at 1.
        let hi = self.points.partition_point(|p| p.position < x).min(self.points.len() - 1);
        if hi == 0 {
            let p = self.points[0];
            return (p.color, p.opacity);
        }
        let (a, b) = (self.points[hi - 1], self.points[hi]);
        let span = b.position - a.position;
        let t = if span > 0.0 { (x - a.position) / span } else { 1.0 };
        (a.color.lerp(b.color, t), a.opacity + (b.opacity - a.opacity) * t)
    }

    /// Bakes the function into an RGBA8 lookup table of `size` entries, where
    /// entry `i` corresponds to intensity `i / (size - 1)`.
    pub fn bake(&self, size: usize) -> Vec<Rgba8> {
        let denom = (size.max(2) - 1) as f32;
        (0..size)
            .map(|i| {
                let (c, a) = self.sample(i as f32 / denom);
                [unit_to_u8(c.r), unit_to_u8(c.g), unit_to_u8(c.b), unit_to_u8(a)]
            })
            .collect()
    }

    /// Maximum opacity over the closed intensity interval `[lo, hi]`.
    ///
    /// Used to classify empty space: a region whose values all map to zero
    /// opacity can be skipped by the ray marcher.
    pub fn max_opacity_in(&self, lo: f32, hi: f32) -> f32 {
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let ends = self.sample(lo).1.max(self.sample(hi).1);
        self.points.iter().filter(|p| p.position > lo && p.position < hi).map(|p| p.opacity).fold(ends, f32::max)
    }

    /// Moves control point `index` to `(position, opacity)`.
    ///
    /// Endpoints keep their position; interior points are clamped between
    /// their neighbours so that the ordering invariant is preserved.
    pub fn move_point(&mut self, index: usize, position: f32, opacity: f32) -> Result<(), TransferFunctionError> {
        let n = self.points.len();
        if index >= n {
            return Err(TransferFunctionError::BadIndex(index));
        }
        let pos = if index == 0 {
            0.0
        } else if index == n - 1 {
            1.0
        } else {
            position.clamp(self.points[index - 1].position, self.points[index + 1].position)
        };
        let p = &mut self.points[index];
        p.position = pos;
        p.opacity = opacity.clamp(0.0, 1.0);
        Ok(())
    }

    /// Sets the colour of control point `index`.
    pub fn set_color(&mut self, index: usize, color: Rgb) -> Result<(), TransferFunctionError> {
        let p = self.points.get_mut(index).ok_or(TransferFunctionError::BadIndex(index))?;
        p.color = color.clamped();
        Ok(())
    }

    /// Inserts a control point at `position` whose colour and opacity equal
    /// the current function value there (so the curve is unchanged).
    /// Returns the index of the new point.
    pub fn insert_point(&mut self, position: f32) -> usize {
        let position = position.clamp(0.0, 1.0);
        let (color, opacity) = self.sample(position);
        let idx = self.points.partition_point(|p| p.position <= position).clamp(1, self.points.len() - 1);
        self.points.insert(idx, ControlPoint::new(position, color, opacity));
        idx
    }

    /// Removes an interior control point.
    pub fn remove_point(&mut self, index: usize) -> Result<(), TransferFunctionError> {
        let n = self.points.len();
        if index >= n {
            return Err(TransferFunctionError::BadIndex(index));
        }
        if index == 0 || index == n - 1 {
            return Err(TransferFunctionError::EndpointRemoval);
        }
        self.points.remove(index);
        Ok(())
    }

    /// Index of the control point nearest to `(position, opacity)` within
    /// `radius` (Euclidean distance in the unit square), if any.
    pub fn hit_test(&self, position: f32, opacity: f32, radius: f32) -> Option<usize> {
        self.points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let d = ((p.position - position).powi(2) + (p.opacity - opacity).powi(2)).sqrt();
                (i, d)
            })
            .filter(|(_, d)| *d <= radius)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }
}

impl Default for TransferFunction {
    fn default() -> Self {
        Self::legacy_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        let p = |x: f32, a: f32| ControlPoint::new(x, Rgb::BLACK, a);
        assert_eq!(TransferFunction::new(vec![p(0.0, 0.0)]), Err(TransferFunctionError::TooFewPoints));
        assert_eq!(
            TransferFunction::new(vec![p(0.0, 0.0), p(0.6, 0.0), p(0.5, 0.0), p(1.0, 0.0)]),
            Err(TransferFunctionError::Unsorted)
        );
        assert_eq!(TransferFunction::new(vec![p(0.1, 0.0), p(1.0, 0.0)]), Err(TransferFunctionError::OpenEnds));
        assert_eq!(TransferFunction::new(vec![p(0.0, 1.5), p(1.0, 0.0)]), Err(TransferFunctionError::OutOfRange));
        assert!(TransferFunction::new(vec![p(0.0, 0.0), p(1.0, 1.0)]).is_ok());
    }

    #[test]
    fn presets_are_valid() {
        for tf in [TransferFunction::legacy_default(), TransferFunction::linear_ramp(), TransferFunction::bone(0.4)] {
            assert!(TransferFunction::new(tf.points().to_vec()).is_ok());
        }
    }

    #[test]
    fn ct_presets_are_valid_and_placed_in_hounsfield_units() {
        let range = IntensityRange::new(-1024.0, 3071.0).unwrap();
        for p in CtPreset::ALL {
            let tf = TransferFunction::ct_preset(p, range);
            assert!(TransferFunction::new(tf.points().to_vec()).is_ok(), "{p:?}");
        }
        let bone = TransferFunction::ct_preset(CtPreset::Bone, range);
        // air and water are transparent, dense bone is opaque
        assert_eq!(bone.sample(range.normalize(-1000.0)).1, 0.0);
        assert_eq!(bone.sample(range.normalize(0.0)).1, 0.0);
        assert!(bone.sample(range.normalize(1000.0)).1 > 0.8);
        // a narrow range clips points but stays valid
        let narrow = IntensityRange::new(-100.0, 200.0).unwrap();
        for p in CtPreset::ALL {
            assert!(TransferFunction::new(TransferFunction::ct_preset(p, narrow).points().to_vec()).is_ok());
        }
    }

    #[test]
    fn sample_interpolates() {
        let tf = TransferFunction::linear_ramp();
        let (c, a) = tf.sample(0.25);
        assert!((a - 0.25).abs() < 1e-6);
        assert!((c.r - 0.25).abs() < 1e-6);
        assert_eq!(tf.sample(-1.0).1, 0.0);
        assert_eq!(tf.sample(2.0).1, 1.0);
    }

    #[test]
    fn bake_endpoints() {
        let lut = TransferFunction::linear_ramp().bake(256);
        assert_eq!(lut.len(), 256);
        assert_eq!(lut[0], [0, 0, 0, 0]);
        assert_eq!(lut[255], [255, 255, 255, 255]);
        assert_eq!(lut[128][3], 128);
    }

    #[test]
    fn legacy_default_matches_original_handles() {
        let tf = TransferFunction::legacy_default();
        assert_eq!(tf.points().len(), 10);
        // handle #2 of the web viewer: x=40, opacity 0.3, colour red
        let p = tf.points()[2];
        assert!((p.position - 40.0 / 255.0).abs() < 1e-6);
        assert_eq!(p.opacity, 0.3);
        assert_eq!(p.color, Rgb::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn move_point_respects_neighbours_and_endpoints() {
        let mut tf = TransferFunction::legacy_default();
        let left = tf.points()[2].position;
        tf.move_point(3, 0.0, 2.0).unwrap();
        assert_eq!(tf.points()[3].position, left);
        assert_eq!(tf.points()[3].opacity, 1.0);
        tf.move_point(0, 0.5, 0.5).unwrap();
        assert_eq!(tf.points()[0].position, 0.0);
        let n = tf.points().len();
        tf.move_point(n - 1, 0.2, 0.5).unwrap();
        assert_eq!(tf.points()[n - 1].position, 1.0);
        assert_eq!(tf.move_point(99, 0.0, 0.0), Err(TransferFunctionError::BadIndex(99)));
    }

    #[test]
    fn insert_keeps_curve_and_remove_restores() {
        let mut tf = TransferFunction::linear_ramp();
        let before = tf.sample(0.3);
        let idx = tf.insert_point(0.4);
        assert_eq!(idx, 1);
        assert_eq!(tf.sample(0.3), before);
        tf.remove_point(idx).unwrap();
        assert_eq!(tf, TransferFunction::linear_ramp());
        assert_eq!(tf.remove_point(0), Err(TransferFunctionError::EndpointRemoval));
    }

    #[test]
    fn max_opacity_detects_peaks_inside_interval() {
        let tf = TransferFunction::legacy_default();
        // Interval covering handle #2 (opacity 0.3) but with zero-ish ends.
        assert!((tf.max_opacity_in(30.0 / 255.0, 50.0 / 255.0) - 0.3).abs() < 1e-6);
        // Transparent region between handles 4 and 5.
        assert_eq!(tf.max_opacity_in(62.0 / 255.0, 110.0 / 255.0), 0.0);
    }

    #[test]
    fn hit_test_picks_nearest() {
        let tf = TransferFunction::linear_ramp();
        assert_eq!(tf.hit_test(0.01, 0.0, 0.05), Some(0));
        assert_eq!(tf.hit_test(0.99, 0.98, 0.05), Some(1));
        assert_eq!(tf.hit_test(0.5, 0.5, 0.05), None);
    }
}
