//! The result of a computation.

use crate::feature::{Feature, Unit};
use crate::params::{Params, ValueUnit};

/// A value of one feature.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A finite number.
    Number(f64),
    /// The feature is undefined for this segment (a plane has no volume
    /// axis, a constant region no skewness). NaN and infinity never leave
    /// the crate: they become this, with the reason.
    Null {
        /// Why there is no value.
        reason: String,
    },
}

impl Value {
    /// A number; `Null` with `reason` if it is not finite.
    pub fn number(x: f64, reason: &str) -> Self {
        if x.is_finite() {
            Self::Number(x)
        } else {
            Self::Null { reason: reason.to_owned() }
        }
    }

    /// The number, if there is one.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(x) => Some(*x),
            Self::Null { .. } => None,
        }
    }
}

/// One feature and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureValue {
    /// The feature.
    pub feature: &'static Feature,
    /// Its value.
    pub value: Value,
    /// The unit of the value (`HU` for CT, `mm`, `mL`; empty for counts).
    pub unit: String,
}

/// Features of one segment under one profile.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricsResult {
    /// Name of the profile.
    pub profile: &'static str,
    /// Version of the profile.
    pub profile_version: &'static str,
    /// Unit of the voxel values.
    pub unit: ValueUnit,
    /// Parameters the numbers depend on.
    pub params: Params,
    /// Values in the order of the profile.
    pub features: Vec<FeatureValue>,
    /// Things worth knowing about the numbers.
    pub warnings: Vec<String>,
}

impl MetricsResult {
    /// The value of feature `id`.
    pub fn get(&self, id: &str) -> Option<&FeatureValue> {
        self.features.iter().find(|f| f.feature.id == id)
    }

    /// The number of feature `id`, if it has one.
    pub fn number(&self, id: &str) -> Option<f64> {
        self.get(id).and_then(|f| f.value.as_number())
    }
}

/// Unit label of `feature` for values of `unit`.
pub(crate) fn unit_label(feature: &Feature, unit: &ValueUnit) -> String {
    match feature.unit {
        Unit::Fixed(u) => u.to_owned(),
        Unit::Value => unit.label().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_finite_numbers_become_null_with_a_reason() {
        assert_eq!(Value::number(1.5, "r"), Value::Number(1.5));
        assert_eq!(Value::number(f64::NAN, "undefined"), Value::Null { reason: "undefined".into() });
        assert_eq!(Value::number(f64::INFINITY, "x").as_number(), None);
    }
}
