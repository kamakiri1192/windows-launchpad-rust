//! Folder panel app adapter. It joins stable domain ids to discovered icon
//! records, then submits the pure `layout::folder_panel` result.

use crate::app::state::App;
use crate::domain::folders::FolderId;
use crate::domain::launcher_item::LauncherItem;
use crate::layout::folder_panel::{self, FolderChildInput, FolderPanelInput};
use crate::renderer::text_engine::{CenteredLineSpec, GlyphQuad, Label, TextRenderer};
use crate::ui_model::geometry::{Rect, UvRect};
use crate::ui_model::grid::TileAnim;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, GlassLayer, GlyphLane, GlyphView, IconSource, IconView, InkLane, InkView,
    TileView,
};
use crate::ui_model::text::{TextRole, TextWeight};
use cosmic_text::Weight;

impl App {
    pub(crate) fn open_folder(&mut self, id: FolderId) {
        if !self.launcher_state.folders.contains_key(&id) {
            return;
        }
        if self.control.wants_keyboard() {
            self.control.press_close();
        }
        self.pending_press = None;
        if let Some(scroller) = self.scroller.as_mut() {
            scroller.velocity = 0.0;
            scroller.phase = crate::scroll::Phase::Idle;
        }
        self.folders.hover = None;
        self.folders.hover_opened = None;
        self.folder_scroller = None;
        self.folders.open(id);
        self.relayout();
        self.request_redraw();
    }

    pub(crate) fn close_folder(&mut self) {
        self.folders.close();
        self.request_redraw();
    }

    pub(crate) fn folder_source_rect(&self, id: &FolderId) -> Option<Rect> {
        self.launcher_item_rect(&LauncherItem::Folder(id.clone()))
    }

    pub(crate) fn launcher_item_rect(&self, item: &LauncherItem) -> Option<Rect> {
        let index = self
            .visible_launcher_items()
            .iter()
            .position(|candidate| candidate == item)?;
        let (mut x, mut y) = self
            .layout
            .tile_position(self.viewport_phys().0 as f32, index);
        if let Some((_, spring)) = self
            .tile_springs
            .iter()
            .find(|(candidate, _)| candidate == item)
        {
            x = spring.x.value;
            y = spring.y.value;
        }
        x += self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        Some(Rect::new(
            x,
            y,
            self.layout.tile_size,
            self.layout.tile_size,
        ))
    }

    pub(crate) fn render_folder_panel(&mut self) {
        let presentation = if let Some(folder_id) = self.folders.active.clone() {
            let Some(folder) = self.launcher_state.folders.get(&folder_id).cloned() else {
                self.folders = crate::features::folders::FolderFeatureState::default();
                self.folder_scroller = None;
                self.clear_folder_panel_presentation();
                return;
            };
            let source = self.folder_source_rect(&folder_id).unwrap_or_else(|| {
                let viewport = self.viewport_phys();
                Rect::new(
                    viewport.0 as f32 * 0.5 - 0.5,
                    viewport.1 as f32 * 0.5 - 0.5,
                    1.0,
                    1.0,
                )
            });
            let order = self
                .folders
                .child_drag
                .as_ref()
                .filter(|drag| drag.folder_id == folder_id)
                .map(|drag| drag.preview_order.clone())
                .or_else(|| {
                    self.folders
                        .child_exit_preview
                        .as_ref()
                        .filter(|preview| preview.source_folder == folder_id)
                        .map(|preview| preview.source_children_without_dragged(&folder.children))
                })
                .unwrap_or_else(|| folder.children.clone());
            let dragged_key = self
                .folders
                .child_drag
                .as_ref()
                .map(|drag| drag.app_id.as_str().to_owned());
            Some((
                folder_id.as_str().to_owned(),
                folder.name,
                order,
                source,
                self.folders.page,
                self.folders.motion.visual_progress(),
                dragged_key,
                true,
            ))
        } else {
            match (self.folders.hover.as_ref(), self.drag_item.as_ref()) {
                (Some(hover), Some(LauncherItem::App(dragged))) if hover.ready() => {
                    match &hover.target {
                        LauncherItem::App(target) => {
                            self.launcher_item_rect(&hover.target).map(|source| {
                                (
                                    format!("pending:{}:{}", target.as_str(), dragged.as_str()),
                                    "フォルダ".to_owned(),
                                    vec![target.clone(), dragged.clone()],
                                    source,
                                    0,
                                    hover.panel_progress(),
                                    Some(dragged.as_str().to_owned()),
                                    false,
                                )
                            })
                        }
                        LauncherItem::Folder(_) => None,
                    }
                }
                _ => None,
            }
        };
        let Some((folder_key, folder_name, order, source, page, progress, dragged_key, durable)) =
            presentation
        else {
            self.clear_folder_panel_presentation();
            return;
        };
        let owned: Vec<_> = order
            .iter()
            .enumerate()
            .filter_map(|(index, app_id)| {
                if self.launcher_state.is_hidden(app_id) || !self.settings.shows_app(app_id) {
                    return None;
                }
                let record = self.registry.get(app_id)?;
                let (r, g, b) = crate::layout::grid::app_color(index);
                Some((
                    app_id.as_str().to_owned(),
                    record.name.clone(),
                    record.uv,
                    Color::rgba(r, g, b, 1.0),
                ))
            })
            .collect();
        let children: Vec<_> = owned
            .iter()
            .map(|(key, label, uv, color)| FolderChildInput {
                key,
                label,
                uv: *uv,
                color: *color,
            })
            .collect();
        let rename_text = durable
            .then(|| {
                self.folders
                    .rename
                    .as_ref()
                    .map(|editor| editor.visible_text())
            })
            .flatten();
        let viewport = self.viewport_phys();
        let page_scroll_x = self
            .folder_scroller
            .as_ref()
            .map(|scroller| scroller.position)
            .unwrap_or(0.0);
        let (frame_x, frame_y, frame_width, frame_height) =
            self.layout.frame_panel_rect(viewport.0 as f32);
        let mut model = folder_panel::build(FolderPanelInput {
            viewport,
            scale_factor: self.scale_factor,
            folder_key: &folder_key,
            name: &folder_name,
            rename_text: rename_text.as_deref(),
            source_rect: source,
            source_radius: self.layout.scaled(19.0),
            page_frame_rect: Rect::new(
                frame_x - frame_width * 0.5,
                frame_y - frame_height * 0.5,
                frame_width,
                frame_height,
            ),
            page_frame_radius: self.layout.scaled(crate::layout::grid::FRAME_CORNER_RADIUS),
            children: &children,
            page,
            page_scroll_x,
            progress,
            editing: durable && self.editing,
            dragged_child_key: dragged_key.as_deref(),
        });
        if durable {
            let animate_reorder = self.editing
                && self
                    .folder_scroller
                    .as_ref()
                    .is_none_or(|scroller| scroller.phase == crate::scroll::Phase::Idle)
                && progress > 0.99;
            self.update_folder_child_springs(&children, &model.child_rects, animate_reorder);
            if animate_reorder {
                self.apply_folder_child_springs(&folder_key, &children, &mut model);
            }
        } else {
            self.folder_child_springs.clear();
        }
        if durable {
            let bounds = crate::scroll::ScrollBounds {
                page_extent: model.target_panel_rect.width.max(1.0),
                page_count: model.page_count,
            };
            if let Some(scroller) = self.folder_scroller.as_mut() {
                scroller.set_bounds(bounds);
            } else {
                let mut scroller = crate::scroll::Scroller::new(bounds);
                scroller.position = -(page.min(model.page_count.saturating_sub(1)) as f32)
                    * model.target_panel_rect.width;
                self.folder_scroller = Some(scroller);
            }
        }

        // A top-level app remains pointer-attached after an existing folder
        // spring-opens. Submit that lifted copy through the generic modal lanes
        // so it stays above the panel glass while the user moves across child
        // drop targets. The domain move still commits only on release.
        if durable
            && self
                .folders
                .hover_opened
                .as_ref()
                .is_some_and(|id| id.as_str() == folder_key.as_str())
        {
            if let Some(LauncherItem::App(app_id)) = self.drag_item.as_ref() {
                if let Some(record) = self.registry.get(app_id) {
                    let drag_ui_key = LauncherItem::App(app_id.clone()).stable_key();
                    let size = self.layout.tile_size;
                    let rect = Rect::new(
                        self.drag_x - size * 0.5,
                        self.drag_y - size * 0.5,
                        size,
                        size,
                    );
                    let motion = TileAnim {
                        phase: 0.0,
                        lift: 18.0 * self.scale_factor,
                        scale: 1.12,
                        flags: TileAnim::FLAG_FIXED | TileAnim::FLAG_DRAG,
                    };
                    let (r, g, b) = crate::layout::grid::app_color(children.len());
                    model
                        .result
                        .render
                        .modal_tiles
                        .get_or_insert_with(Vec::new)
                        .push(TileView {
                            id: UiId::launcher_item(&drag_ui_key),
                            rect,
                            radius: 19.0 * self.scale_factor,
                            color: Color::rgba(r, g, b, 1.0),
                            has_icon: record.uv.is_some(),
                            motion,
                            z: 160,
                        });
                    if let Some(uv) = record.uv {
                        model
                            .result
                            .render
                            .modal_icons
                            .get_or_insert_with(Vec::new)
                            .push(IconView {
                                id: UiId::launcher_item(&drag_ui_key),
                                rect,
                                source: IconSource::AtlasUv(uv),
                                motion,
                                motion_pivot: None,
                                z: 161,
                            });
                    }
                }
            }
        }

        let mut glyphs = Vec::new();
        if let Some(text) = self.text.as_mut() {
            let labels: Vec<_> = model
                .result
                .render
                .text
                .iter()
                .filter(|view| view.style.role == TextRole::FolderItemLabel)
                .map(|view| Label {
                    text: view.text.clone(),
                    x: view.rect.x,
                    y: view.rect.y,
                    max_width: view.rect.width,
                    color: [
                        view.style.color.r,
                        view.style.color.g,
                        view.style.color.b,
                        view.style.color.a,
                    ],
                })
                .collect();
            glyphs.extend(text.layout_labels(&labels, self.scale_factor));

            for view in model
                .result
                .render
                .text
                .iter()
                .filter(|view| view.style.role == TextRole::FolderTitle)
            {
                let line_height = view.rect.height / self.scale_factor.max(0.01);
                let fitted = fit_centered_text(
                    text,
                    &view.text,
                    view.rect.width,
                    view.style.size,
                    line_height,
                    self.scale_factor,
                    cosmic_weight(view.style.weight),
                );
                glyphs.append(&mut text.layout_centered_line_weighted(
                    &CenteredLineSpec {
                        text: &fitted,
                        font_size: view.style.size,
                        line_height,
                        family: "Yu Gothic UI",
                        color: [
                            view.style.color.r,
                            view.style.color.g,
                            view.style.color.b,
                            view.style.color.a,
                        ],
                        center: (view.rect.center().x, view.rect.center().y),
                        scale_factor: self.scale_factor,
                    },
                    cosmic_weight(view.style.weight),
                ));
            }
            if let (Some(editor), Some(title_view)) = (
                self.folders.rename.as_ref(),
                model
                    .result
                    .render
                    .text
                    .iter()
                    .find(|view| view.style.role == TextRole::FolderTitle),
            ) {
                let visible = editor.visible_text();
                let mut caret_prefix = editor.text[..editor.cursor].to_owned();
                caret_prefix.push_str(&editor.preedit);
                let line_height = title_view.rect.height / self.scale_factor.max(0.01);
                let visible_spec = CenteredLineSpec {
                    text: &visible,
                    font_size: title_view.style.size,
                    line_height,
                    family: "Yu Gothic UI",
                    color: [1.0; 4],
                    center: (0.0, 0.0),
                    scale_factor: self.scale_factor,
                };
                let prefix_spec = CenteredLineSpec {
                    text: &caret_prefix,
                    font_size: title_view.style.size,
                    line_height,
                    family: "Yu Gothic UI",
                    color: [1.0; 4],
                    center: (0.0, 0.0),
                    scale_factor: self.scale_factor,
                };
                let weight = cosmic_weight(title_view.style.weight);
                let total_width = text.measure_text_weighted(&visible_spec, weight);
                let prefix_width = text.measure_text_weighted(&prefix_spec, weight);
                let text_gap = if prefix_width > 0.0 {
                    1.5 * self.scale_factor
                } else {
                    0.0
                };
                let caret_x =
                    title_view.rect.center().x - total_width * 0.5 + prefix_width + text_gap;
                let caret_opacity =
                    crate::layout::control_geometry::caret_blink_opacity(self.control.caret_phase);
                if let Some(batch) = model
                    .result
                    .render
                    .ink
                    .iter_mut()
                    .find(|batch| batch.lane == InkLane::Modal)
                {
                    batch.views.push(InkView {
                        id: UiId::folder_title("rename-caret"),
                        center: crate::ui_model::geometry::Point::new(
                            caret_x,
                            title_view.rect.center().y,
                        ),
                        extent: 10.0 * self.scale_factor,
                        opacity: 0.95 * caret_opacity,
                        scene_blur: 0.0,
                        stroke: 1.0 * self.scale_factor,
                        corner_radius: 1.0 * self.scale_factor,
                        color: Color::rgba(1.0, 1.0, 1.0, 1.0),
                        kind: ControlKind::Caret,
                        z: 131,
                    });
                }
            }
            if text.atlas_dirty {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.upload_atlas(text.atlas_rgba());
                }
                text.atlas_dirty = false;
            }
        }

        let modal = model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::Modal)
            .map(|batch| batch.surfaces.clone())
            .unwrap_or_default();
        let backdrop = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Backdrop)
            .map(|batch| batch.views.clone())
            .unwrap_or_default();
        let ink = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::Modal)
            .map(|batch| batch.views.clone())
            .unwrap_or_default();
        self.render_model.set_glass_batch(GlassLayer::Modal, modal);
        self.render_model.set_ink_batch(InkLane::Backdrop, backdrop);
        self.render_model.set_ink_batch(InkLane::Modal, ink);
        self.render_model.modal_tiles = model.result.render.modal_tiles.clone();
        self.render_model.modal_icons = model.result.render.modal_icons.clone();
        self.render_model
            .set_glyph_batch(GlyphLane::Modal, glyph_views(&glyphs));
        self.folder_layout = Some(model);
    }

    fn update_folder_child_springs(
        &mut self,
        children: &[FolderChildInput<'_>],
        rects: &[Rect],
        animate: bool,
    ) {
        let mut old = std::mem::take(&mut self.folder_child_springs);
        self.folder_child_springs.reserve(children.len());
        for (child, rect) in children.iter().zip(rects) {
            let id = crate::domain::app_id::AppId::from_normalized(child.key.to_owned());
            if let Some(index) = old.iter().position(|(candidate, _)| candidate == &id) {
                let (_, mut spring) = old.swap_remove(index);
                if animate {
                    spring.glide_to(rect.x, rect.y);
                } else {
                    spring.snap_to(rect.x, rect.y);
                }
                self.folder_child_springs.push((id, spring));
            } else {
                self.folder_child_springs
                    .push((id, crate::scroll::Spring2::at(rect.x, rect.y)));
            }
        }
    }

    fn apply_folder_child_springs(
        &self,
        folder_key: &str,
        children: &[FolderChildInput<'_>],
        model: &mut folder_panel::FolderPanelModel,
    ) {
        for (child, target) in children.iter().zip(model.child_rects.clone()) {
            let id = crate::domain::app_id::AppId::from_normalized(child.key.to_owned());
            let Some((_, spring)) = self
                .folder_child_springs
                .iter()
                .find(|(candidate, _)| candidate == &id)
            else {
                continue;
            };
            let dx = spring.x.value - target.x;
            let dy = spring.y.value - target.y;
            let ui_id = UiId::folder_child(folder_key, child.key);
            let matches_child = |candidate: &UiId| candidate == &ui_id;

            for tile in model
                .result
                .render
                .modal_tiles
                .iter_mut()
                .flatten()
                .filter(|view| matches_child(&view.id))
            {
                tile.rect.x += dx;
                tile.rect.y += dy;
            }
            for icon in model
                .result
                .render
                .modal_icons
                .iter_mut()
                .flatten()
                .filter(|view| matches_child(&view.id))
            {
                icon.rect.x += dx;
                icon.rect.y += dy;
            }
            for text in model
                .result
                .render
                .text
                .iter_mut()
                .filter(|view| matches_child(&view.id))
            {
                text.rect.x += dx;
                text.rect.y += dy;
            }
        }
    }

    pub(crate) fn step_folder_child_springs(&mut self, dt: f32) -> bool {
        let cfg = self
            .folder_scroller
            .as_ref()
            .or(self.scroller.as_ref())
            .map(|scroller| scroller.cfg)
            .unwrap_or_default();
        self.folder_child_springs
            .iter_mut()
            .fold(false, |animating, (_, spring)| {
                spring.step(dt, &cfg) || animating
            })
    }

    fn clear_folder_panel_presentation(&mut self) {
        if !self.folders.is_active() {
            self.folder_scroller = None;
            self.folder_child_springs.clear();
        }
        self.folder_layout = None;
        self.render_model.modal_tiles = Some(Vec::new());
        self.render_model.modal_icons = Some(Vec::new());
        self.render_model
            .set_ink_batch(InkLane::Backdrop, Vec::new());
        self.render_model.set_ink_batch(InkLane::Modal, Vec::new());
        self.render_model
            .set_glyph_batch(GlyphLane::Modal, Vec::new());
        if !self.settings_panel_active() {
            self.render_model
                .set_glass_batch(GlassLayer::Modal, Vec::new());
        }
    }
}

/// Fit a single title/label to its renderer-neutral layout rect without ever
/// slicing a UTF-8 code point. The persistent name remains untouched; only the
/// presentation gets an ellipsis.
fn fit_centered_text(
    renderer: &mut TextRenderer,
    value: &str,
    max_width: f32,
    font_size: f32,
    line_height: f32,
    scale_factor: f32,
    weight: Weight,
) -> String {
    let measure = |renderer: &mut TextRenderer, text: &str| {
        renderer.measure_text_weighted(
            &CenteredLineSpec {
                text,
                font_size,
                line_height,
                family: "Yu Gothic UI",
                color: [1.0; 4],
                center: (0.0, 0.0),
                scale_factor,
            },
            weight,
        )
    };
    if measure(renderer, value) <= max_width {
        return value.to_owned();
    }

    let chars: Vec<char> = value.chars().collect();
    let mut low = 0usize;
    let mut high = chars.len();
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = chars[..mid]
            .iter()
            .copied()
            .chain(std::iter::once('…'))
            .collect::<String>();
        if measure(renderer, &candidate) <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    chars[..low]
        .iter()
        .copied()
        .chain(std::iter::once('…'))
        .collect()
}

fn cosmic_weight(weight: TextWeight) -> Weight {
    match weight {
        TextWeight::Regular => Weight::NORMAL,
        TextWeight::Medium => Weight::MEDIUM,
        TextWeight::Bold => Weight::BOLD,
    }
}

fn glyph_views(quads: &[GlyphQuad]) -> Vec<GlyphView> {
    quads
        .iter()
        .map(|quad| GlyphView {
            id: UiId::backdrop("folder-text"),
            rect: Rect::new(quad.x, quad.y, quad.w, quad.h),
            uv: UvRect {
                u0: quad.u0,
                v0: quad.v0,
                u1: quad.u1,
                v1: quad.v1,
            },
            color: Color::rgba(quad.color[0], quad.color[1], quad.color[2], quad.color[3]),
            z: 130,
        })
        .collect()
}
