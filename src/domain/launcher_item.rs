//! User-owned launcher items: apps and folders on the launcher grid.
//!
//! Phase 7 introduces `LauncherItem` as the thing the launcher grid lays out.
//! Until now the grid was "an array of apps"; with folders coming in Phase 8 it
//! becomes "an ordered list of launcher items", where each item is either an
//! app reference or a folder reference.
//!
//! `LauncherItem` is intentionally a thin enum: it only carries stable ids, not
//! any rediscoverable data (icon, name, target path). Discovered-app data lives
//! in [`crate::domain::app_registry::AppRegistry`]; folder display data lives in
//! [`crate::domain::folders::Folder`]. Keeping the item itself id-only is what
//! lets the user's arrangement survive rescans untouched.

use crate::domain::app_id::AppId;
use crate::domain::folders::FolderId;

/// One entry in the user-owned launcher layout.
///
/// Order in the launcher grid is defined by a `Vec<LauncherItem>`; each variant
/// only references a stable id so the layout is independent of rediscoverable
/// app or folder display data.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum LauncherItem {
    /// A top-level app tile.
    App(AppId),
    /// A top-level folder tile.
    Folder(FolderId),
}

impl LauncherItem {
    /// Build an app item.
    pub fn app(id: impl Into<AppId>) -> Self {
        Self::App(id.into())
    }

    /// Build a folder item.
    pub fn folder(id: impl Into<FolderId>) -> Self {
        Self::Folder(id.into())
    }

    /// The app id if this is an app item, else `None`.
    pub fn as_app_id(&self) -> Option<&AppId> {
        match self {
            LauncherItem::App(id) => Some(id),
            LauncherItem::Folder(_) => None,
        }
    }

    /// The folder id if this is a folder item, else `None`.
    pub fn as_folder_id(&self) -> Option<&FolderId> {
        match self {
            LauncherItem::App(_) => None,
            LauncherItem::Folder(id) => Some(id),
        }
    }

    /// True if this item is an app item holding `app_id`.
    pub fn is_app(&self, app_id: &AppId) -> bool {
        matches!(self, LauncherItem::App(id) if id == app_id)
    }

    /// Stable string key suitable for `UiId` construction. Apps use the
    /// normalized path; folders use the folder id key. This is the boundary
    /// between a domain id and a renderer-neutral `UiId`.
    pub fn stable_key(&self) -> String {
        match self {
            LauncherItem::App(id) => id.as_str().to_string(),
            LauncherItem::Folder(id) => id.as_str().to_string(),
        }
    }
}

impl From<AppId> for LauncherItem {
    fn from(id: AppId) -> Self {
        LauncherItem::App(id)
    }
}

impl From<FolderId> for LauncherItem {
    fn from(id: FolderId) -> Self {
        LauncherItem::Folder(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(s: &str) -> AppId {
        AppId::from_normalized(s.to_string())
    }

    #[test]
    fn app_item_projects_to_app_id() {
        let item = LauncherItem::app(app("a"));
        assert_eq!(item.as_app_id(), Some(&app("a")));
        assert_eq!(item.as_folder_id(), None);
        assert!(item.is_app(&app("a")));
        assert!(!item.is_app(&app("b")));
    }

    #[test]
    fn folder_item_projects_to_folder_id() {
        let item = LauncherItem::folder(FolderId::from_normalized("f1"));
        assert_eq!(item.as_app_id(), None);
        assert!(item.as_folder_id().is_some());
    }

    #[test]
    fn stable_key_round_trips_for_app() {
        let item = LauncherItem::app(app("c:/x.lnk"));
        assert_eq!(item.stable_key(), "c:/x.lnk");
    }

    #[test]
    fn from_conversions_preserve_id() {
        let id = app("a");
        let item: LauncherItem = id.clone().into();
        assert_eq!(item, LauncherItem::App(app("a")));

        let fid = FolderId::from_normalized("folder");
        let item: LauncherItem = fid.into();
        assert_eq!(
            item,
            LauncherItem::Folder(FolderId::from_normalized("folder"))
        );
    }

    #[test]
    fn items_order_by_enum_then_id() {
        // Apps and folders are comparable; apps sort before folders.
        let mut v = vec![
            LauncherItem::Folder(FolderId::from_normalized("z")),
            LauncherItem::App(app("b")),
            LauncherItem::App(app("a")),
            LauncherItem::Folder(FolderId::from_normalized("a")),
        ];
        v.sort();
        assert_eq!(
            v,
            vec![
                LauncherItem::App(app("a")),
                LauncherItem::App(app("b")),
                LauncherItem::Folder(FolderId::from_normalized("a")),
                LauncherItem::Folder(FolderId::from_normalized("z")),
            ]
        );
    }
}
