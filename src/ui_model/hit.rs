#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HitTarget {
    LauncherItem { key: String },
    LauncherItemBadge { key: String },
    BottomControl,
    BottomControlClose,
    SettingsPanel,
    SettingsClose,
    SettingsRow { key: String },
    Backdrop,
}

impl HitTarget {
    pub fn launcher_item(key: impl Into<String>) -> Self {
        Self::LauncherItem { key: key.into() }
    }

    pub fn launcher_item_badge(key: impl Into<String>) -> Self {
        Self::LauncherItemBadge { key: key.into() }
    }

    pub fn settings_row(key: impl Into<String>) -> Self {
        Self::SettingsRow { key: key.into() }
    }
}
