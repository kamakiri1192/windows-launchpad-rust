//! HUD-style overlays drawn directly on the final surface view.
//!
//! Currently only the optional FPS counter is rendered here. It is generated
//! every frame from [`App`]'s cached reading of the renderer's FPS tracker
//! and uploaded into [`GlyphLane::Overlay`], which the renderer draws last so
//! the counter floats above every glass/modal layer.

use crate::app::state::App;
use crate::renderer::text_engine as text;
use crate::ui_model::geometry::{Rect, UvRect};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{Color, GlyphLane, GlyphView};

/// Margin from the viewport's top-right corner to the FPS counter's outer
/// edge, in logical px (scaled by `scale_factor` before use).
const OVERLAY_MARGIN_LOGICAL: f32 = 10.0;
/// Font size of the FPS counter, in logical px.
const OVERLAY_FONT_SIZE: f32 = 13.0;
/// Line height of the FPS counter, in logical px.
const OVERLAY_LINE_HEIGHT: f32 = 18.0;
/// Flat white at ~85% alpha — legible over both bright and dark scenes
/// without the heavy layered shadow the app labels use.
const OVERLAY_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.85];

impl App {
    /// Build and upload the FPS overlay glyphs for this frame.
    ///
    /// No-op (uploads an empty batch) when the overlay is disabled, so the
    /// renderer's dirty tracking simply stops touching the overlay lane.
    pub(crate) fn render_fps_overlay(&mut self) {
        // QA capture must not record the overlay — it would clash with the
        // golden-image harness and also the offscreen path never calls
        // present() so the tracker reading is meaningless.
        let show = self.settings.show_fps && !self.qa_enabled();

        let mut quads: Vec<text::GlyphQuad> = Vec::new();
        if show {
            // Borrow self immutably first to snapshot the values we need,
            // before we take the mutable text-renderer borrow below.
            let fps = self.renderer.as_ref().map(|r| r.last_fps).unwrap_or(0);
            let (vp_w, _vp_h) = self.viewport_phys();
            let scale = self.scale_factor;
            let margin = OVERLAY_MARGIN_LOGICAL * scale;
            let line_height = OVERLAY_LINE_HEIGHT * scale;
            let font_size = OVERLAY_FONT_SIZE * scale;
            let label = format!("FPS: {fps}");

            if let Some(t) = self.text.as_mut() {
                // Measure first so we can right-align the short label against
                // the top-right corner, mirroring `push_text_right`.
                let width = t.measure_text(&text::CenteredLineSpec {
                    text: &label,
                    font_size,
                    line_height,
                    family: text::UI_FONT_FAMILY,
                    color: OVERLAY_COLOR,
                    center: (0.0, 0.0),
                    scale_factor: scale,
                });
                let center_x = vp_w as f32 - margin - width * 0.5;
                let center_y = margin + line_height * 0.5;
                quads.extend(t.layout_centered_line(&text::CenteredLineSpec {
                    text: &label,
                    font_size,
                    line_height,
                    family: text::UI_FONT_FAMILY,
                    color: OVERLAY_COLOR,
                    center: (center_x, center_y),
                    scale_factor: scale,
                }));
            }
        }

        self.render_model
            .set_glyph_batch(GlyphLane::Overlay, overlay_glyph_views(&quads));

        // Keep the text atlas current so the renderer uploads any newly
        // rasterized glyphs (the overlay is the first consumer of digits /
        // colon glyphs on a fresh atlas).
        if let (Some(renderer), Some(text)) = (self.renderer.as_mut(), self.text.as_ref()) {
            if text.atlas_dirty {
                let (aw, ah) = text.atlas_dimensions();
                renderer.upload_atlas(text.atlas_rgba(), aw, ah);
            }
        }
        if let Some(text) = self.text.as_mut() {
            text.atlas_dirty = false;
        }
    }
}

fn overlay_glyph_views(quads: &[text::GlyphQuad]) -> Vec<GlyphView> {
    quads
        .iter()
        .map(|quad| GlyphView {
            id: UiId::fps_overlay(),
            rect: Rect::new(quad.x, quad.y, quad.w, quad.h),
            uv: UvRect {
                u0: quad.u0,
                v0: quad.v0,
                u1: quad.u1,
                v1: quad.v1,
            },
            color: Color::rgba(quad.color[0], quad.color[1], quad.color[2], quad.color[3]),
            z: 0,
            clip: None,
        })
        .collect()
}
