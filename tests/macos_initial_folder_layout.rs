use std::collections::{BTreeMap, BTreeSet};

use launchpad_windows::domain::{
    app_diff::SnapshotEntry,
    app_id::AppId,
    initial_layout::{
        self,
        catalog::{Catalog, Classification, Platform},
    },
    launcher_state::LauncherState,
    settings::Settings,
};

#[derive(serde::Deserialize)]
struct ObservedApp {
    name: String,
    bundle: String,
    preset: Option<String>,
    product: Option<String>,
}

fn observed() -> Vec<ObservedApp> {
    serde_json::from_str(include_str!("fixtures/macos_catalog_observed.json")).unwrap()
}

fn entry(bundle: &str, name: &str) -> SnapshotEntry {
    SnapshotEntry {
        app_id: AppId::from_normalized(format!("mac:{bundle}")),
        name: name.into(),
        link_path: format!("/Applications/{name}.app"),
        source_modified_at: 0,
        target_path: format!("/Applications/{name}.app/Contents/MacOS/App"),
        launch_arguments: None,
        target_modified_at: 0,
        icon_location: String::new(),
        icon_index: 0,
        provider_icon: None,
        version: String::new(),
        publisher: String::new(),
        identifier: bundle.into(),
    }
}

fn snapshot() -> BTreeMap<AppId, SnapshotEntry> {
    observed()
        .into_iter()
        .map(|app| {
            let entry = entry(&app.bundle, &app.name);
            (entry.app_id.clone(), entry)
        })
        .collect()
}

fn all_ids(state: &LauncherState) -> Vec<AppId> {
    state
        .top_level_app_ids()
        .chain(state.folder_child_ids())
        .cloned()
        .collect()
}

#[test]
fn installed_mac_metadata_matches_expected_suites_and_rejects_vendor_helpers() {
    let catalog = Catalog::bundled(None);
    for app in observed() {
        let mut e = entry(&app.bundle, &app.name);
        if let Some(preset) = app.preset {
            assert_eq!(
                catalog.classify(&e, Platform::Macos),
                Some(Classification::Product {
                    preset: &preset,
                    product: app.product.unwrap(),
                }),
                "{}",
                app.bundle,
            );
            assert!(!matches!(
                catalog.classify(&e, Platform::Windows),
                Some(Classification::Product { .. })
            ));
            // Neither translated labels, copyright nor installation folders
            // establish product identity. Only the observed bundle ID does.
            let expected = catalog.classify(&e, Platform::Macos);
            e.name = "別のアプリ名".into();
            e.publisher = "Unrelated copyright".into();
            e.link_path = "/Users/test/Applications/Renamed.app".into();
            e.target_path = "/Users/test/Applications/Renamed.app/Contents/MacOS/App".into();
            assert_eq!(catalog.classify(&e, Platform::Macos), expected);
            e.identifier.push_str(".helper");
            assert_eq!(catalog.classify(&e, Platform::Macos), None);
        } else {
            assert!(
                !matches!(
                    catalog.classify(&e, Platform::Macos),
                    Some(Classification::Product { .. })
                ),
                "{}",
                app.bundle
            );
        }
    }
}

#[test]
fn mac_first_run_reset_and_undo_preserve_every_app_exactly_once() {
    let snapshot = snapshot();
    let catalog = Catalog::bundled(None);
    let proposal = initial_layout::generate(
        &snapshot,
        &Settings::default(),
        Platform::Macos,
        true,
        "mac-first-run".into(),
        &catalog,
    );
    let mut state = proposal.state;
    assert_eq!(state.folders.len(), 14);
    assert_eq!(all_ids(&state).len(), snapshot.len());
    assert_eq!(
        all_ids(&state).into_iter().collect::<BTreeSet<_>>(),
        snapshot.keys().cloned().collect()
    );
    let reset = initial_layout::reset(&snapshot, Platform::Macos, true, 7, &catalog);
    let membership = |s: &LauncherState| {
        s.folders
            .values()
            .map(|f| (f.name.clone(), f.children.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(membership(&state), membership(&reset));
    assert_eq!(reset.layout_revision, 8);
    // A persisted first-run result still offers the production undo path.
    state = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    let before = state
        .initial_layout
        .as_ref()
        .unwrap()
        .undo
        .as_ref()
        .unwrap()
        .before
        .clone();
    assert!(state.undo_initial_layout());
    assert_eq!(state.items, before);
    assert!(state.folders.is_empty());
}

#[test]
fn mac_standard_apps_split_accessories_utilities_and_remote_desktop() {
    let state = initial_layout::reset(
        &snapshot(),
        Platform::Macos,
        true,
        0,
        &Catalog::bundled(None),
    );
    let folder_for = |bundle: &str| {
        state
            .folders
            .values()
            .find(|f| {
                f.children
                    .contains(&AppId::from_normalized(format!("mac:{bundle}")))
            })
            .map(|f| f.name.as_str())
    };
    assert_eq!(folder_for("com.apple.TextEdit"), Some("Macアクセサリ"));
    assert_eq!(
        folder_for("com.apple.Terminal"),
        Some("macOSユーティリティ")
    );
    assert_eq!(
        folder_for("com.apple.ScreenSharing"),
        Some("リモートデスクトップ")
    );
    for bundle in [
        "com.apple.Safari",
        "com.apple.calculator",
        "com.apple.systempreferences",
        "com.apple.Notes",
    ] {
        assert_eq!(folder_for(bundle), None);
    }
}

#[test]
fn mac_versions_aliases_and_duplicate_launches_do_not_satisfy_two_product_threshold() {
    let catalog = Catalog::bundled(None);
    for bundles in [
        [
            "com.adobe.AfterEffects",
            "com.adobe.AfterEffects.application",
        ],
        ["com.steinberg.cubase14", "com.steinberg.cubasesoft13"],
        ["com.adobe.PremierePro.25", "com.adobe.PremierePro.26"],
        ["com.unity3d.UnityEditor5.x", "com.unity3d.UnityEditor5.x"],
        [
            "jp.co.celsys.CLIPSTUDIOPAINT",
            "jp.co.celsys.CLIPSTUDIOPAINT",
        ],
    ] {
        let entries = bundles
            .into_iter()
            .enumerate()
            .map(|(index, bundle)| {
                let mut e = entry(bundle, &format!("App {index}"));
                e.app_id = AppId::from_normalized(format!("copy:{index}"));
                (e.app_id.clone(), e)
            })
            .collect();
        let state = initial_layout::reset(&entries, Platform::Macos, false, 0, &catalog);
        assert!(state.folders.is_empty(), "{bundles:?}");
        assert_eq!(state.items.len(), 2);
    }
}
