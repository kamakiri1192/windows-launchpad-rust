//! Pure paged-grid geometry and hit classification.
//!
//! This is the renderer-neutral, testable core of the launcher grid. It owns
//! the page-frame rectangle, the per-cell tile/label rectangles, the scroll page
//! extent, and the hit classification that pointer routing consumes. It is the
//! Phase 3 counterpart of [`crate::layout::settings_panel`] and
//! [`crate::layout::bottom_control`].
//!
//! All geometry here is in **physical pixels** and expressed relative to the
//! *content* origin (which the scroller shifts horizontally). One page spans
//! the **content width** — the liquid-glass page-frame panel width
//! (`grid_w + FRAME_PADDING_WIDTH`, clamped to the viewport). This is narrower
//! than the full viewport, so pages slide in adjacent to each other with a
//! small gutter — like iOS Launchpad.
//!
//! Behavior preservation: the GPU-facing instance builders
//! (`TileInstance`/`IconInstance`/`text::Label`) and the
//! [`scroll::ScrollBounds`](crate::scroll::ScrollBounds)-returning `bounds()`
//! adapter stay in the binary [`crate::grid`] module. This module only owns the
//! pure geometry and hit classification so it can compile and be unit-tested as
//! part of the library target without pulling in `wgpu`/`winit`/`Win32`.

const LABEL_CLICK_EXTRA_X: f32 = 10.0;
const LABEL_CLICK_EXTRA_Y: f32 = 42.0;
pub const BASE_TILE_SIZE: f32 = 84.0;

// --- Fixed page-frame geometry --------------------------------------------
// Single source of truth for the glass panel that surrounds the tiles. These
// tune the panel's extra padding around the grid and its corner radius. They
// are shared by `GridLayout::frame_panel_rect`, the liquid-glass shape build,
// and the GPU-side rounded-rect clip in the tile/icon/text shaders.
/// Extra height the panel adds to the raw grid height (incl. label rows).
pub const FRAME_EXTRA_HEIGHT: f32 = 52.0;
/// Extra height the panel adds *beyond* `FRAME_EXTRA_HEIGHT`.
pub const FRAME_PADDING_HEIGHT: f32 = 38.0;
/// Extra width the panel adds around the grid.
pub const FRAME_PADDING_WIDTH: f32 = 112.0;
/// Inset from the viewport edge the panel keeps (minimum gutter).
pub const FRAME_VIEWPORT_GUTTER: f32 = 48.0;
/// How far the panel's top sits above the first tile row (margin_top offset).
pub const FRAME_TOP_OFFSET: f32 = 34.0;
/// Corner radius of the page frame.
pub const FRAME_CORNER_RADIUS: f32 = 54.0;

/// Page geometry that scales with the window.
///
/// This is a pure data struct: the constructor and computation methods on it
/// contain no renderer/platform dependencies. The binary
/// [`crate::grid`] module adds the GPU-facing instance builders and the
/// [`ScrollBounds`](crate::scroll::ScrollBounds) adapter on top of it.
#[derive(Debug, Clone, Copy)]
pub struct GridLayout {
    pub cols: usize,
    pub rows: usize,
    pub page_count: usize,
    /// Window DPI scale factor used to convert 100% DPI logical design units
    /// into the physical-pixel geometry consumed by the renderer.
    pub scale: f32,
    /// Tile side length in physical px.
    pub tile_size: f32,
    pub gap: f32,
    pub row_gap: f32,
    pub margin_top: f32,
    pub margin_left: f32,
}

impl Default for GridLayout {
    fn default() -> Self {
        Self {
            cols: 7,
            rows: 5,
            page_count: 3,
            scale: 1.0,
            tile_size: BASE_TILE_SIZE,
            gap: 22.0,
            row_gap: 48.0,
            margin_top: 56.0,
            margin_left: 0.0, // recomputed per-viewport to center the grid
        }
    }
}

impl GridLayout {
    /// Build a layout sized to hold `app_count` apps. `page_count` grows with
    /// the app count so every app is reachable by scrolling. Always keeps at
    /// least one page, but does not create blank trailing pages.
    pub fn for_app_count(app_count: usize) -> Self {
        let base = Self::default();
        let per_page = base.cols * base.rows;
        let needed = app_count.div_ceil(per_page);
        let page_count = needed.max(1);
        Self { page_count, ..base }
    }

    /// Convert the 100% DPI layout constants into physical pixels for the
    /// current monitor scale factor. Keep `cols`/`rows` stable; only distances
    /// and radii scale.
    pub fn with_scale_factor(mut self, scale_factor: f32) -> Self {
        let scale = sanitize_scale(scale_factor);
        let ratio = scale / sanitize_scale(self.scale);
        self.scale = scale;
        self.tile_size *= ratio;
        self.gap *= ratio;
        self.row_gap *= ratio;
        self.margin_top *= ratio;
        self.margin_left *= ratio;
        self
    }

    /// Total tiles across all pages.
    pub fn total_tiles(&self) -> usize {
        self.cols * self.rows * self.page_count
    }

    /// Recompute the left margin so the grid is centered horizontally in a
    /// viewport of `width` px. Returns an updated layout.
    pub fn centered(mut self, width: f32) -> Self {
        let grid_w =
            self.cols as f32 * self.tile_size + (self.cols.saturating_sub(1)) as f32 * self.gap;
        self.margin_left = ((width - grid_w) * 0.5).max(0.0);
        self
    }

    /// The content width of a single page — the liquid-glass page-frame panel
    /// width. This is the single source of truth that both the scroller's
    /// `page_extent` and the tile pages' stride derive from, so snap targets
    /// always line up with the actual tile pages at every window size.
    ///
    /// It is the grid width plus the frame's horizontal padding, clamped to
    /// keep a minimum viewport gutter and never narrower than the grid itself.
    pub fn page_width(&self, viewport_w: f32) -> f32 {
        let grid_w = self.grid_w();
        (grid_w + self.scaled(FRAME_PADDING_WIDTH))
            .min(viewport_w - self.scaled(FRAME_VIEWPORT_GUTTER))
            .max(grid_w)
    }

    /// The scroll page extent for this layout & viewport, in physical px.
    /// This equals [`Self::page_width`]; it exists so the pure geometry layer
    /// can express the page stride without depending on
    /// [`scroll::ScrollBounds`](crate::scroll::ScrollBounds). The binary
    /// [`crate::grid`] adapter wraps this into a `ScrollBounds`.
    pub fn page_extent(&self, viewport_w: f32) -> f32 {
        self.page_width(viewport_w)
    }

    /// Raw grid width (cols of tiles + gaps) in physical pixels.
    pub fn grid_w(&self) -> f32 {
        self.cols as f32 * self.tile_size + (self.cols.saturating_sub(1)) as f32 * self.gap
    }

    /// Raw grid height (rows of tiles + gaps + label allowance) in physical px.
    pub fn grid_h(&self) -> f32 {
        self.rows as f32 * self.tile_size
            + (self.rows.saturating_sub(1)) as f32 * self.row_gap
            + self.scaled(FRAME_EXTRA_HEIGHT)
    }

    /// Return the fixed page-frame panel rectangle in content/screen pixels.
    /// This is the single source of truth for the glass panel that surrounds
    /// the tiles and stays put while tiles scroll beneath.
    ///
    /// Returns `(center_x, center_y, width, height)` in physical pixels.
    pub fn frame_panel_rect(&self, viewport_w: f32) -> (f32, f32, f32, f32) {
        let grid_w = self.grid_w();
        let grid_h = self.grid_h();
        // The panel width equals one content page; see `page_width`.
        let panel_w = self.page_width(viewport_w);
        let panel_h = grid_h + self.scaled(FRAME_PADDING_HEIGHT);
        let center_x = self.margin_left + grid_w * 0.5;
        let center_y = self.margin_top - self.scaled(FRAME_TOP_OFFSET) + panel_h * 0.5;
        (center_x, center_y, panel_w, panel_h)
    }

    /// Return the app index under a screen-space pointer, if it hits a real app cell.
    ///
    /// `screen_x` / `screen_y` are physical window pixels. The renderer draws
    /// each tile at `content_x + scroll_x`, so hit testing maps back with
    /// `content_x = screen_x - scroll_x`. The clickable region intentionally
    /// includes the label area, not just the icon square, because this is an app
    /// launcher rather than a pure icon atlas demo.
    pub fn hit_test_app(
        &self,
        viewport_w: f32,
        screen_x: f32,
        screen_y: f32,
        scroll_x: f32,
        app_count: usize,
    ) -> Option<usize> {
        self.hit_test_cell(viewport_w, screen_x, screen_y, scroll_x, app_count, true)
    }

    /// Return the tile-cell index under a screen-space pointer, excluding label
    /// text and allowing callers to pass a larger `cell_count` than the number
    /// of visible apps. Used by edit-mode drag/drop so empty final-page slots
    /// can be valid drop targets.
    pub fn hit_test_tile_cell(
        &self,
        viewport_w: f32,
        screen_x: f32,
        screen_y: f32,
        scroll_x: f32,
        cell_count: usize,
    ) -> Option<usize> {
        self.hit_test_cell(viewport_w, screen_x, screen_y, scroll_x, cell_count, false)
    }

    fn hit_test_cell(
        &self,
        viewport_w: f32,
        screen_x: f32,
        screen_y: f32,
        scroll_x: f32,
        cell_count: usize,
        include_label: bool,
    ) -> Option<usize> {
        if viewport_w <= 0.0
            || !viewport_w.is_finite()
            || !screen_x.is_finite()
            || !screen_y.is_finite()
            || !scroll_x.is_finite()
        {
            return None;
        }

        if !self.frame_contains_point(viewport_w, screen_x, screen_y) {
            return None;
        }

        // Pages are spaced by the content page width, not the full viewport.
        let page_w = self.page_width(viewport_w);

        let y_in_grid = screen_y - self.margin_top;
        if y_in_grid < 0.0 {
            return None;
        }

        let step_x = self.tile_size + self.gap;
        let step_y = self.tile_size + self.row_gap;
        let label_click_extra_x = self.scaled(LABEL_CLICK_EXTRA_X);
        let label_click_extra_y = self.scaled(LABEL_CLICK_EXTRA_Y);
        let row = (y_in_grid / step_y).floor() as usize;
        if row >= self.rows {
            return None;
        }

        let tile_y = row as f32 * step_y;
        let content_x = screen_x - scroll_x;

        for page in 0..self.page_count {
            // Mirror tile placement exactly: x = page * page_w + margin_left +
            // col * step_x. The grid can be centered in the viewport while
            // page_w is narrower than the viewport, so deriving `page` from
            // content_x / page_w before subtracting margin_left misclassifies
            // the rightmost columns as the next page.
            let x_in_page = content_x - page as f32 * page_w - self.margin_left;
            if x_in_page < -label_click_extra_x {
                continue;
            }

            let col = ((x_in_page + label_click_extra_x) / step_x).floor() as usize;
            if col >= self.cols {
                continue;
            }

            let tile_x = col as f32 * step_x;
            let in_tile = x_in_page >= tile_x
                && x_in_page <= tile_x + self.tile_size
                && y_in_grid >= tile_y
                && y_in_grid <= tile_y + self.tile_size;
            let in_label = include_label
                && x_in_page >= tile_x - label_click_extra_x
                && x_in_page <= tile_x + self.tile_size + label_click_extra_x
                && y_in_grid >= tile_y + self.tile_size
                && y_in_grid <= tile_y + self.tile_size + label_click_extra_y;
            if !in_tile && !in_label {
                continue;
            }

            let per_page = self.cols * self.rows;
            let index = page * per_page + row * self.cols + col;
            if index < cell_count {
                return Some(index);
            }
        }

        None
    }

    pub fn frame_contains_point(&self, viewport_w: f32, screen_x: f32, screen_y: f32) -> bool {
        let (cx, cy, w, h) = self.frame_panel_rect(viewport_w);
        let half_w = w * 0.5;
        let half_h = h * 0.5;
        let radius = self
            .scaled(FRAME_CORNER_RADIUS)
            .min(half_w)
            .min(half_h)
            .max(0.0);
        let qx = (screen_x - cx).abs() - half_w + radius;
        let qy = (screen_y - cy).abs() - half_h + radius;
        let outside_x = qx.max(0.0);
        let outside_y = qy.max(0.0);
        let outside = (outside_x * outside_x + outside_y * outside_y).sqrt();
        let inside = qx.max(qy).min(0.0);
        outside + inside - radius <= 0.0
    }

    /// The home-cell top-left position (content px) for the app at display
    /// index `idx`, mirroring what the GPU builders compute. Used to drive
    /// per-tile position springs for reorder animations.
    pub fn tile_position(&self, viewport_w: f32, idx: usize) -> (f32, f32) {
        let per_page = self.cols * self.rows;
        let p = idx / per_page;
        let row_in_page = idx % per_page;
        let r = row_in_page / self.cols;
        let c = row_in_page % self.cols;
        let page_origin_x = (p as f32) * self.page_width(viewport_w);
        let x = page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
        let y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);
        (x, y)
    }

    #[inline]
    pub fn scaled(&self, value: f32) -> f32 {
        value * self.scale
    }

    #[inline]
    pub fn edit_badge_radius(&self) -> f32 {
        edit_badge_radius_for_tile_size(self.tile_size)
    }

    #[inline]
    pub fn edit_badge_hit_slop(&self) -> f32 {
        6.0 * self.scale
    }

    /// Classify a screen-space pointer against the grid in one calculation.
    ///
    /// This is the unified entry point pointer routing uses to decide, at press
    /// time, whether the press is over an app cell (with its visible-stream
    /// index), over empty space *inside* the page frame (swallowed — no launch,
    /// no passthrough), or *outside* the page frame (transparent launcher area
    /// → hide + click passthrough on a stationary release).
    ///
    /// `app_count` is the number of currently visible apps
    /// (`visible_app_ids().len()`); cells at or beyond it are treated as empty.
    /// The classification is exactly equivalent to combining
    /// [`Self::frame_contains_point`] and [`Self::hit_test_app`], but expressed
    /// as a single intent so callers do not duplicate the frame/empty/app
    /// decision inline.
    pub fn classify(
        &self,
        viewport_w: f32,
        screen_x: f32,
        screen_y: f32,
        scroll_x: f32,
        app_count: usize,
    ) -> GridHit {
        if !self.frame_contains_point(viewport_w, screen_x, screen_y) {
            return GridHit::OutsideFrame;
        }
        match self.hit_test_app(viewport_w, screen_x, screen_y, scroll_x, app_count) {
            Some(index) => GridHit::App(index),
            None => GridHit::EmptyInFrame,
        }
    }
}

/// Result of classifying a pointer against the launcher grid. See
/// [`GridLayout::classify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridHit {
    /// The pointer is over a visible app cell at the given visible-stream
    /// index.
    App(usize),
    /// The pointer is inside the page-frame panel but not over any app cell
    /// (gap, past the last app, or the page gutter). A stationary release here
    /// neither launches nor passes the click through.
    EmptyInFrame,
    /// The pointer is outside the page-frame panel — the transparent launcher
    /// area. A stationary release here hides the launcher and replays the click
    /// to the underlying window.
    OutsideFrame,
}

impl GridHit {
    /// The visible-stream app index, if this is an `App` hit.
    pub fn app_index(self) -> Option<usize> {
        match self {
            GridHit::App(index) => Some(index),
            _ => None,
        }
    }

    /// True when the pointer is outside the page-frame glass (transparent
    /// launcher area). Mirrors the historical `outside_glass` flag recorded on
    /// `PendingPress`.
    pub fn is_outside_frame(self) -> bool {
        matches!(self, GridHit::OutsideFrame)
    }
}

fn sanitize_scale(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

pub fn edit_badge_radius_for_tile_size(tile_size: f32) -> f32 {
    let scale = if tile_size.is_finite() && tile_size > 0.0 {
        tile_size / BASE_TILE_SIZE
    } else {
        1.0
    };
    (tile_size * 0.16).clamp(9.0 * scale, 13.5 * scale)
}

/// Stable per-app accent color, derived from the app's grid index so the same
/// app always gets the same color across relayouts. Used as the fallback tile
/// color when no icon is available, and as the squircle tint behind icons.
///
/// Pure and renderer-neutral so the layout layer (and its tests) can reason
/// about which color a placeholder tile would show.
pub fn app_color(idx: usize) -> (f32, f32, f32) {
    hsl_to_rgb((idx as f32) * 0.0273, 0.62, 0.58)
}

/// HSL → linear-ish RGB (simple conversion, good enough for placeholder art).
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(1.0);
    if s == 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let mut t = t;
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

/// Resolve a label rect (content px) for the app at display index `idx`,
/// mirroring the GPU label builder. Used so render geometry and any future
/// hit/animation reasoning share one calculation.
pub fn label_rect(layout: &GridLayout, viewport_w: f32, idx: usize) -> (f32, f32, f32, f32) {
    let (tile_x, tile_y) = layout.tile_position(viewport_w, idx);
    let label_w = layout.tile_size + layout.scaled(20.0); // a little wider than the tile
    let label_x = tile_x + (layout.tile_size - label_w) * 0.5;
    let label_y = tile_y + layout.tile_size + layout.scaled(8.0);
    (label_x, label_y, label_w, layout.scaled(20.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_frame_panel_rect_is_centered_on_grid() {
        let g = GridLayout::default().centered(1280.0);
        let (cx, cy, w, h) = g.frame_panel_rect(1280.0);
        let grid_w = g.grid_w();
        let grid_h = g.grid_h();
        let expected_w = g.page_width(1280.0);
        let expected_h = grid_h + FRAME_PADDING_HEIGHT;
        assert!((cx - (g.margin_left + grid_w * 0.5)).abs() < 1e-2);
        assert!(
            (cy - (g.margin_top - FRAME_TOP_OFFSET + expected_h * 0.5)).abs() < 1e-2,
            "center_y should sit above the first tile by the frame top offset"
        );
        assert!((w - expected_w).abs() < 1e-2);
        assert!((h - expected_h).abs() < 1e-2);
    }

    #[test]
    fn frame_contains_point_rounds_corners() {
        let g = GridLayout::default().centered(1280.0);
        let (cx, cy, w, h) = g.frame_panel_rect(1280.0);
        // Center is inside.
        assert!(g.frame_contains_point(1280.0, cx, cy));
        // A point far outside the panel misses.
        assert!(!g.frame_contains_point(1280.0, 5.0, 5.0));
        // The rounded corner gap just outside the panel corner misses.
        let half_w = w * 0.5;
        let half_h = h * 0.5;
        let corner_x = cx + half_w;
        let corner_y = cy + half_h;
        assert!(
            !g.frame_contains_point(1280.0, corner_x, corner_y),
            "panel corner should be clipped by the rounded radius"
        );
    }

    #[test]
    fn tile_position_mirrors_grid_placement() {
        let g = GridLayout::default().centered(1280.0);
        let page_w = g.page_width(1280.0);
        let step_x = g.tile_size + g.gap;
        let step_y = g.tile_size + g.row_gap;
        // Index 0: top-left of page 0.
        let (x0, y0) = g.tile_position(1280.0, 0);
        assert!((x0 - g.margin_left).abs() < 1e-2);
        assert!((y0 - g.margin_top).abs() < 1e-2);
        // Index cols: first tile of row 1.
        let (x1, y1) = g.tile_position(1280.0, g.cols);
        assert!((x1 - g.margin_left).abs() < 1e-2);
        assert!((y1 - (g.margin_top + step_y)).abs() < 1e-2);
        // Index cols*rows: first tile of page 1.
        let per_page = g.cols * g.rows;
        let (xp, yp) = g.tile_position(1280.0, per_page);
        assert!((xp - (page_w + g.margin_left)).abs() < 1e-2);
        assert!((yp - g.margin_top).abs() < 1e-2);
        // Column col on page 0.
        let col = 3;
        let (xc, _yc) = g.tile_position(1280.0, col);
        assert!((xc - (g.margin_left + col as f32 * step_x)).abs() < 1e-2);
    }

    #[test]
    fn label_rect_sits_below_tile_centered_and_slightly_wider() {
        let g = GridLayout::default().centered(1280.0);
        let (tile_x, tile_y) = g.tile_position(1280.0, 0);
        let (lx, ly, lw, _lh) = label_rect(&g, 1280.0, 0);
        let expected_w = g.tile_size + g.scaled(20.0);
        assert!((lw - expected_w).abs() < 1e-2);
        assert!((lx - (tile_x + (g.tile_size - expected_w) * 0.5)).abs() < 1e-2);
        assert!((ly - (tile_y + g.tile_size + g.scaled(8.0))).abs() < 1e-2);
    }

    #[test]
    fn hit_test_finds_each_tile_center() {
        let g = GridLayout::default().centered(1280.0);
        let per_page = g.cols * g.rows;
        // Sample across pages 0 and 1 so scroll-position handling is exercised.
        for idx in [0, 1, g.cols, g.cols - 1, per_page, per_page + 1] {
            let (content_x, content_y) = g.tile_position(1280.0, idx);
            let scroll_x = page_scroll_for(&g, idx);
            // `hit_test_app` takes screen-space coordinates; the renderer draws
            // at content + scroll, so screen = content + scroll.
            let screen_x = content_x + g.tile_size * 0.5 + scroll_x;
            let screen_y = content_y + g.tile_size * 0.5;
            assert_eq!(
                g.hit_test_app(1280.0, screen_x, screen_y, scroll_x, g.total_tiles()),
                Some(idx),
                "tile {idx} center should hit"
            );
        }
    }

    fn page_scroll_for(g: &GridLayout, idx: usize) -> f32 {
        let per_page = g.cols * g.rows;
        let page = idx / per_page;
        -(page as f32) * g.page_width(1280.0)
    }

    #[test]
    fn hit_test_includes_label_area() {
        let g = GridLayout::default().centered(1280.0);
        let (x, y) = g.tile_position(1280.0, 0);
        let cx = x + g.tile_size * 0.5;
        let cy = y + g.tile_size + g.scaled(24.0);
        assert_eq!(
            g.hit_test_app(1280.0, cx, cy, 0.0, g.total_tiles()),
            Some(0)
        );
    }

    #[test]
    fn hit_test_ignores_gaps_between_cells() {
        let g = GridLayout::default().centered(1280.0);
        let (x, y) = g.tile_position(1280.0, 0);
        let gap_x = x + g.tile_size + g.gap * 0.5;
        let cy = y + g.tile_size * 0.5;
        assert_eq!(
            g.hit_test_app(1280.0, gap_x, cy, 0.0, g.total_tiles()),
            None
        );
    }

    #[test]
    fn hit_test_ignores_empty_slots_beyond_app_count() {
        let g = GridLayout::default().centered(1280.0);
        let (x, y) = g.tile_position(1280.0, 0);
        let cx = x + g.tile_size * 0.5;
        let cy = y + g.tile_size * 0.5;
        // No apps at all → first cell is empty → no hit.
        assert_eq!(g.hit_test_app(1280.0, cx, cy, 0.0, 0), None);
    }

    #[test]
    fn tile_cell_hit_test_allows_empty_slots() {
        let g = GridLayout::default().centered(1280.0);
        let step_x = g.tile_size + g.gap;
        let x = g.margin_left + step_x + g.tile_size * 0.5;
        let y = g.margin_top + g.tile_size * 0.5;
        // hit_test_app requires a real app; hit_test_tile_cell allows empty.
        assert_eq!(g.hit_test_app(1280.0, x, y, 0.0, 1), None);
        assert_eq!(g.hit_test_tile_cell(1280.0, x, y, 0.0, 2), Some(1));
    }

    #[test]
    fn tile_cell_hit_test_allows_rightmost_columns() {
        let g = GridLayout::default().centered(1280.0);
        let step_x = g.tile_size + g.gap;
        let y = g.margin_top + g.tile_size * 0.5;
        for col in [g.cols - 2, g.cols - 1] {
            let x = g.margin_left + col as f32 * step_x + g.tile_size * 0.5;
            assert_eq!(
                g.hit_test_tile_cell(1280.0, x, y, 0.0, g.total_tiles()),
                Some(col),
                "column {col} should be reachable"
            );
        }
    }

    #[test]
    fn hit_test_accounts_for_scroll_position() {
        let g = GridLayout::default().centered(1280.0);
        let per_page = g.cols * g.rows;
        let page_w = g.page_width(1280.0);
        let screen_x = g.margin_left + g.tile_size * 0.5;
        let screen_y = g.margin_top + g.tile_size * 0.5;
        // Scrolling left by one page width lands page 1's first tile under the
        // pointer that page 0's first tile started at.
        assert_eq!(
            g.hit_test_app(1280.0, screen_x, screen_y, -page_w, g.total_tiles()),
            Some(per_page)
        );
    }

    #[test]
    fn hit_test_clips_to_rounded_frame() {
        let g = GridLayout::default().centered(600.0);
        let x = g.margin_left + 1.0;
        let y = g.margin_top + 1.0;
        assert!(!g.frame_contains_point(600.0, x, y));
        assert_eq!(g.hit_test_app(600.0, x, y, 0.0, g.total_tiles()), None);
    }

    #[test]
    fn scaled_layout_keeps_label_hit_area() {
        let scale = 1.5;
        let vw = 1920.0;
        let g = GridLayout::default().with_scale_factor(scale).centered(vw);
        assert!((g.tile_size - 126.0).abs() < 1e-2);
        assert!((g.row_gap - 72.0).abs() < 1e-2);
        let (x, y) = g.tile_position(vw, 0);
        let cx = x + g.tile_size * 0.5;
        let cy = y + g.tile_size + 41.0 * scale;
        assert_eq!(g.hit_test_app(vw, cx, cy, 0.0, 1), Some(0));
    }

    #[test]
    fn scale_factor_replaces_previous_scale_instead_of_accumulating() {
        let g = GridLayout::default()
            .with_scale_factor(1.5)
            .with_scale_factor(2.0);
        assert!((g.tile_size - 168.0).abs() < 1e-2);
        assert!((g.gap - 44.0).abs() < 1e-2);
        assert!((g.margin_top - 112.0).abs() < 1e-2);
    }

    #[test]
    fn page_extent_equals_page_width_and_is_narrower_than_viewport() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let grid_w = g.grid_w();
        let expected = (grid_w + FRAME_PADDING_WIDTH)
            .min(vw - FRAME_VIEWPORT_GUTTER)
            .max(grid_w);
        assert!((g.page_extent(vw) - expected).abs() < 1e-2);
        assert!((g.page_extent(vw) - g.page_width(vw)).abs() < 1e-2);
        assert!(g.page_extent(vw) < vw);
        assert!(g.page_extent(vw) > grid_w);
    }

    #[test]
    fn page_width_clamps_to_grid_width_when_window_is_narrow() {
        let g = GridLayout::default().centered(600.0);
        let grid_w = g.grid_w();
        // 600 - 48 = 552 < grid_w (7*84 + 6*22 = 720), so the .max(grid_w) arm
        // kicks in.
        assert!(
            (g.page_width(600.0) - grid_w).abs() < 1e-2,
            "page width clamps to the grid width when the viewport is too narrow"
        );
    }

    #[test]
    fn for_app_count_uses_only_needed_pages() {
        let per_page = 7 * 5;
        assert_eq!(GridLayout::for_app_count(10).page_count, 1);
        assert_eq!(GridLayout::for_app_count(0).page_count, 1);
        assert_eq!(GridLayout::for_app_count(per_page).page_count, 1);
        assert_eq!(GridLayout::for_app_count(per_page + 1).page_count, 2);
        assert_eq!(GridLayout::for_app_count(per_page * 3).page_count, 3);
        assert_eq!(GridLayout::for_app_count(per_page * 3 + 1).page_count, 4);
    }

    #[test]
    fn classify_outside_frame_for_passthrough() {
        let g = GridLayout::default().centered(1280.0);
        // A point in the transparent launcher area (corner of the viewport).
        assert_eq!(
            g.classify(1280.0, 5.0, 5.0, 0.0, g.total_tiles()),
            GridHit::OutsideFrame
        );
        assert!(g
            .classify(1280.0, 5.0, 5.0, 0.0, g.total_tiles())
            .is_outside_frame());
        assert_eq!(
            g.classify(1280.0, 5.0, 5.0, 0.0, g.total_tiles())
                .app_index(),
            None
        );
    }

    #[test]
    fn classify_app_for_visible_app_center() {
        let g = GridLayout::default().centered(1280.0);
        let (x, y) = g.tile_position(1280.0, 0);
        let cx = x + g.tile_size * 0.5;
        let cy = y + g.tile_size * 0.5;
        assert_eq!(
            g.classify(1280.0, cx, cy, 0.0, g.total_tiles()),
            GridHit::App(0)
        );
        assert_eq!(
            g.classify(1280.0, cx, cy, 0.0, g.total_tiles()).app_index(),
            Some(0)
        );
    }

    #[test]
    fn classify_empty_in_frame_for_gap_inside_panel() {
        let g = GridLayout::default().centered(1280.0);
        let (x, y) = g.tile_position(1280.0, 0);
        // A point between two tiles but inside the page frame.
        let gap_x = x + g.tile_size + g.gap * 0.5;
        let cy = y + g.tile_size * 0.5;
        assert!(g.frame_contains_point(1280.0, gap_x, cy));
        assert_eq!(
            g.classify(1280.0, gap_x, cy, 0.0, g.total_tiles()),
            GridHit::EmptyInFrame
        );
        assert!(!g
            .classify(1280.0, gap_x, cy, 0.0, g.total_tiles())
            .is_outside_frame());
    }

    #[test]
    fn classify_empty_in_frame_for_empty_slot_inside_panel() {
        let g = GridLayout::default().centered(1280.0);
        let (x, y) = g.tile_position(1280.0, 0);
        let cx = x + g.tile_size * 0.5;
        let cy = y + g.tile_size * 0.5;
        // No apps visible → the cell is empty but still inside the frame.
        assert!(g.frame_contains_point(1280.0, cx, cy));
        assert_eq!(g.classify(1280.0, cx, cy, 0.0, 0), GridHit::EmptyInFrame);
    }

    #[test]
    fn classify_respects_app_count_for_search_filtering() {
        // Simulates a filtered grid where only 1 app is visible: index 0 hits,
        // index 1's cell is now empty (even though total_tiles is large).
        let g = GridLayout::default().centered(1280.0);
        let (x0, y) = g.tile_position(1280.0, 0);
        let step_x = g.tile_size + g.gap;
        let cx1 = x0 + step_x + g.tile_size * 0.5;
        let cy = y + g.tile_size * 0.5;
        assert_eq!(g.classify(1280.0, cx1, cy, 0.0, 1), GridHit::EmptyInFrame);
        // The first cell is still a real app.
        let cx0 = x0 + g.tile_size * 0.5;
        assert_eq!(g.classify(1280.0, cx0, cy, 0.0, 1), GridHit::App(0));
    }

    #[test]
    fn classify_accounts_for_scroll_position() {
        let g = GridLayout::default().centered(1280.0);
        let per_page = g.cols * g.rows;
        let page_w = g.page_width(1280.0);
        let screen_x = g.margin_left + g.tile_size * 0.5;
        let screen_y = g.margin_top + g.tile_size * 0.5;
        assert_eq!(
            g.classify(1280.0, screen_x, screen_y, -page_w, g.total_tiles()),
            GridHit::App(per_page)
        );
    }

    #[test]
    fn edit_badge_radius_scales_with_layout_scale_factor() {
        let normal = GridLayout::default();
        let scaled = GridLayout::default().with_scale_factor(1.5);
        assert!((normal.edit_badge_radius() - 13.44).abs() < 1e-2);
        assert!((scaled.edit_badge_radius() - normal.edit_badge_radius() * 1.5).abs() < 1e-2);
        assert!((scaled.edit_badge_hit_slop() - 9.0).abs() < 1e-2);
    }

    #[test]
    fn app_color_is_stable_per_index() {
        assert_eq!(app_color(3), app_color(3));
        assert_ne!(app_color(0), app_color(7));
    }
}
