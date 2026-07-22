//! Discover, identify, launch, and decode icons for macOS application bundles.

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use icns::{IconFamily, PixelFormat};
use plist::{Dictionary, Value};

use crate::domain::app_diff::SnapshotEntry;
use crate::domain::app_id::AppId;
use crate::icons::normalize::{DecodedIcon, TARGET};

const MAX_SCAN_DEPTH: usize = 6;

/// Scan standard user, local, and system application directories.
///
/// Earlier roots win bundle-id collisions, so a per-user app shadows a local
/// or system copy with the same identifier.
pub fn scan_applications() -> BTreeMap<AppId, SnapshotEntry> {
    let mut applications = BTreeMap::new();
    let preferred_languages = preferred_languages();
    for root in application_roots() {
        scan_root(&root, &preferred_languages, &mut applications);
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

fn scan_root(
    root: &Path,
    preferred_languages: &[String],
    applications: &mut BTreeMap<AppId, SnapshotEntry>,
) {
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
                if let Some(entry) = snapshot_entry(&path, preferred_languages) {
                    applications.entry(entry.app_id.clone()).or_insert(entry);
                }
            } else if depth < MAX_SCAN_DEPTH {
                pending.push_back((path, depth + 1));
            }
        }
    }
}

fn snapshot_entry(bundle_path: &Path, preferred_languages: &[String]) -> Option<SnapshotEntry> {
    let (metadata_bundle, info_path) = locate_metadata_bundle(bundle_path)?;
    let info = Value::from_file(&info_path).ok()?;
    let dictionary = info.as_dictionary()?;

    if dictionary_bool(dictionary, "LSBackgroundOnly") || dictionary_bool(dictionary, "LSUIElement")
    {
        return None;
    }

    let is_wrapped_ios_app = metadata_bundle != bundle_path;
    let name = ios_store_name(bundle_path)
        .or_else(|| localized_bundle_name(&metadata_bundle, preferred_languages))
        .or_else(|| dictionary_string(dictionary, "CFBundleDisplayName").map(str::to_owned))
        .or_else(|| dictionary_string(dictionary, "CFBundleName").map(str::to_owned))
        .or_else(|| {
            bundle_path
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })?;
    let name = if is_wrapped_ios_app {
        name
    } else {
        prefer_descriptive_bundle_name(bundle_path, name)
    };
    let bundle_id = dictionary_string(dictionary, "CFBundleIdentifier");
    let app_id = AppId::from_macos_bundle(bundle_id, bundle_path);

    let executable = dictionary_string(dictionary, "CFBundleExecutable").map(|name| {
        if metadata_bundle == bundle_path {
            bundle_path.join("Contents/MacOS").join(name)
        } else {
            metadata_bundle.join(name)
        }
    });
    let target_path = executable.as_deref().unwrap_or(bundle_path);
    let icon_path = resolve_icon_path(&metadata_bundle, dictionary);

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

fn preferred_languages() -> Vec<String> {
    use objc2_foundation::NSLocale;

    NSLocale::preferredLanguages()
        .iter()
        .map(|language| language.to_string())
        .collect()
}

fn localized_bundle_name(bundle_path: &Path, preferred_languages: &[String]) -> Option<String> {
    localized_loctable_value(bundle_path, preferred_languages, "CFBundleDisplayName")
        .or_else(|| localized_loctable_value(bundle_path, preferred_languages, "CFBundleName"))
        .or_else(|| localized_info_value(bundle_path, "CFBundleDisplayName"))
        .or_else(|| localized_info_value(bundle_path, "CFBundleName"))
}

/// Newer system apps store localized Info.plist values in a binary
/// `InfoPlist.loctable`. Foundation does not expose those translations when
/// inspecting another app bundle from this process, so resolve the table using
/// the same ordered language preferences supplied by `NSLocale`.
fn localized_loctable_value(
    bundle_path: &Path,
    preferred_languages: &[String],
    key: &str,
) -> Option<String> {
    let resources = bundle_path.join("Contents/Resources");
    let table = Value::from_file(resources.join("InfoPlist.loctable")).ok()?;
    let localizations = table.as_dictionary()?;
    for preferred in preferred_languages {
        for candidate in localization_candidates(preferred) {
            let Some(values) = dictionary_case_insensitive(localizations, &candidate)
                .and_then(Value::as_dictionary)
            else {
                continue;
            };
            if let Some(value) = dictionary_string(values, key) {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn localization_candidates(language: &str) -> Vec<String> {
    let normalized = language.replace('-', "_");
    let mut candidates = vec![normalized.clone()];
    let parts: Vec<&str> = normalized.split('_').collect();
    if parts
        .first()
        .is_some_and(|part| part.eq_ignore_ascii_case("zh"))
    {
        if parts.iter().any(|part| part.eq_ignore_ascii_case("Hans")) {
            candidates.push("zh_CN".to_owned());
        } else if parts.iter().any(|part| part.eq_ignore_ascii_case("Hant")) {
            candidates.push("zh_TW".to_owned());
        }
    }
    if parts.len() > 1 {
        candidates.push(parts[0].to_owned());
    }
    candidates.dedup();
    candidates
}

fn dictionary_case_insensitive<'a>(dictionary: &'a Dictionary, key: &str) -> Option<&'a Value> {
    dictionary
        .iter()
        .find_map(|(candidate, value)| candidate.eq_ignore_ascii_case(key).then_some(value))
}

fn localized_info_value(bundle_path: &Path, key: &str) -> Option<String> {
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSBundle, NSString};

    autoreleasepool(|_| {
        let path = NSString::from_str(&bundle_path.to_string_lossy());
        let bundle = NSBundle::bundleWithPath(&path)?;
        let key = NSString::from_str(key);
        bundle
            .objectForInfoDictionaryKey(&key)?
            .downcast_ref::<NSString>()
            .map(ToString::to_string)
    })
}

/// Return the bundle whose Info.plist describes the launchable application.
/// macOS apps use `Contents/Info.plist`; iPhone/iPad apps installed on Apple
/// silicon wrap their original iOS bundle below `Wrapper/*.app`.
fn locate_metadata_bundle(bundle_path: &Path) -> Option<(PathBuf, PathBuf)> {
    let mac_info = bundle_path.join("Contents/Info.plist");
    if mac_info.is_file() {
        return Some((bundle_path.to_path_buf(), mac_info));
    }

    let wrapper = bundle_path.join("Wrapper");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(wrapper)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
        .filter(|path| path.join("Info.plist").is_file())
        .collect();
    candidates.sort();
    let metadata_bundle = candidates.into_iter().next()?;
    let info_path = metadata_bundle.join("Info.plist");
    Some((metadata_bundle, info_path))
}

fn ios_store_name(bundle_path: &Path) -> Option<String> {
    let metadata = Value::from_file(bundle_path.join("Wrapper/iTunesMetadata.plist")).ok()?;
    dictionary_string(metadata.as_dictionary()?, "itemName").map(str::to_owned)
}

/// Preserve the OS-localized display name unless the installed bundle name is
/// a strict extension of it. Some Mac apps omit an edition or year from
/// `CFBundleDisplayName` (for example "Adobe Premiere" inside
/// "Adobe Premiere Pro 2026.app"). Keeping the descriptive suffix lets the
/// shared two-line launcher label distinguish installed versions, while a
/// localized name such as "カレンダー" still wins over `Calendar.app`.
fn prefer_descriptive_bundle_name(bundle_path: &Path, display_name: String) -> String {
    let Some(bundle_name) = bundle_path
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return display_name;
    };
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|ch| ch.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    let display_key = normalize(&display_name);
    let bundle_key = normalize(&bundle_name);
    if !display_key.is_empty()
        && bundle_key.len() > display_key.len()
        && bundle_key.starts_with(&display_key)
    {
        bundle_name
    } else {
        display_name
    }
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
    let resources = if bundle_path.join("Contents/Resources").is_dir() {
        bundle_path.join("Contents/Resources")
    } else {
        bundle_path.to_path_buf()
    };
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
    // Ask Launch Services first. This is the same icon resolution path Finder
    // uses, so it handles asset-catalog-only system apps (for example
    // Calendar), custom file icons, and ICNS encodings our portable decoder
    // does not support. The icon worker calls this off the UI thread;
    // `NSWorkspace::iconForFile` is explicitly documented as thread-safe.
    if let Some(icon) = extract_workspace_icon(bundle_path) {
        return Some(icon);
    }

    // Retain the direct decoder as a best-effort fallback for unusual bundles
    // where Launch Services cannot produce a bitmap representation.
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

fn extract_workspace_icon(bundle_path: &Path) -> Option<DecodedIcon> {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey, NSWorkspace,
    };
    use objc2_core_graphics::CGImage;
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

    autoreleasepool(|_| {
        let path = NSString::from_str(&bundle_path.to_string_lossy());
        let image = NSWorkspace::sharedWorkspace().iconForFile(&path);

        // A 64-point proposal normally selects a 128-pixel representation on
        // Retina displays. If it does not, retry at 128 points so normalization
        // never has to upscale the native icon.
        let mut proposed = NSRect::new(
            NSPoint::ZERO,
            NSSize::new((TARGET / 2) as f64, (TARGET / 2) as f64),
        );
        let mut cg_image =
            unsafe { image.CGImageForProposedRect_context_hints(&mut proposed, None, None)? };
        if CGImage::width(Some(&cg_image)) < TARGET as usize
            || CGImage::height(Some(&cg_image)) < TARGET as usize
        {
            proposed.size = NSSize::new(TARGET as f64, TARGET as f64);
            cg_image =
                unsafe { image.CGImageForProposedRect_context_hints(&mut proposed, None, None)? };
        }

        let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
        let properties: objc2::rc::Retained<NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>> =
            NSDictionary::new();
        let png = unsafe {
            bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)?
        };
        // `png` is immutable and retained for the whole decode call, so the
        // borrowed NSData bytes stay valid and avoid one allocation per app.
        let png_bytes = unsafe { png.as_bytes_unchecked() };
        image::load_from_memory(png_bytes)
            .ok()
            .map(DecodedIcon::from_dynamic)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "launchpad-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn missing_roots_produce_an_empty_snapshot() {
        let mut applications = BTreeMap::new();
        scan_root(
            Path::new("/definitely-not-a-real-app-directory"),
            &["ja-JP".to_owned()],
            &mut applications,
        );
        assert!(applications.is_empty());
    }

    #[test]
    fn ios_wrapper_bundle_uses_store_name_and_outer_launch_path() {
        let root = temporary_directory("ios-bundle");
        let outer = root.join("Localized Game.app");
        let inner = outer.join("Wrapper/Game.app");
        std::fs::create_dir_all(&inner).unwrap();

        let mut info = Dictionary::new();
        info.insert(
            "CFBundleIdentifier".into(),
            Value::String("com.example.game".into()),
        );
        info.insert(
            "CFBundleDisplayName".into(),
            Value::String("Internal Game".into()),
        );
        info.insert("CFBundleExecutable".into(), Value::String("Game".into()));
        Value::Dictionary(info)
            .to_file_xml(inner.join("Info.plist"))
            .unwrap();
        std::fs::write(inner.join("Game"), []).unwrap();

        let mut store = Dictionary::new();
        store.insert("itemName".into(), Value::String("ローカライズ名".into()));
        Value::Dictionary(store)
            .to_file_xml(outer.join("Wrapper/iTunesMetadata.plist"))
            .unwrap();

        let entry = snapshot_entry(&outer, &["ja-JP".to_owned()])
            .expect("wrapped iOS app should be discovered");
        assert_eq!(entry.app_id.as_str(), "mac:com.example.game");
        assert_eq!(entry.name, "ローカライズ名");
        assert_eq!(entry.link_path, outer.to_string_lossy());
        assert_eq!(entry.target_path, inner.join("Game").to_string_lossy());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loctable_uses_os_language_and_falls_back_to_base_language() {
        let root = temporary_directory("localized-name");
        let bundle = root.join("Calendar.app");
        let resources = bundle.join("Contents/Resources");
        std::fs::create_dir_all(&resources).unwrap();

        let mut japanese = Dictionary::new();
        japanese.insert(
            "CFBundleDisplayName".into(),
            Value::String("カレンダー".into()),
        );
        let mut english = Dictionary::new();
        english.insert(
            "CFBundleDisplayName".into(),
            Value::String("Calendar".into()),
        );
        let mut table = Dictionary::new();
        table.insert("ja".into(), Value::Dictionary(japanese));
        table.insert("en".into(), Value::Dictionary(english));
        Value::Dictionary(table)
            .to_file_binary(resources.join("InfoPlist.loctable"))
            .unwrap();

        assert_eq!(
            localized_loctable_value(&bundle, &["ja-JP".to_owned()], "CFBundleDisplayName"),
            Some("カレンダー".to_owned())
        );
        assert_eq!(
            localized_loctable_value(&bundle, &["en-US".to_owned()], "CFBundleDisplayName"),
            Some("Calendar".to_owned())
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_app_uses_descriptive_bundle_suffix_for_two_line_label() {
        let root = temporary_directory("descriptive-mac-name");
        let bundle = root.join("Adobe Premiere Pro 2026.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();

        let mut info = Dictionary::new();
        info.insert(
            "CFBundleIdentifier".into(),
            Value::String("com.adobe.PremierePro.26".into()),
        );
        info.insert(
            "CFBundleDisplayName".into(),
            Value::String("Adobe Premiere".into()),
        );
        info.insert(
            "CFBundleExecutable".into(),
            Value::String("Adobe Premiere Pro 2026".into()),
        );
        Value::Dictionary(info)
            .to_file_xml(bundle.join("Contents/Info.plist"))
            .unwrap();

        let entry = snapshot_entry(&bundle, &["ja-JP".to_owned()])
            .expect("native Mac app should be discovered");
        assert_eq!(entry.name, "Adobe Premiere Pro 2026");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn localized_name_wins_when_bundle_name_is_not_its_extension() {
        assert_eq!(
            prefer_descriptive_bundle_name(Path::new("Calendar.app"), "カレンダー".to_owned()),
            "カレンダー"
        );
    }

    #[test]
    fn system_calendar_uses_current_os_localization() {
        let bundle = Path::new("/System/Applications/Calendar.app");
        if !bundle.is_dir() {
            return;
        }
        let languages = preferred_languages();
        let expected = localized_bundle_name(bundle, &languages)
            .expect("Calendar should expose a localized display name");
        let entry = snapshot_entry(bundle, &languages).expect("Calendar should be discovered");
        eprintln!(
            "preferred_languages={languages:?} localized_calendar={}",
            entry.name
        );
        assert_eq!(entry.name, expected);
    }

    #[test]
    fn installed_premiere_keeps_edition_and_year_when_available() {
        let bundle = Path::new("/Applications/Adobe Premiere Pro 2026/Adobe Premiere Pro 2026.app");
        if !bundle.is_dir() {
            return;
        }
        let entry = snapshot_entry(bundle, &preferred_languages())
            .expect("installed Premiere should be discovered");
        assert_eq!(entry.name, "Adobe Premiere Pro 2026");
    }

    #[test]
    fn application_scan_keeps_installed_premiere_edition_and_year() {
        let bundle = Path::new("/Applications/Adobe Premiere Pro 2026/Adobe Premiere Pro 2026.app");
        if !bundle.is_dir() {
            return;
        }
        let id = AppId::from_normalized("mac:com.adobe.PremierePro.26".to_owned());
        let applications = scan_applications();
        let entry = applications
            .get(&id)
            .expect("installed Premiere should be included in the application scan");
        assert_eq!(entry.name, "Adobe Premiere Pro 2026");
        assert_eq!(Path::new(&entry.link_path), bundle);
    }

    #[test]
    fn workspace_extracts_a_nonempty_system_app_icon() {
        let bundle = [
            Path::new("/System/Applications/Calendar.app"),
            Path::new("/System/Library/CoreServices/Finder.app"),
        ]
        .into_iter()
        .find(|path| path.is_dir())
        .expect("macOS system app bundle should exist");

        let icon = extract_workspace_icon(bundle).expect("workspace icon should decode");
        assert!(icon.w >= TARGET);
        assert!(icon.h >= TARGET);
        assert_eq!(icon.rgba.len(), (icon.w * icon.h * 4) as usize);
        assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
    }
}
