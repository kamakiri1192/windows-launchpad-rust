//! Presentation timing for the resident launcher's open transition.
//!
//! The values intentionally describe a short, interruptible transition rather
//! than a long spring simulation. UIKit exposes spring timing parameters, but
//! not a single public SpringBoard constant; the perceptual target here is the
//! same one Apple recommends for launch motion: brief, purposeful, and settled
//! before the user can meaningfully interact with the grid.

/// Total open-transition duration. This is long enough to read at 60 Hz while
/// staying below the usual "UI feels delayed" threshold.
pub const OPEN_DURATION_SECONDS: f32 = 0.34;
/// The backdrop and control fade reaches full opacity slightly earlier than
/// the app tiles finish gathering.
pub const FADE_DURATION_SECONDS: f32 = 0.18;
/// Initial scale of each app tile at the outside edge of its gather path.
pub const START_SCALE: f32 = 0.84;
/// Maximum radial stagger as a fraction of the open transition. Center items
/// start first; items near the page-frame edge start this much later.
pub const MAX_STAGGER: f32 = 0.14;
/// Initial radial distance as a fraction of the smaller page-frame half-size.
/// This is the actual travel distance for the outside-in gather, not merely a
/// timing delay. At a typical 1280×800 viewport it is roughly 30–50 px.
pub const START_OFFSET_FRACTION: f32 = 0.14;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaunchPresentation {
    elapsed: f32,
    active: bool,
}

impl Default for LaunchPresentation {
    fn default() -> Self {
        Self {
            elapsed: OPEN_DURATION_SECONDS,
            active: false,
        }
    }
}

impl LaunchPresentation {
    pub fn begin(&mut self) {
        self.elapsed = 0.0;
        self.active = true;
    }

    pub fn cancel(&mut self) {
        self.elapsed = OPEN_DURATION_SECONDS;
        self.active = false;
    }

    /// Advance the transition. Returns true while a redraw is still needed.
    pub fn tick(&mut self, dt: f32) -> bool {
        if !self.active {
            return false;
        }
        self.elapsed += dt.clamp(0.0, 0.1);
        if self.elapsed >= OPEN_DURATION_SECONDS {
            self.elapsed = OPEN_DURATION_SECONDS;
            self.active = false;
        }
        true
    }

    pub fn is_active(self) -> bool {
        self.active
    }

    /// Values consumed by the tile, icon, text, control, and glass shaders:
    /// `(gather_progress, opacity, start_scale, max_stagger)`.
    pub fn shader_values(self) -> [f32; 4] {
        let gather = smoothstep(self.elapsed / OPEN_DURATION_SECONDS);
        let opacity = smoothstep(self.elapsed / FADE_DURATION_SECONDS);
        [gather, opacity, START_SCALE, MAX_STAGGER]
    }

    /// Motion values consumed by the tile, icon, and text vertex shaders:
    /// `(start_offset_fraction, _, _, _)`. The extra slots keep the uniform
    /// block extensible without changing the existing timing values.
    pub fn motion_values(self) -> [f32; 4] {
        [START_OFFSET_FRACTION, 0.0, 0.0, 0.0]
    }
}

fn smoothstep(value: f32) -> f32 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_hidden_and_reaches_a_settled_state() {
        let mut presentation = LaunchPresentation::default();
        presentation.begin();

        assert!(presentation.is_active());
        assert_eq!(presentation.shader_values()[0], 0.0);
        assert_eq!(presentation.shader_values()[1], 0.0);

        for _ in 0..4 {
            presentation.tick(OPEN_DURATION_SECONDS / 4.0);
        }

        assert!(!presentation.is_active());
        assert_eq!(
            presentation.shader_values(),
            [1.0, 1.0, START_SCALE, MAX_STAGGER]
        );
        assert_eq!(
            presentation.motion_values(),
            [START_OFFSET_FRACTION, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn progress_is_monotonic_and_dt_is_bounded() {
        let mut presentation = LaunchPresentation::default();
        presentation.begin();
        let mut previous = presentation.shader_values();

        for _ in 0..10 {
            presentation.tick(1.0);
            let current = presentation.shader_values();
            assert!(current[0] >= previous[0]);
            assert!(current[1] >= previous[1]);
            previous = current;
        }
        assert!(!presentation.is_active());
    }
}
