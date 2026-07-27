//! `Label` and `Heading` widgets — non-interactive text.
//!
//! These follow the text rendering patterns from the existing settings panel
//! (HEADER_SIZE/LABEL_SIZE/DETAIL_SIZE, TextRole, TextWeight).

use crate::ui::context::Ui;
use crate::ui::response::Response;
use crate::ui_model::geometry::Rect;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::Color;
use crate::ui_model::text::{TextAlign, TextRole, TextStyle, TextView, TextWeight};

use super::{color_from_array, Z_CONTROL};

// ---------------------------------------------------------------------------
// Sizing constants (mirrors `settings_panel.rs`)
// ---------------------------------------------------------------------------

/// Heading font size in logical pixels.
pub const HEADING_SIZE: f32 = 21.0;
/// Heading line height in logical pixels.
pub const HEADING_LINE: f32 = 28.0;
/// Default label font size in logical pixels.
pub const LABEL_SIZE: f32 = 14.0;
/// Default label line height in logical pixels.
pub const LABEL_LINE: f32 = 20.0;
/// Detail text font size in logical pixels.
pub const DETAIL_SIZE: f32 = 12.0;
/// Detail text line height in logical pixels.
pub const DETAIL_LINE: f32 = 18.0;

// ---------------------------------------------------------------------------
// Label
// ---------------------------------------------------------------------------

/// A non-interactive text label.
///
/// Use `ui.label(&label_def)` to place it.  For a heading variant use
/// `Heading::new(text)` (which calls `ui.heading(&label_def)` internally) or
/// call `ui.label()` on a `Label` with a heading-sized style.
#[derive(Clone, Debug)]
pub struct Label {
    pub id: UiId,
    pub text: String,
    pub style: TextStyle,
    pub color: [f32; 4],
    pub align: TextAlign,
}

impl Label {
    /// Create a new label with the given text.
    ///
    /// Default style: `SettingsRow` role, `LABEL_SIZE` size, `Regular` weight,
    /// `Start` alignment, ink colour.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            id: UiId::named(""),
            text: text.into(),
            style: TextStyle::new(
                TextRole::SettingsRow,
                LABEL_SIZE,
                Color::rgba(1.0, 1.0, 1.0, 0.92),
                TextWeight::Regular,
                TextAlign::Start,
            ),
            color: [0.95, 0.96, 0.98, 1.0], // theme ink default
            align: TextAlign::Start,
        }
    }

    /// Assign a stable `UiId` for element-state tracking and registration.
    pub fn id(mut self, id: UiId) -> Self {
        self.id = id;
        self
    }

    /// Override the text style.
    pub fn style(mut self, style: TextStyle) -> Self {
        self.style = style;
        self
    }

    /// Override the text colour (RGBA `[f32; 4]`).
    pub fn color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    /// Override the horizontal text alignment.
    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    /// Use a detail-sized style (`DETAIL_SIZE`, `DETAIL_LINE`).
    pub fn detail_style(mut self) -> Self {
        self.style.size = DETAIL_SIZE;
        self
    }

    /// Ensure the label has an id (generate anonymous if empty).
    fn ensure_id(mut self, ui: &mut Ui) -> Self {
        if self.id.as_str().is_empty() {
            self.id = ui.next_anon_id();
        }
        self
    }
}

impl Ui {
    /// Place a [`Label`] widget and return its [`Response`].
    ///
    /// Labels are non-interactive: `hovered`, `pressed`, and `clicked` are
    /// always `false`.  No `HitRegion` is pushed.  The visual rect uses the
    /// label's line height scaled by the theme's DPI factor.
    pub fn label(&mut self, label: &Label) -> Response {
        let d = label.clone().ensure_id(self);
        let scale = self.scale_factor();
        let line_height = d.style.size * scale;

        self.begin_widget();

        let rect = Rect::new(
            self.cursor_x,
            self.cursor_y,
            self.available_width,
            line_height,
        );

        // Advance cursor past this label.
        match self.direction {
            crate::ui::context::LayoutDirection::Vertical => {
                self.cursor_y += rect.height;
            }
            crate::ui::context::LayoutDirection::Horizontal => {
                self.cursor_x += rect.width;
            }
        }

        // Build a TextView with colour from the label's own color field.
        let text_view = TextView {
            id: d.id.clone(),
            text: d.text.clone(),
            rect,
            style: TextStyle {
                size: d.style.size,
                color: color_from_array(d.color),
                ..d.style
            },
            z: Z_CONTROL + 1,
            clip: None,
        };

        self.push_text(text_view);

        // Labels are non-interactive.
        self.register(d.id.clone(), rect, rect);

        Response {
            id: d.id,
            rect,
            hit_rect: rect,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Heading convenience
// ---------------------------------------------------------------------------

/// A heading variant of [`Label`].
///
/// Uses `SettingsHeader` text role, `Bold` weight, and heading-sized fonts.
/// Constructed via `Heading::new(text)`; placed via `ui.heading(&label)`.
#[derive(Clone, Debug)]
pub struct Heading {
    label: Label,
}

impl Heading {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            label: Label::new(text)
                .style(TextStyle::new(
                    TextRole::SettingsHeader,
                    HEADING_SIZE,
                    Color::rgba(0.95, 0.96, 0.98, 1.0),
                    TextWeight::Bold,
                    TextAlign::Start,
                ))
                .color([0.95, 0.96, 0.98, 1.0]),
        }
    }

    /// Assign a stable `UiId`.
    pub fn id(mut self, id: UiId) -> Self {
        self.label = self.label.id(id);
        self
    }

    /// Access the inner [`Label`] so it can be passed to `ui.label()` or
    /// `ui.heading()`.
    pub fn as_label(&self) -> &Label {
        &self.label
    }
}

impl Ui {
    /// Place a [`Heading`] using its inner [`Label`] definition.
    ///
    /// Equivalent to `ui.label(heading.as_label())` but communicates intent.
    pub fn heading(&mut self, heading: &Heading) -> Response {
        self.label(heading.as_label())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Theme;
    use crate::ui_model::text::TextAlign;

    fn new_ui() -> Ui {
        Ui::new(Theme::default(), 800.0, 600.0)
    }

    // --- Label ---

    #[test]
    fn label_pushes_text_view() {
        let mut ui = new_ui();
        let resp = ui.label(&Label::new("Hello"));
        assert_eq!(resp.rect.height, LABEL_SIZE); // scale_factor 1.0
        assert!(!resp.hovered);
        assert!(!resp.pressed);
        assert!(!resp.clicked);
    }

    #[test]
    fn label_height_scales_with_dpi() {
        let theme = Theme {
            scale_factor: 2.0,
            ..Default::default()
        };
        let mut ui = Ui::new(theme, 800.0, 600.0);
        let resp = ui.label(&Label::new("DPI"));
        assert_eq!(resp.rect.height, LABEL_SIZE * 2.0);
    }

    #[test]
    fn label_is_non_interactive() {
        let mut ui = new_ui();
        ui.set_pointer(Some(crate::ui_model::geometry::Point::new(0.0, 0.0)));
        ui.set_pointer_pressed(true);
        let resp = ui.label(&Label::new("text"));
        assert!(!resp.hovered);
        assert!(!resp.pressed);
        assert!(!resp.clicked);
    }

    #[test]
    fn label_with_explicit_id_registers() {
        let mut ui = new_ui();
        let id = UiId::named("my-label");
        let resp = ui.label(&Label::new("test").id(id.clone()));
        assert_eq!(resp.id, id);
        assert_eq!(ui.rect(&id), Some(resp.rect));
    }

    #[test]
    fn label_detail_style_uses_detail_size() {
        let l = Label::new("detail").detail_style();
        assert_eq!(l.style.size, DETAIL_SIZE);
    }

    #[test]
    fn label_color_method_overrides() {
        let l = Label::new("colored").color([1.0, 0.0, 0.0, 1.0]);
        assert_eq!(l.color, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn label_align_method_overrides() {
        let l = Label::new("centered").align(TextAlign::Center);
        assert_eq!(l.align, TextAlign::Center);
    }

    // --- Heading ---

    #[test]
    fn heading_uses_header_role_and_bold_weight() {
        let h = Heading::new("Settings");
        let l = h.as_label();
        assert_eq!(l.style.role, TextRole::SettingsHeader);
        assert_eq!(l.style.weight, TextWeight::Bold);
        assert_eq!(l.style.size, HEADING_SIZE);
    }

    #[test]
    fn heading_with_id() {
        let id = UiId::named("section-header");
        let h = Heading::new("Title").id(id.clone());
        assert_eq!(h.as_label().id, id);
    }

    #[test]
    fn ui_heading_delegates_to_label() {
        let theme = Theme::default();
        let mut ui = Ui::new(theme, 800.0, 600.0);
        let resp = ui.heading(&Heading::new("Hello"));
        assert_eq!(resp.rect.height, HEADING_SIZE);
    }

    // --- Anonymous id ---

    #[test]
    fn label_without_explicit_id_generates_anonymous() {
        let mut ui = new_ui();
        let resp = ui.label(&Label::new("anon"));
        assert!(!resp.id.as_str().is_empty());
        assert!(resp.id.as_str().starts_with("_ui_"));
    }
}
