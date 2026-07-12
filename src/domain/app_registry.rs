//! App registry — the source of truth for "what apps exist and where is each one".
//!
//! This replaces the old `LoadedIcons { apps, atlas }` combo. Splitting the
//! *app list* from the *icon atlas* is what lets us:
//!   - show the app list + labels before any icon is extracted,
//!   - apply icons incrementally as the worker reports them,
//!   - add / remove / update apps at runtime without repacking the whole atlas.
//!
//! The registry keeps apps in a stable display order and maps each [`AppId`] to
//! a stable **slot** in the icon atlas (see [`IconAtlas`]). Click resolution
//! goes through `app_id`, never a raw positional index, so a rescan that
//! inserts/removes apps can't shift which app a click hits.
//!
//! ## Display order
//! The order is *user-customizable* (iOS Launchpad-style drag-to-reorder). It is
//! stored as an explicit `order: Vec<AppId>` rather than a sort key so it can be
//! rearranged freely. When no user order is set (first launch, or the order list
//! is empty), the registry falls back to sorting by display name. New apps that
//! aren't yet in the order list are appended to its **tail** (last page's end),
//! matching iOS. Apps the user hid via the edit-mode ✕ badge are kept in a
//! separate `hidden` set and excluded from the visible stream.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::domain::app_id::AppId;
use crate::ui_model::geometry::UvRect;

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
/// Ordering is driven by the explicit [`Self::order`] list when it covers the
/// apps (iOS-style drag-to-reorder); otherwise it falls back to display-name
/// sort so the grid is stable across rescans. Apps can be hidden from the
/// visible stream via [`Self::hidden`].
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
    /// User-customized display order, as a sequence of `AppId`s. When
    /// [`Self::user_order_set`] is true this list drives `apps` ordering; ids
    /// present in the registry but missing from this list are appended at the
    /// tail. Stored separately so the user's arrangement survives rescans and is
    /// directly persistable.
    order: Vec<AppId>,
    /// True once the user (or the persisted state on load) has explicitly set an
    /// order via [`Self::set_order`]. While false, `order` is rebuilt from a
    /// display-name sort on every structural change, so the registry behaves
    /// exactly like the legacy name-sorted registry until customization begins.
    user_order_set: bool,
    /// Apps the user hid via the edit-mode ✕ badge. Kept in the registry (so a
    /// rescan doesn't resurrect them) but excluded from the visible stream.
    hidden: HashSet<AppId>,
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
        self.order.retain(|ordered_id| ordered_id != id);
        self.hidden.remove(id);
        self.reindex_and_sort();
        true
    }

    /// Rebuild `by_id` and re-sort `apps` into display order. Called after any
    /// structural change so iteration order stays predictable.
    ///
    /// Order resolution:
    ///   - If [`Self::order`] covers all current apps, the apps are arranged to
    ///     match it (user's drag-to-reorder result).
    ///   - Apps missing from `order` (new installs, or order never set) are
    ///     appended to the **tail of `order`** so subsequent rescans keep them
    ///     stable, then sorted among themselves by display name (so a fresh
    ///     install while the user has a custom layout lands at the very end in a
    ///     predictable spot — iOS-like).
    ///   - When `order` is empty (first launch) everything sorts by display name
    ///     and is seeded into `order` in that sequence.
    fn reindex_and_sort(&mut self) {
        // 1. Make `order` cover exactly the current app set. When no user order
        //    is set yet, this rebuilds `order` entirely from a name sort (so the
        //    registry behaves like the legacy name-sorted one). Once a user
        //    order exists, it preserves arrangement and appends new ids.
        self.reconcile_order();

        // 2. Sort `apps` to follow `order`. Stable sort keeps the name-based
        //    fallback deterministic for ids absent from `order`. Clone the
        //    order list so the borrow checker lets us mutate `apps` in place.
        let order = self.order.clone();
        self.apps.sort_by_key(|a| order_rank(&order, &a.app_id));
        debug_assert!(
            self.apps.iter().all(|a| self.order.contains(&a.app_id)),
            "every app must have an order entry after reconcile"
        );

        // 3. Rebuild the id→index map for the new positions.
        self.by_id.clear();
        self.by_id.reserve(self.apps.len());
        for (i, a) in self.apps.iter().enumerate() {
            self.by_id.insert(a.app_id.clone(), i);
        }
    }

    /// Make [`Self::order`] cover exactly the current app set.
    ///
    /// - When [`Self::user_order_set`] is false (no customization yet): rebuild
    ///   `order` from scratch as the apps sorted by display name. This keeps the
    ///   registry identical to the legacy name-sorted behavior.
    /// - When true (user has a custom layout): drop gone ids from `order`, then
    ///   append new ids at the tail in display-name order (iOS-like: fresh
    ///   installs land at the end in a predictable spot, without disturbing the
    ///   user's arrangement).
    fn reconcile_order(&mut self) {
        if !self.user_order_set {
            // No user customization: `order` mirrors a fresh name sort. Build it
            // directly from `apps` rather than trusting `by_id` (on a first
            // insert it still points at push order, and a name change mid-
            // `update` can leave it momentarily stale).
            let mut named: Vec<(AppId, String)> = self
                .apps
                .iter()
                .map(|a| (a.app_id.clone(), a.name.to_lowercase()))
                .collect();
            named.sort_by(|a, b| a.1.cmp(&b.1));
            self.order = named.into_iter().map(|(id, _)| id).collect();
            return;
        }

        // Snapshot the display names so we can sort new ids without borrowing
        // `self.apps` while we also extend `self.order`.
        let name_of: HashMap<AppId, String> = self
            .apps
            .iter()
            .map(|a| (a.app_id.clone(), a.name.to_lowercase()))
            .collect();

        // Append new ids in display-name order.
        let mut new_ids: Vec<AppId> = self
            .apps
            .iter()
            .map(|a| a.app_id.clone())
            .filter(|id| !self.order.contains(id))
            .collect();
        new_ids.sort_by(|a, b| {
            name_of
                .get(a)
                .cloned()
                .unwrap_or_default()
                .cmp(&name_of.get(b).cloned().unwrap_or_default())
        });
        self.order.extend(new_ids);
    }

    /// Allocate the next slot index. Used by callers building an `AppRecord`.
    pub fn alloc_slot(&mut self) -> u32 {
        let s = self.next_slot;
        self.next_slot += 1;
        s
    }

    /// Reset the registry entirely (used on full reload / corrupt state). Keeps
    /// the user's `order` and `hidden` lists intact so a reload doesn't wipe
    /// their customization — use [`Self::reset_customization`] for a full wipe.
    pub fn clear(&mut self) {
        self.apps.clear();
        self.by_id.clear();
        self.next_slot = 0;
    }

    /// Replace the user-customized display order. Pass an empty vec to fall
    /// back to display-name sort (this still marks a user order as *set*, so a
    /// later rescan keeps the name-derived arrangement rather than re-sorting
    /// on every insert). The order is reconciled against the current app set on
    /// the next structural change; ids that no longer exist are dropped, missing
    /// ids are appended.
    pub fn set_order(&mut self, order: Vec<AppId>) {
        self.order = order;
        self.user_order_set = true;
        self.reindex_and_sort();
    }

    /// Return the current display order as a stable sequence of `AppId`s (the
    /// source of truth for persistence). Reflects any drag-to-reorder changes
    /// once [`Self::set_order`] is called.
    pub fn order(&self) -> &[AppId] {
        &self.order
    }

    /// Replace the hidden-app set. Hidden apps stay in the registry (a rescan
    /// doesn't resurrect them) but are excluded from the visible stream.
    pub fn set_hidden(&mut self, ids: Vec<AppId>) {
        self.hidden = ids.into_iter().collect();
    }

    /// All currently hidden app ids (for persistence).
    pub fn hidden(&self) -> &HashSet<AppId> {
        &self.hidden
    }

    /// Hide an app from the visible stream. No-op if already hidden.
    pub fn hide(&mut self, id: &AppId) {
        self.hidden.insert(id.clone());
    }

    /// Reveal a previously hidden app.
    pub fn unhide(&mut self, id: &AppId) {
        self.hidden.remove(id);
    }

    /// True if `id` is currently hidden from the grid.
    pub fn is_hidden(&self, id: &AppId) -> bool {
        self.hidden.contains(id)
    }

    /// Wipe user customization (order + hidden) — used when resetting state.
    pub fn reset_customization(&mut self) {
        self.order.clear();
        self.hidden.clear();
        self.user_order_set = false;
        self.reindex_and_sort();
    }

    /// Snapshot lookup by id for click resolution.
    pub fn launch_info(&self, id: &AppId) -> Option<AppLaunchInfo> {
        self.get(id).map(AppLaunchInfo::from)
    }
}

/// Position of `id` within `order`, or `usize::MAX` if absent so unknown ids
/// sort last. A free function (not a method) so it can be called from a
/// `sort_by_key` closure without holding a borrow on the registry.
fn order_rank(order: &[AppId], id: &AppId) -> usize {
    order.iter().position(|x| x == id).unwrap_or(usize::MAX)
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

    // ---- user-customized display order ----

    fn insert_named(r: &mut AppRegistry, id: &str, name: &str) {
        let mut a = rec(id, name);
        a.slot = r.alloc_slot();
        r.insert(a);
    }

    fn names(r: &AppRegistry) -> Vec<String> {
        r.apps().iter().map(|a| a.name.clone()).collect()
    }

    #[test]
    fn user_order_overrides_name_sort() {
        let mut r = AppRegistry::new();
        insert_named(&mut r, "a", "Apple");
        insert_named(&mut r, "b", "Banana");
        insert_named(&mut r, "c", "Cherry");
        // Default is name sort.
        assert_eq!(names(&r), vec!["Apple", "Banana", "Cherry"]);

        // User reverses the order.
        r.set_order(vec![
            AppId::from_normalized("c".to_string()),
            AppId::from_normalized("b".to_string()),
            AppId::from_normalized("a".to_string()),
        ]);
        assert_eq!(names(&r), vec!["Cherry", "Banana", "Apple"]);
        // order() reflects the user arrangement.
        assert_eq!(
            r.order().iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
            vec!["c", "b", "a"]
        );
    }

    #[test]
    fn new_app_appends_to_end_after_user_order() {
        let mut r = AppRegistry::new();
        insert_named(&mut r, "a", "Apple");
        insert_named(&mut r, "b", "Banana");
        r.set_order(vec![
            AppId::from_normalized("b".to_string()),
            AppId::from_normalized("a".to_string()),
        ]);
        // A brand-new app lands at the tail, after the existing layout.
        insert_named(&mut r, "c", "Cherry");
        assert_eq!(names(&r), vec!["Banana", "Apple", "Cherry"]);

        // A second new app also lands at the tail, name-sorted relative to other
        // new apps (here just "Date").
        insert_named(&mut r, "d", "Date");
        assert_eq!(names(&r), vec!["Banana", "Apple", "Cherry", "Date"]);
    }

    #[test]
    fn empty_set_order_keeps_name_sort_but_marks_set() {
        // Passing an empty order is allowed (e.g. persisted state was empty)
        // and must still produce a stable name-sorted arrangement.
        let mut r = AppRegistry::new();
        insert_named(&mut r, "b", "Banana");
        insert_named(&mut r, "a", "Apple");
        r.set_order(vec![]);
        assert_eq!(names(&r), vec!["Apple", "Banana"]);
    }

    #[test]
    fn removed_app_dropped_from_user_order() {
        let mut r = AppRegistry::new();
        insert_named(&mut r, "a", "Apple");
        insert_named(&mut r, "b", "Banana");
        insert_named(&mut r, "c", "Cherry");
        r.set_order(vec![
            AppId::from_normalized("c".to_string()),
            AppId::from_normalized("a".to_string()),
            AppId::from_normalized("b".to_string()),
        ]);
        assert!(r.remove(&AppId::from_normalized("b".to_string())));
        assert_eq!(names(&r), vec!["Cherry", "Apple"]);
        assert_eq!(
            r.order().iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
            vec!["c", "a"]
        );
    }

    #[test]
    fn removed_app_dropped_from_hidden_set() {
        let mut r = AppRegistry::new();
        insert_named(&mut r, "a", "Apple");
        let id = AppId::from_normalized("a".to_string());
        r.hide(&id);
        assert!(r.remove(&id));
        assert!(!r.is_hidden(&id));
    }

    #[test]
    fn rename_keeps_position_in_user_order() {
        // Renaming an app must NOT re-sort by name when a user order is active.
        let mut r = AppRegistry::new();
        insert_named(&mut r, "a", "Apple");
        insert_named(&mut r, "b", "Banana");
        r.set_order(vec![
            AppId::from_normalized("b".to_string()),
            AppId::from_normalized("a".to_string()),
        ]);
        r.update(&AppId::from_normalized("a".to_string()), |rec| {
            rec.name = "Zucchini".to_string();
        });
        // Position preserved: Banana still first.
        assert_eq!(names(&r), vec!["Banana", "Zucchini"]);
    }

    #[test]
    fn set_order_preserves_ids_not_yet_inserted() {
        // Persisted order is loaded before the first scan is ingested, so it
        // necessarily references ids not yet inserted. Those ids must be kept
        // pending and applied as matching apps arrive.
        let mut r = AppRegistry::new();
        r.set_order(vec![
            AppId::from_normalized("b".to_string()),
            AppId::from_normalized("a".to_string()),
        ]);
        assert_eq!(
            r.order().iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        insert_named(&mut r, "a", "Apple");
        insert_named(&mut r, "b", "Banana");
        assert_eq!(names(&r), vec!["Banana", "Apple"]);
        assert_eq!(
            r.order().iter().map(|i| i.as_ref()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );
    }

    // ---- hidden apps ----

    #[test]
    fn hide_and_unhide_toggle_visibility_flag() {
        let mut r = AppRegistry::new();
        insert_named(&mut r, "a", "Apple");
        let id = AppId::from_normalized("a".to_string());
        assert!(!r.is_hidden(&id));
        r.hide(&id);
        assert!(r.is_hidden(&id));
        assert!(r.hidden().contains(&id));
        r.unhide(&id);
        assert!(!r.is_hidden(&id));
    }

    #[test]
    fn reset_customization_clears_order_and_hidden() {
        let mut r = AppRegistry::new();
        insert_named(&mut r, "b", "Banana");
        insert_named(&mut r, "a", "Apple");
        r.set_order(vec![AppId::from_normalized("b".to_string())]);
        r.hide(&AppId::from_normalized("a".to_string()));
        r.reset_customization();
        // Back to name sort, nothing hidden, no user order.
        assert_eq!(names(&r), vec!["Apple", "Banana"]);
        assert!(r.hidden().is_empty());
        // Re-insert preserves name sort (user_order_set is false again).
        insert_named(&mut r, "c", "Cherry");
        assert_eq!(names(&r), vec!["Apple", "Banana", "Cherry"]);
    }
}
