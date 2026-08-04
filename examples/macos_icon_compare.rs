//! Compare icon capture fixtures from multiple macOS versions.
//!
//! Usage:
//!   cargo run --example macos_icon_compare -- \
//!     --root target/macos-captures --baseline macos-14
//!
//! Differences are reported, not treated as failures. Missing or malformed
//! fixtures fail the command; pixel differences are the result being measured.

use std::path::{Path, PathBuf};

use image::{imageops, ImageFormat, Rgba, RgbaImage};
use serde::Serialize;

#[derive(Debug)]
struct Args {
    root: PathBuf,
    baseline: String,
    report: PathBuf,
    preview: PathBuf,
}

#[derive(Debug, Serialize)]
struct AppComparison {
    app: String,
    candidate: String,
    baseline_os: String,
    candidate_os: String,
    source: ImageComparison,
    normalized: ImageComparison,
}

#[derive(Debug, Serialize)]
struct ImageComparison {
    dimensions_equal: bool,
    png_bytes_equal: bool,
    rgba_bytes_equal: bool,
    changed_pixels: u64,
    total_pixels: u64,
    mean_absolute_error: f64,
    max_absolute_error: u8,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("macos-icon-compare: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let baseline_dir = args.root.join(&args.baseline);
    if !baseline_dir.is_dir() {
        return Err(format!(
            "baseline directory does not exist: {}",
            baseline_dir.display()
        ));
    }

    let mut candidates = std::fs::read_dir(&args.root)
        .map_err(|error| format!("read {}: {error}", args.root.display()))?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != &args.baseline)
        .collect::<Vec<_>>();
    candidates.sort();
    if candidates.is_empty() {
        return Err("no candidate OS capture directories found".into());
    }

    let app_names = fixture_names(&baseline_dir)?;
    if app_names.is_empty() {
        return Err(format!(
            "no app fixtures found in {}",
            baseline_dir.display()
        ));
    }

    let mut comparisons = Vec::new();
    for candidate in candidates {
        let candidate_dir = args.root.join(&candidate);
        for app in &app_names {
            let baseline_app = baseline_dir.join(app);
            let candidate_app = candidate_dir.join(app);
            if !candidate_app.is_dir() {
                return Err(format!(
                    "candidate {candidate} is missing app fixture {app}"
                ));
            }
            comparisons.push(compare_app(
                app,
                &args.baseline,
                &candidate,
                &baseline_app,
                &candidate_app,
            )?);
        }
    }

    let report = render_report(&args.baseline, &comparisons);
    std::fs::write(&args.report, &report)
        .map_err(|error| format!("write {}: {error}", args.report.display()))?;
    let json_path = args.report.with_extension("json");
    let json = serde_json::to_vec_pretty(&comparisons)
        .map_err(|error| format!("serialize comparison JSON: {error}"))?;
    std::fs::write(&json_path, json)
        .map_err(|error| format!("write {}: {error}", json_path.display()))?;
    write_preview(&args.root, &args.baseline, &comparisons, &args.preview)?;
    print!("{report}");
    Ok(())
}

fn parse_args() -> Result<Args, String> {
    let mut root = PathBuf::from("target/macos-captures");
    let mut baseline = "macos-14".to_owned();
    let mut report = PathBuf::from("target/macos-captures/compatibility-report.md");
    let mut preview = PathBuf::from("target/macos-captures/compatibility-preview.png");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => {
                root = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--root requires a directory".to_owned())?,
                );
            }
            "--baseline" => {
                baseline = args
                    .next()
                    .ok_or_else(|| "--baseline requires a capture name".to_owned())?;
            }
            "--report" => {
                report = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--report requires a file".to_owned())?,
                );
            }
            "--preview" => {
                preview = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--preview requires a file".to_owned())?,
                );
            }
            "--help" | "-h" => {
                println!(
                    "Usage: macos_icon_compare [--root DIR] [--baseline NAME] [--report FILE] [--preview FILE]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        root,
        baseline,
        report,
        preview,
    })
}

fn fixture_names(root: &Path) -> Result<Vec<String>, String> {
    let mut names = std::fs::read_dir(root)
        .map_err(|error| format!("read {}: {error}", root.display()))?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

fn compare_app(
    app: &str,
    baseline_name: &str,
    candidate_name: &str,
    baseline_dir: &Path,
    candidate_dir: &Path,
) -> Result<AppComparison, String> {
    let baseline_os = metadata_os(baseline_dir)?;
    let candidate_os = metadata_os(candidate_dir)?;
    Ok(AppComparison {
        app: app.to_owned(),
        candidate: candidate_name.to_owned(),
        baseline_os: format!("{baseline_name} / {baseline_os}"),
        candidate_os: format!("{candidate_name} / {candidate_os}"),
        source: compare_image_files(
            &baseline_dir.join("source.png"),
            &candidate_dir.join("source.png"),
        )?,
        normalized: compare_image_files(
            &baseline_dir.join("normalized.png"),
            &candidate_dir.join("normalized.png"),
        )?,
    })
}

fn metadata_os(app_dir: &Path) -> Result<String, String> {
    let path = app_dir.join("metadata.json");
    let bytes =
        std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    let product = value
        .get("os_product_version")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let build = value
        .get("os_build")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    Ok(format!("{product} ({build})"))
}

fn compare_image_files(baseline: &Path, candidate: &Path) -> Result<ImageComparison, String> {
    let baseline_bytes =
        std::fs::read(baseline).map_err(|error| format!("read {}: {error}", baseline.display()))?;
    let candidate_bytes = std::fs::read(candidate)
        .map_err(|error| format!("read {}: {error}", candidate.display()))?;
    let baseline_image = image::load_from_memory(&baseline_bytes)
        .map_err(|error| format!("decode {}: {error}", baseline.display()))?
        .to_rgba8();
    let candidate_image = image::load_from_memory(&candidate_bytes)
        .map_err(|error| format!("decode {}: {error}", candidate.display()))?
        .to_rgba8();
    compare_images(
        &baseline_image,
        &candidate_image,
        baseline_bytes == candidate_bytes,
    )
}

fn compare_images(
    baseline: &RgbaImage,
    candidate: &RgbaImage,
    png_bytes_equal: bool,
) -> Result<ImageComparison, String> {
    if baseline.dimensions() != candidate.dimensions() {
        return Ok(ImageComparison {
            dimensions_equal: false,
            png_bytes_equal,
            rgba_bytes_equal: false,
            changed_pixels: 0,
            total_pixels: 0,
            mean_absolute_error: 0.0,
            max_absolute_error: 0,
        });
    }

    let baseline_raw = baseline.as_raw();
    let candidate_raw = candidate.as_raw();
    let mut changed_pixels = 0u64;
    let mut absolute_error = 0u64;
    let mut max_absolute_error = 0u8;
    for (left, right) in baseline_raw
        .chunks_exact(4)
        .zip(candidate_raw.chunks_exact(4))
    {
        let mut pixel_changed = false;
        for (&left, &right) in left.iter().zip(right) {
            let difference = left.abs_diff(right);
            pixel_changed |= difference != 0;
            absolute_error += u64::from(difference);
            max_absolute_error = max_absolute_error.max(difference);
        }
        if pixel_changed {
            changed_pixels += 1;
        }
    }
    let total_pixels = u64::from(baseline.width()) * u64::from(baseline.height());
    let mean_absolute_error = if baseline_raw.is_empty() {
        0.0
    } else {
        absolute_error as f64 / baseline_raw.len() as f64
    };
    Ok(ImageComparison {
        dimensions_equal: true,
        png_bytes_equal,
        rgba_bytes_equal: baseline_raw == candidate_raw,
        changed_pixels,
        total_pixels,
        mean_absolute_error,
        max_absolute_error,
    })
}

fn render_report(baseline: &str, comparisons: &[AppComparison]) -> String {
    let mut report = String::from("# macOS icon compatibility\n\n");
    report.push_str(&format!("Baseline capture: `{baseline}`\n\n"));
    report.push_str(
        "Pixel differences are informational. Missing fixtures or malformed images fail the command.\n\n",
    );
    report.push_str(
        "| App | Candidate OS | Source PNG bytes | Source RGBA exact | Source changed pixels | Normalized PNG bytes | Normalized RGBA exact | Normalized changed pixels |\n",
    );
    report.push_str("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n");
    for comparison in comparisons {
        report.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {}/{} | {} | {} | {}/{} |\n",
            comparison.app,
            comparison.candidate_os,
            yes_no(comparison.source.png_bytes_equal),
            yes_no(comparison.source.rgba_bytes_equal),
            comparison.source.changed_pixels,
            comparison.source.total_pixels,
            yes_no(comparison.normalized.png_bytes_equal),
            yes_no(comparison.normalized.rgba_bytes_equal),
            comparison.normalized.changed_pixels,
            comparison.normalized.total_pixels,
        ));
    }
    report
}

/// Create a single image that makes captured pixels easy to inspect after
/// downloading the workflow artifact. Border colors are the legend:
/// blue = baseline source, orange = candidate source, green = baseline
/// normalized, magenta = candidate normalized.
fn write_preview(
    root: &Path,
    baseline: &str,
    comparisons: &[AppComparison],
    output: &Path,
) -> Result<(), String> {
    const CELL: u32 = 256;
    const GAP: u32 = 12;
    const BORDER: u32 = 4;
    let columns = 4;
    let rows = comparisons.len() as u32;
    let width = GAP + columns * (CELL + GAP);
    let height = GAP + rows * (CELL + GAP);
    let mut canvas = RgbaImage::from_pixel(width, height, Rgba([32, 35, 41, 255]));

    for (row, comparison) in comparisons.iter().enumerate() {
        let baseline_app = root.join(baseline).join(&comparison.app);
        let candidate_app = root.join(&comparison.candidate).join(&comparison.app);
        let images = [
            (baseline_app.join("source.png"), [80, 150, 255, 255]),
            (candidate_app.join("source.png"), [255, 160, 70, 255]),
            (baseline_app.join("normalized.png"), [100, 210, 130, 255]),
            (candidate_app.join("normalized.png"), [230, 100, 220, 255]),
        ];
        for (column, (path, border_color)) in images.into_iter().enumerate() {
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("read preview image {}: {error}", path.display()))?;
            let image = image::load_from_memory(&bytes)
                .map_err(|error| format!("decode preview image {}: {error}", path.display()))?
                .to_rgba8();
            let content_size = CELL - BORDER * 2;
            let image = imageops::resize(
                &image,
                content_size,
                content_size,
                imageops::FilterType::Nearest,
            );
            let x = GAP + column as u32 * (CELL + GAP);
            let y = GAP + row as u32 * (CELL + GAP);
            imageops::overlay(
                &mut canvas,
                &image,
                i64::from(x + BORDER),
                i64::from(y + BORDER),
            );
            for offset in 0..CELL {
                for border in 0..BORDER {
                    canvas.put_pixel(x + offset, y + border, Rgba(border_color));
                    canvas.put_pixel(x + offset, y + CELL - 1 - border, Rgba(border_color));
                    canvas.put_pixel(x + border, y + offset, Rgba(border_color));
                    canvas.put_pixel(x + CELL - 1 - border, y + offset, Rgba(border_color));
                }
            }
        }
    }
    canvas
        .save_with_format(output, ImageFormat::Png)
        .map_err(|error| format!("write preview {}: {error}", output.display()))
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_are_exact() {
        let image = RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let result = compare_images(&image, &image, true).unwrap();
        assert!(result.rgba_bytes_equal);
        assert_eq!(result.changed_pixels, 0);
        assert_eq!(result.mean_absolute_error, 0.0);
    }

    #[test]
    fn image_diff_counts_changed_pixels_and_channels() {
        let baseline = RgbaImage::from_pixel(2, 1, image::Rgba([0, 0, 0, 255]));
        let mut candidate = baseline.clone();
        candidate.put_pixel(1, 0, image::Rgba([10, 20, 30, 255]));
        let result = compare_images(&baseline, &candidate, false).unwrap();
        assert!(!result.rgba_bytes_equal);
        assert_eq!(result.changed_pixels, 1);
        assert_eq!(result.total_pixels, 2);
        assert_eq!(result.mean_absolute_error, 60.0 / 8.0);
        assert_eq!(result.max_absolute_error, 30);
    }
}
