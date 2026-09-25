//! Collapse shortcuts only when their observable launch settings are equivalent.
//! Different executables (including updater routes) remain separate. App labels,
//! product names and directory naming conventions are not application identity.
use std::collections::{BTreeMap, BTreeSet};

use windows::Win32::UI::Shell::{
    SLDF_ENABLE_TARGET_METADATA, SLDF_HAS_ARGS, SLDF_HAS_EXP_ICON_SZ, SLDF_HAS_ICONLOCATION,
    SLDF_HAS_ID_LIST, SLDF_HAS_LINK_INFO, SLDF_HAS_NAME, SLDF_HAS_RELPATH, SLDF_HAS_WORKINGDIR,
    SLDF_RUNAS_USER, SLDF_RUN_IN_SEPARATE, SLDF_UNICODE,
};

use crate::domain::{app_diff::SnapshotEntry, app_id::AppId};
use crate::icons::extract::LnkMetadata;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LaunchKey {
    target: String,
    arguments: String,
    directory: String,
    execution_flags: u32,
    show_command: i32,
}

pub(super) fn deduplicate(
    entries: &mut BTreeMap<AppId, SnapshotEntry>,
    metadata: &BTreeMap<AppId, LnkMetadata>,
) {
    let mut seen = BTreeSet::new();
    // BTreeMap visits IDs in order, so the retained shortcut is independent of
    // filesystem enumeration order and keeps its existing identity.
    entries.retain(|id, _| {
        metadata
            .get(id)
            .and_then(launch_key)
            .is_none_or(|key| seen.insert(key))
    });
}

fn launch_key(meta: &LnkMetadata) -> Option<LaunchKey> {
    if !std::path::Path::new(&meta.target_path).is_absolute() || !meta.arguments_known {
        return None;
    }
    let flags = meta.link_flags?;
    // These flags describe storage/presentation of fields already read above,
    // or cached target metadata. They do not select a different launch mode.
    let storage_flags = (SLDF_HAS_ID_LIST.0
        | SLDF_HAS_LINK_INFO.0
        | SLDF_HAS_NAME.0
        | SLDF_HAS_RELPATH.0
        | SLDF_HAS_WORKINGDIR.0
        | SLDF_HAS_ARGS.0
        | SLDF_HAS_ICONLOCATION.0
        | SLDF_HAS_EXP_ICON_SZ.0
        | SLDF_UNICODE.0
        | SLDF_ENABLE_TARGET_METADATA.0) as u32;
    let execution_flags = (SLDF_RUNAS_USER.0 | SLDF_RUN_IN_SEPARATE.0) as u32;
    // Flags requiring additional data (MSI activation, compatibility shims,
    // environment-based target resolution, etc.) are deliberately unsupported.
    // Equal flags alone do not imply equal payloads. Preserve unknown modes too.
    if flags & !(storage_flags | execution_flags) != 0 {
        return None;
    }
    Some(LaunchKey {
        target: normalize_path(&meta.target_path),
        // Applications can inspect their raw command line; do not trim arguments.
        arguments: meta.arguments.clone(),
        directory: normalize_path(meta.working_directory.as_deref()?),
        execution_flags: flags & execution_flags,
        show_command: meta.show_command?,
    })
}

fn normalize_path(path: &str) -> String {
    // Keep roots, relative paths, and invalid "Start in" values intact. In
    // particular C: and C:\ are different, and an exe is not its parent directory.
    path.replace('/', "\\").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Shell::{SLDF_HAS_DARWINID, SLDF_RUN_WITH_SHIMLAYER};

    #[test]
    fn real_shell_links_preserve_elevation_and_window_mode() {
        use windows::core::{Interface, PCWSTR};
        use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
        use windows::Win32::UI::Shell::{IShellLinkDataList, IShellLinkW, ShellLink};
        use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWMINNOACTIVE, SW_SHOWNORMAL};

        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                // Only this test's unique directory is removed, also on assertion failure.
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        fn wide(value: &str) -> Vec<u16> {
            value.encode_utf16().chain(Some(0)).collect()
        }
        let _com = crate::icons::extract::ComScope::new();
        let root = std::env::temp_dir().join(format!(
            "launchpad-launch-modes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let fixture = Fixture(root);
        let target = std::env::current_exe().unwrap();
        let target_wide = wide(&target.to_string_lossy());
        let directory_wide = wide(&target.parent().unwrap().to_string_lossy());
        let mut input = Vec::new();
        for (name, elevated, show) in [
            ("normal", false, SW_SHOWNORMAL),
            ("normal-copy", false, SW_SHOWNORMAL),
            ("admin", true, SW_SHOWNORMAL),
            ("minimized", false, SW_SHOWMINNOACTIVE),
        ] {
            let path = fixture.0.join(format!("{name}.lnk"));
            let path_wide = wide(&path.to_string_lossy());
            unsafe {
                let link: IShellLinkW =
                    CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
                link.SetPath(PCWSTR(target_wide.as_ptr())).unwrap();
                link.SetWorkingDirectory(PCWSTR(directory_wide.as_ptr()))
                    .unwrap();
                link.SetShowCmd(show).unwrap();
                let data: IShellLinkDataList = link.cast().unwrap();
                let flags = data.GetFlags().unwrap();
                if elevated {
                    data.SetFlags(flags | SLDF_RUNAS_USER.0 as u32).unwrap();
                }
                let persist: IPersistFile = link.cast().unwrap();
                persist.Save(PCWSTR(path_wide.as_ptr()), true).unwrap();
            }
            let meta = crate::icons::extract::resolve_lnk_metadata(&path).unwrap();
            assert_eq!(
                meta.link_flags.unwrap() & SLDF_RUNAS_USER.0 as u32 != 0,
                elevated
            );
            assert_eq!(meta.show_command, Some(show.0));
            let (mut snapshot, _) = entry(name, &meta.target_path, Some(&meta.arguments));
            snapshot.link_path = path.to_string_lossy().into_owned();
            input.push((snapshot, meta));
        }
        let retained = dedup(input);
        assert_eq!(retained.len(), 3);
        for name in ["normal", "admin", "minimized"] {
            assert!(retained.contains_key(&AppId::from_normalized(name)));
        }
    }

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
                link_flags: Some(0),
                show_command: Some(1),
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
    fn equivalent_shortcuts_keep_the_same_id_regardless_of_order_or_label() {
        let a = entry("a", "C:/Example/App.exe", Some(""));
        let mut b = entry("b", "c:\\example\\app.exe", Some(""));
        b.0.name = "another label".into();
        b.1.link_flags = Some(SLDF_HAS_NAME.0 as u32);
        let forward = dedup(vec![a.clone(), b.clone()]);
        assert_eq!(forward, dedup(vec![b, a]));
        assert_eq!(forward.len(), 1);
        assert!(forward.contains_key(&AppId::from_normalized("a")));
    }

    #[test]
    fn distinct_arguments_and_installs_survive() {
        assert_eq!(
            dedup(vec![
                entry("a", "C:/app.exe", Some("")),
                entry("b", "C:/app.exe", Some("--profile Work")),
                entry("c", "C:/app.exe", Some("--profile work")),
                entry("d", "D:/app.exe", Some("")),
                entry("e", "C:/app.exe", Some(" ")),
            ])
            .len(),
            5
        );
    }

    #[test]
    fn incomplete_or_unsupported_metadata_never_collapses() {
        let baseline = entry("a", "C:/app.exe", Some(""));
        let mut variants = Vec::new();
        let mut meta = baseline.1.clone();
        meta.arguments_known = false;
        variants.push(meta);
        let mut meta = baseline.1.clone();
        meta.working_directory = None;
        variants.push(meta);
        let mut meta = baseline.1.clone();
        meta.link_flags = None;
        variants.push(meta);
        let mut meta = baseline.1.clone();
        meta.show_command = None;
        variants.push(meta);
        for target in ["", "app.exe", "C:app.exe"] {
            let mut meta = baseline.1.clone();
            meta.target_path = target.into();
            variants.push(meta);
        }
        for flag in [
            SLDF_HAS_DARWINID.0 as u32,
            SLDF_RUN_WITH_SHIMLAYER.0 as u32,
            0x80000000,
        ] {
            let mut meta = baseline.1.clone();
            meta.link_flags = Some(flag);
            variants.push(meta);
        }
        for meta in variants {
            let mut a = baseline.clone();
            a.1 = meta.clone();
            let mut b = entry("b", "C:/app.exe", Some(""));
            b.1 = meta;
            assert_eq!(dedup(vec![a, b]).len(), 2);
        }
    }

    #[test]
    fn execution_flags_and_window_modes_are_distinct() {
        let normal = entry("normal", "C:/app.exe", Some(""));
        let mut admin = entry("admin", "C:/app.exe", Some(""));
        admin.1.link_flags = Some(SLDF_RUNAS_USER.0 as u32);
        let mut separate = entry("separate", "C:/app.exe", Some(""));
        separate.1.link_flags = Some(SLDF_RUN_IN_SEPARATE.0 as u32);
        let mut minimized = entry("minimized", "C:/app.exe", Some(""));
        minimized.1.show_command = Some(7);
        assert_eq!(dedup(vec![normal, admin, separate, minimized]).len(), 4);
    }

    #[test]
    fn working_directories_are_not_guessed_or_repaired() {
        let entries = ["", "C:/project", "C:/app.exe", "C:/", "C:"]
            .into_iter()
            .enumerate()
            .map(|(index, directory)| {
                let mut e = entry(&index.to_string(), "C:/app.exe", Some(""));
                e.1.working_directory = Some(directory.into());
                e
            })
            .collect();
        assert_eq!(dedup(entries).len(), 5);
    }

    #[test]
    fn updater_and_direct_executable_routes_are_not_assumed_equivalent() {
        assert_eq!(
            dedup(vec![
                entry("direct", "C:/Example/app-1.2.3/Example.exe", Some("")),
                entry(
                    "updater",
                    "C:/Example/Update.exe",
                    Some("--processStart Example.exe")
                ),
            ])
            .len(),
            2
        );
    }
}
