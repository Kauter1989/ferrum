//! CPU reference renderer.

pub mod raycast;
pub mod slice;

pub use raycast::{trilinear, CpuRaycaster, CpuScene, SegmentLayer};
pub use slice::slice_overlay;
