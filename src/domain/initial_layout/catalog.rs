//! Embedded product catalog; identifiers and sources are documented in
//! docs/INITIAL_FOLDER_LAYOUT_CATALOG.md.
use super::super::app_diff::SnapshotEntry;

mod products;
pub(super) mod windows;

pub const VERSION: u32 = 7;
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Preset {
    pub id: String,
    pub japanese_name: String,
    pub english_name: String,
    pub platforms: Vec<Platform>,
    pub evidence: String,
    pub rules: Vec<Rule>,
}
impl Preset {
    pub fn name(&self, japanese: bool) -> &str {
        if japanese {
            &self.japanese_name
        } else {
            &self.english_name
        }
    }
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Rule {
    WindowsPackage {
        aumid: String,
        product: String,
    },
    SteamApp {
        id: String,
    },
    MacBundle {
        bundle: String,
        product: String,
    },
    MacVersionedBundle {
        prefix: String,
        product: String,
    },
    MacBundleInDirectory {
        bundle: String,
        directory: String,
        product: String,
    },
    MacCorelSuite,
    WindowsProduct {
        executable: String,
        directory: String,
        product: String,
        versioned_executable: bool,
    },
    WindowsMetadata {
        executable: String,
        publisher: String,
        product_name: String,
        product: String,
    },
    WindowsOffice,
    WindowsSystem {
        group: String,
    },
    VisualStudio,
    LibreOffice,
    JetBrains,
    AdobeCreative,
    RelatedWindows {
        group: String,
    },
    Steam,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Catalog {
    pub version: u32,
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
        super::document::CatalogDocument::bundled().into_catalog(windows_directory)
    }

    pub fn classify(
        &self,
        entry: &SnapshotEntry,
        platform: Platform,
    ) -> Option<Classification<'_>> {
        let file = basename(&entry.target_path).to_lowercase();
        // Explicit exclusions run before Steam and product rules. Do not use publisher alone.
        if (platform == Platform::Windows
            && matches!(
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
            ))
            || (platform == Platform::Macos
                && matches!(
                    entry.identifier.as_str(),
                    "com.apple.AppStore"
                        | "com.apple.Safari"
                        | "com.google.Chrome"
                        | "com.microsoft.edgemac"
                        | "com.apple.systempreferences"
                        | "com.apple.calculator"
                        | "com.valvesoftware.steam"
                ))
            || (platform == Platform::Windows
                && matches!(
                    entry.identifier.as_str(),
                    "com.valvesoftware.steam"
                        | "com.microsoft.VSCode"
                        | "com.microsoft.teams"
                        | "com.microsoft.OneDrive"
                        | "com.microsoft.edgemac"
                ))
        {
            return Some(Classification::KeepTopLevel);
        }
        // Explicit VR app IDs take precedence; unlisted Steam titles stay in the library.
        if entry.app_id.is_steam() {
            for preset in &self.presets {
                if preset.platforms.contains(&platform)
                    && preset.rules.iter().any(
                        |rule| matches!(rule, Rule::SteamApp { id } if entry.app_id.as_str() == *id),
                    )
                {
                    return Some(Classification::Product {
                        preset: &preset.id,
                        product: entry.app_id.to_string(),
                    });
                }
            }
            return self
                .presets
                .iter()
                .find(|p| {
                    p.platforms.contains(&platform)
                        && p.rules.iter().any(|r| matches!(r, Rule::Steam))
                })
                .map(|p| Classification::Product {
                    preset: &p.id,
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
                    let priority = if matches!(rule, Rule::WindowsSystem { .. }) {
                        1
                    } else {
                        0
                    };
                    matches.push((priority, preset.id.as_str(), product));
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
        if let Rule::MacBundleInDirectory {
            bundle,
            directory,
            product,
        } = rule
        {
            // Script Editor-generated IDs alone do not establish suite ownership.
            return (platform == Platform::Macos
                && e.identifier == *bundle
                && std::path::Path::new(&e.link_path).components().any(|part| {
                    part.as_os_str().to_str().is_some_and(|part| {
                        part == *directory
                            || part.strip_prefix(directory.as_str()).is_some_and(|suffix| {
                                suffix.strip_prefix(' ').is_some_and(|year| {
                                    year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit())
                                })
                            })
                    })
                }))
            .then(|| product.to_string());
        }
        if let Rule::MacBundle { bundle, product } = rule {
            return (platform == Platform::Macos && e.identifier == *bundle)
                .then(|| product.to_string());
        }
        if let Rule::MacVersionedBundle { prefix, product } = rule {
            return (platform == Platform::Macos
                && e.identifier
                    .strip_prefix(prefix.as_str())
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
            return (entry_aumid(e) == Some(aumid.as_str())).then(|| product.to_string());
        }
        if let Rule::WindowsSystem { group } = rule {
            return windows::identify(e, self.windows_directory.as_deref())
                .filter(|(_, found)| *found == Some(group.as_str()))
                .map(|(product, _)| product.to_owned());
        }
        let path = windows_path(&e.target_path);
        let file = basename(&path);
        let args = e.launch_arguments.as_deref()?;
        match rule {
            Rule::RelatedWindows { group } => products::related_windows(
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
            Rule::WindowsMetadata {
                executable,
                publisher,
                product_name,
                product,
            } if args.is_empty()
                && file == executable
                && e.publisher == *publisher
                && e.identifier == *product_name =>
            {
                Some(product.clone())
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
