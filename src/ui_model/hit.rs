#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HitTarget {
    LauncherItem {
        key: String,
    },
    LauncherItemBadge {
        key: String,
    },
    LauncherCell {
        index: usize,
    },
    FolderPanel {
        key: String,
    },
    FolderTitle {
        key: String,
    },
    FolderChild {
        folder: String,
        child: String,
        index: usize,
    },
    FolderChildBadge {
        folder: String,
        child: String,
        index: usize,
    },
    FolderPagePrevious {
        key: String,
    },
    FolderPageNext {
        key: String,
    },
    BottomControl,
    BottomControlClose,
    SearchField,
    EditSettingsGear,
    SettingsPanel,
    Settings {
        target: SettingsTarget,
    },
    Backdrop {
        kind: BackdropKind,
    },
}

impl HitTarget {
    pub fn launcher_item(key: impl Into<String>) -> Self {
        Self::LauncherItem { key: key.into() }
    }

    pub fn launcher_item_badge(key: impl Into<String>) -> Self {
        Self::LauncherItemBadge { key: key.into() }
    }

    pub const fn launcher_cell(index: usize) -> Self {
        Self::LauncherCell { index }
    }

    pub fn folder_panel(key: impl Into<String>) -> Self {
        Self::FolderPanel { key: key.into() }
    }

    pub fn folder_title(key: impl Into<String>) -> Self {
        Self::FolderTitle { key: key.into() }
    }

    pub fn folder_child(folder: impl Into<String>, child: impl Into<String>, index: usize) -> Self {
        Self::FolderChild {
            folder: folder.into(),
            child: child.into(),
            index,
        }
    }

    pub fn folder_child_badge(
        folder: impl Into<String>,
        child: impl Into<String>,
        index: usize,
    ) -> Self {
        Self::FolderChildBadge {
            folder: folder.into(),
            child: child.into(),
            index,
        }
    }

    pub fn settings_category(key: impl Into<String>) -> Self {
        Self::Settings {
            target: SettingsTarget::Category { key: key.into() },
        }
    }

    pub fn settings_sort_option(key: impl Into<String>) -> Self {
        Self::Settings {
            target: SettingsTarget::SortOption { key: key.into() },
        }
    }

    pub fn settings_toggle(key: impl Into<String>) -> Self {
        Self::Settings {
            target: SettingsTarget::Toggle { key: key.into() },
        }
    }

    pub fn settings_action(key: impl Into<String>) -> Self {
        Self::Settings {
            target: SettingsTarget::Action { key: key.into() },
        }
    }

    pub const fn launcher_passthrough_backdrop() -> Self {
        Self::Backdrop {
            kind: BackdropKind::LauncherPassthrough,
        }
    }

    pub const fn modal_dismiss_backdrop() -> Self {
        Self::Backdrop {
            kind: BackdropKind::ModalDismiss,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SettingsTarget {
    Panel,
    Close,
    Category { key: String },
    SortOption { key: String },
    Toggle { key: String },
    Action { key: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackdropKind {
    /// Transparent launcher area: a stationary left-click should hide the
    /// launcher and request OS click replay to the window underneath.
    LauncherPassthrough,
    /// Modal overlay backdrop: a click should dismiss the overlay without
    /// replaying input to the window underneath.
    ModalDismiss,
}

#[cfg(test)]
mod tests {
    use super::{BackdropKind, HitTarget, SettingsTarget};

    #[test]
    fn backdrop_targets_distinguish_passthrough_from_modal_dismiss() {
        assert_eq!(
            HitTarget::launcher_passthrough_backdrop(),
            HitTarget::Backdrop {
                kind: BackdropKind::LauncherPassthrough
            }
        );
        assert_ne!(
            HitTarget::launcher_passthrough_backdrop(),
            HitTarget::modal_dismiss_backdrop()
        );
    }

    #[test]
    fn settings_targets_preserve_current_click_intents() {
        assert_eq!(
            HitTarget::Settings {
                target: SettingsTarget::Close
            },
            HitTarget::Settings {
                target: SettingsTarget::Close
            }
        );
        assert_eq!(
            HitTarget::settings_category("apps"),
            HitTarget::Settings {
                target: SettingsTarget::Category {
                    key: "apps".to_owned()
                }
            }
        );
        assert_eq!(
            HitTarget::settings_sort_option("name"),
            HitTarget::Settings {
                target: SettingsTarget::SortOption {
                    key: "name".to_owned()
                }
            }
        );
        assert_eq!(
            HitTarget::settings_toggle("frequent-apps"),
            HitTarget::Settings {
                target: SettingsTarget::Toggle {
                    key: "frequent-apps".to_owned()
                }
            }
        );
        assert_eq!(
            HitTarget::settings_action("reset-cache"),
            HitTarget::Settings {
                target: SettingsTarget::Action {
                    key: "reset-cache".to_owned()
                }
            }
        );
    }
}
