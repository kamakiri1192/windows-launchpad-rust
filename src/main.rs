#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// The startup/icon pipeline exposes a number of items that are part of its
// public surface or reserved for near-term phases (the `Stale` icon state,
// registry/cache inspectors, diff helpers, the `Mark` timing record, …). They
// aren't all wired into the event loop yet, so we allow dead_code crate-wide
// rather than littering every struct with #[allow].
#![allow(dead_code)]

//! Launchpad (Windows) — app launcher entry point.
//!
//! Startup pipeline (see docs/STARTUP_PERFORMANCE.md for the full design):
//!   1. Create the window + renderer.
//!   2. **Paint the first frame immediately** (empty/loading state) — we do
//!      *not* wait for icons.
//!   3. The [`app_scan`] + [`icon_worker`] threads run in the background:
//!        - the worker serves cached icons first, then extracts misses;
//!        - the refresh watcher detects Start Menu changes while we run.
//!   4. Icons arrive one at a time as [`UserEvent::IconLoaded`] and are blitted
//!      into a single fixed-slot atlas texture, so the UI never stalls on
//!      Shell/GDI/COM.
//!
//! Operation:
//!   - Left-drag horizontally → page swipe with rubber-band + spring snap.
//!   - Click an app icon → launch its Start Menu shortcut.
//!   - Esc → quit.

mod app;
mod app_icon;
mod debug_logger;
mod domain;
mod features;
mod grid;
mod icon_cache;
mod icons;
mod layout;
mod liquid_glass;
mod platform;
mod qa;
mod renderer;
mod scroll;
mod startup_timer;
mod ui_model;
mod workers;

use std::sync::Arc;
use std::sync::Mutex;

use icon_cache::IconCache;
use renderer::icon_atlas::IconAtlas;
use startup_timer::{prefix, StartupTimer};
use winit::dpi::PhysicalPosition;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::Icon;
use workers::icon_worker::IconResult;
use workers::refresh_watcher::{RefreshConfig, RefreshMessage};

/// Cell edge (icon + padding) imported from the atlas module for readability.
const CELL: u32 = renderer::icon_atlas::CELL;

// ---- app shell re-exports ------------------------------------------------
// The `App` struct, its constructor, runtime value types (`PendingPress`,
// `SettingsPressTarget`, `WorkerMessage`, `Inbox`), `UserEvent`, and the
// shell constants (`CLICK_SLOP_PHYS`, `INITIAL_WINDOW_*`, `SUMMON_FOCUS_GRACE`,
// `LONG_PRESS_THRESHOLD`, `EDIT_EDGE_SCROLL_ZONE`) now live in `src/app/`.
// They are re-exported here so the adapter code that remains in `main.rs`
// (the `impl App` methods, the free renderer helpers, and `main()`) keeps
// referring to them by their historical unqualified names. Phase 5 moves the
// method bodies into `app/` incrementally; this `use` is the bridge.
use app::state::{App, Inbox, WorkerMessage, INITIAL_WINDOW_HEIGHT, INITIAL_WINDOW_WIDTH};
use app::UserEvent;

pub(crate) fn initial_window_position(
    event_loop: &ActiveEventLoop,
) -> Option<PhysicalPosition<i32>> {
    let monitor = event_loop
        .primary_monitor()
        .or_else(|| event_loop.available_monitors().next())?;
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let scale_factor = monitor.scale_factor();

    let window_width = (INITIAL_WINDOW_WIDTH * scale_factor).round() as i64;
    let window_height = (INITIAL_WINDOW_HEIGHT * scale_factor).round() as i64;
    let x = monitor_position.x as i64 + (monitor_size.width as i64 - window_width) / 2;
    let y = monitor_position.y as i64 + (monitor_size.height as i64 - window_height) / 2;

    Some(PhysicalPosition::new(
        x.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        y.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
    ))
}

pub(crate) fn load_window_icon() -> Option<Icon> {
    let icon = app_icon::load_rgba(Some(256))?;
    Icon::from_rgba(icon.rgba, icon.width, icon.height).ok()
}

/// Bridge the merged background channel into the shared inbox + wake the UI
/// thread. One forwarder thread serves both the icon worker and the refresh
/// watcher (they share `merged_rx`).
fn forward_inbox(
    rx: std::sync::mpsc::Receiver<WorkerMessage>,
    inbox: Arc<Inbox>,
    proxy: EventLoopProxy<UserEvent>,
) {
    std::thread::Builder::new()
        .name("inbox-forwarder".to_string())
        .spawn(move || {
            while let Ok(msg) = rx.recv() {
                if let Ok(mut g) = inbox.lock() {
                    g.push(msg);
                }
                // Wake the UI thread; it drains the inbox on receipt.
                if proxy.send_event(UserEvent::InboxWakeup).is_err() {
                    break;
                }
            }
        })
        .expect("spawn inbox-forwarder");
}

/// Diagnostic helper: write the current CPU-side icon atlas to
/// `target/atlas-dump.png` so we can eyeball that cells don't overlap after a
/// grow. Only runs when `LAUNCHPAD_DUMP_ATLAS` is set.
pub(crate) fn dump_atlas_png(atlas: &IconAtlas) {
    let path = std::path::Path::new("target/atlas-dump.png");
    match image::save_buffer(
        path,
        atlas.rgba(),
        atlas.width(),
        atlas.height(),
        image::ColorType::Rgba8,
    ) {
        Ok(()) => eprintln!(
            "icon-atlas: dumped {}x{} atlas to {}",
            atlas.width(),
            atlas.height(),
            path.display(),
        ),
        Err(e) => eprintln!("icon-atlas: dump failed: {e}"),
    }
}

fn main() {
    #[cfg(windows)]
    let _single_instance = if std::env::var_os(qa::SCENARIO_ENV).is_some() {
        // Hidden deterministic QA must be able to run beside the user's
        // foreground launcher (and beside other branch worktrees). It owns no
        // tray/hotkey/persistence state, so the production singleton does not
        // apply to this process.
        None
    } else {
        match platform::windows::SingleInstanceGuard::acquire() {
            Ok(guard) => Some(guard),
            Err(e) if e.is_already_running() => {
                crate::debug_log!("single-instance: existing instance signaled");
                return;
            }
            Err(e) => {
                eprintln!("single-instance: {e}");
                std::process::exit(1);
            }
        }
    };

    let timer = StartupTimer::new();
    timer.mark(prefix::STARTUP, "process start");
    startup_timer::install(timer.clone());

    // File-backed debug logger. Opt-in via LAUNCHPAD_DEBUG env var so the
    // release build is silent by default; when on it writes to
    // %LOCALAPPDATA%\Launchpad\debug.log (visible even with no console).
    debug_logger::init();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // `--reset-cache`: delete the SQLite cache file before opening so the next
    // launch rebuilds it from scratch. Useful if the cache is corrupted or you
    // want to force a clean re-extraction without editing EXTRACTION_VERSION.
    let reset_cache_requested = std::env::args().any(|a| a == "--reset-cache");
    if reset_cache_requested {
        let path = icon_cache::default_db_path();
        eprintln!("icon-cache: --reset-cache: removing {}", path.display());
        let _ = std::fs::remove_file(&path);
        // WAL/SHM sidecars too, if present.
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    // Open (or rebuild) the SQLite cache before the event loop starts so it's
    // ready for the first scan. This is cheap (a few ms) and never blocks on
    // Shell/GDI.
    let cache = Arc::new(IconCache::open_or_rebuild());

    // Shared inbox for worker + watcher → UI.
    let inbox: Arc<Inbox> = Arc::new(Mutex::new(Vec::new()));

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    // One merged channel: both background threads send `WorkerMessage`s here.
    let (merged_tx, merged_rx): (
        std::sync::mpsc::Sender<WorkerMessage>,
        std::sync::mpsc::Receiver<WorkerMessage>,
    ) = std::sync::mpsc::channel();

    // Spawn the icon worker, bridging its typed results into the merged channel.
    let (result_tx, result_rx): (
        std::sync::mpsc::Sender<IconResult>,
        std::sync::mpsc::Receiver<IconResult>,
    ) = std::sync::mpsc::channel();
    let worker = workers::icon_worker::spawn(cache.clone(), result_tx);
    spawn_bridge(result_rx, merged_tx.clone(), WorkerMessage::Icon);

    // Spawn the Start Menu refresh watcher, bridging it the same way.
    let (refresh_tx, refresh_rx): (
        std::sync::mpsc::Sender<RefreshMessage>,
        std::sync::mpsc::Receiver<RefreshMessage>,
    ) = std::sync::mpsc::channel();
    workers::refresh_watcher::spawn(refresh_tx, RefreshConfig::default());
    spawn_bridge(refresh_rx, merged_tx, WorkerMessage::Refresh);

    // Single forwarder for the merged channel into the shared inbox.
    forward_inbox(merged_rx, inbox.clone(), proxy.clone());

    // OS integration: global hot key (Win+Space) + tray icon. Spawned before
    // the event loop so the hot key works even during the very first frame.
    #[cfg(windows)]
    let os = (std::env::var_os(qa::SCENARIO_ENV).is_none())
        .then(|| platform::windows::OsIntegrationHandle::spawn(proxy.clone()));

    let mut app = App::new(proxy, timer, cache, inbox, worker);
    // Anchor the OS-integration thread for the whole process lifetime.
    #[cfg(windows)]
    {
        app._os = os;
    }
    // Restore the user's saved layout (drag-to-reorder + hidden apps) before the
    // first scan lands, so apps appear in the user's arrangement from frame one.
    if app.qa_enabled() {
        app.install_qa_fixture();
    } else {
        app.load_customization();
    }
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("event loop error: {e}");
        std::process::exit(1);
    }
}

/// Pump one typed mpsc channel into the merged `WorkerMessage` channel by
/// wrapping each item with `wrap`. Lets the icon worker (`IconResult`) and the
/// refresh watcher (`RefreshMessage`) share a single inbox without their
/// public APIs depending on `WorkerMessage`.
fn spawn_bridge<T: Send + 'static>(
    rx: std::sync::mpsc::Receiver<T>,
    tx: std::sync::mpsc::Sender<WorkerMessage>,
    wrap: fn(T) -> WorkerMessage,
) {
    std::thread::Builder::new()
        .name("channel-bridge".to_string())
        .spawn(move || {
            while let Ok(item) = rx.recv() {
                if tx.send(wrap(item)).is_err() {
                    break;
                }
            }
        })
        .expect("spawn channel-bridge");
}

#[cfg(test)]
mod tests {
    use super::App;

    #[test]
    fn search_matching_is_case_insensitive_for_ascii() {
        assert!(App::matches_search("Windows Terminal", "terminal"));
        assert!(App::matches_search("Windows Terminal", "WIN term"));
        assert!(!App::matches_search("Windows Terminal", "memo"));
    }

    #[test]
    fn search_matching_handles_japanese_names() {
        assert!(App::matches_search("メモ帳", "メモ"));
        assert!(App::matches_search("アプリ設定", "アプリ"));
        assert!(!App::matches_search("メモ帳", "アプリ"));
    }
}
