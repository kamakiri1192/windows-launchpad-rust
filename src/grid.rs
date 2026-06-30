//! Paged grid layout that produces the static tile instances drawn by the GPU.
//!
//! All geometry here is in **physical pixels** and expressed relative to the
//! *content* origin (which the scroller shifts horizontally). The renderer
//! converts these into clip space at draw time.
//!
//! One page spans the **content width**, which is the liquid-glass page-frame
//! panel width (`grid_w + FRAME_PADDING_WIDTH`, clamped to the viewport). This
//! is narrower than the full viewport, so pages slide in adjacent to each other
//! with a small gutter — like iOS Launchpad — rather than leaving a wide empty
//! gap. `page_extent` (the scroller's per-page stride) equals this content
//! width, and tile pages are spaced by it too, keeping snap targets aligned
//! with the actual tile pages at every window size.
//!
//! Layout (per page):
//! ```text
//!   ┌────────── content width (page_extent) ──────────┐
//!   │  ┌──┬──┬──┬──┬──┬──┬──┐                        │
//!   │  ├──┼──┼──┼──┼──┼──┼──┤   rows = 5            │
//!   │  ├──┼──┼──┼──┼──┼──┼──┤   cols = 7            │
//!   │  ├──┼──┼──┼──┼──┼──┼──┤                        │
//!   │  ├──┼──┼──┼──┼──┼──┼──┤                        │
//!   │  └──┴──┴──┴──┴──┴──┴──┘                        │
//!   └─────────────────────────────────────────────────┘
//!         ↑ centered in the viewport via margin_left
//! ```

use crate::icons::AppEntry;
use crate::icons::UvRect;
use crate::scroll::ScrollBounds;

const LABEL_CLICK_EXTRA_X: f32 = 10.0;
const LABEL_CLICK_EXTRA_Y: f32 = 42.0;

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

/// Minimal view of one app that the layout needs: a label and an optional
/// atlas UV. This decouples [`GridLayout`] from whichever concrete app-list
/// type owns the full records (old `AppEntry`, new `AppRecord`, …), so the
/// grid code doesn't churn when the registry changes.
#[derive(Debug, Clone, Copy)]
pub struct GridApp<'a> {
    pub name: &'a str,
    pub uv: Option<UvRect>,
}

impl<'a> From<&'a AppEntry> for GridApp<'a> {
    fn from(a: &'a AppEntry) -> Self {
        Self {
            name: &a.name,
            uv: a.uv,
        }
    }
}

/// Per-app edit-mode animation parameters, packed into the tile/icon instance
/// `extra` vec4.
///
/// - `phase` — wiggle phase offset (seconds), per-app so icons wobble out of
///   sync. Ignored unless `FLAG_WIGGLE` is set.
/// - `lift` — vertical lift in physical px (dragged icon rises above the grid).
/// - `scale` — uniform scale multiplier (dragged icon is enlarged).
/// - `flags` — bitfield; bit 0 = wiggling, bit 1 = dragged (bypass frame clip).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TileAnim {
    pub phase: f32,
    pub lift: f32,
    pub scale: f32,
    pub flags: u32,
}

impl TileAnim {
    /// Bit set in `flags` while edit mode is active (icon should wiggle).
    pub const FLAG_WIGGLE: u32 = 1 << 0;
    /// Bit set in `flags` while this icon is the one being dragged (lifted,
    /// pointer-following, frame clip bypassed).
    pub const FLAG_DRAG: u32 = 1 << 1;

    /// An all-zero animation (the resting state — no wiggle, no lift).
    pub const IDLE: Self = Self {
        phase: 0.0,
        lift: 0.0,
        scale: 1.0,
        flags: 0,
    };

    #[inline]
    fn to_extra(self) -> [f32; 4] {
        [self.phase, self.lift, self.scale, self.flags as f32]
    }
}

/// One drawable tile, matching the WGSL `@location(0..4)` instance attributes.
/// 48 bytes for clean GPU alignment.
///
/// `extra` carries the edit-mode animation parameters:
/// `(phase, lift, scale, flags)` where flags bit 0 = wiggling and bit 1 = being
/// dragged (lifted + pointer-following, frame clip bypassed).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TileInstance {
    /// Top-left corner of the tile in content pixels.
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub radius: f32,
    /// sRGB-ish color packed as linear RGB in 0..1.
    pub r: f32,
    pub g: f32,
    pub b: f32,
    /// Icon index into the atlas. `-1.0` means "no icon → render the color
    /// tile as a fallback". Otherwise it's the atlas entry index as a float.
    pub icon_index: f32,
    /// Edit-mode animation: `(phase, lift, scale, flags)`.
    pub extra: [f32; 4],
}

impl TileInstance {
    /// Vertex attributes describing this struct for `wgpu::VertexBufferLayout`.
    pub const ATTRIBS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
        0 => Float32x2,
        1 => Float32x2,
        2 => Float32x3,
        3 => Float32,
        4 => Float32x4
    ];

    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<TileInstance>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &TileInstance::ATTRIBS,
    };
}

/// Page geometry that scales with the window.
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
            tile_size: 84.0,
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

    /// Build the scroll bounds implied by this layout & viewport.
    pub fn bounds(&self, viewport_w: f32) -> ScrollBounds {
        ScrollBounds {
            page_extent: self.page_width(viewport_w),
            page_count: self.page_count,
        }
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

    /// Produce the flat list of tile instances for real apps in the current layout.
    ///
    /// Each page is laid out within its own content-wide "slot": the grid is
    /// centered via `margin_left`, and page `p` starts at `p * page_w` where
    /// `page_w` is the liquid-glass page-frame width. Because the scroller also
    /// advances one page width per page, every page is centered on screen at
    /// rest — regardless of window size — and pages slide in adjacent to each
    /// other with a small gutter, like iOS Launchpad.
    ///
    /// Tiles are filled left-to-right, top-to-bottom across pages. Apps without
    /// loaded icon UVs still get color fallback tiles. Empty slots after the
    /// last app are skipped.
    pub fn build_instances(
        &self,
        viewport_w: f32,
        apps: &[GridApp<'_>],
        anim: &[TileAnim],
    ) -> Vec<TileInstance> {
        let per_page = self.cols * self.rows;
        let app_count = apps.len().min(self.total_tiles());
        let page_w = self.page_width(viewport_w);
        let mut out = Vec::with_capacity(app_count);
        for (idx, app) in apps.iter().take(app_count).enumerate() {
            let p = idx / per_page;
            let row_in_page = idx % per_page;
            let r = row_in_page / self.cols;
            let c = row_in_page % self.cols;
            let page_origin_x = (p as f32) * page_w;
            let x = page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
            let y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);
            let (r_, g_, b_) = app_color(idx);
            let icon_index = if app.uv.is_some() { idx as f32 } else { -1.0 };
            let anim = anim.get(idx).copied().unwrap_or(TileAnim::IDLE);
            out.push(TileInstance {
                x,
                y,
                size: self.tile_size,
                radius: self.scaled(19.0),
                r: r_,
                g: g_,
                b: b_,
                icon_index,
                extra: anim.to_extra(),
            });
        }
        out
    }

    /// Build per-icon instance data: one entry per tile that has an icon UV.
    ///
    /// Tiles without an app or whose app has no icon are skipped (the fallback
    /// color tile from `build_instances` shows through underneath).
    pub fn build_icon_instances(
        &self,
        viewport_w: f32,
        apps: &[GridApp<'_>],
        anim: &[TileAnim],
    ) -> Vec<crate::icon_pipeline::IconInstance> {
        let per_page = self.cols * self.rows;
        let app_count = apps.len().min(self.total_tiles());
        let page_w = self.page_width(viewport_w);
        let mut out = Vec::with_capacity(app_count);
        for (idx, app) in apps.iter().take(app_count).enumerate() {
            let Some(uv) = app.uv else {
                continue;
            };
            let p = idx / per_page;
            let row_in_page = idx % per_page;
            let r = row_in_page / self.cols;
            let c = row_in_page % self.cols;
            let page_origin_x = (p as f32) * page_w;
            let x = page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
            let y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);
            let anim = anim.get(idx).copied().unwrap_or(TileAnim::IDLE);
            out.push(crate::icon_pipeline::IconInstance {
                x,
                y,
                size: self.tile_size,
                radius: self.scaled(19.0),
                u0: uv.u0,
                v0: uv.v0,
                u1: uv.u1,
                v1: uv.v1,
                extra: anim.to_extra(),
            });
        }
        out
    }

    /// Build the label list for the current layout.
    ///
    /// Each label sits below its tile, horizontally centered, with a max
    /// width slightly wider than the tile so two lines can fit. The label
    /// text comes from `apps[i].name`; empty slots after the last app are skipped.
    pub fn build_labels(&self, viewport_w: f32, apps: &[GridApp<'_>]) -> Vec<crate::text::Label> {
        let per_page = self.cols * self.rows;
        let app_count = apps.len().min(self.total_tiles());
        let page_w = self.page_width(viewport_w);
        let mut out = Vec::with_capacity(app_count);
        for (idx, app) in apps.iter().take(app_count).enumerate() {
            let p = idx / per_page;
            let row_in_page = idx % per_page;
            let r = row_in_page / self.cols;
            let c = row_in_page % self.cols;
            let page_origin_x = (p as f32) * page_w;
            let tile_x = page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
            let tile_y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);
            let label_w = self.tile_size + self.scaled(20.0); // a little wider than the tile
            let label_x = tile_x + (self.tile_size - label_w) * 0.5;
            let label_y = tile_y + self.tile_size + self.scaled(8.0);
            out.push(crate::text::Label {
                text: app.name.to_string(),
                x: label_x,
                y: label_y,
                max_width: label_w,
            });
        }
        out
    }

    #[inline]
    pub fn scaled(&self, value: f32) -> f32 {
        value * self.scale
    }

    /// The home-cell top-left position (content px) for the app at display
    /// index `idx`, mirroring what `build_instances` computes. Used to drive
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
}

fn sanitize_scale(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

/// Stable per-app accent color, derived from the app's grid index so the same
/// app always gets the same color across relayouts. Used as the fallback tile
/// color when no icon is available, and as the squircle tint behind icons.
fn app_color(idx: usize) -> (f32, f32, f32) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Owned app-list helper for tests (so `GridApp` borrows stable storage).
    struct OwnedApp {
        name: String,
        uv: Option<UvRect>,
    }

    /// Build a minimal app list of `n` entries, half with icons (UV set),
    /// half without — exercises both code paths.
    fn fake_apps(n: usize) -> Vec<OwnedApp> {
        (0..n)
            .map(|i| OwnedApp {
                name: format!("App{i}"),
                uv: if i % 2 == 0 {
                    Some(UvRect {
                        u0: 0.0,
                        v0: 0.0,
                        u1: 0.1,
                        v1: 0.1,
                    })
                } else {
                    None
                },
            })
            .collect()
    }

    /// Map owned apps to borrowed grid views.
    fn view<'a>(apps: &'a [OwnedApp]) -> Vec<GridApp<'a>> {
        apps.iter()
            .map(|a| GridApp {
                name: a.name.as_str(),
                uv: a.uv,
            })
            .collect()
    }

    // Keep the legacy AppEntry builder around so the public `From<&AppEntry>`
    // impl stays exercised (and compiles even when unused by other tests).
    #[allow(dead_code)]
    fn fake_app_entries(n: usize) -> Vec<AppEntry> {
        (0..n)
            .map(|i| AppEntry {
                name: format!("App{i}"),
                uv: if i % 2 == 0 {
                    Some(UvRect {
                        u0: 0.0,
                        v0: 0.0,
                        u1: 0.1,
                        v1: 0.1,
                    })
                } else {
                    None
                },
                link_path: PathBuf::new(),
            })
            .collect()
    }

    #[test]
    fn counts_match() {
        let g = GridLayout::default().centered(1280.0);
        let apps = fake_apps(g.total_tiles());
        assert_eq!(g.total_tiles(), 7 * 5 * 3);
        assert_eq!(
            g.build_instances(1280.0, &view(&apps), &[]).len(),
            g.total_tiles()
        );
    }

    #[test]
    fn pages_are_offset_by_one_page_width() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let apps = fake_apps(g.total_tiles());
        let inst = g.build_instances(vw, &view(&apps), &[]);
        let page_w = g.page_width(vw);
        let p0 = inst[0].x;
        let p1 = inst[7 * 5].x; // first tile of page 1
                                // Page 1's first tile must be exactly one page width to the right.
        assert!(
            (p1 - p0 - page_w).abs() < 1e-2,
            "pages spaced by the content page width"
        );
    }

    #[test]
    fn page_width_is_panel_width_and_narrower_than_viewport() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let page_w = g.page_width(vw);
        let grid_w = g.grid_w();
        // page width = grid_w + frame padding, clamped to viewport - gutter.
        let expected = (grid_w + FRAME_PADDING_WIDTH)
            .min(vw - FRAME_VIEWPORT_GUTTER)
            .max(grid_w);
        assert!(
            (page_w - expected).abs() < 1e-2,
            "page width matches panel width"
        );
        assert!(
            page_w < vw,
            "page width should be narrower than the full viewport"
        );
        assert!(
            page_w > grid_w,
            "page width should be wider than the bare grid (has frame gutter)"
        );
    }

    #[test]
    fn page_width_clamps_to_viewport_gutter_when_window_is_narrow() {
        // A very narrow window must keep the minimum viewport gutter and never
        // shrink below the grid itself.
        let g = GridLayout::default().centered(600.0);
        let page_w = g.page_width(600.0);
        let grid_w = g.grid_w();
        // 600 - 48 = 552 < grid_w (7*84 + 6*22 = 720), so the .max(grid_w) arm
        // kicks in.
        assert!(
            (page_w - grid_w).abs() < 1e-2,
            "page width clamps to the grid width when the viewport is too narrow"
        );
    }

    #[test]
    fn frame_leaves_room_for_bottom_control_outside_panel() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let (_cx, cy, _w, h) = g.frame_panel_rect(vw);
        let frame_bottom = cy + h * 0.5;

        assert!(
            frame_bottom <= 724.0 + 1e-2,
            "page frame must leave room below for the separate search control"
        );
    }

    #[test]
    fn bounds_page_extent_equals_page_width() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let bounds = g.bounds(vw);
        assert!(
            (bounds.page_extent - g.page_width(vw)).abs() < 1e-2,
            "scroll page_extent must equal the content page width"
        );
    }

    #[test]
    fn grid_is_centered_in_viewport() {
        // The first tile of page 0 sits at margin_left, which centers the grid.
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let apps = fake_apps(g.total_tiles());
        let inst = g.build_instances(vw, &view(&apps), &[]);
        let grid_w = g.cols as f32 * g.tile_size + (g.cols - 1) as f32 * g.gap;
        let expected_left = (vw - grid_w) * 0.5;
        assert!(
            (inst[0].x - expected_left).abs() < 1e-2,
            "first tile x should center the grid"
        );
    }

    #[test]
    fn hit_test_finds_first_tile() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let x = g.margin_left + g.tile_size * 0.5;
        let y = g.margin_top + g.tile_size * 0.5;

        assert_eq!(g.hit_test_app(vw, x, y, 0.0, g.total_tiles()), Some(0));
    }

    #[test]
    fn hit_test_includes_label_area() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let x = g.margin_left + g.tile_size * 0.5;
        let y = g.margin_top + g.tile_size + 24.0;

        assert_eq!(g.hit_test_app(vw, x, y, 0.0, g.total_tiles()), Some(0));
    }

    #[test]
    fn tile_cell_hit_test_excludes_label_area_for_edit_drop() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let x = g.margin_left + g.tile_size * 0.5;
        let y = g.margin_top + g.tile_size + 24.0;

        assert_eq!(g.hit_test_tile_cell(vw, x, y, 0.0, g.total_tiles()), None);
    }

    #[test]
    fn tile_cell_hit_test_allows_empty_slots() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let step_x = g.tile_size + g.gap;
        let x = g.margin_left + step_x + g.tile_size * 0.5;
        let y = g.margin_top + g.tile_size * 0.5;

        assert_eq!(g.hit_test_app(vw, x, y, 0.0, 1), None);
        assert_eq!(g.hit_test_tile_cell(vw, x, y, 0.0, 2), Some(1));
    }

    #[test]
    fn tile_cell_hit_test_allows_rightmost_column() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let step_x = g.tile_size + g.gap;
        let y = g.margin_top + g.tile_size * 0.5;

        for col in [g.cols - 2, g.cols - 1] {
            let x = g.margin_left + col as f32 * step_x + g.tile_size * 0.5;
            assert_eq!(
                g.hit_test_tile_cell(vw, x, y, 0.0, g.total_tiles()),
                Some(col),
                "column {col} should be reachable"
            );
        }
    }

    #[test]
    fn hit_test_ignores_space_between_app_cells() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let x = g.margin_left + g.tile_size + g.gap * 0.5;
        let y = g.margin_top + g.tile_size * 0.5;

        assert_eq!(g.hit_test_app(vw, x, y, 0.0, g.total_tiles()), None);
    }

    #[test]
    fn hit_test_accounts_for_scroll_position() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let per_page = g.cols * g.rows;
        // Scrolling left by exactly one page width lands the first tile of
        // page 1 under the pointer that page 0's first tile started at.
        let page_w = g.page_width(vw);
        let screen_x = g.margin_left + g.tile_size * 0.5;
        let screen_y = g.margin_top + g.tile_size * 0.5;

        assert_eq!(
            g.hit_test_app(vw, screen_x, screen_y, -page_w, g.total_tiles()),
            Some(per_page)
        );
    }

    #[test]
    fn scaled_layout_keeps_label_hit_area_with_scaled_text() {
        let scale = 1.5;
        let vw = 1920.0;
        let g = GridLayout::default().with_scale_factor(scale).centered(vw);
        assert!((g.tile_size - 126.0).abs() < 1e-2);
        assert!((g.row_gap - 72.0).abs() < 1e-2);

        let apps = fake_apps(1);
        let labels = g.build_labels(vw, &view(&apps));
        let label = &labels[0];
        assert!((label.y - (g.margin_top + g.tile_size + 8.0 * scale)).abs() < 1e-2);
        assert!((label.max_width - (g.tile_size + 20.0 * scale)).abs() < 1e-2);

        let x = g.margin_left + g.tile_size * 0.5;
        let y = g.margin_top + g.tile_size + 41.0 * scale;
        assert_eq!(g.hit_test_app(vw, x, y, 0.0, apps.len()), Some(0));
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
    fn hit_test_ignores_empty_slots() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let x = g.margin_left + g.tile_size * 0.5;
        let y = g.margin_top + g.tile_size * 0.5;

        assert_eq!(g.hit_test_app(vw, x, y, 0.0, 0), None);
    }

    #[test]
    fn icon_index_reflects_icon_presence() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let apps = fake_apps(g.total_tiles());
        let inst = g.build_instances(vw, &view(&apps), &[]);
        // fake_apps gives even indices an icon (uv.is_some()).
        for (i, tile) in inst.iter().enumerate() {
            if apps[i].uv.is_some() {
                assert_eq!(
                    tile.icon_index, i as f32,
                    "icon tile should carry its index"
                );
            } else {
                assert_eq!(tile.icon_index, -1.0, "icon-less tile should fall back");
            }
        }
    }

    #[test]
    fn empty_app_list_draws_no_tiles() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let apps: Vec<OwnedApp> = vec![];
        let inst = g.build_instances(vw, &view(&apps), &[]);
        assert!(inst.is_empty());
    }

    #[test]
    fn partial_final_page_draws_only_real_apps() {
        let vw = 1280.0;
        let per_page = 7 * 5;
        let app_count = per_page + 3;
        let g = GridLayout::for_app_count(app_count).centered(vw);
        let apps = fake_apps(app_count);

        assert_eq!(g.page_count, 2);
        assert_eq!(g.build_instances(vw, &view(&apps), &[]).len(), app_count);
        assert_eq!(g.build_labels(vw, &view(&apps)).len(), app_count);
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
        assert_eq!(GridLayout::for_app_count(132).page_count, 4);
        assert!(GridLayout::for_app_count(132).total_tiles() >= 132);
    }
}
