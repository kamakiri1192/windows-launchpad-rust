use std::{
    fs,
    num::NonZeroU64,
    path::{Path, PathBuf},
};

const LAUNCHPAD_UNIFORMS_SIZE: u64 = 48;
const GLASS_UNIFORMS_SIZE: u64 = 128;
const BLUR_UNIFORMS_SIZE: u64 = 16;
const SCENE_FLATTEN_UNIFORMS_SIZE: u64 = 32;
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;
const GEOMETRY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const BLUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const AUTO_DARKNESS_UNIFORMS_SIZE: u64 = 20;
const AUTO_DARKNESS_STATE_SIZE: u64 = 96;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TestAutoDarknessUniforms {
    now: f32,
    mode: u32,
    flags: u32,
    max_darkness: f32,
    strategy: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct TestAutoDarknessState {
    from_darkness: f32,
    target_darkness: f32,
    transition_started_at: f32,
    filtered_coverage: f32,
    last_full_update_at: f32,
    initialized: u32,
    classification_count: u32,
    displayed_darkness: f32,
    probe_samples: [f32; 16],
}

#[test]
fn blur_downsample_preserves_constant_color_energy() {
    const NORMALIZATION: f32 = 1.0 / 8.0;
    let constant_output = (4.0 * 0.5 + 4.0 * 1.0 + 4.0 * 0.25 + 1.0) * NORMALIZATION;
    assert!((constant_output - 1.0).abs() < f32::EPSILON);

    let shader = include_str!("../assets/shaders/liquid_glass_blur_downsample.wgsl");
    assert!(shader.contains("const KERNEL_NORMALIZATION: f32 = 1.0 / 8.0;"));
    assert_eq!(shader.matches("KERNEL_NORMALIZATION").count(), 5);
}

#[test]
fn auto_darkness_uses_parallel_reduction_and_per_frame_interpolation() {
    let reduction = include_str!("../assets/shaders/liquid_glass_backdrop_stats.wgsl");
    assert!(reduction.contains("@compute @workgroup_size(16, 16, 1)"));
    assert!(reduction.contains("elapsed >= 0.125"));
    assert!(reduction.contains("probe_delta >= 0.035"));

    assert!(reduction.contains("state.displayed_darkness = interpolated_darkness(u.now);"));

    let final_shader = include_str!("../assets/shaders/liquid_glass_final.wgsl");
    assert!(final_shader.contains("return auto_darkness.displayed_darkness;"));
}

#[test]
fn auto_darkness_compute_respects_maximum_and_update_modes() {
    pollster::block_on(async {
        let (device, queue) = create_device().await;
        let (pipeline, bind_group_layout) = create_auto_darkness_test_pipeline(&device).await;
        let resources = create_auto_darkness_test_resources(&device, &bind_group_layout);
        write_test_backdrop(&queue, &resources.texture, 255);

        reset_test_auto_darkness_state(&queue, &resources.state);
        dispatch_test_auto_darkness(
            &device,
            &queue,
            &pipeline,
            &resources.bind_group,
            &resources.uniforms,
            TestAutoDarknessUniforms {
                now: 0.0,
                mode: 1,
                flags: 1,
                max_darkness: 0.42,
                strategy: 0,
            },
        );
        let initial = read_test_auto_darkness_state(&device, &queue, &resources.state);
        assert_eq!(initial.classification_count, 1);
        assert!((initial.target_darkness - 0.42).abs() < 0.0001);

        // Adaptive mode skips a stable frame before the 125 ms deadline, but
        // performs a full classification at the deadline.
        for now in [0.05, 0.126] {
            dispatch_test_auto_darkness(
                &device,
                &queue,
                &pipeline,
                &resources.bind_group,
                &resources.uniforms,
                TestAutoDarknessUniforms {
                    now,
                    mode: 1,
                    flags: 0,
                    max_darkness: 0.42,
                    strategy: 0,
                },
            );
        }
        let adaptive = read_test_auto_darkness_state(&device, &queue, &resources.state);
        assert_eq!(adaptive.classification_count, 2);

        // A coarse-probe change wakes adaptive mode immediately, before the
        // next 8 Hz deadline.
        write_test_backdrop(&queue, &resources.texture, 0);
        dispatch_test_auto_darkness(
            &device,
            &queue,
            &pipeline,
            &resources.bind_group,
            &resources.uniforms,
            TestAutoDarknessUniforms {
                now: 0.14,
                mode: 1,
                flags: 0,
                max_darkness: 0.42,
                strategy: 0,
            },
        );
        let changed = read_test_auto_darkness_state(&device, &queue, &resources.state);
        assert_eq!(changed.classification_count, 3);

        // Fixed mode ignores probe changes and runs only on its 8 Hz cadence.
        reset_test_auto_darkness_state(&queue, &resources.state);
        write_test_backdrop(&queue, &resources.texture, 255);
        for (index, now) in [0.0, 0.05, 0.126].into_iter().enumerate() {
            dispatch_test_auto_darkness(
                &device,
                &queue,
                &pipeline,
                &resources.bind_group,
                &resources.uniforms,
                TestAutoDarknessUniforms {
                    now,
                    mode: 2,
                    flags: u32::from(index == 0),
                    max_darkness: 0.42,
                    strategy: 0,
                },
            );
        }
        let fixed = read_test_auto_darkness_state(&device, &queue, &resources.state);
        assert_eq!(fixed.classification_count, 2);

        // Full-rate mode classifies every dispatched capture.
        reset_test_auto_darkness_state(&queue, &resources.state);
        write_test_backdrop(&queue, &resources.texture, 255);
        for (index, now) in [0.0, 0.01, 0.02].into_iter().enumerate() {
            dispatch_test_auto_darkness(
                &device,
                &queue,
                &pipeline,
                &resources.bind_group,
                &resources.uniforms,
                TestAutoDarknessUniforms {
                    now,
                    mode: 0,
                    flags: u32::from(index == 0),
                    max_darkness: 0.42,
                    strategy: 0,
                },
            );
        }
        let full_rate = read_test_auto_darkness_state(&device, &queue, &resources.state);
        assert_eq!(full_rate.classification_count, 3);
    });
}

struct AutoDarknessTestResources {
    texture: wgpu::Texture,
    uniforms: wgpu::Buffer,
    state: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

async fn create_auto_darkness_test_pipeline(
    device: &wgpu::Device,
) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("auto darkness execution test shader"),
        source: wgpu::ShaderSource::Wgsl(
            include_str!("../assets/shaders/liquid_glass_backdrop_stats.wgsl").into(),
        ),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("auto darkness execution test bgl"),
        entries: &[
            texture_entry_for(0, true, wgpu::ShaderStages::COMPUTE),
            sampler_entry_for(1, wgpu::ShaderStages::COMPUTE),
            uniform_entry(2, wgpu::ShaderStages::COMPUTE, AUTO_DARKNESS_UNIFORMS_SIZE),
            storage_entry(
                3,
                wgpu::ShaderStages::COMPUTE,
                false,
                AUTO_DARKNESS_STATE_SIZE,
            ),
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("auto darkness execution test pipeline layout"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("auto darkness execution test pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    (pipeline, bgl)
}

fn create_auto_darkness_test_resources(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> AutoDarknessTestResources {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("auto darkness execution test backdrop"),
        size: wgpu::Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("auto darkness execution test uniforms"),
        size: AUTO_DARKNESS_UNIFORMS_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let state = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("auto darkness execution test state"),
        size: AUTO_DARKNESS_STATE_SIZE,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("auto darkness execution test bind group"),
        layout: bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: state.as_entire_binding(),
            },
        ],
    });
    AutoDarknessTestResources {
        texture,
        uniforms,
        state,
        bind_group,
    }
}

#[test]
fn auto_darkness_strategies_map_white_black_and_gray_backdrops() {
    pollster::block_on(async {
        let (device, queue) = create_device().await;
        let (pipeline, bind_group_layout) = create_auto_darkness_test_pipeline(&device).await;
        let resources = create_auto_darkness_test_resources(&device, &bind_group_layout);

        // Hand-computed target_darkness for max_darkness = 0.42 on a uniform
        // backdrop (first classification uses filter_alpha = 1). Tuned and
        // perceptual constants are the darkness_qa harness winners (issue
        // #186, docs/qa/auto-darkness/results.md):
        //   white 255: coverage 1.0; strategy 2 quantile bin 7 center 0.9375
        //     -> all strategies saturate to 0.42.
        //   black 0:   coverage 0; strategy 2 quantile bin 0 center 0.0625 <
        //     L_TARGET -> all strategies stay clear at 0.0.
        //   gray 128:  strategy 0/1 white_score 0 (luma 0.502 < luma_lo);
        //     strategy 2 quantile bin 1 center 0.1875 < L_TARGET 0.20 ->
        //     0.0 for all strategies.
        //   gray 200:  strategy 0 white_score 0.4615 < coverage lo 0.55 ->
        //     0.0; strategy 1 white_score 0.6630, smoothstep(0.40, 0.70) =
        //     0.958 -> 0.402; strategy 2 quantile bin 4 center 0.5625 ->
        //     clamp(1 - 0.20/0.5625) = 0.644 -> 0.42.
        let cases: [(u8, [f32; 3]); 4] = [
            (255, [0.42, 0.42, 0.42]),
            (0, [0.0, 0.0, 0.0]),
            (128, [0.0, 0.0, 0.0]),
            (200, [0.0, 0.402, 0.42]),
        ];
        for strategy in 0..=2u32 {
            for &(value, expected) in &cases {
                reset_test_auto_darkness_state(&queue, &resources.state);
                write_test_backdrop(&queue, &resources.texture, value);
                dispatch_test_auto_darkness(
                    &device,
                    &queue,
                    &pipeline,
                    &resources.bind_group,
                    &resources.uniforms,
                    TestAutoDarknessUniforms {
                        now: 0.0,
                        mode: 0,
                        flags: 1,
                        max_darkness: 0.42,
                        strategy,
                    },
                );
                let result = read_test_auto_darkness_state(&device, &queue, &resources.state);
                assert_eq!(result.classification_count, 1);
                let expected = expected[strategy as usize];
                assert!(
                    (result.target_darkness - expected).abs() < 0.005,
                    "strategy {strategy}, backdrop {value}: target_darkness {} expected {}",
                    result.target_darkness,
                    expected,
                );
            }
        }
    });
}

#[test]
fn auto_darkness_strategy_2_probe_wakes_adaptive_classification() {
    pollster::block_on(async {
        let (device, queue) = create_device().await;
        let (pipeline, bind_group_layout) = create_auto_darkness_test_pipeline(&device).await;
        let resources = create_auto_darkness_test_resources(&device, &bind_group_layout);

        // Strategy 2 probes relative luminance, so a white -> black backdrop
        // moves the probe by 1.0 and wakes a full classification before the
        // 8 Hz (0.125 s) deadline.
        reset_test_auto_darkness_state(&queue, &resources.state);
        write_test_backdrop(&queue, &resources.texture, 255);
        dispatch_test_auto_darkness(
            &device,
            &queue,
            &pipeline,
            &resources.bind_group,
            &resources.uniforms,
            TestAutoDarknessUniforms {
                now: 0.0,
                mode: 1,
                flags: 1,
                max_darkness: 0.42,
                strategy: 2,
            },
        );
        let initial = read_test_auto_darkness_state(&device, &queue, &resources.state);
        assert_eq!(initial.classification_count, 1);

        // Unchanged backdrop before the deadline: probe delta 0, no wake.
        dispatch_test_auto_darkness(
            &device,
            &queue,
            &pipeline,
            &resources.bind_group,
            &resources.uniforms,
            TestAutoDarknessUniforms {
                now: 0.05,
                mode: 1,
                flags: 0,
                max_darkness: 0.42,
                strategy: 2,
            },
        );
        let stable = read_test_auto_darkness_state(&device, &queue, &resources.state);
        assert_eq!(stable.classification_count, 1);

        // White -> black flips the luminance probe, waking immediately.
        write_test_backdrop(&queue, &resources.texture, 0);
        dispatch_test_auto_darkness(
            &device,
            &queue,
            &pipeline,
            &resources.bind_group,
            &resources.uniforms,
            TestAutoDarknessUniforms {
                now: 0.10,
                mode: 1,
                flags: 0,
                max_darkness: 0.42,
                strategy: 2,
            },
        );
        let woken = read_test_auto_darkness_state(&device, &queue, &resources.state);
        assert_eq!(woken.classification_count, 2);
    });
}

#[test]
fn auto_darkness_strategy_switches_dispatch_cleanly() {
    pollster::block_on(async {
        let (device, queue) = create_device().await;
        let (pipeline, bind_group_layout) = create_auto_darkness_test_pipeline(&device).await;
        let resources = create_auto_darkness_test_resources(&device, &bind_group_layout);

        // The renderer resets GPU state on strategy change; here we verify
        // the shader itself survives consecutive dispatches across all three
        // strategies without shared-memory or validation issues.
        reset_test_auto_darkness_state(&queue, &resources.state);
        write_test_backdrop(&queue, &resources.texture, 255);
        for (index, strategy) in [0u32, 1, 2].into_iter().enumerate() {
            dispatch_test_auto_darkness(
                &device,
                &queue,
                &pipeline,
                &resources.bind_group,
                &resources.uniforms,
                TestAutoDarknessUniforms {
                    now: index as f32 * 0.01,
                    mode: 0,
                    flags: 1,
                    max_darkness: 0.42,
                    strategy,
                },
            );
        }
        let result = read_test_auto_darkness_state(&device, &queue, &resources.state);
        assert_eq!(result.classification_count, 3);
        assert!((0.0..=0.42).contains(&result.target_darkness));
        assert!(result.displayed_darkness.is_finite());
    });
}

fn dispatch_test_auto_darkness(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    uniform_buffer: &wgpu::Buffer,
    uniforms: TestAutoDarknessUniforms,
) {
    queue.write_buffer(uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("auto darkness execution test encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("auto darkness execution test pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    queue.submit(std::iter::once(encoder.finish()));
}

fn reset_test_auto_darkness_state(queue: &wgpu::Queue, state: &wgpu::Buffer) {
    queue.write_buffer(
        state,
        0,
        bytemuck::bytes_of(&TestAutoDarknessState::default()),
    );
}

fn read_test_auto_darkness_state(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    state: &wgpu::Buffer,
) -> TestAutoDarknessState {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("auto darkness execution test readback"),
        size: AUTO_DARKNESS_STATE_SIZE,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("auto darkness execution test readback encoder"),
    });
    encoder.copy_buffer_to_buffer(state, 0, &readback, 0, AUTO_DARKNESS_STATE_SIZE);
    queue.submit(std::iter::once(encoder.finish()));
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll auto darkness execution test readback");
    let mapped = slice.get_mapped_range();
    let result = bytemuck::pod_read_unaligned(&mapped);
    drop(mapped);
    readback.unmap();
    result
}

fn write_test_backdrop(queue: &wgpu::Queue, texture: &wgpu::Texture, value: u8) {
    let pixels = vec![value; 16 * 16 * 4];
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
            bytes_per_row: Some(16 * 4),
            rows_per_image: Some(16),
        },
        wgpu::Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 1,
        },
    );
}

const TILE_ATTRIBS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x3, 3 => Float32, 4 => Float32x4];
const TILE_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 48,
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

const ICON_ATTRIBS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x4,
    1 => Float32x4,
    2 => Float32x4,
    3 => Float32x4
];
const ICON_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: 64,
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
        entries: &[
            texture_entry(0, true),
            sampler_entry(1),
            uniform_entry(2, wgpu::ShaderStages::FRAGMENT, BLUR_UNIFORMS_SIZE),
        ],
    });
    let final_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test liquid glass final bgl"),
        entries: &[
            uniform_entry(0, wgpu::ShaderStages::FRAGMENT, GLASS_UNIFORMS_SIZE),
            texture_entry(1, true),
            sampler_entry(2),
            texture_entry(3, false),
            texture_entry(4, true),
            texture_entry(5, true),
            storage_entry(
                6,
                wgpu::ShaderStages::FRAGMENT,
                true,
                AUTO_DARKNESS_STATE_SIZE,
            ),
            uniform_entry(7, wgpu::ShaderStages::FRAGMENT, AUTO_DARKNESS_UNIFORMS_SIZE),
        ],
    });
    let backdrop_stats_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test liquid glass backdrop stats bgl"),
        entries: &[
            texture_entry_for(0, true, wgpu::ShaderStages::COMPUTE),
            sampler_entry_for(1, wgpu::ShaderStages::COMPUTE),
            uniform_entry(2, wgpu::ShaderStages::COMPUTE, AUTO_DARKNESS_UNIFORMS_SIZE),
            storage_entry(
                3,
                wgpu::ShaderStages::COMPUTE,
                false,
                AUTO_DARKNESS_STATE_SIZE,
            ),
        ],
    });
    let scene_flatten_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test liquid glass scene flatten bgl"),
        entries: &[
            texture_entry(0, true),
            texture_entry(1, true),
            sampler_entry(2),
            uniform_entry(3, wgpu::ShaderStages::FRAGMENT, SCENE_FLATTEN_UNIFORMS_SIZE),
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
    let scene_flatten_shader = create_shader_module_checked(
        device,
        "assets/shaders/liquid_glass_scene_flatten.wgsl",
        &manifest_dir.join("assets/shaders/liquid_glass_scene_flatten.wgsl"),
    )
    .await;
    let backdrop_stats_shader = create_shader_module_checked(
        device,
        "assets/shaders/liquid_glass_backdrop_stats.wgsl",
        &manifest_dir.join("assets/shaders/liquid_glass_backdrop_stats.wgsl"),
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

    let scene_flatten_pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("test liquid glass scene flatten pipeline layout"),
            bind_group_layouts: &[Some(&scene_flatten_bgl)],
            immediate_size: 0,
        });
    let _scene_flatten_pipeline = create_fullscreen_render_pipeline_checked(
        device,
        "liquid glass scene flatten pipeline",
        "test liquid glass scene flatten pipeline",
        &scene_flatten_pipeline_layout,
        &scene_flatten_shader,
        BLUR_FORMAT,
        None,
    )
    .await;

    let backdrop_stats_pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("test liquid glass backdrop stats pipeline layout"),
            bind_group_layouts: &[Some(&backdrop_stats_bgl)],
            immediate_size: 0,
        });
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _backdrop_stats_pipeline =
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("test liquid glass backdrop stats pipeline"),
            layout: Some(&backdrop_stats_pipeline_layout),
            module: &backdrop_stats_shader,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
    assert_no_validation_error(error_scope, "liquid glass backdrop stats pipeline").await;

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
    texture_entry_for(binding, filterable, wgpu::ShaderStages::FRAGMENT)
}

fn texture_entry_for(
    binding: u32,
    filterable: bool,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    sampler_entry_for(binding, wgpu::ShaderStages::FRAGMENT)
}

fn sampler_entry_for(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn storage_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    read_only: bool,
    min_binding_size: u64,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(min_binding_size),
        },
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
