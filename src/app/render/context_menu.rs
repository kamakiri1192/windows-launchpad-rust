//! Context menu app adapter. Joins the live [`ContextMenuState`] to the pure
//! [`layout::context_menu`] builder, then submits the result to the renderer
//! model on the dedicated `ContextMenu` glass/ink/glyph lanes. These lanes
//! are isolated from the folder panel's `Modal` lanes so the menu can float
//! above an open folder without their Liquid Glass smooth-unioning together.

use crate::app::state::App;
use crate::domain::app_id::AppId;
use crate::features::context_menu::MenuTarget;
use crate::layout::context_menu::{
    self, open_panel_origin, open_panel_size_logical, ContextMenuInput,
};
use crate::renderer::text_engine::{self, GlyphQuad, UI_FONT_FAMILY};
use crate::ui_model::render_model::{GlassLayer, GlyphLane, InkLane};

/// Menu font metrics, in logical px at 1× DPI. Match the app-icon label size
/// (`LABEL_FONT_SIZE` = 14) so the menu reads at the same scale as the grid.
/// The text renderer multiplies these by the DPI `scale_factor`.
const MENU_FONT_SIZE: f32 = 14.0;
const MENU_LINE_HEIGHT: f32 = 18.0;

impl App {
    /// Open the context menu for `app_id`, anchored at the physical-px click
    /// point. The menu uses its own `ContextMenu` glass lane, isolated from the
    /// folder/settings `Modal` lane, so an open folder panel stays visible
    /// while the menu is shown and remains open after it closes.
    pub(crate) fn open_context_menu(&mut self, app_id: AppId, x: f32, y: f32) {
        if self.control.wants_keyboard() {
            self.control.press_close();
        }
        self.pending_press = None;

        let scale = self.scale_factor.max(0.01);
        // Measure the longest label once at open time so the open-animation
        // target and the per-frame layout agree on the same panel width.
        let max_label_w = self.measure_menu_max_label_width_logical(scale);
        self.context_menu_open_width_logical = max_label_w;
        let (lw, lh) =
            open_panel_size_logical(context_menu::ContextMenuItem::ALL.len(), max_label_w);
        let size_phys = (lw * scale, lh * scale);
        let origin = open_panel_origin((x, y), size_phys, self.viewport_phys());
        let target = MenuTarget {
            x: origin.0,
            y: origin.1,
            width: size_phys.0,
            height: size_phys.1,
        };
        self.context_menu.open(app_id, x, y, target);
        self.request_redraw();
    }

    /// Begin the close animation. The menu stays visible until the close
    /// animation finishes.
    pub(crate) fn close_context_menu(&mut self) {
        if !self.context_menu.is_active() {
            return;
        }
        self.context_menu.close();
        self.request_redraw();
    }

    /// Press while the menu is open. Outside the panel → dismiss; we let the
    /// release handler finalize so a drag that returns inside still works.
    pub(crate) fn handle_context_menu_pointer_press(&mut self, x: f32, y: f32) {
        let inside = self
            .context_menu_layout
            .as_ref()
            .map(|m| {
                m.panel_rect
                    .contains(crate::ui_model::geometry::Point::new(x, y))
            })
            .unwrap_or(false);
        if !inside {
            // Mark intent; the release confirms dismiss. For simplicity we
            // close immediately on outside press — the menu has no drag.
            self.close_context_menu();
        }
    }

    /// Release while the menu is open. Inside a row → mock action (close);
    /// outside → already closed by the press, or close now.
    pub(crate) fn handle_context_menu_pointer_release(&mut self, x: f32, y: f32) {
        if !self.context_menu.is_active() {
            return;
        }
        let hit = self.context_menu_hit_target(x, y);
        match hit {
            // A row was selected: mock action — just close. Real actions are
            // wired in a later iteration.
            Some(_) => self.close_context_menu(),
            None => self.close_context_menu(),
        }
    }

    fn context_menu_hit_target(&self, x: f32, y: f32) -> Option<usize> {
        let model = self.context_menu_layout.as_ref()?;
        let p = crate::ui_model::geometry::Point::new(x, y);
        for (index, row) in model.rows.iter().enumerate() {
            if row.rect.contains(p) {
                return Some(index);
            }
        }
        None
    }

    /// Build the context-menu render model from the live animation state and
    /// submit it to the Modal lanes. Called from the frame loop while the menu
    /// is active.
    pub(crate) fn render_context_menu(&mut self) {
        let app_id = match self.context_menu.active_app.clone() {
            Some(id) => id,
            None => {
                self.clear_context_menu_presentation();
                return;
            }
        };

        let scale = self.scale_factor.max(0.01);
        let items = context_menu::ContextMenuItem::ALL;
        let labels: Vec<&str> = items.iter().map(|i| i.label()).collect();

        // The fully-open panel size is fixed at open time and stays constant
        // through the animation; the live (animated) size is separate. We reuse
        // the label width measured in `open_context_menu` so the laid-out rows
        // match the animated panel exactly.
        let (open_lw, open_lh) =
            open_panel_size_logical(items.len(), self.context_menu_open_width_logical);
        let open_size = (open_lw * scale, open_lh * scale);

        let input = ContextMenuInput {
            viewport: self.viewport_phys(),
            scale_factor: scale,
            app_id: app_id.as_str(),
            pos: (self.context_menu.pos_x(), self.context_menu.pos_y()),
            size: (self.context_menu.width(), self.context_menu.height()),
            open_size,
            radius: self.context_menu.radius(),
            content_scale: self.context_menu.content_scale(),
            content_opacity: self.context_menu.content_opacity(),
            content_blur: self.context_menu.content_blur(),
            activation: self.context_menu.activation(),
            items: &items,
            labels: &labels,
        };
        let model = context_menu::build(&input);

        // Promote the layout's ink/glass into the shared Modal lanes.
        let modal = model
            .result
            .render
            .glass
            .iter()
            .find(|batch| batch.layer == GlassLayer::ContextMenu)
            .map(|batch| batch.surfaces.clone())
            .unwrap_or_default();
        let ink = model
            .result
            .render
            .ink
            .iter()
            .find(|batch| batch.lane == InkLane::ContextMenu)
            .map(|batch| batch.views.clone())
            .unwrap_or_default();

        // Shape label text into glyph quads. We render only when the content
        // has meaningfully revealed to avoid wasted raster work mid-collapse.
        let mut glyphs: Vec<GlyphQuad> = Vec::new();
        let opacity = self.context_menu.content_opacity();
        let content_scale = self.context_menu.content_scale().max(0.0);
        if opacity > 0.02 {
            let color = [0.95, 0.96, 0.98, opacity.clamp(0.0, 1.0)];
            if let Some(text) = self.text.as_mut() {
                for (row, label) in model.rows.iter().zip(labels.iter()) {
                    let left = row.label_rect.x;
                    let center_y = row.label_rect.y;
                    // Scale the font with content_scale so the text shrinks/grows
                    // in sync with the glass + ink during open/close morph.
                    push_menu_text(
                        text,
                        &mut glyphs,
                        label,
                        left,
                        center_y,
                        MENU_FONT_SIZE * content_scale,
                        MENU_LINE_HEIGHT * content_scale,
                        color,
                        scale,
                    );
                }
            }
        }

        // The menu owns the Modal lane exclusively (the folder/settings panels
        // are dismissed on open), so a plain replace is correct and keeps the
        // Liquid Glass modal pass on a single surface.
        //
        // Glass has no opacity field, so once the content has faded below the
        // reveal threshold we drop the glass disc entirely — otherwise the
        // collapsed seed (40×40, radius 130 = full disc) lingers until the slow
        // close position spring settles. We still keep the layout so hit/dismiss
        // logic stays valid during the close tail.
        if opacity > 0.02 {
            self.render_model
                .set_glass_batch(GlassLayer::ContextMenu, modal);
            self.render_model.set_ink_batch(InkLane::ContextMenu, ink);
            self.render_model
                .set_glyph_batch(GlyphLane::ContextMenu, glyph_views(&glyphs));
            self.render_model.context_menu_tiles = model.result.render.context_menu_tiles.clone();
            self.render_model.context_menu_icons = model.result.render.context_menu_icons.clone();
        } else {
            self.render_model
                .set_glass_batch(GlassLayer::ContextMenu, Vec::new());
            self.render_model
                .set_ink_batch(InkLane::ContextMenu, Vec::new());
            self.render_model
                .set_glyph_batch(GlyphLane::ContextMenu, Vec::new());
            self.render_model.context_menu_tiles = Some(Vec::new());
            self.render_model.context_menu_icons = Some(Vec::new());
        }

        // Keep the text atlas current so the renderer uploads any newly
        // rasterized menu glyphs. Every other text adapter does this;
        // omitting it left the menu's new glyphs missing from the GPU
        // texture, so the menu text vanished (and, once the base atlas
        // filled, every other lane's text too).
        if let (Some(renderer), Some(text)) = (self.renderer.as_mut(), self.text.as_ref()) {
            if text.atlas_dirty {
                renderer.upload_atlas(text.atlas_rgba());
            }
        }
        if let Some(text) = self.text.as_mut() {
            text.atlas_dirty = false;
        }

        self.context_menu_layout = Some(model);
    }

    /// Drop the context menu's Modal-lane content (called when the menu is
    /// fully closed). The Modal lane is exclusive — the folder/settings panels
    /// are already dismissed — so a full clear is correct.
    pub(crate) fn clear_context_menu_presentation(&mut self) {
        self.render_model
            .set_glass_batch(GlassLayer::ContextMenu, Vec::new());
        self.render_model
            .set_ink_batch(InkLane::ContextMenu, Vec::new());
        self.render_model
            .set_glyph_batch(GlyphLane::ContextMenu, Vec::new());
        self.render_model.context_menu_tiles = Some(Vec::new());
        self.render_model.context_menu_icons = Some(Vec::new());
        self.context_menu_layout = None;
    }

    /// Measure the widest menu label and return its width in logical px at 1×
    /// DPI (i.e. the physical measurement divided by `scale`). Used once at
    /// open time to size the panel to its content. Falls back to the layout
    /// layer's [`FALLBACK_MAX_LABEL_WIDTH`] when the text engine is absent.
    fn measure_menu_max_label_width_logical(&mut self, scale: f32) -> f32 {
        let Some(t) = self.text.as_mut() else {
            return context_menu::FALLBACK_MAX_LABEL_WIDTH;
        };
        let mut widest_phys = 0.0f32;
        for item in context_menu::ContextMenuItem::ALL {
            let w = t.measure_text(&text_engine::CenteredLineSpec {
                text: item.label(),
                font_size: MENU_FONT_SIZE,
                line_height: MENU_LINE_HEIGHT,
                family: UI_FONT_FAMILY,
                color: [1.0; 4],
                center: (0.0, 0.0),
                scale_factor: scale,
            });
            widest_phys = widest_phys.max(w);
        }
        (widest_phys / scale.max(0.01)).max(0.0)
    }
}

#[allow(clippy::too_many_arguments)]
fn push_menu_text(
    t: &mut text_engine::TextRenderer,
    quads: &mut Vec<GlyphQuad>,
    value: &str,
    left: f32,
    center_y: f32,
    font_size: f32,
    line_height: f32,
    color: [f32; 4],
    scale: f32,
) {
    let width = t.measure_text(&text_engine::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: UI_FONT_FAMILY,
        color,
        center: (0.0, 0.0),
        scale_factor: scale,
    });
    quads.append(&mut t.layout_centered_line(&text_engine::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: UI_FONT_FAMILY,
        color,
        center: (left + width * 0.5, center_y),
        scale_factor: scale,
    }));
}

fn glyph_views(quads: &[GlyphQuad]) -> Vec<crate::ui_model::render_model::GlyphView> {
    quads
        .iter()
        .map(|q| crate::ui_model::render_model::GlyphView {
            id: crate::ui_model::ids::UiId::named("context-menu-glyph"),
            rect: crate::ui_model::geometry::Rect::new(q.x, q.y, q.w, q.h),
            uv: crate::ui_model::geometry::UvRect {
                u0: q.u0,
                v0: q.v0,
                u1: q.u1,
                v1: q.v1,
            },
            color: crate::ui_model::render_model::Color::rgba(
                q.color[0], q.color[1], q.color[2], q.color[3],
            ),
            z: 141,
            clip: None,
        })
        .collect()
}
