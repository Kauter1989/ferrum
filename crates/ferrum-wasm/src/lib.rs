//! FERRUM in the browser: WebGPU rendering into a `<canvas>`.
//!
//! This crate is the browser face of `ferrum-render`: the same WGSL, the same
//! frame model and the same camera as the desktop app, so a panel in a web
//! harness shows the study in real time instead of a streamed image. It holds
//! the device, the surface of the canvas and the renderer, and exposes the
//! few operations a panel needs:
//!
//! ```js
//! const view = await FerrumView.create(canvas);
//! view.set_volume(dims, spacing, values);   // Float32Array, i fastest
//! view.render();                            // draws the current state
//! ```
//!
//! The Rust side keeps no policy: the panel decides what to show and calls
//! the setters, exactly as `ferrum-agent` commands do on the desktop.

#![cfg(target_arch = "wasm32")]

use std::sync::Arc;

use ferrum_domain::{OrbitCamera, RenderMode, RenderSettings, SliceAxis, TransferFunction, Volume, WindowLevel};
use ferrum_processing::BrickGrid;
use ferrum_render::frame::brick_classifier;
use ferrum_render::gpu::{GpuContext, VolumeRenderer, OFFSCREEN_FORMAT};
use ferrum_render::{FrameParams, SliceParams};
use glam::{UVec2, Vec2, Vec3};
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

/// Where a frame is drawn: the whole surface, or one view of an MPR layout.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FerrumMode {
    /// One 2D slice.
    Slice2d,
    /// A 3D volume render.
    Volume3d,
}

/// A live FERRUM view bound to one canvas.
#[wasm_bindgen]
pub struct FerrumView {
    ctx: GpuContext,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    renderer: VolumeRenderer,
    volume: Arc<Volume>,
    grid: BrickGrid,
    camera: OrbitCamera,
    settings: RenderSettings,
    window: WindowLevel,
    transfer_function: TransferFunction,
    axis: SliceAxis,
    slice: u32,
    mode: FerrumMode,
    size: UVec2,
}

#[wasm_bindgen]
impl FerrumView {
    /// Creates a view for `canvas` on the browser's WebGPU adapter.
    ///
    /// Fails when the browser has no WebGPU adapter; the caller shows that
    /// reason instead of a blank canvas.
    pub async fn create(canvas: HtmlCanvasElement) -> Result<FerrumView, JsValue> {
        console_error_panic_hook::set_once();
        // On wasm the only backend is the browser's WebGPU: `Backends::all()` there
        // does not include it, so it is named explicitly.
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::BROWSER_WEBGPU;
        let instance = wgpu::Instance::new(descriptor);
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|e| JsValue::from_str(&format!("cannot use the canvas: {e}")))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|e| JsValue::from_str(&format!("no WebGPU adapter: {e}")))?;
        let (device, queue) = adapter
            .request_device(&ferrum_render::gpu::device_descriptor(&adapter))
            .await
            .map_err(|e| JsValue::from_str(&format!("no WebGPU device: {e}")))?;
        let caps = ferrum_render::gpu::GpuCaps::detect(&adapter, &device);
        let width = canvas.width().max(1);
        let height = canvas.height().max(1);
        let surface_config = surface
            .get_default_config(&adapter, width, height)
            .ok_or_else(|| JsValue::from_str("the canvas has no usable surface format"))?;
        surface.configure(&device, &surface_config);
        let ctx = GpuContext { adapter, device, queue, caps };
        let renderer = VolumeRenderer::new(&ctx.device, &ctx.queue, ctx.caps, surface_config.format);
        // An empty volume until the panel loads a study: the frame model needs
        // one, and a 1³ box renders nothing.
        let empty = Volume::from_physical(ferrum_domain::Dims3::new(1, 1, 1), Vec3::ONE, &[0.0])
            .map_err(|e| JsValue::from_str(&format!("cannot build an empty volume: {e}")))?;
        let grid = BrickGrid::compute(&empty, 8);
        Ok(Self {
            ctx,
            surface,
            surface_config,
            renderer,
            volume: Arc::new(empty),
            grid,
            camera: OrbitCamera::default(),
            settings: RenderSettings::default(),
            window: WindowLevel::new(0.5, 1.0),
            transfer_function: TransferFunction::legacy_default(),
            axis: SliceAxis::Axial,
            slice: 0,
            mode: FerrumMode::Volume3d,
            size: UVec2::new(width, height),
        })
    }

    /// Loads a study: `dims` (i, j, k) and `values` as f32, i fastest, in the
    /// canonical LPS frame. Resizes the canvas to the viewport.
    pub fn set_volume(&mut self, dims: Vec<u32>, spacing: Vec<f32>, values: Vec<f32>) -> Result<(), JsValue> {
        if dims.len() != 3 || spacing.len() != 3 {
            return Err(JsValue::from_str("dims and spacing take three numbers"));
        }
        let volume = Volume::from_physical(
            ferrum_domain::Dims3::new(dims[0], dims[1], dims[2]),
            Vec3::new(spacing[0], spacing[1], spacing[2]),
            &values,
        )
        .map_err(|e| JsValue::from_str(&format!("cannot take the volume: {e}")))?;
        self.renderer.set_volume(&self.ctx.device, &self.ctx.queue, &volume);
        self.grid = BrickGrid::compute(&volume, 8);
        self.window = WindowLevel::auto(volume.range(), &ferrum_domain::storage_histogram(volume.data(), 256));
        self.slice = dims[2] / 2;
        self.volume = Arc::new(volume);
        self.upload_occupancy();
        Ok(())
    }

    /// Resizes the drawing buffer (device pixels); the panel calls this on a
    /// layout or DPI change.
    pub fn resize(&mut self, width: u32, height: u32) {
        let (w, h) = (width.max(1), height.max(1));
        if self.size != UVec2::new(w, h) {
            self.size = UVec2::new(w, h);
            self.surface_config.width = w;
            self.surface_config.height = h;
            self.surface.configure(&self.ctx.device, &self.surface_config);
        }
    }

    /// Draws one frame of the current state into the canvas.
    pub fn render(&mut self) -> Result<(), JsValue> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                self.surface.configure(&self.ctx.device, &self.surface_config);
                f
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.ctx.device, &self.surface_config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
                    other => return Err(JsValue::from_str(&format!("surface: {other:?}"))),
                }
            }
            other => return Err(JsValue::from_str(&format!("surface: {other:?}"))),
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("ferrum-wasm frame") });
        match self.mode {
            FerrumMode::Slice2d => {
                self.renderer.prepare_slice(&self.ctx.device, &self.ctx.queue, 0, &self.slice_params());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("slice pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    self.renderer.paint_slice(&mut pass, 0);
                }
            }
            FerrumMode::Volume3d => {
                let params = self.frame_params();
                self.renderer.render_volume(&self.ctx.device, &self.ctx.queue, &mut encoder, 0, &params, self.size);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("present pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                self.renderer.paint_volume(&mut pass, 0);
            }
        }
        self.ctx.queue.submit(Some(encoder.finish()));
        self.ctx.queue.present(frame);
        Ok(())
    }

    /// Shows the 2D slice view or the 3D volume view.
    pub fn set_mode(&mut self, mode: FerrumMode) {
        self.mode = mode;
    }

    /// The 3D rendering mode (`tissue`, `isosurface`, `mip`, `transfer_function`).
    pub fn set_render_mode(&mut self, mode: &str) -> Result<(), JsValue> {
        self.settings.mode = RenderMode::ALL
            .into_iter()
            .find(|m| m.label().replace(' ', "_").eq_ignore_ascii_case(mode))
            .ok_or_else(|| JsValue::from_str(&format!("unknown render mode {mode:?}")))?;
        self.upload_occupancy();
        Ok(())
    }

    /// The window in the volume's units.
    pub fn set_window(&mut self, center: f32, width: f32) {
        self.window = WindowLevel::new(center, width);
    }

    /// The plane of the 2D view.
    pub fn set_slice_axis(&mut self, axis: &str) -> Result<(), JsValue> {
        self.axis = SliceAxis::ALL
            .into_iter()
            .find(|a| a.label().eq_ignore_ascii_case(axis))
            .ok_or_else(|| JsValue::from_str(&format!("unknown plane {axis:?}")))?;
        Ok(())
    }

    /// The slice index of the 2D view (clamped to the volume).
    pub fn set_slice(&mut self, index: u32) {
        self.slice = index.min(self.volume.dims().z.saturating_sub(1));
    }

    /// Orbits the 3D camera by a drag in pixels.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.camera.rotate(dx, dy);
    }

    /// Zooms the 3D camera (positive moves closer).
    pub fn zoom(&mut self, amount: f32) {
        self.camera.zoom(amount);
    }

    /// Where the camera starts.
    pub fn reset_camera(&mut self) {
        self.camera = OrbitCamera::default();
    }

    /// The renderer's description, for a status line.
    pub fn adapter_info(&self) -> String {
        let info = self.ctx.adapter.get_info();
        format!("{} ({:?})", info.name, info.backend)
    }

    /// The value window currently shown.
    pub fn window_center(&self) -> f32 {
        self.window.center
    }

    /// The window width currently shown.
    pub fn window_width(&self) -> f32 {
        self.window.width
    }
}

impl FerrumView {
    fn upload_occupancy(&mut self) {
        let tf = self.transfer_function.bake(256);
        self.renderer.set_transfer_function(&self.ctx.queue, &tf);
        let occupancy = self.grid.occupancy(brick_classifier(&self.settings, &self.transfer_function));
        self.renderer.set_occupancy(&self.ctx.device, &self.ctx.queue, self.grid.grid_dims(), &occupancy);
    }

    fn slice_params(&self) -> SliceParams {
        let position = self.axis.slice_position(&self.volume, self.slice);
        SliceParams {
            rect: (Vec2::ZERO, self.size.as_vec2()),
            window: self.window.normalized_bounds(self.volume.range()),
            position,
            axis: self.axis.id(),
            nearest: true,
            background: [0.0, 0.0, 0.0, 1.0],
            segments: false,
            segment_style: ferrum_domain::SegmentStyle::default(),
            fill_opacity: 1.0,
        }
    }

    fn frame_params(&self) -> FrameParams {
        FrameParams::new(
            &self.volume,
            &self.camera,
            &self.settings,
            &ferrum_domain::ClipSettings::default(),
            self.size.as_vec2(),
            8,
            None,
        )
        .with_segments(false)
    }
}

/// The camera of the volume view as Euler angles, for a panel that keeps the
/// orientation between frames.
#[wasm_bindgen]
pub fn camera_angles(camera: &[f32]) -> Vec<f32> {
    camera.to_vec()
}

/// The render modes in display order (`tissue`, `isosurface`, `mip`,
/// `transfer_function`).
#[wasm_bindgen]
pub fn render_modes() -> Vec<JsValue> {
    RenderMode::ALL.into_iter().map(|m| JsValue::from_str(&m.label().replace(' ', "_").to_lowercase())).collect()
}

/// The plane names of the 2D view.
#[wasm_bindgen]
pub fn planes() -> Vec<JsValue> {
    SliceAxis::ALL.into_iter().map(|a| JsValue::from_str(a.label())).collect()
}

/// Formats a voxel for a status line (`i, j, k`).
#[wasm_bindgen]
pub fn describe_voxel(i: u32, j: u32, k: u32) -> String {
    format!("{i}, {j}, {k}")
}

/// The visibility toggle a panel keeps for the overview.
#[wasm_bindgen]
pub fn default_transfer_function() -> Vec<f32> {
    let tf = TransferFunction::legacy_default();
    let mut out = Vec::new();
    for sample in tf.bake(16) {
        out.extend_from_slice(&[
            f32::from(sample[0]) / 255.0,
            f32::from(sample[1]) / 255.0,
            f32::from(sample[2]) / 255.0,
            f32::from(sample[3]) / 255.0,
        ]);
    }
    out
}

/// The offscreen format the renderer expects for a headless frame.
#[wasm_bindgen]
pub fn offscreen_format() -> String {
    format!("{:?}", OFFSCREEN_FORMAT)
}
