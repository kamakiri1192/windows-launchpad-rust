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

use std::time::{Duration, Instant};

// ---- overlay instance data (mirrors shader_control.wgsl) --------------------

/// One drawable overlay element for the bottom control. Matches the WGSL
/// `@location(0..3)` instance attributes of `shader_control.wgsl`. Built by
/// [`build_overlay_instances`] from a resolved geometry + layer list.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ControlInstance {
    /// Element center in physical px.
    pub center: [f32; 2],
    /// (size/radius, alpha, extra, _pad).
    pub params: [f32; 4],
    /// RGBA tint (non-premultiplied).
    pub color: [f32; 4],
    /// (kind, a, b, c) element-specific payload.
    pub kind: [f32; 4],
}

impl ControlInstance {
    pub const ATTRIBS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];

    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<ControlInstance>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &ControlInstance::ATTRIBS,
    };
}

/// Element kind values matching `shader_control.wgsl`.
const KIND_MAGNIFIER: f32 = 0.0;
const KIND_DOT: f32 = 1.0;
const KIND_CARET: f32 = 2.0;
const KIND_CLOSE: f32 = 3.0;

// ---- tunables ---------------------------------------------------------------

/// Seconds the page indicator stays visible after a page change before
/// returning to the search pill.
pub const INDICATOR_HOLD: Duration = Duration::from_millis(1800);

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
/// Vertical gap from the bottom of the fixed page frame to the capsule.
const BOTTOM_MARGIN: f32 = 30.0;

/// Caret blink cycle length (seconds). ~1.07s is the classic text-edit blink.
const CARET_BLINK_PERIOD: f32 = 1.07;

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
#[derive(Debug, Clone, Copy)]
pub struct ControlLayer {
    pub visual: Visual,
    pub alpha: f32,
}

/// The resolved geometry + content for one frame of the control.
#[derive(Debug, Clone, Copy)]
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
        matches!(self.mode, Mode::Field | Mode::Expanding | Mode::Collapsing)
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
                self.step_indicator(0.0, dt, INDICATOR_HOLD.as_secs_f32() * 0.5);
                false
            }
            Mode::Indicator => {
                self.step_expand(0.0, dt, COLLAPSE_DURATION);
                self.step_indicator(1.0, dt, INDICATOR_HOLD.as_secs_f32() * 0.5);
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
        let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
        let center_x = vw * 0.5;
        // Sit a fixed margin below the page frame, clamped into the viewport.
        let center_y = (frame_bottom + BOTTOM_MARGIN + CAPSULE_HEIGHT * 0.5)
            .min(vh - CAPSULE_HEIGHT * 0.5 - 8.0)
            .max(CAPSULE_HEIGHT * 0.5 + 8.0);

        // Half-width interpolates from the compact pill to the wide field.
        // The progress is eased with an iOS-style ease-out curve: it shoots
        // out quickly and settles softly, which reads as "deliberate but
        // lively" rather than mechanical.
        let pill_hw = pill_half_width();
        let hw = lerp(pill_hw, FIELD_HALF_WIDTH, ease_ios_out(self.expand));
        let hh = CAPSULE_HEIGHT * 0.5;

        let geom = ControlGeometry {
            center: (center_x, center_y),
            half_size: (hw, hh),
            radius: CAPSULE_RADIUS,
            expand: self.expand,
            indicator: self.indicator,
            page,
            page_count,
        };

        // Build the active content layers. During morphs we draw both sides
        // and cross-fade; the renderer multiplies each layer's alpha.
        let mut layers = Vec::with_capacity(2);
        match self.mode {
            Mode::Pill => {
                // Mostly pill; a sliver of indicator only while fading out.
                if self.indicator > 0.01 {
                    layers.push(ControlLayer {
                        visual: Visual::PageIndicator,
                        alpha: self.indicator,
                    });
                }
                layers.push(ControlLayer {
                    visual: Visual::SearchPill,
                    alpha: 1.0 - self.indicator,
                });
            }
            Mode::Indicator => {
                layers.push(ControlLayer {
                    visual: Visual::PageIndicator,
                    alpha: self.indicator,
                });
                if self.indicator < 0.99 {
                    layers.push(ControlLayer {
                        visual: Visual::SearchPill,
                        alpha: 1.0 - self.indicator,
                    });
                }
            }
            Mode::Expanding => {
                // Field content fades in as the capsule widens.
                let a = ease_in_out(self.expand);
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
                let a = ease_in_out(self.expand);
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
    pub fn hit_test(&self, viewport: (u32, u32), frame_bottom: f32, x: f32, y: f32) -> bool {
        let (geom, _) = self.resolve(viewport, frame_bottom, 0, 0);
        let dx = (x - geom.center.0).abs();
        let dy = (y - geom.center.1).abs();
        // Cheap capsule test: inside the inner rect, or inside a cap circle.
        let hw = geom.half_size.0;
        let hh = geom.half_size.1;
        if dy > hh {
            return false;
        }
        if dx <= hw - hh {
            return true;
        }
        // Endcap circle test.
        let cx = hw - hh;
        let ex = dx - cx;
        ex * ex + dy * dy <= hh * hh
    }

    /// The geometry's left edge X, accounting for the close button hit region
    /// inside an open field. Returns `Some(x)` only when the field is open
    /// enough to show the close button.
    pub fn close_button_x(&self, viewport: (u32, u32), frame_bottom: f32) -> Option<f32> {
        if !matches!(self.mode, Mode::Field | Mode::Expanding | Mode::Collapsing) {
            return None;
        }
        let (geom, _) = self.resolve(viewport, frame_bottom, 0, 0);
        if geom.expand < 0.5 {
            return None;
        }
        Some(geom.center.0 + geom.half_size.0 - 16.0)
    }
}

// ---- overlay instance builder ----------------------------------------------

/// Ink color for the control foreground (translucent white). Tuned to read
/// clearly over the glass capsule without being harsh.
const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
/// Active (current-page) indicator dot color.
const DOT_ACTIVE: [f32; 4] = [1.0, 1.0, 1.0, 0.96];
/// Inactive indicator dot color.
const DOT_IDLE: [f32; 4] = [1.0, 1.0, 1.0, 0.40];

/// Build the procedural overlay instances for one frame of the control. The
/// text glyphs are laid out separately by the caller (via the text renderer);
/// this only emits the SDF shapes (magnifier, dots, caret, close ×).
///
/// `query_width` is the laid-out width of the current query text (0 if empty),
/// used to place the caret at the right edge of the typed text. `caret_blink`
/// is a 0..1 visibility factor for the caret this frame.
pub fn build_overlay_instances(
    geom: &ControlGeometry,
    layers: &[ControlLayer],
    query_width: f32,
    caret_blink: f32,
) -> Vec<ControlInstance> {
    let mut out = Vec::new();
    let (cx, cy) = geom.center;
    let hw = geom.half_size.0;

    for layer in layers {
        let a = layer.alpha;
        if a <= 0.01 {
            continue;
        }
        match layer.visual {
            Visual::SearchPill => {
                // Compact pill: magnifier on the left, "検索" label to its right.
                // The label text is drawn separately; here only the magnifier.
                let mag_size = 11.0;
                let mag_cx = cx - hw + mag_size + 8.0;
                out.push(ControlInstance {
                    center: [mag_cx, cy],
                    params: [mag_size, a, 0.0, 0.0],
                    color: INK,
                    kind: [KIND_MAGNIFIER, 0.0, 0.0, 0.0],
                });
            }
            Visual::PageIndicator => {
                let dots = geom.page_count.max(1);
                // Active dot is slightly larger.
                let active_r = 3.2;
                let idle_r = 2.4;
                let gap = 8.0;
                let total = dots as f32 * active_r * 2.0 + (dots.saturating_sub(1)) as f32 * gap;
                let start_x = cx - total * 0.5 + active_r;
                for i in 0..dots {
                    let is_active = i == geom.page;
                    let r = if is_active { active_r } else { idle_r };
                    out.push(ControlInstance {
                        center: [start_x + i as f32 * (active_r * 2.0 + gap), cy],
                        params: [r, a, 0.0, 0.0],
                        color: if is_active { DOT_ACTIVE } else { DOT_IDLE },
                        kind: [KIND_DOT, 0.0, 0.0, 0.0],
                    });
                }
            }
            Visual::SearchField => {
                // Magnifier at the left inside padding.
                let mag_size = 11.0;
                let mag_cx = cx - hw + mag_size + 10.0;
                out.push(ControlInstance {
                    center: [mag_cx, cy],
                    params: [mag_size, a, 0.0, 0.0],
                    color: INK,
                    kind: [KIND_MAGNIFIER, 0.0, 0.0, 0.0],
                });
                // Caret: just past the typed text (which starts after the
                // magnifier). Only when there's no close button overlap.
                if caret_blink > 0.01 {
                    let text_origin_x = mag_cx + mag_size + 6.0;
                    let caret_x = text_origin_x + query_width;
                    out.push(ControlInstance {
                        center: [caret_x, cy],
                        // (half-height, alpha, half-width, _)
                        params: [8.0, a * caret_blink, 1.0, 0.0],
                        color: INK,
                        kind: [KIND_CARET, 0.0, 0.0, 0.0],
                    });
                }
                // Close × at the right inside padding.
                let close_cx = cx + hw - 13.0;
                out.push(ControlInstance {
                    center: [close_cx, cy],
                    params: [7.0, a, 1.4, 0.0],
                    color: INK,
                    kind: [KIND_CLOSE, 0.0, 0.0, 0.0],
                });
            }
        }
    }
    out
}

/// The X origin (physical px) where the search-field query text should start,
/// relative to the control. Used by the caller to lay out the query glyphs.
pub fn field_text_origin_x(geom: &ControlGeometry) -> f32 {
    let mag_size = 11.0;
    let mag_cx = geom.center.0 - geom.half_size.0 + mag_size + 10.0;
    mag_cx + mag_size + 6.0
}

/// Build the Liquid Glass capsule shape for the control this frame, ready to
/// push into the geometry buffer. Returns `None` if the control should not be
/// drawn (e.g. fully-transparent transition).
pub fn glass_shape(geom: &ControlGeometry) -> Option<crate::liquid_glass::geometry::GlassShape> {
    // Hide the capsule entirely while it's effectively zero-width (avoids a
    // degenerate shape at startup before the first resolve).
    if geom.half_size.0 < 1.0 {
        return None;
    }
    Some(
        crate::liquid_glass::geometry::GlassShape::control_rounded_rect(
            [geom.center.0, geom.center.1],
            [geom.half_size.0 * 2.0, geom.half_size.1 * 2.0],
            geom.radius,
        ),
    )
}

// ---- free helpers -----------------------------------------------------------

/// Half-width of the compact search pill (content-aware: magnifier + label).
pub fn pill_half_width() -> f32 {
    // Magnifier (~16px) + gap + 2-char label "検索" (~28px) + side padding.
    46.0
}

/// Linear advancement: moves `v` toward `target` at a constant rate so it
/// completes in exactly `duration` seconds (frame-rate independent). The
/// easing curve is applied by the consumer, which lets `resolve` shape the
/// visual morph with an iOS-style ease-out rather than an exponential tail.
fn advance_linear(v: f32, target: f32, dt: f32, duration: f32) -> f32 {
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

/// Cubic ease-in-out, symmetric S-curve. Used for content cross-fades so they
/// ramp gently rather than cutting in.
fn ease_in_out(t: f32) -> f32 {
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
fn ease_ios_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    // cubic-bezier(0.32, 0.72, 0, 1) — evaluate via Newton iteration on the
    // parametric cubic. x(t)=3(1-t)^2 t·0.32 + 3(1-t)t^2·0 + t^3, then y(t)=...
    // Cheaper + exact enough: sample the curve with a few Newton steps.
    cubic_bezier_y(0.32, 0.72, 0.0, 1.0, t)
}

/// Evaluate y(x) of a CSS cubic-bezier(p1x,p1y,p2x,p2y) easing curve at a
/// given progress `x` ∈ [0,1]. Solves the parametric x(s) for s then returns
/// y(s). Uses Newton-Raphson with a bisection fallback.
fn cubic_bezier_y(p1x: f32, p1y: f32, p2x: f32, p2y: f32, x: f32) -> f32 {
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

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn resolve_pill_geometry_is_centered() {
        let c = bc();
        let (g, layers) = c.resolve((1280, 800), 600.0, 0, 3);
        assert!((g.center.0 - 640.0).abs() < 1e-3, "centered horizontally");
        assert!((g.half_size.1 - CAPSULE_HEIGHT * 0.5).abs() < 1e-3);
        // Default pill shows one SearchPill layer.
        assert!(layers.iter().any(|l| l.visual == Visual::SearchPill));
    }

    #[test]
    fn resolve_field_geometry_is_wide() {
        let mut c = bc();
        c.mode = Mode::Field;
        c.expand = 1.0;
        let (g, _) = c.resolve((1280, 800), 600.0, 0, 3);
        assert!((g.half_size.0 - FIELD_HALF_WIDTH).abs() < 1e-2);
    }

    #[test]
    fn resolve_indicator_has_dots() {
        let mut c = bc();
        c.mode = Mode::Indicator;
        c.indicator = 1.0;
        let (g, layers) = c.resolve((1280, 800), 600.0, 1, 4);
        assert_eq!(g.page, 1);
        assert_eq!(g.page_count, 4);
        assert!(layers.iter().any(|l| l.visual == Visual::PageIndicator));
    }

    #[test]
    fn hit_test_inside_capsule() {
        let c = bc();
        let (g, _) = c.resolve((1280, 800), 600.0, 0, 1);
        // Center point.
        assert!(c.hit_test((1280, 800), 600.0, g.center.0, g.center.1));
        // Well outside.
        assert!(!c.hit_test((1280, 800), 600.0, 10.0, 10.0));
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
    fn preedit_set_and_cleared() {
        let mut c = bc();
        // Ignored while the field is closed.
        c.set_preedit("あ".to_string());
        assert_eq!(c.preedit, "");
        // Used while the field is open.
        c.mode = Mode::Field;
        c.set_preedit("あいう".to_string());
        assert_eq!(c.preedit, "あいう");
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
}
