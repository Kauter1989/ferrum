//! Distribution of the voxel values: shape of the histogram, deviation
//! measures, the intensity-volume histogram and shares of HU ranges.
//!
//! Skewness, kurtosis, energy, RMS and the deviations are taken on the
//! voxel values; entropy and uniformity on a histogram with a fixed bin
//! width, bins anchored at 0 HU. The intensity-volume histogram is taken on
//! the voxel values, without binning. Everything is the population version
//! (divisor N).

use crate::intensity::{nearest_rank, FirstOrder};
use crate::params::HuRange;

/// Ids of the features [`distribution`] provides.
pub const IDS: [&str; 12] = [
    "intensity.skewness",
    "intensity.kurtosis",
    "intensity.energy",
    "intensity.rms",
    "intensity.mad",
    "intensity.rmad",
    "intensity.entropy",
    "intensity.uniformity",
    "intensity.ivh_v10",
    "intensity.ivh_v90",
    "intensity.ivh_i10",
    "intensity.ivh_i90",
];

/// Distribution measures of a set of values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distribution {
    /// `m3 / m2^1.5`; `None` if all values are equal.
    pub skewness: Option<f64>,
    /// `m4 / m2^2 - 3`; `None` if all values are equal.
    pub kurtosis: Option<f64>,
    /// Sum of squares.
    pub energy: f64,
    /// `sqrt(energy / N)`.
    pub rms: f64,
    /// Mean absolute deviation from the mean.
    pub mad: f64,
    /// Mean absolute deviation of the values between the 10th and 90th
    /// percentile, from their own mean.
    pub rmad: f64,
    /// Entropy of the histogram, in bits.
    pub entropy: f64,
    /// Sum of squared histogram probabilities.
    pub uniformity: f64,
    /// Fraction of the volume at or above 10 % of the intensity range.
    pub ivh_v10: f64,
    /// Fraction of the volume at or above 90 % of the intensity range.
    pub ivh_v90: f64,
    /// Highest intensity reached by at least 10 % of the volume.
    pub ivh_i10: f64,
    /// Highest intensity reached by at least 90 % of the volume.
    pub ivh_i90: f64,
}

/// Fraction of `sorted` at or above `gamma`.
fn fraction_at_or_above(sorted: &[f32], gamma: f64) -> f64 {
    let below = sorted.partition_point(|v| f64::from(*v) < gamma);
    (sorted.len() - below) as f64 / sorted.len() as f64
}

/// The highest value reached by at least `percent` % of `sorted`: the
/// `ceil(percent * N / 100)`-th largest.
fn intensity_at_volume(sorted: &[f32], percent: usize) -> f64 {
    let k = (percent * sorted.len()).div_ceil(100).max(1);
    f64::from(sorted[sorted.len() - k])
}

/// Entropy (bits) and uniformity of the histogram with bins `[k w, (k + 1) w)`.
fn histogram_measures(sorted: &[f32], bin_width: f64) -> (f64, f64) {
    let n = sorted.len() as f64;
    let bin = |v: &f32| (f64::from(*v) / bin_width).floor();
    let (mut entropy, mut uniformity) = (0.0, 0.0);
    for run in sorted.chunk_by(|a, b| bin(a) == bin(b)) {
        let p = run.len() as f64 / n;
        entropy -= p * p.log2();
        uniformity += p * p;
    }
    // a single bin gives -0.0
    (entropy + 0.0, uniformity)
}

/// Mean absolute deviation of the values between the 10th and 90th
/// percentile (nearest rank), from the mean of those values.
fn robust_mad(sorted: &[f32]) -> f64 {
    let (lo, hi) = (nearest_rank(sorted, 0.1), nearest_rank(sorted, 0.9));
    let inner: Vec<f64> = sorted.iter().map(|v| f64::from(*v)).filter(|v| *v >= lo && *v <= hi).collect();
    let mean = inner.iter().sum::<f64>() / inner.len() as f64;
    inner.iter().map(|v| (v - mean).abs()).sum::<f64>() / inner.len() as f64
}

/// The distribution measures of `sorted` (ascending, not empty) with the
/// statistics `first` of the same values.
pub fn distribution(sorted: &[f32], first: &FirstOrder, bin_width: f64) -> Distribution {
    let n = sorted.len() as f64;
    let values = || sorted.iter().map(|v| f64::from(*v));
    let central = |k: i32| values().map(|v| (v - first.mean).powi(k)).sum::<f64>() / n;
    let spread = first.min < first.max && first.std > 0.0;
    let energy: f64 = values().map(|v| v * v).sum();
    let (entropy, uniformity) = histogram_measures(sorted, bin_width);
    let at_range = |percent: f64| fraction_at_or_above(sorted, first.min + percent / 100.0 * first.range());
    Distribution {
        skewness: spread.then(|| central(3) / first.std.powi(3)),
        kurtosis: spread.then(|| central(4) / first.std.powi(4) - 3.0),
        energy,
        rms: (energy / n).sqrt(),
        mad: values().map(|v| (v - first.mean).abs()).sum::<f64>() / n,
        rmad: robust_mad(sorted),
        entropy,
        uniformity,
        ivh_v10: at_range(10.0),
        ivh_v90: at_range(90.0),
        ivh_i10: intensity_at_volume(sorted, 10),
        ivh_i90: intensity_at_volume(sorted, 90),
    }
}

/// The intensity-volume histogram sampled at 0, 1, … 100 % of the intensity
/// range.
#[derive(Debug, Clone, PartialEq)]
pub struct IvhCurve {
    /// Intensity at each sample.
    pub intensity: Vec<f64>,
    /// Fraction of the volume at or above that intensity (starts at 1,
    /// never rises).
    pub volume_fraction: Vec<f64>,
}

/// The curve of `sorted` (ascending, not empty).
pub fn ivh_curve(sorted: &[f32]) -> IvhCurve {
    let (min, max) = (f64::from(sorted[0]), f64::from(sorted[sorted.len() - 1]));
    let intensity: Vec<f64> = (0..=100).map(|p| min + f64::from(p) / 100.0 * (max - min)).collect();
    let volume_fraction = intensity.iter().map(|g| fraction_at_or_above(sorted, *g)).collect();
    IvhCurve { intensity, volume_fraction }
}

/// The share of voxels in one HU range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeShare {
    /// The range `[lo, hi)`.
    pub range: HuRange,
    /// Voxels in it.
    pub voxels: u64,
    /// Their share of all voxels.
    pub fraction: f64,
}

/// Shares of the voxels in a set of HU ranges.
#[derive(Debug, Clone, PartialEq)]
pub struct HuRangeShares {
    /// One entry per range, in the order given.
    pub ranges: Vec<RangeShare>,
    /// Share of the voxels in none of the ranges.
    pub outside: f64,
    /// Voxels in none of the ranges.
    pub outside_voxels: u64,
    /// The ranges are the built-in defaults, not given by the caller.
    pub is_default: bool,
}

/// How many of `sorted` lie in `[lo, hi)`, by two binary searches.
fn count_in(sorted: &[f32], r: HuRange) -> u64 {
    let below = |x: f64| sorted.partition_point(|v| f64::from(*v) < x);
    (below(r.hi) - below(r.lo)) as u64
}

/// The shares of `sorted` (ascending, not empty) in `ranges`, which must
/// not overlap (see `Params::validate`).
pub fn hu_range_shares(sorted: &[f32], ranges: &[HuRange], is_default: bool) -> HuRangeShares {
    let n = sorted.len() as f64;
    let shares: Vec<RangeShare> = ranges
        .iter()
        .map(|r| {
            let voxels = count_in(sorted, *r);
            RangeShare { range: *r, voxels, fraction: voxels as f64 / n }
        })
        .collect();
    let inside: u64 = shares.iter().map(|s| s.voxels).sum();
    let outside_voxels = sorted.len() as u64 - inside;
    HuRangeShares { ranges: shares, outside: outside_voxels as f64 / n, outside_voxels, is_default }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intensity::first_order;

    fn measure(values: &[f32], bin_width: f64) -> (Distribution, Vec<f32>) {
        let mut sorted = values.to_vec();
        sorted.sort_by(f32::total_cmp);
        let first = first_order(sorted.clone()).unwrap();
        (distribution(&sorted, &first, bin_width), sorted)
    }

    fn near(a: f64, b: f64, what: &str) {
        assert!((a - b).abs() < 1e-12, "{what}: {a} != {b}");
    }

    /// Ten voxels, by hand: 2 4 4 4 5 5 7 9 plus 0 and 20 -> sorted 0 2 4 4 4 5 5 7 9 20.
    const TEN: [f32; 10] = [20.0, 4.0, 0.0, 5.0, 4.0, 2.0, 9.0, 4.0, 7.0, 5.0];

    // covers 18.9-a, 18.9-b
    #[test]
    fn ten_voxels_give_the_hand_computed_moments_and_deviations() {
        let (d, _) = measure(&TEN, 5.0);
        // sum 60, mean 6; deviations -6 -4 -2 -2 -2 -1 -1 1 3 14
        // m2 = (36+16+4+4+4+1+1+1+9+196)/10 = 27.2
        // m3 = (-216-64-8-8-8-1-1+1+27+2744)/10 = 246.6
        // m4 = (1296+256+16+16+16+1+1+1+81+38416)/10 = 4010.0
        near(d.skewness.unwrap(), 246.6 / 27.2f64.powf(1.5), "skewness");
        near(d.kurtosis.unwrap(), 4010.0 / (27.2 * 27.2) - 3.0, "excess kurtosis");
        // squares: 0 4 16 16 16 25 25 49 81 400 = 632
        near(d.energy, 632.0, "energy");
        near(d.rms, 63.2f64.sqrt(), "rms");
        // |dev|: 6 4 2 2 2 1 1 1 3 14 = 36
        near(d.mad, 3.6, "mad");
        // nearest rank over 10 values: p10 index round(0.9) = 1 -> 2; p90 index round(8.1) = 8 -> 9;
        // inside: 2 4 4 4 5 5 7 9 (sum 40, mean 5): |dev| 3 1 1 1 0 0 2 4 = 12 -> 1.5
        near(d.rmad, 1.5, "rmad");
    }

    // covers 18.9-a, 18.9-b, 18.9-c
    #[test]
    fn ten_voxels_give_the_hand_computed_entropy_and_uniformity_for_the_bin_width() {
        // width 5: bins [0,5): 0 2 4 4 4 -> 5; [5,10): 5 5 7 9 -> 4; [20,25): 20 -> 1
        let (d, _) = measure(&TEN, 5.0);
        let h = -(0.5f64 * 0.5f64.log2() + 0.4 * 0.4f64.log2() + 0.1 * 0.1f64.log2());
        near(d.entropy, h, "entropy, width 5");
        near(d.uniformity, 0.25 + 0.16 + 0.01, "uniformity, width 5");
        // width 100: one bin
        let (d, _) = measure(&TEN, 100.0);
        assert_eq!((d.entropy, d.uniformity), (0.0, 1.0));
        assert!(d.entropy.is_sign_positive(), "not -0");
        // width 1: values 0 2 4 4 4 5 5 7 9 20 -> bins 0 2 4(x3) 5(x2) 7 9 20
        let (d, _) = measure(&TEN, 1.0);
        let probs = [0.1, 0.1, 0.3, 0.2, 0.1, 0.1, 0.1];
        near(d.entropy, -probs.iter().map(|p: &f64| p * p.log2()).sum::<f64>(), "entropy, width 1");
        near(d.uniformity, probs.iter().map(|p| p * p).sum::<f64>(), "uniformity, width 1");
        // a finer bin never lowers entropy
        let (coarse, _) = measure(&TEN, 5.0);
        assert!(d.entropy >= coarse.entropy);
    }

    #[test]
    fn bins_are_anchored_at_zero_so_negative_values_bin_downwards() {
        // width 10: -1 is in [-10, 0), 1 in [0, 10): two bins, not one
        let (d, _) = measure(&[-1.0, 1.0], 10.0);
        near(d.entropy, 1.0, "two bins of half the voxels");
        let (d, _) = measure(&[-9.0, -1.0], 10.0);
        assert_eq!(d.entropy, 0.0);
    }

    // covers 18.9-d
    #[test]
    fn a_constant_region_has_no_skewness_or_kurtosis_and_finite_everything_else() {
        let (d, _) = measure(&[42.0; 7], 25.0);
        assert_eq!((d.skewness, d.kurtosis), (None, None));
        near(d.energy, 7.0 * 42.0 * 42.0, "energy");
        near(d.rms, 42.0, "rms");
        assert_eq!((d.mad, d.rmad, d.entropy, d.uniformity), (0.0, 0.0, 0.0, 1.0));
        assert_eq!((d.ivh_v10, d.ivh_v90, d.ivh_i10, d.ivh_i90), (1.0, 1.0, 42.0, 42.0));
        let (one, _) = measure(&[-3.0], 25.0);
        assert_eq!((one.skewness, one.kurtosis, one.rms), (None, None, 3.0));
    }

    #[test]
    fn a_symmetric_set_has_no_skew_and_a_flat_one_has_negative_excess_kurtosis() {
        let uniform: Vec<f32> = (0..101).map(|i| i as f32).collect();
        let (d, _) = measure(&uniform, 25.0);
        assert!(d.skewness.unwrap().abs() < 1e-12);
        // continuous uniform has excess kurtosis -1.2; the discrete 0..=100 is a hair below
        assert!((d.kurtosis.unwrap() + 1.2).abs() < 0.01, "{:?}", d.kurtosis);
        // a long right tail skews positive
        let (tail, _) = measure(&[0.0, 0.0, 0.0, 0.0, 100.0], 25.0);
        assert!(tail.skewness.unwrap() > 1.0);
    }

    // covers 18.10-a, 18.10-b
    #[test]
    fn a_linear_ramp_gives_the_exact_volume_and_intensity_figures() {
        // 0..=99 HU, one voxel each: V_x counts the values >= x % of the range (0..99)
        let ramp: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let (d, sorted) = measure(&ramp, 25.0);
        // threshold 9.9 -> values 10..=99 -> 90 voxels; threshold 89.1 -> values 90..=99 -> 10 voxels
        near(d.ivh_v10, 0.9, "V10");
        near(d.ivh_v90, 0.1, "V90");
        // the 10 highest voxels are 90..=99, so 90 is the highest value that 10 % reach;
        // the 90 highest are 10..=99, so 10 is the highest value that 90 % reach
        assert_eq!((d.ivh_i10, d.ivh_i90), (90.0, 10.0));
        let curve = ivh_curve(&sorted);
        assert_eq!((curve.intensity.len(), curve.volume_fraction.len()), (101, 101));
        assert_eq!((curve.volume_fraction[0], curve.intensity[0], curve.intensity[100]), (1.0, 0.0, 99.0));
        near(curve.volume_fraction[10], d.ivh_v10, "curve at 10 %");
        near(curve.volume_fraction[90], d.ivh_v90, "curve at 90 %");
        // the top of the range is reached by the one voxel that holds the maximum
        near(curve.volume_fraction[100], 0.01, "curve at 100 %");
    }

    #[test]
    fn the_ten_voxel_set_gives_the_ranked_intensities() {
        let (d, _) = measure(&TEN, 5.0);
        // 10 % of 10 voxels = the largest (20); 90 % = the 9 largest, whose lowest is 2
        assert_eq!((d.ivh_i10, d.ivh_i90), (20.0, 2.0));
        // range 0..20: V10 counts >= 2 (9 voxels), V90 counts >= 18 (1 voxel)
        assert_eq!((d.ivh_v10, d.ivh_v90), (0.9, 0.1));
    }

    // covers 18.10-a
    #[test]
    fn a_fraction_of_the_volume_that_is_not_a_whole_number_of_voxels_rounds_up() {
        // 15 voxels 1..=15: 10 % of 15 = 1.5 voxels -> the 2 largest are needed, the lower of them is 14;
        // 90 % = 13.5 -> the 14 largest, the lowest of which is 2
        let values: Vec<f32> = (1..=15).map(|i| i as f32).collect();
        let (d, _) = measure(&values, 25.0);
        assert_eq!((d.ivh_i10, d.ivh_i90), (14.0, 2.0));
    }

    // covers 18.10-c
    #[test]
    fn the_curve_never_rises_whatever_the_values() {
        let mut seed = 9u64;
        for len in [1usize, 2, 3, 17, 400] {
            let values: Vec<f32> = (0..len)
                .map(|_| {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    ((seed >> 40) % 3000) as f32 - 1000.0
                })
                .collect();
            let (_, sorted) = measure(&values, 25.0);
            let curve = ivh_curve(&sorted);
            assert!(curve.volume_fraction.windows(2).all(|w| w[1] <= w[0]), "len {len}");
            assert!(curve.intensity.windows(2).all(|w| w[1] >= w[0]));
            assert_eq!(curve.volume_fraction[0], 1.0);
            assert!(curve.volume_fraction.iter().all(|f| *f > 0.0 && *f <= 1.0));
        }
    }

    // covers 18.11-a, 18.11-d
    #[test]
    fn range_shares_are_half_open_and_sum_to_one_with_the_outside() {
        let sorted: Vec<f32> = [-1100.0, -500.0, -100.0, -1.0, 0.0, 0.0, 99.0, 100.0, 299.0, 1000.0].to_vec();
        let ranges = [HuRange::new(-1000.0, -500.0), HuRange::new(-500.0, 0.0), HuRange::new(0.0, 100.0)];
        let s = hu_range_shares(&sorted, &ranges, false);
        // -500 belongs to the second range, not the first; 100 to none of these
        assert_eq!(s.ranges.iter().map(|r| r.voxels).collect::<Vec<_>>(), [0, 3, 3]);
        near(s.ranges[1].fraction, 0.3, "second range");
        near(s.outside, 0.4, "outside: -1100, 100, 299, 1000");
        assert_eq!(s.outside_voxels, 4);
        near(s.ranges.iter().map(|r| r.fraction).sum::<f64>() + s.outside, 1.0, "sum");
        assert!(!s.is_default);
        assert_eq!(s.ranges[0].range, ranges[0], "the order given is kept");
        // ranges that cover everything leave nothing outside
        let all = hu_range_shares(&sorted, &[HuRange::new(-2000.0, 2000.0)], true);
        assert_eq!((all.ranges[0].voxels, all.outside, all.is_default), (10, 0.0, true));
    }
}
