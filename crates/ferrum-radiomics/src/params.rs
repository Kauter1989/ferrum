//! Parameters of a computation and the unit of the voxel values.
//!
//! Everything that changes a number is recorded in the result, so two
//! results can be compared only when their parameters agree.

use crate::error::MetricsError;

/// Preprocessing applied to the image before features are computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preprocessing {
    /// The voxel values and the grid are used as they are.
    None,
}

impl Preprocessing {
    /// Name recorded in the result.
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
        }
    }
}

/// Width of a histogram bin in HU when none is given. Bins are anchored at
/// 0 HU (a bin is `[k * width, (k + 1) * width)`), so segments of different
/// studies are binned alike.
pub const DEFAULT_BIN_WIDTH: f64 = 25.0;

/// A half-open range of values `[lo, hi)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HuRange {
    /// Lowest value in the range.
    pub lo: f64,
    /// First value above the range.
    pub hi: f64,
}

impl HuRange {
    /// The range `[lo, hi)`.
    pub const fn new(lo: f64, hi: f64) -> Self {
        Self { lo, hi }
    }
}

/// Ranges used when the caller gives none: orientation bins of 100 to 500
/// HU, not clinical thresholds. Values outside all of them are reported
/// as "outside".
pub const DEFAULT_HU_RANGES: [HuRange; 6] = [
    HuRange::new(-1000.0, -500.0),
    HuRange::new(-500.0, -100.0),
    HuRange::new(-100.0, 0.0),
    HuRange::new(0.0, 100.0),
    HuRange::new(100.0, 300.0),
    HuRange::new(300.0, 1000.0),
];

/// Parameters of a computation, all recorded in the result.
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    /// Preprocessing of the image.
    pub preprocessing: Preprocessing,
    /// Histogram bin width, in the unit of the values (HU).
    pub bin_width: f64,
    /// HU ranges to report shares for; `None` for [`DEFAULT_HU_RANGES`].
    pub hu_ranges: Option<Vec<HuRange>>,
}

impl Default for Params {
    fn default() -> Self {
        Self { preprocessing: Preprocessing::None, bin_width: DEFAULT_BIN_WIDTH, hu_ranges: None }
    }
}

impl Params {
    /// The ranges in use and whether they are the defaults.
    pub fn ranges(&self) -> (Vec<HuRange>, bool) {
        match &self.hu_ranges {
            Some(r) => (r.clone(), false),
            None => (DEFAULT_HU_RANGES.to_vec(), true),
        }
    }

    /// Checks the parameters: a positive finite bin width, and ranges that
    /// are finite, non-empty and do not overlap.
    pub fn validate(&self) -> Result<(), MetricsError> {
        let bad = |what: String| Err(MetricsError::BadParameter(what));
        if !(self.bin_width.is_finite() && self.bin_width > 0.0) {
            return bad(format!("bin_width must be a positive number, got {}", self.bin_width));
        }
        let (ranges, _) = self.ranges();
        if let Some(r) = ranges.iter().find(|r| !(r.lo.is_finite() && r.hi.is_finite() && r.lo < r.hi)) {
            return bad(format!("a HU range must be finite with min below max, got [{}, {})", r.lo, r.hi));
        }
        let mut sorted = ranges;
        sorted.sort_by(|a, b| a.lo.total_cmp(&b.lo));
        match sorted.windows(2).find(|w| w[1].lo < w[0].hi) {
            Some(w) => bad(format!(
                "HU ranges [{}, {}) and [{}, {}) overlap; ranges are half-open and must not share values",
                w[0].lo, w[0].hi, w[1].lo, w[1].hi
            )),
            None => Ok(()),
        }
    }
}

/// Unit of the voxel values of a study.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueUnit {
    /// Hounsfield units: the study is CT.
    Hu,
    /// Not known: a study that is not CT, or one whose modality was not
    /// declared (NIfTI carries none).
    Unknown {
        /// The modality of the study; empty if it carries none.
        modality: String,
    },
}

impl ValueUnit {
    /// The unit of a study of `modality` (`CT` gives HU, case-insensitive).
    pub fn from_modality(modality: &str) -> Self {
        if modality.trim().eq_ignore_ascii_case("CT") {
            Self::Hu
        } else {
            Self::Unknown { modality: modality.trim().to_owned() }
        }
    }

    /// Label of the unit; empty if it is not known.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Hu => "HU",
            Self::Unknown { .. } => "",
        }
    }

    /// `true` if the unit is known.
    pub fn is_known(&self) -> bool {
        matches!(self, Self::Hu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers 18.9-c, 18.11-b
    #[test]
    fn parameters_are_checked_and_ranges_may_touch_but_not_overlap() {
        assert!(Params::default().validate().is_ok());
        assert!(Params::default().ranges().1);
        let with = |bin_width: f64, r: &[(f64, f64)]| Params {
            bin_width,
            hu_ranges: Some(r.iter().map(|(a, b)| HuRange::new(*a, *b)).collect()),
            ..Params::default()
        };
        assert!(with(10.0, &[(0.0, 50.0), (50.0, 100.0)]).validate().is_ok(), "half-open ranges may touch");
        assert!(with(10.0, &[(100.0, 200.0), (0.0, 50.0)]).validate().is_ok(), "order does not matter");
        for (w, r) in [(0.0, vec![]), (-1.0, vec![]), (f64::NAN, vec![]), (f64::INFINITY, vec![])] {
            assert!(matches!(with(w, &r).validate(), Err(MetricsError::BadParameter(m)) if m.contains("bin_width")));
        }
        for r in
            [vec![(0.0, 60.0), (50.0, 100.0)], vec![(0.0, 100.0), (20.0, 30.0)], vec![(5.0, 5.0)], vec![(9.0, 1.0)]]
        {
            assert!(matches!(with(10.0, &r).validate(), Err(MetricsError::BadParameter(_))), "{r:?}");
        }
        match with(10.0, &[(0.0, 60.0), (50.0, 100.0)]).validate() {
            Err(MetricsError::BadParameter(m)) => assert!(m.contains("overlap"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(with(10.0, &[(f64::NAN, 1.0)]).validate(), Err(MetricsError::BadParameter(_))));
    }

    #[test]
    fn only_ct_has_a_known_unit() {
        assert_eq!(ValueUnit::from_modality("CT"), ValueUnit::Hu);
        assert_eq!(ValueUnit::from_modality(" ct "), ValueUnit::Hu);
        assert_eq!(ValueUnit::from_modality("").label(), "");
        assert!(!ValueUnit::from_modality("MR").is_known());
        assert_eq!(ValueUnit::Hu.label(), "HU");
    }
}
