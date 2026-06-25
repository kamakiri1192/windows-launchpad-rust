//! App registry — the source of truth for "what apps exist and where is each one".
//!
//! This replaces the old `LoadedIcons { apps, atlas }` combo. Splitting the
//! *app list* from the *icon atlas* is what lets us:
//!   - show the app list + labels before any icon is extracted,
//!   - apply icons incrementally as the worker reports them,
//!   - add / remove / update apps at runtime without repacking the whole atlas.
//!
//! The registry keeps apps in a stable, sorted-by-display-name order and maps
//! each [`AppId`] to a stable **slot** in the icon atlas (see [`IconAtlas`]).
//! Click resolution goes through `app_id`, never a raw positional index, so a
//! rescan that inserts/removes apps can't shift which app a click hits.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::app_id::AppId;
use crate::icons::UvRect;

/// Lifecycle state of one app's icon. Drives placeholder rendering and which
/// apps the worker should (re)extract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    /// Never been looked at.
    Missing,
    /// Worker has been asked for it; not yet returned.
    Loading,
    /// Served from the on-disk cache at startup (may still be revalidated).
    Cached,
    /// Freshly extracted this session and on the atlas.
    Loaded,
    /// Extraction failed; keep the color-tile placeholder until a rescan
    /// reports a change.
    Failed,
    /// Cache entry exists but probe fields changed; worker is re-extracting.
    Stale,
}

impl IconState {
    pub fn has_pixels(self) -> bool {
        matches!(self, IconState::Cached | IconState::Loaded)
    }
}

/// One app, as the registry stores it. `uv` is `Some` once an icon is on the
/// atlas; the slot index is fixed for the app's lifetime in the registry.
#[derive(Debug, Clone)]
pub struct AppRecord {
    pub app_id: AppId,
    pub name: String,
    pub link_path: PathBuf,
    pub resolved_target: PathBuf,
    /// Stable atlas slot. Used both as the atlas cell and as the
    /// `icon_index` the GPU instance buffer carries.
    pub slot: u32,
    pub icon_state: IconState,
    pub uv: Option<UvRect>,
}

/// Snapshot of one app for click-to-launch. Owns its data so it stays valid
/// even if the registry is mutated between pick and launch.
#[derive(Debug, Clone)]
pub struct AppLaunchInfo {
    pub name: String,
    pub link_path: PathBuf,
}

impl From<&AppRecord> for AppLaunchInfo {
    fn from(r: &AppRecord) -> Self {
        Self {
            name: r.name.clone(),
            link_path: r.link_path.clone(),
        }
    }
}

/// The registry: an ordered list of records plus an id→index map.
///
/// Ordering is by display name (case-insensitive), so the grid order is stable
/// across rescans even when apps are added/removed — only the diff changes.
#[derive(Debug, Default)]
pub struct AppRegistry {
    /// Display-order apps.
    apps: Vec<AppRecord>,
    /// `app_id` → index into `apps`.
    by_id: HashMap<AppId, usize>,
    /// Next free slot index. Slots are never reused while an app holds them
    /// (so existing UVs never shift); deleted slots become free for *new* apps
    /// only after a compaction, which we don't do yet (the atlas grows).
    next_slot: u32,
}

impl AppRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.apps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }

    pub fn apps(&self) -> &[AppRecord] {
        &self.apps
    }

    pub fn get(&self, id: &AppId) -> Option<&AppRecord> {
        self.by_id.get(id).map(|&i| &self.apps[i])
    }

    /// Borrow a record mutably by id. Returns `None` if the id isn't present.
    pub fn get_mut(&mut self, id: &AppId) -> Option<&mut AppRecord> {
        let i = *self.by_id.get(id)?;
        Some(&mut self.apps[i])
    }

    /// Highest slot currently in use. The atlas sizes itself from this.
    pub fn max_slot(&self) -> u32 {
        self.apps.iter().map(|a| a.slot).max().unwrap_or(0)
    }

    /// Total slot count needed (slot values are 0-based and contiguous so far).
    /// Equals `next_slot`.
    pub fn slot_count(&self) -> u32 {
        self.next_slot
    }

    /// Insert a brand-new app, assigning it the next free slot. Returns `false`
    /// if the id already exists (caller should use [`update`][Self::update]).
    /// The registry is re-sorted by display name after insertion.
    pub fn insert(&mut self, record: AppRecord) -> bool {
        if self.by_id.contains_key(&record.app_id) {
            return false;
        }
        self.next_slot = self.next_slot.max(record.slot + 1);
        self.apps.push(record);
        self.reindex_and_sort();
        true
    }

    /// Apply a partial update to an existing app's mutable fields (name, target,
    /// icon state, uv). The slot is never changed here. Returns `false` if the
    /// id isn't known.
    pub fn update(&mut self, id: &AppId, f: impl FnOnce(&mut AppRecord)) -> bool {
        let Some(i) = self.by_id.get(id).copied() else {
            return false;
        };
        let name_before = self.apps[i].name.clone();
        f(&mut self.apps[i]);
        if self.apps[i].name != name_before {
            self.reindex_and_sort();
        }
        true
    }

    /// Remove an app by id. Its slot is *not* reused (UVs of other apps stay
    /// fixed); the cell just renders transparent until the atlas is rebuilt.
    pub fn remove(&mut self, id: &AppId) -> bool {
        let Some(i) = self.by_id.get(id).copied() else {
            return false;
        };
        self.apps.remove(i);
        self.reindex_and_sort();
        true
    }

    /// Rebuild `by_id` and sort `apps` by display name. Called after any
    /// structural change so iteration order stays predictable.
    fn reindex_and_sort(&mut self) {
        self.apps.sort_by_key(|a| a.name.to_lowercase());
        self.by_id.clear();
        self.by_id.reserve(self.apps.len());
        for (i, a) in self.apps.iter().enumerate() {
            self.by_id.insert(a.app_id.clone(), i);
        }
    }

    /// Allocate the next slot index. Used by callers building an `AppRecord`.
    pub fn alloc_slot(&mut self) -> u32 {
        let s = self.next_slot;
        self.next_slot += 1;
        s
    }

    /// Reset the registry entirely (used on full reload / corrupt state).
    pub fn clear(&mut self) {
        self.apps.clear();
        self.by_id.clear();
        self.next_slot = 0;
    }

    /// Snapshot lookup by id for click resolution.
    pub fn launch_info(&self, id: &AppId) -> Option<AppLaunchInfo> {
        self.get(id).map(AppLaunchInfo::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, name: &str) -> AppRecord {
        AppRecord {
            app_id: AppId::from_normalized(id.to_string()),
            name: name.to_string(),
            link_path: PathBuf::from(format!("C:\\{id}.lnk")),
            resolved_target: PathBuf::new(),
            slot: 0,
            icon_state: IconState::Missing,
            uv: None,
        }
    }

    #[test]
    fn insert_assigns_distinct_slots() {
        let mut r = AppRegistry::new();
        let mut a = rec("a", "A");
        a.slot = r.alloc_slot();
        let mut b = rec("b", "B");
        b.slot = r.alloc_slot();
        assert!(r.insert(a));
        assert!(r.insert(b));
        assert_ne!(r.apps()[0].slot, r.apps()[1].slot);
        assert_eq!(r.slot_count(), 2);
    }

    #[test]
    fn keeps_sorted_by_name() {
        let mut r = AppRegistry::new();
        for (id, name) in [("c", "Cherry"), ("a", "Apple"), ("b", "Banana")] {
            let mut x = rec(id, name);
            x.slot = r.alloc_slot();
            r.insert(x);
        }
        let names: Vec<_> = r.apps().iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "Banana", "Cherry"]);
    }

    #[test]
    fn update_keeps_slot_stable_and_renames_resort() {
        let mut r = AppRegistry::new();
        let mut a = rec("a", "Zeta");
        a.slot = r.alloc_slot();
        r.insert(a);
        let mut b = rec("b", "Alpha");
        b.slot = r.alloc_slot();
        r.insert(b);
        let id = AppId::from_normalized("a".to_string());
        let slot_before = r.get(&id).unwrap().slot;
        r.update(&id, |rec| {
            rec.name = "Aardvark".to_string();
        });
        assert_eq!(r.apps()[0].name, "Aardvark");
        assert_eq!(r.get(&id).unwrap().slot, slot_before);
    }

    #[test]
    fn remove_does_not_reuse_slot_of_remaining() {
        let mut r = AppRegistry::new();
        let mut a = rec("a", "A");
        a.slot = r.alloc_slot();
        let id_a = a.app_id.clone();
        r.insert(a);
        let mut b = rec("b", "B");
        b.slot = r.alloc_slot();
        let slot_b = b.slot;
        let id_b = b.app_id.clone();
        r.insert(b);
        assert!(r.remove(&id_a));
        assert_eq!(r.get(&id_b).unwrap().slot, slot_b);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn duplicate_insert_is_rejected() {
        let mut r = AppRegistry::new();
        let mut a = rec("a", "A");
        a.slot = r.alloc_slot();
        assert!(r.insert(a.clone()));
        assert!(!r.insert(a));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn launch_info_snapshots_ownership() {
        let mut r = AppRegistry::new();
        let mut a = rec("a", "App");
        a.slot = r.alloc_slot();
        let id = a.app_id.clone();
        r.insert(a);
        let info = r.launch_info(&id).unwrap();
        // Mutate registry after snapshotting; info must be unaffected.
        r.update(&id, |rec| rec.name = "Changed".to_string());
        assert_eq!(info.name, "App");
    }

    #[test]
    fn icon_state_has_pixels() {
        assert!(!IconState::Missing.has_pixels());
        assert!(!IconState::Loading.has_pixels());
        assert!(IconState::Cached.has_pixels());
        assert!(IconState::Loaded.has_pixels());
        assert!(!IconState::Failed.has_pixels());
        assert!(!IconState::Stale.has_pixels());
    }
}
