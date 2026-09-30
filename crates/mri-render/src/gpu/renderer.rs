//! GPU resources and draw calls of the volume and slice views.

use std::collections::HashMap;

use glam::{UVec2, UVec3, Vec4};
use mri_domain::{Dims3, Rgba8, Volume, VoxelMask};
use mri_processing::AmbientOcclusion;

use super::context::{GpuCaps, GpuError};
use crate::frame::{FrameParams, SliceParams, SliceUniforms, VolumeUniforms};
use crate::shaders::{BLIT_WGSL, SLICE_WGSL, VOLUME_WGSL};

/// Colour format of the off-screen 3D render targets.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Identifies a view (a panel of the UI) owning its own uniforms and
/// render target, so several views can be drawn in the same frame.
pub type ViewId = u64;

/// Maximum number of slices uploaded per `write_texture` call, bounding the
/// size of staging allocations.
const UPLOAD_SLICES_PER_CHUNK: u32 = 16;

struct Texture3d {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    dims: Dims3,
}

struct Offscreen {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: UVec2,
    blit_group: wgpu::BindGroup,
}

struct ViewResources {
    volume_ubo: wgpu::Buffer,
    slice_ubo: wgpu::Buffer,
    volume_group: Option<(u64, wgpu::BindGroup)>,
    slice_group: Option<(u64, wgpu::BindGroup)>,
    target: Option<Offscreen>,
}

/// Owns every GPU resource of the viewer.
///
/// Volume-dependent textures are replaced on load; each change bumps a
/// generation counter so per-view bind groups are rebuilt lazily.
pub struct VolumeRenderer {
    caps: GpuCaps,
    present_format: wgpu::TextureFormat,
    linear: wgpu::Sampler,
    nearest: wgpu::Sampler,
    volume_layout: wgpu::BindGroupLayout,
    slice_layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    volume_module: wgpu::ShaderModule,
    volume_pipeline_layout: wgpu::PipelineLayout,
    volume_pipelines: HashMap<u32, wgpu::RenderPipeline>,
    slice_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    blit_pipeline: wgpu::RenderPipeline,
    volume: Texture3d,
    mask: Texture3d,
    ao: Texture3d,
    occupancy: Texture3d,
    tf: wgpu::Texture,
    tf_view: wgpu::TextureView,
    generation: u64,
    views: HashMap<ViewId, ViewResources>,
}

fn texture_3d(device: &wgpu::Device, label: &str, dims: Dims3, format: wgpu::TextureFormat) -> Texture3d {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: dims.x, height: dims.y, depth_or_array_layers: dims.z },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Texture3d { texture, view, dims }
}

/// Uploads a sub-box of an 8-bit 3D grid.
fn upload_u8_region(queue: &wgpu::Queue, tex: &Texture3d, data: &[u8], lo: UVec3, hi: UVec3) {
    let d = tex.dims;
    let size = hi - lo + UVec3::ONE;
    let mut buf = Vec::with_capacity((size.x * size.y * size.z) as usize);
    for k in lo.z..=hi.z {
        for j in lo.y..=hi.y {
            let row = d.index(lo.x, j, k);
            buf.extend_from_slice(&data[row..row + size.x as usize]);
        }
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex.texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: lo.x, y: lo.y, z: lo.z },
            aspect: wgpu::TextureAspect::All,
        },
        &buf,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.x), rows_per_image: Some(size.y) },
        wgpu::Extent3d { width: size.x, height: size.y, depth_or_array_layers: size.z },
    );
}

fn upload_u8_full(queue: &wgpu::Queue, tex: &Texture3d, data: &[u8]) {
    let d = tex.dims;
    let mut z = 0;
    while z < d.z {
        let n = UPLOAD_SLICES_PER_CHUNK.min(d.z - z);
        upload_u8_region(queue, tex, data, UVec3::new(0, 0, z), UVec3::new(d.x - 1, d.y - 1, z + n - 1));
        z += n;
    }
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    constants: &[(&str, f64)],
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions { constants, ..Default::default() },
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions { constants, ..Default::default() },
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn layout_entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None }
}

fn tex_entry(binding: u32, dim: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    layout_entry(
        binding,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
    )
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    layout_entry(
        binding,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
    )
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    layout_entry(binding, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering))
}

impl VolumeRenderer {
    /// Creates the renderer. `present_format` is the format of the surface
    /// the views are finally drawn into (egui's target format).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, caps: GpuCaps, present_format: wgpu::TextureFormat) -> Self {
        let sampler = |filter: wgpu::FilterMode, label: &str| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let linear = sampler(wgpu::FilterMode::Linear, "linear");
        let nearest = sampler(wgpu::FilterMode::Nearest, "nearest");
        let d3 = wgpu::TextureViewDimension::D3;
        let volume_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("volume layout"),
            entries: &[
                uniform_entry(0),
                tex_entry(1, d3),
                sampler_entry(2),
                tex_entry(3, d3),
                tex_entry(4, d3),
                tex_entry(5, d3),
                tex_entry(6, wgpu::TextureViewDimension::D2),
            ],
        });
        let slice_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("slice layout"),
            entries: &[uniform_entry(0), tex_entry(1, d3), sampler_entry(2), sampler_entry(3)],
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit layout"),
            entries: &[tex_entry(0, wgpu::TextureViewDimension::D2), sampler_entry(1)],
        });
        let module = |src: &str, label: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            })
        };
        let volume_module = module(VOLUME_WGSL, "volume.wgsl");
        let blit_module = module(BLIT_WGSL, "blit.wgsl");
        let pl = |layout: &wgpu::BindGroupLayout, label: &str| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            })
        };
        let volume_pipeline_layout = pl(&volume_layout, "volume pipeline layout");
        let blit_pipeline =
            fullscreen_pipeline(device, "blit", &pl(&blit_layout, "blit pl"), &blit_module, present_format, &[]);

        let one = Dims3::new(1, 1, 1);
        let volume = texture_3d(device, "volume (empty)", one, caps.volume_format);
        let mask = texture_3d(device, "mask (none)", one, wgpu::TextureFormat::R8Unorm);
        let ao = texture_3d(device, "ao (none)", one, wgpu::TextureFormat::R8Unorm);
        let occupancy = texture_3d(device, "occupancy (none)", one, wgpu::TextureFormat::R8Unorm);
        for t in [&mask, &ao, &occupancy] {
            upload_u8_full(queue, t, &[255]);
        }
        let tf = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("transfer function"),
            size: wgpu::Extent3d { width: 256, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let tf_view = tf.create_view(&wgpu::TextureViewDescriptor::default());
        let mut me = Self {
            caps,
            present_format,
            linear,
            nearest,
            volume_layout,
            slice_layout,
            blit_layout,
            volume_module,
            volume_pipeline_layout,
            volume_pipelines: HashMap::new(),
            slice_pipelines: HashMap::new(),
            blit_pipeline,
            volume,
            mask,
            ao,
            occupancy,
            tf,
            tf_view,
            generation: 0,
            views: HashMap::new(),
        };
        me.set_transfer_function(queue, &[[0, 0, 0, 0]; 256]);
        me.ensure_slice_pipeline(device, present_format);
        me
    }

    /// Detected capabilities.
    pub fn caps(&self) -> GpuCaps {
        self.caps
    }

    fn ensure_volume_pipeline(&mut self, device: &wgpu::Device, mode: u32) {
        if !self.volume_pipelines.contains_key(&mode) {
            let p = fullscreen_pipeline(
                device,
                &format!("volume mode {mode}"),
                &self.volume_pipeline_layout,
                &self.volume_module,
                OFFSCREEN_FORMAT,
                &[("MODE", f64::from(mode))],
            );
            self.volume_pipelines.insert(mode, p);
        }
    }

    fn ensure_slice_pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if !self.slice_pipelines.contains_key(&format) {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("slice.wgsl"),
                source: wgpu::ShaderSource::Wgsl(SLICE_WGSL.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("slice pl"),
                bind_group_layouts: &[Some(&self.slice_layout)],
                immediate_size: 0,
            });
            let p = fullscreen_pipeline(device, "slice", &layout, &module, format, &[]);
            self.slice_pipelines.insert(format, p);
        }
    }

    /// Uploads a new volume. Resets mask, AO and occupancy.
    ///
    /// Volumes exceeding the device's 3D texture limit are box-downsampled
    /// first; all sampling uses normalised coordinates, so the rest of the
    /// pipeline (mask, AO, clipping, picking) is unaffected.
    pub fn set_volume(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, volume: &Volume) {
        let factors = mri_processing::resample::fit_factors(volume.dims(), self.caps.max_texture_3d, usize::MAX);
        let reduced;
        let volume = if factors == UVec3::ONE {
            volume
        } else {
            log::warn!("volume {:?} exceeds GPU limits, downsampling by {factors}", volume.dims());
            match mri_processing::resample::downsample(volume, factors) {
                Ok(v) => {
                    reduced = v;
                    &reduced
                }
                Err(e) => {
                    log::error!("downsampling failed: {e}");
                    return;
                }
            }
        };
        let dims = volume.dims();
        let tex = texture_3d(device, "volume", dims, self.caps.volume_format);
        let data = volume.data();
        let slice = dims.slice_len();
        let mut z = 0;
        while z < dims.z {
            let n = UPLOAD_SLICES_PER_CHUNK.min(dims.z - z);
            let src = &data[z as usize * slice..(z + n) as usize * slice];
            let bytes: Vec<u8> = match self.caps.volume_format {
                wgpu::TextureFormat::R16Unorm => bytemuck::cast_slice(src).to_vec(),
                _ => {
                    let h: Vec<half::f16> = src.iter().map(|&v| half::f16::from_f32(f32::from(v) / 65535.0)).collect();
                    bytemuck::cast_slice(&h).to_vec()
                }
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(dims.x * 2),
                    rows_per_image: Some(dims.y),
                },
                wgpu::Extent3d { width: dims.x, height: dims.y, depth_or_array_layers: n },
            );
            z += n;
        }
        self.volume = tex;
        self.reset_mask(device, queue);
        self.set_ambient_occlusion(device, queue, None);
        let one = Dims3::new(1, 1, 1);
        self.occupancy = texture_3d(device, "occupancy (none)", one, wgpu::TextureFormat::R8Unorm);
        upload_u8_full(queue, &self.occupancy, &[255]);
        self.generation += 1;
    }

    /// Synchronises the eraser mask. Uploads only the `dirty` region when
    /// the full-size mask texture already exists.
    pub fn update_mask(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mask: &VoxelMask,
        dirty: Option<(UVec3, UVec3)>,
    ) {
        if mask.dims().as_uvec3().max_element() > self.caps.max_texture_3d {
            log::warn!("eraser mask {:?} exceeds the GPU 3D texture limit; not displayed", mask.dims());
            return;
        }
        if self.mask.dims != mask.dims() {
            self.mask = texture_3d(device, "mask", mask.dims(), wgpu::TextureFormat::R8Unorm);
            upload_u8_full(queue, &self.mask, mask.data());
            self.generation += 1;
        } else if let Some((lo, hi)) = dirty {
            upload_u8_region(queue, &self.mask, mask.data(), lo, hi);
        }
    }

    /// Removes the mask (everything visible).
    pub fn reset_mask(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.mask = texture_3d(device, "mask (none)", Dims3::new(1, 1, 1), wgpu::TextureFormat::R8Unorm);
        upload_u8_full(queue, &self.mask, &[255]);
        self.generation += 1;
    }

    /// Sets or clears the ambient occlusion volume.
    pub fn set_ambient_occlusion(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, ao: Option<&AmbientOcclusion>) {
        self.ao = match ao {
            Some(a) => {
                let t = texture_3d(device, "ambient occlusion", a.dims, wgpu::TextureFormat::R8Unorm);
                upload_u8_full(queue, &t, &a.data);
                t
            }
            None => {
                let t = texture_3d(device, "ao (none)", Dims3::new(1, 1, 1), wgpu::TextureFormat::R8Unorm);
                upload_u8_full(queue, &t, &[255]);
                t
            }
        };
        self.generation += 1;
    }

    /// Uploads the brick occupancy map for empty-space skipping.
    pub fn set_occupancy(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, grid: Dims3, occupancy: &[u8]) {
        if occupancy.len() != grid.voxel_count() || grid.is_empty() {
            log::warn!("occupancy size mismatch, ignored");
            return;
        }
        if self.occupancy.dims != grid {
            self.occupancy = texture_3d(device, "occupancy", grid, wgpu::TextureFormat::R8Unorm);
            self.generation += 1;
        }
        upload_u8_full(queue, &self.occupancy, occupancy);
    }

    /// Uploads a 256-entry transfer function lookup table.
    pub fn set_transfer_function(&mut self, queue: &wgpu::Queue, lut: &[Rgba8]) {
        let mut data = [[0u8; 4]; 256];
        for (d, s) in data.iter_mut().zip(lut) {
            *d = *s;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.tf,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&data),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256 * 4), rows_per_image: Some(1) },
            wgpu::Extent3d { width: 256, height: 1, depth_or_array_layers: 1 },
        );
    }

    fn view_mut(&mut self, device: &wgpu::Device, id: ViewId) -> &mut ViewResources {
        self.views.entry(id).or_insert_with(|| {
            let ubo = |size: usize, label: &str| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: size as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            ViewResources {
                volume_ubo: ubo(std::mem::size_of::<VolumeUniforms>(), "volume uniforms"),
                slice_ubo: ubo(std::mem::size_of::<SliceUniforms>(), "slice uniforms"),
                volume_group: None,
                slice_group: None,
                target: None,
            }
        })
    }

    fn ensure_groups(&mut self, device: &wgpu::Device, id: ViewId) {
        let generation = self.generation;
        let (volume_layout, slice_layout) = (&self.volume_layout, &self.slice_layout);
        let (vol, mask, ao, occ, tf) =
            (&self.volume.view, &self.mask.view, &self.ao.view, &self.occupancy.view, &self.tf_view);
        let (linear, nearest) = (&self.linear, &self.nearest);
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        if view.volume_group.as_ref().is_none_or(|(g, _)| *g != generation) {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("volume group"),
                layout: volume_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: view.volume_ubo.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(vol) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(linear) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(mask) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(ao) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(occ) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(tf) },
                ],
            });
            view.volume_group = Some((generation, group));
        }
        if view.slice_group.as_ref().is_none_or(|(g, _)| *g != generation) {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("slice group"),
                layout: slice_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: view.slice_ubo.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(vol) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(linear) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(nearest) },
                ],
            });
            view.slice_group = Some((generation, group));
        }
    }

    fn ensure_target(&mut self, device: &wgpu::Device, id: ViewId, size: UVec2) {
        let size = size.max(UVec2::ONE);
        let (blit_layout, linear) = (&self.blit_layout, &self.linear);
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        if view.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("volume target"),
            size: wgpu::Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let tview = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let blit_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit group"),
            layout: blit_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&tview) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(linear) },
            ],
        });
        view.target = Some(Offscreen { texture, view: tview, size, blit_group });
    }

    /// Ray casts the volume of view `id` into its off-screen target of
    /// `size` pixels (the dynamic-resolution render size).
    pub fn render_volume(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        id: ViewId,
        params: &FrameParams,
        size: UVec2,
    ) {
        let mode = params.mode_id();
        self.ensure_volume_pipeline(device, mode);
        self.view_mut(device, id);
        self.ensure_groups(device, id);
        self.ensure_target(device, id, size);
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let (Some(target), Some((_, group)), Some(pipeline)) =
            (view.target.as_ref(), view.volume_group.as_ref(), self.volume_pipelines.get(&mode))
        else {
            return;
        };
        let size = target.size;
        let u = params.uniforms(Vec4::new(0.0, 0.0, size.x as f32, size.y as f32));
        queue.write_buffer(&view.volume_ubo, 0, bytemuck::bytes_of(&u));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("volume pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Draws the off-screen result of view `id` into the current pass
    /// (whose viewport is the view's rectangle).
    pub fn paint_volume(&self, pass: &mut wgpu::RenderPass<'_>, id: ViewId) {
        if let Some(target) = self.views.get(&id).and_then(|v| v.target.as_ref()) {
            pass.set_pipeline(&self.blit_pipeline);
            pass.set_bind_group(0, &target.blit_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Updates the uniforms of slice view `id`.
    pub fn prepare_slice(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: ViewId, params: &SliceParams) {
        self.view_mut(device, id);
        self.ensure_groups(device, id);
        if let Some(view) = self.views.get(&id) {
            queue.write_buffer(&view.slice_ubo, 0, bytemuck::bytes_of(&params.uniforms()));
        }
    }

    /// Draws slice view `id` into a pass targeting the present format.
    pub fn paint_slice(&self, pass: &mut wgpu::RenderPass<'_>, id: ViewId) {
        let pipeline = self.slice_pipelines.get(&self.present_format);
        if let (Some(p), Some((_, group))) = (pipeline, self.views.get(&id).and_then(|v| v.slice_group.as_ref())) {
            pass.set_pipeline(p);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Renders the volume off-screen and reads the image back (screenshots,
    /// tests). Returns RGBA8 pixels, top row first.
    pub fn render_volume_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &FrameParams,
        size: UVec2,
    ) -> Result<Vec<Rgba8>, GpuError> {
        const ID: ViewId = u64::MAX;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("offscreen") });
        self.render_volume(device, queue, &mut encoder, ID, params, size);
        let target = self
            .views
            .get(&ID)
            .and_then(|v| v.target.as_ref())
            .ok_or_else(|| GpuError::Readback("no target".into()))?;
        read_texture(device, queue, encoder, &target.texture, target.size)
    }

    /// Renders a slice into a new texture of `size` pixels and reads it back.
    pub fn render_slice_image(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &SliceParams,
        size: UVec2,
    ) -> Result<Vec<Rgba8>, GpuError> {
        const ID: ViewId = u64::MAX - 1;
        self.ensure_slice_pipeline(device, OFFSCREEN_FORMAT);
        self.prepare_slice(device, queue, ID, params);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("slice target"),
            size: wgpu::Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("slice offscreen") });
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
                ..Default::default()
            });
            let pipeline = self.slice_pipelines.get(&OFFSCREEN_FORMAT);
            let group = self.views.get(&ID).and_then(|v| v.slice_group.as_ref());
            if let (Some(p), Some((_, g))) = (pipeline, group) {
                pass.set_pipeline(p);
                pass.set_bind_group(0, g, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        read_texture(device, queue, encoder, &texture, size)
    }
}

/// Copies an RGBA8 texture to the CPU.
fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    size: UVec2,
) -> Result<Vec<Rgba8>, GpuError> {
    let unpadded = size.x * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded) * u64::from(size.y),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.y),
            },
        },
        wgpu::Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| GpuError::Readback(e.to_string()))?;
    rx.recv().map_err(|e| GpuError::Readback(e.to_string()))?.map_err(|e| GpuError::Readback(e.to_string()))?;
    let data = slice.get_mapped_range().map_err(|e| GpuError::Readback(e.to_string()))?;
    let mut out = Vec::with_capacity((size.x * size.y) as usize);
    for row in 0..size.y as usize {
        let start = row * padded as usize;
        out.extend_from_slice(data[start..start + unpadded as usize].as_chunks::<4>().0);
    }
    drop(data);
    buffer.unmap();
    Ok(out)
}
