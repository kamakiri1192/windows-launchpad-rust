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
        if !self.params.enabled || self.shape_count == 0 {
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
                self.backdrop_mapping = BackdropMapping::full(width, height);
                if let GpuCaptureFrame::New { texture, view } = gpu_frame {
                    if !self.using_gpu_backdrop {
                        eprintln!("liquid glass capture path: GPU shared texture");
                    }
                    self.bind_backdrop_view(device, &view);
                    self.gpu_backdrop_texture = Some(texture);
                    self.using_gpu_backdrop = true;
                }
                captured = true;
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
            self.backdrop_mapping,
        );
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        let blur_levels = self.blur_level_count();
        let refreshed_blur = should_refresh_blur(self.blur_dirty, captured);

        // Each blur pass runs in its OWN command encoder. wgpu groups all
        // passes in a single encoder into one "usage scope", and a texture
        // may not be both RESOURCE and COLOR_TARGET within that scope. Since a
        // dual-Kawase pyramid feeds each pass's output into the next pass's
        // input (L2 is written by down then read by up), we must split scopes
        // by submitting one encoder per pass.
        let _ = encoder; // the caller's encoder is used only for geometry/final.

        // Downsample: backdrop -> L1 -> ... -> L(k-1). down[i] reads the
        // backdrop for i==0 else levels[i-1], and writes levels[i].
        for i in 0..if refreshed_blur { blur_levels } else { 0 } {
            let dst = &self.blur_levels[i].1;
            let label = format!("liquid glass blur downsample L{i}->L{}", i + 1);
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some(label.as_str()),
            });
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
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
                pass.set_pipeline(&self.blur_downsample_pipeline);
                pass.set_bind_group(0, &self.blur_down_bind_groups[i], &[]);
                pass.draw(0..3, 0..1);
            }
            queue.submit(std::iter::once(enc.finish()));
        }

        // Upsample: L(k-1) -> L(k-2) -> ... -> L1 -> full-res blur.
        // up pass j reads levels[k-1-j] (bind index 3-k+j in the fixed
        // [L3,L2,L1] bind array) and writes levels[k-2-j], or the full-res
        // blur texture for the final hop (j == k-1).
        for j in 0..if refreshed_blur { blur_levels } else { 0 } {
            let dst = if j == blur_levels - 1 {
                &self.blur_view
            } else {
                &self.blur_levels[blur_levels - 2 - j].1
            };
            let bind_idx = 3 - blur_levels + j;
            let label = format!(
                "liquid glass blur upsample L{}->L{}",
                blur_levels - j,
                blur_levels - 1 - j
            );
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some(label.as_str()),
            });
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
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
            queue.submit(std::iter::once(enc.finish()));
        }
        if refreshed_blur {
            self.blur_dirty = false;
        }

        let geometry_key = self.geometry_key(scroll_x);
        if self.last_geometry_key != Some(geometry_key) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass geometry pass"),
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

        self.last_geometry_key = None;
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

        self.last_geometry_key = None;
    }

    pub fn render_badges(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        scroll_x: f32,
        time: f32,
    ) {
        if !self.params.enabled || self.badge_shape_count == 0 {
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
            self.backdrop_mapping,
        );
        queue.write_buffer(&self.badge_uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass badge geometry pass"),
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
            pass.set_bind_group(0, &self.badge_geometry_bind_group, &[]);
            pass.draw(0..3, 0..1);
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

        // The badge pass reuses the main geometry texture, so force the base
        // glass pass to repaint its mask next frame instead of reusing the
        // now-overwritten badge mask.
        self.last_geometry_key = None;
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

        self.last_geometry_key = None;
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
        let uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            0.0,
            self.control_shape_count,
            0.0,
            self.backdrop_mapping,
        );
        queue.write_buffer(
            &self.control_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass control geometry pass"),
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
            pass.set_bind_group(0, &self.control_geometry_bind_group, &[]);
            pass.draw(0..3, 0..1);
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

        self.last_geometry_key = None;
    }

    /// Render the settings overlay panel glass. Drawn last (over everything),
    /// so it composites above the grid, control, and gear.
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
        let uniforms = uniforms_from_params(
            &self.params,
            self.debug,
            (width, height),
            0.0,
            self.settings_panel_shape_count,
            0.0,
            self.backdrop_mapping,
        );
        queue.write_buffer(
            &self.settings_panel_uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms),
        );

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("liquid glass settings panel geometry pass"),
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
            pass.set_bind_group(0, &self.settings_panel_final_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        self.last_geometry_key = None;
    }
}
