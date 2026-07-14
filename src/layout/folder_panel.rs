//! Pure folder-panel geometry. The same rectangles emit renderer-neutral
//! primitives and hit regions, including the tile-to-panel container morph.

use crate::layout::hit_map::{HitMap, HitRegion};
use crate::layout::LayoutResult;
use crate::ui_model::geometry::UvRect;
use crate::ui_model::geometry::{Point, Rect};
use crate::ui_model::grid::TileAnim;
use crate::ui_model::hit::HitTarget;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, GlassBehavior, GlassLayer, GlassMaterial, GlassSurface, IconSource,
    IconView, InkLane, InkView, RenderModel, TileView,
};
use crate::ui_model::text::{TextAlign, TextRole, TextStyle, TextView, TextWeight};

pub const PAGE_SIZE: usize = 9;
pub const COLS: usize = 3;
const VIEWPORT_MARGIN: f32 = 28.0;
const PANEL_MIN_WIDTH: f32 = 250.0;
const PANEL_PADDING_X: f32 = 34.0;
const PANEL_PADDING_TOP: f32 = 60.0;
const PANEL_PADDING_BOTTOM: f32 = 34.0;
const CELL_SIZE: f32 = 76.0;
const CELL_GAP_X: f32 = 34.0;
const CELL_GAP_Y: f32 = 42.0;
const LABEL_HEIGHT: f32 = 36.0;
const PANEL_RADIUS: f32 = 42.0;
/// Cool-neutral tint layered after the scene-space focus blur. Blur carries the
/// visual separation; this restrained wash only lowers residual contrast.
const GLASS_FOCUS_VEIL_OPACITY: f32 = 0.14;
/// Portion of the closed end of the morph used to collapse each child's
/// colored tile fill into its own center. Icons keep their full trajectory.
const CHILD_FILL_COLLAPSE_PROGRESS: f32 = 0.42;

#[derive(Debug, Clone)]
pub struct FolderChildInput<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub uv: Option<UvRect>,
    pub color: Color,
}

#[derive(Debug, Clone)]
pub struct FolderPanelInput<'a> {
    pub viewport: (u32, u32),
    pub scale_factor: f32,
    pub folder_key: &'a str,
    pub name: &'a str,
    pub rename_text: Option<&'a str>,
    pub source_rect: Rect,
    /// Physical-pixel corner radius of the closed folder container. Supplying
    /// it with the source rect keeps the morph endpoint identical to the grid.
    pub source_radius: f32,
    /// Physical-pixel bounds of the fixed page-frame Liquid Glass surface.
    /// The focus veil is clipped to this shape rather than the whole window.
    pub page_frame_rect: Rect,
    pub page_frame_radius: f32,
    pub children: &'a [FolderChildInput<'a>],
    pub page: usize,
    /// Horizontal content origin from the shared paging scroller. Page zero
    /// rests at 0; later pages use negative panel-width multiples.
    pub page_scroll_x: f32,
    pub progress: f32,
    pub editing: bool,
    pub wiggle_phase: f32,
    pub dragged_child_key: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FolderPanelModel {
    pub result: LayoutResult,
    pub target_panel_rect: Rect,
    pub current_panel_rect: Rect,
    pub title_rect: Rect,
    pub child_rects: Vec<Rect>,
    pub page: usize,
    pub page_count: usize,
}

pub fn build(input: FolderPanelInput<'_>) -> FolderPanelModel {
    let scale = sanitize_scale(input.scale_factor);
    let viewport_w = input.viewport.0.max(1) as f32;
    let viewport_h = input.viewport.1.max(1) as f32;
    let page_count = input.children.len().div_ceil(PAGE_SIZE).max(1);
    let page = input.page.min(page_count - 1);
    let layout_count = input.children.len().min(PAGE_SIZE);
    let cols = layout_count.clamp(1, COLS);
    let rows = layout_count.div_ceil(COLS);
    let content_width =
        cols as f32 * CELL_SIZE * scale + cols.saturating_sub(1) as f32 * CELL_GAP_X * scale;
    let panel_w = (content_width + PANEL_PADDING_X * 2.0 * scale)
        .max(PANEL_MIN_WIDTH * scale)
        .min((viewport_w - VIEWPORT_MARGIN * 2.0 * scale).max(120.0));
    let content_height = if rows == 0 {
        20.0 * scale
    } else {
        rows as f32 * (CELL_SIZE + LABEL_HEIGHT) * scale
            + rows.saturating_sub(1) as f32 * CELL_GAP_Y * scale
    };
    let indicator_height = if page_count > 1 {
        24.0 * scale
    } else {
        8.0 * scale
    };
    let panel_h = (PANEL_PADDING_TOP * scale
        + content_height
        + indicator_height
        + PANEL_PADDING_BOTTOM * scale)
        .min((viewport_h - VIEWPORT_MARGIN * 2.0 * scale).max(120.0));
    let target = Rect::new(
        (viewport_w - panel_w) * 0.5,
        (viewport_h - panel_h) * 0.5,
        panel_w,
        panel_h,
    );
    let progress = smooth(input.progress.clamp(0.0, 1.0));
    let current = lerp_rect(input.source_rect, target, progress);
    let radius = lerp(input.source_radius.max(0.0), PANEL_RADIUS * scale, progress)
        .min(current.width * 0.5)
        .min(current.height * 0.5);

    let mut render = RenderModel::new();
    render.set_glass_batch(
        GlassLayer::Modal,
        vec![GlassSurface {
            id: UiId::folder_panel(input.folder_key),
            rect: current,
            radius,
            material: GlassMaterial::Regular,
            behavior: GlassBehavior::Control,
            z: 100,
        }],
    );

    let page_frame_radius = input
        .page_frame_radius
        .max(0.0)
        .min(input.page_frame_rect.width * 0.5)
        .min(input.page_frame_rect.height * 0.5);
    let backdrop = InkView {
        id: UiId::backdrop("glass-focus-veil"),
        center: input.page_frame_rect.center(),
        extent: input.page_frame_rect.height * 0.5,
        opacity: GLASS_FOCUS_VEIL_OPACITY * progress,
        scene_blur: progress,
        stroke: input.page_frame_rect.width * 0.5,
        corner_radius: page_frame_radius,
        color: Color::rgba(0.12, 0.15, 0.20, 1.0),
        kind: ControlKind::RowBackground,
        z: 90,
    };
    render.set_ink_batch(InkLane::Backdrop, vec![backdrop]);

    let title_alpha = ((progress - 0.34) / 0.66).clamp(0.0, 1.0);
    let title_rect = Rect::new(
        target.x + 24.0 * scale,
        target.y + 17.0 * scale,
        target.width - 48.0 * scale,
        32.0 * scale,
    );
    let mut modal_ink = Vec::new();
    if input.rename_text.is_some() && title_alpha > 0.001 {
        modal_ink.push(InkView {
            id: UiId::folder_title(input.folder_key),
            center: title_rect.center(),
            extent: title_rect.height * 0.5,
            opacity: 0.16 * title_alpha,
            scene_blur: 0.0,
            stroke: title_rect.width * 0.5,
            corner_radius: title_rect.height * 0.5,
            color: Color::rgba(1.0, 1.0, 1.0, 1.0),
            kind: ControlKind::RowBackground,
            z: 128,
        });
    }
    if title_alpha > 0.001 {
        render.text.push(TextView {
            id: UiId::folder_title(input.folder_key),
            text: input.rename_text.unwrap_or(input.name).to_owned(),
            rect: title_rect,
            style: TextStyle::new(
                TextRole::FolderTitle,
                18.0,
                Color::rgba(1.0, 1.0, 1.0, 0.96 * title_alpha),
                TextWeight::Bold,
                TextAlign::Center,
            ),
            z: 130,
        });
    }

    let grid_top = target.y + PANEL_PADDING_TOP * scale;
    let mut modal_tiles = Vec::new();
    let mut modal_icons = Vec::new();
    let mut dragged_tile = None;
    let mut dragged_icon = None;
    let mut child_rects = Vec::with_capacity(input.children.len());
    for (global_index, child) in input.children.iter().enumerate() {
        let child_page = global_index / PAGE_SIZE;
        let local_index = global_index % PAGE_SIZE;
        let page_start = child_page * PAGE_SIZE;
        let page_child_count = (input.children.len() - page_start).min(PAGE_SIZE);
        let row = local_index / COLS;
        let col = local_index % COLS;
        let row_start = row * COLS;
        let row_count = (page_child_count - row_start).min(COLS);
        let row_width = row_count as f32 * CELL_SIZE * scale
            + row_count.saturating_sub(1) as f32 * CELL_GAP_X * scale;
        let page_origin = child_page as f32 * target.width + input.page_scroll_x;
        let grid_left = target.x + page_origin + (target.width - row_width) * 0.5;
        let final_rect = Rect::new(
            grid_left + col as f32 * (CELL_SIZE + CELL_GAP_X) * scale,
            grid_top + row as f32 * (CELL_SIZE + LABEL_HEIGHT + CELL_GAP_Y) * scale,
            CELL_SIZE * scale,
            CELL_SIZE * scale,
        );
        let source = miniature_rect(input.source_rect, local_index.min(8));
        let child_progress = if child_page == 0 {
            progress
        } else {
            ((progress - 0.72) / 0.28).clamp(0.0, 1.0)
        };
        let rect = lerp_rect(source, final_rect, smooth(child_progress));
        child_rects.push(rect);
        let dragged = input.dragged_child_key == Some(child.key);
        let motion = TileAnim {
            phase: if input.editing {
                input.wiggle_phase + global_index as f32 * 0.37
            } else {
                0.0
            },
            lift: if dragged { 18.0 * scale } else { 0.0 },
            scale: if dragged { 1.12 } else { 1.0 },
            flags: TileAnim::FLAG_FIXED
                | if input.editing {
                    TileAnim::FLAG_WIGGLE
                } else {
                    0
                }
                | if dragged { TileAnim::FLAG_DRAG } else { 0 },
        };
        let fill_scale = child_fill_scale(progress);
        if fill_scale > 0.001 {
            let view = TileView {
                id: UiId::folder_child(input.folder_key, child.key),
                rect: scale_rect_about_center(rect, fill_scale),
                radius: 17.0 * scale * fill_scale,
                color: child.color,
                has_icon: child.uv.is_some(),
                motion,
                z: if dragged { 150 } else { 120 },
            };
            if dragged {
                dragged_tile = Some(view);
            } else {
                modal_tiles.push(view);
            }
        }
        if let Some(uv) = child.uv {
            let view = IconView {
                id: UiId::folder_child(input.folder_key, child.key),
                rect,
                source: IconSource::AtlasUv(uv),
                motion,
                z: if dragged { 151 } else { 121 },
            };
            if dragged {
                dragged_icon = Some(view);
            } else {
                modal_icons.push(view);
            }
        }
        if title_alpha > 0.001 {
            render.text.push(TextView {
                id: UiId::folder_child(input.folder_key, child.key),
                text: child.label.to_owned(),
                rect: Rect::new(
                    final_rect.x - 12.0 * scale,
                    final_rect.max_y() + 5.0 * scale,
                    final_rect.width + 24.0 * scale,
                    LABEL_HEIGHT * scale,
                ),
                style: TextStyle::new(
                    TextRole::FolderItemLabel,
                    14.0,
                    Color::rgba(1.0, 1.0, 1.0, 0.90 * title_alpha),
                    TextWeight::Regular,
                    TextAlign::Center,
                ),
                z: 125,
            });
        }
    }
    if let Some(tile) = dragged_tile {
        modal_tiles.push(tile);
    }
    if let Some(icon) = dragged_icon {
        modal_icons.push(icon);
    }
    render.modal_tiles = Some(modal_tiles);
    render.modal_icons = Some(modal_icons);

    if page_count > 1 && title_alpha > 0.001 {
        let dot_y = target.max_y() - 20.0 * scale;
        let total_w = (page_count.saturating_sub(1) as f32 * 12.0 + 6.0) * scale;
        let first_x = target.center().x - total_w * 0.5 + 3.0 * scale;
        for dot in 0..page_count {
            modal_ink.push(InkView {
                id: UiId::folder_page(input.folder_key, dot),
                center: Point::new(first_x + dot as f32 * 12.0 * scale, dot_y),
                extent: if dot == page {
                    3.5 * scale
                } else {
                    2.5 * scale
                },
                opacity: if dot == page {
                    title_alpha
                } else {
                    0.42 * title_alpha
                },
                scene_blur: 0.0,
                stroke: 1.0,
                corner_radius: 0.0,
                color: Color::rgba(1.0, 1.0, 1.0, if dot == page { 0.9 } else { 0.42 }),
                kind: ControlKind::Dot,
                z: 130,
            });
        }
    }
    render.set_ink_batch(InkLane::Modal, modal_ink);

    let mut hits = HitMap::new();
    hits.push(HitRegion::new(
        UiId::backdrop("folder-modal"),
        Rect::new(0.0, 0.0, viewport_w, viewport_h),
        HitTarget::modal_dismiss_backdrop(),
        90,
    ));
    hits.push(HitRegion::new(
        UiId::folder_panel(input.folder_key),
        current,
        HitTarget::folder_panel(input.folder_key),
        100,
    ));
    if progress > 0.9 {
        hits.push(HitRegion::new(
            UiId::folder_title(input.folder_key),
            title_rect,
            HitTarget::folder_title(input.folder_key),
            130,
        ));
        for (global_index, (child, rect)) in input.children.iter().zip(&child_rects).enumerate() {
            if let Some(clipped) = intersect_rect(*rect, target) {
                hits.push(HitRegion::new(
                    UiId::folder_child(input.folder_key, child.key),
                    clipped,
                    HitTarget::folder_child(input.folder_key, child.key, global_index),
                    140,
                ));
                if input.editing && input.dragged_child_key != Some(child.key) {
                    let radius = crate::layout::grid::edit_badge_radius_for_tile_size(rect.width);
                    let inset = radius * crate::layout::edit_mode::BADGE_CENTER_INSET_FRAC;
                    let hit_radius = radius + 6.0 * scale;
                    let center = Point::new(rect.x + inset, rect.y + inset);
                    if let Some(badge_rect) = intersect_rect(
                        Rect::new(
                            center.x - hit_radius,
                            center.y - hit_radius,
                            hit_radius * 2.0,
                            hit_radius * 2.0,
                        ),
                        target,
                    ) {
                        hits.push(HitRegion::new(
                            UiId::folder_child_badge(input.folder_key, child.key),
                            badge_rect,
                            HitTarget::folder_child_badge(
                                input.folder_key,
                                child.key,
                                global_index,
                            ),
                            150,
                        ));
                    }
                }
            }
        }
        if page_count > 1 {
            let nav = Rect::new(
                target.x,
                target.max_y() - 42.0 * scale,
                target.width,
                42.0 * scale,
            );
            if page > 0 {
                hits.push(HitRegion::new(
                    UiId::folder_page(input.folder_key, page - 1),
                    Rect::new(nav.x, nav.y, nav.width * 0.5, nav.height),
                    HitTarget::FolderPagePrevious {
                        key: input.folder_key.to_owned(),
                    },
                    120,
                ));
            }
            if page + 1 < page_count {
                hits.push(HitRegion::new(
                    UiId::folder_page(input.folder_key, page + 1),
                    Rect::new(nav.center().x, nav.y, nav.width * 0.5, nav.height),
                    HitTarget::FolderPageNext {
                        key: input.folder_key.to_owned(),
                    },
                    120,
                ));
            }
        }
    }

    FolderPanelModel {
        result: LayoutResult::new(render, hits),
        target_panel_rect: target,
        current_panel_rect: current,
        title_rect,
        child_rects,
        page,
        page_count,
    }
}

fn miniature_rect(source: Rect, slot: usize) -> Rect {
    let mini = source.width.min(source.height) * 0.22;
    let gap = source.width.min(source.height) * 0.07;
    let width = mini * 3.0 + gap * 2.0;
    let left = source.center().x - width * 0.5;
    let top = source.center().y - width * 0.5;
    Rect::new(
        left + (slot % 3) as f32 * (mini + gap),
        top + (slot / 3) as f32 * (mini + gap),
        mini,
        mini,
    )
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::new(
        lerp(a.x, b.x, t),
        lerp(a.y, b.y, t),
        lerp(a.width, b.width, t),
        lerp(a.height, b.height, t),
    )
}

fn child_fill_scale(progress: f32) -> f32 {
    smooth((progress / CHILD_FILL_COLLAPSE_PROGRESS).clamp(0.0, 1.0))
}

fn scale_rect_about_center(rect: Rect, scale: f32) -> Rect {
    let scale = scale.clamp(0.0, 1.0);
    let width = rect.width * scale;
    let height = rect.height * scale;
    Rect::new(
        rect.center().x - width * 0.5,
        rect.center().y - height * 0.5,
        width,
        height,
    )
}

fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.max_x().min(b.max_x());
    let y1 = a.max_y().min(b.max_y());
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn sanitize_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn children(count: usize) -> Vec<(String, String)> {
        (0..count)
            .map(|i| (format!("id-{i}"), format!("App {i}")))
            .collect()
    }

    fn model(count: usize, progress: f32, scale: f32) -> FolderPanelModel {
        model_from_source(
            count,
            progress,
            scale,
            Rect::new(100.0, 120.0, 84.0 * scale, 84.0 * scale),
        )
    }

    fn model_from_source(
        count: usize,
        progress: f32,
        scale: f32,
        source_rect: Rect,
    ) -> FolderPanelModel {
        model_with_page(count, progress, scale, source_rect, 0, 0.0)
    }

    fn model_with_page(
        count: usize,
        progress: f32,
        scale: f32,
        source_rect: Rect,
        page: usize,
        page_scroll_x: f32,
    ) -> FolderPanelModel {
        let owned = children(count);
        let input: Vec<_> = owned
            .iter()
            .map(|(id, label)| FolderChildInput {
                key: id,
                label,
                uv: None,
                color: Color::rgba(0.4, 0.5, 0.7, 1.0),
            })
            .collect();
        build(FolderPanelInput {
            viewport: (1280, 800),
            scale_factor: scale,
            folder_key: "folder-0",
            name: "仕事",
            rename_text: None,
            source_rect,
            source_radius: 19.0 * scale,
            page_frame_rect: Rect::new(80.0, 60.0, 1120.0, 680.0),
            page_frame_radius: 54.0 * scale,
            children: &input,
            page,
            page_scroll_x,
            progress,
            editing: false,
            wiggle_phase: 0.0,
            dragged_child_key: None,
        })
    }

    #[test]
    fn geometry_is_continuous_at_endpoints() {
        let closed = model(4, 0.0, 1.0);
        assert_eq!(
            closed.current_panel_rect,
            Rect::new(100.0, 120.0, 84.0, 84.0)
        );
        let closed_glass = &closed
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Modal)
            .unwrap()
            .surfaces[0];
        assert_eq!(closed_glass.radius, 19.0);
        let open = model(4, 1.0, 1.0);
        assert_eq!(open.current_panel_rect, open.target_panel_rect);
    }

    #[test]
    fn child_trajectory_starts_at_miniature_and_ends_at_open_cell() {
        let source = Rect::new(100.0, 120.0, 84.0, 84.0);
        let closed = model_from_source(4, 0.0, 1.0, source);
        assert_eq!(closed.child_rects[0], miniature_rect(source, 0));
        let open = model_from_source(4, 1.0, 1.0, source);
        let first_tile = &open.result.render.modal_tiles.as_ref().unwrap()[0];
        assert_eq!(open.child_rects[0], first_tile.rect);
        assert_ne!(open.child_rects[0], miniature_rect(source, 0));
    }

    #[test]
    fn child_fill_collapses_into_its_center_before_closed_handoff() {
        let closed = model(4, 0.0, 1.0);
        assert!(closed
            .result
            .render
            .modal_tiles
            .as_ref()
            .unwrap()
            .is_empty());

        let nearly_closed = model(4, 0.2, 1.0);
        let tile = &nearly_closed.result.render.modal_tiles.as_ref().unwrap()[0];
        assert_eq!(tile.rect.center(), nearly_closed.child_rects[0].center());
        assert!(tile.rect.width < nearly_closed.child_rects[0].width * 0.25);

        let open = model(4, 1.0, 1.0);
        let tile = &open.result.render.modal_tiles.as_ref().unwrap()[0];
        assert_eq!(tile.rect, open.child_rects[0]);
    }

    #[test]
    fn morph_retargets_when_latest_source_tile_moves() {
        let first = model_from_source(4, 0.5, 1.0, Rect::new(100.0, 120.0, 84.0, 84.0));
        let moved = model_from_source(4, 0.5, 1.0, Rect::new(260.0, 220.0, 84.0, 84.0));
        assert!(moved.current_panel_rect.x > first.current_panel_rect.x);
        assert!(moved.current_panel_rect.y > first.current_panel_rect.y);
        assert_eq!(moved.target_panel_rect, first.target_panel_rect);
    }

    #[test]
    fn panel_scales_and_clamps_for_empty_and_many_children() {
        for count in [0, 1, 4, 5, 18] {
            let value = model(count, 1.0, 1.5);
            assert!(value.target_panel_rect.min_x() >= 0.0);
            assert!(value.target_panel_rect.max_x() <= 1280.0);
            assert!(value.target_panel_rect.max_y() <= 800.0);
        }
    }

    #[test]
    fn sparse_panels_shrink_and_incomplete_rows_are_centered() {
        let one = model(1, 1.0, 1.0);
        let four = model(4, 1.0, 1.0);
        let five = model(5, 1.0, 1.0);
        assert!(one.target_panel_rect.width < four.target_panel_rect.width);
        assert!(one.target_panel_rect.height < four.target_panel_rect.height);
        let last_four = four.child_rects[3];
        assert!((last_four.center().x - four.target_panel_rect.center().x).abs() < 0.01);
        let last_five_center =
            (five.child_rects[3].center().x + five.child_rects[4].center().x) * 0.5;
        assert!((last_five_center - five.target_panel_rect.center().x).abs() < 0.01);
    }

    #[test]
    fn dpi_scales_child_cells_without_mixing_coordinate_spaces() {
        let normal = model(1, 1.0, 1.0);
        let scaled = model(1, 1.0, 1.5);
        assert!((scaled.child_rects[0].width - normal.child_rects[0].width * 1.5).abs() < 0.01);
    }

    #[test]
    fn nine_children_fit_one_page_and_ten_require_two() {
        assert_eq!(model(9, 1.0, 1.0).page_count, 1);
        assert_eq!(model(10, 1.0, 1.0).page_count, 2);
    }

    #[test]
    fn ten_children_create_two_pages_and_indicator() {
        let value = model(10, 1.0, 1.0);
        assert_eq!(value.page_count, 2);
        assert_eq!(value.child_rects.len(), 10);
        assert!(value
            .result
            .render
            .ink
            .iter()
            .any(|batch| batch.lane == InkLane::Modal));
    }

    #[test]
    fn horizontal_scroll_moves_the_next_page_into_the_same_panel() {
        let source = Rect::new(100.0, 120.0, 84.0, 84.0);
        let first = model_with_page(10, 1.0, 1.0, source, 0, 0.0);
        let page_width = first.target_panel_rect.width;
        assert!(first.child_rects[9].center().x > first.target_panel_rect.max_x());

        let second = model_with_page(10, 1.0, 1.0, source, 1, -page_width);
        assert!(
            (second.child_rects[9].center().x - second.target_panel_rect.center().x).abs() < 0.01
        );
        assert!(second.child_rects[0].center().x < second.target_panel_rect.x);
        assert!(matches!(
            second
                .result
                .hits
                .hit_test(second.child_rects[9].center())
                .map(|hit| &hit.target),
            Some(HitTarget::FolderChild { child, index: 9, .. }) if child == "id-9"
        ));
    }

    #[test]
    fn panel_geometry_stays_fixed_across_sparse_last_page() {
        let source = Rect::new(100.0, 120.0, 84.0, 84.0);
        let first = model_with_page(10, 1.0, 1.0, source, 0, 0.0);
        let second = model_with_page(10, 1.0, 1.0, source, 1, -first.target_panel_rect.width);
        assert_eq!(first.target_panel_rect, second.target_panel_rect);
    }

    #[test]
    fn child_hit_wins_over_panel_and_backdrop() {
        let value = model(1, 1.0, 1.0);
        let point = value.child_rects[0].center();
        assert!(matches!(
            value.result.hits.hit_test(point).map(|hit| &hit.target),
            Some(HitTarget::FolderChild { .. })
        ));
    }

    #[test]
    fn title_panel_and_backdrop_follow_modal_z_order() {
        let value = model(4, 1.0, 1.0);
        let title = value
            .result
            .render
            .text
            .iter()
            .find(|view| view.style.role == TextRole::FolderTitle)
            .unwrap();
        assert_eq!(title.style.weight, TextWeight::Bold);
        assert!(matches!(
            value
                .result
                .hits
                .hit_test(value.title_rect.center())
                .map(|hit| &hit.target),
            Some(HitTarget::FolderTitle { .. })
        ));
        let panel_point = Point::new(
            value.current_panel_rect.x + 8.0,
            value.current_panel_rect.center().y,
        );
        assert!(matches!(
            value
                .result
                .hits
                .hit_test(panel_point)
                .map(|hit| &hit.target),
            Some(HitTarget::FolderPanel { .. })
        ));
        assert!(matches!(
            value
                .result
                .hits
                .hit_test(Point::new(2.0, 2.0))
                .map(|hit| &hit.target),
            Some(HitTarget::Backdrop { .. })
        ));
    }

    #[test]
    fn folder_children_use_the_normal_wiggle_flag_while_editing() {
        let owned = children(2);
        let input: Vec<_> = owned
            .iter()
            .map(|(id, label)| FolderChildInput {
                key: id,
                label,
                uv: None,
                color: Color::rgba(0.4, 0.5, 0.7, 1.0),
            })
            .collect();
        let value = build(FolderPanelInput {
            viewport: (1280, 800),
            scale_factor: 1.0,
            folder_key: "folder-0",
            name: "Folder",
            rename_text: None,
            source_rect: Rect::new(100.0, 120.0, 84.0, 84.0),
            source_radius: 19.0,
            page_frame_rect: Rect::new(80.0, 60.0, 1120.0, 680.0),
            page_frame_radius: 54.0,
            children: &input,
            page: 0,
            page_scroll_x: 0.0,
            progress: 1.0,
            editing: true,
            wiggle_phase: 1.25,
            dragged_child_key: None,
        });
        let radius =
            crate::layout::grid::edit_badge_radius_for_tile_size(value.child_rects[0].width);
        let inset = radius * crate::layout::edit_mode::BADGE_CENTER_INSET_FRAC;
        let badge_point = Point::new(
            value.child_rects[0].x + inset,
            value.child_rects[0].y + inset,
        );
        assert!(matches!(
            value
                .result
                .hits
                .hit_test(badge_point)
                .map(|hit| &hit.target),
            Some(HitTarget::FolderChildBadge { child, index: 0, .. }) if child == "id-0"
        ));
        let flat_badge_ink_count = value
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Modal)
            .unwrap()
            .views
            .iter()
            .filter(|view| matches!(view.kind, ControlKind::CloseButton))
            .count();
        assert_eq!(
            flat_badge_ink_count, 0,
            "folder badges are derived from modal tile motion by the GPU renderer"
        );
        let tiles = value.result.render.modal_tiles.unwrap();
        assert!(tiles
            .iter()
            .all(|tile| tile.motion.flags & TileAnim::FLAG_WIGGLE != 0));
        assert_eq!(tiles[0].motion.phase, 1.25);
        assert_ne!(tiles[0].motion.phase, tiles[1].motion.phase);
    }

    #[test]
    fn modal_glass_and_focus_veil_are_renderer_neutral_outputs() {
        let value = model(4, 0.5, 1.0);
        let modal = value
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Modal)
            .unwrap();
        assert_eq!(modal.surfaces[0].material, GlassMaterial::Regular);
        let veil = &value
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Backdrop)
            .unwrap()
            .views[0];
        assert_eq!(veil.id, UiId::backdrop("glass-focus-veil"));
        assert_eq!(veil.center, Point::new(640.0, 400.0));
        assert_eq!(veil.stroke, 560.0);
        assert_eq!(veil.extent, 340.0);
        assert_eq!(veil.corner_radius, 54.0);
        assert!((veil.opacity - GLASS_FOCUS_VEIL_OPACITY * 0.5).abs() < 0.001);
        assert!((veil.scene_blur - 0.5).abs() < 0.001);
        assert!(veil.stroke < 1280.0 * 0.5);
        assert!(veil.extent < 800.0 * 0.5);
    }
}
