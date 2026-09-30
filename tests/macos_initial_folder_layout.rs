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
fn installed_mac_metadata_matches_suites_purposes_and_rejects_unknown_helpers() {
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
    assert_eq!(state.folders.len(), 15);
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

#[derive(serde::Deserialize)]
struct FullyReviewedApp {
    id: String,
    name: String,
    bundle: String,
    preset: Option<String>,
    product: Option<String>,
    reason: String,
    directory: Option<String>,
}

fn fully_reviewed() -> Vec<FullyReviewedApp> {
    serde_json::from_str(include_str!("fixtures/macos_catalog_full_observed.json")).unwrap()
}

fn full_snapshot() -> BTreeMap<AppId, SnapshotEntry> {
    fully_reviewed()
        .into_iter()
        .map(|app| {
            let mut e = entry(&app.bundle, &app.name);
            e.app_id = AppId::from_normalized(app.id);
            if let Some(directory) = app.directory {
                e.link_path = format!(
                    "/Applications/{directory}/Scripting.localized/{}.app",
                    app.name
                );
            }
            (e.app_id.clone(), e)
        })
        .collect()
}

#[test]
fn every_app_in_full_installed_inventory_has_a_reviewed_destination() {
    let snapshot = full_snapshot();
    assert_eq!(snapshot.len(), 217);
    let catalog = Catalog::bundled(None);
    let proposal = initial_layout::generate(
        &snapshot,
        &Settings::default(),
        Platform::Macos,
        true,
        "whole-inventory".into(),
        &catalog,
    );
    assert_eq!(proposal.state.folders.len(), 33);
    assert_eq!(proposal.state.top_level_app_ids().count(), 6);
    assert_eq!(proposal.state.folder_child_ids().count(), 211);
    let ids = all_ids(&proposal.state);
    assert_eq!(ids.len(), snapshot.len());
    assert_eq!(
        ids.into_iter().collect::<BTreeSet<_>>(),
        snapshot.keys().cloned().collect()
    );
    for app in fully_reviewed() {
        assert!(
            !app.reason.is_empty(),
            "{} lacks a reviewed reason",
            app.name
        );
        let e = &snapshot[&AppId::from_normalized(app.id)];
        if let Some(preset) = app.preset {
            assert_eq!(
                catalog.classify(e, Platform::Macos),
                Some(Classification::Product {
                    preset: &preset,
                    product: app.product.unwrap()
                }),
                "{}",
                app.name
            );
            assert_eq!(
                proposal.decisions[&e.app_id],
                initial_layout::Decision::Grouped(preset),
                "{}",
                app.name
            );
            if !e.app_id.is_steam() {
                assert!(!matches!(
                    catalog.classify(e, Platform::Windows),
                    Some(Classification::Product { .. })
                ));
                let mut renamed = e.clone();
                renamed.name = "用途を連想できない表示名".into();
                renamed.publisher = "Unrelated publisher".into();
                assert_eq!(
                    catalog.classify(&renamed, Platform::Macos),
                    catalog.classify(e, Platform::Macos)
                );
                renamed.identifier.push_str(".unknown-helper");
                assert_eq!(catalog.classify(&renamed, Platform::Macos), None);
            }
        } else {
            assert_eq!(
                catalog.classify(e, Platform::Macos),
                Some(Classification::KeepTopLevel),
                "{}",
                app.name
            );
            assert_eq!(
                proposal.decisions[&e.app_id],
                initial_layout::Decision::KeepTopLevel
            );
        }
    }
    let reset = initial_layout::reset(&snapshot, Platform::Macos, true, 10, &catalog);
    assert_eq!(reset.items, proposal.state.items);
    assert_eq!(reset.folders, proposal.state.folders);
    let mut persisted: LauncherState =
        serde_json::from_str(&serde_json::to_string(&proposal.state).unwrap()).unwrap();
    assert!(persisted.undo_initial_layout());
    assert!(persisted.folders.is_empty());
    assert_eq!(persisted.items.len(), 217);
}

#[test]
fn illustrator_generated_script_ids_require_the_verified_suite_directory() {
    let catalog = Catalog::bundled(None);
    for bundle in [
        "com.apple.ScriptEditor.id.Contact-Sheets",
        "com.apple.ScriptEditor.id.Make-Calendar",
        "com.apple.ScriptEditor.id.Web-Gallery",
    ] {
        let mut e = entry(bundle, "User Script");
        assert_eq!(catalog.classify(&e, Platform::Macos), None);
        for directory in [
            "Adobe Illustrator",
            "Adobe Illustrator 2024",
            "Adobe Illustrator 2026",
        ] {
            e.link_path = format!("/Applications/{directory}/Scripting.localized/Sample Scripts.localized/User Script.app");
            assert_eq!(
                catalog.classify(&e, Platform::Macos),
                Some(Classification::Product {
                    preset: "adobe-creative-cloud",
                    product: "illustrator".into()
                })
            );
        }
        for directory in [
            "Adobe Illustrator Helper",
            "Adobe Illustrator 20xx",
            "Adobe Illustrator 2026 Extra",
            "My Adobe Illustrator 2026",
        ] {
            e.link_path = format!("/Applications/{directory}/User Script.app");
            assert_eq!(catalog.classify(&e, Platform::Macos), None);
        }
    }
}

#[test]
fn mac_purpose_rules_do_not_reclassify_windows_daily_entry_points() {
    let catalog = Catalog::bundled(None);
    let mut e = entry("com.microsoft.VSCode", "Code");
    assert!(matches!(
        catalog.classify(&e, Platform::Macos),
        Some(Classification::Product {
            preset: "macos-development",
            ..
        })
    ));
    e.target_path = "C:/Program Files/Microsoft VS Code/Code.exe".into();
    e.identifier.clear();
    e.launch_arguments = Some(String::new());
    assert_eq!(
        catalog.classify(&e, Platform::Windows),
        Some(Classification::KeepTopLevel)
    );
    assert_eq!(catalog.classify(&e, Platform::Macos), None);
    e.identifier = "com.google.UnknownApp".into();
    e.name = "Google Docs".into();
    assert_eq!(catalog.classify(&e, Platform::Macos), None);
}

#[test]
fn voice_versions_and_suite_support_tools_do_not_manufacture_two_products() {
    let catalog = Catalog::bundled(None);
    for bundles in [
        vec![
            "com.dreamtonics.svstudio.basic",
            "com.dreamtonics.svstudio.pro",
            "com.dreamtonics.svstudio2.pro",
        ],
        vec![
            "com.adobe.Install",
            "com.adobe.ACCC.Uninstaller",
            "com.adobe.accmac",
        ],
        vec![
            "com.googlecode.iterm2",
            "com.googlecode.iterm2.iTermBrowserPlugin",
        ],
        vec!["notion.id", "notion.id.NotionSafariExtension"],
        vec![
            "net.maxon.cinema4d",
            "net.maxon.python",
            "net.maxon.cinema4d.installer.uninstall",
        ],
        vec![
            "jp.co.celsys.CLIPSTUDIO",
            "jp.co.celsys.CertMdul",
            "jp.co.celsys.AggregateMdul",
        ],
    ] {
        let snapshot = bundles
            .iter()
            .map(|b| {
                let e = entry(b, b);
                (e.app_id.clone(), e)
            })
            .collect();
        let state = initial_layout::reset(&snapshot, Platform::Macos, true, 0, &catalog);
        assert!(state.folders.is_empty(), "{bundles:?}");
    }
}
