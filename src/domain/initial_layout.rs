//! One-shot, offline initial placement. No OS calls or persistence in this module.
pub mod catalog;
pub mod document;
mod order;

use super::{
    app_diff::SnapshotEntry,
    app_id::AppId,
    folders::{Folder, FolderId},
    launcher_item::LauncherItem,
    launcher_state::LauncherState,
    name_order,
    settings::Settings,
};
use catalog::{Catalog, Platform};
use std::collections::{BTreeMap, BTreeSet};

pub const SPEC_VERSION: u32 = 3;

pub fn japanese_locale(locale: Option<&str>) -> bool {
    locale.is_some_and(|s| {
        s.split(['-', '_'])
            .next()
            .is_some_and(|s| s.eq_ignore_ascii_case("ja"))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitializationStatus {
    Applied,
    CompletedFlat,
    PreservedExisting,
    SkippedError,
    Undone,
    UserReset,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InitialLayoutRecord {
    pub status: InitializationStatus,
    pub spec_version: u32,
    pub catalog_version: u32,
    pub generation: String,
    pub undo: Option<InitialLayoutUndo>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InitialLayoutUndo {
    pub before: Vec<LauncherItem>,
    pub after: Vec<LauncherItem>,
    pub created_folders: BTreeMap<FolderId, Folder>,
    pub applied_revision: u64,
    /// Detection membership is checked again on restart; icon changes are irrelevant.
    pub discovered: BTreeMap<AppId, (String, String, Option<String>)>,
}

impl InitialLayoutRecord {
    pub fn new(status: InitializationStatus, generation: String) -> Self {
        Self {
            status,
            spec_version: SPEC_VERSION,
            catalog_version: catalog::VERSION,
            generation,
            undo: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Grouped(String),
    KeepTopLevel,
    Unresolved,
}

#[derive(Debug)]
pub struct Proposal {
    pub state: LauncherState,
    pub decisions: BTreeMap<AppId, Decision>,
}

pub fn generate(
    snapshot: &BTreeMap<AppId, SnapshotEntry>,
    settings: &Settings,
    platform: Platform,
    japanese: bool,
    generation: String,
    catalog: &Catalog,
) -> Proposal {
    let mut state = LauncherState::new();
    state.customized = true;
    // Include display-filtered apps in the saved flat order, but never in a folder.
    // Toggling their visibility later cannot trigger initial placement again.
    let mut sorted: Vec<_> = snapshot.values().collect();
    sorted.sort_by_key(|e| name_order::key(&e.name, e.app_id.as_str()));
    let before: Vec<_> = sorted
        .iter()
        .map(|e| LauncherItem::App(e.app_id.clone()))
        .collect();
    state.items = before.clone();
    let mut decisions = BTreeMap::new();
    let mut groups: BTreeMap<String, Vec<(&SnapshotEntry, String)>> = BTreeMap::new();
    for entry in &sorted {
        let classification = if settings.shows_app(&entry.app_id) {
            catalog.classify(entry, platform)
        } else {
            None
        };
        let decision = match classification {
            Some(catalog::Classification::Product { preset, product }) => {
                groups
                    .entry(preset.to_owned())
                    .or_default()
                    .push((entry, product));
                Decision::Grouped(preset.to_owned())
            }
            Some(catalog::Classification::KeepTopLevel) => Decision::KeepTopLevel,
            None if !settings.shows_app(&entry.app_id) => Decision::KeepTopLevel,
            None => Decision::Unresolved,
        };
        decisions.insert(entry.app_id.clone(), decision);
    }
    let mut folder_presets = BTreeMap::new();
    for (preset_id, entries) in groups {
        let products: BTreeSet<_> = entries
            .iter()
            .filter(|(e, _)| launch_identity(e, platform).is_some())
            .map(|(_, p)| p)
            .collect();
        let launches: BTreeSet<_> = entries
            .iter()
            .filter_map(|(e, _)| launch_identity(e, platform))
            .collect();
        if products.len() < 2 || launches.len() < 2 {
            for (e, _) in entries {
                decisions.insert(e.app_id.clone(), Decision::KeepTopLevel);
            }
            continue;
        }
        let id = FolderId::generate(state.next_folder_counter());
        let preset = catalog
            .presets
            .iter()
            .find(|p| p.id == preset_id)
            .expect("classified preset");
        let folder = Folder {
            id: id.clone(),
            name: preset.name(japanese).to_owned(),
            children: entries.iter().map(|(e, _)| e.app_id.clone()).collect(),
        };
        let children: BTreeSet<_> = folder.children.iter().collect();
        state.items.retain(|item| match item {
            LauncherItem::App(a) => !children.contains(a),
            _ => true,
        });
        state.items.push(LauncherItem::Folder(id.clone()));
        folder_presets.insert(id.clone(), preset_id.to_owned());
        state.folders.insert(id, folder);
    }
    sort_items(&mut state, |id| snapshot.get(id).map(|e| e.name.clone()));
    order::apply(
        &mut state,
        snapshot,
        platform,
        catalog,
        &folder_presets,
        settings,
    );
    let status = if state.folders.is_empty() && state.items == before {
        InitializationStatus::CompletedFlat
    } else {
        InitializationStatus::Applied
    };
    let mut record = InitialLayoutRecord::new(status, generation);
    record.catalog_version = catalog.version;
    state.layout_revision = 1;
    if status == InitializationStatus::Applied {
        record.undo = Some(InitialLayoutUndo {
            before,
            after: state.items.clone(),
            created_folders: state.folders.clone(),
            applied_revision: state.layout_revision,
            discovered: discovery_signature(snapshot),
        });
    }
    state.initial_layout = Some(record);
    Proposal { state, decisions }
}

pub fn discovery_signature(
    snapshot: &BTreeMap<AppId, SnapshotEntry>,
) -> BTreeMap<AppId, (String, String, Option<String>)> {
    snapshot
        .iter()
        .map(|(id, e)| {
            (
                id.clone(),
                (
                    e.name.clone(),
                    e.target_path.clone(),
                    e.launch_arguments.clone(),
                ),
            )
        })
        .collect()
}

/// An explicit settings reset reapplies the bundled layout to the current
/// detected apps, even when an automatic first-run scan was incomplete.
pub fn reset(
    snapshot: &BTreeMap<AppId, SnapshotEntry>,
    platform: Platform,
    japanese: bool,
    previous_revision: u64,
    catalog: &Catalog,
) -> LauncherState {
    let revision = previous_revision.saturating_add(1);
    let mut state = generate(
        snapshot,
        &Settings::default(),
        platform,
        japanese,
        format!("reset-{revision}"),
        catalog,
    )
    .state;
    state.layout_revision = revision;
    if let Some(undo) = state
        .initial_layout
        .as_mut()
        .and_then(|record| record.undo.as_mut())
    {
        undo.applied_revision = revision;
    }
    state
}

/// Separate from AppId: keep all shortcuts, count only known distinct launches.
fn launch_identity(e: &SnapshotEntry, platform: Platform) -> Option<(String, String)> {
    if e.app_id.is_steam() {
        return Some((e.app_id.as_str().to_owned(), String::new()));
    }
    match platform {
        Platform::Windows => {
            if let Some(aumid) = catalog::entry_shell_id(e) {
                return Some((format!("shell:AppsFolder\\{aumid}"), String::new()));
            }
            let args = e.launch_arguments.as_ref()?;
            if e.target_path.is_empty() {
                return None;
            }
            Some((
                e.target_path.replace('/', "\\").to_lowercase(),
                args.trim().to_owned(),
            ))
        }
        Platform::Macos => {
            (!e.identifier.is_empty()).then(|| (e.identifier.clone(), String::new()))
        }
    }
}

pub fn sort_items(state: &mut LauncherState, name: impl Fn(&AppId) -> Option<String>) {
    state.items.sort_by_cached_key(|item| {
        let label = match item {
            LauncherItem::App(id) => name(id).unwrap_or_default(),
            LauncherItem::Folder(id) => state
                .folders
                .get(id)
                .map(|f| f.name.clone())
                .unwrap_or_default(),
        };
        name_order::key(&label, &item.stable_key())
    });
    state.customized = true;
}

impl LauncherState {
    pub fn initial_undo_available(&self) -> bool {
        self.initial_layout
            .as_ref()
            .filter(|r| r.status == InitializationStatus::Applied)
            .and_then(|r| r.undo.as_ref())
            .is_some_and(|u| {
                u.applied_revision == self.layout_revision
                    && self.items == u.after
                    && self.folders == u.created_folders
                    && self.hidden_apps.is_empty()
            })
    }

    pub fn undo_initial_layout(&mut self) -> bool {
        if !self.initial_undo_available() {
            return false;
        }
        let undo = self
            .initial_layout
            .as_ref()
            .and_then(|r| r.undo.as_ref())
            .expect("checked undo");
        let before: BTreeSet<_> = undo
            .before
            .iter()
            .filter_map(LauncherItem::as_app_id)
            .collect();
        let current: BTreeSet<_> = self
            .top_level_app_ids()
            .chain(self.folder_child_ids())
            .collect();
        if before.len() != undo.before.len() || before != current {
            return false;
        }
        let record = self.initial_layout.as_mut().expect("checked record");
        let undo = record.undo.take().expect("checked undo");
        self.items = undo.before;
        for id in undo.created_folders.keys() {
            self.folders.remove(id);
        }
        self.layout_revision = self.layout_revision.saturating_add(1);
        self.customized = true;
        record.status = InitializationStatus::Undone;
        true
    }

    pub fn layout_changed(&mut self) {
        self.layout_revision = self.layout_revision.saturating_add(1);
        if let Some(record) = &mut self.initial_layout {
            record.undo = None;
        }
    }
}

/// Apply the one-shot layout to the canonical IDs produced by Windows discovery.
/// Identity aliases and this resulting layout are committed together by the caller.
pub fn prepare_catalog_layout(
    mut existing: LauncherState,
    snapshot: &BTreeMap<AppId, SnapshotEntry>,
    settings: &crate::domain::settings::Settings,
    fresh: bool,
    _complete: bool,
    japanese: bool,
    catalog: &catalog::Catalog,
) -> LauncherState {
    if existing.initial_layout.is_some() {
        // Also freeze flat arrangements saved by earlier versions after a partial scan.
        existing.customized = true;
        return existing;
    }
    let generation = "catalog-initial".to_owned();
    // Source failures remain diagnostics; arrange the apps actually available now.
    if fresh {
        return generate(
            snapshot,
            settings,
            Platform::Windows,
            japanese,
            generation,
            catalog,
        )
        .state;
    }
    existing.initial_layout = Some(InitialLayoutRecord::new(
        InitializationStatus::PreservedExisting,
        generation,
    ));
    existing.customized = true;
    existing
}
