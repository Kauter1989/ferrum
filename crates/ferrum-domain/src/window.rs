//! Window / level (VOI) transformation used to display 2D slices.

use crate::volume::IntensityRange;

/// Linear display window in physical units (DICOM "window center / width").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowLevel {
    /// Window centre (level).
    pub center: f32,
    /// Window width (> 0).
    pub width: f32,
}

impl WindowLevel {
    /// Smallest allowed window width.
    pub const MIN_WIDTH: f32 = 1e-3;

    /// Creates a window, clamping the width to [`Self::MIN_WIDTH`].
    pub fn new(center: f32, width: f32) -> Self {
        Self { center, width: width.max(Self::MIN_WIDTH) }
    }

    /// A window covering the whole intensity range.
    pub fn full_range(range: IntensityRange) -> Self {
        Self::new((range.min + range.max) * 0.5, range.span())
    }

    /// Lower bound of the window.
    pub fn lower(&self) -> f32 {
        self.center - self.width * 0.5
    }

    /// Upper bound of the window.
    pub fn upper(&self) -> f32 {
        self.center + self.width * 0.5
    }

    /// Maps a physical value to display intensity in `[0, 1]`.
    pub fn apply(&self, value: f32) -> f32 {
        ((value - self.lower()) / self.width).clamp(0.0, 1.0)
    }

    /// Expresses the window in normalised storage units of `range`, returning
    /// `(lower, upper)` so that shaders can apply it to texture samples.
    pub fn normalized_bounds(&self, range: IntensityRange) -> (f32, f32) {
        ((self.lower() - range.min) / range.span(), (self.upper() - range.min) / range.span())
    }

    /// Mouse-drag interaction used by radiology viewers: horizontal motion
    /// changes the width, vertical motion changes the centre. `dx`/`dy` are
    /// fractions of the viewport; the sensitivity scales with the data range.
    pub fn dragged(&self, dx: f32, dy: f32, range: IntensityRange) -> Self {
        let s = range.span();
        Self::new(self.center - dy * s, self.width + dx * s)
    }

    /// Builds a 256-entry-per-byte lookup table from `u16` storage to 8-bit
    /// grey levels, for fast CPU rendering of slices.
    pub fn build_lut(&self, range: IntensityRange) -> Vec<u8> {
        (0..=u16::MAX).map(|s| crate::color::unit_to_u8(self.apply(range.from_storage(s)))).collect()
    }
}

/// Number of bins of the histogram an automatic window is read from. The
/// desktop app's histogram uses the same count, so that the app and the
/// agent choose the same window ([`WindowLevel::auto`]).
pub const AUTO_WINDOW_BINS: usize = 256;

/// Share of the voxels left below the automatic window.
pub const AUTO_WINDOW_LOW: f64 = 0.005;

/// Share of the voxels at or below the top of the automatic window.
pub const AUTO_WINDOW_HIGH: f64 = 0.995;

impl WindowLevel {
    /// The window from the 0.5 and 99.5 percentiles of the histogram
    /// `counts` (uniform bins over the normalised storage range of `range`,
    /// e.g. [`storage_histogram`]): robust to a few outlier voxels, which
    /// matter in MR and PET where no window is stored with the data.
    pub fn auto(range: IntensityRange, counts: &[u64]) -> Self {
        let lo = histogram_percentile(counts, AUTO_WINDOW_LOW);
        let hi = histogram_percentile(counts, AUTO_WINDOW_HIGH).max(lo + 1e-3);
        let (a, b) = (range.denormalize(lo), range.denormalize(hi));
        Self::new((a + b) * 0.5, b - a)
    }
}

/// The bin of a normalised `u16` storage sample in a histogram of `bins`
/// uniform bins over `[0, 1]`.
#[inline]
pub fn storage_bin(sample: u16, bins: usize) -> usize {
    ((sample as usize * bins) >> 16).min(bins.max(1) - 1)
}

/// Histogram of normalised `u16` storage samples with `bins` uniform bins
/// (at least 1).
pub fn storage_histogram(samples: &[u16], bins: usize) -> Vec<u64> {
    let bins = bins.max(1);
    let mut counts = vec![0u64; bins];
    for &s in samples {
        counts[storage_bin(s, bins)] += 1;
    }
    counts
}

/// Normalised intensity (upper edge of a bin) below which `fraction` of
/// the samples of the histogram `counts` lie; `1` for an empty histogram.
pub fn histogram_percentile(counts: &[u64], fraction: f64) -> f32 {
    let total: u64 = counts.iter().sum();
    let target = (fraction.clamp(0.0, 1.0) * total as f64).ceil() as u64;
    let mut acc = 0;
    for (i, &c) in counts.iter().enumerate() {
        acc += c;
        if acc >= target && acc > 0 {
            return (i as f32 + 1.0) / counts.len() as f32;
        }
    }
    1.0
}

/// Well-known window presets (values in Hounsfield units for CT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowPreset {
    /// Whole data range.
    FullRange,
    /// Brain (C 40, W 80).
    Brain,
    /// Soft tissue / abdomen (C 50, W 400).
    SoftTissue,
    /// Lung (C -600, W 1500).
    Lung,
    /// Bone (C 400, W 1800).
    Bone,
}

impl WindowPreset {
    /// All presets in display order.
    pub const ALL: [WindowPreset; 5] = [
        WindowPreset::FullRange,
        WindowPreset::Brain,
        WindowPreset::SoftTissue,
        WindowPreset::Lung,
        WindowPreset::Bone,
    ];

    /// Human-readable name.
    pub fn label(&self) -> &'static str {
        match self {
            WindowPreset::FullRange => "Full range",
            WindowPreset::Brain => "Brain",
            WindowPreset::SoftTissue => "Soft tissue",
            WindowPreset::Lung => "Lung",
            WindowPreset::Bone => "Bone",
        }
    }

    /// Resolves the preset for a given data range.
    pub fn window(&self, range: IntensityRange) -> WindowLevel {
        match self {
            WindowPreset::FullRange => WindowLevel::full_range(range),
            WindowPreset::Brain => WindowLevel::new(40.0, 80.0),
            WindowPreset::SoftTissue => WindowLevel::new(50.0, 400.0),
            WindowPreset::Lung => WindowLevel::new(-600.0, 1500.0),
            WindowPreset::Bone => WindowLevel::new(400.0, 1800.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_auto_window_ignores_a_few_outliers() {
        // covers 2.7-h
        let range = IntensityRange::new(0.0, 1000.0).unwrap();
        let mut samples: Vec<u16> = (0..1000).map(|i| range.to_storage(100.0 + (i % 100) as f32)).collect();
        samples.push(range.to_storage(1000.0));
        let w = WindowLevel::auto(range, &storage_histogram(&samples, AUTO_WINDOW_BINS));
        // Bins of 1000/256: upper edges of the bins holding 100 (25) and 199 (50).
        assert!((w.lower() - 101.5625).abs() < 1e-3, "{w:?}");
        assert!((w.upper() - 199.21875).abs() < 1e-3, "{w:?}");
        assert_ne!(w, WindowLevel::full_range(range));
    }

    #[test]
    fn percentiles_of_an_empty_histogram_are_the_top() {
        assert_eq!(histogram_percentile(&[0, 0], 0.5), 1.0);
        assert_eq!(storage_histogram(&[0, u16::MAX], 0), vec![2]);
        assert_eq!(storage_histogram(&[0, u16::MAX], 10), vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn apply_is_linear_and_clamped() {
        let w = WindowLevel::new(100.0, 200.0);
        assert_eq!(w.apply(0.0), 0.0);
        assert_eq!(w.apply(-50.0), 0.0);
        assert_eq!(w.apply(100.0), 0.5);
        assert_eq!(w.apply(200.0), 1.0);
        assert_eq!(w.apply(500.0), 1.0);
    }

    #[test]
    fn width_never_zero() {
        assert!(WindowLevel::new(0.0, 0.0).width > 0.0);
        assert!(WindowLevel::new(0.0, -5.0).width > 0.0);
    }

    #[test]
    fn normalized_bounds_of_full_range() {
        let r = IntensityRange::new(-1000.0, 1000.0).unwrap();
        let (lo, hi) = WindowLevel::full_range(r).normalized_bounds(r);
        assert!((lo - 0.0).abs() < 1e-6 && (hi - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lut_matches_apply() {
        let r = IntensityRange::new(0.0, 1000.0).unwrap();
        let w = WindowLevel::new(500.0, 500.0);
        let lut = w.build_lut(r);
        assert_eq!(lut.len(), 65536);
        assert_eq!(lut[0], 0);
        assert_eq!(lut[65535], 255);
        assert_eq!(lut[r.to_storage(500.0) as usize], 128);
    }

    #[test]
    fn drag_changes_width_and_center() {
        let r = IntensityRange::new(0.0, 100.0).unwrap();
        let w = WindowLevel::new(50.0, 50.0);
        let d = w.dragged(0.1, 0.1, r);
        assert!((d.width - 60.0).abs() < 1e-4);
        assert!((d.center - 40.0).abs() < 1e-4);
    }

    #[test]
    fn presets_resolve() {
        let r = IntensityRange::new(-1024.0, 3071.0).unwrap();
        for p in WindowPreset::ALL {
            let w = p.window(r);
            assert!(w.width > 0.0, "{}", p.label());
        }
    }
}
