//! Renderer-neutral geometry for the morphing bottom-center control.
//!
//! This module owns the pure types and math that describe the search pill ↔
//! page indicator ↔ search field capsule and its edit-mode satellites (Done
//! capsule, settings gear). It is the Phase 2 counterpart of the settings
//! panel geometry and lives in the `layout` layer so it compiles as part of
//! the library target and can be shared by [`crate::layout::bottom_control`].
//!
//! The state machine ([`crate::features::bottom_control::BottomControl`]) and the GPU
//! instance builder (`ControlInstance` / `build_overlay_instances`) remain in
//! the binary-side `bottom_control` module. Those call into the pure helpers
//! here through thin wrappers so the existing public API is unchanged.
//!
//! All geometry is in **physical pixels**, matching the rest of the renderer.

use crate::ui_model::geometry::Point;

// ---- tunables ---------------------------------------------------------------

/// Seconds the page indicator stays visible after a page change before
/// returning to the search pill.
pub const INDICATOR_HOLD: std::time::Duration = std::time::Duration::from_millis(1800);

/// Cross-fade duration for the search pill ↔ page indicator swap. Snappy —
/// the indicator is a transient info flash, so it should pop in and out
/// quickly rather than slowly cross-fading.
pub const INDICATOR_CROSSFADE: f32 = 0.18;

/// Time for the pill → field expand animation, in seconds. iOS-ish: a touch
/// slower than before so the sideways growth feels deliberate, not snappy.
pub const EXPAND_DURATION: f32 = 0.42;
/// Time for the field → pill collapse animation, in seconds. Closing is a
/// little quicker than opening, matching iOS sheet/button behavior.
pub const COLLAPSE_DURATION: f32 = 0.34;
/// Half-width of the expanded search field, centered on the control.
pub const FIELD_HALF_WIDTH: f32 = 250.0;
/// Capsule height for the search pill / indicator (physical px). A bit taller
/// than before for more comfortable padding around the icon/label/dots.
pub const CAPSULE_HEIGHT: f32 = 38.0;
/// Capsule corner radius (half the height → fully rounded ends).
pub const CAPSULE_RADIUS: f32 = CAPSULE_HEIGHT * 0.5;
/// Horizontal padding around the edit-mode Done label.
pub const DONE_HORIZONTAL_PADDING: f32 = 18.0;
/// Nominal laid-out width of the edit-mode Done label at 1x scale. The actual
/// Done capsule still uses measured text width; this keeps the idle Search
/// pill visually aligned before edit-mode text measurement is available.
pub const NOMINAL_DONE_LABEL_WIDTH: f32 = 28.0;
/// Nominal laid-out width of the idle Search label at 1x scale.
pub const SEARCH_LABEL_WIDTH: f32 = 28.0;
/// Gap between the edit-mode Done capsule and the settings gear capsule, in
/// physical px (scaled by DPI).
pub const EDIT_GEAR_GAP: f32 = 16.0;
/// Vertical gap from the bottom of the fixed page frame to the capsule.
pub const BOTTOM_MARGIN: f32 = 30.0;

/// Caret blink cycle length (seconds). ~1.07s is the classic text-edit blink.
pub const CARET_BLINK_PERIOD: f32 = 1.07;

/// Shared text-caret visibility used by both the search field and folder
/// rename editor. Keeping one phase curve prevents the two editors from
/// feeling like unrelated controls.
pub fn caret_blink_opacity(phase: f32) -> f32 {
    let phase = phase.rem_euclid(CARET_BLINK_PERIOD);
    if phase < CARET_BLINK_PERIOD * 0.56 {
        1.0
    } else {
        0.0
    }
}

// ---- state / visual enums ---------------------------------------------------

/// Coarse logical state used for hit-testing and event routing.
///
/// `Visual` describes what is actually *drawn* (a blend of two states while
/// animating); `Mode` describes what the control is *doing*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Default: compact search pill.
    Pill,
    /// Transient page indicator, shown briefly after a page change.
    Indicator,
    /// Pill expanding into the search field (forward animation).
    Expanding,
    /// Expanded search input (fully open, caret blinking).
    Field,
    /// Field collapsing back to the pill (reverse animation).
    Collapsing,
}

impl Mode {
    /// `true` while the capsule geometry is mid-morph (expand/collapse).
    pub fn is_morphing(self) -> bool {
        matches!(self, Mode::Expanding | Mode::Collapsing)
    }

    /// Whether the control currently wants keyboard input (field open or
    /// opening). The app routes `KeyboardInput` to the text handlers only
    /// while this is true.
    pub fn wants_keyboard(self) -> bool {
        matches!(self, Mode::Field | Mode::Expanding | Mode::Collapsing)
    }
}

/// Which content layer is dominant — used by the renderer to cross-fade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visual {
    SearchPill,
    PageIndicator,
    SearchField,
}

/// One drawable content layer of the control, with a 0..1 opacity. Several of
/// these are emitted per frame so the renderer can cross-fade during morphs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlLayer {
    pub visual: Visual,
    pub alpha: f32,
}

/// The resolved geometry + content for one frame of the control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlGeometry {
    /// Capsule center in physical px.
    pub center: (f32, f32),
    /// Capsule half-size (hw, hh) in physical px.
    pub half_size: (f32, f32),
    /// Capsule corner radius.
    pub radius: f32,
    /// `u` channel: pill↔field expand progress (0 = pill, 1 = field).
    pub expand: f32,
    /// `v` channel: pill↔indicator cross-fade (0 = pill, 1 = indicator).
    pub indicator: f32,
    /// Current page index (0-based) and total page count, for the dots.
    pub page: usize,
    pub page_count: usize,
}

impl ControlGeometry {
    /// Capsule center X / Y.
    pub const fn cx(&self) -> f32 {
        self.center.0
    }
    pub const fn cy(&self) -> f32 {
        self.center.1
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditWidth {
    pub half_width: f32,
    pub progress: f32,
}

/// Read-only snapshot of the control's animated state, used by the pure
/// resolve/hit-test helpers. The state machine
/// ([`crate::features::bottom_control::BottomControl`]) builds this from its fields so
/// the geometry math stays free of `&self` borrows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlState {
    pub mode: Mode,
    /// 0 = pill size, 1 = full field size. Animated toward the mode's target.
    pub expand: f32,
    /// 0 = search pill content, 1 = page indicator content. Animated.
    pub indicator: f32,
}

impl ControlState {
    pub const fn new(mode: Mode, expand: f32, indicator: f32) -> Self {
        Self {
            mode,
            expand,
            indicator,
        }
    }
}

// ---- edit-mode settings gear (second capsule beside Done) ------------------

/// Geometry for the edit-mode settings gear capsule: its center in physical px
/// and the glass capsule radius. A circular capsule the same height as the
/// Done pill, placed to its right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditGearGeometry {
    pub center: (f32, f32),
    pub radius: f32,
    pub glass_radius: f32,
}

// ---- capsule resolve --------------------------------------------------------

/// Resolve the geometry + active content layers for the current frame without
/// the edit-width morph. Convenience wrapper around
/// [`resolve_scaled_with_edit_width`].
#[allow(clippy::too_many_arguments)]
pub fn resolve(
    state: ControlState,
    viewport: (u32, u32),
    frame_bottom: f32,
    page: usize,
    page_count: usize,
) -> (ControlGeometry, Vec<ControlLayer>) {
    resolve_scaled(state, viewport, frame_bottom, page, page_count, 1.0)
}

#[allow(clippy::too_many_arguments)]
pub fn resolve_scaled(
    state: ControlState,
    viewport: (u32, u32),
    frame_bottom: f32,
    page: usize,
    page_count: usize,
    scale_factor: f32,
) -> (ControlGeometry, Vec<ControlLayer>) {
    resolve_scaled_with_edit_width(
        state,
        viewport,
        frame_bottom,
        page,
        page_count,
        scale_factor,
        None,
    )
}

/// Resolve the geometry + active content layers for the current frame.
///
/// `viewport` is `(width, height)` in physical px. `page` is the current
/// 0-based page index, `page_count` the total. `frame_bottom` is the Y of
/// the bottom edge of the fixed page frame, so the control can sit below
/// it; if not known, pass the viewport height and it falls back to a
/// fixed bottom margin.
#[allow(clippy::too_many_arguments)]
pub fn resolve_scaled_with_edit_width(
    state: ControlState,
    viewport: (u32, u32),
    frame_bottom: f32,
    page: usize,
    page_count: usize,
    scale_factor: f32,
    edit_width: Option<EditWidth>,
) -> (ControlGeometry, Vec<ControlLayer>) {
    let scale = sanitize_scale(scale_factor);
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    let center_x = vw * 0.5;
    let capsule_height = CAPSULE_HEIGHT * scale;
    let capsule_radius = CAPSULE_RADIUS * scale;
    let bottom_margin = BOTTOM_MARGIN * scale;
    let edge_inset = 8.0 * scale;
    // Sit a fixed margin below the page frame, clamped into the viewport.
    let center_y = (frame_bottom + bottom_margin + capsule_height * 0.5)
        .min(vh - capsule_height * 0.5 - edge_inset)
        .max(capsule_height * 0.5 + edge_inset);

    // Half-width interpolates from the compact pill to the wide field.
    let pill_hw = pill_half_width() * scale;
    let hh = capsule_height * 0.5;
    let normal_hw = lerp(
        pill_hw,
        FIELD_HALF_WIDTH * scale,
        ease_ios_out(state.expand),
    );
    let hw = match edit_width {
        Some(edit) if edit.progress > 0.0 => lerp(
            normal_hw,
            edit.half_width.max(hh),
            ease_ios_out(edit.progress.clamp(0.0, 1.0)),
        ),
        _ => normal_hw,
    };

    // In edit mode a second capsule (the settings gear) sits to the right
    // of the Done capsule. To keep the pair visually centered, shift the
    // Done capsule left by half the gear pair width, eased in with the
    // same edit progress so it slides as it shrinks.
    let edit_offset = match edit_width {
        Some(edit) if edit.progress > 0.0 => {
            let gear_r = hh;
            let pair_shift = gear_r + EDIT_GEAR_GAP * scale * 0.5 + gear_r;
            ease_ios_out(edit.progress.clamp(0.0, 1.0)) * pair_shift * 0.5
        }
        _ => 0.0,
    };
    let geom = ControlGeometry {
        center: (center_x - edit_offset, center_y),
        half_size: (hw, hh),
        radius: capsule_radius,
        expand: state.expand,
        indicator: state.indicator,
        page,
        page_count,
    };

    // Build the active content layers. During morphs we draw both sides
    // and cross-fade; the renderer multiplies each layer's alpha.
    let mut layers = Vec::with_capacity(2);
    match state.mode {
        Mode::Pill => {
            // Mostly pill; a sliver of indicator only while fading out.
            if state.indicator > 0.01 {
                layers.push(ControlLayer {
                    visual: Visual::PageIndicator,
                    alpha: state.indicator,
                });
            }
            layers.push(ControlLayer {
                visual: Visual::SearchPill,
                alpha: 1.0 - state.indicator,
            });
        }
        Mode::Indicator => {
            layers.push(ControlLayer {
                visual: Visual::PageIndicator,
                alpha: state.indicator,
            });
            if state.indicator < 0.99 {
                layers.push(ControlLayer {
                    visual: Visual::SearchPill,
                    alpha: 1.0 - state.indicator,
                });
            }
        }
        Mode::Expanding => {
            // Field content fades in as the capsule widens.
            let a = ease_in_out(state.expand);
            layers.push(ControlLayer {
                visual: Visual::SearchField,
                alpha: a,
            });
            if a < 0.99 {
                layers.push(ControlLayer {
                    visual: Visual::SearchPill,
                    alpha: 1.0 - a,
                });
            }
        }
        Mode::Field => {
            layers.push(ControlLayer {
                visual: Visual::SearchField,
                alpha: 1.0,
            });
        }
        Mode::Collapsing => {
            let a = ease_in_out(state.expand);
            layers.push(ControlLayer {
                visual: Visual::SearchField,
                alpha: a,
            });
            if a < 0.99 {
                layers.push(ControlLayer {
                    visual: Visual::SearchPill,
                    alpha: 1.0 - a,
                });
            }
        }
    }

    (geom, layers)
}

/// Hit-test a physical-pixel point against the control's capsule, using
/// the *current* (possibly animating) geometry. Returns `true` if the
/// point is inside the capsule.
#[allow(clippy::too_many_arguments)]
pub fn hit_test(
    state: ControlState,
    viewport: (u32, u32),
    frame_bottom: f32,
    x: f32,
    y: f32,
) -> bool {
    hit_test_scaled(state, viewport, frame_bottom, x, y, 1.0)
}

#[allow(clippy::too_many_arguments)]
pub fn hit_test_scaled(
    state: ControlState,
    viewport: (u32, u32),
    frame_bottom: f32,
    x: f32,
    y: f32,
    scale_factor: f32,
) -> bool {
    let (geom, _) = resolve_scaled(state, viewport, frame_bottom, 0, 0, scale_factor);
    contains_capsule(&geom, Point::new(x, y))
}

/// Capsule-shape containment test (inner rect + endcap circles). Exposed so
/// callers can classify a point against the precise capsule shape rather than
/// its bounding rect.
pub fn contains_capsule(geom: &ControlGeometry, point: Point) -> bool {
    let dx = (point.x - geom.center.0).abs();
    let dy = (point.y - geom.center.1).abs();
    let hw = geom.half_size.0;
    let hh = geom.half_size.1;
    if dy > hh {
        return false;
    }
    if dx <= hw - hh {
        return true;
    }
    let cx = hw - hh;
    let ex = dx - cx;
    ex * ex + dy * dy <= hh * hh
}

/// The geometry's left edge X, accounting for the close button hit region
/// inside an open field. Returns `Some(x)` only when the field is open
/// enough to show the close button.
pub fn close_button_x(state: ControlState, viewport: (u32, u32), frame_bottom: f32) -> Option<f32> {
    close_button_x_scaled(state, viewport, frame_bottom, 1.0)
}

pub fn close_button_x_scaled(
    state: ControlState,
    viewport: (u32, u32),
    frame_bottom: f32,
    scale_factor: f32,
) -> Option<f32> {
    if !matches!(state.mode, Mode::Field | Mode::Expanding | Mode::Collapsing) {
        return None;
    }
    let scale = sanitize_scale(scale_factor);
    let (geom, _) = resolve_scaled(state, viewport, frame_bottom, 0, 0, scale);
    if geom.expand < 0.5 {
        return None;
    }
    Some(geom.center.0 + geom.half_size.0 - 20.0 * scale)
}

// ---- glass shape helpers ----------------------------------------------------
//
// `glass_shape` / `edit_gear_glass_shape` build a renderer-specific
// `GlassShape` (binary-side `liquid_glass` module) and therefore stay in the
// binary-side `bottom_control` module. This layer only owns the numeric
// geometry (center, half-size, radius) that those helpers consume.

/// The X origin (physical px) where the search-field query text should start,
/// relative to the control. Used by the caller to lay out the query glyphs.
pub fn field_text_origin_x(geom: &ControlGeometry) -> f32 {
    let scale = control_scale(geom);
    let mag_size = search_magnifier_size(scale);
    let mag_cx = geom.center.0 - geom.half_size.0 + mag_size + 10.0 * scale;
    mag_cx + mag_size + 6.0 * scale
}

/// Centers of the idle Search pill's magnifier and label. The pair is centered
/// as one content group, so widening the capsule does not leave the label
/// visually left-aligned.
pub fn search_pill_content_centers(geom: &ControlGeometry) -> (f32, f32) {
    let scale = control_scale(geom);
    let mag_size = search_magnifier_size(scale);
    let label_width = SEARCH_LABEL_WIDTH * scale;
    let gap = 6.0 * scale;
    let group_width = mag_size * 2.0 + gap + label_width;
    let group_left = geom.center.0 - group_width * 0.5;
    let mag_cx = group_left + mag_size;
    let label_cx = group_left + mag_size * 2.0 + gap + label_width * 0.5;
    (mag_cx, label_cx)
}

// ---- edit-mode settings gear geometry ---------------------------------------

/// Resolve the edit-mode gear capsule center + radius. Returns `None` when the
/// edit morph isn't active (progress <= 0). `done_half_width` is the resolved
/// half-width of the Done capsule; `done_center_x` is inferred from the
/// viewport center minus the same pair-shift used in resolve.
///
/// `alpha` (0..1) is the cross-fade alpha tied to the edit progress so the
/// gear fades in/out with the Done label.
pub fn edit_gear_geometry(
    viewport: (u32, u32),
    frame_bottom: f32,
    scale_factor: f32,
    done_half_width: f32,
    edit_progress: f32,
) -> Option<(EditGearGeometry, f32)> {
    let p = ease_ios_out(edit_progress.clamp(0.0, 1.0));
    if p <= 0.0 {
        return None;
    }
    let scale = sanitize_scale(scale_factor);
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    let capsule_height = CAPSULE_HEIGHT * scale;
    let hh = capsule_height * 0.5;
    let gear_r = hh;
    let glass_grow = ease_ios_out((p * 1.2).clamp(0.0, 1.0));
    let glass_r = (gear_r * glass_grow).max(1.0);
    let edge_inset = 8.0 * scale;
    let center_y = (frame_bottom + BOTTOM_MARGIN * scale + capsule_height * 0.5)
        .min(vh - capsule_height * 0.5 - edge_inset)
        .max(capsule_height * 0.5 + edge_inset);

    // The Done capsule center, mirroring the offset applied in resolve.
    let pair_shift = gear_r + EDIT_GEAR_GAP * scale * 0.5 + gear_r;
    let done_cx = vw * 0.5 - p * pair_shift * 0.5;
    // Grow from the Done capsule's right edge, then settle into the final
    // gap. Because the glass pass smooth-unions both SDFs, this reads as a
    // small liquid bud pulling away into the settings gear.
    let attached_cx = done_cx + done_half_width + glass_r * 0.38;
    let final_cx = done_cx + done_half_width + EDIT_GEAR_GAP * scale + gear_r;
    let gear_cx = lerp(attached_cx, final_cx, ease_ios_out(p));

    Some((
        EditGearGeometry {
            center: (gear_cx, center_y),
            radius: gear_r,
            glass_radius: glass_r,
        },
        p,
    ))
}

/// Hit-test the edit-mode gear at physical-px pointer `(x, y)`.
pub fn edit_gear_hit(geom: &EditGearGeometry, x: f32, y: f32) -> bool {
    let dx = x - geom.center.0;
    let dy = y - geom.center.1;
    dx * dx + dy * dy <= geom.radius * geom.radius
}

// ---- free helpers -----------------------------------------------------------

/// Half-width of the compact search pill (content-aware: magnifier + label).
pub fn pill_half_width() -> f32 {
    edit_pair_half_width(done_half_width(NOMINAL_DONE_LABEL_WIDTH, 1.0), 1.0)
}

/// Half-width for the edit-mode Done capsule, based on measured text width.
pub fn done_half_width(label_width: f32, scale_factor: f32) -> f32 {
    let scale = sanitize_scale(scale_factor);
    let min_hw = CAPSULE_HEIGHT * scale * 0.5;
    let content_hw = label_width.max(0.0) * 0.5 + DONE_HORIZONTAL_PADDING * scale;
    content_hw.max(min_hw)
}

/// Half-width of the full edit-mode control pair: Done capsule + gap + gear.
pub fn edit_pair_half_width(done_half_width: f32, scale_factor: f32) -> f32 {
    let scale = sanitize_scale(scale_factor);
    let gear_r = CAPSULE_HEIGHT * scale * 0.5;
    done_half_width + EDIT_GEAR_GAP * scale * 0.5 + gear_r
}

pub(crate) fn control_scale(geom: &ControlGeometry) -> f32 {
    (geom.half_size.1 / (CAPSULE_HEIGHT * 0.5)).max(0.01)
}

pub(crate) fn search_magnifier_size(scale: f32) -> f32 {
    11.0 * scale
}

/// Linear advancement: moves `v` toward `target` at a constant rate so it
/// completes in exactly `duration` seconds (frame-rate independent). The
/// easing curve is applied by the consumer, which lets `resolve` shape the
/// visual morph with an iOS-style ease-out rather than an exponential tail.
///
/// Public so the binary-side `BottomControl` state machine can reuse it from
/// its `tick`/`step_*` methods without duplicating the math.
pub fn advance_linear(v: f32, target: f32, dt: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return target;
    }
    let dir = if target >= v { 1.0 } else { -1.0 };
    let step = dt / duration;
    let next = v + dir * step;
    // Clamp to target on overshoot.
    if dir > 0.0 {
        next.min(target)
    } else {
        next.max(target)
    }
}

pub(crate) fn sanitize_scale(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

/// Cubic ease-in-out, symmetric S-curve. Used for content cross-fades so they
/// ramp gently rather than cutting in.
pub(crate) fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) * 0.5
    }
}

/// iOS-style ease-out: approximates the cubic-bezier `cubic-bezier(0.32, 0.72,
/// 0, 1)` used by UIKit for spring-free controls. Starts fast and decelerates
/// into its rest position, giving the "deliberate but lively" feel of an iOS
/// pill expanding. Asymmetric by design (no ease-in).
///
/// Implemented as a rational quadratic Bézier through the curve's control
/// points, which is cheap and avoids a full cubic solver per frame.
pub(crate) fn ease_ios_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    // cubic-bezier(0.32, 0.72, 0, 1) — evaluate via Newton iteration on the
    // parametric cubic. x(t)=3(1-t)^2 t·0.32 + 3(1-t)t^2·0 + t^3, then y(t)=...
    // Cheaper + exact enough: sample the curve with a few Newton steps.
    cubic_bezier_y(0.32, 0.72, 0.0, 1.0, t)
}

/// Evaluate y(x) of a CSS cubic-bezier(p1x,p1y,p2x,p2y) easing curve at a
/// given progress `x` ∈ [0,1]. Solves the parametric x(s) for s then returns
/// y(s). Uses Newton-Raphson with a bisection fallback.
pub(crate) fn cubic_bezier_y(p1x: f32, p1y: f32, p2x: f32, p2y: f32, x: f32) -> f32 {
    // x(s) = 3(1-s)^2 s p1x + 3(1-s) s^2 p2x + s^3
    let bezier = |s: f32| -> f32 {
        let one_minus = 1.0 - s;
        3.0 * one_minus * one_minus * s * p1x + 3.0 * one_minus * s * s * p2x + s * s * s
    };
    // Solve bezier(s) == x for s.
    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    let mut s = x; // initial guess
    for _ in 0..8 {
        let cx = bezier(s);
        if (cx - x).abs() < 1e-4 {
            break;
        }
        if cx < x {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) * 0.5;
    }
    // y(s) with the same Bernstein form.
    let one_minus = 1.0 - s;
    3.0 * one_minus * one_minus * s * p1y + 3.0 * one_minus * s * s * p2y + s * s * s
}

pub(crate) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(mode: Mode) -> ControlState {
        ControlState::new(
            mode,
            match mode {
                Mode::Field => 1.0,
                _ => 0.0,
            },
            0.0,
        )
    }

    #[test]
    fn shared_caret_blink_has_visible_and_hidden_phases() {
        assert_eq!(caret_blink_opacity(0.0), 1.0);
        assert_eq!(caret_blink_opacity(CARET_BLINK_PERIOD * 0.55), 1.0);
        assert_eq!(caret_blink_opacity(CARET_BLINK_PERIOD * 0.75), 0.0);
        assert_eq!(caret_blink_opacity(CARET_BLINK_PERIOD), 1.0);
    }

    #[test]
    fn resolve_pill_geometry_is_centered() {
        let (g, layers) = resolve(state(Mode::Pill), (1280, 800), 600.0, 0, 3);
        assert!((g.center.0 - 640.0).abs() < 1e-3, "centered horizontally");
        assert!((g.half_size.1 - CAPSULE_HEIGHT * 0.5).abs() < 1e-3);
        // Default pill shows one SearchPill layer.
        assert!(layers.iter().any(|l| l.visual == Visual::SearchPill));
    }

    #[test]
    fn resolve_field_geometry_is_wide() {
        let (g, _) = resolve(state(Mode::Field), (1280, 800), 600.0, 0, 3);
        assert!((g.half_size.0 - FIELD_HALF_WIDTH).abs() < 1e-2);
    }

    #[test]
    fn done_half_width_tracks_text_width_and_scale() {
        let label_width = 28.0;
        let hw = done_half_width(label_width, 1.0);
        assert!((hw - 32.0).abs() < 1e-3);

        let scaled = done_half_width(label_width * 2.0, 2.0);
        assert!((scaled - hw * 2.0).abs() < 1e-3);
    }

    #[test]
    fn search_pill_width_matches_nominal_done_gear_pair() {
        let done_hw = done_half_width(28.0, 1.0);
        assert!((pill_half_width() - edit_pair_half_width(done_hw, 1.0)).abs() < 1e-3);
    }

    #[test]
    fn search_pill_content_group_is_centered() {
        let (geom, _) = resolve(state(Mode::Pill), (1280, 800), 600.0, 0, 3);
        let (mag_cx, label_cx) = search_pill_content_centers(&geom);
        let scale = control_scale(&geom);
        let mag_size = search_magnifier_size(scale);
        let label_width = SEARCH_LABEL_WIDTH * scale;
        let group_left = mag_cx - mag_size;
        let group_right = label_cx + label_width * 0.5;

        assert!(((group_left + group_right) * 0.5 - geom.center.0).abs() < 1e-3);
    }

    #[test]
    fn edit_width_morph_uses_done_half_width() {
        let done_hw = done_half_width(28.0, 1.0);
        let (normal, _) = resolve(state(Mode::Pill), (1280, 800), 600.0, 0, 3);
        let (done, _) = resolve_scaled_with_edit_width(
            state(Mode::Pill),
            (1280, 800),
            600.0,
            0,
            3,
            1.0,
            Some(EditWidth {
                half_width: done_hw,
                progress: 1.0,
            }),
        );

        assert!((normal.half_size.0 - pill_half_width()).abs() < 1e-3);
        assert!((done.half_size.0 - done_hw).abs() < 1e-3);
        assert!(done.half_size.0 < normal.half_size.0);
    }

    #[test]
    fn edit_gear_settles_with_configured_gap() {
        let done_hw = done_half_width(28.0, 1.0);
        let (gear, _) =
            edit_gear_geometry((1280, 800), 600.0, 1.0, done_hw, 1.0).expect("edit gear geometry");
        let pair_shift = gear.radius + EDIT_GEAR_GAP * 0.5 + gear.radius;
        let done_cx = 1280.0 * 0.5 - pair_shift * 0.5;
        let done_right = done_cx + done_hw;
        let gear_left = gear.center.0 - gear.radius;

        assert!((gear_left - done_right - EDIT_GEAR_GAP).abs() < 1e-3);
    }

    #[test]
    fn close_button_x_only_when_field_open() {
        assert!(close_button_x(state(Mode::Pill), (1280, 800), 600.0).is_none());
        assert!(close_button_x(state(Mode::Field), (1280, 800), 600.0).is_some());
    }

    #[test]
    fn hit_test_inside_capsule() {
        let (g, _) = resolve(state(Mode::Pill), (1280, 800), 600.0, 0, 1);
        assert!(hit_test(
            state(Mode::Pill),
            (1280, 800),
            600.0,
            g.center.0,
            g.center.1
        ));
        // Well outside.
        assert!(!hit_test(state(Mode::Pill), (1280, 800), 600.0, 10.0, 10.0));
    }

    #[test]
    fn contains_capsule_rejects_bounding_rect_corner() {
        let (geom, _) = resolve(state(Mode::Pill), (1280, 800), 600.0, 0, 1);
        let hw = geom.half_size.0;
        let hh = geom.half_size.1;
        // Bounding-rect corner is outside the rounded capsule.
        assert!(!contains_capsule(
            &geom,
            Point::new(geom.center.0 + hw, geom.center.1 + hh)
        ));
    }

    #[test]
    fn resolve_indicator_has_dots_layer() {
        let st = ControlState::new(Mode::Indicator, 0.0, 1.0);
        let (g, layers) = resolve(st, (1280, 800), 600.0, 1, 4);
        assert_eq!(g.page, 1);
        assert_eq!(g.page_count, 4);
        assert!(layers.iter().any(|l| l.visual == Visual::PageIndicator));
    }

    #[test]
    fn advance_linear_reaches_target() {
        // Linear advance reaches the target in roughly `duration` seconds.
        let mut v = 0.0f32;
        let mut t: f32 = 0.0;
        for _ in 0..1000 {
            v = advance_linear(v, 1.0, 1.0 / 60.0, 0.42);
            t += 1.0 / 60.0;
            if (v - 1.0).abs() < 1e-4 {
                break;
            }
        }
        assert!((v - 1.0).abs() < 1e-3);
        // Should complete in about 0.42s (± a frame).
        assert!((t - 0.42).abs() < 0.05, "completed at t={t}s, want ~0.42s");
    }

    #[test]
    fn ease_in_out_endpoints() {
        assert!((ease_in_out(0.0) - 0.0).abs() < 1e-6);
        assert!((ease_in_out(1.0) - 1.0).abs() < 1e-6);
        // Symmetric about 0.5.
        assert!((ease_in_out(0.25) + ease_in_out(0.75) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ease_ios_out_endpoints() {
        assert!((ease_ios_out(0.0) - 0.0).abs() < 1e-3, "starts at 0");
        assert!((ease_ios_out(1.0) - 1.0).abs() < 1e-3, "ends at 1");
        // Monotonically increasing.
        let mut prev = -1.0;
        let mut ok = true;
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            let y = ease_ios_out(t);
            if y < prev - 1e-6 {
                ok = false;
                break;
            }
            prev = y;
        }
        assert!(ok, "ease_ios_out is monotonic");
        // Decelerating: y at 0.5 should be past 0.5 (ease-out front-loads).
        assert!(
            ease_ios_out(0.5) > 0.5,
            "ease-out should overshoot the midpoint"
        );
    }

    #[test]
    fn wants_keyboard_matches_mode_set() {
        assert!(!Mode::Pill.wants_keyboard());
        assert!(!Mode::Indicator.wants_keyboard());
        assert!(Mode::Expanding.wants_keyboard());
        assert!(Mode::Field.wants_keyboard());
        assert!(Mode::Collapsing.wants_keyboard());
    }
}
