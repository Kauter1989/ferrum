//! WGSL sources embedded at compile time.

/// Ray-casting shader.
pub const VOLUME_WGSL: &str = include_str!("volume.wgsl");
/// 2D slice shader.
pub const SLICE_WGSL: &str = include_str!("slice.wgsl");
/// Blit shader.
pub const BLIT_WGSL: &str = include_str!("blit.wgsl");
