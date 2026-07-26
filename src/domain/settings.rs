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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
