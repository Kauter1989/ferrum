//! Intensity histogram used by the transfer function editor and for
//! automatic windowing.

use ferrum_domain::Volume;
use rayon::prelude::*;

/// Histogram of normalised intensities with uniform bins over `[0, 1]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    counts: Vec<u64>,
    total: u64,
}

impl Histogram {
    /// Computes a histogram of `volume` with `bins` bins (at least 1).
    pub fn compute(volume: &Volume, bins: usize) -> Self {
        Self::from_samples(volume.data(), bins)
    }

    /// Computes a histogram of raw normalised `u16` samples.
    pub fn from_samples(samples: &[u16], bins: usize) -> Self {
        let bins = bins.max(1);
        let counts = samples
            .par_chunks(1 << 16)
            .fold(
                || vec![0u64; bins],
                |mut acc, chunk| {
                    for &s in chunk {
                        acc[bin_of(s, bins)] += 1;
                    }
                    acc
                },
            )
            .reduce(
                || vec![0u64; bins],
                |mut a, b| {
                    a.iter_mut().zip(b).for_each(|(x, y)| *x += y);
                    a
                },
            );
        Self { counts, total: samples.len() as u64 }
    }

    /// Raw counts.
    pub fn counts(&self) -> &[u64] {
        &self.counts
    }

    /// Number of bins.
    pub fn bins(&self) -> usize {
        self.counts.len()
    }

    /// Total number of samples.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Counts normalised to the maximum bin, optionally log-compressed
    /// (`ln(1 + c) / ln(1 + max)`), in `[0, 1]` — ready for plotting.
    pub fn normalized(&self, log_scale: bool) -> Vec<f32> {
        let max = self.counts.iter().copied().max().unwrap_or(0);
        if max == 0 {
            return vec![0.0; self.counts.len()];
        }
        self.counts
            .iter()
            .map(|&c| {
                if log_scale {
                    ((c as f64).ln_1p() / (max as f64).ln_1p()) as f32
                } else {
                    (c as f64 / max as f64) as f32
                }
            })
            .collect()
    }

    /// Gaussian-smoothed counts (as floats) with standard deviation `sigma`
    /// bins.
    pub fn smoothed(&self, sigma: f32) -> Vec<f32> {
        let src: Vec<f32> = self.counts.iter().map(|&c| c as f32).collect();
        if sigma <= 0.0 {
            return src;
        }
        let radius = (3.0 * sigma).ceil() as isize;
        let kernel: Vec<f32> = (-radius..=radius).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let n = src.len() as isize;
        (0..n)
            .map(|i| {
                let mut acc = 0.0;
                let mut wsum = 0.0;
                for (k, w) in kernel.iter().enumerate() {
                    let j = i + k as isize - radius;
                    if (0..n).contains(&j) {
                        acc += src[j as usize] * w;
                        wsum += w;
                    }
                }
                acc / wsum
            })
            .collect()
    }

    /// Indices of strict local maxima of the smoothed histogram, ignoring
    /// the first bin (usually background air), sorted by height descending.
    pub fn peaks(&self, sigma: f32) -> Vec<usize> {
        let s = self.smoothed(sigma);
        let mut peaks: Vec<usize> =
            (2..s.len().saturating_sub(1)).filter(|&i| s[i] > s[i - 1] && s[i] >= s[i + 1] && s[i] > 0.0).collect();
        peaks.sort_by(|&a, &b| s[b].total_cmp(&s[a]));
        peaks
    }

    /// Normalised intensity below which `fraction` of the samples lie.
    pub fn percentile(&self, fraction: f64) -> f32 {
        let target = (fraction.clamp(0.0, 1.0) * self.total as f64).ceil() as u64;
        let mut acc = 0;
        for (i, &c) in self.counts.iter().enumerate() {
            acc += c;
            if acc >= target && acc > 0 {
                return (i as f32 + 1.0) / self.counts.len() as f32;
            }
        }
        1.0
    }
}

#[inline]
fn bin_of(sample: u16, bins: usize) -> usize {
    ((sample as usize * bins) >> 16).min(bins - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_sum_to_total() {
        let samples: Vec<u16> = (0..=u16::MAX).collect();
        let h = Histogram::from_samples(&samples, 256);
        assert_eq!(h.total(), 65536);
        assert_eq!(h.counts().iter().sum::<u64>(), 65536);
        assert!(h.counts().iter().all(|&c| c == 256));
    }

    #[test]
    fn extreme_samples_land_in_edge_bins() {
        let h = Histogram::from_samples(&[0, u16::MAX], 10);
        assert_eq!(h.counts()[0], 1);
        assert_eq!(h.counts()[9], 1);
    }

    #[test]
    fn normalized_scales_to_one() {
        let h = Histogram::from_samples(&[0, 0, 0, u16::MAX], 2);
        assert_eq!(h.normalized(false), vec![1.0, 1.0 / 3.0]);
        let l = h.normalized(true);
        assert_eq!(l[0], 1.0);
        assert!(l[1] > 1.0 / 3.0 && l[1] < 1.0);
        assert_eq!(Histogram::from_samples(&[], 4).normalized(true), vec![0.0; 4]);
    }

    #[test]
    fn smoothing_preserves_mass_away_from_borders() {
        let mut samples = vec![32768u16; 1000];
        samples.extend(vec![0u16; 10]);
        let h = Histogram::from_samples(&samples, 64);
        let s = h.smoothed(2.0);
        let mass: f32 = s[20..44].iter().sum();
        assert!((mass - 1000.0).abs() < 1.0, "{mass}");
    }

    #[test]
    fn detects_two_peaks() {
        let mut samples = vec![16384u16; 500];
        samples.extend(vec![49152u16; 800]);
        let h = Histogram::from_samples(&samples, 64);
        let p = h.peaks(1.0);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0], 48);
        assert_eq!(p[1], 16);
    }

    #[test]
    fn percentile_bounds() {
        let samples: Vec<u16> = (0..=u16::MAX).collect();
        let h = Histogram::from_samples(&samples, 100);
        assert!((h.percentile(0.5) - 0.5).abs() < 0.02);
        assert!(h.percentile(1.0) <= 1.0);
        assert!(h.percentile(0.0) > 0.0);
    }
}
