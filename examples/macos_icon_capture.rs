//! Capture macOS app icons through the production NSWorkspace path.
//!
//! Usage:
//!   cargo run --example macos_icon_capture -- --out target/macos-icon-capture
//!   cargo run --example macos_icon_capture -- --app "/path/App.app" --out <dir>
//!
//! The output is intentionally a small, portable fixture: the source bitmap,
//! the normalized bitmap, and JSON metadata containing the OS/build and alpha
//! geometry. It is used by the macOS compatibility workflow.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("macos-icon-capture: this tool is macOS-only.");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use image::{ImageFormat, RgbaImage};
    use launchpad_windows::icons::normalize::{self, DecodedIcon};
    use serde::Serialize;

    const TARGET: u32 = 128;
    const METADATA_VERSION: u32 = 1;

    #[derive(Debug)]
    struct Args {
        out_dir: PathBuf,
        apps: Vec<PathBuf>,
    }

    #[derive(Debug, Serialize)]
    struct CaptureMetadata {
        metadata_version: u32,
        os_product_version: String,
        os_build: String,
        architecture: String,
        app_path: String,
        app_name: String,
        source: ImageStats,
        normalized: ImageStats,
        category: String,
        scale: f64,
    }

    #[derive(Debug, Serialize)]
    struct ImageStats {
        width: u32,
        height: u32,
        zero_alpha_pixels: usize,
        partial_alpha_pixels: usize,
        opaque_pixels: usize,
        min_alpha: u8,
        bbox_alpha_gt_10: Option<BoundingBox>,
        bbox_alpha_ge_128: Option<BoundingBox>,
    }

    #[derive(Debug, Serialize)]
    struct BoundingBox {
        min_x: u32,
        min_y: u32,
        max_x: u32,
        max_y: u32,
        width: u32,
        height: u32,
    }

    pub fn run() -> Result<(), String> {
        let args = parse_args()?;
        std::fs::create_dir_all(&args.out_dir)
            .map_err(|error| format!("create output directory: {error}"))?;

        let (os_product_version, os_build) = os_version();
        let apps = if args.apps.is_empty() {
            default_apps()
        } else {
            args.apps
        };
        if apps.is_empty() {
            return Err("no app bundles found".into());
        }

        for app_path in apps {
            capture_app(&app_path, &args.out_dir, &os_product_version, &os_build)?;
        }
        Ok(())
    }

    fn parse_args() -> Result<Args, String> {
        let mut out_dir = PathBuf::from("target/macos-icon-capture");
        let mut apps = Vec::new();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--out" => {
                    out_dir = PathBuf::from(
                        args.next()
                            .ok_or_else(|| "--out requires a path".to_owned())?,
                    );
                }
                "--app" => {
                    apps.push(PathBuf::from(
                        args.next()
                            .ok_or_else(|| "--app requires a bundle path".to_owned())?,
                    ));
                }
                "--help" | "-h" => {
                    println!(
                        "Usage: macos_icon_capture [--out DIR] [--app APP_BUNDLE]...\n\n\
                         With no --app, captures App Store and Activity Monitor."
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(Args { out_dir, apps })
    }

    fn default_apps() -> Vec<PathBuf> {
        [
            "/System/Applications/App Store.app",
            "/System/Applications/Utilities/Activity Monitor.app",
        ]
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .collect()
    }

    fn capture_app(
        app_path: &Path,
        out_dir: &Path,
        os_product_version: &str,
        os_build: &str,
    ) -> Result<(), String> {
        if !app_path.is_dir() {
            return Err(format!("app bundle does not exist: {}", app_path.display()));
        }
        let source = extract_workspace_icon(app_path)
            .ok_or_else(|| format!("could not extract icon: {}", app_path.display()))?;
        let normalized = normalize::normalize(&source);
        let app_name = app_path
            .file_stem()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "app".to_owned());
        let app_dir = out_dir.join(slugify(&app_name));
        std::fs::create_dir_all(&app_dir)
            .map_err(|error| format!("create {}: {error}", app_dir.display()))?;

        save_png(&source, &app_dir.join("source.png"))?;
        save_png(&normalized.image, &app_dir.join("normalized.png"))?;
        let metadata = CaptureMetadata {
            metadata_version: METADATA_VERSION,
            os_product_version: os_product_version.to_owned(),
            os_build: os_build.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            app_path: app_path.to_string_lossy().into_owned(),
            app_name,
            source: image_stats(&source),
            normalized: image_stats(&normalized.image),
            category: normalized.category.as_str().to_owned(),
            scale: normalized.scale,
        };
        let metadata_bytes = serde_json::to_vec_pretty(&metadata)
            .map_err(|error| format!("serialize metadata: {error}"))?;
        std::fs::write(app_dir.join("metadata.json"), metadata_bytes)
            .map_err(|error| format!("write metadata: {error}"))?;
        println!("captured {} -> {}", app_path.display(), app_dir.display());
        Ok(())
    }

    fn save_png(icon: &DecodedIcon, path: &Path) -> Result<(), String> {
        let image = RgbaImage::from_raw(icon.w, icon.h, icon.rgba.clone())
            .ok_or_else(|| format!("invalid RGBA buffer for {}", path.display()))?;
        image
            .save_with_format(path, ImageFormat::Png)
            .map_err(|error| format!("write {}: {error}", path.display()))
    }

    fn image_stats(icon: &DecodedIcon) -> ImageStats {
        let mut zero_alpha_pixels = 0;
        let mut partial_alpha_pixels = 0;
        let mut opaque_pixels = 0;
        let mut min_alpha = u8::MAX;
        for pixel in icon.rgba.chunks_exact(4) {
            let alpha = pixel[3];
            min_alpha = min_alpha.min(alpha);
            match alpha {
                0 => zero_alpha_pixels += 1,
                255 => opaque_pixels += 1,
                _ => partial_alpha_pixels += 1,
            }
        }
        ImageStats {
            width: icon.w,
            height: icon.h,
            zero_alpha_pixels,
            partial_alpha_pixels,
            opaque_pixels,
            min_alpha,
            bbox_alpha_gt_10: bounding_box(icon, 11),
            bbox_alpha_ge_128: bounding_box(icon, 128),
        }
    }

    fn bounding_box(icon: &DecodedIcon, threshold: u8) -> Option<BoundingBox> {
        let mut min_x = icon.w;
        let mut min_y = icon.h;
        let mut max_x = 0;
        let mut max_y = 0;
        let mut found = false;
        for y in 0..icon.h {
            for x in 0..icon.w {
                let index = ((y * icon.w + x) * 4 + 3) as usize;
                if icon.rgba[index] >= threshold {
                    found = true;
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        found.then(|| BoundingBox {
            min_x,
            min_y,
            max_x,
            max_y,
            width: max_x - min_x + 1,
            height: max_y - min_y + 1,
        })
    }

    fn slugify(name: &str) -> String {
        let mut slug = String::new();
        for character in name.chars() {
            if character.is_ascii_alphanumeric() {
                slug.push(character.to_ascii_lowercase());
            } else if !slug.ends_with('-') {
                slug.push('-');
            }
        }
        slug.trim_matches('-').to_owned()
    }

    fn os_version() -> (String, String) {
        let output = Command::new("sw_vers")
            .args(["-productVersion"])
            .output()
            .ok();
        let product = output
            .as_ref()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unknown".to_owned());
        let output = Command::new("sw_vers")
            .args(["-buildVersion"])
            .output()
            .ok();
        let build = output
            .as_ref()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unknown".to_owned());
        (product, build)
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
                cg_image = unsafe {
                    image.CGImageForProposedRect_context_hints(&mut proposed, None, None)?
                };
            }
            let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg_image);
            let properties: objc2::rc::Retained<
                NSDictionary<NSBitmapImageRepPropertyKey, AnyObject>,
            > = NSDictionary::new();
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
    if let Err(error) = macos::run() {
        eprintln!("macos-icon-capture: {error}");
        std::process::exit(1);
    }
}
