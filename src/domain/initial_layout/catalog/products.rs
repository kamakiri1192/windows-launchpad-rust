//! Product allowlists. Never match a display name or a vendor prefix alone.
use super::{Rule, SnapshotEntry};

pub(super) fn append_rules(preset: &'static str, rules: &mut Vec<Rule>) {
    for &(group, aumid, product) in WINDOWS_PACKAGES {
        if group == preset {
            rules.push(Rule::WindowsPackage { aumid, product });
        }
    }
    let steam_ids: &[&str] = match preset {
        "vr-tools" => &["steam:250820", "steam:1173510"],
        "vr-games" => &["steam:438100", "steam:620980", "steam:546560"],
        _ => &[],
    };
    rules.extend(steam_ids.iter().map(|id| Rule::SteamApp(id)));
    if matches!(
        preset,
        "vr-tools"
            | "remote-desktop"
            | "windows-dev-tools"
            | "git-tools"
            | "unity-tools"
            | "live2d-cubism"
            | "razer-tools"
            | "windows-accessibility"
    ) {
        rules.push(Rule::RelatedWindows(preset));
    }
    if preset == "coreldraw-suite" {
        rules.push(Rule::MacCorelSuite);
    }
    for &(group, bundle, product) in MAC_BUNDLES {
        if group == preset {
            rules.push(Rule::MacBundle { bundle, product });
        }
    }
    for &(group, prefix, product) in MAC_VERSIONED_BUNDLES {
        if group == preset {
            rules.push(Rule::MacVersionedBundle { prefix, product });
        }
    }
    for &(group, executable, directory, product, versioned_executable) in WINDOWS_PRODUCTS {
        if group == preset {
            rules.push(Rule::WindowsProduct {
                executable,
                directory,
                product,
                versioned_executable,
            });
        }
    }
}

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
const WINDOWS_PRODUCTS: &[(&str, &str, &str, &str, bool)] = &[
    (
        "maxon-creative",
        "commandline.exe",
        "maxon cinema 4d",
        "commandline",
        false,
    ),
    (
        "maxon-creative",
        "cinema 4d team render client.exe",
        "maxon cinema 4d",
        "team-render-client",
        false,
    ),
    (
        "maxon-creative",
        "cinema 4d team render server.exe",
        "maxon cinema 4d",
        "team-render-server",
        false,
    ),
    (
        "affinity-legacy",
        "designer.exe",
        "affinity",
        "designer",
        false,
    ),
    ("affinity-legacy", "photo.exe", "affinity", "photo", false),
    (
        "affinity-legacy",
        "publisher.exe",
        "affinity",
        "publisher",
        false,
    ),
    ("coreldraw-suite", "coreldrw.exe", "corel", "draw", false),
    (
        "coreldraw-suite",
        "corelpp.exe",
        "corel",
        "photo-paint",
        false,
    ),
    (
        "cyberlink-director-suite",
        "pdr.exe",
        "cyberlink",
        "powerdirector",
        true,
    ),
    (
        "cyberlink-director-suite",
        "photodirector.exe",
        "cyberlink",
        "photodirector",
        true,
    ),
    (
        "cyberlink-director-suite",
        "audiodirector.exe",
        "cyberlink",
        "audiodirector",
        true,
    ),
    (
        "cyberlink-director-suite",
        "colordirector.exe",
        "cyberlink",
        "colordirector",
        true,
    ),
    (
        "blackmagic-creative",
        "resolve.exe",
        "blackmagic design",
        "resolve",
        false,
    ),
    (
        "blackmagic-creative",
        "fusion.exe",
        "blackmagic design",
        "fusion",
        false,
    ),
    (
        "maxon-creative",
        "cinema 4d.exe",
        "maxon cinema 4d",
        "cinema-4d",
        false,
    ),
    (
        "maxon-creative",
        "zbrush.exe",
        "maxon zbrush",
        "zbrush",
        false,
    ),
    ("maxon-creative", "zbrush.exe", "pixologic", "zbrush", false),
    ("maxon-creative", "zbrush.exe", "zbrush", "zbrush", false),
    (
        "steinberg-creative",
        "cubase.exe",
        "steinberg",
        "cubase",
        true,
    ),
    (
        "steinberg-creative",
        "dorico.exe",
        "steinberg",
        "dorico",
        false,
    ),
    (
        "steinberg-creative",
        "wavelab.exe",
        "steinberg",
        "wavelab",
        true,
    ),
    (
        "steinberg-creative",
        "wavelabpro.exe",
        "steinberg",
        "wavelab",
        true,
    ),
    (
        "steinberg-creative",
        "wavelabpro_.exe",
        "steinberg",
        "wavelab",
        true,
    ),
    (
        "native-instruments-standalone",
        "kontakt.exe",
        "native instruments",
        "kontakt",
        true,
    ),
    (
        "native-instruments-standalone",
        "kontakt 5.exe",
        "native instruments",
        "kontakt",
        false,
    ),
    (
        "native-instruments-standalone",
        "kontakt 7.exe",
        "native instruments",
        "kontakt",
        false,
    ),
    (
        "native-instruments-standalone",
        "kontakt 8.exe",
        "native instruments",
        "kontakt",
        false,
    ),
    (
        "native-instruments-standalone",
        "reaktor 5.exe",
        "native instruments",
        "reaktor",
        false,
    ),
    (
        "native-instruments-standalone",
        "reaktor 6.exe",
        "native instruments",
        "reaktor",
        false,
    ),
    (
        "autodesk-creative",
        "acad.exe",
        "autodesk",
        "autocad",
        false,
    ),
    ("autodesk-creative", "maya.exe", "autodesk", "maya", false),
    (
        "autodesk-creative",
        "3dsmax.exe",
        "autodesk",
        "3ds-max",
        false,
    ),
];

const MAC_BUNDLES: &[(&str, &str, &str)] = &[
    ("jetbrains-ides", "com.jetbrains.toolbox", "toolbox"),
    (
        "macos-utilities",
        "com.apple.ActivityMonitor",
        "activity-monitor",
    ),
    ("macos-utilities", "com.apple.Console", "console"),
    ("macos-utilities", "com.apple.DiskUtility", "disk-utility"),
    ("apple-documents", "com.apple.iWork.Pages", "pages"),
    ("apple-documents", "com.apple.iWork.Numbers", "numbers"),
    ("apple-documents", "com.apple.iWork.Keynote", "keynote"),
    ("apple-documents", "com.apple.Pages", "pages"),
    ("apple-documents", "com.apple.Numbers", "numbers"),
    ("apple-documents", "com.apple.Keynote", "keynote"),
    ("libreoffice", "org.libreoffice.script", "libreoffice"),
    ("adobe-creative-cloud", "com.adobe.Photoshop", "photoshop"),
    (
        "adobe-creative-cloud",
        "com.adobe.illustrator",
        "illustrator",
    ),
    (
        "adobe-creative-cloud",
        "com.adobe.AfterEffects",
        "after-effects",
    ),
    ("adobe-creative-cloud", "com.adobe.Audition", "audition"),
    (
        "affinity-legacy",
        "com.seriflabs.affinitydesigner",
        "designer",
    ),
    ("affinity-legacy", "com.seriflabs.affinityphoto", "photo"),
    (
        "affinity-legacy",
        "com.seriflabs.affinitypublisher",
        "publisher",
    ),
    (
        "affinity-legacy",
        "com.seriflabs.affinitydesigner2",
        "designer",
    ),
    ("affinity-legacy", "com.seriflabs.affinityphoto2", "photo"),
    (
        "affinity-legacy",
        "com.seriflabs.affinitypublisher2",
        "publisher",
    ),
    ("coreldraw-suite", "com.corel.CorelDRAW", "draw"),
    (
        "cyberlink-director-suite",
        "com.cyberlink.powerdirectormac",
        "powerdirector",
    ),
    (
        "cyberlink-director-suite",
        "com.cyberlink.photodirectormac",
        "photodirector",
    ),
    ("final-cut-suite", "com.apple.FinalCut", "final-cut"),
    ("final-cut-suite", "com.apple.FinalCutApp", "final-cut"),
    ("final-cut-suite", "com.apple.motionapp", "motion"),
    ("final-cut-suite", "com.apple.motionappApp", "motion"),
    ("final-cut-suite", "com.apple.Compressor", "compressor"),
    ("final-cut-suite", "com.apple.CompressorApp", "compressor"),
    ("logic-suite", "com.apple.logic10", "logic"),
    ("logic-suite", "com.apple.mainstage3", "mainstage"),
    (
        "blackmagic-creative",
        "com.blackmagic-design.DaVinciResolve",
        "resolve",
    ),
    (
        "blackmagic-creative",
        "com.blackmagic-design.DaVinciResolveLite",
        "resolve",
    ),
    (
        "blackmagic-creative",
        "com.blackmagic-design.fusion",
        "fusion",
    ),
    ("maxon-creative", "net.maxon.cinema4d", "cinema-4d"),
    ("maxon-creative", "com.pixologic.zbrush", "zbrush"),
    ("steinberg-creative", "com.steinberg.cubase", "cubase"),
    (
        "native-instruments-standalone",
        "com.native-instruments.Kontakt Application",
        "kontakt",
    ),
    (
        "native-instruments-standalone",
        "com.native-instruments.Reaktor 6",
        "reaktor",
    ),
    ("jetbrains-ides", "com.jetbrains.intellij", "idea"),
    ("jetbrains-ides", "com.jetbrains.intellij.ce", "idea"),
    ("jetbrains-ides", "com.jetbrains.pycharm", "pycharm"),
    ("jetbrains-ides", "com.jetbrains.pycharm.ce", "pycharm"),
    ("jetbrains-ides", "com.jetbrains.WebStorm", "webstorm"),
    ("jetbrains-ides", "com.jetbrains.PhpStorm", "phpstorm"),
    ("jetbrains-ides", "com.jetbrains.CLion", "clion"),
    ("jetbrains-ides", "com.jetbrains.goland", "goland"),
    ("jetbrains-ides", "com.jetbrains.rider", "rider"),
    ("jetbrains-ides", "com.jetbrains.rubymine", "rubymine"),
    ("jetbrains-ides", "com.jetbrains.datagrip", "datagrip"),
];

const MAC_VERSIONED_BUNDLES: &[(&str, &str, &str)] = &[
    (
        "adobe-creative-cloud",
        "com.adobe.PremierePro.",
        "premiere-pro",
    ),
    ("adobe-creative-cloud", "com.adobe.Audition.", "audition"),
    (
        "adobe-creative-cloud",
        "com.adobe.ame.application.",
        "media-encoder",
    ),
    ("steinberg-creative", "com.steinberg.cubase", "cubase"),
    ("steinberg-creative", "com.steinberg.dorico", "dorico"),
    ("steinberg-creative", "net.steinberg.WaveLab-", "wavelab"),
    (
        "native-instruments-standalone",
        "com.native-instruments.Kontakt ",
        "kontakt",
    ),
    ("autodesk-creative", "com.autodesk.AutoCAD", "autocad"),
    ("autodesk-creative", "com.autodesk.Maya.", "maya"),
];

// Verified via Windows AppsFolder/Get-StartApps, 2026-09-25.
const WINDOWS_PACKAGES: &[(&str, &str, &str)] = &[
    (
        "omen",
        "AD2F1837.OMENAudioControl_v10z8vjag6ke6!App",
        "audio-control",
    ),
    (
        "omen",
        "AD2F1837.OMENCommandCenter_v10z8vjag6ke6!AD2F1837.OMENCommandCenter",
        "gaming-hub",
    ),
    (
        "windows-dev-tools",
        "Microsoft.WinDbg_8wekyb3d8bbwe!Microsoft.WinDbg",
        "windbg",
    ),
    (
        "windows-dev-tools",
        "Microsoft.Windows.DevHome_8wekyb3d8bbwe!App",
        "dev-home",
    ),
];

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
