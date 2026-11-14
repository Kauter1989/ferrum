//! The GPU backend: `ferrum-render`'s wgpu renderer, off-screen.
//!
//! It draws the same slice as the desktop's slice view (`slice.wgsl`, nearest
//! sampling, the full native rectangle) and reads it back, so a host process
//! without a window still uses the machine's GPU and the rendering rule of
//! the repository is unchanged.

use ferrum_app::Viewer;
use ferrum_domain::SliceAxis;
use ferrum_render::gpu::{GpuContext, VolumeRenderer, OFFSCREEN_FORMAT};
use glam::UVec2;

use crate::frame::{mapping_of, slice_setup, Backend, FrameSource, RawFrame};
use crate::session::ViewError;

/// Draws slices with the wgpu renderer, off-screen.
pub struct GpuFrameSource {
    ctx: GpuContext,
    renderer: VolumeRenderer,
    volume_revision: Option<u64>,
    labels_revision: Option<u64>,
    labels_uploaded: bool,
}

impl GpuFrameSource {
    /// Creates a headless GPU context and the renderer on it.
    pub fn new() -> Result<Self, ViewError> {
        let ctx = GpuContext::headless().map_err(|e| ViewError::Gpu(e.to_string()))?;
        let renderer = VolumeRenderer::new(&ctx.device, &ctx.queue, ctx.caps, OFFSCREEN_FORMAT);
        Ok(Self { ctx, renderer, volume_revision: None, labels_revision: None, labels_uploaded: false })
    }

    /// The adapter, for a log line: driver name, backend and limits.
    pub fn describe(&self) -> String {
        self.ctx.describe()
    }
}

impl FrameSource for GpuFrameSource {
    fn backend(&self) -> Backend {
        Backend::Gpu
    }

    fn render(&mut self, viewer: &Viewer, axis: SliceAxis) -> Result<RawFrame, ViewError> {
        let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
        let setup = slice_setup(viewer, axis)?;
        if self.volume_revision != Some(dataset.revision) {
            self.renderer.set_volume(&self.ctx.device, &self.ctx.queue, &dataset.volume);
            self.ctx.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| ViewError::Gpu(e.to_string()))?;
            self.volume_revision = Some(dataset.revision);
            self.labels_uploaded = false;
        }
        let set = viewer.segmentation().set().filter(|_| setup.params.segments);
        let revision = set.map(|s| s.revision());
        if !self.labels_uploaded || self.labels_revision != revision {
            match set {
                Some(segments) => {
                    self.renderer.update_labels(&self.ctx.device, &self.ctx.queue, Some(segments.labels()), None);
                    self.renderer.set_segment_colors(&self.ctx.queue, &segments.lut());
                }
                None => self.renderer.update_labels(&self.ctx.device, &self.ctx.queue, None, None),
            }
            self.labels_uploaded = true;
            self.labels_revision = revision;
        }
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
            mapping: mapping_of(viewer, axis, UVec2::new(setup.size.x, setup.size.y))?,
        })
    }
}
