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

#[cfg(test)]
use std::time::Duration;

/// Target page the content should rest on. `position` is the *content origin*,
/// so larger values scroll the viewport to the right. Page `n` rests at
/// `position = n * page_extent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Dragging,
    Inertial,
    Settling,
    /// A trackpad wheel gesture is driving `position` directly from OS
    /// momentum deltas (see [`Scroller::apply_wheel_delta`]). Like
    /// [`Phase::Dragging`] the position is event-driven — we only integrate
    /// here on a timeout, once the OS stops streaming deltas. This keeps the
    /// macOS trackpad's native inertia intact and hands off to the same spring
    /// snap as a pointer drag once the gesture ends.
    WheelGesture,
    /// A trackpad gesture has ended and we are gliding to the target page via a
    /// fixed-duration cubic-bezier ease (homepad style), separate from the
    /// spring-based [`Phase::Settling`] used by pointer drags. See
    /// [`Scroller::begin_wheel_snap`] and the `WheelSnap` arm of
    /// [`Scroller::step_once`].
    WheelSnap,
}

/// Phase of a wheel/trackpad gesture, mirroring winit's `TouchPhase` without
/// taking a winit dependency in this pure-physics module. The handler layer
/// converts before calling [`Scroller::apply_wheel_delta`].
///
/// - [`WheelPhase::Started`]: first delta of a new finger gesture (finger(s)
///   just touched the trackpad).
/// - [`WheelPhase::Moved`]: an intermediate delta (fingers still down, or the
///   OS-synthesised momentum coast that follows `Ended`).
/// - [`WheelPhase::Ended`]: fingers lifted with no further OS momentum
///   expected — start the snap immediately.
/// - [`WheelPhase::Cancelled`]: the gesture was interrupted; snap to the
///   nearest page from rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
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

/// Spring / inertia tunables for pointer-drag paging. Defaults mimic an iOS
/// Launchpad page swipe. Trackpad wheel gestures use a separate [`WheelConfig`]
/// so the two never bleed into each other's feel.
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

/// Trackpad wheel-gesture tunables, ported from homepad's `PagingNSScrollView`.
/// These are intentionally separate from [`PhysicsConfig`] so the mouse-drag
/// feel is never disturbed by trackpad tuning.
///
/// All constants mirror homepad verbatim so the two apps feel identical:
/// - `delta_multiplier` = `preciseScrollMultiplier` (0.7)
/// - `rubber_stiffness_ratio` = `edgeRubberBandStiffnessMultiplier` (0.028)
/// - `rubber_max_pages` = `edgeRubberBandPageMultiplier` (3)
/// - `velocity_threshold` = homepad's `velocityThreshold` (700 pt/s)
/// - `snap_duration` = homepad's `snapReleaseDuration` (0.5 s)
/// - `snap_bezier` = homepad's `CAMediaTimingFunction(0.15, 0.0, 0.1, 1.0)`
#[derive(Debug, Clone, Copy)]
pub struct WheelConfig {
    /// Scale applied to every trackpad delta (homepad `preciseScrollMultiplier`).
    /// Raw trackpad deltas are too sensitive; 0.7 calms them.
    pub delta_multiplier: f32,
    /// Rubber-band stiffness as a fraction of page extent
    /// (homepad `edgeRubberBandStiffnessMultiplier`). The visible pull drops to
    /// 50% at `page_extent × 0.028` of overshoot — it stiffens fast.
    pub rubber_stiffness_ratio: f32,
    /// Maximum rubber-band pull as a multiple of page extent
    /// (homepad `edgeRubberBandPageMultiplier`).
    pub rubber_max_pages: f32,
    /// Velocity (px/s) above which the snap target follows the flick direction
    /// regardless of position; below it the nearest page wins (homepad 700).
    pub velocity_threshold: f32,
    /// Fixed duration of the snap ease in seconds (homepad 0.5).
    pub snap_duration: f32,
    /// If no wheel delta arrives within this many seconds during a gesture, we
    /// assume OS momentum stopped and start the snap ourselves (safety net).
    pub momentum_coalesce: f32,
    /// Cubic-bezier control points (x1, y1, x2, y2) for the snap ease, matching
    /// homepad's `CAMediaTimingFunction(controlPoints: 0.15, 0.0, 0.1, 1.0)`.
    pub snap_bezier: (f32, f32, f32, f32),
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            delta_multiplier: 0.7,
            rubber_stiffness_ratio: 0.028,
            rubber_max_pages: 3.0,
            velocity_threshold: 700.0,
            snap_duration: 0.5,
            // macOS momentum deltas arrive at ~16ms cadence; 120ms is ~7 missed
            // events, a safe "momentum truly stopped" threshold.
            momentum_coalesce: 0.12,
            snap_bezier: (0.15, 0.0, 0.1, 1.0),
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
    // ---- trackpad wheel gesture state (homepad-style) -------------------
    /// Wheel tuning, separate from pointer-drag `cfg`.
    pub wheel_cfg: WheelConfig,
    /// Snap position the active wheel gesture started from. The gesture can
    /// move at most one page away from here, exactly like a pointer drag
    /// (`gesture_start_snap`). Ported from homepad's `sessionStartPage`.
    wheel_from_snap: f32,
    /// Accumulated, multiplier-scaled displacement of the active wheel gesture
    /// (logical px). `position` is derived as
    /// `clamp_wheel_homepad(wheel_from_snap + wheel_accumulated)`.
    wheel_accumulated: f32,
    /// Latest per-event velocity sample (px/s) of the wheel gesture, used by
    /// the snap-direction decision (homepad's `lastVelocityX`). Simple
    /// finite difference, no smoothing.
    wheel_velocity: f32,
    /// homepad's `ignoreMomentum`: once the fingers lift and we start our own
    /// snap, we discard all subsequent OS-synthesized momentum deltas so they
    /// can't fight the snap animation.
    ignore_momentum: bool,
    /// Wall-clock of the most recent wheel delta. Used by the gesture-timeout
    /// safety net in `tick`.
    last_wheel_time: Option<Instant>,
    /// Wall-clock of the previous wheel delta, for the per-event velocity
    /// sample (`dx / dt`).
    prev_wheel_time: Option<Instant>,
    // ---- WheelSnap animation state (fixed-duration cubic-bezier ease) ---
    /// Position when the snap started.
    wheel_snap_from: f32,
    /// Target page position for the snap.
    wheel_snap_to: f32,
    /// Elapsed snap time in seconds.
    wheel_snap_elapsed: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollDiagnostics {
    /// Direct-manipulation position implied by the latest pointer after the
    /// same rubber-band function used by `drag_move`.
    pub input_target: Option<f32>,
    pub settle_target: Option<f32>,
    pub velocity_sample_count: usize,
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
            wheel_cfg: WheelConfig::default(),
            wheel_from_snap: 0.0,
            wheel_accumulated: 0.0,
            wheel_velocity: 0.0,
            ignore_momentum: false,
            last_wheel_time: None,
            prev_wheel_time: None,
            wheel_snap_from: 0.0,
            wheel_snap_to: 0.0,
            wheel_snap_elapsed: 0.0,
        }
    }

    pub fn set_bounds(&mut self, bounds: ScrollBounds) {
        let bounds_unchanged = self.bounds.page_count == bounds.page_count
            && (self.bounds.page_extent - bounds.page_extent).abs()
                <= f32::EPSILON * self.bounds.page_extent.abs().max(1.0);
        self.bounds = bounds;
        // The rubber-band dimension tracks the content (page) extent so the
        // overshoot feel scales with the page width, exactly like iOS.
        self.cfg.rubber_dimension = bounds.page_extent;
        // Layout is rebuilt while a pointer gesture or snap animation is in
        // progress. Re-applying identical bounds must not clamp the live
        // rubber-band position: on a one-page folder min == max == 0, which
        // otherwise makes layout and direct manipulation fight every frame.
        if bounds_unchanged {
            return;
        }
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
        // If a trackpad wheel gesture/snap is mid-flight, a pointer press takes
        // over: cancel the wheel (drop its residual momentum) and start a
        // clean direct-manipulation drag from the current position.
        if matches!(self.phase, Phase::WheelGesture | Phase::WheelSnap) {
            self.wheel_accumulated = 0.0;
            self.wheel_velocity = 0.0;
            self.ignore_momentum = false;
            self.last_wheel_time = None;
            self.prev_wheel_time = None;
        }
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

    /// Feed a trackpad wheel delta into the paging scroller, ported from
    /// homepad's `PagingNSScrollView.scrollWheel(with:)`.
    ///
    /// This mirrors homepad's design exactly so the two apps feel identical:
    /// - Each delta is scaled by [`WheelConfig::delta_multiplier`] (0.7).
    /// - Position is derived from a gesture anchor with homepad's rubber-band
    ///   curve `1/(1+(x/k)²)`, bounded to ±1 page from the anchor.
    /// - On finger lift ([`WheelPhase::Ended`]) we start our own snap and set
    ///   `ignore_momentum` so the OS-synthesized momentum that follows cannot
    ///   fight the snap — the same trick homepad uses.
    ///
    /// Sign convention: `dx` is the raw horizontal logical-px delta as given to
    /// us by the handler (which has already accounted for macOS "natural
    /// scrolling" inversion). Positive `dx` scrolls content toward the previous
    /// page (position increases), matching a rightward pointer drag.
    pub fn apply_wheel_delta(&mut self, dx: f32, now: Instant, phase: WheelPhase) {
        // A pointer drag owns the scroller; ignore wheel input while one is
        // active so the two can't fight over `position`.
        if self.phase == Phase::Dragging {
            return;
        }

        // homepad: a new finger contact always clears ignoreMomentum and opens
        // a fresh session — even mid-snap, the user's new gesture wins.
        if phase == WheelPhase::Started {
            self.ignore_momentum = false;
            self.begin_wheel_session(now);
        }

        // homepad: while we're ignoring OS momentum (post-release), drop all
        // momentum events except the terminal one, which clears the flag.
        if self.ignore_momentum {
            if phase == WheelPhase::Ended || phase == WheelPhase::Cancelled {
                self.ignore_momentum = false;
            }
            return;
        }

        // If we have no open session and this isn't a Started, open one (covers
        // mouse-wheel and any momentum that arrived without a preceding Started).
        if self.phase != Phase::WheelGesture && phase != WheelPhase::Ended {
            self.begin_wheel_session(now);
        }
        // Momentum deltas without a session are dropped (homepad guard).
        if self.phase != Phase::WheelGesture {
            return;
        }

        // Apply the delta (scaled) and refresh velocity + position.
        let scaled = dx * self.wheel_cfg.delta_multiplier;
        if scaled != 0.0 {
            // Per-event velocity: simple finite difference (homepad style).
            if let Some(prev) = self.prev_wheel_time {
                let dt = now.duration_since(prev).as_secs_f32();
                if dt >= 1e-3 {
                    self.wheel_velocity = scaled / dt;
                }
            }
            self.wheel_accumulated += scaled;
            let raw = self.wheel_from_snap + self.wheel_accumulated;
            self.position = self.clamp_wheel_homepad(raw);
        }
        self.prev_wheel_time = Some(now);
        self.last_wheel_time = Some(now);

        // Finger lift: immediately snap and start ignoring OS momentum.
        if phase == WheelPhase::Ended || phase == WheelPhase::Cancelled {
            self.begin_wheel_snap();
        }
    }

    /// Open a wheel gesture session anchored on the current page (homepad's
    /// `beginScrollSessionIfNeeded`). Resets accumulation and velocity.
    fn begin_wheel_session(&mut self, now: Instant) {
        self.wheel_from_snap = self.bounds.snap_target(self.position);
        self.wheel_accumulated = 0.0;
        self.wheel_velocity = 0.0;
        self.prev_wheel_time = Some(now);
        self.last_wheel_time = Some(now);
        self.phase = Phase::WheelGesture;
        self.last_time = Some(now);
    }

    /// Decide the snap target page (homepad's `decideTargetPage`) and start the
    /// fixed-duration bezier ease (homepad's `snapToPage`). At most one page
    /// from the gesture anchor; a velocity above the threshold overrides the
    /// nearest-page pick in the flick direction.
    fn begin_wheel_snap(&mut self) {
        let target = self.decide_wheel_target_page();
        self.wheel_snap_from = self.position;
        self.wheel_snap_to = target;
        self.wheel_snap_elapsed = 0.0;
        self.ignore_momentum = true;
        self.phase = Phase::WheelSnap;
        // Keep `last_time` as-is (set to `now` during the wheel session) so the
        // very first tick after entering WheelSnap measures a real dt instead
        // of warming the clock and wasting a frame.
    }

    /// homepad's `decideTargetPage`: round to the nearest page, unless the
    /// velocity exceeds the threshold, in which case advance one page in the
    /// flick direction. Clamped to ±1 page from the gesture anchor and to the
    /// content bounds.
    ///
    /// Sign note: `position` *decreases* toward later pages, so a *negative*
    /// velocity means scrolling to the *next* page.
    fn decide_wheel_target_page(&self) -> f32 {
        let page = self.bounds.page_extent;
        let raw_page = self.position / page;
        let mut target_page = raw_page.round();

        let v = self.wheel_velocity;
        if v.abs() > self.wheel_cfg.velocity_threshold {
            // Negative velocity → next page (more negative position).
            if v < 0.0 {
                target_page = raw_page.floor() - 1.0;
            } else {
                target_page = raw_page.ceil() + 1.0;
            }
        }

        // Clamp to ±1 page from the gesture anchor.
        let anchor_page = self.wheel_from_snap / page;
        target_page = target_page.clamp(anchor_page - 1.0, anchor_page + 1.0);
        // Clamp to content bounds.
        let min_page = self.bounds.min_pos() / page;
        let max_page = self.bounds.max_pos() / page;
        target_page.clamp(min_page, max_page) * page
    }

    /// homepad's rubber-band clamp: bound the target to a ±1-page window around
    /// the gesture anchor, applying homepad's `1/(1+(x/k)²)` attenuation past
    /// the limit, with the content's hard bounds layered on top so the
    /// first/last page rubber-bands against the content edge.
    ///
    /// Note this differs from the pointer-drag [`Self::clamp_with_rubber`]
    /// (Apple's `B(x)` curve): homepad's curve stiffens much faster (factor
    /// drops to 0.5 at just 2.8% of page width), giving the "hard wall with
    /// slight give" feel of macOS Launchpad.
    fn clamp_wheel_homepad(&self, raw: f32) -> f32 {
        let page = self.bounds.page_extent;
        // Soft ±1-page window around the anchor.
        let soft_min = self.wheel_from_snap - page;
        let soft_max = self.wheel_from_snap + page;
        // Hard content bounds.
        let hard_min = self.bounds.min_pos();
        let hard_max = self.bounds.max_pos();
        // Effective limit at each end = the tighter of soft/hard.
        let lim_min = soft_min.max(hard_min);
        let lim_max = soft_max.min(hard_max);

        let stiffness = (page * self.wheel_cfg.rubber_stiffness_ratio).max(1.0);
        let max_pull = page * self.wheel_cfg.rubber_max_pages;

        if raw > lim_max {
            let over = (raw - lim_max).min(max_pull);
            lim_max + over * self.homepad_rubber_factor(over, stiffness)
        } else if raw < lim_min {
            let over = (lim_min - raw).min(max_pull);
            lim_min - over * self.homepad_rubber_factor(over, stiffness)
        } else {
            raw
        }
    }

    /// homepad's `rubberBandFactor`: `1 / (1 + (overscroll/stiffness)²)`.
    /// The *delta multiplier*, not the displacement — so effective movement
    /// decays quadratically as you pull further past the limit.
    #[inline]
    fn homepad_rubber_factor(&self, overscroll: f32, stiffness: f32) -> f32 {
        let ratio = overscroll / stiffness;
        1.0 / (1.0 + ratio * ratio)
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

        // While waiting for the next wheel delta, watch for the OS-momentum
        // timeout. If no delta arrives within `momentum_coalesce`, the OS has
        // stopped (or never sent an `Ended`) — start the snap ourselves.
        if self.phase == Phase::WheelGesture {
            let stopped = self
                .last_wheel_time
                .map(|t| now.duration_since(t).as_secs_f32() >= self.wheel_cfg.momentum_coalesce)
                .unwrap_or(true);
            if stopped {
                self.begin_wheel_snap();
            }
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

    pub fn diagnostics(&self, pointer_x: f32) -> ScrollDiagnostics {
        let input_target = (self.phase == Phase::Dragging).then(|| {
            let raw = self.drag_anchor + (pointer_x - self.drag_start_pointer);
            self.clamp_with_rubber(raw)
        });
        ScrollDiagnostics {
            input_target,
            settle_target: (self.phase == Phase::Settling).then_some(self.settle_target),
            velocity_sample_count: self.sample_count,
        }
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
            Phase::Idle | Phase::Dragging | Phase::WheelGesture => {
                // Position is driven directly by events (pointer moves for
                // Dragging, OS wheel deltas for WheelGesture); nothing to
                // integrate here. WheelGesture's timeout→Settling hand-off is
                // handled in `tick`, not per-substep.
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
            Phase::WheelSnap => {
                // homepad-style fixed-duration cubic-bezier ease. Unlike the
                // spring-based Settling above, this has no overshoot/bounce —
                // it's a pure ease-out glide to the target page (the "settling
                // into place" feel of macOS Launchpad).
                self.wheel_snap_elapsed += dt;
                let t = (self.wheel_snap_elapsed / self.wheel_cfg.snap_duration).min(1.0);
                let e = cubic_bezier_easing_y(t, self.wheel_cfg.snap_bezier);
                self.position =
                    self.wheel_snap_from + (self.wheel_snap_to - self.wheel_snap_from) * e;
                if t >= 1.0 {
                    self.position = self.wheel_snap_to;
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
        let first_valid = VEL_SAMPLES - self.sample_count;
        for i in (first_valid..VEL_SAMPLES - 1).rev() {
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

// ---- Vertical continuous-scroll model (iOS-style, no page snap) --------------

/// Configuration for [`ContinuousScroller`] physics.
#[derive(Debug, Clone, Copy)]
pub struct ContinuousConfig {
    /// Exponential decay factor per second for inertial coasting.
    pub inertia_decay: f32,
    /// Inertial velocity below which we cut to spring settling (px/s).
    pub inertia_cutoff: f32,
    /// Spring angular frequency ω₀ (rad/s).
    pub spring_omega: f32,
    /// Damping ratio ζ.
    pub spring_zeta: f32,
    /// Rubber-band stiffness divisor (Apple: 0.55).
    pub rubber_c: f32,
    /// Below this speed & distance we consider the spring settled.
    pub settle_eps: f32,
    /// Maximum frame dt before we subdivide (s).
    pub max_dt: f32,
}

impl Default for ContinuousConfig {
    fn default() -> Self {
        Self {
            inertia_decay: 3.2,
            inertia_cutoff: 18.0,
            spring_omega: 22.0,
            spring_zeta: 0.82,
            rubber_c: 0.55,
            settle_eps: 0.35,
            max_dt: 1.0 / 60.0,
        }
    }
}

/// Vertical continuous-scroll phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuousPhase {
    Idle,
    Dragging,
    Inertial,
    Settling,
}

/// Vertical continuous-scroll physics (iOS-style): 1:1 drag tracking,
/// inertia, rubber-band at bounds, spring return. Reuses Spring/rubber
/// from the paging Scroller but without page snapping.
pub struct ContinuousScroller {
    /// Current content offset in logical px. 0 = top of content.
    pub position: f32,
    /// Current velocity in px/s.
    pub velocity: f32,
    pub phase: ContinuousPhase,
    pub cfg: ContinuousConfig,
    /// Total content height in logical px.
    content_size: f32,
    /// Viewport height in logical px.
    viewport_size: f32,
    /// Content position captured at drag start.
    drag_anchor: f32,
    /// Pointer position captured at drag start.
    drag_start_pointer: f32,
    /// Pointer history for velocity estimation: (seconds since clock_origin, pos).
    samples: [(f32, f32); VEL_SAMPLES],
    sample_count: usize,
    /// Target position the spring settles toward.
    settle_target: f32,
    /// Last clock reading for dt, in seconds.
    last_time: Option<Instant>,
    /// Monotonic clock origin.
    clock_origin: Instant,
    /// Timestamp of the most recent OS wheel/momentum delta. While the OS is
    /// still streaming momentum events past a bound, `apply_wheel` overwrites
    /// `position` each event, so the Settling spring must NOT also integrate —
    /// otherwise the two fight and the view jitters. `step_once` checks this
    /// timestamp and freezes the spring while deltas keep arriving.
    last_wheel_time: Option<Instant>,
}

impl ContinuousScroller {
    pub fn new(cfg: ContinuousConfig) -> Self {
        let clock_origin = Instant::now();
        Self {
            position: 0.0,
            velocity: 0.0,
            phase: ContinuousPhase::Idle,
            cfg,
            content_size: 0.0,
            viewport_size: 0.0,
            drag_anchor: 0.0,
            drag_start_pointer: 0.0,
            samples: [(0.0, 0.0); VEL_SAMPLES],
            sample_count: 0,
            settle_target: 0.0,
            last_time: None,
            clock_origin,
            last_wheel_time: None,
        }
    }

    /// Update content and viewport sizes. Clamps position if needed.
    pub fn set_sizes(&mut self, content_size: f32, viewport_size: f32) {
        self.content_size = content_size;
        self.viewport_size = viewport_size;
        // If idle, clamp position into the new valid range immediately.
        let max = self.max_offset();
        if self.phase == ContinuousPhase::Idle {
            self.position = self.position.clamp(0.0, max);
        }
    }

    /// Minimum scroll offset (always 0 for vertical: content starts at top).
    #[inline]
    pub fn min_offset(&self) -> f32 {
        0.0
    }

    /// Maximum scroll offset in px: max(0, content - viewport).
    #[inline]
    pub fn max_offset(&self) -> f32 {
        (self.content_size - self.viewport_size).max(0.0)
    }

    /// Begin a drag gesture from the current pointer y position.
    pub fn drag_start(&mut self, pointer_y: f32, now: Instant) {
        self.phase = ContinuousPhase::Dragging;
        self.drag_anchor = self.position;
        self.drag_start_pointer = pointer_y;
        self.velocity = 0.0;
        self.sample_count = 0;
        self.last_time = Some(now);
    }

    /// Update the drag with the latest pointer y. Returns new position.
    pub fn drag_move(&mut self, pointer_y: f32, now: Instant) -> f32 {
        if self.phase != ContinuousPhase::Dragging {
            return self.position;
        }
        // Direct manipulation: content offset tracks pointer displacement.
        // Sign: dragging down (pointer_y increases) → content scrolls down
        // (position increases). pointer_y - drag_start_pointer positive → scroll down.
        let raw = self.drag_anchor + (pointer_y - self.drag_start_pointer);
        let pos = self.clamp_with_rubber(raw);
        let prev = self.position;
        self.position = pos;
        self.push_sample(pos, prev);
        // Update last_time so tick-based animation integrates correctly.
        self.last_time = Some(now);
        pos
    }

    /// End the drag and transition to inertia (if velocity is significant)
    /// or settling.
    pub fn drag_end(&mut self, now: Instant) {
        if self.phase != ContinuousPhase::Dragging {
            return;
        }
        let v = self.estimate_velocity();
        self.velocity = v;
        self.last_time = Some(now);

        let min = self.min_offset();
        let max = self.max_offset();
        let out_of_bounds = self.position < min || self.position > max;

        if v.abs() > self.cfg.inertia_cutoff && !out_of_bounds {
            // Launch into inertia with the estimated velocity.
            self.phase = ContinuousPhase::Inertial;
        } else {
            // Settle to nearest bound or current position.
            let target = self.position.clamp(min, max);
            self.begin_settle_to(target);
        }
    }

    /// Apply a wheel / trackpad delta (in logical px) directly. This is for
    /// OS momentum scroll events which already include inertia; we do NOT
    /// accumulate velocity, just move the position. Positive delta = scroll down.
    /// Returns the new position.
    ///
    /// winit wheel deltas use "scroll content backward (down) = negative y",
    /// but our `position` increases when scrolling down. We flip the sign once
    /// here so callers can pass the raw winit delta without per-platform sign
    /// juggling. Direct drag (`drag_move`) is unaffected because it tracks
    /// pointer displacement, not wheel delta.
    pub fn apply_wheel(&mut self, delta_px: f32, now: Instant) -> f32 {
        let delta_px = -delta_px;
        self.last_time = Some(now);
        self.last_wheel_time = Some(now);
        let raw = self.position + delta_px;
        let pos = self.clamp_with_rubber(raw);

        let min = self.min_offset();
        let max = self.max_offset();
        let in_bounds = pos >= min && pos <= max;

        if self.phase == ContinuousPhase::Idle {
            if in_bounds {
                self.position = pos;
                self.velocity = 0.0;
            } else {
                // Rubber-banded past a bound: show the compression but DON'T
                // settle yet. Record the timestamp; the frame tick will start a
                // settle after the OS momentum stops feeding deltas.
                self.position = pos;
                self.phase = ContinuousPhase::Settling;
                self.settle_target = pos.clamp(min, max);
                // velocity stays 0 — the spring pulls toward the boundary.
                // OS momentum deltas overwrite position each frame via this
                // method, so the spring cannot fight the ongoing momentum.
            }
        } else {
            // Already animating (Settling/Inertial/Dragging). OS delta arrived:
            // overwrite position and update target so the spring knows where
            // the boundary is once momentum stops.
            self.position = pos;
            if in_bounds && self.phase == ContinuousPhase::Settling {
                // Back inside bounds → stop at current position.
                self.settle_target = pos;
                self.velocity = 0.0;
            } else if !in_bounds && self.phase == ContinuousPhase::Settling {
                self.settle_target = pos.clamp(min, max);
            }
        }
        self.position
    }

    /// Advance the simulation by real elapsed time. Returns the new phase.
    pub fn tick(&mut self, now: Instant) -> ContinuousPhase {
        let dt = match self.last_time {
            None => {
                self.last_time = Some(now);
                return self.phase;
            }
            Some(t) => {
                let d = now.duration_since(t).as_secs_f32();
                self.last_time = Some(now);
                d.min(0.1)
            }
        };
        if dt <= 0.0 {
            return self.phase;
        }

        let mut remaining = dt;
        while remaining > 0.0 {
            let step = remaining.min(self.cfg.max_dt);
            remaining -= step;
            self.step_once(step, now);
            if self.phase == ContinuousPhase::Idle {
                break;
            }
        }
        self.phase
    }

    /// True while content is moving.
    pub fn is_animating(&self) -> bool {
        self.phase != ContinuousPhase::Idle
    }

    /// Set position immediately (for `ensure_visible` etc.).
    pub fn set_position(&mut self, pos: f32) {
        let min = self.min_offset();
        let max = self.max_offset();
        self.position = pos.clamp(min, max);
        self.velocity = 0.0;
        self.phase = ContinuousPhase::Idle;
    }

    /// Reset the timer used for dt (call when the app resumes after a pause).
    pub fn reset_clock(&mut self, now: Instant) {
        self.last_time = Some(now);
    }

    /// Adjust position so the given item rect (in content coordinates) is
    /// fully visible inside the viewport. If scrolling is needed, transitions
    /// to Settling.
    pub fn ensure_visible(&mut self, item_top: f32, item_bottom: f32) {
        let vp_h = self.viewport_size;
        let min = self.min_offset();
        let max = self.max_offset();

        let mut target = self.position;
        if item_top < self.position {
            target = item_top;
        } else if item_bottom > self.position + vp_h {
            target = item_bottom - vp_h;
        }
        target = target.clamp(min, max);

        if (target - self.position).abs() > self.cfg.settle_eps {
            self.settle_target = target;
            self.velocity = 0.0;
            self.phase = ContinuousPhase::Settling;
        }
    }

    // ---- internals -------------------------------------------------------

    fn step_once(&mut self, dt: f32, now: Instant) {
        match self.phase {
            ContinuousPhase::Idle | ContinuousPhase::Dragging => {
                // Position is driven directly by pointer events.
            }
            ContinuousPhase::Inertial => {
                let decay = (-self.cfg.inertia_decay * dt).exp();
                self.velocity *= decay;
                self.position += self.velocity * dt;

                let min = self.min_offset();
                let max = self.max_offset();
                let overshot = self.position < min || self.position > max;
                let stalled = self.velocity.abs() < self.cfg.inertia_cutoff;
                if overshot || stalled {
                    let target = self.position.clamp(min, max);
                    self.begin_settle_to(target);
                }
            }
            ContinuousPhase::Settling => {
                // If the OS is still streaming momentum deltas that push us
                // past a bound (rubber-band region), `apply_wheel` overwrites
                // `position` every event. Letting the spring ODE also run here
                // makes the two fight and the view jitters. Freeze the spring
                // while deltas keep arriving (within a short grace window),
                // so the spring only takes over once momentum actually stops.
                let wheel_recent = self
                    .last_wheel_time
                    .map(|t| now.duration_since(t).as_secs_f32() < 0.06)
                    .unwrap_or(false);
                let min = self.min_offset();
                let max = self.max_offset();
                let out_of_bounds = self.position < min || self.position > max;
                if wheel_recent && out_of_bounds {
                    // Hold position; OS momentum owns it until it stops.
                    return;
                }
                // Semi-implicit Euler on the spring ODE:
                //   a = -ω₀²·(x - target) - 2·ζ·ω₀·v
                let dx = self.position - self.settle_target;
                // If position is well inside the rubber-band region (far from
                // target), the OS momentum is still delivering deltas that
                // overwrite position every frame via apply_wheel, so the ODE
                // step here is harmless but irrelevant. Once momentum stops,
                // the spring pulls position from the rubber-band back to the
                // boundary smoothly.
                if dx.abs() < self.cfg.settle_eps && self.velocity.abs() < self.cfg.settle_eps {
                    self.position = self.settle_target;
                    self.velocity = 0.0;
                    self.phase = ContinuousPhase::Idle;
                } else {
                    let acc = -self.cfg.spring_omega * self.cfg.spring_omega * dx
                        - 2.0 * self.cfg.spring_zeta * self.cfg.spring_omega * self.velocity;
                    self.velocity += acc * dt;
                    self.position += self.velocity * dt;
                    // Prevent overshoot past the target (i.e. spring crossing
                    // from the rubber-band side to the opposite side of the
                    // boundary).
                    if (dx > 0.0 && self.position < self.settle_target)
                        || (dx < 0.0 && self.position > self.settle_target)
                    {
                        self.position = self.settle_target;
                        self.velocity = 0.0;
                        self.phase = ContinuousPhase::Idle;
                    }
                }
            }
        }
    }

    fn begin_settle_to(&mut self, target: f32) {
        self.settle_target = target;
        self.phase = ContinuousPhase::Settling;
    }

    /// Clamp `raw` to [min, max] with rubber-band past the ends.
    fn clamp_with_rubber(&self, raw: f32) -> f32 {
        let min = self.min_offset();
        let max = self.max_offset();
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

    /// Apple's rubber-band curve.
    #[inline]
    fn rubber(&self, x: f32) -> f32 {
        let c = self.cfg.rubber_c;
        let d = self.viewport_size.max(1.0);
        (1.0 - 1.0 / (x * c / d + 1.0)) * d
    }

    fn push_sample(&mut self, pos: f32, _prev: f32) {
        let t = self.clock_origin.elapsed().as_secs_f32();
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
        let mut chosen = last;
        let first_valid = VEL_SAMPLES - self.sample_count;
        for i in (first_valid..VEL_SAMPLES - 1).rev() {
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

/// Convert a line-scroll delta to logical pixels.
///
/// `px_per_line` is typically the row height (e.g. ~62 px for settings rows).
/// This is a convenience for callers that have line-based deltas.
pub fn line_delta_to_px(lines: f32, px_per_line: f32) -> f32 {
    lines * px_per_line
}

/// Evaluate the y component of a CSS-style cubic-bezier easing curve at
/// progress `x` (0..=1), matching `CAMediaTimingFunction(controlPoints: x1,y1,
/// x2,y2)`. Used by the wheel snap to reproduce homepad's
/// `(0.15, 0.0, 0.1, 1.0)` ease-out glide.
///
/// Given the curve `B(s) = 3(1-s)²s·P1 + 3(1-s)s²·P2 + s³·1` (P0=(0,0),
/// P3=(1,1)), we solve `Bx(s) = x` for `s` with Newton-Raphson, then return
/// `By(s)`.
pub fn cubic_bezier_easing_y(x: f32, ctrl: (f32, f32, f32, f32)) -> f32 {
    let (cx1, cy1, cx2, cy2) = ctrl;
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    // Solve Bx(s) = x for s ∈ [0,1]. Bx(s) = 3(1-s)²s·cx1 + 3(1-s)s²·cx2 + s³.
    let sample_curve_x = |s: f32| {
        let one_minus = 1.0 - s;
        3.0 * one_minus * one_minus * s * cx1 + 3.0 * one_minus * s * s * cx2 + s * s * s
    };
    let sample_curve_y = |s: f32| {
        let one_minus = 1.0 - s;
        3.0 * one_minus * one_minus * s * cy1 + 3.0 * one_minus * s * s * cy2 + s * s * s
    };
    let sample_curve_derivative_x = |s: f32| {
        let one_minus = 1.0 - s;
        3.0 * one_minus * one_minus * cx1
            + 6.0 * one_minus * s * (cx2 - cx1)
            + 3.0 * s * s * (1.0 - cx2)
    };

    // Newton-Raphson with a bisection fallback for robustness.
    let mut s = x; // good initial guess
    for _ in 0..8 {
        let x_err = sample_curve_x(s) - x;
        if x_err.abs() < 1e-6 {
            return sample_curve_y(s);
        }
        let d = sample_curve_derivative_x(s);
        if d.abs() < 1e-6 {
            break;
        }
        s -= x_err / d;
    }
    // Bisection fallback.
    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    s = x;
    for _ in 0..32 {
        let xv = sample_curve_x(s);
        if (xv - x).abs() < 1e-6 {
            return sample_curve_y(s);
        }
        if x < xv {
            hi = s;
        } else {
            lo = s;
        }
        s = (lo + hi) * 0.5;
    }
    sample_curve_y(s)
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
    fn identical_bounds_do_not_cancel_single_page_rubber_band() {
        let mut s = Scroller::new(bounds(1));
        s.drag_start(500.0);
        s.drag_move(400.0);
        let rubber_position = s.position;
        assert!(rubber_position < 0.0);

        // Folder relayout re-submits the same bounds every pointer frame.
        s.set_bounds(bounds(1));

        assert_eq!(s.phase, Phase::Dragging);
        assert_eq!(s.position, rubber_position);
    }

    #[test]
    fn velocity_estimation_ignores_unused_sample_slots() {
        let mut s = Scroller::new(bounds(2));
        s.samples = [(0.0, -9000.0), (0.0, -9000.0), (1.00, 0.0), (1.02, -10.0)];
        s.sample_count = 2;

        assert!((s.estimate_velocity() - -500.0).abs() < 0.1);
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

    // ---- wheel gesture paging (homepad-style trackpad) ------------------

    /// Feed a wheel gesture: a sequence of `(dx, phase)` at ~16ms cadence,
    /// with optional trailing momentum events. The handler is expected to
    /// pre-invert the sign for natural scrolling, so these tests pass the raw
    /// "content-space" sign (positive dx = scroll content toward previous page).
    fn run_wheel(s: &mut Scroller, start_pos: f32, events: &[(f32, WheelPhase)]) {
        s.position = start_pos;
        let t0 = Instant::now();
        for (i, &(dx, phase)) in events.iter().enumerate() {
            s.apply_wheel_delta(dx, t0 + Duration::from_millis(16 * i as u64), phase);
        }
    }

    #[test]
    fn wheel_delta_multiplier_is_07() {
        // homepad's preciseScrollMultiplier: a 100px delta becomes 70px of
        // accumulated displacement (position tracks the anchor + accumulated,
        // so the move equals accumulated when in-range).
        let mut s = Scroller::new(bounds(4));
        s.position = -1000.0; // page 1, room both ways
        let t = Instant::now();
        s.apply_wheel_delta(100.0, t, WheelPhase::Started);
        assert!(
            (s.position - (-1000.0 + 70.0)).abs() < 0.5,
            "100px delta * 0.7 should move 70px, got {}",
            s.position
        );
    }

    #[test]
    fn wheel_ignores_momentum_after_release() {
        // homepad's ignoreMomentum: after the finger-lift Ended starts the
        // snap, subsequent OS-synthesized momentum Moved events must NOT move
        // position (they'd fight the snap).
        let mut s = Scroller::new(bounds(4));
        run_wheel(
            &mut s,
            -1000.0,
            &[
                (-50.0, WheelPhase::Started),
                (-100.0, WheelPhase::Moved),
                (-100.0, WheelPhase::Ended),
            ],
        );
        assert_eq!(s.phase, Phase::WheelSnap);
        let pos_at_release = s.position;
        // Momentum events arrive — must be ignored.
        let t = Instant::now() + Duration::from_millis(100);
        s.apply_wheel_delta(-300.0, t, WheelPhase::Moved);
        assert_eq!(
            s.position, pos_at_release,
            "momentum after release must not move position"
        );
        assert_eq!(s.phase, Phase::WheelSnap, "still snapping");
    }

    #[test]
    fn wheel_momentum_ended_clears_ignore_flag() {
        // The terminal momentum Ended clears ignoreMomentum so a later gesture
        // works normally (homepad resets the flag on momentum end).
        let mut s = Scroller::new(bounds(4));
        run_wheel(
            &mut s,
            -1000.0,
            &[
                (-50.0, WheelPhase::Started),
                (-100.0, WheelPhase::Moved),
                (-100.0, WheelPhase::Ended), // finger lift → snap + ignore
            ],
        );
        assert_eq!(s.phase, Phase::WheelSnap);
        // Momentum events ignored until the terminal Ended.
        let t0 = Instant::now() + Duration::from_millis(200);
        s.apply_wheel_delta(-300.0, t0, WheelPhase::Moved); // ignored
        assert_eq!(s.phase, Phase::WheelSnap);
        s.apply_wheel_delta(-300.0, t0 + Duration::from_millis(16), WheelPhase::Ended);
        // ignored_momentum now cleared; a new Started should open a fresh
        // gesture (re-anchored at the current snapped position).
        s.apply_wheel_delta(-10.0, t0 + Duration::from_millis(32), WheelPhase::Started);
        assert_eq!(
            s.phase,
            Phase::WheelGesture,
            "new gesture should open after momentum-Ended clears the flag"
        );
    }

    #[test]
    fn wheel_velocity_threshold_700_advances_page() {
        // homepad: |velocity| > 700 pt/s overrides the nearest-page pick and
        // advances one page in the flick direction. Here a fast next-page
        // swipe (negative dx) on page 1 must target page 2.
        let mut s = Scroller::new(bounds(4));
        // Build a fast flick: -120px over 16ms = -7500 px/s (> 700).
        run_wheel(
            &mut s,
            -1000.0,
            &[
                (-5.0, WheelPhase::Started),
                (-120.0, WheelPhase::Moved),
                (-0.0, WheelPhase::Ended),
            ],
        );
        assert_eq!(s.phase, Phase::WheelSnap);
        assert_eq!(s.wheel_snap_to, -2000.0, "fast flick should target page 2");
    }

    #[test]
    fn wheel_below_threshold_snaps_to_nearest() {
        // Below 700 pt/s, the target is the nearest page by position.
        let mut s = Scroller::new(bounds(4));
        // Slow drag: -700px over a long (1s) dt = -700 px/s, right at/below the
        // threshold. Position lands at -1000 + 0.7*-700 = -1490, nearest page
        // is -1000 (page 1).
        let t0 = Instant::now();
        s.position = -1000.0;
        s.apply_wheel_delta(-2.0, t0, WheelPhase::Started);
        // 1s gap → velocity sample = -700/1.0 = -700 (not > 700).
        s.apply_wheel_delta(-700.0, t0 + Duration::from_millis(1000), WheelPhase::Moved);
        s.apply_wheel_delta(0.0, t0 + Duration::from_millis(1016), WheelPhase::Ended);
        assert_eq!(s.phase, Phase::WheelSnap);
        // position ~ -1490 → raw_page 1.49 → round = 1 → page 1 (-1000).
        assert_eq!(s.wheel_snap_to, -1000.0);
    }

    #[test]
    fn wheel_caps_at_one_page_from_anchor() {
        // Even a violent swipe targets at most one page from the gesture anchor.
        let mut s = Scroller::new(bounds(6));
        run_wheel(
            &mut s,
            -2000.0, // page 2
            &[
                (-10.0, WheelPhase::Started),
                (-5000.0, WheelPhase::Moved),
                (0.0, WheelPhase::Ended),
            ],
        );
        assert_eq!(s.phase, Phase::WheelSnap);
        assert_eq!(
            s.wheel_snap_to, -3000.0,
            "must cap at one page ahead (-3000)"
        );
    }

    #[test]
    fn wheel_rubber_homepad_curve_stiffens_fast() {
        // homepad's rubber-band: factor = 1/(1+(over/k)²) with k = page*0.028.
        // At the first page (0), pulling right (previous-page direction) must
        // attenuate fast. A 100px*0.7=70px pull past 0 should land well under 70.
        let mut s = Scroller::new(bounds(4));
        s.position = 0.0;
        run_wheel(&mut s, 0.0, &[(100.0, WheelPhase::Started)]);
        assert!(s.position > 0.0, "should pull past the edge a bit");
        assert!(
            s.position < 70.0,
            "rubber band must attenuate (70px input → <70px move), got {}",
            s.position
        );
    }

    #[test]
    fn wheel_sustained_edge_momentum_asymptotes() {
        // Sustained OS momentum at the first page must not run away — it
        // asymptotes toward page*rubber_max_pages (3) above 0.
        let mut s = Scroller::new(bounds(4));
        let mut events = vec![(10.0, WheelPhase::Started)];
        for _ in 0..30 {
            events.push((100.0, WheelPhase::Moved));
        }
        run_wheel(&mut s, 0.0, &events);
        assert!(
            s.position < 1000.0,
            "sustained edge swipe must asymptote (<3 pages), got {}",
            s.position
        );
    }

    #[test]
    fn wheel_snap_completes_in_fixed_duration() {
        // homepad's snap is a fixed 0.5s ease. Step with a fixed dt (no wall
        // clock) and confirm it reaches the target at ~0.5s, not before.
        let mut s = Scroller::new(bounds(4));
        run_wheel(
            &mut s,
            -1000.0,
            &[
                (-5.0, WheelPhase::Started),
                (-120.0, WheelPhase::Moved),
                (0.0, WheelPhase::Ended),
            ],
        );
        assert_eq!(s.phase, Phase::WheelSnap);
        let target = s.wheel_snap_to;
        let dur = s.wheel_cfg.snap_duration;
        let step = 1.0 / 120.0;
        // Step to just before completion — should still be animating.
        let n_before = ((dur - step) / step).floor() as usize;
        for _ in 0..n_before {
            if s.phase == Phase::Idle {
                break;
            }
            s.step_once(step);
        }
        assert_ne!(
            s.phase,
            Phase::Idle,
            "snap shouldn't finish before {}s",
            dur
        );
        // Step past completion.
        for _ in 0..40 {
            if s.phase == Phase::Idle {
                break;
            }
            s.step_once(step);
        }
        assert_eq!(s.phase, Phase::Idle);
        assert!(
            (s.position - target).abs() < 0.5,
            "should rest at target {}, got {}",
            target,
            s.position
        );
    }

    #[test]
    fn wheel_snap_uses_bezier_midpoint() {
        // The snap ease must follow the cubic-bezier (0.15,0,0.1,1) curve, not
        // a linear interpolation. We step the WheelSnap with a fixed dt and
        // check the eased fraction matches the bezier, independent of wall
        // clock timing.
        let mut s = Scroller::new(bounds(4));
        run_wheel(
            &mut s,
            -1000.0,
            &[
                (-5.0, WheelPhase::Started),
                (-120.0, WheelPhase::Moved),
                (0.0, WheelPhase::Ended),
            ],
        );
        let from = s.wheel_snap_from;
        let to = s.wheel_snap_to;
        let dur = s.wheel_cfg.snap_duration;
        // Advance exactly half the snap duration in substeps (no wall clock).
        let n = (dur / (1.0 / 120.0) / 2.0).round() as usize;
        for _ in 0..n {
            s.step_once(1.0 / 120.0);
        }
        // Expected eased position at the half-way time fraction.
        let time_frac = (n as f32 * (1.0 / 120.0)) / dur;
        let mid_bezier = cubic_bezier_easing_y(time_frac, s.wheel_cfg.snap_bezier);
        let expected = from + (to - from) * mid_bezier;
        assert!(
            (s.position - expected).abs() < 1.0,
            "snap at t={} should match bezier-eased {}, got {}",
            time_frac,
            expected,
            s.position
        );
        // cubic-bezier(0.15,0,0.1,1) at the midpoint ≠ 0.5 (it's an ease-out).
        assert!(
            (mid_bezier - 0.5).abs() > 0.02,
            "bezier at midpoint should differ from linear 0.5, got {}",
            mid_bezier
        );
    }

    #[test]
    fn wheel_timeout_snaps_when_no_ended() {
        // Safety net: if no Ended arrives, the momentum_coalesce timeout in
        // tick starts the snap.
        let mut s = Scroller::new(bounds(4));
        run_wheel(
            &mut s,
            -1000.0,
            &[(-5.0, WheelPhase::Started), (-120.0, WheelPhase::Moved)],
        );
        assert_eq!(s.phase, Phase::WheelGesture);
        let mut t = Instant::now() + Duration::from_millis(64);
        for _ in 0..40 {
            s.tick(t);
            t += Duration::from_millis(16);
            if s.phase != Phase::WheelGesture {
                break;
            }
        }
        assert_eq!(s.phase, Phase::WheelSnap, "timeout should start a snap");
    }

    #[test]
    fn wheel_ignored_during_pointer_drag() {
        let mut s = Scroller::new(bounds(4));
        s.position = -1000.0;
        s.drag_start(500.0);
        s.drag_move(450.0);
        let pos_before = s.position;
        s.apply_wheel_delta(-500.0, Instant::now(), WheelPhase::Moved);
        assert_eq!(s.phase, Phase::Dragging);
        assert_eq!(s.position, pos_before);
    }

    #[test]
    fn pointer_drag_cancels_wheel_snap() {
        let mut s = Scroller::new(bounds(4));
        run_wheel(
            &mut s,
            -1000.0,
            &[(-5.0, WheelPhase::Started), (-120.0, WheelPhase::Ended)],
        );
        assert_eq!(s.phase, Phase::WheelSnap);
        let live = s.position;
        s.drag_start(300.0);
        assert_eq!(s.phase, Phase::Dragging);
        assert!((s.position - live).abs() < 1.0);
    }

    #[test]
    fn wheel_is_animating_in_gesture_and_snap() {
        let mut s = Scroller::new(bounds(4));
        assert!(!s.is_animating());
        s.apply_wheel_delta(-10.0, Instant::now(), WheelPhase::Started);
        assert!(s.is_animating());
    }

    #[test]
    fn cubic_bezier_endpoints_are_0_and_1() {
        let b = (0.15, 0.0, 0.1, 1.0);
        assert!((cubic_bezier_easing_y(0.0, b) - 0.0).abs() < 1e-4);
        assert!((cubic_bezier_easing_y(1.0, b) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn cubic_bezier_linear_identity() {
        // A linear curve (0,0,1,1) must reproduce y=x.
        let lin = (0.0, 0.0, 1.0, 1.0);
        for &x in &[0.1, 0.25, 0.5, 0.75, 0.9] {
            assert!(
                (cubic_bezier_easing_y(x, lin) - x).abs() < 1e-3,
                "linear bezier at {} should be {}",
                x,
                x
            );
        }
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

    // ---- ContinuousScroller -------------------------------------------------

    fn default_cfg() -> ContinuousConfig {
        ContinuousConfig::default()
    }

    #[test]
    fn continuous_scroller_apply_wheel_sign_convention() {
        // winit wheel delta convention: scroll down = negative y.
        // Our position convention: scroll down = positive position.
        // apply_wheel flips the sign internally, so a negative delta
        // (scroll down) increases position.
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0);
        let now = Instant::now();
        let pos = s.apply_wheel(-50.0, now); // winit: scroll down = -50
        assert!((pos - 50.0).abs() < 0.01);
        assert!((s.position - 50.0).abs() < 0.01);
    }

    #[test]
    fn continuous_scroller_apply_wheel_positive_delta_scrolls_up() {
        // A positive winit delta means scroll up (content backward).
        // With sign flip, position decreases.
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0);
        let now = Instant::now();
        s.position = 100.0;
        let pos = s.apply_wheel(30.0, now); // winit: scroll up = +30
        assert!((pos - 70.0).abs() < 0.01);
        assert!((s.position - 70.0).abs() < 0.01);
    }

    #[test]
    fn continuous_scroller_apply_wheel_at_top_bound_clamps() {
        // Scrolling up (positive winit delta) at top → sign flip makes it
        // negative internal, rubber-banded at 0.
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0); // max = 600
        let now = Instant::now();
        s.position = 0.0;
        let pos = s.apply_wheel(50.0, now); // scroll up at top
                                            // Should rubber-band (position near 0 but negative).
        assert!(pos < 0.1, "at top, scroll up should rubber-band, got {pos}");
        assert!(
            s.position < 0.1,
            "position should still be at rubber-band near top"
        );
    }

    #[test]
    fn continuous_scroller_apply_wheel_at_bottom_bound_clamps() {
        // Scrolling down (negative winit delta) past bottom → rubber-band.
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0); // max = 600
        let now = Instant::now();
        s.position = 600.0;
        let pos = s.apply_wheel(-100.0, now); // scroll down past bottom
                                              // Should rubber-band near max.
        assert!(
            pos > 599.0,
            "at bottom, scroll down should rubber-band, got {pos}"
        );
        assert!(
            s.position > 599.0,
            "position should still be at rubber-band near bottom"
        );
    }

    #[test]
    fn continuous_scroller_min_max_clamp() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(200.0, 400.0); // content smaller than viewport
        assert_eq!(s.min_offset(), 0.0);
        assert_eq!(s.max_offset(), 0.0);
    }

    #[test]
    fn continuous_scroller_max_offset_correct() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0);
        assert_eq!(s.min_offset(), 0.0);
        assert_eq!(s.max_offset(), 600.0); // 1000 - 400
    }

    #[test]
    fn line_delta_to_px_converts_correctly() {
        assert_eq!(line_delta_to_px(3.0, 62.0), 186.0);
        assert_eq!(line_delta_to_px(-1.5, 20.0), -30.0);
        assert_eq!(line_delta_to_px(0.0, 100.0), 0.0);
    }

    #[test]
    fn continous_scroller_rubber_is_sublinear() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0); // max=600
        let r50 = s.rubber(50.0);
        let r500 = s.rubber(500.0);
        assert!(r50 < 50.0, "rubber attenuates overshoot");
        assert!(r500 < 500.0, "rubber attenuates large overshoot");
        assert!(
            r500 / 500.0 < r50 / 50.0,
            "larger pull must feel stiffer per pixel"
        );
    }

    #[test]
    fn continous_scroller_rubber_clamps_position() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0); // max=600, viewport=400
                                    // Move past max. In the rubber-band, visible position should be
                                    // less than the overshoot amount.
        let raw = s.max_offset() + 100.0; // 700
        let clamped = s.clamp_with_rubber(raw);
        assert!(clamped < raw);
        assert!(clamped > s.max_offset());
    }

    #[test]
    fn continous_scroller_spring_returns_to_bounds() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0);
        let now = Instant::now();
        s.apply_wheel(100.0, now); // at 100
                                   // Manually push out of bounds to simulate rubber-band release
        s.position = 700.0; // past max of 600
        s.phase = ContinuousPhase::Settling;
        s.settle_target = 600.0;
        s.velocity = 0.0;

        for _ in 0..2000 {
            s.step_once(
                1.0 / 120.0,
                Instant::now() + std::time::Duration::from_millis(100),
            );
            if s.phase == ContinuousPhase::Idle {
                break;
            }
        }
        assert_eq!(s.phase, ContinuousPhase::Idle);
        assert!((s.position - 600.0).abs() < s.cfg.settle_eps);
    }

    #[test]
    fn continous_scroller_inertia_decay() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(2000.0, 400.0);
        let now = Instant::now();
        s.drag_start(200.0, now);
        s.drag_move(205.0, now); // small move to build velocity sample
                                 // Fake velocity for inertia test
        s.velocity = 500.0;
        s.phase = ContinuousPhase::Inertial;

        let v_before = s.velocity;
        s.step_once(
            1.0 / 60.0,
            Instant::now() + std::time::Duration::from_millis(100),
        );
        assert!(s.velocity.abs() < v_before.abs(), "velocity should decay");
    }

    #[test]
    fn continous_scroller_60hz_120hz_consistent() {
        // Same drag sequence, different dt rates → final positions should be close.
        let run = |dt: f32, steps: usize| -> f32 {
            let mut s = ContinuousScroller::new(default_cfg());
            s.set_sizes(2000.0, 400.0);
            let now = Instant::now();
            s.drag_start(100.0, now);
            s.drag_move(150.0, now);
            s.drag_move(200.0, now);
            s.drag_end(now);

            for _ in 0..steps {
                s.step_once(dt, Instant::now() + std::time::Duration::from_millis(100));
                if s.phase == ContinuousPhase::Idle {
                    break;
                }
            }
            s.position
        };

        let pos_60 = run(1.0 / 60.0, 6000);
        let pos_120 = run(1.0 / 120.0, 12000);
        assert!(
            (pos_60 - pos_120).abs() < 3.0,
            "60Hz pos={pos_60}, 120Hz pos={pos_120}, diff too large"
        );
    }

    #[test]
    fn continous_scroller_ensure_visible_scrolls_into_view() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0); // max = 600

        // Item at bottom (900..950) is not visible when position=0
        s.position = 0.0;
        s.phase = ContinuousPhase::Idle;
        s.ensure_visible(900.0, 950.0);

        // Should have started a settle to make the item visible.
        assert_eq!(s.phase, ContinuousPhase::Settling);
        // Target: 900 - 400 = 500 (bottom aligned) or 550 (item_bottom - vp)?
        // ensure_visible uses: if item_bottom > position + vp_h → target = item_bottom - vp_h.
        // 950 > 0 + 400 → target = 950 - 400 = 550.
        assert!((s.settle_target - 550.0).abs() < 1.0);

        // Settle it
        for _ in 0..2000 {
            s.step_once(
                1.0 / 120.0,
                Instant::now() + std::time::Duration::from_millis(100),
            );
            if s.phase == ContinuousPhase::Idle {
                break;
            }
        }
        // After settling, item should be visible.
        assert!(s.position <= 900.0); // item_top visible
        assert!(s.position + 400.0 >= 950.0); // item_bottom visible
    }

    #[test]
    fn continous_scroller_ensure_visible_top_item() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0);
        s.position = 300.0;
        s.phase = ContinuousPhase::Idle;
        s.ensure_visible(50.0, 100.0); // item above viewport

        assert_eq!(s.phase, ContinuousPhase::Settling);
        assert!((s.settle_target - 50.0).abs() < 1.0);
    }

    #[test]
    fn continous_scroller_set_position_immediate() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0);
        s.set_position(300.0);
        assert_eq!(s.position, 300.0);
        assert_eq!(s.phase, ContinuousPhase::Idle);
        assert_eq!(s.velocity, 0.0);
    }

    #[test]
    fn continous_scroller_drag_move_is_1_to_1() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(2000.0, 400.0);
        let now = Instant::now();
        s.drag_start(100.0, now);
        let pos = s.drag_move(150.0, now); // +50 px pointer → +50 px content
        assert!((pos - 50.0).abs() < 1.0);
    }
}
