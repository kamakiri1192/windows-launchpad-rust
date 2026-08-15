//! `Slider` widget — a horizontal slider with track, knob, and optional
//! reset icon (Ink-based).
//!
//! Matches the existing slider rendering in `app/render/settings.rs`
//! (`SliderTrack`, `SliderKnob`, `ResetIcon`) and the geometry helpers in
//! `layout/settings_panel.rs` (`debug_slider_geometry`,
//! `debug_slider_value_from_pointer`).

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
// Palette constants
// ---------------------------------------------------------------------------

const INK: [f32; 4] = [0.11, 0.11, 0.118, 0.92];
const MUTED: [f32; 4] = [0.11, 0.11, 0.118, 0.58];
const TRACK_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.16];
const KNOB_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
const RESET_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 0.7];

// ---- Dimensions (logical px, mirrors `debug_slider_geometry`) ----

/// Track width in logical pixels.
const TRACK_WIDTH: f32 = 120.0;
/// Track half-height.
const TRACK_HALF_H: f32 = 2.5;
/// Knob radius.
const KNOB_RADIUS: f32 = 7.5;
/// Reset icon radius.
const RESET_RADIUS: f32 = 9.0;
/// Reset icon stroke width multiplier.
const RESET_STROKE: f32 = 1.4;
/// Gap between track right edge and reset icon.
const RESET_GAP: f32 = 10.0;

/// Row height used when a label is present.
const ROW_H: f32 = 46.0;

// ---------------------------------------------------------------------------
// Slider
// ---------------------------------------------------------------------------

/// A horizontal slider widget (builder pattern).
///
/// Placed via `ui.slider(&slider_def) -> SliderResponse`.
#[derive(Clone, Debug)]
pub struct Slider {
    pub id: UiId,
    pub value: f32,
    pub range: (f32, f32),
    pub label: Option<String>,
    pub detail: Option<String>,
    pub control_size: super::super::theme::ControlSize,
    pub enabled: bool,
    pub reset: bool,
    /// Explicit hit-target for the track HitRegion.
    pub hit_target: Option<HitTarget>,
    /// Explicit hit-target for the reset HitRegion.
    pub hit_target_reset: Option<HitTarget>,
}

impl Slider {
    /// Create a new slider with the given initial value and `(min, max)` range.
    pub fn new(value: f32, range: (f32, f32)) -> Self {
        let (min, max) = range;
        Self {
            id: UiId::named(""),
            value: value.clamp(min, max),
            range: (min, max),
            label: None,
            detail: None,
            control_size: super::super::theme::ControlSize::Regular,
            enabled: true,
            reset: true,
            hit_target: None,
            hit_target_reset: None,
        }
    }

    /// Assign a stable `UiId`.
    pub fn id(mut self, id: UiId) -> Self {
        self.id = id;
        self
    }

    /// Set the label text.
    pub fn label(mut self, text: impl Into<String>) -> Self {
        self.label = Some(text.into());
        self
    }

    /// Set the detail/subtitle text.
    pub fn detail(mut self, text: impl Into<String>) -> Self {
        self.detail = Some(text.into());
        self
    }

    /// Set the control size.
    pub fn control_size(mut self, size: super::super::theme::ControlSize) -> Self {
        self.control_size = size;
        self
    }

    /// Enable or disable the slider.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Show or hide the reset icon.
    pub fn reset_opt(mut self, reset: bool) -> Self {
        self.reset = reset;
        self
    }

    /// Override the hit-test target for the track HitRegion.
    pub fn hit_target(mut self, target: HitTarget) -> Self {
        self.hit_target = Some(target);
        self
    }

    /// Override the hit-test target for the reset HitRegion.
    pub fn hit_target_reset(mut self, target: HitTarget) -> Self {
        self.hit_target_reset = Some(target);
        self
    }

    fn ensure_id(mut self, ui: &mut Ui) -> Self {
        if self.id.as_str().is_empty() {
            self.id = ui.next_anon_id();
        }
        self
    }

    /// Compute the slider's `t` from the current value: `(value - min) / (max - min)`.
    fn normalized_value(&self) -> f32 {
        let (min, max) = self.range;
        if (max - min).abs() < f32::EPSILON {
            return 0.0;
        }
        ((self.value - min) / (max - min)).clamp(0.0, 1.0)
    }

    // ------------------------------------------------------------------
    // Static geometry helpers (public so tests can call them)
    // ------------------------------------------------------------------

    /// Convert a normalised `t` (0..1) to a knob X coordinate relative to
    /// the track left edge.
    ///
    /// The returned value includes the track's leftmost position so callers
    /// can use it directly as a screen-space X.
    pub fn value_to_x(track_left: f32, track_width: f32, t: f32) -> f32 {
        track_left + track_width * t
    }

    /// Convert a pointer X coordinate (screen-space) to a slider value
    /// within `range`.
    ///
    /// Returns the value clamped to `[min, max]`.
    pub fn value_from_pointer(
        pointer_x: f32,
        track_left: f32,
        track_width: f32,
        range: (f32, f32),
    ) -> f32 {
        let (min, max) = range;
        if track_width <= 0.0 {
            return min;
        }
        let t = ((pointer_x - track_left) / track_width).clamp(0.0, 1.0);
        min + (max - min) * t
    }

    /// Compute the slider's geometry for a given available width and scale.
    ///
    /// Returns `(track_left, track_width, knob_r, reset_cx, reset_r, track_hh)`.
    pub fn geometry(content_right: f32, scale: f32) -> (f32, f32, f32, f32, f32, f32) {
        let reset_r = RESET_RADIUS * scale;
        let gap = RESET_GAP * scale;
        let track_right = content_right - reset_r * 2.0 - gap;
        let track_width = TRACK_WIDTH * scale;
        let track_left = track_right - track_width;
        let knob_r = KNOB_RADIUS * scale;
        let track_hh = TRACK_HALF_H * scale;
        (
            track_left,
            track_width,
            knob_r,
            content_right - reset_r,
            reset_r,
            track_hh,
        )
    }
}

// ---------------------------------------------------------------------------
// SliderResponse
// ---------------------------------------------------------------------------

/// Response returned by `ui.slider()`.
#[derive(Clone, Debug)]
pub struct SliderResponse {
    pub response: Response,
    /// The slider value (may have changed if clicked).
    pub value: f32,
    /// `true` when the slider value changed this frame.
    pub changed: bool,
    /// `true` when the reset icon was clicked this frame.
    pub reset_clicked: bool,
}

impl Ui {
    /// Place a [`Slider`] and return a [`SliderResponse`].
    ///
    /// The slider renders a track bar, a knob positioned according to the
    /// current value, and optionally a reset icon.
    ///
    /// Click handling: the slider track region and reset icon region each have
    /// their own "click" logic — we check `active_click_id` against separate
    /// per-region ids derived from the slider's id.
    pub fn slider(&mut self, slider: &Slider) -> SliderResponse {
        let s = slider.clone().ensure_id(self);
        let scale = self.scale_factor() * control_size_multiplier(s.control_size);
        let has_label = s.label.is_some();

        let height = if has_label || s.detail.is_some() {
            ROW_H * scale
        } else {
            // Just enough to hold the slider widgets.
            KNOB_RADIUS * 2.0 * scale + 8.0 * scale
        };

        self.begin_widget();

        let rect = Rect::new(self.cursor_x, self.cursor_y, self.available_width, height);

        // --- Row background ---
        if has_label || s.detail.is_some() {
            let center = rect.center();
            let half_h = height * 0.5;
            let half_w = rect.width * 0.5;
            let corner = 12.0 * scale;
            let bg = InkView {
                id: s.id.clone(),
                center,
                extent: half_h,
                opacity: 0.12,
                scene_blur: 0.0,
                stroke: half_w,
                corner_radius: corner,
                color: Color::rgba(0.11, 0.11, 0.118, 0.06),
                kind: ControlKind::RowBackground,
                z: Z_CONTROL,
                clip: None,
            };
            self.push_ink(bg);
        }

        // --- Label / detail (left side) ---
        let label_x = rect.x + 16.0 * scale;
        if let Some(ref text) = s.label {
            let mut label_y = rect.center().y;
            if s.detail.is_some() {
                label_y -= 8.0 * scale;
            }
            let line_h = LABEL_LINE * scale;
            let text_view = TextView {
                id: s.id.clone(),
                text: text.clone(),
                rect: Rect::new(label_x, label_y - line_h * 0.5, 0.0, line_h),
                style: TextStyle::new(
                    TextRole::SettingsRow,
                    14.0,
                    color_from_array(if s.enabled {
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
        if let Some(ref text) = s.detail {
            let detail_y = rect.center().y + 8.0 * scale;
            let line_h = DETAIL_LINE * scale;
            let text_view = TextView {
                id: s.id.clone(),
                text: text.clone(),
                rect: Rect::new(label_x, detail_y - line_h * 0.5, 0.0, line_h),
                style: TextStyle::new(
                    TextRole::SettingsDetail,
                    12.0,
                    color_from_array(if s.enabled {
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

        // --- Slider geometry ---
        // `rect.max_x()` is the content-area right edge (set by the layout to
        // `content_right` = panel_right - CONTENT_PAD). Pass it straight into
        // Slider::geometry so the rendered track/reset/knob match the
        // hit_test geometry exactly (no extra inset).
        let content_right = rect.max_x();
        let (track_left, track_width, knob_r, reset_cx, reset_r, track_hh) =
            Slider::geometry(content_right, scale);

        let row_cy = rect.center().y;
        let t = s.normalized_value();
        let knob_x = Slider::value_to_x(track_left, track_width, t);

        // --- Track ink ---
        let track_ink = InkView {
            id: s.id.clone(),
            center: Point::new(track_left + track_width * 0.5, row_cy),
            extent: track_hh,
            opacity: if s.enabled {
                TRACK_COLOR[3]
            } else {
                TRACK_COLOR[3] * 0.4
            },
            scene_blur: 0.0,
            stroke: track_width * 0.5,
            corner_radius: track_hh,
            color: color_from_array(TRACK_COLOR),
            kind: ControlKind::SliderTrack,
            z: Z_CONTROL,
            clip: None,
        };
        self.push_ink(track_ink);

        // --- Knob ink ---
        let knob_ink = InkView {
            id: s.id.clone(),
            center: Point::new(knob_x, row_cy),
            extent: knob_r,
            opacity: if s.enabled {
                KNOB_COLOR[3]
            } else {
                KNOB_COLOR[3] * 0.4
            },
            scene_blur: 0.0,
            stroke: 0.0,
            corner_radius: 0.0,
            color: color_from_array(KNOB_COLOR),
            kind: ControlKind::SliderKnob,
            z: Z_CONTROL + 1,
            clip: None,
        };
        self.push_ink(knob_ink);

        // --- Reset icon ---
        if s.reset {
            let reset_ink = InkView {
                id: s.id.clone(),
                center: Point::new(reset_cx, row_cy),
                extent: reset_r,
                opacity: if s.enabled {
                    RESET_COLOR[3]
                } else {
                    RESET_COLOR[3] * 0.4
                },
                scene_blur: 0.0,
                stroke: RESET_STROKE * scale,
                corner_radius: 0.0,
                color: color_from_array(RESET_COLOR),
                kind: ControlKind::ResetIcon,
                z: Z_CONTROL + 1,
                clip: None,
            };
            self.push_ink(reset_ink);
        }

        // --- Hit regions ---
        // Track hit region (the full track area)
        let track_hit_rect = Rect::new(
            track_left - 4.0 * scale,
            row_cy - track_hh - 4.0 * scale,
            track_width + 8.0 * scale,
            track_hh * 2.0 + 8.0 * scale,
        );
        if s.enabled {
            let track_target = s
                .hit_target
                .clone()
                .unwrap_or_else(|| HitTarget::settings_toggle(format!("{}-track", s.id.as_str())));
            self.push_hit(crate::layout::hit_map::HitRegion::new(
                format_slider_id(&s.id, "track"),
                track_hit_rect,
                track_target,
                Z_CONTROL + 2,
            ));
        }

        // Reset hit region
        let reset_hit_rect = Rect::new(
            reset_cx - reset_r * 1.6,
            row_cy - reset_r * 1.6,
            reset_r * 3.2,
            reset_r * 3.2,
        );
        if s.reset && s.enabled {
            let reset_target = s
                .hit_target_reset
                .clone()
                .unwrap_or_else(|| HitTarget::settings_action(format!("{}-reset", s.id.as_str())));
            self.push_hit(crate::layout::hit_map::HitRegion::new(
                format_slider_id(&s.id, "reset"),
                reset_hit_rect,
                reset_target,
                Z_CONTROL + 2,
            ));
        }

        // --- Interaction ---
        let hovered = self
            .pointer_pos()
            .map(|p| track_hit_rect.contains(p) || reset_hit_rect.contains(p))
            .unwrap_or(false)
            && s.enabled;
        let pressed = hovered && self.pointer_pressed();
        let track_clicked = s.enabled && self.is_active_click(&format_slider_id(&s.id, "track"));
        let reset_clicked =
            s.reset && self.is_active_click(&format_slider_id(&s.id, "reset")) && s.enabled;

        // For Phase 2, "changed" only fires on click. We compute a new value
        // from the pointer position when the track is clicked.
        let (new_value, changed) = if track_clicked {
            let pointer_x = self.pointer_pos().map(|p| p.x).unwrap_or(knob_x);
            let v = Slider::value_from_pointer(pointer_x, track_left, track_width, s.range);
            (v, true)
        } else {
            (s.value, false)
        };

        {
            let state = self.element_state_mut(&s.id);
            state.hovered = hovered;
            state.pressed = pressed;
            state.hover_amount = if hovered { 1.0 } else { 0.0 };
            state.press_amount = if pressed { 1.0 } else { 0.0 };
            state.phase = if !s.enabled {
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

        self.register(s.id.clone(), rect, rect);

        let response = Response {
            id: s.id,
            rect,
            hit_rect: track_hit_rect, // use track hit as primary
            hovered,
            pressed,
            clicked: track_clicked,
            focused: false,
            changed,
        };

        SliderResponse {
            response,
            value: new_value,
            changed,
            reset_clicked,
        }
    }
}

/// Create a sub-id for slider hit regions (track and reset).
fn format_slider_id(parent: &UiId, suffix: &str) -> UiId {
    UiId::named(format!("{}-{}", parent.as_str(), suffix))
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

    // --- value_to_x ---

    #[test]
    fn value_to_x_min_maps_to_track_left() {
        let x = Slider::value_to_x(100.0, 200.0, 0.0);
        assert_eq!(x, 100.0);
    }

    #[test]
    fn value_to_x_max_maps_to_track_right() {
        let x = Slider::value_to_x(100.0, 200.0, 1.0);
        assert_eq!(x, 300.0);
    }

    #[test]
    fn value_to_x_midpoint() {
        let x = Slider::value_to_x(100.0, 200.0, 0.5);
        assert_eq!(x, 200.0);
    }

    // --- value_from_pointer ---

    #[test]
    fn value_from_pointer_min() {
        let v = Slider::value_from_pointer(100.0, 100.0, 200.0, (0.0, 100.0));
        assert!((v - 0.0).abs() < 0.001);
    }

    #[test]
    fn value_from_pointer_max() {
        let v = Slider::value_from_pointer(300.0, 100.0, 200.0, (0.0, 100.0));
        assert!((v - 100.0).abs() < 0.001);
    }

    #[test]
    fn value_from_pointer_mid() {
        let v = Slider::value_from_pointer(200.0, 100.0, 200.0, (0.0, 100.0));
        assert!((v - 50.0).abs() < 0.001);
    }

    #[test]
    fn value_from_pointer_clamps_below_min() {
        let v = Slider::value_from_pointer(50.0, 100.0, 200.0, (10.0, 20.0));
        assert!((v - 10.0).abs() < 0.001);
    }

    #[test]
    fn value_from_pointer_clamps_above_max() {
        let v = Slider::value_from_pointer(500.0, 100.0, 200.0, (10.0, 20.0));
        assert!((v - 20.0).abs() < 0.001);
    }

    #[test]
    fn value_from_pointer_zero_width() {
        let v = Slider::value_from_pointer(200.0, 100.0, 0.0, (10.0, 20.0));
        assert!((v - 10.0).abs() < 0.001);
    }

    // --- Round-trip ---

    #[test]
    fn value_roundtrip() {
        let range = (1.02, 1.75);
        let v = 1.42;
        let t = (v - range.0) / (range.1 - range.0);
        let x = Slider::value_to_x(100.0, 200.0, t);
        let back = Slider::value_from_pointer(x, 100.0, 200.0, range);
        assert!((back - v).abs() < 0.001);
    }

    // --- Render ---

    #[test]
    fn slider_pushes_track_and_knob() {
        let mut ui = new_ui();
        ui.slider(&Slider::new(0.5, (0.0, 1.0)));
        let (render, _hits, _reg) = ui.take();
        let all: Vec<_> = render.ink.iter().flat_map(|b| &b.views).collect();
        assert!(all.iter().any(|v| v.kind == ControlKind::SliderTrack));
        assert!(all.iter().any(|v| v.kind == ControlKind::SliderKnob));
    }

    #[test]
    fn slider_with_reset_pushes_reset_icon() {
        let mut ui = new_ui();
        ui.slider(&Slider::new(0.5, (0.0, 1.0)).reset_opt(true));
        let (render, _hits, _reg) = ui.take();
        let all: Vec<_> = render.ink.iter().flat_map(|b| &b.views).collect();
        assert!(all.iter().any(|v| v.kind == ControlKind::ResetIcon));
    }

    #[test]
    fn slider_without_reset_omits_reset_icon() {
        let mut ui = new_ui();
        ui.slider(&Slider::new(0.5, (0.0, 1.0)).reset_opt(false));
        let (render, _hits, _reg) = ui.take();
        let all: Vec<_> = render.ink.iter().flat_map(|b| &b.views).collect();
        assert!(!all.iter().any(|v| v.kind == ControlKind::ResetIcon));
    }

    #[test]
    fn slider_clicked_via_active_click() {
        let mut ui = new_ui();
        let id = UiId::named("s1");
        let track_id = format_slider_id(&id, "track");
        ui.set_pointer(Some(Point::new(200.0, 23.0)));
        ui.set_active_click(Some(track_id.clone()));
        let resp = ui.slider(&Slider::new(0.5, (0.0, 1.0)).id(id));
        assert!(resp.changed);
    }

    #[test]
    fn slider_reset_clicked_via_active_click() {
        let mut ui = new_ui();
        let id = UiId::named("s2");
        let reset_id = format_slider_id(&id, "reset");
        ui.set_active_click(Some(reset_id));
        let resp = ui.slider(&Slider::new(0.5, (0.0, 1.0)).id(id));
        assert!(resp.reset_clicked);
    }

    #[test]
    fn slider_disabled_does_not_interact() {
        let mut ui = new_ui();
        let id = UiId::named("s3");
        let track_id = format_slider_id(&id, "track");
        ui.set_active_click(Some(track_id));
        let resp = ui.slider(&Slider::new(0.5, (0.0, 1.0)).enabled(false).id(id));
        assert!(!resp.changed);
    }

    #[test]
    fn slider_scales_with_dpi() {
        let theme = Theme {
            scale_factor: 2.0,
            ..Default::default()
        };
        let mut ui = Ui::new(theme, 800.0, 600.0);
        let resp = ui.slider(&Slider::new(0.5, (0.0, 1.0)));
        assert!(resp.response.rect.height > 20.0);
    }

    #[test]
    fn slider_registers_in_registry() {
        let mut ui = new_ui();
        let id = UiId::named("reg-sl");
        ui.slider(&Slider::new(0.5, (0.0, 1.0)).id(id.clone()));
        let (_, _, reg) = ui.take();
        assert!(reg.rect(&id).is_some());
    }

    #[test]
    fn slider_normalized_value_works() {
        let s = Slider::new(0.5, (0.0, 1.0));
        assert!((s.normalized_value() - 0.5).abs() < 0.001);
        let s2 = Slider::new(10.0, (0.0, 100.0));
        assert!((s2.normalized_value() - 0.1).abs() < 0.001);
    }
}
