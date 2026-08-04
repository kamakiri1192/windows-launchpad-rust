//! Liquid Glass frame and lane render-pass orchestration.

use super::*;

impl LiquidGlassRenderer {
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
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
                        captured = self.copy_ephemeral_gpu_backdrop(device, queue, frame);
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
        let next_status = self.capture.status();
        if next_status != self.capture_status {
            log_capture_status(&next_status);
            self.capture_status = next_status;
        }

        let uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            scroll_x,
            self.shape_count,
            0.0,
            0.0,
            self.backdrop_mapping,
        );
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
        let _ = encoder; // the caller's encoder is used only for geometry/final.

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
                &self.blur_downsample_pipeline,
                &self.blur_upsample_pipeline,
                &self.blur_levels,
                &self.blur_view,
                &self.blur_down_bind_groups,
                &self.blur_up_bind_groups,
                global_blur.level_count,
                "liquid glass global blur",
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
        let uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            scroll_x,
            self.grid_overlay_shape_count,
            time,
            0.0,
            self.backdrop_mapping,
        );
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

    pub fn has_context_menu_glass(&self) -> bool {
        self.params.enabled && self.context_menu_shape_count > 0
    }

    pub fn has_settings_panel_glass(&self) -> bool {
        self.params.enabled
            && self.settings_panel_completed_scene_enabled
            && self.settings_panel_shape_count > 0
    }

    /// Capture every layer already rendered to the transparent swapchain,
    /// flatten it over the real desktop capture, then build the menu's blur.
    /// This must run after modal content and before `render_context_menu_glass`.
    pub fn prepare_context_menu_scene_blur(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pre_menu_scene: &wgpu::Texture,
    ) {
        if !self.has_context_menu_glass() {
            return;
        }
        self.prepare_completed_scene_blur(
            device,
            queue,
            pre_menu_scene,
            self.context_menu_blur_radius
                .unwrap_or(self.params.blur_radius),
        );
    }

    /// Capture the completed scene immediately below the settings surface and
    /// build the same full-resolution blur used by the context menu.
    pub fn prepare_settings_panel_scene_blur(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pre_settings_scene: &wgpu::Texture,
    ) {
        if !self.has_settings_panel_glass() {
            return;
        }
        self.prepare_completed_scene_blur(
            device,
            queue,
            pre_settings_scene,
            self.settings_panel_blur_radius
                .unwrap_or(self.params.blur_radius),
        );
    }

    fn prepare_completed_scene_blur(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pre_scene: &wgpu::Texture,
        blur_radius: f32,
    ) {
        let (viewport_width, viewport_height) = self.texture_size;
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
        copy_encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: pre_scene,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.context_menu_scene_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: viewport_width,
                height: viewport_height,
                depth_or_array_layers: 1,
            },
        );
        commands.push(copy_encoder.finish());

        let mut flatten_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("liquid glass context menu scene flatten encoder"),
        });
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
            pass.set_pipeline(&self.context_menu_flatten_pipeline);
            pass.set_bind_group(0, &self.context_menu_flatten_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
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
                &self.blur_downsample_pipeline,
                &self.blur_upsample_pipeline,
                &self.blur_levels,
                &self.context_menu_blur_view,
                &self.context_menu_blur_down_bind_groups,
                &self.context_menu_blur_up_bind_groups,
                profile.level_count,
                "liquid glass context menu completed-scene blur",
            ));
        }
        queue.submit(commands);
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

/// Build one complete downsample + upsample chain into a full-resolution
/// output. The shared pyramid is only scratch storage; callers must bind the
/// returned `output` texture in the final material pass, never an intermediate
/// level from `levels`.
#[allow(clippy::too_many_arguments)]
fn encode_blur_profile(
    device: &wgpu::Device,
    downsample_pipeline: &wgpu::RenderPipeline,
    upsample_pipeline: &wgpu::RenderPipeline,
    levels: &[(wgpu::Texture, wgpu::TextureView); 3],
    output: &wgpu::TextureView,
    down_bind_groups: &[wgpu::BindGroup; 3],
    up_bind_groups: &[wgpu::BindGroup; 3],
    level_count: usize,
    label_prefix: &str,
) -> Vec<wgpu::CommandBuffer> {
    debug_assert!((1..=3).contains(&level_count));
    let mut commands = Vec::with_capacity(level_count * 2);

    for i in 0..level_count {
        let label = format!("{label_prefix} downsample L{i}->L{}", i + 1);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(label.as_str()),
        });
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
        commands.push(encoder.finish());
    }

    commands
}
