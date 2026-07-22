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
}

impl SettingsCategory {
    pub const ALL: [Self; 4] = [Self::Apps, Self::Search, Self::System, Self::About];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Apps => "アプリ",
            Self::Search => "表示と検索",
            Self::System => "システム",
            Self::About => "このアプリについて",
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
    }

    #[test]
    fn settings_round_trip_json() {
        let s = Settings {
            sort_order: SortOrder::Frequent,
            frequent_apps_enabled: true,
            search_includes_hidden: true,
            show_steam_apps: false,
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
