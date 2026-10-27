//! Liquid Glass frame and lane render-pass orchestration.

use crate::renderer::gpu_profile::GpuProfilerState;

use super::*;

impl LiquidGlassRenderer {
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        profiler: &mut GpuProfilerState,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        scroll_x: f32,
        defer_backdrop_capture: bool,
    ) {
        if !self.params.enabled || self.base_shapes.is_empty() {
            return;
        }

        self.refresh_active_base_shapes(queue, scroll_x);
        if self.shape_count == 0 {
            return;
        }

        let render_started = Instant::now();
        let (width, height) = self.texture_size;

        let mut captured = false;
        let mut capture_time = Duration::ZERO;
        let mut upload_time = Duration::ZERO;
        if self.should_capture(defer_backdrop_capture) {
            let capture_region = self.planned_capture_region(scroll_x);
            self.capture.set_capture_region(capture_region);
            let capture_started = Instant::now();
            if let Some(gpu_frame) = self.capture.latest_frame_texture(device, width, height) {
                capture_time = capture_started.elapsed();
                match gpu_frame {
                    GpuCaptureFrame::New { texture, view } => {
                        self.backdrop_mapping = BackdropMapping::full(width, height);
                        if !self.using_gpu_backdrop {
                            eprintln!("liquid glass capture path: GPU shared texture");
                        }
                        self.bind_backdrop_view(device, &view);
                        self.gpu_backdrop_texture = Some(texture);
                        self.using_gpu_backdrop = true;
                        self.gpu_backdrop_is_copy_target = false;
                        captured = true;
                    }
                    GpuCaptureFrame::Ephemeral(frame) => {
                        let upload_started = Instant::now();
                        captured = self.copy_ephemeral_gpu_backdrop(device, queue, profiler, frame);
                        upload_time = upload_started.elapsed();
                    }
                    GpuCaptureFrame::Updated => {
                        self.backdrop_mapping = BackdropMapping::full(width, height);
                        captured = true;
                    }
                }
            } else if let Some(frame) = self.capture.latest_frame_rgba(width, height) {
                capture_time = capture_started.elapsed();
                let upload_started = Instant::now();
                let was_using_gpu = self.using_gpu_backdrop;
                if self.configure_cpu_backdrop(device, &frame) {
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &self.backdrop_texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &frame.pixels,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(frame.width * 4),
                            rows_per_image: Some(frame.height),
                        },
                        wgpu::Extent3d {
                            width: frame.width,
                            height: frame.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    upload_time = upload_started.elapsed();
                    if was_using_gpu {
                        eprintln!("liquid glass capture path: CPU texture upload fallback");
                    }
                    captured = true;
                }
            } else {
                capture_time = capture_started.elapsed();
            }
            self.last_capture_at = Some(Instant::now());
        }
        if captured {
            self.has_captured_backdrop = true;
            self.capture_sequence = self.capture_sequence.wrapping_add(1);
        }
        let next_status = self.capture.status();
        if next_status != self.capture_status {
            log_capture_status(&next_status);
            self.capture_status = next_status;
        }

        if self.params.auto_darkness_enabled {
            self.update_auto_darkness_uniforms(queue);
            if captured || self.backdrop_stats_dirty {
                // The 16x16 reduction is encoded into the caller's frame encoder.
                // This preserves full-rate classification without adding a queue
                // submission between capture and the visible glass pass.
                self.encode_backdrop_stats(encoder);
            }
        }

        let mut uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            scroll_x,
            self.shape_count,
            0.0,
            0.0,
            self.backdrop_mapping,
        );
        // The sampled backdrop is already in this pixel. Alpha is coverage,
        // not material transparency; otherwise the OS adds sharp desktop again.
        uniforms.backdrop_replacement = if self.has_captured_backdrop { 1.0 } else { 0.0 };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let global_blur = self.blur_profile(self.params.blur_radius);
        let refreshed_global_blur = should_refresh_blur(self.blur_dirty, captured)
            && !self.debug.disable_blur
            && global_blur.level_count > 0;

        // Each blur pass runs in its OWN command encoder. wgpu groups all
        // passes in a single encoder into one "usage scope", and a texture
        // may not be both RESOURCE and COLOR_TARGET within that scope. Since a
        // dual-Kawase pyramid feeds each pass's output into the next pass's
        // input (L2 is written by down then read by up), we must split scopes
        // by submitting one encoder per pass.
        // Downsample: backdrop -> L1 -> ... -> L(k-1). down[i] reads the
        // backdrop for i==0 else levels[i-1], and writes levels[i].
        let mut blur_commands = Vec::with_capacity(global_blur.level_count * 2);
        if refreshed_global_blur {
            queue.write_buffer(
                &self.blur_uniform_buffer,
                0,
                bytemuck::bytes_of(&BlurUniforms {
                    sample_scale: global_blur.sample_scale,
                    _pad: [0.0; 3],
                }),
            );
            blur_commands.extend(encode_blur_profile(
                device,
                profiler,
                &self.blur_downsample_pipeline,
                &self.blur_upsample_pipeline,
                &self.blur_levels,
                &self.blur_view,
                &self.blur_down_bind_groups,
                &self.blur_up_bind_groups,
                global_blur.level_count,
                "liquid glass global blur",
                GLOBAL_BLUR_DOWN_SCOPES,
                GLOBAL_BLUR_UP_SCOPES,
            ));
        }
        if !blur_commands.is_empty() {
            queue.submit(blur_commands);
        }
        if refreshed_global_blur {
            self.blur_dirty = false;
        }
        let refreshed_blur = refreshed_global_blur;

        let geometry_key = self.geometry_key(scroll_x);
        let refreshed_geometry = self.last_geometry_key != Some(geometry_key);
        if refreshed_geometry {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass geometry pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.geometry_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.geometry_tint_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
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

        // Opt-in debug readback only. The tiny copy is mapped asynchronously
        // after submission, so displaying the value never waits on the GPU.
        self.encode_auto_darkness_debug_readback(encoder);

        let _ = device;
        self.stats.record(
            captured,
            refreshed_blur,
            refreshed_geometry.then_some(self.shape_count),
            capture_time,
            upload_time,
            render_started.elapsed(),
        );
    }

    /// Render glass nested inside the grid page after opaque tile fills and
    /// before icons/text. A separate SDF field keeps inner boundaries from
    /// being swallowed by the page frame's union.
    pub fn render_grid_overlay(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        scroll_x: f32,
        time: f32,
    ) {
        if !self.params.enabled || self.grid_overlay_shape_count == 0 {
            return;
        }

        let (width, height) = self.texture_size;
        let mut uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            scroll_x,
            self.grid_overlay_shape_count,
            time,
            0.0,
            self.backdrop_mapping,
        );
        // Presentation is an extra transform on the existing scrolling/edit
        // geometry, scoped to closed folders. Modal/drag/control lanes retain
        // their ordinary transform and only share the window fade.
        uniforms.presentation_grid = 1;
        queue.write_buffer(
            &self.grid_overlay_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass grid overlay geometry pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_geometry_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_tint_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.geometry_pipeline);
            pass.set_bind_group(0, &self.grid_overlay_geometry_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass grid overlay final pass"),
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
            pass.set_bind_group(0, &self.grid_overlay_final_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Render the lifted folder's Liquid Glass after normal grid content and
    /// badges, but immediately before the dragged tile/icon pass. This lane
    /// owns a separate SDF field, so it cannot merge with closed folders in
    /// the grid-overlay lane.
    pub fn render_drag_overlay(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        time: f32,
    ) {
        if !self.params.enabled || self.drag_overlay_shape_count == 0 {
            return;
        }

        let (width, height) = self.texture_size;
        let uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            0.0,
            self.drag_overlay_shape_count,
            time,
            0.0,
            self.backdrop_mapping,
        );
        queue.write_buffer(
            &self.drag_overlay_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass drag overlay geometry pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_geometry_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_tint_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.geometry_pipeline);
            pass.set_bind_group(0, &self.drag_overlay_geometry_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass drag overlay final pass"),
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
            pass.set_bind_group(0, &self.drag_overlay_final_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub fn render_badges(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        scroll_x: f32,
        time: f32,
    ) {
        if !self.params.enabled || self.badge_shapes.is_empty() {
            return;
        }

        self.refresh_active_badge_shapes(queue, scroll_x);
        // A clip-only shape cannot produce any glass by itself.
        if self.badge_shape_count <= 1 {
            return;
        }

        let (width, height) = self.texture_size;
        let uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            scroll_x,
            self.badge_shape_count,
            time,
            0.0,
            self.backdrop_mapping,
        );
        queue.write_buffer(&self.badge_uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass badge geometry pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_geometry_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_tint_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.badge_geometry_pipeline);
            pass.set_bind_group(0, &self.badge_geometry_bind_group, &[]);
            // Index zero is the page clip; each remaining instance is one
            // tightly bounded badge quad.
            pass.draw(0..6, 1..self.badge_shape_count);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass badge final pass"),
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
            pass.set_bind_group(0, &self.badge_final_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub fn render_modal_badges(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        time: f32,
    ) {
        if !self.params.enabled || self.modal_badge_shape_count == 0 {
            return;
        }

        let (width, height) = self.texture_size;
        let uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            0.0,
            self.modal_badge_shape_count,
            time,
            0.0,
            self.backdrop_mapping,
        );
        queue.write_buffer(
            &self.modal_badge_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass modal badge geometry pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_geometry_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_tint_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.geometry_pipeline);
            pass.set_bind_group(0, &self.modal_badge_geometry_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass modal badge final pass"),
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
            pass.set_bind_group(0, &self.modal_badge_final_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub fn render_control(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        if !self.params.enabled {
            return;
        }
        if self.control_shape_count == 0 {
            return;
        }

        let (width, height) = self.texture_size;
        // Compute max activation from control shapes for interactive glass.
        let control_activation = self
            .control_shapes
            .iter()
            .map(|s| s.activation)
            .fold(0.0f32, f32::max);
        let uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            0.0,
            self.control_shape_count,
            0.0,
            control_activation,
            self.backdrop_mapping,
        );
        queue.write_buffer(
            &self.control_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        // Caching: skip geometry pass when shapes + params are unchanged,
        // matching the base pass geometry_key pattern.
        let current_key = self
            .last_control_geometry_key
            .wrapping_add((width as u64) << 32 | height as u64)
            .wrapping_add(self.params.thickness.to_bits() as u64)
            .wrapping_add(self.params.refractive_index.to_bits() as u64)
            .wrapping_add(self.params.blend.to_bits() as u64);
        let geometry_changed = current_key != self.control_geometry_rendered_key;
        if geometry_changed {
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("liquid glass control geometry pass"),
                    color_attachments: &[
                        Some(wgpu::RenderPassColorAttachment {
                            view: &self.control_geometry_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                        Some(wgpu::RenderPassColorAttachment {
                            view: &self.control_tint_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                    ],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.geometry_pipeline);
                pass.set_bind_group(0, &self.control_geometry_bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            self.control_geometry_rendered_key = current_key;
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass control final pass"),
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
            pass.set_bind_group(0, &self.control_final_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Render settings-panel control glass after the panel surface. The
    /// settings thumb lenses use a completed scene that includes the current
    /// toggle track, so the green/white underlay remains visible through the
    /// transparent lens.
    pub fn render_settings_controls(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        if !self.params.enabled || self.settings_control_shape_count == 0 {
            return;
        }

        let (width, height) = self.texture_size;
        let control_activation = self
            .settings_control_shapes
            .iter()
            .map(|shape| shape.activation)
            .fold(0.0f32, f32::max);
        let mut uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            0.0,
            self.settings_control_shape_count,
            0.0,
            control_activation,
            self.backdrop_mapping,
        );
        // A pressed toggle is a transparent lens over the already-composited
        // track. Do not feed it the panel's completed-scene blur: that turns
        // the green/white track into a milky white pill and hides the colour
        // change the lens is meant to reveal.
        uniforms.blur_radius = 0.0;
        // Scale the rim to this small lens instead of using page-panel optics.
        uniforms.thickness = self
            .settings_control_shapes
            .iter()
            .map(|shape| shape.size[1] * 0.12)
            .fold(f32::INFINITY, f32::min)
            .clamp(1.0, 6.0);
        uniforms.max_displacement = uniforms.thickness * 10.0;
        // Settings Toggle lenses are interactive controls, not part of the
        // panel's replacement/darkness treatment. Preserve their transparent
        // backdrop so the captured track and desktop remain visible through it.
        uniforms.backdrop_replacement = 0.0;
        uniforms.auto_darkness_enabled = 0;
        uniforms.glass_darkness = 0.0;
        queue.write_buffer(
            &self.settings_control_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        if self.settings_control_geometry_dirty {
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("liquid glass settings control geometry pass"),
                    color_attachments: &[
                        Some(wgpu::RenderPassColorAttachment {
                            view: &self.settings_control_geometry_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                        Some(wgpu::RenderPassColorAttachment {
                            view: &self.settings_control_tint_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        }),
                    ],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.geometry_pipeline);
                pass.set_bind_group(0, &self.settings_control_geometry_bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
            self.settings_control_geometry_dirty = false;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("liquid glass settings control final pass"),
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
        pass.set_pipeline(&self.settings_control_final_pipeline);
        pass.set_bind_group(0, &self.settings_control_final_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Render the settings/folder modal glass from the shared modal-shape
    /// buffer. Settings uses the completed-scene bind group; folders use their
    /// ordinary backdrop path so they do not sample an unprepared settings
    /// capture.
    pub fn render_settings_panel(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        if !self.params.enabled {
            return;
        }
        if self.settings_panel_shape_count == 0 {
            return;
        }

        let (width, height) = self.texture_size;
        let mut uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            0.0,
            self.settings_panel_shape_count,
            0.0,
            0.0,
            self.backdrop_mapping,
        );
        if self.settings_panel_completed_scene_enabled && !self.debug.disable_blur {
            uniforms.blur_radius = self
                .settings_panel_blur_radius
                .unwrap_or(self.params.blur_radius);
        }
        if self.settings_panel_completed_scene_enabled {
            uniforms.backdrop_replacement = self.settings_panel_backdrop_replacement;
            uniforms.glass_darkness = self.settings_panel_glass_darkness;
            // The completed scene already contains lower-layer darkening.
            // Keep folder panels on the inherited automatic-darkness path.
            uniforms.auto_darkness_enabled = 0;
        }
        queue.write_buffer(
            &self.settings_panel_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass settings panel geometry pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_geometry_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_tint_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.geometry_pipeline);
            pass.set_bind_group(0, &self.settings_panel_geometry_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass settings panel final pass"),
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
            let final_bind_group = if self.settings_panel_completed_scene_enabled {
                &self.settings_panel_final_bind_group
            } else {
                &self.modal_final_bind_group
            };
            pass.set_bind_group(0, final_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    pub fn has_settings_control_glass(&self) -> bool {
        self.params.enabled && self.settings_control_shape_count > 0
    }

    pub fn has_context_menu_glass(&self) -> bool {
        self.params.enabled && self.context_menu_shape_count > 0
    }

    pub fn has_settings_panel_glass(&self) -> bool {
        self.params.enabled
            && self.settings_panel_completed_scene_enabled
            && self.settings_panel_shape_count > 0
    }

    /// Prepare only the bottom capsule from the completed lower in-app scene.
    /// Interaction/gear glass remains on its existing translucent lane.
    pub fn prepare_bottom_bar_scene_blur(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        profiler: &mut GpuProfilerState,
        pre_bar_scene: &wgpu::Texture,
    ) {
        if !self.has_bottom_bar_glass() || !self.has_captured_backdrop {
            return;
        }
        let bounds = completed_scene_screen_bounds(
            self.bottom_bar_shapes(),
            self.texture_size.0,
            self.texture_size.1,
        );
        self.prepare_completed_scene_blur(
            device,
            queue,
            profiler,
            pre_bar_scene,
            self.params.blur_radius,
            bounds,
        );
    }

    /// Capture every layer already rendered to the transparent swapchain,
    /// flatten it over the real desktop capture, then build the menu's blur.
    /// This must run after modal content and before `render_context_menu_glass`.
    pub fn prepare_context_menu_scene_blur(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        profiler: &mut GpuProfilerState,
        pre_menu_scene: &wgpu::Texture,
    ) {
        if !self.has_context_menu_glass() {
            return;
        }
        let bounds = completed_scene_screen_bounds(
            &self.context_menu_shapes,
            self.texture_size.0,
            self.texture_size.1,
        );
        self.prepare_completed_scene_blur(
            device,
            queue,
            profiler,
            pre_menu_scene,
            self.context_menu_blur_radius
                .unwrap_or(self.params.blur_radius),
            bounds,
        );
    }

    /// Capture the completed scene immediately below the settings surface and
    /// build the same full-resolution blur used by the context menu.
    pub fn prepare_settings_panel_scene_blur(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        profiler: &mut GpuProfilerState,
        pre_settings_scene: &wgpu::Texture,
    ) {
        if !self.has_settings_panel_glass() {
            return;
        }
        let bounds = completed_scene_screen_bounds(
            &self.settings_panel_shapes,
            self.texture_size.0,
            self.texture_size.1,
        );
        self.prepare_completed_scene_blur(
            device,
            queue,
            profiler,
            pre_settings_scene,
            self.settings_panel_blur_radius
                .unwrap_or(self.params.blur_radius),
            bounds,
        );
    }

    /// Capture the scene immediately below the interactive settings thumbs.
    /// This checkpoint is deliberately later than the settings underlay pass,
    /// so the switch track itself participates in the lens backdrop.
    pub fn prepare_settings_control_scene(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        pre_settings_control_scene: &wgpu::Texture,
    ) {
        if !self.has_settings_control_glass() {
            return;
        }
        // The settings/menu completed-scene cache must never be overwritten
        // by the post-track checkpoint. No blur/flatten pass is needed here.
        encoder.copy_texture_to_texture(
            pre_settings_control_scene.as_image_copy(),
            self.settings_control_scene_texture.as_image_copy(),
            wgpu::Extent3d {
                width: self.texture_size.0,
                height: self.texture_size.1,
                depth_or_array_layers: 1,
            },
        );
    }

    fn prepare_completed_scene_blur(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        profiler: &mut GpuProfilerState,
        pre_scene: &wgpu::Texture,
        blur_radius: f32,
        screen_bounds: Option<[f32; 4]>,
    ) {
        let (viewport_width, viewport_height) = self.texture_size;
        let roi = self.completed_scene_roi(screen_bounds, blur_radius);
        queue.write_buffer(
            &self.context_menu_flatten_uniform_buffer,
            0,
            bytemuck::bytes_of(&SceneFlattenUniforms {
                viewport: [viewport_width as f32, viewport_height as f32],
                backdrop_origin: [
                    self.backdrop_mapping.region.x as f32,
                    self.backdrop_mapping.region.y as f32,
                ],
                backdrop_extent: [
                    self.backdrop_mapping.region.width as f32,
                    self.backdrop_mapping.region.height as f32,
                ],
                _pad: [0.0; 2],
            }),
        );

        let mut commands = Vec::with_capacity(8);
        let mut copy_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("liquid glass context menu pre-menu scene copy encoder"),
        });
        let copy_scope = profiler.begin("completed_scene_copy", &mut copy_encoder);
        copy_encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: pre_scene,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: roi.copy.0,
                    y: roi.copy.1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.context_menu_scene_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: roi.copy.0,
                    y: roi.copy.1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: roi.copy.2,
                height: roi.copy.3,
                depth_or_array_layers: 1,
            },
        );
        profiler.end(&mut copy_encoder, copy_scope);
        profiler.resolve(&mut copy_encoder);
        commands.push(copy_encoder.finish());

        let mut flatten_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("liquid glass context menu scene flatten encoder"),
        });
        let flatten_scope = profiler.begin("completed_scene_flatten", &mut flatten_encoder);
        {
            let mut pass = flatten_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass context menu scene flatten pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.context_menu_source_view,
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
            pass.set_scissor_rect(roi.blur.0, roi.blur.1, roi.blur.2, roi.blur.3);
            pass.set_pipeline(&self.context_menu_flatten_pipeline);
            pass.set_bind_group(0, &self.context_menu_flatten_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        profiler.end(&mut flatten_encoder, flatten_scope);
        profiler.resolve(&mut flatten_encoder);
        commands.push(flatten_encoder.finish());

        let profile = self.blur_profile(blur_radius);
        if !self.debug.disable_blur && profile.level_count > 0 {
            queue.write_buffer(
                &self.context_menu_blur_uniform_buffer,
                0,
                bytemuck::bytes_of(&BlurUniforms {
                    sample_scale: profile.sample_scale,
                    _pad: [0.0; 3],
                }),
            );
            commands.extend(encode_blur_profile(
                device,
                profiler,
                &self.blur_downsample_pipeline,
                &self.blur_upsample_pipeline,
                &self.blur_levels,
                &self.context_menu_blur_view,
                &self.context_menu_blur_down_bind_groups,
                &self.context_menu_blur_up_bind_groups,
                profile.level_count,
                "liquid glass context menu completed-scene blur",
                COMPLETED_BLUR_DOWN_SCOPES,
                COMPLETED_BLUR_UP_SCOPES,
            ));
        }
        queue.submit(commands);
    }

    /// Compute the region-of-interest rectangles for a completed-scene chain
    /// so the copy and flatten passes only touch pixels the lane's glass can
    /// sample, instead of the whole frame on every open frame.
    fn completed_scene_roi(
        &self,
        screen_bounds: Option<[f32; 4]>,
        blur_radius: f32,
    ) -> CompletedSceneRoi {
        let (viewport_width, viewport_height) = self.texture_size;
        let texture_size = (
            self.backdrop_mapping.texture_size.0.max(1),
            self.backdrop_mapping.texture_size.1.max(1),
        );
        let region = &self.backdrop_mapping.region;

        let full = CompletedSceneRoi {
            copy: (0, 0, viewport_width, viewport_height),
            blur: (0, 0, texture_size.0, texture_size.1),
        };
        let Some(bounds) = screen_bounds else {
            return full;
        };

        // The final glass shader samples the blurred scene beyond the shape
        // bounds (refraction, reflection, chromatic split); the capture-region
        // padding models that reach for the same shaders.
        let pad = capture_sampling_padding(
            &self.params,
            blur_radius,
            self.debug,
            viewport_width,
            viewport_height,
        );
        let screen = [
            bounds[0] - pad,
            bounds[1] - pad,
            bounds[2] + pad,
            bounds[3] + pad,
        ];

        // Map into capture-texture space and grow by the dual-Kawase kernel
        // reach so texels near the shapes see the same neighborhood a
        // full-frame blur would give them. The pyramid passes themselves stay
        // full-frame (scissored fullscreen passes proved unreliable on this
        // wgpu/Metal combination), so this padding protects the scissored
        // flatten pass feeding them.
        let scale_x = texture_size.0 as f32 / region.width.max(1) as f32;
        let scale_y = texture_size.1 as f32 / region.height.max(1) as f32;
        let profile = self.blur_profile(blur_radius);
        let blur_enabled = !self.debug.disable_blur && profile.level_count > 0;
        let kernel_pad = if blur_enabled {
            (profile.sample_scale.max(1.0) * 48.0).ceil() as u32 + 8
        } else {
            2
        };
        let capture_min_x = (screen[0] - region.x as f32) * scale_x - kernel_pad as f32;
        let capture_min_y = (screen[1] - region.y as f32) * scale_y - kernel_pad as f32;
        let capture_max_x = (screen[2] - region.x as f32) * scale_x + kernel_pad as f32;
        let capture_max_y = (screen[3] - region.y as f32) * scale_y + kernel_pad as f32;

        // 16 px alignment keeps the flatten pass on the same sampling phase a
        // full-frame pass would use, so written texels stay identical.
        const ALIGN: u32 = 16;
        let x = ((capture_min_x.max(0.0).floor() as u32) / ALIGN) * ALIGN;
        let y = ((capture_min_y.max(0.0).floor() as u32) / ALIGN) * ALIGN;
        let right = ((capture_max_x.clamp(0.0, texture_size.0 as f32).ceil() as u32)
            .div_ceil(ALIGN)
            * ALIGN)
            .min(texture_size.0);
        let bottom = ((capture_max_y.clamp(0.0, texture_size.1 as f32).ceil() as u32)
            .div_ceil(ALIGN)
            * ALIGN)
            .min(texture_size.1);
        let blur = (
            x.min(texture_size.0 - 1),
            y.min(texture_size.1 - 1),
            right.saturating_sub(x).max(1),
            bottom.saturating_sub(y).max(1),
        );

        // The flatten pass reads the pre-scene surface exactly where its
        // scissored capture pixels map, so copy at least that inverse mapping.
        let inv_x = |cx: f32| region.x as f32 + cx / texture_size.0 as f32 * region.width as f32;
        let inv_y = |cy: f32| region.y as f32 + cy / texture_size.1 as f32 * region.height as f32;
        let copy_x = inv_x(blur.0 as f32)
            .floor()
            .clamp(0.0, viewport_width.saturating_sub(1) as f32) as u32;
        let copy_y = inv_y(blur.1 as f32)
            .floor()
            .clamp(0.0, viewport_height.saturating_sub(1) as f32) as u32;
        let copy_right = (inv_x((blur.0 + blur.2) as f32)
            .ceil()
            .clamp(1.0, viewport_width as f32) as u32)
            .max(copy_x + 1);
        let copy_bottom = (inv_y((blur.1 + blur.3) as f32)
            .ceil()
            .clamp(1.0, viewport_height as f32) as u32)
            .max(copy_y + 1);
        let copy = (
            copy_x,
            copy_y,
            copy_right.saturating_sub(copy_x).min(viewport_width),
            copy_bottom.saturating_sub(copy_y).min(viewport_height),
        );

        CompletedSceneRoi { copy, blur }
    }

    /// Render the context-menu glass. Drawn after the modal/settings glass so
    /// it composites above an open folder panel, but isolated in its own SDF
    /// field so the two never smooth-union together.
    pub fn render_context_menu_glass(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        if !self.params.enabled {
            return;
        }
        if self.context_menu_shape_count == 0 {
            return;
        }

        let (width, height) = self.texture_size;
        let mut uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            0.0,
            self.context_menu_shape_count,
            0.0,
            0.0,
            self.backdrop_mapping,
        );
        if !self.debug.disable_blur {
            uniforms.blur_radius = self
                .context_menu_blur_radius
                .unwrap_or(self.params.blur_radius);
        }
        uniforms.backdrop_replacement = self.context_menu_backdrop_replacement;
        uniforms.glass_darkness = self.context_menu_glass_darkness;
        // Apply only this panel's manual darkness to the completed scene.
        uniforms.auto_darkness_enabled = 0;
        queue.write_buffer(
            &self.context_menu_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass context menu geometry pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_geometry_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.overlay_tint_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.geometry_pipeline);
            pass.set_bind_group(0, &self.context_menu_geometry_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass context menu final pass"),
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
            pass.set_bind_group(0, &self.context_menu_final_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// GPU profiler scope names for the shared pyramid users. Global = backdrop
/// pyramid; completed = context-menu/settings completed-scene blur.
const GLOBAL_BLUR_DOWN_SCOPES: [&str; 3] = [
    "global_blur_down_1",
    "global_blur_down_2",
    "global_blur_down_3",
];
const GLOBAL_BLUR_UP_SCOPES: [&str; 3] =
    ["global_blur_up_1", "global_blur_up_2", "global_blur_up_3"];
const COMPLETED_BLUR_DOWN_SCOPES: [&str; 3] = [
    "completed_blur_down_1",
    "completed_blur_down_2",
    "completed_blur_down_3",
];
const COMPLETED_BLUR_UP_SCOPES: [&str; 3] = [
    "completed_blur_up_1",
    "completed_blur_up_2",
    "completed_blur_up_3",
];

/// Screen- and capture-space rectangles that limit the completed-scene
/// copy/flatten/blur chain to the pixels a lane's glass can actually sample.
#[derive(Clone, Copy, Debug)]
struct CompletedSceneRoi {
    /// Viewport-space (x, y, w, h) copied from the pre-scene surface.
    copy: (u32, u32, u32, u32),
    /// Capture-space (x, y, w, h) for the flatten output and blur pyramid.
    blur: (u32, u32, u32, u32),
}

/// Build one complete downsample + upsample chain into a full-resolution
/// output. The shared pyramid is only scratch storage; callers must bind the
/// returned `output` texture in the final material pass, never an intermediate
/// level from `levels`.
///
#[allow(clippy::too_many_arguments)]
fn encode_blur_profile(
    device: &wgpu::Device,
    profiler: &mut GpuProfilerState,
    downsample_pipeline: &wgpu::RenderPipeline,
    upsample_pipeline: &wgpu::RenderPipeline,
    levels: &[(wgpu::Texture, wgpu::TextureView); 3],
    output: &wgpu::TextureView,
    down_bind_groups: &[wgpu::BindGroup; 3],
    up_bind_groups: &[wgpu::BindGroup; 3],
    level_count: usize,
    label_prefix: &str,
    down_scopes: [&'static str; 3],
    up_scopes: [&'static str; 3],
) -> Vec<wgpu::CommandBuffer> {
    debug_assert!((1..=3).contains(&level_count));
    let mut commands = Vec::with_capacity(level_count * 2);

    for i in 0..level_count {
        let label = format!("{label_prefix} downsample L{i}->L{}", i + 1);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(label.as_str()),
        });
        let profile_scope = profiler.begin(down_scopes[i], &mut encoder);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label.as_str()),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &levels[i].1,
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
            pass.set_pipeline(downsample_pipeline);
            pass.set_bind_group(0, &down_bind_groups[i], &[]);
            pass.draw(0..3, 0..1);
        }
        profiler.end(&mut encoder, profile_scope);
        profiler.resolve(&mut encoder);
        commands.push(encoder.finish());
    }

    for j in 0..level_count {
        let destination = if j == level_count - 1 {
            output
        } else {
            &levels[level_count - 2 - j].1
        };
        let bind_index = 3 - level_count + j;
        let label = format!(
            "{label_prefix} upsample L{}->L{}",
            level_count - j,
            level_count - 1 - j
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(label.as_str()),
        });
        let profile_scope = profiler.begin(up_scopes[j], &mut encoder);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label.as_str()),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: destination,
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
            pass.set_pipeline(upsample_pipeline);
            pass.set_bind_group(0, &up_bind_groups[bind_index], &[]);
            pass.draw(0..3, 0..1);
        }
        profiler.end(&mut encoder, profile_scope);
        profiler.resolve(&mut encoder);
        commands.push(encoder.finish());
    }

    commands
}

/// Union of a lane's shape bounds in screen space, clipped to the viewport.
/// Screen-fixed lanes (context menu, settings panel) pass a scroll of zero.
fn completed_scene_screen_bounds(
    shapes: &[GlassShape],
    width: u32,
    height: u32,
) -> Option<[f32; 4]> {
    let viewport = [0.0, 0.0, width.max(1) as f32, height.max(1) as f32];
    let mut union: Option<[f32; 4]> = None;
    for shape in shapes {
        if shape.is_clip_only() {
            continue;
        }
        let Some(bounds) = intersect_bounds(shape.screen_bounds(0.0), viewport) else {
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
    union
}
