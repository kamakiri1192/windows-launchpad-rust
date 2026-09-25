//! Scan Start Menu/desktop shortcuts, packaged apps and Steam libraries into records.
//!
//! This is the *fast* side of icon loading: it walks the two Start Menu roots,
//! resolves each `.lnk`'s target + icon location, and reads mtimes — but it
//! does **not** touch Shell/GDI for pixels. That separation is what lets us
//! show the app list + placeholders before any icon is extracted, and what
//! makes polling-based diff detection cheap.
//!
//! The expensive Win32 calls here (`SHGetKnownFolderPath`, `IShellLinkW`,
//! `GetFileAttributesExW`) all live behind [`scan_start_menu`]; the diffing is
//! pure and lives in [`crate::domain::app_diff`].

use std::collections::BTreeMap;
#[cfg(windows)]
use std::collections::HashSet;
#[cfg(windows)]
use std::path::Path;

use crate::domain::app_diff::SnapshotEntry;
use crate::domain::app_id::AppId;
#[cfg(windows)]
#[path = "shortcut_dedup.rs"]
mod shortcut_dedup;
#[cfg(windows)]
use crate::icons::extract::{self, enumerate_start_menu};
#[cfg(any(windows, target_os = "macos"))]
use crate::workers::steam_scan::scan_steam_apps;

/// Scan Start Menu and desktop known folders plus installed Steam apps.
///
/// Failures on individual shortcuts are logged and skipped; one unreadable
/// `.lnk` can't blank the whole grid. The map is keyed by stable `AppId` so two
/// scans of the same set compare equal regardless of iteration order.
pub fn scan_start_menu() -> BTreeMap<AppId, SnapshotEntry> {
    #[cfg(target_os = "macos")]
    {
        let mut out = crate::platform::macos::apps::scan_applications();
        for entry in scan_steam_apps() {
            out.insert(entry.app_id.clone(), entry);
        }
        out
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        BTreeMap::new()
    }

    #[cfg(windows)]
    {
        let _com = extract::ComScope::new();
        let shortcuts = enumerate_start_menu();
        let uninstaller_targets = collect_uninstaller_targets();
        let mut out = BTreeMap::new();
        let mut launch_metadata = BTreeMap::new();
        for sc in shortcuts {
            let app_id = AppId::from_link_path(&sc.path);
            let link_mtime = extract::file_mtime(&sc.path);

            let meta = extract::resolve_lnk_metadata(&sc.path).unwrap_or_default();
            // A filesystem Start Menu can contain shortcuts to documents and
            // URLs as well as applications. Keep packaged AppsFolder entries,
            // but only expose conventional executable targets from .lnk
            // shortcuts so .txt/.html links do not become launcher tiles.
            if !extract::is_apps_folder_uri(&sc.path)
                && !extract::is_application_target(Path::new(&meta.target_path))
            {
                continue;
            }
            if uninstaller_targets.matches(Path::new(&meta.target_path), &meta.arguments, &sc.name)
            {
                continue;
            }
            let target_mtime = if meta.target_path.is_empty() {
                0
            } else {
                extract::file_mtime(Path::new(&meta.target_path))
            };
            let exe_meta = if meta.target_path.is_empty() {
                crate::platform::windows::ExeMetadata::default()
            } else {
                crate::platform::windows::exe_metadata(Path::new(&meta.target_path))
            };

            launch_metadata.insert(app_id.clone(), meta.clone());
            let entry = SnapshotEntry {
                app_id: app_id.clone(),
                name: sc.name,
                link_path: sc.path.to_string_lossy().into_owned(),
                link_mtime,
                target_path: meta.target_path,
                target_mtime,
                icon_location: meta.icon_location,
                icon_index: meta.icon_index,
                version: exe_meta.version,
                publisher: exe_meta.publisher,
                identifier: exe_meta.product_name,
            };
            // Duplicate ids (same file via two roots) collapse; last one wins,
            // which is fine — they're the same shortcut.
            out.insert(app_id, entry);
        }
        shortcut_dedup::deduplicate(&mut out, &launch_metadata);
        for entry in scan_steam_apps() {
            // Steam may also have created a Start Menu shortcut. Prefer the Steam
            // manifest record because it has a stable app id and survives shortcut
            // creation/removal. Only collapse an exact-name entry targeting Steam.
            out.retain(|_, existing| {
                !(existing.name.eq_ignore_ascii_case(&entry.name)
                    && existing
                        .target_path
                        .rsplit(['\\', '/'])
                        .next()
                        .is_some_and(|file| file.eq_ignore_ascii_case("steam.exe")))
            });
            out.insert(entry.app_id.clone(), entry);
        }
        out
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ExecutableKey {
    AbsolutePath(String),
    FileName(String),
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct UninstallerTargets {
    /// Commands registered without arguments can be matched without shortcut
    /// arguments. This covers dedicated uninstallers whose primary registry
    /// command is just the executable path.
    path_only: HashSet<ExecutableKey>,
    /// Shared helpers such as `msiexec.exe` and `steam.exe` need their
    /// arguments too, otherwise every shortcut launching that helper would be
    /// mistaken for an uninstall entry.
    command_lines: HashSet<(ExecutableKey, String)>,
    /// Registry display names used only for the rare shortcut whose target is
    /// the product executable rather than the registered uninstaller command.
    /// The label must still explicitly say "uninstall"; ordinary app names
    /// are never filtered from this set alone.
    display_names: HashSet<String>,
}

#[cfg(windows)]
impl UninstallerTargets {
    fn insert_registry_entry(&mut self, command: &str, display_name: Option<&str>) {
        if let Some(display_name) = display_name {
            let display_name = normalize_display_name(display_name);
            if !display_name.is_empty() {
                self.display_names.insert(display_name);
            }
        }
        self.insert_registry_command(command);
    }

    fn insert_registry_command(&mut self, command: &str) {
        let Some((executable, arguments)) = parse_command_line(command) else {
            return;
        };
        let executable = extract::expand_env(&executable);
        let path = Path::new(&executable);
        let key = if path.is_absolute() {
            ExecutableKey::AbsolutePath(normalize_windows_path(path))
        } else {
            let Some(name) = path.file_name() else {
                return;
            };
            let name = name.to_string_lossy().to_ascii_lowercase();
            if name.is_empty() {
                return;
            }
            ExecutableKey::FileName(name)
        };
        let arguments = normalize_arguments(&arguments);

        if arguments.is_empty() {
            self.path_only.insert(key);
        } else {
            self.command_lines.insert((key, arguments));
        }
    }

    fn matches(&self, target: &Path, arguments: &str, shortcut_name: &str) -> bool {
        if target.as_os_str().is_empty() {
            return false;
        }
        let keys = [
            Some(ExecutableKey::AbsolutePath(normalize_windows_path(target))),
            target
                .file_name()
                .map(|name| ExecutableKey::FileName(name.to_string_lossy().to_ascii_lowercase())),
        ];
        let arguments = normalize_arguments(arguments);
        let command_match = keys.into_iter().flatten().any(|key| {
            self.path_only.contains(&key) || self.command_lines.contains(&(key, arguments.clone()))
        });
        command_match || self.matches_explicit_uninstall_label(shortcut_name)
    }

    fn matches_explicit_uninstall_label(&self, shortcut_name: &str) -> bool {
        let shortcut_name = normalize_display_name(shortcut_name);
        self.display_names.iter().any(|display_name| {
            shortcut_name == format!("uninstall {display_name}")
                || shortcut_name == format!("{display_name} uninstall")
        })
    }
}

#[cfg(windows)]
fn collect_uninstaller_targets() -> UninstallerTargets {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        KEY_READ,
    };

    const UNINSTALL_ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    const UNINSTALL_ROOT_WOW6432: &str =
        r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

    let mut targets = UninstallerTargets::default();
    for (hive, root) in [
        (HKEY_CURRENT_USER, UNINSTALL_ROOT),
        (HKEY_CURRENT_USER, UNINSTALL_ROOT_WOW6432),
        (HKEY_LOCAL_MACHINE, UNINSTALL_ROOT),
        (HKEY_LOCAL_MACHINE, UNINSTALL_ROOT_WOW6432),
    ] {
        let root_wide = wide_null(root);
        let mut root_key = HKEY::default();
        let opened = unsafe {
            RegOpenKeyExW(
                hive,
                PCWSTR(root_wide.as_ptr()),
                None,
                KEY_READ,
                &mut root_key,
            )
        }
        .ok()
        .is_ok();
        if !opened {
            continue;
        }

        let mut index = 0u32;
        loop {
            let mut name = [0u16; 512];
            let mut name_len = (name.len() - 1) as u32;
            let enumerated = unsafe {
                RegEnumKeyExW(
                    root_key,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut name_len,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .ok()
            .is_ok();
            if !enumerated {
                break;
            }

            let subkey = String::from_utf16_lossy(&name[..name_len as usize]);
            let full_key = format!(r"{root}\{subkey}");
            let display_name = read_registry_string(hive, &full_key, "DisplayName");
            for value_name in ["UninstallString", "QuietUninstallString"] {
                if let Some(command) = read_registry_string(hive, &full_key, value_name) {
                    targets.insert_registry_entry(&command, display_name.as_deref());
                }
            }
            index = index.saturating_add(1);
        }

        unsafe {
            let _ = RegCloseKey(root_key);
        }
    }
    targets
}

#[cfg(windows)]
fn read_registry_string(
    hive: windows::Win32::System::Registry::HKEY,
    key: &str,
    value: &str,
) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ};

    let key_wide = wide_null(key);
    let value_wide = wide_null(value);
    let mut bytes = 0u32;
    unsafe {
        RegGetValueW(
            hive,
            PCWSTR(key_wide.as_ptr()),
            PCWSTR(value_wide.as_ptr()),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            None,
            Some(&mut bytes),
        )
    }
    .ok()
    .ok()?;
    if bytes < 2 {
        return None;
    }

    let mut buffer = vec![0u16; (bytes as usize / 2) + 1];
    unsafe {
        RegGetValueW(
            hive,
            PCWSTR(key_wide.as_ptr()),
            PCWSTR(value_wide.as_ptr()),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    }
    .ok()
    .ok()?;
    let len = buffer
        .iter()
        .position(|&ch| ch == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

#[cfg(windows)]
fn parse_command_line(command: &str) -> Option<(String, String)> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }

    if let Some(rest) = command.strip_prefix('"') {
        let (executable, arguments) = rest.split_once('"').unwrap_or((rest, ""));
        return Some((executable.trim().to_owned(), arguments.trim().to_owned()));
    }

    let lowercase = command.to_ascii_lowercase();
    let extension_end = [".exe", ".com"]
        .into_iter()
        .filter_map(|extension| {
            lowercase
                .find(extension)
                .map(|index| index + extension.len())
        })
        .min();
    if let Some(end) = extension_end {
        return Some((
            command[..end].trim().to_owned(),
            command[end..].trim().to_owned(),
        ));
    }

    let (executable, arguments) = command
        .split_once(char::is_whitespace)
        .map_or((command, ""), |(executable, arguments)| {
            (executable, arguments.trim())
        });
    Some((executable.to_owned(), arguments.to_owned()))
}

#[cfg(windows)]
fn normalize_windows_path(path: &Path) -> String {
    let mut value = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let extended_prefix = "\\\\?\\";
    if let Some(stripped) = value.strip_prefix(extended_prefix) {
        value = stripped.to_owned();
    }
    while value.len() > 3 && value.ends_with('\\') {
        value.pop();
    }
    value
}

#[cfg(windows)]
fn normalize_arguments(arguments: &str) -> String {
    let mut normalized = Vec::<String>::new();
    for token in arguments.split_whitespace() {
        let token = token.to_ascii_lowercase();
        // Windows Installer accepts both `/x{GUID}` and `/x {GUID}`. Shell
        // links commonly use the latter while ARP's UninstallString uses the
        // former, so fold the split form into the canonical one before
        // comparing command lines.
        if token.starts_with('{') {
            if let Some(previous) = normalized.last_mut() {
                if matches!(
                    previous.as_str(),
                    "/a" | "/f" | "/i" | "/j" | "/n" | "/p" | "/x"
                ) {
                    previous.push_str(&token);
                    continue;
                }
            }
        }
        normalized.push(token);
    }
    normalized.join(" ")
}

#[cfg(windows)]
fn normalize_display_name(name: &str) -> String {
    name.trim_matches('\0')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    #[ignore = "Read-only diagnostic; installed apps vary by machine"]
    fn report_discovered_app_launchers() {
        let snapshot = scan_start_menu();
        eprintln!("{} launcher(s)", snapshot.len());
        for entry in snapshot.values() {
            eprintln!(
                "{}: {} -> {}",
                entry.name, entry.link_path, entry.target_path
            );
        }
    }

    #[test]
    fn scan_returns_map_even_when_empty() {
        // We can't assert a real count without a Start Menu, but the function
        // must never panic and must return a usable map type.
        let m = scan_start_menu();
        // Keys must be stable ids (normalized). If any are present, verify shape.
        for (id, e) in &m {
            assert_eq!(id.as_str(), &e.app_id.as_str().to_string());
            assert!(!e.link_path.is_empty());
        }
    }

    #[cfg(windows)]
    #[test]
    fn uninstaller_matching_uses_registry_command_and_shortcut_arguments() {
        let mut targets = UninstallerTargets::default();
        targets.insert_registry_command(r#""C:\Program Files\Example\uninstall.exe""#);
        targets.insert_registry_command(r#"MsiExec.exe /X{ABC-123}"#);

        // The shortcut label is intentionally unrelated: the registry-backed
        // command is the source of truth.
        assert!(targets.matches(
            Path::new(r"C:\Program Files\Example\uninstall.exe"),
            "",
            "Example",
        ));
        assert!(targets.matches(
            Path::new(r"C:\Windows\System32\msiexec.exe"),
            "/x{abc-123}",
            "Uninstall Example",
        ));
        assert!(targets.matches(
            Path::new(r"C:\Windows\System32\msiexec.exe"),
            "/x {abc-123}",
            "Uninstall Example",
        ));
        assert!(!targets.matches(
            Path::new(r"C:\Windows\System32\msiexec.exe"),
            "/i{abc-123}",
            "Example",
        ));
    }

    #[cfg(windows)]
    #[test]
    fn uninstaller_matching_handles_product_target_with_registry_display_name() {
        let mut targets = UninstallerTargets::default();
        targets.insert_registry_entry(
            r#"C:\Program Files\Meta Horizon\Setup.exe /uninstall"#,
            Some("Meta Horizon Link"),
        );

        assert!(targets.matches(
            Path::new(r"C:\Program Files\Meta Horizon\Support\oculus-client\Client.exe"),
            "",
            "Uninstall Meta Horizon Link",
        ));
        assert!(!targets.matches(
            Path::new(r"C:\Program Files\Meta Horizon\Support\oculus-client\Client.exe"),
            "",
            "Meta Horizon Link",
        ));
    }
}
