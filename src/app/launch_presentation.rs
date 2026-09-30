//! Platform-independent presentation timing for the resident launcher's transitions.
//!
//! The values intentionally describe a short, interruptible transition rather
//! than a long spring simulation. UIKit exposes spring timing parameters, but
//! not a single public SpringBoard constant; the perceptual target here is the
//! same one Apple recommends for launch motion: brief, purposeful, and settled
//! before the user can meaningfully interact with the grid.

/// Total open-transition duration. This is long enough to read at 60 Hz while
/// staying below the usual "UI feels delayed" threshold.
pub const OPEN_DURATION_SECONDS: f32 = 0.34;
/// Dismissal retraces the reveal motion at a slightly faster pace.
pub const CLOSE_DURATION_SECONDS: f32 = 0.22;
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
    phase: PresentationPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresentationPhase {
    Opening,
    Visible,
    Closing,
    Hidden,
}

impl Default for LaunchPresentation {
    fn default() -> Self {
        Self {
            elapsed: OPEN_DURATION_SECONDS,
            phase: PresentationPhase::Visible,
        }
    }
}

impl LaunchPresentation {
    pub fn begin(&mut self) {
        // A summon during dismissal reverses from the currently drawn frame.
        if !self.is_closing() {
            self.elapsed = 0.0;
        }
        self.phase = PresentationPhase::Opening;
    }

    pub fn begin_close(&mut self) {
        if matches!(
            self.phase,
            PresentationPhase::Opening | PresentationPhase::Visible
        ) {
            self.phase = PresentationPhase::Closing;
        }
    }

    /// Stop motion at the hidden endpoint after an immediate native hide.
    pub fn cancel(&mut self) {
        self.elapsed = 0.0;
        self.phase = PresentationPhase::Hidden;
    }

    /// Advance the transition. Returns true while a redraw is still needed.
    pub fn tick(&mut self, dt: f32) -> bool {
        let dt = dt.clamp(0.0, 0.1);
        match self.phase {
            PresentationPhase::Opening => {
                self.elapsed = (self.elapsed + dt).min(OPEN_DURATION_SECONDS);
                if self.elapsed >= OPEN_DURATION_SECONDS {
                    self.phase = PresentationPhase::Visible;
                }
            }
            PresentationPhase::Closing => {
                self.elapsed =
                    (self.elapsed - dt * OPEN_DURATION_SECONDS / CLOSE_DURATION_SECONDS).max(0.0);
                if self.elapsed <= 0.0 {
                    self.phase = PresentationPhase::Hidden;
                }
            }
            PresentationPhase::Visible | PresentationPhase::Hidden => return false,
        }
        true
    }

    pub fn is_active(self) -> bool {
        matches!(
            self.phase,
            PresentationPhase::Opening | PresentationPhase::Closing
        )
    }

    pub fn is_closing(self) -> bool {
        self.phase == PresentationPhase::Closing
    }

    pub fn is_hidden(self) -> bool {
        self.phase == PresentationPhase::Hidden
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

    #[test]
    fn dismissal_reaches_transparent_endpoint_before_finishing() {
        let mut presentation = LaunchPresentation::default();
        presentation.begin_close();
        let mut previous = presentation.shader_values();
        assert_eq!(&previous[..2], &[1.0, 1.0]);

        for _ in 0..14 {
            presentation.tick(1.0 / 60.0);
            let current = presentation.shader_values();
            assert!(current[0] <= previous[0]);
            assert!(current[1] <= previous[1]);
            if current[1] > 0.0 {
                assert!(presentation.is_closing());
                assert!(!presentation.is_hidden());
            }
            previous = current;
        }

        assert!(presentation.is_hidden());
        assert!(!presentation.is_active());
        assert_eq!(&presentation.shader_values()[..2], &[0.0, 0.0]);
        assert!(!presentation.tick(1.0 / 60.0));
    }

    #[test]
    fn repeated_dismissals_do_not_restart_motion() {
        let mut presentation = LaunchPresentation::default();
        presentation.begin_close();
        for _ in 0..14 {
            let before = presentation.shader_values();
            presentation.begin_close();
            assert_eq!(presentation.shader_values(), before);
            presentation.tick(1.0 / 60.0);
        }
        assert!(presentation.is_hidden());
    }

    #[test]
    fn closing_during_reveal_and_summoning_during_close_preserve_the_frame() {
        let mut presentation = LaunchPresentation::default();
        presentation.begin();
        presentation.tick(0.1);
        let opening_frame = presentation.shader_values();
        presentation.begin_close();
        assert_eq!(presentation.shader_values(), opening_frame);
        presentation.tick(1.0 / 60.0);
        let closing_frame = presentation.shader_values();
        assert!(closing_frame[0] < opening_frame[0]);
        presentation.begin();
        assert_eq!(presentation.shader_values(), closing_frame);
        assert!(!presentation.is_closing());
        for _ in 0..4 {
            presentation.tick(0.1);
        }
        assert!(!presentation.is_hidden());
        assert!(!presentation.is_active());
        assert_eq!(&presentation.shader_values()[..2], &[1.0, 1.0]);
    }

    #[test]
    fn immediate_hide_cancels_motion_and_next_summon_restarts_reveal() {
        let mut presentation = LaunchPresentation::default();
        presentation.begin_close();
        presentation.tick(0.1);
        presentation.cancel();
        assert!(presentation.is_hidden());
        assert!(!presentation.is_active());
        assert_eq!(&presentation.shader_values()[..2], &[0.0, 0.0]);
        presentation.begin();
        assert!(presentation.is_active());
        assert_eq!(&presentation.shader_values()[..2], &[0.0, 0.0]);
    }
}
