//! Phase 7 integration tests for the item-based launcher domain.
//!
//! These tests exercise the full discovery ↔ user-layout ↔ launch resolution
//! data flow using only the library-public domain surface (`AppRegistry` +
//! `LauncherState`). They cover the required Phase 7 scenarios:
//!
//! - app item / folder order persists and restores,
//! - stable AppId / FolderId references survive normalization,
//! - duplicate app items and folder children are normalized,
//! - newly discovered apps integrate deterministically without disturbing the
//!   user order,
//! - app removal / temporary undiscovery / rediscovery preserve the user's
//!   order and folder membership,
//! - a removed/undiscovered AppId does not resolve to a launch target,
//! - an app item resolves through stable AppId to the right discovered app,
//! - folder child order persists,
//! - hidden-app behavior carries over to the new model,
//! - legacy persistence migrates to the new model,
//! - corrupt persistence is handled safely.

use std::collections::HashSet;

use launchpad_windows::domain::app_id::AppId;
use launchpad_windows::domain::app_registry::AppRegistry;
use launchpad_windows::domain::folders::Folder;
use launchpad_windows::domain::folders::FolderId;
use launchpad_windows::domain::launcher_item::LauncherItem;
use launchpad_windows::domain::launcher_state::LauncherState;

fn app(s: &str) -> AppId {
    AppId::from_normalized(s.to_string())
}

fn discovered(ids: &[&str]) -> HashSet<AppId> {
    ids.iter().map(|s| app(s)).collect()
}

fn name_lookup(reg: &AppRegistry) -> impl Fn(&AppId) -> Option<String> + '_ {
    |id: &AppId| reg.lowercased_name_of(id)
}

/// Insert a record into `reg` with the given id and display name.
fn insert_app(reg: &mut AppRegistry, id: &str, name: &str) {
    use launchpad_windows::domain::app_registry::AppRecord;
    use launchpad_windows::domain::app_registry::IconState;
    use std::path::PathBuf;
    let mut r = AppRecord {
        app_id: app(id),
        name: name.to_string(),
        link_path: PathBuf::from(format!("C:\\{id}.lnk")),
        resolved_target: PathBuf::new(),
        slot: 0,
        icon_state: IconState::Missing,
        uv: None,
    };
    r.slot = reg.alloc_slot();
    reg.insert(r);
}

// ---- required test: app/folder order saves and restores ----

#[test]
fn launcher_item_order_round_trips_through_serde() {
    let mut state = LauncherState::new();
    state.set_items(vec![
        LauncherItem::App(app("a")),
        LauncherItem::Folder(FolderId::generate(0)),
        LauncherItem::App(app("b")),
    ]);
    state.upsert_folder(Folder::new(FolderId::generate(0), "Games"));

    let json = serde_json::to_string(&state).expect("serialize");
    let restored: LauncherState = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(restored.items, state.items);
    assert_eq!(restored.folders.len(), 1);
    assert!(restored.customized);
}

// ---- required test: stable FolderId and AppId references ----

#[test]
fn folder_and_app_references_are_stable_across_normalize() {
    let mut state = LauncherState::new();
    let fid = FolderId::generate(3);
    state.upsert_folder(Folder::new(fid.clone(), "Tools"));
    state.set_items(vec![
        LauncherItem::App(app("x")),
        LauncherItem::Folder(fid.clone()),
    ]);
    state.normalize(&HashSet::new(), false);
    assert!(state.items.contains(&LauncherItem::Folder(fid.clone())));
    assert!(state.folders.contains_key(&fid));
    assert!(state.items.contains(&LauncherItem::App(app("x"))));
}

// ---- required test: duplicate app items / folder children normalized ----

#[test]
fn duplicate_app_items_are_deduplicated() {
    let mut state = LauncherState::from_legacy(vec![app("a"), app("a"), app("b")], vec![]);
    state.normalize(&HashSet::new(), false);
    let count_a = state
        .items
        .iter()
        .filter(|i| matches!(i, LauncherItem::App(id) if id == &app("a")))
        .count();
    assert_eq!(count_a, 1);
}

#[test]
fn duplicate_folder_children_are_deduplicated() {
    let mut state = LauncherState::new();
    let fid = FolderId::generate(0);
    let mut f = Folder::new(fid, "F");
    f.children = vec![app("a"), app("b"), app("a")];
    state.upsert_folder(f);
    state.normalize(&HashSet::new(), false);
    let folder = state.folders.get(&FolderId::generate(0)).unwrap();
    assert_eq!(folder.children, vec![app("a"), app("b")]);
}

// ---- required test: new apps integrated deterministically ----

#[test]
fn new_apps_append_in_name_order_without_disturbing_layout() {
    let mut reg = AppRegistry::new();
    insert_app(&mut reg, "b", "Banana");
    insert_app(&mut reg, "a", "Apple");
    let mut state = LauncherState::from_legacy(vec![app("b"), app("a")], vec![]);

    // User reversed the order.
    state.reorder_app_items(vec![app("a"), app("b")]);

    // Discover a new app.
    insert_app(&mut reg, "c", "Cherry");
    state.integrate_discovered_apps(&reg.discovered_id_set(), name_lookup(&reg));

    assert_eq!(
        state.top_level_app_ids().cloned().collect::<Vec<_>>(),
        vec![app("a"), app("b"), app("c")]
    );
}

// ---- required test: removal / undiscovery / rediscovery preserves order ----

#[test]
fn temporary_undiscovery_preserves_order_and_rediscovery_restores() {
    let mut reg = AppRegistry::new();
    insert_app(&mut reg, "a", "Apple");
    insert_app(&mut reg, "b", "Banana");
    insert_app(&mut reg, "c", "Cherry");
    let mut state = LauncherState::new();
    state.integrate_discovered_apps(&reg.discovered_id_set(), name_lookup(&reg));
    // User order: c, a, b
    state.reorder_app_items(vec![app("c"), app("a"), app("b")]);

    // App "a" temporarily undiscovered (uninstalled).
    reg.remove(&app("a"));
    state.integrate_discovered_apps(&reg.discovered_id_set(), name_lookup(&reg));
    // "a" retained as a placeholder.
    assert!(state
        .items
        .iter()
        .any(|i| matches!(i, LauncherItem::App(id) if id == &app("a"))));

    // Rediscover "a"; order unchanged.
    insert_app(&mut reg, "a", "Apple");
    state.integrate_discovered_apps(&reg.discovered_id_set(), name_lookup(&reg));
    assert_eq!(
        state.top_level_app_ids().cloned().collect::<Vec<_>>(),
        vec![app("c"), app("a"), app("b")]
    );
}

// ---- required test: removed/undiscovered AppId does not launch ----

#[test]
fn undiscovered_app_id_does_not_resolve_to_launch_info() {
    let mut reg = AppRegistry::new();
    insert_app(&mut reg, "a", "Apple");
    let mut state = LauncherState::new();
    state.integrate_discovered_apps(&reg.discovered_id_set(), name_lookup(&reg));

    // App "b" is in the layout but not discovered.
    state.items.push(LauncherItem::App(app("b")));
    assert!(reg.launch_info(&app("b")).is_none());
    assert!(reg.launch_info(&app("a")).is_some());
}

// ---- required test: app item resolves through stable AppId ----

#[test]
fn app_item_resolves_to_correct_discovered_app() {
    let mut reg = AppRegistry::new();
    insert_app(&mut reg, "a", "Apple");
    insert_app(&mut reg, "b", "Banana");
    let mut state = LauncherState::new();
    state.integrate_discovered_apps(&reg.discovered_id_set(), name_lookup(&reg));

    // Each top-level app item resolves to the right record.
    for item in &state.items {
        if let LauncherItem::App(id) = item {
            let info = reg.launch_info(id).expect("app resolves");
            let rec = reg.get(id).expect("record exists");
            assert_eq!(info.name, rec.name);
        }
    }
}

// ---- required test: folder child order persists ----

#[test]
fn folder_child_order_persists_through_serde() {
    let mut state = LauncherState::new();
    let fid = FolderId::generate(0);
    let mut f = Folder::new(fid.clone(), "Games");
    f.children = vec![app("z"), app("a"), app("m")];
    state.upsert_folder(f);
    state.items.push(LauncherItem::Folder(fid.clone()));

    let json = serde_json::to_string(&state).expect("serialize");
    let restored: LauncherState = serde_json::from_str(&json).expect("deserialize");
    let folder = restored.folders.get(&fid).unwrap();
    assert_eq!(folder.children, vec![app("z"), app("a"), app("m")]);
}

// ---- required test: hidden app behavior carries over ----

#[test]
fn hidden_apps_excluded_from_top_level_and_persisted() {
    let mut state = LauncherState::from_legacy(vec![app("a"), app("b")], vec![app("h")]);
    state.normalize(&discovered(&["a", "b", "h"]), false);
    // Hidden app is not in top-level items.
    assert!(!state.top_level_app_ids().any(|id| id == &app("h")));
    assert!(state.is_hidden(&app("h")));

    let json = serde_json::to_string(&state).expect("serialize");
    let restored: LauncherState = serde_json::from_str(&json).expect("deserialize");
    assert!(restored.is_hidden(&app("h")));
}

// ---- required test: legacy migration ----

#[test]
fn legacy_order_and_hidden_migrate_to_launcher_state() {
    let state = LauncherState::from_legacy(
        vec![app("x"), app("y"), app("z")],
        vec![app("hidden1"), app("hidden2")],
    );
    assert_eq!(
        state.top_level_app_ids().cloned().collect::<Vec<_>>(),
        vec![app("x"), app("y"), app("z")]
    );
    assert!(state.is_hidden(&app("hidden1")));
    assert!(state.is_hidden(&app("hidden2")));
    assert!(state.customized);
}

#[test]
fn legacy_empty_order_is_not_customized() {
    let state = LauncherState::from_legacy(vec![], vec![]);
    assert!(!state.customized);
}

// ---- required test: corrupt persistence handled safely ----

#[test]
fn corrupt_launcher_state_json_falls_back() {
    // Simulate what IconCache::get_launcher_state does: serde_json::from_slice
    // on garbage returns Err, which the cache maps to None.
    let garbage = b"{not valid json";
    let result: Result<LauncherState, _> = serde_json::from_slice(garbage);
    assert!(result.is_err());
    // The caller treats None as "no stored state" → empty default.
    let fallback = LauncherState::new();
    assert!(!fallback.customized);
    assert!(fallback.items.is_empty());
}

#[test]
fn truncated_launcher_state_json_falls_back() {
    // A valid-looking but truncated blob.
    let truncated = b"{\"items\":[{\"App\":\"a";
    let result: Result<LauncherState, _> = serde_json::from_slice(truncated);
    assert!(result.is_err());
}

// ---- required test: hide does not break unrelated order ----

#[test]
fn hide_app_preserves_remaining_order() {
    let mut state = LauncherState::from_legacy(vec![app("a"), app("b"), app("c")], vec![]);
    state.hide_app(&app("b"));
    assert!(state.is_hidden(&app("b")));
    assert_eq!(
        state.top_level_app_ids().cloned().collect::<Vec<_>>(),
        vec![app("a"), app("c")]
    );
    // Unhide lands at the tail.
    state.unhide_app(&app("b"));
    assert_eq!(
        state.top_level_app_ids().cloned().collect::<Vec<_>>(),
        vec![app("a"), app("c"), app("b")]
    );
}

// ---- required test: hidden intent wins over stale top-level placement ----

#[test]
fn hidden_app_in_top_level_stays_hidden_after_normalize() {
    // A hidden app that is also in the top-level item list (simulating a
    // damaged persisted state or a legacy migration that kept hidden ids in the
    // order tail) must stay hidden after normalize. The hidden intent wins over
    // the visible placement.
    let mut state = LauncherState::from_legacy(vec![app("a")], vec![]);
    state.hidden_apps.insert(app("a"));
    state.normalize(&discovered(&["a"]), false);
    assert!(state.is_hidden(&app("a")));
    assert!(!state.top_level_app_ids().any(|id| id == &app("a")));
}

// ---- required test: folder item without folder data is dropped ----

#[test]
fn orphan_folder_item_is_dropped_on_normalize() {
    let mut state = LauncherState::new();
    state
        .items
        .push(LauncherItem::Folder(FolderId::generate(99)));
    state.normalize(&HashSet::new(), false);
    assert!(state.items.is_empty());
}

// ---- required test: integrate is idempotent ----

#[test]
fn integrate_discovered_apps_is_idempotent() {
    let mut reg = AppRegistry::new();
    insert_app(&mut reg, "a", "Apple");
    insert_app(&mut reg, "b", "Banana");
    let mut state = LauncherState::new();
    state.integrate_discovered_apps(&reg.discovered_id_set(), name_lookup(&reg));
    let after_first = state.items.clone();
    state.integrate_discovered_apps(&reg.discovered_id_set(), name_lookup(&reg));
    assert_eq!(state.items, after_first);
}

// ---- required test: next folder counter ----

#[test]
fn next_folder_counter_is_deterministic() {
    let mut state = LauncherState::new();
    assert_eq!(state.next_folder_counter(), 0);
    state.upsert_folder(Folder::new(FolderId::generate(0), "A"));
    state.upsert_folder(Folder::new(FolderId::generate(5), "B"));
    assert_eq!(state.next_folder_counter(), 6);
}

// ---- required test: architecture — domain does not depend on renderer/winit/wgpu ----
// (covered by tests/architecture_boundaries.rs, asserted here too for the new
// modules by compiling this integration test which links only the library.)

#[test]
fn domain_compiles_without_binary_deps() {
    // If this compiles, LauncherItem/Folder/LauncherState/AppRegistry are all
    // reachable from the library surface without wgpu/winit.
    let _ = AppId::from_normalized("x".to_string());
    let _ = FolderId::generate(0);
    let _ = LauncherItem::app(AppId::from_normalized("x".to_string()));
    let _ = LauncherState::new();
    let _ = AppRegistry::new();
}
