//! Parameters of a computation and the unit of the voxel values.
//!
//! Everything that changes a number is recorded in the result, so two
//! results can be compared only when their parameters agree.

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

/// Parameters of a computation. They grow with the profiles (bin width,
/// HU ranges); today only the preprocessing is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// Preprocessing of the image.
    pub preprocessing: Preprocessing,
}

impl Default for Params {
    fn default() -> Self {
        Self { preprocessing: Preprocessing::None }
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

    #[test]
    fn only_ct_has_a_known_unit() {
        assert_eq!(ValueUnit::from_modality("CT"), ValueUnit::Hu);
        assert_eq!(ValueUnit::from_modality(" ct "), ValueUnit::Hu);
        assert_eq!(ValueUnit::from_modality("").label(), "");
        assert!(!ValueUnit::from_modality("MR").is_known());
        assert_eq!(ValueUnit::Hu.label(), "HU");
    }
}
