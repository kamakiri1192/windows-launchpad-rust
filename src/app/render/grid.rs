//! Grid layout / spring / edit animation render adapter methods.

use std::collections::BTreeSet;

use crate::domain::launcher_item::LauncherItem;
use crate::grid;
use crate::scroll::{self, Phase};
use crate::ui_model::geometry::{Rect, UvRect};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, GlassBehavior, GlassLayer, GlassMaterial, GlassSurface, GlyphLane, GlyphView, IconView,
    TileView,
};

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
        self.relayout_serial = self.relayout_serial.wrapping_add(1);
        let (w, _h) = self.viewport_phys();
        let owned = self.grid_items_owned();
        // Size pages to the current visible item count so every filtered item is
        // reachable and blank trailing pages disappear during search.
        self.layout = grid::GridLayout::for_app_count(owned.len())
            .with_scale_factor(self.scale_factor)
            .centered(w as f32);
        let bounds = self.layout.bounds(w as f32);
        if let Some(s) = self.scroller.as_mut() {
            s.set_bounds(bounds);
        }

        let items: Vec<grid::GridItem<'_>> = owned
            .iter()
            .map(|entry| grid::GridItem {
                key: entry.key.as_str(),
                name: entry.name.as_str(),
                uv: entry.uv,
                preview_uvs: &entry.preview_uvs,
            })
            .collect();

        // Text labels.
        let scale = self.scale_factor;
        let (grid_glyphs, dirty) = if let Some(t) = self.text.as_mut() {
            let labels = self.layout.build_labels(w as f32, &items);
            let quads = t.layout_labels(&labels, scale);
            let dirty = t.atlas_dirty;
            if dirty {
                if let Some(r) = self.renderer.as_mut() {
                    r.upload_atlas(t.atlas_rgba());
                }
            }
            (grid_glyph_views(&quads), dirty)
        } else {
            (Vec::new(), false)
        };
        self.render_model
            .set_glyph_batch(GlyphLane::Grid, grid_glyphs);
        if dirty {
            if let Some(t) = self.text.as_mut() {
                t.atlas_dirty = false;
            }
        }

        let visible_items = self.visible_launcher_items();
        let anim = self.edit_anim(&visible_items);
        // Update the per-tile position springs to the new home cells (keeping
        // each spring's current value so tiles glide from where they were).
        self.update_tile_springs(&visible_items, w as f32);
        // Build the instances and override each tile's position with its spring
        // value so a reorder (or relayout) animates the icons sliding into place
        // rather than snapping. Done before the renderer borrow so we can read
        // the springs under &self.
        let mut tile_instances = self.layout.build_instances(w as f32, &items, &anim);
        self.apply_tile_spring_positions(&visible_items, &mut tile_instances);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &items, &anim);
        self.apply_icon_spring_offsets(&visible_items, w as f32, &mut icon_instances);
        self.suppress_active_folder_grid_icons(&mut icon_instances);
        self.refresh_grid_glass_lanes(w as f32, &items, &visible_items, &tile_instances);
        // While dragging, lift the dragged app off the grid: remove it from the
        // normal instance list and append a pointer-following copy at the end so
        // it draws on top of everything else.
        self.lift_dragged_instances(&mut tile_instances, &mut icon_instances);
        self.interaction_glass = self.build_interaction_glass();
        self.render_model.tiles = Some(tile_instances);
        self.render_model.icons = Some(icon_instances);

        let atlas_grew = self.ensure_atlas_uploaded();
        if atlas_grew {
            // Growing the atlas changes UVs for icons that were already cached
            // before this relayout, so refresh the icon instance buffer once
            // more after the registry has been re-synced.
            self.rebuild_icon_instances();
        }
    }

    fn refresh_grid_glass_lanes(
        &mut self,
        viewport_w: f32,
        items: &[grid::GridItem<'_>],
        visible_items: &[LauncherItem],
        tiles: &[TileView],
    ) {
        let mut surfaces = self.layout.build_glass_surfaces(viewport_w, items);
        align_glass_to_tiles(&mut surfaces, tiles);
        let folder_ids: BTreeSet<_> = visible_items.iter().filter_map(folder_item_id).collect();
        let excluded_folder_ids: BTreeSet<_> = self
            .folders
            .active
            .iter()
            .map(|id| LauncherItem::Folder(id.clone()))
            .chain(self.drag_item.iter().cloned())
            .filter_map(|item| folder_item_id(&item))
            .collect();
        let (base_glass, mut folder_glass) =
            split_folder_glass_surfaces(surfaces, &folder_ids, &excluded_folder_ids);
        fit_folder_glass_to_tile(
            &mut folder_glass,
            self.layout.tile_size,
            self.layout.scaled(19.0),
        );
        // The dragged folder container belongs behind its miniature icons.
        // Keeping it in the later control-overlay lane tinted those icons and
        // made them look translucent while moving.
        folder_glass.extend(self.dragged_folder_glass_surface());
        self.render_model
            .set_glass_batch(GlassLayer::Base, base_glass);
        self.render_model
            .set_glass_batch(GlassLayer::GridOverlay, folder_glass);
    }

    pub(crate) fn edit_anim(&self, visible_items: &[LauncherItem]) -> Vec<grid::TileAnim> {
        let drag_item = self.drag_item.as_ref();
        let folder_progress = self.folders.motion.visual_progress();
        visible_items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let is_drag = drag_item == Some(item);
                let item_flags = launcher_item_tile_flags(item);
                let is_pressed_folder = self
                    .pending_press
                    .as_ref()
                    .and_then(|press| press.item.as_ref())
                    == Some(item)
                    && matches!(item, LauncherItem::Folder(_));
                let background_scale = 1.0 - folder_progress * 0.035;
                if !self.editing {
                    return grid::TileAnim {
                        phase: 0.0,
                        lift: 0.0,
                        scale: background_scale * if is_pressed_folder { 0.96 } else { 1.0 },
                        flags: item_flags,
                    };
                }
                if is_drag {
                    grid::TileAnim {
                        phase: self.wiggle_phase + i as f32 * 0.37,
                        lift: 24.0 * self.scale_factor.max(1.0),
                        scale: 1.15 * background_scale,
                        flags: grid::TileAnim::FLAG_WIGGLE | grid::TileAnim::FLAG_DRAG | item_flags,
                    }
                } else {
                    grid::TileAnim {
                        phase: self.wiggle_phase + i as f32 * 0.37,
                        lift: 0.0,
                        scale: background_scale,
                        flags: grid::TileAnim::FLAG_WIGGLE | item_flags,
                    }
                }
            })
            .collect()
    }

    /// Realign `tile_springs` with the current visible app set. Existing
    /// springs are matched by `AppId`, not position, so a reordered app keeps
    /// its previous cell as the spring value and glides to its new home cell.
    pub(crate) fn update_tile_springs(&mut self, visible_ids: &[LauncherItem], viewport_w: f32) {
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
    pub(crate) fn apply_tile_spring_positions(
        &self,
        visible_ids: &[LauncherItem],
        instances: &mut [TileView],
    ) {
        for (id, inst) in visible_ids.iter().zip(instances.iter_mut()) {
            if let Some((_, spring)) = self
                .tile_springs
                .iter()
                .find(|(spring_id, _)| spring_id == id)
            {
                inst.rect.x = spring.x.value;
                inst.rect.y = spring.y.value;
            }
        }
    }

    fn build_interaction_glass(&self) -> Vec<GlassSurface> {
        let mut surfaces = Vec::new();
        let Some(hover) = self.folders.hover.as_ref() else {
            return surfaces;
        };
        let Some(index) = self
            .visible_launcher_items()
            .iter()
            .position(|item| item == &hover.target)
        else {
            return Vec::new();
        };
        let progress = hover.progress();
        let (x, y) = self
            .layout
            .tile_position(self.viewport_phys().0 as f32, index);
        let scroll = self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        let target_size = self.layout.tile_size * (1.08 + 0.08 * progress);
        let pointer_size = self.layout.tile_size * (0.98 + 0.08 * progress);
        surfaces.extend([
            GlassSurface {
                id: UiId::backdrop("folder-hover-target"),
                rect: Rect::new(
                    x + scroll + (self.layout.tile_size - target_size) * 0.5,
                    y + (self.layout.tile_size - target_size) * 0.5,
                    target_size,
                    target_size,
                ),
                radius: 27.0 * self.scale_factor,
                material: GlassMaterial::Regular,
                behavior: GlassBehavior::Control,
                z: 20,
            },
            GlassSurface {
                id: UiId::backdrop("folder-hover-drag"),
                rect: Rect::new(
                    self.drag_x - pointer_size * 0.5,
                    self.drag_y - pointer_size * 0.5,
                    pointer_size,
                    pointer_size,
                ),
                radius: 27.0 * self.scale_factor,
                material: GlassMaterial::Regular,
                behavior: GlassBehavior::Control,
                z: 21,
            },
        ]);
        surfaces
    }

    fn dragged_folder_glass_surface(&self) -> Option<GlassSurface> {
        matches!(self.drag_item.as_ref(), Some(LauncherItem::Folder(_))).then(|| {
            let size = self.layout.tile_size * 1.15;
            GlassSurface {
                id: UiId::backdrop("dragged-folder-glass"),
                rect: Rect::new(
                    self.drag_x - size * 0.5,
                    self.drag_y - size * 0.5,
                    size,
                    size,
                ),
                radius: self.layout.scaled(19.0) * 1.15,
                material: GlassMaterial::Regular,
                behavior: GlassBehavior::Control,
                z: 22,
            }
        })
    }

    pub(crate) fn refresh_interaction_glass(&mut self) {
        self.interaction_glass = self.build_interaction_glass();
    }

    pub(crate) fn apply_icon_spring_offsets(
        &self,
        visible_items: &[LauncherItem],
        viewport_w: f32,
        instances: &mut [IconView],
    ) {
        for (index, item) in visible_items.iter().enumerate() {
            let Some((_, spring)) = self
                .tile_springs
                .iter()
                .find(|(spring_item, _)| spring_item == item)
            else {
                continue;
            };
            let (target_x, target_y) = self.layout.tile_position(viewport_w, index);
            let dx = spring.x.value - target_x;
            let dy = spring.y.value - target_y;
            let key = item.stable_key();
            let item_id = UiId::launcher_item(&key);
            let preview_prefix = format!("launcher-preview:{key}:");
            for instance in instances.iter_mut().filter(|instance| {
                instance.id == item_id || instance.id.as_str().starts_with(&preview_prefix)
            }) {
                instance.rect.x += dx;
                instance.rect.y += dy;
                if let Some(pivot) = instance.motion_pivot.as_mut() {
                    pivot.x += dx;
                    pivot.y += dy;
                }
            }
        }
    }

    /// The modal lane owns every child icon while a folder is opening, open,
    /// or closing. Remove the matching grid preview so the source tile does
    /// not leave a second set of miniatures behind the morph.
    pub(crate) fn suppress_active_folder_grid_icons(&self, icons: &mut Vec<IconView>) {
        let Some(folder_id) = self.folders.active.as_ref() else {
            return;
        };
        let key = LauncherItem::Folder(folder_id.clone()).stable_key();
        suppress_folder_preview_icons(icons, &key);
    }

    /// While an edit-mode drag is in flight, move the dragged app's tile + icon
    /// to the end of the instance lists so it draws on top of everything else —
    /// but keep it as the *same* instance, not a duplicate. The shader uses
    /// `drag_pos` to make that trailing instance follow the pointer.
    pub(crate) fn lift_dragged_instances(
        &self,
        tile_instances: &mut Vec<TileView>,
        icon_instances: &mut Vec<IconView>,
    ) {
        let is_drag = |flags: u32| flags & grid::TileAnim::FLAG_DRAG != 0;

        let (mut normal_tiles, dragged_tiles): (Vec<_>, Vec<_>) = std::mem::take(tile_instances)
            .into_iter()
            .partition(|tile| !is_drag(tile.motion.flags));
        normal_tiles.extend(dragged_tiles);
        *tile_instances = normal_tiles;

        let (mut normal_icons, dragged_icons): (Vec<_>, Vec<_>) = std::mem::take(icon_instances)
            .into_iter()
            .partition(|icon| !is_drag(icon.motion.flags));
        normal_icons.extend(dragged_icons);
        *icon_instances = normal_icons;
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
        let owned = self.grid_items_owned();
        let items: Vec<grid::GridItem<'_>> = owned
            .iter()
            .map(|entry| grid::GridItem {
                key: entry.key.as_str(),
                name: entry.name.as_str(),
                uv: entry.uv,
                preview_uvs: &entry.preview_uvs,
            })
            .collect();
        let (w, _h) = self.viewport_phys();
        let visible_items = self.visible_launcher_items();
        let anim = self.edit_anim(&visible_items);
        let mut tile_instances = self.layout.build_instances(w as f32, &items, &anim);
        self.apply_tile_spring_positions(&visible_items, &mut tile_instances);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &items, &anim);
        self.apply_icon_spring_offsets(&visible_items, w as f32, &mut icon_instances);
        self.suppress_active_folder_grid_icons(&mut icon_instances);
        self.refresh_grid_glass_lanes(w as f32, &items, &visible_items, &tile_instances);
        self.lift_dragged_instances(&mut tile_instances, &mut icon_instances);
        self.render_model.tiles = Some(tile_instances);
        self.render_model.icons = Some(icon_instances);
    }
}

fn folder_item_id(item: &LauncherItem) -> Option<UiId> {
    matches!(item, LauncherItem::Folder(_)).then(|| UiId::launcher_item(item.stable_key()))
}

fn suppress_folder_preview_icons(icons: &mut Vec<IconView>, folder_key: &str) {
    let preview_ids: BTreeSet<_> = (0..9)
        .map(|slot| UiId::launcher_preview(folder_key, slot))
        .collect();
    icons.retain(|icon| !preview_ids.contains(&icon.id));
}

fn align_glass_to_tiles(surfaces: &mut [GlassSurface], tiles: &[TileView]) {
    for surface in surfaces
        .iter_mut()
        .filter(|surface| surface.behavior == GlassBehavior::Scrolling)
    {
        let Some(tile) = tiles.iter().find(|tile| tile.id == surface.id) else {
            continue;
        };
        let center = tile.rect.center();
        surface.rect.x = center.x - surface.rect.width * 0.5;
        surface.rect.y = center.y - surface.rect.height * 0.5;
    }
}

fn split_folder_glass_surfaces(
    surfaces: Vec<GlassSurface>,
    folder_ids: &BTreeSet<UiId>,
    excluded_folder_ids: &BTreeSet<UiId>,
) -> (Vec<GlassSurface>, Vec<GlassSurface>) {
    let frame = surfaces
        .iter()
        .find(|surface| surface.behavior == GlassBehavior::FixedFrame)
        .cloned();
    let mut base = Vec::with_capacity(surfaces.len());
    let mut folders = Vec::new();
    for surface in surfaces {
        if folder_ids.contains(&surface.id) {
            if !excluded_folder_ids.contains(&surface.id) {
                folders.push(surface);
            }
        } else {
            base.push(surface);
        }
    }
    if !folders.is_empty() {
        if let Some(mut clip) = frame {
            clip.id = UiId::backdrop("folder-grid-clip");
            clip.behavior = GlassBehavior::ClipOnly;
            clip.z = -1;
            folders.insert(0, clip);
        }
    }
    (base, folders)
}

fn fit_folder_glass_to_tile(surfaces: &mut [GlassSurface], size: f32, radius: f32) {
    for surface in surfaces
        .iter_mut()
        .filter(|surface| surface.behavior == GlassBehavior::Scrolling)
    {
        let center = surface.rect.center();
        surface.rect = Rect::new(center.x - size * 0.5, center.y - size * 0.5, size, size);
        surface.radius = radius;
    }
}

fn launcher_item_tile_flags(item: &LauncherItem) -> u32 {
    if matches!(item, LauncherItem::Folder(_)) {
        grid::TileAnim::FLAG_NO_BADGE | grid::TileAnim::FLAG_NO_FILL
    } else {
        0
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{app_id::AppId, folders::FolderId};
    use crate::ui_model::render_model::{IconSource, IconView};

    fn icon(id: UiId) -> IconView {
        IconView {
            id,
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            source: IconSource::Placeholder,
            motion: grid::TileAnim::IDLE,
            motion_pivot: None,
            z: 0,
        }
    }

    #[test]
    fn folders_suppress_fill_and_badge_while_apps_do_not() {
        let folder = LauncherItem::Folder(FolderId::generate(1));
        assert_eq!(
            launcher_item_tile_flags(&folder),
            grid::TileAnim::FLAG_NO_BADGE | grid::TileAnim::FLAG_NO_FILL
        );

        let app = LauncherItem::App(AppId::from_normalized("app"));
        assert_eq!(launcher_item_tile_flags(&app), 0);
    }

    #[test]
    fn folder_glass_is_separated_from_the_page_union() {
        let folder = LauncherItem::Folder(FolderId::generate(1));
        let folder_id = folder_item_id(&folder).unwrap();
        let frame = GlassSurface {
            id: UiId::backdrop("page-frame"),
            rect: Rect::new(0.0, 0.0, 500.0, 400.0),
            radius: 40.0,
            material: GlassMaterial::Regular,
            behavior: GlassBehavior::FixedFrame,
            z: -10,
        };
        let app_id = UiId::launcher_item("app");
        let app = GlassSurface {
            id: app_id.clone(),
            rect: Rect::new(20.0, 20.0, 100.0, 100.0),
            radius: 28.0,
            material: GlassMaterial::Regular,
            behavior: GlassBehavior::Scrolling,
            z: 0,
        };
        let folder_surface = GlassSurface {
            id: folder_id.clone(),
            ..app.clone()
        };
        let (base, mut overlay) = split_folder_glass_surfaces(
            vec![frame, app, folder_surface],
            &BTreeSet::from([folder_id.clone()]),
            &BTreeSet::new(),
        );
        fit_folder_glass_to_tile(&mut overlay, 84.0, 19.0);

        assert!(base.iter().any(|surface| surface.id == app_id));
        assert!(!base.iter().any(|surface| surface.id == folder_id));
        assert_eq!(overlay[0].behavior, GlassBehavior::ClipOnly);
        assert_eq!(overlay[1].id, folder_id);
        assert_eq!(overlay[1].rect.width, 84.0);
        assert_eq!(overlay[1].rect.height, 84.0);
        assert_eq!(overlay[1].radius, 19.0);
    }

    #[test]
    fn active_folder_preview_icons_are_removed_without_touching_other_icons() {
        let mut icons = vec![
            icon(UiId::launcher_preview("folder:active", 0)),
            icon(UiId::launcher_preview("folder:active", 8)),
            icon(UiId::launcher_preview("folder:other", 0)),
            icon(UiId::launcher_item("app")),
        ];

        suppress_folder_preview_icons(&mut icons, "folder:active");

        assert_eq!(
            icons.iter().map(|icon| icon.id.clone()).collect::<Vec<_>>(),
            vec![
                UiId::launcher_preview("folder:other", 0),
                UiId::launcher_item("app"),
            ]
        );
    }
}
