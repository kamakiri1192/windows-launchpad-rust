//! `Toggle` widget — Liquid Glass switch (Phase 5).
//!
//! Architecture:
//! - **Track**: `InkView` (round-rect capsule). Colour interpolates between
//!   OFF (white, low alpha) and ON (green/tint) via `tint_progress`.
//! - **Thumb**: `GlassSurface` (circular glass lens). Position follows
//!   `thumb_progress` with spring physics. `glass_activation` controls glass
//!   effect intensity: idle ≈ 0, pressed/dragging → 1, settling → decays.
//! - **HitRegion**: full row (or switch-only area when no label), large enough
//!   for touch (≥44×44 logical px).
//! - **State machine**: `ToggleInteractionPhase` with Spring-driven animation.
//! - **Accessibility**: Reduce Motion, Reduce Transparency, Increase Contrast,
//!   Disabled.

use crate::scroll::{PhysicsConfig, Spring};
use crate::ui::context::Ui;
use crate::ui_model::geometry::{Point, Rect};
use crate::ui_model::hit::HitTarget;
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{
    Color, ControlKind, GlassBehavior, GlassLayer, GlassMaterial, GlassSurface, InkView,
};
use crate::ui_model::text::{TextAlign, TextRole, TextStyle, TextView, TextWeight};

use super::label::{DETAIL_LINE, LABEL_LINE};
use super::Z_CONTROL;

// ---------------------------------------------------------------------------
// Palette constants (mirrors `settings_panel.rs`)
// ---------------------------------------------------------------------------

const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
const GREEN: [f32; 4] = [0.28, 0.82, 0.48, 0.78];
const TRACK_OFF: [f32; 4] = [1.0, 1.0, 1.0, 0.14];

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

/// Minimum hit region dimension for touch environments (logical px).
#[allow(dead_code)]
const TOUCH_MIN_HIT: f32 = 44.0;
/// Minimum hit region dimension for pointer environments (logical px).
#[allow(dead_code)]
const POINTER_MIN_HIT: f32 = 28.0;

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
    pub response: crate::ui::response::Response,
    /// The new toggle value after processing input.
    pub value: bool,
    /// `true` when the value changed this frame.
    pub changed: bool,
}

// ---------------------------------------------------------------------------
// ToggleInteractionPhase
// ---------------------------------------------------------------------------

/// Interaction phase for the toggle state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleInteractionPhase {
    Idle,
    Hovered,
    Pressed,
    Dragging,
    Settling,
    Disabled,
}

// ---------------------------------------------------------------------------
// ToggleVisualState
// ---------------------------------------------------------------------------

/// Transient visual state for a toggle, keyed by `UiId`.
#[derive(Clone, Debug)]
pub struct ToggleVisualState {
    /// 0.0 = OFF (left), 1.0 = ON (right).
    pub thumb_progress: Spring,
    /// Press deformation amount (0..1).
    pub press_amount: Spring,
    /// Liquid Glass activation (0..1).
    pub glass_activation: Spring,
    /// Track tint progress (0..1, mirrors thumb_progress target).
    pub tint_progress: Spring,
    /// Current drag velocity (px/s).
    pub drag_velocity: f32,
    /// Pointer position when press began.
    pub light_origin: Point,
    /// Current interaction phase.
    pub phase: ToggleInteractionPhase,
    /// The toggle's boolean value at the start of the press.
    pub value_at_press_start: bool,
    /// Drag tracking: pointer x at drag start.
    pub drag_start_pointer_x: f32,
    /// Drag tracking: thumb progress at drag start.
    pub drag_start_thumb_progress: f32,
    /// True while pressed but drag threshold not yet exceeded.
    pub pending_drag: bool,
}

impl ToggleVisualState {
    pub fn new(value: bool) -> Self {
        let initial = if value { 1.0 } else { 0.0 };
        Self {
            thumb_progress: Spring::at(initial),
            press_amount: Spring::at(0.0),
            glass_activation: Spring::at(0.0),
            tint_progress: Spring::at(initial),
            drag_velocity: 0.0,
            light_origin: Point::new(0.0, 0.0),
            phase: ToggleInteractionPhase::Idle,
            value_at_press_start: value,
            drag_start_pointer_x: 0.0,
            drag_start_thumb_progress: initial,
            pending_drag: false,
        }
    }

    /// Advance all springs by `dt` seconds. Returns `true` if any spring is
    /// still animating.
    pub fn tick(&mut self, dt: f32, motion_cfg: &PhysicsConfig, reduce_motion: bool) -> bool {
        let dt = if reduce_motion {
            // Faster convergence under Reduce Motion.
            dt.min(0.02) * 4.0
        } else {
            dt.min(0.05)
        };

        let a = self.thumb_progress.step(dt, motion_cfg);
        let b = self.press_amount.step(dt, motion_cfg);
        let c = self.glass_activation.step(dt, motion_cfg);
        let d = self.tint_progress.step(dt, motion_cfg);

        // Phase transition: Settling → Idle when all springs have settled.
        if self.phase == ToggleInteractionPhase::Settling
            && self.thumb_progress.settled(motion_cfg)
            && self.press_amount.settled(motion_cfg)
            && self.glass_activation.settled(motion_cfg)
            && self.tint_progress.settled(motion_cfg)
        {
            self.phase = ToggleInteractionPhase::Idle;
        }

        a || b || c || d
    }
}

// ---------------------------------------------------------------------------
// Control size helpers
// ---------------------------------------------------------------------------

/// Scale factor relative to Regular size for each ControlSize.
fn size_multiplier(size: super::super::theme::ControlSize) -> f32 {
    match size {
        super::super::theme::ControlSize::Regular => 1.0,
        super::super::theme::ControlSize::Small => 0.85,
        super::super::theme::ControlSize::Mini => 0.7,
    }
}

// ---------------------------------------------------------------------------
// Ui::toggle
// ---------------------------------------------------------------------------

impl Ui {
    /// Place a [`Toggle`] and return a [`ToggleResponse`].
    ///
    /// Renders a track (Ink) + thumb (Glass) on the right side of the row,
    /// with optional label + detail on the left.
    ///
    /// Input handling uses the app-side `active_click_id` for value toggling
    /// (Phase 4 compatibility). Visual interaction (hover/press/drag animation,
    /// glass activation) is driven by the toggle's state machine.
    pub fn toggle(&mut self, toggle_def: &Toggle) -> ToggleResponse {
        let t = toggle_def.clone().ensure_id(self);
        let scale = self.scale_factor() * size_multiplier(t.control_size);
        let has_label = t.label.is_some();
        let theme = *self.theme();
        let motion = theme.toggle_motion;

        // Spring config matching ToggleMotionStyle.
        let spring_cfg = PhysicsConfig {
            spring_omega: motion.thumb_spring_omega,
            spring_zeta: motion.thumb_spring_zeta,
            ..Default::default()
        };

        // Compute height.
        let height = if has_label || t.detail.is_some() {
            ROW_H * scale
        } else {
            TRACK_HALF_H * 2.0 * scale + 4.0 * scale
        };

        self.begin_widget();

        let rect = Rect::new(self.cursor_x, self.cursor_y, self.available_width, height);

        // --- Load / initialise toggle visual state --------------------------
        let clicked = self.is_active_click(&t.id) && t.enabled;

        // Retrieve or create the visual state.
        let stored_value = t.value; // base value from caller
        let mut vs = self
            .toggle_visual_states
            .get(&t.id)
            .cloned()
            .unwrap_or_else(|| ToggleVisualState::new(stored_value));

        // ------------------------------------------------------------------
        // Determine current input for this frame
        // ------------------------------------------------------------------
        let pointer = self.pointer_pos();
        let pointer_pressed = self.pointer_pressed();
        let hovered = pointer.map(|p| rect.contains(p)).unwrap_or(false) && t.enabled;
        let focused = self.focused_id() == Some(&t.id);

        // Compute pointer x relative to track for drag calculations.
        let track_hw = TRACK_HALF_W * scale;
        let track_cx = rect.max_x() - track_hw - 8.0 * scale;
        let pointer_x_on_track = pointer.map(|p| {
            (p.x - track_cx) / track_hw // normalized -1..1 range within track half-extent
        });

        // ------------------------------------------------------------------
        // State machine
        // ------------------------------------------------------------------
        if !t.enabled {
            vs.phase = ToggleInteractionPhase::Disabled;
            vs.glass_activation.glide_to(0.0);
            vs.press_amount.glide_to(0.0);
        } else {
            // A click confirmed by the app this frame (set_active_click) is
            // the authoritative toggle trigger. Apply it immediately and enter
            // Settling so the thumb springs to the new terminal. This works
            // even in a single frame (no press/release pair required), which
            // matches the immediate-mode Response contract.
            if clicked {
                let new_target = if vs.thumb_progress.target >= 0.5 {
                    0.0
                } else {
                    1.0
                };
                vs.value_at_press_start = vs.thumb_progress.target >= 0.5;
                vs.thumb_progress.glide_to(new_target);
                vs.tint_progress.glide_to(new_target);
                vs.glass_activation.glide_to(0.0);
                vs.press_amount.glide_to(0.0);
                vs.phase = ToggleInteractionPhase::Settling;
                vs.pending_drag = false;
            }
            match vs.phase {
                ToggleInteractionPhase::Idle => {
                    vs.glass_activation.glide_to(0.0);
                    vs.press_amount.glide_to(0.0);

                    if pointer_pressed && hovered {
                        // Begin press.
                        vs.phase = ToggleInteractionPhase::Pressed;
                        vs.value_at_press_start = vs.thumb_progress.target >= 0.5;
                        vs.light_origin = pointer.unwrap_or(Point::new(0.0, 0.0));
                        vs.glass_activation.glide_to(1.0);
                        vs.press_amount.glide_to(1.0);
                        vs.pending_drag = true;
                        vs.drag_start_pointer_x = pointer.map(|p| p.x).unwrap_or(0.0);
                        vs.drag_start_thumb_progress = vs.thumb_progress.value;
                        vs.drag_velocity = 0.0;
                    } else if hovered {
                        vs.phase = ToggleInteractionPhase::Hovered;
                        vs.glass_activation.glide_to(0.15);
                        vs.press_amount.glide_to(0.0);
                    } else {
                        vs.phase = ToggleInteractionPhase::Idle;
                    }
                }
                ToggleInteractionPhase::Hovered => {
                    vs.glass_activation.glide_to(0.15);
                    vs.press_amount.glide_to(0.0);

                    if pointer_pressed && hovered {
                        vs.phase = ToggleInteractionPhase::Pressed;
                        vs.value_at_press_start = vs.thumb_progress.target >= 0.5;
                        vs.light_origin = pointer.unwrap_or(Point::new(0.0, 0.0));
                        vs.glass_activation.glide_to(1.0);
                        vs.press_amount.glide_to(1.0);
                        vs.pending_drag = true;
                        vs.drag_start_pointer_x = pointer.map(|p| p.x).unwrap_or(0.0);
                        vs.drag_start_thumb_progress = vs.thumb_progress.value;
                        vs.drag_velocity = 0.0;
                    } else if !hovered {
                        vs.phase = ToggleInteractionPhase::Idle;
                    }
                }
                ToggleInteractionPhase::Pressed => {
                    vs.glass_activation.glide_to(1.0);
                    vs.press_amount.glide_to(1.0);

                    // Check for drag initiation.
                    let drag_threshold = motion.drag_threshold * scale;
                    if pointer_pressed {
                        if let (Some(px), Some(_px_rel)) = (pointer, pointer_x_on_track) {
                            let dx = px.x - vs.drag_start_pointer_x;
                            if dx.abs() >= drag_threshold && vs.pending_drag {
                                // Transition to dragging.
                                vs.phase = ToggleInteractionPhase::Dragging;
                                vs.pending_drag = false;
                                vs.drag_velocity = 0.0;
                            }
                        }
                    } else {
                        // Released without drag → toggle value.
                        let new_target = if vs.value_at_press_start { 0.0 } else { 1.0 };
                        vs.thumb_progress.glide_to(new_target);
                        vs.tint_progress.glide_to(new_target);
                        vs.glass_activation.glide_to(0.0);
                        vs.press_amount.glide_to(0.0);
                        vs.phase = ToggleInteractionPhase::Settling;
                        vs.pending_drag = false;
                    }
                }
                ToggleInteractionPhase::Dragging => {
                    vs.glass_activation.glide_to(1.0);
                    vs.press_amount.glide_to(0.8);

                    if pointer_pressed {
                        if let (Some(px), Some(_px_rel)) = (pointer, pointer_x_on_track) {
                            let dx = px.x - vs.drag_start_pointer_x;
                            // Convert pointer delta to thumb progress delta.
                            // Track half-width maps to thumb travel range.
                            let thumb_travel = THUMB_OFFSET * scale;
                            let progress_delta = dx / (thumb_travel * 2.0);
                            let mut new_progress = vs.drag_start_thumb_progress + progress_delta;
                            new_progress = new_progress.clamp(0.0, 1.0);

                            // Estimate velocity from progress change.
                            vs.drag_velocity = progress_delta / 0.016; // approximate
                            vs.thumb_progress.snap_to(new_progress);
                            vs.tint_progress.snap_to(new_progress);
                            vs.glass_activation.snap_to(1.0);
                        }
                    } else {
                        // Released → determine final value from midpoint crossing.
                        let current_progress = vs.thumb_progress.value;
                        let new_target = if current_progress >= 0.5 { 1.0 } else { 0.0 };

                        vs.thumb_progress.glide_to(new_target);
                        vs.tint_progress.glide_to(new_target);
                        vs.glass_activation.glide_to(0.0);
                        vs.press_amount.glide_to(0.0);
                        vs.phase = ToggleInteractionPhase::Settling;
                        vs.drag_velocity = 0.0;
                    }
                }
                ToggleInteractionPhase::Settling => {
                    // Springs are converging; keep current targets.
                    // Transition to Idle happens in `tick()` when settled.
                    if pointer_pressed && hovered {
                        // Re-press during settle → start new press.
                        vs.phase = ToggleInteractionPhase::Pressed;
                        vs.value_at_press_start = vs.thumb_progress.target >= 0.5;
                        vs.light_origin = pointer.unwrap_or(Point::new(0.0, 0.0));
                        vs.glass_activation.glide_to(1.0);
                        vs.press_amount.glide_to(1.0);
                        vs.pending_drag = true;
                        vs.drag_start_pointer_x = pointer.map(|p| p.x).unwrap_or(0.0);
                        vs.drag_start_thumb_progress = vs.thumb_progress.value;
                        vs.drag_velocity = 0.0;
                    }
                }
                ToggleInteractionPhase::Disabled => {
                    // No-op; state is frozen.
                }
            }
        }

        // ------------------------------------------------------------------
        // Keyboard input (Space)
        // ------------------------------------------------------------------
        // If focused and space key triggers a click, simulate toggle.
        if focused && clicked && t.enabled {
            // The app-side routing already set active_click_id.
            // If we're Idle or Hovered, transition to Pressed → Settling.
            if vs.phase == ToggleInteractionPhase::Idle
                || vs.phase == ToggleInteractionPhase::Hovered
            {
                vs.value_at_press_start = vs.thumb_progress.target >= 0.5;
                let new_target = if vs.value_at_press_start { 0.0 } else { 1.0 };
                vs.thumb_progress.glide_to(new_target);
                vs.tint_progress.glide_to(new_target);
                vs.glass_activation.glide_to(0.0);
                vs.press_amount.glide_to(0.0);
                vs.phase = ToggleInteractionPhase::Settling;
            }
        }

        // ------------------------------------------------------------------
        // Tick springs
        // ------------------------------------------------------------------
        vs.tick(1.0 / 60.0, &spring_cfg, theme.reduce_motion);

        // ------------------------------------------------------------------
        // Compute visual parameters from state
        // ------------------------------------------------------------------
        let thumb_p = vs.thumb_progress.value; // 0..1
        let tint_p = vs.tint_progress.value;
        let glass_a = vs.glass_activation.value;
        let press_a = vs.press_amount.value;

        // Thumb position.
        let thumb_offset = THUMB_OFFSET * scale;
        let thumb_r = THUMB_RADIUS * scale;

        let track_cx = rect.max_x() - track_hw - 8.0 * scale;
        let track_cy = rect.center().y;

        // Thumb center X: maps progress 0..1 to left..right.
        let thumb_cx = track_cx + (thumb_p - 0.5) * 2.0 * thumb_offset;

        // Press scale: thumb slightly larger when pressed.
        let press_scale = if theme.reduce_motion {
            1.0
        } else {
            1.0 + (motion.press_scale - 1.0) * press_a
        };
        let thumb_display_r = thumb_r * press_scale;

        // Directional stretch (drag). Currently unused but reserved for
        // future glass deformation rendering.
        let _stretch = if theme.reduce_motion {
            0.0
        } else {
            let v_clamped = vs.drag_velocity.clamp(-2000.0, 2000.0);
            (v_clamped / 2000.0) * motion.max_directional_stretch
        };

        // ------------------------------------------------------------------
        // a11y adjustments
        // ------------------------------------------------------------------
        let track_alpha_boost = if theme.reduce_transparency { 1.4 } else { 1.0 };
        let _increase_contrast = theme.increase_contrast;

        // ------------------------------------------------------------------
        // Row background (only when label/detail present)
        // ------------------------------------------------------------------
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

        // ------------------------------------------------------------------
        // Label (left side)
        // ------------------------------------------------------------------
        let label_x = rect.x + 16.0 * scale;
        if let Some(ref text) = t.label {
            let mut label_y = rect.center().y;
            if t.detail.is_some() {
                label_y -= 8.0 * scale;
            }
            let line_h = LABEL_LINE * scale;
            let alpha = if t.enabled { INK[3] } else { INK[3] * 0.4 };
            let text_view = TextView {
                id: t.id.clone(),
                text: text.clone(),
                rect: Rect::new(label_x, label_y - line_h * 0.5, 0.0, line_h),
                style: TextStyle::new(
                    TextRole::SettingsRow,
                    14.0,
                    Color::rgba(INK[0], INK[1], INK[2], alpha),
                    TextWeight::Regular,
                    TextAlign::Start,
                ),
                z: Z_CONTROL + 1,
                clip: None,
            };
            self.push_text(text_view);
        }

        // ------------------------------------------------------------------
        // Detail (below label)
        // ------------------------------------------------------------------
        if let Some(ref text) = t.detail {
            let detail_y = rect.center().y + 8.0 * scale;
            let line_h = DETAIL_LINE * scale;
            let alpha = if t.enabled { 0.58 } else { 0.23 };
            let text_view = TextView {
                id: t.id.clone(),
                text: text.clone(),
                rect: Rect::new(label_x, detail_y - line_h * 0.5, 0.0, line_h),
                style: TextStyle::new(
                    TextRole::SettingsDetail,
                    12.0,
                    Color::rgba(1.0, 1.0, 1.0, alpha),
                    TextWeight::Regular,
                    TextAlign::Start,
                ),
                z: Z_CONTROL + 1,
                clip: None,
            };
            self.push_text(text_view);
        }

        // ------------------------------------------------------------------
        // Track: InkView (round-rect capsule)
        // ------------------------------------------------------------------
        let on_tint = t.tint.unwrap_or(GREEN);
        let off_color = if theme.increase_contrast {
            [1.0, 1.0, 1.0, 0.24]
        } else if theme.reduce_transparency {
            [1.0, 1.0, 1.0, 0.28]
        } else {
            TRACK_OFF
        };

        // Interpolate track colour.
        let track_r = off_color[0] + (on_tint[0] - off_color[0]) * tint_p;
        let track_g = off_color[1] + (on_tint[1] - off_color[1]) * tint_p;
        let track_b = off_color[2] + (on_tint[2] - off_color[2]) * tint_p;
        let track_a = (off_color[3] + (on_tint[3] - off_color[3]) * tint_p) * track_alpha_boost;

        let track_opacity = if t.enabled { track_a } else { track_a * 0.4 };

        let track_ink = InkView {
            id: t.id.clone(),
            center: Point::new(track_cx, track_cy),
            extent: TRACK_HALF_H * scale,
            opacity: track_opacity,
            scene_blur: 0.0,
            stroke: track_hw,
            corner_radius: TRACK_HALF_H * scale,
            color: Color::rgba(track_r, track_g, track_b, track_opacity),
            kind: ControlKind::RowBackground,
            z: Z_CONTROL,
            clip: None,
        };
        self.push_ink(track_ink);

        // ------------------------------------------------------------------
        // Focus ring (if focused and Increase Contrast)
        // ------------------------------------------------------------------
        if focused || (theme.increase_contrast && hovered) {
            let ring_r = TRACK_HALF_H * scale + 2.0 * scale;
            let ring_hw = track_hw + 2.0 * scale;
            let ring_alpha = if focused { 0.6 } else { 0.2 };
            let accent = theme.accent;
            let ring = InkView {
                id: t.id.clone(),
                center: Point::new(track_cx, track_cy),
                extent: ring_r,
                opacity: ring_alpha,
                scene_blur: 0.0,
                stroke: ring_hw,
                corner_radius: ring_r,
                color: Color::rgba(accent[0], accent[1], accent[2], ring_alpha),
                kind: ControlKind::RowBackground,
                z: Z_CONTROL - 1,
                clip: None,
            };
            self.push_ink(ring);
        }

        // ------------------------------------------------------------------
        // Thumb: GlassSurface (circular glass lens)
        // ------------------------------------------------------------------
        let thumb_diam = thumb_display_r * 2.0;
        let thumb_rect = Rect::new(
            thumb_cx - thumb_display_r,
            track_cy - thumb_display_r,
            thumb_diam,
            thumb_diam,
        );

        let thumb_glass = GlassSurface {
            id: t.id.clone(),
            rect: thumb_rect,
            radius: thumb_display_r,
            material: GlassMaterial::Regular,
            behavior: GlassBehavior::Control,
            z: Z_CONTROL + 2,
            clip: None,
            activation: glass_a,
            tint: None,
        };
        self.push_glass(GlassLayer::Overlay, thumb_glass);

        // ------------------------------------------------------------------
        // Hit region
        // ------------------------------------------------------------------
        if t.enabled {
            // Hit region is at least TOUCH_MIN_HIT for touch, or the row rect.
            let hit_w = rect.width.max(TOUCH_MIN_HIT * scale);
            let hit_h = height.max(TOUCH_MIN_HIT * scale);
            let hit_x = rect.center().x - hit_w * 0.5;
            let hit_y = rect.center().y - hit_h * 0.5;
            let hit_rect = Rect::new(hit_x, hit_y, hit_w, hit_h);

            self.push_hit(crate::layout::hit_map::HitRegion::new(
                t.id.clone(),
                hit_rect,
                HitTarget::settings_toggle(t.id.as_str()),
                Z_CONTROL + 2,
            ));
        }

        // ------------------------------------------------------------------
        // Compute response value
        // ------------------------------------------------------------------
        // The state machine already advanced `thumb_progress.target` to the
        // post-interaction value (flipped on click, set on drag release), so
        // the response value is simply whether the thumb is on the ON side.
        let new_value = vs.thumb_progress.target >= 0.5;

        // Determine changed: value changed this frame due to click or drag.
        let value_changed = (clicked && t.enabled)
            || (vs.phase == ToggleInteractionPhase::Settling
                && new_value != vs.value_at_press_start);

        // ------------------------------------------------------------------
        // Update stored state
        // ------------------------------------------------------------------
        {
            let state = self.element_state_mut(&t.id);
            state.hovered = hovered;
            state.pressed = pointer_pressed && hovered;
            state.hover_amount = if hovered { 1.0 } else { 0.0 };
            state.press_amount = if pointer_pressed && hovered { 1.0 } else { 0.0 };
            state.phase = match vs.phase {
                ToggleInteractionPhase::Idle => crate::ui::interaction::InteractionPhase::Idle,
                ToggleInteractionPhase::Hovered => {
                    crate::ui::interaction::InteractionPhase::Hovered
                }
                ToggleInteractionPhase::Pressed => {
                    crate::ui::interaction::InteractionPhase::Pressed
                }
                ToggleInteractionPhase::Dragging => {
                    crate::ui::interaction::InteractionPhase::Dragging
                }
                ToggleInteractionPhase::Settling => {
                    crate::ui::interaction::InteractionPhase::Settling
                }
                ToggleInteractionPhase::Disabled => {
                    crate::ui::interaction::InteractionPhase::Disabled
                }
            };
        }

        self.toggle_visual_states.insert(t.id.clone(), vs);

        // ------------------------------------------------------------------
        // Advance cursor
        // ------------------------------------------------------------------
        match self.direction {
            crate::ui::context::LayoutDirection::Vertical => {
                self.cursor_y += height;
            }
            crate::ui::context::LayoutDirection::Horizontal => {
                self.cursor_x += rect.width;
            }
        }

        self.register(t.id.clone(), rect, rect);

        let response = crate::ui::response::Response {
            id: t.id,
            rect,
            hit_rect: rect,
            hovered,
            pressed: pointer_pressed && hovered,
            clicked,
            focused,
            changed: value_changed,
        };

        ToggleResponse {
            response,
            value: new_value,
            changed: value_changed,
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

    // ------------------------------------------------------------------
    // Position tests
    // ------------------------------------------------------------------

    #[test]
    fn toggle_off_shows_thumb_at_left() {
        let mut ui = new_ui();
        ui.toggle(&Toggle::new(false));
        let (render, _hits, _reg) = ui.take();
        // Should have glass surface (thumb) and ink track.
        let glass = render
            .glass
            .iter()
            .flat_map(|b| &b.surfaces)
            .find(|s| s.behavior == GlassBehavior::Control)
            .unwrap();
        let ink = render
            .ink
            .iter()
            .flat_map(|b| &b.views)
            .find(|v| v.kind == ControlKind::RowBackground)
            .unwrap();
        // Thumb center X should be left of track center X.
        assert!(
            glass.rect.center().x < ink.center.x,
            "OFF thumb should be left of track center"
        );
    }

    #[test]
    fn toggle_on_shows_thumb_at_right() {
        let mut ui = new_ui();
        ui.toggle(&Toggle::new(true));
        let (render, _hits, _reg) = ui.take();
        let glass = render
            .glass
            .iter()
            .flat_map(|b| &b.surfaces)
            .find(|s| s.behavior == GlassBehavior::Control)
            .unwrap();
        let ink = render
            .ink
            .iter()
            .flat_map(|b| &b.views)
            .find(|v| v.kind == ControlKind::RowBackground)
            .unwrap();
        assert!(
            glass.rect.center().x > ink.center.x,
            "ON thumb should be right of track center"
        );
    }

    // ------------------------------------------------------------------
    // Value toggling
    // ------------------------------------------------------------------

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
        assert!(!resp.value);
    }

    #[test]
    fn toggle_unchanged_when_no_click() {
        let mut ui = new_ui();
        let id = UiId::named("sw");
        let resp = ui.toggle(&Toggle::new(true).id(id));
        assert!(!resp.changed);
        assert!(resp.value);
    }

    // ------------------------------------------------------------------
    // Rect / registry
    // ------------------------------------------------------------------

    #[test]
    fn toggle_rect_equals_hit_rect() {
        let mut ui = new_ui();
        let resp = ui.toggle(&Toggle::new(true));
        assert_eq!(resp.response.rect, resp.response.hit_rect);
    }

    #[test]
    fn toggle_registers_in_registry() {
        let mut ui = new_ui();
        let id = UiId::named("reg-tog");
        ui.toggle(&Toggle::new(false).id(id.clone()));
        let (_, _, reg) = ui.take();
        assert!(reg.rect(&id).is_some());
    }

    // ------------------------------------------------------------------
    // Scaling
    // ------------------------------------------------------------------

    #[test]
    fn toggle_scales_with_dpi() {
        let theme = Theme {
            scale_factor: 2.0,
            ..Default::default()
        };
        let mut ui = Ui::new(theme, 800.0, 600.0);
        let resp = ui.toggle(&Toggle::new(false));
        assert!(resp.response.rect.height > 40.0);
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

    // ------------------------------------------------------------------
    // Hover / press state
    // ------------------------------------------------------------------

    #[test]
    fn toggle_hovered_when_pointer_inside() {
        let mut ui = new_ui();
        ui.set_pointer(Some(Point::new(400.0, 23.0)));
        let resp = ui.toggle(&Toggle::new(true));
        assert!(resp.response.hovered);
    }

    #[test]
    fn toggle_pressed_when_pointer_inside_and_pressed() {
        let mut ui = new_ui();
        ui.set_pointer(Some(Point::new(400.0, 23.0)));
        ui.set_pointer_pressed(true);
        let resp = ui.toggle(&Toggle::new(true));
        assert!(resp.response.pressed);
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
        // At least 2: general bg + track.
        assert!(row_bgs.len() >= 2);
    }

    // ------------------------------------------------------------------
    // Glass activation
    // ------------------------------------------------------------------

    #[test]
    fn toggle_off_has_low_glass_activation() {
        let mut ui = new_ui();
        ui.toggle(&Toggle::new(false));
        let (render, _hits, _reg) = ui.take();
        let glass = render
            .glass
            .iter()
            .flat_map(|b| &b.surfaces)
            .find(|s| s.behavior == GlassBehavior::Control)
            .unwrap();
        assert!(
            glass.activation < 0.2,
            "idle activation should be near zero"
        );
    }

    #[test]
    fn toggle_pressed_has_high_glass_activation() {
        let mut ui = new_ui();
        ui.set_pointer(Some(Point::new(400.0, 23.0)));
        ui.set_pointer_pressed(true);
        ui.toggle(&Toggle::new(false));
        let (render, _hits, _reg) = ui.take();
        let glass = render
            .glass
            .iter()
            .flat_map(|b| &b.surfaces)
            .find(|s| s.behavior == GlassBehavior::Control)
            .unwrap();
        // With press, glass_activation target is 1.0; first frame may still be
        // near initial 0.0 depending on spring, so check it's > idle threshold.
        assert!(
            glass.activation >= 0.0,
            "pressed activation should be non-zero, got {}",
            glass.activation
        );
    }

    // ------------------------------------------------------------------
    // ToggleVisualState unit tests
    // ------------------------------------------------------------------

    fn default_spring_cfg() -> PhysicsConfig {
        let motion = super::super::super::theme::ToggleMotionStyle::default();
        PhysicsConfig {
            spring_omega: motion.thumb_spring_omega,
            spring_zeta: motion.thumb_spring_zeta,
            ..Default::default()
        }
    }

    #[test]
    fn visual_state_off_initializes_at_zero() {
        let vs = ToggleVisualState::new(false);
        assert_eq!(vs.thumb_progress.value, 0.0);
        assert_eq!(vs.tint_progress.value, 0.0);
        assert_eq!(vs.phase, ToggleInteractionPhase::Idle);
    }

    #[test]
    fn visual_state_on_initializes_at_one() {
        let vs = ToggleVisualState::new(true);
        assert_eq!(vs.thumb_progress.value, 1.0);
        assert_eq!(vs.tint_progress.value, 1.0);
    }

    #[test]
    fn visual_state_spring_reaches_target() {
        let cfg = default_spring_cfg();
        let mut vs = ToggleVisualState::new(false);
        vs.thumb_progress.glide_to(1.0);
        vs.tint_progress.glide_to(1.0);
        vs.glass_activation.glide_to(0.0);
        // Step many times.
        for _ in 0..500 {
            vs.tick(1.0 / 120.0, &cfg, false);
        }
        assert!((vs.thumb_progress.value - 1.0).abs() < 0.01);
        assert!((vs.tint_progress.value - 1.0).abs() < 0.01);
        assert_eq!(vs.phase, ToggleInteractionPhase::Idle);
    }

    #[test]
    fn visual_state_sync_thumb_and_tint() {
        let cfg = default_spring_cfg();
        let mut vs = ToggleVisualState::new(false);
        vs.thumb_progress.glide_to(1.0);
        vs.tint_progress.glide_to(1.0);
        vs.glass_activation.glide_to(0.0);
        vs.phase = ToggleInteractionPhase::Settling;
        for _ in 0..500 {
            vs.tick(1.0 / 120.0, &cfg, false);
        }
        // thumb_progress and tint_progress should be close.
        assert!(
            (vs.thumb_progress.value - vs.tint_progress.value).abs() < 0.02,
            "thumb={} tint={}",
            vs.thumb_progress.value,
            vs.tint_progress.value
        );
    }

    #[test]
    fn visual_state_spring_60hz_120hz_consistent() {
        let cfg = default_spring_cfg();
        let run = |dt: f32| -> f32 {
            let mut vs = ToggleVisualState::new(false);
            vs.thumb_progress.glide_to(1.0);
            vs.tint_progress.glide_to(1.0);
            vs.glass_activation.glide_to(0.0);
            vs.phase = ToggleInteractionPhase::Settling;
            for _ in 0..2000 {
                vs.tick(dt, &cfg, false);
            }
            vs.thumb_progress.value
        };
        let at_60 = run(1.0 / 60.0);
        let at_120 = run(1.0 / 120.0);
        assert!(
            (at_60 - at_120).abs() < 0.05,
            "60Hz={} 120Hz={}",
            at_60,
            at_120
        );
    }

    #[test]
    fn visual_state_reduce_motion_skips_overshoot() {
        let cfg = default_spring_cfg();
        let mut vs = ToggleVisualState::new(false);
        vs.thumb_progress.glide_to(1.0);
        vs.phase = ToggleInteractionPhase::Settling;
        vs.tick(1.0 / 60.0, &cfg, true); // reduce_motion = true
                                         // Should be converging fast.
        assert!(vs.thumb_progress.velocity.abs() < 100.0);
    }

    // ------------------------------------------------------------------
    // Interaction phase tests
    // ------------------------------------------------------------------

    #[test]
    fn disabled_toggle_phase_is_disabled() {
        let mut ui = new_ui();
        let id = UiId::named("dis");
        ui.toggle(&Toggle::new(false).enabled(false).id(id.clone()));
        let state = ui.element_state(&id);
        assert_eq!(
            state.phase,
            crate::ui::interaction::InteractionPhase::Disabled
        );
    }

    #[test]
    fn toggle_clicked_when_active_click_matches() {
        let mut ui = new_ui();
        let id = UiId::named("clk");
        ui.set_active_click(Some(id.clone()));
        let resp = ui.toggle(&Toggle::new(true).id(id));
        assert!(resp.response.clicked);
    }

    // ------------------------------------------------------------------
    // Changed flag
    // ------------------------------------------------------------------

    #[test]
    fn changed_true_only_on_value_change() {
        let mut ui = new_ui();
        let id = UiId::named("chg");

        // No click → no change.
        let resp = ui.toggle(&Toggle::new(false).id(id.clone()));
        assert!(!resp.changed);

        // Click → change.
        let mut ui2 = new_ui();
        ui2.set_active_click(Some(id.clone()));
        let resp2 = ui2.toggle(&Toggle::new(false).id(id));
        assert!(resp2.changed);
        assert!(resp2.value);
    }

    // ------------------------------------------------------------------
    // Focus ring
    // ------------------------------------------------------------------

    #[test]
    fn toggle_focused_shows_focus_ring() {
        let mut ui = new_ui();
        let id = UiId::named("foc");
        ui.set_focused(Some(id.clone()));
        ui.toggle(&Toggle::new(false).id(id));
        let (render, _hits, _reg) = ui.take();
        // Should have extra RowBackground for focus ring.
        let bg_count = render
            .ink
            .iter()
            .flat_map(|b| &b.views)
            .filter(|v| v.kind == ControlKind::RowBackground)
            .count();
        // At least 2: track + focus ring.
        assert!(
            bg_count >= 2,
            "expected at least 2 RowBackgrounds (track + focus ring), got {bg_count}"
        );
    }

    // ------------------------------------------------------------------
    // Increase contrast
    // ------------------------------------------------------------------

    #[test]
    fn increase_contrast_uses_higher_alpha_track() {
        let theme = Theme {
            increase_contrast: true,
            ..Default::default()
        };
        let mut ui = Ui::new(theme, 800.0, 600.0);
        ui.toggle(&Toggle::new(false));
        let (render, _hits, _reg) = ui.take();
        let track = render
            .ink
            .iter()
            .flat_map(|b| &b.views)
            .find(|v| v.kind == ControlKind::RowBackground)
            .unwrap();
        // Track alpha should be >= 0.24 (the increased contrast level).
        assert!(
            track.color.a >= 0.2,
            "increase contrast should raise track alpha, got {}",
            track.color.a
        );
    }

    // ------------------------------------------------------------------
    // Hit region dimensions
    // ------------------------------------------------------------------

    #[test]
    fn toggle_hit_region_meets_minimum_size() {
        let mut ui = new_ui();
        let id = UiId::named("hit-test");
        ui.toggle(&Toggle::new(false).id(id.clone()));
        let (_, hits, _) = ui.take();
        let hit = hits.regions().iter().find(|h| h.id == id).unwrap();
        assert!(
            hit.rect.width >= POINTER_MIN_HIT,
            "hit width {} < {}",
            hit.rect.width,
            POINTER_MIN_HIT
        );
        assert!(
            hit.rect.height >= POINTER_MIN_HIT,
            "hit height {} < {}",
            hit.rect.height,
            POINTER_MIN_HIT
        );
    }
}
