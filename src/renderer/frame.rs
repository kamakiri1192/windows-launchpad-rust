//! Per-frame draw-pass orchestration and the optional QA self-capture path.
//!
//! The draw pass order is load-bearing and preserved verbatim from the
//! historical monolithic renderer:
//!
//! 1. lower-scene texture clear pass (transparent)
//! 2. Liquid Glass base pass (page frame + scrolling tile halos, backdrop)
//! 3. tile fill pass (opaque app fills; folder fallback fills are discarded)
//! 4. nested grid Liquid Glass pass (closed folder containers)
//! 5. grid icon + text pass (content above nested glass)
//! 6. edit-badge glass + foreground marks (above grid, below dragged icon)
//! 7. isolated dragged-folder Liquid Glass pass
//! 8. drag overlay pass (dragged tile + icon on top)
//! 9. Liquid Glass control pass (capsule + gear merge)
//! 10. control overlay pass (control ink, gear ink, control text)
//! 11. optional lower-scene Dual-Kawase blur + rounded focus composite
//! 12. focus tint backdrop
//! 13. Liquid Glass settings/folder panel pass (modal)
//! 14. modal content pass
//!
//! The per-frame uniform updates are tiny (viewport + scroll + time + drag);
//! no static scene is rebuilt here.

use wgpu::{Color, TextureViewDescriptor};

use crate::renderer::tiles::TileInstance;
use crate::ui_model::render_model::{GlassLayer, GlassMaterial};

use super::controls::ControlUniforms;
use super::focus_blur::{FocusBlurParams, ProminentBlurParams};
use super::tiles::Uniforms;
use super::{DrawArgs, Renderer};

enum RenderFrame {
    Surface(wgpu::SurfaceTexture),
    Offscreen(wgpu::Texture),
}

const PROMINENT_FOCUS_BLUR_SPREAD: f32 = 20.0;

impl RenderFrame {
    fn texture(&self) -> &wgpu::Texture {
        match self {
            Self::Surface(frame) => &frame.texture,
            Self::Offscreen(texture) => texture,
        }
    }

    fn present(self) {
        if let Self::Surface(frame) = self {
            frame.present();
        }
    }
}

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

        let frame = if let Some(surface) = &self.surface {
            match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(frame)
                | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => RenderFrame::Surface(frame),
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
            }
        } else if let Some(texture) = &self.qa_offscreen {
            RenderFrame::Offscreen(texture.clone())
        } else {
            eprintln!("renderer has neither a surface nor an offscreen target");
            return;
        };
        let output_view = frame
            .texture()
            .create_view(&TextureViewDescriptor::default());
        let view = self.presentation.create_view();
        let prominent_surface = self
            .prepared_model
            .glass
            .iter()
            .filter(|batch| batch.layer == GlassLayer::Modal)
            .flat_map(|batch| batch.surfaces.iter())
            .enumerate()
            .filter(|(_, surface)| surface.material == GlassMaterial::Prominent)
            .max_by_key(|(index, surface)| (surface.z, *index))
            .map(|(_, surface)| surface);
        let focus_blur_params = self
            .prepared_model
            .ink
            .iter()
            .find(|batch| batch.lane == crate::ui_model::render_model::InkLane::Backdrop)
            .and_then(|batch| batch.views.iter().find(|view| view.scene_blur > 0.001))
            .map(|focus| FocusBlurParams {
                viewport: args.viewport,
                center: [focus.center.x, focus.center.y],
                half_size: [focus.stroke, focus.extent],
                radius: focus.corner_radius,
                strength: focus.scene_blur,
                prominent: prominent_surface.map(|surface| {
                    let center = surface.rect.center();
                    ProminentBlurParams {
                        center: [center.x, center.y],
                        half_size: [surface.rect.width * 0.5, surface.rect.height * 0.5],
                        radius: surface.radius,
                        strength: focus.scene_blur,
                        spread: PROMINENT_FOCUS_BLUR_SPREAD,
                    }
                }),
            })
            .unwrap_or(FocusBlurParams {
                viewport: args.viewport,
                center: [clip.0, clip.1],
                half_size: [clip.2, clip.3],
                radius: clip.4,
                strength: 0.0,
                prominent: None,
            });
        let scene_view = self.focus_blur.scene_view();

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        let profile_scope = self.gpu_profiler.begin("lower_scene_clear", &mut encoder);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lower scene clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_view,
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
        self.gpu_profiler.end(&mut encoder, profile_scope);

        let profile_scope = self.gpu_profiler.begin("base_liquid_glass", &mut encoder);
        self.liquid_glass.render(
            &self.device,
            &self.queue,
            &mut encoder,
            scene_view,
            args.scroll_x,
            args.defer_backdrop_capture,
        );
        self.gpu_profiler.end(&mut encoder, profile_scope);

        let instance_count = self.instance_buffer.len();
        let icon_instance_count = self.icon_instance_buffer.len();
        let top_level_drag_active = self.top_level_dragged_tile_instance && instance_count > 0;
        let normal_tile_count = if top_level_drag_active {
            instance_count - 1
        } else {
            instance_count
        };
        let dragged_icon_count = self.dragged_icon_instance_count.min(icon_instance_count);
        let drag_icon_active = dragged_icon_count > 0;
        let normal_icon_count = icon_instance_count - dragged_icon_count;

        let profile_scope = self.gpu_profiler.begin("grid_tile_fill", &mut encoder);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tile fill pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_view,
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
        }
        self.gpu_profiler.end(&mut encoder, profile_scope);

        let profile_scope = self.gpu_profiler.begin("folder_grid_glass", &mut encoder);
        self.liquid_glass.render_grid_overlay(
            &self.queue,
            &mut encoder,
            scene_view,
            args.scroll_x,
            args.time,
        );
        self.gpu_profiler.end(&mut encoder, profile_scope);

        let profile_scope = self.gpu_profiler.begin("grid_icons_text", &mut encoder);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("grid icon and text pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_view,
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
        self.gpu_profiler.end(&mut encoder, profile_scope);

        // Edit badges sit above the normal grid but below the lifted dragged
        // icon. The bottom control remains a later, screen-fixed overlay.
        self.queue.write_buffer(
            &self.control_uniform_buffer,
            0,
            bytemuck::bytes_of(&ControlUniforms {
                viewport_scroll: [
                    args.viewport.0 as f32,
                    args.viewport.1 as f32,
                    args.scroll_x,
                    args.time,
                ],
                frame_center_radius: [clip.0, clip.1, clip.4, 0.0],
                frame_half_size: [clip.2, clip.3, 0.0, 0.0],
            }),
        );
        let profile_scope = self.gpu_profiler.begin("edit_badge_glass", &mut encoder);
        self.liquid_glass.render_badges(
            &self.queue,
            &mut encoder,
            scene_view,
            args.scroll_x,
            args.time,
        );
        self.gpu_profiler.end(&mut encoder, profile_scope);
        let profile_scope = self.gpu_profiler.begin("edit_badge_ink", &mut encoder);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("edit badge foreground pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_view,
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
        self.gpu_profiler.end(&mut encoder, profile_scope);

        let profile_scope = self
            .gpu_profiler
            .begin("drag_folder_liquid_glass", &mut encoder);
        self.liquid_glass
            .render_drag_overlay(&self.queue, &mut encoder, scene_view, args.time);
        self.gpu_profiler.end(&mut encoder, profile_scope);

        let profile_scope = self
            .gpu_profiler
            .begin("top_level_drag_overlay", &mut encoder);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("drag overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_view,
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

            if top_level_drag_active {
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
                    pass.draw(0..6, 0..dragged_icon_count);
                }
            }
        }
        self.gpu_profiler.end(&mut encoder, profile_scope);

        let profile_scope = self
            .gpu_profiler
            .begin("control_liquid_glass", &mut encoder);
        self.liquid_glass
            .render_control(&self.queue, &mut encoder, scene_view);
        self.gpu_profiler.end(&mut encoder, profile_scope);

        let profile_scope = self.gpu_profiler.begin("control_content", &mut encoder);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("control overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_view,
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
        self.gpu_profiler.end(&mut encoder, profile_scope);

        // Finish the complete lower scene before any blur pass samples it.
        // The pyramid uses separate submissions because wgpu/D3D12 cannot
        // read and write successive levels inside one texture usage scope.
        self.gpu_profiler.resolve(&mut encoder);
        self.queue.submit(std::iter::once(encoder.finish()));
        if focus_blur_params.strength > 0.001 {
            self.focus_blur
                .blur(&self.device, &self.queue, &mut self.gpu_profiler);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("modal and focus encoder"),
            });
        let profile_scope = self
            .gpu_profiler
            .begin("focus_blur_composite", &mut encoder);
        self.focus_blur
            .composite(&self.queue, &mut encoder, &view, focus_blur_params);
        self.gpu_profiler.end(&mut encoder, profile_scope);

        // Generic modal focus tint. The lower-scene blur has already replaced
        // sharp grid content inside the same rounded geometry.
        let profile_scope = self.gpu_profiler.begin("focus_veil_tint", &mut encoder);
        if self.backdrop_instance_buffer.len() > 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("modal backdrop pass"),
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
            if let Some(buf) = self.backdrop_instance_buffer.as_ref() {
                pass.set_pipeline(&self.control_pipeline);
                pass.set_bind_group(0, &self.control_bind_group, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..6, 0..self.backdrop_instance_buffer.len());
            }
        }
        self.gpu_profiler.end(&mut encoder, profile_scope);

        // Generic dynamic modal Liquid Glass surface.
        let profile_scope = self.gpu_profiler.begin("modal_liquid_glass", &mut encoder);
        self.liquid_glass
            .render_settings_panel(&self.queue, &mut encoder, &view);
        self.gpu_profiler.end(&mut encoder, profile_scope);

        // Generic fixed modal content, plus settings-specific content, on top
        // of the modal glass.
        let profile_scope = self.gpu_profiler.begin("modal_content", &mut encoder);
        if self.modal_tile_instance_buffer.len() > 0
            || self.modal_icon_instance_buffer.len() > 0
            || self.modal_instance_buffer.len() > 0
            || self.modal_text_instance_buffer.len() > 0
            || self.settings_instance_buffer.len() > 0
            || self.settings_text_instance_buffer.len() > 0
        {
            let modal_tile_count = self.modal_tile_instance_buffer.len();
            let normal_modal_tile_count =
                modal_tile_count.saturating_sub(u32::from(self.modal_dragged_tile_instance));
            let modal_icon_count = self.modal_icon_instance_buffer.len();
            let normal_modal_icon_count =
                modal_icon_count.saturating_sub(u32::from(self.modal_dragged_icon_instance));
            let full_scissor = (0, 0, self.config.width.max(1), self.config.height.max(1));
            let content_scissor = self
                .modal_clip_rect
                .map(|rect| {
                    let x = rect.x.floor().max(0.0) as u32;
                    let y = rect.y.floor().max(0.0) as u32;
                    let max_x = rect.max_x().ceil().clamp(0.0, self.config.width as f32) as u32;
                    let max_y = rect.max_y().ceil().clamp(0.0, self.config.height as f32) as u32;
                    (
                        x.min(self.config.width.saturating_sub(1)),
                        y.min(self.config.height.saturating_sub(1)),
                        max_x.saturating_sub(x).max(1),
                        max_y.saturating_sub(y).max(1),
                    )
                })
                .unwrap_or(full_scissor);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("modal content pass"),
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
                pass.set_scissor_rect(
                    content_scissor.0,
                    content_scissor.1,
                    content_scissor.2,
                    content_scissor.3,
                );
                if normal_modal_tile_count > 0 {
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    pass.set_vertex_buffer(0, self.modal_tile_instance_buffer.buffer().slice(..));
                    pass.draw(0..6, 0..normal_modal_tile_count);
                }
                if normal_modal_icon_count > 0 {
                    if let Some(buf) = self.modal_icon_instance_buffer.as_ref() {
                        pass.set_pipeline(&self.icon_pipeline);
                        pass.set_bind_group(0, &self.icon_atlas_bind_group, &[]);
                        pass.set_vertex_buffer(0, buf.slice(..));
                        pass.draw(0..6, 0..normal_modal_icon_count);
                    }
                }
                if self.modal_instance_buffer.len() > 0 {
                    if let Some(buf) = self.modal_instance_buffer.as_ref() {
                        pass.set_pipeline(&self.control_pipeline);
                        pass.set_bind_group(0, &self.control_bind_group, &[]);
                        pass.set_vertex_buffer(0, buf.slice(..));
                        pass.draw(0..6, 0..self.modal_instance_buffer.len());
                    }
                }
                if self.modal_text_instance_buffer.len() > 0 {
                    if let Some(buf) = self.modal_text_instance_buffer.as_ref() {
                        pass.set_pipeline(&self.control_text_pipeline);
                        pass.set_bind_group(0, &self.control_text_bind_group, &[]);
                        pass.set_vertex_buffer(0, buf.slice(..));
                        pass.draw(0..6, 0..self.modal_text_instance_buffer.len());
                    }
                }
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

            // Folder child badges are a nested Liquid Glass layer over the
            // modal content. Their GPU animation uses the same time/pivot as
            // the child tiles, so the disk and × remain attached while
            // wiggling and while reorder springs move the base rect.
            self.liquid_glass
                .render_modal_badges(&self.queue, &mut encoder, &view, args.time);
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("modal edit badge foreground pass"),
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
                pass.set_scissor_rect(
                    content_scissor.0,
                    content_scissor.1,
                    content_scissor.2,
                    content_scissor.3,
                );
                if self.modal_badge_instance_buffer.len() > 0 {
                    if let Some(buf) = self.modal_badge_instance_buffer.as_ref() {
                        pass.set_pipeline(&self.control_pipeline);
                        pass.set_bind_group(0, &self.control_bind_group, &[]);
                        pass.set_vertex_buffer(0, buf.slice(..));
                        pass.draw(0..6, 0..self.modal_badge_instance_buffer.len());
                    }
                }
            }

            // Keep the pointer-attached child above every non-dragged badge.
            if self.modal_dragged_tile_instance || self.modal_dragged_icon_instance {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("modal drag overlay pass"),
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
                if self.modal_dragged_tile_instance && modal_tile_count > 0 {
                    let stride = std::mem::size_of::<crate::renderer::tiles::TileInstance>()
                        as wgpu::BufferAddress;
                    let offset = stride * normal_modal_tile_count as wgpu::BufferAddress;
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                    pass.set_vertex_buffer(
                        0,
                        self.modal_tile_instance_buffer.buffer().slice(offset..),
                    );
                    pass.draw(0..6, 0..1);
                }
                if self.modal_dragged_icon_instance && modal_icon_count > 0 {
                    if let Some(buf) = self.modal_icon_instance_buffer.as_ref() {
                        let stride =
                            std::mem::size_of::<crate::renderer::icon_pipeline::IconInstance>()
                                as wgpu::BufferAddress;
                        let offset = stride * normal_modal_icon_count as wgpu::BufferAddress;
                        pass.set_pipeline(&self.icon_pipeline);
                        pass.set_bind_group(0, &self.icon_atlas_bind_group, &[]);
                        pass.set_vertex_buffer(0, buf.slice(offset..));
                        pass.draw(0..6, 0..1);
                    }
                }
            }
        }
        self.gpu_profiler.end(&mut encoder, profile_scope);

        self.gpu_profiler.resolve(&mut encoder);
        self.queue.submit(std::iter::once(encoder.finish()));

        // Internal blending is premultiplied. Resolve the completed frame to
        // the alpha representation required by the platform surface (or PNG
        // QA) only after every visual layer has been composited.
        let mut presentation_encoder =
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("presentation resolve encoder"),
                });
        let profile_scope = self
            .gpu_profiler
            .begin("presentation_resolve", &mut presentation_encoder);
        self.presentation
            .encode(&mut presentation_encoder, &output_view);
        self.gpu_profiler
            .end(&mut presentation_encoder, profile_scope);
        self.gpu_profiler.resolve(&mut presentation_encoder);
        self.queue
            .submit(std::iter::once(presentation_encoder.finish()));
        self.gpu_profiler.finish_frame(&self.queue);

        // Optional QA self-capture: copy the surface texture to a host-readable
        // buffer and save it as PNG. Driven by `LAUNCHPAD_QA_SHOT_FILE`.
        if let Some(path) = self.qa_shot.take() {
            self.save_frame_png(frame.texture(), path);
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
            normalize_capture_rgba(self.surface_format, &mut pixels);
            // Reuse the `image` crate already in the dependency tree.
            if let Some(img) = image::RgbaImage::from_raw(w, h, pixels) {
                let _ = img.save(&path);
            }
        }
    }
}

fn normalize_capture_rgba(format: wgpu::TextureFormat, pixels: &mut [u8]) {
    if matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    ) {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_capture_rgba;

    #[test]
    fn qa_capture_converts_bgra_surfaces_to_png_rgba() {
        let mut pixels = [10, 20, 30, 255, 1, 2, 3, 4];
        normalize_capture_rgba(wgpu::TextureFormat::Bgra8UnormSrgb, &mut pixels);
        assert_eq!(pixels, [30, 20, 10, 255, 3, 2, 1, 4]);
    }

    #[test]
    fn qa_capture_keeps_rgba_surfaces_unchanged() {
        let mut pixels = [10, 20, 30, 255];
        normalize_capture_rgba(wgpu::TextureFormat::Rgba8UnormSrgb, &mut pixels);
        assert_eq!(pixels, [10, 20, 30, 255]);
    }
}
