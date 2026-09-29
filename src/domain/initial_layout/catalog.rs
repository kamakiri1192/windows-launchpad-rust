//! Embedded product catalog; identifiers and sources are documented in
//! docs/INITIAL_FOLDER_LAYOUT_CATALOG.md.
use super::super::app_diff::SnapshotEntry;
mod macos;
mod products;
pub(super) mod windows;

pub const VERSION: u32 = 6;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Macos,
}
impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Windows
        }
    }
}

#[derive(Debug, Clone)]
pub struct Preset {
    pub id: &'static str,
    pub japanese_name: &'static str,
    pub english_name: &'static str,
    pub platforms: &'static [Platform],
    pub evidence: &'static str,
    pub rules: Vec<Rule>,
}
impl Preset {
    pub fn name(&self, japanese: bool) -> &str {
        if japanese {
            self.japanese_name
        } else {
            self.english_name
        }
    }
}
#[derive(Debug, Clone)]
pub enum Rule {
    WindowsPackage {
        aumid: &'static str,
        product: &'static str,
    },
    SteamApp(&'static str),
    MacBundle {
        bundle: &'static str,
        product: &'static str,
    },
    MacVersionedBundle {
        prefix: &'static str,
        product: &'static str,
    },
    MacCorelSuite,
    WindowsProduct {
        executable: &'static str,
        directory: &'static str,
        product: &'static str,
        versioned_executable: bool,
    },
    WindowsOffice,
    WindowsSystem(&'static str),
    VisualStudio,
    LibreOffice,
    JetBrains,
    AdobeCreative,
    RelatedWindows(&'static str),
    Steam,
}
#[derive(Debug, Clone)]
pub struct Catalog {
    pub presets: Vec<Preset>,
    pub windows_directory: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Classification<'a> {
    Product { preset: &'a str, product: String },
    KeepTopLevel,
}

impl Catalog {
    pub fn bundled(windows_directory: Option<String>) -> Self {
        use Platform::{Macos as M, Windows as W};
        let definitions = [
            ("windows-tools", "PC管理", "PC Management", &[W][..]),
            (
                "windows-accessories",
                "Windowsアクセサリ",
                "Windows Accessories",
                &[W][..],
            ),
            (
                "macos-utilities",
                "macOSユーティリティ",
                "macOS Utilities",
                &[M][..],
            ),
            (
                "microsoft-office",
                "Microsoft Office",
                "Microsoft Office",
                &[W, M][..],
            ),
            (
                "apple-documents",
                "Apple文書アプリ",
                "Apple Documents",
                &[M][..],
            ),
            (
                "macos-accessories",
                "Macアクセサリ",
                "Mac Accessories",
                &[M][..],
            ),
            (
                "google-workspace",
                "Google Workspace",
                "Google Workspace",
                &[M][..],
            ),
            ("clip-studio", "CLIP STUDIO", "CLIP STUDIO", &[M][..]),
            ("vocaloid", "VOCALOID", "VOCALOID", &[M][..]),
            ("libreoffice", "LibreOffice", "LibreOffice", &[W, M][..]),
            (
                "adobe-creative-cloud",
                "Adobe Creative Cloud",
                "Adobe Creative Cloud",
                &[W, M][..],
            ),
            ("affinity-legacy", "Affinity", "Affinity", &[W, M][..]),
            ("coreldraw-suite", "CorelDRAW", "CorelDRAW", &[W, M][..]),
            (
                "cyberlink-director-suite",
                "CyberLink",
                "CyberLink",
                &[W, M][..],
            ),
            (
                "final-cut-suite",
                "Final Cut関連",
                "Final Cut Apps",
                &[M][..],
            ),
            ("logic-suite", "Logic関連", "Logic Apps", &[M][..]),
            (
                "blackmagic-creative",
                "Blackmagic Design",
                "Blackmagic Design",
                &[W, M][..],
            ),
            ("maxon-creative", "Maxon", "Maxon", &[W, M][..]),
            ("steinberg-creative", "Steinberg", "Steinberg", &[W, M][..]),
            (
                "native-instruments-standalone",
                "Native Instruments",
                "Native Instruments",
                &[W, M][..],
            ),
            ("git-tools", "Git", "Git", &[W][..]),
            ("vr-tools", "VRツール", "VR Tools", &[W][..]),
            ("vr-games", "VR Games", "VR Games", &[W][..]),
            (
                "remote-desktop",
                "リモートデスクトップ",
                "Remote Desktop",
                &[W, M][..],
            ),
            (
                "windows-dev-tools",
                "Windows Dev tools",
                "Windows Dev tools",
                &[W][..],
            ),
            ("omen", "OMEN", "OMEN", &[W][..]),
            ("unity-tools", "Unity", "Unity", &[W, M][..]),
            (
                "live2d-cubism",
                "Live2D Cubism",
                "Live2D Cubism",
                &[W, M][..],
            ),
            ("razer-tools", "Razer", "Razer", &[W][..]),
            (
                "windows-accessibility",
                "アクセシビリティ",
                "Accessibility",
                &[W][..],
            ),
            ("jetbrains-ides", "JetBrains", "JetBrains", &[W, M][..]),
            ("autodesk-creative", "Autodesk", "Autodesk", &[W, M][..]),
            (
                "visual-studio-tools",
                "Visual Studioツール",
                "Visual Studio Tools",
                &[W][..],
            ),
            (
                "steam-library",
                "Steamライブラリ",
                "Steam Library",
                &[W, M][..],
            ),
        ];
        let presets = definitions.into_iter().map(|(id, ja, en, platforms)| {
            let (evidence, rules) = match id {
                "windows-tools" | "windows-accessories" => ("Windows 11 local AppsFolder / Start Menu / version resources, 2026-09-25", vec![Rule::WindowsSystem(id)]),
                "visual-studio-tools" => ("Visual Studio 18 Build Tools local shortcuts, 2026-09-25", vec![Rule::VisualStudio]),
                "jetbrains-ides" => ("JetBrains launcher documentation and Homebrew signed bundle recipes", vec![Rule::JetBrains]),
                "adobe-creative-cloud" => ("Installed Adobe Windows 2025/2026 shortcuts and executable product/publisher metadata, 2026-09-25", vec![Rule::AdobeCreative]),
                "libreoffice" => ("https://help.libreoffice.org/latest/en-US/text/shared/guide/start_parameters.html", vec![Rule::LibreOffice]),
                "microsoft-office" => ("https://learn.microsoft.com/en-us/office/dev/add-ins/testing/sideload-an-office-add-in-on-mac and Microsoft Office command-line documentation", vec![Rule::WindowsOffice]),
                "steam-library" => ("src/workers/steam_scan.rs manifest records (not name-based)", vec![Rule::Steam]),
                _ => ("docs/INITIAL_FOLDER_LAYOUT_CATALOG.md", Vec::new()),
            };
            let mut rules = rules;
            products::append_rules(id, &mut rules);
            macos::append_rules(id, &mut rules);
            Preset { id, japanese_name: ja, english_name: en, platforms, evidence, rules }
        }).collect();
        Self {
            presets,
            windows_directory: windows_directory.map(|p| windows_path(&p)),
        }
    }

    pub fn classify(
        &self,
        entry: &SnapshotEntry,
        platform: Platform,
    ) -> Option<Classification<'_>> {
        let file = basename(&entry.target_path).to_lowercase();
        // Explicit exclusions run before Steam and product rules. Do not use publisher alone.
        if matches!(
            file.as_str(),
            "steam.exe"
                | "devenv.exe"
                | "code.exe"
                | "msedge.exe"
                | "onedrive.exe"
                | "teams.exe"
                | "ms-teams.exe"
                | "acrord32.exe"
                | "calc.exe"
        ) || matches!(
            entry.identifier.as_str(),
            "com.valvesoftware.steam"
                | "com.microsoft.VSCode"
                | "com.microsoft.teams"
                | "com.microsoft.OneDrive"
                | "com.microsoft.edgemac"
        ) {
            return Some(Classification::KeepTopLevel);
        }
        // Explicit VR app IDs take precedence; unlisted Steam titles stay in the library.
        if entry.app_id.is_steam() {
            for preset in &self.presets {
                if preset.platforms.contains(&platform)
                    && preset.rules.iter().any(
                        |rule| matches!(rule, Rule::SteamApp(id) if entry.app_id.as_str() == *id),
                    )
                {
                    return Some(Classification::Product {
                        preset: preset.id,
                        product: entry.app_id.to_string(),
                    });
                }
            }
            return Some(Classification::Product {
                preset: "steam-library",
                product: entry.app_id.to_string(),
            });
        }
        let mut matches = Vec::new();
        for preset in &self.presets {
            if !preset.platforms.contains(&platform) {
                continue;
            }
            for rule in &preset.rules {
                if let Some(product) = self.match_rule(rule, entry, platform) {
                    let priority = if matches!(rule, Rule::WindowsSystem(_)) {
                        1
                    } else {
                        0
                    };
                    matches.push((priority, preset.id, product));
                }
            }
        }
        let priority = matches.iter().map(|m| m.0).min()?;
        matches.retain(|m| m.0 == priority);
        let first = matches.first()?;
        if matches.iter().any(|m| m.1 != first.1 || m.2 != first.2) {
            return None;
        }
        Some(Classification::Product {
            preset: first.1,
            product: first.2.clone(),
        })
    }

    fn match_rule(&self, rule: &Rule, e: &SnapshotEntry, platform: Platform) -> Option<String> {
        if let Rule::MacBundle { bundle, product } = rule {
            return (platform == Platform::Macos && e.identifier == *bundle)
                .then(|| product.to_string());
        }
        if let Rule::MacVersionedBundle { prefix, product } = rule {
            return (platform == Platform::Macos
                && e.identifier
                    .strip_prefix(prefix)
                    .is_some_and(products::version_suffix))
            .then(|| product.to_string());
        }
        if matches!(rule, Rule::MacCorelSuite) && platform == Platform::Macos {
            let (year, app) = e
                .identifier
                .strip_prefix("com.corel.coreldrawsuite.")?
                .split_once('.')?;
            if year.len() == 4 && year.bytes().all(|c| c.is_ascii_digit()) {
                return match app {
                    "coreldraw" => Some("draw".into()),
                    "photopaint" => Some("photo-paint".into()),
                    _ => None,
                };
            }
        }
        if platform != Platform::Windows {
            return None;
        }
        if let Rule::WindowsPackage { aumid, product } = rule {
            return (entry_aumid(e) == Some(*aumid)).then(|| product.to_string());
        }
        if let Rule::WindowsSystem(group) = rule {
            return windows::identify(e, self.windows_directory.as_deref())
                .filter(|(_, found)| *found == Some(*group))
                .map(|(product, _)| product.to_owned());
        }
        let path = windows_path(&e.target_path);
        let file = basename(&path);
        let args = e.launch_arguments.as_deref()?;
        match rule {
            Rule::RelatedWindows(group) => products::related_windows(
                group,
                file,
                &path,
                args,
                e,
                self.windows_directory.as_deref(),
            ),
            Rule::WindowsProduct {
                executable,
                directory,
                product,
                versioned_executable,
            } if args.is_empty()
                && path
                    .split('\\')
                    .any(|part| products::directory_matches(part, directory))
                && (file == *executable
                    || (*versioned_executable
                        && file
                            .strip_suffix(".exe")
                            .and_then(|s| s.strip_prefix(executable.trim_end_matches(".exe")))
                            .is_some_and(products::version_suffix))) =>
            {
                Some(product.to_string())
            }
            Rule::AdobeCreative if args.is_empty() => {
                adobe_creative_product(file, e).map(str::to_owned)
            }
            Rule::WindowsOffice
                if args.is_empty()
                    && path.split('\\').any(|c| c == "microsoft office")
                    && matches!(file, "winword.exe" | "excel.exe" | "powerpnt.exe") =>
            {
                Some(file.to_owned())
            }
            Rule::VisualStudio
                if file == "cmd.exe"
                    && self.is_windows_binary(&path)
                    && e.publisher == "Microsoft Corporation" =>
            {
                visual_studio_feature(args)
            }
            Rule::VisualStudio
                if file == "powershell.exe"
                    && self.windows_directory.as_ref().is_some_and(|root| {
                        ["system32", "syswow64"].iter().any(|arch| {
                            path == format!(
                                "{root}\\{arch}\\windowspowershell\\v1.0\\powershell.exe"
                            )
                        })
                    })
                    && e.publisher == "Microsoft Corporation" =>
            {
                visual_studio_powershell(args)
            }
            Rule::LibreOffice
                if file == "soffice.exe"
                    && path.split('\\').any(|c| c == "libreoffice")
                    && matches!(
                        args,
                        "--writer" | "--calc" | "--impress" | "--draw" | "--math" | "--base"
                    ) =>
            {
                Some(args.to_owned())
            }
            Rule::LibreOffice
                if args.is_empty() && path.split('\\').any(|c| c == "libreoffice") =>
            {
                match file {
                    "swriter.exe" => Some("--writer".into()),
                    "scalc.exe" => Some("--calc".into()),
                    "simpress.exe" => Some("--impress".into()),
                    "sdraw.exe" => Some("--draw".into()),
                    "smath.exe" => Some("--math".into()),
                    "sbase.exe" => Some("--base".into()),
                    _ => None,
                }
            }
            Rule::JetBrains if args.is_empty() => {
                products::jetbrains_product(file, e).map(str::to_owned)
            }
            _ => None,
        }
    }

    fn is_windows_binary(&self, path: &str) -> bool {
        let Some(root) = &self.windows_directory else {
            return false;
        };
        let Some((parent, _)) = path.rsplit_once('\\') else {
            return false;
        };
        parent == *root
            || parent == format!("{root}\\system32")
            || parent == format!("{root}\\syswow64")
    }
}

fn adobe_creative_product(file: &str, entry: &SnapshotEntry) -> Option<&'static str> {
    if !matches!(
        entry.publisher.as_str(),
        "Adobe" | "Adobe Inc." | "Adobe Systems Incorporated"
    ) {
        return None;
    }
    // Exact executable names and product families observed on installed apps.
    // Strip only a four-digit release year, so versions share one product key
    // while similarly named editions, helpers and updaters stay outside.
    if file == "creative cloud.exe" && entry.identifier == "Creative Cloud Desktop" {
        return Some("creative-cloud");
    }
    let (family, year) = entry.identifier.rsplit_once(' ')?;
    if year.len() != 4 || !year.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match (file, family) {
        ("afterfx.exe", "Adobe After Effects") => Some("after-effects"),
        ("adobe audition.exe", "Adobe Audition") => Some("audition"),
        ("illustrator.exe", "Adobe Illustrator") => Some("illustrator"),
        ("adobe media encoder.exe", "Adobe Media Encoder") => Some("media-encoder"),
        ("photoshop.exe", "Adobe Photoshop") => Some("photoshop"),
        ("adobe premiere pro.exe", "Adobe Premiere Pro" | "Adobe Premiere") => Some("premiere-pro"),
        _ => None,
    }
}

fn visual_studio_feature(args: &str) -> Option<String> {
    let command = args.strip_prefix("/k ")?.trim();
    // Only a single quoted, verified vcvars script: arbitrary commands remain outside.
    let script = command.strip_prefix('"')?.strip_suffix('"')?;
    let script = windows_path(script);
    if script.contains("\\microsoft visual studio\\")
        && script.ends_with("\\common7\\tools\\vsdevcmd.bat")
        && !script.contains(['&', '|', '"', ';'])
    {
        return Some("developer-command-prompt".into());
    }
    if !script.contains("\\microsoft visual studio\\")
        || !script.contains("\\vc\\auxiliary\\build\\")
    {
        return None;
    }
    let file = basename(&script);
    matches!(
        file,
        "vcvars64.bat" | "vcvars32.bat" | "vcvarsamd64_x86.bat" | "vcvarsx86_amd64.bat"
    )
    .then(|| file.to_owned())
}

fn visual_studio_powershell(args: &str) -> Option<String> {
    // The Start Menu shortcut emitted by VS 18 Build Tools. Match the entire
    // command, including the instance ID; don't classify arbitrary shell code
    // merely because it contains the name of the developer shell module.
    let normalized = args.to_ascii_lowercase();
    let command = normalized.strip_prefix("-noe -c \"&{import-module \"\"\"")?;
    let (module, instance) = command.split_once("\"\"\"; enter-vsdevshell ")?;
    let instance = instance.strip_suffix("}\"")?;
    let module = windows_path(module);
    (module.contains("\\microsoft visual studio\\")
        && module.ends_with("\\common7\\tools\\microsoft.visualstudio.devshell.dll")
        && !module.contains(['&', '|', '"', ';'])
        && instance.len() == 8
        && instance.bytes().all(|c| c.is_ascii_hexdigit()))
    .then(|| "developer-powershell".into())
}

fn basename(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or("")
}
fn windows_path(path: &str) -> String {
    path.replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// Package launch identity comes from the shell AUMID, never the display label.
pub(super) fn entry_aumid(entry: &SnapshotEntry) -> Option<&str> {
    let id = entry.link_path.strip_prefix(r"shell:AppsFolder\")?;
    let (package, app) = id.split_once('!')?;
    (!package.is_empty() && !app.is_empty() && !id.contains(['\\', '/', ' '])).then_some(id)
}

pub(super) fn entry_shell_id(entry: &SnapshotEntry) -> Option<&str> {
    entry
        .link_path
        .strip_prefix(r"shell:AppsFolder\")
        .filter(|id| !id.is_empty() && !id.contains(['\\', '/', ' ']))
}
