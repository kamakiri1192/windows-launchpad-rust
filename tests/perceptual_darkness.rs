//! CPU invariants and execution of the actual production WGSL color functions.
//! These tests do not certify capture-to-present latency or native appearance.
use wgpu::util::DeviceExt;

const FINAL: &str = include_str!("../assets/shaders/liquid_glass_final.wgsl");

fn decode(c: f32) -> f32 {
    let c = c.max(0.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(c: f32) -> f32 {
    let c = c.max(0.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn luminance(rgb: [f32; 3]) -> f32 {
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}

fn reference(rgb: [f32; 3], manual: f32, maximum: f32) -> [f32; 3] {
    let m = manual.clamp(0.0, 1.0);
    let mrgb = rgb.map(|c| c * (1.0 - m));
    if maximum <= 0.0 || m >= 1.0 {
        return mrgb;
    }
    let y = luminance(rgb.map(decode));
    if y <= 0.0 {
        return mrgb;
    }
    let white = decode(1.0 - maximum.clamp(0.0, 1.0));
    let gain = white / (white + (1.0 - white) * y.clamp(0.0, 1.0));
    let target = y * gain;
    let ml = mrgb.map(decode);
    let my = luminance(ml);
    if my <= target {
        return mrgb;
    }
    ml.map(|c| encode(c * target / my))
}

#[test]
fn local_curve_is_bounded_monotone_and_preserves_manual_off_contract() {
    for cap in [0.0_f32, 0.01, 0.16, 0.42, 0.85, 0.999, 1.0] {
        let mut previous = 0.0;
        for i in 0..=4096 {
            let rgb = [i as f32 / 4096.0; 3];
            let out = reference(rgb, 0.0, cap);
            assert!(out[0] >= previous - 0.000001);
            assert!(out[0] <= rgb[0] + 0.000001);
            previous = out[0];
            for manual in [0.0, 0.1, 0.5, 1.0] {
                assert_eq!(
                    reference(rgb, manual, 0.0),
                    rgb.map(|c| c * (1.0 - manual))
                );
                let adjusted = reference(rgb, manual, cap);
                let manual_y = luminance(rgb.map(|c| decode(c * (1.0 - manual))));
                assert!(luminance(adjusted.map(decode)) <= manual_y + 0.000001);
            }
        }
        assert!((reference([1.0; 3], 0.0, cap)[0] - (1.0 - cap)).abs() < 0.000001);
    }
}

#[test]
fn production_path_has_no_stats_or_drag_freeze_dependency() {
    let wrapper = FINAL
        .split_once("fn apply_glass_darkness(rgb:")
        .unwrap()
        .1
        .split_once("fn sample_geometry_height")
        .unwrap()
        .0;
    assert!(wrapper.contains("perceptual_darkness_color(rgb, u.glass_darkness, maximum)"));
    assert!(!wrapper.contains("auto_darkness."));
    assert!(!wrapper.contains("auto_glass_darkness()"));
    assert_eq!(FINAL.matches("= apply_glass_darkness(").count(), 2);
    let frame = include_str!("../src/app/frame.rs");
    assert!(frame.contains("defer_backdrop_capture: qa_enabled,"));
    assert!(!frame.contains("defer_backdrop_capture: dragging || qa_enabled"));
}

async fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("GPU parity requires a wgpu adapter; no silent CPU-only pass");
    adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("request test device")
}

fn readback(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Vec<u8> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
        sender.send(result).unwrap();
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    receiver.recv().unwrap().unwrap();
    let data = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    data
}

#[test]
fn production_wgsl_matches_independent_cpu_reference() {
    pollster::block_on(async {
        let (device, queue) = device().await;
        let start = FINAL.find("fn darkness_srgb_to_linear(").unwrap();
        let end = FINAL.find("// END PERCEPTUAL_DARKNESS").unwrap();
        let source = format!(
            "{}\n{}",
            &FINAL[start..end],
            r#"
@group(0) @binding(0) var<storage, read> inputs: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> outputs: array<vec4<f32>>;
@compute @workgroup_size(64)
fn check(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&inputs) / 2u { return; }
    let item = inputs[id.x * 2u];
    outputs[id.x] = vec4<f32>(perceptual_darkness_color(
        item.rgb, item.a, inputs[id.x * 2u + 1u].x), 1.0);
}
"#
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("production perceptual darkness helpers"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("perceptual darkness parity"),
            layout: None,
            module: &shader,
            entry_point: Some("check"),
            compilation_options: Default::default(),
            cache: None,
        });
        let mut colors = vec![
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.04045, 0.04046, 0.04044],
            [1.45, 0.5, 0.1],
        ];
        let mut seed = 20260928_u32;
        for i in 0..=256 {
            colors.push([i as f32 / 256.0; 3]);
            colors.push(std::array::from_fn(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                seed as f32 / u32::MAX as f32
            }));
        }
        let mut inputs: Vec<[f32; 4]> = Vec::new();
        let mut expected = Vec::new();
        for cap in [-0.1, 0.0, 0.0000001, 0.16, 0.42, 0.999, 1.0, 2.0] {
            for manual in [0.0, 0.1, 0.7, 1.0] {
                for rgb in &colors {
                    inputs.push([rgb[0], rgb[1], rgb[2], manual]);
                    inputs.push([cap, 0.0, 0.0, 0.0]);
                    expected.push(reference(*rgb, manual, cap));
                }
            }
        }
        let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&inputs),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let size = (expected.len() * 16) as u64;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups((expected.len() as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &staging, 0, size);
        queue.submit([encoder.finish()]);
        let bytes = readback(&device, &staging);
        for (index, (bytes, expected)) in bytes.chunks_exact(16).zip(expected.iter()).enumerate() {
            let actual: [f32; 4] = std::array::from_fn(|channel| {
                f32::from_le_bytes(bytes[channel * 4..channel * 4 + 4].try_into().unwrap())
            });
            for (channel, (got, want)) in actual[..3].iter().zip(expected).enumerate() {
                assert!(got.is_finite());
                assert!(
                    (got - want).abs() < 0.00004,
                    "case {index}, channel {channel}: {actual:?} != {expected:?}"
                );
            }
        }
    });
}
