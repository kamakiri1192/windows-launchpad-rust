//! Discover locally installed Steam apps from Steam library manifests.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::app_diff::SnapshotEntry;
use crate::domain::app_id::AppId;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SteamManifest {
    app_id: String,
    name: String,
    install_dir: String,
}

/// Scan every configured Steam library for installed app manifests.
pub fn scan_steam_apps() -> Vec<SnapshotEntry> {
    let Some(steam_root) = find_steam_root() else {
        return Vec::new();
    };

    scan_steam_apps_at(&steam_root)
}

fn scan_steam_apps_at(steam_root: &Path) -> Vec<SnapshotEntry> {
    let steam_root = steam_root.to_path_buf();

    let mut libraries = vec![steam_root.clone()];
    let library_file = steam_root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = fs::read_to_string(&library_file) {
        libraries.extend(parse_library_paths(&text));
    }
    dedupe_paths(&mut libraries);

    let mut entries = Vec::new();
    let mut seen_app_ids = HashSet::new();
    for library in libraries {
        let steamapps = library.join("steamapps");
        let Ok(items) = fs::read_dir(&steamapps) else {
            continue;
        };
        for item in items.flatten() {
            let manifest_path = item.path();
            if !is_app_manifest(&manifest_path) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&manifest_path) else {
                continue;
            };
            let Some(manifest) = parse_app_manifest(&text) else {
                continue;
            };
            // Steam installs this shared runtime as an app manifest, but it is
            // not user-launchable and Steam itself hides it from the library.
            if manifest.app_id == "228980" {
                continue;
            }
            if !seen_app_ids.insert(manifest.app_id.clone()) {
                continue;
            }

            let icon_path = find_steam_icon(&steam_root, &library, &manifest)
                .unwrap_or_else(|| fallback_steam_icon(&steam_root));
            let manifest_mtime = file_mtime(&manifest_path);
            let icon_mtime = file_mtime(&icon_path);
            entries.push(SnapshotEntry {
                app_id: AppId::from_normalized(format!("steam:{}", manifest.app_id)),
                name: manifest.name,
                link_path: format!("steam://rungameid/{}", manifest.app_id),
                link_mtime: manifest_mtime,
                target_path: manifest_path.to_string_lossy().into_owned(),
                target_mtime: icon_mtime,
                icon_location: icon_path.to_string_lossy().into_owned(),
                icon_index: 0,
            });
        }
    }
    entries
}

#[cfg(windows)]
fn fallback_steam_icon(steam_root: &Path) -> PathBuf {
    steam_root.join("steam.exe")
}

#[cfg(target_os = "macos")]
fn fallback_steam_icon(_steam_root: &Path) -> PathBuf {
    PathBuf::from("/Applications/Steam.app")
}

fn file_mtime(path: &Path) -> u64 {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn find_steam_root() -> Option<PathBuf> {
    registry_steam_root()
        .into_iter()
        .chain(default_steam_roots())
        .find(|path| path.join("steamapps").is_dir())
}

#[cfg(windows)]
fn registry_steam_root() -> Option<PathBuf> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let candidates = [
        (HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath"),
        (
            HKEY_LOCAL_MACHINE,
            r"Software\WOW6432Node\Valve\Steam",
            "InstallPath",
        ),
        (HKEY_LOCAL_MACHINE, r"Software\Valve\Steam", "InstallPath"),
    ];
    for (hive, key, value) in candidates {
        if let Some(value) = read_registry_string(hive, key, value) {
            let path = PathBuf::from(value);
            if !path.as_os_str().is_empty() {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn registry_steam_root() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn default_steam_roots() -> impl Iterator<Item = PathBuf> {
    let mut roots = Vec::new();
    if let Some(path) = std::env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(path).join("Steam"));
    }
    if let Some(path) = std::env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(path).join("Steam"));
    }
    roots.into_iter()
}

#[cfg(target_os = "macos")]
fn default_steam_roots() -> impl Iterator<Item = PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/Steam"))
        .into_iter()
}

fn parse_library_paths(text: &str) -> Vec<PathBuf> {
    let tokens = quoted_tokens(text);
    tokens
        .windows(2)
        .filter_map(|pair| {
            let key = &pair[0];
            let value = &pair[1];
            let is_path_field =
                key.eq_ignore_ascii_case("path") || key.chars().all(|ch| ch.is_ascii_digit());
            (is_path_field && looks_like_absolute_library_path(value)).then(|| PathBuf::from(value))
        })
        .collect()
}

fn parse_app_manifest(text: &str) -> Option<SteamManifest> {
    let tokens = quoted_tokens(text);
    let value = |wanted: &str| {
        tokens.windows(2).find_map(|pair| {
            pair[0]
                .eq_ignore_ascii_case(wanted)
                .then(|| pair[1].clone())
        })
    };
    let app_id = value("appid")?;
    let name = value("name")?;
    let install_dir = value("installdir").unwrap_or_default();
    if app_id.chars().all(|ch| ch.is_ascii_digit()) && !name.trim().is_empty() {
        Some(SteamManifest {
            app_id,
            name,
            install_dir,
        })
    } else {
        None
    }
}

/// Tokenize Valve's text VDF enough for the flat fields used here. Braces do
/// not matter; quoted strings and their escaped backslashes do.
fn quoted_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '"' {
            continue;
        }
        let mut token = String::new();
        while let Some(ch) = chars.next() {
            match ch {
                '"' => break,
                '\\' => match chars.peek().copied() {
                    Some('\\') | Some('"') => token.push(chars.next().unwrap()),
                    _ => token.push('\\'),
                },
                _ => token.push(ch),
            }
        }
        out.push(token);
    }
    out
}

fn find_steam_icon(steam_root: &Path, library: &Path, manifest: &SteamManifest) -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(executable) = find_matching_root_executable(library, manifest) {
        return Some(executable);
    }

    #[cfg(windows)]
    {
        let display_icon = find_uninstall_display_icon(&manifest.app_id);
        if display_icon
            .as_deref()
            .is_some_and(icon_source_is_high_resolution)
        {
            return display_icon;
        }

        display_icon.or_else(|| find_small_client_icon(steam_root, &manifest.app_id))
    }

    #[cfg(target_os = "macos")]
    {
        let _ = (library, manifest);
        find_small_client_icon(steam_root, &manifest.app_id)
    }
}

#[cfg(windows)]
fn find_uninstall_display_icon(app_id: &str) -> Option<PathBuf> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let subkeys = [
        format!(
            r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Steam App {app_id}"
        ),
        format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Steam App {app_id}"),
    ];
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for subkey in &subkeys {
            let Some(raw) = read_registry_string(hive, subkey, "DisplayIcon") else {
                continue;
            };
            let path = parse_display_icon_path(&raw);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(windows)]
fn read_registry_string(
    hive: windows::Win32::System::Registry::HKEY,
    key: &str,
    value: &str,
) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};

    let key = wide_null(key);
    let value = wide_null(value);
    let mut bytes = 0u32;
    unsafe {
        RegGetValueW(
            hive,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
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
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
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
fn parse_display_icon_path(value: &str) -> PathBuf {
    let value = value.trim();
    let path = if let Some(quoted) = value.strip_prefix('"') {
        quoted.split_once('"').map_or(quoted, |(path, _)| path)
    } else if let Some((path, index)) = value.rsplit_once(',') {
        if index.trim().parse::<i32>().is_ok() {
            path.trim()
        } else {
            value
        }
    } else {
        value
    };
    PathBuf::from(path)
}

#[cfg(windows)]
fn icon_source_is_high_resolution(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "exe" | "dll") {
        return true;
    }
    image::image_dimensions(path)
        .map(|(width, height)| width.max(height) >= crate::icons::normalize::TARGET)
        .unwrap_or(false)
}

#[cfg(windows)]
fn find_matching_root_executable(library: &Path, manifest: &SteamManifest) -> Option<PathBuf> {
    if manifest.install_dir.is_empty() {
        return None;
    }
    let install_root = library
        .join("steamapps")
        .join("common")
        .join(&manifest.install_dir);
    let wanted_name = normalized_executable_name(&manifest.name);
    let wanted_dir = normalized_executable_name(&manifest.install_dir);
    if wanted_name.is_empty() && wanted_dir.is_empty() {
        return None;
    }
    let mut candidates: Vec<PathBuf> = fs::read_dir(install_root)
        .ok()?
        .flatten()
        .map(|item| item.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        })
        .filter(|path| {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(normalized_executable_name)
                .unwrap_or_default();
            stem == wanted_name || stem == wanted_dir
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

#[cfg(windows)]
fn normalized_executable_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn find_small_client_icon(steam_root: &Path, app_id: &str) -> Option<PathBuf> {
    let cache = steam_root.join("appcache").join("librarycache");
    for extension in ["jpg", "png", "jpeg"] {
        let candidate = cache.join(format!("{app_id}_icon.{extension}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let app_cache = cache.join(app_id);
    let items = fs::read_dir(app_cache).ok()?;
    let mut candidates: Vec<PathBuf> = items
        .flatten()
        .filter_map(|item| {
            let path = item.path();
            if !path.is_file() || !is_image(&path) || !looks_like_content_hash(&path) {
                return None;
            }
            Some(path)
        })
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

fn is_app_manifest(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.starts_with("appmanifest_") && name.ends_with(".acf")
}

fn is_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "bmp")
    )
}

fn looks_like_content_hash(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return false;
    };
    stem.len() >= 16 && stem.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn looks_like_absolute_library_path(value: &str) -> bool {
    if Path::new(value).is_absolute() {
        return true;
    }
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn dedupe_paths(paths: &mut Vec<PathBuf>) {
    let mut seen = HashSet::new();
    paths.retain(|path| {
        seen.insert(
            path.to_string_lossy()
                .replace('\\', "/")
                .to_ascii_lowercase(),
        )
    });
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "launchpad-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn parses_modern_and_legacy_library_paths() {
        let text = r#"
            "libraryfolders"
            {
                "0" { "path" "C:\\Program Files (x86)\\Steam" }
                "1" "D:\\SteamLibrary"
            }
        "#;
        assert_eq!(
            parse_library_paths(text),
            vec![
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from(r"D:\SteamLibrary")
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_macos_library_path() {
        let text = r#""libraryfolders" { "0" { "path" "/Volumes/Games/SteamLibrary" } }"#;
        assert_eq!(
            parse_library_paths(text),
            vec![PathBuf::from("/Volumes/Games/SteamLibrary")]
        );
    }

    #[test]
    fn parses_app_manifest_name_and_id() {
        let text = r#""AppState" { "appid" "620" "name" "Portal 2" "installdir" "Portal 2" }"#;
        assert_eq!(
            parse_app_manifest(text),
            Some(SteamManifest {
                app_id: "620".to_string(),
                name: "Portal 2".to_string(),
                install_dir: "Portal 2".to_string(),
            })
        );
    }

    #[test]
    fn rejects_non_numeric_app_id() {
        let text = r#""AppState" { "appid" "oops" "name" "Broken" }"#;
        assert_eq!(parse_app_manifest(text), None);
    }

    #[test]
    fn scans_installed_game_manifest_into_launchable_entry() {
        let root = temporary_directory("steam-library");
        let steamapps = root.join("steamapps");
        std::fs::create_dir_all(&steamapps).unwrap();
        std::fs::write(
            steamapps.join("appmanifest_620.acf"),
            r#""AppState" { "appid" "620" "name" "Portal 2" "installdir" "Portal 2" }"#,
        )
        .unwrap();

        let entries = scan_steam_apps_at(&root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].app_id.as_str(), "steam:620");
        assert_eq!(entries[0].name, "Portal 2");
        assert_eq!(entries[0].link_path, "steam://rungameid/620");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn parses_quoted_and_indexed_display_icon_paths() {
        assert_eq!(
            parse_display_icon_path(r#""C:\Steam\game.ico",0"#),
            PathBuf::from(r"C:\Steam\game.ico")
        );
        assert_eq!(
            parse_display_icon_path(r"C:\Steam\game.exe,-1"),
            PathBuf::from(r"C:\Steam\game.exe")
        );
    }

    #[cfg(windows)]
    #[test]
    fn normalizes_names_for_strong_executable_matching() {
        assert_eq!(normalized_executable_name("Desktop Mate"), "desktopmate");
        assert_eq!(
            normalized_executable_name("XSOverlay_Beta"),
            "xsoverlaybeta"
        );
    }
}
