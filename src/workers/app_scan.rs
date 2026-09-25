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
use crate::icons::extract;
#[cfg(target_os = "macos")]
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
        let scan = scan_catalog();
        let mut aliases = crate::domain::app_catalog::IdentityAliases::default();
        aliases.reconcile(
            crate::domain::app_catalog::group(scan.candidates),
            &mut crate::domain::launcher_state::LauncherState::default(),
        )
    }
}

#[cfg(windows)]
pub fn scan_catalog() -> crate::domain::app_catalog::CatalogScan {
    use crate::domain::app_catalog::{Candidate, CatalogScan, PropertyValue, Source};
    let _com = extract::ComScope::new();
    let (shortcuts, errors, failed_sources) = extract::enumerate_application_candidates();
    let mut scan = CatalogScan {
        candidates: Vec::new(),
        errors,
        failed_sources,
        failed_locators: Default::default(),
    };
    let uninstallers = collect_uninstaller_targets();
    for sc in shortcuts {
        let meta = if let Some(meta) = sc.metadata {
            meta
        } else {
            match extract::resolve_lnk_metadata(&sc.path) {
                Some(meta) => meta,
                None => {
                    scan.failed_locators
                        .insert(sc.path.to_string_lossy().into_owned());
                    scan.errors
                        .push(format!("Could not read {}", sc.path.display()));
                    continue;
                }
            }
        };
        let target = Path::new(&meta.target_path);
        let kinds = if sc.source == Source::Shell
            && target.is_absolute()
            && !extract::is_application_target(target)
        {
            extract::target_kinds(target)
        } else {
            Vec::new()
        };
        if !is_application_candidate(sc.source, &meta.target_path, &kinds) {
            continue;
        }
        if uninstallers.matches(Path::new(&meta.target_path), &meta.arguments, &sc.name) {
            continue;
        }
        let exe = crate::platform::windows::exe_metadata(Path::new(&meta.target_path));
        let identity = steam_identity(&meta.target_path, &meta.arguments).or_else(|| {
            meta.app_user_model_id
                .value()
                .map(|id| format!("aumid:{id}"))
        });
        let native_key = (meta.app_user_model_id != PropertyValue::Unavailable
            && meta.arguments_known
            && Path::new(&meta.target_path).is_file()
            && extract::is_application_target(Path::new(&meta.target_path))
            && !is_host(&meta.target_path))
        .then(|| native_invocation_key(&meta.target_path, &meta.arguments));
        let exact_key = (meta.app_user_model_id != PropertyValue::Unavailable
            && Path::new(&meta.target_path).is_absolute()
            && meta.arguments_known
            && meta.link_flags.is_some_and(known_launch_flags)
            && meta.working_directory.is_some()
            && meta.show_command.is_some())
        .then(|| {
            format!(
                "exact:{:?}",
                (
                    &meta.target_path,
                    &meta.arguments,
                    &meta.working_directory,
                    meta.link_flags,
                    meta.show_command
                )
            )
        });
        let valid = sc.source == Source::Shell
            || (Path::new(&meta.target_path).is_file()
                && meta
                    .working_directory
                    .as_ref()
                    .is_none_or(|d| d.is_empty() || Path::new(d).is_dir()));
        let normal = sc.source == Source::Shell
            || (meta.link_flags.is_some_and(|flags| flags & 0x2000 == 0)
                && meta.show_command == Some(1));
        let app_id = if sc.source == Source::Shell {
            AppId::from_normalized(sc.path.to_string_lossy().into_owned())
        } else {
            AppId::from_link_path(&sc.path)
        };
        scan.candidates.push(Candidate {
            entry: SnapshotEntry {
                app_id,
                name: sc.name,
                link_path: sc.path.to_string_lossy().into_owned(),
                link_mtime: extract::file_mtime(&sc.path),
                target_mtime: extract::file_mtime(Path::new(&meta.target_path)),
                target_path: meta.target_path,
                icon_location: meta.icon_location,
                icon_index: meta.icon_index,
                version: exe.version,
                publisher: exe.publisher,
                identifier: exe.product_name,
            },
            identity,
            native_key,
            exact_key,
            source: sc.source,
            normal,
            valid,
        });
    }
    let (steam, errors) = super::steam_scan::scan_steam_apps_checked();
    if !errors.is_empty() {
        scan.failed_sources.insert(Source::Steam);
        scan.errors.extend(errors);
    }
    for entry in steam {
        scan.candidates.push(Candidate {
            identity: Some(entry.app_id.as_str().into()),
            entry,
            native_key: None,
            exact_key: None,
            source: Source::Steam,
            normal: true,
            valid: true,
        });
    }
    scan
}

/// Additional link payloads (MSI, compatibility shims, etc.) are not compared.
/// Without explicit identity, equal executable/flags cannot prove equal payloads.
#[cfg(windows)]
fn known_launch_flags(flags: u32) -> bool {
    use windows::Win32::UI::Shell::*;
    let known = (SLDF_HAS_ID_LIST.0
        | SLDF_HAS_LINK_INFO.0
        | SLDF_HAS_NAME.0
        | SLDF_HAS_RELPATH.0
        | SLDF_HAS_WORKINGDIR.0
        | SLDF_HAS_ARGS.0
        | SLDF_HAS_ICONLOCATION.0
        | SLDF_HAS_EXP_ICON_SZ.0
        | SLDF_UNICODE.0
        | SLDF_ENABLE_TARGET_METADATA.0
        | SLDF_RUNAS_USER.0
        | SLDF_RUN_IN_SEPARATE.0) as u32;
    flags & !known == 0
}

#[cfg(windows)]
fn native_invocation_key(target: &str, arguments: &str) -> String {
    let path = normalize_windows_path(Path::new(target));
    if arguments.is_empty() {
        // Preserve existing aliases for argument-free native applications.
        format!("native:{path}")
    } else {
        // Do not strip, case-fold or reorder arguments: they may identify
        // different applications/profiles hosted by the same executable.
        format!("native-args:{:?}", (path, arguments))
    }
}

#[cfg(windows)]
fn is_application_candidate(
    source: crate::domain::app_catalog::Source,
    target: &str,
    kinds: &[String],
) -> bool {
    let path = Path::new(target);
    // AppsFolder also contains ordinary documents and Internet shortcuts.
    // An AUMID identifies the entry but does not make its target an application.
    let lower = target.to_ascii_lowercase();
    if lower.starts_with("http:")
        || lower.starts_with("https:")
        || path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("url"))
    {
        return false;
    }
    if extract::is_application_target(path) {
        return true;
    }
    if source != crate::domain::app_catalog::Source::Shell {
        return false;
    }
    // Namespace entries, packaged apps and provider launch protocols have no
    // filesystem document target. Preserve these and unknown metadata.
    if !path.is_absolute() {
        return true;
    }
    let has_kind = |kind: &str| kinds.iter().any(|value| value.eq_ignore_ascii_case(kind));
    if has_kind("program") || has_kind("game") {
        return true;
    }
    ![
        "document",
        "picture",
        "music",
        "video",
        "movie",
        "recordedtv",
        "email",
        "communication",
        "contact",
        "calendar",
        "task",
        "note",
        "journal",
        "feed",
        "webhistory",
        "searchfolder",
    ]
    .iter()
    .any(|kind| has_kind(kind))
}

#[cfg(windows)]
fn steam_identity(target: &str, args: &str) -> Option<String> {
    let id = if let Some(rest) = target.strip_prefix("steam://rungameid/") {
        rest
    } else {
        if !Path::new(target)
            .file_name()?
            .eq_ignore_ascii_case("steam.exe")
        {
            return None;
        }
        args.strip_prefix("-applaunch ")?
            .split_whitespace()
            .next()?
    };
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| format!("steam:{id}"))
}

#[cfg(windows)]
fn is_host(target: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CLASSES_ROOT, RRF_RT_ANY};
    let Some(name) = Path::new(target).file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    let key = wide_null(&format!("Applications\\{name}"));
    let value = wide_null("IsHostApp");
    unsafe {
        RegGetValueW(
            HKEY_CLASSES_ROOT,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_ANY,
            None,
            None,
            None,
        )
        .is_ok()
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
    fn shell_content_is_classified_by_target_not_label_or_identity() {
        use crate::domain::app_catalog::Source;
        for target in [
            "https://example.com/app.exe",
            "HTTP://example.com/",
            "C:/docs/renamed.URL",
        ] {
            assert!(!is_application_candidate(Source::Shell, target, &[]));
        }
        for extension in ["txt", "html", "rtf", "pdf", "custom"] {
            assert!(!is_application_candidate(
                Source::Shell,
                &format!("C:/docs/any-name.{extension}"),
                &["link".into(), "Document".into()]
            ));
        }
        for target in [
            "",
            "shell:AppsFolder\\vendor.app",
            "::{shell-namespace}",
            "steam://rungameid/123",
            "C:/Windows/system32/eventvwr.msc",
            "C:/Apps/renamed.exe",
            "C:/Apps/unknown.target",
        ] {
            assert!(
                is_application_candidate(Source::Shell, target, &[]),
                "{target}"
            );
        }
        assert!(is_application_candidate(
            Source::Shell,
            "C:/Apps/tool.custom",
            &["program".into(), "document".into()]
        ));
    }

    #[cfg(windows)]
    #[test]
    fn native_invocations_preserve_arguments_and_existing_alias_keys() {
        let key = native_invocation_key("C:/Apps/Editor.exe", "--profile work");
        assert_eq!(
            key,
            native_invocation_key("c:/apps/editor.EXE", "--profile work")
        );
        assert_ne!(
            key,
            native_invocation_key("C:/Apps/Editor.exe", "--profile personal")
        );
        assert_ne!(
            key,
            native_invocation_key("C:/Apps/Editor.exe", "--profile Work")
        );
        assert_eq!(
            native_invocation_key("C:/Apps/Editor.exe", ""),
            r"native:c:\apps\editor.exe"
        );
    }

    #[cfg(windows)]
    #[test]
    fn same_named_document_cannot_suppress_shell_app_and_steam_uses_ids() {
        use windows::Win32::UI::Shell::{SLDF_HAS_DARWINID, SLDF_RUN_WITH_SHIMLAYER};
        assert!(!known_launch_flags(SLDF_HAS_DARWINID.0 as u32));
        assert!(!known_launch_flags(SLDF_RUN_WITH_SHIMLAYER.0 as u32));
        assert!(known_launch_flags(0));
        use crate::domain::app_catalog::Source;
        assert!(!is_application_candidate(
            Source::UserStart,
            "C:/docs/readme.txt",
            &[],
        ));
        assert!(is_application_candidate(Source::Shell, "", &[]));
        assert_eq!(
            steam_identity("C:/Steam/steam.exe", "-applaunch 123"),
            Some("steam:123".into())
        );
        assert_eq!(
            steam_identity("steam://rungameid/456", ""),
            Some("steam:456".into())
        );
        assert_eq!(steam_identity("C:/Steam/steam.exe", "-uninstall 123"), None);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Read-only diagnostic; installed apps vary by machine"]
    fn report_discovered_app_launchers() {
        let scan = scan_catalog();
        eprintln!("Scan errors: {:?}", scan.errors);
        if let Ok(path) = std::env::var("LAUNCHPAD_CATALOG_REPORT") {
            let rows: Vec<_> = scan.candidates.iter().map(|c| serde_json::json!({
                "name": c.entry.name, "source": format!("{:?}", c.source), "locator": c.entry.link_path,
                "identity": c.identity, "native_key": c.native_key, "target": c.entry.target_path,
                "normal": c.normal, "valid": c.valid,
            })).collect();
            std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
        }
        let mut aliases = crate::domain::app_catalog::IdentityAliases::default();
        let snapshot = aliases.reconcile(
            crate::domain::app_catalog::group(scan.candidates),
            &mut crate::domain::launcher_state::LauncherState::default(),
        );
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
