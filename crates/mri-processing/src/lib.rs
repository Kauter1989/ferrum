//! # mri-processing
//!
//! CPU algorithms operating on [`mri_domain::Volume`]. Every algorithm is
//! data-parallel (rayon) and deterministic, so results are identical
//! regardless of the number of worker threads.

pub mod ambient_occlusion;
pub mod bricks;
pub mod filters;
pub mod histogram;
pub mod resample;

pub use ambient_occlusion::{AmbientOcclusion, AoParams};
pub use bricks::BrickGrid;
pub use histogram::Histogram;
