//! Background icon-extraction worker.
//!
//! Runs on its own OS thread so Shell/GDI/COM never stalls the UI thread. The
//! UI sends [`IconRequest`]s via the request channel; the worker posts
//! [`IconResult`]s back via the result channel (which the UI polls from its
//! event loop). Only *ownable* Rust data crosses the boundary — no `HICON`,
//! `HBITMAP`, or `HDC` ever reaches the UI thread.
//!
//! COM is initialized once per worker thread (`COINIT_APARTMENTTHREADED`).
//! Each request is processed in isolation; one failure (or even a panic) can't
//! take down the rest of the batch — panics are caught and reported as a
//! [`IconResult::Failed`] for that id.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::domain::app_diff::IconSource;
use crate::domain::app_id::AppId;
use crate::icon_cache::{self, CachedIcon, IconCache};
#[cfg(windows)]
use crate::icons::extract::{self, ComScope};
#[cfg(target_os = "macos")]
use crate::icons::normalize::normalize_macos_app_icon;
use crate::icons::normalize::{normalize, DecodedIcon};
use crate::icons::IconCategory;
use crate::startup_timer::{self, prefix};

/// Why we're asking for an icon. Drives logging only.
#[derive(Debug, Clone)]
pub enum IconReason {
    /// First-time extraction (cache miss at startup).
    Fresh,
    /// Cache probe says the stored icon is stale.
    Stale,
    /// A rescan reported this app as updated.
    Updated,
}

/// A unit of work the UI hands to the worker.
#[derive(Debug, Clone)]
pub struct IconRequest {
    pub app_id: AppId,
    pub name: String,
    pub link_path: PathBuf,
    /// Snapshot fields used as the cache key (so the worker can validate /
    /// store against them without re-resolving the `.lnk`).
    pub source_modified_at: u64,
    pub target_path: String,
    pub target_modified_at: u64,
    pub icon_location: String,
    pub icon_index: i32,
    pub provider_icon: Option<IconSource>,
    pub reason: IconReason,
}

/// What the worker hands back. One message per request.
#[derive(Debug)]
pub enum IconResult {
    /// Icon extracted (and written to the cache). `image` is normalized RGBA.
    Loaded {
        app_id: AppId,
        image: DecodedIcon,
        hide_tile_fill: bool,
    },
    /// Extraction failed; UI should keep the placeholder.
    Failed { app_id: AppId, error: String },
}

/// Handle returned by [`spawn`]: drop the `Sender` side to stop the worker.
pub struct WorkerHandle {
    pub requests: Sender<IconRequest>,
}

/// Spawn the icon worker. Shares the cache via `Arc<IconCache>` (SQLite handles
/// its own locking). Returns the request sender; results arrive on `results`.
pub fn spawn(cache: Arc<IconCache>, results: Sender<IconResult>) -> WorkerHandle {
    let (tx, rx): (Sender<IconRequest>, Receiver<IconRequest>) = mpsc::channel();
    thread::Builder::new()
        .name("icon-worker".to_string())
        .spawn(move || worker_loop(rx, cache, results))
        .expect("spawn icon-worker");

    WorkerHandle { requests: tx }
}

fn worker_loop(rx: Receiver<IconRequest>, cache: Arc<IconCache>, results: Sender<IconResult>) {
    // COM lives for the whole thread. STA matches the shell's expectations.
    #[cfg(windows)]
    let _com = ComScope::new();
    let timer = startup_timer::get();

    use std::time::Duration;
    // Grace period after the queue drains during which a new request still
    // counts as part of the same "batch" (so we don't fragment one logical
    // startup scan into many totals). Anything arriving after this emits a
    // fresh total for the next batch.
    const BATCH_GRACE: Duration = Duration::from_millis(500);

    // Per-batch accumulator for the "icon extraction total" timing mark.
    let mut batch_start: Option<std::time::Instant> = None;
    let mut batch_extract_ms: u128 = 0;
    let mut batch_count: u32 = 0;

    loop {
        let next = if batch_start.is_some() {
            // Mid-batch: wait briefly for more requests before closing it out.
            rx.recv_timeout(BATCH_GRACE)
        } else {
            // Idle: block indefinitely for the next batch to start.
            match rx.recv() {
                Ok(m) => Ok(m),
                Err(_) => break,
            }
        };

        match next {
            Ok(req) => {
                if batch_start.is_none() {
                    batch_start = Some(std::time::Instant::now());
                }
                let extract_t0 = std::time::Instant::now();
                // Process inside a catch_unwind so a panic in extraction can't
                // kill the worker (and leave the UI waiting forever).
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    process_one(&req, &cache, &timer)
                }));
                batch_extract_ms += extract_t0.elapsed().as_millis();
                batch_count += 1;

                let result = match outcome {
                    Ok(Ok((image, hide_tile_fill))) => IconResult::Loaded {
                        app_id: req.app_id.clone(),
                        image,
                        hide_tile_fill,
                    },
                    Ok(Err(err)) => IconResult::Failed {
                        app_id: req.app_id.clone(),
                        error: err,
                    },
                    Err(_) => IconResult::Failed {
                        app_id: req.app_id.clone(),
                        error: "icon worker panicked".to_string(),
                    },
                };
                if results.send(result).is_err() {
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Queue drained → close out the batch with a total mark.
                if let Some(start) = batch_start {
                    timer.mark_with(prefix::ICON_WORKER, "icon extraction total", || {
                        format!(
                            "({} icons, extract={}ms, total={}ms)",
                            batch_count,
                            batch_extract_ms,
                            start.elapsed().as_millis()
                        )
                    });
                }
                batch_start = None;
                batch_extract_ms = 0;
                batch_count = 0;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Extract + normalize + cache one icon. Returns the normalized image on
/// success. Errors are logged with the app_id and propagated.
///
/// Emits two distinct timing marks per the startup-logging spec: an
/// `icon-worker: extracted icon` mark covering the Shell/GDI extraction, and
/// an `icon-worker: normalized icon` mark covering the resize-to-TARGET step.
fn process_one(
    req: &IconRequest,
    cache: &IconCache,
    timer: &startup_timer::StartupTimer,
) -> Result<(DecodedIcon, bool), String> {
    // (1) Extraction: Shell/GDI/COM → raw RGBA. This is the expensive part.
    let extract_start = std::time::Instant::now();
    let raw = extract_request_icon(req).ok_or_else(|| {
        format!(
            "no icon for app_id={} path={}",
            req.app_id,
            req.link_path.display()
        )
    })?;
    timer.mark_with(prefix::ICON_WORKER, "extracted icon", || {
        format!(
            "app_id={} ({}ms)",
            req.app_id,
            extract_start.elapsed().as_millis()
        )
    });

    // (2) Normalization: resize → TARGET×TARGET straight-alpha square.
    let normalize_start = std::time::Instant::now();
    #[cfg(target_os = "macos")]
    let image = if req.link_path.extension().is_some_and(|ext| ext == "app") {
        // NSWorkspace app icons include a platform drop shadow. Use the opaque
        // app body for geometry so the body, rather than the shadow, aligns to
        // the launcher's tile fill. Steam/local image files retain the generic
        // cross-platform normalization path.
        normalize_macos_app_icon(&raw)
    } else {
        normalize(&raw)
    };
    #[cfg(not(target_os = "macos"))]
    let image = normalize(&raw);
    timer.mark_with(prefix::ICON_WORKER, "normalized icon", || {
        format!(
            "app_id={} ({}ms)",
            req.app_id,
            normalize_start.elapsed().as_millis()
        )
    });

    // Best-effort cache write; a failure is logged but doesn't fail the
    // extraction itself (the UI still gets the icon for this session).
    let entry = CachedIcon {
        app_id: req.app_id.clone(),
        link_path: req.link_path.to_string_lossy().into_owned(),
        display_name: req.name.clone(),
        source_modified_at: req.source_modified_at,
        target_path: req.target_path.clone(),
        target_modified_at: req.target_modified_at,
        icon_location: req.icon_location.clone(),
        icon_index: req.icon_index,
        provider_icon: req.provider_icon.clone(),
        image: image.image.clone(),
        extracted_at_version: icon_cache::EXTRACTION_VERSION,
        category: image.category,
        scale: image.scale as f32,
    };
    if let Err(e) = cache.put(&entry) {
        eprintln!("icon-cache: write failed for app_id={}: {e}", req.app_id);
    }

    let hide_tile_fill = image.category == IconCategory::FullBleed;
    Ok((image.image, hide_tile_fill))
}

/// Artwork selection depends on provider metadata, never the chosen launch URI.
pub(crate) fn extract_request_icon(req: &IconRequest) -> Option<DecodedIcon> {
    if let Some(icon) = req.provider_icon.as_ref().and_then(extract_provider_icon) {
        return Some(icon);
    }
    #[cfg(target_os = "macos")]
    {
        crate::platform::macos::apps::extract_icon(&req.link_path, &req.icon_location)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = req;
        None
    }
    #[cfg(windows)]
    {
        extract::extract_icon_from_lnk(&req.link_path)
    }
}

fn extract_provider_icon(source: &IconSource) -> Option<DecodedIcon> {
    let path = std::path::Path::new(&source.path);
    #[cfg(windows)]
    if path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("dll"))
    {
        // Read PE icon resources directly, without loading the entire executable.
        return extract::extract_icon_from_path(path);
    }
    #[cfg(target_os = "macos")]
    if path.is_dir() && path.extension().is_some_and(|ext| ext == "app") {
        return crate::platform::macos::apps::extract_icon(path, "");
    }
    // A missing/corrupt image is not a generic file-type icon: let the caller
    // fall back to the original Shell item or shortcut.
    let bytes = std::fs::read(path).ok()?;
    image::load_from_memory(&bytes)
        .ok()
        .map(DecodedIcon::from_dynamic)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(link: &str, provider: Option<IconSource>) -> IconRequest {
        IconRequest {
            app_id: AppId::from_normalized("test:123"),
            name: "Arbitrary label".into(),
            link_path: link.into(),
            source_modified_at: 0,
            target_path: String::new(),
            target_modified_at: 0,
            icon_location: String::new(),
            icon_index: 0,
            provider_icon: provider,
            reason: IconReason::Fresh,
        }
    }

    fn temporary_artwork() -> PathBuf {
        std::env::temp_dir().join(format!(
            "launchpad-provider-{}-{}.png",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn provider_image_is_used_for_any_launch_route() {
        let path = temporary_artwork();
        let pixels = image::RgbaImage::from_pixel(256, 256, image::Rgba([10, 20, 30, 255]));
        pixels.save(&path).unwrap();
        for link in [
            "shell:AppsFolder\\vendor.app",
            "shortcut.lnk",
            "steam://rungameid/123",
        ] {
            let req = request(
                link,
                Some(IconSource {
                    path: path.to_string_lossy().into_owned(),
                    modified_at: 1,
                }),
            );
            let actual = extract_request_icon(&req).unwrap();
            assert_eq!((actual.w, actual.h), (256, 256));
            assert_eq!(actual.rgba, pixels.as_raw().as_slice());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn missing_or_invalid_provider_artwork_falls_back_to_launch_icon() {
        let _com = ComScope::new();
        let target = std::env::current_exe().unwrap();
        let mut req = request(target.to_str().unwrap(), None);
        let expected = extract_request_icon(&req).unwrap();
        let path = temporary_artwork();
        req.provider_icon = Some(IconSource {
            path: path.to_string_lossy().into_owned(),
            modified_at: 1,
        });
        let missing = extract_request_icon(&req).unwrap();
        assert_eq!(missing.rgba, expected.rgba);
        std::fs::write(&path, b"not an image").unwrap();
        let corrupt = extract_request_icon(&req).unwrap();
        assert_eq!(corrupt.rgba, expected.rgba);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Read-only installed Steam artwork diagnostic; writes PNGs under target"]
    fn report_catalog_provider_icons() {
        use crate::domain::{app_catalog::IdentityAliases, launcher_state::LauncherState};
        let _com = ComScope::new();
        let scan = crate::workers::app_scan::scan_catalog();
        eprintln!("Scan diagnostics: {:?}", scan.errors);
        assert!(!scan
            .failed_sources
            .contains(&crate::domain::app_catalog::Source::Steam));
        let expected: std::collections::BTreeMap<_, _> = scan
            .candidates
            .iter()
            .filter_map(|c| {
                c.entry
                    .provider_icon
                    .clone()
                    .map(|icon| (c.identity.clone().unwrap(), icon))
            })
            .collect();
        let mut aliases = IdentityAliases::default();
        let snapshot = aliases.reconcile(
            aliases.groups(scan.candidates),
            &mut LauncherState::default(),
        );
        let out = PathBuf::from("target/steam-icon-probe");
        std::fs::create_dir_all(&out).unwrap();
        assert!(!expected.is_empty());
        for (id, source) in &expected {
            let entry = &snapshot[&AppId::from_normalized(id)];
            assert_eq!(entry.provider_icon.as_ref(), Some(source));
            let req = request(&entry.link_path, entry.provider_icon.clone());
            let started = std::time::Instant::now();
            let actual = extract_request_icon(&req).unwrap();
            let elapsed = started.elapsed();
            let provider = extract_provider_icon(source).unwrap();
            assert_eq!(actual.rgba, provider.rgba);
            let stem = id.replace(':', "-");
            for (suffix, raw) in [
                ("after", Some(actual.clone())),
                (
                    "before",
                    extract_request_icon(&request(&entry.link_path, None)),
                ),
            ] {
                if let Some(raw) = raw {
                    let normalized = normalize(&raw).image;
                    image::save_buffer(
                        out.join(format!("{stem}-{suffix}.png")),
                        &normalized.rgba,
                        normalized.w,
                        normalized.h,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
            eprintln!(
                "{} | {} | launch={} | artwork={} | {}x{} | {:.1}ms",
                id,
                entry.name,
                entry.link_path,
                source.path,
                actual.w,
                actual.h,
                elapsed.as_secs_f64() * 1000.0
            );
        }
        eprintln!(
            "Verified {} provider icons across {} catalog entries",
            expected.len(),
            snapshot.len()
        );
    }
}
