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
    /// Reserved / padding so the struct is 32 bytes (nice for GPU alignment).
    pub _pad: f32,
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

    /// Produce the flat list of tile instances for the current layout.
    ///
    /// Each page is laid out within its own viewport-wide "slot": the grid is
    /// centered via `margin_left`, and page `p` starts at `p * viewport_w`.
    /// Because the scroller also moves one viewport per page, every page is
    /// centered on screen at rest — regardless of window size.
    pub fn build_instances(&self, viewport_w: f32) -> Vec<TileInstance> {
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
                    let (r_, g_, b_) = hsl_to_rgb((idx as f32) * 0.0273, 0.62, 0.58);
                    out.push(TileInstance {
                        x,
                        y,
                        size: self.tile_size,
                        radius: 19.0,
                        r: r_,
                        g: g_,
                        b: b_,
                        _pad: 0.0,
                    });
                }
            }
        }
        out
    }

    /// Build the label list for the current layout.
    ///
    /// Each label sits below its tile, horizontally centered, with a max
    /// width slightly wider than the tile so two lines can fit.
    pub fn build_labels(&self, viewport_w: f32) -> Vec<crate::text::Label> {
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
                    let name = DUMMY_NAMES[idx % DUMMY_NAMES.len()];
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

    #[test]
    fn counts_match() {
        let g = GridLayout::default().centered(1280.0);
        assert_eq!(g.total_tiles(), 7 * 5 * 3);
        assert_eq!(g.build_instances(1280.0).len(), g.total_tiles());
    }

    #[test]
    fn pages_are_offset_by_one_viewport() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let inst = g.build_instances(vw);
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
        let inst = g.build_instances(vw);
        let grid_w = g.cols as f32 * g.tile_size + (g.cols - 1) as f32 * g.gap;
        let expected_left = (vw - grid_w) * 0.5;
        assert!(
            (inst[0].x - expected_left).abs() < 1e-2,
            "first tile x should center the grid"
        );
    }
}
