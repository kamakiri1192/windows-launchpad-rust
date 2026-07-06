#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HitTarget {
    LauncherItem { key: String },
    LauncherItemBadge { key: String },
    BottomControl,
    BottomControlClose,
    SettingsPanel,
    SettingsClose,
    SettingsRow { key: String },
    Backdrop { kind: BackdropKind },
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
    use super::{BackdropKind, HitTarget};

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
}
