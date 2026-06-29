use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use wgpu::util::DeviceExt;

use super::capture::{BackdropCapture, CaptureStatus, GpuCaptureFrame};
use super::geometry::{shapes_from_layout, with_control, GlassShape};
use super::params::{DebugOptions, LiquidGlassParams};
use crate::grid::{GridApp, GridLayout};

/// Entrance (appear) reveal parameters threaded through to the glass shaders:
/// a composited opacity, a uniform scale about the frame center, and that
/// center pivot itself. Bundled so the render path doesn't grow past clippy's
/// argument limit.
#[derive(Debug, Clone, Copy, Default)]
pub struct EntranceReveal {
    /// Composited opacity (0..1), 1.0 = fully shown.
    pub alpha: f32,
    /// Uniform scale about `center` (0.92..1.0), 1.0 = at rest.
    pub scale: f32,
    /// Page-frame center (physical px), the pivot for `scale`.
    pub center: [f32; 2],
}

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
    /// Entrance reveal: composited opacity (0..1) applied to the glass alpha.
    appear_alpha: f32,
    /// Entrance reveal: uniform scale about the frame center (0.92..1.0).
    appear_scale: f32,
    /// Page-frame center (physical px), the pivot for `appear_scale`.
    frame_center: [f32; 2],
    shape_count: u32,
    debug_flags: u32,
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
    /// The base shapes (frame + tile halos) without the bottom control. Kept
    /// so the shape buffer can be rebuilt when only the control changes.
    base_shapes: Vec<GlassShape>,
    /// The optional bottom-control capsule appended after the base shapes.
    /// `None` = no control; `Some(s)` = appended as the last shape.
    control_shape: Option<GlassShape>,
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

        let uniforms = uniforms_from_params(
            &params,
            debug,
            width,
            height,
            0.0,
            0,
            EntranceReveal {
                alpha: 1.0,
                scale: 1.0,
                center: [width as f32 * 0.5, height as f32 * 0.5],
            },
        );
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("liquid glass uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let shapes = shapes_from_layout(layout, width as f32, &[]);
        let shape_buffer = create_shape_buffer(device, &shapes);
        let shape_count = shapes.len() as u32;

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
        let final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &uniform_buffer,
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
            shape_buffer,
            shape_count,
            base_shapes: Vec::new(),
            control_shape: None,
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
        self.final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.uniform_buffer,
            view,
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

    pub fn rebuild_shapes(
        &mut self,
        device: &wgpu::Device,
        layout: &GridLayout,
        viewport_w: f32,
        apps: &[GridApp<'_>],
    ) {
        self.base_shapes = shapes_from_layout(layout, viewport_w, apps);
        self.rebuild_shape_buffer(device);
    }

    /// Replace just the bottom-control capsule shape (the last shape in the
    /// buffer). Cheaper than a full `rebuild_shapes` — only re-uploads the
    /// shape storage buffer. Pass `None` to hide the control entirely.
    pub fn set_control_shape(&mut self, device: &wgpu::Device, shape: Option<GlassShape>) {
        if self.control_shape == shape {
            return;
        }
        self.control_shape = shape;
        self.rebuild_shape_buffer(device);
    }

    /// Rebuild the GPU shape buffer from `base_shapes` + the optional control.
    fn rebuild_shape_buffer(&mut self, device: &wgpu::Device) {
        let shapes = with_control(self.base_shapes.clone(), self.control_shape);
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

    /// Render the liquid-glass composite. The GPU context (device/queue/encoder/
    /// target) is passed by reference each frame rather than stored, so the
    /// argument count is inherent to the multi-pass render entry point.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        scroll_x: f32,
        defer_backdrop_capture: bool,
        reveal: EntranceReveal,
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
            reveal,
        );
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let mut captured = false;
        let mut capture_time = Duration::ZERO;
        let mut upload_time = Duration::ZERO;
        if self.should_capture(defer_backdrop_capture) {
            let capture_started = Instant::now();
            if let Some(gpu_frame) = self.capture.latest_frame_texture(device, width, height) {
                capture_time = capture_started.elapsed();
                if let GpuCaptureFrame::New { texture, view } = gpu_frame {
                    if !self.using_gpu_backdrop {
                        eprintln!("liquid glass capture path: GPU shared texture");
                    }
                    self.bind_backdrop_view(device, &view);
                    self.gpu_backdrop_texture = Some(texture);
                    self.using_gpu_backdrop = true;
                }
                captured = true;
            } else if let Some(frame) = self.capture.latest_frame_rgba(width, height) {
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
                if self.using_gpu_backdrop {
                    eprintln!("liquid glass capture path: CPU texture upload fallback");
                    self.bind_cpu_backdrop(device);
                    self.gpu_backdrop_texture = None;
                    self.using_gpu_backdrop = false;
                }
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

        let blur_levels = self.blur_level_count();

        // Each blur pass runs in its OWN command encoder. wgpu groups all
        // passes in a single encoder into one "usage scope", and a texture
        // may not be both RESOURCE and COLOR_TARGET within that scope. Since a
        // dual-Kawase pyramid feeds each pass's output into the next pass's
        // input (L2 is written by down then read by up), we must split scopes
        // by submitting one encoder per pass.
        let _ = encoder; // the caller's encoder is used only for geometry/final.

        // Downsample: backdrop -> L1 -> ... -> L(k-1). down[i] reads the
        // backdrop for i==0 else levels[i-1], and writes levels[i].
        for i in 0..blur_levels {
            let dst = &self.blur_levels[i].1;
            let label = format!("liquid glass blur downsample L{i}->L{}", i + 1);
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some(label.as_str()),
            });
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label.as_str()),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: dst,
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
                pass.set_bind_group(0, &self.blur_down_bind_groups[i], &[]);
                pass.draw(0..3, 0..1);
            }
            queue.submit(std::iter::once(enc.finish()));
        }

        // Upsample: L(k-1) -> L(k-2) -> ... -> L1 -> full-res blur.
        // up pass j reads levels[k-1-j] (bind index 3-k+j in the fixed
        // [L3,L2,L1] bind array) and writes levels[k-2-j], or the full-res
        // blur texture for the final hop (j == k-1).
        for j in 0..blur_levels {
            let dst = if j == blur_levels - 1 {
                &self.blur_view
            } else {
                &self.blur_levels[blur_levels - 2 - j].1
            };
            let bind_idx = 3 - blur_levels + j;
            let label = format!(
                "liquid glass blur upsample L{}->L{}",
                blur_levels - j,
                blur_levels - 1 - j
            );
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some(label.as_str()),
            });
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label.as_str()),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: dst,
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
                pass.set_bind_group(0, &self.blur_up_bind_groups[bind_idx], &[]);
                pass.draw(0..3, 0..1);
            }
            queue.submit(std::iter::once(enc.finish()));
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
    reveal: EntranceReveal,
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
        appear_alpha: reveal.alpha,
        appear_scale: reveal.scale,
        frame_center: reveal.center,
        shape_count,
        debug_flags: debug.flags(),
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

/// Create a blur texture for a pyramid level. `level` selects the size:
/// 0 = full-res (final output), 1 = 1/2, 2 = 1/4, 3 = 1/8.
fn create_blur_texture_raw(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    level: u32,
    label: &'static str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let (lw, lh) = blur_level_extent(width, height, level);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: lw,
            height: lh,
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

/// Pyramid size for a given level (0 = full-res, 1 = 1/2, 2 = 1/4, 3 = 1/8).
fn blur_level_extent(width: u32, height: u32, level: u32) -> (u32, u32) {
    let mut w = width.max(1);
    let mut h = height.max(1);
    for _ in 0..level.min(3) {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    (w, h)
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

/// Build a single blur bind group: [0]=source texture, [1]=sampler.
fn create_blur_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("liquid glass blur bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// Build all six pyramid bind groups for one frame's worth of blur.
///
/// - `down[i]` reads source `i` (backdrop for i==0, else `levels[i-1]`) and
///   writes `levels[i]`.
/// - `up[i]` reads `levels[2-i]` and writes `levels[1-i]` (or the full-res
///   `blur_view` for i==2).
fn create_blur_pyramid_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    backdrop_view: &wgpu::TextureView,
    levels: &[(wgpu::Texture, wgpu::TextureView); 3],
    blur_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> ([wgpu::BindGroup; 3], [wgpu::BindGroup; 3]) {
    // Down sources: backdrop, L1, L2.
    let down = [
        create_blur_bind_group(device, layout, backdrop_view, sampler),
        create_blur_bind_group(device, layout, &levels[0].1, sampler),
        create_blur_bind_group(device, layout, &levels[1].1, sampler),
    ];
    // Up sources: L3, L2, L1 (reverse). Each writes the next level up; the
    // last hop writes the full-res blur texture.
    let up = [
        create_blur_bind_group(device, layout, &levels[2].1, sampler),
        create_blur_bind_group(device, layout, &levels[1].1, sampler),
        create_blur_bind_group(device, layout, &levels[0].1, sampler),
    ];
    let _ = blur_view;
    (down, up)
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
