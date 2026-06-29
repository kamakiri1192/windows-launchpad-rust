//! 1D horizontal scroll with iOS/macOS-style physics.
//!
//! The state machine drives a single value `position` (in physical pixels)
//! through four phases:
//!
//! - [`Phase::Idle`]: nothing to do, no redraw requested.
//! - [`Phase::Dragging`]: follows the pointer 1:1, applies a rubber-band
//!   resistance past the bounds, and records recent samples to estimate the
//!   flick velocity at release.
//! - [`Phase::Inertial`]: coasts with exponential velocity decay until a
//!   bound is hit, then hands off to [`Phase::Settling`].
//! - [`Phase::Settling`]: a slightly under-damped spring snaps `position` to
//!   the nearest page boundary. This is the source of the iOS "bounce".
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
    /// Pointer position (physical px) captured at drag start.
    drag_start_pointer: f32,
    /// Pointer history for velocity estimation: (seconds since epoch-ish, pos).
    samples: [(f32, f32); VEL_SAMPLES],
    sample_count: usize,
    /// Target position the spring settles toward.
    settle_target: f32,
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
            drag_start_pointer: 0.0,
            samples: [(0.0, 0.0); VEL_SAMPLES],
            sample_count: 0,
            settle_target: 0.0,
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

    /// End the drag and launch inertial coasting (or settle if already at a
    /// bound).
    pub fn drag_end(&mut self) {
        if self.phase != Phase::Dragging {
            return;
        }
        let v = self.estimate_velocity();
        self.velocity = v;
        self.phase = Phase::Inertial;
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
                // Exponential velocity decay: v *= exp(-k·dt)
                let decay = (-self.cfg.inertia_decay * dt).exp();
                self.velocity *= decay;
                self.position += self.velocity * dt;

                let min = self.bounds.min_pos();
                let max = self.bounds.max_pos();
                let overshot = self.position < min || self.position > max;
                let stalled = self.velocity.abs() < self.cfg.inertia_cutoff;
                if overshot {
                    // Hit a bound → spring back.
                    self.begin_settle_to(self.bounds.snap_target(self.position));
                } else if stalled {
                    // Coasted to a stop inside the range → snap to nearest page.
                    self.begin_settle_to(self.bounds.snap_target(self.position));
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

    fn begin_settle_to(&mut self, target: f32) {
        self.settle_target = target;
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
        s.begin_settle_to(-1000.0);
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
}
