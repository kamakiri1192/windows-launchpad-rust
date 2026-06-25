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

use crate::app_id::AppId;
use crate::icon_cache::{self, CachedIcon, IconCache};
use crate::icons::extract::{self, ComScope};
use crate::icons::normalize::{normalize, DecodedIcon};
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
    pub link_mtime: u64,
    pub target_path: String,
    pub target_mtime: u64,
    pub icon_location: String,
    pub icon_index: i32,
    pub reason: IconReason,
}

/// What the worker hands back. One message per request.
#[derive(Debug)]
pub enum IconResult {
    /// Icon extracted (and written to the cache). `image` is normalized RGBA.
    Loaded { app_id: AppId, image: DecodedIcon },
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
    let _com = ComScope::new();
    let timer = startup_timer::get();

    for req in rx {
        // Process inside a catch_unwind so a panic in extraction can't kill
        // the worker thread (and leave the UI waiting forever).
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_one(&req, &cache, &timer)
        }));
        let result = match outcome {
            Ok(Ok(image)) => IconResult::Loaded {
                app_id: req.app_id.clone(),
                image,
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
            // UI went away; stop the worker.
            break;
        }
    }
}

/// Extract + normalize + cache one icon. Returns the normalized image on
/// success. Errors are logged with the app_id and propagated.
fn process_one(
    req: &IconRequest,
    cache: &IconCache,
    timer: &startup_timer::StartupTimer,
) -> Result<DecodedIcon, String> {
    let start = std::time::Instant::now();
    let raw = extract::extract_icon_from_lnk(&req.link_path).ok_or_else(|| {
        format!(
            "no icon for app_id={} path={}",
            req.app_id,
            req.link_path.display()
        )
    })?;
    let image = normalize(&raw);
    let dur = start.elapsed();
    timer.mark_with(
        prefix::ICON_WORKER,
        "extracted icon",
        format!("app_id={} ({}ms)", req.app_id, dur.as_millis()),
    );

    // Best-effort cache write; a failure is logged but doesn't fail the
    // extraction itself (the UI still gets the icon for this session).
    let entry = CachedIcon {
        app_id: req.app_id.clone(),
        link_path: req.link_path.to_string_lossy().into_owned(),
        display_name: req.name.clone(),
        link_mtime: req.link_mtime,
        target_path: req.target_path.clone(),
        target_mtime: req.target_mtime,
        icon_location: req.icon_location.clone(),
        icon_index: req.icon_index,
        image: image.clone(),
        extracted_at_version: icon_cache::EXTRACTION_VERSION,
    };
    if let Err(e) = cache.put(&entry) {
        eprintln!("icon-cache: write failed for app_id={}: {e}", req.app_id);
    }

    Ok(image)
}
