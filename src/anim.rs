//! Entrance (appear) animation for the launcher window.
//!
//! When the window is first shown or re-summoned from a shortcut, the whole UI
//! scales up slightly (0.92 -> 1.0) about the page-frame center while fading in
//! alpha (0 -> 1), giving an iOS / Spotlight-style "liquid glass" reveal.
//!
//! The architecture mirrors the established codebase pattern (see
//! `bottom_control.rs`): a normalized progress value is advanced **linearly**
//! per frame so the animation completes in a fixed wall-clock duration regardless
//! of frame rate, and the easing curve is applied only when the value is
//! *consumed* (`alpha()` / `scale()`). A `tick()` returns whether the animation
//! is still running so the caller can keep the redraw loop alive.

/// Wall-clock duration of the reveal, in seconds. Tuned to feel snappy but
/// present — comparable to iOS Spotlight / Launchpad.
const DURATION: f32 = 0.32;

/// Scale the UI starts at before easing to 1.0 about the frame center. A touch
/// under 1.0 reads as "settling into place" without an obvious zoom.
const START_SCALE: f32 = 0.92;

/// Drives the entrance reveal. Owns a normalized progress in `0.0..=1.0`.
#[derive(Debug, Clone, Copy)]
pub struct EntranceAnimation {
    progress: f32,
    active: bool,
    /// True until the first `tick()` after a `start()`. The first post-summon
    /// frame can carry a huge `dt` (the loop was idle while hidden, so
    /// `last_redraw` is stale), which would jump `progress` partway in and make
    /// the UI "flash" at a partial-alpha/scale state. Dropping that first dt
    /// guarantees the reveal always begins from alpha 0 / start scale.
    primed: bool,
}

impl Default for EntranceAnimation {
    fn default() -> Self {
        Self {
            progress: 0.0,
            active: false,
            primed: false,
        }
    }
}

impl EntranceAnimation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm the reveal (progress back to 0, marked active). Called on first show
    /// and on every subsequent summon. No-op when `LAUNCHPAD_NO_ENTRANCE` is set
    /// (diagnostic switch to compare with/without the animation).
    pub fn start(&mut self) {
        if std::env::var_os("LAUNCHPAD_NO_ENTRANCE").is_some() {
            // Skip the reveal entirely: jump straight to fully shown.
            self.progress = 1.0;
            self.active = false;
            self.primed = false;
            return;
        }
        self.progress = 0.0;
        self.active = true;
        self.primed = true;
    }

    /// Advance the linear progress by `dt` seconds. Returns `true` while the
    /// animation is still running (the caller keeps requesting redraws), or
    /// `false` once it has settled at 1.0.
    pub fn tick(&mut self, dt: f32) -> bool {
        if !self.active {
            return false;
        }
        // Discard the stale first-frame dt so the reveal starts from rest.
        if self.primed {
            self.primed = false;
            return self.active;
        }
        self.progress = advance_linear(self.progress, 1.0, dt, DURATION);
        if self.progress >= 1.0 {
            self.progress = 1.0;
            self.active = false;
        }
        self.active
    }

    /// Whether the reveal is still animating this frame.
    pub fn is_animating(&self) -> bool {
        self.active
    }

    /// Composited opacity for the reveal (`ease_in_out` of progress): a gentle
    /// symmetric ramp so the content fades in smoothly rather than cutting.
    pub fn alpha(&self) -> f32 {
        ease_in_out(self.progress)
    }

    /// Uniform scale factor about the page-frame center (`ease_ios_out` of
    /// progress): starts fast and decelerates into rest, the "deliberate but
    /// lively" iOS feel.
    pub fn scale(&self) -> f32 {
        lerp(START_SCALE, 1.0, ease_ios_out(self.progress))
    }
}

/// Linear advance of `v` toward `target` so it completes in exactly `duration`
/// seconds (frame-rate independent). The easing curve is applied by the
/// consumer. Mirrors `bottom_control::advance_linear`.
fn advance_linear(v: f32, target: f32, dt: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return target;
    }
    let dir = if target >= v { 1.0 } else { -1.0 };
    let step = dt / duration;
    let next = v + dir * step;
    if dir > 0.0 {
        next.min(target)
    } else {
        next.max(target)
    }
}

/// Cubic ease-in-out, symmetric S-curve. Mirrors `bottom_control::ease_in_out`.
fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) * 0.5
    }
}

/// iOS-style ease-out: approximates `cubic-bezier(0.32, 0.72, 0, 1)` used by
/// UIKit for spring-free controls. Starts fast and decelerates into rest.
/// Mirrors `bottom_control::ease_ios_out`.
fn ease_ios_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    cubic_bezier_y(0.32, 0.72, 0.0, 1.0, t)
}

/// Evaluate y(x) of a CSS cubic-bezier easing curve. Mirrors
/// `bottom_control::cubic_bezier_y`.
fn cubic_bezier_y(p1x: f32, p1y: f32, p2x: f32, p2y: f32, x: f32) -> f32 {
    let bezier = |s: f32| -> f32 {
        let one_minus = 1.0 - s;
        3.0 * one_minus * one_minus * s * p1x + 3.0 * one_minus * s * s * p2x + s * s * s
    };
    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    let mut s = x;
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
    let one_minus = 1.0 - s;
    3.0 * one_minus * one_minus * s * p1y + 3.0 * one_minus * s * s * p2y + s * s * s
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_arms_and_resets() {
        let mut a = EntranceAnimation::new();
        assert!(!a.is_animating());
        a.start();
        assert!(a.is_animating());
        assert_eq!(a.progress, 0.0);
    }

    #[test]
    fn tick_advances_and_completes() {
        let mut a = EntranceAnimation::new();
        a.start();
        // The first tick after start() is discarded (stale dt guard), so the
        // reveal needs DURATION of *real* ticks on top of that.
        a.tick(0.0); // primed frame dropped
        let still = a.tick(DURATION);
        assert!(!still);
        assert!(!a.is_animating());
        assert_eq!(a.progress, 1.0);
    }

    #[test]
    fn first_tick_starts_from_rest() {
        // A huge first-frame dt (e.g. the loop was idle while hidden) must NOT
        // jump progress — the reveal always begins from alpha 0 / start scale.
        let mut a = EntranceAnimation::new();
        a.start();
        let still = a.tick(1.0); // absurd dt on the primed frame
        assert!(still);
        assert_eq!(a.progress, 0.0);
        assert!((a.alpha() - 0.0).abs() < 1e-5);
        assert!((a.scale() - START_SCALE).abs() < 1e-5);
    }

    #[test]
    fn tick_is_frame_rate_independent() {
        // Two dt slices summing to DURATION must reach the same place as one
        // big slice, regardless of how the time is split (after the primed
        // frame is dropped).
        let mut big = EntranceAnimation::new();
        big.start();
        big.tick(0.0);
        big.tick(DURATION);

        let mut small = EntranceAnimation::new();
        small.start();
        small.tick(0.0); // primed frame dropped
        let n = 64;
        let mut still = false;
        for _ in 0..n {
            still = small.tick(DURATION / n as f32);
        }
        assert!(!still);
        assert!((small.progress - big.progress).abs() < 1e-4);
    }

    #[test]
    fn alpha_and_scale_bounds() {
        let mut a = EntranceAnimation::new();
        a.start();
        // At progress 0, alpha is 0 and scale is the start value.
        assert!((a.alpha() - 0.0).abs() < 1e-5);
        assert!((a.scale() - START_SCALE).abs() < 1e-5);
        // At progress 1, alpha is 1 and scale is 1 (drop the primed frame first).
        a.tick(0.0);
        a.tick(DURATION);
        assert!((a.alpha() - 1.0).abs() < 1e-3);
        assert!((a.scale() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn alpha_monotonic_and_eased() {
        // alpha should be monotonic in progress and within [0,1].
        let mut prev = -1.0;
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            let a = ease_in_out(t);
            assert!(a >= prev - 1e-6);
            assert!((0.0..=1.0).contains(&a));
            prev = a;
        }
        // At t=0.5 the cubic ease-in-out is exactly 0.5.
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn ease_ios_out_endpoints() {
        assert!((ease_ios_out(0.0) - 0.0).abs() < 1e-3);
        assert!((ease_ios_out(1.0) - 1.0).abs() < 1e-3);
        // Ease-out: at the midpoint it should already be past 0.5.
        assert!(ease_ios_out(0.5) > 0.5);
    }
}
