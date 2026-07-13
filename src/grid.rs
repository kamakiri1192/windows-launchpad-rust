//! Binary adapter for the launcher grid: GPU-facing instance builders on top
//! of the pure geometry in [`crate::layout::grid`].
//!
//! All geometry here is in **physical pixels** and expressed relative to the
//! *content* origin (which the scroller shifts horizontally). The renderer
//! converts these into clip space at draw time.
//!
//! The pure page-frame geometry, hit classification, and DPI scaling live in
//! [`crate::layout::grid`], which compiles as part of the library target so it
//! can be unit-tested without `wgpu`/`winit`/`Win32`. This module adds the
//! GPU-facing pieces:
//!
//! - [`TileInstance`] — the `#[repr(C)]` instance struct the tile shader reads.
//! - [`GridItem`] — a minimal item view carrying a label, icon, or preview UVs
//!   ([`UvRect`]).
//! - [`TileAnim`] — per-app edit-mode animation parameters packed into the
//!   tile/icon instance `extra` vec4.
//! - [`GridLayout::bounds`] — the [`ScrollBounds`]-returning adapter that
//!   converts the pure [`GridLayout::page_extent`] into the scroller's bounds
//!   type.
//! - [`GridLayout::build_instances`] / [`GridLayout::build_icon_instances`] /
//!   [`GridLayout::build_labels`] — the GPU instance builders.
//!
//! Behavior preservation: every pure calculation delegates to
//! [`crate::layout::grid::GridLayout`] unchanged. Only the GPU instance structs
//! and the [`ScrollBounds`] adapter are added here.

use crate::icons::AppEntry;
use crate::layout::grid as layout_grid;
use crate::scroll::ScrollBounds;
use crate::ui_model::geometry::Rect;
pub use crate::ui_model::grid::{GridItem, TileAnim};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, GlassBehavior, GlassMaterial, GlassSurface, IconSource, IconView, TileView,
};

// Re-exported for other binary modules (renderer, liquid_glass) that still
// reference `crate::grid::*`. These are not all used *inside* this file, hence
// `unused_imports` is allowed.
#[allow(unused_imports)]
pub use layout_grid::GridLayout;
#[allow(unused_imports)]
pub use layout_grid::{edit_badge_radius_for_tile_size, BASE_TILE_SIZE, FRAME_CORNER_RADIUS};

impl<'a> From<&'a AppEntry> for GridItem<'a> {
    fn from(a: &'a AppEntry) -> Self {
        Self {
            key: a.name.as_str(),
            name: &a.name,
            uv: a.uv,
            preview_uvs: &[],
        }
    }
}

impl GridLayout {
    /// Build the base Liquid Glass scene (fixed page frame + scrolling tile
    /// halos) as renderer-neutral surfaces.
    pub fn build_glass_surfaces(
        &self,
        viewport_w: f32,
        items: &[GridItem<'_>],
    ) -> Vec<GlassSurface> {
        let (cx, cy, width, height) = self.frame_panel_rect(viewport_w);
        let mut surfaces = Vec::with_capacity(1 + items.len().min(self.total_tiles()));
        surfaces.push(GlassSurface {
            id: UiId::backdrop("page-frame"),
            rect: Rect::new(cx - width * 0.5, cy - height * 0.5, width, height),
            radius: self.scaled(layout_grid::FRAME_CORNER_RADIUS),
            material: GlassMaterial::Regular,
            behavior: GlassBehavior::FixedFrame,
            z: -10,
        });
        for (index, item) in items.iter().take(self.total_tiles()).enumerate() {
            let (x, y) = self.tile_position(viewport_w, index);
            let halo = self.tile_size + self.scaled(18.0);
            surfaces.push(GlassSurface {
                id: UiId::launcher_item(item.key),
                rect: Rect::new(
                    x + (self.tile_size - halo) * 0.5,
                    y + (self.tile_size - halo) * 0.5,
                    halo,
                    halo,
                ),
                radius: self.scaled(28.0),
                material: GlassMaterial::Regular,
                behavior: GlassBehavior::Scrolling,
                z: 0,
            });
        }
        surfaces
    }

    /// Build the scroll bounds implied by this layout & viewport.
    ///
    /// This is the binary adapter over the pure
    /// [`GridLayout::page_extent`](layout_grid::GridLayout::page_extent):
    /// `page_extent` equals [`GridLayout::page_width`], so the resulting
    /// `ScrollBounds` is identical to the historical in-place construction.
    pub fn bounds(&self, viewport_w: f32) -> ScrollBounds {
        ScrollBounds {
            page_extent: self.page_extent(viewport_w),
            page_count: self.page_count,
        }
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
        items: &[GridItem<'_>],
        anim: &[TileAnim],
    ) -> Vec<TileView> {
        let per_page = self.cols * self.rows;
        let item_count = items.len().min(self.total_tiles());
        let page_w = self.page_width(viewport_w);
        let mut out = Vec::with_capacity(item_count);
        for (idx, item) in items.iter().take(item_count).enumerate() {
            let p = idx / per_page;
            let row_in_page = idx % per_page;
            let r = row_in_page / self.cols;
            let c = row_in_page % self.cols;
            let page_origin_x = (p as f32) * page_w;
            let x = page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
            let y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);
            let (r_, g_, b_) = layout_grid::app_color(idx);
            let icon_index = if item.uv.is_some() || item.preview_uvs.iter().any(Option::is_some) {
                idx as f32
            } else {
                -1.0
            };
            let anim = anim.get(idx).copied().unwrap_or(TileAnim::IDLE);
            out.push(TileView {
                id: UiId::launcher_item(item.key),
                rect: Rect::new(x, y, self.tile_size, self.tile_size),
                radius: self.scaled(19.0),
                color: Color::rgba(r_, g_, b_, 1.0),
                has_icon: icon_index >= 0.0,
                motion: anim,
                z: if anim.flags & TileAnim::FLAG_DRAG != 0 {
                    20
                } else {
                    0
                },
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
        items: &[GridItem<'_>],
        anim: &[TileAnim],
    ) -> Vec<IconView> {
        let per_page = self.cols * self.rows;
        let item_count = items.len().min(self.total_tiles());
        let page_w = self.page_width(viewport_w);
        let mut out = Vec::with_capacity(item_count * 3);
        for (idx, item) in items.iter().take(item_count).enumerate() {
            let p = idx / per_page;
            let row_in_page = idx % per_page;
            let r = row_in_page / self.cols;
            let c = row_in_page % self.cols;
            let page_origin_x = (p as f32) * page_w;
            let x = page_origin_x + self.margin_left + c as f32 * (self.tile_size + self.gap);
            let y = self.margin_top + r as f32 * (self.tile_size + self.row_gap);
            let anim = anim.get(idx).copied().unwrap_or(TileAnim::IDLE);
            let z = if anim.flags & TileAnim::FLAG_DRAG != 0 {
                20
            } else {
                0
            };
            if let Some(uv) = item.uv {
                out.push(IconView {
                    id: UiId::launcher_item(item.key),
                    rect: Rect::new(x, y, self.tile_size, self.tile_size),
                    source: IconSource::AtlasUv(uv),
                    motion: anim,
                    z,
                });
            } else {
                // The drag shader centers every dragged instance on the
                // pointer. A folder has up to nine independently positioned
                // miniatures, so omit them during the lift instead of stacking
                // all of them at the same point.
                if anim.flags & TileAnim::FLAG_DRAG != 0 {
                    continue;
                }
                let mini = self.tile_size * 0.22;
                let mini_gap = self.tile_size * 0.07;
                let preview_w = mini * 3.0 + mini_gap * 2.0;
                let preview_x = x + (self.tile_size - preview_w) * 0.5;
                let preview_y = y + (self.tile_size - preview_w) * 0.5;
                for (slot, uv) in item.preview_uvs.iter().take(9).enumerate() {
                    let Some(uv) = *uv else { continue };
                    let row = slot / 3;
                    let col = slot % 3;
                    out.push(IconView {
                        id: UiId::launcher_preview(item.key, slot),
                        rect: Rect::new(
                            preview_x + col as f32 * (mini + mini_gap),
                            preview_y + row as f32 * (mini + mini_gap),
                            mini,
                            mini,
                        ),
                        source: IconSource::AtlasUv(uv),
                        motion: anim,
                        z,
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
    /// text comes from `apps[i].name`; empty slots after the last app are skipped.
    pub fn build_labels(
        &self,
        viewport_w: f32,
        items: &[GridItem<'_>],
    ) -> Vec<crate::renderer::text_engine::Label> {
        let per_page = self.cols * self.rows;
        let app_count = items.len().min(self.total_tiles());
        let page_w = self.page_width(viewport_w);
        let mut out = Vec::with_capacity(app_count);
        for (idx, item) in items.iter().take(app_count).enumerate() {
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
            out.push(crate::renderer::text_engine::Label {
                text: item.name.to_string(),
                x: label_x,
                y: label_y,
                max_width: label_w,
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_model::geometry::UvRect;
    use std::path::PathBuf;

    /// Owned app-list helper for tests (so `GridItem` borrows stable storage).
    struct OwnedApp {
        id: String,
        name: String,
        uv: Option<UvRect>,
    }

    /// Build a minimal app list of `n` entries, half with icons (UV set),
    /// half without — exercises both code paths.
    fn fake_apps(n: usize) -> Vec<OwnedApp> {
        (0..n)
            .map(|i| OwnedApp {
                id: format!("app-{i}"),
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
    fn view<'a>(apps: &'a [OwnedApp]) -> Vec<GridItem<'a>> {
        apps.iter()
            .map(|a| GridItem {
                key: a.id.as_str(),
                name: a.name.as_str(),
                uv: a.uv,
                preview_uvs: &[],
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
    fn bounds_page_extent_equals_page_width() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let bounds = g.bounds(vw);
        assert!(
            (bounds.page_extent - g.page_width(vw)).abs() < 1e-2,
            "scroll page_extent must equal the content page width"
        );
        assert_eq!(bounds.page_count, g.page_count);
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
        let p0 = inst[0].rect.x;
        let p1 = inst[7 * 5].rect.x; // first tile of page 1
                                     // Page 1's first tile must be exactly one page width to the right.
        assert!(
            (p1 - p0 - page_w).abs() < 1e-2,
            "pages spaced by the content page width"
        );
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
                assert!(tile.has_icon, "icon tile should carry icon presence");
            } else {
                assert!(!tile.has_icon, "icon-less tile should fall back");
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
    fn grid_is_centered_in_viewport() {
        let vw = 1280.0;
        let g = GridLayout::default().centered(vw);
        let apps = fake_apps(g.total_tiles());
        let inst = g.build_instances(vw, &view(&apps), &[]);
        let grid_w = g.cols as f32 * g.tile_size + (g.cols - 1) as f32 * g.gap;
        let expected_left = (vw - grid_w) * 0.5;
        assert!(
            (inst[0].rect.x - expected_left).abs() < 1e-2,
            "first tile x should center the grid"
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
}
