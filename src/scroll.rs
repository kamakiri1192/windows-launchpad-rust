//! 1D horizontal scroll with iOS/macOS-style physics.
//!
//! The state machine drives a single value `position` (in physical pixels)
//! through four phases:
//!
//! - [`Phase::Idle`]: nothing to do, no redraw requested.
//! - [`Phase::Dragging`]: follows the pointer 1:1, applies a rubber-band
//!   resistance past the bounds, and records recent samples to estimate the
//!   flick velocity at release.
//! - [`Phase::Inertial`]: free exponential coasting. Used for continuous
//!   scroll surfaces; paging does not pass through here.
//! - [`Phase::Settling`]: an under-damped spring glides `position` to the
//!   chosen page boundary. For a flick, the spring is launched with the
//!   release velocity so the page carries its momentum into a smooth glide
//!   (the iOS "glide to the page" feel); for a soft return it eases from
//!   rest, and overshoot of a content bound gives the iOS "bounce".
//!
//! Integration is semi-implicit Euler with adaptive substepping so the feel
//! is identical at 60/120/144 Hz (and after a stutter). The model is written
//! axis-generically; swapping to vertical paging later only means renaming
//! the axis.

use std::time::Instant;

/// Target page the content should rest on. `position` is the *content origin*,
/// so larger values scroll the viewport to the right. Page `n` rests at
/// `position = n * page_extent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Dragging,
    Inertial,
    Settling,
}

/// Bounds for the scrollable content, in physical pixels.
#[derive(Debug, Clone, Copy)]
pub struct ScrollBounds {
    /// Extent (width for horizontal) of one page == one content/panel width.
    /// Set by the layout to the liquid-glass page-frame width (narrower than
    /// the full viewport), so a page flip costs a proportionally smaller drag
    /// and the rubber-band feel scales with the page rather than the window.
    pub page_extent: f32,
    pub page_count: usize,
}

impl ScrollBounds {
    /// Minimum content position (fully scrolled to the last page).
    /// Note: sign convention — we scroll *negative* to move right, so min is
    /// the last page and max is the first page (0).
    #[inline]
    pub fn min_pos(&self) -> f32 {
        -((self.page_count.saturating_sub(1) as f32) * self.page_extent)
    }

    #[inline]
    pub fn max_pos(&self) -> f32 {
        0.0
    }

    /// Nearest page boundary to `pos` (clamped to the valid range).
    #[inline]
    pub fn snap_target(&self, pos: f32) -> f32 {
        let p = (pos / self.page_extent).round();
        let p = p.clamp(-((self.page_count.saturating_sub(1)) as f32), 0.0);
        p * self.page_extent
    }

    /// Pick the page a paging flick should settle on, given the gesture's
    /// start page (`from`, already snapped) and the release position+velocity.
    ///
    /// iOS-style paging: at most one page from `from`, in the direction of
    /// motion. A decisive flick (past the midpoint, or strong velocity) flips
    /// to the adjacent page; a weak flick returns to the start page.
    ///
    /// Sign note: `position` decreases toward later pages, so a *negative*
    /// velocity means scrolling to the *next* page.
    pub fn paging_target(&self, from: f32, pos: f32, velocity: f32) -> f32 {
        let delta = pos - from; // signed displacement during the drag
        let page = self.page_extent;
        // Flip only when the content moved past half a page, or the flick is
        // energetic enough to clearly intend a page change. The velocity
        // threshold (~0.4 px/ms ≈ one page in ~2.5 s) is intentionally low:
        // even a modest flick should carry the page over.
        let crossed_midpoint = delta.abs() > page * 0.5;
        let energetic = velocity.abs() > 400.0;

        if !crossed_midpoint && !energetic {
            return from.clamp(self.min_pos(), self.max_pos());
        }

        // Sign convention: `position` *decreases* toward later pages, so a
        // negative velocity/displacement means "next page" (subtract a page).
        // Pick the motion direction from whichever signal is meaningful; a
        // real flick trusts the velocity sign, otherwise use the drag delta.
        let motion = if velocity.abs() > 50.0 {
            velocity
        } else {
            delta
        };
        let target = if motion < 0.0 {
            from - page // next page
        } else {
            from + page // previous page
        };
        target.clamp(self.min_pos(), self.max_pos())
    }
}

/// Recent pointer samples used to estimate the release velocity.
/// We keep a short ring of `(time, pos)` deltas.
const VEL_SAMPLES: usize = 4;

/// Spring / inertia tunables. Defaults mimic an iOS Launchpad page swipe.
#[derive(Debug, Clone, Copy)]
pub struct PhysicsConfig {
    /// Rubber-band stiffness divisor. Smaller = stiffer rubber.
    /// UIScrollView rubber-band constant. Apple uses `c = 0.55`.
    pub rubber_c: f32,
    /// Dimension (width for horizontal) of the viewport in physical px. The
    /// rubber-band displacement asymptotes to this value.
    pub rubber_dimension: f32,
    /// Exponential decay factor per second for inertial coasting.
    pub inertia_decay: f32,
    /// Inertial velocity below which we cut to spring settling (px/s).
    pub inertia_cutoff: f32,
    /// Spring angular frequency ω₀ (rad/s). Higher = snappier.
    pub spring_omega: f32,
    /// Damping ratio ζ. <1 under-damped (bouncy), =1 critical, >1 over.
    pub spring_zeta: f32,
    /// Below this speed & distance we consider the spring settled.
    pub settle_eps: f32,
    /// Maximum frame dt before we subdivide (s).
    pub max_dt: f32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            // Apple's rubber-band constant. The viewport dimension is set by
            // the caller via `set_rubber_dimension()` (default 1000 px).
            rubber_c: 0.55,
            rubber_dimension: 1000.0,
            inertia_decay: 3.2,
            inertia_cutoff: 18.0,
            // ω₀ ≈ 2π·f, f≈3.2 Hz → ω₀≈20. ζ≈0.80 gives a gentle bounce.
            spring_omega: 22.0,
            spring_zeta: 0.82,
            settle_eps: 0.35,
            max_dt: 1.0 / 60.0,
        }
    }
}

pub struct Scroller {
    pub position: f32,
    pub velocity: f32,
    pub phase: Phase,
    pub cfg: PhysicsConfig,
    bounds: ScrollBounds,
    /// Content position captured at drag start.
    drag_anchor: f32,
    /// Snapped page the gesture started on. Inertia is limited to at most one
    /// page away from this (iOS-style paging), so a single flick can never jump
    /// multiple pages regardless of release speed.
    gesture_start_snap: f32,
    /// Pointer position (physical px) captured at drag start.
    drag_start_pointer: f32,
    /// Pointer history for velocity estimation: (seconds since epoch-ish, pos).
    samples: [(f32, f32); VEL_SAMPLES],
    sample_count: usize,
    /// Target position the spring settles toward.
    settle_target: f32,
    /// True when settling toward a *new* page driven by a flick. The spring
    /// keeps the release velocity at launch so the page glides to its target
    /// (iOS feel) instead of easing from a standstill. False for a soft
    /// return-to-current-page, where we ease cleanly from rest.
    settle_flick: bool,
    /// Last clock reading for dt, in seconds.
    last_time: Option<Instant>,
    /// Monotonic clock origin (so we can store f32 sample times without overflow).
    clock_origin: Instant,
}

impl Scroller {
    pub fn new(bounds: ScrollBounds) -> Self {
        let clock_origin = Instant::now();
        Self {
            position: 0.0,
            velocity: 0.0,
            phase: Phase::Idle,
            cfg: PhysicsConfig::default(),
            bounds,
            drag_anchor: 0.0,
            gesture_start_snap: 0.0,
            drag_start_pointer: 0.0,
            samples: [(0.0, 0.0); VEL_SAMPLES],
            sample_count: 0,
            settle_target: 0.0,
            settle_flick: false,
            last_time: None,
            clock_origin,
        }
    }

    pub fn set_bounds(&mut self, bounds: ScrollBounds) {
        self.bounds = bounds;
        // The rubber-band dimension tracks the content (page) extent so the
        // overshoot feel scales with the page width, exactly like iOS.
        self.cfg.rubber_dimension = bounds.page_extent;
        // Re-clamp current position into the new range and re-snap if idle.
        let clamped = self
            .position
            .clamp(self.bounds.min_pos(), self.bounds.max_pos());
        if clamped != self.position {
            self.position = clamped;
        }
        if self.phase == Phase::Idle {
            self.position = self.bounds.snap_target(self.position);
        }
    }

    /// Begin a drag gesture from the current pointer position.
    ///
    /// From here on the content follows the pointer 1:1 ("direct
    /// manipulation", like grabbing the page with your finger). Moving the
    /// pointer right moves the content right (reveals the previous page);
    /// moving left reveals the next page.
    pub fn drag_start(&mut self, pointer_x: f32) {
        self.phase = Phase::Dragging;
        self.drag_anchor = self.position;
        // Remember the page we started on (rounded to a boundary). Inertia is
        // later clamped to at most one page away from here, so a single flick
        // can never jump multiple pages — iOS home-screen paging.
        self.gesture_start_snap = self.bounds.snap_target(self.position);
        self.drag_start_pointer = pointer_x;
        self.velocity = 0.0;
        self.sample_count = 0;
        self.last_time = None;
    }

    /// Update the drag with the latest pointer position.
    pub fn drag_move(&mut self, pointer_x: f32) {
        if self.phase != Phase::Dragging {
            return;
        }
        // Direct manipulation: content offset tracks pointer displacement.
        let raw = self.drag_anchor + (pointer_x - self.drag_start_pointer);
        // Apply rubber-band resistance outside [min,max].
        let pos = self.clamp_with_rubber(raw);
        let prev = self.position;
        self.position = pos;
        self.push_sample(pos, prev);
    }

    /// End the drag and snap to a page, iOS-style: at most one page from the
    /// gesture's start, in the flick direction.
    ///
    /// We *decide the target page immediately* from the release velocity and
    /// how far the content was dragged, then glide there with a spring. The
    /// release velocity is preserved as the spring's initial velocity so a
    /// flick carries its momentum into the landing glide (this is what gives
    /// iOS its "glide to the page" feel), instead of clamping mid-coast. For a
    /// soft return to the current page (no real flick), we drop the velocity
    /// and ease back cleanly from rest.
    pub fn drag_end(&mut self) {
        if self.phase != Phase::Dragging {
            return;
        }
        let v = self.estimate_velocity();
        let target = self
            .bounds
            .paging_target(self.gesture_start_snap, self.position, v);
        let is_flick = (target - self.gesture_start_snap).abs() > 1.0 && v.abs() > 50.0;

        // Cap the carried velocity so a violent flick doesn't blow past the
        // one-page target in the first substep. Roughly one page over ~120 ms.
        let max_v = self.bounds.page_extent * 8.0;
        self.velocity = v.clamp(-max_v, max_v);

        if is_flick {
            // Keep velocity: the spring launches with the flick's momentum.
            self.begin_settle_to(target, true);
        } else {
            // Soft return to the current page: ease from rest.
            self.velocity = 0.0;
            self.begin_settle_to(target, false);
        }
    }

    /// Advance the simulation by real elapsed time. Returns the new phase.
    pub fn tick(&mut self, now: Instant) -> Phase {
        let dt = match self.last_time {
            None => {
                self.last_time = Some(now);
                return self.phase;
            }
            Some(t) => {
                let d = now.duration_since(t).as_secs_f32();
                self.last_time = Some(now);
                d.min(0.1) // clamp huge stalls to 100ms
            }
        };
        if dt <= 0.0 {
            return self.phase;
        }

        // Substep so integration is frame-rate independent.
        let mut remaining = dt;
        while remaining > 0.0 {
            let step = remaining.min(self.cfg.max_dt);
            remaining -= step;
            self.step_once(step);
            if self.phase == Phase::Idle {
                break;
            }
        }
        self.phase
    }

    /// True while content is moving — the main loop should keep redrawing.
    pub fn is_animating(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Programmatically glide to a page boundary. Used by edit-mode
    /// drag-to-reorder when the lifted icon is held near a page edge.
    ///
    /// Returns `true` when a new settle animation was started.
    pub fn settle_to_page(&mut self, page: usize) -> bool {
        let max_page = self.bounds.page_count.saturating_sub(1);
        let page = page.min(max_page);
        let target = -(page as f32) * self.bounds.page_extent;
        if self.phase == Phase::Idle && (self.position - target).abs() < self.cfg.settle_eps {
            return false;
        }
        if self.phase == Phase::Settling
            && (self.settle_target - target).abs() < self.cfg.settle_eps
        {
            return false;
        }
        self.velocity = 0.0;
        self.last_time = None;
        self.begin_settle_to(
            target.clamp(self.bounds.min_pos(), self.bounds.max_pos()),
            false,
        );
        true
    }

    /// Reset the timer used for dt (call when the app resumes after a pause).
    #[allow(dead_code)]
    pub fn reset_clock(&mut self) {
        self.last_time = None;
    }

    // ---- internals -------------------------------------------------------

    fn step_once(&mut self, dt: f32) {
        match self.phase {
            Phase::Idle | Phase::Dragging => {
                // Position is driven directly by pointer events; nothing to
                // integrate here. We just keep the clock warm.
            }
            Phase::Inertial => {
                // Free exponential coasting: v *= exp(-k·dt). This phase is not
                // used for paging (paging decides its target in `drag_end` and
                // goes straight to `Settling`), but is kept for future
                // continuous-scroll surfaces. While coasting we hand off to the
                // spring when we overshoot a bound or stall.
                let decay = (-self.cfg.inertia_decay * dt).exp();
                self.velocity *= decay;
                self.position += self.velocity * dt;

                let min = self.bounds.min_pos();
                let max = self.bounds.max_pos();
                let overshot = self.position < min || self.position > max;
                let stalled = self.velocity.abs() < self.cfg.inertia_cutoff;
                if overshot || stalled {
                    self.begin_settle_to(self.bounds.snap_target(self.position), false);
                }
            }
            Phase::Settling => {
                // Semi-implicit Euler on the spring ODE:
                //   a = -ω₀²·(x - target) - 2·ζ·ω₀·v
                let dx = self.position - self.settle_target;
                let acc = -self.cfg.spring_omega * self.cfg.spring_omega * dx
                    - 2.0 * self.cfg.spring_zeta * self.cfg.spring_omega * self.velocity;
                self.velocity += acc * dt;
                self.position += self.velocity * dt;

                if dx.abs() < self.cfg.settle_eps && self.velocity.abs() < self.cfg.settle_eps {
                    self.position = self.settle_target;
                    self.velocity = 0.0;
                    self.phase = Phase::Idle;
                }
            }
        }
    }

    fn begin_settle_to(&mut self, target: f32, flick: bool) {
        self.settle_target = target;
        self.settle_flick = flick;
        self.phase = Phase::Settling;
    }

    /// Clamp `raw` to bounds, but apply a soft rubber-band curve past the ends
    /// so it asymptotes instead of hard-stopping.
    fn clamp_with_rubber(&self, raw: f32) -> f32 {
        let min = self.bounds.min_pos();
        let max = self.bounds.max_pos();
        if raw > max {
            let over = raw - max;
            max + self.rubber(over)
        } else if raw < min {
            let over = min - raw;
            min - self.rubber(over)
        } else {
            raw
        }
    }

    /// Apple's rubber-band curve (reverse-engineered from UIScrollView):
    ///
    /// ```text
    /// B(x) = (1 - 1 / (x · c / d + 1)) · d
    /// ```
    ///
    /// where `c` is [`PhysicsConfig::rubber_c`] (Apple: 0.55) and `d` is
    /// [`PhysicsConfig::rubber_dimension`] (the viewport extent). It has the
    /// diminishing-returns property: the further you pull, the more each
    /// additional pixel of input is resisted (`B(x)/x → 0`), and the visible
    /// overshoot asymptotes to `d`.
    #[inline]
    fn rubber(&self, x: f32) -> f32 {
        let c = self.cfg.rubber_c;
        let d = self.cfg.rubber_dimension;
        (1.0 - 1.0 / (x * c / d + 1.0)) * d
    }

    fn push_sample(&mut self, pos: f32, _prev: f32) {
        let t = self.clock_origin.elapsed().as_secs_f32();
        // Shift the ring left and append.
        for i in 0..(VEL_SAMPLES - 1) {
            self.samples[i] = self.samples[i + 1];
        }
        self.samples[VEL_SAMPLES - 1] = (t, pos);
        if self.sample_count < VEL_SAMPLES {
            self.sample_count += 1;
        }
    }

    /// Estimate current velocity from the last ~80ms of samples.
    fn estimate_velocity(&self) -> f32 {
        if self.sample_count < 2 {
            return 0.0;
        }
        let last = self.samples[VEL_SAMPLES - 1];
        // Walk back to find a sample at least 16ms older but within ~120ms.
        let mut chosen = last;
        for i in (0..VEL_SAMPLES - 1).rev() {
            let s = self.samples[i];
            let dt = last.0 - s.0;
            if dt >= 0.016 {
                chosen = s;
                if dt <= 0.12 {
                    break;
                }
            }
        }
        let dt = last.0 - chosen.0;
        if dt < 1e-4 {
            return 0.0;
        }
        (last.1 - chosen.1) / dt
    }
}

// ---- Generic spring (reused from the scroller's Settling ODE) ---------------

/// A critically/under-damped 1D spring, useful for animating a single scalar
/// toward a target with an iOS-like glide. The integration is the same semi-
/// implicit Euler step the scroller uses in [`Phase::Settling`], so the feel
/// matches the page-snap motion.
#[derive(Debug, Clone, Copy)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
}

impl Spring {
    pub fn at(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
        }
    }

    /// Snap instantly to `target` (no animation).
    pub fn snap_to(&mut self, target: f32) {
        self.target = target;
        self.value = target;
        self.velocity = 0.0;
    }

    /// Set a new target the spring glides toward from its current value.
    pub fn glide_to(&mut self, target: f32) {
        self.target = target;
    }

    /// True once the spring has come to rest at its target.
    pub fn settled(&self, cfg: &PhysicsConfig) -> bool {
        (self.value - self.target).abs() < cfg.settle_eps && self.velocity.abs() < cfg.settle_eps
    }

    /// Advance one step. Returns `true` while still animating.
    pub fn step(&mut self, dt: f32, cfg: &PhysicsConfig) -> bool {
        let dx = self.value - self.target;
        let acc = -cfg.spring_omega * cfg.spring_omega * dx
            - 2.0 * cfg.spring_zeta * cfg.spring_omega * self.velocity;
        self.velocity += acc * dt;
        self.value += self.velocity * dt;
        if self.settled(cfg) {
            self.value = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }
}

/// A 2D spring (two independent [`Spring`]s on x and y). Convenient for
/// animating a point — e.g. a tile's offset as it slides to a new cell during a
/// drag-to-reorder.
#[derive(Debug, Clone, Copy)]
pub struct Spring2 {
    pub x: Spring,
    pub y: Spring,
}

impl Spring2 {
    pub fn at(x: f32, y: f32) -> Self {
        Self {
            x: Spring::at(x),
            y: Spring::at(y),
        }
    }

    pub fn glide_to(&mut self, x: f32, y: f32) {
        self.x.glide_to(x);
        self.y.glide_to(y);
    }

    pub fn snap_to(&mut self, x: f32, y: f32) {
        self.x.snap_to(x);
        self.y.snap_to(y);
    }

    /// Advance both axes. Returns `true` while either is still animating.
    pub fn step(&mut self, dt: f32, cfg: &PhysicsConfig) -> bool {
        let a = self.x.step(dt, cfg);
        let b = self.y.step(dt, cfg);
        a || b
    }

    pub fn settled(&self, cfg: &PhysicsConfig) -> bool {
        self.x.settled(cfg) && self.y.settled(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(pages: usize) -> ScrollBounds {
        ScrollBounds {
            page_extent: 1000.0,
            page_count: pages,
        }
    }

    #[test]
    fn snap_targets_page_boundaries() {
        let b = bounds(3);
        assert_eq!(b.snap_target(0.0), 0.0);
        assert_eq!(b.snap_target(-499.0), 0.0);
        assert_eq!(b.snap_target(-501.0), -1000.0);
        assert_eq!(b.snap_target(-1499.0), -1000.0);
        assert_eq!(b.snap_target(-1501.0), -2000.0);
        assert_eq!(b.snap_target(-99999.0), -2000.0); // clamped
    }

    #[test]
    fn rubber_is_sublinear_and_zero_at_origin() {
        // Scroller's default rubber_dimension tracks page_extent when
        // constructed via new(); set it explicitly to be safe.
        let mut s = Scroller::new(bounds(2));
        s.set_bounds(bounds(2)); // ensures rubber_dimension = 1000
        assert_eq!(s.rubber(0.0), 0.0, "no overshoot → no displacement");

        // The visible overshoot is always smaller than the requested pull.
        let r50 = s.rubber(50.0);
        let r500 = s.rubber(500.0);
        assert!(r50 < 50.0, "rubber attenuates small overshoot");
        assert!(r500 < 500.0, "rubber attenuates large overshoot");

        // Diminishing returns: per-pixel responsiveness drops as the pull
        // grows (B(x)/x is monotonically decreasing).
        assert!(
            r500 / 500.0 < r50 / 50.0,
            "larger pull must feel stiffer per pixel"
        );

        // The displacement is bounded above by the viewport dimension (d).
        assert!(
            s.rubber(100_000.0) <= s.cfg.rubber_dimension + 1.0,
            "overshoot asymptotes to the viewport dimension"
        );
    }

    #[test]
    fn drag_move_is_during_dragging_only() {
        // Start on page 1 (position = -1000) so a left drag stays in-range
        // and isn't attenuated by the rubber band.
        let mut s = Scroller::new(bounds(3));
        s.position = -1000.0;
        s.drag_start(500.0);
        s.drag_move(450.0); // pointer -50 → content follows -50 (next page)
        assert!((s.position - (-1050.0)).abs() < 1e-3);
    }

    #[test]
    fn drag_direction_is_direct_manipulation() {
        // Moving the pointer RIGHT must move the content RIGHT (positive),
        // not reveal the next page. This is the iOS "grab and drag" feel.
        let mut s = Scroller::new(bounds(3));
        s.drag_start(200.0);
        s.drag_move(250.0); // +50 to the right
        assert!(s.position > 0.0, "right drag must move content right");
        // And left drag must move content left (negative) → next page.
        let mut s = Scroller::new(bounds(3));
        s.position = -1000.0; // start on page 1 so we have room both ways
        s.drag_start(500.0);
        s.drag_move(450.0); // -50 to the left
        assert!(s.position < -1000.0, "left drag must move content left");
    }

    #[test]
    fn settling_reaches_target() {
        let mut s = Scroller::new(bounds(2));
        s.cfg.spring_omega = 30.0;
        s.position = -1234.0;
        s.begin_settle_to(-1000.0, false);
        // Step many times to converge.
        for _ in 0..2000 {
            s.step_once(1.0 / 120.0);
            if s.phase == Phase::Idle {
                break;
            }
        }
        assert_eq!(s.phase, Phase::Idle);
        assert!((s.position - (-1000.0)).abs() < s.cfg.settle_eps);
    }

    // ---- paging_target: pure page-selection logic ------------------------

    #[test]
    fn paging_target_strong_flick_advances_one_page() {
        // Start on page 2 (-2000). A strong flick toward the next page
        // (negative velocity) must target page 3 (-3000), exactly one ahead.
        let b = bounds(4);
        assert_eq!(b.paging_target(-2000.0, -2000.0, -5000.0), -3000.0);
    }

    #[test]
    fn paging_target_strong_flick_backward_one_page() {
        // Start on page 2, strong flick toward previous page → page 1 (-1000).
        let b = bounds(4);
        assert_eq!(b.paging_target(-2000.0, -2000.0, 5000.0), -1000.0);
    }

    #[test]
    fn paging_target_dragged_past_midpoint_flips() {
        // Even with zero velocity, dragging past half a page flips to the
        // adjacent page in the drag direction.
        let b = bounds(4);
        // Start page 2, dragged 0.6 page toward next → page 3.
        assert_eq!(b.paging_target(-2000.0, -2600.0, 0.0), -3000.0);
        // Start page 2, dragged 0.6 page toward previous → page 1.
        assert_eq!(b.paging_target(-2000.0, -1400.0, 0.0), -1000.0);
    }

    #[test]
    fn paging_target_small_drag_no_flick_returns_to_start() {
        // A small drag that doesn't cross the midpoint, with no real flick,
        // must return to the start page.
        let b = bounds(4);
        assert_eq!(b.paging_target(-2000.0, -2100.0, 0.0), -2000.0);
    }

    #[test]
    fn paging_target_never_jumps_more_than_one_page() {
        // Even a violent flick can only reach one page away — never two.
        let b = bounds(4);
        assert_eq!(b.paging_target(-2000.0, -2000.0, -500_000.0), -3000.0);
        assert_eq!(b.paging_target(-2000.0, -2000.0, 500_000.0), -1000.0);
    }

    #[test]
    fn paging_target_clamps_at_content_bounds() {
        // At the first page (0), a next-page flick targets page 1, not beyond.
        let b = bounds(4);
        assert_eq!(b.paging_target(0.0, 0.0, -50_000.0), -1000.0);
        // At the last page (-3000), a prev-page flick targets page 2.
        assert_eq!(b.paging_target(-3000.0, -3000.0, 50_000.0), -2000.0);
        // A prev-page flick at the first page stays put (already at bound).
        assert_eq!(b.paging_target(0.0, 0.0, 50_000.0), 0.0);
    }

    // ---- drag_end integration: decide target + glide via spring ----------

    /// Run a paging flick end-to-end: start on `start_pos`, fake the release
    /// velocity via the sample ring, call `drag_end`, then integrate the
    /// resulting `Settling` phase to idle. Returns `(resting_position, eps)`.
    fn run_flick(mut s: Scroller, start_pos: f32, release_velocity: f32) -> (f32, f32) {
        s.position = start_pos;
        s.drag_start(0.0);
        // Fake two samples ~20 ms apart so estimate_velocity returns the
        // intended release velocity (delta_pos / 0.02).
        let p0 = start_pos;
        let p1 = start_pos + release_velocity * 0.02;
        s.samples = [(0.0, p0), (0.0, p0), (0.0, p0), (0.02, p1)];
        s.sample_count = VEL_SAMPLES;
        s.drag_end();
        assert_eq!(
            s.phase,
            Phase::Settling,
            "drag_end should go straight to Settling for paging"
        );
        for _ in 0..10_000 {
            s.step_once(1.0 / 120.0);
            if s.phase == Phase::Idle {
                break;
            }
        }
        (s.position, s.cfg.settle_eps)
    }

    #[test]
    fn drag_end_strong_flick_lands_one_page_ahead() {
        // Page 2 → strong next-page flick → page 3 (-3000), never further.
        let s = Scroller::new(bounds(4));
        let (rest, eps) = run_flick(s, -2000.0, -5000.0);
        assert!(
            (-rest - 3000.0).abs() < eps,
            "strong next-page flick should land on page 3 (-3000), got {rest}"
        );
    }

    #[test]
    fn drag_end_strong_flick_lands_one_page_back() {
        // Page 2 → strong prev-page flick → page 1 (-1000).
        let s = Scroller::new(bounds(4));
        let (rest, eps) = run_flick(s, -2000.0, 5000.0);
        assert!(
            (-rest - 1000.0).abs() < eps,
            "strong prev-page flick should land on page 1 (-1000), got {rest}"
        );
    }

    #[test]
    fn drag_end_small_drag_returns_to_start_page() {
        // A small drag (well under half a page) with a weak release settles
        // back on the start page.
        let s = Scroller::new(bounds(4));
        let (rest, eps) = run_flick(s, -2000.0, -300.0);
        assert!(
            (-rest - 2000.0).abs() < eps,
            "small drag should return to start page (-2000), got {rest}"
        );
    }

    #[test]
    fn drag_end_flick_carries_velocity_into_settle() {
        // A flick must preserve release velocity as the spring's initial
        // velocity (the "glide" feel), so the content keeps moving the instant
        // after release rather than easing from rest.
        let mut s = Scroller::new(bounds(4));
        s.position = -1000.0;
        s.drag_start(0.0);
        let p0 = -1000.0;
        let p1 = -1000.0 + (-4000.0) * 0.02;
        s.samples = [(0.0, p0), (0.0, p0), (0.0, p0), (0.02, p1)];
        s.sample_count = VEL_SAMPLES;
        s.drag_end();
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.settle_target, -2000.0);
        assert!(
            s.velocity < -100.0,
            "flick should keep momentum into Settling, got v={}",
            s.velocity
        );
    }

    #[test]
    fn drag_end_soft_return_drops_velocity() {
        // A soft return to the current page (no real flick) should drop the
        // velocity and ease from rest, not launch with residual momentum.
        let mut s = Scroller::new(bounds(4));
        s.position = -1050.0; // barely off the start page (-1000)
        s.drag_start(0.0);
        let p0 = -1050.0;
        let p1 = -1050.0 + (-30.0) * 0.02; // weak, below the flick threshold
        s.samples = [(0.0, p0), (0.0, p0), (0.0, p0), (0.02, p1)];
        s.sample_count = VEL_SAMPLES;
        s.drag_end();
        assert_eq!(s.phase, Phase::Settling);
        assert!(
            s.velocity.abs() < 1.0,
            "soft return should start from rest, got v={}",
            s.velocity
        );
    }

    #[test]
    fn drag_end_caps_release_velocity() {
        // An unrealistically large estimated velocity must be clamped to
        // 8×page_extent/s so a violent flick can't blow past the one-page
        // target in a single substep.
        let mut s = Scroller::new(bounds(4));
        s.position = -1000.0;
        s.drag_start(0.0);
        s.samples = [
            (0.0, -1000.0),
            (0.0, -1000.0),
            (0.02, -1000.0),
            (0.02, -5000.0),
        ];
        s.sample_count = VEL_SAMPLES;
        s.drag_end();
        let max_v = 1000.0 * 8.0;
        assert!(
            s.velocity.abs() <= max_v + 1e-3,
            "release velocity must be clamped to ±{max_v}, got {}",
            s.velocity
        );
        assert_eq!(s.phase, Phase::Settling);
    }

    #[test]
    fn settle_to_page_starts_programmatic_page_glide() {
        let mut s = Scroller::new(bounds(4));
        assert!(s.settle_to_page(2));
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.settle_target, -2000.0);
        assert_eq!(s.velocity, 0.0);
    }

    #[test]
    fn settle_to_page_is_noop_when_already_on_target() {
        let mut s = Scroller::new(bounds(4));
        s.position = -1000.0;
        assert!(!s.settle_to_page(1));
        assert_eq!(s.phase, Phase::Idle);
    }

    // ---- generic Spring ----

    #[test]
    fn spring_glides_to_target_and_settles() {
        let cfg = PhysicsConfig::default();
        let mut s = Spring::at(0.0);
        s.glide_to(100.0);
        let mut animating = true;
        for _ in 0..2000 {
            animating = s.step(1.0 / 120.0, &cfg);
            if !animating {
                break;
            }
        }
        assert!(!animating, "spring must come to rest");
        assert!((s.value - 100.0).abs() < cfg.settle_eps);
    }

    #[test]
    fn spring_snap_instantly_reaches_target() {
        let cfg = PhysicsConfig::default();
        let mut s = Spring::at(0.0);
        s.snap_to(50.0);
        assert!(
            !s.step(1.0 / 120.0, &cfg),
            "snapped spring is already settled"
        );
        assert_eq!(s.value, 50.0);
    }

    #[test]
    fn spring2_advances_both_axes() {
        let cfg = PhysicsConfig::default();
        let mut s = Spring2::at(0.0, 10.0);
        s.glide_to(20.0, 30.0);
        let mut animating = true;
        for _ in 0..4000 {
            animating = s.step(1.0 / 120.0, &cfg);
            if !animating {
                break;
            }
        }
        assert!(!animating);
        assert!((s.x.value - 20.0).abs() < cfg.settle_eps);
        assert!((s.y.value - 30.0).abs() < cfg.settle_eps);
    }
}
