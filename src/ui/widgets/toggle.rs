//! `Toggle` widget — a switch-style toggle (Ink-based).
//!
//! Matches the existing `toggle_instances` in `app/render/settings.rs`:
//! - Track: round-rect RowBackground, green when ON, white/dim when OFF.
//! - Thumb: Dot offset from center by +10*scale (ON) or -10*scale (OFF).
//!
//! Phase 5 will add Liquid Glass animation (glass track + spring thumb).

use crate::ui::context::Ui;
use crate::ui::interaction::InteractionPhase;
use crate::ui::response::Response;
use crate::ui_model::geometry::{Point, Rect};
use crate::ui_model::hit::HitTarget;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{Color, ControlKind, InkView};
use crate::ui_model::text::{TextAlign, TextRole, TextStyle, TextView, TextWeight};

use super::label::{DETAIL_LINE, LABEL_LINE};
use super::{color_from_array, control_size_multiplier, scale_color_alpha, Z_CONTROL};

// ---------------------------------------------------------------------------
// Palette constants (mirrors `settings_panel.rs`)
// ---------------------------------------------------------------------------

const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
const GREEN: [f32; 4] = [0.28, 0.82, 0.48, 0.78];
const TRACK_OFF: [f32; 4] = [1.0, 1.0, 1.0, 0.14];
const THUMB_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.78];

/// Logical track half-width (matches `toggle_instances`: 22.0).
const TRACK_HALF_W: f32 = 22.0;
/// Logical track half-height (matches `toggle_instances`: 11.0).
const TRACK_HALF_H: f32 = 11.0;
/// Thumb radius (matches `toggle_instances`: 6.0).
const THUMB_RADIUS: f32 = 6.0;
/// Thumb offset from track center (matches `toggle_instances`: 10.0).
const THUMB_OFFSET: f32 = 10.0;

/// Row height used when a label is present.
const ROW_H: f32 = 46.0;

// ---------------------------------------------------------------------------
// ToggleStyle
// ---------------------------------------------------------------------------

/// Visual style for a [`Toggle`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleStyle {
    /// Single Switch variant (current only choice).
    Switch,
}

// ---------------------------------------------------------------------------
// Toggle
// ---------------------------------------------------------------------------

/// A toggle switch widget (builder pattern).
///
/// Placed via `ui.toggle(&toggle_def) -> ToggleResponse`.
#[derive(Clone, Debug)]
pub struct Toggle {
    pub id: UiId,
    pub value: bool,
    pub label: Option<String>,
    pub detail: Option<String>,
    pub style: ToggleStyle,
    pub control_size: super::super::theme::ControlSize,
    pub tint: Option<[f32; 4]>,
    pub enabled: bool,
}

impl Toggle {
    /// Create a new toggle with the given initial value.
    pub fn new(value: bool) -> Self {
        Self {
            id: UiId::named(""),
            value,
            label: None,
            detail: None,
            style: ToggleStyle::Switch,
            control_size: super::super::theme::ControlSize::Regular,
            tint: None,
            enabled: true,
        }
    }

    /// Assign a stable `UiId`.
    pub fn id(mut self, id: UiId) -> Self {
        self.id = id;
        self
    }

    /// Set the label text (shown left of the switch).
    pub fn label(mut self, text: impl Into<String>) -> Self {
        self.label = Some(text.into());
        self
    }

    /// Set the detail/subtitle text.
    pub fn detail(mut self, text: impl Into<String>) -> Self {
        self.detail = Some(text.into());
        self
    }

    /// Set the visual style.
    pub fn style(mut self, style: ToggleStyle) -> Self {
        self.style = style;
        self
    }

    /// Set the control size (scales the switch dimensions).
    pub fn control_size(mut self, size: super::super::theme::ControlSize) -> Self {
        self.control_size = size;
        self
    }

    /// Override the tint colour (used for the ON track colour).
    pub fn tint(mut self, color: [f32; 4]) -> Self {
        self.tint = Some(color);
        self
    }

    /// Enable or disable the toggle.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    fn ensure_id(mut self, ui: &mut Ui) -> Self {
        if self.id.as_str().is_empty() {
            self.id = ui.next_anon_id();
        }
        self
    }
}

// ---------------------------------------------------------------------------
// ToggleResponse
// ---------------------------------------------------------------------------

/// Response returned by `ui.toggle()`.
#[derive(Clone, Debug)]
pub struct ToggleResponse {
    pub response: Response,
    /// The new toggle value after processing a click.
    pub value: bool,
    /// `true` when the value changed this frame.
    pub changed: bool,
}

impl Ui {
    /// Place a [`Toggle`] and return a [`ToggleResponse`].
    ///
    /// The toggle is rendered as a track + thumb pair on the right side of the
    /// row, with an optional label + detail on the left.
    ///
    /// Click handling: if `active_click_id` matches the toggle's id, the value
    /// is toggled and `changed` is set to `true`.
    pub fn toggle(&mut self, toggle: &Toggle) -> ToggleResponse {
        let t = toggle.clone().ensure_id(self);
        let scale = self.scale_factor() * control_size_multiplier(t.control_size);
        let has_label = t.label.is_some();

        // Compute height: if label/detail present, use row height; otherwise
        // just enough for the switch.
        let height = if has_label || t.detail.is_some() {
            ROW_H * scale
        } else {
            TRACK_HALF_H * 2.0 * scale + 4.0 * scale
        };

        self.begin_widget();

        let rect = Rect::new(self.cursor_x, self.cursor_y, self.available_width, height);

        // --- Row background (only when label/detail present, for parity with
        //     the existing settings panel look) ---
        if has_label || t.detail.is_some() {
            let center = rect.center();
            let half_h = height * 0.5;
            let half_w = rect.width * 0.5;
            let corner = 12.0 * scale;
            let bg = InkView {
                id: t.id.clone(),
                center,
                extent: half_h,
                opacity: 0.12,
                scene_blur: 0.0,
                stroke: half_w,
                corner_radius: corner,
                color: Color::rgba(1.0, 1.0, 1.0, 0.12),
                kind: ControlKind::RowBackground,
                z: Z_CONTROL,
                clip: None,
            };
            self.push_ink(bg);
        }

        // --- Label (left side) ---
        let label_x = rect.x + 16.0 * scale;
        if let Some(ref text) = t.label {
            let mut label_y = rect.center().y;
            if t.detail.is_some() {
                label_y -= 8.0 * scale;
            }
            let line_h = LABEL_LINE * scale;
            let text_view = TextView {
                id: t.id.clone(),
                text: text.clone(),
                rect: Rect::new(label_x, label_y - line_h * 0.5, 0.0, line_h),
                style: TextStyle::new(
                    TextRole::SettingsRow,
                    14.0,
                    color_from_array(if t.enabled {
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

        // --- Detail (below label) ---
        if let Some(ref text) = t.detail {
            let detail_y = rect.center().y + 8.0 * scale;
            let line_h = DETAIL_LINE * scale;
            let text_view = TextView {
                id: t.id.clone(),
                text: text.clone(),
                rect: Rect::new(label_x, detail_y - line_h * 0.5, 0.0, line_h),
                style: TextStyle::new(
                    TextRole::SettingsDetail,
                    12.0,
                    color_from_array(if t.enabled {
                        [1.0, 1.0, 1.0, 0.58]
                    } else {
                        [1.0, 1.0, 1.0, 0.23]
                    }),
                    TextWeight::Regular,
                    TextAlign::Start,
                ),
                z: Z_CONTROL + 1,
                clip: None,
            };
            self.push_text(text_view);
        }

        // --- Toggle switch (right side) ---
        let track_hw = TRACK_HALF_W * scale;
        let track_hh = TRACK_HALF_H * scale;
        let thumb_r = THUMB_RADIUS * scale;
        let thumb_offset = THUMB_OFFSET * scale;

        let track_cx = rect.max_x() - track_hw - 8.0 * scale;
        let track_cy = rect.center().y;

        // Track colour: green when ON, dim-white when OFF.
        let track_color = if t.value {
            t.tint.unwrap_or(GREEN)
        } else {
            TRACK_OFF
        };
        let track_opacity = if t.enabled {
            track_color[3]
        } else {
            track_color[3] * 0.4
        };

        let track_ink = InkView {
            id: t.id.clone(),
            center: Point::new(track_cx, track_cy),
            extent: track_hh,
            opacity: track_opacity,
            scene_blur: 0.0,
            stroke: track_hw,
            corner_radius: track_hh,
            color: color_from_array([
                track_color[0],
                track_color[1],
                track_color[2],
                track_opacity,
            ]),
            kind: ControlKind::RowBackground,
            z: Z_CONTROL,
            clip: None,
        };
        self.push_ink(track_ink);

        // Thumb: ON = right offset, OFF = left offset (matches doc requirement).
        let thumb_x = track_cx + if t.value { thumb_offset } else { -thumb_offset };
        let thumb_opacity = if t.enabled {
            THUMB_COLOR[3]
        } else {
            THUMB_COLOR[3] * 0.4
        };

        let thumb_ink = InkView {
            id: t.id.clone(),
            center: Point::new(thumb_x, track_cy),
            extent: thumb_r,
            opacity: thumb_opacity,
            scene_blur: 0.0,
            stroke: 0.0,
            corner_radius: 0.0,
            color: color_from_array([
                THUMB_COLOR[0],
                THUMB_COLOR[1],
                THUMB_COLOR[2],
                thumb_opacity,
            ]),
            kind: ControlKind::Dot,
            z: Z_CONTROL + 1,
            clip: None,
        };
        self.push_ink(thumb_ink);

        // --- Hit region (full row) ---
        if t.enabled {
            self.push_hit(crate::layout::hit_map::HitRegion::new(
                t.id.clone(),
                rect,
                HitTarget::settings_toggle(t.id.as_str()),
                Z_CONTROL + 2,
            ));
        }

        // --- Interaction ---
        let hovered = self
            .pointer_pos()
            .map(|p| rect.contains(p))
            .unwrap_or(false)
            && t.enabled;
        let pressed = hovered && self.pointer_pressed();
        let clicked = self.is_active_click(&t.id);

        let new_value = if clicked && t.enabled {
            !t.value
        } else {
            t.value
        };
        let changed = clicked && t.enabled;

        {
            let state = self.element_state_mut(&t.id);
            state.hovered = hovered;
            state.pressed = pressed;
            state.hover_amount = if hovered { 1.0 } else { 0.0 };
            state.press_amount = if pressed { 1.0 } else { 0.0 };
            state.phase = if !t.enabled {
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

        self.register(t.id.clone(), rect, rect);

        let response = Response {
            id: t.id,
            rect,
            hit_rect: rect,
            hovered,
            pressed,
            clicked,
            focused: false,
            changed,
        };

        ToggleResponse {
            response,
            value: new_value,
            changed,
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

    #[test]
    fn toggle_off_shows_thumb_at_left() {
        let mut ui = new_ui();
        ui.toggle(&Toggle::new(false));
        let (render, _hits, _reg) = ui.take();
        // Should have track (RowBackground) and thumb (Dot).
        let all: Vec<_> = render.ink.iter().flat_map(|b| &b.views).collect();
        let thumb = all.iter().find(|v| v.kind == ControlKind::Dot).unwrap();
        let track = all
            .iter()
            .find(|v| v.kind == ControlKind::RowBackground)
            .unwrap();
        // Thumb X should be track_center_x - THUMB_OFFSET.
        assert!(thumb.center.x < track.center.x);
    }

    #[test]
    fn toggle_on_shows_thumb_at_right() {
        let mut ui = new_ui();
        ui.toggle(&Toggle::new(true));
        let (render, _hits, _reg) = ui.take();
        let all: Vec<_> = render.ink.iter().flat_map(|b| &b.views).collect();
        let thumb = all.iter().find(|v| v.kind == ControlKind::Dot).unwrap();
        let track = all
            .iter()
            .find(|v| v.kind == ControlKind::RowBackground)
            .unwrap();
        // Thumb X should be track_center_x + THUMB_OFFSET.
        assert!(thumb.center.x > track.center.x);
    }

    #[test]
    fn toggle_on_has_green_track() {
        let mut ui = new_ui();
        ui.toggle(&Toggle::new(true));
        let (render, _hits, _reg) = ui.take();
        let all: Vec<_> = render.ink.iter().flat_map(|b| &b.views).collect();
        let track = all
            .iter()
            .find(|v| v.kind == ControlKind::RowBackground)
            .unwrap();
        let expected = color_from_array(GREEN);
        let tolerance: f32 = 0.01;
        assert!((track.color.r - expected.r).abs() < tolerance);
        assert!((track.color.g - expected.g).abs() < tolerance);
        assert!((track.color.b - expected.b).abs() < tolerance);
    }

    #[test]
    fn toggle_off_has_white_track() {
        let mut ui = new_ui();
        ui.toggle(&Toggle::new(false));
        let (render, _hits, _reg) = ui.take();
        let all: Vec<_> = render.ink.iter().flat_map(|b| &b.views).collect();
        let track = all
            .iter()
            .find(|v| v.kind == ControlKind::RowBackground)
            .unwrap();
        let expected = color_from_array(TRACK_OFF);
        assert!((track.color.a - expected.a).abs() < 0.01);
    }

    #[test]
    fn toggle_clicked_toggles_value() {
        let mut ui = new_ui();
        let id = UiId::named("sw");
        ui.set_active_click(Some(id.clone()));
        let resp = ui.toggle(&Toggle::new(false).id(id));
        assert!(resp.changed);
        assert!(resp.value);
    }

    #[test]
    fn toggle_disabled_does_not_toggle() {
        let mut ui = new_ui();
        let id = UiId::named("sw");
        ui.set_active_click(Some(id.clone()));
        let resp = ui.toggle(&Toggle::new(false).enabled(false).id(id));
        assert!(!resp.changed);
        assert!(!resp.value); // stays false
    }

    #[test]
    fn toggle_rect_equals_hit_rect() {
        let mut ui = new_ui();
        let resp = ui.toggle(&Toggle::new(true));
        assert_eq!(resp.response.rect, resp.response.hit_rect);
    }

    #[test]
    fn toggle_scales_with_dpi() {
        let theme = Theme {
            scale_factor: 2.0,
            ..Default::default()
        };
        let mut ui = Ui::new(theme, 800.0, 600.0);
        let resp = ui.toggle(&Toggle::new(false));
        // Height should be roughly 2x the base switch height.
        assert!(resp.response.rect.height > 40.0);
    }

    #[test]
    fn toggle_with_label_has_row_background() {
        let mut ui = new_ui();
        ui.toggle(&Toggle::new(true).label("Enable"));
        let (render, _hits, _reg) = ui.take();
        let row_bgs: Vec<_> = render
            .ink
            .iter()
            .flat_map(|b| &b.views)
            .filter(|v| v.kind == ControlKind::RowBackground)
            .collect();
        // Two row backgrounds: the general bg + the track.
        assert!(row_bgs.len() >= 2);
    }

    #[test]
    fn toggle_hovered_when_pointer_inside() {
        let mut ui = new_ui();
        ui.set_pointer(Some(Point::new(400.0, 23.0)));
        let resp = ui.toggle(&Toggle::new(true));
        assert!(resp.response.hovered);
    }

    #[test]
    fn toggle_registers_in_registry() {
        let mut ui = new_ui();
        let id = UiId::named("reg-tog");
        ui.toggle(&Toggle::new(false).id(id.clone()));
        let (_, _, reg) = ui.take();
        assert!(reg.rect(&id).is_some());
    }

    #[test]
    fn toggle_control_size_mini_reduces_dimensions() {
        let mut ui = new_ui();
        let resp = ui.toggle(
            &Toggle::new(false).control_size(super::super::super::theme::ControlSize::Mini),
        );
        let mut ui2 = new_ui();
        let resp_reg = ui2.toggle(
            &Toggle::new(false).control_size(super::super::super::theme::ControlSize::Regular),
        );
        assert!(resp.response.rect.height < resp_reg.response.rect.height);
    }
}
