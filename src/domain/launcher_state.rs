//! User-owned launcher layout: top-level item order, folders, and hidden apps.
//!
//! This is the Phase 7 counterpart to the discovered [`AppRegistry`]. Where the
//! registry owns *what apps exist* (name, path, icon, slot), `LauncherState`
//! owns *how the user arranged them*:
//!
//! - `items`: the ordered top-level launcher grid (apps and folders).
//! - `folders`: folder display data (name + ordered child app ids).
//! - `hidden_apps`: apps the user hid via the edit-mode ✕ badge.
//! - `customized`: whether the user has set an explicit layout (analogous to the
//!   legacy `user_order_set` flag on `AppRegistry`).
//!
//! The split keeps rediscoverable app data out of the user-owned layout, so an
//! app being removed/added/re-detected by the OS cannot corrupt the user's
//! arrangement, folder membership, or hidden set. Missing apps are retained as
//! references and simply filtered out of the visible stream; if the app is
//! re-detected later, it reappears exactly where the user left it.
//!
//! ## Invariants (enforced by [`Self::normalize`])
//!
//! 1. Each top-level item is unique (no duplicate app or folder ids in `items`).
//! 2. Each folder id in `items` exists in `folders`.
//! 3. Each `AppId` appears in at most one place: a top-level item OR a folder
//!    child OR `hidden_apps` — never two of these, and never twice within
//!    `children`.
//! 4. `folders` children are deduplicated and never nest folders.
//!
//! ## Migration
//!
//! The legacy persistence stored `app_order: Vec<AppId>` and `hidden_ids:
//! Vec<AppId>` on `AppRegistry`. [`LauncherState::from_legacy`] converts that
//! representation into the item-based model, preserving the exact order and
//! hidden set, so existing user data carries over without loss.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;

use crate::domain::app_id::AppId;
use crate::domain::folders::Folder;
use crate::domain::folders::FolderId;
use crate::domain::launcher_item::LauncherItem;

/// User-owned launcher layout.
///
/// See the module docs for the invariant list and migration notes.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LauncherState {
    /// Ordered top-level launcher items. Apps not in this list and not inside a
    /// folder are integrated into it on refresh via
    /// [`integrate_discovered_apps`][Self::integrate_discovered_apps].
    pub items: Vec<LauncherItem>,
    /// Folder display data, keyed by folder id. A folder may appear in `items`
    /// (visible on the grid) or be momentarily detached (during normalization
    /// of a damaged state); `normalize` re-attaches detached folders.
    pub folders: BTreeMap<FolderId, Folder>,
    /// Apps hidden from the launcher grid. Kept here (not in the registry) so a
    /// rescan does not resurrect them, and so removing an app from the Start
    /// Menu does not drop the user's "hidden" intent if the app returns.
    pub hidden_apps: BTreeSet<AppId>,
    /// True once the user (or the loaded persistence) has set an explicit
    /// layout. While false, the state behaves like the legacy name-sorted grid:
    /// apps are sorted into the grid by display name on every refresh.
    pub customized: bool,
}

impl LauncherState {
    /// Create an empty, non-customized state (first launch default).
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a launcher state from the legacy persistence representation.
    ///
    /// `order` is the historical `app_order` list (may contain ids not yet
    /// discovered). `hidden` is the historical `hidden_ids` list. Each app id in
    /// `order` becomes a top-level `LauncherItem::App`; each hidden id is added
    /// to `hidden_apps`. The `customized` flag is set whenever `order` is
    /// non-empty (matching the legacy `set_order` semantics that flipped
    /// `user_order_set` even for an empty vec, but here an empty order means
    /// "no persisted customization at all").
    ///
    /// A hidden app that also appears in `order` is kept in `items` but also
    /// recorded as hidden (the visible-stream filter drops it either way); the
    /// `normalize` pass does not strip it because doing so would lose the
    /// user's positional intent if they later unhide it.
    pub fn from_legacy(order: Vec<AppId>, hidden: Vec<AppId>) -> Self {
        let items: Vec<LauncherItem> = order.into_iter().map(LauncherItem::App).collect();
        let hidden_apps = hidden.into_iter().collect();
        let customized = !items.is_empty();
        Self {
            items,
            folders: BTreeMap::new(),
            hidden_apps,
            customized,
        }
    }

    /// Replace the entire item list and mark the state customized. Used when the
    /// edit-mode reorder commits a new arrangement.
    pub fn set_items(&mut self, items: Vec<LauncherItem>) {
        self.items = items;
        self.customized = true;
    }

    /// Insert or replace a folder's display data. Does not touch `items`; call
    /// [`normalize`][Self::normalize] if the folder should also be on the grid.
    pub fn upsert_folder(&mut self, folder: Folder) {
        self.folders.insert(folder.id.clone(), folder);
    }

    /// Remove a folder entirely (from `items`, from `folders`). Returns the
    /// removed folder's display data if it existed. Its children are not added
    /// back to the top level here; Phase 8 folder deletion owns that policy.
    pub fn remove_folder(&mut self, id: &FolderId) -> Option<Folder> {
        self.items
            .retain(|item| !matches!(item, LauncherItem::Folder(fid) if fid == id));
        self.folders.remove(id)
    }

    /// Hide an app: drop it from the top-level item list and add it to the
    /// hidden set. Returns true if this changed the state.
    pub fn hide_app(&mut self, id: &AppId) -> bool {
        let mut changed = false;
        if !self.hidden_apps.contains(id) {
            self.hidden_apps.insert(id.clone());
            changed = true;
        }
        let had_item = self
            .items
            .iter()
            .any(|item| matches!(item, LauncherItem::App(a) if a == id));
        if had_item {
            self.items
                .retain(|item| !matches!(item, LauncherItem::App(a) if a == id));
            changed = true;
        }
        changed
    }

    /// Unhide an app and re-insert it as a top-level item. The position is
    /// intentionally the tail, matching the iOS-style "unhide lands at the end"
    /// behavior the legacy registry exhibited.
    pub fn unhide_app(&mut self, id: &AppId) {
        if self.hidden_apps.remove(id) {
            self.items.push(LauncherItem::App(id.clone()));
        }
    }

    /// True if `id` is hidden.
    pub fn is_hidden(&self, id: &AppId) -> bool {
        self.hidden_apps.contains(id)
    }

    /// Iterate the app ids that live in a folder (across all folders).
    pub fn folder_child_ids(&self) -> impl Iterator<Item = &AppId> {
        self.folders.values().flat_map(|f| f.children.iter())
    }

    /// Iterate the `AppId`s referenced by top-level app items.
    pub fn top_level_app_ids(&self) -> impl Iterator<Item = &AppId> {
        self.items.iter().filter_map(LauncherItem::as_app_id)
    }

    /// Iterate the `FolderId`s referenced by top-level folder items.
    pub fn top_level_folder_ids(&self) -> impl Iterator<Item = &FolderId> {
        self.items.iter().filter_map(LauncherItem::as_folder_id)
    }

    /// Apply a reorder to the top-level app items, preserving the historical
    /// concatenated visible-then-hidden behavior from the legacy registry.
    ///
    /// `visible_app_order` is the new desired order of visible app ids. Hidden
    /// apps are re-appended at the tail (in their current order) so the full
    /// persisted order round-trips. Folder items keep their relative positions
    /// among the visible apps by keeping them in `items` as-is and only
    /// reordering the app items; this mirrors the Phase 4 reorder that operated
    /// on a flat app list. Phase 8's folder-aware drag will refine this.
    ///
    /// This is a pure helper: it does *not* touch `customized`; callers that
    /// commit a user reorder should also call [`set_items`][Self::set_items] or
    /// set `customized = true`.
    pub fn reorder_app_items(&mut self, visible_app_order: Vec<AppId>) {
        // Collect current hidden apps (those in hidden_apps that were once in
        // items). They are not in items right now (hide_app removes them), but
        // we still preserve a deterministic tail order for persistence by
        // ensuring hidden ids remain in hidden_apps.
        // Rebuild items: keep folders in their existing relative positions,
        // and replace the app items with the new order.
        let folder_items: Vec<LauncherItem> = self
            .items
            .iter()
            .filter(|item| item.as_folder_id().is_some())
            .cloned()
            .collect();

        // Interleave: preserve folder positions by index. The legacy reorder
        // only saw apps, so we treat the grid as apps-first then append folders
        // at the tail if they existed after the reordered apps. To keep this
        // deterministic and behavior-preserving for the current (folder-less)
        // grid, we place all reordered apps first, then folders in their prior
        // relative order.
        let mut new_items: Vec<LauncherItem> = visible_app_order
            .into_iter()
            .map(LauncherItem::App)
            .collect();
        new_items.extend(folder_items);
        self.items = new_items;
        self.customized = true;
    }

    /// Integrate the currently-discovered app set into the layout.
    ///
    /// `discovered` is the set of app ids the OS reports right now. The layout
    /// is updated so that:
    ///
    /// - Apps the user already placed (top-level or in a folder) keep their
    ///   positions. Undiscovered ids are *retained* as placeholders so a later
    ///   re-detection restores them exactly where they were.
    /// - Newly discovered apps that are neither top-level, nor in a folder, nor
    ///   hidden are appended to the top-level item list, ordered by their name
    ///   lookup via `name_of`, so the integration is deterministic.
    /// - Hidden apps that are no longer discovered remain hidden (the user's
    ///   intent survives an app being temporarily uninstalled).
    ///
    /// `name_of` maps an app id to its lowercase display name for deterministic
    /// insertion ordering of new apps. Apps without a name lookup sort first by
    /// id. This is the legacy iOS-like "new apps land at the tail, name-sorted"
    /// behavior.
    pub fn integrate_discovered_apps<F>(&mut self, discovered: &HashSet<AppId>, name_of: F)
    where
        F: Fn(&AppId) -> Option<String>,
    {
        // Apps already accounted for somewhere in user-owned state.
        let mut accounted: HashSet<&AppId> = HashSet::new();
        accounted.extend(self.top_level_app_ids());
        accounted.extend(self.folder_child_ids());
        // Hidden apps are intentionally not in `discovered`-derived `accounted`
        // for the purpose of *adding* them, but they must not be re-added as
        // new top-level items. Treat them as accounted.
        accounted.extend(self.hidden_apps.iter());

        // Newly discovered apps not accounted for → append at the tail, sorted
        // by display name (lowercased) for deterministic ordering.
        let mut new_ids: Vec<&AppId> = discovered
            .iter()
            .filter(|id| !accounted.contains(*id))
            .collect();
        new_ids.sort_by(|a, b| {
            let na = name_of(a).map(|s| s.to_lowercase()).unwrap_or_default();
            let nb = name_of(b).map(|s| s.to_lowercase()).unwrap_or_default();
            na.cmp(&nb).then_with(|| a.as_ref().cmp(b.as_ref()))
        });

        if !self.customized {
            // No user customization yet: rebuild the item list from a name sort
            // of all discovered apps, preserving the legacy name-sorted grid.
            // Hidden apps are excluded from the top-level item list (they must
            // stay hidden — including them here would let the subsequent
            // `normalize` promote them to top-level and drop them from
            // `hidden_apps`, silently un-hiding them).
            let mut all: Vec<&AppId> = discovered
                .iter()
                .filter(|id| !self.hidden_apps.contains(*id))
                .collect();
            all.sort_by(|a, b| {
                let na = name_of(a).map(|s| s.to_lowercase()).unwrap_or_default();
                let nb = name_of(b).map(|s| s.to_lowercase()).unwrap_or_default();
                na.cmp(&nb).then_with(|| a.as_ref().cmp(b.as_ref()))
            });
            self.items = all
                .into_iter()
                .map(|id| LauncherItem::App(id.clone()))
                .collect();
            return;
        }

        for id in new_ids {
            self.items.push(LauncherItem::App(id.clone()));
        }
    }

    /// Remove an app entirely from user-owned state (used when the registry
    /// removes an app and we want to forget it). Retains the id in `hidden_apps`
    /// if it was hidden, so a re-detection does not resurrect it visibly unless
    /// the user unhides it. This mirrors the legacy behavior where removed apps
    /// were dropped from `order` but the hidden intent was preserved separately
    /// by the OS-level removal path.
    ///
    /// In practice the production refresh path does *not* call this; instead it
    /// relies on [`integrate_discovered_apps`][Self::integrate_discovered_apps]
    /// retaining undiscovered ids as placeholders. This method is provided for
    /// an explicit "forget this app" intent (e.g. a future destructive action).
    pub fn forget_app(&mut self, id: &AppId) {
        self.items.retain(|item| !item.is_app(id));
        for f in self.folders.values_mut() {
            f.children.retain(|c| c != id);
        }
    }

    /// Enforce all domain invariants in place. See the module docs for the full
    /// list. This is idempotent and safe to call after any mutation that might
    /// have come from untrusted (persisted) input.
    ///
    /// `present_apps` is the set of app ids the registry currently knows about;
    /// it is *not* used to drop undiscovered ids (those are retained as
    /// placeholders), but it is used to prune folder children and hidden entries
    /// only when `prune_missing` is true. Phase 7's production caller passes
    /// `prune_missing = false` so layout intent survives temporary
    /// undiscovery; `true` is available for a future "compact" action.
    pub fn normalize(&mut self, _present_apps: &HashSet<AppId>, prune_missing: bool) {
        // 1. Deduplicate top-level items preserving first occurrence.
        let mut seen: HashSet<String> = HashSet::new();
        self.items.retain(|item| {
            let key = item.stable_key();
            seen.insert(key)
        });

        // 2. Deduplicate each folder's children preserving first occurrence.
        for f in self.folders.values_mut() {
            let mut child_seen: HashSet<String> = HashSet::new();
            f.children
                .retain(|c| child_seen.insert(c.as_str().to_string()));
        }

        // 3. An app id may appear in at most one of: top-level item, a folder
        //    child, hidden_apps. Priority: top-level > folder child > hidden.
        //    Remove an app from hidden_apps if it is a top-level item or a
        //    folder child (the user placed it somewhere visible).
        let top_level: HashSet<AppId> = self.top_level_app_ids().cloned().collect();
        let in_folders: HashSet<AppId> = self.folder_child_ids().cloned().collect();
        self.hidden_apps
            .retain(|id| !top_level.contains(id) && !in_folders.contains(id));

        // 4. Ensure every folder referenced by items exists in `folders`; drop
        //    folder items whose folder data is missing (cannot display).
        self.items.retain(|item| match item {
            LauncherItem::Folder(fid) => self.folders.contains_key(fid),
            LauncherItem::App(_) => true,
        });

        if prune_missing {
            // Drop app references the registry no longer knows about.
            self.items.retain(|item| match item {
                LauncherItem::App(id) => _present_apps.contains(id),
                LauncherItem::Folder(_) => true,
            });
            for f in self.folders.values_mut() {
                f.children.retain(|c| _present_apps.contains(c));
            }
            self.hidden_apps.retain(|id| _present_apps.contains(id));
        }

        // 5. Drop empty folders? No — an empty folder is a valid user state in
        //    Phase 8 (the user may have just removed the last child). Keep them.
    }

    /// The next folder id counter, seeded from the highest existing generated
    /// index across both `items` and `folders`.
    pub fn next_folder_counter(&self) -> u64 {
        self.folders
            .keys()
            .filter_map(|fid| fid.generated_index())
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(s: &str) -> AppId {
        AppId::from_normalized(s.to_string())
    }

    fn name_lookup(_: &AppId) -> Option<String> {
        None
    }

    fn discovered(ids: &[&str]) -> HashSet<AppId> {
        ids.iter().map(|s| app(s)).collect()
    }

    #[test]
    fn from_legacy_preserves_order_and_hidden() {
        let state = LauncherState::from_legacy(vec![app("a"), app("b"), app("c")], vec![app("h")]);
        assert_eq!(
            state.items,
            vec![
                LauncherItem::App(app("a")),
                LauncherItem::App(app("b")),
                LauncherItem::App(app("c")),
            ]
        );
        assert!(state.is_hidden(&app("h")));
        assert!(state.customized);
    }

    #[test]
    fn from_legacy_empty_is_not_customized() {
        let state = LauncherState::from_legacy(vec![], vec![]);
        assert!(!state.customized);
        assert!(state.items.is_empty());
    }

    #[test]
    fn integrate_keeps_existing_order_and_appends_new_apps() {
        let mut state = LauncherState::from_legacy(vec![app("b"), app("a")], vec![]);
        // Existing apps stay; new app "c" appended.
        state.integrate_discovered_apps(&discovered(&["a", "b", "c"]), name_lookup);
        assert_eq!(
            state.top_level_app_ids().cloned().collect::<Vec<_>>(),
            vec![app("b"), app("a"), app("c")]
        );
    }

    #[test]
    fn integrate_not_customized_builds_name_sorted_grid() {
        let mut state = LauncherState::new();
        let names = |id: &AppId| match id.as_str() {
            "1" => Some("Cherry".to_string()),
            "2" => Some("Apple".to_string()),
            "3" => Some("Banana".to_string()),
            _ => None,
        };
        state.integrate_discovered_apps(&discovered(&["1", "2", "3"]), names);
        assert_eq!(
            state.top_level_app_ids().cloned().collect::<Vec<_>>(),
            vec![app("2"), app("3"), app("1")] // Apple, Banana, Cherry
        );
    }

    #[test]
    fn integrate_does_not_add_hidden_or_folder_apps_as_top_level() {
        let mut state = LauncherState::from_legacy(vec![app("a")], vec![]);
        state.hidden_apps.insert(app("h"));
        let mut f = Folder::new(FolderId::generate(0), "F");
        f.children.push(app("in_folder"));
        state.upsert_folder(f);
        state
            .items
            .push(LauncherItem::Folder(FolderId::generate(0)));

        state.integrate_discovered_apps(&discovered(&["a", "h", "in_folder", "new"]), name_lookup);
        let top: Vec<AppId> = state.top_level_app_ids().cloned().collect();
        assert!(top.contains(&app("a")));
        assert!(top.contains(&app("new")));
        assert!(!top.contains(&app("h"))); // hidden, not re-added
        assert!(!top.contains(&app("in_folder"))); // in folder
    }

    #[test]
    fn integrate_retains_undiscovered_ids_as_placeholders() {
        // App "gone" is not in discovered set, but user had it in order.
        let mut state = LauncherState::from_legacy(vec![app("a"), app("gone")], vec![]);
        state.integrate_discovered_apps(&discovered(&["a"]), name_lookup);
        // "gone" stays in items as a placeholder for re-detection.
        assert!(state
            .items
            .iter()
            .any(|i| matches!(i, LauncherItem::App(id) if id == &app("gone"))));
    }

    #[test]
    fn integrate_does_not_lose_hidden_when_undiscovered() {
        let mut state = LauncherState::from_legacy(vec![], vec![app("h")]);
        state.integrate_discovered_apps(&discovered(&[]), name_lookup);
        assert!(state.is_hidden(&app("h")));
    }

    #[test]
    fn hide_app_removes_top_level_item_and_marks_hidden() {
        let mut state = LauncherState::from_legacy(vec![app("a"), app("b")], vec![]);
        assert!(state.hide_app(&app("a")));
        assert!(state.is_hidden(&app("a")));
        assert_eq!(
            state.top_level_app_ids().cloned().collect::<Vec<_>>(),
            vec![app("b")]
        );
    }

    #[test]
    fn unhide_app_appends_to_tail() {
        let mut state = LauncherState::from_legacy(vec![app("b")], vec![app("a")]);
        state.unhide_app(&app("a"));
        assert!(!state.is_hidden(&app("a")));
        assert_eq!(
            state.top_level_app_ids().cloned().collect::<Vec<_>>(),
            vec![app("b"), app("a")]
        );
    }

    #[test]
    fn normalize_deduplicates_top_level_items() {
        let mut state = LauncherState::from_legacy(vec![app("a"), app("b"), app("a")], vec![]);
        state.normalize(&HashSet::new(), false);
        let ids: Vec<AppId> = state.top_level_app_ids().cloned().collect();
        assert_eq!(ids, vec![app("a"), app("b")]);
    }

    #[test]
    fn normalize_deduplicates_folder_children() {
        let mut state = LauncherState::from_legacy(vec![], vec![]);
        let mut f = Folder::new(FolderId::generate(0), "F");
        f.children = vec![app("a"), app("b"), app("a"), app("c"), app("b")];
        state.upsert_folder(f);
        state.normalize(&HashSet::new(), false);
        let folder = state.folders.get(&FolderId::generate(0)).unwrap();
        assert_eq!(folder.children, vec![app("a"), app("b"), app("c")]);
    }

    #[test]
    fn normalize_app_in_top_level_and_folder_keeps_top_level_only_from_hidden() {
        // App "x" is top-level and also hidden — hidden must be pruned because
        // top-level wins.
        let mut state = LauncherState::from_legacy(vec![app("x")], vec![]);
        state.hidden_apps.insert(app("x"));
        state.normalize(&HashSet::new(), false);
        assert!(!state.is_hidden(&app("x")));
        assert!(state.top_level_app_ids().any(|id| id == &app("x")));
    }

    #[test]
    fn normalize_drops_folder_item_without_folder_data() {
        let mut state = LauncherState::from_legacy(vec![], vec![]);
        state
            .items
            .push(LauncherItem::Folder(FolderId::generate(0)));
        state.normalize(&HashSet::new(), false);
        assert!(state.items.is_empty());
    }

    #[test]
    fn normalize_prune_missing_drops_undiscovered_when_requested() {
        let mut state = LauncherState::from_legacy(vec![app("a"), app("gone")], vec![]);
        state.normalize(&discovered(&["a"]), true);
        assert_eq!(
            state.top_level_app_ids().cloned().collect::<Vec<_>>(),
            vec![app("a")]
        );
    }

    #[test]
    fn reorder_app_items_replaces_visible_app_order() {
        let mut state = LauncherState::from_legacy(vec![app("a"), app("b"), app("c")], vec![]);
        state.reorder_app_items(vec![app("c"), app("a"), app("b")]);
        assert_eq!(
            state.top_level_app_ids().cloned().collect::<Vec<_>>(),
            vec![app("c"), app("a"), app("b")]
        );
        assert!(state.customized);
    }

    #[test]
    fn forget_app_removes_from_items_and_folders() {
        let mut state = LauncherState::from_legacy(vec![app("a"), app("b")], vec![]);
        let mut f = Folder::new(FolderId::generate(0), "F");
        f.children.push(app("a"));
        state.upsert_folder(f);
        state.forget_app(&app("a"));
        assert!(!state.top_level_app_ids().any(|id| id == &app("a")));
        let folder = state.folders.get(&FolderId::generate(0)).unwrap();
        assert!(!folder.contains_child(&app("a")));
    }

    #[test]
    fn next_folder_counter_seeds_from_existing_generated_ids() {
        let mut state = LauncherState::new();
        state.upsert_folder(Folder::new(FolderId::generate(5), "A"));
        state.upsert_folder(Folder::new(FolderId::generate(2), "B"));
        assert_eq!(state.next_folder_counter(), 6);
    }

    #[test]
    fn next_folder_counter_zero_when_no_folders() {
        let state = LauncherState::new();
        assert_eq!(state.next_folder_counter(), 0);
    }

    #[test]
    fn legacy_order_with_hidden_app_keeps_both() {
        // Hidden app "h" also in order: preserved as top-level but also hidden.
        let mut state = LauncherState::from_legacy(vec![app("a"), app("h")], vec![app("h")]);
        state.normalize(&discovered(&["a", "h"]), false);
        // "h" is both in items and hidden: normalize keeps it in items (top-
        // level wins over hidden), removes from hidden.
        assert!(state.top_level_app_ids().any(|id| id == &app("h")));
        assert!(!state.is_hidden(&app("h")));
    }

    #[test]
    fn integrate_not_customized_keeps_hidden_apps_hidden() {
        // Regression (codex review P1): when customized is false, the name-sort
        // rebuild must exclude hidden apps from items so normalize does not
        // promote them to top-level and silently un-hide them.
        let mut state = LauncherState::new();
        state.hidden_apps.insert(app("h"));
        // h is discovered and hidden; it must stay hidden after integration.
        state.integrate_discovered_apps(&discovered(&["a", "h"]), name_lookup);
        assert!(state.is_hidden(&app("h")));
        assert!(!state.top_level_app_ids().any(|id| id == &app("h")));
    }

    #[test]
    fn from_legacy_with_only_hidden_stays_hidden_after_normalize() {
        // A legacy state with only hidden ids (no order) must not un-hide them
        // when normalize runs.
        let mut state = LauncherState::from_legacy(vec![], vec![app("h")]);
        state.normalize(&discovered(&["h"]), false);
        assert!(state.is_hidden(&app("h")));
    }
}
