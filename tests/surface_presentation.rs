//! Execute the production Rust resource/bind-group/pipeline path, not a test
//! reimplementation of the presentation shader. Requires a wgpu adapter (a
//! software Vulkan adapter is sufficient); missing adapters fail, never skip.
#[path = "../src/renderer/presentation.rs"]
mod presentation;

use presentation::SurfacePresentation;

fn decode_srgb(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
fn production_presentation_preserves_color_alpha_and_resize() {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("surface presentation regression requires a wgpu adapter");
        eprintln!("surface presentation adapter: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("create surface presentation test device");

        for format in [
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Bgra8Unorm,
        ] {
            use wgpu::{Backend::*, CompositeAlphaMode::*};
            // Exercise native compositor contracts independently of the GPU
            // running this readback test. Metal's legacy advertised Post mode
            // must store premultiplied pixels; genuine Post modes stay straight.
            for (backend, mode, expected_mode) in [
                (Dx12, PreMultiplied, PreMultiplied),
                (Vulkan, PostMultiplied, PostMultiplied),
                (Metal, PostMultiplied, PreMultiplied),
                (Metal, PreMultiplied, PreMultiplied),
            ] {
                let config = wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format,
                    width: 1,
                    height: 1,
                    present_mode: wgpu::PresentMode::Fifo,
                    desired_maximum_frame_latency: 2,
                    alpha_mode: mode,
                    view_formats: vec![],
                };
                let mut presentation = SurfacePresentation::new(&device, &config, backend);
                for (width, height) in [(1, 1), (3, 2)] {
                    // The second iteration exercises actual target/view/group
                    // recreation rather than constructing another renderer.
                    if width != 1 {
                        presentation.resize(&device, width, height);
                    }
                    assert_eq!(presentation.texture.width(), width);
                    assert_eq!(presentation.texture.height(), height);
                    for rgb in [[128, 128, 128], [64, 128, 224]] {
                        for alpha in [0.0, 0.25, 0.5, 1.0] {
                            render_and_assert(
                                &device,
                                &queue,
                                &presentation,
                                format,
                                expected_mode,
                                width,
                                height,
                                rgb,
                                alpha,
                            );
                        }
                    }
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn render_and_assert(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    presentation: &SurfacePresentation,
    format: wgpu::TextureFormat,
    mode: wgpu::CompositeAlphaMode,
    width: u32,
    height: u32,
    rgb: [u8; 3],
    alpha: f64,
) {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface presentation readback target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("surface presentation readback buffer"),
        size: u64::from(height) * 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let scene_view = presentation.view();
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("clear production scene with linear premultiplied color"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &scene_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: decode_srgb(f64::from(rgb[0]) / 255.0) * alpha,
                        g: decode_srgb(f64::from(rgb[1]) / 255.0) * alpha,
                        b: decode_srgb(f64::from(rgb[2]) / 255.0) * alpha,
                        a: alpha,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    presentation.present(&mut encoder, &target.create_view(&Default::default()));
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap();
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("wait for presentation readback");
    receiver.recv().unwrap().expect("map presentation readback");
    let bytes = readback.slice(..).get_mapped_range();
    let factor = if alpha == 0.0 || mode == wgpu::CompositeAlphaMode::PreMultiplied {
        alpha
    } else {
        1.0
    };
    let expected = [
        (f64::from(rgb[0]) * factor).round() as u8,
        (f64::from(rgb[1]) * factor).round() as u8,
        (f64::from(rgb[2]) * factor).round() as u8,
        (alpha * 255.0).round() as u8,
    ];
    for y in 0..height {
        for x in 0..width {
            let offset = (y * 256 + x * 4) as usize;
            let mut actual: [u8; 4] = bytes[offset..offset + 4].try_into().unwrap();
            if matches!(
                format,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
            ) {
                actual.swap(0, 2);
            }
            for channel in 0..4 {
                // Eight-bit linear targets lose precision before unpremultiplication.
                // Three encoded levels cover that quantization, not a gamma error.
                assert!(actual[channel].abs_diff(expected[channel]) <= 3,
                    "{format:?} {mode:?} {width}x{height}, rgb={rgb:?} alpha={alpha}: {actual:?} != {expected:?}");
            }
        }
    }
}
