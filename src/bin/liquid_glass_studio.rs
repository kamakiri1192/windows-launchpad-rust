use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use wgpu::util::DeviceExt;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
#[cfg(windows)]
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowAttributes, WindowId};

const INITIAL_WIDTH: u32 = 1180;
const INITIAL_HEIGHT: u32 = 760;
const GEOMETRY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const BACKDROP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const BLUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const SHAPE_SCROLLING: u32 = 0;
const SHAPE_CLIP_ONLY: u32 = 3;
const UI_PANEL_WIDTH: f32 = 324.0;
const UI_PANEL_MARGIN: f32 = 18.0;
const UI_ROW_HEIGHT: f32 = 42.0;
const UI_SLIDER_TRACK_WIDTH: f32 = 150.0;
const UI_SLIDER_TRACK_HEIGHT: f32 = 6.0;
const UI_BUTTON_WIDTH: f32 = 116.0;
const UI_BUTTON_HEIGHT: f32 = 30.0;

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
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct GlassShape {
    center: [f32; 2],
    size: [f32; 2],
    radius: f32,
    shape_type: u32,
    _pad: [u32; 2],
}

impl GlassShape {
    fn rounded_rect(center: [f32; 2], size: [f32; 2], radius: f32) -> Self {
        Self {
            center,
            size,
            radius,
            shape_type: SHAPE_SCROLLING,
            _pad: [0; 2],
        }
    }

    fn clip_rect(center: [f32; 2], size: [f32; 2]) -> Self {
        Self {
            center,
            size,
            radius: 1.0,
            shape_type: SHAPE_CLIP_ONLY,
            _pad: [0; 2],
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct StudioParams {
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
}

impl Default for StudioParams {
    fn default() -> Self {
        Self {
            thickness: 24.0,
            refractive_index: 1.42,
            chromatic_aberration: 0.075,
            blur_radius: 18.0,
            saturation: 1.36,
            glass_color: [0.94, 0.98, 1.0, 0.05],
            light_direction: normalize2([-0.42, -0.9]),
            light_intensity: 1.35,
            ambient_strength: 0.3,
            blend: 42.0,
        }
    }
}

impl StudioParams {
    fn launchpad_defaults() -> Self {
        Self {
            thickness: 26.0,
            refractive_index: 1.42,
            chromatic_aberration: 0.075,
            blur_radius: 16.0,
            saturation: 1.34,
            glass_color: [0.94, 0.98, 1.0, 0.045],
            light_direction: normalize2([-0.45, -0.9]),
            light_intensity: 1.25,
            ambient_strength: 0.28,
            blend: 26.0,
        }
    }

    fn is_launchpad_defaults(self) -> bool {
        let defaults = Self::launchpad_defaults();
        nearly_equal(self.thickness, defaults.thickness)
            && nearly_equal(self.refractive_index, defaults.refractive_index)
            && nearly_equal(self.chromatic_aberration, defaults.chromatic_aberration)
            && nearly_equal(self.blur_radius, defaults.blur_radius)
            && nearly_equal(self.saturation, defaults.saturation)
            && nearly_equal(self.glass_color[3], defaults.glass_color[3])
            && nearly_equal(self.light_intensity, defaults.light_intensity)
            && nearly_equal(self.ambient_strength, defaults.ambient_strength)
            && nearly_equal(self.blend, defaults.blend)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DebugOptions {
    show_backdrop_texture: bool,
    show_geometry_texture: bool,
    show_displacement: bool,
    show_alpha_mask: bool,
    show_final_glass_only: bool,
    disable_chromatic_aberration: bool,
    disable_edge_lighting: bool,
    disable_blur: bool,
}

impl DebugOptions {
    fn flags(self) -> u32 {
        let mut flags = 0;
        flags |= self.show_backdrop_texture as u32;
        flags |= (self.show_geometry_texture as u32) << 1;
        flags |= (self.show_displacement as u32) << 2;
        flags |= (self.show_alpha_mask as u32) << 3;
        flags |= (self.show_final_glass_only as u32) << 4;
        flags |= (self.disable_chromatic_aberration as u32) << 5;
        flags |= (self.disable_edge_lighting as u32) << 6;
        flags |= (self.disable_blur as u32) << 7;
        flags
    }
}

#[derive(Debug, Clone, Copy)]
struct StudioControls {
    spring_stiffness: f32,
    spring_damping: f32,
    stretch_factor: f32,
}

impl Default for StudioControls {
    fn default() -> Self {
        Self {
            spring_stiffness: 46.0,
            spring_damping: 11.5,
            stretch_factor: 0.018,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackgroundPreset {
    ColorField,
    Black,
    White,
    SplitTone,
    Checkerboard,
    TextContrast,
}

impl BackgroundPreset {
    const ALL: [Self; 6] = [
        Self::ColorField,
        Self::Black,
        Self::White,
        Self::SplitTone,
        Self::Checkerboard,
        Self::TextContrast,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::ColorField => "COLOR FIELD",
            Self::Black => "BLACK",
            Self::White => "WHITE",
            Self::SplitTone => "SPLIT TONE",
            Self::Checkerboard => "CHECKER",
            Self::TextContrast => "TEXT TEST",
        }
    }

    fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|preset| *preset == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|preset| *preset == self)
            .unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SliderId {
    Thickness,
    RefractiveIndex,
    ChromaticAberration,
    BlurRadius,
    Saturation,
    TintAlpha,
    LightIntensity,
    AmbientStrength,
    MergeDistance,
    SpringStiffness,
    SpringDamping,
    StretchFactor,
}

const SLIDERS: [SliderId; 12] = [
    SliderId::Thickness,
    SliderId::RefractiveIndex,
    SliderId::ChromaticAberration,
    SliderId::BlurRadius,
    SliderId::Saturation,
    SliderId::TintAlpha,
    SliderId::LightIntensity,
    SliderId::AmbientStrength,
    SliderId::MergeDistance,
    SliderId::SpringStiffness,
    SliderId::SpringDamping,
    SliderId::StretchFactor,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonId {
    LaunchpadDefaults,
    AppComposite,
    BackgroundPrevious,
    BackgroundNext,
}

#[derive(Debug, Clone, Copy)]
struct StudioUi {
    visible: bool,
    pointer: [f32; 2],
    active_slider: Option<SliderId>,
}

impl Default for StudioUi {
    fn default() -> Self {
        Self {
            visible: true,
            pointer: [0.0, 0.0],
            active_slider: None,
        }
    }
}

impl StudioUi {
    fn contains_pointer(self, width: f32, height: f32, pointer: [f32; 2]) -> bool {
        if !self.visible {
            return false;
        }
        let (x, y, w, h) = ui_panel_rect(width, height);
        pointer[0] >= x && pointer[0] <= x + w && pointer[1] >= y && pointer[1] <= y + h
    }

    fn begin_pointer(
        &mut self,
        width: f32,
        height: f32,
        pointer: [f32; 2],
        params: &mut StudioParams,
        controls: &mut StudioControls,
    ) -> bool {
        if !self.visible {
            return false;
        }
        self.pointer = pointer;
        self.active_slider = slider_at(width, height, pointer);
        if let Some(slider) = self.active_slider {
            set_slider_from_pointer(slider, width, pointer[0], params, controls);
            true
        } else {
            self.contains_pointer(width, height, pointer)
        }
    }

    fn drag_pointer(
        &mut self,
        width: f32,
        pointer: [f32; 2],
        params: &mut StudioParams,
        controls: &mut StudioControls,
    ) -> bool {
        self.pointer = pointer;
        if let Some(slider) = self.active_slider {
            set_slider_from_pointer(slider, width, pointer[0], params, controls);
            true
        } else {
            false
        }
    }

    fn end_pointer(&mut self) {
        self.active_slider = None;
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct UiUniforms {
    viewport: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct UiInstance {
    center: [f32; 2],
    size: [f32; 2],
    radius: f32,
    _pad: [f32; 3],
    color: [f32; 4],
}

impl UiInstance {
    fn rect(center: [f32; 2], size: [f32; 2], radius: f32, color: [f32; 4]) -> Self {
        Self {
            center,
            size,
            radius,
            _pad: [0.0; 3],
            color,
        }
    }
}

struct StudioApp {
    renderer: Option<StudioRenderer>,
    pointer: [f32; 2],
    spring: [f32; 2],
    velocity: [f32; 2],
    last_frame: Option<Instant>,
    show_anchor: bool,
    controls: StudioControls,
    ui: StudioUi,
}

impl Default for StudioApp {
    fn default() -> Self {
        Self {
            renderer: None,
            pointer: [INITIAL_WIDTH as f32 * 0.68, INITIAL_HEIGHT as f32 * 0.5],
            spring: [INITIAL_WIDTH as f32 * 0.68, INITIAL_HEIGHT as f32 * 0.5],
            velocity: [0.0, 0.0],
            last_frame: None,
            show_anchor: true,
            controls: StudioControls::default(),
            ui: StudioUi::default(),
        }
    }
}

impl ApplicationHandler for StudioApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }

        let mut attrs = WindowAttributes::default()
            .with_title("Liquid Glass Studio")
            .with_transparent(true)
            .with_inner_size(LogicalSize::new(INITIAL_WIDTH, INITIAL_HEIGHT))
            .with_min_inner_size(LogicalSize::new(980, 760));
        #[cfg(windows)]
        {
            attrs = attrs.with_no_redirection_bitmap(true);
        }
        let window = event_loop
            .create_window(attrs)
            .expect("create studio window");

        let size = window.inner_size();
        self.pointer = [size.width as f32 * 0.68, size.height as f32 * 0.5];
        self.spring = self.pointer;

        let mut renderer = pollster::block_on(StudioRenderer::new(window))
            .expect("initialize liquid glass studio renderer");
        renderer.update_title();
        self.renderer = Some(renderer);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size);
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(renderer.window.inner_size());
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let pointer = [position.x as f32, position.y as f32];
                self.ui.pointer = pointer;
                if let Some(renderer) = self.renderer.as_mut() {
                    let width = renderer.config.width as f32;
                    if self.ui.drag_pointer(
                        width,
                        pointer,
                        &mut renderer.params,
                        &mut self.controls,
                    ) {
                        renderer.update_title();
                        return;
                    }
                    if self
                        .ui
                        .contains_pointer(width, renderer.config.height as f32, pointer)
                    {
                        return;
                    }
                }
                self.pointer = pointer;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button != MouseButton::Left {
                    return;
                }
                if state == ElementState::Released {
                    self.ui.end_pointer();
                    return;
                }
                if let Some(renderer) = self.renderer.as_mut() {
                    let width = renderer.config.width as f32;
                    let height = renderer.config.height as f32;
                    if let Some(button) = button_at(width, height, self.ui.pointer) {
                        match button {
                            ButtonId::LaunchpadDefaults => {
                                renderer.params = StudioParams::launchpad_defaults();
                                renderer.debug = DebugOptions::default();
                                renderer.draw_backdrop_layer = false;
                            }
                            ButtonId::AppComposite => {
                                renderer.draw_backdrop_layer = !renderer.draw_backdrop_layer;
                            }
                            ButtonId::BackgroundPrevious => renderer.cycle_backdrop(-1),
                            ButtonId::BackgroundNext => renderer.cycle_backdrop(1),
                        }
                        renderer.update_title();
                        return;
                    }
                    let consumed = self.ui.begin_pointer(
                        width,
                        height,
                        self.ui.pointer,
                        &mut renderer.params,
                        &mut self.controls,
                    );
                    if consumed {
                        renderer.update_title();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                if code == KeyCode::Escape {
                    event_loop.exit();
                    return;
                }
                if let Some(renderer) = self.renderer.as_mut() {
                    if handle_key(
                        code,
                        renderer,
                        &mut self.show_anchor,
                        &mut self.controls,
                        &mut self.ui,
                    ) {
                        renderer.update_title();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = self
                    .last_frame
                    .map(|last| now.duration_since(last).as_secs_f32().min(0.05))
                    .unwrap_or(1.0 / 60.0);
                self.last_frame = Some(now);
                step_spring(
                    &mut self.spring,
                    &mut self.velocity,
                    self.pointer,
                    dt,
                    self.controls.spring_stiffness,
                    self.controls.spring_damping,
                );

                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.render(
                        self.pointer,
                        self.spring,
                        self.velocity,
                        self.show_anchor,
                        self.controls,
                        self.ui,
                    );
                    renderer.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(renderer) = self.renderer.as_ref() {
            renderer.window.request_redraw();
        }
    }
}

struct StudioRenderer {
    window: Window,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    params: StudioParams,
    debug: DebugOptions,
    uniform_buffer: wgpu::Buffer,
    shape_buffer: wgpu::Buffer,
    geometry_texture: wgpu::Texture,
    geometry_view: wgpu::TextureView,
    backdrop_texture: wgpu::Texture,
    backdrop_view: wgpu::TextureView,
    blur_texture: wgpu::Texture,
    blur_view: wgpu::TextureView,
    blur_levels: [(wgpu::Texture, wgpu::TextureView); 3],
    sampler: wgpu::Sampler,
    background_pipeline: wgpu::RenderPipeline,
    geometry_pipeline: wgpu::RenderPipeline,
    blur_downsample_pipeline: wgpu::RenderPipeline,
    blur_upsample_pipeline: wgpu::RenderPipeline,
    final_pipeline: wgpu::RenderPipeline,
    ui_pipeline: wgpu::RenderPipeline,
    background_bind_group_layout: wgpu::BindGroupLayout,
    geometry_bind_group_layout: wgpu::BindGroupLayout,
    blur_bind_group_layout: wgpu::BindGroupLayout,
    final_bind_group_layout: wgpu::BindGroupLayout,
    background_bind_group: wgpu::BindGroup,
    geometry_bind_group: wgpu::BindGroup,
    blur_down_bind_groups: [wgpu::BindGroup; 3],
    blur_up_bind_groups: [wgpu::BindGroup; 3],
    final_bind_group: wgpu::BindGroup,
    ui_uniform_buffer: wgpu::Buffer,
    ui_bind_group: wgpu::BindGroup,
    ui_instance_buffer: wgpu::Buffer,
    backdrop_preset: BackgroundPreset,
    draw_backdrop_layer: bool,
    last_title_update: Instant,
}

impl StudioRenderer {
    async fn new(window: Window) -> Result<Self, Box<dyn std::error::Error>> {
        let mut dx12 = wgpu::Dx12BackendOptions::from_env_or_default();
        if wgpu::Dx12SwapchainKind::from_env().is_none() {
            dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: default_backends(),
            backend_options: wgpu::BackendOptions {
                dx12,
                ..wgpu::BackendOptions::from_env_or_default()
            },
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = unsafe {
            let static_window: &'static Window = &*(&window as *const _);
            instance.create_surface(static_window)?
        };

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("liquid glass studio device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await?;

        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: select_present_mode(&caps.present_modes),
            desired_maximum_frame_latency: 2,
            alpha_mode: select_alpha_mode(&caps.alpha_modes),
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let params = StudioParams::launchpad_defaults();
        let debug = DebugOptions::default();
        let uniforms = uniforms_from_params(&params, debug, config.width, config.height, 0);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("studio liquid glass uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shape_buffer = create_shape_buffer(&device, &[]);
        let ui_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("studio ui uniforms"),
            contents: bytemuck::bytes_of(&UiUniforms {
                viewport: [config.width as f32, config.height as f32],
                _pad: [0.0; 2],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let ui_instance_buffer = create_ui_instance_buffer(&device, &[]);

        let (geometry_texture, geometry_view) =
            create_geometry_texture(&device, config.width, config.height);
        let (backdrop_texture, backdrop_view) =
            create_backdrop_texture(&device, config.width, config.height);
        let backdrop_preset = BackgroundPreset::ColorField;
        upload_backdrop(
            &queue,
            &backdrop_texture,
            config.width,
            config.height,
            backdrop_preset,
        );
        let (blur_texture, blur_view) = create_blur_texture_raw(
            &device,
            config.width,
            config.height,
            0,
            "studio blur texture",
        );
        let blur_levels = [
            create_blur_texture_raw(
                &device,
                config.width,
                config.height,
                1,
                "studio blur level 1",
            ),
            create_blur_texture_raw(
                &device,
                config.width,
                config.height,
                2,
                "studio blur level 2",
            ),
            create_blur_texture_raw(
                &device,
                config.width,
                config.height,
                3,
                "studio blur level 3",
            ),
        ];

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("studio liquid glass sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let background_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("studio background bgl"),
                entries: &[texture_entry(0, true), sampler_entry(1)],
            });
        let geometry_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("studio liquid glass geometry bgl"),
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
        let blur_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("studio liquid glass blur bgl"),
                entries: &[texture_entry(0, true), sampler_entry(1)],
            });
        let final_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("studio liquid glass final bgl"),
                entries: &[
                    uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                    texture_entry(1, true),
                    sampler_entry(2),
                    texture_entry(3, false),
                    texture_entry(4, true),
                ],
            });
        let ui_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("studio ui bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<UiUniforms>() as u64),
                    },
                    count: None,
                }],
            });

        let background_bind_group = create_background_bind_group(
            &device,
            &background_bind_group_layout,
            &backdrop_view,
            &sampler,
        );
        let geometry_bind_group = create_geometry_bind_group(
            &device,
            &geometry_bind_group_layout,
            &uniform_buffer,
            &shape_buffer,
        );
        let (blur_down_bind_groups, blur_up_bind_groups) = create_blur_pyramid_bind_groups(
            &device,
            &blur_bind_group_layout,
            &backdrop_view,
            &blur_levels,
            &sampler,
        );
        let final_bind_group = create_final_bind_group(
            &device,
            &final_bind_group_layout,
            &uniform_buffer,
            &backdrop_view,
            &sampler,
            &geometry_view,
            &blur_view,
        );
        let ui_bind_group =
            create_ui_bind_group(&device, &ui_bind_group_layout, &ui_uniform_buffer);

        let background_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio background shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_studio_background.wgsl").into(),
            ),
        });
        let geometry_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio liquid glass geometry shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_geometry.wgsl").into(),
            ),
        });
        let final_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio liquid glass final shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_final.wgsl").into(),
            ),
        });
        let blur_downsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio liquid glass blur downsample shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_blur_downsample.wgsl").into(),
            ),
        });
        let blur_upsample_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio liquid glass blur upsample shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_blur_upsample.wgsl").into(),
            ),
        });
        let ui_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("studio ui shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/liquid_glass_studio_ui.wgsl").into(),
            ),
        });

        let background_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("studio background pipeline layout"),
                bind_group_layouts: &[Some(&background_bind_group_layout)],
                immediate_size: 0,
            });
        let background_pipeline = create_fullscreen_pipeline(
            &device,
            "studio background pipeline",
            &background_pipeline_layout,
            &background_shader,
            surface_format,
            None,
        );

        let geometry_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("studio liquid glass geometry pipeline layout"),
                bind_group_layouts: &[Some(&geometry_bind_group_layout)],
                immediate_size: 0,
            });
        let geometry_pipeline = create_fullscreen_pipeline(
            &device,
            "studio liquid glass geometry pipeline",
            &geometry_pipeline_layout,
            &geometry_shader,
            GEOMETRY_FORMAT,
            None,
        );

        let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("studio liquid glass blur pipeline layout"),
            bind_group_layouts: &[Some(&blur_bind_group_layout)],
            immediate_size: 0,
        });
        let blur_downsample_pipeline = create_fullscreen_pipeline(
            &device,
            "studio liquid glass blur downsample pipeline",
            &blur_pipeline_layout,
            &blur_downsample_shader,
            BLUR_FORMAT,
            None,
        );
        let blur_upsample_pipeline = create_fullscreen_pipeline(
            &device,
            "studio liquid glass blur upsample pipeline",
            &blur_pipeline_layout,
            &blur_upsample_shader,
            BLUR_FORMAT,
            None,
        );

        let final_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("studio liquid glass final pipeline layout"),
                bind_group_layouts: &[Some(&final_bind_group_layout)],
                immediate_size: 0,
            });
        let final_pipeline = create_fullscreen_pipeline(
            &device,
            "studio liquid glass final pipeline",
            &final_pipeline_layout,
            &final_shader,
            surface_format,
            Some(premultiplied_blend()),
        );
        let ui_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("studio ui pipeline layout"),
            bind_group_layouts: &[Some(&ui_bind_group_layout)],
            immediate_size: 0,
        });
        let ui_pipeline = create_ui_pipeline(
            &device,
            "studio ui pipeline",
            &ui_pipeline_layout,
            &ui_shader,
            surface_format,
        );

        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            params,
            debug,
            uniform_buffer,
            shape_buffer,
            geometry_texture,
            geometry_view,
            backdrop_texture,
            backdrop_view,
            blur_texture,
            blur_view,
            blur_levels,
            sampler,
            background_pipeline,
            geometry_pipeline,
            blur_downsample_pipeline,
            blur_upsample_pipeline,
            final_pipeline,
            ui_pipeline,
            background_bind_group_layout,
            geometry_bind_group_layout,
            blur_bind_group_layout,
            final_bind_group_layout,
            background_bind_group,
            geometry_bind_group,
            blur_down_bind_groups,
            blur_up_bind_groups,
            final_bind_group,
            ui_uniform_buffer,
            ui_bind_group,
            ui_instance_buffer,
            backdrop_preset,
            draw_backdrop_layer: false,
            last_title_update: Instant::now() - Duration::from_secs(1),
        })
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);

        let (geometry_texture, geometry_view) =
            create_geometry_texture(&self.device, self.config.width, self.config.height);
        let (backdrop_texture, backdrop_view) =
            create_backdrop_texture(&self.device, self.config.width, self.config.height);
        upload_backdrop(
            &self.queue,
            &backdrop_texture,
            self.config.width,
            self.config.height,
            self.backdrop_preset,
        );
        let (blur_texture, blur_view) = create_blur_texture_raw(
            &self.device,
            self.config.width,
            self.config.height,
            0,
            "studio blur texture",
        );
        let blur_levels = [
            create_blur_texture_raw(
                &self.device,
                self.config.width,
                self.config.height,
                1,
                "studio blur level 1",
            ),
            create_blur_texture_raw(
                &self.device,
                self.config.width,
                self.config.height,
                2,
                "studio blur level 2",
            ),
            create_blur_texture_raw(
                &self.device,
                self.config.width,
                self.config.height,
                3,
                "studio blur level 3",
            ),
        ];

        self.geometry_texture = geometry_texture;
        self.geometry_view = geometry_view;
        self.backdrop_texture = backdrop_texture;
        self.backdrop_view = backdrop_view;
        self.blur_texture = blur_texture;
        self.blur_view = blur_view;
        self.blur_levels = blur_levels;
        self.background_bind_group = create_background_bind_group(
            &self.device,
            &self.background_bind_group_layout,
            &self.backdrop_view,
            &self.sampler,
        );
        let (down, up) = create_blur_pyramid_bind_groups(
            &self.device,
            &self.blur_bind_group_layout,
            &self.backdrop_view,
            &self.blur_levels,
            &self.sampler,
        );
        self.blur_down_bind_groups = down;
        self.blur_up_bind_groups = up;
        self.final_bind_group = create_final_bind_group(
            &self.device,
            &self.final_bind_group_layout,
            &self.uniform_buffer,
            &self.backdrop_view,
            &self.sampler,
            &self.geometry_view,
            &self.blur_view,
        );
        self.queue.write_buffer(
            &self.ui_uniform_buffer,
            0,
            bytemuck::bytes_of(&UiUniforms {
                viewport: [self.config.width as f32, self.config.height as f32],
                _pad: [0.0; 2],
            }),
        );
    }

    fn render(
        &mut self,
        pointer: [f32; 2],
        spring: [f32; 2],
        velocity: [f32; 2],
        show_anchor: bool,
        controls: StudioControls,
        ui: StudioUi,
    ) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.resize(self.window.inner_size());
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return,
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let shapes = studio_shapes(
            self.config.width as f32,
            self.config.height as f32,
            pointer,
            spring,
            velocity,
            show_anchor,
            controls,
        );
        self.shape_buffer = create_shape_buffer(&self.device, &shapes);
        self.geometry_bind_group = create_geometry_bind_group(
            &self.device,
            &self.geometry_bind_group_layout,
            &self.uniform_buffer,
            &self.shape_buffer,
        );
        let uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            self.config.width,
            self.config.height,
            shapes.len() as u32,
        );
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        let ui_instances = build_ui_instances(
            self.config.width as f32,
            self.config.height as f32,
            &self.params,
            &controls,
            self.debug,
            show_anchor,
            self.backdrop_preset,
            self.draw_backdrop_layer,
            !surface_preserves_alpha(self.config.alpha_mode),
            ui,
        );
        self.ui_instance_buffer = create_ui_instance_buffer(&self.device, &ui_instances);

        let blur_levels = studio_blur_level_count(&self.params, self.debug);
        for i in 0..blur_levels {
            let label = format!("studio liquid glass blur downsample L{i}->L{}", i + 1);
            let mut blur_encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some(label.as_str()),
                    });
            {
                let mut pass = blur_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label.as_str()),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.blur_levels[i].1,
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
            self.queue.submit(std::iter::once(blur_encoder.finish()));
        }

        for j in 0..blur_levels {
            let dst = if j == blur_levels - 1 {
                &self.blur_view
            } else {
                &self.blur_levels[blur_levels - 2 - j].1
            };
            let bind_idx = 3 - blur_levels + j;
            let label = format!(
                "studio liquid glass blur upsample L{}->L{}",
                blur_levels - j,
                blur_levels - 1 - j
            );
            let mut blur_encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some(label.as_str()),
                    });
            {
                let mut pass = blur_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
            self.queue.submit(std::iter::once(blur_encoder.finish()));
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("studio liquid glass encoder"),
            });

        let draw_backdrop_layer = self.effective_draw_backdrop_layer();
        if draw_backdrop_layer {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("studio background pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
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
            pass.set_pipeline(&self.background_pipeline);
            pass.set_bind_group(0, &self.background_bind_group, &[]);
            pass.draw(0..3, 0..1);
        } else {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("studio transparent clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
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
            drop(pass);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("studio liquid glass geometry pass"),
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
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("studio liquid glass final pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
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

        if !ui_instances.is_empty() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("studio ui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
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
            pass.set_pipeline(&self.ui_pipeline);
            pass.set_bind_group(0, &self.ui_bind_group, &[]);
            pass.set_vertex_buffer(0, self.ui_instance_buffer.slice(..));
            pass.draw(0..6, 0..ui_instances.len() as u32);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        if self.last_title_update.elapsed() > Duration::from_millis(250) {
            self.update_title();
        }
    }

    fn update_title(&mut self) {
        self.last_title_update = Instant::now();
        self.window.set_title(&format!(
            "Liquid Glass Studio | {}  bg {}  composite {}  thickness {:.0}  merge {:.0}  blur {:.0}  chroma {:.3}  U panel  R app reset",
            if self.params.is_launchpad_defaults() {
                "APP DEFAULTS"
            } else {
                "TUNED"
            },
            self.backdrop_preset.label(),
            if self.draw_backdrop_layer {
                "preview"
            } else if !surface_preserves_alpha(self.config.alpha_mode) {
                "app-sim"
            } else {
                "app"
            },
            self.params.thickness,
            self.params.blend,
            self.params.blur_radius,
            self.params.chromatic_aberration,
        ));
    }

    fn set_backdrop_preset(&mut self, preset: BackgroundPreset) {
        if self.backdrop_preset == preset {
            return;
        }
        self.backdrop_preset = preset;
        upload_backdrop(
            &self.queue,
            &self.backdrop_texture,
            self.config.width,
            self.config.height,
            self.backdrop_preset,
        );
    }

    fn cycle_backdrop(&mut self, direction: i32) {
        let preset = if direction < 0 {
            self.backdrop_preset.previous()
        } else {
            self.backdrop_preset.next()
        };
        self.set_backdrop_preset(preset);
    }

    fn effective_draw_backdrop_layer(&self) -> bool {
        self.draw_backdrop_layer || !surface_preserves_alpha(self.config.alpha_mode)
    }
}

fn handle_key(
    code: KeyCode,
    renderer: &mut StudioRenderer,
    show_anchor: &mut bool,
    controls: &mut StudioControls,
    ui: &mut StudioUi,
) -> bool {
    match code {
        KeyCode::Digit1 => renderer.params.thickness = (renderer.params.thickness - 2.0).max(2.0),
        KeyCode::Digit2 => renderer.params.thickness = (renderer.params.thickness + 2.0).min(80.0),
        KeyCode::Digit3 => renderer.params.blend = (renderer.params.blend - 4.0).max(0.0),
        KeyCode::Digit4 => renderer.params.blend = (renderer.params.blend + 4.0).min(120.0),
        KeyCode::Digit5 => {
            renderer.params.blur_radius = (renderer.params.blur_radius - 2.0).max(0.0)
        }
        KeyCode::Digit6 => {
            renderer.params.blur_radius = (renderer.params.blur_radius + 2.0).min(40.0)
        }
        KeyCode::Digit7 => {
            renderer.params.chromatic_aberration =
                (renderer.params.chromatic_aberration - 0.01).max(0.0);
        }
        KeyCode::Digit8 => {
            renderer.params.chromatic_aberration =
                (renderer.params.chromatic_aberration + 0.01).min(0.2);
        }
        KeyCode::Space => *show_anchor = !*show_anchor,
        KeyCode::KeyB => {
            renderer.debug.show_backdrop_texture = !renderer.debug.show_backdrop_texture
        }
        KeyCode::KeyG => {
            renderer.debug.show_geometry_texture = !renderer.debug.show_geometry_texture
        }
        KeyCode::KeyD => renderer.debug.show_displacement = !renderer.debug.show_displacement,
        KeyCode::KeyA => renderer.debug.show_alpha_mask = !renderer.debug.show_alpha_mask,
        KeyCode::KeyF => {
            renderer.debug.show_final_glass_only = !renderer.debug.show_final_glass_only
        }
        KeyCode::KeyC => {
            renderer.debug.disable_chromatic_aberration =
                !renderer.debug.disable_chromatic_aberration;
        }
        KeyCode::KeyL => renderer.debug.disable_blur = !renderer.debug.disable_blur,
        KeyCode::KeyN => renderer.cycle_backdrop(1),
        KeyCode::KeyP => renderer.cycle_backdrop(-1),
        KeyCode::KeyM => {
            renderer.params = StudioParams::launchpad_defaults();
            renderer.debug = DebugOptions::default();
            renderer.draw_backdrop_layer = false;
        }
        KeyCode::KeyR => {
            renderer.params = StudioParams::launchpad_defaults();
            renderer.debug = DebugOptions::default();
            renderer.set_backdrop_preset(BackgroundPreset::ColorField);
            renderer.draw_backdrop_layer = false;
            *controls = StudioControls::default();
            *show_anchor = true;
        }
        KeyCode::KeyU => ui.visible = !ui.visible,
        _ => return false,
    }
    true
}

fn step_spring(
    position: &mut [f32; 2],
    velocity: &mut [f32; 2],
    target: [f32; 2],
    dt: f32,
    stiffness: f32,
    damping: f32,
) {
    for axis in 0..2 {
        let acceleration = (target[axis] - position[axis]) * stiffness - velocity[axis] * damping;
        velocity[axis] += acceleration * dt;
        position[axis] += velocity[axis] * dt;
    }
}

fn studio_shapes(
    width: f32,
    height: f32,
    pointer: [f32; 2],
    spring: [f32; 2],
    velocity: [f32; 2],
    show_anchor: bool,
    controls: StudioControls,
) -> Vec<GlassShape> {
    let mut shapes = Vec::with_capacity(5);
    shapes.push(GlassShape::clip_rect(
        [width * 0.5, height * 0.5],
        [width * 2.0, height * 2.0],
    ));

    if show_anchor {
        shapes.push(GlassShape::rounded_rect(
            [width * 0.5, height * 0.5],
            [260.0, 168.0],
            62.0,
        ));
    }

    let speed = (velocity[0] * velocity[0] + velocity[1] * velocity[1]).sqrt();
    let stretch = (speed * controls.stretch_factor).min(90.0);
    shapes.push(GlassShape::rounded_rect(
        spring,
        [178.0 + stretch, 132.0 + stretch * 0.24],
        58.0,
    ));
    shapes.push(GlassShape::rounded_rect(
        [pointer[0], pointer[1]],
        [64.0, 64.0],
        32.0,
    ));
    shapes
}

fn uniforms_from_params(
    params: &StudioParams,
    debug: DebugOptions,
    width: u32,
    height: u32,
    shape_count: u32,
) -> GlassUniforms {
    GlassUniforms {
        viewport: [width as f32, height as f32],
        scroll_x: 0.0,
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

fn studio_blur_level_count(params: &StudioParams, debug: DebugOptions) -> usize {
    if debug.disable_blur {
        return 0;
    }
    let radius = params.blur_radius;
    if radius < 6.0 {
        1
    } else if radius < 16.0 {
        2
    } else {
        3
    }
}

fn ui_panel_rect(width: f32, height: f32) -> (f32, f32, f32, f32) {
    let h = (height - UI_PANEL_MARGIN * 2.0).max(420.0);
    (
        width - UI_PANEL_WIDTH - UI_PANEL_MARGIN,
        UI_PANEL_MARGIN,
        UI_PANEL_WIDTH,
        h,
    )
}

fn slider_row_y(index: usize) -> f32 {
    UI_PANEL_MARGIN + 118.0 + index as f32 * UI_ROW_HEIGHT
}

fn slider_track_rect(width: f32, index: usize) -> (f32, f32, f32, f32) {
    let panel_x = width - UI_PANEL_WIDTH - UI_PANEL_MARGIN;
    (
        panel_x + 144.0,
        slider_row_y(index) + 22.0,
        UI_SLIDER_TRACK_WIDTH,
        UI_SLIDER_TRACK_HEIGHT,
    )
}

fn background_button_rect(width: f32, height: f32, id: ButtonId) -> (f32, f32, f32, f32) {
    let (panel_x, panel_y, _, panel_h) = ui_panel_rect(width, height);
    match id {
        ButtonId::LaunchpadDefaults => (panel_x + 24.0, panel_y + 74.0, UI_BUTTON_WIDTH, 26.0),
        ButtonId::AppComposite => (panel_x + 156.0, panel_y + 74.0, UI_BUTTON_WIDTH, 26.0),
        ButtonId::BackgroundPrevious => (
            panel_x + 24.0,
            panel_y + panel_h - 42.0,
            UI_BUTTON_WIDTH,
            UI_BUTTON_HEIGHT,
        ),
        ButtonId::BackgroundNext => (
            panel_x + 156.0,
            panel_y + panel_h - 42.0,
            UI_BUTTON_WIDTH,
            UI_BUTTON_HEIGHT,
        ),
    }
}

fn button_at(width: f32, height: f32, pointer: [f32; 2]) -> Option<ButtonId> {
    for button in [
        ButtonId::LaunchpadDefaults,
        ButtonId::AppComposite,
        ButtonId::BackgroundPrevious,
        ButtonId::BackgroundNext,
    ] {
        let (x, y, w, h) = background_button_rect(width, height, button);
        if pointer[0] >= x && pointer[0] <= x + w && pointer[1] >= y && pointer[1] <= y + h {
            return Some(button);
        }
    }
    None
}

fn slider_at(width: f32, height: f32, pointer: [f32; 2]) -> Option<SliderId> {
    let ui = StudioUi {
        visible: true,
        pointer,
        active_slider: None,
    };
    if !ui.contains_pointer(width, height, pointer) {
        return None;
    }

    for (index, slider) in SLIDERS.iter().enumerate() {
        let (x, y, w, _) = slider_track_rect(width, index);
        if pointer[0] >= x - 10.0
            && pointer[0] <= x + w + 10.0
            && pointer[1] >= y - 14.0
            && pointer[1] <= y + 18.0
        {
            return Some(*slider);
        }
    }
    None
}

fn slider_spec(id: SliderId) -> (&'static str, f32, f32) {
    match id {
        SliderId::Thickness => ("THICK", 2.0, 80.0),
        SliderId::RefractiveIndex => ("IOR", 1.0, 2.2),
        SliderId::ChromaticAberration => ("CHROMA", 0.0, 0.2),
        SliderId::BlurRadius => ("BLUR", 0.0, 40.0),
        SliderId::Saturation => ("SAT", 0.4, 2.2),
        SliderId::TintAlpha => ("TINT", 0.0, 0.35),
        SliderId::LightIntensity => ("LIGHT", 0.0, 3.0),
        SliderId::AmbientStrength => ("AMBIENT", 0.0, 1.0),
        SliderId::MergeDistance => ("MERGE", 0.0, 120.0),
        SliderId::SpringStiffness => ("SPRING", 10.0, 120.0),
        SliderId::SpringDamping => ("DAMP", 2.0, 30.0),
        SliderId::StretchFactor => ("STRETCH", 0.0, 0.05),
    }
}

fn slider_value(id: SliderId, params: &StudioParams, controls: &StudioControls) -> f32 {
    match id {
        SliderId::Thickness => params.thickness,
        SliderId::RefractiveIndex => params.refractive_index,
        SliderId::ChromaticAberration => params.chromatic_aberration,
        SliderId::BlurRadius => params.blur_radius,
        SliderId::Saturation => params.saturation,
        SliderId::TintAlpha => params.glass_color[3],
        SliderId::LightIntensity => params.light_intensity,
        SliderId::AmbientStrength => params.ambient_strength,
        SliderId::MergeDistance => params.blend,
        SliderId::SpringStiffness => controls.spring_stiffness,
        SliderId::SpringDamping => controls.spring_damping,
        SliderId::StretchFactor => controls.stretch_factor,
    }
}

fn set_slider_value(
    id: SliderId,
    value: f32,
    params: &mut StudioParams,
    controls: &mut StudioControls,
) {
    let (_, min, max) = slider_spec(id);
    let value = value.clamp(min, max);
    match id {
        SliderId::Thickness => params.thickness = value,
        SliderId::RefractiveIndex => params.refractive_index = value,
        SliderId::ChromaticAberration => params.chromatic_aberration = value,
        SliderId::BlurRadius => params.blur_radius = value,
        SliderId::Saturation => params.saturation = value,
        SliderId::TintAlpha => params.glass_color[3] = value,
        SliderId::LightIntensity => params.light_intensity = value,
        SliderId::AmbientStrength => params.ambient_strength = value,
        SliderId::MergeDistance => params.blend = value,
        SliderId::SpringStiffness => controls.spring_stiffness = value,
        SliderId::SpringDamping => controls.spring_damping = value,
        SliderId::StretchFactor => controls.stretch_factor = value,
    }
}

fn set_slider_from_pointer(
    id: SliderId,
    width: f32,
    pointer_x: f32,
    params: &mut StudioParams,
    controls: &mut StudioControls,
) {
    let Some(index) = SLIDERS.iter().position(|candidate| *candidate == id) else {
        return;
    };
    let (x, _, w, _) = slider_track_rect(width, index);
    let (_, min, max) = slider_spec(id);
    let t = ((pointer_x - x) / w).clamp(0.0, 1.0);
    set_slider_value(id, min + (max - min) * t, params, controls);
}

#[allow(clippy::too_many_arguments)]
fn build_ui_instances(
    width: f32,
    height: f32,
    params: &StudioParams,
    controls: &StudioControls,
    debug: DebugOptions,
    show_anchor: bool,
    backdrop_preset: BackgroundPreset,
    draw_backdrop_layer: bool,
    app_mode_simulated: bool,
    ui: StudioUi,
) -> Vec<UiInstance> {
    if !ui.visible {
        return Vec::new();
    }

    let mut instances = Vec::with_capacity(900);
    let (panel_x, panel_y, panel_w, panel_h) = ui_panel_rect(width, height);
    let panel_color = [0.06, 0.075, 0.09, 0.78];
    let panel_border = [0.84, 0.94, 1.0, 0.22];
    let text_color = [0.86, 0.94, 1.0, 0.88];
    let muted_text = [0.60, 0.72, 0.82, 0.72];
    let active_color = [0.58, 0.88, 1.0, 0.96];
    let track_color = [0.20, 0.27, 0.34, 0.72];
    let fill_color = [0.44, 0.74, 0.92, 0.88];
    let button_color = [0.16, 0.22, 0.28, 0.86];
    let button_hover_color = [0.24, 0.34, 0.42, 0.92];

    instances.push(UiInstance::rect(
        [panel_x + panel_w * 0.5, panel_y + panel_h * 0.5],
        [panel_w + 1.5, panel_h + 1.5],
        18.0,
        panel_border,
    ));
    instances.push(UiInstance::rect(
        [panel_x + panel_w * 0.5, panel_y + panel_h * 0.5],
        [panel_w, panel_h],
        18.0,
        panel_color,
    ));
    push_text(
        &mut instances,
        panel_x + 24.0,
        panel_y + 24.0,
        2.7,
        "LIQUID GLASS",
        text_color,
    );
    push_text(
        &mut instances,
        panel_x + 24.0,
        panel_y + 50.0,
        1.9,
        "DEV CONTROLS",
        muted_text,
    );
    push_button(
        &mut instances,
        width,
        height,
        ButtonId::LaunchpadDefaults,
        "APP RESET",
        ui.pointer,
        button_color,
        button_hover_color,
        text_color,
    );
    push_button(
        &mut instances,
        width,
        height,
        ButtonId::AppComposite,
        if draw_backdrop_layer {
            "PREVIEW"
        } else if app_mode_simulated {
            "APP SIM"
        } else {
            "APP MODE"
        },
        ui.pointer,
        button_color,
        button_hover_color,
        text_color,
    );
    push_text(
        &mut instances,
        panel_x + 24.0,
        panel_y + 104.0,
        1.35,
        if params.is_launchpad_defaults() {
            "APP DEFAULTS ACTIVE"
        } else {
            "PARAMS TUNED"
        },
        muted_text,
    );

    for (index, slider) in SLIDERS.iter().copied().enumerate() {
        let (label, min, max) = slider_spec(slider);
        let value = slider_value(slider, params, controls);
        let t = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let row_y = slider_row_y(index);
        let (track_x, track_y, track_w, track_h) = slider_track_rect(width, index);
        let color = if ui.active_slider == Some(slider) {
            active_color
        } else {
            text_color
        };

        push_text(
            &mut instances,
            panel_x + 24.0,
            row_y + 11.0,
            1.65,
            label,
            color,
        );
        push_text(
            &mut instances,
            panel_x + 234.0,
            row_y + 4.0,
            1.45,
            &slider_value_label(slider, value),
            muted_text,
        );
        instances.push(UiInstance::rect(
            [track_x + track_w * 0.5, track_y + track_h * 0.5],
            [track_w, track_h],
            track_h * 0.5,
            track_color,
        ));
        instances.push(UiInstance::rect(
            [track_x + track_w * t * 0.5, track_y + track_h * 0.5],
            [(track_w * t).max(track_h), track_h],
            track_h * 0.5,
            fill_color,
        ));
        instances.push(UiInstance::rect(
            [track_x + track_w * t, track_y + track_h * 0.5],
            [17.0, 17.0],
            8.5,
            active_color,
        ));
    }

    let footer_y = panel_y + panel_h - 110.0;
    let anchor = if show_anchor {
        "ANCHOR ON"
    } else {
        "ANCHOR OFF"
    };
    let chroma = if debug.disable_chromatic_aberration {
        "CHROMA OFF"
    } else {
        "CHROMA ON"
    };
    let blur = if debug.disable_blur {
        "BLUR OFF"
    } else {
        "BLUR ON"
    };
    push_text(
        &mut instances,
        panel_x + 24.0,
        footer_y,
        1.55,
        anchor,
        muted_text,
    );
    push_text(
        &mut instances,
        panel_x + 24.0,
        footer_y + 20.0,
        1.55,
        chroma,
        muted_text,
    );
    push_text(
        &mut instances,
        panel_x + 170.0,
        footer_y + 20.0,
        1.55,
        blur,
        muted_text,
    );
    push_text(
        &mut instances,
        panel_x + 24.0,
        footer_y + 40.0,
        1.45,
        backdrop_preset.label(),
        muted_text,
    );
    push_text(
        &mut instances,
        panel_x + 170.0,
        footer_y + 40.0,
        1.45,
        if draw_backdrop_layer {
            "PREVIEW"
        } else if app_mode_simulated {
            "APP SIM"
        } else {
            "APP MODE"
        },
        muted_text,
    );
    push_button(
        &mut instances,
        width,
        height,
        ButtonId::BackgroundPrevious,
        "PREV",
        ui.pointer,
        button_color,
        button_hover_color,
        text_color,
    );
    push_button(
        &mut instances,
        width,
        height,
        ButtonId::BackgroundNext,
        "NEXT",
        ui.pointer,
        button_color,
        button_hover_color,
        text_color,
    );

    instances
}

#[allow(clippy::too_many_arguments)]
fn push_button(
    instances: &mut Vec<UiInstance>,
    width: f32,
    height: f32,
    id: ButtonId,
    label: &str,
    pointer: [f32; 2],
    color: [f32; 4],
    hover_color: [f32; 4],
    text_color: [f32; 4],
) {
    let (x, y, w, h) = background_button_rect(width, height, id);
    let hovered = pointer[0] >= x && pointer[0] <= x + w && pointer[1] >= y && pointer[1] <= y + h;
    instances.push(UiInstance::rect(
        [x + w * 0.5, y + h * 0.5],
        [w, h],
        8.0,
        if hovered { hover_color } else { color },
    ));
    let label_width = label.chars().count() as f32 * 6.0 * 1.65;
    push_text(
        instances,
        x + (w - label_width) * 0.5,
        y + 9.0,
        1.65,
        label,
        text_color,
    );
}

fn slider_value_label(id: SliderId, value: f32) -> String {
    match id {
        SliderId::ChromaticAberration | SliderId::TintAlpha | SliderId::StretchFactor => {
            format!("{value:.3}")
        }
        SliderId::RefractiveIndex | SliderId::Saturation | SliderId::AmbientStrength => {
            format!("{value:.2}")
        }
        SliderId::LightIntensity => format!("{value:.1}"),
        _ => format!("{value:.0}"),
    }
}

fn push_text(
    instances: &mut Vec<UiInstance>,
    x: f32,
    y: f32,
    scale: f32,
    text: &str,
    color: [f32; 4],
) {
    let mut cursor = x;
    for ch in text.chars() {
        if ch == ' ' {
            cursor += scale * 4.0;
            continue;
        }
        if let Some(glyph) = glyph_5x7(ch) {
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        instances.push(UiInstance::rect(
                            [
                                cursor + col as f32 * scale + scale * 0.5,
                                y + row as f32 * scale + scale * 0.5,
                            ],
                            [scale * 0.82, scale * 0.82],
                            scale * 0.18,
                            color,
                        ));
                    }
                }
            }
            cursor += scale * 6.0;
        } else {
            cursor += scale * 3.0;
        }
    }
}

fn glyph_5x7(ch: char) -> Option<[u8; 7]> {
    Some(match ch.to_ascii_uppercase() {
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0c],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1e, 0x01, 0x01, 0x0e, 0x01, 0x01, 0x1e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x10, 0x1e, 0x01, 0x01, 0x1e],
        '6' => [0x0e, 0x10, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x01, 0x0e],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        '-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        _ => return None,
    })
}

fn create_shape_buffer(device: &wgpu::Device, shapes: &[GlassShape]) -> wgpu::Buffer {
    let fallback = [GlassShape::clip_rect([0.0, 0.0], [1.0, 1.0])];
    let slice = if shapes.is_empty() { &fallback } else { shapes };
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("studio liquid glass shape buffer"),
        contents: bytemuck::cast_slice(slice),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    })
}

fn create_ui_instance_buffer(device: &wgpu::Device, instances: &[UiInstance]) -> wgpu::Buffer {
    let fallback = [UiInstance::rect(
        [-100.0, -100.0],
        [1.0, 1.0],
        0.0,
        [0.0, 0.0, 0.0, 0.0],
    )];
    let slice = if instances.is_empty() {
        &fallback
    } else {
        instances
    };
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("studio ui instance buffer"),
        contents: bytemuck::cast_slice(slice),
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
    })
}

fn create_geometry_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("studio liquid glass geometry texture"),
        size: texture_extent(width, height),
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
        label: Some("studio liquid glass backdrop texture"),
        size: texture_extent(width, height),
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

fn create_blur_texture_raw(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    level: u32,
    label: &'static str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let (width, height) = blur_level_extent(width, height, level);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: texture_extent(width, height),
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

fn texture_extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    }
}

fn blur_level_extent(width: u32, height: u32, level: u32) -> (u32, u32) {
    let mut width = width.max(1);
    let mut height = height.max(1);
    for _ in 0..level.min(3) {
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    (width, height)
}

fn upload_backdrop(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    preset: BackgroundPreset,
) {
    let pixels = make_backdrop(width.max(1), height.max(1), preset);
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
        texture_extent(width, height),
    );
}

fn make_backdrop(width: u32, height: u32, preset: BackgroundPreset) -> Vec<u8> {
    match preset {
        BackgroundPreset::ColorField => make_color_field_backdrop(width, height),
        BackgroundPreset::Black => make_solid_backdrop(width, height, [0.0, 0.0, 0.0]),
        BackgroundPreset::White => make_solid_backdrop(width, height, [1.0, 1.0, 1.0]),
        BackgroundPreset::SplitTone => make_split_tone_backdrop(width, height),
        BackgroundPreset::Checkerboard => make_checkerboard_backdrop(width, height),
        BackgroundPreset::TextContrast => make_text_contrast_backdrop(width, height),
    }
}

fn make_color_field_backdrop(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let fx = x as f32 / width.max(1) as f32;
            let fy = y as f32 / height.max(1) as f32;
            let stripe = ((fx * 18.0 + fy * 6.0).sin() * 0.5 + 0.5).powf(1.8);
            let grid = if x % 96 < 3 || y % 96 < 3 { 0.18 } else { 0.0 };
            let card = if x % 260 > 28 && x % 260 < 214 && y % 180 > 28 && y % 180 < 132 {
                0.16
            } else {
                0.0
            };
            let r = (0.10 + 0.68 * fx + 0.18 * stripe + card).min(1.0);
            let g = (0.18 + 0.42 * (1.0 - fy) + 0.22 * stripe + grid).min(1.0);
            let b = (0.30 + 0.45 * fy + 0.14 * (1.0 - stripe) + card * 0.5).min(1.0);
            let idx = ((y * width + x) * 4) as usize;
            pixels[idx] = (r * 255.0) as u8;
            pixels[idx + 1] = (g * 255.0) as u8;
            pixels[idx + 2] = (b * 255.0) as u8;
            pixels[idx + 3] = 255;
        }
    }
    pixels
}

fn make_solid_backdrop(width: u32, height: u32, color: [f32; 3]) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            write_pixel(&mut pixels, width, x, y, color);
        }
    }
    pixels
}

fn make_split_tone_backdrop(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let quadrant = (x >= width / 2, y >= height / 2);
            let color = match quadrant {
                (false, false) => [0.0, 0.0, 0.0],
                (true, false) => [1.0, 1.0, 1.0],
                (false, true) => [0.08, 0.10, 0.13],
                (true, true) => [0.90, 0.92, 0.86],
            };
            write_pixel(&mut pixels, width, x, y, color);
        }
    }
    draw_backdrop_text(
        &mut pixels,
        width,
        height,
        42,
        42,
        4,
        "BLACK",
        [1.0, 1.0, 1.0],
    );
    draw_backdrop_text(
        &mut pixels,
        width,
        height,
        width / 2 + 42,
        42,
        4,
        "WHITE",
        [0.0, 0.0, 0.0],
    );
    draw_backdrop_text(
        &mut pixels,
        width,
        height,
        42,
        height / 2 + 42,
        4,
        "DARK TEXT",
        [0.82, 0.90, 1.0],
    );
    draw_backdrop_text(
        &mut pixels,
        width,
        height,
        width / 2 + 42,
        height / 2 + 42,
        4,
        "LIGHT TEXT",
        [0.08, 0.08, 0.08],
    );
    pixels
}

fn make_checkerboard_backdrop(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let checker = ((x / 64) + (y / 64)) % 2 == 0;
            let fine = x % 64 < 2 || y % 64 < 2;
            let base: f32 = if checker { 0.92 } else { 0.08 };
            let edge: f32 = if fine { 0.18 } else { 0.0 };
            let value = if checker {
                (base - edge).max(0.0)
            } else {
                (base + edge).min(1.0)
            };
            write_pixel(&mut pixels, width, x, y, [value, value, value]);
        }
    }
    draw_backdrop_text(
        &mut pixels,
        width,
        height,
        48,
        48,
        3,
        "CHECKER CONTRAST",
        [0.0, 0.48, 1.0],
    );
    pixels
}

fn make_text_contrast_backdrop(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let fx = x as f32 / width.max(1) as f32;
            let fy = y as f32 / height.max(1) as f32;
            let band = ((fy * 10.0).floor() as u32) % 2 == 0;
            let base = if band {
                0.12 + fx * 0.28
            } else {
                0.88 - fx * 0.24
            };
            let tint = if band {
                [base * 0.8, base * 0.95, base * 1.25]
            } else {
                [base * 1.05, base, base * 0.86]
            };
            write_pixel(&mut pixels, width, x, y, tint);
        }
    }

    let lines = [
        ("WHITE TEXT ON DARK", [0.98, 0.98, 0.98]),
        ("BLACK TEXT ON LIGHT", [0.02, 0.02, 0.02]),
        ("LOW CONTRAST LIGHT", [0.68, 0.72, 0.76]),
        ("LOW CONTRAST DARK", [0.32, 0.36, 0.40]),
        ("SATURATED COLOR", [0.0, 0.52, 1.0]),
    ];
    for (index, (text, color)) in lines.iter().enumerate() {
        draw_backdrop_text(
            &mut pixels,
            width,
            height,
            54,
            64 + index as u32 * 86,
            4,
            text,
            *color,
        );
    }
    pixels
}

fn write_pixel(pixels: &mut [u8], width: u32, x: u32, y: u32, color: [f32; 3]) {
    let idx = ((y * width + x) * 4) as usize;
    pixels[idx] = (color[0].clamp(0.0, 1.0) * 255.0) as u8;
    pixels[idx + 1] = (color[1].clamp(0.0, 1.0) * 255.0) as u8;
    pixels[idx + 2] = (color[2].clamp(0.0, 1.0) * 255.0) as u8;
    pixels[idx + 3] = 255;
}

#[allow(clippy::too_many_arguments)]
fn draw_backdrop_text(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    scale: u32,
    text: &str,
    color: [f32; 3],
) {
    let mut cursor = x;
    for ch in text.chars() {
        if ch == ' ' {
            cursor += scale * 4;
            continue;
        }
        if let Some(glyph) = glyph_5x7(ch) {
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5u32 {
                    let mask = 1u8 << (4 - col);
                    if (*bits & mask) == 0 {
                        continue;
                    }
                    for py in 0..scale {
                        for px in 0..scale {
                            let dst_x = cursor + col * scale + px;
                            let dst_y = y + row as u32 * scale + py;
                            if dst_x < width && dst_y < height {
                                write_pixel(pixels, width, dst_x, dst_y, color);
                            }
                        }
                    }
                }
            }
            cursor += scale * 6;
        } else {
            cursor += scale * 3;
        }
    }
}

fn create_background_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    backdrop_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("studio background bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(backdrop_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

fn create_geometry_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    shapes: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("studio liquid glass geometry bind group"),
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

fn create_blur_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("studio liquid glass blur bind group"),
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

fn create_blur_pyramid_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    backdrop_view: &wgpu::TextureView,
    levels: &[(wgpu::Texture, wgpu::TextureView); 3],
    sampler: &wgpu::Sampler,
) -> ([wgpu::BindGroup; 3], [wgpu::BindGroup; 3]) {
    let down = [
        create_blur_bind_group(device, layout, backdrop_view, sampler),
        create_blur_bind_group(device, layout, &levels[0].1, sampler),
        create_blur_bind_group(device, layout, &levels[1].1, sampler),
    ];
    let up = [
        create_blur_bind_group(device, layout, &levels[2].1, sampler),
        create_blur_bind_group(device, layout, &levels[1].1, sampler),
        create_blur_bind_group(device, layout, &levels[0].1, sampler),
    ];
    (down, up)
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
        label: Some("studio liquid glass final bind group"),
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

fn create_ui_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("studio ui bind group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniforms.as_entire_binding(),
        }],
    })
}

fn create_fullscreen_pipeline(
    device: &wgpu::Device,
    label: &'static str,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
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
    })
}

fn create_ui_pipeline(
    device: &wgpu::Device,
    label: &'static str,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<UiInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[
                    wgpu::VertexAttribute {
                        offset: 0,
                        shader_location: 0,
                        format: wgpu::VertexFormat::Float32x2,
                    },
                    wgpu::VertexAttribute {
                        offset: 8,
                        shader_location: 1,
                        format: wgpu::VertexFormat::Float32x2,
                    },
                    wgpu::VertexAttribute {
                        offset: 16,
                        shader_location: 2,
                        format: wgpu::VertexFormat::Float32,
                    },
                    wgpu::VertexAttribute {
                        offset: 32,
                        shader_location: 3,
                        format: wgpu::VertexFormat::Float32x4,
                    },
                ],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(premultiplied_blend()),
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
    })
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

fn select_present_mode(available: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    if available.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else if available.contains(&wgpu::PresentMode::AutoVsync) {
        wgpu::PresentMode::AutoVsync
    } else {
        wgpu::PresentMode::Fifo
    }
}

fn select_alpha_mode(available: &[wgpu::CompositeAlphaMode]) -> wgpu::CompositeAlphaMode {
    let selected = if available.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
        wgpu::CompositeAlphaMode::PreMultiplied
    } else if available.contains(&wgpu::CompositeAlphaMode::PostMultiplied) {
        wgpu::CompositeAlphaMode::PostMultiplied
    } else if available.contains(&wgpu::CompositeAlphaMode::Auto) {
        wgpu::CompositeAlphaMode::Auto
    } else {
        wgpu::CompositeAlphaMode::Opaque
    };
    eprintln!(
        "studio surface alpha_mode: {:?} (available: {:?})",
        selected, available
    );
    selected
}

fn surface_preserves_alpha(alpha_mode: wgpu::CompositeAlphaMode) -> bool {
    matches!(
        alpha_mode,
        wgpu::CompositeAlphaMode::PreMultiplied | wgpu::CompositeAlphaMode::PostMultiplied
    )
}

fn default_backends() -> wgpu::Backends {
    #[cfg(windows)]
    {
        wgpu::Backends::DX12
    }
    #[cfg(not(windows))]
    {
        wgpu::Backends::DX12 | wgpu::Backends::VULKAN
    }
}

fn normalize2(v: [f32; 2]) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len <= f32::EPSILON {
        [1.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
}

fn nearly_equal(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.001
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = StudioApp::default();
    event_loop
        .run_app(&mut app)
        .expect("run liquid glass studio");
}
