//! Edit-mode pure geometry and hit regions.
//!
//! This is the Phase 4 layout counterpart of [`crate::layout::grid`] and
//! [`crate::layout::bottom_control`]. It owns the renderer-neutral geometry and
//! hit classification that edit mode (iOS-style long-press → drag-to-reorder →
//! hide) consumes:
//!
//! - the edit badge center / radius / slop (shared by the renderer's badge
//!   source geometry and the pointer hit-test, so a visible badge always clicks
//!   where it renders);
//! - the empty-cell drop hit (a thin wrapper over
//!   [`GridLayout::hit_test_tile_cell`] that explicitly excludes the label
//!   area, mirroring the rule that app *launch* includes the label slop but
//!   edit *drop* does not);
//! - the edge-autoscroll zone, with its gutter clamp so the rightmost tile
//!   columns stay reachable as drop targets;
//! - the edge-autoscroll *target* decision (one page toward the edge the
//!   dragged icon is held in);
//! - the reorder insert-index decision (pure calculation that
//!   [`crate::features::edit_mode`] uses to compute the new visible order).
//!
//! Behavior preservation: every calculation here is the exact body the
//! historical `main.rs` helpers (`badge_hit`, `edit_drop_index_at_pointer`,
//! `maybe_autoscroll_edit_drag`, `live_reorder`) performed inline, expressed as
//! pure functions so they can be unit-tested without `wgpu`/`winit`/`Win32`. The
//! app boundary (`main.rs`) still runs the side effects (`registry.set_order`,
//! `scroller.settle_to_page`, redraw) — this module only decides geometry and
//! intent.
//!
//! The edit-mode Done capsule and settings gear are **not** re-implemented here.
//! They already live in [`crate::layout::bottom_control`] (Phase 2) and are
//! reached through [`BottomControlPointerIntent::EditGear`]; edit mode reuses
//! that boundary rather than duplicating the gear geometry.

use crate::layout::grid::GridLayout;
use crate::ui_model::geometry::{Insets, Point, Rect};

/// Configured (pre-gutter-clamp) width of the edit-mode edge-autoscroll zone,
/// in 100% DPI design px. Matches the historical `EDIT_EDGE_SCROLL_ZONE`
/// constant in `main.rs`.
pub const EDIT_EDGE_SCROLL_ZONE: f32 = 72.0;
/// Floor for the configured zone, in physical px, after scaling. Matches the
/// historical `.max(24.0)` clamp in `maybe_autoscroll_edit_drag`.
pub const EDGE_ZONE_FLOOR: f32 = 24.0;
/// Cap for the configured zone, as a fraction of the page-frame panel width.
/// Matches the historical `.min(panel_w * 0.25)` clamp.
pub const EDGE_ZONE_PANEL_FRAC: f32 = 0.25;
/// Fraction of a destination tile reserved for the deliberate app-on-app or
/// app-on-folder merge gesture. The surrounding area remains a normal reorder
/// target, preventing a pass across a tile from being captured as a folder.
pub const FOLDER_MERGE_ZONE_FRACTION: f32 = 0.52;

pub fn folder_merge_zone(tile: Rect) -> Rect {
    let inset_x = tile.width * (1.0 - FOLDER_MERGE_ZONE_FRACTION) * 0.5;
    let inset_y = tile.height * (1.0 - FOLDER_MERGE_ZONE_FRACTION) * 0.5;
    tile.inset(Insets::new(inset_y, inset_x, inset_y, inset_x))
}

pub fn folder_merge_intent(tile: Rect, pointer: Point) -> bool {
    pointer.x.is_finite()
        && pointer.y.is_finite()
        && tile.width > 0.0
        && tile.height > 0.0
        && folder_merge_zone(tile).contains(pointer)
}

/// Edit badge geometry for one tile, produced from the same calculation the
/// renderer's badge source uses. `base_center` is the badge center before the
/// wiggle animation offset; `radius` is the rendered badge radius; `hit_radius`
/// is `radius + slop`, the circle the pointer hit-test uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditBadgeGeometry {
    pub base_center: (f32, f32),
    pub radius: f32,
    pub hit_radius: f32,
}

/// The inset of the badge center from the tile's top-left corner, as a fraction
/// of the rendered badge radius. Shared by the renderer's badge source geometry
/// and this hit-test so a visible badge always clicks where it renders.
pub const BADGE_CENTER_INSET_FRAC: f32 = 0.45;

impl EditBadgeGeometry {
    /// Resolve the badge geometry for the tile at visible index `idx`.
    ///
    /// `tile_x` / `tile_y` are the tile's *screen-space* top-left (i.e. its
    /// content position plus the current scroll offset), mirroring the
    /// `tile.x` / `tile.y` the renderer uploads and the historical
    /// `main.rs::badge_hit` calculation (`tx + scroll_x + inset`).
    pub fn for_tile(layout: &GridLayout, tile_x: f32, tile_y: f32) -> Self {
        let radius = layout.edit_badge_radius();
        let inset = radius * BADGE_CENTER_INSET_FRAC;
        let hit_radius = radius + layout.edit_badge_hit_slop();
        Self {
            base_center: (tile_x + inset, tile_y + inset),
            radius,
            hit_radius,
        }
    }

    /// True if `(x, y)` is inside the badge hit circle (`radius + slop`). This
    /// is the exact test the historical `main.rs::badge_hit` performed.
    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        let dx = x - self.base_center.0;
        let dy = y - self.base_center.1;
        dx * dx + dy * dy <= self.hit_radius * self.hit_radius
    }
}

/// Resolve the badge hit-test for the tile at visible index `idx`, against a
/// screen-space pointer `(x, y)`. Mirrors the historical `main.rs::badge_hit`:
/// the tile position is computed in content space, shifted by `scroll_x`, then
/// the badge circle (radius + slop, centered at `tile + inset`) is tested.
///
/// Returns `false` for a non-finite pointer or scroll to match the grid
/// hit-test's defensive guards.
pub fn badge_hit(
    layout: &GridLayout,
    viewport_w: f32,
    x: f32,
    y: f32,
    scroll_x: f32,
    idx: usize,
) -> bool {
    if !x.is_finite() || !y.is_finite() || !scroll_x.is_finite() {
        return false;
    }
    let (tx, ty) = layout.tile_position(viewport_w, idx);
    let geom = EditBadgeGeometry::for_tile(layout, tx + scroll_x, ty);
    geom.contains_point(x, y)
}

/// Resolve the drop cell (visible index) under a screen-space pointer for an
/// edit-mode drag, excluding the label area. This is a thin, explicit wrapper
/// over [`GridLayout::hit_test_tile_cell`] with `total_tiles` as the cell bound:
///
/// - app *launch* uses [`GridLayout::hit_test_app`] (app-count-bounded,
///   `include_label = true` — the label slop widens the clickable cell);
/// - edit *drop* uses the cell variant (cell-count-bounded,
///   `include_label = false` — the label band is **not** a drop target, and the
///   empty slot immediately after the last visible app on the current page is a
///   valid drop target).
pub fn drop_cell_at(
    layout: &GridLayout,
    viewport_w: f32,
    x: f32,
    y: f32,
    scroll_x: f32,
) -> Option<usize> {
    layout.hit_test_tile_cell(viewport_w, x, y, scroll_x, layout.total_tiles())
}

/// The configured (pre-gutter-clamp) edge-autoscroll zone width in physical px,
/// after scaling. Clamped to `panel_w * 0.25` and floored at `EDGE_ZONE_FLOOR`,
/// matching the historical `maybe_autoscroll_edit_drag` computation.
pub fn configured_edge_zone(layout: &GridLayout, panel_w: f32) -> f32 {
    layout
        .scaled(EDIT_EDGE_SCROLL_ZONE)
        .min(panel_w * EDGE_ZONE_PANEL_FRAC)
        .max(EDGE_ZONE_FLOOR)
}

/// The actual left/right edge-autoscroll zones, clamped to the gutter between
/// the page-frame panel edge and the grid edge.
///
/// The page-frame panel is wider than the grid (the grid is centered inside
/// it), so the configured zone would otherwise overlap the outermost tile
/// columns and make them unreachable as drop targets. The historical code
/// clamps the zone to the gutter:
///
/// ```text
/// left_zone  = zone.min((grid_left  - panel_left).max(0))
/// right_zone = zone.min((panel_right - grid_right).max(0))
/// ```
///
/// This preserves page-edge dragging while keeping normal drop targets on the
/// right side of the grid reachable (see `docs/EDIT_MODE_VISUAL_QA.md`).
pub fn edge_autoscroll_zones(
    zone: f32,
    panel_left: f32,
    panel_right: f32,
    grid_left: f32,
    grid_right: f32,
) -> (f32, f32) {
    let left_zone = zone.min((grid_left - panel_left).max(0.0));
    let right_zone = zone.min((panel_right - grid_right).max(0.0));
    (left_zone, right_zone)
}

/// Inputs to the edge-autoscroll target decision. Grouped so the decision
/// function does not carry a long parameter list and so callers build the
/// snapshot once per frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeAutoscrollInput {
    /// The lifted icon's current pointer position (physical px).
    pub drag: (f32, f32),
    /// Page-frame panel left / right / top / bottom edges (physical px).
    pub panel: (f32, f32, f32, f32),
    /// Clamped left / right gutter zone widths (physical px), from
    /// [`edge_autoscroll_zones`].
    pub zones: (f32, f32),
    /// Current page index the scroller is settled on.
    pub current_page: usize,
    /// Total page count.
    pub page_count: usize,
}

/// Decide whether an edit-mode drag should trigger a one-page autoscroll, and
/// in which direction. Returns the target page index (one less or one more than
/// `current_page`), or `None` when no autoscroll should fire.
///
/// Mirrors the historical `maybe_autoscroll_edit_drag` decision:
/// - the drag y must be within the page panel's vertical span (`panel_top..=panel_bottom`);
/// - the left zone only fires when `current_page > 0` and `drag_x` is within
///   `[panel_left, panel_left + left_zone]`;
/// - the right zone only fires when `current_page + 1 < page_count` and
///   `drag_x` is within `[panel_right - right_zone, panel_right]`;
/// - a zero-width zone (no gutter on that side) never fires, so autoscroll is
///   only reachable in the actual gutter.
///
/// Whether the scroller is in a state that allows a new settle (`Idle`) and the
/// actual `settle_to_page` call are the app boundary's responsibility — this
/// function only decides the target.
pub fn edge_autoscroll_target(input: &EdgeAutoscrollInput) -> Option<usize> {
    let (drag_x, drag_y) = input.drag;
    let (panel_left, panel_right, panel_top, panel_bottom) = input.panel;
    let (left_zone, right_zone) = input.zones;
    if drag_y < panel_top || drag_y > panel_bottom {
        return None;
    }
    if left_zone > 0.0 && drag_x <= panel_left + left_zone && input.current_page > 0 {
        return Some(input.current_page - 1);
    }
    if right_zone > 0.0
        && drag_x >= panel_right - right_zone
        && input.current_page + 1 < input.page_count
    {
        return Some(input.current_page + 1);
    }
    None
}

/// Decide the visible insertion index for a live reorder, or `None` when the
/// dragged app should stay put. Mirrors the historical `live_reorder` decision:
///
/// ```text
/// insert_idx = target_idx.min(visible_len)
/// if insert_idx == drag_pos { None } else { Some(insert_idx) }
/// ```
///
/// `target_idx` is the drop cell under the pointer (from [`drop_cell_at`]),
/// `visible_len` is the current visible-app count, and `drag_pos` is the dragged
/// app's current position in the visible stream. The caller performs the actual
/// registry mutation (keyed by stable `AppId`).
pub fn reorder_insert_index(
    visible_len: usize,
    drag_pos: usize,
    target_idx: usize,
) -> Option<usize> {
    let insert_idx = target_idx.min(visible_len);
    if insert_idx == drag_pos {
        None
    } else {
        Some(insert_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> GridLayout {
        GridLayout::default().centered(1280.0)
    }

    #[test]
    fn folder_merge_requires_the_central_overlap_zone() {
        let tile = Rect::new(100.0, 200.0, 100.0, 100.0);
        let zone = folder_merge_zone(tile);
        assert_eq!(zone, Rect::new(124.0, 224.0, 52.0, 52.0));
        assert!(folder_merge_intent(tile, Point::new(150.0, 250.0)));
        assert!(!folder_merge_intent(tile, Point::new(105.0, 250.0)));
        assert!(!folder_merge_intent(tile, Point::new(150.0, 205.0)));
    }

    #[test]
    fn folder_merge_rejects_invalid_geometry_and_pointer() {
        assert!(!folder_merge_intent(
            Rect::new(0.0, 0.0, 0.0, 84.0),
            Point::new(0.0, 0.0)
        ));
        assert!(!folder_merge_intent(
            Rect::new(0.0, 0.0, 84.0, 84.0),
            Point::new(f32::NAN, 20.0)
        ));
    }

    // ---- badge geometry / hit ------------------------------------------------

    #[test]
    fn badge_geometry_matches_renderer_inset_and_hit_slop() {
        let g = layout();
        let (tx, ty) = g.tile_position(1280.0, 0);
        let geom = EditBadgeGeometry::for_tile(&g, tx, ty);
        // Rendered radius matches edit_badge_radius (the shader's badge radius).
        assert!((geom.radius - g.edit_badge_radius()).abs() < 1e-4);
        // Hit radius = radius + slop.
        assert!((geom.hit_radius - (g.edit_badge_radius() + g.edit_badge_hit_slop())).abs() < 1e-4);
        // Base center sits at tile + inset (0.45 * radius), matching the
        // renderer's edit_badge_sources geometry.
        let inset = g.edit_badge_radius() * BADGE_CENTER_INSET_FRAC;
        assert!((geom.base_center.0 - (tx + inset)).abs() < 1e-4);
        assert!((geom.base_center.1 - (ty + inset)).abs() < 1e-4);
    }

    #[test]
    fn badge_hit_accepts_point_inside_hit_circle() {
        let g = layout();
        let (tx, ty) = g.tile_position(1280.0, 2);
        let scroll_x = 0.0;
        // The badge center itself must hit.
        assert!(badge_hit(
            &g,
            1280.0,
            tx + scroll_x + g.edit_badge_radius() * BADGE_CENTER_INSET_FRAC,
            ty + g.edit_badge_radius() * BADGE_CENTER_INSET_FRAC,
            scroll_x,
            2
        ));
    }

    #[test]
    fn badge_hit_rejects_point_outside_hit_circle() {
        let g = layout();
        let (tx, ty) = g.tile_position(1280.0, 2);
        // The tile center is well outside the badge hit circle.
        let center_x = tx + g.tile_size * 0.5;
        let center_y = ty + g.tile_size * 0.5;
        assert!(!badge_hit(&g, 1280.0, center_x, center_y, 0.0, 2));
    }

    #[test]
    fn badge_hit_uses_slop_for_forgiving_touch_target() {
        let g = layout();
        let (tx, ty) = g.tile_position(1280.0, 0);
        let inset = g.edit_badge_radius() * BADGE_CENTER_INSET_FRAC;
        let cx = tx + inset;
        let cy = ty + inset;
        // Just inside the hit radius (radius + slop) hits.
        let on_edge = cx + g.edit_badge_radius() + g.edit_badge_hit_slop() * 0.9;
        assert!(badge_hit(&g, 1280.0, on_edge, cy, 0.0, 0));
        // Just outside the hit radius misses.
        let off_edge = cx + g.edit_badge_radius() + g.edit_badge_hit_slop() * 1.1;
        assert!(!badge_hit(&g, 1280.0, off_edge, cy, 0.0, 0));
    }

    // ---- drop cell hit (empty cell / rightmost / label area) ----------------

    #[test]
    fn drop_cell_allows_empty_slot_after_last_visible_app() {
        let g = layout();
        let (x0, y) = g.tile_position(1280.0, 0);
        let step_x = g.tile_size + g.gap;
        // The cell immediately after the only visible app is a valid drop
        // target even though hit_test_app returns None there.
        let x = x0 + step_x + g.tile_size * 0.5;
        let cy = y + g.tile_size * 0.5;
        assert_eq!(drop_cell_at(&g, 1280.0, x, cy, 0.0), Some(1));
    }

    #[test]
    fn drop_cell_allows_rightmost_two_columns() {
        let g = layout();
        let step_x = g.tile_size + g.gap;
        let y = g.margin_top + g.tile_size * 0.5;
        for col in [g.cols - 2, g.cols - 1] {
            let x = g.margin_left + col as f32 * step_x + g.tile_size * 0.5;
            assert_eq!(
                drop_cell_at(&g, 1280.0, x, y, 0.0),
                Some(col),
                "column {col} should be a reachable drop target"
            );
        }
    }

    #[test]
    fn drop_cell_excludes_label_area() {
        let g = layout();
        let (x, y) = g.tile_position(1280.0, 0);
        // A point in the label band below the tile: app *launch* would resolve
        // to this tile (label slop), but edit *drop* must not (label excluded).
        let label_x = x + g.tile_size * 0.5;
        let label_y = y + g.tile_size + g.scaled(24.0);
        assert!(label_y > y + g.tile_size, "sanity: label is below the tile");
        // hit_test_app would return Some(0) here (label included), but
        // drop_cell_at must return None for the label band.
        assert_eq!(drop_cell_at(&g, 1280.0, label_x, label_y, 0.0), None);
    }

    // ---- edge autoscroll zones ----------------------------------------------

    #[test]
    fn configured_edge_zone_clamps_to_panel_frac_and_floor() {
        let g = layout();
        // Wide panel: zone is the scaled configured value (72 * scale = 72 at
        // scale 1.0), under the 0.25 * panel_w cap.
        assert!((configured_edge_zone(&g, 1000.0) - 72.0).abs() < 1e-4);
        // Very narrow panel: the 0.25 cap kicks in.
        assert!((configured_edge_zone(&g, 100.0) - 25.0).abs() < 1e-4);
        // Pathological: below the 24px floor (can't happen with a real panel
        // because page_width clamps to grid_w, but the floor guards it).
        assert!((configured_edge_zone(&g, 10.0) - 24.0).abs() < 1e-4);
    }

    #[test]
    fn edge_zones_clamp_to_gutter_between_panel_and_grid() {
        // panel spans [100, 900], grid spans [300, 700]; configured zone is 250.
        // left gutter = 200, right gutter = 200, so both zones clamp to 200.
        let (lz, rz) = edge_autoscroll_zones(250.0, 100.0, 900.0, 300.0, 700.0);
        assert!((lz - 200.0).abs() < 1e-4);
        assert!((rz - 200.0).abs() < 1e-4);
        // With a small configured zone, neither side is clamped.
        let (lz, rz) = edge_autoscroll_zones(50.0, 100.0, 900.0, 300.0, 700.0);
        assert!((lz - 50.0).abs() < 1e-4);
        assert!((rz - 50.0).abs() < 1e-4);
    }

    #[test]
    fn edge_zones_are_zero_when_grid_meets_panel() {
        // No gutter on either side → zones are zero, so autoscroll is
        // unreachable (the rightmost tile columns stay drop targets).
        let (lz, rz) = edge_autoscroll_zones(250.0, 100.0, 900.0, 100.0, 900.0);
        assert!((lz - 0.0).abs() < 1e-4);
        assert!((rz - 0.0).abs() < 1e-4);
    }

    // ---- edge autoscroll target decision ------------------------------------

    /// Build an input with a panel [100, 900] x [100, 700] and 80px zones on
    /// both sides, varying only the drag position and current page.
    fn autoscroll_input(
        drag: (f32, f32),
        current_page: usize,
        page_count: usize,
    ) -> EdgeAutoscrollInput {
        EdgeAutoscrollInput {
            drag,
            panel: (100.0, 900.0, 100.0, 700.0),
            zones: (80.0, 80.0),
            current_page,
            page_count,
        }
    }

    #[test]
    fn autoscroll_left_when_icon_in_left_gutter_and_not_first_page() {
        let input = autoscroll_input((110.0, 400.0), 2, 4);
        let target = edge_autoscroll_target(&input);
        assert_eq!(target, Some(1));
        // Sanity check the input survived the borrow.
        assert_eq!(input.current_page, 2);
    }

    #[test]
    fn autoscroll_right_when_icon_in_right_gutter_and_not_last_page() {
        let target = edge_autoscroll_target(&autoscroll_input((890.0, 400.0), 1, 4));
        assert_eq!(target, Some(2));
    }

    #[test]
    fn autoscroll_does_not_fire_on_first_page_left() {
        // current_page = 0: the left zone is unreachable (no previous page).
        let target = edge_autoscroll_target(&autoscroll_input((110.0, 400.0), 0, 4));
        assert_eq!(target, None);
    }

    #[test]
    fn autoscroll_does_not_fire_on_last_page_right() {
        // current_page = 3, page_count = 4: the right zone is unreachable.
        let target = edge_autoscroll_target(&autoscroll_input((890.0, 400.0), 3, 4));
        assert_eq!(target, None);
    }

    #[test]
    fn autoscroll_does_not_fire_when_icon_off_panel_vertically() {
        // drag_y above the panel top → no autoscroll even in the gutter.
        let target = edge_autoscroll_target(&autoscroll_input((110.0, 50.0), 2, 4));
        assert_eq!(target, None);
    }

    #[test]
    fn autoscroll_does_not_fire_when_zone_is_zero() {
        // No gutter → zone is zero → no autoscroll, the rightmost column stays a
        // drop target.
        let input = EdgeAutoscrollInput {
            drag: (110.0, 400.0),
            panel: (100.0, 900.0, 100.0, 700.0),
            zones: (0.0, 0.0),
            current_page: 2,
            page_count: 4,
        };
        let target = edge_autoscroll_target(&input);
        assert_eq!(target, None);
    }

    #[test]
    fn autoscroll_does_not_fire_over_tile_columns() {
        // drag_x is over the grid (not in either gutter) → no autoscroll.
        let target = edge_autoscroll_target(&autoscroll_input((500.0, 400.0), 1, 4));
        assert_eq!(target, None);
    }

    // ---- reorder insert index -----------------------------------------------

    #[test]
    fn reorder_insert_index_clamps_to_visible_len() {
        // target_idx past the end clamps to visible_len (drop at the tail).
        assert_eq!(reorder_insert_index(3, 0, 10), Some(3));
    }

    #[test]
    fn reorder_insert_index_none_when_target_equals_drag_pos() {
        // No move needed → None so the caller skips the registry mutation.
        assert_eq!(reorder_insert_index(5, 2, 2), None);
    }

    #[test]
    fn reorder_insert_index_clamps_then_compares() {
        // target_idx clamps to visible_len and equals drag_pos → None.
        assert_eq!(reorder_insert_index(3, 3, 10), None);
    }

    #[test]
    fn reorder_insert_index_moves_to_earlier_cell() {
        assert_eq!(reorder_insert_index(5, 4, 1), Some(1));
    }
}
