//! Per-frame draw-pass orchestration and the optional QA self-capture path.
//!
//! The draw pass order is load-bearing and preserved verbatim from the
//! historical monolithic renderer:
//!
//! 1. surface clear pass (transparent)
//! 2. Liquid Glass base pass (page frame + scrolling tile halos, backdrop)
//! 3. tile pass (color tiles, normal icons, text labels)
//! 4. edit-badge glass + foreground ✕ marks (above grid, below dragged icon)
//! 5. drag overlay pass (dragged tile + icon on top)
//! 6. Liquid Glass control pass (capsule + gear merge)
//! 7. control overlay pass (control ink, gear ink, control text)
//! 8. Liquid Glass settings panel pass (modal)
//! 9. settings overlay pass (close ×, title text)
//!
//! The per-frame uniform updates are tiny (viewport + scroll + time + drag);
//! no static scene is rebuilt here.

use wgpu::{Color, TextureViewDescriptor};

use crate::renderer::tiles::TileInstance;

use super::controls::ControlUniforms;
use super::tiles::Uniforms;
use super::{DrawArgs, Renderer};

impl Renderer {
    /// Render one frame.
    pub fn render(&mut self, args: &DrawArgs) {
        // Update uniforms (tiny, every frame).
        let clip = self.frame_clip;
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: [args.viewport.0 as f32, args.viewport.1 as f32],
                scroll_x: args.scroll_x,
                time: args.time,
                frame_center: [clip.0, clip.1],
                frame_half_size: [clip.2, clip.3],
                frame_radius: clip.4,
                drag_active: args.drag_active,
                drag_pos: [args.drag_pos.0, args.drag_pos.1],
            }),
        );

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                eprintln!("surface outdated/lost; skipping frame");
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout => return,
            wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Validation => {
                eprintln!("surface validation error; skipping frame");
                return;
            }
        };
        let view = frame.texture.create_view(&TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("surface clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
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

        self.liquid_glass.render(
            &self.device,
            &self.queue,
            &mut encoder,
            &view,
            args.scroll_x,
            args.defer_backdrop_capture,
        );

        let instance_count = self.instance_buffer.len();
        let icon_instance_count = self.icon_instance_buffer.len();
        let drag_active = args.drag_active > 0.5 && instance_count > 0;
        let normal_tile_count = if drag_active {
            instance_count - 1
        } else {
            instance_count
        };
        let drag_icon_active = self.dragged_icon_instance && icon_instance_count > 0;
        let normal_icon_count = if drag_icon_active {
            icon_instance_count - 1
        } else {
            icon_instance_count
        };

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tile pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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

            // Normal color tiles. The dragged tile, if any, is withheld and
            // drawn again after badges so its lifted visual unit stays above
            // every non-dragged edit badge.
            if normal_tile_count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_vertex_buffer(0, self.instance_buffer.buffer().slice(..));
                // 6 verts per quad (two tris), instance_count quads.
                pass.draw(0..6, 0..normal_tile_count);
            }

            // Normal icons: drawn over the color tiles before labels. The
            // dragged icon, if any, is withheld until after text.
            if normal_icon_count > 0 {
                if let Some(buf) = self.icon_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.icon_pipeline);
                    pass.set_bind_group(0, &self.icon_atlas_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..normal_icon_count);
                }
            }

            // Text labels: same pass, third draw call. Uses the same
            // uniform (scroll/viewport) plus the atlas texture.
            if self.text_instance_buffer.len() > 0 {
                if let Some(buf) = self.text_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.text_pipeline);
                    pass.set_bind_group(0, &self.atlas_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.text_instance_buffer.len());
                }
            }
        }

        // Edit badges sit above the normal grid but below the lifted dragged
        // icon. The bottom control remains a later, screen-fixed overlay.
        self.update_edit_badges(args.time);
        self.queue.write_buffer(
            &self.control_uniform_buffer,
            0,
            bytemuck::bytes_of(&ControlUniforms {
                viewport_scroll: [
                    args.viewport.0 as f32,
                    args.viewport.1 as f32,
                    args.scroll_x,
                    0.0,
                ],
                frame_center_radius: [clip.0, clip.1, clip.4, 0.0],
                frame_half_size: [clip.2, clip.3, 0.0, 0.0],
            }),
        );
        self.liquid_glass
            .render_badges(&self.queue, &mut encoder, &view, args.scroll_x);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("edit badge foreground pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            if self.badge_instance_buffer.len() > 0 {
                if let Some(buf) = self.badge_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_pipeline);
                    pass.set_bind_group(0, &self.control_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.badge_instance_buffer.len());
                }
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("drag overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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

            if drag_active {
                let stride = std::mem::size_of::<TileInstance>() as wgpu::BufferAddress;
                let offset = stride * normal_tile_count as wgpu::BufferAddress;
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                pass.set_vertex_buffer(0, self.instance_buffer.buffer().slice(offset..));
                pass.draw(0..6, 0..1);
            }
            if drag_icon_active {
                if let Some(buf) = self.icon_instance_buffer.as_ref() {
                    let stride = std::mem::size_of::<crate::renderer::icon_pipeline::IconInstance>()
                        as wgpu::BufferAddress;
                    let offset = stride * normal_icon_count as wgpu::BufferAddress;
                    pass.set_pipeline(&self.icon_pipeline);
                    pass.set_bind_group(0, &self.icon_atlas_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(offset..));
                    pass.draw(0..6, 0..1);
                }
            }
        }

        self.liquid_glass
            .render_control(&self.queue, &mut encoder, &view);

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("control overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            if self.control_instance_buffer.len() > 0 {
                if let Some(buf) = self.control_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_pipeline);
                    pass.set_bind_group(0, &self.control_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.control_instance_buffer.len());
                }
            }
            // Corner gear ink shares the control ink pipeline.
            if self.gear_instance_buffer.len() > 0 {
                if let Some(buf) = self.gear_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_pipeline);
                    pass.set_bind_group(0, &self.control_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.gear_instance_buffer.len());
                }
            }
            if self.control_text_instance_buffer.len() > 0 {
                if let Some(buf) = self.control_text_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_text_pipeline);
                    pass.set_bind_group(0, &self.control_text_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.control_text_instance_buffer.len());
                }
            }
        }

        // Settings overlay panel — drawn last so it composites over everything
        // (grid, control, gear).
        self.liquid_glass
            .render_settings_panel(&self.queue, &mut encoder, &view);

        // Settings panel ink (close ×) + title text, on top of the panel glass.
        if self.settings_instance_buffer.len() > 0 || self.settings_text_instance_buffer.len() > 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("settings overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            if self.settings_instance_buffer.len() > 0 {
                if let Some(buf) = self.settings_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_pipeline);
                    pass.set_bind_group(0, &self.control_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.settings_instance_buffer.len());
                }
            }
            if self.settings_text_instance_buffer.len() > 0 {
                if let Some(buf) = self.settings_text_instance_buffer.as_ref() {
                    pass.set_pipeline(&self.control_text_pipeline);
                    pass.set_bind_group(0, &self.control_text_bind_group, &[]);
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..6, 0..self.settings_text_instance_buffer.len());
                }
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        // Optional QA self-capture: copy the surface texture to a host-readable
        // buffer and save it as PNG. Driven by `LAUNCHPAD_QA_SHOT_FILE`.
        if let Some(path) = self.qa_shot.take() {
            self.save_frame_png(&frame.texture, path);
        }

        frame.present();
    }

    /// Copy `src` (the current surface texture) into a host buffer and write it
    /// to `path` as a PNG. Used only by the `qa_shot` QA harness; lets CI /
    /// sandboxes capture rendered frames without foreground access. See
    /// `docs/EDIT_MODE_VISUAL_QA.md` for the trigger protocol.
    fn save_frame_png(&self, src: &wgpu::Texture, path: std::path::PathBuf) {
        let w = src.width();
        let h = src.height();
        if w == 0 || h == 0 {
            return;
        }
        let bytes_per_row = w * 4;
        let padded = (bytes_per_row + 255) & !255; // wgpu requires 256-byte align
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("qa capture buffer"),
            size: (padded as u64) * (h as u64),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("qa capture encoder"),
            });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: src,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(enc.finish()));

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        {
            let data = slice.get_mapped_range();
            // De-pad rows into a tight RGBA buffer, then save.
            let mut pixels: Vec<u8> = Vec::with_capacity((bytes_per_row as usize) * (h as usize));
            for row in 0..h {
                let start = (row as usize) * (padded as usize);
                pixels.extend_from_slice(&data[start..start + bytes_per_row as usize]);
            }
            // Reuse the `image` crate already in the dependency tree.
            if let Some(img) = image::RgbaImage::from_raw(w, h, pixels) {
                let _ = img.save(&path);
            }
        }
    }
}
