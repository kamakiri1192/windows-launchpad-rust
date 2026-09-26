//! Initial placement only. The shared alphabetical sorter and saved user order stay independent.
use super::{
    catalog::{self, Catalog, Classification, Platform},
    *,
};

type Rank = (u8, u16);
const GENERAL: Rank = (2, 0);
const SUPPORT: Rank = (3, 0);

pub(super) fn apply(
    state: &mut LauncherState,
    snapshot: &BTreeMap<AppId, SnapshotEntry>,
    platform: Platform,
    catalog: &Catalog,
    folder_presets: &BTreeMap<FolderId, String>,
    settings: &Settings,
) {
    if platform != Platform::Windows {
        return;
    }
    let placements: BTreeMap<_, _> = snapshot
        .iter()
        .map(|(id, e)| {
            (
                id.clone(),
                if settings.shows_app(id) {
                    app_placement(e, catalog)
                } else {
                    (GENERAL, id.to_string())
                },
            )
        })
        .collect();
    // Promote one representative of each product; versions do not consume every early cell.
    let mut representatives: BTreeMap<&str, &AppId> = BTreeMap::new();
    for id in state.top_level_app_ids() {
        let Some((rank, product)) = placements.get(id) else {
            continue;
        };
        if rank.0 >= 2 {
            continue;
        }
        representatives
            .entry(product)
            .and_modify(|chosen| {
                let version = stable_version(&snapshot[id].version);
                let previous = stable_version(&snapshot[*chosen].version);
                if version > previous || (version == previous && id < *chosen) {
                    *chosen = id;
                }
            })
            .or_insert(id);
    }
    let promoted: BTreeSet<_> = representatives.values().map(|id| (*id).clone()).collect();
    for (folder_id, folder) in &mut state.folders {
        if folder_presets
            .get(folder_id)
            .is_some_and(|p| matches!(p.as_str(), "windows-accessories" | "windows-tools"))
        {
            folder.children.sort_by_cached_key(|id| {
                let e = &snapshot[id];
                let product = catalog::windows::identify(e, catalog.windows_directory.as_deref());
                (
                    child_rank(product.map(|(product, _)| product)),
                    name_order::key(&e.name, id.as_str()),
                )
            });
        }
    }
    state.items.sort_by_cached_key(|item| {
        let (rank, name) = match item {
            LauncherItem::App(id) => {
                let rank = placements.get(id).map(|p| p.0).unwrap_or(GENERAL);
                let rank = if rank.0 < 2 && !promoted.contains(id) {
                    GENERAL
                } else {
                    rank
                };
                (
                    rank,
                    snapshot
                        .get(id)
                        .map(|e| e.name.as_str())
                        .unwrap_or_default(),
                )
            }
            LauncherItem::Folder(id) => (
                folder_presets
                    .get(id)
                    .map(|p| preset_rank(p))
                    .unwrap_or(GENERAL),
                state
                    .folders
                    .get(id)
                    .map(|f| f.name.as_str())
                    .unwrap_or_default(),
            ),
        };
        (rank, name_order::key(name, &item.stable_key()))
    });
}

fn preset_rank(preset: &str) -> Rank {
    match preset {
        "adobe-creative-cloud" => (1, 0),
        "windows-accessories" => (1, 60),
        "microsoft-office" | "libreoffice" => (1, 70),
        "affinity-legacy"
        | "blackmagic-creative"
        | "coreldraw-suite"
        | "cyberlink-director-suite"
        | "steinberg-creative"
        | "native-instruments-standalone"
        | "autodesk-creative" => (1, 80),
        "unity-tools" => (1, 90),
        "jetbrains-ides" => (1, 100),
        "live2d-cubism" => (1, 110),
        "maxon-creative" => (1, 120),
        "git-tools" | "windows-dev-tools" | "visual-studio-tools" | "razer-tools" | "omen" => {
            SUPPORT
        }
        "windows-tools" => (3, 10),
        _ => GENERAL,
    }
}

fn app_placement(e: &SnapshotEntry, catalog: &Catalog) -> (Rank, String) {
    if let Some(Classification::Product { preset, product }) =
        catalog.classify(e, Platform::Windows)
    {
        return (preset_rank(preset), format!("{preset}:{product}"));
    }
    if let Some((product, _)) = catalog::windows::identify(e, catalog.windows_directory.as_deref())
    {
        let rank = match product {
            "explorer" => (0, 0),
            "store" => (0, 40),
            "settings" => (0, 50),
            "calculator" => (0, 60),
            "chatgpt" => (1, 50),
            "chatgpt-classic" => (1, 51),
            "game-bar" => SUPPORT,
            _ => GENERAL,
        };
        return (rank, product.into());
    }
    let path = e.target_path.replace('/', "\\").to_lowercase();
    let file = path.rsplit('\\').next().unwrap_or_default();
    let Some(args) = e.launch_arguments.as_deref() else {
        return (GENERAL, e.app_id.to_string());
    };
    if file == "vlc.exe"
        && e.identifier == "VLC media player"
        && e.publisher == "VideoLAN"
        && matches!(
            args,
            "-Iskins" | "--reset-config --reset-plugins-cache vlc://quit"
        )
    {
        return (SUPPORT, "vlc-helper".into());
    }
    if !args.is_empty() {
        return (GENERAL, e.app_id.to_string());
    }
    let known = match (file, e.identifier.as_str(), e.publisher.as_str()) {
        ("chrome.exe", "Google Chrome", "Google LLC")
            if path.ends_with(r"\google\chrome\application\chrome.exe") =>
        {
            Some(((0, 10), "chrome"))
        }
        ("msedge.exe", "Microsoft Edge", "Microsoft Corporation")
            if path.ends_with(r"\microsoft\edge\application\msedge.exe") =>
        {
            Some(((0, 20), "edge"))
        }
        ("discord.exe", "Discord", "Discord Inc.") => Some(((0, 30), "discord")),
        ("obs64.exe", "OBS Studio", "OBS") => Some(((1, 10), "obs")),
        ("blender-launcher.exe" | "blender.exe", "Blender", "Blender Foundation") => {
            Some(((1, 20), "blender"))
        }
        ("unrealeditor.exe", "UnrealEditor", "Epic Games, Inc.") => Some(((1, 30), "unreal")),
        ("code.exe", "Visual Studio Code", "Microsoft Corporation") => Some(((1, 40), "vscode")),
        _ => None,
    };
    known
        .map(|(rank, key)| (rank, key.into()))
        .unwrap_or((GENERAL, e.app_id.to_string()))
}

fn child_rank(product: Option<&str>) -> u16 {
    match product {
        Some("notepad" | "security") => 0,
        Some("snipping" | "control-panel") => 1,
        Some("paint" | "backup") => 2,
        Some("clock") => 3,
        Some("sound-recorder") => 4,
        Some("sticky-notes") => 5,
        Some("task-manager") => 6,
        Some("terminal") => 7,
        Some("powershell") => 8,
        Some("command-prompt") => 9,
        Some("run-dialog") => 10,
        Some("get-help" | "feedback") => 100,
        _ => 50,
    }
}

// Only known release formats. Missing/preview metadata falls back to stable IDs.
fn stable_version(value: &str) -> Option<Vec<u64>> {
    let value = if let Some(release) = value.strip_prefix("++UE5+Release-") {
        let (version, build) = release.split_once("-CL-")?;
        if build.is_empty() || !build.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        version
    } else {
        value
    };
    value
        .split('.')
        .map(|part| {
            (!part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        })
        .collect()
}
