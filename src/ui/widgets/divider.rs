//! `Divider` widget — a thin horizontal separator line.
//!
//! Matches the existing `divider_instance` in `app/render/settings.rs`.

use crate::ui::context::Ui;
use crate::ui::response::Response;
use crate::ui_model::geometry::Rect;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{ControlKind, InkView};

use super::{color_from_array, Z_CONTROL};

/// Colour palette mirroring `settings_panel::DIM`.
pub const DIM: [f32; 4] = [1.0, 1.0, 1.0, 0.34];

// ---------------------------------------------------------------------------
// Divider
// ---------------------------------------------------------------------------

/// A thin horizontal divider line.
///
/// Placed via `ui.divider()` (or `ui.divider_with(&divider_def)`).  It spans
/// the full available width with a height of `1px * scale_factor`.
#[derive(Clone, Debug)]
pub struct Divider {
    pub id: UiId,
}

impl Default for Divider {
    fn default() -> Self {
        Self::new()
    }
}

impl Divider {
    /// Create a default divider.
    pub fn new() -> Self {
        Self {
            id: UiId::named(""),
        }
    }

    /// Assign a stable `UiId`.
    pub fn id(mut self, id: UiId) -> Self {
        self.id = id;
        self
    }

    fn ensure_id(mut self, ui: &mut Ui) -> Self {
        if self.id.as_str().is_empty() {
            self.id = ui.next_anon_id();
        }
        self
    }
}

impl Ui {
    /// Place a divider widget and return its [`Response`].
    ///
    /// The divider is non-interactive.  It renders as a thin rounded rectangle
    /// spanning the full available width, using the `DIM` colour.
    pub fn divider(&mut self, divider: &Divider) -> Response {
        let d = divider.clone().ensure_id(self);
        let scale = self.scale_factor();
        let height = 1.0 * scale;

        self.begin_widget();

        let rect = Rect::new(self.cursor_x, self.cursor_y, self.available_width, height);

        // Draw as a thin round-rect InkView (matches existing divider_instance).
        let center = rect.center();
        let half_h = height * 0.5;
        let half_w = rect.width * 0.5;

        let ink = InkView {
            id: d.id.clone(),
            center,
            extent: half_h,
            opacity: DIM[3],
            scene_blur: 0.0,
            stroke: half_w,
            corner_radius: half_h,
            color: color_from_array(DIM),
            kind: ControlKind::Divider,
            z: Z_CONTROL,
            clip: None,
        };
        self.push_ink(ink);

        // Advance cursor.
        match self.direction {
            crate::ui::context::LayoutDirection::Vertical => {
                self.cursor_y += height;
            }
            crate::ui::context::LayoutDirection::Horizontal => {
                self.cursor_x += rect.width;
            }
        }

        self.register(d.id.clone(), rect, rect);

        Response {
            id: d.id,
            rect,
            hit_rect: rect,
            ..Default::default()
        }
    }

    /// Convenience: place a divider with default styling and no explicit id.
    pub fn divider_default(&mut self) -> Response {
        self.divider(&Divider::new())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Theme;

    fn new_ui() -> Ui {
        Ui::new(Theme::default(), 800.0, 600.0)
    }

    #[test]
    fn divider_pushes_ink_view() {
        let mut ui = new_ui();
        let resp = ui.divider(&Divider::new());
        // Non-interactive.
        assert!(!resp.hovered);
        assert!(!resp.pressed);
        assert!(!resp.clicked);
        assert_eq!(resp.rect.width, 800.0);
        assert_eq!(resp.rect.height, 1.0);
    }

    #[test]
    fn divider_height_scales_with_dpi() {
        let theme = Theme {
            scale_factor: 2.0,
            ..Default::default()
        };
        let mut ui = Ui::new(theme, 800.0, 600.0);
        let resp = ui.divider(&Divider::new());
        assert_eq!(resp.rect.height, 2.0);
    }

    #[test]
    fn divider_with_explicit_id_registers() {
        let mut ui = new_ui();
        let id = UiId::named("my-divider");
        let resp = ui.divider(&Divider::new().id(id.clone()));
        assert_eq!(resp.id, id);
        assert_eq!(ui.rect(&id), Some(resp.rect));
    }

    #[test]
    fn divider_take_output_contains_ink() {
        let mut ui = new_ui();
        ui.divider(&Divider::new());
        let (render, _hits, _reg) = ui.take();
        assert!(!render.ink.is_empty());
        let ink = &render.ink[0].views[0];
        assert_eq!(ink.kind, ControlKind::Divider);
        assert_eq!(ink.extent, 0.5); // half of 1px height
    }

    #[test]
    fn divider_default_works() {
        let mut ui = new_ui();
        let resp = ui.divider_default();
        assert_eq!(resp.rect.height, 1.0);
    }
}
