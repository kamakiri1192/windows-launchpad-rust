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
    Backends, Buffer, BufferAddress, Color, CompositeAlphaMode, Device, Instance, Limits,
    PresentMode, Queue, RenderPipeline, Surface, SurfaceConfiguration, TextureFormat,
    TextureViewDescriptor,
};

use crate::bottom_control::ControlInstance;
use crate::grid::{GridLayout, TileInstance};
use crate::icon_pipeline::IconInstance;
use crate::liquid_glass::capture::FallbackCapture;
use crate::liquid_glass::geometry::GlassShape;
use crate::liquid_glass::LiquidGlassRenderer;
use crate::text::GlyphQuad;
use crate::UserEvent;

/// Uniform block mirrored in WGSL.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    scroll_x: f32,
    /// Global animation clock (seconds). Drives the edit-mode wiggle.
    time: f32,
    /// Fixed page-frame center in physical px.
    frame_center: [f32; 2],
    /// Fixed page-frame half-size in physical px.
    frame_half_size: [f32; 2],
    /// Fixed page-frame corner radius in physical px.
    frame_radius: f32,
    /// 1.0 while an edit-mode drag is in flight, else 0.0. Tells the dragged
    /// instance's vertex shader to follow `drag_pos` instead of its home cell.
    drag_active: f32,
    /// Pointer position (screen px) the dragged icon follows. Only meaningful
    /// while `drag_active` is 1.0.
    drag_pos: [f32; 2],
}

/// Uniform for the bottom-control overlay + text shaders. The bottom control
/// uses only the viewport; edit badges also use scroll and the page frame clip.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ControlUniforms {
    viewport_scroll: [f32; 4],
    frame_center_radius: [f32; 4],
    frame_half_size: [f32; 4],
}

pub struct Renderer {
    /// Owned window. Kept here so the surface (which borrows it) is valid.
    pub window: winit::window::Window,
    pub device: Arc<Device>,
    pub queue: Arc<Queue>,
    pub surface: Surface<'static>,
    pub config: SurfaceConfiguration,
    pipeline: RenderPipeline,
    /// Current decorations state (borderless by default, toggle with M).
    decorated: bool,
    /// Static per-tile instance data.
    instance_buffer: Buffer,
    /// Per-frame uniform data (viewport + scroll).
    uniform_buffer: Buffer,
    uniform_bind_group: wgpu::BindGroup,
    instance_count: u32,
    /// Current sRGB surface format (saved for future MSAA / gamma work).
    #[allow(dead_code)]
    surface_format: TextureFormat,
    liquid_glass: LiquidGlassRenderer,

    // -- Text rendering -------------------------------------------------
    text_pipeline: RenderPipeline,
    text_instance_buffer: Option<Buffer>,
    text_instance_count: u32,
    atlas_texture: wgpu::Texture,
    atlas_bind_group: wgpu::BindGroup,
    /// Copy of the bind group layout (for texture/sampler + uniform).
    #[allow(dead_code)]
    text_bgl: wgpu::BindGroupLayout,

    // -- Icon rendering -------------------------------------------------
    icon_pipeline: RenderPipeline,
    icon_instance_buffer: Option<Buffer>,
    icon_instance_count: u32,
    dragged_icon_instance: bool,
    icon_atlas_texture: wgpu::Texture,
    icon_atlas_bind_group: wgpu::BindGroup,

    // -- Frame clip for tiles ------------------------------------------
    // Fixed page-frame geometry in physical px, fed to the tile/icon/text
    // shaders so they clip to the frame's rounded rect. `(cx, cy, hw, hh, r)`.
    frame_clip: (f32, f32, f32, f32, f32),

    // -- Bottom control overlays --------------------------------------
    // The control's glass capsule is drawn by the Liquid Glass pass (it's a
    // shape in the geometry buffer). These two pipelines draw the foreground
    // ink on top: procedural shapes (magnifier, dots, caret, close) and the
    // cosmic-text glyphs for the label / query / placeholder.
    control_pipeline: RenderPipeline,
    control_uniform_buffer: Buffer,
    control_bind_group: wgpu::BindGroup,
    control_instance_buffer: Option<Buffer>,
    control_instance_count: u32,
    /// Corner gear ink instances (settings entry). Drawn in the control
    /// overlay pass alongside the bottom-control ink.
    gear_instance_buffer: Option<Buffer>,
    gear_instance_count: u32,
    badge_sources: Vec<EditBadgeSource>,
    badge_instance_buffer: Option<Buffer>,
    badge_instance_count: u32,
    control_text_pipeline: RenderPipeline,
    control_text_bind_group: wgpu::BindGroup,
    control_text_instance_buffer: Option<Buffer>,
    control_text_instance_count: u32,
    /// Settings overlay ink (close ×) + title text instances, drawn in a final
    /// overlay pass on top of the panel glass. They reuse the control pipelines.
    settings_instance_buffer: Option<Buffer>,
    settings_instance_count: u32,
    settings_text_instance_buffer: Option<Buffer>,
    settings_text_instance_count: u32,
    /// When set, the next rendered frame is also copied to a host-readable
    /// buffer and saved as a PNG at this path. Driven by the
    /// `LAUNCHPAD_QA_SHOT_FILE` trigger (see `docs/EDIT_MODE_VISUAL_QA.md`) so
    /// CI / sandboxes without foreground access can capture rendered frames.
    /// Cleared after one frame.
    pub qa_shot: Option<std::path::PathBuf>,
}

pub struct DrawArgs {
    pub scroll_x: f32,
    pub viewport: (u32, u32),
    pub defer_backdrop_capture: bool,
    /// Global animation clock in seconds, fed to the shaders for the edit-mode
    /// wiggle. Caller accumulates this from the redraw cadence.
    pub time: f32,
    /// 1.0 while an edit-mode drag is in flight, else 0.0.
    pub drag_active: f32,
    /// Pointer position (screen px) the dragged icon follows while dragging.
    pub drag_pos: (f32, f32),
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
        event_proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // On Windows we render to a transparent winit window. The default DX12
        // swapchain (DxgiFromHwnd) can't carry per-pixel alpha to the DWM, so
        // the window's clear areas read as black instead of see-through.
        // DirectComposition (DxgiFromVisual) composes the swapchain through a
        // DComp visual, which is what makes real transparency work. Allow an
        // explicit override via WGPU_DX12_PRESENTATION_SYSTEM for debugging.
        let mut dx12 = wgpu::Dx12BackendOptions::from_env_or_default();
        if wgpu::Dx12SwapchainKind::from_env().is_none() {
            dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
        }

        let instance = Instance::new(wgpu::InstanceDescriptor {
            backends: default_backends(),
            backend_options: wgpu::BackendOptions {
                dx12,
                ..wgpu::BackendOptions::from_env_or_default()
            },
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
            // COPY_SRC is only needed when `qa_shot` captures the frame, but
            // wgpu requires the usage to be set at config time, so we always
            // include it. The cost is negligible.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            format: surface_format,
            width: size.width.max(1).min(max_dim),
            height: size.height.max(1).min(max_dim),
            present_mode: select_present_mode(&caps.present_modes),
            desired_maximum_frame_latency: 2,
            alpha_mode: select_alpha_mode(&caps.alpha_modes),
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
                // The fragment stage also reads the uniforms (frame clip rect).
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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

        // Instance buffer: written once. Empty app list here — `rebuild_instances`
        // (called from App::relayout after icons load) supplies the real one.
        let instances = layout.build_instances(config.width as f32, &[], &[]);
        let instance_count = instances.len() as u32;
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("instance buffer"),
            contents: bytemuck::cast_slice(&instances),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        // Initial uniform upload.
        let frame = frame_clip(layout, size.width);
        queue.write_buffer(
            &uniform_buffer,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: [size.width as f32, size.height as f32],
                scroll_x: 0.0,
                time: 0.0,
                frame_center: [frame.0, frame.1],
                frame_half_size: [frame.2, frame.3],
                frame_radius: frame.4,
                drag_active: 0.0,
                drag_pos: [0.0, 0.0],
            }),
        );

        // ---- Text pipeline + glyph atlas --------------------------------
        let (aw, ah) = crate::text::TextRenderer::atlas_dimensions();
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: aw,
                height: ah,
                depth_or_array_layers: 1,
            },
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
                    // The fragment stage also reads the uniforms (frame clip rect).
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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

        // ---- Icon pipeline + atlas --------------------------------------
        // The atlas starts as a 1×1 placeholder; the real atlas is uploaded
        // once the launcher's icon set is loaded (see upload_icon_atlas).
        let icon_atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("icon atlas placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // sRGB-encoded: icon pixels are stored as sRGB bytes, so sampling
            // auto-decodes to linear for correct compositing onto the sRGB
            // surface. Using plain Rgba8Unorm would double-apply gamma and wash
            // colors out.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let icon_atlas_view = icon_atlas_texture.create_view(&TextureViewDescriptor::default());
        // The icon sampler matches the glyph sampler: clamp + linear.
        let icon_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("icon atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        // Reuse the text bind group layout: uniform[0] + texture[1] + sampler[2].
        let icon_atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("icon atlas bg"),
            layout: &text_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&icon_atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&icon_sampler),
                },
            ],
        });
        let icon_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("icon shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_icon.wgsl").into()),
        });
        let icon_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("icon pipeline layout"),
            bind_group_layouts: &[Some(&text_bgl)],
            immediate_size: 0,
        });
        let icon_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("icon pipeline"),
            layout: Some(&icon_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &icon_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[IconInstance::LAYOUT],
            },
            fragment: Some(wgpu::FragmentState {
                module: &icon_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    // The icon shader outputs premultiplied alpha (rgb*a, a).
                    // PREMULTIPLIED_ALPHA_BLENDING = src.rgb*1 + dst.rgb*(1-src.a),
                    // which is correct for premultiplied output. ALPHA_BLENDING
                    // would double-multiply by alpha and wash colors out.
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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

        let capture = create_backdrop_capture(&window, event_proxy);
        let liquid_glass = LiquidGlassRenderer::new(
            &device,
            &queue,
            surface_format,
            config.width,
            config.height,
            layout,
            capture,
        );

        // ---- Bottom-control overlay pipelines ---------------------------
        // Small viewport-only uniform shared by the control shape + text
        // shaders. Reuses the text bind group layout (uniform + atlas +
        // sampler) so the text pipeline can sample the glyph atlas; the shape
        // pipeline binds only [0].
        let control_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("control uniform buffer"),
            size: std::mem::size_of::<ControlUniforms>() as BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let control_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("control bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(
                            std::mem::size_of::<ControlUniforms>() as u64
                        ),
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
        let control_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("control bg"),
            layout: &control_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: control_uniform_buffer.as_entire_binding(),
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

        let control_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("control overlay shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_control.wgsl").into()),
        });
        let control_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("control overlay pipeline layout"),
                bind_group_layouts: &[Some(&control_bgl)],
                immediate_size: 0,
            });
        let control_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("control overlay pipeline"),
            layout: Some(&control_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &control_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[ControlInstance::LAYOUT],
            },
            fragment: Some(wgpu::FragmentState {
                module: &control_shader,
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

        // Control text pipeline: same bind group (uniform + atlas + sampler).
        let control_text_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("control text shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader_control_text.wgsl").into()),
        });
        let control_text_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("control text pipeline"),
                layout: Some(&control_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &control_text_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[GlyphQuad::LAYOUT],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &control_text_shader,
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
            decorated: false,
            instance_buffer,
            uniform_buffer,
            uniform_bind_group,
            instance_count,
            surface_format,
            liquid_glass,
            text_pipeline,
            text_instance_buffer: None,
            text_instance_count: 0,
            atlas_texture,
            atlas_bind_group,
            text_bgl,
            icon_pipeline,
            icon_instance_buffer: None,
            icon_instance_count: 0,
            dragged_icon_instance: false,
            icon_atlas_texture,
            icon_atlas_bind_group,
            frame_clip: frame_clip(layout, size.width),
            control_pipeline,
            control_uniform_buffer,
            control_bind_group: control_bind_group.clone(),
            control_instance_buffer: None,
            control_instance_count: 0,
            gear_instance_buffer: None,
            gear_instance_count: 0,
            badge_sources: Vec::new(),
            badge_instance_buffer: None,
            badge_instance_count: 0,
            control_text_pipeline,
            control_text_bind_group: control_bind_group,
            control_text_instance_buffer: None,
            control_text_instance_count: 0,
            settings_instance_buffer: None,
            settings_instance_count: 0,
            settings_text_instance_buffer: None,
            settings_text_instance_count: 0,
            qa_shot: None,
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
        self.liquid_glass.resize(
            &self.device,
            &self.queue,
            self.config.width,
            self.config.height,
        );
    }

    #[allow(dead_code)]
    pub fn surface_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Rebuild the static instance buffer from a fresh layout.
    ///
    /// Call after a resize (or any change to tile data) so the GPU sees the
    /// new tile positions. The buffer is reallocated to fit.
    pub fn rebuild_instances(
        &mut self,
        layout: &GridLayout,
        apps: &[crate::grid::GridApp<'_>],
        anim: &[crate::grid::TileAnim],
    ) {
        let instances = layout.build_instances(self.config.width as f32, apps, anim);
        self.instance_count = instances.len() as u32;
        self.instance_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("instance buffer"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
        self.liquid_glass
            .rebuild_shapes(&self.device, layout, self.config.width as f32, apps);
        self.frame_clip = frame_clip(layout, self.config.width);
    }

    /// Push a caller-built tile instance list to the GPU, reallocating the
    /// buffer to fit. Used by the reorder animation, which overrides the tile
    /// positions with per-tile spring offsets before uploading.
    pub fn set_tile_instances(&mut self, instances: &[crate::grid::TileInstance]) {
        self.instance_count = instances.len() as u32;
        self.instance_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("instance buffer"),
                contents: bytemuck::cast_slice(instances),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            });
        self.badge_sources = edit_badge_sources(instances);
        self.update_edit_badges(0.0);
    }

    pub fn handle_liquid_glass_key(&mut self, key: winit::keyboard::KeyCode) -> bool {
        self.liquid_glass.handle_debug_key(key)
    }

    /// Toggle the OS window frame on/off. Borderless by default; press M to
    /// bring back the title bar + resize edges while debugging.
    pub fn toggle_decorations(&mut self) {
        self.decorated = !self.decorated;
        self.window.set_decorations(self.decorated);
        eprintln!(
            "window decorations: {}",
            if self.decorated { "on" } else { "off" }
        );
    }

    pub fn notify_window_moved(&mut self) {
        self.liquid_glass.notify_window_moved();
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
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Replace the per-glyph text instance buffer.
    pub fn set_text_instances(&mut self, quads: &[GlyphQuad]) {
        self.text_instance_count = quads.len() as u32;
        if quads.is_empty() {
            self.text_instance_buffer = None;
            return;
        }
        self.text_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("text instance buffer"),
                contents: bytemuck::cast_slice(quads),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Upload the icon atlas, replacing the 1×1 placeholder created in `new`.
    ///
    /// Reallocates the texture to match `(w, h)` and rebuilds the bind group
    /// that points the icon pipeline at it. Call once after icons are loaded;
    /// safe to call again if the atlas changes (e.g. app list refresh).
    pub fn upload_icon_atlas(&mut self, rgba: &[u8], w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let reallocated =
            self.icon_atlas_texture.width() != w || self.icon_atlas_texture.height() != h;
        if reallocated {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("icon atlas"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // sRGB-encoded: icon pixels are stored as sRGB bytes, so sampling
                // auto-decodes to linear for correct compositing onto the sRGB
                // surface. Using plain Rgba8Unorm would double-apply gamma and wash
                // colors out.
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.icon_atlas_texture = texture;
            self.rebind_icon_atlas();
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.icon_atlas_texture,
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
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Update a single icon's pixels in the existing atlas texture (fixed-slot
    /// design). `(x, y)` is the icon's top-left inside the texture (including
    /// the cell padding); `w`/`h` is the icon bitmap size. Cheaper than a full
    /// `upload_icon_atlas` re-blit and leaves all other UVs untouched.
    pub fn write_icon_cell(&self, rgba: &[u8], x: u32, y: u32, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.icon_atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Current icon-atlas texture dimensions, so callers can tell whether a
    /// full re-upload (reallocate) is needed vs. a partial cell write.
    pub fn icon_atlas_size(&self) -> (u32, u32) {
        (
            self.icon_atlas_texture.width(),
            self.icon_atlas_texture.height(),
        )
    }

    /// Rebuild the icon atlas bind group against the current texture. Used
    /// after `icon_atlas_texture` is reallocated.
    fn rebind_icon_atlas(&mut self) {
        let view = self
            .icon_atlas_texture
            .create_view(&TextureViewDescriptor::default());
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("icon atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        self.icon_atlas_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("icon atlas bg"),
            layout: &self.text_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
    }

    /// Replace the per-icon instance buffer (one entry per tile with an icon).
    pub fn set_icon_instances(&mut self, instances: &[IconInstance]) {
        self.icon_instance_count = instances.len() as u32;
        self.dragged_icon_instance = instances
            .last()
            .map(|i| (i.extra[3] as u32 & 2) != 0)
            .unwrap_or(false);
        if instances.is_empty() {
            self.icon_instance_buffer = None;
            return;
        }
        self.icon_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("icon instance buffer"),
                contents: bytemuck::cast_slice(instances),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Push the bottom-control's glass capsule shape into the Liquid Glass
    /// geometry buffer. `None` hides the control. Called every frame from the
    /// app (the geometry is tiny and rebuilt cheaply).
    pub fn set_control_glass_shape(
        &mut self,
        shape: Option<crate::liquid_glass::geometry::GlassShape>,
    ) {
        self.liquid_glass.set_control_shape(&self.device, shape);
    }

    /// Push the corner gear's glass capsule shape. `None` hides it.
    pub fn set_gear_glass_shape(
        &mut self,
        shape: Option<crate::liquid_glass::geometry::GlassShape>,
    ) {
        self.liquid_glass.set_gear_shape(&self.device, shape);
    }

    /// Push the settings overlay panel shape. `None` hides it.
    pub fn set_settings_panel_glass_shape(
        &mut self,
        shape: Option<crate::liquid_glass::geometry::GlassShape>,
    ) {
        self.liquid_glass
            .set_settings_panel_shape(&self.device, shape);
    }

    /// Replace the procedural overlay instances (magnifier, dots, caret,
    /// close ×) for the bottom control.
    pub fn set_control_instances(&mut self, instances: &[ControlInstance]) {
        self.control_instance_count = instances.len() as u32;
        if instances.is_empty() {
            self.control_instance_buffer = None;
            return;
        }
        self.control_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("control instance buffer"),
                contents: bytemuck::cast_slice(instances),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Replace the corner gear ink instances. Drawn in the same control
    /// overlay pass as the bottom-control ink (they share the pipeline).
    pub fn set_gear_instances(&mut self, instances: &[ControlInstance]) {
        self.gear_instance_count = instances.len() as u32;
        if instances.is_empty() {
            self.gear_instance_buffer = None;
            return;
        }
        self.gear_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("gear instance buffer"),
                contents: bytemuck::cast_slice(instances),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Replace the settings overlay ink instances (close ×). Drawn in a final
    /// overlay pass on top of the panel glass.
    pub fn set_settings_instances(&mut self, instances: &[ControlInstance]) {
        self.settings_instance_count = instances.len() as u32;
        if instances.is_empty() {
            self.settings_instance_buffer = None;
            return;
        }
        self.settings_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("settings instance buffer"),
                contents: bytemuck::cast_slice(instances),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Replace the settings overlay text quads (title).
    pub fn set_settings_text_instances(&mut self, quads: &[GlyphQuad]) {
        self.settings_text_instance_count = quads.len() as u32;
        if quads.is_empty() {
            self.settings_text_instance_buffer = None;
            return;
        }
        self.settings_text_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("settings text instance buffer"),
                contents: bytemuck::cast_slice(quads),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Replace the text glyph quads for the bottom control (label / query /
    /// placeholder).
    pub fn set_control_text_instances(&mut self, quads: &[GlyphQuad]) {
        self.control_text_instance_count = quads.len() as u32;
        if quads.is_empty() {
            self.control_text_instance_buffer = None;
            return;
        }
        self.control_text_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("control text instance buffer"),
                contents: bytemuck::cast_slice(quads),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Render one frame.
    pub fn render(&mut self, args: &DrawArgs) {
        // Update uniforms (tiny, every frame).
        let clip = self.frame_clip;
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: [args.viewport.0 as f32, args.viewport.1 as f32],
                scroll_x: args.scroll_x,
                time: args.time,
                frame_center: [clip.0, clip.1],
                frame_half_size: [clip.2, clip.3],
                frame_radius: clip.4,
                drag_active: args.drag_active,
                drag_pos: [args.drag_pos.0, args.drag_pos.1],
            }),
        );

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
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
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("surface clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            drop(pass);
        }

        self.liquid_glass.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &view,
            args.scroll_x,
            args.defer_backdrop_capture,
        );

        let drag_active = args.drag_active > 0.5 && self.instance_count > 0;
        let normal_tile_count = if drag_active {
            self.instance_count - 1
        } else {
            self.instance_count
        };
        let drag_icon_active = self.dragged_icon_instance && self.icon_instance_count > 0;
        let normal_icon_count = if drag_icon_active {
            self.icon_instance_count - 1
        } else {
            self.icon_instance_count
        };

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tile pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // Normal color tiles. The dragged tile, if any, is withheld and
            // drawn again after badges so its lifted visual unit stays above
            // every non-dragged edit badge.
            if normal_tile_count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
                // 6 verts per quad (two tris), instance_count quads.
                pass.draw(0..6, 0..normal_tile_count);
            }

            // Normal icons: drawn over the color tiles before labels. The
            // dragged icon, if any, is withheld until after text.
            if normal_icon_count > 0 {
                if let Some(buf) = self.icon_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.icon_pipeline);
                    pass.set_bind_group(0, &self.icon_atlas_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..normal_icon_count);
                }
            }

            // Text labels: same pass, third draw call. Uses the same
            // uniform (scroll/viewport) plus the atlas texture.
            if self.text_instance_count > 0 {
                if let Some(buf) = self.text_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.text_pipeline);
                    pass.set_bind_group(0, &self.atlas_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.text_instance_count);
                }
            }
        }

        // Edit badges sit above the normal grid but below the lifted dragged
        // icon. The bottom control remains a later, screen-fixed overlay.
        self.update_edit_badges(args.time);
        self.queue.write_buffer(
            &self.control_uniform_buffer,
            0,
            bytemuck::bytes_of(&ControlUniforms {
                viewport_scroll: [
                    args.viewport.0 as f32,
                    args.viewport.1 as f32,
                    args.scroll_x,
                    0.0,
                ],
                frame_center_radius: [clip.0, clip.1, clip.4, 0.0],
                frame_half_size: [clip.2, clip.3, 0.0, 0.0],
            }),
        );
        self.liquid_glass
            .render_badges(&self.queue, &mut encoder, &view, args.scroll_x);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("edit badge foreground pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.badge_instance_count > 0 {
                if let Some(buf) = self.badge_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_pipeline);
                    pass.set_bind_group(0, &self.control_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.badge_instance_count);
                }
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("drag overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if drag_active {
                let stride =
                    std::mem::size_of::<crate::grid::TileInstance>() as wgpu::BufferAddress;
                let offset = stride * normal_tile_count as wgpu::BufferAddress;
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_vertex_buffer(0, self.instance_buffer.slice(offset..));
                pass.draw(0..6, 0..1);
            }
            if drag_icon_active {
                if let Some(buf) = self.icon_instance_buffer.as_ref() {
                    let stride = std::mem::size_of::<crate::icon_pipeline::IconInstance>()
                        as wgpu::BufferAddress;
                    let offset = stride * normal_icon_count as wgpu::BufferAddress;
                    pass.set_pipeline(&self.icon_pipeline);
                    pass.set_bind_group(0, &self.icon_atlas_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(offset..));
                    pass.draw(0..6, 0..1);
                }
            }
        }

        self.liquid_glass
            .render_control(&self.queue, &mut encoder, &view);

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("control overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.control_instance_count > 0 {
                if let Some(buf) = self.control_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_pipeline);
                    pass.set_bind_group(0, &self.control_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.control_instance_count);
                }
            }
            // Corner gear ink shares the control ink pipeline.
            if self.gear_instance_count > 0 {
                if let Some(buf) = self.gear_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_pipeline);
                    pass.set_bind_group(0, &self.control_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.gear_instance_count);
                }
            }
            if self.control_text_instance_count > 0 {
                if let Some(buf) = self.control_text_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_text_pipeline);
                    pass.set_bind_group(0, &self.control_text_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.control_text_instance_count);
                }
            }
        }

        // Settings overlay panel — drawn last so it composites over everything
        // (grid, control, gear).
        self.liquid_glass
            .render_settings_panel(&self.queue, &mut encoder, &view);

        // Settings panel ink (close ×) + title text, on top of the panel glass.
        if self.settings_instance_count > 0 || self.settings_text_instance_count > 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("settings overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.settings_instance_count > 0 {
                if let Some(buf) = self.settings_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_pipeline);
                    pass.set_bind_group(0, &self.control_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.settings_instance_count);
                }
            }
            if self.settings_text_instance_count > 0 {
                if let Some(buf) = self.settings_text_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_text_pipeline);
                    pass.set_bind_group(0, &self.control_text_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.settings_text_instance_count);
                }
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        // Optional QA self-capture: copy the surface texture to a host-readable
        // buffer and save it as PNG. Driven by `LAUNCHPAD_QA_SHOT_FILE`.
        if let Some(path) = self.qa_shot.take() {
            self.save_frame_png(&frame.texture, path);
        }

        frame.present();
    }

    /// Copy `src` (the current surface texture) into a host buffer and write it
    /// to `path` as a PNG. Used only by the `qa_shot` QA harness; lets CI /
    /// sandboxes capture rendered frames without foreground access. See
    /// `docs/EDIT_MODE_VISUAL_QA.md` for the trigger protocol.
    fn save_frame_png(&self, src: &wgpu::Texture, path: std::path::PathBuf) {
        let w = src.width();
        let h = src.height();
        if w == 0 || h == 0 {
            return;
        }
        let bytes_per_row = w * 4;
        let padded = (bytes_per_row + 255) & !255; // wgpu requires 256-byte align
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("qa capture buffer"),
            size: (padded as u64) * (h as u64),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("qa capture encoder"),
            });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: src,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(enc.finish()));

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        {
            let data = slice.get_mapped_range();
            // De-pad rows into a tight RGBA buffer, then save.
            let mut pixels: Vec<u8> = Vec::with_capacity((bytes_per_row as usize) * (h as usize));
            for row in 0..h {
                let start = (row as usize) * (padded as usize);
                pixels.extend_from_slice(&data[start..start + bytes_per_row as usize]);
            }
            // Reuse the `image` crate already in the dependency tree.
            if let Some(img) = image::RgbaImage::from_raw(w, h, pixels) {
                let _ = img.save(&path);
            }
        }
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

fn default_backends() -> Backends {
    #[cfg(windows)]
    {
        Backends::DX12
    }
    #[cfg(not(windows))]
    {
        Backends::DX12 | Backends::VULKAN
    }
}

#[derive(Debug, Clone, Copy)]
struct EditBadgeSource {
    base_center: [f32; 2],
    tile_center: [f32; 2],
    radius: f32,
    phase: f32,
}

impl Renderer {
    fn update_edit_badges(&mut self, time: f32) {
        const KIND_BADGE_CLOSE: f32 = 4.0;

        let mut shapes = Vec::with_capacity(self.badge_sources.len() + 1);
        let mut marks = Vec::with_capacity(self.badge_sources.len());
        let frame = self.frame_clip;
        let clip_shape = GlassShape::clip_rounded_rect(
            [frame.0, frame.1],
            [frame.2 * 2.0, frame.3 * 2.0],
            frame.4,
        );
        for source in &self.badge_sources {
            let center = animated_badge_center(*source, time);
            shapes.push(GlassShape::rounded_rect(
                center,
                [source.radius * 2.15, source.radius * 2.15],
                source.radius,
            ));
            marks.push(ControlInstance {
                center,
                params: [source.radius, 0.92, (source.radius * 0.13).max(1.4), 0.0],
                color: [1.0, 1.0, 1.0, 0.92],
                kind: [KIND_BADGE_CLOSE, 0.0, 0.0, 0.0],
            });
        }

        if !marks.is_empty() {
            shapes.insert(0, clip_shape);
        }

        self.liquid_glass.set_badge_shapes(&self.device, &shapes);
        self.badge_instance_count = marks.len() as u32;
        self.badge_instance_buffer = if marks.is_empty() {
            None
        } else {
            Some(
                self.device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("badge foreground instance buffer"),
                        contents: bytemuck::cast_slice(&marks),
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    }),
            )
        };
    }
}

fn edit_badge_sources(instances: &[TileInstance]) -> Vec<EditBadgeSource> {
    const FLAG_WIGGLE: u32 = crate::grid::TileAnim::FLAG_WIGGLE;
    const FLAG_DRAG: u32 = crate::grid::TileAnim::FLAG_DRAG;

    let mut sources = Vec::new();
    for tile in instances {
        let flags = tile.extra[3] as u32;
        if flags & FLAG_WIGGLE == 0 || flags & FLAG_DRAG != 0 {
            continue;
        }

        let radius = crate::grid::edit_badge_radius_for_tile_size(tile.size);
        let inset = radius * 0.45;
        let center = [tile.x + inset, tile.y + inset];
        sources.push(EditBadgeSource {
            base_center: center,
            tile_center: [tile.x + tile.size * 0.5, tile.y + tile.size * 0.5],
            radius,
            phase: tile.extra[0],
        });
    }

    sources
}

fn animated_badge_center(source: EditBadgeSource, time: f32) -> [f32; 2] {
    let t = time + source.phase;
    let rot = (t * 8.0).sin() * 0.06;
    let dy = (t * 8.0).sin().abs() * 2.0;
    let rel_x = source.base_center[0] - source.tile_center[0];
    let rel_y = source.base_center[1] - source.tile_center[1];
    let cosr = rot.cos();
    let sinr = rot.sin();

    [
        source.tile_center[0] + rel_x * cosr - rel_y * sinr,
        source.tile_center[1] + rel_x * sinr + rel_y * cosr - dy,
    ]
}

/// Frame clip geometry for the tile/icon/text shaders: `(cx, cy, hw, hh, r)`
/// — center, half-size, and corner radius of the fixed page frame, in physical
/// px. Single source is `GridLayout::frame_panel_rect`.
fn frame_clip(layout: &GridLayout, viewport_w: u32) -> (f32, f32, f32, f32, f32) {
    let (cx, cy, w, h) = layout.frame_panel_rect(viewport_w.max(1) as f32);
    (
        cx,
        cy,
        w * 0.5,
        h * 0.5,
        layout.scaled(crate::grid::FRAME_CORNER_RADIUS),
    )
}

fn select_alpha_mode(available: &[CompositeAlphaMode]) -> CompositeAlphaMode {
    let selected = if available.contains(&CompositeAlphaMode::PreMultiplied) {
        CompositeAlphaMode::PreMultiplied
    } else if available.contains(&CompositeAlphaMode::PostMultiplied) {
        CompositeAlphaMode::PostMultiplied
    } else if available.contains(&CompositeAlphaMode::Auto) {
        CompositeAlphaMode::Auto
    } else {
        CompositeAlphaMode::Opaque
    };
    eprintln!(
        "surface alpha_mode: {:?} (available: {:?})",
        selected, available
    );
    selected
}

fn create_backdrop_capture(
    window: &winit::window::Window,
    event_proxy: winit::event_loop::EventLoopProxy<UserEvent>,
) -> Box<dyn crate::liquid_glass::capture::BackdropCapture> {
    #[cfg(windows)]
    {
        match crate::liquid_glass::windows_capture::create_monitor_capture(window, event_proxy) {
            Ok(capture) => capture,
            Err(err) => {
                match crate::liquid_glass::windows_capture::enable_system_backdrop_fallback(window)
                {
                    Ok(()) => eprintln!("liquid glass fallback: DWM system backdrop enabled"),
                    Err(fallback_err) => {
                        eprintln!(
                            "liquid glass fallback: DWM system backdrop failed: {fallback_err}"
                        )
                    }
                }
                Box::new(FallbackCapture::new(format!(
                    "Windows.Graphics.Capture initialization failed: {err}"
                )))
            }
        }
    }

    #[cfg(not(windows))]
    {
        let _ = (window, event_proxy);
        Box::new(FallbackCapture::new(
            "Windows.Graphics.Capture is only available on Windows",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(size: f32) -> TileInstance {
        TileInstance {
            x: 100.0,
            y: 50.0,
            size,
            radius: 19.0,
            r: 0.0,
            g: 0.0,
            b: 0.0,
            icon_index: -1.0,
            extra: [0.25, 0.0, 1.0, crate::grid::TileAnim::FLAG_WIGGLE as f32],
        }
    }

    #[test]
    fn edit_badge_sources_use_scaled_radius() {
        let normal = edit_badge_sources(&[tile(crate::grid::BASE_TILE_SIZE)]);
        let scaled = edit_badge_sources(&[tile(crate::grid::BASE_TILE_SIZE * 1.5)]);

        assert!((scaled[0].radius - normal[0].radius * 1.5).abs() < 1e-2);
    }

    #[test]
    fn edit_badge_center_starts_on_tile_top_left() {
        let source = edit_badge_sources(&[tile(crate::grid::BASE_TILE_SIZE)])[0];
        let inset = source.radius * 0.45;

        assert!((source.base_center[0] - (100.0 + inset)).abs() < 1e-4);
        assert!((source.base_center[1] - (50.0 + inset)).abs() < 1e-4);
    }
}
