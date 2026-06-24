//! wgpu renderer for the Launchpad MVP.
//!
//! Owns the window, device/queue, surface, render pipeline, and GPU buffers.
//! The instance buffer is written **once** (tiles are static); only the
//! ~16-byte uniform (viewport + scroll offset) is updated per frame, so
//! scrolling costs essentially nothing on the CPU/GPU bus.
//!
//! The `Window` is moved into this struct so that the `Surface` can borrow it
//! for `'static` — both live and die together.
//!
//! Note: written against the wgpu 29 API.

use std::num::NonZeroU64;
use std::sync::Arc;

use wgpu::util::DeviceExt;
use wgpu::{
    Backends, Buffer, BufferAddress, Color, Device, Instance, Limits, PresentMode, Queue,
    RenderPipeline, Surface, SurfaceConfiguration, TextureFormat, TextureViewDescriptor,
};

use crate::grid::{GridLayout, TileInstance};
use crate::text::GlyphQuad;

/// Uniform block mirrored in WGSL. Kept 16 bytes for alignment.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    scroll_x: f32,
    _pad: f32,
}

pub struct Renderer {
    /// Owned window. Kept here so the surface (which borrows it) is valid.
    pub window: winit::window::Window,
    pub device: Arc<Device>,
    pub queue: Arc<Queue>,
    pub surface: Surface<'static>,
    pub config: SurfaceConfiguration,
    pipeline: RenderPipeline,
    /// Static per-tile instance data.
    instance_buffer: Buffer,
    /// Per-frame uniform data (viewport + scroll).
    uniform_buffer: Buffer,
    uniform_bind_group: wgpu::BindGroup,
    instance_count: u32,
    /// Current sRGB surface format (saved for future MSAA / gamma work).
    #[allow(dead_code)]
    surface_format: TextureFormat,

    // -- Text rendering -------------------------------------------------
    text_pipeline: RenderPipeline,
    text_instance_buffer: Option<Buffer>,
    text_instance_count: u32,
    atlas_texture: wgpu::Texture,
    atlas_bind_group: wgpu::BindGroup,
    /// Copy of the bind group layout (for texture/sampler + uniform).
    #[allow(dead_code)]
    text_bgl: wgpu::BindGroupLayout,
}

pub struct DrawArgs {
    pub scroll_x: f32,
    pub viewport: (u32, u32),
}

impl Renderer {
    /// Create the renderer and configure the surface.
    ///
    /// Takes ownership of `window`; the surface borrows it for `'static`,
    /// which is sound because both are owned by the returned `Renderer` and
    /// dropped together (surface first, by field order).
    pub async fn new(
        window: winit::window::Window,
        layout: &GridLayout,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let instance = Instance::new(wgpu::InstanceDescriptor {
            backends: Backends::DX12 | Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        // Safety: we move `window` into the returned Renderer immediately, so
        // the surface's `'static` borrow is valid for as long as the Renderer
        // exists. The window outlives the surface by field order.
        let surface = unsafe {
            let static_window: &'static winit::window::Window = &*(&window as *const _);
            instance.create_surface(static_window)?
        };

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| format!("no suitable GPU adapter: {e}"))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("launchpad device"),
                required_features: wgpu::Features::empty(),
                // `downlevel_defaults()` caps the max texture dimension at
                // 2048, which high-DPI windows easily exceed (e.g. a 150%
                // scale 1920x1080 window becomes 2880x1620). Use the full
                // WebGPU limits (8192) instead.
                required_limits: Limits::default(),
                ..Default::default()
            })
            .await?;

        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let size = window.inner_size();
        let max_dim = device.limits().max_texture_dimension_2d;
        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1).min(max_dim),
            height: size.height.max(1).min(max_dim),
            present_mode: select_present_mode(&caps.present_modes),
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        // Shaders
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tile shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniform buffer"),
            size: std::mem::size_of::<Uniforms>() as BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uniform bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tile pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[TileInstance::LAYOUT],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Instance buffer: written once.
        let instances = layout.build_instances(config.width as f32);
        let instance_count = instances.len() as u32;
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("instance buffer"),
            contents: bytemuck::cast_slice(&instances),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        // Initial uniform upload.
        queue.write_buffer(
            &uniform_buffer,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: [size.width as f32, size.height as f32],
                scroll_x: 0.0,
                _pad: 0.0,
            }),
        );

        // ---- Text pipeline + glyph atlas --------------------------------
        let (aw, ah) = crate::text::TextRenderer::atlas_dimensions();
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d { width: aw, height: ah, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // Text bind group: [0]=uniform (shared), [1]=atlas texture, [2]=sampler.
        let text_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas bg"),
            layout: &text_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });

        let text_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_text.wgsl").into()),
        });
        let text_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text pipeline layout"),
            bind_group_layouts: &[Some(&text_bgl)],
            immediate_size: 0,
        });
        let text_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("text pipeline"),
            layout: Some(&text_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &text_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[GlyphQuad::LAYOUT],
            },
            fragment: Some(wgpu::FragmentState {
                module: &text_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Ok(Self {
            window,
            device,
            queue,
            surface,
            config,
            pipeline,
            instance_buffer,
            uniform_buffer,
            uniform_bind_group,
            instance_count,
            surface_format,
            text_pipeline,
            text_instance_buffer: None,
            text_instance_count: 0,
            atlas_texture,
            atlas_bind_group,
            text_bgl,
        })
    }

    /// Reconfigure the surface after a window resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        // Clamp to the device's max texture dimension as a safety net on
        // extremely high-DPI / multi-monitor setups.
        let max = self.device.limits().max_texture_dimension_2d;
        self.config.width = width.min(max);
        self.config.height = height.min(max);
        self.surface.configure(&self.device, &self.config);
    }

    #[allow(dead_code)]
    pub fn surface_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Rebuild the static instance buffer from a fresh layout.
    ///
    /// Call after a resize (or any change to tile data) so the GPU sees the
    /// new tile positions. The buffer is reallocated to fit.
    pub fn rebuild_instances(&mut self, layout: &GridLayout) {
        let instances = layout.build_instances(self.config.width as f32);
        self.instance_count = instances.len() as u32;
        self.instance_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("instance buffer"),
            contents: bytemuck::cast_slice(&instances),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
    }

    /// Upload the glyph atlas texture from the given RGBA buffer.
    pub fn upload_atlas(&self, rgba: &[u8]) {
        let (w, h) = crate::text::TextRenderer::atlas_dimensions();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
    }

    /// Replace the per-glyph text instance buffer.
    pub fn set_text_instances(&mut self, quads: &[GlyphQuad]) {
        self.text_instance_count = quads.len() as u32;
        if quads.is_empty() {
            self.text_instance_buffer = None;
            return;
        }
        self.text_instance_buffer = Some(
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("text instance buffer"),
                    contents: bytemuck::cast_slice(quads),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                }),
        );
    }

    /// Render one frame.
    pub fn render(&self, args: &DrawArgs) {
        // Update uniforms (tiny, every frame).
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: [args.viewport.0 as f32, args.viewport.1 as f32],
                scroll_x: args.scroll_x,
                _pad: 0.0,
            }),
        );

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                eprintln!("surface outdated/lost; skipping frame");
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout => return,
            wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Validation => {
                eprintln!("surface validation error; skipping frame");
                return;
            }
        };
        let view = frame.texture.create_view(&TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tile pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(Color {
                            r: 0.108,
                            g: 0.110,
                            b: 0.118,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            // 6 verts per quad (two tris), instance_count quads.
            pass.draw(0..6, 0..self.instance_count);

            // Text labels: same pass, second draw call. Uses the same
            // uniform (scroll/viewport) plus the atlas texture.
            if let Some(buf) = self.text_instance_buffer.as_ref() {
                pass.set_pipeline(&self.text_pipeline);
                pass.set_bind_group(0, &self.atlas_bind_group, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..6, 0..self.text_instance_count);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
    }
}

/// Prefer Mailbox (low-latency VSync); fall back to FIFO.
fn select_present_mode(available: &[PresentMode]) -> PresentMode {
    if available.contains(&PresentMode::Mailbox) {
        PresentMode::Mailbox
    } else if available.contains(&PresentMode::AutoVsync) {
        PresentMode::AutoVsync
    } else {
        PresentMode::Fifo
    }
}
