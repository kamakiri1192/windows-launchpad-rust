//! Per-widget interaction phase and transient visual state.

/// Interaction phase for a widget, following a simple state machine:
///
/// ```text
/// Idle -> Hovered -> Pressed -> Dragging
///                ↓          ↓
///            Settling   Settling
///                          ↓
///                        Idle
/// ```
///
/// `Disabled` is orthogonal — widgets in this phase do not process input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InteractionPhase {
    #[default]
    Idle,
    Hovered,
    Pressed,
    Dragging,
    Settling,
    Disabled,
}

/// Transient visual and input state keyed by a stable [`UiId`].
///
/// `hover_amount` and `press_amount` are stored as 0.0 / 1.0 for now.
/// Continuous animation values (springs, etc.) will be added in Phase 5.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ElementState {
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    /// 0.0 = not hovered, 1.0 = fully hovered. Kept as a float so later
    /// animation phases can lerp smoothly.
    pub hover_amount: f32,
    /// 0.0 = not pressed, 1.0 = fully pressed.
    pub press_amount: f32,
    pub phase: InteractionPhase,
}

#[cfg(test)]
mod tests {
    use super::{ElementState, InteractionPhase};

    #[test]
    fn default_interaction_phase_is_idle() {
        assert_eq!(InteractionPhase::default(), InteractionPhase::Idle);
    }

    #[test]
    fn default_element_state_is_idle_with_zero_amounts() {
        let s = ElementState::default();
        assert_eq!(s.phase, InteractionPhase::Idle);
        assert!(!s.hovered);
        assert!(!s.pressed);
        assert!(!s.focused);
        assert_eq!(s.hover_amount, 0.0);
        assert_eq!(s.press_amount, 0.0);
    }

    #[test]
    fn interaction_phase_all_variants_exist() {
        // Compile-time check that all expected variants are constructible.
        let phases = [
            InteractionPhase::Idle,
            InteractionPhase::Hovered,
            InteractionPhase::Pressed,
            InteractionPhase::Dragging,
            InteractionPhase::Settling,
            InteractionPhase::Disabled,
        ];
        assert_eq!(phases.len(), 6);
    }
}
