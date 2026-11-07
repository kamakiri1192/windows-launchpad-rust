//! The bottom capsule and gear share a completed-scene backdrop, unlike interaction lenses.
//! Keep its storage/uniforms separate: queued buffer writes are not draw snapshots.
use super::*;

/// Smooth union can grow an outline by at most blend/4 per union. Include
/// that growth plus neighboring derivative/rim texels, and align to 2x2 quads.
/// Keep the full-size textures and fullscreen triangle's sampling coordinates.
fn draw_bounds(shapes: &[GlassShape], blend: f32, size: (u32, u32)) -> (u32, u32, u32, u32) {
    let padding = blend.max(0.0) * shapes.len() as f32 * 0.25 + 2.0;
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for shape in shapes {
        let b = shape.screen_bounds(0.0);
        bounds = [
            bounds[0].min(b[0]),
            bounds[1].min(b[1]),
            bounds[2].max(b[2]),
            bounds[3].max(b[3]),
        ];
    }
    if !bounds.iter().all(|v| v.is_finite()) || !padding.is_finite() {
        return (0, 0, size.0, size.1);
    }
    let x = ((bounds[0] - padding).max(0.0) as u32 / 2 * 2).min(size.0 - 1);
    let y = ((bounds[1] - padding).max(0.0) as u32 / 2 * 2).min(size.1 - 1);
    let right = (((bounds[2] + padding).clamp(0.0, size.0 as f32).ceil() as u32).div_ceil(2) * 2)
        .min(size.0);
    let bottom = (((bounds[3] + padding).clamp(0.0, size.1 as f32).ceil() as u32).div_ceil(2) * 2)
        .min(size.1);
    (
        x,
        y,
        right.saturating_sub(x).max(1),
        bottom.saturating_sub(y).max(1),
    )
}

pub(super) struct BottomBarLane {
    pub(super) shapes: Vec<GlassShape>,
    shape_buffer: wgpu::Buffer,
    shape_capacity: usize,
    uniforms: wgpu::Buffer,
    geometry_texture: wgpu::Texture,
    geometry_view: wgpu::TextureView,
    tint_texture: wgpu::Texture,
    tint_view: wgpu::TextureView,
    geometry_bind_group: wgpu::BindGroup,
    completed_bind_group: wgpu::BindGroup,
    fallback_bind_group: wgpu::BindGroup,
    size: (u32, u32),
    // Only the inputs read by liquid_glass_geometry.wgsl in this lane.
    // Shape changes and texture recreation invalidate this separately from
    // captured pixels, darkness, fade and material/blur uniforms.
    geometry_key: Option<[f32; 8]>,
    #[cfg(test)]
    geometry_passes: u64,
    #[cfg(test)]
    force_full_redraw: bool,
}

impl BottomBarLane {
    fn new(device: &wgpu::Device, renderer: &LiquidGlassRenderer) -> Self {
        let (width, height) = renderer.texture_size;
        let uniforms = create_uniform_buffer(
            device,
            "bottom bar glass uniforms",
            &uniforms_from_params(
                &renderer.params,
                renderer.debug,
                (width, height),
                0.0,
                0,
                0.0,
                0.0,
                renderer.backdrop_mapping,
            ),
        );
        let shape_buffer = create_shape_buffer_with_capacity(device, 1, "bottom bar glass shapes");
        let (geometry_texture, geometry_view) = create_geometry_texture(device, width, height);
        let (tint_texture, tint_view) =
            create_tint_texture(device, width, height, "bottom bar glass tint");
        let geometry_bind_group = create_geometry_bind_group(
            device,
            &renderer.geometry_bind_group_layout,
            &uniforms,
            &shape_buffer,
        );
        let bind = |backdrop, blur| {
            create_final_bind_group(
                device,
                &renderer.final_bind_group_layout,
                &uniforms,
                backdrop,
                &renderer.sampler,
                &geometry_view,
                &tint_view,
                blur,
                &renderer.auto_darkness_state_buffer,
                &renderer.auto_darkness_uniform_buffer,
            )
        };
        let completed_bind_group = bind(
            &renderer.context_menu_source_view,
            &renderer.context_menu_blur_view,
        );
        let fallback_bind_group = bind(&renderer.bound_backdrop_view, &renderer.blur_view);
        Self {
            shapes: Vec::new(),
            shape_buffer,
            shape_capacity: 1,
            uniforms,
            geometry_texture,
            geometry_view,
            tint_texture,
            tint_view,
            geometry_bind_group,
            completed_bind_group,
            fallback_bind_group,
            size: (width, height),
            geometry_key: None,
            #[cfg(test)]
            geometry_passes: 0,
            #[cfg(test)]
            force_full_redraw: false,
        }
    }

    fn rebind(&mut self, device: &wgpu::Device, renderer: &LiquidGlassRenderer) {
        if self.size != renderer.texture_size {
            self.geometry_key = None;
            self.size = renderer.texture_size;
            (self.geometry_texture, self.geometry_view) =
                create_geometry_texture(device, self.size.0, self.size.1);
            (self.tint_texture, self.tint_view) =
                create_tint_texture(device, self.size.0, self.size.1, "bottom bar glass tint");
        }
        let bind = |backdrop, blur| {
            create_final_bind_group(
                device,
                &renderer.final_bind_group_layout,
                &self.uniforms,
                backdrop,
                &renderer.sampler,
                &self.geometry_view,
                &self.tint_view,
                blur,
                &renderer.auto_darkness_state_buffer,
                &renderer.auto_darkness_uniform_buffer,
            )
        };
        let completed = bind(
            &renderer.context_menu_source_view,
            &renderer.context_menu_blur_view,
        );
        let fallback = bind(&renderer.bound_backdrop_view, &renderer.blur_view);
        self.completed_bind_group = completed;
        self.fallback_bind_group = fallback;
    }
}

impl LiquidGlassRenderer {
    pub fn set_bottom_bar_shapes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shapes: &[GlassShape],
    ) {
        if shapes.is_empty() && self.bottom_bar.is_none() {
            return;
        }
        let mut lane = self
            .bottom_bar
            .take()
            .unwrap_or_else(|| BottomBarLane::new(device, self));
        let changed = lane.shapes != shapes;
        if changed {
            lane.geometry_key = None;
            lane.shapes.clear();
            lane.shapes.extend_from_slice(shapes);
            if shapes.len() > lane.shape_capacity {
                lane.shape_capacity = next_shape_capacity(lane.shape_capacity, shapes.len());
                lane.shape_buffer = create_shape_buffer_with_capacity(
                    device,
                    lane.shape_capacity,
                    "bottom bar glass shapes",
                );
                lane.geometry_bind_group = create_geometry_bind_group(
                    device,
                    &self.geometry_bind_group_layout,
                    &lane.uniforms,
                    &lane.shape_buffer,
                );
            }
            if !shapes.is_empty() {
                queue.write_buffer(&lane.shape_buffer, 0, bytemuck::cast_slice(shapes));
            }
        }
        self.bottom_bar = Some(lane);
        if changed {
            self.bump_content_epoch();
        }
    }

    pub fn bottom_bar_shapes(&self) -> &[GlassShape] {
        self.bottom_bar
            .as_ref()
            .map_or(&[], |lane| lane.shapes.as_slice())
    }

    pub fn has_bottom_bar_glass(&self) -> bool {
        self.params.enabled
            && self
                .bottom_bar
                .as_ref()
                .is_some_and(|lane| !lane.shapes.is_empty())
    }

    pub(super) fn rebuild_bottom_bar_bind_groups(&mut self, device: &wgpu::Device) {
        if let Some(mut lane) = self.bottom_bar.take() {
            lane.rebind(device, self);
            self.bottom_bar = Some(lane);
        }
    }

    pub fn render_bottom_bar(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        profiler: &crate::renderer::gpu_profile::GpuProfilerState,
    ) {
        if !self.has_bottom_bar_glass() {
            return;
        }
        let lane = self.bottom_bar.as_mut().expect("checked bottom bar lane");
        let activation = lane
            .shapes
            .iter()
            .map(|shape| shape.activation)
            .fold(0.0, f32::max);
        let mut uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            self.texture_size,
            0.0,
            lane.shapes.len() as u32,
            0.0,
            activation,
            self.backdrop_mapping,
        );
        uniforms.backdrop_replacement = if self.has_captured_backdrop { 1.0 } else { 0.0 };
        queue.write_buffer(&lane.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let bounds = draw_bounds(&lane.shapes, uniforms.blend, lane.size);
        #[cfg(test)]
        let bounds = if lane.force_full_redraw {
            (0, 0, lane.size.0, lane.size.1)
        } else {
            bounds
        };
        let geometry_key = [
            uniforms.thickness,
            uniforms.refractive_index,
            uniforms.blend,
            uniforms.max_displacement,
            uniforms.glass_color[0],
            uniforms.glass_color[1],
            uniforms.glass_color[2],
            uniforms.glass_color[3],
        ];
        // Animation updates the shape buffer and invalidates this key. The
        // backdrop and final material still update at their original cadence.
        let reuse_geometry = lane.geometry_key == Some(geometry_key);
        #[cfg(test)]
        let reuse_geometry = reuse_geometry && !lane.force_full_redraw;
        if !reuse_geometry {
            let geometry_scope = profiler.begin_pass("bottom_bar_geometry_pass", encoder);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bottom bar glass geometry"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &lane.geometry_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &lane.tint_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: geometry_scope.timestamp_writes(),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.geometry_pipeline);
            pass.set_scissor_rect(bounds.0, bounds.1, bounds.2, bounds.3);
            pass.set_bind_group(0, &lane.geometry_bind_group, &[]);
            pass.draw(0..3, 0..1);
            drop(pass);
            profiler.end(encoder, geometry_scope);
            lane.geometry_key = Some(geometry_key);
            #[cfg(test)]
            {
                lane.geometry_passes += 1;
            }
        }
        let material_scope = profiler.begin_pass("bottom_bar_material_pass", encoder);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bottom bar glass completed-scene material"),
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
            timestamp_writes: material_scope.timestamp_writes(),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.final_pipeline);
        // Diagnostic views intentionally fill the viewport outside the glass.
        if self.debug.flags() == 0 {
            pass.set_scissor_rect(bounds.0, bounds.1, bounds.2, bounds.3);
        }
        pass.set_bind_group(
            0,
            if self.has_captured_backdrop {
                &lane.completed_bind_group
            } else {
                &lane.fallback_bind_group
            },
            &[],
        );
        pass.draw(0..3, 0..1);
        drop(pass);
        profiler.end(encoder, material_scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_geometry_cache_and_culling_match_full_redraw_after_shared_writes() {
        pollster::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
            let mut renderer = LiquidGlassRenderer::new(
                &device,
                &queue,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                160,
                96,
                &GridLayout::default(),
                Box::new(super::super::super::capture::FallbackCapture::new(
                    "grid cache test",
                )),
            );
            renderer.params.auto_darkness_enabled = false;
            for width in [160, 192, 640] {
                let height = if width == 640 { 480 } else { 96 };
                renderer.resize(&device, &queue, width, height);
                renderer.params.presentation[2] = 0.84;
                renderer.params.presentation_start_offset = 0.14;
                renderer.params.presentation_frame = [
                    width as f32 * 0.5,
                    height as f32 * 0.5,
                    width as f32 * 0.45,
                    height as f32 * 0.45,
                ];
                let output = target(&device, width, height);
                let view = output.create_view(&Default::default());
                let mut shapes = vec![GlassShape::clip_rounded_rect(
                    [width as f32 * 0.5, height as f32 * 0.5],
                    [width as f32 - 8.0, height as f32 - 8.0],
                    8.0,
                )];
                shapes.extend((0..30).map(|i| {
                    GlassShape::animated_scrolling_rounded_rect(
                        if width == 640 {
                            [40.25 + (i % 7) as f32 * 90.0, 45.5 + (i / 7) as f32 * 90.0]
                        } else {
                            [14.25 + i as f32 * 24.0, 42.5]
                        },
                        if width == 640 {
                            [50.5, 53.25]
                        } else {
                            [17.5, 21.25]
                        },
                        7.0,
                        i as f32 * 0.13,
                    )
                    .with_tint(Some([
                        i as f32 / 30.0,
                        0.4,
                        1.0 - i as f32 / 30.0,
                        0.2,
                    ]))
                }));
                renderer.set_grid_overlay_shapes(&device, &queue, &shapes);
                let draw = |r: &mut LiquidGlassRenderer, scroll, time| {
                    fill(&queue, &output, [0; 4]);
                    let mut encoder = device.create_command_encoder(&Default::default());
                    r.render_grid_overlay(&device, &queue, &mut encoder, &view, scroll, time);
                    queue.submit([encoder.finish()]);
                    pixels(&device, &queue, &output)
                };
                for (step, scroll) in [0.0, -100.25, -620.5, 1000.0, 0.0].into_iter().enumerate() {
                    renderer.params.thickness = 3.0 + step as f32;
                    renderer.params.blend = [2.0, 16.0, 32.0, 64.0, 0.0][step];
                    renderer.params.glass_color = [0.8, 0.4, 0.6, 0.12];
                    for progress in [1.0, 0.45, 1.0] {
                        renderer.params.presentation[0] = progress;
                        let time = step as f32 * 0.17;
                        let optimized = draw(&mut renderer, scroll, time);
                        assert_eq!(
                            draw(&mut renderer, scroll, time),
                            optimized,
                            "retained grid drift"
                        );
                        if progress == 1.0
                            && step == 0
                            && width != 640
                            && !cfg!(target_os = "windows")
                        {
                            assert!(renderer.active_grid_shapes.len() < shapes.len());
                        }
                        renderer.force_full_grid_redraw = true;
                        let reference = draw(&mut renderer, scroll, time);
                        // CPU/GPU sin/cos implementations may round differently.
                        // Require <= 1 RGBA8 unit; retained data itself is exact.
                        let delta = optimized
                            .iter()
                            .zip(&reference)
                            .map(|(&a, &b)| a.abs_diff(b))
                            .max()
                            .unwrap_or(0);
                        assert!(
                            delta <= 1,
                            "grid mismatch: {width} {scroll} {progress}, max delta {delta}"
                        );
                        renderer.force_full_grid_redraw = false;

                        renderer.set_drag_overlay_shapes(
                            &device,
                            &queue,
                            &[GlassShape::control_rounded_rect(
                                [45.0, 40.0],
                                [30.0, 25.0],
                                8.0,
                            )],
                        );
                        let mut encoder = device.create_command_encoder(&Default::default());
                        renderer.render_drag_overlay(&queue, &mut encoder, &view, time);
                        queue.submit([encoder.finish()]);
                        assert!(
                            renderer.grid_geometry_uniforms.is_none(),
                            "shared writer did not invalidate grid"
                        );
                        assert_eq!(
                            draw(&mut renderer, scroll, time),
                            optimized,
                            "stale shared geometry"
                        );
                    }
                }
            }
        });
    }

    /// Readback-free, synchronized microbenchmark. This is NOT window FPS:
    /// it isolates the shared capsule/gear checkpoint and draw on either OS.
    #[test]
    #[ignore = "explicit release GPU performance probe"]
    fn benchmark_bottom_bar_completed_scene() {
        pollster::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .unwrap();
            eprintln!("bar benchmark adapter: {:?}", adapter.get_info());
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let (width, height) = (2560, 1600);
            let mut renderer = LiquidGlassRenderer::new(
                &device,
                &queue,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                width,
                height,
                &GridLayout::default(),
                Box::new(super::super::super::capture::FallbackCapture::new(
                    "benchmark",
                )),
            );
            renderer.params.auto_darkness_enabled = false;
            renderer.has_captured_backdrop = true;
            let output = target(&device, width, height);
            let view = output.create_view(&Default::default());
            let mut profiler = crate::renderer::gpu_profile::GpuProfilerState::new(&device);
            for animate in [false, true] {
                let mut samples = Vec::new();
                for frame in 0..210 {
                    let start = std::time::Instant::now();
                    let offset = if animate {
                        (frame as f32 * 0.1).sin() * 32.0
                    } else {
                        0.0
                    };
                    renderer.set_bottom_bar_shapes(
                        &device,
                        &queue,
                        &[
                            GlassShape::control_rounded_rect([1280.0, 1460.0], [400.0, 90.0], 45.0),
                            GlassShape::control_rounded_rect(
                                [1550.0 + offset, 1460.0],
                                [90.0, 90.0],
                                45.0,
                            ),
                        ],
                    );
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: frame as f64 / 210.0,
                                        g: 0.2,
                                        b: 0.4,
                                        a: 1.0,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            ..Default::default()
                        });
                    }
                    queue.submit([encoder.finish()]);
                    renderer.prepare_bottom_bar_scene_blur(&device, &queue, &mut profiler, &output);
                    let mut encoder = device.create_command_encoder(&Default::default());
                    renderer.render_bottom_bar(&queue, &mut encoder, &view, &profiler);
                    let submission = queue.submit([encoder.finish()]);
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: Some(submission),
                            timeout: None,
                        })
                        .unwrap();
                    if frame >= 30 {
                        samples.push(start.elapsed().as_secs_f64() * 1000.0);
                    }
                }
                samples.sort_by(f64::total_cmp);
                eprintln!("bar benchmark: animated_shapes={animate} samples={} mean_ms={:.4} p50_ms={:.4} p95_ms={:.4}",
                    samples.len(), samples.iter().sum::<f64>() / samples.len() as f64,
                    samples[samples.len()/2], samples[samples.len()*95/100]);
            }
        });
    }

    fn pixels(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
        let width = texture.width();
        let height = texture.height();
        let stride = (width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bottom bar readback"),
            size: (stride * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            texture.size(),
        );
        queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = slice.get_mapped_range();
        let mut result = Vec::new();
        for row in bytes.chunks(stride as usize) {
            result.extend_from_slice(&row[..(width * 4) as usize]);
        }
        drop(bytes);
        buffer.unmap();
        result
    }

    fn target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bottom bar lower scene"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn fill(queue: &wgpu::Queue, texture: &wgpu::Texture, rgba: [u8; 4]) {
        queue.write_texture(
            texture.as_image_copy(),
            &rgba.repeat((texture.width() * texture.height()) as usize),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(texture.width() * 4),
                rows_per_image: Some(texture.height()),
            },
            texture.size(),
        );
    }

    #[test]
    fn actual_bottom_bar_lane_preserves_capsule_and_gear_backdrop_lifecycle() {
        pollster::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("bottom bar GPU regression requires adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .unwrap();
            let mut renderer = LiquidGlassRenderer::new(
                &device,
                &queue,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                32,
                32,
                &GridLayout::default(),
                Box::new(super::super::super::capture::FallbackCapture::new("test")),
            );
            renderer.params.auto_darkness_enabled = false;
            renderer.params.blur_radius = 0.0;
            renderer.params.thickness = 2.0;
            renderer.params.chromatic_aberration = 0.0;
            renderer.params.saturation = 1.0;
            renderer.params.glass_color = [0.0; 4];
            renderer.params.light_intensity = 0.0;
            renderer.params.ambient_strength = 0.0;
            let capsule = GlassShape::control_rounded_rect([24.0, 24.0], [32.0, 20.0], 10.0);
            let gear = GlassShape::control_rounded_rect([72.0, 24.0], [20.0, 20.0], 10.0);
            renderer.set_bottom_bar_shapes(&device, &queue, &[capsule, gear]);
            assert_eq!(renderer.bottom_bar_shapes().len(), 2);
            let mut profiler = crate::renderer::gpu_profile::GpuProfilerState::new(&device);
            for width in [96, 112] {
                renderer.params.blur_radius = 0.0;
                renderer.params.presentation_alpha = 1.0;
                renderer.resize(&device, &queue, width, 48);
                assert!(
                    !renderer.has_captured_backdrop,
                    "resize must restore no-capture policy"
                );
                let output = target(&device, width, 48);
                let view = output.create_view(&Default::default());
                let centers = [24, 72].map(|x| ((24 * width + x) * 4) as usize);
                let gap = ((24 * width + 50) * 4) as usize;
                let draw = |renderer: &mut LiquidGlassRenderer| {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    renderer.render_bottom_bar(
                        &queue,
                        &mut encoder,
                        &view,
                        &crate::renderer::gpu_profile::GpuProfilerState::new(&device),
                    );
                    queue.submit([encoder.finish()]);
                    pixels(&device, &queue, &output)
                };
                fill(&queue, &output, [0; 4]);
                let fallback = draw(&mut renderer);
                let geometry_passes = renderer.bottom_bar.as_ref().unwrap().geometry_passes;
                fill(&queue, &output, [0; 4]);
                assert_eq!(
                    draw(&mut renderer),
                    fallback,
                    "cached geometry changed pixels"
                );
                assert_eq!(
                    renderer.bottom_bar.as_ref().unwrap().geometry_passes,
                    geometry_passes,
                    "unchanged shapes must reuse geometry"
                );
                for center in centers {
                    assert!(
                        (150..180).contains(&fallback[center + 3]),
                        "fallback alpha at {center}: {}",
                        fallback[center + 3]
                    );
                }
                for outside in [0, gap] {
                    assert_eq!(&fallback[outside..outside + 4], &[0; 4]);
                }

                renderer.has_captured_backdrop = true;
                renderer.params.blur_radius = 16.0;
                // Foreground overlaps both surfaces. Desktop is black, so a
                // desktop-only replacement would erase the completed red scene.
                fill(&queue, &output, [255, 0, 0, 255]);
                renderer.prepare_bottom_bar_scene_blur(&device, &queue, &mut profiler, &output);
                let completed = draw(&mut renderer);
                for center in centers {
                    assert!(
                        completed[center] > 220
                            && completed[center + 1] < 80
                            && completed[center + 2] < 80,
                        "completed foreground was lost: {:?}",
                        &completed[center..center + 4]
                    );
                    assert_eq!(completed[center + 3], 255);
                }
                for outside in [0, gap] {
                    assert_eq!(&completed[outside..outside + 4], &[255, 0, 0, 255]);
                }
                // Both shapes must sample the same checkpoint, not themselves
                // or one another. Repeated draws with a cleared destination
                // must reproduce identical faded pixels without re-preparation.
                renderer.params.presentation_alpha = 0.5;
                fill(&queue, &output, [0; 4]);
                let faded = draw(&mut renderer);
                for center in centers {
                    assert!((127..=129).contains(&faded[center + 3]));
                    assert!(
                        faded[center] > 170 && faded[center + 1] < 80,
                        "faded glass lost captured foreground: {:?}",
                        &faded[center..center + 4]
                    );
                }
                for outside in [0, gap] {
                    assert_eq!(&faded[outside..outside + 4], &[0; 4]);
                }
                assert!(
                    faded
                        .chunks_exact(4)
                        .any(|pixel| pixel[3] > 0 && pixel[3] < 127),
                    "real geometry must preserve partial AA coverage during fade"
                );
                fill(&queue, &output, [0; 4]);
                assert_eq!(draw(&mut renderer), faded, "snapshot must remain stable");

                // Losing capture must not reuse an opaque completed snapshot;
                // recovery must refresh both controls with the new scene.
                renderer.has_captured_backdrop = false;
                renderer.params.presentation_alpha = 1.0;
                fill(&queue, &output, [0; 4]);
                let lost = draw(&mut renderer);
                for center in centers {
                    assert!((150..180).contains(&lost[center + 3]));
                }
                renderer.has_captured_backdrop = true;
                fill(&queue, &output, [0, 255, 0, 255]);
                renderer.prepare_bottom_bar_scene_blur(&device, &queue, &mut profiler, &output);
                fill(&queue, &output, [0; 4]);
                let recovered = draw(&mut renderer);
                assert_eq!(
                    renderer.bottom_bar.as_ref().unwrap().geometry_passes,
                    geometry_passes,
                    "changing backdrop, blur, capture policy and fade must not rebuild geometry"
                );
                for center in centers {
                    assert!(recovered[center] < 80 && recovered[center + 1] > 220);
                    assert_eq!(recovered[center + 3], 255);
                }

                // During edit entry, the gear intentionally buds from Done.
                // The bridge pixel is outside either standalone shape, yet
                // covered by their smooth union. Unrelated interaction shapes
                // never enter this lane.
                let budding_gear =
                    GlassShape::control_rounded_rect([54.0, 24.0], [20.0, 20.0], 10.0);
                let bridge = ((24 * width + 42) * 4) as usize;
                for shape in [capsule, budding_gear] {
                    renderer.set_bottom_bar_shapes(&device, &queue, &[shape]);
                    fill(&queue, &output, [0; 4]);
                    assert_eq!(draw(&mut renderer)[bridge + 3], 0);
                }
                renderer.set_bottom_bar_shapes(&device, &queue, &[capsule, budding_gear]);
                fill(&queue, &output, [0; 4]);
                assert!(
                    draw(&mut renderer)[bridge + 3] > 0,
                    "intended liquid bridge missing"
                );
                renderer.set_bottom_bar_shapes(&device, &queue, &[capsule, gear]);

                // Compare optimized drawing with a full-viewport, forced
                // geometry refresh using the same production shaders. Exercise
                // geometry uniforms independently of the shape buffer, and
                // fractional positions/sizes representing DPI/layout changes.
                let params = renderer.params;
                for step in 0..12 {
                    renderer.params.thickness = 2.0 + step as f32;
                    renderer.params.refractive_index = 1.1 + step as f32 * 0.05;
                    renderer.params.blend = step as f32 * 4.0;
                    renderer.params.glass_color = [0.7, 0.8, step as f32 / 12.0, 0.08];
                    let moved = GlassShape::control_rounded_rect(
                        [38.25 + step as f32 * 3.0, 23.75],
                        [20.5, 19.25],
                        9.5,
                    );
                    renderer.set_bottom_bar_shapes(&device, &queue, &[capsule, moved]);
                    fill(&queue, &output, [0; 4]);
                    let optimized = draw(&mut renderer);
                    let passes = renderer.bottom_bar.as_ref().unwrap().geometry_passes;
                    fill(&queue, &output, [0; 4]);
                    assert_eq!(draw(&mut renderer), optimized);
                    assert_eq!(
                        renderer.bottom_bar.as_ref().unwrap().geometry_passes,
                        passes
                    );
                    renderer.bottom_bar.as_mut().unwrap().force_full_redraw = true;
                    fill(&queue, &output, [0; 4]);
                    assert_eq!(
                        draw(&mut renderer),
                        optimized,
                        "full redraw differs at step {step}"
                    );
                    renderer.bottom_bar.as_mut().unwrap().force_full_redraw = false;

                    renderer.params.glass_color[0] += 0.05;
                    fill(&queue, &output, [0; 4]);
                    let recolored = draw(&mut renderer);
                    assert_eq!(
                        renderer.bottom_bar.as_ref().unwrap().geometry_passes,
                        passes + 2,
                        "global geometry tint must invalidate even without a shape change"
                    );
                    renderer.bottom_bar.as_mut().unwrap().force_full_redraw = true;
                    fill(&queue, &output, [0; 4]);
                    assert_eq!(draw(&mut renderer), recolored);
                    renderer.bottom_bar.as_mut().unwrap().force_full_redraw = false;
                }
                renderer.params = params;
                renderer.set_bottom_bar_shapes(&device, &queue, &[capsule, gear]);
            }
        });
    }
}
