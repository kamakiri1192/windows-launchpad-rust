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
    shape_preview: PathBuf,
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
    shape: Option<ShapeComparison>,
}

#[derive(Debug, Serialize)]
struct ShapeComparison {
    baseline: AlphaGeometry,
    candidate: AlphaGeometry,
    mask_changed_pixels_alpha_gt_0: u64,
    mask_changed_pixels_alpha_ge_11: u64,
    mask_changed_pixels_alpha_ge_128: u64,
}

#[derive(Debug, Serialize)]
struct AlphaGeometry {
    bbox_alpha_gt_0: Option<BoundingBox>,
    bbox_alpha_ge_11: Option<BoundingBox>,
    bbox_alpha_ge_128: Option<BoundingBox>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
struct BoundingBox {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
    width: u32,
    height: u32,
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
    write_shape_preview(
        &args.root,
        &args.baseline,
        &comparisons,
        &args.shape_preview,
    )?;
    print!("{report}");
    Ok(())
}

fn parse_args() -> Result<Args, String> {
    let mut root = PathBuf::from("target/macos-captures");
    let mut baseline = "macos-14".to_owned();
    let mut report = PathBuf::from("target/macos-captures/compatibility-report.md");
    let mut preview = PathBuf::from("target/macos-captures/compatibility-preview.png");
    let mut shape_preview = PathBuf::from("target/macos-captures/compatibility-shape-preview.png");
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
            "--shape-preview" => {
                shape_preview = PathBuf::from(
                    args.next()
                        .ok_or_else(|| "--shape-preview requires a file".to_owned())?,
                );
            }
            "--help" | "-h" => {
                println!(
                    "Usage: macos_icon_compare [--root DIR] [--baseline NAME] [--report FILE] [--preview FILE] [--shape-preview FILE]"
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
        shape_preview,
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
            shape: None,
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
    let shape = compare_shapes(baseline, candidate);
    Ok(ImageComparison {
        dimensions_equal: true,
        png_bytes_equal,
        rgba_bytes_equal: baseline_raw == candidate_raw,
        changed_pixels,
        total_pixels,
        mean_absolute_error,
        max_absolute_error,
        shape: Some(shape),
    })
}

fn compare_shapes(baseline: &RgbaImage, candidate: &RgbaImage) -> ShapeComparison {
    ShapeComparison {
        baseline: alpha_geometry(baseline),
        candidate: alpha_geometry(candidate),
        mask_changed_pixels_alpha_gt_0: changed_alpha_mask_pixels(baseline, candidate, 1),
        mask_changed_pixels_alpha_ge_11: changed_alpha_mask_pixels(baseline, candidate, 11),
        mask_changed_pixels_alpha_ge_128: changed_alpha_mask_pixels(baseline, candidate, 128),
    }
}

fn alpha_geometry(image: &RgbaImage) -> AlphaGeometry {
    AlphaGeometry {
        bbox_alpha_gt_0: alpha_bounding_box(image, 1),
        bbox_alpha_ge_11: alpha_bounding_box(image, 11),
        bbox_alpha_ge_128: alpha_bounding_box(image, 128),
    }
}

fn alpha_bounding_box(image: &RgbaImage, threshold: u8) -> Option<BoundingBox> {
    let (width, height) = image.dimensions();
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;
    for y in 0..height {
        for x in 0..width {
            if image.get_pixel(x, y)[3] >= threshold {
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

fn changed_alpha_mask_pixels(baseline: &RgbaImage, candidate: &RgbaImage, threshold: u8) -> u64 {
    baseline
        .pixels()
        .zip(candidate.pixels())
        .filter(|(left, right)| (left[3] >= threshold) != (right[3] >= threshold))
        .count() as u64
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
    report.push_str("\n## Outer shape comparison\n\n");
    report.push_str(
        "`bbox` is shown as `width×height at (x,y)`. `alpha ≥ 11` is the visible outer shape used by the current crop, and `alpha ≥ 128` is the opaque core. Mask differences count pixels whose threshold membership changed.\n\n",
    );
    report.push_str(
        "| App | Candidate OS | Source outer bbox (α≥11) | Source core bbox (α≥128) | Source mask diff (α≥11) | Normalized outer bbox (α≥11) | Normalized core bbox (α≥128) | Normalized mask diff (α≥11) |\n",
    );
    report.push_str("| --- | --- | --- | --- | ---: | --- | --- | ---: |\n");
    for comparison in comparisons {
        report.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} | {} | {} | {} |\n",
            comparison.app,
            comparison.candidate_os,
            shape_bbox_pair(&comparison.source, |geometry| &geometry.bbox_alpha_ge_11),
            shape_bbox_pair(&comparison.source, |geometry| &geometry.bbox_alpha_ge_128),
            shape_mask_diff(&comparison.source, |shape| shape
                .mask_changed_pixels_alpha_ge_11),
            shape_bbox_pair(&comparison.normalized, |geometry| &geometry
                .bbox_alpha_ge_11,),
            shape_bbox_pair(&comparison.normalized, |geometry| &geometry
                .bbox_alpha_ge_128,),
            shape_mask_diff(&comparison.normalized, |shape| shape
                .mask_changed_pixels_alpha_ge_11,),
        ));
    }
    report
}

fn shape_bbox_pair<F>(comparison: &ImageComparison, pick: F) -> String
where
    F: Fn(&AlphaGeometry) -> &Option<BoundingBox>,
{
    let Some(shape) = &comparison.shape else {
        return "unavailable".to_owned();
    };
    format!(
        "{} → {}",
        format_bbox(pick(&shape.baseline)),
        format_bbox(pick(&shape.candidate)),
    )
}

fn shape_mask_diff<F>(comparison: &ImageComparison, pick: F) -> String
where
    F: Fn(&ShapeComparison) -> u64,
{
    let Some(shape) = &comparison.shape else {
        return "unavailable".to_owned();
    };
    format!("{}/{}", pick(shape), comparison.total_pixels)
}

fn format_bbox(bbox: &Option<BoundingBox>) -> String {
    bbox.as_ref()
        .map(|bbox| {
            format!(
                "{}×{} at ({},{})",
                bbox.width, bbox.height, bbox.min_x, bbox.min_y
            )
        })
        .unwrap_or_else(|| "none".to_owned())
}

/// Create a labeled contact sheet that makes captured pixels easy to inspect
/// after downloading the workflow artifact. Border colors are the legend:
/// blue = baseline source, orange = candidate source, green = baseline
/// normalized, magenta = candidate normalized.
fn write_preview(
    root: &Path,
    baseline: &str,
    comparisons: &[AppComparison],
    output: &Path,
) -> Result<(), String> {
    const CELL: u32 = 256;
    const LABEL_HEIGHT: u32 = 32;
    const ROW_HEIGHT: u32 = LABEL_HEIGHT + CELL;
    const ROW_LABEL_WIDTH: u32 = 180;
    const GAP: u32 = 12;
    const BORDER: u32 = 4;
    let columns = 4u32;
    let rows = comparisons.len() as u32;
    let image_start_x = GAP + ROW_LABEL_WIDTH + GAP;
    let width = image_start_x + columns * (CELL + GAP);
    let height = GAP + rows * (ROW_HEIGHT + GAP);
    let mut canvas = RgbaImage::from_pixel(width, height, Rgba([32, 35, 41, 255]));

    for (row, comparison) in comparisons.iter().enumerate() {
        let row_y = GAP + row as u32 * (ROW_HEIGHT + GAP);
        let baseline_label = os_label(baseline);
        let candidate_label = os_label(&comparison.candidate);
        draw_row_label(
            &mut canvas,
            GAP,
            row_y,
            ROW_LABEL_WIDTH,
            &comparison.app,
            &candidate_label,
        );
        let headers = [
            format!("{baseline_label} SOURCE"),
            format!("{candidate_label} SOURCE"),
            format!("{baseline_label} NORMALIZED"),
            format!("{candidate_label} NORMALIZED"),
        ];
        let baseline_app = root.join(baseline).join(&comparison.app);
        let candidate_app = root.join(&comparison.candidate).join(&comparison.app);
        let images = [
            (baseline_app.join("source.png"), [80, 150, 255, 255]),
            (candidate_app.join("source.png"), [255, 160, 70, 255]),
            (baseline_app.join("normalized.png"), [100, 210, 130, 255]),
            (candidate_app.join("normalized.png"), [230, 100, 220, 255]),
        ];
        for (column, (path, border_color)) in images.into_iter().enumerate() {
            let x = image_start_x + column as u32 * (CELL + GAP);
            draw_text_centered(
                &mut canvas,
                x,
                row_y + 4,
                CELL,
                &headers[column],
                2,
                Rgba([220, 220, 225, 255]),
            );
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
            let y = row_y + LABEL_HEIGHT;
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

/// Create an alpha-only preview for the geometry comparison. Each row has six
/// labeled cells: baseline/candidate source masks, their overlay, then the
/// same three views for normalized images. In the masks, light gray is alpha
/// >= 128 and dark gray is the softer visible fringe alpha >= 11. In the
/// overlay, blue is baseline-only, orange is candidate-only, and light gray
/// is shared.
fn write_shape_preview(
    root: &Path,
    baseline: &str,
    comparisons: &[AppComparison],
    output: &Path,
) -> Result<(), String> {
    const CELL: u32 = 256;
    const LABEL_HEIGHT: u32 = 32;
    const ROW_HEIGHT: u32 = LABEL_HEIGHT + CELL;
    const ROW_LABEL_WIDTH: u32 = 180;
    const GAP: u32 = 12;
    const BORDER: u32 = 4;
    const COLUMNS: usize = 6;
    let rows = comparisons.len() as u32;
    let image_start_x = GAP + ROW_LABEL_WIDTH + GAP;
    let width = image_start_x + COLUMNS as u32 * (CELL + GAP);
    let height = GAP + rows * (ROW_HEIGHT + GAP);
    let mut canvas = RgbaImage::from_pixel(width, height, Rgba([32, 35, 41, 255]));

    for (row, comparison) in comparisons.iter().enumerate() {
        let row_y = GAP + row as u32 * (ROW_HEIGHT + GAP);
        let baseline_label = os_label(baseline);
        let candidate_label = os_label(&comparison.candidate);
        draw_row_label(
            &mut canvas,
            GAP,
            row_y,
            ROW_LABEL_WIDTH,
            &comparison.app,
            &candidate_label,
        );

        let baseline_app = root.join(baseline).join(&comparison.app);
        let candidate_app = root.join(&comparison.candidate).join(&comparison.app);
        let source_baseline = load_png(&baseline_app.join("source.png"))?;
        let source_candidate = load_png(&candidate_app.join("source.png"))?;
        let normalized_baseline = load_png(&baseline_app.join("normalized.png"))?;
        let normalized_candidate = load_png(&candidate_app.join("normalized.png"))?;
        let cells = [
            alpha_mask_preview(&source_baseline),
            alpha_mask_preview(&source_candidate),
            alpha_overlay_preview(&source_baseline, &source_candidate),
            alpha_mask_preview(&normalized_baseline),
            alpha_mask_preview(&normalized_candidate),
            alpha_overlay_preview(&normalized_baseline, &normalized_candidate),
        ];
        let headers = [
            format!("{baseline_label} SOURCE"),
            format!("{candidate_label} SOURCE"),
            "SOURCE SHAPE DIFF".to_owned(),
            format!("{baseline_label} NORMALIZED"),
            format!("{candidate_label} NORMALIZED"),
            "NORMALIZED SHAPE DIFF".to_owned(),
        ];
        for (column, image) in cells.iter().enumerate() {
            let x = image_start_x + column as u32 * (CELL + GAP);
            draw_text_centered(
                &mut canvas,
                x,
                row_y + 4,
                CELL,
                &headers[column],
                2,
                Rgba([220, 220, 225, 255]),
            );
            let image = imageops::resize(
                image,
                CELL - BORDER * 2,
                CELL - BORDER * 2,
                imageops::FilterType::Nearest,
            );
            let y = row_y + LABEL_HEIGHT;
            imageops::overlay(
                &mut canvas,
                &image,
                i64::from(x + BORDER),
                i64::from(y + BORDER),
            );
            let border_color = match column {
                0 => [80, 150, 255, 255],
                1 => [255, 160, 70, 255],
                2 => [210, 210, 210, 255],
                3 => [100, 210, 130, 255],
                4 => [230, 100, 220, 255],
                _ => [210, 210, 210, 255],
            };
            draw_border(&mut canvas, x, y, CELL, BORDER, border_color);
        }
    }
    canvas
        .save_with_format(output, ImageFormat::Png)
        .map_err(|error| format!("write shape preview {}: {error}", output.display()))
}

fn draw_row_label(
    canvas: &mut RgbaImage,
    x: u32,
    row_y: u32,
    width: u32,
    app: &str,
    candidate_label: &str,
) {
    const LABEL_HEIGHT: u32 = 32;
    const CELL: u32 = 256;
    let app_line_1 = if app == "activity-monitor" {
        "ACTIVITY"
    } else {
        "APP"
    };
    let app_line_2 = if app == "activity-monitor" {
        "MONITOR"
    } else {
        "STORE"
    };
    let row_label_y = row_y + LABEL_HEIGHT + (CELL - 3 * 15 - 2 * 6) / 2;
    draw_text_centered(
        canvas,
        x,
        row_label_y,
        width,
        app_line_1,
        3,
        Rgba([230, 230, 235, 255]),
    );
    draw_text_centered(
        canvas,
        x,
        row_label_y + 21,
        width,
        app_line_2,
        3,
        Rgba([230, 230, 235, 255]),
    );
    draw_text_centered(
        canvas,
        x,
        row_label_y + 42,
        width,
        candidate_label,
        3,
        Rgba([170, 175, 185, 255]),
    );
}

fn os_label(name: &str) -> String {
    match name {
        "macos-14" => "MACOS 14".to_owned(),
        "macos-15" => "MACOS 15".to_owned(),
        "macos-26" => "MACOS 26".to_owned(),
        other => other.to_ascii_uppercase().replace('-', " "),
    }
}

fn draw_text_centered(
    canvas: &mut RgbaImage,
    x: u32,
    y: u32,
    width: u32,
    text: &str,
    scale: u32,
    color: Rgba<u8>,
) {
    let text_width = text.len() as u32 * 4 * scale;
    let text_x = x + width.saturating_sub(text_width) / 2;
    draw_text(canvas, text_x, y, text, scale, color);
}

/// Draw the small fixed 3x5 uppercase/digit font used for deterministic
/// labels in the PNG preview. This avoids depending on fonts installed on the
/// GitHub runner.
fn draw_text(canvas: &mut RgbaImage, x: u32, y: u32, text: &str, scale: u32, color: Rgba<u8>) {
    let mut cursor_x = x;
    for character in text.chars() {
        if character == ' ' {
            cursor_x += 4 * scale;
            continue;
        }
        let glyph = glyph_3x5(character);
        for (row, bits) in glyph.iter().enumerate() {
            for column in 0..3 {
                if bits & (1 << (2 - column)) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        canvas.put_pixel(
                            cursor_x + column * scale + dx,
                            y + row as u32 * scale + dy,
                            color,
                        );
                    }
                }
            }
        }
        cursor_x += 4 * scale;
    }
}

fn glyph_3x5(character: char) -> [u8; 5] {
    match character {
        'A' => [0b111, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b111, 0b100, 0b100, 0b100, 0b111],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b111, 0b100, 0b101, 0b101, 0b111],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b111],
        'K' => [0b101, 0b110, 0b100, 0b110, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b111, 0b101, 0b101, 0b101, 0b111],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b111, 0b101, 0b101, 0b111, 0b001],
        'R' => [0b110, 0b101, 0b110, 0b110, 0b101],
        'S' => [0b111, 0b100, 0b111, 0b001, 0b111],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b110, 0b001, 0b010, 0b100, 0b111],
        '3' => [0b110, 0b001, 0b010, 0b001, 0b110],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b110, 0b001, 0b110],
        '6' => [0b011, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b110],
        _ => [0; 5],
    }
}

fn load_png(path: &Path) -> Result<RgbaImage, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    image::load_from_memory(&bytes)
        .map_err(|error| format!("decode {}: {error}", path.display()))
        .map(|image| image.to_rgba8())
}

fn alpha_mask_preview(image: &RgbaImage) -> RgbaImage {
    let mut preview = RgbaImage::new(image.width(), image.height());
    for (source, target) in image.pixels().zip(preview.pixels_mut()) {
        let alpha = source[3];
        let value = if alpha >= 128 {
            240
        } else if alpha >= 11 {
            120
        } else {
            32
        };
        *target = Rgba([value, value, value, 255]);
    }
    preview
}

fn alpha_overlay_preview(baseline: &RgbaImage, candidate: &RgbaImage) -> RgbaImage {
    let width = baseline.width().min(candidate.width());
    let height = baseline.height().min(candidate.height());
    let mut preview = RgbaImage::from_pixel(width, height, Rgba([32, 35, 41, 255]));
    for y in 0..height {
        for x in 0..width {
            let baseline_inside = baseline.get_pixel(x, y)[3] >= 11;
            let candidate_inside = candidate.get_pixel(x, y)[3] >= 11;
            let color = match (baseline_inside, candidate_inside) {
                (true, true) => [220, 220, 220, 255],
                (true, false) => [80, 150, 255, 255],
                (false, true) => [255, 160, 70, 255],
                (false, false) => [32, 35, 41, 255],
            };
            preview.put_pixel(x, y, Rgba(color));
        }
    }
    preview
}

fn draw_border(canvas: &mut RgbaImage, x: u32, y: u32, size: u32, border: u32, color: [u8; 4]) {
    for offset in 0..size {
        for thickness in 0..border {
            canvas.put_pixel(x + offset, y + thickness, Rgba(color));
            canvas.put_pixel(x + offset, y + size - 1 - thickness, Rgba(color));
            canvas.put_pixel(x + thickness, y + offset, Rgba(color));
            canvas.put_pixel(x + size - 1 - thickness, y + offset, Rgba(color));
        }
    }
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
