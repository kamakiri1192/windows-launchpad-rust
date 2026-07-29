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
    /// physical-contact deltas (see [`Scroller::apply_wheel_delta`]). Like
    /// [`Phase::Dragging`] the position is event-driven and remains so until an
    /// explicit terminal event. Native momentum is quarantined by the router.
    WheelGesture,
}

/// Phase of a wheel/trackpad gesture, mirroring winit's `TouchPhase` without
/// taking a winit dependency in this pure-physics module. The handler layer
/// converts before calling [`Scroller::apply_wheel_delta`].
///
/// - [`WheelPhase::Started`]: first delta of a new finger gesture (finger(s)
///   just touched the trackpad).
/// - [`WheelPhase::Moved`]: an intermediate physical-contact delta.
/// - [`WheelPhase::Ended`]: fingers lifted — start the snap immediately.
/// - [`WheelPhase::Cancelled`]: the gesture was interrupted; snap to the
///   nearest page from rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// Resolved dominant axis of an in-flight wheel gesture. macOS trackpads emit
/// small concurrent `dx`/`dy` even during a mostly-vertical or mostly-horizontal
/// swipe, so we lock the axis once the intent is clear and keep it until the
/// gesture ends — this stops a vertical scroll from nudging the page sideways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelDirection {
    /// Not enough movement yet to decide; accumulate deltas until a threshold.
    Undecided,
    /// Horizontal intent confirmed — deltas drive the grid pager.
    Horizontal,
    /// Vertical intent confirmed — deltas are ignored on the grid (Launchpad
    /// only pages horizontally), but the lock is kept so a later horizontal
    /// component mid-gesture can't reopen paging.
    Vertical,
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

/// Ring depth for the wheel-gesture velocity filter. 32 samples retain the full
/// 80 ms window even when a high-refresh trackpad delivers near 240 Hz.
const WHEEL_VEL_SAMPLES: usize = 32;

/// Release velocity is estimated from a deterministic, recency-weighted linear
/// regression over this window. The history stores the displayed position,
/// not raw input, so a rubber-band release hands its visible velocity to the
/// spring without a discontinuity.
const WHEEL_VEL_WINDOW_MICROS: u64 = 80_000;
const MICROS_PER_SECOND: f64 = 1_000_000.0;
const WHEEL_PROJECTION_HORIZON_SECS: f32 = 0.35;

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

/// Trackpad wheel-gesture tunables. Separate from [`PhysicsConfig`] so the
/// mouse-drag feel is never disturbed by trackpad tuning.
///
/// The snap reuses the same spring ODE as pointer drags (see
/// [`Phase::Settling`]) but with a higher damping ratio (ζ ≈ 0.9) so the page
/// critically settles without bounce. The target page is chosen once at
/// release by projecting the filtered visible velocity.
#[derive(Debug, Clone, Copy)]
pub struct WheelConfig {
    /// Scale applied to every trackpad delta (homepad `preciseScrollMultiplier`).
    /// Defaults to 1.0 now that release velocity is filtered through an 80 ms
    /// window; reduce it if raw deltas still feel too sensitive on your hardware.
    pub delta_multiplier: f32,
    /// Maximum visible rubber-band pull as a multiple of page extent.
    pub rubber_max_pages: f32,
    /// Shape parameter `a` in `M*u/(a+u)`. Larger values make the edge stiffer.
    pub rubber_curve_a: f32,
    /// Spring angular frequency ω₀ (rad/s) for the wheel snap. Lower than the
    /// pointer-drag `spring_omega` (20 vs 22): a page UI should feel smooth
    /// rather than sharp, avoiding the bouncy feel of higher ω₀.
    pub spring_omega: f32,
    /// Spring damping ratio ζ for the wheel snap. 1.0 is critically damped:
    /// release velocity stays continuous without adding a second bounce.
    pub spring_zeta: f32,
    /// Projection horizon used once, at physical contact release.
    pub projection_horizon: f32,
    /// Logical-pixel path length required to commit the H/V axis. The value is
    /// converted once using the gesture-start scale factor and then kept in
    /// physical pixels for the lifetime of that contact.
    pub axis_lock_distance: f32,
    /// Dominance ratio required to commit one axis over the other.
    pub axis_lock_ratio: f32,
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            // Raw trackpad deltas are now fed through a filtered velocity, so we
            // no longer damp them via the multiplier. Kept configurable for tuning.
            delta_multiplier: 1.0,
            rubber_max_pages: 0.20,
            rubber_curve_a: 0.30,
            // ω₀ ≈ 2π·f, f≈3.2 Hz → ω₀≈20. Combined with ζ=1.0 this gives a
            // smooth, non-oscillating settle.
            spring_omega: 20.0,
            spring_zeta: 1.0,
            projection_horizon: WHEEL_PROJECTION_HORIZON_SECS,
            axis_lock_distance: 10.0,
            axis_lock_ratio: 1.2,
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
    // ---- trackpad physical-contact paging state -------------------------
    /// Wheel tuning, separate from pointer-drag `cfg`.
    pub wheel_cfg: WheelConfig,
    /// Snap position the active wheel gesture started from. The gesture can
    /// move at most one page away from here, exactly like a pointer drag
    /// (`gesture_start_snap`).
    wheel_from_snap: f32,
    /// Live display position captured at physical contact start. This differs
    /// from `wheel_from_snap` when a new contact grabs an in-flight spring.
    wheel_anchor_position: f32,
    /// Accumulated, multiplier-scaled displacement of the active wheel gesture
    /// (physical px). `position` is derived as
    /// `clamp_wheel_rational(wheel_anchor_position + wheel_accumulated)`.
    wheel_accumulated: f32,
    /// Ring of `(microseconds-since-origin, displayed_position)` for the active
    /// wheel gesture, used to estimate a smoothed release velocity over an
    /// ~80 ms window (replaces the old single-event finite difference, which
    /// was dominated by the noisy terminal delta).
    wheel_samples: [(u64, f32); WHEEL_VEL_SAMPLES],
    wheel_sample_count: usize,
    /// Resolved dominant axis for the active wheel gesture. Stays `Undecided`
    /// until accumulated movement crosses the lock threshold, then sticks for
    /// the rest of the gesture so diagonal noise can't flip the page.
    wheel_direction: WheelDirection,
    /// Accumulated unsigned path length on each axis. Left/right reversals add
    /// horizontal evidence instead of cancelling it; only H/V is sticky.
    wheel_acc_x: f32,
    wheel_acc_y: f32,
    /// Gesture-start axis threshold converted to physical pixels. Capturing it
    /// once prevents a mid-contact DPI change from changing classification.
    wheel_axis_lock_distance: f32,
    /// True while a [`Phase::Settling`] was initiated by a wheel gesture, so
    /// the spring integrator picks the critically damped [`WheelConfig`] parameters
    /// instead of the pointer-drag [`PhysicsConfig`].
    settling_from_wheel: bool,
    wheel_target_decision_count: u32,
    wheel_spring_generation_count: u32,
    wheel_reanchor_count: u32,
    wheel_spring_id: Option<u64>,
    next_wheel_spring_id: u64,
    /// Filtered physical-contact velocity captured exactly once at release.
    /// Settling diagnostics keep this immutable initial value while
    /// `self.velocity` evolves under the spring.
    wheel_release_filtered_velocity: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollDiagnostics {
    /// Direct-manipulation position implied by the latest pointer after the
    /// same rubber-band function used by `drag_move`.
    pub input_target: Option<f32>,
    pub settle_target: Option<f32>,
    pub velocity_sample_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelDiagnostics {
    pub axis: WheelDirection,
    pub signed_displacement: f32,
    pub anchor_position: f32,
    pub filtered_velocity: f32,
    pub settle_target: Option<f32>,
    pub target_decision_count: u32,
    pub spring_generation_count: u32,
    pub reanchor_count: u32,
    pub spring_id: Option<u64>,
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
            wheel_anchor_position: 0.0,
            wheel_accumulated: 0.0,
            wheel_samples: [(0, 0.0); WHEEL_VEL_SAMPLES],
            wheel_sample_count: 0,
            wheel_direction: WheelDirection::Undecided,
            wheel_acc_x: 0.0,
            wheel_acc_y: 0.0,
            wheel_axis_lock_distance: WheelConfig::default().axis_lock_distance,
            settling_from_wheel: false,
            wheel_target_decision_count: 0,
            wheel_spring_generation_count: 0,
            wheel_reanchor_count: 0,
            wheel_spring_id: None,
            next_wheel_spring_id: 1,
            wheel_release_filtered_velocity: None,
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
        // A domain mutation invalidates an active contact's local one-page
        // domain. Treat it as an explicit cancellation: keep the live display
        // position, zero the release velocity, and return to the saved start
        // page clamped into the new bounds. Never silently re-anchor to the
        // nearest page under the user's fingers.
        if self.phase == Phase::WheelGesture {
            self.wheel_from_snap = self
                .wheel_from_snap
                .clamp(self.bounds.min_pos(), self.bounds.max_pos());
            self.begin_wheel_settle(true, None);
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
        if self.phase == Phase::WheelGesture
            || (self.phase == Phase::Settling && self.settling_from_wheel)
        {
            self.wheel_accumulated = 0.0;
            self.wheel_sample_count = 0;
            self.wheel_direction = WheelDirection::Undecided;
            self.wheel_acc_x = 0.0;
            self.wheel_acc_y = 0.0;
            self.settling_from_wheel = false;
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

    /// Feed one **physical contact** sample into the paging scroller.
    ///
    /// Momentum is deliberately not part of this engine contract. The input
    /// router must quarantine native momentum and only pass the physical
    /// `Started/Moved/Ended/Cancelled` sequence here.
    ///
    /// Sign convention: `dx` is the canonical horizontal physical-px delta as given to
    /// us by the handler (which has already accounted for macOS "natural
    /// scrolling" inversion). Positive `dx` scrolls content toward the previous
    /// page (position increases), matching a rightward pointer drag. `dy` is the
    /// vertical delta, used only for direction locking.
    pub fn apply_wheel_delta(&mut self, dx: f32, dy: f32, now: Instant, phase: WheelPhase) {
        self.apply_wheel_delta_scaled(dx, dy, 1.0, now, phase);
    }

    /// Feed one physical-contact sample using the scale factor captured at
    /// gesture start. `dx` and `dy` remain physical pixels; only the logical
    /// axis-lock threshold is scaled. Scale changes on non-`Started` samples
    /// are intentionally ignored until the next contact.
    pub fn apply_wheel_delta_scaled(
        &mut self,
        dx: f32,
        dy: f32,
        scale_factor: f32,
        now: Instant,
        phase: WheelPhase,
    ) {
        // A pointer drag owns the scroller; ignore wheel input while one is
        // active so the two can't fight over `position`.
        if self.phase == Phase::Dragging {
            return;
        }

        if phase == WheelPhase::Started {
            self.begin_wheel_session(now, scale_factor);
        }

        // A contact-less Moved must never reopen a session. This is also the
        // engine-side safety net if a momentum sample escapes router quarantine.
        if self.phase != Phase::WheelGesture {
            return;
        }

        if !dx.is_finite() || !dy.is_finite() {
            self.begin_wheel_settle(true, None);
            return;
        }

        let is_terminal = phase == WheelPhase::Ended || phase == WheelPhase::Cancelled;
        // A terminal event may carry a real final delta. Apply it exactly once
        // before release; a zero terminal is not added to velocity history.
        let movement = phase == WheelPhase::Moved
            || ((phase == WheelPhase::Started || is_terminal) && (dx != 0.0 || dy != 0.0));
        if movement && self.wheel_direction != WheelDirection::Vertical {
            let scaled = dx * self.wheel_cfg.delta_multiplier;
            let next_displacement = self.wheel_accumulated + scaled;
            if !next_displacement.is_finite() {
                self.begin_wheel_settle(true, None);
                return;
            }
            self.wheel_accumulated = next_displacement;
            let raw = self.wheel_anchor_position + self.wheel_accumulated;
            self.position = self.clamp_wheel_rational(raw);
            if scaled != 0.0 {
                self.push_wheel_sample(now);
            }
            self.update_wheel_direction(dx, dy);
        }

        if is_terminal {
            self.begin_wheel_settle(
                phase == WheelPhase::Cancelled,
                (phase == WheelPhase::Ended).then_some(now),
            );
        }
    }

    /// Accumulate raw deltas and resolve the gesture's dominant axis. Once
    /// locked, the direction stays fixed until [`begin_wheel_session`] resets
    /// it. Note this only ever *suppresses* horizontal motion (by resolving to
    /// `Vertical`): while still `Undecided` the caller treats the gesture as
    /// horizontal, so a light swipe stays responsive — this is the key
    /// difference from gating the page until the lock resolves.
    fn update_wheel_direction(&mut self, dx: f32, dy: f32) {
        if self.wheel_direction != WheelDirection::Undecided {
            return;
        }
        self.wheel_acc_x += dx.abs();
        self.wheel_acc_y += dy.abs();
        let ax = self.wheel_acc_x;
        let ay = self.wheel_acc_y;
        // Need enough total movement to rule out pure noise.
        if ax.max(ay) < self.wheel_axis_lock_distance {
            return;
        }
        // One axis must dominate the other by the configured ratio.
        if ax >= ay * self.wheel_cfg.axis_lock_ratio {
            self.wheel_direction = WheelDirection::Horizontal;
        } else if ay >= ax * self.wheel_cfg.axis_lock_ratio {
            self.wheel_direction = WheelDirection::Vertical;
        }
    }

    /// Open a physical-contact session from the live display position. Resets
    /// accumulation, velocity history, and the axis lock.
    fn begin_wheel_session(&mut self, now: Instant, scale_factor: f32) {
        // A new physical contact may grab an in-flight wheel spring. Preserve
        // the live x/v; the spring target remains the logical page anchor.
        self.wheel_from_snap = if self.phase == Phase::Settling && self.settling_from_wheel {
            self.settle_target
        } else {
            self.bounds.snap_target(self.position)
        };
        self.wheel_anchor_position = self.position;
        self.wheel_accumulated = 0.0;
        self.wheel_sample_count = 0;
        self.wheel_direction = WheelDirection::Undecided;
        self.wheel_acc_x = 0.0;
        self.wheel_acc_y = 0.0;
        let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        let threshold = self.wheel_cfg.axis_lock_distance * scale_factor;
        self.wheel_axis_lock_distance = if threshold.is_finite() && threshold >= 0.0 {
            threshold
        } else {
            WheelConfig::default().axis_lock_distance
        };
        self.wheel_target_decision_count = 0;
        self.wheel_spring_generation_count = 0;
        self.wheel_reanchor_count = 0;
        self.wheel_spring_id = None;
        self.wheel_release_filtered_velocity = None;
        self.phase = Phase::WheelGesture;
        self.last_time = Some(now);
        // Seed the ring with the gesture's origin so the first release estimate
        // has a real baseline rather than reporting 0.
        self.push_wheel_sample(now);
    }

    /// Project the filtered visible velocity once and start exactly one spring.
    fn begin_wheel_settle(&mut self, cancelled: bool, release_time: Option<Instant>) {
        let vertical = self.wheel_direction == WheelDirection::Vertical;
        let v = if cancelled || vertical {
            0.0
        } else {
            release_time
                .map(|now| self.estimate_wheel_velocity_at(now))
                .unwrap_or_else(|| self.estimate_wheel_velocity())
        };
        let target = if cancelled || vertical {
            self.wheel_from_snap
                .clamp(self.bounds.min_pos(), self.bounds.max_pos())
        } else {
            self.decide_wheel_target_page(v)
        };
        self.wheel_target_decision_count += 1;
        self.wheel_spring_generation_count += 1;
        self.wheel_spring_id = Some(self.next_wheel_spring_id);
        self.next_wheel_spring_id = self.next_wheel_spring_id.wrapping_add(1).max(1);
        // Preserve the filtered visible velocity exactly. Clipping here would
        // make the release handoff discontinuous and can create a false stop.
        self.velocity = v;
        self.wheel_release_filtered_velocity = Some(v);
        self.settle_target = target;
        self.settle_flick = (target - self.wheel_from_snap).abs() > 1.0 && v.abs() > 50.0;
        self.settling_from_wheel = true;
        self.phase = Phase::Settling;
    }

    /// Project once at release and choose the nearest of previous/current/next.
    /// No left/right sign is locked during contact.
    fn decide_wheel_target_page(&self, velocity: f32) -> f32 {
        let page = self.bounds.page_extent;
        if !page.is_finite() || page <= 0.0 {
            return 0.0;
        }
        let projected = self.position + velocity * self.wheel_cfg.projection_horizon;
        let q0 = (-self.wheel_from_snap / page).round() as isize;
        let max_q = self.bounds.page_count.saturating_sub(1) as isize;
        let mut best_q = q0.clamp(0, max_q);
        let mut best_distance = (-(best_q as f32) * page - projected).abs();
        for candidate in [q0 - 1, q0 + 1] {
            if !(0..=max_q).contains(&candidate) {
                continue;
            }
            let distance = (-(candidate as f32) * page - projected).abs();
            // Exact and near ties intentionally stay on q0.
            if distance + f32::EPSILON * page < best_distance {
                best_q = candidate;
                best_distance = distance;
            }
        }
        -(best_q as f32) * page
    }

    /// Bound tracking to a local ±1-page domain and apply the same strictly
    /// monotonic rational rubber curve at both local and content edges.
    fn clamp_wheel_rational(&self, raw: f32) -> f32 {
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

        if raw > lim_max {
            lim_max + self.wheel_rubber(raw - lim_max)
        } else if raw < lim_min {
            lim_min - self.wheel_rubber(lim_min - raw)
        } else {
            raw
        }
    }

    /// Strictly increasing, concave rational rubber curve:
    /// `R(x) = P * M * (x/P) / (a + x/P)`.
    #[inline]
    fn wheel_rubber(&self, overscroll: f32) -> f32 {
        let page = self.bounds.page_extent.max(1.0);
        let u = (overscroll.max(0.0) / page).min(f32::MAX.sqrt());
        page * self.wheel_cfg.rubber_max_pages * u / (self.wheel_cfg.rubber_curve_a + u)
    }

    /// Advance the simulation by real elapsed time. Returns the new phase.
    pub fn tick(&mut self, now: Instant) -> Phase {
        let dt = match self.last_time {
            None => {
                self.last_time = Some(now);
                return self.phase;
            }
            Some(t) => {
                let Some(elapsed) = now.checked_duration_since(t) else {
                    // A platform timestamp may briefly lead the redraw clock
                    // when queued native events are drained in one batch.
                    // The event has already been dispatched, so waiting for a
                    // malformed/future timestamp can freeze a return spring for
                    // seconds. Rebase once to the processing clock; the next
                    // display frame then advances the same single spring.
                    self.last_time = Some(now);
                    return self.phase;
                };
                let d = elapsed.as_secs_f32();
                self.last_time = Some(now);
                d.min(0.1) // clamp huge stalls to 100ms
            }
        };
        if dt <= 0.0 {
            return self.phase;
        }

        // Note: there is intentionally NO timeout that snaps a wheel gesture
        // while it's still in `Phase::WheelGesture`. On macOS the trackpad
        // reliably delivers an `Ended`/`Cancelled` event on finger lift, and
        // `apply_wheel_delta` starts the snap from there. A timeout would
        // misfire while the finger rests motionless on the glass (no deltas
        // arrive even though the finger is still down), snapping the page back
        // under the user's finger — which is exactly the bug it caused.

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

    pub fn wheel_diagnostics(&self) -> WheelDiagnostics {
        let filtered_velocity = if self.phase == Phase::Settling && self.settling_from_wheel {
            self.wheel_release_filtered_velocity
                .unwrap_or(self.velocity)
        } else {
            self.estimate_wheel_velocity()
        };
        WheelDiagnostics {
            axis: self.wheel_direction,
            signed_displacement: self.wheel_accumulated,
            anchor_position: self.wheel_anchor_position,
            filtered_velocity,
            settle_target: (self.phase == Phase::Settling && self.settling_from_wheel)
                .then_some(self.settle_target),
            target_decision_count: self.wheel_target_decision_count,
            spring_generation_count: self.wheel_spring_generation_count,
            reanchor_count: self.wheel_reanchor_count,
            spring_id: self.wheel_spring_id,
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
                // Wheel-initiated settles use the critical WheelConfig spring;
                // pointer drags retain their existing PhysicsConfig behavior.
                let (omega, zeta) = if self.settling_from_wheel {
                    (self.wheel_cfg.spring_omega, self.wheel_cfg.spring_zeta)
                } else {
                    (self.cfg.spring_omega, self.cfg.spring_zeta)
                };
                if self.settling_from_wheel && (zeta - 1.0).abs() <= f32::EPSILON {
                    // Exact critically-damped update. Unlike Euler integration,
                    // this has the semigroup property, so 60/120/144 Hz produce
                    // the same trajectory at equal wall-clock timestamps.
                    let displacement = self.position - self.settle_target;
                    let c2 = self.velocity + omega * displacement;
                    let decay = (-omega * dt).exp();
                    self.position = self.settle_target + (displacement + c2 * dt) * decay;
                    self.velocity = (c2 - omega * (displacement + c2 * dt)) * decay;
                } else {
                    let dx = self.position - self.settle_target;
                    let acc = -omega * omega * dx - 2.0 * zeta * omega * self.velocity;
                    self.velocity += acc * dt;
                    self.position += self.velocity * dt;
                }

                let remaining = self.position - self.settle_target;
                if remaining.abs() < self.cfg.settle_eps
                    && self.velocity.abs() < self.cfg.settle_eps
                {
                    self.position = self.settle_target;
                    self.velocity = 0.0;
                    self.settling_from_wheel = false;
                    self.phase = Phase::Idle;
                }
            }
        }
    }

    fn begin_settle_to(&mut self, target: f32, flick: bool) {
        self.settle_target = target;
        self.settle_flick = flick;
        self.settling_from_wheel = false;
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

    /// Push `(now, displayed_position)` onto the wheel velocity ring.
    /// Integer microseconds preserve event spacing even after a long resident
    /// lifetime; storing absolute `f32` seconds would eventually quantize
    /// distinct high-refresh samples to the same timestamp.
    fn push_wheel_sample(&mut self, now: Instant) {
        let t = now
            .checked_duration_since(self.clock_origin)
            .unwrap_or_default()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        if self.wheel_sample_count > 0 {
            let last_index = WHEEL_VEL_SAMPLES - 1;
            if self.wheel_samples[last_index].0 == t {
                self.wheel_samples[last_index].1 = self.position;
                return;
            }
        }
        // Shift the ring left and append at the tail.
        for i in 0..(WHEEL_VEL_SAMPLES - 1) {
            self.wheel_samples[i] = self.wheel_samples[i + 1];
        }
        self.wheel_samples[WHEEL_VEL_SAMPLES - 1] = (t, self.position);
        if self.wheel_sample_count < WHEEL_VEL_SAMPLES {
            self.wheel_sample_count += 1;
        }
    }

    /// Deterministic recency-weighted linear regression over displayed x.
    fn estimate_wheel_velocity(&self) -> f32 {
        self.estimate_wheel_velocity_f64() as f32
    }

    /// Estimate release velocity at the physical terminal timestamp without
    /// mutating the movement ring. Time spent stationary after the final
    /// movement linearly ages the filtered velocity out over the same 80 ms
    /// history window. This avoids a regression fit briefly growing steeper
    /// when an unchanged virtual endpoint is appended to a reversing trace.
    fn estimate_wheel_velocity_at(&self, now: Instant) -> f32 {
        if self.wheel_sample_count < 2 {
            return 0.0;
        }
        let release_us = self.wheel_timestamp_us(now);
        let last_movement_us = self.wheel_samples[WHEEL_VEL_SAMPLES - 1].0;
        let stationary_us = release_us.saturating_sub(last_movement_us);
        if stationary_us >= WHEEL_VEL_WINDOW_MICROS {
            return 0.0;
        }
        let retained = 1.0 - stationary_us as f64 / WHEEL_VEL_WINDOW_MICROS as f64;
        (self.estimate_wheel_velocity_f64() * retained) as f32
    }

    /// Full-precision form used by the deterministic trace contract. The
    /// rendering physics remains `f32`, but all timestamp and WLS arithmetic
    /// stays exact/f64 until the final engine handoff.
    fn estimate_wheel_velocity_f64(&self) -> f64 {
        if self.wheel_sample_count < 2 {
            return 0.0;
        }
        let last = self.wheel_samples[WHEEL_VEL_SAMPLES - 1];
        let first_valid = WHEEL_VEL_SAMPLES - self.wheel_sample_count;
        let mut window_start = first_valid;
        while window_start + 1 < WHEEL_VEL_SAMPLES
            && last.0.saturating_sub(self.wheel_samples[window_start].0) > WHEEL_VEL_WINDOW_MICROS
        {
            window_start += 1;
        }
        Self::estimate_wheel_velocity_samples(&self.wheel_samples[window_start..])
    }

    fn estimate_wheel_velocity_samples(samples: &[(u64, f32)]) -> f64 {
        let count = samples.len();
        if count < 2 {
            return 0.0;
        }
        let first = samples[0];
        let last = samples[count - 1];
        let span_us = last.0.saturating_sub(first.0);

        if count >= 3 && span_us >= 16_000 {
            let span = span_us as f64;
            let mut sum_w = 0.0_f64;
            let mut sum_t = 0.0_f64;
            let mut sum_x = 0.0_f64;
            for &(time, position) in samples {
                let weight = 1.0 + time.saturating_sub(first.0) as f64 / span;
                let relative_time = -(last.0.saturating_sub(time) as f64) / MICROS_PER_SECOND;
                sum_w += weight;
                sum_t += weight * relative_time;
                sum_x += weight * f64::from(position);
            }
            let mean_t = sum_t / sum_w;
            let mean_x = sum_x / sum_w;
            let mut covariance = 0.0_f64;
            let mut variance = 0.0_f64;
            for &(time, position) in samples {
                let weight = 1.0 + time.saturating_sub(first.0) as f64 / span;
                let relative_time = -(last.0.saturating_sub(time) as f64) / MICROS_PER_SECOND;
                let centered_t = relative_time - mean_t;
                covariance += weight * centered_t * (f64::from(position) - mean_x);
                variance += weight * centered_t * centered_t;
            }
            if variance > f64::EPSILON {
                return covariance / variance;
            }
        }

        let previous = samples[count - 2];
        let dt_us = last.0.saturating_sub(previous.0);
        if dt_us >= 1_000 {
            f64::from(last.1 - previous.1) * MICROS_PER_SECOND / dt_us as f64
        } else {
            0.0
        }
    }

    fn wheel_timestamp_us(&self, now: Instant) -> u64 {
        now.checked_duration_since(self.clock_origin)
            .unwrap_or_default()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64
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
                let Some(elapsed) = now.checked_duration_since(t) else {
                    self.last_time = Some(now);
                    return self.phase;
                };
                let d = elapsed.as_secs_f32();
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

    /// Cancel transient input/inertia state without moving the visible
    /// content. Focus loss uses this while settings/edit/folder policy keeps
    /// the window visible; an actual hide may reset the position separately.
    pub fn cancel_motion_preserving_position(&mut self) {
        self.velocity = 0.0;
        self.phase = ContinuousPhase::Idle;
        self.drag_anchor = self.position;
        self.drag_start_pointer = 0.0;
        self.samples = [(0.0, 0.0); VEL_SAMPLES];
        self.sample_count = 0;
        self.settle_target = self.position;
        self.last_time = None;
        self.last_wheel_time = None;
        self.clock_origin = Instant::now();
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
                    .map(|t| {
                        now.checked_duration_since(t)
                            .is_none_or(|elapsed| elapsed.as_secs_f32() < 0.06)
                    })
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

    // ---- wheel gesture paging (trackpad, Apple-style spring snap) ------

    /// Feed a wheel gesture: a sequence of `(dx, phase)` at ~16ms cadence.
    /// `dx` uses the "content-space" sign (positive = toward previous page),
    /// i.e. the handler has already inverted for natural scrolling. `dy` is 0
    /// (purely horizontal) unless the caller overrides it via `run_wheel_dy`.
    fn run_wheel(s: &mut Scroller, start_pos: f32, events: &[(f32, WheelPhase)]) {
        run_wheel_dy(s, start_pos, events, 0.0);
    }

    /// Like [`run_wheel`] but lets the caller inject a vertical component per
    /// event, exercising the direction lock.
    fn run_wheel_dy(s: &mut Scroller, start_pos: f32, events: &[(f32, WheelPhase)], dy: f32) {
        s.position = start_pos;
        let t0 = Instant::now();
        for (i, &(dx, phase)) in events.iter().enumerate() {
            s.apply_wheel_delta(dx, dy, t0 + Duration::from_millis(16 * i as u64), phase);
        }
    }

    #[test]
    fn wheel_delta_multiplier_is_one() {
        // delta_multiplier now defaults to 1.0 (raw deltas feed the filtered
        // velocity), so a 100px delta moves 100px of accumulated displacement.
        // Started carries dx=0 (contact notification); the motion is a Moved.
        let mut s = Scroller::new(bounds(4));
        s.position = -1000.0; // page 1, room both ways
        let now = Instant::now();
        s.apply_wheel_delta(0.0, 0.0, now, WheelPhase::Started);
        s.apply_wheel_delta(100.0, 0.0, now, WheelPhase::Moved);
        assert!(
            (s.position - (-1000.0 + 100.0)).abs() < 0.5,
            "100px × 1.0 should move 100px, got {}",
            s.position
        );
    }

    #[test]
    fn wheel_settle_preserves_velocity() {
        // A flick's release velocity must carry into the spring (the "glide"
        // feel), matching drag_end. Here a fast next-page flick on page 1.
        let mut s = Scroller::new(bounds(4));
        run_wheel(
            &mut s,
            -1000.0,
            &[
                (-5.0, WheelPhase::Started),
                (-120.0, WheelPhase::Moved), // ~-7500 px/s
                (0.0, WheelPhase::Ended),
            ],
        );
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.settle_target, -2000.0, "should target page 2");
        assert!(
            s.velocity < -100.0,
            "release velocity should carry into the spring, got {}",
            s.velocity
        );
        assert!(s.settling_from_wheel, "wheel settle must flag wheel origin");
    }

    #[test]
    fn wheel_settle_settles_smoothly_without_large_overshoot() {
        // With ζ = 0.90 (raised from 0.80), the snap intentionally does NOT
        // bounce hard: even a fast flick glides to the target and may overshoot
        // by at most a hair, never a large amount. This replaces the old
        // "must overshoot for Apple feel" assertion — the new tuning trades a
        // touch of overshoot for a calmer settle on a page grid.
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
        let target = s.settle_target; // -2000
        assert_eq!(target, -2000.0);
        let mut min_pos = s.position;
        let step = 1.0 / 120.0;
        for _ in 0..2000 {
            s.step_once(step);
            if s.position < min_pos {
                min_pos = s.position;
            }
            if s.phase == Phase::Idle {
                break;
            }
        }
        // No large overshoot: the furthest point past the target stays within
        // half a page (the old 0.80 ζ could overshoot by tens of px; 0.90 doesn't).
        assert!(
            min_pos > target - 500.0,
            "spring must not overshoot by more than half a page, min={min_pos} target={target}"
        );
        assert_eq!(s.phase, Phase::Idle);
        assert!((s.position - target).abs() < s.cfg.settle_eps);
    }

    #[test]
    fn wheel_projected_velocity_picks_next_page() {
        // A fast next-page flick projects nearest to page 2 even though the
        // displacement is modest. The
        // real terminal event arrives as Ended with dx=0, so we model it that
        // way (the -120px of motion is the last Moved, not the Ended).
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
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.settle_target, -2000.0);
    }

    #[test]
    fn wheel_low_velocity_stays_on_nearest_page() {
        // A slow, small swipe: displacement is well under the distance threshold
        // and the velocity is under the threshold, so it targets the current page.
        let mut s = Scroller::new(bounds(4));
        // -5px over one 16ms step → ~-312 px/s (under the 650 threshold) and
        // 5px << 0.32×1000 page, so neither gate trips → stay on page 1. The
        // motion is a Moved event; Ended arrives with dx=0 on real hardware.
        run_wheel(
            &mut s,
            -1000.0,
            &[
                (0.0, WheelPhase::Started),
                (-5.0, WheelPhase::Moved),
                (0.0, WheelPhase::Ended),
            ],
        );
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.settle_target, -1000.0, "slow small swipe stays on page 1");
    }

    #[test]
    fn wheel_caps_at_one_page_from_anchor() {
        // Even a violent swipe targets at most one page from the anchor.
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
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.settle_target, -3000.0, "must cap at one page ahead");
    }

    #[test]
    fn wheel_contactless_moved_is_dropped_after_release() {
        // Momentum isolation itself belongs to PagerInputRouter (and is covered
        // by its gesture-ID quarantine tests). This engine-side safety net must
        // still drop a Moved sample when no physical-contact session is active.
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
        assert_eq!(s.phase, Phase::Settling);
        let pos_at_release = s.position;
        s.apply_wheel_delta(
            -300.0,
            0.0,
            Instant::now() + Duration::from_millis(100),
            WheelPhase::Moved,
        );
        assert_eq!(
            s.position, pos_at_release,
            "a session-less Moved must not move position"
        );
        assert_eq!(s.phase, Phase::Settling);
    }

    #[test]
    fn wheel_contactless_terminal_does_not_block_next_started() {
        // PagerInputRouter normally consumes the old sequence terminal. If one
        // reaches the engine without an active contact, it is a no-op and must
        // not prevent a later physical Started from opening a fresh session.
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
        let t0 = Instant::now() + Duration::from_millis(200);
        let released_position = s.position;
        s.apply_wheel_delta(-300.0, 0.0, t0, WheelPhase::Moved);
        s.apply_wheel_delta(
            -300.0,
            0.0,
            t0 + Duration::from_millis(16),
            WheelPhase::Ended,
        );
        s.apply_wheel_delta(
            -10.0,
            0.0,
            t0 + Duration::from_millis(32),
            WheelPhase::Started,
        );
        assert_eq!(
            s.position,
            released_position - 10.0,
            "the new Started delta must be applied exactly once"
        );
        assert_eq!(
            s.phase,
            Phase::WheelGesture,
            "a new physical Started must open a fresh session"
        );
    }

    #[test]
    fn wheel_tick_before_future_native_release_timestamp_is_safe() {
        let mut s = Scroller::new(bounds(4));
        s.position = -1000.0;
        let future = s.clock_origin + Duration::from_secs(10);
        s.apply_wheel_delta(0.0, 0.0, future, WheelPhase::Started);
        s.apply_wheel_delta(
            -200.0,
            0.0,
            future + Duration::from_millis(16),
            WheelPhase::Moved,
        );
        s.apply_wheel_delta(
            0.0,
            0.0,
            future + Duration::from_millis(32),
            WheelPhase::Ended,
        );
        let release_position = s.position;

        assert_eq!(
            s.tick(s.clock_origin + Duration::from_secs(1)),
            Phase::Settling
        );
        assert_eq!(s.position.to_bits(), release_position.to_bits());
        assert!(s.position.is_finite());
        assert!(s.velocity.is_finite());
    }

    #[test]
    fn wheel_future_release_timestamp_cannot_freeze_return_spring_until_that_timestamp() {
        let mut s = Scroller::new(bounds(4));
        s.position = -1000.0;
        let processing_now = s.clock_origin + Duration::from_secs(1);
        let malformed_future = processing_now + Duration::from_secs(10);
        s.apply_wheel_delta(0.0, 0.0, malformed_future, WheelPhase::Started);
        s.apply_wheel_delta(
            -80.0,
            0.0,
            malformed_future + Duration::from_millis(16),
            WheelPhase::Moved,
        );
        s.apply_wheel_delta(
            0.0,
            0.0,
            malformed_future + Duration::from_millis(200),
            WheelPhase::Ended,
        );
        assert_eq!(s.settle_target, -1000.0);
        let release_position = s.position;

        // The first frame may only rebase the invalid future event clock, but
        // the next ordinary display frame must advance the single return spring.
        s.tick(processing_now);
        s.tick(processing_now + Duration::from_millis(16));
        assert!(
            s.position > release_position,
            "future event time froze the return spring at {}",
            s.position
        );
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.wheel_diagnostics().spring_generation_count, 1);
    }

    #[test]
    fn wheel_stay_target_traces_never_freeze_away_from_snap() {
        fn run(start: f32, dx: f32, hold_ms: u64, expected_target: f32) {
            let mut s = Scroller::new(bounds(4));
            s.position = start;
            let t0 = s.clock_origin + Duration::from_secs(1);
            s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);
            let moved_at = t0 + Duration::from_millis(16);
            s.apply_wheel_delta(dx, 0.0, moved_at, WheelPhase::Moved);
            let release_at = moved_at + Duration::from_millis(hold_ms);
            s.apply_wheel_delta(0.0, 0.0, release_at, WheelPhase::Ended);

            assert_eq!(s.phase, Phase::Settling);
            assert_eq!(s.settle_target, expected_target);
            assert_eq!(s.wheel_diagnostics().spring_generation_count, 1);
            let release_position = s.position;
            let mut changed_early = false;
            for frame in 1..=240 {
                let before = s.position;
                s.tick(release_at + Duration::from_micros(8_333 * frame));
                if frame <= 2 && s.position.to_bits() != before.to_bits() {
                    changed_early = true;
                }
                if s.phase == Phase::Idle {
                    break;
                }
            }
            assert!(
                changed_early,
                "stay spring did not move in its first two frames: start={start}, dx={dx}, release={release_position}"
            );
            assert_eq!(s.phase, Phase::Idle);
            assert!((s.position - expected_target).abs() < s.cfg.settle_eps);
            assert_eq!(s.wheel_diagnostics().spring_generation_count, 1);
        }

        // Interior, genuinely light movement with a low projected velocity.
        run(-1000.0, -8.0, 16, -1000.0);
        // A short flick followed by a deliberate hold must return from rest.
        run(-1000.0, -80.0, 80, -1000.0);
        // First-page rubber-band return uses the same single-spring contract.
        run(0.0, 80.0, 80, 0.0);
    }

    #[test]
    fn wheel_return_to_exact_origin_finishes_without_false_idle_or_extra_spring() {
        let mut s = Scroller::new(bounds(4));
        s.position = -1000.0;
        let t0 = s.clock_origin + Duration::from_secs(1);
        s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);
        s.apply_wheel_delta(
            -20.0,
            0.0,
            t0 + Duration::from_millis(16),
            WheelPhase::Moved,
        );
        s.apply_wheel_delta(20.0, 0.0, t0 + Duration::from_millis(32), WheelPhase::Moved);
        let release_at = t0 + Duration::from_millis(112);
        s.apply_wheel_delta(0.0, 0.0, release_at, WheelPhase::Ended);

        assert_eq!(s.position, -1000.0);
        assert_eq!(s.settle_target, -1000.0);
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.wheel_diagnostics().spring_generation_count, 1);
        s.tick(release_at + Duration::from_millis(8));
        assert_eq!(s.phase, Phase::Idle);
        assert_eq!(s.position, -1000.0);
        assert_eq!(s.wheel_diagnostics().spring_generation_count, 1);
    }

    #[test]
    fn wheel_rubber_homepad_curve_stiffens_fast() {
        // homepad rubber: at the first page, pulling toward the previous page
        // attenuates fast (100px input → <100px move). With delta_multiplier
        // now 1.0, the input is the full 100px and the curve must still attenuate.
        // Started carries dx=0; the 100px motion arrives as a Moved.
        let mut s = Scroller::new(bounds(4));
        run_wheel(
            &mut s,
            0.0,
            &[(0.0, WheelPhase::Started), (100.0, WheelPhase::Moved)],
        );
        assert!(s.position > 0.0, "should pull past the edge a bit");
        assert!(
            s.position < 100.0,
            "rubber band must attenuate (100px input → <100px), got {}",
            s.position
        );
    }

    #[test]
    fn wheel_sustained_edge_momentum_asymptotes() {
        // The rational rubber-band is strictly increasing and asymptotes to
        // 20% of a page regardless of how many deltas arrive.
        let mut s = Scroller::new(bounds(4));
        let mut events = vec![(10.0, WheelPhase::Started)];
        for _ in 0..30 {
            events.push((100.0, WheelPhase::Moved));
        }
        run_wheel(&mut s, 0.0, &events);
        assert!(
            s.position < 200.0,
            "sustained edge swipe must asymptote (<20% of page), got {}",
            s.position
        );
    }

    #[test]
    fn wheel_no_ended_stays_in_gesture() {
        // There is no longer a timeout that snaps a wheel gesture: the page must
        // follow the finger until an `Ended`/`Cancelled` arrives. This is what
        // stops the page from snapping back while the finger rests motionless
        // on the trackpad (the bug the timeout caused).
        let mut s = Scroller::new(bounds(4));
        run_wheel(
            &mut s,
            -1000.0,
            &[(-5.0, WheelPhase::Started), (-120.0, WheelPhase::Moved)],
        );
        assert_eq!(s.phase, Phase::WheelGesture);
        // Advance the clock well past the old 120 ms timeout.
        let mut t = Instant::now() + Duration::from_millis(64);
        for _ in 0..40 {
            s.tick(t);
            t += Duration::from_millis(16);
        }
        assert_eq!(
            s.phase,
            Phase::WheelGesture,
            "no Ended → must stay in gesture (no timeout snap)"
        );
    }

    #[test]
    fn wheel_ignored_during_pointer_drag() {
        let mut s = Scroller::new(bounds(4));
        s.position = -1000.0;
        s.drag_start(500.0);
        s.drag_move(450.0);
        let pos_before = s.position;
        s.apply_wheel_delta(-500.0, 0.0, Instant::now(), WheelPhase::Moved);
        assert_eq!(s.phase, Phase::Dragging);
        assert_eq!(s.position, pos_before);
    }

    #[test]
    fn pointer_drag_cancels_wheel_settle() {
        // A pointer press during a wheel settle cancels it and starts a drag.
        let mut s = Scroller::new(bounds(4));
        run_wheel(
            &mut s,
            -1000.0,
            &[(-5.0, WheelPhase::Started), (-120.0, WheelPhase::Ended)],
        );
        assert_eq!(s.phase, Phase::Settling);
        let live = s.position;
        s.drag_start(300.0);
        assert_eq!(s.phase, Phase::Dragging);
        assert!((s.position - live).abs() < 1.0);
        assert!(
            !s.settling_from_wheel,
            "wheel flag cleared on drag takeover"
        );
    }

    #[test]
    fn wheel_is_animating_in_gesture() {
        let mut s = Scroller::new(bounds(4));
        assert!(!s.is_animating());
        s.apply_wheel_delta(-10.0, 0.0, Instant::now(), WheelPhase::Started);
        assert!(s.is_animating());
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
    fn continuous_scroller_focus_cancel_preserves_live_position() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0);
        let now = Instant::now();
        s.apply_wheel(-180.0, now);
        let live_position = s.position;
        s.velocity = 240.0;
        s.phase = ContinuousPhase::Inertial;

        s.cancel_motion_preserving_position();

        assert_eq!(s.position.to_bits(), live_position.to_bits());
        assert_eq!(s.velocity, 0.0);
        assert_eq!(s.phase, ContinuousPhase::Idle);
        assert_eq!(s.sample_count, 0);
        assert!(s.last_time.is_none());
        assert!(s.last_wheel_time.is_none());
    }

    #[test]
    fn continuous_scroller_tick_before_future_wheel_timestamp_is_safe() {
        let mut s = ContinuousScroller::new(default_cfg());
        s.set_sizes(1000.0, 400.0);
        let now = Instant::now();
        let future = now + Duration::from_secs(10);
        s.apply_wheel(100.0, future);
        let event_position = s.position;

        assert_eq!(s.tick(now), ContinuousPhase::Settling);
        assert_eq!(s.position.to_bits(), event_position.to_bits());
        assert!(s.position.is_finite());
        assert!(s.velocity.is_finite());
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

    // ---- wheel paging tests -------------------------------------------------
    //
    // These cover the trackpad paging path: the 80 ms velocity window, the
    // distance+velocity target-page decision, and the direction lock. They
    // manipulate the scroller's private gesture state directly so the physics
    // is deterministic and doesn't depend on wall-clock timing.

    /// Build a scroller parked on page 0 of a 3-page grid (page_extent 1000).
    fn wheel_scroller() -> Scroller {
        let mut s = Scroller::new(bounds(3));
        s.set_bounds(bounds(3));
        s
    }

    /// Set up a gesture anchored on the current page, with the accumulated
    /// displacement and velocity-ring samples given. Times are seconds since
    /// the scroller's clock origin (arbitrary, as long as the window math holds).
    fn arm_wheel_gesture(
        s: &mut Scroller,
        accumulated: f32,
        samples: &[(u64, f32)],
        direction: WheelDirection,
    ) {
        s.wheel_from_snap = s.bounds.snap_target(s.position);
        s.wheel_anchor_position = s.position;
        s.wheel_accumulated = accumulated;
        s.position = s.clamp_wheel_rational(s.wheel_anchor_position + accumulated);
        s.wheel_sample_count = 0;
        for &(t, p) in samples {
            // Shift left and append, mirroring push_wheel_sample.
            for i in 0..(WHEEL_VEL_SAMPLES - 1) {
                s.wheel_samples[i] = s.wheel_samples[i + 1];
            }
            s.wheel_samples[WHEEL_VEL_SAMPLES - 1] = (t, p);
            if s.wheel_sample_count < WHEEL_VEL_SAMPLES {
                s.wheel_sample_count += 1;
            }
        }
        s.wheel_direction = direction;
        s.wheel_acc_x = 0.0;
        s.wheel_acc_y = 0.0;
    }

    #[test]
    fn wheel_velocity_window_ignores_stale_sample() {
        // A sample older than the 80 ms window must not skew the slope. Two
        // scenarios with the same recent samples but different stale histories
        // should report the same filtered velocity.
        let mut s = wheel_scroller();

        // Recent samples at t=1.060 and t=1.080 (positions 6 and 8): slope = 100.
        let recent = [(1_060_000, 6.0), (1_080_000, 8.0)];

        arm_wheel_gesture(&mut s, 8.0, &recent, WheelDirection::Horizontal);
        let v_in_window = s.estimate_wheel_velocity();

        // Prepend a stale sample well outside the 80 ms window (t=0.800,
        // position 1000) that would drastically change the slope if included.
        let with_stale = [(800_000, 1000.0), (1_060_000, 6.0), (1_080_000, 8.0)];
        arm_wheel_gesture(&mut s, 8.0, &with_stale, WheelDirection::Horizontal);
        let v_with_stale = s.estimate_wheel_velocity();

        assert!(
            (v_in_window - v_with_stale).abs() < 1.0,
            "stale sample leaked into velocity: in-window={v_in_window}, with-stale={v_with_stale}"
        );
        // And the value should be ~100 px/s, the slope of the recent samples.
        assert!(
            (v_in_window - 100.0).abs() < 5.0,
            "expected ~100 px/s, got {v_in_window}"
        );
    }

    #[test]
    fn wheel_velocity_single_sample_is_zero() {
        // With fewer than two samples we can't form a slope.
        let mut s = wheel_scroller();
        arm_wheel_gesture(&mut s, 5.0, &[(80_000, 5.0)], WheelDirection::Horizontal);
        assert_eq!(s.estimate_wheel_velocity(), 0.0);
    }

    #[test]
    fn wheel_velocity_coalesces_same_timestamp_and_ages_at_zero_terminal() {
        let mut s = wheel_scroller();
        s.position = -1000.0;
        let t0 = s.clock_origin + Duration::from_secs(1);
        s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);
        let t1 = t0 + Duration::from_millis(16);
        s.apply_wheel_delta(-10.0, 0.0, t1, WheelPhase::Moved);
        s.apply_wheel_delta(-10.0, 0.0, t1, WheelPhase::Moved);
        assert_eq!(s.wheel_sample_count, 2);
        let before_terminal = s.estimate_wheel_velocity();
        assert!((before_terminal + 1250.0).abs() < 0.1);
        s.apply_wheel_delta(0.0, 0.0, t0 + Duration::from_millis(32), WheelPhase::Ended);
        assert_eq!(s.wheel_sample_count, 2);
        assert!(
            (s.velocity - before_terminal * 0.8).abs() < 0.1,
            "16ms stationary release should retain 80% of velocity: before={before_terminal}, release={}",
            s.velocity
        );
    }

    #[test]
    fn wheel_release_velocity_decays_monotonically_while_finger_holds_still() {
        fn release_velocity_after_hold(hold_ms: u64) -> f32 {
            let mut s = wheel_scroller();
            s.position = -1000.0;
            let t0 = s.clock_origin + Duration::from_secs(1);
            s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);
            let movement_time = t0 + Duration::from_millis(16);
            s.apply_wheel_delta(-200.0, 0.0, movement_time, WheelPhase::Moved);
            s.apply_wheel_delta(
                0.0,
                0.0,
                movement_time + Duration::from_millis(hold_ms),
                WheelPhase::Ended,
            );
            s.velocity
        }

        let velocities = [16, 40, 80, 120].map(release_velocity_after_hold);
        assert!(
            velocities
                .windows(2)
                .all(|pair| pair[0].abs() >= pair[1].abs()),
            "stationary hold must monotonically age velocity out: {velocities:?}"
        );
        assert!(velocities[0] < 0.0);
        assert_eq!(velocities[2], 0.0);
        assert_eq!(velocities[3], 0.0);
    }

    #[test]
    fn wheel_long_hold_releases_from_rest_to_nearest_page_without_stale_reacceleration() {
        let mut s = wheel_scroller();
        s.position = -1000.0;
        let t0 = s.clock_origin + Duration::from_secs(1);
        s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);
        let movement_time = t0 + Duration::from_millis(16);
        s.apply_wheel_delta(-200.0, 0.0, movement_time, WheelPhase::Moved);
        let release_position = s.position;

        s.apply_wheel_delta(
            0.0,
            0.0,
            movement_time + Duration::from_millis(500),
            WheelPhase::Ended,
        );

        assert_eq!(s.velocity, 0.0);
        assert_eq!(s.wheel_diagnostics().filtered_velocity, s.velocity);
        assert_eq!(s.settle_target, -1000.0);
        assert_eq!(s.position, release_position);
        s.tick(movement_time + Duration::from_millis(516));
        assert!(
            s.position > release_position,
            "spring may return to the nearest page, but must not resume the stale leftward flick"
        );
        assert!(s.velocity > 0.0);
    }

    #[test]
    fn wheel_nonzero_started_delta_is_applied_once() {
        let mut s = wheel_scroller();
        s.position = -1000.0;
        let t0 = s.clock_origin + Duration::from_secs(1);

        s.apply_wheel_delta_scaled(12.0, 1.0, 1.0, t0, WheelPhase::Started);

        assert_eq!(s.phase, Phase::WheelGesture);
        assert_eq!(s.wheel_anchor_position, -1000.0);
        assert_eq!(s.wheel_accumulated, 12.0);
        assert_eq!(s.position, -988.0);
        assert_eq!(s.wheel_acc_x, 12.0);
        assert_eq!(s.wheel_acc_y, 1.0);
        assert_eq!(s.wheel_direction, WheelDirection::Horizontal);
        // The anchor seed and the Started movement share a timestamp, so the
        // WLS ring keeps the latest position once rather than double-counting.
        assert_eq!(s.wheel_sample_count, 1);
        assert_eq!(s.wheel_samples[WHEEL_VEL_SAMPLES - 1].1, -988.0);
        let tracking = s.wheel_diagnostics();
        assert_eq!(tracking.filtered_velocity, 0.0);
        assert_eq!(tracking.target_decision_count, 0);
        assert_eq!(tracking.spring_generation_count, 0);

        s.apply_wheel_delta_scaled(
            0.0,
            0.0,
            2.0,
            t0 + Duration::from_millis(16),
            WheelPhase::Ended,
        );
        let released = s.wheel_diagnostics();
        assert_eq!(released.target_decision_count, 1);
        assert_eq!(released.spring_generation_count, 1);
    }

    #[test]
    fn wheel_zero_started_delta_seeds_wls_without_moving() {
        let mut s = wheel_scroller();
        s.position = -1000.0;
        let t0 = s.clock_origin + Duration::from_secs(1);

        s.apply_wheel_delta_scaled(0.0, 0.0, 1.0, t0, WheelPhase::Started);
        assert_eq!(s.position, -1000.0);
        assert_eq!(s.wheel_accumulated, 0.0);
        assert_eq!(s.wheel_sample_count, 1);

        s.apply_wheel_delta_scaled(
            10.0,
            0.0,
            2.0,
            t0 + Duration::from_millis(16),
            WheelPhase::Moved,
        );
        assert_eq!(s.position, -990.0);
        assert_eq!(s.wheel_sample_count, 2);
        assert!((s.estimate_wheel_velocity() - 625.0).abs() < 0.1);
    }

    #[test]
    fn wheel_projected_position_before_midpoint_stays() {
        // A zero-velocity release at 20% remains nearest to the anchor page.
        let mut s = wheel_scroller();
        // position decreases toward later pages; move 200px toward "next".
        arm_wheel_gesture(&mut s, -200.0, &[], WheelDirection::Horizontal);
        let target = s.decide_wheel_target_page(0.0);
        assert_eq!(
            target, s.wheel_from_snap,
            "release before the midpoint must not flip"
        );
    }

    #[test]
    fn wheel_projected_position_past_midpoint_advances() {
        // Projection with zero velocity reduces to nearest-page selection.
        // Moving past the midpoint selects the adjacent page.
        let mut s = wheel_scroller();
        arm_wheel_gesture(&mut s, -600.0, &[], WheelDirection::Horizontal);
        let target = s.decide_wheel_target_page(0.0);
        assert_eq!(
            target,
            s.wheel_from_snap - 1000.0,
            "release past the midpoint must advance to next page"
        );
    }

    #[test]
    fn wheel_projected_velocity_can_override_displacement_sign() {
        // On the middle page a strong opposite-sign release velocity can select
        // next even when the visible displacement points toward previous.
        let mut s = wheel_scroller();
        s.position = -1000.0;
        arm_wheel_gesture(&mut s, 50.0, &[], WheelDirection::Horizontal);
        let target = s.decide_wheel_target_page(-2000.0);
        assert_eq!(
            target,
            s.wheel_from_snap - 1000.0,
            "energetic flick must advance in the velocity direction"
        );
    }

    #[test]
    fn wheel_target_clamped_to_one_page() {
        // Even with a huge displacement and velocity, the target can't jump
        // more than one page from the anchor.
        let mut s = wheel_scroller();
        arm_wheel_gesture(&mut s, -5000.0, &[], WheelDirection::Horizontal);
        let target = s.decide_wheel_target_page(-9999.0);
        assert_eq!(target, s.wheel_from_snap - 1000.0);
    }

    #[test]
    fn wheel_target_clamped_to_content_bounds() {
        // On the first page of a 3-page grid, a "previous page" intent has
        // nowhere to go: the target must clamp to page 0 (position 0).
        let mut s = wheel_scroller();
        // positive displacement + positive velocity → previous page
        arm_wheel_gesture(&mut s, 400.0, &[], WheelDirection::Horizontal);
        let target = s.decide_wheel_target_page(800.0);
        assert_eq!(target, 0.0, "can't page before the first page");
    }

    #[test]
    fn wheel_direction_lock_horizontal() {
        // dx dominates dy by more than the 1.2 ratio and exceeds 6px total:
        // the gesture locks horizontal. Started arrives with dx=0 (contact
        // notification); the motion is delivered as Moved events.
        let mut s = wheel_scroller();
        let now = Instant::now();
        s.apply_wheel_delta(0.0, 0.0, now, WheelPhase::Started);
        s.apply_wheel_delta(12.0, 2.0, now, WheelPhase::Moved);
        assert_eq!(s.wheel_direction, WheelDirection::Horizontal);
    }

    #[test]
    fn wheel_axis_lock_distance_uses_gesture_start_scale() {
        for scale_factor in [1.0_f32, 1.5, 2.0] {
            let mut s = wheel_scroller();
            s.position = -1000.0;
            let t0 = s.clock_origin + Duration::from_secs(1);
            s.apply_wheel_delta_scaled(0.0, 0.0, scale_factor, t0, WheelPhase::Started);
            assert_eq!(
                s.wheel_axis_lock_distance,
                s.wheel_cfg.axis_lock_distance * scale_factor
            );

            // Nine logical pixels are insufficient at every DPI. Deliberately
            // pass a different later scale to prove the start value is sticky.
            s.apply_wheel_delta_scaled(
                9.0 * scale_factor,
                0.0,
                scale_factor * 10.0,
                t0 + Duration::from_millis(16),
                WheelPhase::Moved,
            );
            assert_eq!(s.wheel_direction, WheelDirection::Undecided);

            s.apply_wheel_delta_scaled(
                2.0 * scale_factor,
                0.0,
                0.5,
                t0 + Duration::from_millis(32),
                WheelPhase::Moved,
            );
            assert_eq!(s.wheel_direction, WheelDirection::Horizontal);
        }
    }

    #[test]
    fn wheel_invalid_gesture_start_scale_falls_back_to_one() {
        for scale_factor in [0.0_f32, -1.0, f32::NAN, f32::INFINITY] {
            let mut s = wheel_scroller();
            let t0 = s.clock_origin + Duration::from_secs(1);
            s.apply_wheel_delta_scaled(0.0, 0.0, scale_factor, t0, WheelPhase::Started);
            assert_eq!(
                s.wheel_axis_lock_distance,
                WheelConfig::default().axis_lock_distance
            );
            s.apply_wheel_delta_scaled(
                9.0,
                0.0,
                2.0,
                t0 + Duration::from_millis(16),
                WheelPhase::Moved,
            );
            assert_eq!(s.wheel_direction, WheelDirection::Undecided);
            s.apply_wheel_delta_scaled(
                1.0,
                0.0,
                2.0,
                t0 + Duration::from_millis(32),
                WheelPhase::Moved,
            );
            assert_eq!(s.wheel_direction, WheelDirection::Horizontal);
        }
    }

    #[test]
    fn wheel_direction_lock_vertical() {
        // The deciding sample remains a render-only preview. Once vertical is
        // sticky, subsequent horizontal noise freezes that preview.
        let mut s = wheel_scroller();
        let now = Instant::now();
        let pos_before = s.position;
        s.apply_wheel_delta(0.0, 0.0, now, WheelPhase::Started);
        s.apply_wheel_delta(2.0, 12.0, now, WheelPhase::Moved);
        assert_eq!(s.wheel_direction, WheelDirection::Vertical);
        assert!(s.position > pos_before, "deciding sample must be previewed");
        let frozen = s.position;
        s.apply_wheel_delta(50.0, 0.0, now, WheelPhase::Moved);
        assert_eq!(s.position, frozen, "vertical lock freezes preview");
        s.apply_wheel_delta(0.0, 0.0, now, WheelPhase::Ended);
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.settle_target, pos_before);
        assert_eq!(s.velocity, 0.0);
    }

    #[test]
    fn wheel_direction_lock_is_sticky() {
        // Once locked vertical, subsequent horizontal deltas must not reopen
        // paging — the lock holds until the gesture ends.
        let mut s = wheel_scroller();
        let now = Instant::now();
        s.apply_wheel_delta(0.0, 0.0, now, WheelPhase::Started);
        s.apply_wheel_delta(2.0, 12.0, now, WheelPhase::Moved);
        assert_eq!(s.wheel_direction, WheelDirection::Vertical);
        let pos_before = s.position;
        // A large horizontal delta mid-gesture is ignored.
        s.apply_wheel_delta(200.0, 1.0, now, WheelPhase::Moved);
        assert_eq!(s.wheel_direction, WheelDirection::Vertical);
        assert_eq!(s.position, pos_before, "locked gesture ignores new axis");
    }

    #[test]
    fn wheel_direction_lock_resets_per_gesture() {
        // A new Started event opens a fresh session and clears the lock.
        let mut s = wheel_scroller();
        let now = Instant::now();
        s.apply_wheel_delta(0.0, 0.0, now, WheelPhase::Started);
        s.apply_wheel_delta(2.0, 12.0, now, WheelPhase::Moved);
        s.apply_wheel_delta(0.0, 0.0, now, WheelPhase::Ended);
        assert_eq!(s.phase, Phase::Settling);
        // New gesture: now horizontal should win.
        s.apply_wheel_delta(0.0, 0.0, now, WheelPhase::Started);
        s.apply_wheel_delta(12.0, 2.0, now, WheelPhase::Moved);
        assert_eq!(s.wheel_direction, WheelDirection::Horizontal);
    }

    #[test]
    fn wheel_direction_undecided_still_moves_page() {
        // Movement under the 6px lock threshold keeps the direction Undecided,
        // but — by design — the page must still follow the horizontal delta.
        // This is what keeps a light swipe responsive: we never withhold the
        // page waiting for the lock to resolve.
        let mut s = wheel_scroller();
        s.position = -1000.0; // page 1, mid-content so deltas aren't rubber-banded
        let now = Instant::now();
        s.apply_wheel_delta(0.0, 0.0, now, WheelPhase::Started);
        let pos_before = s.position;
        s.apply_wheel_delta(3.0, 1.0, now, WheelPhase::Moved);
        assert_eq!(s.wheel_direction, WheelDirection::Undecided);
        assert!(
            (s.position - (pos_before + 3.0)).abs() < 1.0,
            "Undecided gesture must still move the page by the horizontal delta, got {}",
            s.position - pos_before
        );
    }

    #[test]
    fn wheel_horizontal_gesture_moves_page() {
        // A horizontal-locked gesture with a real delta moves position by the
        // scaled delta (delta_multiplier defaults to 1.0 now). We measure the
        // delta of a single Moved event in isolation, after the lock is already
        // resolved, so the lock's own movement doesn't pollute the measurement.
        // Start mid-content (page 1) so small deltas stay out of the rubber-band
        // region and move 1:1.
        let mut s = wheel_scroller();
        s.position = -1000.0; // page 1
        let now = Instant::now();
        // Lock horizontal first via a Moved event (Started carries dx=0).
        s.apply_wheel_delta(0.0, 0.0, now, WheelPhase::Started);
        s.apply_wheel_delta(12.0, 0.0, now, WheelPhase::Moved);
        assert_eq!(s.wheel_direction, WheelDirection::Horizontal);
        let pos_after_lock = s.position;
        // A -50px Moved must move position by exactly 50px (multiplier 1.0).
        s.apply_wheel_delta(-50.0, 0.0, now, WheelPhase::Moved);
        assert!(
            (s.position - (pos_after_lock - 50.0)).abs() < 1.0,
            "expected 50px toward next page, got {}",
            s.position - pos_after_lock
        );
    }

    #[test]
    fn wheel_small_oscillation_then_reverse_tracks_through_zero() {
        let mut s = wheel_scroller();
        s.position = -1000.0;
        let page = s.bounds.page_extent;
        let t0 = s.clock_origin + Duration::from_secs(1);
        s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);

        let trace = [
            (20.0, 16, 0.020, 1.25_f64),
            (-15.0, 32, 0.005, 0.025),
            (10.0, 48, 0.015, 0.157_894_736_842_105_25),
            (-450.0, 64, -0.435, -6.477_941_176_470_588),
        ];
        for (dx, millis, expected_d, expected_v) in trace {
            s.apply_wheel_delta(
                dx,
                0.0,
                t0 + Duration::from_millis(millis),
                WheelPhase::Moved,
            );
            let diagnostics = s.wheel_diagnostics();
            assert_eq!(s.phase, Phase::WheelGesture);
            assert!((diagnostics.signed_displacement / page - expected_d).abs() < 1e-6);
            assert!(((s.position + page) / page - expected_d).abs() < 1e-6);
            assert!((s.estimate_wheel_velocity_f64() / f64::from(page) - expected_v).abs() < 1e-9);
            assert_eq!(diagnostics.target_decision_count, 0);
            assert_eq!(diagnostics.spring_generation_count, 0);
            assert_eq!(diagnostics.reanchor_count, 0);
        }

        let release_position = s.position;
        let tracking_velocity = s.wheel_diagnostics().filtered_velocity;
        s.apply_wheel_delta(0.0, 0.0, t0 + Duration::from_millis(80), WheelPhase::Ended);
        let diagnostics = s.wheel_diagnostics();
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.position, release_position);
        assert!((s.velocity - tracking_velocity * 0.8).abs() < 1e-3);
        assert_eq!(
            diagnostics.filtered_velocity.to_bits(),
            s.velocity.to_bits(),
            "release diagnostics must report the spring's captured initial velocity"
        );
        assert_eq!(s.settle_target, -2000.0);
        assert_eq!(diagnostics.target_decision_count, 1);
        assert_eq!(diagnostics.spring_generation_count, 1);
        assert_eq!(diagnostics.reanchor_count, 0);
        assert!(diagnostics.spring_id.is_some());
    }

    #[test]
    fn wheel_wls_keeps_16ms_samples_distinct_after_long_uptime() {
        fn run_after(offset: Duration) -> (usize, f64) {
            let mut s = wheel_scroller();
            s.position = -1000.0;
            let page = s.bounds.page_extent;
            let t0 = s.clock_origin + offset;
            s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);
            for (index, dx) in [20.0, -15.0, 10.0, -450.0].into_iter().enumerate() {
                s.apply_wheel_delta(
                    dx,
                    0.0,
                    t0 + Duration::from_millis(16 * (index as u64 + 1)),
                    WheelPhase::Moved,
                );
            }
            (
                s.wheel_sample_count,
                s.estimate_wheel_velocity_f64() / f64::from(page),
            )
        }

        let near_origin = run_after(Duration::from_secs(1));
        let after_seven_days = run_after(Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(near_origin.0, 5);
        assert_eq!(after_seven_days.0, 5);
        assert!((near_origin.1 - (-6.477_941_176_470_588)).abs() < 1e-9);
        assert!((after_seven_days.1 - near_origin.1).abs() < 1e-12);
    }

    #[test]
    fn wheel_deep_reverse_does_not_lock_left_or_right() {
        let mut s = wheel_scroller();
        s.position = -1000.0;
        let t0 = s.clock_origin + Duration::from_secs(1);
        s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);

        let trace = [(700.0, -300.0), (-850.0, -1150.0), (-300.0, -1450.0)];
        for (index, (dx, expected_position)) in trace.into_iter().enumerate() {
            s.apply_wheel_delta(
                dx,
                0.0,
                t0 + Duration::from_millis(16 * (index as u64 + 1)),
                WheelPhase::Moved,
            );
            assert!((s.position - expected_position).abs() < 1e-4);
            assert_eq!(s.phase, Phase::WheelGesture);
            assert_eq!(s.wheel_diagnostics().target_decision_count, 0);
        }
        assert!((s.wheel_accumulated + 450.0).abs() < 1e-4);
        s.apply_wheel_delta(0.0, 0.0, t0 + Duration::from_millis(64), WheelPhase::Ended);
        assert_eq!(s.settle_target, -2000.0);
        assert_eq!(s.wheel_diagnostics().spring_generation_count, 1);
    }

    #[test]
    fn wheel_rational_rubber_is_strictly_monotonic() {
        let mut s = wheel_scroller();
        let t0 = s.clock_origin + Duration::from_secs(1);
        s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);
        let mut previous = 0.0;
        for (index, dx) in [1.0, 4.0, 23.0, 112.0, 360.0].into_iter().enumerate() {
            s.apply_wheel_delta(
                dx,
                0.0,
                t0 + Duration::from_millis(16 * (index as u64 + 1)),
                WheelPhase::Moved,
            );
            assert!(
                s.position > previous,
                "non-zero edge input must keep moving: {} <= {previous}",
                s.position
            );
            assert!(s.position < 200.0);
            previous = s.position;
        }
        assert!((s.wheel_rubber(500.0) / 1000.0 - 0.125).abs() < 1e-6);
    }

    #[test]
    fn wheel_critical_spring_is_frame_rate_independent() {
        fn run(hz: usize) -> (f32, f32) {
            let mut s = wheel_scroller();
            s.position = -1435.0;
            s.velocity = -2200.0;
            s.settle_target = -2000.0;
            s.phase = Phase::Settling;
            s.settling_from_wheel = true;
            for _ in 0..(hz / 6) {
                s.step_once(1.0 / hz as f32);
            }
            (s.position, s.velocity)
        }
        let at_60 = run(60);
        for hz in [120, 144] {
            let current = run(hz);
            assert!((current.0 - at_60.0).abs() < 0.01, "{hz} Hz position");
            assert!((current.1 - at_60.1).abs() < 0.1, "{hz} Hz velocity");
        }
    }

    #[test]
    fn wheel_cancel_returns_to_saved_page_from_rest() {
        let mut s = wheel_scroller();
        s.position = -1000.0;
        let t0 = s.clock_origin + Duration::from_secs(1);
        s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);
        s.apply_wheel_delta(
            -650.0,
            0.0,
            t0 + Duration::from_millis(16),
            WheelPhase::Moved,
        );
        let live_position = s.position;
        s.apply_wheel_delta(
            0.0,
            0.0,
            t0 + Duration::from_millis(32),
            WheelPhase::Cancelled,
        );
        assert_eq!(s.phase, Phase::Settling);
        assert_eq!(s.position, live_position);
        assert_eq!(s.settle_target, -1000.0);
        assert_eq!(s.velocity, 0.0);
        assert_eq!(s.wheel_diagnostics().spring_generation_count, 1);
    }

    #[test]
    fn wheel_recontact_uses_live_spring_position_as_anchor() {
        let mut s = wheel_scroller();
        s.position = -420.0;
        s.velocity = -900.0;
        s.settle_target = -1000.0;
        s.phase = Phase::Settling;
        s.settling_from_wheel = true;
        let live_position = s.position;
        let live_velocity = s.velocity;
        let t0 = s.clock_origin + Duration::from_secs(1);
        s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);
        assert_eq!(s.phase, Phase::WheelGesture);
        assert_eq!(s.position, live_position);
        assert_eq!(s.velocity, live_velocity);
        assert_eq!(s.wheel_anchor_position, live_position);
        assert_eq!(s.wheel_from_snap, -1000.0);
        s.apply_wheel_delta(40.0, 0.0, t0 + Duration::from_millis(16), WheelPhase::Moved);
        assert!((s.position - (live_position + 40.0)).abs() < 1e-5);
        assert_eq!(s.wheel_diagnostics().reanchor_count, 0);
    }

    #[test]
    fn wheel_single_page_rubbers_both_directions() {
        for direction in [-1.0, 1.0] {
            let mut s = Scroller::new(ScrollBounds {
                page_extent: 1000.0,
                page_count: 1,
            });
            let t0 = s.clock_origin + Duration::from_secs(1);
            s.apply_wheel_delta(0.0, 0.0, t0, WheelPhase::Started);
            s.apply_wheel_delta(
                direction * 100.0,
                0.0,
                t0 + Duration::from_millis(16),
                WheelPhase::Moved,
            );
            assert_eq!(s.position.signum(), direction);
            assert!(s.position.abs() < 100.0);
            s.apply_wheel_delta(0.0, 0.0, t0 + Duration::from_millis(32), WheelPhase::Ended);
            assert_eq!(s.settle_target, 0.0);
        }
    }

    #[test]
    fn wheel_normalized_trace_is_page_extent_independent() {
        let mut outputs = Vec::new();
        for page in [500.0_f32, 1000.0, 2000.0] {
            let mut s = Scroller::new(ScrollBounds {
                page_extent: page,
                page_count: 3,
            });
            s.position = -page;
            let scale_factor = page / 1000.0;
            let t0 = s.clock_origin + Duration::from_secs(1);
            s.apply_wheel_delta_scaled(0.0, 0.0, scale_factor, t0, WheelPhase::Started);
            for (index, delta) in [0.020, -0.015, 0.010, -0.450].into_iter().enumerate() {
                s.apply_wheel_delta_scaled(
                    delta * page,
                    0.0,
                    scale_factor,
                    t0 + Duration::from_millis(16 * (index as u64 + 1)),
                    WheelPhase::Moved,
                );
            }
            outputs.push((
                s.position / page,
                s.wheel_diagnostics().filtered_velocity / page,
            ));
        }
        for output in &outputs[1..] {
            assert!((output.0 - outputs[0].0).abs() < 1e-6);
            assert!((output.1 - outputs[0].1).abs() < 1e-5);
        }
    }
}
