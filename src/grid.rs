//! Paged grid layout that produces the static tile instances drawn by the GPU.
//!
//! All geometry here is in **physical pixels** and expressed relative to the
//! *content* origin (which the scroller shifts horizontally). The renderer
//! converts these into clip space at draw time.
//!
//! Layout (per page):
//! ```text
//!   ┌──────────── viewport (page_extent) ────────────┐
//!   │  margin                                        │
//!   │   ┌──┬──┬──┬──┬──┬──┬──┐                       │
//!   │   ├──┼──┼──┼──┼──┼──┼──┤   rows = 5           │
//!   │   ├──┼──┼──┼──┼──┼──┼──┤   cols = 7           │
//!   │   ├──┼──┼──┼──┼──┼──┼──┤                       │
//!   │   ├──┼──┼──┼──┼──┼──┼──┤                       │
//!   │   └──┴──┴──┴──┴──┴──┴──┘                       │
//!   │  margin                                        │
//!   └────────────────────────────────────────────────┘
//! ```

use crate::icons::AppEntry;
use crate::icons::UvRect;
use crate::scroll::ScrollBounds;

const LABEL_CLICK_EXTRA_X: f32 = 10.0;
const LABEL_CLICK_EXTRA_Y: f32 = 42.0;

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

/// One drawable tile, matching the WGSL `@location(0..3)` instance attributes.
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
    /// Reuses the old `_pad` slot so the struct stays 32 bytes.
    pub icon_index: f32,
}

impl TileInstance {
    /// Vertex attributes describing this struct for `wgpu::VertexBufferLayout`.
    pub const ATTRIBS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x3, 3 => Float32];

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
            tile_size: 84.0,
            gap: 22.0,
            row_gap: 48.0,
            margin_top: 96.0,
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

    /// Build the scroll bounds implied by this layout & viewport.
    pub fn bounds(&self, viewport_w: f32) -> ScrollBounds {
        ScrollBounds {
            page_extent: viewport_w,
            page_count: self.page_count,
        }
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
        if viewport_w <= 0.0
            || !viewport_w.is_finite()
            || !screen_x.is_finite()
            || !screen_y.is_finite()
            || !scroll_x.is_finite()
        {
            return None;
        }

        let content_x = screen_x - scroll_x;
        if content_x < 0.0 {
            return None;
        }

        let page = (content_x / viewport_w).floor() as usize;
        if page >= self.page_count {
            return None;
        }

        let x_in_page = content_x - page as f32 * viewport_w - self.margin_left;
        let y_in_grid = screen_y - self.margin_top;
        if y_in_grid < 0.0 {
            return None;
        }

        let step_x = self.tile_size + self.gap;
        let step_y = self.tile_size + self.row_gap;
        let col = ((x_in_page + LABEL_CLICK_EXTRA_X) / step_x).floor() as usize;
        let row = (y_in_grid / step_y).floor() as usize;
        if col >= self.cols || row >= self.rows {
            return None;
        }

        let tile_x = col as f32 * step_x;
        let tile_y = row as f32 * step_y;
        let in_tile = x_in_page >= tile_x
            && x_in_page <= tile_x + self.tile_size
            && y_in_grid >= tile_y
            && y_in_grid <= tile_y + self.tile_size;
        let in_label = x_in_page >= tile_x - LABEL_CLICK_EXTRA_X
            && x_in_page <= tile_x + self.tile_size + LABEL_CLICK_EXTRA_X
            && y_in_grid >= tile_y + self.tile_size
            && y_in_grid <= tile_y + self.tile_size + LABEL_CLICK_EXTRA_Y;
        if !in_tile && !in_label {
            return None;
        }

        let per_page = self.cols * self.rows;
        let index = page * per_page + row * self.cols + col;
        (index < app_count).then_some(index)
    }

    /// Produce the flat list of tile instances for real apps in the current layout.
    ///
    /// Each page is laid out within its own viewport-wide "slot": the grid is
    /// centered via `margin_left`, and page `p` starts at `p * viewport_w`.
    /// Because the scroller also moves one viewport per page, every page is
    /// centered on screen at rest — regardless of window size.
    ///
    /// Tiles are filled left-to-right, top-to-bottom across pages. Apps without
    /// loaded icon UVs still get color fallback tiles. Empty slots after the
    /// last app are skipped.
    pub fn build_instances(&self, viewport_w: f32, apps: &[GridApp<'_>]) -> Vec<TileInstance> {
        let per_page = self.cols * self.rows;
        let app_count = apps.len().min(self.total_tiles());
        let mut out = Vec::with_capacity(app_count);
        for (idx, app) in apps.iter().take(app_count).enumerate() {
            let p = idx / per_page;
            let row_in_page = idx % per_page;
            let r = row_in_page / self.cols;
            let c = row_in_page % self.cols;
            let page_origin_x = (p as f32) * viewport_w;
            let x = page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
            let y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);
            let (r_, g_, b_) = app_color(idx);
            let icon_index = if app.uv.is_some() { idx as f32 } else { -1.0 };
            out.push(TileInstance {
                x,
                y,
                size: self.tile_size,
                radius: 19.0,
                r: r_,
                g: g_,
                b: b_,
                icon_index,
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
    ) -> Vec<crate::icon_pipeline::IconInstance> {
        let per_page = self.cols * self.rows;
        let app_count = apps.len().min(self.total_tiles());
        let mut out = Vec::with_capacity(app_count);
        for (idx, app) in apps.iter().take(app_count).enumerate() {
            let Some(uv) = app.uv else {
                continue;
            };
            let p = idx / per_page;
            let row_in_page = idx % per_page;
            let r = row_in_page / self.cols;
            let c = row_in_page % self.cols;
            let page_origin_x = (p as f32) * viewport_w;
            let x = page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
            let y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);
            out.push(crate::icon_pipeline::IconInstance {
                x,
                y,
                size: self.tile_size,
                radius: 19.0,
                u0: uv.u0,
                v0: uv.v0,
                u1: uv.u1,
                v1: uv.v1,
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
        let mut out = Vec::with_capacity(app_count);
        for (idx, app) in apps.iter().take(app_count).enumerate() {
            let p = idx / per_page;
            let row_in_page = idx % per_page;
            let r = row_in_page / self.cols;
            let c = row_in_page % self.cols;
            let page_origin_x = (p as f32) * viewport_w;
            let tile_x = page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
            let tile_y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);
            let label_w = self.tile_size + 20.0; // a little wider than the tile
            let label_x = tile_x + (self.tile_size - label_w) * 0.5;
            let label_y = tile_y + self.tile_size + 8.0;
            out.push(crate::text::Label {
                text: app.name.to_string(),
                x: label_x,
                y: label_y,
                max_width: label_w,
            });
        }
        out
    }
}

/// Stable per-app accent color, derived from the app's grid index so the same
/// app always gets the same color across relayouts. Used as the fallback tile
/// color when no icon is available, and as the squircle tint behind icons.
fn app_color(idx: usize) -> (f32, f32, f32) {
    hsl_to_rgb((idx as f32) * 0.0273, 0.62, 0.58)
}

/// macOS-Launchpad-flavored dummy app names (Japanese), cycled across tiles.
const DUMMY_NAMES: &[&str] = &[
    "メモ",
    "設定",
    "写真",
    "メール",
    "マップ",
    "カレンダー",
    "時計",
    "天気",
    "リマインダー",
    "メッセージ",
    "FaceTime",
    "App Store",
    "Safari",
    "音楽",
    "Podcasts",
    "TV",
    "ホーム",
    "ヘルス",
    "Wallet",
    "計算機",
    "ボイスメモ",
    "コンパス",
    "ショートカット",
    "翻訳",
    "ファイル",
    "ヒント",
    "プレビュー",
    "テキストエディット",
    "グラブ",
    "ディスクユーティリティ",
    "アクティビティモニタ",
    "システム環境設定",
    "メール",
    "連絡先",
    "メモ",
];

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
            g.build_instances(1280.0, &view(&apps)).len(),
            g.total_tiles()
        );
    }

    #[test]
    fn pages_are_offset_by_one_viewport() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let apps = fake_apps(g.total_tiles());
        let inst = g.build_instances(vw, &view(&apps));
        let p0 = inst[0].x;
        let p1 = inst[7 * 5].x; // first tile of page 1
                                // Page 1's first tile must be exactly one viewport to the right.
        assert!(
            (p1 - p0 - vw).abs() < 1e-2,
            "pages spaced by viewport width"
        );
    }

    #[test]
    fn grid_is_centered_in_viewport() {
        // The first tile of page 0 sits at margin_left, which centers the grid.
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let apps = fake_apps(g.total_tiles());
        let inst = g.build_instances(vw, &view(&apps));
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
        let screen_x = g.margin_left + g.tile_size * 0.5;
        let screen_y = g.margin_top + g.tile_size * 0.5;

        assert_eq!(
            g.hit_test_app(vw, screen_x, screen_y, -vw, g.total_tiles()),
            Some(per_page)
        );
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
        let inst = g.build_instances(vw, &view(&apps));
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
        let inst = g.build_instances(vw, &view(&apps));
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
        assert_eq!(g.build_instances(vw, &view(&apps)).len(), app_count);
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
