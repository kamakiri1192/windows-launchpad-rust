//! Per-property spring/easing animation channels.
//!
//! This is a minimal Rust port of the property animation model used by the
//! `liquid-dom` reference (see `packages/layout/src/animation.ts`). Unlike the
//! omega/zeta springs in [`crate::scroll`] and [`crate::features::folders`],
//! this module uses the explicit *stiffness / damping / mass* formulation so
//! that the exact feel of the reference menu demo can be reproduced.
//!
//! Each animated value owns an independent [`Channel`] with its own
//! [`Transition`]. A menu opening can drive position with a fast spring, size
//! with a cubic-bezier easing, and corner radius with a slow ease-out — all in
//! parallel and each settling on its own timescale.
//!
//! Integration is semi-implicit Euler (matching the reference `stepChannel`),
//! which is stable and frame-rate independent for the parameter ranges used
//! here.

/// Cubic easing curve. `Bezier` stores the two non-endpoint control points
/// `(x1, y1, x2, y2)` of a unit cubic Bézier, identical to CSS
/// `cubic-bezier(x1, y1, x2, y2)`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::enum_variant_names)] // Ease{Out,In} mirror CSS easing names
pub enum Ease {
    /// `1 - (1 - t)^2`.
    EaseOut,
    /// `t * t`.
    EaseIn,
    /// `cubic-bezier(x1, y1, x2, y2)`. `x1`/`x2` are clamped to `[0, 1]`.
    Bezier { x1: f32, y1: f32, x2: f32, y2: f32 },
}

impl Ease {
    /// The reference menu's size transition: `bezier(0.8, 0.3, 0.5, 0.8)`.
    pub const MENU_SIZE: Self = Self::Bezier {
        x1: 0.8,
        y1: 0.3,
        x2: 0.5,
        y2: 0.8,
    };

    /// Evaluate the easing function for progress `t` in `[0, 1]`.
    pub fn at(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Ease::EaseIn => t * t,
            Ease::Bezier { x1, y1, x2, y2 } => bezier_solve_y_for_x(t, x1, y1, x2, y2),
        }
    }
}

/// Solve a cubic-bézier easing curve for the output `y` at a given input `x`.
///
/// `x` is the *horizontal* coordinate (time). The curve is parameterized by a
/// parameter `t in [0, 1]`, but `x(t)` is generally not equal to `t`, so we
/// must invert `x(t) -> t` before computing `y(t)`. This mirrors the reference
/// implementation (Newton–Raphson with a bisection fallback).
fn bezier_solve_y_for_x(x: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    if x <= 0.0 || x >= 1.0 {
        return x;
    }
    let cx1 = x1.clamp(0.0, 1.0);
    let cx2 = x2.clamp(0.0, 1.0);

    // Newton–Raphson on f(t) = bezier_x(t) - x.
    let mut t = x;
    for _ in 0..8 {
        let current_x = bezier_coord(t, cx1, cx2) - x;
        if current_x.abs() < 1e-6 {
            return bezier_coord(t, y1, y2);
        }
        let derivative = bezier_derivative(t, cx1, cx2);
        if derivative.abs() < 1e-6 {
            break;
        }
        let next_t = t - current_x / derivative;
        if !(0.0..=1.0).contains(&next_t) {
            break;
        }
        t = next_t;
    }

    // Bisection fallback.
    let mut lower = 0.0_f32;
    let mut upper = 1.0_f32;
    t = x;
    for _ in 0..16 {
        let current_x = bezier_coord(t, cx1, cx2);
        if (current_x - x).abs() < 1e-6 {
            break;
        }
        if current_x < x {
            lower = t;
        } else {
            upper = t;
        }
        t = (lower + upper) * 0.5;
    }
    bezier_coord(t, y1, y2)
}

/// `B(t) = 3(1-t)^2 t p1 + 3(1-t) t^2 p2 + t^3` for one coordinate.
fn bezier_coord(t: f32, p1: f32, p2: f32) -> f32 {
    let inv_t = 1.0 - t;
    3.0 * inv_t * inv_t * t * p1 + 3.0 * inv_t * t * t * p2 + t * t * t
}

/// `B'(t)` for one coordinate, used by Newton–Raphson.
fn bezier_derivative(t: f32, p1: f32, p2: f32) -> f32 {
    let inv_t = 1.0 - t;
    3.0 * inv_t * inv_t * p1 + 6.0 * inv_t * t * (p2 - p1) + 3.0 * t * t * (1.0 - p2)
}

/// How a single value moves toward its target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transition {
    /// Critically/under-damped spring in the stiffness/damping/mass form.
    Spring {
        stiffness: f32,
        damping: f32,
        mass: f32,
        /// Initial velocity injected at retarget time, with the sign of
        /// `(target - current)`. Zero keeps the current velocity.
        velocity: f32,
    },
    /// Time-based easing over `duration` seconds.
    Easing { duration: f32, ease: Ease },
    /// Snap to the target instantly (used for momentary bumps).
    Snap,
}

/// A single animated scalar value and its integration state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Channel {
    /// Current displayed value.
    pub current: f32,
    /// Value at the start of an easing transition; ignored for springs.
    pub origin: f32,
    /// Value the channel is moving toward.
    pub target: f32,
    pub velocity: f32,
}

impl Channel {
    pub const fn rest(target: f32) -> Self {
        Self {
            current: target,
            origin: target,
            target,
            velocity: 0.0,
        }
    }
}

/// Rest thresholds below which a spring is considered settled, matching the
/// reference defaults (`restSpeed` / `restDelta`).
const REST_SPEED: f32 = 0.01;
const REST_DELTA: f32 = 0.01;

/// Advance `channel` one step under `config`. Returns `true` if the channel is
/// still animating (i.e. the caller should keep ticking).
pub fn step(channel: &mut Channel, config: Transition, elapsed: &mut f32, dt: f32) -> bool {
    match config {
        Transition::Snap => {
            channel.current = channel.target;
            channel.origin = channel.target;
            channel.velocity = 0.0;
            *elapsed = 0.0;
            false
        }
        Transition::Spring {
            stiffness,
            damping,
            mass,
            velocity: configured_velocity,
        } => {
            // On the first step after a retarget, seed the configured initial
            // velocity (signed toward the target). `elapsed` is reset to 0 on
            // retarget, so we treat elapsed == 0 as the seeding boundary.
            if *elapsed == 0.0 && configured_velocity != 0.0 && channel.velocity == 0.0 {
                channel.velocity =
                    configured_velocity.abs() * (channel.target - channel.current).signum();
            }
            *elapsed += dt;

            // Sub-step for stability with stiff springs, mirroring the
            // reference clamp of `min(0.064, dt)` split into 1/60 slices.
            let spring_dt = dt.min(0.064);
            let step_count = (spring_dt / (1.0 / 60.0)).ceil().max(1.0) as usize;
            let step_seconds = spring_dt / step_count as f32;
            for _ in 0..step_count {
                let displacement = channel.current - channel.target;
                let spring_force = -stiffness * displacement;
                let damping_force = -damping * channel.velocity;
                let acceleration = (spring_force + damping_force) / mass;
                channel.velocity += acceleration * step_seconds;
                channel.current += channel.velocity * step_seconds;
            }

            let settled = channel.velocity.abs() <= REST_SPEED
                && (channel.target - channel.current).abs() <= REST_DELTA;
            if settled {
                channel.current = channel.target;
                channel.velocity = 0.0;
                false
            } else {
                true
            }
        }
        Transition::Easing { duration, ease } => {
            if duration <= 0.0 {
                channel.current = channel.target;
                *elapsed = 0.0;
                return false;
            }
            *elapsed += dt;
            let progress = (*elapsed / duration).clamp(0.0, 1.0);
            let eased = ease.at(progress);
            channel.current = channel.origin + (channel.target - channel.origin) * eased;
            if progress >= 1.0 {
                channel.current = channel.target;
                false
            } else {
                true
            }
        }
    }
}

/// Retarget a channel to a new `target`, switching to `config`.
///
/// For easing transitions the origin resets to the current value (so the
/// easing runs from here to the new target). For springs the origin is unused;
/// the configured `velocity` seeds the initial impulse on the next step.
pub fn retarget(channel: &mut Channel, target: f32, config: Transition, elapsed: &mut f32) {
    channel.target = target;
    match config {
        Transition::Snap => {}
        Transition::Easing { .. } => {
            channel.origin = channel.current;
            channel.velocity = 0.0;
            *elapsed = 0.0;
        }
        Transition::Spring { .. } => {
            *elapsed = 0.0;
            // Keep the live velocity for momentum carry-over; the seeding guard
            // in `step` only injects configured velocity when current velocity
            // is zero.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_endpoints() {
        assert_eq!(Ease::EaseOut.at(0.0), 0.0);
        assert!((Ease::EaseOut.at(1.0) - 1.0).abs() < 1e-6);
        assert!((Ease::EaseOut.at(0.5) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn bezier_identity_is_linear() {
        // cubic-bezier(0, 0, 1, 1) is the identity line y = x.
        for probe in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let v = Ease::Bezier {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 1.0,
            }
            .at(probe);
            assert!((v - probe).abs() < 1e-4, "expected ~{probe}, got {v}");
        }
    }

    #[test]
    fn bezier_menu_size_curve_is_monotone_and_bounded() {
        let curve = Ease::MENU_SIZE;
        let mut prev = 0.0;
        for i in 1..=10 {
            let t = i as f32 / 10.0;
            let v = curve.at(t);
            assert!(
                (0.0..=1.0).contains(&v),
                "curve value {v} out of [0,1] at t={t}"
            );
            assert!(
                v >= prev - 1e-4,
                "curve not monotone at t={t}: {v} < {prev}"
            );
            prev = v;
        }
    }

    #[test]
    fn spring_settles_to_target() {
        let mut channel = Channel::rest(0.0);
        let config = Transition::Spring {
            stiffness: 144.0,
            damping: 14.0,
            mass: 1.0,
            velocity: 2400.0,
        };
        let mut elapsed = 0.0;
        retarget(&mut channel, 100.0, config, &mut elapsed);
        let mut animating = true;
        let mut safety = 0;
        while animating && safety < 10_000 {
            animating = step(&mut channel, config, &mut elapsed, 1.0 / 60.0);
            safety += 1;
        }
        assert!(
            (channel.current - 100.0).abs() < 0.5,
            "settled at {}",
            channel.current
        );
        assert!(!animating);
    }

    #[test]
    fn easing_reaches_target_within_duration() {
        let mut channel = Channel::rest(0.0);
        let config = Transition::Easing {
            duration: 0.3,
            ease: Ease::MENU_SIZE,
        };
        let mut elapsed = 0.0;
        retarget(&mut channel, 320.0, config, &mut elapsed);
        let mut animating = true;
        // duration / dt = 0.3 / (1/60) = 18 frames; allow a little slack.
        for _ in 0..25 {
            if !animating {
                break;
            }
            animating = step(&mut channel, config, &mut elapsed, 1.0 / 60.0);
        }
        assert!(!animating, "still animating after duration");
        assert!((channel.current - 320.0).abs() < 1e-3);
    }

    #[test]
    fn snap_jumps_immediately() {
        let mut channel = Channel::rest(0.0);
        let mut elapsed = 0.0;
        retarget(&mut channel, 1.0, Transition::Snap, &mut elapsed);
        let animating = step(&mut channel, Transition::Snap, &mut elapsed, 1.0 / 60.0);
        assert!(!animating);
        assert_eq!(channel.current, 1.0);
    }
}
