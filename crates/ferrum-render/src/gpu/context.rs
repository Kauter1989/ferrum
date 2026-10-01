//! Device creation and capability detection.

use thiserror::Error;

/// Errors of the GPU layer.
#[derive(Debug, Error)]
pub enum GpuError {
    /// No suitable adapter.
    #[error("no GPU adapter available: {0}")]
    NoAdapter(String),
    /// Device creation failed.
    #[error("cannot create GPU device: {0}")]
    Device(String),
    /// Reading back a texture failed.
    #[error("GPU readback failed: {0}")]
    Readback(String),
}

/// Capabilities that influence resource formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuCaps {
    /// Format of the scalar volume texture.
    pub volume_format: wgpu::TextureFormat,
    /// Largest 3D texture dimension.
    pub max_texture_3d: u32,
}

impl GpuCaps {
    /// Detects capabilities of `device` created from `adapter`.
    pub fn detect(adapter: &wgpu::Adapter, device: &wgpu::Device) -> Self {
        let r16 = wgpu::TextureFormat::R16Unorm;
        let r16_ok = device.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
            && adapter.get_texture_format_features(r16).flags.contains(wgpu::TextureFormatFeatureFlags::FILTERABLE);
        Self {
            volume_format: if r16_ok { r16 } else { wgpu::TextureFormat::R16Float },
            max_texture_3d: device.limits().max_texture_dimension_3d,
        }
    }
}

/// Device descriptor requesting what the viewer benefits from: filterable
/// 16-bit normalised textures and the adapter's full 3D texture/buffer
/// limits.
pub fn device_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
    let mut features = wgpu::Features::empty();
    let r16_filterable = adapter
        .get_texture_format_features(wgpu::TextureFormat::R16Unorm)
        .flags
        .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE);
    if adapter.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM) && r16_filterable {
        features |= wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
    }
    let supported = adapter.limits();
    let base = if adapter.get_downlevel_capabilities().is_webgpu_compliant() {
        wgpu::Limits::default()
    } else {
        wgpu::Limits::downlevel_webgl2_defaults()
    };
    let limits = wgpu::Limits {
        max_texture_dimension_1d: supported.max_texture_dimension_1d,
        max_texture_dimension_2d: supported.max_texture_dimension_2d,
        max_texture_dimension_3d: supported.max_texture_dimension_3d,
        max_buffer_size: supported.max_buffer_size,
        ..base
    };
    wgpu::DeviceDescriptor {
        label: Some("ferrum device"),
        required_features: features,
        required_limits: limits,
        ..Default::default()
    }
}

/// A self-contained device for headless rendering (tests, screenshots,
/// benchmarks). Works with software adapters such as lavapipe.
pub struct GpuContext {
    /// Adapter.
    pub adapter: wgpu::Adapter,
    /// Device.
    pub device: wgpu::Device,
    /// Queue.
    pub queue: wgpu::Queue,
    /// Detected capabilities.
    pub caps: GpuCaps,
}

impl GpuContext {
    /// Creates a headless context, or an error if no adapter exists.
    pub fn headless() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|e| GpuError::NoAdapter(e.to_string()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&device_descriptor(&adapter)))
            .map_err(|e| GpuError::Device(e.to_string()))?;
        let caps = GpuCaps::detect(&adapter, &device);
        Ok(Self { adapter, device, queue, caps })
    }

    /// Adapter description for logs.
    pub fn describe(&self) -> String {
        let i = self.adapter.get_info();
        format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type)
    }
}
