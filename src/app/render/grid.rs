//! Grid layout / spring / edit animation render adapter methods.

use crate::domain::app_id::AppId;
use crate::grid;
use crate::scroll::{self, Phase};
use crate::ui_model::geometry::{Rect, UvRect};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, GlassBatch, GlassLayer, GlyphBatch, GlyphLane, GlyphView, IconView, RenderModel,
    TileView,
};

use super::helpers::SpringPos;
use crate::app::state::App;

impl App {
    pub(crate) fn search_input_changed(&mut self) {
        self.relayout();
        let (w, _h) = self.viewport_phys();
        let bounds = self.layout.bounds(w as f32);
        if let Some(s) = self.scroller.as_mut() {
            s.position = bounds.snap_target(s.position);
            s.velocity = 0.0;
            s.phase = Phase::Idle;
        }
        self.last_page = self.current_page() as i32;
        self.request_redraw();
    }

    /// Recompute layout/bounds for the current window size and push tile +
    /// label + icon instance buffers to the GPU.
    pub(crate) fn relayout(&mut self) {
        let (w, _h) = self.viewport_phys();
        let owned = self.grid_apps_owned();
        // Size pages to the current visible app count so every filtered app is
        // reachable and blank trailing pages disappear during search.
        self.layout = grid::GridLayout::for_app_count(owned.len())
            .with_scale_factor(self.scale_factor)
            .centered(w as f32);
        let bounds = self.layout.bounds(w as f32);
        if let Some(s) = self.scroller.as_mut() {
            s.set_bounds(bounds);
        }

        let apps: Vec<grid::GridApp<'_>> = owned
            .iter()
            .map(|(id, name, uv)| grid::GridApp {
                id: id.as_str(),
                name: name.as_str(),
                uv: *uv,
            })
            .collect();

        // Text labels.
        let scale = self.scale_factor;
        let dirty = if let Some(t) = self.text.as_mut() {
            let labels = self.layout.build_labels(w as f32, &apps);
            let quads = t.layout_labels(&labels, scale);
            let dirty = t.atlas_dirty;
            if let Some(r) = self.renderer.as_mut() {
                let mut model = RenderModel::new();
                model.glyphs.push(GlyphBatch {
                    lane: GlyphLane::Grid,
                    views: grid_glyph_views(&quads),
                });
                r.prepare(&model);
                if dirty {
                    r.upload_atlas(t.atlas_rgba());
                }
            }
            dirty
        } else {
            false
        };
        if dirty {
            if let Some(t) = self.text.as_mut() {
                t.atlas_dirty = false;
            }
        }

        let visible_ids = self.visible_app_ids();
        let anim = self.edit_anim(&visible_ids);
        // Update the per-tile position springs to the new home cells (keeping
        // each spring's current value so tiles glide from where they were).
        self.update_tile_springs(&visible_ids, w as f32);
        // Build the instances and override each tile's position with its spring
        // value so a reorder (or relayout) animates the icons sliding into place
        // rather than snapping. Done before the renderer borrow so we can read
        // the springs under &self.
        let mut tile_instances = self.layout.build_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut tile_instances);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut icon_instances);
        // While dragging, lift the dragged app off the grid: remove it from the
        // normal instance list and append a pointer-following copy at the end so
        // it draws on top of everything else.
        self.lift_dragged_instances(&mut tile_instances, &mut icon_instances, &visible_ids);
        if let Some(r) = self.renderer.as_mut() {
            // The liquid-glass shape rebuild uses the resting positions (the
            // glass doesn't need to follow the slide); the tile/icon instance
            // buffers carry the spring-adjusted positions.
            let mut model = RenderModel::new();
            model.glass.push(GlassBatch {
                layer: GlassLayer::Base,
                surfaces: self.layout.build_glass_surfaces(w as f32, &apps),
            });
            model.tiles = Some(tile_instances);
            model.icons = Some(icon_instances);
            r.prepare(&model);
        }

        let atlas_grew = self.ensure_atlas_uploaded();
        if atlas_grew {
            // Growing the atlas changes UVs for icons that were already cached
            // before this relayout, so refresh the icon instance buffer once
            // more after the registry has been re-synced.
            self.rebuild_icon_instances();
        }
    }

    pub(crate) fn edit_anim(&self, visible_ids: &[AppId]) -> Vec<grid::TileAnim> {
        if !self.editing {
            return Vec::new();
        }
        let drag_id = self.drag_app.as_ref();
        visible_ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let is_drag = drag_id.map(|d| d == id).unwrap_or(false);
                if is_drag {
                    grid::TileAnim {
                        phase: self.wiggle_phase + i as f32 * 0.37,
                        lift: 24.0 * self.scale_factor.max(1.0),
                        scale: 1.15,
                        flags: grid::TileAnim::FLAG_WIGGLE | grid::TileAnim::FLAG_DRAG,
                    }
                } else {
                    grid::TileAnim {
                        phase: self.wiggle_phase + i as f32 * 0.37,
                        lift: 0.0,
                        scale: 1.0,
                        flags: grid::TileAnim::FLAG_WIGGLE,
                    }
                }
            })
            .collect()
    }

    /// Realign `tile_springs` with the current visible app set. Existing
    /// springs are matched by `AppId`, not position, so a reordered app keeps
    /// its previous cell as the spring value and glides to its new home cell.
    pub(crate) fn update_tile_springs(&mut self, visible_ids: &[AppId], viewport_w: f32) {
        let mut old = std::mem::take(&mut self.tile_springs);
        self.tile_springs.reserve(visible_ids.len());
        for (i, id) in visible_ids.iter().enumerate() {
            let (x, y) = self.layout.tile_position(viewport_w, i);
            if let Some(pos) = old.iter().position(|(spring_id, _)| spring_id == id) {
                let (_, mut spring) = old.swap_remove(pos);
                spring.glide_to(x, y);
                self.tile_springs.push((id.clone(), spring));
            } else {
                self.tile_springs
                    .push((id.clone(), scroll::Spring2::at(x, y)));
            }
        }
    }

    /// Override each instance's position with its spring value, so the tile
    /// slides from where it was toward its home cell. Works for both
    /// `TileInstance` and `IconInstance` via the [`SpringPos`] trait.
    pub(crate) fn apply_spring_positions<T: SpringPos>(
        &self,
        visible_ids: &[AppId],
        instances: &mut [T],
    ) {
        for (id, inst) in visible_ids.iter().zip(instances.iter_mut()) {
            if let Some((_, spring)) = self
                .tile_springs
                .iter()
                .find(|(spring_id, _)| spring_id == id)
            {
                inst.set_pos(spring.x.value, spring.y.value);
            }
        }
    }

    /// While an edit-mode drag is in flight, move the dragged app's tile + icon
    /// to the end of the instance lists so it draws on top of everything else —
    /// but keep it as the *same* instance, not a duplicate. The shader uses
    /// `drag_pos` to make that trailing instance follow the pointer.
    pub(crate) fn lift_dragged_instances(
        &self,
        tile_instances: &mut Vec<TileView>,
        icon_instances: &mut Vec<IconView>,
        _visible_ids: &[AppId],
    ) {
        let is_drag = |flags: u32| flags & grid::TileAnim::FLAG_DRAG != 0;

        if let Some(pos) = tile_instances.iter().position(|t| is_drag(t.motion.flags)) {
            let item = tile_instances.swap_remove(pos);
            tile_instances.push(item);
        }
        if let Some(pos) = icon_instances.iter().position(|i| is_drag(i.motion.flags)) {
            let item = icon_instances.swap_remove(pos);
            icon_instances.push(item);
        }
    }

    /// Advance every tile position spring by `dt`. Returns `true` while any
    /// spring is still animating (so the caller keeps redrawing).
    pub(crate) fn step_tile_springs(&mut self, dt: f32) -> bool {
        let cfg = self.scroller.as_ref().map(|s| s.cfg).unwrap_or_default();
        let mut any = false;
        for (_, s) in &mut self.tile_springs {
            if s.step(dt, &cfg) {
                any = true;
            }
        }
        any
    }

    /// Rebuild + re-push the tile/icon instance buffers using the current
    /// spring positions, without recomputing the layout. Called every frame
    /// while the springs are animating so the slide is visible.
    pub(crate) fn refresh_spring_instances(&mut self) {
        let owned = self.grid_apps_owned();
        let apps: Vec<grid::GridApp<'_>> = owned
            .iter()
            .map(|(id, name, uv)| grid::GridApp {
                id: id.as_str(),
                name: name.as_str(),
                uv: *uv,
            })
            .collect();
        let (w, _h) = self.viewport_phys();
        let visible_ids = self.visible_app_ids();
        let anim = self.edit_anim(&visible_ids);
        let mut tile_instances = self.layout.build_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut tile_instances);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut icon_instances);
        self.lift_dragged_instances(&mut tile_instances, &mut icon_instances, &visible_ids);
        if let Some(r) = self.renderer.as_mut() {
            let mut model = RenderModel::new();
            model.tiles = Some(tile_instances);
            model.icons = Some(icon_instances);
            r.prepare(&model);
        }
    }
}

fn grid_glyph_views(quads: &[crate::renderer::text_engine::GlyphQuad]) -> Vec<GlyphView> {
    quads
        .iter()
        .map(|quad| GlyphView {
            id: UiId::backdrop("grid-label"),
            rect: Rect::new(quad.x, quad.y, quad.w, quad.h),
            uv: UvRect {
                u0: quad.u0,
                v0: quad.v0,
                u1: quad.u1,
                v1: quad.v1,
            },
            color: Color::rgba(quad.color[0], quad.color[1], quad.color[2], quad.color[3]),
            z: 0,
        })
        .collect()
}
