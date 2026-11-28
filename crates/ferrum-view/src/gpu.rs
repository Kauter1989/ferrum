//! The GPU backend: `ferrum-render`'s wgpu renderer, off-screen.
//!
//! It draws the same frames as the desktop — `slice.wgsl` for a slice,
//! `volume.wgsl` for the 3D view — and reads them back, so a host process
//! without a window still uses the machine's GPU and the rendering rule of
//! the repository is unchanged. The data uploads go through the
//! [`GpuSink`] port exactly as the desktop's presentation layer does
//! (`Viewer::sync_gpu` decides what changed); the MPR grid is tiled by the
//! one compositor of [`crate::frame`].

use ferrum_app::{GpuSink, GpuSyncState, Viewer};
use ferrum_domain::{Dims3, LabelMap, Rgba8, SliceAxis, ViewComponent, Volume, VoxelBox, VoxelMask};
use ferrum_processing::AmbientOcclusion;
use ferrum_render::gpu::{GpuContext, VolumeRenderer, OFFSCREEN_FORMAT};
use glam::{UVec2, UVec3};

use crate::frame::{
    composite_mpr, mapping_of, projection_mapping, slice_setup, Backend, FrameSource, Quarter, RawFrame, VOLUME_SIZE,
};
use crate::session::ViewError;

/// Draws the view components with the wgpu renderer, off-screen.
pub struct GpuFrameSource {
    ctx: GpuContext,
    renderer: VolumeRenderer,
    sync: GpuSyncState,
}

impl GpuFrameSource {
    /// Creates a headless GPU context and the renderer on it.
    pub fn new() -> Result<Self, ViewError> {
        let ctx = GpuContext::headless().map_err(|e| ViewError::Gpu(e.to_string()))?;
        let renderer = VolumeRenderer::new(&ctx.device, &ctx.queue, ctx.caps, OFFSCREEN_FORMAT);
        Ok(Self { ctx, renderer, sync: GpuSyncState::default() })
    }

    /// The adapter, for a log line: driver name, backend and limits.
    pub fn describe(&self) -> String {
        self.ctx.describe()
    }

    /// One slice at native resolution (nearest sampling, the full rectangle,
    /// an opaque background — one pixel per voxel).
    fn render_slice(&mut self, viewer: &Viewer, axis: SliceAxis) -> Result<RawFrame, ViewError> {
        let setup = slice_setup(viewer, axis)?;
        let image = self
            .renderer
            .render_slice_image(&self.ctx.device, &self.ctx.queue, &setup.params, setup.size)
            .map_err(|e| ViewError::Gpu(e.to_string()))?;
        let rgba: Vec<u8> = image.into_iter().flatten().collect();
        if rgba.len() != (setup.size.x * setup.size.y * 4) as usize {
            return Err(ViewError::Gpu("the readback has the wrong size".into()));
        }
        Ok(RawFrame {
            width: setup.size.x,
            height: setup.size.y,
            rgba,
            window: viewer.window(),
            mapping: mapping_of(viewer, axis, setup.size)?,
        })
    }

    /// The 3D view at a fixed size, with the viewer's camera and settings.
    /// Jitter is off so the frame is deterministic and comparable with the
    /// CPU reference pixel by pixel.
    fn render_volume(&mut self, viewer: &Viewer, size: UVec2) -> Result<RawFrame, ViewError> {
        let mut params = viewer
            .frame_params(size.as_vec2())
            .ok_or_else(|| ViewError::Internal("the viewer has no 3D parameters".into()))?;
        params.jitter = false;
        let image = self
            .renderer
            .render_volume_image(&self.ctx.device, &self.ctx.queue, &params, size)
            .map_err(|e| ViewError::Gpu(e.to_string()))?;
        let rgba: Vec<u8> = image.into_iter().flatten().collect();
        if rgba.len() != (size.x * size.y * 4) as usize {
            return Err(ViewError::Gpu("the readback has the wrong size".into()));
        }
        Ok(RawFrame {
            width: size.x,
            height: size.y,
            rgba,
            window: viewer.window(),
            mapping: projection_mapping(viewer, size)?,
        })
    }
}

impl FrameSource for GpuFrameSource {
    fn backend(&self) -> Backend {
        Backend::Gpu
    }

    fn render(&mut self, viewer: &mut Viewer, component: ViewComponent) -> Result<RawFrame, ViewError> {
        {
            let mut sink = Sink { device: &self.ctx.device, queue: &self.ctx.queue, renderer: &mut self.renderer };
            viewer.sync_gpu(&mut self.sync, &mut sink);
        }
        self.ctx.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| ViewError::Gpu(e.to_string()))?;
        match component {
            ViewComponent::Slice => self.render_slice(viewer, viewer.slices.axis),
            ViewComponent::Volume => self.render_volume(viewer, UVec2::splat(VOLUME_SIZE)),
            ViewComponent::Mpr => composite_mpr(viewer, |quarter, size| match quarter {
                Quarter::Slice(axis) => self.render_slice(viewer, axis),
                Quarter::Volume => self.render_volume(viewer, size),
            }),
        }
    }
}

/// The [`GpuSink`] port on the off-screen renderer — the same glue the
/// desktop's presentation layer uses (`ferrum-ui`'s `RendererSink`). What to
/// upload and when stays in `Viewer::sync_gpu`; this only forwards.
struct Sink<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    renderer: &'a mut VolumeRenderer,
}

impl GpuSink for Sink<'_> {
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

    fn upload_labels(&mut self, labels: Option<&LabelMap>, dirty: Option<VoxelBox>) {
        self.renderer.update_labels(self.device, self.queue, labels, dirty);
    }

    fn upload_segment_colors(&mut self, lut: &[Rgba8; 256]) {
        self.renderer.set_segment_colors(self.queue, lut);
    }
}
