use launchpad_windows::domain::{
    app_diff::SnapshotEntry,
    app_id::AppId,
    initial_layout::{
        self,
        catalog::{Catalog, Classification, Platform},
        InitializationStatus,
    },
    launcher_item::LauncherItem,
    launcher_state::LauncherState,
    settings::Settings,
};
use std::collections::{BTreeMap, BTreeSet};

fn app(id: &str, name: &str, path: &str, product: &str, publisher: &str) -> SnapshotEntry {
    SnapshotEntry {
        app_id: AppId::from_normalized(id),
        name: name.into(),
        link_path: format!("C:/{id}.lnk"),
        target_path: path.into(),
        launch_arguments: Some(String::new()),
        source_modified_at: 0,
        target_modified_at: 0,
        icon_location: String::new(),
        icon_index: 0,
        provider_icon: None,
        version: String::new(),
        publisher: publisher.into(),
        identifier: product.into(),
    }
}
fn shell(id: &str, name: &str, shell_id: &str) -> SnapshotEntry {
    let mut e = app(id, name, "", "", "");
    e.link_path = format!("shell:AppsFolder\\{shell_id}");
    e.launch_arguments = None;
    e
}
fn catalog() -> Catalog {
    Catalog::bundled(Some("C:/Windows".into()))
}
fn snapshot(apps: Vec<SnapshotEntry>) -> BTreeMap<AppId, SnapshotEntry> {
    apps.into_iter().map(|e| (e.app_id.clone(), e)).collect()
}
fn generate(apps: Vec<SnapshotEntry>) -> LauncherState {
    initial_layout::generate(
        &snapshot(apps),
        &Settings::default(),
        Platform::Windows,
        true,
        "order-test".into(),
        &catalog(),
    )
    .state
}
fn ids(state: &LauncherState) -> Vec<String> {
    state.top_level_app_ids().map(ToString::to_string).collect()
}
fn items(state: &LauncherState) -> Vec<String> {
    state
        .items
        .iter()
        .map(|item| match item {
            LauncherItem::App(id) => id.to_string(),
            LauncherItem::Folder(id) => state.folders[id].name.clone(),
        })
        .collect()
}
fn settings() -> SnapshotEntry {
    shell(
        "settings",
        "設定",
        "windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel",
    )
}
fn unknown() -> SnapshotEntry {
    app("unknown", "AAA unknown", "C:/Unknown.exe", "", "")
}
fn notepad() -> SnapshotEntry {
    shell(
        "notepad",
        "メモ帳",
        "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App",
    )
}
fn security() -> SnapshotEntry {
    shell(
        "security",
        "Windows セキュリティ",
        "Microsoft.SecHealthUI_8wekyb3d8bbwe!SecHealthUI",
    )
}

#[test]
fn everyday_and_work_items_precede_general_and_management_without_losing_apps() {
    let apps = vec![
        unknown(),
        settings(),
        notepad(),
        security(),
        shell("explorer", "エクスプローラー", "Microsoft.Windows.Explorer"),
        shell(
            "store",
            "Microsoft Store",
            "Microsoft.WindowsStore_8wekyb3d8bbwe!App",
        ),
        shell(
            "calc",
            "電卓",
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
        ),
        shell(
            "snip",
            "Snipping Tool",
            "Microsoft.ScreenSketch_8wekyb3d8bbwe!App",
        ),
        shell(
            "feedback",
            "Feedback",
            "Microsoft.WindowsFeedbackHub_8wekyb3d8bbwe!App",
        ),
        shell(
            "gamebar",
            "AAA Game Bar",
            "Microsoft.XboxGamingOverlay_8wekyb3d8bbwe!App",
        ),
        app(
            "chrome",
            "Chrome",
            "C:/Program Files/Google/Chrome/Application/chrome.exe",
            "Google Chrome",
            "Google LLC",
        ),
        app(
            "edge",
            "Edge",
            "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
            "Microsoft Edge",
            "Microsoft Corporation",
        ),
        app(
            "discord",
            "Discord",
            "C:/Apps/Discord/Discord.exe",
            "Discord",
            "Discord Inc.",
        ),
        app("obs", "OBS", "C:/OBS/obs64.exe", "OBS Studio", "OBS"),
        app(
            "photoshop",
            "PS",
            "C:/Adobe/Photoshop.exe",
            "Adobe Photoshop 2026",
            "Adobe",
        ),
        app(
            "illustrator",
            "AI",
            "C:/Adobe/Illustrator.exe",
            "Adobe Illustrator 2026",
            "Adobe",
        ),
    ];
    let expected: BTreeSet<_> = apps.iter().map(|e| e.app_id.clone()).collect();
    let state = generate(apps.clone());
    assert_eq!(
        items(&state),
        [
            "explorer",
            "chrome",
            "edge",
            "discord",
            "store",
            "settings",
            "calc",
            "Adobe Creative Cloud",
            "obs",
            "Windowsアクセサリ",
            "unknown",
            "gamebar",
            "PC管理"
        ]
    );
    let all: Vec<_> = state
        .top_level_app_ids()
        .chain(state.folder_child_ids())
        .cloned()
        .collect();
    assert_eq!(all.len(), expected.len());
    assert_eq!(all.into_iter().collect::<BTreeSet<_>>(), expected);
    let mut reverse = apps;
    reverse.reverse();
    assert_eq!(state, generate(reverse));
    let accessory = state
        .folders
        .values()
        .find(|f| f.name == "Windowsアクセサリ")
        .unwrap();
    assert_eq!(
        accessory.children,
        [
            AppId::from_normalized("notepad"),
            AppId::from_normalized("snip")
        ]
    );
    let management = state.folders.values().find(|f| f.name == "PC管理").unwrap();
    assert_eq!(
        management.children,
        [
            AppId::from_normalized("security"),
            AppId::from_normalized("feedback")
        ]
    );
}

#[test]
fn ordering_without_folders_can_be_undone_and_survives_serialization() {
    let source = vec![unknown(), settings()];
    let state = generate(source.clone());
    assert!(state.folders.is_empty());
    assert_eq!(ids(&state), ["settings", "unknown"]);
    assert_eq!(
        state.initial_layout.as_ref().unwrap().status,
        InitializationStatus::Applied
    );
    let json = serde_json::to_string(&state).unwrap();
    let mut restored: LauncherState = serde_json::from_str(&json).unwrap();
    assert!(restored.initial_undo_available());
    assert!(restored.undo_initial_layout());
    assert_eq!(ids(&restored), ["unknown", "settings"]);
    assert!(!restored.undo_initial_layout());
    let reset = initial_layout::reset(
        &snapshot(source),
        Platform::Windows,
        true,
        restored.layout_revision,
        &catalog(),
    );
    assert_eq!(ids(&reset), ["settings", "unknown"]);
    let mut changed = reset;
    changed.items.swap(0, 1);
    changed.layout_changed();
    assert!(!changed.initial_undo_available());
}

#[test]
fn no_missing_apps_are_synthesized_and_singletons_keep_their_role() {
    assert!(generate(vec![]).items.is_empty());
    assert_eq!(
        generate(vec![settings()]).initial_layout.unwrap().status,
        InitializationStatus::CompletedFlat
    );
    let state = generate(vec![security(), unknown(), notepad()]);
    assert!(state.folders.is_empty());
    assert_eq!(ids(&state), ["notepad", "unknown", "security"]);
    let ps = app(
        "ps",
        "ZZ Photoshop",
        "C:/Adobe/Photoshop.exe",
        "Adobe Photoshop 2026",
        "Adobe",
    );
    assert_eq!(ids(&generate(vec![unknown(), ps])), ["ps", "unknown"]);
}

#[test]
fn partial_first_scan_orders_available_apps_and_later_discoveries_append() {
    for entries in [vec![], vec![unknown()], vec![unknown(), settings()]] {
        let mut apps = snapshot(entries.clone());
        let mut state = initial_layout::prepare_catalog_layout(
            LauncherState::new(),
            &apps,
            &Settings::default(),
            true,
            false,
            true,
            &catalog(),
        );
        assert_eq!(state.items, generate(entries).items);
        assert!(state.customized);
        let before = state.items.clone();
        let late = app("late", "000 Late app", "C:/late.exe", "", "");
        apps.insert(late.app_id.clone(), late);
        state = initial_layout::prepare_catalog_layout(
            state,
            &apps,
            &Settings::default(),
            false,
            true,
            true,
            &catalog(),
        );
        state.integrate_discovered_apps(&apps.keys().cloned().collect(), |id| {
            apps.get(id).map(|e| e.name.clone())
        });
        assert_eq!(&state.items[..before.len()], before.as_slice());
        assert_eq!(
            state.items.last().unwrap().as_app_id().unwrap().as_str(),
            "late"
        );
    }
}

#[test]
fn legacy_skipped_scan_keeps_its_order_and_appends_later_discoveries() {
    let mut state = LauncherState::new();
    state.items = vec![LauncherItem::App(AppId::from_normalized("unknown"))];
    state.initial_layout = Some(initial_layout::InitialLayoutRecord::new(
        InitializationStatus::SkippedError,
        "old-version".into(),
    ));
    let apps = snapshot(vec![
        unknown(),
        app("late", "000 Late app", "C:/late.exe", "", ""),
    ]);
    let mut restored = initial_layout::prepare_catalog_layout(
        state.clone(),
        &apps,
        &Settings::default(),
        false,
        false,
        true,
        &catalog(),
    );
    assert_eq!(restored.items, state.items);
    assert_eq!(restored.initial_layout, state.initial_layout);
    restored.integrate_discovered_apps(&apps.keys().cloned().collect(), |id| {
        apps.get(id).map(|e| e.name.clone())
    });
    assert_eq!(ids(&restored), ["unknown", "late"]);
}

#[test]
fn versions_promote_one_representative_using_numeric_release_not_name_order() {
    let blender = |id: &str, version: &str| {
        let mut e = app(
            id,
            id,
            &format!("C:/{id}/blender.exe"),
            "Blender",
            "Blender Foundation",
        );
        e.version = version.into();
        e
    };
    let state = generate(vec![
        blender("z-new", "10.0"),
        blender("a-old", "9.9"),
        blender("a-preview", "11.0 Alpha"),
        unknown(),
    ]);
    assert_eq!(ids(&state)[0], "z-new");
    assert_eq!(ids(&state)[1], "a-old");
    let state = generate(vec![
        blender("b", "unknown"),
        blender("a", "unknown"),
        unknown(),
    ]);
    assert_eq!(ids(&state)[0], "a");
    let mut ue1 = app(
        "ue-old",
        "Unreal",
        "C:/UE/UnrealEditor.exe",
        "UnrealEditor",
        "Epic Games, Inc.",
    );
    ue1.version = "++UE5+Release-5.7-CL-1234".into();
    let mut ue2 = ue1.clone();
    ue2.app_id = AppId::from_normalized("ue-new");
    ue2.version = "++UE5+Release-5.8-CL-56702186".into();
    assert_eq!(ids(&generate(vec![ue1, ue2, unknown()]))[0], "ue-new");
}

#[test]
fn windows_classification_accepts_only_exact_system_launch_modes() {
    let c = catalog();
    for (path, args) in [
        ("C:/Windows/system32/compmgmt.msc", "/s"),
        ("C:/Windows/system32/eventvwr.msc", ""),
        (
            "C:/Windows/system32/mmc.exe",
            r#""C:\Windows\system32\services.msc""#,
        ),
        (
            "C:/Windows/system32/rundll32.exe",
            "C:\\WINDOWS\\system32\\utv_core.dll,GlobalConfigDialog",
        ),
        ("C:/Windows/SysWOW64/rundll32.exe", "x264vfw.dll,Configure"),
    ] {
        let mut e = app("system", "Unknown", path, "", "Microsoft Corporation");
        e.launch_arguments = Some(args.into());
        assert!(
            matches!(
                c.classify(&e, Platform::Windows),
                Some(Classification::Product {
                    preset: "windows-tools",
                    ..
                })
            ),
            "{path}"
        );
        e.target_path = e.target_path.replace("Windows", "Other");
        assert!(c.classify(&e, Platform::Windows).is_none());
    }
    for (path, args) in [
        ("C:/Windows/explorer.exe", "C:\\Projects"),
        ("C:/Windows/system32/mmc.exe", "C:\\Users\\me\\services.msc"),
        ("C:/Windows/system32/compmgmt.msc", "--unknown"),
        (
            "C:/Windows/system32/rundll32.exe",
            "C:\\Other\\utv_core.dll,GlobalConfigDialog",
        ),
        (
            "C:/Windows/system32/rundll32.exe",
            "C:\\Windows\\system32\\utv_core.dll,Other",
        ),
    ] {
        let mut e = app("fake", "設定", path, "", "Microsoft Corporation");
        e.launch_arguments = Some(args.into());
        assert!(c.classify(&e, Platform::Windows).is_none());
        assert_eq!(ids(&generate(vec![e, unknown()])), ["unknown", "fake"]);
    }
}

#[test]
fn misleading_names_and_publishers_do_not_promote_apps() {
    for mut fake in [
        settings(),
        app(
            "fake",
            "設定",
            "C:/chrome.exe",
            "Google Chrome",
            "Google LLC",
        ),
        app(
            "fake",
            "Discord",
            "C:/Discord.exe",
            "Discord",
            "Other Publisher",
        ),
    ] {
        if fake.app_id.as_str() == "settings" {
            fake.link_path.push_str(".fake");
        }
        let id = fake.app_id.to_string();
        assert_eq!(
            ids(&generate(vec![fake, unknown()])),
            ["unknown", id.as_str()]
        );
    }
}

#[test]
fn vlc_auxiliary_modes_move_back_but_normal_playback_stays_general() {
    let mut reset = app(
        "reset",
        "AAA reset",
        "C:/VLC/vlc.exe",
        "VLC media player",
        "VideoLAN",
    );
    reset.launch_arguments = Some("--reset-config --reset-plugins-cache vlc://quit".into());
    let vlc = app(
        "vlc",
        "VLC",
        "C:/VLC/vlc.exe",
        "VLC media player",
        "VideoLAN",
    );
    assert_eq!(
        ids(&generate(vec![reset, unknown(), vlc])),
        ["unknown", "vlc", "reset"]
    );
}

#[test]
fn windows_roles_stay_stable_across_label_and_locale_changes_and_do_not_apply_on_mac() {
    let entries = snapshot(vec![settings(), unknown(), notepad(), security()]);
    let windows = initial_layout::generate(
        &entries,
        &Settings::default(),
        Platform::Windows,
        false,
        "locale".into(),
        &catalog(),
    )
    .state;
    assert_eq!(
        ids(&windows),
        ["settings", "notepad", "unknown", "security"]
    );
    let mut renamed = entries.clone();
    for e in renamed.values_mut() {
        e.name = "同じ名前".into();
    }
    let result = initial_layout::generate(
        &renamed,
        &Settings::default(),
        Platform::Windows,
        true,
        "locale".into(),
        &catalog(),
    )
    .state;
    assert_eq!(ids(&result), ids(&windows));
    let mac = initial_layout::generate(
        &entries,
        &Settings::default(),
        Platform::Macos,
        false,
        "mac".into(),
        &catalog(),
    )
    .state;
    assert_eq!(ids(&mac), ["unknown", "security", "notepad", "settings"]);
    assert!(!mac.initial_undo_available());
}
