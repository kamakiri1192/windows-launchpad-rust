//! Device / surface / pipeline initialization and window lifecycle helpers.

use std::num::NonZeroU64;
use std::sync::Arc;

use wgpu::{
    Backends, BufferAddress, CompositeAlphaMode, Instance, Limits, PresentMode,
    SurfaceConfiguration, TextureViewDescriptor,
};

use crate::layout::grid::GridLayout;
use crate::liquid_glass::capture::FallbackCapture;
use crate::liquid_glass::LiquidGlassRenderer;
use crate::renderer::controls::ControlInstance;
use crate::renderer::icon_pipeline::IconInstance;
use crate::renderer::text_engine::GlyphQuad;
use crate::renderer::tiles::TileInstance;
use crate::UserEvent;

use super::controls::ControlUniforms;
use super::counters::BufferCounters;
use super::focus_blur::FocusBlurRenderer;
use super::frame_clip;
use super::resources::InstanceBuffer;
use super::tiles::Uniforms;
use super::Renderer;

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
        backdrop_capture_enabled: bool,
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

        let qa_headless = std::env::var_os(crate::qa::HEADLESS_ENV).is_some();
        // Safety: we move `window` into the returned Renderer immediately, so
        // the surface's `'static` borrow is valid for as long as the Renderer
        // exists. Headless QA skips surface creation entirely.
        let surface = if qa_headless {
            None
        } else {
            Some(unsafe {
                let static_window: &'static winit::window::Window = &*(&window as *const _);
                instance.create_surface(static_window)?
            })
        };

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface.as_ref(),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| format!("no suitable GPU adapter: {e}"))?;

        let required_features =
            super::gpu_profile::GpuProfilerState::required_features(adapter.features());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("launchpad device"),
                required_features,
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

        let (surface_format, present_mode, alpha_mode) = if let Some(surface) = &surface {
            let caps = surface.get_capabilities(&adapter);
            (
                caps.formats
                    .iter()
                    .copied()
                    .find(|format| format.is_srgb())
                    .unwrap_or(caps.formats[0]),
                select_present_mode(&caps.present_modes),
                select_alpha_mode(&caps.alpha_modes),
            )
        } else {
            (
                wgpu::TextureFormat::Rgba8UnormSrgb,
                PresentMode::Fifo,
                CompositeAlphaMode::Auto,
            )
        };

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
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        if let Some(surface) = &surface {
            surface.configure(&device, &config);
        }
        let qa_offscreen = qa_headless.then(|| create_qa_offscreen_texture(&device, &config));

        // Shaders
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tile shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shader.wgsl").into()),
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

        // Instance buffer starts empty; `Renderer::prepare` (called from
        // App::relayout after icons load) supplies the real tiles via the
        // capacity-managed `InstanceBuffer`.
        let _ = layout.build_instances(config.width as f32, &[], &[]);

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
        let (aw, ah) = crate::renderer::text_engine::TextRenderer::atlas_dimensions();
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
            source: wgpu::ShaderSource::Wgsl(include_str!("../shader_text.wgsl").into()),
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
            source: wgpu::ShaderSource::Wgsl(include_str!("../shader_icon.wgsl").into()),
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

        let capture = create_backdrop_capture(&window, event_proxy, backdrop_capture_enabled);
        let liquid_glass = LiquidGlassRenderer::new(
            &device,
            &queue,
            surface_format,
            config.width,
            config.height,
            layout,
            capture,
        );
        let focus_blur =
            FocusBlurRenderer::new(&device, surface_format, config.width, config.height);
        let gpu_profiler = super::gpu_profile::GpuProfilerState::new(&device);

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
            source: wgpu::ShaderSource::Wgsl(include_str!("../shader_control.wgsl").into()),
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
            source: wgpu::ShaderSource::Wgsl(include_str!("../shader_control_text.wgsl").into()),
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
            qa_offscreen,
            config,
            pipeline,
            decorated: false,
            instance_buffer: InstanceBuffer::new("instance buffer"),
            top_level_dragged_tile_instance: false,
            modal_tile_instance_buffer: InstanceBuffer::new("modal tile instance buffer"),
            modal_dragged_tile_instance: false,
            uniform_buffer,
            uniform_bind_group,
            surface_format,
            liquid_glass,
            focus_blur,
            gpu_profiler,
            text_pipeline,
            text_instance_buffer: InstanceBuffer::new("text instance buffer"),
            atlas_texture,
            atlas_bind_group,
            text_bgl,
            icon_pipeline,
            icon_instance_buffer: InstanceBuffer::new("icon instance buffer"),
            modal_icon_instance_buffer: InstanceBuffer::new("modal icon instance buffer"),
            modal_dragged_icon_instance: false,
            dragged_icon_instance_count: 0,
            icon_atlas_texture,
            icon_atlas_bind_group,
            frame_clip: frame_clip(layout, size.width),
            modal_clip_rect: None,
            modal_clip_radius: 0.0,
            control_pipeline,
            control_uniform_buffer,
            control_bind_group: control_bind_group.clone(),
            control_instance_buffer: InstanceBuffer::new("control instance buffer"),
            backdrop_instance_buffer: InstanceBuffer::new("backdrop instance buffer"),
            gear_instance_buffer: InstanceBuffer::new("gear instance buffer"),
            badge_sources: Vec::new(),
            badge_shape_scratch: Vec::new(),
            badge_mark_scratch: Vec::new(),
            badge_instance_buffer: InstanceBuffer::new("badge foreground instance buffer"),
            modal_badge_sources: Vec::new(),
            modal_badge_shape_scratch: Vec::new(),
            modal_badge_mark_scratch: Vec::new(),
            modal_badge_instance_buffer: InstanceBuffer::new(
                "modal badge foreground instance buffer",
            ),
            control_text_pipeline,
            control_text_bind_group: control_bind_group,
            control_text_instance_buffer: InstanceBuffer::new("control text instance buffer"),
            settings_instance_buffer: InstanceBuffer::new("settings instance buffer"),
            settings_text_instance_buffer: InstanceBuffer::new("settings text instance buffer"),
            modal_instance_buffer: InstanceBuffer::new("modal ink instance buffer"),
            modal_text_instance_buffer: InstanceBuffer::new("modal text instance buffer"),
            prepared_model: crate::ui_model::render_model::RenderModel::new(),
            counters: BufferCounters::default(),
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
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        if self.qa_offscreen.is_some() {
            self.qa_offscreen = Some(create_qa_offscreen_texture(&self.device, &self.config));
        }
        self.liquid_glass.resize(
            &self.device,
            &self.queue,
            self.config.width,
            self.config.height,
        );
        self.focus_blur
            .resize(&self.device, self.config.width, self.config.height);
    }

    #[allow(dead_code)]
    pub fn surface_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
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
        if let Ok(position) = self.window.outer_position() {
            self.liquid_glass.notify_window_moved(
                position.x,
                position.y,
                self.window.scale_factor(),
            );
        }
    }
}

/// Use FIFO VSync for the continuously animated launcher surface. Mailbox
/// replaces queued frames and allows the edit-mode redraw loop to run up to
/// `maximum_frame_latency * monitor_hz` on DX12, which can saturate the GPU
/// while most submitted frames are never displayed.
fn select_present_mode(available: &[PresentMode]) -> PresentMode {
    let selected = if available.contains(&PresentMode::Fifo) {
        PresentMode::Fifo
    } else if available.contains(&PresentMode::AutoVsync) {
        PresentMode::AutoVsync
    } else if available.contains(&PresentMode::Mailbox) {
        PresentMode::Mailbox
    } else {
        PresentMode::Fifo
    };
    eprintln!(
        "surface present_mode: {:?} (available: {:?})",
        selected, available
    );
    selected
}

fn default_backends() -> Backends {
    #[cfg(windows)]
    {
        Backends::DX12
    }
    #[cfg(target_os = "macos")]
    {
        Backends::METAL
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Backends::VULKAN
    }
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
    enabled: bool,
) -> Box<dyn crate::liquid_glass::capture::BackdropCapture> {
    if !enabled {
        return Box::new(FallbackCapture::new(
            "desktop backdrop capture disabled for deterministic QA",
        ));
    }

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

    #[cfg(target_os = "macos")]
    {
        let _ = event_proxy;
        match crate::liquid_glass::macos_capture::create_monitor_capture(window) {
            Ok(capture) => capture,
            Err(error) => Box::new(FallbackCapture::new(format!(
                "ScreenCaptureKit initialization failed: {error}"
            ))),
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (window, event_proxy);
        Box::new(FallbackCapture::new(
            "desktop backdrop capture is unavailable on this platform",
        ))
    }
}

fn create_qa_offscreen_texture(
    device: &wgpu::Device,
    config: &SurfaceConfiguration,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("headless QA render target"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: config.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

#[cfg(test)]
mod present_mode_tests {
    use super::*;

    #[test]
    fn fifo_is_preferred_over_mailbox_for_animation_pacing() {
        assert_eq!(
            select_present_mode(&[PresentMode::Mailbox, PresentMode::Fifo]),
            PresentMode::Fifo
        );
    }
}
