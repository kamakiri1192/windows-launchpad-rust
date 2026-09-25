use launchpad_windows::domain::{
    app_diff::SnapshotEntry,
    app_id::AppId,
    initial_layout::{
        self,
        catalog::{Catalog, Platform, Rule},
        Decision, InitializationStatus,
    },
    launcher_state::LauncherState,
    settings::Settings,
};
use std::collections::{BTreeMap, BTreeSet};

fn entry(
    id: &str,
    name: &str,
    target: &str,
    args: Option<&str>,
    identifier: &str,
) -> SnapshotEntry {
    SnapshotEntry {
        provider_icon: None,
        app_id: AppId::from_normalized(id),
        name: name.into(),
        link_path: format!("C:/Start Menu/{id}.lnk"),
        link_mtime: 0,
        target_path: target.into(),
        launch_arguments: args.map(str::to_owned),
        target_mtime: 0,
        icon_location: String::new(),
        icon_index: 0,
        version: String::new(),
        publisher: "Microsoft Corporation".into(),
        identifier: identifier.into(),
    }
}
fn steam(id: &str, name: &str) -> SnapshotEntry {
    entry(&format!("steam:{id}"), name, "manifest.acf", None, "")
}
fn map(entries: Vec<SnapshotEntry>) -> BTreeMap<AppId, SnapshotEntry> {
    entries.into_iter().map(|e| (e.app_id.clone(), e)).collect()
}
fn generate(entries: Vec<SnapshotEntry>, platform: Platform) -> LauncherState {
    initial_layout::generate(
        &map(entries),
        &Settings::default(),
        platform,
        false,
        "test-generation".into(),
        &Catalog::bundled(Some("C:/Windows".into())),
    )
    .state
}
fn all_ids(state: &LauncherState) -> Vec<AppId> {
    state
        .top_level_app_ids()
        .chain(state.folder_child_ids())
        .cloned()
        .collect()
}

#[test]
fn zero_one_two_distinct_targets_and_saved_identity() {
    for n in 0..=2 {
        let state = generate(
            (0..n)
                .map(|i| steam(&i.to_string(), &format!("App {i}")))
                .collect(),
            Platform::Windows,
        );
        assert_eq!(state.folders.len(), usize::from(n == 2));
        assert_eq!(all_ids(&state).len(), n);
        assert!(state.customized);
        assert_eq!(
            serde_json::from_str::<LauncherState>(&serde_json::to_string(&state).unwrap()).unwrap(),
            state
        );
    }
}

#[test]
fn same_product_versions_and_duplicate_shortcuts_cannot_create_a_folder() {
    let word = |id, version| {
        entry(
            id,
            "Word",
            &format!("C:/Program Files/Microsoft Office/Office{version}/WINWORD.EXE"),
            Some(""),
            "",
        )
    };
    assert!(
        generate(vec![word("a", 16), word("b", 16)], Platform::Windows)
            .folders
            .is_empty()
    );
    assert!(
        generate(vec![word("a", 15), word("b", 16)], Platform::Windows)
            .folders
            .is_empty()
    );
    let mut entries = vec![word("a", 15), word("b", 16)];
    entries.push(entry(
        "excel",
        "Excel",
        "C:/Program Files/Microsoft Office/Office16/EXCEL.EXE",
        Some(""),
        "",
    ));
    let state = generate(entries, Platform::Windows);
    assert_eq!(state.folders.len(), 1);
    assert_eq!(state.folders.values().next().unwrap().children.len(), 3);
}

#[test]
fn shared_libreoffice_exe_counts_component_arguments_and_unknown_is_conservative() {
    let office = |id, args| {
        entry(
            id,
            id,
            "C:/Program Files/LibreOffice/program/soffice.exe",
            args,
            "",
        )
    };
    assert_eq!(
        generate(
            vec![
                office("writer", Some("--writer")),
                office("calc", Some("--calc"))
            ],
            Platform::Windows
        )
        .folders
        .len(),
        1
    );
    assert!(generate(
        vec![
            office("writer", Some("--writer")),
            office("duplicate", Some("--writer"))
        ],
        Platform::Windows
    )
    .folders
    .is_empty());
    assert!(generate(
        vec![office("writer", Some("--writer")), office("unknown", None)],
        Platform::Windows
    )
    .folders
    .is_empty());
    assert!(generate(
        vec![
            office("writer", Some("--writer")),
            office("calc", Some("--calc file.ods"))
        ],
        Platform::Windows
    )
    .folders
    .is_empty());
    assert!(generate(
        vec![office("single-bundle", Some("--writer"))],
        Platform::Macos
    )
    .folders
    .is_empty());
}

#[test]
fn visual_studio_shared_cmd_features_have_distinct_identity() {
    let vs = |id, script| {
        entry(id,id,"C:/Windows/System32/cmd.exe",
        Some(&format!("/k \"C:\\Program Files (x86)\\Microsoft Visual Studio\\18\\BuildTools\\VC\\Auxiliary\\Build\\{script}\"")),"")
    };
    let state = generate(
        vec![vs("x64", "vcvars64.bat"), vs("x86", "vcvars32.bat")],
        Platform::Windows,
    );
    assert_eq!(
        state.folders.values().next().unwrap().name,
        "Visual Studio Tools"
    );
    assert!(generate(
        vec![vs("x64", "vcvars64.bat"), vs("x64-again", "vcvars64.bat")],
        Platform::Windows
    )
    .folders
    .is_empty());
    assert!(generate(
        vec![vs("a", "arbitrary.bat"), vs("b", "uninstall.bat")],
        Platform::Windows
    )
    .folders
    .is_empty());
}

#[test]
fn windows_tools_require_system_location_and_do_not_group_calculator() {
    let a = entry(
        "cleanup",
        "ディスククリーンアップ",
        "C:/WINDOWS/system32/cleanmgr.exe",
        Some(""),
        "",
    );
    let b = entry(
        "system",
        "System Information",
        "C:/Windows/system32/msinfo32.exe",
        Some(""),
        "",
    );
    let calculator = entry(
        "calc",
        "Calculator",
        "C:/Windows/system32/calc.exe",
        Some(""),
        "",
    );
    let impostor = entry(
        "fake",
        "Other app",
        "C:/Downloads/cleanmgr.exe",
        Some(""),
        "",
    );
    let state = generate(vec![a, b, calculator, impostor], Platform::Windows);
    assert_eq!(state.folders.len(), 1);
    assert_eq!(state.top_level_app_ids().count(), 2);
    assert_eq!(all_ids(&state).len(), 4);
}

#[test]
fn adobe_installed_product_metadata_groups_all_six_families_and_keeps_versions_together() {
    // Read-only verification of installed Windows shortcuts on 2026-09-25.
    let fixtures = [
        ("AfterFX.exe", "Adobe After Effects 2025", "Adobe Inc."),
        (
            "AfterFX.exe",
            "Adobe After Effects 2026",
            "Adobe Systems Incorporated",
        ),
        ("Adobe Audition.exe", "Adobe Audition 2025", "Adobe"),
        ("Adobe Audition.exe", "Adobe Audition 2026", "Adobe"),
        ("Illustrator.exe", "Adobe Illustrator 2026", "Adobe Inc."),
        (
            "Adobe Media Encoder.exe",
            "Adobe Media Encoder 2025",
            "Adobe",
        ),
        (
            "Adobe Media Encoder.exe",
            "Adobe Media Encoder 2026",
            "Adobe",
        ),
        ("Photoshop.exe", "Adobe Photoshop 2026", "Adobe"),
        ("Adobe Premiere Pro.exe", "Adobe Premiere Pro 2025", "Adobe"),
        ("Adobe Premiere Pro.exe", "Adobe Premiere 2026", "Adobe"),
    ];
    let entries: Vec<_> = fixtures
        .into_iter()
        .enumerate()
        .map(|(i, (exe, product, publisher))| {
            let mut app = entry(
                &format!("adobe-{i}"),
                product,
                &format!("C:/Program Files/Adobe/{product}/{exe}"),
                Some(""),
                product,
            );
            app.publisher = publisher.into();
            app
        })
        .collect();
    let state = generate(entries.clone(), Platform::Windows);
    assert_eq!(state.folders.len(), 1);
    let folder = state.folders.values().next().unwrap();
    assert_eq!(folder.name, "Adobe Creative Cloud");
    assert_eq!(folder.children.len(), 10);
    assert_eq!(
        all_ids(&state).into_iter().collect::<BTreeSet<_>>(),
        entries.iter().map(|e| e.app_id.clone()).collect()
    );
    // Two releases of one application do not make a product suite.
    assert!(generate(entries[..2].to_vec(), Platform::Windows)
        .folders
        .is_empty());
    assert!(generate(entries[8..].to_vec(), Platform::Windows)
        .folders
        .is_empty());
    // Explicit reset uses the same catalog on an existing user's current scan.
    let reset = initial_layout::reset(
        &map(entries),
        Platform::Windows,
        true,
        12,
        &Catalog::bundled(None),
    );
    assert_eq!(reset.folders.values().next().unwrap().children.len(), 10);
    assert!(reset.initial_undo_available());
}

#[test]
fn adobe_rules_exclude_reader_helpers_and_unverified_metadata() {
    let catalog = Catalog::bundled(None);
    let mut app = entry(
        "adobe",
        "Photoshop",
        "C:/Adobe/Photoshop.exe",
        Some(""),
        "Adobe Photoshop 2026",
    );
    app.publisher = "Adobe".into();
    {
        let (exe, product) = ("AcroRd32.exe", "Adobe Acrobat Reader");
        let mut client = app.clone();
        client.target_path = format!("C:/Adobe/{exe}");
        client.identifier = product.into();
        assert_eq!(
            catalog.classify(&client, Platform::Windows),
            Some(initial_layout::catalog::Classification::KeepTopLevel)
        );
    }
    for (exe, product, publisher, args) in [
        ("Unknown.exe", "Adobe Photoshop 2026", "Adobe", ""),
        (
            "Photoshop.exe",
            "Adobe Photoshop Elements 2026",
            "Adobe",
            "",
        ),
        (
            "Photoshop.exe",
            "Adobe Photoshop 2026",
            "Other publisher",
            "",
        ),
        ("Photoshop.exe", "Adobe Photoshop 2026", "Adobe", "--update"),
        ("Photoshop.exe", "Adobe Photoshop helper", "Adobe", ""),
    ] {
        let mut negative = app.clone();
        negative.target_path = format!("C:/Adobe/{exe}");
        negative.identifier = product.into();
        negative.publisher = publisher.into();
        negative.launch_arguments = Some(args.into());
        assert_eq!(catalog.classify(&negative, Platform::Windows), None);
    }
    app.launch_arguments = None;
    assert_eq!(catalog.classify(&app, Platform::Windows), None);
}

#[test]
fn jetbrains_requires_verified_product_and_publisher_and_one_ide_stays_flat() {
    let catalog = Catalog::bundled(None);
    let mut idea = entry(
        "idea",
        "IntelliJ IDEA",
        "C:/JetBrains/bin/idea64.exe",
        Some(""),
        "IntelliJ IDEA",
    );
    idea.publisher = "JetBrains s.r.o.".into();
    assert!(matches!(
        catalog.classify(&idea, Platform::Windows),
        Some(initial_layout::catalog::Classification::Product {
            preset: "jetbrains-ides",
            ..
        })
    ));
    assert!(generate(vec![idea.clone()], Platform::Windows)
        .folders
        .is_empty());
    idea.publisher = "Another publisher".into();
    assert_eq!(catalog.classify(&idea, Platform::Windows), None);
    idea.publisher = "JetBrains s.r.o.".into();
    idea.identifier = "Unknown Product".into();
    assert_eq!(catalog.classify(&idea, Platform::Windows), None);
}

#[test]
fn mac_office_uses_exact_bundle_ids_not_localized_names_or_publisher() {
    let word = entry(
        "word",
        "ワード",
        "/Applications/Word.app",
        None,
        "com.microsoft.Word",
    );
    let excel = entry(
        "excel",
        "Excel",
        "/Applications/Excel.app",
        None,
        "com.microsoft.Excel",
    );
    let unknown = entry(
        "other",
        "Microsoft Excel",
        "/Applications/Other.app",
        None,
        "unknown.id",
    );
    let mut duplicate = word.clone();
    duplicate.app_id = AppId::from_normalized("word-copy");
    assert!(generate(vec![word.clone(), duplicate], Platform::Macos)
        .folders
        .is_empty());
    let state = generate(vec![word, excel, unknown], Platform::Macos);
    assert_eq!(state.folders.len(), 1);
    assert_eq!(state.top_level_app_ids().count(), 1);
}

#[test]
fn deterministic_membership_order_and_localization_preserve_app_set() {
    let entries = vec![
        steam("1", "Zebra"),
        steam("2", "abc"),
        entry("a", "Alpha", "", None, ""),
        entry("b", "Beta", "", None, ""),
    ];
    let first = generate(entries.clone(), Platform::Windows);
    let second = generate(entries.into_iter().rev().collect(), Platform::Windows);
    assert_eq!(first, second);
    assert_eq!(
        first.folders.values().next().unwrap().children[0].as_str(),
        "steam:2"
    );
    assert_eq!(
        first.items.last().unwrap().as_folder_id(),
        first.folders.keys().next()
    );
    let snapshot = map(vec![steam("1", "X"), steam("2", "Y")]);
    let ja = initial_layout::generate(
        &snapshot,
        &Settings::default(),
        Platform::Windows,
        true,
        "ja".into(),
        &Catalog::bundled(None),
    );
    assert_eq!(
        ja.state.folders.values().next().unwrap().name,
        "Steamライブラリ"
    );
    let ids = all_ids(&first);
    assert_eq!(ids.len(), ids.iter().collect::<BTreeSet<_>>().len());
}

#[test]
fn steam_display_filter_precedence_and_conflicts() {
    let mut creative = steam("123", "Word");
    creative.identifier = "com.microsoft.Word".into();
    let settings = Settings {
        show_steam_apps: false,
        ..Settings::default()
    };
    let snapshot = map(vec![creative.clone(), steam("456", "Another")]);
    let catalog = Catalog::bundled(None);
    assert!(initial_layout::generate(
        &snapshot,
        &settings,
        Platform::Macos,
        false,
        "hidden".into(),
        &catalog
    )
    .state
    .folders
    .is_empty());
    let result = initial_layout::generate(
        &snapshot,
        &Settings::default(),
        Platform::Macos,
        false,
        "visible".into(),
        &catalog,
    );
    assert_eq!(result.state.folders.len(), 1);
    assert_eq!(
        result.decisions[&creative.app_id],
        Decision::Grouped("steam-library".into())
    );
    let mut catalog = Catalog::bundled(None);
    catalog
        .presets
        .iter_mut()
        .find(|p| p.id == "apple-documents")
        .unwrap()
        .rules
        .push(Rule::MacBundle {
            bundle: "com.microsoft.Word",
            product: "word",
        });
    let word = entry(
        "word",
        "Word",
        "/Applications/Word.app",
        None,
        "com.microsoft.Word",
    );
    assert!(catalog.classify(&word, Platform::Macos).is_none());
    let toolbox = entry(
        "toolbox",
        "IntelliJ IDEA",
        "C:/JetBrains/jetbrains-toolbox.exe",
        Some(""),
        "IntelliJ IDEA",
    );
    assert_eq!(catalog.classify(&toolbox, Platform::Windows), None);
}

#[test]
fn undo_is_persistent_local_and_rejected_after_edit_or_discovery_change() {
    let initial = generate(vec![steam("1", "A"), steam("2", "B")], Platform::Windows);
    assert!(initial.initial_undo_available());
    let mut restored: LauncherState =
        serde_json::from_slice(&serde_json::to_vec(&initial).unwrap()).unwrap();
    assert!(restored.undo_initial_layout());
    assert!(restored.folders.is_empty());
    assert_eq!(all_ids(&restored).len(), 2);
    assert_eq!(
        restored.initial_layout.unwrap().status,
        InitializationStatus::Undone
    );
    let mut edited = initial.clone();
    edited.folders.values_mut().next().unwrap().name = "Personal".into();
    assert!(!edited.undo_initial_layout());
    let mut corrupt = initial.clone();
    corrupt
        .initial_layout
        .as_mut()
        .unwrap()
        .undo
        .as_mut()
        .unwrap()
        .before
        .pop();
    let unchanged = corrupt.clone();
    assert!(!corrupt.undo_initial_layout());
    assert_eq!(corrupt, unchanged);
    let mut added = initial;
    added.layout_changed();
    assert!(!added.undo_initial_layout());
}

#[test]
fn settings_reset_regenerates_layout_with_a_new_revision_and_valid_undo() {
    let snapshot = map(vec![steam("1", "B"), steam("2", "A")]);
    let state = initial_layout::reset(
        &snapshot,
        Platform::Windows,
        false,
        42,
        &Catalog::bundled(None),
    );
    assert_eq!(state.layout_revision, 43);
    assert_eq!(state.folders.len(), 1);
    assert!(state.hidden_apps.is_empty());
    assert!(state.initial_undo_available());
    let mut undone = state.clone();
    assert!(undone.undo_initial_layout());
    let repeated = initial_layout::reset(
        &snapshot,
        Platform::Windows,
        false,
        undone.layout_revision,
        &Catalog::bundled(None),
    );
    assert_eq!(repeated.folders, state.folders);
    assert!(repeated.layout_revision > undone.layout_revision);
    assert!(repeated.initial_undo_available());
    let empty = initial_layout::reset(
        &BTreeMap::new(),
        Platform::Windows,
        false,
        45,
        &Catalog::bundled(None),
    );
    assert_eq!(
        empty.initial_layout.unwrap().status,
        InitializationStatus::CompletedFlat
    );
}

#[test]
fn late_apps_append_and_sorting_never_dissolves_folders() {
    let mut state = generate(vec![steam("1", "C"), steam("2", "D")], Platform::Windows);
    let folders = state.folders.clone();
    let before = state.items.clone();
    let ids = ["steam:1", "steam:2", "new-z", "new-a"]
        .map(AppId::from_normalized)
        .into_iter()
        .collect();
    state.integrate_discovered_apps(&ids, |id| Some(id.to_string()));
    assert_eq!(state.items[0], before[0]);
    assert_eq!(state.items[1].as_app_id().unwrap().as_str(), "new-a");
    initial_layout::sort_items(&mut state, |id| Some(id.to_string()));
    assert_eq!(state.folders, folders);
}

#[test]
fn fifty_two_hundred_thousand_apps_remain_complete_and_measure_calculation() {
    for n in [50, 200, 1000] {
        let snapshot = map((0..n)
            .map(|i| steam(&i.to_string(), &format!("App {i}")))
            .collect());
        let start = std::time::Instant::now();
        let proposal = initial_layout::generate(
            &snapshot,
            &Settings::default(),
            Platform::Windows,
            false,
            "benchmark".into(),
            &Catalog::bundled(None),
        );
        eprintln!("initial-layout {n} apps: {:?}", start.elapsed());
        assert_eq!(
            all_ids(&proposal.state)
                .into_iter()
                .collect::<BTreeSet<_>>(),
            snapshot.keys().cloned().collect()
        );
        assert_eq!(proposal.state.folders.len(), 1);
    }
}

#[test]
fn original_nineteen_and_added_catalog_presets_have_active_rules() {
    let catalog = Catalog::bundled(None);
    assert_eq!(
        catalog
            .presets
            .iter()
            .map(|p| p.id)
            .collect::<BTreeSet<_>>()
            .len(),
        30
    );
    for preset in catalog.presets {
        assert!(!preset.rules.is_empty(), "{} has no rules", preset.id);
        assert!(!preset.evidence.starts_with("Unverified:"));
    }
}

#[test]
fn every_windows_product_suite_groups_on_reset_and_rejects_helpers_and_versions_only() {
    let cases = [
        (
            "affinity-legacy",
            "Affinity/Designer/Designer.exe",
            "Affinity/Photo/Photo.exe",
        ),
        (
            "coreldraw-suite",
            "Corel/CorelDRAW Graphics Suite 2025/Programs64/CorelDRW.exe",
            "Corel/CorelDRAW Graphics Suite 2025/Programs64/CorelPP.exe",
        ),
        (
            "cyberlink-director-suite",
            "CyberLink/PowerDirector/PDR.exe",
            "CyberLink/PhotoDirector/PhotoDirector.exe",
        ),
        (
            "blackmagic-creative",
            "Blackmagic Design/DaVinci Resolve/Resolve.exe",
            "Blackmagic Design/Fusion 20/Fusion.exe",
        ),
        (
            "maxon-creative",
            "Maxon Cinema 4D 2026/Cinema 4D.exe",
            "Maxon ZBrush 2026/ZBrush.exe",
        ),
        (
            "steinberg-creative",
            "Steinberg/Cubase 14/Cubase14.exe",
            "Steinberg/Dorico 6/Dorico.exe",
        ),
        (
            "native-instruments-standalone",
            "Native Instruments/Kontakt 8/Kontakt 8.exe",
            "Native Instruments/Reaktor 6/Reaktor 6.exe",
        ),
        (
            "autodesk-creative",
            "Autodesk/AutoCAD 2026/acad.exe",
            "Autodesk/Maya2026/bin/maya.exe",
        ),
    ];
    let catalog = Catalog::bundled(None);
    for (preset, a, b) in cases {
        let first = entry(
            "a",
            "Renamed application",
            &format!("C:/Program Files/{a}"),
            Some(""),
            "",
        );
        let second = entry(
            "b",
            "任意の名前",
            &format!("C:/Program Files/{b}"),
            Some(""),
            "",
        );
        for app in [&first, &second] {
            assert!(
                matches!(catalog.classify(app, Platform::Windows), Some(initial_layout::catalog::Classification::Product { preset: p, .. }) if p == preset),
                "{preset}: {}",
                app.target_path
            );
            assert!(catalog.classify(app, Platform::Macos).is_none());
            let mut helper = app.clone();
            helper.target_path = helper.target_path.replace(".exe", " Helper.exe");
            assert!(catalog.classify(&helper, Platform::Windows).is_none());
            helper.target_path = format!(
                "C:/Unrelated/{}",
                app.target_path.rsplit('/').next().unwrap()
            );
            assert!(catalog.classify(&helper, Platform::Windows).is_none());
            helper = app.clone();
            helper.launch_arguments = Some("--update".into());
            assert!(catalog.classify(&helper, Platform::Windows).is_none());
        }
        let state = initial_layout::reset(
            &map(vec![first.clone(), second]),
            Platform::Windows,
            false,
            3,
            &catalog,
        );
        assert_eq!(state.folders.len(), 1, "{preset}");
        assert_eq!(all_ids(&state).len(), 2);
        assert!(state.initial_undo_available());
        let mut other_version = first.clone();
        other_version.app_id = AppId::from_normalized("older-version");
        other_version.target_path = other_version.target_path.replace("C:/", "D:/");
        assert!(
            generate(vec![first, other_version], Platform::Windows)
                .folders
                .is_empty(),
            "{preset}"
        );
    }
}

#[test]
fn all_mac_suites_use_product_ids_and_reset_preserves_every_app() {
    let cases = [
        (
            "macos-utilities",
            "com.apple.ActivityMonitor",
            "com.apple.Console",
        ),
        (
            "microsoft-office",
            "com.microsoft.Word",
            "com.microsoft.Excel",
        ),
        (
            "apple-documents",
            "com.apple.iWork.Pages",
            "com.apple.Numbers",
        ),
        (
            "adobe-creative-cloud",
            "com.adobe.Photoshop",
            "com.adobe.PremierePro.25",
        ),
        (
            "affinity-legacy",
            "com.seriflabs.affinitydesigner2",
            "com.seriflabs.affinityphoto2",
        ),
        (
            "coreldraw-suite",
            "com.corel.coreldrawsuite.2021.coreldraw",
            "com.corel.coreldrawsuite.2021.photopaint",
        ),
        (
            "cyberlink-director-suite",
            "com.cyberlink.powerdirectormac",
            "com.cyberlink.photodirectormac",
        ),
        (
            "final-cut-suite",
            "com.apple.FinalCut",
            "com.apple.motionapp",
        ),
        ("logic-suite", "com.apple.logic10", "com.apple.mainstage3"),
        (
            "blackmagic-creative",
            "com.blackmagic-design.DaVinciResolve",
            "com.blackmagic-design.fusion",
        ),
        (
            "maxon-creative",
            "net.maxon.cinema4d",
            "com.pixologic.zbrush",
        ),
        (
            "steinberg-creative",
            "com.steinberg.cubase14",
            "com.steinberg.dorico6",
        ),
        (
            "native-instruments-standalone",
            "com.native-instruments.Kontakt 8",
            "com.native-instruments.Reaktor 6",
        ),
        (
            "jetbrains-ides",
            "com.jetbrains.intellij",
            "com.jetbrains.pycharm",
        ),
        (
            "autodesk-creative",
            "com.autodesk.AutoCAD2025",
            "com.autodesk.Maya.2024",
        ),
    ];
    let catalog = Catalog::bundled(None);
    for (preset, a, b) in cases {
        let first = entry("a", "任意の名前", "/Applications/First.app", None, a);
        let second = entry("b", "Renamed", "/Applications/Second.app", None, b);
        for app in [&first, &second] {
            assert!(
                matches!(catalog.classify(app, Platform::Macos), Some(initial_layout::catalog::Classification::Product { preset: p, .. }) if p == preset),
                "{preset}: {}",
                app.identifier
            );
            assert!(catalog.classify(app, Platform::Windows).is_none());
            let mut helper = app.clone();
            helper.identifier.push_str(".helper");
            assert!(catalog.classify(&helper, Platform::Macos).is_none());
        }
        let state = initial_layout::reset(
            &map(vec![first.clone(), second]),
            Platform::Macos,
            false,
            2,
            &catalog,
        );
        assert_eq!(state.folders.len(), 1, "{preset}");
        assert_eq!(all_ids(&state).len(), 2);
        let mut duplicate = first.clone();
        duplicate.app_id = AppId::from_normalized("duplicate");
        assert!(generate(vec![first, duplicate], Platform::Macos)
            .folders
            .is_empty());
    }
    for bundle in [
        "org.libreoffice.script",
        "com.seriflabs.affinity",
        "com.apple.Safari",
        "com.apple.garageband10",
        "com.native-instruments.nativeaccess2",
        "com.corel.CDGS2021.stub",
    ] {
        let app = entry("one", "App", "/Applications/App.app", None, bundle);
        assert!(generate(vec![app], Platform::Macos).folders.is_empty());
    }
    for (a, b) in [
        ("com.apple.iWork.Pages", "com.apple.Pages"),
        (
            "com.seriflabs.affinityphoto",
            "com.seriflabs.affinityphoto2",
        ),
        ("com.adobe.PremierePro.24", "com.adobe.PremierePro.25"),
        ("com.steinberg.cubase13", "com.steinberg.cubase14"),
        ("com.autodesk.Maya.2024", "com.autodesk.Maya.2025"),
    ] {
        assert!(generate(
            vec![
                entry("a", "Old", "/Old.app", None, a),
                entry("b", "New", "/New.app", None, b)
            ],
            Platform::Macos
        )
        .folders
        .is_empty());
    }
}

#[test]
fn jetbrains_windows_multiple_products_group() {
    let apps: Vec<_> = [
        ("idea64.exe", "IntelliJ IDEA"),
        ("pycharm64.exe", "PyCharm"),
        ("webstorm64.exe", "WebStorm"),
    ]
    .into_iter()
    .map(|(exe, product)| {
        let mut app = entry(
            exe,
            "Renamed",
            &format!("C:/JetBrains/bin/{exe}"),
            Some(""),
            product,
        );
        app.publisher = "JetBrains s.r.o.".into();
        app
    })
    .collect();
    let state = generate(apps, Platform::Windows);
    assert_eq!(state.folders.len(), 1);
    assert_eq!(state.folders.values().next().unwrap().children.len(), 3);
}

#[test]
fn developer_command_prompt_and_powershell_shortcuts_form_visual_studio_tools() {
    let cmd = entry(
        "cmd",
        "Developer Command Prompt",
        "C:/Windows/System32/cmd.exe",
        Some(
            r#"/k "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\Common7\Tools\VsDevCmd.bat""#,
        ),
        "",
    );
    let ps = entry(
        "ps",
        "Developer PowerShell",
        "C:/Windows/SysWOW64/WindowsPowerShell/v1.0/powershell.exe",
        Some(
            r#"-noe -c "&{Import-Module """C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\Common7\Tools\Microsoft.VisualStudio.DevShell.dll"""; Enter-VsDevShell 6681d841}""#,
        ),
        "",
    );
    assert_eq!(
        generate(vec![cmd, ps.clone()], Platform::Windows)
            .folders
            .len(),
        1
    );
    let catalog = Catalog::bundled(Some("C:/Windows".into()));
    let mut extra_command = ps;
    extra_command
        .launch_arguments
        .as_mut()
        .unwrap()
        .push_str("; arbitrary-command");
    assert!(catalog
        .classify(&extra_command, Platform::Windows)
        .is_none());
}

#[test]
fn remaining_suite_components_and_non_creative_vendor_tools_have_explicit_boundaries() {
    let catalog = Catalog::bundled(None);
    for (preset, path) in [
        ("affinity-legacy", "Affinity/Publisher 2/Publisher.exe"),
        (
            "cyberlink-director-suite",
            "CyberLink/AudioDirector/AudioDirector.exe",
        ),
        (
            "cyberlink-director-suite",
            "CyberLink/ColorDirector/ColorDirector.exe",
        ),
        (
            "steinberg-creative",
            "Steinberg/WaveLab Pro 9.5/WaveLabPro_9_5.exe",
        ),
        (
            "steinberg-creative",
            "Steinberg/WaveLab Pro 12/WaveLabPro12.exe",
        ),
        ("steinberg-creative", "Steinberg/WaveLab 13/WaveLab13.exe"),
        ("autodesk-creative", "Autodesk/3ds Max 2026/3dsmax.exe"),
    ] {
        let app = entry(
            "app",
            "Any name",
            &format!("C:/Program Files/{path}"),
            Some(""),
            "",
        );
        assert!(
            matches!(catalog.classify(&app, Platform::Windows), Some(initial_layout::catalog::Classification::Product { preset: p, .. }) if p == preset),
            "{path}"
        );
    }
    for (preset, bundle) in [
        ("macos-utilities", "com.apple.DiskUtility"),
        ("apple-documents", "com.apple.Keynote"),
        ("adobe-creative-cloud", "com.adobe.Audition.25"),
        ("adobe-creative-cloud", "com.adobe.Audition"),
        ("adobe-creative-cloud", "com.adobe.ame.application.25"),
        ("final-cut-suite", "com.apple.CompressorApp"),
        ("steinberg-creative", "net.steinberg.WaveLab-13"),
    ] {
        let app = entry("app", "Any name", "/Applications/App.app", None, bundle);
        assert!(
            matches!(catalog.classify(&app, Platform::Macos), Some(initial_layout::catalog::Classification::Product { preset: p, .. }) if p == preset),
            "{bundle}"
        );
    }
    for path in [
        "Native Instruments/Native Access/Native Access.exe",
        "Native Instruments/Reaktor 6/Reaktor 6.dll",
        "Blackmagic Design/Desktop Video/DesktopVideoSetup.exe",
        "Corel/Painter 2026/Painter.exe",
        "Autodesk/Autodesk Access/AdskAccessUIHost.exe",
        "Steinberg/Download Assistant/Steinberg Download Assistant.exe",
    ] {
        let app = entry(
            "app",
            "Any name",
            &format!("C:/Program Files/{path}"),
            Some(""),
            "",
        );
        assert!(
            !matches!(
                catalog.classify(&app, Platform::Windows),
                Some(initial_layout::catalog::Classification::Product { .. })
            ),
            "{path}"
        );
    }
}

#[test]
fn libreoffice_component_launchers_and_soffice_switches_share_product_keys() {
    let writer = entry(
        "writer",
        "Writer",
        "C:/Program Files/LibreOffice/program/swriter.exe",
        Some(""),
        "",
    );
    let writer_switch = entry(
        "writer-switch",
        "Writer",
        "C:/Program Files/LibreOffice/program/soffice.exe",
        Some("--writer"),
        "",
    );
    assert!(generate(
        vec![writer.clone(), writer_switch.clone()],
        Platform::Windows
    )
    .folders
    .is_empty());
    let calc = entry(
        "calc",
        "Calc",
        "C:/Program Files/LibreOffice/program/scalc.exe",
        Some(""),
        "",
    );
    let state = generate(vec![writer, writer_switch, calc], Platform::Windows);
    assert_eq!(state.folders.len(), 1);
    assert_eq!(state.folders.values().next().unwrap().children.len(), 3);
}

#[test]
fn toolbox_and_creative_cloud_managers_join_their_products() {
    let mut idea = entry(
        "idea",
        "任意の名前",
        "C:/JetBrains/bin/idea64.exe",
        Some(""),
        "IntelliJ IDEA",
    );
    idea.publisher = "JetBrains s.r.o.".into();
    let mut toolbox = entry(
        "toolbox",
        "管理",
        "C:/JetBrains/Toolbox/bin/jetbrains-toolbox.exe",
        Some(""),
        "JetBrains Toolbox",
    );
    toolbox.publisher = "JetBrains".into();
    assert_eq!(
        generate(vec![idea, toolbox], Platform::Windows)
            .folders
            .values()
            .next()
            .unwrap()
            .children
            .len(),
        2
    );
    assert_eq!(
        generate(
            vec![
                entry("idea", "IDE", "/Idea.app", None, "com.jetbrains.intellij"),
                entry(
                    "toolbox",
                    "Toolbox",
                    "/Toolbox.app",
                    None,
                    "com.jetbrains.toolbox"
                )
            ],
            Platform::Macos
        )
        .folders
        .len(),
        1
    );
    let mut photoshop = entry(
        "ps",
        "PS",
        "C:/Adobe/Photoshop.exe",
        Some(""),
        "Adobe Photoshop 2026",
    );
    photoshop.publisher = "Adobe".into();
    let mut cloud = entry(
        "cc",
        "CC",
        "C:/Adobe/Creative Cloud.exe",
        Some(""),
        "Creative Cloud Desktop",
    );
    cloud.publisher = "Adobe Inc.".into();
    assert_eq!(
        generate(vec![photoshop, cloud], Platform::Windows)
            .folders
            .len(),
        1
    );
}

#[test]
fn installed_related_tools_group_without_classifying_arbitrary_launcher_arguments() {
    let cases = [
        (
            "Git",
            "C:/Program Files/Git/git-bash.exe",
            "--cd-to-home",
            "C:/Program Files/Git/cmd/git-gui.exe",
            "",
        ),
        (
            "Unity",
            "C:/Program Files/Unity/Hub/Editor/6000.4.0f1/Editor/Unity.exe",
            "",
            "C:/Program Files/Unity Hub/Unity Hub.exe",
            "",
        ),
        (
            "Live2D Cubism",
            "C:/Program Files/Live2D Cubism 5.3/CubismEditor5.exe",
            "",
            "C:/Program Files/Live2D Cubism 5.3/CubismViewer5.exe",
            "",
        ),
        (
            "Razer",
            "C:/Program Files/Razer/Razer Cortex/RazerCortex.exe",
            "",
            "C:/Program Files/Razer/RazerAppEngine/RazerAppEngine.exe",
            "--url-params=apps=synapse ",
        ),
        (
            "Maxon",
            "C:/Program Files/Maxon Cinema 4D 2026/Cinema 4D.exe",
            "",
            "C:/Program Files/Maxon Cinema 4D 2026/Cinema 4D Team Render Client.exe",
            "",
        ),
        (
            "Accessibility",
            "C:/Windows/System32/osk.exe",
            "",
            "C:/Windows/System32/narrator.exe",
            "",
        ),
        (
            "Windows Accessories",
            "C:/Windows/System32/taskmgr.exe",
            "/7",
            "C:/Windows/System32/WindowsPowerShell/v1.0/powershell_ise.exe",
            "",
        ),
    ];
    let catalog = Catalog::bundled(Some("C:/Windows".into()));
    for (folder, a, a_args, b, b_args) in cases {
        let first = entry("a", "A", a, Some(a_args), "");
        let second = entry("b", "B", b, Some(b_args), "");
        let snapshot = map(vec![first.clone(), second.clone()]);
        let mut state = initial_layout::reset(&snapshot, Platform::Windows, false, 2, &catalog);
        assert_eq!(state.folders.values().next().unwrap().name, folder);
        assert_eq!(all_ids(&state).len(), 2);
        assert!(state.undo_initial_layout());
        assert_eq!(state.top_level_app_ids().count(), 2);
        let mut other_version = first.clone();
        other_version.app_id = AppId::from_normalized("version");
        other_version.target_path = other_version.target_path.replace("C:/", "D:/");
        assert!(generate(vec![first, other_version], Platform::Windows)
            .folders
            .is_empty());
        let mut unknown_mode = second;
        unknown_mode.launch_arguments = Some("--unrelated-command".into());
        assert!(
            catalog.classify(&unknown_mode, Platform::Windows).is_none(),
            "{folder}"
        );
    }
}

#[test]
fn approved_category_names_and_explicit_launch_routes() {
    let catalog = Catalog::bundled(Some("C:/Windows".into()));
    for (id, name) in [
        ("vr-tools", "VRツール"),
        ("vr-games", "VR Games"),
        ("remote-desktop", "リモートデスクトップ"),
        ("windows-dev-tools", "Windows Dev tools"),
        ("omen", "OMEN"),
    ] {
        assert_eq!(
            catalog
                .presets
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .name(true),
            name
        );
    }
    for (group, target, args) in [
        (
            "vr-tools",
            "C:/Program Files/Meta Horizon/Support/oculus-client/Client.exe",
            "",
        ),
        (
            "vr-tools",
            "C:/Program Files (x86)/SlimeVR Server/SlimeVR.exe",
            "",
        ),
        (
            "vr-tools",
            "C:/Program Files/Virtual Desktop Streamer/VirtualDesktop.Streamer.exe",
            "",
        ),
        (
            "vr-tools",
            "C:/Users/me/Programs/VRChat Creator Companion/CreatorCompanion.exe",
            "",
        ),
        ("remote-desktop", "C:/Program Files/Parsec/parsecd.exe", ""),
        (
            "remote-desktop",
            "C:/Users/me/Paperspace/Paperspace.exe",
            "",
        ),
        (
            "remote-desktop",
            "C:/Program Files/Google/Chrome/Application/chrome_proxy.exe",
            " --profile-directory=Default --app-id=cmkncekebbebpfilplodngbpllndjkfo",
        ),
        ("remote-desktop", "C:/Windows/system32/mstsc.exe", ""),
        (
            "windows-dev-tools",
            "C:/Program Files (x86)/Microsoft Visual Studio/Installer/setup.exe",
            "",
        ),
        (
            "windows-dev-tools",
            "C:/Program Files (x86)/Windows Kits/10/App Certification Kit/appcertui.exe",
            "",
        ),
        (
            "windows-dev-tools",
            "C:/Windows/explorer.exe",
            r#""D:\Program Files (x86)\Windows Kits\10\""#,
        ),
        ("windows-dev-tools", "C:/Windows/system32/appverif.exe", ""),
    ] {
        let app = entry("app", "Unrelated label", target, Some(args), "");
        assert!(
            matches!(catalog.classify(&app, Platform::Windows), Some(initial_layout::catalog::Classification::Product { preset, .. }) if preset == group),
            "{target}"
        );
        assert!(catalog.classify(&app, Platform::Macos).is_none());
        let mut unknown = app.clone();
        unknown.launch_arguments = None;
        assert!(catalog.classify(&unknown, Platform::Windows).is_none());
        let mut other_mode = app;
        other_mode.launch_arguments = Some("--unknown-launch-mode".into());
        assert!(catalog.classify(&other_mode, Platform::Windows).is_none());
    }
    for (target, args) in [
        ("C:/Windows/explorer.exe", r#""C:\Users\me\Documents""#),
        (
            "C:/Program Files/Google/Chrome/Application/chrome_proxy.exe",
            "--profile-directory=Default --app-id=other",
        ),
        ("C:/Unrelated/setup.exe", ""),
    ] {
        assert!(catalog
            .classify(
                &entry("app", "Windows SDK", target, Some(args), ""),
                Platform::Windows
            )
            .is_none());
    }
}

#[test]
fn store_categories_use_aumids_and_work_without_executable_metadata() {
    let catalog = Catalog::bundled(None);
    let mut omen = Vec::new();
    for (id, group, aumid) in [
        (
            "audio",
            "omen",
            "AD2F1837.OMENAudioControl_v10z8vjag6ke6!App",
        ),
        (
            "hub",
            "omen",
            "AD2F1837.OMENCommandCenter_v10z8vjag6ke6!AD2F1837.OMENCommandCenter",
        ),
        (
            "windbg",
            "windows-dev-tools",
            "Microsoft.WinDbg_8wekyb3d8bbwe!Microsoft.WinDbg",
        ),
        (
            "devhome",
            "windows-dev-tools",
            "Microsoft.Windows.DevHome_8wekyb3d8bbwe!App",
        ),
    ] {
        let mut app = entry(id, "Unrelated label", "", None, "");
        app.link_path = format!("shell:AppsFolder\\{aumid}");
        assert!(
            matches!(catalog.classify(&app, Platform::Windows), Some(initial_layout::catalog::Classification::Product { preset, .. }) if preset == group)
        );
        assert!(catalog.classify(&app, Platform::Macos).is_none());
        if group == "omen" {
            omen.push(app.clone());
        }
        app.link_path.push_str("Fake");
        assert!(catalog.classify(&app, Platform::Windows).is_none());
    }
    let grouped = generate(omen.clone(), Platform::Windows);
    assert_eq!(grouped.folders.len(), 1);
    assert_eq!(grouped.folders.values().next().unwrap().name, "OMEN");
    let mut duplicate = omen[0].clone();
    duplicate.app_id = AppId::from_normalized("other-shortcut");
    assert!(generate(vec![omen.remove(0), duplicate], Platform::Windows)
        .folders
        .is_empty());
    assert!(catalog
        .classify(
            &entry("fake", "OMEN Gaming Hub", "", None, ""),
            Platform::Windows
        )
        .is_none());
}

#[test]
fn vr_tools_and_games_override_only_known_steam_ids_and_obey_visibility() {
    let apps = vec![
        steam("250820", "SteamVR"),
        steam("1173510", "XSOverlay"),
        steam("438100", "VRChat"),
        steam("620980", "Beat Saber"),
        steam("546560", "Half-Life: Alyx"),
        steam("3301060", "Desktop Mate"),
        steam("99999999", "Unknown VR game"),
    ];
    let state = generate(apps.clone(), Platform::Windows);
    assert_eq!(state.folders.len(), 3);
    let counts: BTreeMap<_, _> = state
        .folders
        .values()
        .map(|f| (f.name.as_str(), f.children.len()))
        .collect();
    assert_eq!(counts["VR Tools"], 2);
    assert_eq!(counts["VR Games"], 3);
    assert_eq!(counts["Steam Library"], 2);
    assert_eq!(all_ids(&state).len(), apps.len());
    assert!(generate(vec![steam("438100", "VRChat")], Platform::Windows)
        .folders
        .is_empty());
    let settings = Settings {
        show_steam_apps: false,
        ..Default::default()
    };
    let hidden = initial_layout::generate(
        &map(apps),
        &settings,
        Platform::Windows,
        true,
        "test".into(),
        &Catalog::bundled(None),
    );
    assert!(hidden.state.folders.is_empty());
    assert!(hidden
        .state
        .top_level_app_ids()
        .all(|id| !settings.shows_app(id)));
    assert!(hidden
        .decisions
        .values()
        .all(|d| *d == Decision::KeepTopLevel));
    assert_eq!(all_ids(&hidden.state).len(), 7);
}

#[test]
fn canonical_catalog_initialization_preserves_existing_and_partial_layouts() {
    let snapshot = map(vec![
        steam("250820", "SteamVR"),
        steam("1173510", "XSOverlay"),
    ]);
    let settings = Settings::default();
    let catalog = Catalog::bundled(None);
    let fresh = initial_layout::prepare_catalog_layout(
        LauncherState::new(),
        &snapshot,
        &settings,
        true,
        true,
        true,
        &catalog,
    );
    assert_eq!(fresh.folders.len(), 1);
    assert!(fresh.initial_layout.as_ref().unwrap().undo.is_some());
    let again = initial_layout::prepare_catalog_layout(
        fresh.clone(),
        &snapshot,
        &settings,
        true,
        true,
        true,
        &catalog,
    );
    assert_eq!(again, fresh);
    let mut undo = fresh.clone();
    assert!(undo.undo_initial_layout());
    assert_eq!(undo.top_level_app_ids().count(), 2);
    let partial = initial_layout::prepare_catalog_layout(
        LauncherState::new(),
        &snapshot,
        &settings,
        true,
        false,
        true,
        &catalog,
    );
    assert!(partial.folders.is_empty());
    assert_eq!(
        partial.initial_layout.unwrap().status,
        InitializationStatus::SkippedError
    );
    let existing = LauncherState::from_legacy(
        vec![
            AppId::from_normalized("steam:1173510"),
            AppId::from_normalized("steam:250820"),
        ],
        vec![],
    );
    let kept = initial_layout::prepare_catalog_layout(
        existing.clone(),
        &snapshot,
        &settings,
        false,
        true,
        true,
        &catalog,
    );
    assert_eq!(kept.items, existing.items);
    assert!(kept.folders.is_empty());
    assert_eq!(
        kept.initial_layout.unwrap().status,
        InitializationStatus::PreservedExisting
    );
}
