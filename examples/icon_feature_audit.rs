//! Export machine-learning-ready visual features for installed macOS app icons.
//!
//! Usage:
//!   cargo run --example icon_feature_audit -- --out /tmp/icon-feature-audit.json

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("icon-feature-audit: this tool is macOS-only.");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
mod macos_audit {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use serde::Serialize;

    use launchpad_windows::icons::features::{self, IconVisualFeatures, FEATURE_NAMES};
    use launchpad_windows::icons::normalize::DecodedIcon;
    use launchpad_windows::icons::sizing;

    #[derive(Debug, Serialize)]
    struct AuditReport {
        feature_names: &'static [&'static str],
        entries: Vec<AuditEntry>,
    }

    #[derive(Debug, Serialize)]
    struct AuditEntry {
        name: String,
        bundle_path: String,
        category: String,
        rule_scale: f64,
        features: IconVisualFeatures,
        feature_vector: Vec<f32>,
    }

    struct Args {
        output_path: PathBuf,
    }

    struct DiscoveredApp {
        bundle_path: PathBuf,
        display_name: String,
    }

    pub fn run() {
        let args = match parse_args() {
            Ok(args) => args,
            Err(error) => {
                eprintln!("error: {error}");
                std::process::exit(2);
            }
        };

        eprintln!("icon-feature-audit: scanning applications...");
        let apps = enumerate_apps();
        eprintln!("  found {} .app bundles", apps.len());

        let mut entries = Vec::with_capacity(apps.len());
        for (index, app) in apps.iter().enumerate() {
            if let Some(entry) = process_app(app) {
                entries.push(entry);
            }
            if (index + 1) % 50 == 0 || index + 1 == apps.len() {
                eprintln!("  {}/{}", index + 1, apps.len());
            }
        }

        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        let report = AuditReport {
            feature_names: &FEATURE_NAMES,
            entries,
        };
        let json = serde_json::to_string_pretty(&report).expect("serialize feature report");

        if let Some(parent) = args.output_path.parent() {
            std::fs::create_dir_all(parent).expect("create output directory");
        }
        std::fs::write(&args.output_path, json).expect("write feature report");
        eprintln!(
            "icon-feature-audit: wrote {} entries to {}",
            report.entries.len(),
            args.output_path.display()
        );
    }

    fn parse_args() -> Result<Args, String> {
        let mut output_path = PathBuf::from("./icon-feature-audit.json");
        let args: Vec<String> = std::env::args().collect();
        let mut index = 1usize;

        while index < args.len() {
            match args[index].as_str() {
                "--out" => {
                    index += 1;
                    let value = args.get(index).ok_or("--out requires a path")?;
                    output_path = PathBuf::from(value);
                }
                "--help" | "-h" => {
                    eprintln!(
                        "icon-feature-audit — export visual features for installed app icons\n\n\
                         Usage: cargo run --example icon_feature_audit -- [OPTIONS]\n\n\
                         Options:\n\
                           --out <file>  JSON output path (default: ./icon-feature-audit.json)\n\
                           --help        Show this help"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
            index += 1;
        }

        Ok(Args { output_path })
    }

    fn process_app(app: &DiscoveredApp) -> Option<AuditEntry> {
        let source = extract_icon_maxres(&app.bundle_path)?;
        let metrics = sizing::analyze(&source)?;
        let visual_features = features::extract(&source)?;

        Some(AuditEntry {
            name: app.display_name.clone(),
            bundle_path: app.bundle_path.to_string_lossy().into_owned(),
            category: metrics.category.as_str().to_owned(),
            rule_scale: metrics.scale,
            feature_vector: visual_features.as_array().to_vec(),
            features: visual_features,
        })
    }

    fn enumerate_apps() -> Vec<DiscoveredApp> {
        let mut apps = BTreeMap::<String, DiscoveredApp>::new();
        for root in app_roots() {
            scan_root(&root, &mut apps);
        }
        apps.into_values().collect()
    }

    fn app_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
        roots.push("/Applications".into());
        roots.push("/System/Applications".into());
        roots.push("/System/Applications/Utilities".into());
        roots
    }

    fn scan_root(root: &Path, apps: &mut BTreeMap<String, DiscoveredApp>) {
        let Ok(children) = std::fs::read_dir(root) else {
            return;
        };

        for child in children.flatten() {
            let path = child.path();
            let Ok(file_type) = child.file_type() else {
                continue;
            };
            if !file_type.is_dir()
                || file_type.is_symlink()
                || path.extension().is_none_or(|ext| ext != "app")
            {
                continue;
            }

            let name = path
                .file_stem()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            apps.entry(name.clone()).or_insert(DiscoveredApp {
                bundle_path: path,
                display_name: name,
            });
        }
    }

    fn extract_icon_maxres(bundle_path: &Path) -> Option<DecodedIcon> {
        use objc2::rc::autoreleasepool;
        use objc2::AnyThread;
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
        use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

        autoreleasepool(|_| {
            let path = NSString::from_str(&bundle_path.to_string_lossy());
            let image = NSWorkspace::sharedWorkspace().iconForFile(&path);
            let mut proposed = NSRect::new(NSPoint::ZERO, NSSize::new(512.0, 512.0));
            let cg_image =
                unsafe { image.CGImageForProposedRect_context_hints(&mut proposed, None, None)? };
            let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
            let properties: objc2::rc::Retained<
                objc2_foundation::NSDictionary<
                    objc2_app_kit::NSBitmapImageRepPropertyKey,
                    objc2::runtime::AnyObject,
                >,
            > = objc2_foundation::NSDictionary::new();
            let png = unsafe {
                bitmap
                    .representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)?
            };
            let png_bytes = unsafe { png.as_bytes_unchecked() };
            image::load_from_memory(png_bytes)
                .ok()
                .map(DecodedIcon::from_dynamic)
        })
    }
}

#[cfg(target_os = "macos")]
fn main() {
    macos_audit::run();
}
