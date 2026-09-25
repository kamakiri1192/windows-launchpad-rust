//! Collapse equivalent launch routes without changing the retained shortcut's ID.
use std::collections::BTreeMap;

use crate::domain::{app_diff::SnapshotEntry, app_id::AppId};
use crate::icons::extract::LnkMetadata;

pub(super) fn deduplicate(
    entries: &mut BTreeMap<AppId, SnapshotEntry>,
    metadata: &BTreeMap<AppId, LnkMetadata>,
) {
    let mut winners: BTreeMap<(String, String, String), (u8, AppId)> = BTreeMap::new();
    for id in entries.keys() {
        let Some((target, args, directory, preference)) = metadata.get(id).and_then(launch_key)
        else {
            continue;
        };
        let candidate = (preference, id.clone());
        winners
            .entry((target, args, directory))
            .and_modify(|winner| {
                if candidate < *winner {
                    *winner = candidate.clone();
                }
            })
            .or_insert(candidate);
    }
    entries.retain(|id, _| {
        metadata
            .get(id)
            .and_then(launch_key)
            .is_none_or(|(target, args, directory, _)| winners[&(target, args, directory)].1 == *id)
    });
}

fn launch_key(meta: &LnkMetadata) -> Option<(String, String, String, u8)> {
    if meta.target_path.is_empty() || !meta.arguments_known {
        return None;
    }
    let path = normalize_path(&meta.target_path);
    // Arguments can be case-sensitive. Unknown arguments must never be treated as empty.
    let args = meta.arguments.trim();
    let mut directory = normalize_path(meta.working_directory.as_deref()?);
    // Some installers incorrectly put the executable itself in "Start in".
    if directory == path {
        directory = path.rsplit_once('\\')?.0.to_owned();
    }
    if let Some((root, preference)) = discord_route(&path, args) {
        let valid_directory = directory.is_empty()
            || directory == root
            || directory
                .strip_prefix(&format!("{root}\\app-"))
                .is_some_and(numeric_version);
        if valid_directory {
            return Some((
                format!("{root}\\discord.exe"),
                String::new(),
                root.into(),
                preference,
            ));
        }
    }
    Some((path, args.to_owned(), directory, 0))
}

fn normalize_path(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn numeric_version(version: &str) -> bool {
    version
        .split('.')
        .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn discord_route<'a>(path: &'a str, args: &str) -> Option<(&'a str, u8)> {
    if let Some(root) = path.strip_suffix("\\update.exe") {
        if root.ends_with("\\discord") && args == "--processStart Discord.exe" {
            // Keep the updater entry so Discord upgrades do not break the retained route.
            return Some((root, 0));
        }
    }
    let parent = path.strip_suffix("\\discord.exe")?;
    let (root, version) = parent.rsplit_once("\\app-")?;
    (root.ends_with("\\discord") && args.is_empty() && numeric_version(version))
        .then_some((root, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, target: &str, args: Option<&str>) -> (SnapshotEntry, LnkMetadata) {
        (
            SnapshotEntry {
                app_id: AppId::from_normalized(id),
                name: "same label".into(),
                link_path: format!("C:/{id}.lnk"),
                target_path: target.into(),
                link_mtime: 0,
                target_mtime: 0,
                icon_location: String::new(),
                icon_index: 0,
                version: String::new(),
                publisher: String::new(),
                identifier: String::new(),
            },
            LnkMetadata {
                target_path: target.into(),
                arguments: args.unwrap_or_default().into(),
                arguments_known: args.is_some(),
                working_directory: Some(String::new()),
                ..Default::default()
            },
        )
    }

    fn dedup(input: Vec<(SnapshotEntry, LnkMetadata)>) -> BTreeMap<AppId, SnapshotEntry> {
        let metadata = input
            .iter()
            .map(|(e, m)| (e.app_id.clone(), m.clone()))
            .collect();
        let mut map = input
            .into_iter()
            .map(|(e, _)| (e.app_id.clone(), e))
            .collect();
        deduplicate(&mut map, &metadata);
        map
    }

    #[test]
    fn equivalent_shortcuts_collapse_deterministically() {
        let a = entry("a", "C:/Steam/Steam.exe", Some(""));
        let b = entry("b", "c:\\steam\\steam.exe", Some("  "));
        assert_eq!(dedup(vec![a.clone(), b.clone()]), dedup(vec![b, a]));
        assert_eq!(
            dedup(vec![
                entry("a", "c:/app.exe", Some("")),
                entry("b", "C:/APP.exe", Some(""))
            ])
            .len(),
            1
        );
    }

    #[test]
    fn different_modes_installs_and_unknown_arguments_survive() {
        let map = dedup(vec![
            entry("a", "C:/app.exe", Some("")),
            entry("b", "C:/app.exe", Some("--profile Work")),
            entry("c", "C:/app.exe", Some("--profile work")),
            entry("d", "D:/app.exe", Some("")),
            entry("e", "C:/app.exe", None),
            entry("f", "C:/app.exe", None),
            entry("g", "", Some("")),
            entry("h", "", Some("")),
        ]);
        assert_eq!(map.len(), 8);
    }

    #[test]
    fn working_directories_preserve_distinct_launch_modes() {
        let a = entry("a", "C:/app.exe", Some(""));
        let mut b = entry("b", "C:/app.exe", Some(""));
        b.1.working_directory = Some("C:/project".into());
        let mut unknown = entry("unknown", "C:/app.exe", Some(""));
        unknown.1.working_directory = None;
        assert_eq!(dedup(vec![a, b, unknown]).len(), 3);
    }

    #[test]
    fn creator_companion_invalid_start_in_path_matches_install_directory() {
        let mut a = entry(
            "a",
            "C:/Programs/VRChat Creator Companion/CreatorCompanion.exe",
            Some(""),
        );
        let mut b = entry(
            "b",
            "C:/Programs/VRChat Creator Companion/CreatorCompanion.exe",
            Some(""),
        );
        a.1.working_directory = Some(a.1.target_path.clone());
        b.1.working_directory = Some("C:/Programs/VRChat Creator Companion".into());
        assert_eq!(dedup(vec![a, b]).len(), 1);
    }

    #[test]
    fn discord_prefers_updater_but_preserves_other_installs_and_modes() {
        let map = dedup(vec![
            entry("a", "C:/Users/a/Discord/app-1.0.9258/Discord.exe", Some("")),
            entry(
                "z",
                "C:/Users/a/Discord/Update.exe",
                Some("--processStart Discord.exe"),
            ),
            entry(
                "other-user",
                "C:/Users/b/Discord/app-1.0.9258/Discord.exe",
                Some(""),
            ),
            entry(
                "other-mode",
                "C:/Users/a/Discord/Update.exe",
                Some("--processStart Discord.exe --process-start-args test"),
            ),
            entry(
                "not-version",
                "C:/Users/a/Discord/app-custom/Discord.exe",
                Some(""),
            ),
        ]);
        assert_eq!(map.len(), 4);
        assert!(!map.contains_key(&AppId::from_normalized("a")));
        assert!(map.contains_key(&AppId::from_normalized("z")));
    }
}
