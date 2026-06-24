use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use wgpu::util::DeviceExt;

use super::capture::{BackdropCapture, CaptureStatus};
use super::geometry::{shapes_from_layout, GlassShape};
use super::params::{DebugOptions, LiquidGlassParams};
use crate::grid::GridLayout;

const GEOMETRY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const BACKDROP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const BLUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlassUniforms {
    viewport: [f32; 2],
    scroll_x: f32,
    thickness: f32,
    refractive_index: f32,
    chromatic_aberration: f32,
    blur_radius: f32,
    saturation: f32,
    glass_color: [f32; 4],
    light_direction: [f32; 2],
    light_intensity: f32,
    ambient_strength: f32,
    blend: f32,
    max_displacement: f32,
    shape_count: u32,
    debug_flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurUniforms {
    texel_step: [f32; 2],
    radius: f32,
    _pad: f32,
}

#[derive(Debug)]
struct RenderStats {
    last_report_at: Instant,
    frames: u32,
    captured_frames: u32,
    capture_time: Duration,
    upload_time: Duration,
    render_time: Duration,
}

impl RenderStats {
    fn new() -> Self {
        Self {
            last_report_at: Instant::now(),
            frames: 0,
            captured_frames: 0,
            capture_time: Duration::ZERO,
            upload_time: Duration::ZERO,
            render_time: Duration::ZERO,
        }
    }

    fn record(
        &mut self,
        captured: bool,
        capture_time: Duration,
        upload_time: Duration,
        render_time: Duration,
    ) {
        self.frames += 1;
        if captured {
            self.captured_frames += 1;
            self.capture_time += capture_time;
            self.upload_time += upload_time;
        }
        self.render_time += render_time;

        let elapsed = self.last_report_at.elapsed();
        if elapsed < Duration::from_secs(2) {
            return;
        }

        let seconds = elapsed.as_secs_f32().max(0.001);
        let capture_fps = self.captured_frames as f32 / seconds;
        let avg_capture_ms = avg_ms(self.capture_time, self.captured_frames);
        let avg_upload_ms = avg_ms(self.upload_time, self.captured_frames);
        let avg_render_ms = avg_ms(self.render_time, self.frames);
        eprintln!(
            "liquid glass stats: capture_fps={capture_fps:.1} capture_ms={avg_capture_ms:.2} upload_ms={avg_upload_ms:.2} render_ms={avg_render_ms:.2}"
        );

        *self = Self::new();
    }
}

fn avg_ms(total: Duration, count: u32) -> f32 {
    if count == 0 {
        0.0
    } else {
        total.as_secs_f32() * 1000.0 / count as f32
    }
}

pub struct LiquidGlassRenderer {
    params: LiquidGlassParams,
    debug: DebugOptions,
    capture: Box<dyn BackdropCapture>,
    capture_status: CaptureStatus,
    uniform_buffer: wgpu::Buffer,
    shape_buffer: wgpu::Buffer,
    shape_count: u32,
    geometry_texture: wgpu::Texture,
    geometry_view: wgpu::TextureView,
    backdrop_texture: wgpu::Texture,
    backdrop_view: wgpu::TextureView,
    blur_texture: wgpu::Texture,
    blur_view: wgpu::TextureView,
    blur_temp_texture: wgpu::Texture,
    blur_temp_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    geometry_pipeline: wgpu::RenderPipeline,
    blur_downsample_pipeline: wgpu::RenderPipeline,
    blur_upsample_pipeline: wgpu::RenderPipeline,
    final_pipeline: wgpu::RenderPipeline,
    geometry_bind_group_layout: wgpu::BindGroupLayout,
    blur_bind_group_layout: wgpu::BindGroupLayout,
    final_bind_group_layout: wgpu::BindGroupLayout,
    geometry_bind_group: wgpu::BindGroup,
    blur_h_bind_group: wgpu::BindGroup,
    blur_v_bind_group: wgpu::BindGroup,
    final_bind_group: wgpu::BindGroup,
    blur_h_uniform_buffer: wgpu::Buffer,
    blur_v_uniform_buffer: wgpu::Buffer,
    texture_size: (u32, u32),
    blur_size: (u32, u32),
    last_capture_at: Option<Instant>,
    last_geometry_key: Option<GeometryKey>,
    stats: RenderStats,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GeometryKey {
    scroll_x: f32,
    thickness: f32,
    refractive_index: f32,
    blend: f32,
    width: u32,
    height: u32,
    shape_count: u32,
}

impl LiquidGlassRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        layout: &GridLayout,
        capture: Box<dyn BackdropCapture>,
    ) -> Self {
        let params = LiquidGlassParams::default();
        let debug = DebugOptions::default();
        let capture_status = capture.status();
        log_capture_status(&capture_status);

        let uniforms = uniforms_from_params(&params, debug, width, height, 0.0, 0);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("liquid glass uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let shapes = shapes_from_layout(layout, width as f32);
        let shape_buffer = create_shape_buffer(device, &shapes);
        let shape_count = shapes.len() as u32;

        let (geometry_texture, geometry_view) = create_geometry_texture(device, width, height);
        let (backdrop_texture, backdrop_view) = create_backdrop_texture(device, width, height);
        let (blur_texture, blur_view) = create_blur_texture(device, width, height, "blur texture");
        let (blur_temp_texture, blur_temp_view) =
            create_blur_texture(device, width, height, "blur temp texture");
        upload_initial_backdrop(queue, &backdrop_texture, width, height);
        let blur_size = blur_extent(width, height);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("liquid glass sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let geometry_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("liquid glass geometry bgl"),
                entries: &[
                    uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let final_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("liquid glass final bgl"),
                entries: &[
                    uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                    texture_entry(1, true),
                    sampler_entry(2),
                    texture_entry(3, false),
                    texture_entry(4, true),
                ],
            });

        let blur_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("liquid glass blur bgl"),
                entries: &[
                    blur_uniform_entry(0),
                    texture_entry(1, true),
                    sampler_entry(2),
                ],
            });

        let blur_h_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("liquid glass blur horizontal uniforms"),
            contents: bytemuck::bytes_of(&blur_uniforms(
                [1.0 / width.max(1) as f32, 0.0],
                params.blur_radius,
            )),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let blur_v_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("liquid glass blur vertical uniforms"),
            contents: bytemuck::bytes_of(&blur_uniforms(
                [0.0, 1.0 / blur_size.1.max(1) as f32],
                params.blur_radius,
            )),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let geometry_bind_group = create_geometry_bind_group(
            device,
            &geometry_bind_group_layout,
            &uniform_buffer,
            &shape_buffer,
        );
        let final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &uniform_buffer,
            &backdrop_view,
            &sampler,
            &geometry_view,
            &blur_view,
        );
        let blur_h_bind_group = create_blur_bind_group(
            device,
            &blur_bind_group_layout,
            &blur_h_uniform_buffer,
            &backdrop_view,
            &sampler,
        );
        let blur_v_bind_group = create_blur_bind_group(
            device,
            &blur_bind_group_layout,
            &blur_v_uniform_buffer,
            &blur_temp_view,
            &sampler,
        );

        let geometry_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("liquid glass geometry shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_geometry.wgsl").into(),
            ),
        });
        let final_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("liquid glass final shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_final.wgsl").into(),
            ),
        });
        let blur_downsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("liquid glass blur downsample shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_blur_downsample.wgsl").into(),
            ),
        });
        let blur_upsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("liquid glass blur upsample shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_blur_upsample.wgsl").into(),
            ),
        });

        let geometry_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("liquid glass geometry pipeline layout"),
                bind_group_layouts: &[Some(&geometry_bind_group_layout)],
                immediate_size: 0,
            });
        let geometry_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("liquid glass geometry pipeline"),
            layout: Some(&geometry_pipeline_layout),
            vertex: fullscreen_vertex_state(&geometry_shader),
            fragment: Some(wgpu::FragmentState {
                module: &geometry_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: GEOMETRY_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: fullscreen_primitive_state(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("liquid glass blur pipeline layout"),
            bind_group_layouts: &[Some(&blur_bind_group_layout)],
            immediate_size: 0,
        });
        let blur_downsample_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("liquid glass blur downsample pipeline"),
                layout: Some(&blur_pipeline_layout),
                vertex: fullscreen_vertex_state(&blur_downsample_shader),
                fragment: Some(wgpu::FragmentState {
                    module: &blur_downsample_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: BLUR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: fullscreen_primitive_state(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        let blur_upsample_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("liquid glass blur upsample pipeline"),
                layout: Some(&blur_pipeline_layout),
                vertex: fullscreen_vertex_state(&blur_upsample_shader),
                fragment: Some(wgpu::FragmentState {
                    module: &blur_upsample_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: BLUR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: fullscreen_primitive_state(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let final_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("liquid glass final pipeline layout"),
                bind_group_layouts: &[Some(&final_bind_group_layout)],
                immediate_size: 0,
            });
        let final_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("liquid glass final pipeline"),
            layout: Some(&final_pipeline_layout),
            vertex: fullscreen_vertex_state(&final_shader),
            fragment: Some(wgpu::FragmentState {
                module: &final_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(premultiplied_blend()),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: fullscreen_primitive_state(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            params,
            debug,
            capture,
            capture_status,
            uniform_buffer,
            shape_buffer,
            shape_count,
            geometry_texture,
            geometry_view,
            backdrop_texture,
            backdrop_view,
            blur_texture,
            blur_view,
            blur_temp_texture,
            blur_temp_view,
            sampler,
            geometry_pipeline,
            blur_downsample_pipeline,
            blur_upsample_pipeline,
            final_pipeline,
            geometry_bind_group_layout,
            blur_bind_group_layout,
            final_bind_group_layout,
            geometry_bind_group,
            blur_h_bind_group,
            blur_v_bind_group,
            final_bind_group,
            blur_h_uniform_buffer,
            blur_v_uniform_buffer,
            texture_size: (width.max(1), height.max(1)),
            blur_size,
            last_capture_at: None,
            last_geometry_key: None,
            stats: RenderStats::new(),
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.texture_size == (width, height) {
            return;
        }

        let (geometry_texture, geometry_view) = create_geometry_texture(device, width, height);
        let (backdrop_texture, backdrop_view) = create_backdrop_texture(device, width, height);
        let (blur_texture, blur_view) = create_blur_texture(device, width, height, "blur texture");
        let (blur_temp_texture, blur_temp_view) =
            create_blur_texture(device, width, height, "blur temp texture");
        upload_initial_backdrop(queue, &backdrop_texture, width, height);
        let blur_size = blur_extent(width, height);

        self.geometry_texture = geometry_texture;
        self.geometry_view = geometry_view;
        self.backdrop_texture = backdrop_texture;
        self.backdrop_view = backdrop_view;
        self.blur_texture = blur_texture;
        self.blur_view = blur_view;
        self.blur_temp_texture = blur_temp_texture;
        self.blur_temp_view = blur_temp_view;
        self.texture_size = (width, height);
        self.blur_size = blur_size;
        self.last_geometry_key = None;
        self.update_blur_uniforms(queue);
        self.blur_h_bind_group = create_blur_bind_group(
            device,
            &self.blur_bind_group_layout,
            &self.blur_h_uniform_buffer,
            &self.backdrop_view,
            &self.sampler,
        );
        self.blur_v_bind_group = create_blur_bind_group(
            device,
            &self.blur_bind_group_layout,
            &self.blur_v_uniform_buffer,
            &self.blur_temp_view,
            &self.sampler,
        );
        self.final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.uniform_buffer,
            &self.backdrop_view,
            &self.sampler,
            &self.geometry_view,
            &self.blur_view,
        );
    }

    pub fn rebuild_shapes(&mut self, device: &wgpu::Device, layout: &GridLayout, viewport_w: f32) {
        let shapes = shapes_from_layout(layout, viewport_w);
        self.shape_buffer = create_shape_buffer(device, &shapes);
        self.shape_count = shapes.len() as u32;
        self.geometry_bind_group = create_geometry_bind_group(
            device,
            &self.geometry_bind_group_layout,
            &self.uniform_buffer,
            &self.shape_buffer,
        );
        self.last_geometry_key = None;
    }

    pub fn notify_window_moved(&mut self) {
        self.capture.on_window_moved();
        self.last_capture_at = None;
    }

    pub fn handle_debug_key(&mut self, key: winit::keyboard::KeyCode) -> bool {
        use winit::keyboard::KeyCode;

        match key {
            KeyCode::KeyB => self.debug.show_backdrop_texture = !self.debug.show_backdrop_texture,
            KeyCode::KeyG => self.debug.show_geometry_texture = !self.debug.show_geometry_texture,
            KeyCode::KeyD => self.debug.show_displacement = !self.debug.show_displacement,
            KeyCode::KeyA => self.debug.show_alpha_mask = !self.debug.show_alpha_mask,
            KeyCode::KeyF => self.debug.show_final_glass_only = !self.debug.show_final_glass_only,
            KeyCode::KeyC => {
                self.debug.disable_chromatic_aberration = !self.debug.disable_chromatic_aberration
            }
            KeyCode::KeyE => self.debug.disable_edge_lighting = !self.debug.disable_edge_lighting,
            KeyCode::KeyL => self.debug.disable_blur = !self.debug.disable_blur,
            KeyCode::KeyV => self.params.enabled = !self.params.enabled,
            KeyCode::Digit1 => self.params.thickness = (self.params.thickness - 2.0).max(6.0),
            KeyCode::Digit2 => self.params.thickness = (self.params.thickness + 2.0).min(48.0),
            KeyCode::Digit3 => {
                self.params.refractive_index = (self.params.refractive_index - 0.02).max(1.02)
            }
            KeyCode::Digit4 => {
                self.params.refractive_index = (self.params.refractive_index + 0.02).min(1.75)
            }
            KeyCode::Digit5 => self.params.saturation = (self.params.saturation - 0.05).max(0.5),
            KeyCode::Digit6 => self.params.saturation = (self.params.saturation + 0.05).min(2.0),
            KeyCode::Digit7 => {
                self.params.chromatic_aberration =
                    (self.params.chromatic_aberration - 0.005).max(0.0)
            }
            KeyCode::Digit8 => {
                self.params.chromatic_aberration =
                    (self.params.chromatic_aberration + 0.005).min(0.18)
            }
            KeyCode::Digit9 => self.params.blur_radius = (self.params.blur_radius - 2.0).max(0.0),
            KeyCode::Digit0 => self.params.blur_radius = (self.params.blur_radius + 2.0).min(40.0),
            _ => return false,
        }

        eprintln!(
            "liquid glass params: enabled={} thickness={:.1} ri={:.2} chroma={:.3} blur={:.1} saturation={:.2} debug_flags={:#010b}",
            self.params.enabled,
            self.params.thickness,
            self.params.refractive_index,
            self.params.chromatic_aberration,
            self.params.blur_radius,
            self.params.saturation,
            self.debug.flags()
        );
        true
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        scroll_x: f32,
        defer_backdrop_capture: bool,
    ) {
        if !self.params.enabled || self.shape_count == 0 {
            return;
        }

        let render_started = Instant::now();
        let (width, height) = self.texture_size;
        let uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            width,
            height,
            scroll_x,
            self.shape_count,
        );
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let mut captured = false;
        let mut capture_time = Duration::ZERO;
        let mut upload_time = Duration::ZERO;
        if self.should_capture(defer_backdrop_capture) {
            let capture_started = Instant::now();
            if let Some(frame) = self.capture.latest_frame_rgba(width, height) {
                capture_time = capture_started.elapsed();
                let upload_started = Instant::now();
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.backdrop_texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &frame,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(width * 4),
                        rows_per_image: Some(height),
                    },
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                );
                upload_time = upload_started.elapsed();
                captured = true;
            } else {
                capture_time = capture_started.elapsed();
            }
            self.last_capture_at = Some(Instant::now());
        }
        let next_status = self.capture.status();
        if next_status != self.capture_status {
            log_capture_status(&next_status);
            self.capture_status = next_status;
        }

        self.update_blur_uniforms(queue);

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass blur horizontal pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.blur_temp_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.blur_downsample_pipeline);
            pass.set_bind_group(0, &self.blur_h_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass blur vertical pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.blur_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.blur_upsample_pipeline);
            pass.set_bind_group(0, &self.blur_v_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        let geometry_key = self.geometry_key(scroll_x);
        if self.last_geometry_key != Some(geometry_key) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass geometry pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.geometry_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.geometry_pipeline);
            pass.set_bind_group(0, &self.geometry_bind_group, &[]);
            pass.draw(0..3, 0..1);
            self.last_geometry_key = Some(geometry_key);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass final pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
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
            pass.set_pipeline(&self.final_pipeline);
            pass.set_bind_group(0, &self.final_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        let _ = device;
        self.stats.record(
            captured,
            capture_time,
            upload_time,
            render_started.elapsed(),
        );
    }

    fn should_capture(&self, defer_backdrop_capture: bool) -> bool {
        if defer_backdrop_capture {
            return self.last_capture_at.is_none();
        }
        true
    }

    fn geometry_key(&self, scroll_x: f32) -> GeometryKey {
        let (width, height) = self.texture_size;
        GeometryKey {
            scroll_x: (scroll_x * 10.0).round() / 10.0,
            thickness: self.params.thickness,
            refractive_index: self.params.refractive_index,
            blend: self.params.blend,
            width,
            height,
            shape_count: self.shape_count,
        }
    }

    fn update_blur_uniforms(&self, queue: &wgpu::Queue) {
        let (width, _) = self.texture_size;
        let (_, blur_h) = self.blur_size;
        let radius = if self.debug.disable_blur {
            0.0
        } else {
            self.params.blur_radius
        };
        queue.write_buffer(
            &self.blur_h_uniform_buffer,
            0,
            bytemuck::bytes_of(&blur_uniforms([1.0 / width.max(1) as f32, 0.0], radius)),
        );
        queue.write_buffer(
            &self.blur_v_uniform_buffer,
            0,
            bytemuck::bytes_of(&blur_uniforms([0.0, 1.0 / blur_h.max(1) as f32], radius)),
        );
    }
}

fn log_capture_status(status: &CaptureStatus) {
    match status {
        CaptureStatus::Ready => eprintln!("liquid glass capture: Windows.Graphics.Capture ready"),
        CaptureStatus::Fallback { reason } => eprintln!("liquid glass capture fallback: {reason}"),
    }
}

fn uniforms_from_params(
    params: &LiquidGlassParams,
    debug: DebugOptions,
    width: u32,
    height: u32,
    scroll_x: f32,
    shape_count: u32,
) -> GlassUniforms {
    GlassUniforms {
        viewport: [width as f32, height as f32],
        scroll_x,
        thickness: params.thickness,
        refractive_index: params.refractive_index,
        chromatic_aberration: if debug.disable_chromatic_aberration {
            0.0
        } else {
            params.chromatic_aberration
        },
        blur_radius: if debug.disable_blur {
            0.0
        } else {
            params.blur_radius
        },
        saturation: params.saturation,
        glass_color: params.glass_color,
        light_direction: params.light_direction,
        light_intensity: params.light_intensity,
        ambient_strength: params.ambient_strength,
        blend: params.blend,
        max_displacement: params.thickness * 10.0,
        shape_count,
        debug_flags: debug.flags(),
    }
}

fn blur_uniforms(texel_step: [f32; 2], radius: f32) -> BlurUniforms {
    BlurUniforms {
        texel_step,
        radius,
        _pad: 0.0,
    }
}

fn create_shape_buffer(device: &wgpu::Device, shapes: &[GlassShape]) -> wgpu::Buffer {
    let contents: Vec<GlassShape>;
    let slice = if shapes.is_empty() {
        contents = vec![GlassShape::rounded_rect([0.0, 0.0], [1.0, 1.0], 1.0)];
        contents.as_slice()
    } else {
        shapes
    };

    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("liquid glass shape buffer"),
        contents: bytemuck::cast_slice(slice),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

fn create_geometry_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("liquid glass geometry texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: GEOMETRY_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_backdrop_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("liquid glass backdrop texture"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: BACKDROP_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_blur_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    label: &'static str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let (width, height) = blur_extent(width, height);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: BLUR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn blur_extent(width: u32, height: u32) -> (u32, u32) {
    ((width / 2).max(1), (height / 2).max(1))
}

fn upload_initial_backdrop(queue: &wgpu::Queue, texture: &wgpu::Texture, width: u32, height: u32) {
    let pixels = vec![0u8; (width.max(1) * height.max(1) * 4) as usize];
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width.max(1) * 4),
            rows_per_image: Some(height.max(1)),
        },
        wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
    );
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(std::mem::size_of::<GlassUniforms>() as u64),
        },
        count: None,
    }
}

fn blur_uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(std::mem::size_of::<BlurUniforms>() as u64),
        },
        count: None,
    }
}

fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn create_geometry_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    shapes: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("liquid glass geometry bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: shapes.as_entire_binding(),
            },
        ],
    })
}

fn create_final_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    backdrop_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    geometry_view: &wgpu::TextureView,
    blur_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("liquid glass final bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(backdrop_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(geometry_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(blur_view),
            },
        ],
    })
}

fn create_blur_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    source_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("liquid glass blur bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn fullscreen_vertex_state(shader: &wgpu::ShaderModule) -> wgpu::VertexState<'_> {
    wgpu::VertexState {
        module: shader,
        entry_point: Some("vs_main"),
        compilation_options: Default::default(),
        buffers: &[],
    }
}

fn fullscreen_primitive_state() -> wgpu::PrimitiveState {
    wgpu::PrimitiveState {
        topology: wgpu::PrimitiveTopology::TriangleList,
        cull_mode: None,
        ..Default::default()
    }
}

fn premultiplied_blend() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
    }
}
