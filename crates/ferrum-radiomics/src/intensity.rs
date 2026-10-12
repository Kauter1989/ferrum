//! First-order statistics of the voxel values inside a segment.

use ferrum_domain::{MaskRegion, Volume};

/// Count, centre, spread and percentiles of a set of values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FirstOrder {
    /// Number of values.
    pub count: u64,
    /// Mean.
    pub mean: f64,
    /// Population standard deviation (divisor N).
    pub std: f64,
    /// Lowest value.
    pub min: f64,
    /// Highest value.
    pub max: f64,
    /// 5th percentile (nearest rank).
    pub p5: f64,
    /// 25th percentile (nearest rank).
    pub p25: f64,
    /// Median (nearest rank).
    pub p50: f64,
    /// 75th percentile (nearest rank).
    pub p75: f64,
    /// 95th percentile (nearest rank).
    pub p95: f64,
}

impl FirstOrder {
    /// `max - min`.
    pub fn range(&self) -> f64 {
        self.max - self.min
    }

    /// `p75 - p25`.
    pub fn iqr(&self) -> f64 {
        self.p75 - self.p25
    }
}

/// The physical values of the voxels of `region`, in grid order (`k`
/// outermost, `i` fastest). The region must lie inside the volume.
pub fn region_values(region: &MaskRegion, volume: &Volume) -> Vec<f32> {
    let (lo, hi) = (region.bx.min, region.bx.max);
    let mut out = Vec::with_capacity(region.count() as usize);
    let mut n = 0;
    for k in lo.z..hi.z {
        for j in lo.y..hi.y {
            for i in lo.x..hi.x {
                if region.inside[n] {
                    out.extend(volume.physical(i, j, k));
                }
                n += 1;
            }
        }
    }
    out
}

/// Statistics of `values`; `None` if there are none. The mean is summed in
/// the order given, so the same values in the same order always give the
/// same bits.
pub fn first_order(mut values: Vec<f32>) -> Option<FirstOrder> {
    let n = values.len();
    if n == 0 {
        return None;
    }
    let mean = values.iter().map(|v| f64::from(*v)).sum::<f64>() / n as f64;
    let var = values.iter().map(|v| (f64::from(*v) - mean).powi(2)).sum::<f64>() / n as f64;
    values.sort_by(f32::total_cmp);
    let pct = |q: f64| f64::from(values[((n - 1) as f64 * q).round() as usize]);
    Some(FirstOrder {
        count: n as u64,
        mean,
        std: var.sqrt(),
        min: f64::from(values[0]),
        max: f64::from(values[n - 1]),
        p5: pct(0.05),
        p25: pct(0.25),
        p50: pct(0.5),
        p75: pct(0.75),
        p95: pct(0.95),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> Vec<f32> {
        // 0..=100 shuffled: order must not matter for the percentiles
        (0..=100).map(|i| ((i * 37) % 101) as f32).collect()
    }

    // covers 18.3-a
    #[test]
    fn first_order_matches_the_stats_command_on_its_phantom() {
        // the phantom of the `stats` contract test: 0..=100
        let f = first_order(ramp()).unwrap();
        assert_eq!((f.count, f.min, f.max), (101, 0.0, 100.0));
        assert_eq!((f.p5, f.p25, f.p50, f.p75, f.p95), (5.0, 25.0, 50.0, 75.0, 95.0));
        assert!((f.mean - 50.0).abs() < 1e-9);
        assert!((f.std - 29.154_759).abs() < 1e-5, "{}", f.std);
    }

    // covers 18.3-b
    #[test]
    fn range_and_iqr_are_exact_on_a_hand_computed_set() {
        // sorted: 2 4 4 4 5 5 7 9 -> n = 8; index round(7q): p25 = 1.75 -> 2 -> 4, p75 = 5.25 -> 5 -> 5
        let f = first_order(vec![2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]).unwrap();
        assert_eq!((f.mean, f.std), (5.0, 2.0));
        assert_eq!((f.range(), f.iqr()), (7.0, 1.0));
        assert_eq!(f.p50, 5.0);
    }

    #[test]
    fn nothing_to_measure_is_none_and_one_value_has_no_spread() {
        assert!(first_order(vec![]).is_none());
        let one = first_order(vec![42.0]).unwrap();
        assert_eq!((one.mean, one.std, one.p5, one.p95, one.range(), one.iqr()), (42.0, 0.0, 42.0, 42.0, 0.0, 0.0));
    }
}
