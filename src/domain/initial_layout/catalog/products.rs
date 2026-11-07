//! Product allowlists. Never match a display name or a vendor prefix alone.
use super::SnapshotEntry;

/// Additional suites verified from installed shortcut targets and arguments.
/// Shared shell/launcher executables are matched by their complete launch mode.
pub(super) fn related_windows(
    group: &str,
    file: &str,
    path: &str,
    args: &str,
    entry: &SnapshotEntry,
    root: Option<&str>,
) -> Option<String> {
    let has_dir = |dir| path.split('\\').any(|part| directory_matches(part, dir));
    let product = match group {
        "vr-tools" if args.is_empty() => match file {
            "client.exe" if path.ends_with(r"\meta horizon\support\oculus-client\client.exe") => {
                "meta-horizon-link"
            }
            "slimevr.exe" if has_dir("slimevr server") => "slimevr",
            "virtualdesktop.streamer.exe" if has_dir("virtual desktop streamer") => {
                "virtual-desktop"
            }
            "creatorcompanion.exe" if has_dir("vrchat creator companion") => "creator-companion",
            _ => return None,
        },
        "remote-desktop" => match (file, args.trim()) {
            ("parsecd.exe", "") if has_dir("parsec") => "parsec",
            ("paperspace.exe", "") if has_dir("paperspace") => "paperspace",
            (
                "chrome_proxy.exe",
                "--profile-directory=Default --app-id=cmkncekebbebpfilplodngbpllndjkfo",
            ) if path.ends_with(r"\google\chrome\application\chrome_proxy.exe") => {
                "chrome-remote-desktop"
            }
            ("mstsc.exe", "")
                if entry.publisher == "Microsoft Corporation"
                    && path == format!("{}\\system32\\mstsc.exe", root?) =>
            {
                "mstsc"
            }
            _ => return None,
        },
        "windows-dev-tools" => match (file, args) {
            ("setup.exe", "")
                if path.ends_with(r"\microsoft visual studio\installer\setup.exe") =>
            {
                "visual-studio-installer"
            }
            ("appcertui.exe", "")
                if path.ends_with(r"\windows kits\10\app certification kit\appcertui.exe") =>
            {
                "app-cert-kit"
            }
            ("explorer.exe", _)
                if entry.publisher == "Microsoft Corporation"
                    && path == format!("{}\\explorer.exe", root?)
                    && windows_sdk_directory(args) =>
            {
                "windows-sdk"
            }
            ("appverif.exe", "")
                if entry.publisher == "Microsoft Corporation"
                    && ["system32", "syswow64"].iter().any(|arch| {
                        root.is_some_and(|r| path == format!("{r}\\{arch}\\appverif.exe"))
                    }) =>
            {
                "app-verifier"
            }
            _ => return None,
        },
        "git-tools" if has_dir("git") => match (file, args) {
            ("git-bash.exe", "--cd-to-home" | "") => "bash",
            ("git-cmd.exe", "--cd-to-home" | "") => "cmd",
            ("git-gui.exe", "") => "gui",
            _ => return None,
        },
        "unity-tools" if args.is_empty() => match file {
            "unity.exe" if has_dir("unity") => "editor",
            "unity hub.exe" if has_dir("unity hub") => "hub",
            "unitybugreporter.exe" if has_dir("unity") => "bug-reporter",
            _ => return None,
        },
        "live2d-cubism" if args.is_empty() && has_dir("live2d cubism") => match file {
            "cubismeditor5.exe" => "editor",
            "cubismviewer5.exe" => "viewer",
            _ => return None,
        },
        "razer-tools" if has_dir("razer") => match (file, args.trim()) {
            ("razercortex.exe", "") => "cortex",
            ("razerappengine.exe", "--url-params=apps=synapse") => "synapse",
            _ => return None,
        },
        "windows-accessibility"
            if entry.publisher == "Microsoft Corporation" && args.is_empty() =>
        {
            let root = root?;
            if !["system32", "syswow64"]
                .iter()
                .any(|arch| path == format!("{root}\\{arch}\\{file}"))
            {
                return None;
            }
            match file {
                "osk.exe" | "narrator.exe" | "magnify.exe" | "livecaptions.exe"
                | "voiceaccess.exe" => file,
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(product.to_owned())
}

// Only numeric release suffixes; never accept Helper, Updater, plugins or editions
// through a broad starts_with match. A release is not a separate product.
pub(super) fn version_suffix(value: &str) -> bool {
    value
        .split(['.', 'v', '_'])
        .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
}

pub(super) fn directory_matches(value: &str, family: &str) -> bool {
    value == family
        || value
            .strip_prefix(family)
            .and_then(|s| s.strip_prefix(' '))
            .is_some_and(|s| version_suffix(s.strip_prefix('r').unwrap_or(s)))
}

pub(super) fn jetbrains_product(file: &str, entry: &SnapshotEntry) -> Option<&'static str> {
    if file == "jetbrains-toolbox.exe"
        && entry.identifier == "JetBrains Toolbox"
        && matches!(entry.publisher.as_str(), "JetBrains" | "JetBrains s.r.o.")
    {
        return Some("toolbox");
    }
    if entry.publisher != "JetBrains s.r.o." {
        return None;
    }
    JETBRAINS.iter().find_map(|&(exe, names, key)| {
        (file == exe && names.contains(&entry.identifier.as_str())).then_some(key)
    })
}

const JETBRAINS: &[(&str, &[&str], &str)] = &[
    (
        "idea64.exe",
        &[
            "IntelliJ IDEA",
            "IntelliJ IDEA Community Edition",
            "IntelliJ IDEA Ultimate Edition",
        ],
        "idea",
    ),
    (
        "pycharm64.exe",
        &[
            "PyCharm",
            "PyCharm Community Edition",
            "PyCharm Professional Edition",
        ],
        "pycharm",
    ),
    ("webstorm64.exe", &["WebStorm"], "webstorm"),
    ("phpstorm64.exe", &["PhpStorm"], "phpstorm"),
    ("clion64.exe", &["CLion"], "clion"),
    ("goland64.exe", &["GoLand"], "goland"),
    ("rider64.exe", &["Rider", "JetBrains Rider"], "rider"),
    ("rubymine64.exe", &["RubyMine"], "rubymine"),
    ("datagrip64.exe", &["DataGrip"], "datagrip"),
];

// (preset, exact executable, install directory component, stable product, numeric exe suffix)
fn windows_sdk_directory(args: &str) -> bool {
    let Some(path) = args.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return false;
    };
    let path = path.to_ascii_lowercase();
    path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && matches!(
            path.get(1..),
            Some(r":\program files (x86)\windows kits\10\" | r":\program files\windows kits\10\")
        )
}
