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

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::domain::app_id::AppId;
use crate::icon_cache::{self, CachedIcon, IconCache};
use crate::icons::adaptive_normalize;
#[cfg(windows)]
use crate::icons::extract::{self, ComScope};
use crate::icons::normalize::DecodedIcon;
use crate::icons::scale_model::{default_model_dir, ScalePolicy, POLICY_REVISION_FILE_NAME};
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
    let policy_dir = cache
        .path()
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty() && cache.path() != Path::new(":memory:"))
        .map(Path::to_path_buf)
        .unwrap_or_else(default_model_dir);
    let scale_policy = Arc::new(ScalePolicy::load_from_dir(&policy_dir));
    ensure_policy_revision(&cache, &policy_dir, &scale_policy);

    let (tx, rx): (Sender<IconRequest>, Receiver<IconRequest>) = mpsc::channel();
    thread::Builder::new()
        .name("icon-worker".to_string())
        .spawn(move || worker_loop(rx, cache, scale_policy, results))
        .expect("spawn icon-worker");

    WorkerHandle { requests: tx }
}

fn ensure_policy_revision(cache: &IconCache, policy_dir: &Path, policy: &ScalePolicy) {
    if cache.path() == Path::new(":memory:") {
        return;
    }
    if let Err(error) = std::fs::create_dir_all(policy_dir) {
        eprintln!(
            "icon-scale: could not create policy directory {}: {error}",
            policy_dir.display()
        );
        return;
    }

    let marker_path = policy_dir.join(POLICY_REVISION_FILE_NAME);
    let stored_revision = std::fs::read_to_string(&marker_path)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok());
    let current_revision = policy.revision();
    if stored_revision == Some(current_revision) {
        return;
    }

    // A missing marker with no model/overrides is the legacy rule-only state;
    // record it without forcing a pointless first-run re-extraction. Any real
    // policy transition invalidates the baked 128×128 cache pixels.
    if stored_revision.is_some() || current_revision != 0 {
        match cache.clear_all() {
            Ok(count) => {
                eprintln!("icon-scale: policy revision changed; invalidated {count} cached icons")
            }
            Err(error) => {
                // Do not advance the marker. The next launch must retry the
                // invalidation instead of accepting stale, already-baked pixels.
                eprintln!("icon-scale: cache invalidation failed: {error}");
                return;
            }
        }
    }
    if let Err(error) = std::fs::write(&marker_path, current_revision.to_string()) {
        eprintln!(
            "icon-scale: could not write policy marker {}: {error}",
            marker_path.display()
        );
    }
    eprintln!(
        "icon-scale: loaded model={} manual_overrides={}",
        policy.has_model(),
        policy.override_count()
    );
}

fn worker_loop(
    rx: Receiver<IconRequest>,
    cache: Arc<IconCache>,
    scale_policy: Arc<ScalePolicy>,
    results: Sender<IconResult>,
) {
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
                    process_one(&req, &cache, &scale_policy, &timer)
                }));
                batch_extract_ms += extract_t0.elapsed().as_millis();
                batch_count += 1;

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
                    break;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Queue drained → close out the batch with a total mark.
                if let Some(start) = batch_start {
                    timer.mark_with(
                        prefix::ICON_WORKER,
                        "icon extraction total",
                        format!(
                            "({} icons, extract={}ms, total={}ms)",
                            batch_count,
                            batch_extract_ms,
                            start.elapsed().as_millis()
                        ),
                    );
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
    scale_policy: &ScalePolicy,
    timer: &startup_timer::StartupTimer,
) -> Result<DecodedIcon, String> {
    // (1) Extraction: Shell/GDI/COM → raw RGBA. This is the expensive part.
    let extract_start = std::time::Instant::now();
    let raw = extract_request_icon(req).ok_or_else(|| {
        format!(
            "no icon for app_id={} path={}",
            req.app_id,
            req.link_path.display()
        )
    })?;
    timer.mark_with(
        prefix::ICON_WORKER,
        "extracted icon",
        format!(
            "app_id={} ({}ms)",
            req.app_id,
            extract_start.elapsed().as_millis()
        ),
    );

    // (2) Normalization: features + policy → one resize into TARGET×TARGET.
    let normalize_start = std::time::Instant::now();
    let source_path = req.link_path.to_string_lossy();
    let image = adaptive_normalize::normalize_for_app(
        &raw,
        req.app_id.as_ref(),
        &source_path,
        scale_policy,
    );
    timer.mark_with(
        prefix::ICON_WORKER,
        "normalized icon",
        format!(
            "app_id={} scale={:.3} ({}ms)",
            req.app_id,
            image.scale,
            normalize_start.elapsed().as_millis()
        ),
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
        image: image.image.clone(),
        extracted_at_version: icon_cache::EXTRACTION_VERSION,
        category: image.category,
        scale: image.scale as f32,
    };
    if let Err(e) = cache.put(&entry) {
        eprintln!("icon-cache: write failed for app_id={}: {e}", req.app_id);
    }

    Ok(image.image)
}

fn extract_request_icon(req: &IconRequest) -> Option<DecodedIcon> {
    #[cfg(target_os = "macos")]
    {
        if req.link_path.to_string_lossy().starts_with("steam://") {
            let icon_path = PathBuf::from(&req.icon_location);
            if let Ok(bytes) = std::fs::read(&icon_path) {
                if let Ok(image) = image::load_from_memory(&bytes) {
                    return Some(DecodedIcon::from_dynamic(image));
                }
            }
            if icon_path.extension().is_some_and(|ext| ext == "app") {
                return crate::platform::macos::apps::extract_icon(&icon_path, "");
            }
            return None;
        }
        crate::platform::macos::apps::extract_icon(&req.link_path, &req.icon_location)
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = req;
        None
    }

    #[cfg(windows)]
    {
        if req.link_path.to_string_lossy().starts_with("steam://") {
            let icon_path = PathBuf::from(&req.icon_location);
            if let Ok(bytes) = std::fs::read(&icon_path) {
                if let Ok(image) = image::load_from_memory(&bytes) {
                    return Some(DecodedIcon::from_dynamic(image));
                }
            }
            return extract::extract_icon_from_path(&icon_path);
        }
        extract::extract_icon_from_lnk(&req.link_path)
    }
}
