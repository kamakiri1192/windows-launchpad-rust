//! Scan the Start Menu into a snapshot of [`SnapshotEntry`] records.
//!
//! This is the *fast* side of icon loading: it walks the two Start Menu roots,
//! resolves each `.lnk`'s target + icon location, and reads mtimes — but it
//! does **not** touch Shell/GDI for pixels. That separation is what lets us
//! show the app list + placeholders before any icon is extracted, and what
//! makes polling-based diff detection cheap.
//!
//! The expensive Win32 calls here (`SHGetKnownFolderPath`, `IShellLinkW`,
//! `GetFileAttributesExW`) all live behind [`scan_start_menu`]; the diffing is
//! pure and lives in [`crate::domain::app_diff`].

use std::collections::BTreeMap;
use std::path::Path;

use crate::domain::app_diff::SnapshotEntry;
use crate::domain::app_id::AppId;
use crate::icons::extract::{self, enumerate_start_menu};

/// Scan both Start Menu roots and build a `BTreeMap<AppId, SnapshotEntry>`.
///
/// Failures on individual shortcuts are logged and skipped; one unreadable
/// `.lnk` can't blank the whole grid. The map is keyed by stable `AppId` so two
/// scans of the same set compare equal regardless of iteration order.
pub fn scan_start_menu() -> BTreeMap<AppId, SnapshotEntry> {
    let _com = extract::ComScope::new();
    let shortcuts = enumerate_start_menu();
    let mut out = BTreeMap::new();
    for sc in shortcuts {
        let app_id = AppId::from_link_path(&sc.path);
        let link_mtime = extract::file_mtime(&sc.path);

        let meta = extract::resolve_lnk_metadata(&sc.path).unwrap_or_default();
        let target_mtime = if meta.target_path.is_empty() {
            0
        } else {
            extract::file_mtime(Path::new(&meta.target_path))
        };

        let entry = SnapshotEntry {
            app_id: app_id.clone(),
            name: sc.name,
            link_path: sc.path.to_string_lossy().into_owned(),
            link_mtime,
            target_path: meta.target_path,
            target_mtime,
            icon_location: meta.icon_location,
            icon_index: meta.icon_index,
        };
        // Duplicate ids (same file via two roots) collapse; last one wins,
        // which is fine — they're the same shortcut.
        out.insert(app_id, entry);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_returns_map_even_when_empty() {
        // We can't assert a real count without a Start Menu, but the function
        // must never panic and must return a usable map type.
        let m = scan_start_menu();
        // Keys must be stable ids (normalized). If any are present, verify shape.
        for (id, e) in &m {
            assert_eq!(id.as_str(), &e.app_id.as_str().to_string());
            assert!(!e.link_path.is_empty());
        }
    }
}
