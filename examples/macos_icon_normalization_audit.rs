//! Audit macOS icon-normalization captures produced by `macos_icon_capture`.
//!
//! This compares three capture trees made from the same ordered app list:
//! the PR #201 output, an alpha>=128 body-crop experiment, and the stricter
//! opaque-body candidate. It emits machine-readable statistics plus contact
//! sheets so a normalization rule is not selected from only a few icons.
//!
//! Usage:
//!   cargo run --example macos_icon_normalization_audit -- \
//!     --current-root /path/to/current \
//!     --hard-root /path/to/alpha128 \
//!     --opaque-root /path/to/alpha224 \
//!     --out target/macos-icon-normalization-audit

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use image::{imageops, ImageFormat, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

const CORE_ALPHA: u8 = 224;
const THUMBNAIL: u32 = 64;
const HEADER_HEIGHT: u32 = 11;
const INDEX_HEIGHT: u32 = 11;
const APPS_PER_ROW: u32 = 6;

#[derive(Debug)]
struct Args {
    current_root: PathBuf,
    hard_root: PathBuf,
    opaque_root: PathBuf,
    out_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct CaptureMetadata {
    app_name: String,
    app_path: String,
    category: String,
}

#[derive(Debug)]
struct Fixture {
    directory: PathBuf,
    metadata: CaptureMetadata,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct Bounds {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
    width: u32,
    height: u32,
}

impl Bounds {
    fn nearly_square(self) -> bool {
        self.width.abs_diff(self.height) * 20 <= self.width.max(self.height)
    }

    fn centered_inner_square(self) -> Self {
        let side = self.width.min(self.height);
        let min_x = (self.min_x + self.max_x - (side - 1)) / 2;
        let min_y = (self.min_y + self.max_y - (side - 1)) / 2;
        Self {
            min_x,
            min_y,
            max_x: min_x + side - 1,
            max_y: min_y + side - 1,
            width: side,
            height: side,
        }
    }

    fn contains(self, x: u32, y: u32) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }
}

#[derive(Debug, Serialize)]
struct FringeMargins {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

#[derive(Debug, Serialize)]
struct EdgeAlpha {
    left_min: u8,
    left_mean: f64,
    top_min: u8,
    top_mean: f64,
    right_min: u8,
    right_mean: f64,
    bottom_min: u8,
    bottom_mean: f64,
}

impl EdgeAlpha {
    fn minimum_mean(&self) -> f64 {
        self.left_mean
            .min(self.top_mean)
            .min(self.right_mean)
            .min(self.bottom_mean)
    }
}

#[derive(Debug, Serialize)]
struct VariantAudit {
    changed_pixels_from_current: u64,
    zero_alpha_pixels: u64,
    partial_alpha_pixels: u64,
    opaque_pixels: u64,
    edge_alpha: EdgeAlpha,
}

#[derive(Debug, Serialize)]
struct AppAudit {
    index: usize,
    app_name: String,
    app_path: String,
    category: String,
    source_soft_bounds: Option<Bounds>,
    source_hard_bounds: Option<Bounds>,
    source_opaque_core_bounds: Option<Bounds>,
    fringe_margins: Option<FringeMargins>,
    shadow_pattern: String,
    opaque_core_fill: f64,
    opaque_core_horizontal_symmetry: f64,
    opaque_core_vertical_symmetry: f64,
    opaque_core_pixels_trimmed_by_square: u64,
    opaque_core_trim_ratio: f64,
    qualified_candidate_applied: bool,
    current: VariantAudit,
    hard_alpha_128: VariantAudit,
    opaque_alpha_224: VariantAudit,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AuditReport {
    os_note: String,
    app_count: usize,
    category_counts: BTreeMap<String, usize>,
    shadow_pattern_counts: BTreeMap<String, usize>,
    changed_from_current: BTreeMap<String, usize>,
    non_fullbleed_changed: BTreeMap<String, usize>,
    warning_counts: BTreeMap<String, usize>,
    apps: Vec<AppAudit>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("macos-icon-normalization-audit: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let current = load_fixtures(&args.current_root)?;
    let hard = load_fixtures(&args.hard_root)?;
    let opaque = load_fixtures(&args.opaque_root)?;
    let keys = current.keys().cloned().collect::<BTreeSet<_>>();
    if keys != hard.keys().cloned().collect() || keys != opaque.keys().cloned().collect() {
        return Err("capture roots do not contain the same app_path set".into());
    }

    std::fs::create_dir_all(&args.out_dir)
        .map_err(|error| format!("create {}: {error}", args.out_dir.display()))?;

    let mut apps = Vec::with_capacity(keys.len());
    let mut contact_rows = Vec::with_capacity(keys.len());
    for (index, app_path) in keys.into_iter().enumerate() {
        let current_fixture = &current[&app_path];
        let hard_fixture = &hard[&app_path];
        let opaque_fixture = &opaque[&app_path];
        let source = load_rgba(&current_fixture.directory.join("source.png"))?;
        let hard_source = load_rgba(&hard_fixture.directory.join("source.png"))?;
        let opaque_source = load_rgba(&opaque_fixture.directory.join("source.png"))?;
        if source.as_raw() != hard_source.as_raw() || source.as_raw() != opaque_source.as_raw() {
            return Err(format!("source pixels differ between captures: {app_path}"));
        }
        let current_image = load_rgba(&current_fixture.directory.join("normalized.png"))?;
        let hard_image = load_rgba(&hard_fixture.directory.join("normalized.png"))?;
        let opaque_image = load_rgba(&opaque_fixture.directory.join("normalized.png"))?;

        let soft_bounds = alpha_bounds(&source, 11);
        let hard_bounds = alpha_bounds(&source, 128);
        let core_bounds = alpha_bounds(&source, CORE_ALPHA);
        let margins = soft_bounds
            .zip(core_bounds)
            .map(|(soft, core)| FringeMargins {
                left: core.min_x.saturating_sub(soft.min_x),
                top: core.min_y.saturating_sub(soft.min_y),
                right: soft.max_x.saturating_sub(core.max_x),
                bottom: soft.max_y.saturating_sub(core.max_y),
            });
        let shadow_pattern = margins
            .as_ref()
            .map(classify_fringe)
            .unwrap_or("missing-alpha-bounds")
            .to_owned();
        let (core_pixels, trimmed_core, trim_ratio) = core_bounds
            .filter(|bounds| bounds.nearly_square())
            .map(|bounds| core_trim(&source, bounds))
            .unwrap_or((0, 0, 0.0));
        let core_fill = core_bounds
            .map(|bounds| {
                core_pixels as f64 / (u64::from(bounds.width) * u64::from(bounds.height)) as f64
            })
            .unwrap_or(0.0);
        let (horizontal_symmetry, vertical_symmetry) = core_bounds
            .map(|bounds| mask_symmetry(&source, bounds, CORE_ALPHA))
            .unwrap_or((0.0, 0.0));

        let current_audit = variant_audit(&current_image, &current_image);
        let hard_audit = variant_audit(&hard_image, &current_image);
        let opaque_audit = variant_audit(&opaque_image, &current_image);
        let category = current_fixture.metadata.category.clone();
        let qualified_candidate_applied = category == "fullbleed"
            && core_bounds.is_some_and(Bounds::nearly_square)
            && core_fill >= 0.90
            && trim_ratio <= 0.02
            && horizontal_symmetry >= 0.99
            && vertical_symmetry >= 0.99;
        let mut warnings = Vec::new();
        if category != "fullbleed" {
            if hard_audit.changed_pixels_from_current != 0 {
                warnings.push("non-fullbleed-changed-by-alpha128".to_owned());
            }
            if opaque_audit.changed_pixels_from_current != 0 {
                warnings.push("non-fullbleed-changed-by-alpha224".to_owned());
            }
        } else if let Some(core) = core_bounds {
            if !core.nearly_square() {
                warnings.push("opaque-core-non-square-fallback".to_owned());
            } else {
                if core_fill < 0.90 {
                    warnings.push("opaque-core-fill-below-90-percent-fallback".to_owned());
                }
                if trim_ratio > 0.02 {
                    warnings.push("opaque-core-trim-over-2-percent-fallback".to_owned());
                }
                if horizontal_symmetry < 0.99 || vertical_symmetry < 0.99 {
                    warnings.push("opaque-core-symmetry-below-99-percent-fallback".to_owned());
                }
                if qualified_candidate_applied && opaque_audit.edge_alpha.minimum_mean() < 224.0 {
                    warnings.push("opaque-candidate-central-edge-mean-below-224".to_owned());
                }
            }
        } else {
            warnings.push("fullbleed-without-opaque-core".to_owned());
        }

        contact_rows.push(ContactRow {
            index: index + 1,
            source,
            current: current_image,
            hard: hard_image,
            opaque: opaque_image,
            category: category.clone(),
            outlier: !warnings.is_empty(),
        });
        apps.push(AppAudit {
            index: index + 1,
            app_name: current_fixture.metadata.app_name.clone(),
            app_path,
            category,
            source_soft_bounds: soft_bounds,
            source_hard_bounds: hard_bounds,
            source_opaque_core_bounds: core_bounds,
            fringe_margins: margins,
            shadow_pattern,
            opaque_core_fill: core_fill,
            opaque_core_horizontal_symmetry: horizontal_symmetry,
            opaque_core_vertical_symmetry: vertical_symmetry,
            opaque_core_pixels_trimmed_by_square: trimmed_core,
            opaque_core_trim_ratio: trim_ratio,
            qualified_candidate_applied,
            current: current_audit,
            hard_alpha_128: hard_audit,
            opaque_alpha_224: opaque_audit,
            warnings,
        });
    }

    let report = summarize(apps);
    write_index(&args.out_dir.join("apps.tsv"), &report.apps)?;
    write_json(&args.out_dir.join("report.json"), &report)?;
    write_contact_sheet(&args.out_dir.join("contact-sheet-all.png"), &contact_rows)?;
    write_contact_sheet(
        &args.out_dir.join("contact-sheet-fullbleed.png"),
        &contact_rows
            .iter()
            .filter(|row| row.category == "fullbleed")
            .cloned()
            .collect::<Vec<_>>(),
    )?;
    write_contact_sheet(
        &args.out_dir.join("contact-sheet-outliers.png"),
        &contact_rows
            .iter()
            .filter(|row| row.outlier)
            .cloned()
            .collect::<Vec<_>>(),
    )?;
    println!(
        "audited {} apps; report={}",
        report.app_count,
        args.out_dir.join("report.json").display()
    );
    Ok(())
}

fn parse_args() -> Result<Args, String> {
    let mut current_root = None;
    let mut hard_root = None;
    let mut opaque_root = None;
    let mut out_dir = None;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{argument} requires a path"))?;
        match argument.as_str() {
            "--current-root" => current_root = Some(PathBuf::from(value)),
            "--hard-root" => hard_root = Some(PathBuf::from(value)),
            "--opaque-root" => opaque_root = Some(PathBuf::from(value)),
            "--out" => out_dir = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(Args {
        current_root: current_root.ok_or("--current-root is required")?,
        hard_root: hard_root.ok_or("--hard-root is required")?,
        opaque_root: opaque_root.ok_or("--opaque-root is required")?,
        out_dir: out_dir.ok_or("--out is required")?,
    })
}

fn load_fixtures(root: &Path) -> Result<BTreeMap<String, Fixture>, String> {
    let mut metadata_paths = Vec::new();
    collect_named_files(root, "metadata.json", &mut metadata_paths)?;
    let mut fixtures = BTreeMap::new();
    for path in metadata_paths {
        let bytes =
            std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        let metadata: CaptureMetadata = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse {}: {error}", path.display()))?;
        let directory = path.parent().unwrap_or(root).to_path_buf();
        fixtures.insert(
            metadata.app_path.clone(),
            Fixture {
                directory,
                metadata,
            },
        );
    }
    Ok(fixtures)
}

fn collect_named_files(root: &Path, name: &str, output: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in
        std::fs::read_dir(root).map_err(|error| format!("read {}: {error}", root.display()))?
    {
        let entry = entry.map_err(|error| format!("read {} entry: {error}", root.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect_named_files(&path, name, output)?;
        } else if path.file_name().is_some_and(|candidate| candidate == name) {
            output.push(path);
        }
    }
    Ok(())
}

fn load_rgba(path: &Path) -> Result<RgbaImage, String> {
    image::open(path)
        .map(|image| image.to_rgba8())
        .map_err(|error| format!("open {}: {error}", path.display()))
}

fn alpha_bounds(image: &RgbaImage, threshold: u8) -> Option<Bounds> {
    let mut min_x = image.width();
    let mut min_y = image.height();
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] >= threshold {
            found = true;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    found.then_some(Bounds {
        min_x,
        min_y,
        max_x,
        max_y,
        width: max_x - min_x + 1,
        height: max_y - min_y + 1,
    })
}

fn classify_fringe(margins: &FringeMargins) -> &'static str {
    let values = [margins.left, margins.top, margins.right, margins.bottom];
    let minimum = *values.iter().min().unwrap();
    let maximum = *values.iter().max().unwrap();
    if maximum <= 1 {
        return "none-or-antialias-only";
    }
    if minimum >= 2 && maximum - minimum <= 2 {
        return "thin-all-around";
    }
    let directions = [
        ("left-dominant", margins.left),
        ("top-dominant", margins.top),
        ("right-dominant", margins.right),
        ("bottom-dominant", margins.bottom),
    ];
    let (name, value) = directions
        .into_iter()
        .max_by_key(|(_, value)| *value)
        .unwrap();
    if value >= minimum + 2 {
        name
    } else {
        "asymmetric-mixed"
    }
}

fn core_trim(image: &RgbaImage, bounds: Bounds) -> (u64, u64, f64) {
    let square = bounds.centered_inner_square();
    let mut total = 0u64;
    let mut trimmed = 0u64;
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] >= CORE_ALPHA {
            total += 1;
            trimmed += u64::from(!square.contains(x, y));
        }
    }
    let ratio = if total == 0 {
        0.0
    } else {
        trimmed as f64 / total as f64
    };
    (total, trimmed, ratio)
}

fn mask_symmetry(image: &RgbaImage, bounds: Bounds, threshold: u8) -> (f64, f64) {
    let mut horizontal_mismatches = 0u64;
    let mut vertical_mismatches = 0u64;
    for local_y in 0..bounds.height {
        for local_x in 0..bounds.width {
            let x = bounds.min_x + local_x;
            let y = bounds.min_y + local_y;
            let opaque = image.get_pixel(x, y)[3] >= threshold;
            horizontal_mismatches +=
                u64::from(opaque != (image.get_pixel(bounds.max_x - local_x, y)[3] >= threshold));
            vertical_mismatches +=
                u64::from(opaque != (image.get_pixel(x, bounds.max_y - local_y)[3] >= threshold));
        }
    }
    let area = f64::from(bounds.width * bounds.height);
    (
        1.0 - horizontal_mismatches as f64 / area,
        1.0 - vertical_mismatches as f64 / area,
    )
}

fn variant_audit(image: &RgbaImage, current: &RgbaImage) -> VariantAudit {
    let mut changed_pixels = 0u64;
    let mut zero = 0u64;
    let mut partial = 0u64;
    let mut opaque = 0u64;
    for (pixel, current_pixel) in image.pixels().zip(current.pixels()) {
        changed_pixels += u64::from(pixel != current_pixel);
        match pixel[3] {
            0 => zero += 1,
            255 => opaque += 1,
            _ => partial += 1,
        }
    }
    VariantAudit {
        changed_pixels_from_current: changed_pixels,
        zero_alpha_pixels: zero,
        partial_alpha_pixels: partial,
        opaque_pixels: opaque,
        edge_alpha: edge_alpha(image),
    }
}

fn edge_alpha(image: &RgbaImage) -> EdgeAlpha {
    let x0 = image.width() / 4;
    let x1 = image.width() - x0;
    let y0 = image.height() / 4;
    let y1 = image.height() - y0;
    let left = (y0..y1).map(|y| image.get_pixel(0, y)[3]);
    let right = (y0..y1).map(|y| image.get_pixel(image.width() - 1, y)[3]);
    let top = (x0..x1).map(|x| image.get_pixel(x, 0)[3]);
    let bottom = (x0..x1).map(|x| image.get_pixel(x, image.height() - 1)[3]);
    let (left_min, left_mean) = alpha_min_mean(left);
    let (top_min, top_mean) = alpha_min_mean(top);
    let (right_min, right_mean) = alpha_min_mean(right);
    let (bottom_min, bottom_mean) = alpha_min_mean(bottom);
    EdgeAlpha {
        left_min,
        left_mean,
        top_min,
        top_mean,
        right_min,
        right_mean,
        bottom_min,
        bottom_mean,
    }
}

fn alpha_min_mean(values: impl Iterator<Item = u8>) -> (u8, f64) {
    let values = values.collect::<Vec<_>>();
    let minimum = values.iter().copied().min().unwrap_or(0);
    let mean = values.iter().map(|&value| u64::from(value)).sum::<u64>() as f64
        / values.len().max(1) as f64;
    (minimum, mean)
}

fn summarize(apps: Vec<AppAudit>) -> AuditReport {
    let mut category_counts = BTreeMap::new();
    let mut shadow_pattern_counts = BTreeMap::new();
    let mut changed_from_current = BTreeMap::from([
        ("alpha128".to_owned(), 0usize),
        ("alpha224".to_owned(), 0usize),
    ]);
    let mut non_fullbleed_changed = changed_from_current.clone();
    let mut warning_counts = BTreeMap::new();
    for app in &apps {
        *category_counts.entry(app.category.clone()).or_default() += 1;
        *shadow_pattern_counts
            .entry(app.shadow_pattern.clone())
            .or_default() += 1;
        for (label, changed) in [
            (
                "alpha128",
                app.hard_alpha_128.changed_pixels_from_current != 0,
            ),
            (
                "alpha224",
                app.opaque_alpha_224.changed_pixels_from_current != 0,
            ),
        ] {
            if changed {
                *changed_from_current.get_mut(label).unwrap() += 1;
                if app.category != "fullbleed" {
                    *non_fullbleed_changed.get_mut(label).unwrap() += 1;
                }
            }
        }
        for warning in &app.warnings {
            *warning_counts.entry(warning.clone()).or_default() += 1;
        }
    }
    AuditReport {
        os_note: "This report describes captures from the executing macOS/NSWorkspace only; it does not generalize across macOS versions.".to_owned(),
        app_count: apps.len(),
        category_counts,
        shadow_pattern_counts,
        changed_from_current,
        non_fullbleed_changed,
        warning_counts,
        apps,
    }
}

fn write_json(path: &Path, report: &AuditReport) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(report).map_err(|error| format!("serialize report: {error}"))?;
    std::fs::write(path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
}

fn write_index(path: &Path, apps: &[AppAudit]) -> Result<(), String> {
    let mut index = String::from("index\tcategory\tapp_name\tapp_path\twarnings\n");
    for app in apps {
        index.push_str(&format!(
            "{:04}\t{}\t{}\t{}\t{}\n",
            app.index,
            app.category,
            app.app_name.replace('\t', " "),
            app.app_path.replace('\t', " "),
            app.warnings.join(",")
        ));
    }
    std::fs::write(path, index).map_err(|error| format!("write {}: {error}", path.display()))
}

#[derive(Clone)]
struct ContactRow {
    index: usize,
    source: RgbaImage,
    current: RgbaImage,
    hard: RgbaImage,
    opaque: RgbaImage,
    category: String,
    outlier: bool,
}

fn write_contact_sheet(path: &Path, rows: &[ContactRow]) -> Result<(), String> {
    let variants = 4u32;
    let cell_w = THUMBNAIL * variants;
    let cell_h = THUMBNAIL + INDEX_HEIGHT;
    let row_count = (rows.len() as u32).div_ceil(APPS_PER_ROW).max(1);
    let mut sheet = RgbaImage::from_pixel(
        cell_w * APPS_PER_ROW,
        HEADER_HEIGHT + cell_h * row_count,
        Rgba([24, 25, 29, 255]),
    );
    for group in 0..APPS_PER_ROW {
        for (variant, label) in ["SOURCE", "PR201", "A128", "FINAL"].into_iter().enumerate() {
            draw_text_centered(
                &mut sheet,
                group * cell_w + variant as u32 * THUMBNAIL,
                3,
                THUMBNAIL,
                label,
                Rgba([225, 225, 230, 255]),
            );
        }
    }
    for (position, row) in rows.iter().enumerate() {
        let column = position as u32 % APPS_PER_ROW;
        let line = position as u32 / APPS_PER_ROW;
        let x = column * cell_w;
        let y = HEADER_HEIGHT + line * cell_h;
        draw_index(&mut sheet, x + 2, y + 2, row.index);
        for (variant, image) in [&row.source, &row.current, &row.hard, &row.opaque]
            .into_iter()
            .enumerate()
        {
            let thumbnail =
                imageops::resize(image, THUMBNAIL, THUMBNAIL, imageops::FilterType::Lanczos3);
            imageops::overlay(
                &mut sheet,
                &thumbnail,
                i64::from(x + variant as u32 * THUMBNAIL),
                i64::from(y + INDEX_HEIGHT),
            );
        }
    }
    sheet
        .save_with_format(path, ImageFormat::Png)
        .map_err(|error| format!("write {}: {error}", path.display()))
}

fn draw_index(image: &mut RgbaImage, x: u32, y: u32, index: usize) {
    let text = format!("{index:04}");
    draw_text(image, x, y, &text, Rgba([235, 235, 240, 255]));
}

fn draw_text_centered(
    image: &mut RgbaImage,
    x: u32,
    y: u32,
    width: u32,
    text: &str,
    color: Rgba<u8>,
) {
    let text_width = text.len() as u32 * 4;
    draw_text(
        image,
        x + width.saturating_sub(text_width) / 2,
        y,
        text,
        color,
    );
}

fn draw_text(image: &mut RgbaImage, x: u32, y: u32, text: &str, color: Rgba<u8>) {
    for (offset, character) in text.bytes().enumerate() {
        let rows = glyph_rows(character);
        for (dy, bits) in rows.into_iter().enumerate() {
            for dx in 0..3 {
                if bits & (1 << (2 - dx)) != 0 {
                    let px = x + offset as u32 * 4 + dx;
                    let py = y + dy as u32;
                    if px < image.width() && py < image.height() {
                        image.put_pixel(px, py, color);
                    }
                }
            }
        }
    }
}

fn glyph_rows(character: u8) -> [u8; 5] {
    match character {
        b'A' => [0b111, 0b101, 0b111, 0b101, 0b101],
        b'C' => [0b111, 0b100, 0b100, 0b100, 0b111],
        b'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        b'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        b'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        b'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        b'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        b'O' => [0b111, 0b101, 0b101, 0b101, 0b111],
        b'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        b'R' => [0b110, 0b101, 0b110, 0b110, 0b101],
        b'S' => [0b111, 0b100, 0b111, 0b001, 0b111],
        b'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        b'0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        b'1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        b'2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        b'3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        b'4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        b'5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        b'6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        b'7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        b'8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        b'9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        _ => [0; 5],
    }
}
