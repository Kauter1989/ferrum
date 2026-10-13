//! Errors of the crate.

use thiserror::Error;

/// Why a set of metrics could not be computed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MetricsError {
    /// No profile has this name.
    #[error("unknown profile {given:?}; known profiles: {}", known.join(", "))]
    UnknownProfile {
        /// The name that was asked for.
        given: String,
        /// The names that exist.
        known: Vec<&'static str>,
    },
    /// The profile has intensity features, but the unit of the voxel
    /// values is not known (a study that is not CT, or a NIfTI file whose
    /// modality was not declared). No value is computed from a guessed unit.
    #[error("the unit of the voxel values is unknown (modality {modality:?}); needed by {}", features.join(", "))]
    UnitUnknown {
        /// The modality of the study; empty if it carries none.
        modality: String,
        /// The features that need the unit.
        features: Vec<&'static str>,
    },
    /// A parameter is out of range (bin width, HU ranges).
    #[error("{0}")]
    BadParameter(String),
    /// The segment has no voxels.
    #[error("the segment has no voxels")]
    EmptyRegion,
}
