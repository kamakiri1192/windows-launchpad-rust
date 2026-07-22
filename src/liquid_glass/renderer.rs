use std::time::{Duration, Instant};

mod frame;
mod resources;
use resources::*;

use super::capture::{
    BackdropCapture, CaptureRegion, CaptureStatus, CpuCaptureFrame, EphemeralGpuCaptureFrame,
    GpuCaptureFrame,
};
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
    backdrop_origin: [f32; 2],
    backdrop_extent: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BackdropMapping {
    region: CaptureRegion,
    texture_size: (u32, u32),
}

impl BackdropMapping {
    fn full(width: u32, height: u32) -> Self {
        Self {
            region: CaptureRegion::full(width, height),
            texture_size: (width.max(1), height.max(1)),
        }
    }
}

#[derive(Debug)]
struct RenderStats {
    last_report_at: Instant,
    frames: u32,
    captured_frames: u32,
    blurred_frames: u32,
    geometry_frames: u32,
    geometry_shapes: u64,
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
            blurred_frames: 0,
            geometry_frames: 0,
            geometry_shapes: 0,
            capture_time: Duration::ZERO,
            upload_time: Duration::ZERO,
            render_time: Duration::ZERO,
        }
    }

    fn record(
        &mut self,
        captured: bool,
        blurred: bool,
        geometry_shape_count: Option<u32>,
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
        if blurred {
            self.blurred_frames += 1;
        }
        if let Some(geometry_shape_count) = geometry_shape_count {
            self.geometry_frames += 1;
            self.geometry_shapes += u64::from(geometry_shape_count);
        }
        self.render_time += render_time;

        let elapsed = self.last_report_at.elapsed();
        if elapsed < Duration::from_secs(2) {
            return;
        }

        let seconds = elapsed.as_secs_f32().max(0.001);
        let capture_fps = self.captured_frames as f32 / seconds;
        let blur_fps = self.blurred_frames as f32 / seconds;
        let geometry_fps = self.geometry_frames as f32 / seconds;
        let blur_reuse = if self.frames == 0 {
            0.0
        } else {
            100.0 * (1.0 - self.blurred_frames as f32 / self.frames as f32)
        };
        let avg_capture_ms = avg_ms(self.capture_time, self.captured_frames);
        let avg_upload_ms = avg_ms(self.upload_time, self.captured_frames);
        let avg_render_ms = avg_ms(self.render_time, self.frames);
        let geometry_reuse = if self.frames == 0 {
            0.0
        } else {
            100.0 * (1.0 - self.geometry_frames as f32 / self.frames as f32)
        };
        let avg_geometry_shapes = if self.geometry_frames == 0 {
            0.0
        } else {
            self.geometry_shapes as f32 / self.geometry_frames as f32
        };
        eprintln!(
            "liquid glass stats: capture_fps={capture_fps:.1} capture_ms={avg_capture_ms:.2} upload_ms={avg_upload_ms:.2} blur_fps={blur_fps:.1} blur_reuse={blur_reuse:.0}% geometry_fps={geometry_fps:.1} geometry_reuse={geometry_reuse:.0}% geometry_shapes={avg_geometry_shapes:.1} render_ms={avg_render_ms:.2}"
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
    // Base, grid-overlay, drag-overlay, badge, control, and settings passes are
    // encoded into one frame.
    // Keep their uniforms separate because queued buffer writes are not a
    // per-render-pass state snapshot.
    uniform_buffer: wgpu::Buffer,
    grid_overlay_uniform_buffer: wgpu::Buffer,
    drag_overlay_uniform_buffer: wgpu::Buffer,
    badge_uniform_buffer: wgpu::Buffer,
    modal_badge_uniform_buffer: wgpu::Buffer,
    control_uniform_buffer: wgpu::Buffer,
    settings_panel_uniform_buffer: wgpu::Buffer,
    shape_buffer: wgpu::Buffer,
    shape_count: u32,
    shape_capacity: usize,
    grid_overlay_shape_buffer: wgpu::Buffer,
    grid_overlay_shape_count: u32,
    grid_overlay_shape_capacity: usize,
    grid_overlay_shapes: Vec<GlassShape>,
    drag_overlay_shape_buffer: wgpu::Buffer,
    drag_overlay_shape_count: u32,
    drag_overlay_shape_capacity: usize,
    drag_overlay_shapes: Vec<GlassShape>,
    badge_shape_buffer: wgpu::Buffer,
    badge_shape_count: u32,
    badge_shape_capacity: usize,
    badge_shapes: Vec<GlassShape>,
    modal_badge_shape_buffer: wgpu::Buffer,
    modal_badge_shape_count: u32,
    modal_badge_shape_capacity: usize,
    modal_badge_shapes: Vec<GlassShape>,
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
    /// Base shapes that can affect the fixed page frame at the current scroll
    /// position. Off-frame pages are culled on the CPU before the full-screen
    /// SDF shader sees them.
    active_base_shapes: Vec<GlassShape>,
    base_shape_scratch: Vec<GlassShape>,
    geometry_texture: wgpu::Texture,
    geometry_view: wgpu::TextureView,
    overlay_geometry_texture: wgpu::Texture,
    overlay_geometry_view: wgpu::TextureView,
    backdrop_texture: wgpu::Texture,
    backdrop_view: wgpu::TextureView,
    backdrop_mapping: BackdropMapping,
    gpu_backdrop_texture: Option<wgpu::Texture>,
    using_gpu_backdrop: bool,
    gpu_backdrop_is_copy_target: bool,
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
    grid_overlay_final_bind_group: wgpu::BindGroup,
    drag_overlay_final_bind_group: wgpu::BindGroup,
    badge_final_bind_group: wgpu::BindGroup,
    modal_badge_final_bind_group: wgpu::BindGroup,
    control_final_bind_group: wgpu::BindGroup,
    settings_panel_final_bind_group: wgpu::BindGroup,
    grid_overlay_geometry_bind_group: wgpu::BindGroup,
    drag_overlay_geometry_bind_group: wgpu::BindGroup,
    badge_geometry_bind_group: wgpu::BindGroup,
    modal_badge_geometry_bind_group: wgpu::BindGroup,
    control_geometry_bind_group: wgpu::BindGroup,
    texture_size: (u32, u32),
    last_capture_at: Option<Instant>,
    /// The blur pyramid depends on the captured backdrop, not on foreground
    /// animation. Rebuild it only when that backdrop (or blur parameters)
    /// changes instead of re-blurring identical pixels every render frame.
    blur_dirty: bool,
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

fn intersect_bounds(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let intersection = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (intersection[0] < intersection[2] && intersection[1] < intersection[3]).then_some(intersection)
}

fn base_shape_may_affect_frame(
    shape: GlassShape,
    scroll_x: f32,
    frame: GlassShape,
    smooth_union_radius: f32,
) -> bool {
    if !shape.is_scrolling() {
        return true;
    }
    let bounds = shape.screen_bounds(scroll_x);
    let margin = smooth_union_radius.max(0.0);
    let influence_bounds = [
        bounds[0] - margin,
        bounds[1] - margin,
        bounds[2] + margin,
        bounds[3] + margin,
    ];
    if intersect_bounds(influence_bounds, frame.screen_bounds(0.0)).is_none() {
        return false;
    }

    // A scrolling rounded rect that remains at least `blend` inside the fixed
    // frame cannot change the smooth union: the frame's signed distance is
    // already smaller everywhere. Testing the expanded AABB is conservative
    // around rounded corners and guarantees the whole blend neighborhood is
    // contained before the shape is discarded.
    !bounds_inside_rounded_rect(influence_bounds, frame)
}

fn bounds_inside_rounded_rect(bounds: [f32; 4], shape: GlassShape) -> bool {
    [
        [bounds[0], bounds[1]],
        [bounds[2], bounds[1]],
        [bounds[0], bounds[3]],
        [bounds[2], bounds[3]],
    ]
    .into_iter()
    .all(|point| rounded_rect_sdf(point, shape) <= 0.0)
}

fn rounded_rect_sdf(point: [f32; 2], shape: GlassShape) -> f32 {
    let half = [shape.size[0] * 0.5, shape.size[1] * 0.5];
    let radius = shape.radius.min(half[0]).min(half[1]);
    let q = [
        (point[0] - shape.center[0]).abs() - half[0] + radius,
        (point[1] - shape.center[1]).abs() - half[1] + radius,
    ];
    q[0].max(q[1]).min(0.0) + q[0].max(0.0).hypot(q[1].max(0.0)) - radius
}

fn capture_region_for_shapes(
    width: u32,
    height: u32,
    scroll_x: f32,
    padding: f32,
    groups: &[&[GlassShape]],
) -> CaptureRegion {
    let viewport = [0.0, 0.0, width.max(1) as f32, height.max(1) as f32];
    let frame_bounds = groups
        .iter()
        .flat_map(|group| group.iter())
        .find(|shape| shape.is_frame())
        .map(|shape| shape.screen_bounds(0.0));

    let mut union: Option<[f32; 4]> = None;
    for shape in groups.iter().flat_map(|group| group.iter()) {
        if shape.is_clip_only() {
            continue;
        }
        let mut bounds = shape.screen_bounds(scroll_x);
        if shape.is_scrolling() {
            let Some(frame) = frame_bounds else {
                continue;
            };
            let Some(clipped) = intersect_bounds(bounds, frame) else {
                continue;
            };
            bounds = clipped;
        }
        let Some(bounds) = intersect_bounds(bounds, viewport) else {
            continue;
        };
        union = Some(match union {
            Some(current) => [
                current[0].min(bounds[0]),
                current[1].min(bounds[1]),
                current[2].max(bounds[2]),
                current[3].max(bounds[3]),
            ],
            None => bounds,
        });
    }

    let Some(bounds) = union else {
        return CaptureRegion::full(width, height);
    };
    let align_down = |value: f32| ((value.max(0.0).floor() as u32) / 2) * 2;
    let align_up = |value: f32, limit: u32| {
        let rounded = value.ceil().max(1.0).min(limit as f32) as u32;
        rounded.saturating_add(1) / 2 * 2
    };
    let x = align_down(bounds[0] - padding);
    let y = align_down(bounds[1] - padding);
    let right = align_up(bounds[2] + padding, width.max(1)).min(width.max(1));
    let bottom = align_up(bounds[3] + padding, height.max(1)).min(height.max(1));
    CaptureRegion {
        x,
        y,
        width: right.saturating_sub(x).max(1),
        height: bottom.saturating_sub(y).max(1),
    }
    .clamped_to(width, height)
}

fn capture_sampling_padding(
    params: &LiquidGlassParams,
    debug: DebugOptions,
    width: u32,
    height: u32,
) -> f32 {
    let max_displacement = params.thickness * 10.0;
    let chromatic_scale = if debug.disable_chromatic_aberration {
        1.0
    } else {
        1.0 + params.chromatic_aberration * 2.15
    };
    let refraction = max_displacement * chromatic_scale + 3.0;
    let reflection = max_displacement * 0.42 + width.max(height) as f32 * 0.035;
    let blur_support = if debug.disable_blur || params.blur_radius < 0.5 {
        1.0
    } else {
        40.0
    };
    refraction.max(reflection) + blur_support + params.blend * 0.25 + 4.0
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

        let backdrop_mapping = BackdropMapping::full(width, height);
        let uniforms = uniforms_from_params(
            &params,
            debug,
            (width, height),
            0.0,
            0,
            0.0,
            backdrop_mapping,
        );
        let uniform_buffer = create_uniform_buffer(device, "liquid glass uniforms", &uniforms);
        let grid_overlay_uniform_buffer =
            create_uniform_buffer(device, "liquid glass grid overlay uniforms", &uniforms);
        let drag_overlay_uniform_buffer =
            create_uniform_buffer(device, "liquid glass drag overlay uniforms", &uniforms);
        let badge_uniform_buffer =
            create_uniform_buffer(device, "liquid glass badge uniforms", &uniforms);
        let modal_badge_uniform_buffer =
            create_uniform_buffer(device, "liquid glass modal badge uniforms", &uniforms);
        let control_uniform_buffer =
            create_uniform_buffer(device, "liquid glass control uniforms", &uniforms);
        let settings_panel_uniform_buffer =
            create_uniform_buffer(device, "liquid glass settings panel uniforms", &uniforms);

        let shapes = shapes_from_layout(layout, width as f32, &[]);
        let active_base_shapes = shapes.clone();
        let base_shape_scratch = Vec::with_capacity(shapes.len());
        let shape_buffer = create_shape_buffer(device, &shapes);
        let shape_count = shapes.len() as u32;
        let shape_capacity = shapes.len().max(1);
        let grid_overlay_shape_capacity = 1;
        let grid_overlay_shape_buffer = create_shape_buffer_with_capacity(
            device,
            grid_overlay_shape_capacity,
            "liquid glass grid overlay shape buffer",
        );
        let drag_overlay_shape_capacity = 1;
        let drag_overlay_shape_buffer = create_shape_buffer_with_capacity(
            device,
            drag_overlay_shape_capacity,
            "liquid glass drag overlay shape buffer",
        );
        let badge_shape_capacity = 1;
        let badge_shape_buffer = create_shape_buffer_with_capacity(
            device,
            badge_shape_capacity,
            "liquid glass badge shape buffer",
        );
        let modal_badge_shape_capacity = 1;
        let modal_badge_shape_buffer = create_shape_buffer_with_capacity(
            device,
            modal_badge_shape_capacity,
            "liquid glass modal badge shape buffer",
        );
        let control_shape_buffer =
            create_shape_buffer_with_capacity(device, 2, "liquid glass control shape buffer");
        let settings_panel_shape_buffer =
            create_shape_buffer_with_capacity(device, 1, "liquid glass settings shape buffer");
        let badge_shape_count = 0;

        let (geometry_texture, geometry_view) = create_geometry_texture(device, width, height);
        let (overlay_geometry_texture, overlay_geometry_view) =
            create_overlay_geometry_texture(device, width, height);
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
        let grid_overlay_geometry_bind_group = create_geometry_bind_group(
            device,
            &geometry_bind_group_layout,
            &grid_overlay_uniform_buffer,
            &grid_overlay_shape_buffer,
        );
        let drag_overlay_geometry_bind_group = create_geometry_bind_group(
            device,
            &geometry_bind_group_layout,
            &drag_overlay_uniform_buffer,
            &drag_overlay_shape_buffer,
        );
        let badge_geometry_bind_group = create_geometry_bind_group(
            device,
            &geometry_bind_group_layout,
            &badge_uniform_buffer,
            &badge_shape_buffer,
        );
        let modal_badge_geometry_bind_group = create_geometry_bind_group(
            device,
            &geometry_bind_group_layout,
            &modal_badge_uniform_buffer,
            &modal_badge_shape_buffer,
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
        let grid_overlay_final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &grid_overlay_uniform_buffer,
            &backdrop_view,
            &sampler,
            &overlay_geometry_view,
            &blur_view,
        );
        let drag_overlay_final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &drag_overlay_uniform_buffer,
            &backdrop_view,
            &sampler,
            &overlay_geometry_view,
            &blur_view,
        );
        let badge_final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &badge_uniform_buffer,
            &backdrop_view,
            &sampler,
            &overlay_geometry_view,
            &blur_view,
        );
        let modal_badge_final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &modal_badge_uniform_buffer,
            &backdrop_view,
            &sampler,
            &overlay_geometry_view,
            &blur_view,
        );
        let control_final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &control_uniform_buffer,
            &backdrop_view,
            &sampler,
            &overlay_geometry_view,
            &blur_view,
        );
        let settings_panel_final_bind_group = create_final_bind_group(
            device,
            &final_bind_group_layout,
            &settings_panel_uniform_buffer,
            &backdrop_view,
            &sampler,
            &overlay_geometry_view,
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
            grid_overlay_uniform_buffer,
            drag_overlay_uniform_buffer,
            badge_uniform_buffer,
            modal_badge_uniform_buffer,
            control_uniform_buffer,
            settings_panel_uniform_buffer,
            shape_buffer,
            shape_count,
            shape_capacity,
            grid_overlay_shape_buffer,
            grid_overlay_shape_count: 0,
            grid_overlay_shape_capacity,
            grid_overlay_shapes: Vec::new(),
            drag_overlay_shape_buffer,
            drag_overlay_shape_count: 0,
            drag_overlay_shape_capacity,
            drag_overlay_shapes: Vec::new(),
            badge_shape_buffer,
            badge_shape_count,
            badge_shape_capacity,
            badge_shapes: Vec::new(),
            modal_badge_shape_buffer,
            modal_badge_shape_count: 0,
            modal_badge_shape_capacity,
            modal_badge_shapes: Vec::new(),
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
            active_base_shapes,
            base_shape_scratch,
            geometry_texture,
            geometry_view,
            overlay_geometry_texture,
            overlay_geometry_view,
            backdrop_texture,
            backdrop_view,
            backdrop_mapping,
            gpu_backdrop_texture: None,
            using_gpu_backdrop: false,
            gpu_backdrop_is_copy_target: false,
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
            grid_overlay_final_bind_group,
            drag_overlay_final_bind_group,
            badge_final_bind_group,
            modal_badge_final_bind_group,
            control_final_bind_group,
            settings_panel_final_bind_group,
            grid_overlay_geometry_bind_group,
            drag_overlay_geometry_bind_group,
            badge_geometry_bind_group,
            modal_badge_geometry_bind_group,
            control_geometry_bind_group,
            texture_size: (width.max(1), height.max(1)),
            last_capture_at: None,
            blur_dirty: true,
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
        let (overlay_geometry_texture, overlay_geometry_view) =
            create_overlay_geometry_texture(device, width, height);
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
        self.overlay_geometry_texture = overlay_geometry_texture;
        self.overlay_geometry_view = overlay_geometry_view;
        self.backdrop_texture = backdrop_texture;
        self.backdrop_view = backdrop_view;
        self.backdrop_mapping = BackdropMapping::full(width, height);
        self.gpu_backdrop_texture = None;
        self.using_gpu_backdrop = false;
        self.gpu_backdrop_is_copy_target = false;
        self.blur_texture = blur_texture;
        self.blur_view = blur_view;
        self.blur_levels = blur_levels;
        self.texture_size = (width, height);
        self.blur_dirty = true;
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
        self.grid_overlay_final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.grid_overlay_uniform_buffer,
            backdrop_view,
            &self.sampler,
            &self.overlay_geometry_view,
            &self.blur_view,
        );
        self.drag_overlay_final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.drag_overlay_uniform_buffer,
            backdrop_view,
            &self.sampler,
            &self.overlay_geometry_view,
            &self.blur_view,
        );
        self.badge_final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.badge_uniform_buffer,
            backdrop_view,
            &self.sampler,
            &self.overlay_geometry_view,
            &self.blur_view,
        );
        self.modal_badge_final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.modal_badge_uniform_buffer,
            backdrop_view,
            &self.sampler,
            &self.overlay_geometry_view,
            &self.blur_view,
        );
        self.control_final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.control_uniform_buffer,
            backdrop_view,
            &self.sampler,
            &self.overlay_geometry_view,
            &self.blur_view,
        );
        self.settings_panel_final_bind_group = create_final_bind_group(
            device,
            &self.final_bind_group_layout,
            &self.settings_panel_uniform_buffer,
            backdrop_view,
            &self.sampler,
            &self.overlay_geometry_view,
            &self.blur_view,
        );
    }

    fn bind_cpu_backdrop(&mut self, device: &wgpu::Device) {
        // Clone the view so the immutable borrow ends before we mutate `self`.
        let view = self.backdrop_view.clone();
        self.bind_backdrop_view(device, &view);
    }

    fn planned_capture_region(&self, scroll_x: f32) -> CaptureRegion {
        let (width, height) = self.texture_size;
        if self.debug.show_backdrop_texture {
            return CaptureRegion::full(width, height);
        }
        let groups: [&[GlassShape]; 7] = [
            &self.base_shapes,
            &self.grid_overlay_shapes,
            &self.drag_overlay_shapes,
            &self.badge_shapes,
            &self.modal_badge_shapes,
            &self.control_shapes,
            &self.settings_panel_shapes,
        ];
        capture_region_for_shapes(
            width,
            height,
            scroll_x,
            capture_sampling_padding(&self.params, self.debug, width, height),
            &groups,
        )
    }

    fn configure_cpu_backdrop(&mut self, device: &wgpu::Device, frame: &CpuCaptureFrame) -> bool {
        let (viewport_width, viewport_height) = self.texture_size;
        let region = frame.region.clamped_to(viewport_width, viewport_height);
        let expected_len = frame.width as usize * frame.height as usize * 4;
        if region != frame.region
            || frame.width == 0
            || frame.height == 0
            || frame.pixels.len() != expected_len
        {
            eprintln!(
                "liquid glass ignored invalid CPU capture: region={:?} output={}x{} bytes={}",
                frame.region,
                frame.width,
                frame.height,
                frame.pixels.len()
            );
            return false;
        }

        let next_mapping = BackdropMapping {
            region,
            texture_size: (frame.width, frame.height),
        };
        let texture_changed = self.backdrop_mapping.texture_size != next_mapping.texture_size;
        if texture_changed {
            let (backdrop_texture, backdrop_view) =
                create_backdrop_texture(device, frame.width, frame.height);
            let (blur_texture, blur_view) =
                create_blur_texture_raw(device, frame.width, frame.height, 0, "blur texture");
            let blur_levels = [
                create_blur_texture_raw(device, frame.width, frame.height, 1, "blur level 1"),
                create_blur_texture_raw(device, frame.width, frame.height, 2, "blur level 2"),
                create_blur_texture_raw(device, frame.width, frame.height, 3, "blur level 3"),
            ];
            self.backdrop_texture = backdrop_texture;
            self.backdrop_view = backdrop_view;
            self.blur_texture = blur_texture;
            self.blur_view = blur_view;
            self.blur_levels = blur_levels;
        }
        self.backdrop_mapping = next_mapping;
        if texture_changed || self.using_gpu_backdrop {
            self.bind_cpu_backdrop(device);
        }
        self.gpu_backdrop_texture = None;
        self.using_gpu_backdrop = false;
        self.gpu_backdrop_is_copy_target = false;
        true
    }

    fn copy_ephemeral_gpu_backdrop(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: EphemeralGpuCaptureFrame,
    ) -> bool {
        let EphemeralGpuCaptureFrame {
            texture,
            region,
            width,
            height,
            release_after_submit,
        } = frame;
        let (viewport_width, viewport_height) = self.texture_size;
        let region = region.clamped_to(viewport_width, viewport_height);
        if width == 0 || height == 0 {
            return false;
        }

        let next_mapping = BackdropMapping {
            region,
            texture_size: (width, height),
        };
        let texture_changed = self.backdrop_mapping.texture_size != next_mapping.texture_size;
        let needs_copy_target = texture_changed
            || !self.using_gpu_backdrop
            || !self.gpu_backdrop_is_copy_target
            || self.gpu_backdrop_texture.is_none();

        if needs_copy_target {
            let (gpu_texture, gpu_view) = create_gpu_backdrop_texture(device, width, height);
            if texture_changed {
                let (blur_texture, blur_view) =
                    create_blur_texture_raw(device, width, height, 0, "blur texture");
                let blur_levels = [
                    create_blur_texture_raw(device, width, height, 1, "blur level 1"),
                    create_blur_texture_raw(device, width, height, 2, "blur level 2"),
                    create_blur_texture_raw(device, width, height, 3, "blur level 3"),
                ];
                self.blur_texture = blur_texture;
                self.blur_view = blur_view;
                self.blur_levels = blur_levels;
            }
            self.bind_backdrop_view(device, &gpu_view);
            self.gpu_backdrop_texture = Some(gpu_texture);
            self.gpu_backdrop_is_copy_target = true;
            self.using_gpu_backdrop = true;
            eprintln!("liquid glass capture path: IOSurface -> persistent GPU texture");
        }
        self.backdrop_mapping = next_mapping;

        let Some(destination) = self.gpu_backdrop_texture.as_ref() else {
            return false;
        };
        let mut copy_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("liquid glass IOSurface copy encoder"),
        });
        copy_encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: destination,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(copy_encoder.finish()));
        queue.on_submitted_work_done(move || drop(release_after_submit));
        true
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
        self.active_base_shapes.clear();
        self.active_base_shapes.extend_from_slice(shapes);
        self.base_shape_scratch.clear();
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

    fn refresh_active_base_shapes(&mut self, queue: &wgpu::Queue, scroll_x: f32) {
        let frame = self
            .base_shapes
            .iter()
            .find(|shape| shape.is_frame())
            .copied()
            .unwrap_or_else(|| {
                GlassShape::fixed_rounded_rect(
                    [
                        self.texture_size.0 as f32 * 0.5,
                        self.texture_size.1 as f32 * 0.5,
                    ],
                    [self.texture_size.0 as f32, self.texture_size.1 as f32],
                    0.0,
                )
            });
        self.base_shape_scratch.clear();
        let smooth_union_radius = self.params.blend;
        self.base_shape_scratch
            .extend(self.base_shapes.iter().copied().filter(|shape| {
                base_shape_may_affect_frame(*shape, scroll_x, frame, smooth_union_radius)
            }));
        if self.base_shape_scratch == self.active_base_shapes {
            return;
        }
        std::mem::swap(&mut self.base_shape_scratch, &mut self.active_base_shapes);
        self.shape_count = self.active_base_shapes.len() as u32;
        if !self.active_base_shapes.is_empty() {
            queue.write_buffer(
                &self.shape_buffer,
                0,
                bytemuck::cast_slice(&self.active_base_shapes),
            );
        }
        self.last_geometry_key = None;
    }

    /// Replace the grid-overlay lane atomically. These shapes render after
    /// opaque tile fills but before grid icons, so their glass boundary stays
    /// distinct from the page frame without covering icon content.
    pub fn set_grid_overlay_shapes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shapes: &[GlassShape],
    ) {
        if self.grid_overlay_shapes.as_slice() == shapes {
            return;
        }
        self.grid_overlay_shapes.clear();
        self.grid_overlay_shapes.extend_from_slice(shapes);
        self.grid_overlay_shape_count = self.grid_overlay_shapes.len() as u32;
        if self.grid_overlay_shapes.len() > self.grid_overlay_shape_capacity {
            self.grid_overlay_shape_capacity = next_shape_capacity(
                self.grid_overlay_shape_capacity,
                self.grid_overlay_shapes.len(),
            );
            self.grid_overlay_shape_buffer = create_shape_buffer_with_capacity(
                device,
                self.grid_overlay_shape_capacity,
                "liquid glass grid overlay shape buffer",
            );
            self.grid_overlay_geometry_bind_group = create_geometry_bind_group(
                device,
                &self.geometry_bind_group_layout,
                &self.grid_overlay_uniform_buffer,
                &self.grid_overlay_shape_buffer,
            );
        }
        if !self.grid_overlay_shapes.is_empty() {
            queue.write_buffer(
                &self.grid_overlay_shape_buffer,
                0,
                bytemuck::cast_slice(&self.grid_overlay_shapes),
            );
        }
    }

    /// Replace the isolated top-level drag glass lane. Keeping this surface in
    /// a separate SDF field prevents it from smoothly unioning with stationary
    /// closed folders while it passes over them.
    pub fn set_drag_overlay_shapes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shapes: &[GlassShape],
    ) {
        if self.drag_overlay_shapes.as_slice() == shapes {
            return;
        }
        self.drag_overlay_shapes.clear();
        self.drag_overlay_shapes.extend_from_slice(shapes);
        self.drag_overlay_shape_count = self.drag_overlay_shapes.len() as u32;
        if self.drag_overlay_shapes.len() > self.drag_overlay_shape_capacity {
            self.drag_overlay_shape_capacity = next_shape_capacity(
                self.drag_overlay_shape_capacity,
                self.drag_overlay_shapes.len(),
            );
            self.drag_overlay_shape_buffer = create_shape_buffer_with_capacity(
                device,
                self.drag_overlay_shape_capacity,
                "liquid glass drag overlay shape buffer",
            );
            self.drag_overlay_geometry_bind_group = create_geometry_bind_group(
                device,
                &self.geometry_bind_group_layout,
                &self.drag_overlay_uniform_buffer,
                &self.drag_overlay_shape_buffer,
            );
        }
        if !self.drag_overlay_shapes.is_empty() {
            queue.write_buffer(
                &self.drag_overlay_shape_buffer,
                0,
                bytemuck::cast_slice(&self.drag_overlay_shapes),
            );
        }
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

    /// Replace the open-folder child badge shapes. They use a dedicated
    /// resource lane because they must composite after the modal folder glass,
    /// while top-level badges remain below that modal.
    pub fn set_modal_badge_shapes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shapes: &[GlassShape],
    ) {
        if self.modal_badge_shapes.as_slice() == shapes {
            return;
        }
        self.modal_badge_shapes.clear();
        self.modal_badge_shapes.extend_from_slice(shapes);
        self.modal_badge_shape_count = self.modal_badge_shapes.len() as u32;
        if self.modal_badge_shapes.len() > self.modal_badge_shape_capacity {
            self.modal_badge_shape_capacity = next_shape_capacity(
                self.modal_badge_shape_capacity,
                self.modal_badge_shapes.len(),
            );
            self.modal_badge_shape_buffer = create_shape_buffer_with_capacity(
                device,
                self.modal_badge_shape_capacity,
                "liquid glass modal badge shape buffer",
            );
            self.modal_badge_geometry_bind_group = create_geometry_bind_group(
                device,
                &self.geometry_bind_group_layout,
                &self.modal_badge_uniform_buffer,
                &self.modal_badge_shape_buffer,
            );
        }
        if !self.modal_badge_shapes.is_empty() {
            queue.write_buffer(
                &self.modal_badge_shape_buffer,
                0,
                bytemuck::cast_slice(&self.modal_badge_shapes),
            );
        }
    }

    pub fn notify_window_moved(&mut self, x: i32, y: i32, scale_factor: f64) {
        self.capture.on_window_moved(x, y, scale_factor);
        self.last_capture_at = None;
    }

    pub fn set_capture_active(&mut self, active: bool) {
        self.capture.set_active(active);
        if active {
            self.last_capture_at = None;
        }
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

        self.blur_dirty = true;

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
        #[cfg(target_os = "macos")]
        {
            // The macOS SCStream is continuous and non-blocking. Freezing its
            // consumer during a drag makes video behind the launcher visibly
            // jump when the gesture ends, so always take the newest frame.
            let _ = defer_backdrop_capture;
            true
        }

        #[cfg(not(target_os = "macos"))]
        {
            if defer_backdrop_capture {
                self.last_capture_at.is_none()
            } else {
                true
            }
        }
    }

    fn geometry_key(&self, scroll_x: f32) -> GeometryKey {
        let (width, height) = self.texture_size;
        let scroll_x = if self
            .active_base_shapes
            .iter()
            .any(|shape| shape.is_scrolling())
        {
            (scroll_x * 10.0).round() / 10.0
        } else {
            0.0
        };
        GeometryKey {
            scroll_x,
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
        let region = self.backdrop_mapping.region;
        let (texture_width, texture_height) = self.backdrop_mapping.texture_size;
        let capture_scale = (texture_width as f32 / region.width.max(1) as f32)
            .min(texture_height as f32 / region.height.max(1) as f32)
            .clamp(0.25, 1.0);
        // A lower-resolution texture covers more physical pixels per texel.
        // Scale the requested radius before selecting pyramid depth so a 2x
        // Retina downsample does not accidentally double the visible blur.
        let radius = self.params.blur_radius * capture_scale;
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
        CaptureStatus::Ready => eprintln!("liquid glass capture: platform backdrop ready"),
        CaptureStatus::Fallback { reason } => eprintln!("liquid glass capture fallback: {reason}"),
    }
}

fn should_refresh_blur(dirty: bool, captured: bool) -> bool {
    dirty || captured
}

fn uniforms_from_params(
    params: &LiquidGlassParams,
    debug: DebugOptions,
    viewport: (u32, u32),
    scroll_x: f32,
    shape_count: u32,
    time: f32,
    backdrop: BackdropMapping,
) -> GlassUniforms {
    let (width, height) = viewport;
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
        backdrop_origin: [backdrop.region.x as f32, backdrop.region.y as f32],
        backdrop_extent: [backdrop.region.width as f32, backdrop.region.height as f32],
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
    use super::{
        base_shape_may_affect_frame, capture_region_for_shapes, next_shape_capacity,
        should_refresh_blur, CaptureRegion, GlassShape, GlassUniforms,
    };

    fn test_scene_sdf(shapes: &[GlassShape], scroll_x: f32, point: [f32; 2], blend: f32) -> f32 {
        shapes.iter().fold(1.0e6, |distance, shape| {
            let mut resolved = *shape;
            if resolved.is_scrolling() {
                resolved.center[0] += scroll_x;
            }
            let shape_distance = super::rounded_rect_sdf(point, resolved);
            let e = (blend - (distance - shape_distance).abs()).max(0.0);
            distance.min(shape_distance) - e * e * 0.25 / blend
        })
    }

    #[test]
    fn shape_capacity_grows_only_past_current_capacity() {
        assert_eq!(next_shape_capacity(1, 2), 2);
        assert_eq!(next_shape_capacity(8, 9), 16);
        assert_eq!(next_shape_capacity(8, 20), 20);
    }

    #[test]
    fn glass_uniform_layout_matches_wgsl() {
        assert_eq!(std::mem::size_of::<GlassUniforms>(), 112);
        assert_eq!(std::mem::align_of::<GlassUniforms>(), 4);
    }

    #[test]
    fn backdrop_blur_is_reused_until_capture_changes() {
        assert!(should_refresh_blur(true, false));
        assert!(should_refresh_blur(false, true));
        assert!(!should_refresh_blur(false, false));
    }

    #[test]
    fn base_shape_culling_keeps_only_shapes_with_smooth_union_influence() {
        let frame = GlassShape::fixed_rounded_rect([500.0, 350.0], [600.0, 400.0], 40.0);
        let just_outside = GlassShape::rounded_rect([850.0, 350.0], [100.0, 100.0], 30.0);
        let far_page = GlassShape::rounded_rect([1_200.0, 350.0], [100.0, 100.0], 30.0);
        let swallowed = GlassShape::rounded_rect([500.0, 350.0], [100.0, 100.0], 30.0);
        let fixed_control = GlassShape::control_rounded_rect([1_200.0, 700.0], [100.0, 40.0], 20.0);

        assert!(base_shape_may_affect_frame(just_outside, 0.0, frame, 26.0));
        assert!(!base_shape_may_affect_frame(far_page, 0.0, frame, 26.0));
        assert!(!base_shape_may_affect_frame(swallowed, 0.0, frame, 26.0));
        assert!(base_shape_may_affect_frame(fixed_control, 0.0, frame, 26.0));
    }

    #[test]
    fn base_shape_culling_preserves_retina_grid_sdf_during_scroll() {
        let layout = crate::grid::GridLayout::for_app_count(177)
            .with_scale_factor(2.0)
            .centered(2_560.0);
        let (center_x, center_y, width, height) = layout.frame_panel_rect(2_560.0);
        let frame = GlassShape::fixed_rounded_rect(
            [center_x, center_y],
            [width, height],
            layout.scaled(crate::layout::grid::FRAME_CORNER_RADIUS),
        );
        let mut shapes = vec![frame];
        for index in 0..177 {
            let (x, y) = layout.tile_position(2_560.0, index);
            let halo = layout.tile_size + layout.scaled(18.0);
            shapes.push(GlassShape::rounded_rect(
                [x + layout.tile_size * 0.5, y + layout.tile_size * 0.5],
                [halo, halo],
                layout.scaled(28.0),
            ));
        }

        let page = layout.page_width(2_560.0);
        for scroll_x in [0.0, -page * 0.25, -page * 0.5, -page * 0.75, -page] {
            let active: Vec<_> = shapes
                .iter()
                .copied()
                .filter(|shape| base_shape_may_affect_frame(*shape, scroll_x, frame, 26.0))
                .collect();
            for y in (0..1_602).step_by(24) {
                for x in (0..2_560).step_by(24) {
                    let point = [x as f32 + 0.5, y as f32 + 0.5];
                    if super::rounded_rect_sdf(point, frame) >= 0.0 {
                        continue;
                    }
                    let full = test_scene_sdf(&shapes, scroll_x, point, 26.0);
                    let culled = test_scene_sdf(&active, scroll_x, point, 26.0);
                    assert!(
                        (full - culled).abs() < 0.001,
                        "SDF changed at scroll={scroll_x} point={point:?}: full={full} culled={culled} active={}",
                        active.len()
                    );
                }
            }
        }
    }

    #[test]
    fn capture_region_unions_visible_glass_and_ignores_clipped_offscreen_shapes() {
        let base = [
            GlassShape::fixed_rounded_rect([500.0, 350.0], [600.0, 400.0], 40.0),
            GlassShape::rounded_rect([1_200.0, 400.0], [100.0, 100.0], 30.0),
        ];
        let controls = [GlassShape::control_rounded_rect(
            [500.0, 700.0],
            [100.0, 40.0],
            20.0,
        )];
        let groups: [&[GlassShape]; 2] = [&base, &controls];

        assert_eq!(
            capture_region_for_shapes(1_000, 800, 0.0, 20.0, &groups),
            CaptureRegion {
                x: 180,
                y: 130,
                width: 640,
                height: 610,
            }
        );
    }
}
