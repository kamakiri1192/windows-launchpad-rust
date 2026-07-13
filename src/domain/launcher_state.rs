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
//! 3. A hidden app never appears in the visible layout: if an `AppId` is in
//!    `hidden_apps`, `normalize` removes it from `items` (top-level app items)
//!    and from every folder's `children`. The hidden intent wins over a
//!    stale visible placement, so a legacy migration or a reorder that left a
//!    hidden id in `items` cannot silently un-hide it.
//! 4. An app id appears in at most one *visible* place: a top-level app item
//!    OR a folder child, never both. Top-level wins: a folder child that is
//!    also a top-level app item is removed.
//! 5. `folders` children are deduplicated and never nest folders.
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
        let affected_folders: Vec<FolderId> = self
            .folders
            .iter_mut()
            .filter_map(|(folder_id, folder)| {
                let before = folder.children.len();
                folder.children.retain(|child| child != id);
                (folder.children.len() != before).then(|| folder_id.clone())
            })
            .collect();
        for folder_id in affected_folders {
            changed = true;
            self.dissolve_small_folder(&folder_id);
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
        // The visible reorder gives us the new order for *visible* (discovered,
        // non-hidden) app ids. We must also retain:
        //   - folder items (in their existing relative order), and
        //   - undiscovered placeholder app items that are not in the visible
        //     order (so a temporarily-undiscovered app keeps its slot and
        //     rediscovery restores it). These placeholders are not in
        //     `visible_app_order` because `visible_app_ids` skips undiscovered
        //     records.
        let visible_set: HashSet<&AppId> = visible_app_order.iter().collect();

        // App items not in the visible order = undiscovered placeholders. Keep
        // them in their existing relative order so their position is stable.
        let placeholder_apps: Vec<AppId> = self
            .items
            .iter()
            .filter_map(LauncherItem::as_app_id)
            .filter(|id| !visible_set.contains(*id))
            .filter(|id| !self.hidden_apps.contains(*id))
            .cloned()
            .collect();

        let folder_items: Vec<LauncherItem> = self
            .items
            .iter()
            .filter(|item| item.as_folder_id().is_some())
            .cloned()
            .collect();

        // Rebuild: reordered visible apps first, then undiscovered placeholders
        // (stable tail), then folders. This keeps the Phase 7 (folder-less)
        // grid behavior-preserving while not dropping placeholders on reorder.
        let mut new_items: Vec<LauncherItem> = visible_app_order
            .into_iter()
            .map(LauncherItem::App)
            .collect();
        new_items.extend(placeholder_apps.into_iter().map(LauncherItem::App));
        new_items.extend(folder_items);
        self.items = new_items;
        self.customized = true;
    }

    /// Reorder the currently visible top-level items while preserving any
    /// undiscovered placeholder items in their existing slots. The supplied
    /// order must contain exactly the same unique items as `visible_items`.
    pub fn reorder_visible_items(
        &mut self,
        visible_items: &[LauncherItem],
        ordered_items: Vec<LauncherItem>,
    ) -> bool {
        let visible_set: HashSet<LauncherItem> = visible_items.iter().cloned().collect();
        let ordered_set: HashSet<LauncherItem> = ordered_items.iter().cloned().collect();
        if visible_set.len() != visible_items.len()
            || ordered_set.len() != ordered_items.len()
            || visible_set != ordered_set
        {
            return false;
        }

        let mut next = ordered_items.into_iter();
        let mut changed = false;
        for item in &mut self.items {
            if visible_set.contains(item) {
                let replacement = next.next().expect("validated visible item count");
                changed |= *item != replacement;
                *item = replacement;
            }
        }
        if next.next().is_some() {
            return false;
        }
        if changed {
            self.customized = true;
        }
        changed
    }

    /// Create a folder by dropping one top-level app onto another. The target
    /// app's grid position is retained and child order is target then dragged.
    pub fn create_folder_from_apps(
        &mut self,
        target: &AppId,
        dragged: &AppId,
        name: impl Into<String>,
    ) -> Option<FolderId> {
        if target == dragged {
            return None;
        }
        let target_item = LauncherItem::App(target.clone());
        let dragged_item = LauncherItem::App(dragged.clone());
        if !self.items.contains(&target_item) || !self.items.contains(&dragged_item) {
            return None;
        }

        let id = FolderId::generate(self.next_folder_counter());
        let mut folder = Folder::new(id.clone(), name);
        folder.children = vec![target.clone(), dragged.clone()];
        let mut inserted = false;
        let mut items = Vec::with_capacity(self.items.len() - 1);
        for item in self.items.drain(..) {
            if item == target_item {
                items.push(LauncherItem::Folder(id.clone()));
                inserted = true;
            } else if item != dragged_item {
                items.push(item);
            }
        }
        if !inserted {
            return None;
        }
        self.items = items;
        self.folders.insert(id.clone(), folder);
        self.customized = true;
        Some(id)
    }

    /// Move a top-level app into an existing folder, appending it to the child
    /// order. Invalid, duplicate, and self-inconsistent requests are no-ops.
    pub fn move_top_level_app_into_folder(&mut self, app_id: &AppId, folder_id: &FolderId) -> bool {
        let item = LauncherItem::App(app_id.clone());
        if !self.items.contains(&item)
            || self
                .folders
                .get(folder_id)
                .is_none_or(|folder| folder.contains_child(app_id))
        {
            return false;
        }
        self.items.retain(|candidate| candidate != &item);
        self.folders
            .get_mut(folder_id)
            .expect("folder checked above")
            .children
            .push(app_id.clone());
        self.customized = true;
        true
    }

    /// Reorder one child inside its folder. The app id, not its old index, is
    /// the stable identity used for the mutation.
    pub fn reorder_folder_child(
        &mut self,
        folder_id: &FolderId,
        app_id: &AppId,
        insert_index: usize,
    ) -> bool {
        let Some(folder) = self.folders.get_mut(folder_id) else {
            return false;
        };
        let Some(old_index) = folder.children.iter().position(|id| id == app_id) else {
            return false;
        };
        let id = folder.children.remove(old_index);
        let new_index = insert_index.min(folder.children.len());
        folder.children.insert(new_index, id);
        let changed = old_index != new_index;
        self.customized |= changed;
        changed
    }

    /// Move a child between folders and apply the one/zero-child dissolve
    /// policy to the source folder.
    pub fn move_child_between_folders(
        &mut self,
        source: &FolderId,
        destination: &FolderId,
        app_id: &AppId,
        insert_index: usize,
    ) -> bool {
        if source == destination
            || self
                .folders
                .get(destination)
                .is_none_or(|folder| folder.contains_child(app_id))
        {
            return false;
        }
        let Some(source_index) = self
            .folders
            .get(source)
            .and_then(|folder| folder.children.iter().position(|id| id == app_id))
        else {
            return false;
        };
        self.folders
            .get_mut(source)
            .expect("source checked above")
            .children
            .remove(source_index);
        let destination_folder = self
            .folders
            .get_mut(destination)
            .expect("destination checked above");
        destination_folder.children.insert(
            insert_index.min(destination_folder.children.len()),
            app_id.clone(),
        );
        self.dissolve_small_folder(source);
        self.customized = true;
        true
    }

    /// Move a folder child back to the top-level grid, then dissolve its source
    /// folder when zero or one child remains.
    pub fn move_child_to_top_level(
        &mut self,
        folder_id: &FolderId,
        app_id: &AppId,
        insert_index: usize,
    ) -> bool {
        let Some(child_index) = self
            .folders
            .get(folder_id)
            .and_then(|folder| folder.children.iter().position(|id| id == app_id))
        else {
            return false;
        };
        if self.items.iter().any(|item| item.is_app(app_id)) {
            return false;
        }
        self.folders
            .get_mut(folder_id)
            .expect("folder checked above")
            .children
            .remove(child_index);
        self.dissolve_small_folder(folder_id);
        self.items.insert(
            insert_index.min(self.items.len()),
            LauncherItem::App(app_id.clone()),
        );
        self.customized = true;
        true
    }

    /// Apply the Phase 8 dissolve policy. A one-child folder promotes its
    /// remaining app at the folder's exact position; an empty folder vanishes.
    pub fn dissolve_small_folder(&mut self, folder_id: &FolderId) -> bool {
        let Some(folder) = self.folders.get(folder_id) else {
            return false;
        };
        if folder.children.len() > 1 {
            return false;
        }
        let remaining = folder.children.first().cloned();
        if let Some(index) = self
            .items
            .iter()
            .position(|item| item.as_folder_id() == Some(folder_id))
        {
            match remaining.clone() {
                Some(app_id) => self.items[index] = LauncherItem::App(app_id),
                None => {
                    self.items.remove(index);
                }
            }
        } else if let Some(app_id) = remaining {
            // Corrupt/legacy input may contain folder data without the matching
            // top-level item. Do not lose its sole child while normalizing;
            // append it if it is not already represented or hidden.
            if !self.hidden_apps.contains(&app_id)
                && !self.items.iter().any(|item| item.is_app(&app_id))
            {
                self.items.push(LauncherItem::App(app_id));
            }
        }
        self.folders.remove(folder_id);
        self.customized = true;
        true
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

        // 3. Hidden apps must never appear in the visible layout. If a hidden
        //    app id is also in `items` (top-level app item) or a folder child,
        //    the hidden intent wins: remove it from the visible layout and keep
        //    it in `hidden_apps`. This is the opposite of an earlier draft that
        //    let top-level win — that draft silently un-hid apps whenever a
        //    reorder or legacy migration left a hidden id in `items` (the old
        //    hide path kept hidden ids in the persisted order tail, and
        //    `apply_reorder` returns a visible+hidden concatenation that
        //    `reorder_app_items` writes back into `items`).
        self.items.retain(|item| match item {
            LauncherItem::App(id) => !self.hidden_apps.contains(id),
            LauncherItem::Folder(_) => true,
        });
        for f in self.folders.values_mut() {
            f.children.retain(|c| !self.hidden_apps.contains(c));
        }

        // 4. An app id may appear in at most one *visible* place: a top-level
        //    app item OR a folder child (never both). Top-level wins: remove
        //    any folder child that is also a top-level app item. This enforces
        //    the stated invariant and prevents a persisted or future folder
        //    state from rendering the same app as two launcher entries.
        let top_level: HashSet<AppId> = self.top_level_app_ids().cloned().collect();
        for f in self.folders.values_mut() {
            f.children.retain(|c| !top_level.contains(c));
        }

        // 5. Ensure every folder referenced by items exists in `folders`; drop
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

        // 6. Phase 8 folders are never durable containers with fewer than two
        //    children. Promote a sole child at the folder item's exact position
        //    and remove empty folders. This also repairs legacy/corrupt input
        //    after hidden/cross-placement cleanup above reduced membership.
        let small_folders: Vec<FolderId> = self
            .folders
            .iter()
            .filter(|(_, folder)| folder.children.len() <= 1)
            .map(|(id, _)| id.clone())
            .collect();
        for folder_id in small_folders {
            self.dissolve_small_folder(&folder_id);
        }
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
    fn hide_folder_child_removes_it_and_dissolves_pair_folder() {
        let folder_id = FolderId::generate(0);
        let mut state = LauncherState::new();
        let mut folder = Folder::new(folder_id.clone(), "Pair");
        folder.children = vec![app("hidden"), app("remaining")];
        state.upsert_folder(folder);
        state.items = vec![LauncherItem::Folder(folder_id.clone())];
        assert!(state.hide_app(&app("hidden")));
        assert!(state.is_hidden(&app("hidden")));
        assert!(!state.folders.contains_key(&folder_id));
        assert_eq!(state.items, vec![LauncherItem::App(app("remaining"))]);
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
    fn normalize_dissolves_one_child_and_removes_empty_folders() {
        let one_id = FolderId::generate(10);
        let empty_id = FolderId::generate(11);
        let mut one = Folder::new(one_id.clone(), "One");
        one.children.push(app("remaining"));
        let mut state = LauncherState::new();
        state.upsert_folder(one);
        state.upsert_folder(Folder::new(empty_id.clone(), "Empty"));
        state.items = vec![
            LauncherItem::App(app("before")),
            LauncherItem::Folder(one_id.clone()),
            LauncherItem::Folder(empty_id.clone()),
            LauncherItem::App(app("after")),
        ];

        state.normalize(&HashSet::new(), false);

        assert_eq!(
            state.items,
            vec![
                LauncherItem::App(app("before")),
                LauncherItem::App(app("remaining")),
                LauncherItem::App(app("after")),
            ]
        );
        assert!(!state.folders.contains_key(&one_id));
        assert!(!state.folders.contains_key(&empty_id));
    }

    #[test]
    fn normalize_hidden_app_wins_over_top_level_placement() {
        // App "x" is both top-level and hidden. The hidden intent wins: it is
        // removed from the visible layout and kept in hidden_apps. This prevents
        // a legacy migration or reorder that left a hidden id in items from
        // silently un-hiding it.
        let mut state = LauncherState::from_legacy(vec![app("x")], vec![]);
        state.hidden_apps.insert(app("x"));
        state.normalize(&HashSet::new(), false);
        assert!(state.is_hidden(&app("x")));
        assert!(!state.top_level_app_ids().any(|id| id == &app("x")));
    }

    #[test]
    fn normalize_hidden_app_removed_from_folder_children() {
        // A hidden app that is also a folder child is removed from the folder;
        // hidden wins. The remaining one-child folder is then dissolved.
        let mut state = LauncherState::new();
        let fid = FolderId::generate(0);
        let mut f = Folder::new(fid, "F");
        f.children.push(app("visible"));
        f.children.push(app("secret"));
        state.upsert_folder(f);
        state.hidden_apps.insert(app("secret"));
        state.normalize(&HashSet::new(), false);
        assert!(!state.folders.contains_key(&FolderId::generate(0)));
        assert!(state.top_level_app_ids().any(|id| id == &app("visible")));
        assert!(state.is_hidden(&app("secret")));
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
    fn visible_item_reorder_preserves_undiscovered_placeholder_slots() {
        let folder_id = FolderId::generate(0);
        let mut state = LauncherState::new();
        state.items = vec![
            LauncherItem::App(app("a")),
            LauncherItem::App(app("missing")),
            LauncherItem::Folder(folder_id.clone()),
            LauncherItem::App(app("b")),
        ];
        state.upsert_folder(Folder::new(folder_id.clone(), "Work"));
        let visible = vec![
            LauncherItem::App(app("a")),
            LauncherItem::Folder(folder_id.clone()),
            LauncherItem::App(app("b")),
        ];
        assert!(state.reorder_visible_items(
            &visible,
            vec![
                LauncherItem::Folder(folder_id.clone()),
                LauncherItem::App(app("b")),
                LauncherItem::App(app("a")),
            ],
        ));
        assert_eq!(
            state.items,
            vec![
                LauncherItem::Folder(folder_id),
                LauncherItem::App(app("missing")),
                LauncherItem::App(app("b")),
                LauncherItem::App(app("a")),
            ]
        );
    }

    #[test]
    fn app_on_app_creates_folder_at_target_position() {
        let mut state = LauncherState::from_legacy(
            vec![app("before"), app("target"), app("dragged"), app("after")],
            vec![],
        );
        let id = state
            .create_folder_from_apps(&app("target"), &app("dragged"), "Work")
            .unwrap();
        assert_eq!(
            state.items,
            vec![
                LauncherItem::App(app("before")),
                LauncherItem::Folder(id.clone()),
                LauncherItem::App(app("after")),
            ]
        );
        let folder = state.folders.get(&id).unwrap();
        assert_eq!(folder.name, "Work");
        assert_eq!(folder.children, vec![app("target"), app("dragged")]);
    }

    #[test]
    fn self_duplicate_and_invalid_folder_drops_are_noops() {
        let mut state = LauncherState::from_legacy(vec![app("a"), app("b")], vec![]);
        let before = state.clone();
        assert!(state
            .create_folder_from_apps(&app("a"), &app("a"), "Invalid")
            .is_none());
        assert!(
            !state.move_top_level_app_into_folder(&app("a"), &FolderId::from_normalized("missing"))
        );
        assert_eq!(state, before);

        let folder_id = state
            .create_folder_from_apps(&app("a"), &app("b"), "Pair")
            .unwrap();
        assert!(!state.move_top_level_app_into_folder(&app("a"), &folder_id));
        assert!(!state.reorder_folder_child(&folder_id, &app("missing"), 0));
    }

    #[test]
    fn top_level_app_moves_into_existing_folder() {
        let folder_id = FolderId::generate(0);
        let mut state = LauncherState::from_legacy(vec![app("a"), app("b")], vec![]);
        let mut folder = Folder::new(folder_id.clone(), "Tools");
        folder.children.push(app("child"));
        state.upsert_folder(folder);
        state.items.push(LauncherItem::Folder(folder_id.clone()));
        assert!(state.move_top_level_app_into_folder(&app("a"), &folder_id));
        assert!(!state.items.contains(&LauncherItem::App(app("a"))));
        assert_eq!(
            state.folders.get(&folder_id).unwrap().children,
            vec![app("child"), app("a")]
        );
    }

    #[test]
    fn child_reorder_commits_by_stable_app_id() {
        let folder_id = FolderId::generate(0);
        let mut state = LauncherState::new();
        let mut folder = Folder::new(folder_id.clone(), "Tools");
        folder.children = vec![app("a"), app("b"), app("c")];
        state.upsert_folder(folder);
        assert!(state.reorder_folder_child(&folder_id, &app("a"), 2));
        assert_eq!(
            state.folders.get(&folder_id).unwrap().children,
            vec![app("b"), app("c"), app("a")]
        );
    }

    #[test]
    fn moving_between_folders_dissolves_one_child_source_in_place() {
        let source = FolderId::generate(0);
        let destination = FolderId::generate(1);
        let mut state = LauncherState::new();
        let mut source_folder = Folder::new(source.clone(), "Source");
        source_folder.children = vec![app("a"), app("b")];
        let mut destination_folder = Folder::new(destination.clone(), "Destination");
        destination_folder.children = vec![app("c"), app("d")];
        state.upsert_folder(source_folder);
        state.upsert_folder(destination_folder);
        state.items = vec![
            LauncherItem::Folder(source.clone()),
            LauncherItem::App(app("tail")),
            LauncherItem::Folder(destination.clone()),
        ];
        assert!(state.move_child_between_folders(&source, &destination, &app("a"), 1));
        assert!(!state.folders.contains_key(&source));
        assert_eq!(state.items[0], LauncherItem::App(app("b")));
        assert_eq!(
            state.folders.get(&destination).unwrap().children,
            vec![app("c"), app("a"), app("d")]
        );
    }

    #[test]
    fn moving_child_out_promotes_remaining_child_and_inserts_dragged_app() {
        let folder_id = FolderId::generate(0);
        let mut state = LauncherState::new();
        let mut folder = Folder::new(folder_id.clone(), "Pair");
        folder.children = vec![app("a"), app("b")];
        state.upsert_folder(folder);
        state.items = vec![
            LauncherItem::Folder(folder_id.clone()),
            LauncherItem::App(app("tail")),
        ];
        assert!(state.move_child_to_top_level(&folder_id, &app("a"), 2));
        assert!(!state.folders.contains_key(&folder_id));
        assert_eq!(
            state.items,
            vec![
                LauncherItem::App(app("b")),
                LauncherItem::App(app("tail")),
                LauncherItem::App(app("a")),
            ]
        );
    }

    #[test]
    fn legacy_order_with_hidden_app_normalizes_hidden_wins() {
        // Hidden app "h" is also in the legacy order (the old hide path kept
        // hidden ids in the order tail). After normalize, the hidden intent
        // wins: "h" is removed from the visible layout and stays hidden. This
        // is the regression guard for codex review P2-new.
        let mut state = LauncherState::from_legacy(vec![app("a"), app("h")], vec![app("h")]);
        state.normalize(&discovered(&["a", "h"]), false);
        assert!(state.is_hidden(&app("h")));
        assert!(!state.top_level_app_ids().any(|id| id == &app("h")));
        // "a" remains visible.
        assert!(state.top_level_app_ids().any(|id| id == &app("a")));
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

    #[test]
    fn reorder_preserves_undiscovered_placeholders() {
        // Regression (codex review P2-a): a temporarily-undiscovered app kept
        // as a placeholder in items must survive a reorder of visible apps.
        let mut state = LauncherState::from_legacy(vec![app("a"), app("b"), app("gone")], vec![]);
        // Reorder visible apps (gone is not discovered so it is not in the
        // visible order).
        state.reorder_app_items(vec![app("b"), app("a")]);
        // "gone" placeholder retained at the tail.
        assert!(state
            .items
            .iter()
            .any(|i| matches!(i, LauncherItem::App(id) if id == &app("gone"))));
        // Visible order applied.
        let visible: Vec<AppId> = state.top_level_app_ids().cloned().collect();
        assert_eq!(visible[..2], [app("b"), app("a")]);
    }

    #[test]
    fn normalize_removes_cross_placement_duplicates_and_dissolves_folder() {
        // Regression (codex review P2-b): an app that is both a top-level item
        // and a folder child must be removed from the folder child list (top-
        // level wins).
        let mut state = LauncherState::from_legacy(vec![app("a")], vec![]);
        let fid = FolderId::generate(0);
        let mut f = Folder::new(fid, "F");
        f.children = vec![app("a"), app("b")];
        state.upsert_folder(f);
        state.normalize(&discovered(&["a", "b"]), false);
        // "a" is removed from the folder (top-level wins), then the remaining
        // child is promoted when the one-child folder dissolves.
        assert!(!state.folders.contains_key(&FolderId::generate(0)));
        assert!(state.top_level_app_ids().any(|id| id == &app("a")));
        assert!(state.top_level_app_ids().any(|id| id == &app("b")));
    }
}
