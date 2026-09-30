//! # mri-render
//!
//! Rendering layer of the viewer.
//!
//! * [`frame`] — pure per-frame parameters derived from domain state, shared
//!   by the GPU and CPU renderers.
//! * [`gpu`] — wgpu implementation: single-pass ray casting (WGSL) with
//!   analytic clipping, empty-space skipping and ambient occlusion, 2D slice
//!   rendering straight from the 3D texture, dynamic-resolution presentation
//!   and headless off-screen rendering.
//! * [`cpu`] — reference ray caster used as a test oracle, for picking and
//!   as a software fallback.

pub mod cpu;
pub mod frame;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod shaders;

pub use frame::{FrameParams, SliceParams};
