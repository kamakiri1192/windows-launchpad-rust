//! Persistence tests included inside icon_cache, exercising real SQLite transactions.
use super::*;
use crate::domain::app_diff::SnapshotEntry;
use std::collections::BTreeMap;

fn cache() -> IconCache {
    open_at(Path::new(":memory:")).unwrap()
}
fn snapshot(count: usize) -> BTreeMap<AppId, SnapshotEntry> {
    (0..count)
        .map(|n| {
            let app_id = AppId::from_normalized(format!("steam:{n}"));
            (
                app_id.clone(),
                SnapshotEntry {
                    app_id,
                    name: format!("App {n}"),
                    link_path: format!("steam://rungameid/{n}"),
                    link_mtime: 0,
                    target_path: format!("appmanifest_{n}.acf"),
                    launch_arguments: None,
                    provider_icon: None,
                    target_mtime: 0,
                    icon_location: String::new(),
                    icon_index: 0,
                    version: String::new(),
                    publisher: String::new(),
                    identifier: String::new(),
                },
            )
        })
        .collect()
}
fn prepare(c: &IconCache, count: usize, complete: bool) -> LayoutLoad {
    c.prepare_initial_layout(&snapshot(count), complete, Platform::Windows, false)
}

#[test]
fn initial_commit_round_trips_undo_without_regenerating_layout() {
    let c = cache();
    assert!(c.load_layout().fresh);
    let first = prepare(&c, 2, true);
    assert_eq!(first.state.folders.len(), 1);
    let restored = prepare(&c, 2, true);
    assert_eq!(first.state, restored.state);
    assert!(restored.state.initial_undo_available());
    let mut undone = restored.state;
    assert!(undone.undo_initial_layout());
    c.put_launcher_state(&undone).unwrap();
    assert_eq!(prepare(&c, 2, true).state, undone);
}

#[test]
fn order_without_folders_survives_restart_and_undo_is_not_reapplied() {
    let c = cache();
    let mut apps: BTreeMap<_, _> = snapshot(2)
        .into_values()
        .enumerate()
        .map(|(index, mut entry)| {
            entry.app_id = AppId::from_normalized(format!("app:{index}"));
            (entry.app_id.clone(), entry)
        })
        .collect();
    let settings_id = AppId::from_normalized("app:1");
    let unknown_id = AppId::from_normalized("app:0");
    for entry in apps.values_mut() {
        entry.link_path = format!("C:/{}.lnk", entry.name);
        entry.target_path = "C:/Unknown.exe".into();
        entry.launch_arguments = Some(String::new());
    }
    let entry = apps.get_mut(&settings_id).unwrap();
    entry.name = "設定".into();
    entry.link_path = "shell:AppsFolder\\windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel".into();
    entry.target_path.clear();
    let first = c
        .prepare_initial_layout(&apps, true, Platform::Windows, false)
        .state;
    assert!(first.folders.is_empty());
    assert_eq!(
        first.top_level_app_ids().cloned().collect::<Vec<_>>(),
        vec![settings_id.clone(), unknown_id.clone()]
    );
    let restored = c
        .prepare_initial_layout(&apps, true, Platform::Windows, false)
        .state;
    assert_eq!(restored, first);
    assert!(restored.initial_undo_available());
    let mut undone = restored;
    assert!(undone.undo_initial_layout());
    assert_eq!(
        undone.top_level_app_ids().cloned().collect::<Vec<_>>(),
        vec![unknown_id, settings_id]
    );
    c.put_launcher_state(&undone).unwrap();
    assert_eq!(
        c.prepare_initial_layout(&apps, true, Platform::Windows, false)
            .state,
        undone
    );
}

#[test]
fn explicit_reset_groups_current_apps_after_incomplete_scan_and_restores_on_restart() {
    let c = cache();
    let previous = prepare(&c, 2, false).state;
    assert_eq!(previous.folders.len(), 1);
    let state = initial_layout::reset(
        &snapshot(2),
        Platform::Windows,
        false,
        previous.layout_revision,
        &Catalog::bundled(None),
    );
    c.put_settings_and_layout(&Settings::default(), &state)
        .unwrap();
    let restored = prepare(&c, 2, false).state;
    assert_eq!(restored, state);
    assert_eq!(restored.folders.len(), 1);
    assert!(restored.initial_undo_available());
}

#[test]
fn settings_reset_save_failure_preserves_both_settings_and_layout() {
    let c = cache();
    let previous = prepare(&c, 2, true).state;
    let old_settings = Settings {
        show_steam_apps: false,
        ..Settings::default()
    };
    c.put_settings(&old_settings).unwrap();
    let settings_bytes = c.kv_get(SETTINGS_KEY).unwrap();
    c.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_reset BEFORE INSERT ON kv WHEN NEW.key='launcher_state' BEGIN SELECT RAISE(ABORT,'reset save failure'); END;").unwrap();
    let next = initial_layout::reset(
        &snapshot(3),
        Platform::Windows,
        false,
        previous.layout_revision,
        &Catalog::bundled(None),
    );
    assert!(c
        .put_settings_and_layout(&Settings::default(), &next)
        .is_err());
    assert_eq!(c.kv_get(SETTINGS_KEY).unwrap(), settings_bytes);
    assert_eq!(c.get_launcher_state().unwrap(), previous);
}

#[test]
fn empty_scans_and_old_unarranged_users_do_not_retry() {
    for (count, complete, status) in [
        (0, true, InitializationStatus::CompletedFlat),
        (0, false, InitializationStatus::CompletedFlat),
    ] {
        let c = cache();
        let first = prepare(&c, count, complete);
        assert_eq!(first.state.initial_layout.as_ref().unwrap().status, status);
        let later = prepare(&c, 3, true);
        assert!(later.state.folders.is_empty());
        assert_eq!(
            later.state.initial_layout.unwrap().generation,
            first.state.initial_layout.unwrap().generation
        );
    }
    let mut c = cache();
    c.new_data_area = false;
    assert_eq!(
        prepare(&c, 2, true).state.initial_layout.unwrap().status,
        InitializationStatus::PreservedExisting
    );
}

#[test]
fn partial_initial_scan_is_saved_once_and_later_apps_stay_outside_its_folder() {
    let c = cache();
    let first = prepare(&c, 2, false).state;
    assert_eq!(first.folders.len(), 1);
    assert!(first.initial_undo_available());
    assert_eq!(prepare(&c, 2, false).state, first);
    for (count, complete) in [(3, false), (4, true)] {
        let later = prepare(&c, count, complete).state;
        assert_eq!(later.folders, first.folders);
        assert_eq!(later.items[0], first.items[0]);
        assert_eq!(
            later.top_level_app_ids().cloned().collect::<Vec<_>>(),
            (2..count)
                .map(|n| AppId::from_normalized(format!("steam:{n}")))
                .collect::<Vec<_>>()
        );
        assert!(!later.initial_undo_available());
        assert_eq!(prepare(&c, count, complete).state, later);
    }
}

#[test]
fn empty_legacy_keys_settings_and_icon_history_are_existing_usage() {
    for key in [APP_ORDER_KEY, HIDDEN_KEY, SETTINGS_KEY] {
        let c = cache();
        let bytes = if key == SETTINGS_KEY {
            serde_json::to_vec(&Settings::default()).unwrap()
        } else {
            serialize_app_ids(&[])
        };
        c.kv_put(key, &bytes).unwrap();
        assert!(!c.load_layout().fresh);
        let state = prepare(&c, 2, true).state;
        assert!(state.folders.is_empty());
        assert!(state.customized);
    }
    let c = cache();
    c.put(&super::tests::entry("old", 1)).unwrap();
    assert!(!c.load_layout().fresh);
}

#[test]
fn corrupt_new_or_legacy_state_is_read_only_and_bytes_are_preserved() {
    for key in [LAUNCHER_STATE_KEY, APP_ORDER_KEY, HIDDEN_KEY] {
        let c = cache();
        c.kv_put(key, b"broken layout bytes").unwrap();
        assert!(!c.load_layout().writable);
        let result = prepare(&c, 2, true);
        assert!(!result.writable);
        assert!(result.state.folders.is_empty());
        assert_eq!(c.kv_get(key).unwrap(), b"broken layout bytes");
    }
}

#[test]
fn initial_save_failure_rolls_back_and_publishes_flat() {
    let c = cache();
    c.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_layout BEFORE INSERT ON kv WHEN NEW.key='launcher_state' BEGIN SELECT RAISE(ABORT,'injected save failure'); END;").unwrap();
    let result = prepare(&c, 2, true);
    assert!(!result.writable);
    assert!(result.state.folders.is_empty());
    assert!(c.kv_get(LAUNCHER_STATE_KEY).is_none());
    c.put_app_order(&[AppId::from_normalized("old")]).unwrap();
    assert!(c.put_launcher_state(&LauncherState::new()).is_err());
    assert_eq!(c.get_app_order(), vec![AppId::from_normalized("old")]);
}

#[test]
fn cache_reset_preserves_initialization_and_settings() {
    let c = cache();
    let first = prepare(&c, 2, true);
    c.put_settings(&Settings::default()).unwrap();
    c.clear_all().unwrap();
    assert_eq!(c.get_launcher_state().unwrap(), first.state);
    assert!(c.kv_get(SETTINGS_KEY).is_some());
    assert_eq!(prepare(&c, 2, true).state, first.state);
}

#[test]
fn restart_additions_and_removals_invalidate_undo_but_icon_updates_do_not() {
    let c = cache();
    let first = prepare(&c, 2, true);
    let mut icons = snapshot(2);
    icons.values_mut().for_each(|e| {
        e.icon_location = "updated.png".into();
        e.target_mtime = 8;
    });
    assert!(c
        .prepare_initial_layout(&icons, true, Platform::Windows, false)
        .state
        .initial_undo_available());
    let later = prepare(&c, 3, true);
    assert!(!later.state.initial_undo_available());
    assert_eq!(later.state.folders, first.state.folders);
    assert_eq!(
        later
            .state
            .items
            .last()
            .unwrap()
            .as_app_id()
            .unwrap()
            .as_str(),
        "steam:2"
    );
    let c = cache();
    prepare(&c, 2, true);
    assert!(!prepare(&c, 1, true).state.initial_undo_available());
}

#[test]
fn legacy_order_and_hidden_apps_migrate_atomically_without_grouping() {
    let c = cache();
    let a = AppId::from_normalized("steam:1");
    let b = AppId::from_normalized("steam:0");
    c.put_app_order(&[a.clone(), b.clone()]).unwrap();
    c.put_hidden_ids(std::slice::from_ref(&b)).unwrap();
    let state = prepare(&c, 2, true).state;
    assert!(state.folders.is_empty());
    assert!(state.hidden_apps.contains(&b));
    assert_eq!(
        state.top_level_app_ids().cloned().collect::<Vec<_>>(),
        vec![a]
    );
    assert!(c.kv_get(APP_ORDER_KEY).is_none());
    assert!(c.kv_get(HIDDEN_KEY).is_none());
    assert_eq!(c.get_launcher_state().unwrap(), state);
}

#[test]
fn corrupt_database_is_not_deleted_and_reopening_existing_empty_database_is_not_fresh() {
    let root = std::env::temp_dir().join(format!(
        "launchpad-initial-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("cache.sqlite3");
    std::fs::write(&path, b"corrupt database, preserve this").unwrap();
    let c = IconCache::open_preserving_at(path.clone());
    assert!(!c.load_layout().writable);
    drop(c);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"corrupt database, preserve this"
    );
    std::fs::remove_file(&path).unwrap();
    let c = IconCache::open_preserving_at(path.clone());
    assert!(c.load_layout().fresh);
    drop(c);
    let c = IconCache::open_preserving_at(path.clone());
    assert!(!c.load_layout().fresh);
    drop(c);
    std::fs::remove_file(&path).unwrap();
    // SQLite normally removes its sidecars on last close.
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    std::fs::remove_dir(root).unwrap();
}
