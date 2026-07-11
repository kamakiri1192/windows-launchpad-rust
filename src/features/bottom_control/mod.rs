//! The morphing bottom-center control: search pill ↔ page indicator ↔
//! search field.
//!
//! This is the iOS-Launchpad-style control that lives at the bottom center of
//! the window. It is a *single component* that morphs between three visuals:
//!
//! - [`Visual::SearchPill`]: a compact "🔍 検索" Liquid Glass pill (default).
//! - [`Visual::PageIndicator`]: a row of dots showing the current page. Shown
//!   transiently for a few seconds right after a page change, then it fades
//!   back to the pill.
//! - [`Visual::SearchField`]: the pill expanded sideways into a text input.
//!
//! The capsule geometry (center, size, corner radius) is driven by a single
//! animation progress value so the pill↔field morph is continuous. The
//! contents cross-fade/slides on top of that. All geometry is in **physical
//! pixels**, matching the rest of the renderer.
//!
//! State machine:
//! ```text
//!   startup ──▶ IdleSearchPill
//!   page change ──▶ TransientPageIndicator ──(timeout)──▶ IdleSearchPill
//!   pill click ──▶ Expanding ──▶ ExpandedSearchField
//!   field close ──▶ Collapsing ──▶ IdleSearchPill
//! ```
//!
//! ## Layering
//!
//! The pure geometry types and math live in [`crate::layout::control_geometry`]
//! (part of the library target) so the Phase 2 layout layer can compile
//! against them. This module re-exports them for backwards compatibility and
//! owns the state machine plus the GPU-facing overlay instance builder.

// Re-export the pure geometry layer so existing call sites (`bottom_control::
// ControlGeometry`, etc.) keep working without touching every reference. Only
// the symbols `main.rs` actually consumes are re-exported; the rest stay
// reachable as `crate::layout::control_geometry::*`.
pub use crate::layout::control_geometry::{
    done_half_width, edit_gear_geometry, field_text_origin_x, search_pill_content_centers,
    ControlGeometry, ControlLayer, ControlState, EditWidth, Mode, Visual, COLLAPSE_DURATION,
    EXPAND_DURATION,
};

// Used internally by the state machine's `tick`/`step_*` math; not re-exported.
use crate::layout::control_geometry::advance_linear;
// Constants used by the state machine's `tick`. Not re-exported.
use crate::layout::control_geometry::{CARET_BLINK_PERIOD, INDICATOR_CROSSFADE, INDICATOR_HOLD};
// Free geometry helpers used by the `BottomControl` resolve/hit-test methods.
// Not re-exported; `main.rs` calls them through the `BottomControl` methods.
use crate::layout::control_geometry::{
    close_button_x_scaled, hit_test_scaled, resolve_scaled_with_edit_width,
};

use std::time::Instant;

// ---- tunables (edit-mode label width constants used only here) -------------

/// Horizontal padding around the edit-mode Done label.
pub const DONE_HORIZONTAL_PADDING: f32 = 18.0;
/// Nominal laid-out width of the edit-mode Done label at 1x scale. The actual
/// Done capsule still uses measured text width; this keeps the idle Search
/// pill visually aligned before edit-mode text measurement is available.
pub const NOMINAL_DONE_LABEL_WIDTH: f32 = 28.0;
/// Vertical gap from the bottom of the fixed page frame to the capsule.
pub const BOTTOM_MARGIN: f32 = 30.0;

/// The morphing bottom-center control.
///
/// Owns its mode, animation progress, indicator timer, and current search
/// query. Pure logic + timing; the renderer turns [`resolve`] output into
/// GPU draws.
pub struct BottomControl {
    pub mode: Mode,
    /// 0 = pill size, 1 = full field size. Animated toward the mode's target.
    pub expand: f32,
    /// 0 = search pill content, 1 = page indicator content. Animated.
    pub indicator: f32,
    /// Instant when the transient indicator should retire back to the pill.
    indicator_until: Option<Instant>,
    /// Clock for `tick`. Held so callers don't need to track dt themselves.
    last_time: Option<Instant>,
    /// Caret blink phase accumulator (seconds). Wraps every ~1s.
    pub caret_phase: f32,
    /// Current search query text.
    pub query: String,
    /// Where new text is inserted / the caret sits (byte length of `query`).
    pub caret: usize,
    /// Active IME preedit (composition) string, shown inline while composing
    /// Japanese/IME text. Empty when nothing is being composed.
    pub preedit: String,
}

impl Default for BottomControl {
    fn default() -> Self {
        Self::new()
    }
}

impl BottomControl {
    pub fn new() -> Self {
        Self {
            mode: Mode::Pill,
            expand: 0.0,
            indicator: 0.0,
            indicator_until: None,
            last_time: None,
            caret_phase: 0.0,
            query: String::new(),
            caret: 0,
            preedit: String::new(),
        }
    }

    /// Whether the control currently wants keyboard input (field open or
    /// opening). The app routes `KeyboardInput` to [`handle_char`] /
    /// [`handle_backspace`] / [`handle_escape`] only while this is true.
    pub fn wants_keyboard(&self) -> bool {
        self.mode.wants_keyboard()
    }

    /// Notify the control that the user changed pages (swipe / programmatic).
    /// Arms the transient indicator — unless the search field is open, in
    /// which case the page change is ignored so focus isn't yanked away.
    pub fn on_page_change(&mut self, now: Instant) {
        // Never interrupt an open (or opening) search field for a page change.
        if matches!(self.mode, Mode::Field | Mode::Expanding) {
            return;
        }
        self.mode = Mode::Indicator;
        self.indicator_until = Some(now + INDICATOR_HOLD);
    }

    /// Begin expanding the pill into the search field (pill click).
    pub fn open_search(&mut self) {
        if matches!(self.mode, Mode::Field | Mode::Expanding) {
            return;
        }
        self.mode = Mode::Expanding;
        self.indicator_until = None; // cancel any pending indicator
    }

    /// Set the in-progress IME composition string (preedit). Empty clears it.
    /// Only meaningful while the field is focused.
    pub fn set_preedit(&mut self, preedit: String) {
        if self.wants_keyboard() {
            self.preedit = preedit;
        }
    }

    /// Begin collapsing the field back to the pill (close button / Esc / blur).
    pub fn close_search(&mut self) {
        if matches!(self.mode, Mode::Pill | Mode::Collapsing) {
            return;
        }
        self.mode = Mode::Collapsing;
    }

    /// Press the close affordance (× button): clear the query and collapse.
    pub fn press_close(&mut self) {
        self.query.clear();
        self.caret = 0;
        self.preedit.clear();
        self.close_search();
    }

    /// Handle one IME/typed character (only meaningful while the field is the
    /// focus). Returns `true` if it consumed the character.
    pub fn handle_char(&mut self, ch: char) -> bool {
        if !self.wants_keyboard() {
            return false;
        }
        // Ignore control characters; the app sends printable text only.
        if ch.is_control() {
            return false;
        }
        self.query.insert(self.caret, ch);
        self.caret += ch.len_utf8();
        true
    }

    /// Handle Backspace.
    pub fn handle_backspace(&mut self) {
        if !self.wants_keyboard() {
            return;
        }
        if self.caret == 0 {
            return;
        }
        // Find the previous char boundary and drop it.
        let prev = self.query[..self.caret].chars().next_back();
        if let Some(c) = prev {
            self.caret -= c.len_utf8();
            self.query
                .replace_range(self.caret..self.caret + c.len_utf8(), "");
        }
    }

    /// Handle ← (move caret left one char).
    pub fn handle_left(&mut self) {
        if !self.wants_keyboard() {
            return;
        }
        if let Some(c) = self.query[..self.caret].chars().next_back() {
            self.caret -= c.len_utf8();
        }
    }

    /// Handle → (move caret right one char).
    pub fn handle_right(&mut self) {
        if !self.wants_keyboard() {
            return;
        }
        if let Some(c) = self.query[self.caret..].chars().next() {
            self.caret += c.len_utf8();
        }
    }

    /// Handle Esc: if the field is open, close it (don't quit the app).
    /// Returns `true` if the Esc was consumed by the control.
    pub fn handle_escape(&mut self) -> bool {
        if self.wants_keyboard() && !matches!(self.mode, Mode::Collapsing) {
            self.close_search();
            true
        } else {
            false
        }
    }

    /// Advance animations + timers. Returns `true` if the control is still
    /// animating and needs more frames (so the caller keeps redrawing).
    pub fn tick(&mut self, now: Instant, dt: f32) -> bool {
        let dt = dt.max(0.0);
        // Caret blink always ticks (cheap) so it's ready when the field opens.
        // Cycle ~1.07s; on ~56% of the time (slow, calm blink).
        self.caret_phase = (self.caret_phase + dt) % CARET_BLINK_PERIOD;

        match self.mode {
            Mode::Pill => {
                // Ease content back toward the pill visual.
                self.step_expand(0.0, dt, COLLAPSE_DURATION);
                self.step_indicator(0.0, dt, INDICATOR_CROSSFADE);
                false
            }
            Mode::Indicator => {
                self.step_expand(0.0, dt, COLLAPSE_DURATION);
                self.step_indicator(1.0, dt, INDICATOR_CROSSFADE);
                // Retire to the pill when the hold elapses.
                match self.indicator_until {
                    Some(until) if now >= until => {
                        self.indicator_until = None;
                        self.mode = Mode::Pill;
                        true // keep ticking the fade-out
                    }
                    Some(_) => true, // still holding → keep redrawing for the timer
                    None => false,
                }
            }
            Mode::Expanding => {
                self.step_expand(1.0, dt, EXPAND_DURATION);
                self.step_indicator(0.0, dt, EXPAND_DURATION);
                if self.expand > 0.999 {
                    self.expand = 1.0;
                    self.mode = Mode::Field;
                    false
                } else {
                    true
                }
            }
            Mode::Field => {
                // Hold fully open. (Caret blink is handled via caret_phase.)
                self.expand = 1.0;
                self.indicator = 0.0;
                // Keep redrawing while open so the caret blinks.
                true
            }
            Mode::Collapsing => {
                self.step_expand(0.0, dt, COLLAPSE_DURATION);
                self.step_indicator(0.0, dt, COLLAPSE_DURATION);
                if self.expand < 0.001 {
                    self.expand = 0.0;
                    self.mode = Mode::Pill;
                    false
                } else {
                    true
                }
            }
        }
    }

    /// Reset the internal clock (e.g. after the app was paused). The first
    /// `tick` after this records `now` without producing a dt.
    pub fn reset_clock(&mut self) {
        self.last_time = None;
    }

    /// Advance `expand` toward `target` at a constant (linear) rate so it
    /// completes in exactly `duration` seconds. The easing curve is applied by
    /// the consumer (`resolve`), which keeps the visual morph on an iOS-style
    /// ease-out trajectory instead of an exponential tail.
    fn step_expand(&mut self, target: f32, dt: f32, duration: f32) {
        self.expand = advance_linear(self.expand, target, dt, duration);
    }

    /// Advance `indicator` toward `target` (linear; eased on consume).
    fn step_indicator(&mut self, target: f32, dt: f32, duration: f32) {
        self.indicator = advance_linear(self.indicator, target, dt, duration);
    }

    /// Resolve the geometry + active content layers for the current frame.
    ///
    /// `viewport` is `(width, height)` in physical px. `page` is the current
    /// 0-based page index, `page_count` the total. `frame_bottom` is the Y of
    /// the bottom edge of the fixed page frame, so the control can sit below
    /// it; if not known, pass the viewport height and it falls back to a
    /// fixed bottom margin.
    pub fn resolve(
        &self,
        viewport: (u32, u32),
        frame_bottom: f32,
        page: usize,
        page_count: usize,
    ) -> (ControlGeometry, Vec<ControlLayer>) {
        self.resolve_scaled(viewport, frame_bottom, page, page_count, 1.0)
    }

    pub fn resolve_scaled(
        &self,
        viewport: (u32, u32),
        frame_bottom: f32,
        page: usize,
        page_count: usize,
        scale_factor: f32,
    ) -> (ControlGeometry, Vec<ControlLayer>) {
        self.resolve_scaled_with_edit_width(
            viewport,
            frame_bottom,
            page,
            page_count,
            scale_factor,
            None,
        )
    }

    pub fn resolve_scaled_with_edit_width(
        &self,
        viewport: (u32, u32),
        frame_bottom: f32,
        page: usize,
        page_count: usize,
        scale_factor: f32,
        edit_width: Option<EditWidth>,
    ) -> (ControlGeometry, Vec<ControlLayer>) {
        resolve_scaled_with_edit_width(
            self.state(),
            viewport,
            frame_bottom,
            page,
            page_count,
            scale_factor,
            edit_width,
        )
    }

    fn state(&self) -> ControlState {
        ControlState::new(self.mode, self.expand, self.indicator)
    }

    /// Hit-test a physical-pixel point against the control's capsule, using
    /// the *current* (possibly animating) geometry. Returns `true` if the
    /// point is inside the capsule.
    pub fn hit_test(&self, viewport: (u32, u32), frame_bottom: f32, x: f32, y: f32) -> bool {
        self.hit_test_scaled(viewport, frame_bottom, x, y, 1.0)
    }

    pub fn hit_test_scaled(
        &self,
        viewport: (u32, u32),
        frame_bottom: f32,
        x: f32,
        y: f32,
        scale_factor: f32,
    ) -> bool {
        hit_test_scaled(self.state(), viewport, frame_bottom, x, y, scale_factor)
    }

    /// The geometry's left edge X, accounting for the close button hit region
    /// inside an open field. Returns `Some(x)` only when the field is open
    /// enough to show the close button.
    pub fn close_button_x(&self, viewport: (u32, u32), frame_bottom: f32) -> Option<f32> {
        self.close_button_x_scaled(viewport, frame_bottom, 1.0)
    }

    pub fn close_button_x_scaled(
        &self,
        viewport: (u32, u32),
        frame_bottom: f32,
        scale_factor: f32,
    ) -> Option<f32> {
        close_button_x_scaled(self.state(), viewport, frame_bottom, scale_factor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn bc() -> BottomControl {
        BottomControl::new()
    }

    #[test]
    fn starts_as_pill() {
        let c = bc();
        assert_eq!(c.mode, Mode::Pill);
        assert!((c.expand - 0.0).abs() < 1e-6);
    }

    #[test]
    fn page_change_arms_indicator() {
        let mut c = bc();
        c.on_page_change(Instant::now());
        assert_eq!(c.mode, Mode::Indicator);
        assert!(c.indicator_until.is_some());
    }

    #[test]
    fn indicator_retiress_after_hold() {
        let mut c = bc();
        let t0 = Instant::now();
        c.on_page_change(t0);
        // Before the hold elapses: still indicator.
        c.tick(t0, 0.0);
        assert_eq!(c.mode, Mode::Indicator);
        // After the hold: back to pill.
        c.tick(t0 + INDICATOR_HOLD + Duration::from_millis(10), 0.016);
        assert_eq!(c.mode, Mode::Pill);
    }

    #[test]
    fn page_change_ignored_while_field_open() {
        let mut c = bc();
        c.open_search();
        c.mode = Mode::Field;
        c.on_page_change(Instant::now());
        assert_eq!(c.mode, Mode::Field);
        assert!(c.indicator_until.is_none());
    }

    #[test]
    fn pill_click_expands_to_field() {
        let mut c = bc();
        c.open_search();
        assert_eq!(c.mode, Mode::Expanding);
        // Tick most of the way through.
        for _ in 0..200 {
            c.tick(Instant::now(), 1.0 / 60.0);
            if c.mode == Mode::Field {
                break;
            }
        }
        assert_eq!(c.mode, Mode::Field);
        assert!((c.expand - 1.0).abs() < 1e-3);
    }

    #[test]
    fn field_close_collapses_to_pill() {
        let mut c = bc();
        c.mode = Mode::Field;
        c.expand = 1.0;
        c.close_search();
        assert_eq!(c.mode, Mode::Collapsing);
        for _ in 0..200 {
            c.tick(Instant::now(), 1.0 / 60.0);
            if c.mode == Mode::Pill {
                break;
            }
        }
        assert_eq!(c.mode, Mode::Pill);
        assert!((c.expand - 0.0).abs() < 1e-3);
    }

    #[test]
    fn escape_closes_field() {
        let mut c = bc();
        c.mode = Mode::Field;
        assert!(c.handle_escape());
        assert_eq!(c.mode, Mode::Collapsing);
    }

    #[test]
    fn escape_does_not_affect_pill() {
        let mut c = bc();
        assert!(!c.handle_escape());
        assert_eq!(c.mode, Mode::Pill);
    }

    #[test]
    fn handle_char_appends_to_query() {
        let mut c = bc();
        c.mode = Mode::Field;
        assert!(c.handle_char('a'));
        assert!(c.handle_char('b'));
        assert_eq!(c.query, "ab");
        assert_eq!(c.caret, 2);
    }

    #[test]
    fn handle_char_ignored_in_pill_mode() {
        let mut c = bc();
        assert!(!c.handle_char('a'));
        assert_eq!(c.query, "");
    }

    #[test]
    fn backspace_removes_last_char() {
        let mut c = bc();
        c.mode = Mode::Field;
        c.handle_char('x');
        c.handle_char('y');
        c.handle_backspace();
        assert_eq!(c.query, "x");
        assert_eq!(c.caret, 1);
    }

    #[test]
    fn backspace_removes_one_unicode_scalar() {
        let mut c = bc();
        c.mode = Mode::Field;
        c.handle_char('あ');
        c.handle_char('プ');
        c.handle_char('A');
        c.handle_backspace();
        assert_eq!(c.query, "あプ");
        assert_eq!(c.caret, "あプ".len());
        c.handle_backspace();
        assert_eq!(c.query, "あ");
        assert_eq!(c.caret, "あ".len());
    }

    #[test]
    fn backspace_at_empty_is_noop() {
        let mut c = bc();
        c.mode = Mode::Field;
        c.handle_backspace();
        assert_eq!(c.query, "");
        assert_eq!(c.caret, 0);
    }

    #[test]
    fn close_button_visible_only_when_open() {
        let mut c = bc();
        assert!(c.close_button_x((1280, 800), 600.0).is_none());
        c.mode = Mode::Field;
        c.expand = 1.0;
        assert!(c.close_button_x((1280, 800), 600.0).is_some());
    }

    #[test]
    fn preedit_set_and_cleared() {
        let mut c = bc();
        // Ignored while the field is closed.
        c.set_preedit("あ".to_string());
        assert_eq!(c.preedit, "");
        // Used while the field is open.
        c.mode = Mode::Field;
        c.set_preedit("あいう".to_string());
        assert_eq!(c.preedit, "あいう");
        assert_eq!(c.query, "");
        c.set_preedit(String::new());
        assert_eq!(c.preedit, "");
    }

    #[test]
    fn press_close_clears_preedit() {
        let mut c = bc();
        c.mode = Mode::Field;
        c.query = "foo".to_string();
        c.caret = 3;
        c.preedit = "あ".to_string();
        c.press_close();
        assert_eq!(c.query, "");
        assert_eq!(c.caret, 0);
        assert_eq!(c.preedit, "");
        assert_eq!(c.mode, Mode::Collapsing);
    }

    #[test]
    fn caret_blink_period_is_slow() {
        // The blink cycle should be >= 1s (calm, not a fast flicker). Force a
        // runtime read so this isn't flagged as a const-assert.
        let period: f32 = CARET_BLINK_PERIOD;
        let one_sec: f32 = 1.0;
        assert!(period >= one_sec, "blink period {period}s is too fast");
    }

    #[test]
    fn indicator_crossfade_is_snappy() {
        // The pill ↔ indicator swap should complete in ~INDICATOR_CROSSFADE
        // seconds, much faster than the old slow cross-fade. Drive it
        // explicitly to verify the timing.
        let mut c = bc();
        let t0 = Instant::now();
        c.on_page_change(t0);
        // Tick through the fade-in at 60 Hz; it should reach ~1.0 quickly.
        let mut indicator_at_quarter = 0.0;
        for i in 1..=60 {
            let t = t0 + Duration::from_millis(i * 16);
            c.tick(t, 1.0 / 60.0);
            // Sample ~one INDICATOR_CROSSFADE (0.18s ≈ 11 frames) in.
            if i == 11 {
                indicator_at_quarter = c.indicator;
            }
            if c.indicator > 0.999 {
                break;
            }
        }
        // After one cross-fade duration (0.18s) it should be essentially done.
        assert!(
            indicator_at_quarter > 0.9,
            "indicator only reached {indicator_at_quarter} after one crossfade duration; swap too slow"
        );
    }
}
