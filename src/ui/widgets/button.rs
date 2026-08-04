//! `Button` and `IconButton` widgets.
//!
//! `Button` draws a rounded-rect row background plus a label and an optional
//! chevron on the right.  `IconButton` draws a single control icon (close,
//! gear, reset, etc.).

use crate::ui::context::Ui;
use crate::ui::interaction::InteractionPhase;
use crate::ui::response::Response;
use crate::ui_model::geometry::{Point, Rect};
use crate::ui_model::hit::HitTarget;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{ControlKind, InkView};
use crate::ui_model::text::{TextAlign, TextRole, TextStyle, TextView, TextWeight};

use super::label::LABEL_LINE;
use super::{color_from_array, control_size_multiplier, scale_color_alpha, Z_CONTROL};

// ---------------------------------------------------------------------------
// Colour palette (mirrors settings_panel.rs)
// ---------------------------------------------------------------------------

const INK: [f32; 4] = [0.11, 0.11, 0.118, 0.92];
const MUTED: [f32; 4] = [0.11, 0.11, 0.118, 0.58];
const ACCENT: [f32; 4] = [0.039, 0.518, 1.0, 0.20];
const ROW_BG: [f32; 4] = [0.11, 0.11, 0.118, 0.06];

/// Default row height in logical pixels (matches `ROW_H`).
pub const ROW_H: f32 = 46.0;

// ---------------------------------------------------------------------------
// ButtonStyle
// ---------------------------------------------------------------------------

/// Visual style for a [`Button`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Plain row background with label.
    Plain,
    /// Accent-coloured background (prominent action).
    Prominent,
}

// ---------------------------------------------------------------------------
// Button
// ---------------------------------------------------------------------------

/// A row-shaped button with a label, optional detail text, and an optional
/// chevron indicator on the right.
///
/// Placed via `ui.button(&button_def) -> Response`.
#[derive(Clone, Debug)]
pub struct Button {
    pub id: UiId,
    pub label: Option<String>,
    pub detail: Option<String>,
    pub style: ButtonStyle,
    pub control_size: super::super::theme::ControlSize,
    pub tint: Option<[f32; 4]>,
    pub enabled: bool,
    /// Show a chevron (>) on the right side.
    pub chevron: bool,
    /// Explicit hit-target for the HitRegion. When `Some`, replaces the
    /// id-derived default (`HitTarget::settings_action(b.id.as_str())`).
    pub hit_target: Option<HitTarget>,
}

impl Button {
    /// Create a new button with the given label text.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            id: UiId::named(""),
            label: Some(label.into()),
            detail: None,
            style: ButtonStyle::Plain,
            control_size: super::super::theme::ControlSize::Regular,
            tint: None,
            enabled: true,
            chevron: true,
            hit_target: None,
        }
    }

    /// Assign a stable `UiId`.
    pub fn id(mut self, id: UiId) -> Self {
        self.id = id;
        self
    }

    /// Override or clear the label text.
    pub fn label_opt(mut self, label: Option<String>) -> Self {
        self.label = label;
        self
    }

    /// Set the detail (subtitle) text.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Set the visual style.
    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    /// Set the control size (affects row height).
    pub fn control_size(mut self, size: super::super::theme::ControlSize) -> Self {
        self.control_size = size;
        self
    }

    /// Override the tint colour (RGBA `[f32; 4]`).
    pub fn tint(mut self, color: [f32; 4]) -> Self {
        self.tint = Some(color);
        self
    }

    /// Enable or disable the button.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Show or hide the chevron indicator.
    pub fn chevron_opt(mut self, show: bool) -> Self {
        self.chevron = show;
        self
    }

    /// Override the hit-test target for the HitRegion.
    pub fn hit_target(mut self, target: HitTarget) -> Self {
        self.hit_target = Some(target);
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
    /// Place a [`Button`] widget and return its [`Response`].
    ///
    /// Interactive: hover and press state are tracked, and `clicked` is set
    /// when `active_click_id` matches the button's id.
    pub fn button(&mut self, button: &Button) -> Response {
        let b = button.clone().ensure_id(self);
        let scale = self.scale_factor() * control_size_multiplier(b.control_size);
        let height = ROW_H * scale;

        self.begin_widget();

        let rect = Rect::new(self.cursor_x, self.cursor_y, self.available_width, height);

        // --- Row background ---
        let bg_color = match b.style {
            ButtonStyle::Plain => ROW_BG,
            ButtonStyle::Prominent => ACCENT,
        };
        let tint = b.tint.unwrap_or(bg_color);
        let opacity = if b.enabled { tint[3] } else { tint[3] * 0.4 };

        let center = rect.center();
        let half_w = rect.width * 0.5;
        let half_h = height * 0.5;
        let corner = 12.0 * scale;

        let ink = InkView {
            id: b.id.clone(),
            center,
            extent: half_h,
            opacity,
            scene_blur: 0.0,
            stroke: half_w,
            corner_radius: corner,
            color: color_from_array([tint[0], tint[1], tint[2], opacity]),
            kind: ControlKind::RowBackground,
            z: Z_CONTROL,
            clip: None,
        };
        self.push_ink(ink);

        // --- Label text ---
        let label_line = LABEL_LINE * scale;
        let label_x = rect.x + 16.0 * scale;
        if let Some(ref label_text) = b.label {
            let mut label_y = center.y;
            // If detail is present, shift label up slightly.
            if b.detail.is_some() {
                label_y -= 8.0 * scale;
            }
            let text_view = TextView {
                id: b.id.clone(),
                text: label_text.clone(),
                rect: Rect::new(label_x, label_y - label_line * 0.5, 0.0, label_line),
                style: TextStyle::new(
                    TextRole::SettingsRow,
                    14.0,
                    color_from_array(if b.enabled {
                        INK
                    } else {
                        scale_color_alpha(INK, 0.4)
                    }),
                    TextWeight::Regular,
                    TextAlign::Start,
                ),
                z: Z_CONTROL + 1,
                clip: None,
            };
            self.push_text(text_view);
        }

        // --- Detail text ---
        if let Some(ref detail_text) = b.detail {
            let detail_line = super::label::DETAIL_LINE * scale;
            let detail_y = center.y + 8.0 * scale;
            let text_view = TextView {
                id: b.id.clone(),
                text: detail_text.clone(),
                rect: Rect::new(label_x, detail_y - detail_line * 0.5, 0.0, detail_line),
                style: TextStyle::new(
                    TextRole::SettingsDetail,
                    12.0,
                    color_from_array(if b.enabled {
                        MUTED
                    } else {
                        scale_color_alpha(MUTED, 0.4)
                    }),
                    TextWeight::Regular,
                    TextAlign::Start,
                ),
                z: Z_CONTROL + 1,
                clip: None,
            };
            self.push_text(text_view);
        }

        // --- Chevron ---
        if b.chevron {
            let chev_r = 9.0 * scale;
            let chev_x = rect.max_x() - 14.0 * scale;
            let chev_y = center.y;
            let chev_ink = InkView {
                id: b.id.clone(),
                center: Point::new(chev_x, chev_y),
                extent: chev_r,
                opacity: if b.enabled { MUTED[3] } else { MUTED[3] * 0.4 },
                scene_blur: 0.0,
                stroke: 1.6 * scale,
                corner_radius: 0.0,
                color: color_from_array(if b.enabled {
                    MUTED
                } else {
                    scale_color_alpha(MUTED, 0.4)
                }),
                kind: ControlKind::Chevron,
                z: Z_CONTROL + 1,
                clip: None,
            };
            self.push_ink(chev_ink);
        }

        // --- Hit region ---
        if b.enabled {
            let target = b
                .hit_target
                .clone()
                .unwrap_or_else(|| HitTarget::settings_action(b.id.as_str()));
            self.push_hit(crate::layout::hit_map::HitRegion::new(
                b.id.clone(),
                rect,
                target,
                Z_CONTROL + 2,
            ));
        }

        // --- Interaction ---
        let hovered = self
            .pointer_pos()
            .map(|p| rect.contains(p))
            .unwrap_or(false)
            && b.enabled;
        let pressed = hovered && self.pointer_pressed();
        let clicked = self.is_active_click(&b.id);

        {
            let state = self.element_state_mut(&b.id);
            state.hovered = hovered;
            state.pressed = pressed;
            state.hover_amount = if hovered { 1.0 } else { 0.0 };
            state.press_amount = if pressed { 1.0 } else { 0.0 };
            state.phase = if !b.enabled {
                InteractionPhase::Disabled
            } else if pressed {
                InteractionPhase::Pressed
            } else if hovered {
                InteractionPhase::Hovered
            } else {
                InteractionPhase::Idle
            };
        }

        // --- Advance cursor ---
        match self.direction {
            crate::ui::context::LayoutDirection::Vertical => {
                self.cursor_y += height;
            }
            crate::ui::context::LayoutDirection::Horizontal => {
                self.cursor_x += rect.width;
            }
        }

        self.register(b.id.clone(), rect, rect);

        Response {
            id: b.id,
            rect,
            hit_rect: rect,
            hovered,
            pressed,
            clicked,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// IconButton
// ---------------------------------------------------------------------------

/// A compact icon-only button (close, gear, reset icon, etc.).
///
/// Placed via `ui.icon_button(&icon_def) -> Response`.
#[derive(Clone, Debug)]
pub struct IconButton {
    pub id: UiId,
    pub kind: ControlKind,
    pub visual_radius: f32,
    pub hit_radius: f32,
    pub tint: Option<[f32; 4]>,
    pub label: Option<String>,
    /// Explicit hit-target for the HitRegion. When `Some`, replaces the
    /// id-derived default (`HitTarget::settings_action(ib.id.as_str())`).
    pub hit_target: Option<HitTarget>,
}

impl IconButton {
    /// Create a new icon button for the given control kind.
    ///
    /// Default visual radius is 10 px, hit radius is 16 px (following the
    /// CLOSE_HALF/CLOSE_HIT_HALF pattern from the settings panel).
    pub fn new(kind: ControlKind) -> Self {
        Self {
            id: UiId::named(""),
            kind,
            visual_radius: 10.0,
            hit_radius: 16.0,
            tint: None,
            label: None,
            hit_target: None,
        }
    }

    /// Assign a stable `UiId`.
    pub fn id(mut self, id: UiId) -> Self {
        self.id = id;
        self
    }

    /// Override the visual radius in logical pixels.
    pub fn visual_radius(mut self, r: f32) -> Self {
        self.visual_radius = r;
        self
    }

    /// Override the hit-test radius in logical pixels.
    pub fn hit_radius(mut self, r: f32) -> Self {
        self.hit_radius = r;
        self
    }

    /// Override the tint colour.
    pub fn tint(mut self, color: [f32; 4]) -> Self {
        self.tint = Some(color);
        self
    }

    /// Set a tooltip / accessibility label (not currently rendered as text).
    pub fn label_opt(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Override the hit-test target for the HitRegion.
    pub fn hit_target(mut self, target: HitTarget) -> Self {
        self.hit_target = Some(target);
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
    /// Place an [`IconButton`] widget and return its [`Response`].
    ///
    /// The visual rect is a square of side `2 * visual_radius * scale_factor`;
    /// the hit rect is a square of side `2 * hit_radius * scale_factor`.
    /// Both are centered at the current cursor position.
    pub fn icon_button(&mut self, icon: &IconButton) -> Response {
        let ib = icon.clone().ensure_id(self);
        let scale = self.scale_factor();
        let visual_r = ib.visual_radius * scale;
        let hit_r = ib.hit_radius * scale;

        self.begin_widget();

        let center = Point::new(self.cursor_x + hit_r, self.cursor_y + hit_r);
        let rect = Rect::new(
            center.x - visual_r,
            center.y - visual_r,
            visual_r * 2.0,
            visual_r * 2.0,
        );
        let hit_rect = Rect::new(center.x - hit_r, center.y - hit_r, hit_r * 2.0, hit_r * 2.0);

        // --- Icon ink ---
        let tint = ib.tint.unwrap_or(INK);
        let ink = InkView {
            id: ib.id.clone(),
            center,
            extent: visual_r,
            opacity: tint[3],
            scene_blur: 0.0,
            stroke: 1.6 * scale,
            corner_radius: 0.0,
            color: color_from_array(tint),
            kind: ib.kind.clone(),
            z: Z_CONTROL,
            clip: None,
        };
        self.push_ink(ink);

        // --- Hit region ---
        let hit_target = ib
            .hit_target
            .clone()
            .unwrap_or_else(|| HitTarget::settings_action(ib.id.as_str()));
        self.push_hit(crate::layout::hit_map::HitRegion::new(
            ib.id.clone(),
            hit_rect,
            hit_target,
            Z_CONTROL + 2,
        ));

        // --- Interaction ---
        let hovered = self
            .pointer_pos()
            .map(|p| hit_rect.contains(p))
            .unwrap_or(false);
        let pressed = hovered && self.pointer_pressed();
        let clicked = self.is_active_click(&ib.id);

        {
            let state = self.element_state_mut(&ib.id);
            state.hovered = hovered;
            state.pressed = pressed;
            state.hover_amount = if hovered { 1.0 } else { 0.0 };
            state.press_amount = if pressed { 1.0 } else { 0.0 };
            state.phase = if pressed {
                InteractionPhase::Pressed
            } else if hovered {
                InteractionPhase::Hovered
            } else {
                InteractionPhase::Idle
            };
        }

        // --- Advance cursor ---
        let consumed = hit_rect.height;
        match self.direction {
            crate::ui::context::LayoutDirection::Vertical => {
                self.cursor_y += consumed;
            }
            crate::ui::context::LayoutDirection::Horizontal => {
                self.cursor_x += hit_r * 2.0;
            }
        }

        self.register(ib.id.clone(), rect, hit_rect);

        Response {
            id: ib.id,
            rect,
            hit_rect,
            hovered,
            pressed,
            clicked,
            ..Default::default()
        }
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

    // --- Button ---

    #[test]
    fn button_has_row_height() {
        let mut ui = new_ui();
        let resp = ui.button(&Button::new("Click"));
        assert_eq!(resp.rect.height, ROW_H);
        assert!(resp.rect.width > 0.0);
    }

    #[test]
    fn button_without_pointer_is_not_hovered() {
        let mut ui = new_ui();
        let resp = ui.button(&Button::new("Btn"));
        assert!(!resp.hovered);
        assert!(!resp.pressed);
    }

    #[test]
    fn button_hovered_when_pointer_inside() {
        let mut ui = new_ui();
        ui.set_pointer(Some(Point::new(100.0, 23.0)));
        let resp = ui.button(&Button::new("Btn"));
        assert!(resp.hovered);
    }

    #[test]
    fn button_not_hovered_when_pointer_outside() {
        let mut ui = new_ui();
        ui.set_pointer(Some(Point::new(900.0, 900.0)));
        let resp = ui.button(&Button::new("Btn"));
        assert!(!resp.hovered);
    }

    #[test]
    fn button_clicked_via_active_click() {
        let mut ui = new_ui();
        let id = UiId::named("my-btn");
        ui.set_active_click(Some(id.clone()));
        let resp = ui.button(&Button::new("Btn").id(id));
        assert!(resp.clicked);
    }

    #[test]
    fn button_not_clicked_without_active_click() {
        let mut ui = new_ui();
        let id = UiId::named("my-btn");
        let resp = ui.button(&Button::new("Btn").id(id));
        assert!(!resp.clicked);
    }

    #[test]
    fn button_disabled_is_not_hovered_interactive() {
        let mut ui = new_ui();
        ui.set_pointer(Some(Point::new(100.0, 23.0)));
        ui.set_pointer_pressed(true);
        let resp = ui.button(&Button::new("Btn").enabled(false));
        assert!(!resp.hovered);
        assert!(!resp.pressed);
    }

    #[test]
    fn button_prominent_style_sets_accent() {
        let mut ui = new_ui();
        ui.button(&Button::new("Important").style(ButtonStyle::Prominent));
        let (render, _hits, _reg) = ui.take();
        let ink = &render.ink[0].views[0];
        // Accent colour with accent alpha.
        let expected = color_from_array(ACCENT);
        assert!((ink.color.a - expected.a).abs() < 0.001);
    }

    #[test]
    fn button_rect_equals_hit_rect() {
        let mut ui = new_ui();
        let resp = ui.button(&Button::new("Eq"));
        assert_eq!(resp.rect, resp.hit_rect);
    }

    #[test]
    fn button_registers_in_registry() {
        let mut ui = new_ui();
        let id = UiId::named("reg-btn");
        ui.button(&Button::new("Reg").id(id.clone()));
        let (_, _, reg) = ui.take();
        assert!(reg.rect(&id).is_some());
    }

    // --- IconButton ---

    #[test]
    fn icon_button_has_separate_visual_and_hit_rects() {
        let mut ui = new_ui();
        let resp = ui.icon_button(&IconButton::new(ControlKind::CloseButton));
        // Visual rect should be smaller than hit rect.
        assert!(resp.hit_rect.width > resp.rect.width);
    }

    #[test]
    fn icon_button_hovered_when_in_hit_rect() {
        let mut ui = new_ui();
        let resp = ui.icon_button(&IconButton::new(ControlKind::SettingsGear));
        // Pointer at center of hit rect.
        let cx = resp.hit_rect.center();
        let mut ui2 = new_ui();
        ui2.set_pointer(Some(cx));
        let resp2 = ui2.icon_button(&IconButton::new(ControlKind::SettingsGear));
        assert!(resp2.hovered);
    }

    #[test]
    fn icon_button_tint_overrides_color() {
        let b = IconButton::new(ControlKind::CloseButton).tint([1.0, 0.0, 0.0, 0.5]);
        assert_eq!(b.tint, Some([1.0, 0.0, 0.0, 0.5]));
    }

    #[test]
    fn icon_button_scales_with_dpi() {
        let theme = Theme {
            scale_factor: 2.0,
            ..Default::default()
        };
        let mut ui = Ui::new(theme, 800.0, 600.0);
        let resp = ui.icon_button(&IconButton::new(ControlKind::CloseButton));
        // hit_radius=16, scale=2 => hit_r=32, so rect side = 64
        assert_eq!(resp.hit_rect.width, 64.0);
    }
}
