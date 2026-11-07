//! The bottom capsule and gear share a completed-scene backdrop, unlike interaction lenses.
//! Keep its storage/uniforms separate: queued buffer writes are not draw snapshots.
use super::*;

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
        }
    }

    fn rebind(&mut self, device: &wgpu::Device, renderer: &LiquidGlassRenderer) {
        if self.size != renderer.texture_size {
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
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        profiler: &crate::renderer::gpu_profile::GpuProfilerState,
    ) {
        if !self.has_bottom_bar_glass() {
            return;
        }
        let lane = self.bottom_bar.as_ref().expect("checked bottom bar lane");
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
        // Re-evaluate animated shape coverage each frame, independently of the
        // interaction SDF and without merging nearby unrelated surfaces.
        let geometry_scope = profiler.begin_pass("bottom_bar_geometry_pass", encoder);
        {
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
            pass.set_bind_group(0, &lane.geometry_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        profiler.end(encoder, geometry_scope);
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
                let draw = |renderer: &LiquidGlassRenderer| {
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
                let fallback = draw(&renderer);
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
                let completed = draw(&renderer);
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
                let faded = draw(&renderer);
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
                assert_eq!(draw(&renderer), faded, "snapshot must remain stable");

                // Losing capture must not reuse an opaque completed snapshot;
                // recovery must refresh both controls with the new scene.
                renderer.has_captured_backdrop = false;
                renderer.params.presentation_alpha = 1.0;
                fill(&queue, &output, [0; 4]);
                let lost = draw(&renderer);
                for center in centers {
                    assert!((150..180).contains(&lost[center + 3]));
                }
                renderer.has_captured_backdrop = true;
                fill(&queue, &output, [0, 255, 0, 255]);
                renderer.prepare_bottom_bar_scene_blur(&device, &queue, &mut profiler, &output);
                fill(&queue, &output, [0; 4]);
                let recovered = draw(&renderer);
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
                    assert_eq!(draw(&renderer)[bridge + 3], 0);
                }
                renderer.set_bottom_bar_shapes(&device, &queue, &[capsule, budding_gear]);
                fill(&queue, &output, [0; 4]);
                assert!(
                    draw(&renderer)[bridge + 3] > 0,
                    "intended liquid bridge missing"
                );
                renderer.set_bottom_bar_shapes(&device, &queue, &[capsule, gear]);
            }
        });
    }
}
