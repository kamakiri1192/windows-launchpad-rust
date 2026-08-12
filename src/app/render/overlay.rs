//! HUD-style overlays drawn directly on the final surface view.
//!
//! Optional FPS and automatic-darkness diagnostics are generated here and
//! uploaded into [`GlyphLane::Overlay`], which the renderer draws last so the
//! debug HUD floats above every glass/modal layer.

use crate::app::state::App;
use crate::renderer::text_engine as text;
use crate::ui_model::geometry::{Rect, UvRect};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{Color, GlyphLane, GlyphView};

/// Margin from the viewport's top-right corner to the debug HUD's outer
/// edge, in logical px (scaled by `scale_factor` before use).
const OVERLAY_MARGIN_LOGICAL: f32 = 10.0;
/// Font size of the debug HUD, in logical px.
const OVERLAY_FONT_SIZE: f32 = 13.0;
/// Line height of the debug HUD, in logical px.
const OVERLAY_LINE_HEIGHT: f32 = 18.0;
/// Flat white at ~85% alpha — legible over both bright and dark scenes
/// without the heavy layered shadow the app labels use.
const OVERLAY_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.85];

impl App {
    /// Build and upload debug-overlay glyphs for this frame.
    ///
    /// No-op (uploads an empty batch) when the overlay is disabled, so the
    /// renderer's dirty tracking simply stops touching the overlay lane.
    pub(crate) fn render_debug_overlay(&mut self) {
        // QA capture must not record the overlay — it would clash with the
        // golden-image harness and also the offscreen path never calls
        // present() so the tracker reading is meaningless.
        let qa_enabled = self.qa_enabled();
        let show_fps = self.settings.show_fps && !qa_enabled;
        let show_auto_darkness = self.settings.show_auto_darkness_value && !qa_enabled;

        // GPU readback is opt-in and nonblocking. Polling returns the latest
        // completed sample, never waiting for the current frame.
        let (auto_darkness_enabled, auto_darkness) = self
            .renderer
            .as_mut()
            .map(|renderer| {
                let enabled = renderer.liquid_glass_auto_darkness_enabled();
                renderer.set_auto_darkness_debug_readback_enabled(show_auto_darkness && enabled);
                let value = if enabled {
                    renderer.poll_auto_darkness_debug_value()
                } else {
                    0.0
                };
                (enabled, value)
            })
            .unwrap_or((false, 0.0));

        let mut lines = Vec::with_capacity(2);
        if show_fps {
            let fps = self.renderer.as_ref().map(|r| r.last_fps).unwrap_or(0);
            lines.push((UiId::fps_overlay(), format!("FPS: {fps}")));
        }
        if show_auto_darkness {
            lines.push((
                UiId::auto_darkness_overlay(),
                format_auto_darkness_value(auto_darkness_enabled, auto_darkness),
            ));
        }

        let mut glyph_views = Vec::new();
        if !lines.is_empty() {
            // Borrow self immutably first to snapshot the values we need,
            // before we take the mutable text-renderer borrow below.
            let (vp_w, _vp_h) = self.viewport_phys();
            let scale = self.scale_factor;
            let margin = OVERLAY_MARGIN_LOGICAL * scale;
            let line_height = OVERLAY_LINE_HEIGHT * scale;
            let font_size = OVERLAY_FONT_SIZE * scale;

            if let Some(t) = self.text.as_mut() {
                for (line_index, (id, label)) in lines.iter().enumerate() {
                    // Measure first so each short label is independently
                    // right-aligned against the top-right corner.
                    let width = t.measure_text(&text::CenteredLineSpec {
                        text: label,
                        font_size,
                        line_height,
                        family: text::UI_FONT_FAMILY,
                        color: OVERLAY_COLOR,
                        center: (0.0, 0.0),
                        scale_factor: scale,
                    });
                    let center_x = vp_w as f32 - margin - width * 0.5;
                    let center_y = margin + line_height * (line_index as f32 + 0.5);
                    let quads = t.layout_centered_line(&text::CenteredLineSpec {
                        text: label,
                        font_size,
                        line_height,
                        family: text::UI_FONT_FAMILY,
                        color: OVERLAY_COLOR,
                        center: (center_x, center_y),
                        scale_factor: scale,
                    });
                    glyph_views.extend(overlay_glyph_views(&quads, id));
                }
            }
        }

        self.render_model
            .set_glyph_batch(GlyphLane::Overlay, glyph_views);

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

fn format_auto_darkness_value(enabled: bool, value: f32) -> String {
    if enabled {
        format!("Dynamic darkness: {:.3}", value.max(0.0))
    } else {
        "Dynamic darkness: OFF".to_owned()
    }
}

fn overlay_glyph_views(quads: &[text::GlyphQuad], id: &UiId) -> Vec<GlyphView> {
    quads
        .iter()
        .map(|quad| GlyphView {
            id: id.clone(),
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

#[cfg(test)]
mod tests {
    use super::format_auto_darkness_value;

    #[test]
    fn auto_darkness_overlay_uses_three_decimal_places() {
        assert_eq!(
            format_auto_darkness_value(true, 0.12345),
            "Dynamic darkness: 0.123"
        );
        assert_eq!(
            format_auto_darkness_value(true, -0.1),
            "Dynamic darkness: 0.000"
        );
    }

    #[test]
    fn auto_darkness_overlay_says_off_when_feature_is_disabled() {
        assert_eq!(
            format_auto_darkness_value(false, 0.12345),
            "Dynamic darkness: OFF"
        );
    }
}
