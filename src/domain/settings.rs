#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SortOrder {
    Name,
    Manual,
    Recent,
    Frequent,
}

impl SortOrder {
    pub const ALL: [Self; 4] = [Self::Name, Self::Manual, Self::Recent, Self::Frequent];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "名前順",
            Self::Manual => "手動",
            Self::Recent => "最近使用",
            Self::Frequent => "よく使用",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub sort_order: SortOrder,
    pub frequent_apps_enabled: bool,
    pub search_includes_hidden: bool,
    #[serde(default = "default_show_steam_apps")]
    pub show_steam_apps: bool,
    /// Enables single-key developer shortcuts (`M` decoration toggle, `R`
    /// icon-cache reset, and the Liquid Glass parameter/debug keys). Off by
    /// default and on upgrade so production builds ship with debug keys inert
    /// until the user opts in from the settings panel.
    #[serde(default)]
    pub debug_keys_enabled: bool,
    /// Shows the on-screen FPS overlay (top-right). The frame rate is
    /// measured from real presentation statistics where the platform exposes
    /// them (DXGI `GetFrameStatistics` on Windows) and from a
    /// `frame.present()` cadence EMA otherwise. Off by default.
    #[serde(default)]
    pub show_fps: bool,
    /// Persisted Liquid Glass parameters (the master switch plus the six
    /// numeric parameters exposed in the settings panel). Debug-only flags
    /// (the B/G/D/A/F debug views and the C/E/L disable toggles) and the
    /// window-decoration toggle are *not* persisted: they reset on every
    /// launch so a stale debug view can never survive a restart.
    #[serde(default)]
    pub liquid_glass: LiquidGlassSettings,
}

const fn default_show_steam_apps() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sort_order: SortOrder::Name,
            frequent_apps_enabled: false,
            search_includes_hidden: false,
            show_steam_apps: true,
            debug_keys_enabled: false,
            show_fps: false,
            liquid_glass: LiquidGlassSettings::default(),
        }
    }
}

/// Persisted subset of [`crate::liquid_glass::LiquidGlassParams`]. Only the
/// fields that are exposed (and make sense to keep) across restarts live
/// here. The default values mirror `LiquidGlassParams::default()` so that a
/// fresh install and a "reset to defaults" both land on the coded baseline.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LiquidGlassSettings {
    #[serde(default = "default_lg_enabled")]
    pub enabled: bool,
    #[serde(default = "default_lg_thickness")]
    pub thickness: f32,
    #[serde(default = "default_lg_refractive_index")]
    pub refractive_index: f32,
    #[serde(default = "default_lg_saturation")]
    pub saturation: f32,
    #[serde(default = "default_lg_adaptive_darkness")]
    pub adaptive_darkness: f32,
    #[serde(default = "default_lg_chromatic_aberration")]
    pub chromatic_aberration: f32,
    #[serde(default = "default_lg_blur_radius")]
    pub blur_radius: f32,
}

impl Default for LiquidGlassSettings {
    fn default() -> Self {
        Self {
            enabled: default_lg_enabled(),
            thickness: default_lg_thickness(),
            refractive_index: default_lg_refractive_index(),
            saturation: default_lg_saturation(),
            adaptive_darkness: default_lg_adaptive_darkness(),
            chromatic_aberration: default_lg_chromatic_aberration(),
            blur_radius: default_lg_blur_radius(),
        }
    }
}

impl LiquidGlassSettings {
    /// Reset every field to its coded default (the same values the Liquid
    /// Glass renderer boots with).
    pub fn reset_to_defaults(&mut self) {
        *self = Self::default();
    }

    /// Returns `true` if any field still differs from the coded default.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

const fn default_lg_enabled() -> bool {
    true
}

const fn default_lg_thickness() -> f32 {
    26.0
}

const fn default_lg_refractive_index() -> f32 {
    1.42
}

const fn default_lg_saturation() -> f32 {
    1.34
}

const fn default_lg_adaptive_darkness() -> f32 {
    0.65
}

const fn default_lg_chromatic_aberration() -> f32 {
    0.075
}

const fn default_lg_blur_radius() -> f32 {
    16.0
}

/// Identifies one of the six numeric Liquid Glass parameters that the
/// settings panel exposes as a slider. Used by hit-testing, drag tracking,
/// and the per-parameter reset action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidGlassParamField {
    Thickness,
    RefractiveIndex,
    Saturation,
    AdaptiveDarkness,
    ChromaticAberration,
    BlurRadius,
}

impl LiquidGlassParamField {
    pub const ALL: [Self; 6] = [
        Self::Thickness,
        Self::RefractiveIndex,
        Self::Saturation,
        Self::AdaptiveDarkness,
        Self::ChromaticAberration,
        Self::BlurRadius,
    ];

    /// `(min, max, default)` for the slider, matching the keyboard handler's
    /// clamp ranges in `liquid_glass/renderer.rs::handle_debug_key`.
    pub const fn range(self) -> (f32, f32, f32) {
        match self {
            Self::Thickness => (6.0, 48.0, default_lg_thickness()),
            Self::RefractiveIndex => (1.02, 1.75, default_lg_refractive_index()),
            Self::Saturation => (0.5, 2.0, default_lg_saturation()),
            Self::AdaptiveDarkness => (0.0, 1.0, default_lg_adaptive_darkness()),
            Self::ChromaticAberration => (0.0, 0.18, default_lg_chromatic_aberration()),
            Self::BlurRadius => (0.0, 40.0, default_lg_blur_radius()),
        }
    }

    pub fn get(self, s: &LiquidGlassSettings) -> f32 {
        match self {
            Self::Thickness => s.thickness,
            Self::RefractiveIndex => s.refractive_index,
            Self::Saturation => s.saturation,
            Self::AdaptiveDarkness => s.adaptive_darkness,
            Self::ChromaticAberration => s.chromatic_aberration,
            Self::BlurRadius => s.blur_radius,
        }
    }

    pub fn set(self, s: &mut LiquidGlassSettings, value: f32) {
        let (min, max, _) = self.range();
        let value = value.clamp(min, max);
        match self {
            Self::Thickness => s.thickness = value,
            Self::RefractiveIndex => s.refractive_index = value,
            Self::Saturation => s.saturation = value,
            Self::AdaptiveDarkness => s.adaptive_darkness = value,
            Self::ChromaticAberration => s.chromatic_aberration = value,
            Self::BlurRadius => s.blur_radius = value,
        }
    }

    /// Stable id used for `UiId` / hit-target strings.
    pub const fn key(self) -> &'static str {
        match self {
            Self::Thickness => "thickness",
            Self::RefractiveIndex => "refractive-index",
            Self::Saturation => "saturation",
            Self::AdaptiveDarkness => "adaptive-darkness",
            Self::ChromaticAberration => "chromatic-aberration",
            Self::BlurRadius => "blur-radius",
        }
    }
}

/// Identifies one of the Liquid Glass debug flags toggled from the settings
/// panel. These mirror the B/G/D/A/F debug-view keys and the C/E/L disable
/// keys, but are *session-only*: they are never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiquidGlassDebugFlag {
    // Disable flags (C / E / L)
    DisableChromaticAberration,
    DisableEdgeLighting,
    DisableBlur,
    // Debug view overlays (B / G / D / A / F)
    ShowBackdropTexture,
    ShowGeometryTexture,
    ShowDisplacement,
    ShowAlphaMask,
    ShowFinalGlassOnly,
}

impl LiquidGlassDebugFlag {
    pub const ALL: [Self; 8] = [
        Self::DisableChromaticAberration,
        Self::DisableEdgeLighting,
        Self::DisableBlur,
        Self::ShowBackdropTexture,
        Self::ShowGeometryTexture,
        Self::ShowDisplacement,
        Self::ShowAlphaMask,
        Self::ShowFinalGlassOnly,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::DisableChromaticAberration => "disable-chromatic-aberration",
            Self::DisableEdgeLighting => "disable-edge-lighting",
            Self::DisableBlur => "disable-blur",
            Self::ShowBackdropTexture => "show-backdrop-texture",
            Self::ShowGeometryTexture => "show-geometry-texture",
            Self::ShowDisplacement => "show-displacement",
            Self::ShowAlphaMask => "show-alpha-mask",
            Self::ShowFinalGlassOnly => "show-final-glass-only",
        }
    }
}

impl Settings {
    pub fn shows_app(&self, app_id: &crate::domain::app_id::AppId) -> bool {
        self.show_steam_apps || !app_id.is_steam()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsCategory {
    Apps,
    Search,
    System,
    About,
    Debug,
}

impl SettingsCategory {
    pub const ALL: [Self; 5] = [
        Self::Apps,
        Self::Search,
        Self::System,
        Self::About,
        Self::Debug,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Apps => "アプリ",
            Self::Search => "表示と検索",
            Self::System => "システム",
            Self::About => "このアプリについて",
            Self::Debug => "デバッグ",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_v1_settings() {
        let s = Settings::default();
        assert_eq!(s.sort_order, SortOrder::Name);
        assert!(!s.frequent_apps_enabled);
        assert!(!s.search_includes_hidden);
        assert!(s.show_steam_apps);
        assert!(!s.debug_keys_enabled);
        assert!(!s.show_fps);
        assert!(s.liquid_glass.is_default());
    }

    #[test]
    fn settings_round_trip_json() {
        let s = Settings {
            sort_order: SortOrder::Frequent,
            frequent_apps_enabled: true,
            search_includes_hidden: true,
            show_steam_apps: false,
            debug_keys_enabled: true,
            show_fps: true,
            liquid_glass: LiquidGlassSettings {
                enabled: false,
                thickness: 40.0,
                refractive_index: 1.5,
                saturation: 1.8,
                adaptive_darkness: 0.4,
                chromatic_aberration: 0.1,
                blur_radius: 24.0,
            },
        };
        let bytes = serde_json::to_vec(&s).unwrap();
        let decoded: Settings = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, s);
    }

    #[test]
    fn older_json_defaults_steam_apps_to_visible() {
        let json = br#"{
            "sort_order":"Name",
            "frequent_apps_enabled":false,
            "search_includes_hidden":false
        }"#;
        let decoded: Settings = serde_json::from_slice(json).unwrap();
        assert!(decoded.show_steam_apps);
    }

    #[test]
    fn older_json_defaults_debug_keys_to_disabled() {
        let json = br#"{
            "sort_order":"Name",
            "frequent_apps_enabled":false,
            "search_includes_hidden":false,
            "show_steam_apps":true
        }"#;
        let decoded: Settings = serde_json::from_slice(json).unwrap();
        assert!(!decoded.debug_keys_enabled);
    }

    #[test]
    fn older_json_defaults_show_fps_to_disabled() {
        let json = br#"{
            "sort_order":"Name",
            "frequent_apps_enabled":false,
            "search_includes_hidden":false,
            "show_steam_apps":true,
            "debug_keys_enabled":false
        }"#;
        let decoded: Settings = serde_json::from_slice(json).unwrap();
        assert!(!decoded.show_fps);
    }

    #[test]
    fn older_json_defaults_liquid_glass_to_renderer_baseline() {
        // JSON written by an older build (before Liquid Glass was persisted).
        // It must decode with the same numeric baseline the renderer boots
        // with, so existing users see no visual change on upgrade. The values
        // below are inlined (rather than read from
        // `liquid_glass::LiquidGlassParams::default()`) because this crate is
        // wgpu-free and the renderer lives in the binary target.
        let json = br#"{
            "sort_order":"Name",
            "frequent_apps_enabled":false,
            "search_includes_hidden":false,
            "show_steam_apps":true,
            "debug_keys_enabled":false
        }"#;
        let decoded: Settings = serde_json::from_slice(json).unwrap();
        assert!(decoded.liquid_glass.enabled);
        assert_eq!(decoded.liquid_glass.thickness, default_lg_thickness());
        assert_eq!(
            decoded.liquid_glass.refractive_index,
            default_lg_refractive_index()
        );
        assert_eq!(decoded.liquid_glass.saturation, default_lg_saturation());
        assert_eq!(
            decoded.liquid_glass.adaptive_darkness,
            default_lg_adaptive_darkness()
        );
        assert_eq!(
            decoded.liquid_glass.chromatic_aberration,
            default_lg_chromatic_aberration()
        );
        assert_eq!(decoded.liquid_glass.blur_radius, default_lg_blur_radius());
    }

    #[test]
    fn liquid_glass_settings_defaults_match_renderer() {
        // Mirrors `liquid_glass::LiquidGlassParams::default()` (see the note
        // in `older_json_defaults_liquid_glass_to_renderer_baseline` for why
        // we inline the constants here).
        let s = LiquidGlassSettings::default();
        assert!(s.enabled);
        assert_eq!(s.thickness, 26.0);
        assert_eq!(s.refractive_index, 1.42);
        assert_eq!(s.saturation, 1.34);
        assert_eq!(s.adaptive_darkness, 0.65);
        assert_eq!(s.chromatic_aberration, 0.075);
        assert_eq!(s.blur_radius, 16.0);
    }

    #[test]
    fn liquid_glass_reset_to_defaults_restores_baseline() {
        let mut s = LiquidGlassSettings {
            enabled: false,
            thickness: 6.0,
            refractive_index: 1.02,
            saturation: 0.5,
            adaptive_darkness: 0.0,
            chromatic_aberration: 0.0,
            blur_radius: 0.0,
        };
        assert!(!s.is_default());
        s.reset_to_defaults();
        assert!(s.is_default());
        assert_eq!(s, LiquidGlassSettings::default());
    }

    #[test]
    fn liquid_glass_param_field_set_clamps_to_range() {
        let mut s = LiquidGlassSettings::default();
        // Above the max is clamped down.
        LiquidGlassParamField::Thickness.set(&mut s, 999.0);
        assert_eq!(s.thickness, 48.0);
        // Below the min is clamped up.
        LiquidGlassParamField::Thickness.set(&mut s, -10.0);
        assert_eq!(s.thickness, 6.0);
        // In-range value passes through.
        LiquidGlassParamField::Thickness.set(&mut s, 30.0);
        assert_eq!(s.thickness, 30.0);
    }

    #[test]
    fn liquid_glass_param_field_range_defaults_match_settings_default() {
        for field in LiquidGlassParamField::ALL {
            let (min, max, default) = field.range();
            assert!(
                min <= default && default <= max,
                "field {:?} default out of range",
                field
            );
            assert_eq!(field.get(&LiquidGlassSettings::default()), default);
        }
    }

    #[test]
    fn steam_visibility_only_filters_steam_ids() {
        let mut settings = Settings::default();
        let steam = crate::domain::app_id::AppId::from_normalized("steam:620");
        let regular = crate::domain::app_id::AppId::from_normalized("c:/portal 2.lnk");

        assert!(settings.shows_app(&steam));
        settings.show_steam_apps = false;
        assert!(!settings.shows_app(&steam));
        assert!(settings.shows_app(&regular));
    }
}
