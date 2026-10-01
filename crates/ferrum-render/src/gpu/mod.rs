//! wgpu implementation of the renderer.

pub mod context;
pub mod renderer;

pub use context::{device_descriptor, GpuCaps, GpuContext, GpuError};
pub use renderer::{ViewId, VolumeRenderer, OFFSCREEN_FORMAT};
