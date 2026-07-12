use std::time::{Duration, Instant};

mod frame;
mod resources;
use resources::*;

use super::capture::{BackdropCapture, CaptureStatus, GpuCaptureFrame};
use super::geometry::{shapes_from_layout, GlassShape};
use super::params::{DebugOptions, LiquidGlassParams};
use crate::layout::grid::GridLayout;

pub(super) const GEOMETRY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(super) const BACKDROP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub(super) const BLUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct GlassUniforms {
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
    time: f32,
    _pad: [f32; 3],
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
    // Base, badge, control, and settings passes are encoded into one frame.
    // Keep their uniforms separate because queued buffer writes are not a
    // per-render-pass state snapshot.
    uniform_buffer: wgpu::Buffer,
    badge_uniform_buffer: wgpu::Buffer,
    control_uniform_buffer: wgpu::Buffer,
    settings_panel_uniform_buffer: wgpu::Buffer,
    shape_buffer: wgpu::Buffer,
    shape_count: u32,
    shape_capacity: usize,
    badge_shape_buffer: wgpu::Buffer,
    badge_shape_count: u32,
    badge_shape_capacity: usize,
    badge_shapes: Vec<GlassShape>,
    control_shape_buffer: wgpu::Buffer,
    control_shape_count: u32,
    control_shape_capacity: usize,
    control_shapes: Vec<GlassShape>,
    settings_panel_shapes: Vec<GlassShape>,
    settings_panel_shape_count: u32,
    settings_panel_shape_capacity: usize,
    settings_panel_shape_buffer: wgpu::Buffer,
    settings_panel_geometry_bind_group: wgpu::BindGroup,
    /// The base shapes (frame + tile halos). The bottom control renders later
    /// so all of its states share the same overlay order.
    base_shapes: Vec<GlassShape>,
    geometry_texture: wgpu::Texture,
    geometry_view: wgpu::TextureView,
    backdrop_texture: wgpu::Texture,
    backdrop_view: wgpu::TextureView,
    gpu_backdrop_texture: Option<wgpu::Texture>,
    using_gpu_backdrop: bool,
    blur_texture: wgpu::Texture,
    blur_view: wgpu::TextureView,
    /// Pyramids L1=1/2, L2=1/4, L3=1/8 (src for downsample, dst for upsample).
    blur_levels: [(wgpu::Texture, wgpu::TextureView); 3],
    sampler: wgpu::Sampler,
    geometry_pipeline: wgpu::RenderPipeline,
    blur_downsample_pipeline: wgpu::RenderPipeline,
    blur_upsample_pipeline: wgpu::RenderPipeline,
    final_pipeline: wgpu::RenderPipeline,
    geometry_bind_group_layout: wgpu::BindGroupLayout,
    blur_bind_group_layout: wgpu::BindGroupLayout,
    final_bind_group_layout: wgpu::BindGroupLayout,
    geometry_bind_group: wgpu::BindGroup,
    /// Downsample bind groups: index i reads level i (level 0 == backdrop) and
    /// writes blur_levels[i]. Recreated when the backdrop source view changes.
    blur_down_bind_groups: [wgpu::BindGroup; 3],
    /// Upsample bind groups: index i reads blur_levels[i+1] and writes
    /// blur_levels[i] (or the full-res blur texture for i == 2).
    blur_up_bind_groups: [wgpu::BindGroup; 3],
    final_bind_group: wgpu::BindGroup,
    badge_final_bind_group: wgpu::BindGroup,
    control_final_bind_group: wgpu::BindGroup,
    settings_panel_final_bind_group: wgpu::BindGroup,
    badge_geometry_bind_group: wgpu::BindGroup,
    control_geometry_bind_group: wgpu::BindGroup,
    texture_size: (u32, u32),
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

        let uniforms = uniforms_from_params(&params, debug, width, height, 0.0, 0, 0.0);
        let uniform_buffer = create_uniform_buffer(device, "liquid glass uniforms", &uniforms);
        let badge_uniform_buffer =
            create_uniform_buffer(device, "liquid glass badge uniforms", &uniforms);
        let control_uniform_buffer =
            create_uniform_buffer(device, "liquid glass control uniforms", &uniforms);
        let settings_panel_uniform_buffer =
            create_uniform_buffer(device, "liquid glass settings panel uniforms", &uniforms);

        let shapes = shapes_from_layout(layout, width as f32, &[]);
        let shape_buffer = create_shape_buffer(device, &shapes);
        let shape_count = shapes.len() as u32;
        let shape_capacity = shapes.len().max(1);
        let badge_shape_capacity = 1;
        let badge_shape_buffer = create_shape_buffer_with_capacity(
            device,
            badge_shape_capacity,
            "liquid glass badge shape buffer",
        );
        let control_shape_buffer =
            create_shape_buffer_with_capacity(device, 2, "liquid glass control shape buffer");
        let settings_panel_shape_buffer =
            create_shape_buffer_with_capacity(device, 1, "liquid glass settings shape buffer");
        let badge_shape_count = 0;

        let (geometry_texture, geometry_view) = create_geometry_texture(device, width, height);
        let (backdrop_texture, backdrop_view) = create_backdrop_texture(device, width, height);
        // Final blur output is full-res: the final shader samples it without
        // any resolution-mismatch stretch.
        let (blur_texture, blur_view) =
            create_blur_texture_raw(device, width, height, 0, "blur texture");
        // L1=1/2, L2=1/4, L3=1/8 pyramid levels used by both down and up passes.
        let blur_levels = [
            create_blur_texture_raw(device, width, height, 1, "blur level 1"),
            create_blur_texture_raw(device, width, height, 2, "blur level 2"),
            create_blur_texture_raw(device, width, height, 3, "blur level 3"),
        ];
        upload_initial_backdrop(queue, &backdrop_texture, width, height);

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
                entries: &[texture_entry(0, true), sampler_entry(1)],
            });

        let geometry_bind_group = create_geometry_bind_group(
            device,
            &geometry_bind_group_layout,
            &uniform_buffer,
            &shape_buffer,
        );
        let badge_geometry_bind_group = create_geometry_bind_group(
            device,
            &geometry_bind_group_layout,
            &badge_uniform_buffer,
            &badge_shape_buffer,
        );
        let control_geometry_bind_group = create_geometry_bind_group(
            device,
            &geometry_bind_group_layout,
            &control_uniform_buffer,
            &control_shape_buffer,
        );
        let settings_panel_geometry_bind_group = create_geometry_bind_group(
            device,
            &geometry_bind_group_layout,
            &settings_panel_uniform_buffer,
            &settings_panel_shape_buffer,
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
        let badge_final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &badge_uniform_buffer,
            &backdrop_view,
            &sampler,
            &geometry_view,
            &blur_view,
        );
        let control_final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &control_uniform_buffer,
            &backdrop_view,
            &sampler,
            &geometry_view,
            &blur_view,
        );
        let settings_panel_final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &settings_panel_uniform_buffer,
            &backdrop_view,
            &sampler,
            &geometry_view,
            &blur_view,
        );
        let (blur_down_bind_groups, blur_up_bind_groups) = create_blur_pyramid_bind_groups(
            device,
            &blur_bind_group_layout,
            &backdrop_view,
            &blur_levels,
            &blur_view,
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
            badge_uniform_buffer,
            control_uniform_buffer,
            settings_panel_uniform_buffer,
            shape_buffer,
            shape_count,
            shape_capacity,
            badge_shape_buffer,
            badge_shape_count,
            badge_shape_capacity,
            badge_shapes: Vec::new(),
            control_shape_buffer,
            control_shape_count: 0,
            control_shape_capacity: 2,
            control_shapes: Vec::new(),
            settings_panel_shapes: Vec::new(),
            settings_panel_shape_count: 0,
            settings_panel_shape_capacity: 1,
            settings_panel_shape_buffer,
            settings_panel_geometry_bind_group,
            base_shapes: shapes,
            geometry_texture,
            geometry_view,
            backdrop_texture,
            backdrop_view,
            gpu_backdrop_texture: None,
            using_gpu_backdrop: false,
            blur_texture,
            blur_view,
            blur_levels,
            sampler,
            geometry_pipeline,
            blur_downsample_pipeline,
            blur_upsample_pipeline,
            final_pipeline,
            geometry_bind_group_layout,
            blur_bind_group_layout,
            final_bind_group_layout,
            geometry_bind_group,
            blur_down_bind_groups,
            blur_up_bind_groups,
            final_bind_group,
            badge_final_bind_group,
            control_final_bind_group,
            settings_panel_final_bind_group,
            badge_geometry_bind_group,
            control_geometry_bind_group,
            texture_size: (width.max(1), height.max(1)),
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
        let (blur_texture, blur_view) =
            create_blur_texture_raw(device, width, height, 0, "blur texture");
        let blur_levels = [
            create_blur_texture_raw(device, width, height, 1, "blur level 1"),
            create_blur_texture_raw(device, width, height, 2, "blur level 2"),
            create_blur_texture_raw(device, width, height, 3, "blur level 3"),
        ];
        upload_initial_backdrop(queue, &backdrop_texture, width, height);

        self.geometry_texture = geometry_texture;
        self.geometry_view = geometry_view;
        self.backdrop_texture = backdrop_texture;
        self.backdrop_view = backdrop_view;
        self.gpu_backdrop_texture = None;
        self.using_gpu_backdrop = false;
        self.blur_texture = blur_texture;
        self.blur_view = blur_view;
        self.blur_levels = blur_levels;
        self.texture_size = (width, height);
        self.last_geometry_key = None;
        let (down, up) = create_blur_pyramid_bind_groups(
            device,
            &self.blur_bind_group_layout,
            &self.backdrop_view,
            &self.blur_levels,
            &self.blur_view,
            &self.sampler,
        );
        self.blur_down_bind_groups = down;
        self.blur_up_bind_groups = up;
        let backdrop_view = self.backdrop_view.clone();
        self.rebuild_final_bind_groups(device, &backdrop_view);
    }

    fn bind_backdrop_view(&mut self, device: &wgpu::Device, view: &wgpu::TextureView) {
        // The downsample[0] source is the backdrop; rebuild the down/up
        // pyramid bind groups for this view, plus the final bind group.
        let (down, up) = create_blur_pyramid_bind_groups(
            device,
            &self.blur_bind_group_layout,
            view,
            &self.blur_levels,
            &self.blur_view,
            &self.sampler,
        );
        self.blur_down_bind_groups = down;
        self.blur_up_bind_groups = up;
        self.rebuild_final_bind_groups(device, view);
    }

    fn rebuild_final_bind_groups(
        &mut self,
        device: &wgpu::Device,
        backdrop_view: &wgpu::TextureView,
    ) {
        self.final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.uniform_buffer,
            backdrop_view,
            &self.sampler,
            &self.geometry_view,
            &self.blur_view,
        );
        self.badge_final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.badge_uniform_buffer,
            backdrop_view,
            &self.sampler,
            &self.geometry_view,
            &self.blur_view,
        );
        self.control_final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.control_uniform_buffer,
            backdrop_view,
            &self.sampler,
            &self.geometry_view,
            &self.blur_view,
        );
        self.settings_panel_final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.settings_panel_uniform_buffer,
            backdrop_view,
            &self.sampler,
            &self.geometry_view,
            &self.blur_view,
        );
    }

    fn bind_cpu_backdrop(&mut self, device: &wgpu::Device) {
        // Clone the view so the immutable borrow ends before we mutate `self`.
        let view = self.backdrop_view.clone();
        self.bind_backdrop_view(device, &view);
    }

    pub fn set_base_shapes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shapes: &[GlassShape],
    ) {
        if self.base_shapes.as_slice() == shapes {
            return;
        }
        self.base_shapes.clear();
        self.base_shapes.extend_from_slice(shapes);
        self.shape_count = self.base_shapes.len() as u32;
        if self.base_shapes.len() > self.shape_capacity {
            self.shape_capacity = next_shape_capacity(self.shape_capacity, self.base_shapes.len());
            self.shape_buffer = create_shape_buffer_with_capacity(
                device,
                self.shape_capacity,
                "liquid glass base shape buffer",
            );
            self.geometry_bind_group = create_geometry_bind_group(
                device,
                &self.geometry_bind_group_layout,
                &self.uniform_buffer,
                &self.shape_buffer,
            );
        }
        if !self.base_shapes.is_empty() {
            queue.write_buffer(
                &self.shape_buffer,
                0,
                bytemuck::cast_slice(&self.base_shapes),
            );
        }
        self.last_geometry_key = None;
    }

    /// Replace the fixed overlay lane atomically. The bottom control and gear
    /// share one SDF field, so updating them together avoids rebuilding or
    /// uploading the lane twice when both shapes change in the same frame.
    pub fn set_overlay_shapes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shapes: &[GlassShape],
    ) {
        if self.control_shapes.as_slice() == shapes {
            return;
        }
        self.control_shapes.clear();
        self.control_shapes.extend_from_slice(shapes);
        self.control_shape_count = self.control_shapes.len() as u32;
        if self.control_shapes.len() > self.control_shape_capacity {
            self.control_shape_capacity =
                next_shape_capacity(self.control_shape_capacity, self.control_shapes.len());
            self.control_shape_buffer = create_shape_buffer_with_capacity(
                device,
                self.control_shape_capacity,
                "liquid glass control shape buffer",
            );
            self.control_geometry_bind_group = create_geometry_bind_group(
                device,
                &self.geometry_bind_group_layout,
                &self.control_uniform_buffer,
                &self.control_shape_buffer,
            );
        }
        if !self.control_shapes.is_empty() {
            queue.write_buffer(
                &self.control_shape_buffer,
                0,
                bytemuck::cast_slice(&self.control_shapes),
            );
        }
    }

    /// Replace the modal glass lane atomically.
    pub fn set_modal_shapes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shapes: &[GlassShape],
    ) {
        if self.settings_panel_shapes.as_slice() == shapes {
            return;
        }
        self.settings_panel_shapes.clear();
        self.settings_panel_shapes.extend_from_slice(shapes);
        self.settings_panel_shape_count = self.settings_panel_shapes.len() as u32;
        if self.settings_panel_shapes.len() > self.settings_panel_shape_capacity {
            self.settings_panel_shape_capacity = next_shape_capacity(
                self.settings_panel_shape_capacity,
                self.settings_panel_shapes.len(),
            );
            self.settings_panel_shape_buffer = create_shape_buffer_with_capacity(
                device,
                self.settings_panel_shape_capacity,
                "liquid glass modal shape buffer",
            );
            self.settings_panel_geometry_bind_group = create_geometry_bind_group(
                device,
                &self.geometry_bind_group_layout,
                &self.settings_panel_uniform_buffer,
                &self.settings_panel_shape_buffer,
            );
        }
        if !self.settings_panel_shapes.is_empty() {
            queue.write_buffer(
                &self.settings_panel_shape_buffer,
                0,
                bytemuck::cast_slice(&self.settings_panel_shapes),
            );
        }
    }

    /// Replace the edit-mode delete-badge glass shapes. These are rendered as
    /// a separate Liquid Glass overlay after the app tiles/icons, so the badge
    /// actually refracts through the Liquid Glass shader instead of being a
    /// plain painted circle in the tile shader.
    pub fn set_badge_shapes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shapes: &[GlassShape],
    ) {
        if self.badge_shapes.as_slice() == shapes {
            return;
        }
        self.badge_shapes.clear();
        self.badge_shapes.extend_from_slice(shapes);
        self.badge_shape_count = self.badge_shapes.len() as u32;
        if self.badge_shapes.len() > self.badge_shape_capacity {
            self.badge_shape_capacity =
                next_shape_capacity(self.badge_shape_capacity, self.badge_shapes.len());
            self.badge_shape_buffer = create_shape_buffer_with_capacity(
                device,
                self.badge_shape_capacity,
                "liquid glass badge shape buffer",
            );
            self.badge_geometry_bind_group = create_geometry_bind_group(
                device,
                &self.geometry_bind_group_layout,
                &self.badge_uniform_buffer,
                &self.badge_shape_buffer,
            );
        }
        if !self.badge_shapes.is_empty() {
            queue.write_buffer(
                &self.badge_shape_buffer,
                0,
                bytemuck::cast_slice(&self.badge_shapes),
            );
        }
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

    /// How many pyramid levels to run this frame.
    ///
    /// Maps blur_radius to pyramid depth so weak blurs stay cheap and large
    /// radii stay smooth. Returns 0 when blur is disabled (final shader then
    /// bypasses the blur texture via its `blur_radius < 0.5` check).
    fn blur_level_count(&self) -> usize {
        if self.debug.disable_blur {
            return 0;
        }
        let radius = self.params.blur_radius;
        if radius < 6.0 {
            1
        } else if radius < 16.0 {
            2
        } else {
            3
        }
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
    time: f32,
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
        time,
        _pad: [0.0; 3],
    }
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

#[cfg(test)]
mod shape_capacity_tests {
    use super::{next_shape_capacity, GlassUniforms};

    #[test]
    fn shape_capacity_grows_only_past_current_capacity() {
        assert_eq!(next_shape_capacity(1, 2), 2);
        assert_eq!(next_shape_capacity(8, 9), 16);
        assert_eq!(next_shape_capacity(8, 20), 20);
    }

    #[test]
    fn glass_uniform_layout_matches_wgsl() {
        assert_eq!(std::mem::size_of::<GlassUniforms>(), 96);
        assert_eq!(std::mem::align_of::<GlassUniforms>(), 4);
    }
}
