//! Icon audit tool — validates the visual-size analysis + normalization
//! pipeline (Issue #48) against real macOS application icons.
//!
//! Usage:
//!   cargo run --example icon_audit -- --out /tmp/icon-audit
//!
//! Outputs:
//!   - `icon-audit-report.json`   — per-app classification data
//!   - `icon-audit-fullbleed.png` — contact sheet, FullBleed category
//!   - `icon-audit-thinline.png`  — contact sheet, ThinLine category
//!   - `icon-audit-solid.png`     — contact sheet, Solid category
//!   - `icon-audit-compare.png`   — before/after comparison sheet
//!
//! This tool is macOS-only.

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("icon-audit: this tool is macOS-only.");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
mod macos_audit {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use image::{imageops, Rgba, RgbaImage};
    use serde::Serialize;

    use launchpad_windows::icons::normalize::{self, DecodedIcon, NormalizedIcon};
    use launchpad_windows::icons::sizing;
    use launchpad_windows::icons::sizing::IconMetrics;

    // -----------------------------------------------------------------------
    // Constants
    // -----------------------------------------------------------------------

    /// Output icon cell size (matches the launcher's TARGET).
    const TARGET: u32 = 128;

    /// Tile rounded-corner radius (spec §5: TARGET * 19/84).
    const CORNER_RADIUS: f64 = (TARGET as f64) * 19.0 / 84.0;

    /// Cell dimensions on contact sheets.
    const CELL_W: u32 = 160;
    const CELL_H: u32 = 198;

    /// Padding inside each cell (top/left/bottom/right).
    const PAD: u32 = 16;

    /// Columns per contact-sheet row.
    const COLS: u32 = 10;

    /// Background colour (liquid-glass grey).
    const BG: [u8; 3] = [0xDC, 0xE0, 0xE4];

    /// Maximum FullBleed entries on the category contact sheet.
    const MAX_FULLBLEED_SHEET: usize = 100;

    /// Maximum compare-sheet rows.
    const MAX_COMPARE_ROWS: usize = 50;

    // -----------------------------------------------------------------------
    // Audit entry
    // -----------------------------------------------------------------------

    #[derive(Debug, Clone, Serialize)]
    struct AuditEntry {
        name: String,
        /// Lower-case category string.
        category: String,
        scale: f64,
        solid_fill: f64,
        bbox_min_x: u32,
        bbox_min_y: u32,
        bbox_max_x: u32,
        bbox_max_y: u32,
        bbox_w: u32,
        bbox_h: u32,
    }

    impl AuditEntry {
        fn from_metrics(name: String, metrics: &IconMetrics) -> Self {
            Self {
                name,
                category: metrics.category.as_str().to_owned(),
                scale: metrics.scale,
                solid_fill: metrics.solid_fill,
                bbox_min_x: metrics.bbox_min_x,
                bbox_min_y: metrics.bbox_min_y,
                bbox_max_x: metrics.bbox_max_x,
                bbox_max_y: metrics.bbox_max_y,
                bbox_w: metrics.bbox_w,
                bbox_h: metrics.bbox_h,
            }
        }
    }

    // -----------------------------------------------------------------------
    // CLI
    // -----------------------------------------------------------------------

    struct Args {
        out_dir: PathBuf,
    }

    fn parse_args() -> Result<Args, String> {
        let args: Vec<String> = std::env::args().collect();

        if args.iter().any(|a| a == "--help" || a == "-h") {
            eprintln!(
                "icon-audit — validate visual-size analysis on real macOS app icons.\n\
                 \n\
                 Usage: cargo run --example icon_audit -- [OPTIONS]\n\
                 \n\
                 Options:\n\
                   --out <dir>   Output directory (default: ./icon-audit-output)\n\
                   --help, -h    Show this help\n\
                 \n\
                 Outputs:\n\
                   icon-audit-report.json    JSON per-app classification data\n\
                   icon-audit-fullbleed.png  Contact sheet, FullBleed category\n\
                   icon-audit-thinline.png   Contact sheet, ThinLine category\n\
                   icon-audit-solid.png      Contact sheet, Solid category\n\
                   icon-audit-compare.png    Before/after comparison (non-FullBleed only)"
            );
            std::process::exit(0);
        }

        let mut out_dir = PathBuf::from("./icon-audit-output");
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--out" => {
                    i += 1;
                    if i >= args.len() {
                        return Err("--out requires a path argument".into());
                    }
                    out_dir = PathBuf::from(&args[i]);
                }
                other => return Err(format!("unknown flag: {other}")),
            }
            i += 1;
        }

        Ok(Args { out_dir })
    }

    // -----------------------------------------------------------------------
    // App enumeration
    // -----------------------------------------------------------------------

    /// A discovered app bundle with its display name.
    struct DiscoveredApp {
        bundle_path: PathBuf,
        display_name: String,
    }

    fn enumerate_apps() -> Vec<DiscoveredApp> {
        let mut apps: BTreeMap<String, DiscoveredApp> = BTreeMap::new();

        let roots = app_roots();
        for root in &roots {
            scan_root(root, &mut apps);
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
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for child in entries.flatten() {
            let path = child.path();
            let Ok(file_type) = child.file_type() else {
                continue;
            };
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            if path.extension().is_some_and(|ext| ext == "app") {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // De-duplicate by display name (first root wins).
                apps.entry(name.clone()).or_insert(DiscoveredApp {
                    bundle_path: path,
                    display_name: name,
                });
            }
        }
    }

    // -----------------------------------------------------------------------
    // Icon extraction (max resolution)
    // -----------------------------------------------------------------------

    /// Extract the app icon at its maximum available resolution via NSWorkspace.
    fn extract_icon_maxres(bundle_path: &Path) -> Option<DecodedIcon> {
        use objc2::rc::autoreleasepool;
        use objc2::AnyThread;
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
        use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

        autoreleasepool(|_| {
            let path = NSString::from_str(&bundle_path.to_string_lossy());
            let image = NSWorkspace::sharedWorkspace().iconForFile(&path);

            // Request the largest available representation. Proposing a 512-point
            // rect pushes NSWorkspace to return the highest-resolution icon it
            // has, up to 1024 px on Retina (1024 px on standard).
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

    // -----------------------------------------------------------------------
    // Processing
    // -----------------------------------------------------------------------

    struct ProcessedApp {
        entry: AuditEntry,
        normalized: NormalizedIcon,
        /// The decoded icon at max resolution (for comparison rendering).
        source: DecodedIcon,
    }

    fn process_app(app: &DiscoveredApp) -> Option<ProcessedApp> {
        let source = extract_icon_maxres(&app.bundle_path)?;
        let metrics = sizing::analyze(&source)?;
        let normalized = normalize::normalize(&source);

        let entry = AuditEntry::from_metrics(app.display_name.clone(), &metrics);

        Some(ProcessedApp {
            entry,
            normalized,
            source,
        })
    }

    // -----------------------------------------------------------------------
    // Rounded-rect mask
    // -----------------------------------------------------------------------

    /// Returns true if pixel (x, y) is inside a rounded rectangle of size
    /// `(w, h)` with corner radius `r`.
    fn inside_rounded_rect(x: u32, y: u32, w: u32, h: u32, r: f64) -> bool {
        let xf = x as f64;
        let yf = y as f64;
        let wf = w as f64;
        let hf = h as f64;

        // Horizontal strip.
        if xf >= r && xf < wf - r {
            return true;
        }
        // Vertical strip.
        if yf >= r && yf < hf - r {
            return true;
        }

        // Determine which corner. The circle centre for a rounded-rect of
        // radius r is (r, r) measured from the outer corner of the tile.
        let (cx, cy) = if xf < r && yf < r {
            // Top-left.
            (r, r)
        } else if xf >= wf - r && yf < r {
            // Top-right.
            (wf - r, r)
        } else if xf < r && yf >= hf - r {
            // Bottom-left.
            (r, hf - r)
        } else if xf >= wf - r && yf >= hf - r {
            // Bottom-right.
            (wf - r, hf - r)
        } else {
            // Between corner regions vertically — inside.
            return true;
        };

        let dx = xf - cx;
        let dy = yf - cy;
        dx * dx + dy * dy <= r * r
    }

    // -----------------------------------------------------------------------
    // Contact-sheet rendering
    // -----------------------------------------------------------------------

    /// Build a category contact sheet for `apps`.
    /// Each cell: background tile + centered normalized icon + rounded mask + label.
    fn build_contact_sheet(apps: &[&ProcessedApp], category_label: &str) -> RgbaImage {
        let n = apps.len();
        if n == 0 {
            eprintln!("  [skip] {category_label}: no apps in this category");
            return RgbaImage::new(1, 1);
        }

        let rows = n.div_ceil(COLS as usize) as u32;
        let sheet_w = COLS * CELL_W;
        let sheet_h = rows * CELL_H;

        let mut sheet = RgbaImage::from_pixel(sheet_w, sheet_h, Rgba([BG[0], BG[1], BG[2], 255]));

        for (idx, app) in apps.iter().enumerate() {
            let col = idx as u32 % COLS;
            let row = idx as u32 / COLS;

            let cell_x0 = col * CELL_W;
            let cell_y0 = row * CELL_H;

            // Draw the icon centered in the cell's icon area.
            let icon_x = cell_x0 + (CELL_W - TARGET) / 2;
            let icon_y = cell_y0 + PAD;

            let icon_pixels =
                RgbaImage::from_raw(TARGET, TARGET, app.normalized.image.rgba.clone())
                    .unwrap_or_else(|| RgbaImage::new(TARGET, TARGET));

            imageops::overlay(&mut sheet, &icon_pixels, icon_x.into(), icon_y.into());

            // Apply rounded-rect mask to this cell region.
            for y in cell_y0..(cell_y0 + CELL_H).min(sheet_h) {
                for x in cell_x0..(cell_x0 + CELL_W).min(sheet_w) {
                    if !inside_rounded_rect(x - cell_x0, y - cell_y0, CELL_W, CELL_H, CORNER_RADIUS)
                    {
                        sheet.put_pixel(x, y, Rgba([0, 0, 0, 0]));
                    }
                }
            }
        }

        sheet
    }

    // -----------------------------------------------------------------------
    // Before/after comparison sheet
    // -----------------------------------------------------------------------

    /// Build a comparison sheet showing [scale=1.0 | categorized scale] pairs.
    /// Only non-FullBleed apps are included (FullBleed would be identical).
    fn build_compare_sheet(apps: &[&ProcessedApp]) -> RgbaImage {
        // Filter to ThinLine + Solid only.
        let filtered: Vec<&&ProcessedApp> = apps
            .iter()
            .filter(|a| a.entry.category != "fullbleed")
            .collect();

        if filtered.is_empty() {
            eprintln!("  [skip] compare: no non-FullBleed apps");
            return RgbaImage::new(1, 1);
        }

        let n = filtered.len().min(MAX_COMPARE_ROWS);
        let filtered = &filtered[..n];

        // Layout: two 144 px columns (128 icon + 8 px padding each side).
        let col_w: u32 = 144;
        let row_h: u32 = 160; // icon + app-name label area
        let header_h: u32 = 26;
        let sheet_w = col_w * 2;
        let sheet_h = header_h + n as u32 * row_h;

        let mut sheet = RgbaImage::from_pixel(sheet_w, sheet_h, Rgba([BG[0], BG[1], BG[2], 255]));

        // Header bar: tinted bands to label the two columns visually.
        for y in 0..header_h {
            for x in 0..col_w {
                // Left column: subtle blue-grey tint ("scale = 1.0").
                sheet.put_pixel(x, y, Rgba([0xCC, 0xD5, 0xDF, 255]));
            }
            for x in col_w..sheet_w {
                // Right column: subtle green-grey tint ("scaled").
                sheet.put_pixel(x, y, Rgba([0xCC, 0xDF, 0xD5, 255]));
            }
        }

        // Thin divider line between the two columns.
        for y in 0..sheet_h {
            sheet.put_pixel(col_w - 1, y, Rgba([0xBB, 0xBB, 0xBB, 255]));
        }

        for (row, app) in filtered.iter().enumerate() {
            let row_y = header_h + row as u32 * row_h;

            // "Current" column: scale = 1.0 (simulate old pipeline).
            let current = normalize_with_fixed_scale(&app.source, 1.0);
            let cur_x = (col_w - TARGET) / 2;
            let cur_y = row_y + 4;
            let cur_img = RgbaImage::from_raw(TARGET, TARGET, current.rgba.clone())
                .unwrap_or_else(|| RgbaImage::new(TARGET, TARGET));
            imageops::overlay(&mut sheet, &cur_img, cur_x.into(), cur_y.into());

            // "Proposed" column: use normalize() result.
            let prop_x = col_w + (col_w - TARGET) / 2;
            let prop_y = row_y + 4;
            let prop_img = RgbaImage::from_raw(TARGET, TARGET, app.normalized.image.rgba.clone())
                .unwrap_or_else(|| RgbaImage::new(TARGET, TARGET));
            imageops::overlay(&mut sheet, &prop_img, prop_x.into(), prop_y.into());

            // Thin horizontal separator between rows.
            let sep_y = row_y + row_h - 1;
            if sep_y < sheet_h {
                for x in 0..sheet_w {
                    sheet.put_pixel(x, sep_y, Rgba([0xBB, 0xBB, 0xBB, 255]));
                }
            }
        }

        sheet
    }

    /// Run the normalization pipeline but with a forced scale factor instead of
    /// the classification-derived one. Used to simulate "old pipeline" (scale=1.0).
    fn normalize_with_fixed_scale(src: &DecodedIcon, scale: f64) -> DecodedIcon {
        if src.w == 0 || src.h == 0 {
            return DecodedIcon {
                rgba: vec![0; (TARGET as usize).pow(2) * 4],
                w: TARGET,
                h: TARGET,
            };
        }

        let src_img = RgbaImage::from_raw(src.w, src.h, src.rgba.clone())
            .unwrap_or_else(|| RgbaImage::from_pixel(TARGET, TARGET, Rgba([0, 0, 0, 0])));

        let cropped = crop_to_opaque_bounds(&src_img).unwrap_or_else(|| src_img.clone());
        let src_w = cropped.width();
        let src_h = cropped.height();

        let (new_w, new_h) = fit_dimensions(src_w, src_h, (TARGET as f64 * scale).round() as u32);
        let scaled = imageops::resize(&cropped, new_w, new_h, imageops::FilterType::Lanczos3);

        let mut canvas = RgbaImage::from_pixel(TARGET, TARGET, Rgba([0, 0, 0, 0]));
        let dx = (TARGET - new_w) / 2;
        let dy = (TARGET - new_h) / 2;
        imageops::overlay(&mut canvas, &scaled, dx.into(), dy.into());

        DecodedIcon {
            rgba: canvas.into_raw(),
            w: TARGET,
            h: TARGET,
        }
    }

    /// Fit (w, h) so longest side equals `target`, preserving aspect ratio.
    fn fit_dimensions(w: u32, h: u32, target: u32) -> (u32, u32) {
        if w == 0 || h == 0 {
            return (1, 1);
        }
        let max = w.max(h);
        let s = target as f64 / max as f64;
        let nw = ((w as f64 * s).round() as u32).max(1);
        let nh = ((h as f64 * s).round() as u32).max(1);
        (nw, nh)
    }

    /// Crop `src` to its opaque bounding-box (alpha > 10).
    fn crop_to_opaque_bounds(src: &RgbaImage) -> Option<RgbaImage> {
        let (w, h) = src.dimensions();
        let mut min_x = w;
        let mut min_y = h;
        let mut max_x = 0u32;
        let mut max_y = 0u32;

        for y in 0..h {
            for x in 0..w {
                if src.get_pixel(x, y)[3] > 10 {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }

        if min_x > max_x || min_y > max_y {
            return None;
        }

        let crop_w = max_x - min_x + 1;
        let crop_h = max_y - min_y + 1;
        if min_x == 0 && min_y == 0 && crop_w == w && crop_h == h {
            return None;
        }

        Some(imageops::crop_imm(src, min_x, min_y, crop_w, crop_h).to_image())
    }

    // -----------------------------------------------------------------------
    // Well-known app priority (for FullBleed capping)
    // -----------------------------------------------------------------------

    /// Keywords for well-known macOS apps. FullBleed contact sheet keeps
    /// matching apps first, then caps at MAX_FULLBLEED_SHEET.
    const WELL_KNOWN_KEYWORDS: &[&str] = &[
        "Safari",
        "Chrome",
        "Firefox",
        "Edge",
        "Xcode",
        "Terminal",
        "App Store",
        "System Settings",
        "System Preferences",
        "Calendar",
        "Mail",
        "Messages",
        "Notes",
        "Photos",
        "Music",
        "TV",
        "Podcasts",
        "Books",
        "Maps",
        "FaceTime",
        "Reminders",
        "Preview",
        "TextEdit",
        "Calculator",
        "Dictionary",
        "Find My",
        "Freeform",
        "GarageBand",
        "iMovie",
        "Keynote",
        "Numbers",
        "Pages",
        "QuickTime",
        "Script Editor",
        "Stickies",
        "Time Machine",
        "Voice Memos",
        "Weather",
        "Clock",
        "Contacts",
        "Home",
        "News",
        "Stocks",
        "Shortcuts",
        "Activity Monitor",
        "Disk Utility",
        "Console",
        "Keychain Access",
        "ColorSync Utility",
        "Digital Color Meter",
        "Grapher",
        "Screenshot",
        "Photo Booth",
        "Automator",
        "Font Book",
        "Image Capture",
        "Migration Assistant",
        "VoiceOver Utility",
        "Audio MIDI Setup",
        "Bluetooth File Exchange",
        "Boot Camp Assistant",
        "System Information",
        "AirPort Utility",
    ];

    fn is_well_known(name: &str) -> bool {
        WELL_KNOWN_KEYWORDS
            .iter()
            .any(|kw| name.eq_ignore_ascii_case(kw))
    }

    /// Sort FullBleed apps: well-known first, then alphabetically.
    fn sort_fullbleed(apps: &mut [&ProcessedApp]) {
        apps.sort_by(|a, b| {
            let a_known = is_well_known(&a.entry.name);
            let b_known = is_well_known(&b.entry.name);
            b_known.cmp(&a_known).then_with(|| {
                a.entry
                    .name
                    .to_lowercase()
                    .cmp(&b.entry.name.to_lowercase())
            })
        });
    }

    /// Sort non-FullBleed apps alphabetically.
    fn sort_other(apps: &mut [&ProcessedApp]) {
        apps.sort_by(|a, b| {
            a.entry
                .name
                .to_lowercase()
                .cmp(&b.entry.name.to_lowercase())
        });
    }

    // -----------------------------------------------------------------------
    // Main entry point
    // -----------------------------------------------------------------------

    pub fn run() {
        let args = match parse_args() {
            Ok(a) => a,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        };

        std::fs::create_dir_all(&args.out_dir).expect("create output directory");

        eprintln!("icon-audit: scanning applications...");
        let apps = enumerate_apps();
        eprintln!("  found {} .app bundles", apps.len());

        eprintln!("icon-audit: extracting icons and analyzing...");
        let mut processed: Vec<ProcessedApp> = Vec::with_capacity(apps.len());
        for (i, app) in apps.iter().enumerate() {
            if (i + 1) % 50 == 0 || i == apps.len() - 1 {
                eprintln!("  {}/{}", i + 1, apps.len());
            }
            if let Some(p) = process_app(app) {
                processed.push(p);
            }
        }
        eprintln!("  processed {} icons successfully", processed.len());

        // --- JSON report ---
        let report_path = args.out_dir.join("icon-audit-report.json");
        eprintln!(
            "icon-audit: writing JSON report → {}",
            report_path.display()
        );
        let entries: Vec<&AuditEntry> = processed.iter().map(|p| &p.entry).collect();
        let json = serde_json::to_string_pretty(&entries).expect("serialize JSON");
        std::fs::write(&report_path, &json).expect("write JSON report");

        // Count categories.
        let mut fullbleed = 0usize;
        let mut thinline = 0usize;
        let mut solid = 0usize;
        for p in &processed {
            match p.entry.category.as_str() {
                "fullbleed" => fullbleed += 1,
                "thinline" => thinline += 1,
                "solid" => solid += 1,
                _ => {}
            }
        }
        eprintln!("  categories: FullBleed={fullbleed}, ThinLine={thinline}, Solid={solid}");

        // --- Category contact sheets ---
        let processed_refs: Vec<&ProcessedApp> = processed.iter().collect();

        for (cat_label, cat_str) in &[
            ("FullBleed", "fullbleed"),
            ("ThinLine", "thinline"),
            ("Solid", "solid"),
        ] {
            let mut cat_apps: Vec<&ProcessedApp> = processed_refs
                .iter()
                .filter(|p| p.entry.category == *cat_str)
                .copied()
                .collect();

            if cat_apps.is_empty() {
                continue;
            }

            if *cat_str == "fullbleed" {
                sort_fullbleed(&mut cat_apps);
                if cat_apps.len() > MAX_FULLBLEED_SHEET {
                    eprintln!(
                        "  {cat_label}: capping {} → {MAX_FULLBLEED_SHEET} (well-known priority)",
                        cat_apps.len()
                    );
                    cat_apps.truncate(MAX_FULLBLEED_SHEET);
                }
            } else {
                sort_other(&mut cat_apps);
            }

            eprintln!(
                "  building {cat_label} contact sheet ({} icons)...",
                cat_apps.len()
            );
            let sheet = build_contact_sheet(&cat_apps, cat_label);
            let path = args.out_dir.join(format!("icon-audit-{cat_str}.png"));
            sheet.save(&path).expect("write contact sheet PNG");
            eprintln!("    → {}", path.display());
        }

        // --- Before/after comparison sheet ---
        eprintln!("  building compare sheet...");
        let compare = build_compare_sheet(&processed_refs);
        let compare_path = args.out_dir.join("icon-audit-compare.png");
        compare
            .save(&compare_path)
            .expect("write compare sheet PNG");
        eprintln!("    → {}", compare_path.display());

        eprintln!("icon-audit: done.");
    }
}

#[cfg(target_os = "macos")]
fn main() {
    macos_audit::run();
}
