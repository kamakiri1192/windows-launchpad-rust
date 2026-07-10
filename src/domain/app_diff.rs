//! Compare two Start Menu snapshots into an added/updated/removed diff.
//!
//! A snapshot is a `BTreeMap<AppId, SnapshotEntry>`. Stable ids mean we can
//! diff two snapshots taken at different times and know *which* app changed,
//! independent of display order. This module is pure data — no Win32, no I/O —
//! so the diff logic is fully unit-testable.

use std::collections::BTreeMap;

use crate::domain::app_id::AppId;

/// One shortcut's salient fields at scan time. The fields are exactly the ones
/// that, if changed, should trigger an icon re-extraction (see
/// `docs/ICON_CACHE.md` → cache invalidation). Kept small so a snapshot is
/// cheap to hold in memory between polls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    pub app_id: AppId,
    /// Display name (file stem of the `.lnk`).
    pub name: String,
    /// `.lnk` path on disk.
    pub link_path: String,
    /// Last-modified time of the `.lnk` (Windows file time, or any monotonic
    /// int the scanner produces; we only compare for equality).
    pub link_mtime: u64,
    /// Resolved target path the shortcut points at, expanded, or `""` if the
    /// shortcut had no resolvable target.
    pub target_path: String,
    /// Target file mtime, or `0`.
    pub target_mtime: u64,
    /// Shell-reported IconLocation string (may be empty → icon lives in the
    /// target exe).
    pub icon_location: String,
    /// Shell-reported icon index inside `icon_location`.
    pub icon_index: i32,
}

impl SnapshotEntry {
    /// True when two entries for the same `app_id` differ in any field that
    /// should invalidate the cached icon.
    ///
    /// Display-name-only changes do *not* invalidate (the icon is unaffected),
    /// but we still surface them as `Updated` so the label refreshes.
    pub fn icon_relevant_diff(&self, other: &SnapshotEntry) -> bool {
        self.link_mtime != other.link_mtime
            || self.target_path != other.target_path
            || self.target_mtime != other.target_mtime
            || self.icon_location != other.icon_location
            || self.icon_index != other.icon_index
    }
}

/// The result of comparing two snapshots.
#[derive(Debug, Default, Clone)]
pub struct AppDiff {
    /// Present in the new snapshot, absent from the old.
    pub added: Vec<SnapshotEntry>,
    /// Same id in both, but some field changed.
    pub updated: Vec<SnapshotEntry>,
    /// Present in the old snapshot, gone from the new.
    pub removed: Vec<AppId>,
}

impl AppDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.removed.is_empty()
    }

    pub fn total(&self) -> usize {
        self.added.len() + self.updated.len() + self.removed.len()
    }
}

/// Compute `new − old` as an [`AppDiff`].
///
/// - `added`: ids in `new` but not `old`.
/// - `removed`: ids in `old` but not `new`.
/// - `updated`: ids in both whose entries differ.
pub fn diff_snapshots(
    old: &BTreeMap<AppId, SnapshotEntry>,
    new: &BTreeMap<AppId, SnapshotEntry>,
) -> AppDiff {
    let mut out = AppDiff::default();
    for (id, entry) in new {
        match old.get(id) {
            None => out.added.push(entry.clone()),
            Some(prev) if prev != entry => out.updated.push(entry.clone()),
            _ => {}
        }
    }
    for id in old.keys() {
        if !new.contains_key(id) {
            out.removed.push(id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(name: &str, mtime: u64) -> SnapshotEntry {
        let path = format!("C:\\ProgramData\\Start Menu\\{name}.lnk");
        SnapshotEntry {
            app_id: AppId::from_link_path(PathBuf::from(&path)),
            name: name.to_string(),
            link_path: path,
            link_mtime: mtime,
            target_path: String::new(),
            target_mtime: 0,
            icon_location: String::new(),
            icon_index: 0,
        }
    }

    fn snap(entries: &[SnapshotEntry]) -> BTreeMap<AppId, SnapshotEntry> {
        entries
            .iter()
            .map(|e| (e.app_id.clone(), e.clone()))
            .collect()
    }

    #[test]
    fn identical_snapshots_produce_empty_diff() {
        let a = snap(&[entry("A", 1), entry("B", 2)]);
        assert!(diff_snapshots(&a, &a).is_empty());
    }

    #[test]
    fn detects_added() {
        let old = snap(&[entry("A", 1)]);
        let new = snap(&[entry("A", 1), entry("B", 2)]);
        let d = diff_snapshots(&old, &new);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.added[0].name, "B");
        assert!(d.updated.is_empty());
        assert!(d.removed.is_empty());
    }

    #[test]
    fn detects_removed() {
        let old = snap(&[entry("A", 1), entry("B", 2)]);
        let new = snap(&[entry("A", 1)]);
        let d = diff_snapshots(&old, &new);
        assert_eq!(d.removed.len(), 1);
        assert!(d.added.is_empty());
        assert!(d.updated.is_empty());
    }

    #[test]
    fn detects_updated_mtime() {
        let old = snap(&[entry("A", 1)]);
        let new = snap(&[entry("A", 2)]);
        let d = diff_snapshots(&old, &new);
        assert_eq!(d.updated.len(), 1);
        assert!(d.added.is_empty());
        assert!(d.removed.is_empty());
        // mtime change is icon-relevant.
        assert!(old[&d.updated[0].app_id].icon_relevant_diff(&d.updated[0]));
    }

    #[test]
    fn detects_updated_icon_location_only() {
        let a = entry("A", 1);
        let mut b = a.clone();
        b.icon_location = "C:\\icons.dll".to_string();
        let old = snap(&[a]);
        let new = snap(&[b]);
        let d = diff_snapshots(&old, &new);
        assert_eq!(d.updated.len(), 1);
        let prev = &old[&d.updated[0].app_id];
        assert!(prev.icon_relevant_diff(&d.updated[0]));
    }

    #[test]
    fn name_only_change_is_updated_but_not_icon_relevant() {
        let a = entry("A", 1);
        let mut b = a.clone();
        b.name = "Renamed".to_string();
        assert_ne!(a, b);
        assert!(!a.icon_relevant_diff(&b));
    }

    #[test]
    fn diff_handles_all_three_at_once() {
        let old = snap(&[entry("Keep", 1), entry("Gone", 1), entry("Change", 1)]);
        let new = snap(&[entry("Keep", 1), entry("Change", 9), entry("New", 1)]);
        let d = diff_snapshots(&old, &new);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.updated.len(), 1);
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.total(), 3);
    }
}
