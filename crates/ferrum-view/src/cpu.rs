//! The CPU reference backend: the same pixels as the slice shader, without a
//! GPU.
//!
//! The grayscale values come from the slice oracle the parity tests use
//! ([`ferrum_domain::SliceImage::extract`] plus
//! [`WindowLevel::apply`](ferrum_domain::WindowLevel::apply) and
//! [`ferrum_domain::color::unit_to_u8`]); the segment overlay goes through the
//! one CPU rule ([`ferrum_render::cpu::slice_overlay`], mirroring `slice.wgsl`):
//! a pixel in full segment colour where a four-neighbour label differs,
//! otherwise the fill blended with the segment's opacity and the fill opacity.

use ferrum_app::Viewer;
use ferrum_domain::color::unit_to_u8;
use ferrum_domain::{SegmentStyle, SegmentationSet, SliceAxis, SliceImage, ViewComponent, Volume};
use glam::UVec2;

use crate::frame::{
    composite_mpr, mapping_of, projection_mapping, slice_setup, Backend, FrameSource, Quarter, RawFrame, VOLUME_SIZE,
};
use crate::session::ViewError;

/// Draws the view components on the CPU: slices with the slice oracle and
/// the one overlay rule, the 3D view with the reference ray caster
/// ([`Viewer::render_volume_cpu`], the oracle `volume.wgsl` mirrors).
#[derive(Debug, Default, Clone, Copy)]
pub struct CpuFrameSource;

impl CpuFrameSource {
    /// A new CPU backend.
    pub fn new() -> Self {
        Self
    }

    /// One slice at native resolution: the oracle's grey plus the one CPU
    /// overlay rule.
    fn render_slice(&self, viewer: &Viewer, axis: SliceAxis) -> Result<RawFrame, ViewError> {
        let dataset = viewer.dataset().ok_or(ViewError::NoStudy)?;
        let volume = &dataset.volume;
        let index = viewer.slice_index(axis);
        let image = SliceImage::extract(volume, axis, index).ok_or(ViewError::NoSlice)?;
        let setup = slice_setup(viewer, axis)?;
        let window = viewer.window();
        let mut rgba = vec![0u8; image.pixels.len() * 4];
        for (pixel, &stored) in image.pixels.iter().enumerate() {
            let g = unit_to_u8(window.apply(volume.range().from_storage(stored)));
            rgba[pixel * 4..pixel * 4 + 4].copy_from_slice(&[g, g, g, 255]);
        }
        if let Some(set) = viewer.segmentation().set().filter(|_| setup.params.segments) {
            let view = SliceDraw {
                volume,
                axis,
                index,
                width: image.width,
                height: image.height,
                set,
                style: setup.params.segment_style,
                fill_opacity: setup.params.fill_opacity,
            };
            view.draw(&mut rgba);
        }
        let size = UVec2::new(image.width, image.height);
        Ok(RawFrame {
            width: image.width,
            height: image.height,
            rgba,
            window,
            mapping: mapping_of(viewer, axis, size)?,
        })
    }

    /// The 3D view at a fixed size, through the reference ray caster.
    fn render_volume(&self, viewer: &Viewer, size: UVec2) -> Result<RawFrame, ViewError> {
        let pixels = viewer.render_volume_cpu(size).ok_or(ViewError::NoStudy)?;
        let rgba: Vec<u8> = pixels.iter().flat_map(|p| *p).collect();
        Ok(RawFrame {
            width: size.x,
            height: size.y,
            rgba,
            window: viewer.window(),
            mapping: projection_mapping(viewer, size)?,
        })
    }
}

impl FrameSource for CpuFrameSource {
    fn backend(&self) -> Backend {
        Backend::Cpu
    }

    fn render(&mut self, viewer: &mut Viewer, component: ViewComponent) -> Result<RawFrame, ViewError> {
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

/// One slice with its segment overlay, ready to be drawn on the CPU.
struct SliceDraw<'a> {
    volume: &'a Volume,
    axis: SliceAxis,
    index: u32,
    width: u32,
    height: u32,
    set: &'a SegmentationSet,
    style: SegmentStyle,
    fill_opacity: f32,
}

impl SliceDraw<'_> {
    /// Draws the overlay over an already windowed grey slice through the one
    /// CPU rule ([`ferrum_render::cpu::slice_overlay`], which mirrors
    /// `slice.wgsl`): background pixels stay grey, a visible segment fills with
    /// `opacity × fill_opacity`, and any four-neighbour with another label makes
    /// the pixel an outline in full colour.
    fn draw(&self, rgba: &mut [u8]) {
        let labels = self.set.labels();
        let label_at = |x: u32, y: u32| -> u8 {
            self.axis.voxel_at(self.volume, self.index, x, y).and_then(|v| labels.label(v.x, v.y, v.z)).unwrap_or(0)
        };
        let segment = |l: u8| -> Option<([u8; 3], f32)> {
            self.set.segment(l).filter(|s| s.visible && s.opacity > 0.0).map(|s| (s.color, s.opacity))
        };
        ferrum_render::cpu::slice_overlay(
            rgba,
            self.width,
            self.height,
            label_at,
            segment,
            self.style,
            self.fill_opacity,
        );
    }
}
