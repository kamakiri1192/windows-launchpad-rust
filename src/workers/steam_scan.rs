//! Discover locally installed Steam apps from Steam library manifests.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::domain::app_diff::SnapshotEntry;
use crate::domain::app_id::AppId;
use crate::icons::extract;

/// Scan every configured Steam library for installed app manifests.
pub fn scan_steam_apps() -> Vec<SnapshotEntry> {
    let Some(steam_root) = find_steam_root() else {
        return Vec::new();
    };

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
            let Some((app_id, name)) = parse_app_manifest(&text) else {
                continue;
            };
            // Steam installs this shared runtime as an app manifest, but it is
            // not user-launchable and Steam itself hides it from the library.
            if app_id == "228980" {
                continue;
            }
            if !seen_app_ids.insert(app_id.clone()) {
                continue;
            }

            let icon_path = find_library_icon(&steam_root, &app_id)
                .unwrap_or_else(|| steam_root.join("steam.exe"));
            let manifest_mtime = extract::file_mtime(&manifest_path);
            let icon_mtime = extract::file_mtime(&icon_path);
            entries.push(SnapshotEntry {
                app_id: AppId::from_normalized(format!("steam:{app_id}")),
                name,
                link_path: format!("steam://rungameid/{app_id}"),
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

fn find_steam_root() -> Option<PathBuf> {
    registry_steam_root()
        .into_iter()
        .chain(default_steam_roots())
        .find(|path| path.join("steamapps").is_dir())
}

fn registry_steam_root() -> Option<PathBuf> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };

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
        let key = wide_null(key);
        let value = wide_null(value);
        let mut bytes = 0u32;
        let first = unsafe {
            RegGetValueW(
                hive,
                PCWSTR(key.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut bytes),
            )
        };
        if !first.is_ok() || bytes < 2 {
            continue;
        }
        let mut buffer = vec![0u16; (bytes as usize / 2) + 1];
        let second = unsafe {
            RegGetValueW(
                hive,
                PCWSTR(key.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if second.is_ok() {
            let len = buffer
                .iter()
                .position(|&ch| ch == 0)
                .unwrap_or(buffer.len());
            let path = PathBuf::from(String::from_utf16_lossy(&buffer[..len]));
            if !path.as_os_str().is_empty() {
                return Some(path);
            }
        }
    }
    None
}

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

fn parse_library_paths(text: &str) -> Vec<PathBuf> {
    let tokens = quoted_tokens(text);
    tokens
        .windows(2)
        .filter_map(|pair| {
            let key = &pair[0];
            let value = &pair[1];
            let is_path_field =
                key.eq_ignore_ascii_case("path") || key.chars().all(|ch| ch.is_ascii_digit());
            (is_path_field && looks_like_absolute_windows_path(value)).then(|| PathBuf::from(value))
        })
        .collect()
}

fn parse_app_manifest(text: &str) -> Option<(String, String)> {
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
    if app_id.chars().all(|ch| ch.is_ascii_digit()) && !name.trim().is_empty() {
        Some((app_id, name))
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

fn find_library_icon(steam_root: &Path, app_id: &str) -> Option<PathBuf> {
    let cache = steam_root.join("appcache").join("librarycache");
    for extension in ["jpg", "png", "jpeg"] {
        let candidate = cache.join(format!("{app_id}_icon.{extension}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let app_cache = cache.join(app_id);
    let items = fs::read_dir(app_cache).ok()?;
    let mut candidates: Vec<(u8, PathBuf)> = items
        .flatten()
        .filter_map(|item| {
            let path = item.path();
            if !path.is_file() || !is_image(&path) {
                return None;
            }
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            // Modern Steam stores the client icon under its content hash, so a
            // plain hash filename is preferable to the named cover artwork.
            let rank = if name.contains("icon") || looks_like_content_hash(&path) {
                0
            } else if name.contains("logo") {
                1
            } else if name.contains("library_600x900") {
                2
            } else if name.contains("header") {
                3
            } else {
                4
            };
            Some((rank, path))
        })
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    candidates.into_iter().next().map(|(_, path)| path)
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

fn looks_like_absolute_windows_path(value: &str) -> bool {
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

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn parses_app_manifest_name_and_id() {
        let text = r#""AppState" { "appid" "620" "name" "Portal 2" }"#;
        assert_eq!(
            parse_app_manifest(text),
            Some(("620".to_string(), "Portal 2".to_string()))
        );
    }

    #[test]
    fn rejects_non_numeric_app_id() {
        let text = r#""AppState" { "appid" "oops" "name" "Broken" }"#;
        assert_eq!(parse_app_manifest(text), None);
    }
}
