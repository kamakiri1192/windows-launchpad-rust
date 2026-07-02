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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sort_order: SortOrder::Name,
            frequent_apps_enabled: false,
            search_includes_hidden: false,
        }
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
    }

    #[test]
    fn settings_round_trip_json() {
        let s = Settings {
            sort_order: SortOrder::Frequent,
            frequent_apps_enabled: true,
            search_includes_hidden: true,
        };
        let bytes = serde_json::to_vec(&s).unwrap();
        let decoded: Settings = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, s);
    }
}
