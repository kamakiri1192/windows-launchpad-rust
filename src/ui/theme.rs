/// Theme values for Liquid Glass UI components.
///
/// Colours use `[f32; 4]` (RGBA 0..1) matching the existing palette used by
/// `settings_panel.rs` (INK / MUTED / DIM / ACCENT / GREEN).

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControlSize {
    Mini,
    Small,
    Regular,
}

impl Default for ControlSize {
    fn default() -> Self {
        Self::Regular
    }
}

/// Adjustable tunables for the Liquid Glass toggle (see
/// `docs/Liquid_Glass_Toggle.md`). These are starting-point values; they are
/// *not* published Apple constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToggleMotionStyle {
    pub press_response_ms: f32,
    pub release_glass_fade_ms: f32,
    pub thumb_spring_omega: f32,
    pub thumb_spring_zeta: f32,
    pub press_scale: f32,
    pub hover_scale: f32,
    pub max_directional_stretch: f32,
    pub max_settle_overshoot: f32,
    pub drag_threshold: f32,
}

impl Default for ToggleMotionStyle {
    fn default() -> Self {
        Self {
            press_response_ms: 70.0,
            release_glass_fade_ms: 220.0,
            thumb_spring_omega: 24.0,
            thumb_spring_zeta: 0.82,
            press_scale: 1.04,
            hover_scale: 1.01,
            max_directional_stretch: 0.06,
            max_settle_overshoot: 0.04,
            drag_threshold: 3.0,
        }
    }
}

/// macOS / iOS style scrollbar (see `docs/Liquid_Glass_ui_lib.md`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarStyle {
    pub idle_width: f32,
    pub active_width: f32,
    pub minimum_thumb_length: f32,
    pub hold_duration_ms: f32,
    pub fade_duration_ms: f32,
    pub inset: f32,
}

impl Default for ScrollbarStyle {
    fn default() -> Self {
        Self {
            idle_width: 6.0,
            active_width: 11.0,
            minimum_thumb_length: 24.0,
            hold_duration_ms: 600.0,
            fade_duration_ms: 220.0,
            inset: 3.0,
        }
    }
}

/// Complete theme definition for the Liquid Glass UI.
///
/// Colours are `[f32; 4]` (RGBA in 0..1) and mirror the palette already used
/// in `settings_panel.rs` (INK, MUTED, DIM, ACCENT, GREEN).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub control_size: ControlSize,
    pub toggle_motion: ToggleMotionStyle,
    pub scrollbar: ScrollbarStyle,
    pub ink: [f32; 4],
    pub muted: [f32; 4],
    pub dim: [f32; 4],
    pub accent: [f32; 4],
    pub green: [f32; 4],
    pub scale_factor: f32,
    pub reduce_motion: bool,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            control_size: ControlSize::default(),
            toggle_motion: ToggleMotionStyle::default(),
            scrollbar: ScrollbarStyle::default(),
            ink: [0.95, 0.96, 0.98, 1.0],
            muted: [0.70, 0.72, 0.76, 1.0],
            dim: [0.50, 0.52, 0.56, 1.0],
            accent: [0.36, 0.62, 0.95, 1.0],
            green: [0.28, 0.82, 0.48, 1.0],
            scale_factor: 1.0,
            reduce_motion: false,
            reduce_transparency: false,
            increase_contrast: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ScrollbarStyle, Theme, ToggleMotionStyle};

    #[test]
    fn toggle_motion_defaults_match_doc_initial_values() {
        let s = ToggleMotionStyle::default();
        assert_eq!(s.press_response_ms, 70.0);
        assert_eq!(s.release_glass_fade_ms, 220.0);
        assert_eq!(s.thumb_spring_omega, 24.0);
        assert_eq!(s.thumb_spring_zeta, 0.82);
        assert_eq!(s.press_scale, 1.04);
        assert_eq!(s.hover_scale, 1.01);
        assert_eq!(s.max_directional_stretch, 0.06);
        assert_eq!(s.max_settle_overshoot, 0.04);
        assert_eq!(s.drag_threshold, 3.0);
    }

    #[test]
    fn scrollbar_defaults_match_doc_initial_values() {
        let s = ScrollbarStyle::default();
        assert_eq!(s.idle_width, 6.0);
        assert_eq!(s.active_width, 11.0);
        assert_eq!(s.minimum_thumb_length, 24.0);
        assert_eq!(s.hold_duration_ms, 600.0);
        assert_eq!(s.fade_duration_ms, 220.0);
        assert_eq!(s.inset, 3.0);
    }

    #[test]
    fn theme_palette_matches_expected_values() {
        let t = Theme::default();
        assert_eq!(t.ink, [0.95, 0.96, 0.98, 1.0]);
        assert_eq!(t.muted, [0.70, 0.72, 0.76, 1.0]);
        assert_eq!(t.dim, [0.50, 0.52, 0.56, 1.0]);
        assert_eq!(t.accent, [0.36, 0.62, 0.95, 1.0]);
        assert_eq!(t.green, [0.28, 0.82, 0.48, 1.0]);
        assert_eq!(t.scale_factor, 1.0);
        assert!(!t.reduce_motion);
        assert!(!t.reduce_transparency);
        assert!(!t.increase_contrast);
    }
}
