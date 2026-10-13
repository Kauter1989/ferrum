//! Quantitative description of a segment.
//!
//! A *feature* has a stable id, a unit and a definition text
//! ([`feature`]). A *profile* is a versioned list of features
//! ([`profile`]). [`compute`] measures one segment under one profile and
//! returns a [`MetricsResult`] that records the profile, its version and
//! every parameter the numbers depend on.
//!
//! The crate is pure: no I/O, no GPU, no UI. It depends on
//! `ferrum-domain` only and reuses its mask analysis, so each measure has
//! one implementation (ADR 0011).
//!
//! Values come from voxels. A measure that is undefined for a segment is
//! [`Value::Null`] with a reason; NaN and infinity never leave the crate.

pub mod compute;
pub mod diameter;
pub mod distribution;
pub mod error;
pub mod feature;
pub mod hull;
pub mod intensity;
pub mod mesh;
pub mod params;
pub mod pca;
pub mod profile;
pub mod result;
pub mod shape;
pub mod size;
pub mod topology;

pub use compute::{compute, Input};
pub use distribution::{HuRangeShares, IvhCurve, RangeShare};
pub use error::MetricsError;
pub use feature::{feature_by_id, Family, Feature, Unit, FEATURES};
pub use params::{HuRange, Params, Preprocessing, ValueUnit, DEFAULT_BIN_WIDTH, DEFAULT_HU_RANGES};
pub use profile::{Profile, CLINICAL, PROFILES, QUICK};
pub use result::{FeatureValue, MetricsResult, Value};

#[cfg(test)]
mod tests;
