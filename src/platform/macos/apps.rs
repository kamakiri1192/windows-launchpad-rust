//! Discover, identify, launch, and decode icons for macOS application bundles.

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use icns::{IconFamily, PixelFormat};
use plist::{Dictionary, Value};

use crate::domain::app_diff::SnapshotEntry;
use crate::domain::app_id::AppId;
use crate::icons::normalize::DecodedIcon;

const MAX_SCAN_DEPTH: usize = 6;

/// Scan standard user, local, and system application directories.
///
/// Earlier roots win bundle-id collisions, so a per-user app shadows a local
/// or system copy with the same identifier.
pub fn scan_applications() -> BTreeMap<AppId, SnapshotEntry> {
    let mut applications = BTreeMap::new();
    for root in application_roots() {
        scan_root(&root, &mut applications);
    }
    applications
}

fn application_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join("Applications"));
    }
    roots.extend([
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ]);
    roots
}

fn scan_root(root: &Path, applications: &mut BTreeMap<AppId, SnapshotEntry>) {
    let mut pending = VecDeque::from([(root.to_path_buf(), 0usize)]);
    while let Some((directory, depth)) = pending.pop_front() {
        let Ok(children) = std::fs::read_dir(&directory) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            let Ok(file_type) = child.file_type() else {
                continue;
            };
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            if path.extension().is_some_and(|ext| ext == "app") {
                if let Some(entry) = snapshot_entry(&path) {
                    applications.entry(entry.app_id.clone()).or_insert(entry);
                }
            } else if depth < MAX_SCAN_DEPTH {
                pending.push_back((path, depth + 1));
            }
        }
    }
}

fn snapshot_entry(bundle_path: &Path) -> Option<SnapshotEntry> {
    let info_path = bundle_path.join("Contents/Info.plist");
    let info = Value::from_file(&info_path).ok()?;
    let dictionary = info.as_dictionary()?;

    if dictionary_bool(dictionary, "LSBackgroundOnly") || dictionary_bool(dictionary, "LSUIElement")
    {
        return None;
    }

    let name = dictionary_string(dictionary, "CFBundleDisplayName")
        .or_else(|| dictionary_string(dictionary, "CFBundleName"))
        .map(str::to_owned)
        .or_else(|| {
            bundle_path
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })?;
    let bundle_id = dictionary_string(dictionary, "CFBundleIdentifier");
    let app_id = AppId::from_macos_bundle(bundle_id, bundle_path);

    let executable = dictionary_string(dictionary, "CFBundleExecutable")
        .map(|name| bundle_path.join("Contents/MacOS").join(name));
    let target_path = executable.as_deref().unwrap_or(bundle_path);
    let icon_path = resolve_icon_path(bundle_path, dictionary);

    Some(SnapshotEntry {
        app_id,
        name,
        link_path: bundle_path.to_string_lossy().into_owned(),
        link_mtime: file_mtime(&info_path),
        target_path: target_path.to_string_lossy().into_owned(),
        target_mtime: file_mtime(target_path),
        icon_location: icon_path
            .as_deref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        icon_index: 0,
    })
}

fn dictionary_string<'a>(dictionary: &'a Dictionary, key: &str) -> Option<&'a str> {
    dictionary.get(key)?.as_string()
}

fn dictionary_bool(dictionary: &Dictionary, key: &str) -> bool {
    dictionary
        .get(key)
        .and_then(Value::as_boolean)
        .unwrap_or(false)
}

fn resolve_icon_path(bundle_path: &Path, dictionary: &Dictionary) -> Option<PathBuf> {
    let resources = bundle_path.join("Contents/Resources");
    if let Some(icon_name) = dictionary_string(dictionary, "CFBundleIconFile") {
        let icon_name = if Path::new(icon_name).extension().is_some() {
            icon_name.to_owned()
        } else {
            format!("{icon_name}.icns")
        };
        let candidate = resources.join(icon_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    std::fs::read_dir(resources)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "icns"))
}

fn file_mtime(path: &Path) -> u64 {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// Decode the highest-resolution usable icon exposed by an app bundle.
pub fn extract_icon(bundle_path: &Path, icon_location: &str) -> Option<DecodedIcon> {
    let icon_path = if icon_location.is_empty() {
        let info = Value::from_file(bundle_path.join("Contents/Info.plist")).ok()?;
        resolve_icon_path(bundle_path, info.as_dictionary()?)?
    } else {
        PathBuf::from(icon_location)
    };

    if icon_path.extension().is_none_or(|ext| ext != "icns") {
        let bytes = std::fs::read(icon_path).ok()?;
        return image::load_from_memory(&bytes)
            .ok()
            .map(DecodedIcon::from_dynamic);
    }

    let family = IconFamily::read(File::open(icon_path).ok()?).ok()?;
    let icon_type = family
        .available_icons()
        .into_iter()
        .max_by_key(|kind| kind.pixel_width().saturating_mul(kind.pixel_height()))?;
    let image = family.get_icon_with_type(icon_type).ok()?;
    let rgba = image.convert_to(PixelFormat::RGBA);
    Some(DecodedIcon {
        w: rgba.width(),
        h: rgba.height(),
        rgba: rgba.into_data().into_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_roots_produce_an_empty_snapshot() {
        let mut applications = BTreeMap::new();
        scan_root(
            Path::new("/definitely-not-a-real-app-directory"),
            &mut applications,
        );
        assert!(applications.is_empty());
    }
}
