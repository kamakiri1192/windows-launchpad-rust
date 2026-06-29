use std::{
    fs,
    num::NonZeroU64,
    path::{Path, PathBuf},
};

const LAUNCHPAD_UNIFORMS_SIZE: u64 = 40;
const GLASS_UNIFORMS_SIZE: u64 = 96;
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;
const GEOMETRY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const BLUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const TILE_ATTRIBS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x3, 3 => Float32];
const TILE_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 32,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &TILE_ATTRIBS,
};

const GLYPH_ATTRIBS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
const GLYPH_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 48,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &GLYPH_ATTRIBS,
};

const ICON_ATTRIBS: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
const ICON_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 32,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &ICON_ATTRIBS,
};

#[test]
fn all_wgsl_shaders_compile_with_wgpu_validation() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut shader_paths = Vec::new();
    collect_wgsl_files(&manifest_dir.join("src"), &mut shader_paths);
    collect_wgsl_files(&manifest_dir.join("assets"), &mut shader_paths);
    shader_paths.sort();

    assert!(
        !shader_paths.is_empty(),
        "expected at least one .wgsl shader under src/ or assets/"
    );

    pollster::block_on(async {
        let (device, _queue) = create_device().await;

        for path in shader_paths {
            let label = shader_label(&manifest_dir, &path);
            let _shader = create_shader_module_checked(&device, &label, &path).await;
        }
    });
}

#[test]
fn renderer_render_pipelines_compile_with_wgpu_validation() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    pollster::block_on(async {
        let (device, _queue) = create_device().await;

        create_launchpad_render_pipelines(&device, &manifest_dir).await;
        create_liquid_glass_render_pipelines(&device, &manifest_dir).await;
    });
}

async fn create_device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });

    let adapter = request_adapter(&instance).await;
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("wgsl validation device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        })
        .await
        .expect("failed to create wgpu device for WGSL validation")
}

async fn request_adapter(instance: &wgpu::Instance) -> wgpu::Adapter {
    if let Ok(adapter) = instance.request_adapter(&adapter_options(false)).await {
        return adapter;
    }

    instance
        .request_adapter(&adapter_options(true))
        .await
        .expect("failed to request a wgpu adapter for WGSL validation")
}

fn adapter_options(force_fallback_adapter: bool) -> wgpu::RequestAdapterOptions<'static, 'static> {
    wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter,
    }
}

async fn create_launchpad_render_pipelines(device: &wgpu::Device, manifest_dir: &Path) {
    let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test uniform bgl"),
        entries: &[uniform_entry(
            0,
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            LAUNCHPAD_UNIFORMS_SIZE,
        )],
    });
    let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test texture bgl"),
        entries: &[
            uniform_entry(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                LAUNCHPAD_UNIFORMS_SIZE,
            ),
            texture_entry(1, true),
            sampler_entry(2),
        ],
    });

    let tile_shader = create_shader_module_checked(
        device,
        "src/shader.wgsl",
        &manifest_dir.join("src/shader.wgsl"),
    )
    .await;
    let text_shader = create_shader_module_checked(
        device,
        "src/shader_text.wgsl",
        &manifest_dir.join("src/shader_text.wgsl"),
    )
    .await;
    let icon_shader = create_shader_module_checked(
        device,
        "src/shader_icon.wgsl",
        &manifest_dir.join("src/shader_icon.wgsl"),
    )
    .await;

    let tile_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("test tile pipeline layout"),
        bind_group_layouts: &[Some(&uniform_bgl)],
        immediate_size: 0,
    });
    let _tile_pipeline = create_render_pipeline_checked(
        device,
        "tile pipeline",
        &wgpu::RenderPipelineDescriptor {
            label: Some("test tile pipeline"),
            layout: Some(&tile_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &tile_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[TILE_LAYOUT],
            },
            fragment: Some(wgpu::FragmentState {
                module: &tile_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(color_target(
                    SURFACE_FORMAT,
                    Some(wgpu::BlendState::ALPHA_BLENDING),
                ))],
            }),
            primitive: triangle_list_no_cull(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        },
    )
    .await;

    let text_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("test text pipeline layout"),
        bind_group_layouts: &[Some(&texture_bgl)],
        immediate_size: 0,
    });
    let _text_pipeline = create_render_pipeline_checked(
        device,
        "text pipeline",
        &wgpu::RenderPipelineDescriptor {
            label: Some("test text pipeline"),
            layout: Some(&text_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &text_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[GLYPH_LAYOUT],
            },
            fragment: Some(wgpu::FragmentState {
                module: &text_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(color_target(
                    SURFACE_FORMAT,
                    Some(wgpu::BlendState::ALPHA_BLENDING),
                ))],
            }),
            primitive: triangle_list_no_cull(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        },
    )
    .await;

    let icon_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("test icon pipeline layout"),
        bind_group_layouts: &[Some(&texture_bgl)],
        immediate_size: 0,
    });
    let _icon_pipeline = create_render_pipeline_checked(
        device,
        "icon pipeline",
        &wgpu::RenderPipelineDescriptor {
            label: Some("test icon pipeline"),
            layout: Some(&icon_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &icon_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[ICON_LAYOUT],
            },
            fragment: Some(wgpu::FragmentState {
                module: &icon_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(color_target(
                    SURFACE_FORMAT,
                    Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                ))],
            }),
            primitive: triangle_list_no_cull(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        },
    )
    .await;
}

async fn create_liquid_glass_render_pipelines(device: &wgpu::Device, manifest_dir: &Path) {
    let geometry_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test liquid glass geometry bgl"),
        entries: &[
            uniform_entry(0, wgpu::ShaderStages::FRAGMENT, GLASS_UNIFORMS_SIZE),
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
    let blur_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test liquid glass blur bgl"),
        entries: &[texture_entry(0, true), sampler_entry(1)],
    });
    let final_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test liquid glass final bgl"),
        entries: &[
            uniform_entry(0, wgpu::ShaderStages::FRAGMENT, GLASS_UNIFORMS_SIZE),
            texture_entry(1, true),
            sampler_entry(2),
            texture_entry(3, false),
            texture_entry(4, true),
        ],
    });

    let geometry_shader = create_shader_module_checked(
        device,
        "assets/shaders/liquid_glass_geometry.wgsl",
        &manifest_dir.join("assets/shaders/liquid_glass_geometry.wgsl"),
    )
    .await;
    let final_shader = create_shader_module_checked(
        device,
        "assets/shaders/liquid_glass_final.wgsl",
        &manifest_dir.join("assets/shaders/liquid_glass_final.wgsl"),
    )
    .await;
    let blur_downsample_shader = create_shader_module_checked(
        device,
        "assets/shaders/liquid_glass_blur_downsample.wgsl",
        &manifest_dir.join("assets/shaders/liquid_glass_blur_downsample.wgsl"),
    )
    .await;
    let blur_upsample_shader = create_shader_module_checked(
        device,
        "assets/shaders/liquid_glass_blur_upsample.wgsl",
        &manifest_dir.join("assets/shaders/liquid_glass_blur_upsample.wgsl"),
    )
    .await;

    let geometry_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("test liquid glass geometry pipeline layout"),
        bind_group_layouts: &[Some(&geometry_bgl)],
        immediate_size: 0,
    });
    let _geometry_pipeline = create_fullscreen_render_pipeline_checked(
        device,
        "liquid glass geometry pipeline",
        "test liquid glass geometry pipeline",
        &geometry_pipeline_layout,
        &geometry_shader,
        GEOMETRY_FORMAT,
        None,
    )
    .await;

    let blur_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("test liquid glass blur pipeline layout"),
        bind_group_layouts: &[Some(&blur_bgl)],
        immediate_size: 0,
    });
    let _blur_downsample_pipeline = create_fullscreen_render_pipeline_checked(
        device,
        "liquid glass blur downsample pipeline",
        "test liquid glass blur downsample pipeline",
        &blur_pipeline_layout,
        &blur_downsample_shader,
        BLUR_FORMAT,
        None,
    )
    .await;
    let _blur_upsample_pipeline = create_fullscreen_render_pipeline_checked(
        device,
        "liquid glass blur upsample pipeline",
        "test liquid glass blur upsample pipeline",
        &blur_pipeline_layout,
        &blur_upsample_shader,
        BLUR_FORMAT,
        None,
    )
    .await;

    let final_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("test liquid glass final pipeline layout"),
        bind_group_layouts: &[Some(&final_bgl)],
        immediate_size: 0,
    });
    let _final_pipeline = create_fullscreen_render_pipeline_checked(
        device,
        "liquid glass final pipeline",
        "test liquid glass final pipeline",
        &final_pipeline_layout,
        &final_shader,
        SURFACE_FORMAT,
        Some(premultiplied_blend()),
    )
    .await;
}

async fn create_shader_module_checked(
    device: &wgpu::Device,
    label: &str,
    path: &Path,
) -> wgpu::ShaderModule {
    let source = fs::read_to_string(path).unwrap_or_else(|err| {
        panic!("failed to read WGSL shader {}: {err}", path.display());
    });

    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    assert_no_validation_error(error_scope, label).await;
    shader
}

async fn create_render_pipeline_checked(
    device: &wgpu::Device,
    label: &str,
    descriptor: &wgpu::RenderPipelineDescriptor<'_>,
) -> wgpu::RenderPipeline {
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let pipeline = device.create_render_pipeline(descriptor);
    assert_no_validation_error(error_scope, label).await;
    pipeline
}

async fn assert_no_validation_error(error_scope: wgpu::ErrorScopeGuard, label: &str) {
    if let Some(error) = error_scope.pop().await {
        panic!("{label} failed wgpu validation: {error}");
    }
}

async fn create_fullscreen_render_pipeline_checked(
    device: &wgpu::Device,
    error_label: &str,
    pipeline_label: &str,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    create_render_pipeline_checked(
        device,
        error_label,
        &wgpu::RenderPipelineDescriptor {
            label: Some(pipeline_label),
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
                targets: &[Some(color_target(format, blend))],
            }),
            primitive: triangle_list_no_cull(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        },
    )
    .await
}

fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    min_binding_size: u64,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(min_binding_size),
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

fn color_target(
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::ColorTargetState {
    wgpu::ColorTargetState {
        format,
        blend,
        write_mask: wgpu::ColorWrites::ALL,
    }
}

fn triangle_list_no_cull() -> wgpu::PrimitiveState {
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

fn collect_wgsl_files(dir: &Path, output: &mut Vec<PathBuf>) {
    if !dir.exists() {
        return;
    }

    for entry in fs::read_dir(dir).expect("failed to read shader directory") {
        let path = entry.expect("failed to read shader directory entry").path();

        if path.is_dir() {
            collect_wgsl_files(&path, output);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("wgsl") {
            output.push(path);
        }
    }
}

fn shader_label(manifest_dir: &Path, path: &Path) -> String {
    path.strip_prefix(manifest_dir)
        .unwrap_or(path)
        .display()
        .to_string()
}
