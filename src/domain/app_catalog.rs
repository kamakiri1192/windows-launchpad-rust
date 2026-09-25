//! Application identity, launch selection and durable aliases, independent of Win32.
use super::{
    app_diff::SnapshotEntry, app_id::AppId, launcher_item::LauncherItem,
    launcher_state::LauncherState,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PropertyValue {
    Value(String),
    Missing,
    #[default]
    Unavailable,
}
impl PropertyValue {
    pub fn value(&self) -> Option<&str> {
        if let Self::Value(s) = self {
            Some(s)
        } else {
            None
        }
    }
}
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum Source {
    Shell,
    UserStart,
    #[default]
    SharedStart,
    UserDesktop,
    SharedDesktop,
    Steam,
}
#[derive(Debug, Clone)]
pub struct Candidate {
    pub entry: SnapshotEntry,
    pub identity: Option<String>,
    pub native_key: Option<String>,
    pub exact_key: Option<String>,
    pub source: Source,
    pub normal: bool,
    pub valid: bool,
}
#[derive(Debug, Clone, Default)]
pub struct CatalogScan {
    pub candidates: Vec<Candidate>,
    pub errors: Vec<String>,
    pub failed_sources: BTreeSet<Source>,
    pub failed_locators: BTreeSet<String>,
}
#[derive(Debug, Clone)]
pub struct Group {
    pub candidates: Vec<Candidate>,
    pub keys: BTreeSet<String>,
    pub identity: Option<String>,
}

/// Never let an unidentified executable connect two different explicit IDs.
pub fn group(candidates: Vec<Candidate>) -> Vec<Group> {
    let mut native_ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for c in &candidates {
        if let Some(native) = &c.native_key {
            let ids = native_ids.entry(native.clone()).or_default();
            if let Some(id) = &c.identity {
                ids.insert(id.clone());
            }
        }
    }
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for c in candidates {
        let native = c
            .native_key
            .as_ref()
            .filter(|key| native_ids[*key].len() <= 1);
        let identity = c
            .identity
            .clone()
            .or_else(|| native.and_then(|key| native_ids[key].first().cloned()));
        let key = identity
            .clone()
            .or_else(|| native.cloned())
            .or_else(|| c.exact_key.clone())
            .unwrap_or_else(|| format!("source:{}", c.entry.app_id));
        let g = groups.entry(key).or_insert_with(|| Group {
            candidates: vec![],
            keys: BTreeSet::new(),
            identity: identity.clone(),
        });
        if let Some(key) = identity {
            g.keys.insert(key);
        }
        if let Some(key) = native {
            g.keys.insert(key.clone());
        }
        g.keys.insert(format!("source:{}", c.entry.app_id));
        g.candidates.push(c);
    }
    groups.into_values().collect()
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Binding {
    id: AppId,
    identity: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IdentityAliases {
    #[serde(default)]
    redirects: BTreeMap<AppId, AppId>,
    bindings: BTreeMap<String, Binding>,
    selected: BTreeMap<AppId, String>,
    #[serde(default)]
    selected_sources: BTreeMap<AppId, Source>,
    sequence: u64,
}
impl IdentityAliases {
    /// A temporarily absent ID must not erase a previously observed explicit ID
    /// or make an unidentified native route bridge two known applications.
    pub fn groups(&self, mut candidates: Vec<Candidate>) -> Vec<Group> {
        for c in &mut candidates {
            if c.identity.is_none() {
                c.identity = self
                    .bindings
                    .get(&format!("source:{}", c.entry.app_id))
                    .and_then(|b| b.identity.clone());
            }
        }
        group(candidates)
    }
    pub fn source_of(&self, id: &AppId) -> Option<Source> {
        self.selected_sources.get(id).copied()
    }
    pub fn canonical(&self, id: &AppId) -> AppId {
        let mut current = id;
        for _ in 0..self.redirects.len() {
            let Some(next) = self.redirects.get(current) else {
                break;
            };
            if next == current {
                break;
            }
            current = next;
        }
        current.clone()
    }
    /// Reconcile before diffing. Reuse the earliest user placement during merges,
    /// retain source aliases, and reject source-path reuse by another explicit ID.
    pub fn reconcile(
        &mut self,
        groups: Vec<Group>,
        layout: &mut LauncherState,
    ) -> BTreeMap<AppId, SnapshotEntry> {
        let placement = placement_order(layout);
        let mut redirects = BTreeMap::new();
        let mut result = BTreeMap::new();
        for g in groups {
            let compatible = |b: &&Binding| {
                b.identity.is_none() || g.identity.is_none() || b.identity == g.identity
            };
            let mut prior: BTreeSet<AppId> = g
                .keys
                .iter()
                .filter_map(|key| self.bindings.get(key))
                .filter(compatible)
                .map(|b| b.id.clone())
                .collect();
            for c in &g.candidates {
                let source_key = format!("source:{}", c.entry.app_id);
                if !self.bindings.contains_key(&source_key) && placement.contains(&c.entry.app_id) {
                    prior.insert(c.entry.app_id.clone());
                }
                // Older catalogs treated Shell parsing names as filesystem
                // paths and lowercased them. Adopt that layout reference once,
                // without normalizing the actual AUMID or its new source key.
                let legacy = AppId::from_link_path(&c.entry.link_path);
                if c.source == Source::Shell
                    && self.bindings.get(&source_key).is_none_or(|b| {
                        b.identity.is_none() || g.identity.is_none() || b.identity == g.identity
                    })
                    && !self.redirects.contains_key(&legacy)
                    && placement.contains(&legacy)
                {
                    prior.insert(legacy);
                }
            }
            prior.retain(|id| !result.contains_key(id));
            let id = g
                .identity
                .as_ref()
                .filter(|id| id.starts_with("steam:"))
                .map(|id| AppId::from_normalized(id.clone()))
                .or_else(|| {
                    placement
                        .iter()
                        .find(|id| prior.contains(*id))
                        .cloned()
                        .or_else(|| prior.first().cloned())
                })
                .unwrap_or_else(|| {
                    self.sequence += 1;
                    let epoch = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos();
                    AppId::from_normalized(format!("app:{epoch:x}-{:x}", self.sequence))
                });
            for old in prior {
                if old != id {
                    redirects.insert(old, id.clone());
                }
            }
            for c in &g.candidates {
                let key = format!("source:{}", c.entry.app_id);
                if self.bindings.get(&key).is_none_or(|b| {
                    b.identity.is_none() || g.identity.is_none() || b.identity == g.identity
                }) {
                    redirects.insert(c.entry.app_id.clone(), id.clone());
                }
            }
            let selected = self.selected.get(&id);
            let candidate = g
                .candidates
                .iter()
                .min_by_key(|c| {
                    (
                        !c.valid,
                        !c.normal,
                        c.source,
                        selected.is_none_or(|s| s != &c.entry.link_path),
                        c.entry.app_id.clone(),
                    )
                })
                .expect("nonempty group");
            self.selected_sources.insert(id.clone(), candidate.source);
            self.selected
                .insert(id.clone(), candidate.entry.link_path.clone());
            for key in g.keys {
                self.bindings.insert(
                    key,
                    Binding {
                        id: id.clone(),
                        identity: g.identity.clone(),
                    },
                );
            }
            let mut entry = candidate.entry.clone();
            entry.app_id = id.clone();
            result.insert(id, entry);
        }
        // Rewrite old alias bindings too: an absent route must not resurrect an old ID.
        for binding in self.bindings.values_mut() {
            if let Some(id) = redirects.get(&binding.id) {
                binding.id = id.clone();
            }
        }
        for (old, new) in &redirects {
            if old != new {
                self.redirects.insert(old.clone(), new.clone());
            }
        }
        remap_layout(layout, &redirects);
        result
    }
}

fn placement_order(layout: &LauncherState) -> Vec<AppId> {
    let mut result = Vec::new();
    for item in &layout.items {
        match item {
            LauncherItem::App(id) => result.push(id.clone()),
            LauncherItem::Folder(id) => {
                if let Some(f) = layout.folders.get(id) {
                    result.extend(f.children.clone());
                }
            }
        }
    }
    result.extend(layout.hidden_apps.iter().cloned());
    result
}

pub fn remap_layout(layout: &mut LauncherState, redirects: &BTreeMap<AppId, AppId>) {
    let map = |id: &AppId| redirects.get(id).cloned().unwrap_or_else(|| id.clone());
    layout.hidden_apps = layout.hidden_apps.iter().map(&map).collect();
    let mut seen = layout.hidden_apps.clone();
    layout.items.retain_mut(|item| match item {
        LauncherItem::App(id) => {
            *id = map(id);
            seen.insert(id.clone())
        }
        LauncherItem::Folder(id) => {
            if let Some(f) = layout.folders.get_mut(id) {
                f.children.retain_mut(|child| {
                    *child = map(child);
                    seen.insert(child.clone())
                });
            }
            true
        }
    });
}

/// Keep an absent entry until two complete observations confirm absence. A
/// failed source does not count as absence, and never destroys layout references.
#[derive(Default)]
pub struct Presence {
    missing: BTreeMap<AppId, u8>,
}
impl Presence {
    pub fn retain_transient(
        &mut self,
        old: &BTreeMap<AppId, SnapshotEntry>,
        new: &mut BTreeMap<AppId, SnapshotEntry>,
        complete: bool,
    ) {
        self.retain_observed(old, new, |_| complete);
    }
    pub fn retain_observed(
        &mut self,
        old: &BTreeMap<AppId, SnapshotEntry>,
        new: &mut BTreeMap<AppId, SnapshotEntry>,
        complete_for: impl Fn(&AppId) -> bool,
    ) {
        for (id, entry) in old {
            // An observed source reused by a different app is not transient
            // absence: retaining its old launcher would open the wrong app.
            if new
                .values()
                .any(|next| next.app_id != *id && next.link_path == entry.link_path)
            {
                self.missing.remove(id);
                continue;
            }
            let complete = complete_for(id);
            if new.contains_key(id) {
                self.missing.remove(id);
                continue;
            }
            let count = self.missing.entry(id.clone()).or_default();
            if complete {
                *count = count.saturating_add(1);
            }
            if !complete || *count < 2 {
                new.insert(id.clone(), entry.clone());
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::folders::{Folder, FolderId};
    #[test]
    fn measured_windows_fixture_has_one_tile_and_shell_launch_for_discord() {
        #[derive(serde::Deserialize)]
        struct Row {
            name: String,
            source: Source,
            locator: String,
            identity: Option<String>,
            native_key: Option<String>,
            target: String,
            normal: bool,
            valid: bool,
        }
        let rows: Vec<Row> = serde_json::from_str(include_str!(
            "../../tests/fixtures/windows_catalog_observed.json"
        ))
        .unwrap();
        let candidates = rows
            .into_iter()
            .map(|r| {
                let mut item = c(&r.locator, r.identity.as_deref(), r.native_key.as_deref());
                item.entry.name = r.name;
                item.entry.target_path = r.target;
                item.source = r.source;
                item.normal = r.normal;
                item.valid = r.valid;
                item
            })
            .collect();
        let result = scan(
            &mut IdentityAliases::default(),
            &mut LauncherState::default(),
            candidates,
        );
        for name in [
            "Discord",
            "VRChat Creator Companion",
            "Unreal Engine",
            "Steam",
            "FocuSee",
            "Fork",
            "Git Bash",
            "Git CMD",
            "Git GUI",
        ] {
            assert_eq!(
                result.values().filter(|e| e.name == name).count(),
                1,
                "{name}"
            );
        }
        assert_eq!(
            result
                .values()
                .find(|e| e.name == "Discord")
                .unwrap()
                .link_path,
            "shell:AppsFolder\\com.squirrel.Discord.Discord"
        );
        assert!(result
            .values()
            .find(|e| e.name == "Unreal Engine")
            .unwrap()
            .link_path
            .contains("Desktop"));
    }

    #[test]
    fn previously_unidentified_copies_can_split_into_distinct_ids_without_losing_apps() {
        let mut aliases = IdentityAliases::default();
        let mut layout = LauncherState::default();
        assert_eq!(
            scan(
                &mut aliases,
                &mut layout,
                vec![
                    c("a", None, Some("native:app")),
                    c("b", None, Some("native:app"))
                ]
            )
            .len(),
            1
        );
        let result = scan(
            &mut aliases,
            &mut layout,
            vec![
                c("a", Some("aumid:A"), Some("native:app")),
                c("b", Some("aumid:B"), Some("native:app")),
            ],
        );
        assert_eq!(result.len(), 2);
        let missing_properties = scan(
            &mut aliases,
            &mut layout,
            vec![
                c("a", None, Some("native:app")),
                c("b", None, Some("native:app")),
            ],
        );
        assert_eq!(missing_properties.len(), 2);
    }
    fn c(source: &str, identity: Option<&str>, native: Option<&str>) -> Candidate {
        Candidate {
            entry: SnapshotEntry {
                app_id: AppId::from_normalized(source),
                name: "same label".into(),
                link_path: source.into(),
                target_path: "example.exe".into(),
                link_mtime: 1,
                target_mtime: 1,
                icon_location: String::new(),
                icon_index: 0,
                version: String::new(),
                publisher: String::new(),
                identifier: String::new(),
            },
            identity: identity.map(str::to_owned),
            native_key: native.map(str::to_owned),
            exact_key: None,
            source: Source::UserStart,
            normal: true,
            valid: true,
        }
    }
    fn scan(
        aliases: &mut IdentityAliases,
        layout: &mut LauncherState,
        candidates: Vec<Candidate>,
    ) -> BTreeMap<AppId, SnapshotEntry> {
        let groups = aliases.groups(candidates);
        aliases.reconcile(groups, layout)
    }
    #[test]
    fn common_aumid_groups_different_routes_and_shell_wins() {
        let mut direct = c(
            "direct.lnk",
            Some("aumid:com.example.App"),
            Some("native:v1/app.exe"),
        );
        direct.normal = false;
        let update = c("updater.lnk", Some("aumid:com.example.App"), None);
        let mut shell = c(
            "shell:AppsFolder\\com.example.App",
            Some("aumid:com.example.App"),
            None,
        );
        shell.source = Source::Shell;
        let result = scan(
            &mut IdentityAliases::default(),
            &mut LauncherState::default(),
            vec![direct, update, shell.clone()],
        );
        assert_eq!(result.len(), 1);
        assert_eq!(
            result.values().next().unwrap().link_path,
            shell.entry.link_path
        );
    }
    #[test]
    fn different_explicit_ids_and_unidentified_bridge_do_not_merge() {
        let groups = group(vec![
            c("a", Some("aumid:A"), Some("native:host")),
            c("b", Some("aumid:B"), Some("native:host")),
            c("bridge", None, Some("native:host")),
        ]);
        assert_eq!(groups.len(), 3);
        assert!(groups.iter().all(|g| !g.keys.contains("native:host")));
    }
    #[test]
    fn normal_native_duplicates_join_single_explicit_identity() {
        let mut copy = c("copy", None, Some("native:app"));
        copy.normal = false;
        let result = group(vec![
            c("original", Some("aumid:A"), Some("native:app")),
            copy,
        ]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].identity.as_deref(), Some("aumid:A"));
    }
    #[test]
    fn unknown_hosts_and_different_steam_ids_stay_separate_despite_names() {
        assert_eq!(
            group(vec![c("host1", None, None), c("host2", None, None)]).len(),
            2
        );
        assert_eq!(
            group(vec![
                c("game1", Some("steam:1"), None),
                c("game2", Some("steam:2"), None)
            ])
            .len(),
            2
        );
    }
    #[test]
    fn moving_link_changing_executable_and_restarting_preserves_id_and_folder() {
        let old = c("old.lnk", Some("aumid:A"), Some("native:v1/app.exe"));
        let old_id = old.entry.app_id.clone();
        let fid = FolderId::from_normalized("folder");
        let mut layout = LauncherState {
            customized: true,
            items: vec![
                LauncherItem::Folder(fid.clone()),
                LauncherItem::App(AppId::from_normalized("copy.lnk")),
            ],
            ..Default::default()
        };
        layout.folders.insert(
            fid.clone(),
            Folder {
                id: fid.clone(),
                name: "Tools".into(),
                children: vec![old_id.clone(), AppId::from_normalized("other")],
            },
        );
        let mut aliases = IdentityAliases::default();
        let first = scan(
            &mut aliases,
            &mut layout,
            vec![old, c("copy.lnk", Some("aumid:A"), None)],
        );
        assert_eq!(first.keys().next(), Some(&old_id));
        assert_eq!(layout.items.len(), 1);
        assert_eq!(layout.folders[&fid].children[0], old_id);
        aliases = serde_json::from_str(&serde_json::to_string(&aliases).unwrap()).unwrap();
        let next = c("moved.lnk", Some("aumid:A"), Some("native:v2/app.exe"));
        let second = scan(&mut aliases, &mut layout, vec![next]);
        assert_eq!(second.keys().next(), Some(&old_id));
        assert_eq!(second[&old_id].link_path, "moved.lnk");
        assert_eq!(layout.folders[&fid].children[0], old_id);
    }
    #[test]
    fn acquiring_id_preserves_identity_but_repurposing_source_does_not() {
        let mut aliases = IdentityAliases::default();
        let mut layout = LauncherState::default();
        let first = scan(
            &mut aliases,
            &mut layout,
            vec![c("same.lnk", None, Some("native:app"))],
        );
        let id = first.keys().next().unwrap().clone();
        let next = scan(
            &mut aliases,
            &mut layout,
            vec![c("same.lnk", Some("aumid:A"), Some("native:app"))],
        );
        assert!(next.contains_key(&id));
        let other = scan(
            &mut aliases,
            &mut layout,
            vec![c("same.lnk", Some("aumid:B"), Some("native:other"))],
        );
        assert!(!other.contains_key(&id));
    }
    #[test]
    fn earlier_placement_wins_even_inside_a_folder_and_hidden_intent_survives() {
        let a = AppId::from_normalized("a");
        let b = AppId::from_normalized("b");
        let mut layout = LauncherState::from_legacy(vec![b.clone(), a.clone()], vec![]);
        let mut aliases = IdentityAliases::default();
        let result = scan(
            &mut aliases,
            &mut layout,
            vec![c("a", Some("aumid:A"), None), c("b", Some("aumid:A"), None)],
        );
        assert!(result.contains_key(&b));
        assert_eq!(layout.items, vec![LauncherItem::App(b.clone())]);
        layout.hidden_apps.insert(a.clone());
        remap_layout(&mut layout, &BTreeMap::from([(a, b.clone())]));
        assert!(layout.hidden_apps.contains(&b));
        assert!(layout.items.is_empty());
    }
    #[test]
    fn same_rank_new_shortcut_does_not_change_selected_route() {
        let mut aliases = IdentityAliases::default();
        let mut layout = LauncherState::default();
        let first = scan(
            &mut aliases,
            &mut layout,
            vec![c("z", Some("aumid:A"), None)],
        );
        let second = scan(
            &mut aliases,
            &mut layout,
            vec![c("a", Some("aumid:A"), None), c("z", Some("aumid:A"), None)],
        );
        assert_eq!(first, second);
    }
    #[test]
    fn failed_scan_or_one_missing_observation_does_not_remove_app() {
        let e = c("a", None, None).entry;
        let old = BTreeMap::from([(e.app_id.clone(), e)]);
        let mut presence = Presence::default();
        for complete in [false, false, true, false] {
            let mut new = BTreeMap::new();
            presence.retain_transient(&old, &mut new, complete);
            assert_eq!(new, old);
        }
        let mut new = BTreeMap::new();
        presence.retain_transient(&old, &mut new, true);
        assert!(new.is_empty());
    }
    #[test]
    fn reused_locator_does_not_keep_a_stale_launcher_during_grace_period() {
        let old_entry = c("reused.lnk", Some("aumid:Old"), None).entry;
        let mut new_entry = old_entry.clone();
        new_entry.app_id = AppId::from_normalized("new-app");
        let old = BTreeMap::from([(old_entry.app_id.clone(), old_entry)]);
        let mut new = BTreeMap::from([(new_entry.app_id.clone(), new_entry)]);
        Presence::default().retain_transient(&old, &mut new, false);
        assert_eq!(new.len(), 1);
        assert!(!new.contains_key(old.keys().next().unwrap()));
    }

    #[test]
    fn legacy_shell_path_migrates_without_folding_distinct_aumid_case() {
        let mut upper = c(
            r"shell:AppsFolder\Vendor.App!Main",
            Some("aumid:Vendor.App!Main"),
            None,
        );
        upper.source = Source::Shell;
        let mut lower = c(
            r"shell:AppsFolder\vendor.app!main",
            Some("aumid:vendor.app!main"),
            None,
        );
        lower.source = Source::Shell;
        let legacy = AppId::from_link_path(&upper.entry.link_path);
        let mut layout = LauncherState::from_legacy(vec![legacy.clone()], vec![]);
        let mut aliases = IdentityAliases::default();
        let result = scan(
            &mut aliases,
            &mut layout,
            vec![upper.clone(), lower.clone()],
        );
        assert_eq!(result.len(), 2);
        assert!(result.contains_key(&legacy));
        let restarted = scan(&mut aliases, &mut layout, vec![upper, lower]);
        assert_eq!(result, restarted);
    }

    #[test]
    fn late_legacy_shell_layout_adopts_existing_catalog_identity() {
        let mut app = c(
            r"shell:AppsFolder\Vendor.App!Main",
            Some("aumid:Vendor.App!Main"),
            None,
        );
        app.source = Source::Shell;
        let legacy = AppId::from_link_path(&app.entry.link_path);
        let mut layout = LauncherState::default();
        let mut aliases = IdentityAliases::default();
        let first = scan(&mut aliases, &mut layout, vec![app.clone()]);
        layout = LauncherState::from_legacy(
            vec![legacy.clone(), first.keys().next().unwrap().clone()],
            vec![],
        );
        let next = scan(&mut aliases, &mut layout, vec![app]);
        assert!(next.contains_key(&legacy));
        assert_eq!(layout.items, vec![LauncherItem::App(legacy)]);
    }
}
