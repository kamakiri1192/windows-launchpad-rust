#[derive(Debug, Clone, Copy)]
pub struct LiquidGlassParams {
    pub enabled: bool,
    pub auto_darkness_enabled: bool,
    /// Maximum black tint contributed by backdrop-aware darkness.
    pub auto_darkness_max: f32,
    pub thickness: f32,
    pub refractive_index: f32,
    pub chromatic_aberration: f32,
    pub blur_radius: f32,
    pub saturation: f32,
    /// Uniform black tint applied to the whole glass surface. This does not
    /// sample the backdrop and does not change alpha.
    pub glass_darkness: f32,
    pub glass_color: [f32; 4],
    pub light_direction: [f32; 2],
    pub light_intensity: f32,
    pub ambient_strength: f32,
    pub blend: f32,
}

impl Default for LiquidGlassParams {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_darkness_enabled: false,
            auto_darkness_max: 0.16,
            thickness: 26.0,
            refractive_index: 1.42,
            chromatic_aberration: 0.075,
            blur_radius: 16.0,
            saturation: 1.34,
            glass_darkness: 0.0,
            glass_color: [0.94, 0.98, 1.0, 0.045],
            light_direction: normalize2([-0.45, -0.9]),
            light_intensity: 1.25,
            ambient_strength: 0.28,
            blend: 26.0,
        }
    }
}

/// Session-only update policy used to compare automatic-darkness cost from
/// the settings debug panel without changing persisted visual preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoDarknessUpdateMode {
    FullRate,
    #[default]
    Adaptive,
    Fixed8Hz,
}

impl AutoDarknessUpdateMode {
    pub const ALL: [Self; 3] = [Self::FullRate, Self::Adaptive, Self::Fixed8Hz];

    pub const fn key(self) -> &'static str {
        match self {
            Self::FullRate => "full-rate",
            Self::Adaptive => "adaptive",
            Self::Fixed8Hz => "fixed-8hz",
        }
    }

    pub const fn shader_value(self) -> u32 {
        match self {
            Self::FullRate => 0,
            Self::Adaptive => 1,
            Self::Fixed8Hz => 2,
        }
    }
}

/// Session-only backdrop classification formula compared for issue #186.
/// Changing the strategy resets the auto-darkness GPU state so each formula
/// starts from a clean transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoDarknessStrategy {
    /// PR #184 white-coverage heuristic (baseline, unchanged behavior).
    #[default]
    Current,
    /// Same coverage pipeline with visually recalibrated constants.
    CoverageTuned,
    /// Perceptual-luminance percentile inverted into a target contrast.
    ContrastPerceptual,
}

impl AutoDarknessStrategy {
    pub const ALL: [Self; 3] = [Self::Current, Self::CoverageTuned, Self::ContrastPerceptual];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::CoverageTuned => "coverage-tuned",
            Self::ContrastPerceptual => "contrast-perceptual",
        }
    }

    pub const fn shader_value(self) -> u32 {
        match self {
            Self::Current => 0,
            Self::CoverageTuned => 1,
            Self::ContrastPerceptual => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DebugOptions {
    pub show_backdrop_texture: bool,
    pub show_geometry_texture: bool,
    pub show_displacement: bool,
    pub show_alpha_mask: bool,
    pub show_final_glass_only: bool,
    pub disable_chromatic_aberration: bool,
    pub disable_edge_lighting: bool,
    pub disable_blur: bool,
}

impl DebugOptions {
    pub fn flags(self) -> u32 {
        let mut flags = 0;
        flags |= self.show_backdrop_texture as u32;
        flags |= (self.show_geometry_texture as u32) << 1;
        flags |= (self.show_displacement as u32) << 2;
        flags |= (self.show_alpha_mask as u32) << 3;
        flags |= (self.show_final_glass_only as u32) << 4;
        flags |= (self.disable_chromatic_aberration as u32) << 5;
        flags |= (self.disable_edge_lighting as u32) << 6;
        flags |= (self.disable_blur as u32) << 7;
        flags
    }
}

fn normalize2(v: [f32; 2]) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len <= f32::EPSILON {
        [1.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_darkness_modes_have_stable_shader_values_and_keys() {
        let expected = [
            (AutoDarknessUpdateMode::FullRate, 0, "full-rate"),
            (AutoDarknessUpdateMode::Adaptive, 1, "adaptive"),
            (AutoDarknessUpdateMode::Fixed8Hz, 2, "fixed-8hz"),
        ];
        for (mode, shader_value, key) in expected {
            assert_eq!(mode.shader_value(), shader_value);
            assert_eq!(mode.key(), key);
        }
        assert_eq!(
            AutoDarknessUpdateMode::default(),
            AutoDarknessUpdateMode::Adaptive
        );
    }

    #[test]
    fn auto_darkness_default_max_preserves_existing_appearance() {
        assert_eq!(LiquidGlassParams::default().auto_darkness_max, 0.16);
    }
}
