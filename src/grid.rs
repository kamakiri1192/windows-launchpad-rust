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
use crate::scroll::ScrollBounds;

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

    /// Return the app index under a screen-space pointer, if it hits a real app tile.
    ///
    /// `screen_x` / `screen_y` are physical window pixels. `scroll_x` is the
    /// current scroller position; the renderer displays `content_x - scroll_x`,
    /// so hit testing maps back with `content_x = screen_x + scroll_x`.
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

        let content_x = screen_x + scroll_x;
        if content_x < 0.0 {
            return None;
        }

        let page = (content_x / viewport_w).floor() as usize;
        if page >= self.page_count {
            return None;
        }

        let x_in_page = content_x - page as f32 * viewport_w - self.margin_left;
        let y_in_grid = screen_y - self.margin_top;
        if x_in_page < 0.0 || y_in_grid < 0.0 {
            return None;
        }

        let step_x = self.tile_size + self.gap;
        let step_y = self.tile_size + self.row_gap;
        let col = (x_in_page / step_x).floor() as usize;
        let row = (y_in_grid / step_y).floor() as usize;
        if col >= self.cols || row >= self.rows {
            return None;
        }

        let tile_x = col as f32 * step_x;
        let tile_y = row as f32 * step_y;
        if x_in_page > tile_x + self.tile_size || y_in_grid > tile_y + self.tile_size {
            return None;
        }

        let per_page = self.cols * self.rows;
        let index = page * per_page + row * self.cols + col;
        (index < app_count).then_some(index)
    }

    /// Produce the flat list of tile instances for the current layout.
    ///
    /// Each page is laid out within its own viewport-wide "slot": the grid is
    /// centered via `margin_left`, and page `p` starts at `p * viewport_w`.
    /// Because the scroller also moves one viewport per page, every page is
    /// centered on screen at rest — regardless of window size.
    ///
    /// Tiles are filled left-to-right, top-to-bottom across pages. Each tile
    /// takes its icon index from `apps[i]` if present (and if that app has an
    /// icon UV); otherwise it falls back to the HSL color tile with
    /// `icon_index = -1`.
    pub fn build_instances(&self, viewport_w: f32, apps: &[AppEntry]) -> Vec<TileInstance> {
        let per_page = self.cols * self.rows;
        let mut out = Vec::with_capacity(self.total_tiles());
        for p in 0..self.page_count {
            // Each page occupies one viewport width; the grid is centered
            // inside it by `margin_left`, so it sits at viewport center.
            let page_origin_x = (p as f32) * viewport_w;
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let idx = p * per_page + r * self.cols + c;
                    let x =
                        page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
                    let y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);

                    // App at this slot, if any.
                    let (r_, g_, b_, icon_index) = match apps.get(idx) {
                        Some(app) => {
                            let (r, g, b) = app_color(idx);
                            // icon_index >= 0 only if the app has an icon.
                            let ii = if app.uv.is_some() { idx as f32 } else { -1.0 };
                            (r, g, b, ii)
                        }
                        None => {
                            // No app for this slot: dummy color tile, no icon.
                            let (r, g, b) = hsl_to_rgb((idx as f32) * 0.0273, 0.62, 0.58);
                            (r, g, b, -1.0)
                        }
                    };
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
            }
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
        apps: &[AppEntry],
    ) -> Vec<crate::icon_pipeline::IconInstance> {
        let per_page = self.cols * self.rows;
        let mut out = Vec::with_capacity(self.total_tiles());
        for p in 0..self.page_count {
            let page_origin_x = (p as f32) * viewport_w;
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let idx = p * per_page + r * self.cols + c;
                    let Some(app) = apps.get(idx) else {
                        continue;
                    };
                    let Some(uv) = app.uv else {
                        continue;
                    };
                    let x =
                        page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
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
            }
        }
        out
    }

    /// Build the label list for the current layout.
    ///
    /// Each label sits below its tile, horizontally centered, with a max
    /// width slightly wider than the tile so two lines can fit. The label
    /// text comes from `apps[i].name` when available, otherwise a dummy name.
    pub fn build_labels(&self, viewport_w: f32, apps: &[AppEntry]) -> Vec<crate::text::Label> {
        let per_page = self.cols * self.rows;
        let mut out = Vec::with_capacity(self.total_tiles());
        for p in 0..self.page_count {
            let page_origin_x = (p as f32) * viewport_w;
            for r in 0..self.rows {
                for c in 0..self.cols {
                    let idx = p * per_page + r * self.cols + c;
                    let tile_x =
                        page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
                    let tile_y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);
                    let label_w = self.tile_size + 20.0; // a little wider than the tile
                    let label_x = tile_x + (self.tile_size - label_w) * 0.5;
                    // 12px below the tile bottom.
                    let label_y = tile_y + self.tile_size + 8.0;
                    let name = apps
                        .get(idx)
                        .map(|a| a.name.as_str())
                        .unwrap_or_else(|| DUMMY_NAMES[idx % DUMMY_NAMES.len()]);
                    out.push(crate::text::Label {
                        text: name.to_string(),
                        x: label_x,
                        y: label_y,
                        max_width: label_w,
                    });
                }
            }
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

    /// Build a minimal app list of `n` entries, half with icons (UV set),
    /// half without — exercises both code paths.
    fn fake_apps(n: usize) -> Vec<AppEntry> {
        (0..n)
            .map(|i| AppEntry {
                name: format!("App{i}"),
                uv: if i % 2 == 0 {
                    Some(crate::icons::UvRect {
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
        assert_eq!(g.total_tiles(), 7 * 5 * 3);
        assert_eq!(
            g.build_instances(1280.0, &fake_apps(g.total_tiles())).len(),
            g.total_tiles()
        );
    }

    #[test]
    fn pages_are_offset_by_one_viewport() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let inst = g.build_instances(vw, &fake_apps(g.total_tiles()));
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
        let inst = g.build_instances(vw, &fake_apps(g.total_tiles()));
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
    fn hit_test_ignores_tile_gaps() {
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
            g.hit_test_app(vw, screen_x, screen_y, vw, g.total_tiles()),
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
        let inst = g.build_instances(vw, &apps);
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
    fn missing_app_falls_back_to_no_icon() {
        // Empty app list → every tile is a dummy color tile with icon_index -1.
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let inst = g.build_instances(vw, &[]);
        assert!(inst.iter().all(|t| t.icon_index == -1.0));
    }
}
