//! Glue between egui-wgpu and `mri_render::gpu::VolumeRenderer`.
//!
//! The renderer lives in egui-wgpu's `callback_resources`; views draw
//! through paint callbacks, and the application pushes data changes through
//! [`RendererSink`], the presentation-side implementation of the
//! [`mri_app::GpuSink`] port.

use glam::{UVec2, UVec3};
use mri_app::GpuSink;
use mri_domain::{Dims3, Rgba8, Volume, VoxelMask};
use mri_processing::AmbientOcclusion;
use mri_render::gpu::{GpuCaps, ViewId, VolumeRenderer};
use mri_render::{FrameParams, SliceParams};

/// View ids of the panels.
pub mod view_ids {
    use mri_render::gpu::ViewId;

    /// 3D view.
    pub const VOLUME: ViewId = 100;

    /// Slice view of the given orientation id.
    pub fn slice(axis_id: u32) -> ViewId {
        u64::from(axis_id) + 1
    }
}

/// Creates the renderer inside egui-wgpu's resources.
pub fn install(render_state: &egui_wgpu::RenderState) {
    let caps = GpuCaps::detect(&render_state.adapter, &render_state.device);
    log::info!(
        "GPU: {} — volume format {:?}, max 3D texture {}",
        render_state.adapter.get_info().name,
        caps.volume_format,
        caps.max_texture_3d
    );
    let renderer = VolumeRenderer::new(&render_state.device, &render_state.queue, caps, render_state.target_format);
    render_state.renderer.write().callback_resources.insert(renderer);
}

/// [`GpuSink`] backed by the renderer.
pub struct RendererSink<'a> {
    /// Device.
    pub device: &'a wgpu::Device,
    /// Queue.
    pub queue: &'a wgpu::Queue,
    /// Renderer.
    pub renderer: &'a mut VolumeRenderer,
}

impl GpuSink for RendererSink<'_> {
    fn upload_volume(&mut self, volume: &Volume) {
        self.renderer.set_volume(self.device, self.queue, volume);
    }

    fn upload_transfer_function(&mut self, lut: &[Rgba8]) {
        self.renderer.set_transfer_function(self.queue, lut);
    }

    fn upload_occupancy(&mut self, grid: Dims3, occupancy: &[u8]) {
        self.renderer.set_occupancy(self.device, self.queue, grid, occupancy);
    }

    fn upload_mask(&mut self, mask: &VoxelMask, dirty: Option<(UVec3, UVec3)>) {
        self.renderer.update_mask(self.device, self.queue, mask, dirty);
    }

    fn clear_mask(&mut self) {
        self.renderer.reset_mask(self.device, self.queue);
    }

    fn upload_ambient_occlusion(&mut self, ao: Option<&AmbientOcclusion>) {
        self.renderer.set_ambient_occlusion(self.device, self.queue, ao);
    }
}

/// Paint callback of the 3D view: ray casts into an off-screen target at
/// the (dynamic) render size during `prepare`, blits it during `paint`.
pub struct VolumeCallback {
    /// View id.
    pub id: ViewId,
    /// Frame parameters.
    pub params: FrameParams,
    /// Render size in pixels.
    pub size: UVec2,
}

impl egui_wgpu::CallbackTrait for VolumeCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(r) = resources.get_mut::<VolumeRenderer>() {
            r.render_volume(device, queue, encoder, self.id, &self.params, self.size);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(r) = resources.get::<VolumeRenderer>() {
            r.paint_volume(pass, self.id);
        }
    }
}

/// Paint callback of a 2D slice view.
pub struct SliceCallback {
    /// View id.
    pub id: ViewId,
    /// Slice parameters (rect in framebuffer pixels).
    pub params: SliceParams,
}

impl egui_wgpu::CallbackTrait for SliceCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(r) = resources.get_mut::<VolumeRenderer>() {
            r.prepare_slice(device, queue, self.id, &self.params);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(r) = resources.get::<VolumeRenderer>() {
            r.paint_slice(pass, self.id);
        }
    }
}
