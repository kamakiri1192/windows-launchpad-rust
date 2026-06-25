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

mod app_diff;
mod app_id;
mod app_registry;
mod app_scan;
mod grid;
mod icon_atlas;
mod icon_cache;
mod icon_pipeline;
mod icon_worker;
mod icons;
mod launch;
mod liquid_glass;
mod refresh_watcher;
mod renderer;
mod scroll;
mod startup_timer;
mod text;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use app_diff::{AppDiff, SnapshotEntry};
use app_id::AppId;
use app_registry::{AppLaunchInfo, AppRecord, AppRegistry, IconState};
use icon_atlas::IconAtlas;
use icon_cache::{CacheProbe, CachedIcon, IconCache};
use icon_worker::{IconReason, IconRequest, IconResult, WorkerHandle};
use icons::normalize::DecodedIcon;
use refresh_watcher::{RefreshConfig, RefreshMessage};
use renderer::{DrawArgs, Renderer};
use scroll::{Phase, Scroller};
use startup_timer::{prefix, StartupTimer};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId};

/// Cell edge (icon + padding) imported from the atlas module for readability.
const CELL: u32 = icon_atlas::CELL;
const CLICK_SLOP_PHYS: f32 = 8.0;

/// Messages delivered to the UI thread. Besides the existing backdrop frame
/// event, this carries icon-worker results and refresh-watcher diffs.
#[derive(Debug)]
pub(crate) enum UserEvent {
    /// A new Windows.Graphics.Capture frame is ready to composite.
    BackdropFrameArrived,
    /// Generic wakeup from a background thread: "drain the shared inbox". We
    /// use one sentinel variant instead of one per message type so the worker
    /// and watcher can share a single inbox without per-variant allocations.
    InboxWakeup,
    /// Background worker finished extracting one icon.
    IconLoaded { app_id: AppId, image: DecodedIcon },
    /// Background worker failed to extract one icon.
    IconFailed { app_id: AppId, error: String },
    /// Refresh watcher produced a non-empty Start Menu diff.
    AppListDiff(AppDiff),
}

/// Shared inbox the worker + watcher push into; the event loop drains it on
/// each wakeup via a `UserEvent` sentinel that just means "poll the inbox".
/// Using one mpsc + a single winit user event keeps proxy wiring simple even
/// though we have two background threads.
type Inbox = Mutex<Vec<WorkerMessage>>;

#[derive(Debug)]
enum WorkerMessage {
    Icon(IconResult),
    Refresh(RefreshMessage),
}

/// Owns the renderer (which owns the window) plus all app/icon state.
struct App {
    event_proxy: EventLoopProxy<UserEvent>,
    renderer: Option<Renderer>,
    scroller: Option<Scroller>,
    text: Option<text::TextRenderer>,
    layout: grid::GridLayout,
    timer: StartupTimer,

    // ---- app + icon state ----
    registry: AppRegistry,
    /// CPU-side fixed-slot atlas; the GPU texture mirrors it.
    atlas: IconAtlas,
    /// True once the atlas texture has been allocated+uploaded at least once.
    atlas_uploaded: bool,
    /// Most recent Start Menu snapshot (for diffing on the UI thread when an
    /// inline scan is needed; the watcher also keeps its own).
    snapshot: BTreeMap<AppId, SnapshotEntry>,

    // ---- background plumbing ----
    cache: Arc<IconCache>,
    inbox: Arc<Inbox>,
    /// Kept to keep the worker thread alive; requests go through it.
    _worker: Option<WorkerHandle>,

    // ---- input ----
    scale_factor: f32,
    pointer_phys_x: f32,
    pointer_phys_y: f32,
    drag_start_x: f32,
    drag_start_y: f32,
    first_frame_rendered: bool,
}

impl App {
    fn new(
        event_proxy: EventLoopProxy<UserEvent>,
        timer: StartupTimer,
        cache: Arc<IconCache>,
        inbox: Arc<Inbox>,
        worker: WorkerHandle,
    ) -> Self {
        Self {
            event_proxy,
            renderer: None,
            scroller: None,
            text: None,
            layout: grid::GridLayout::default(),
            timer,
            registry: AppRegistry::new(),
            atlas: IconAtlas::new(64),
            atlas_uploaded: false,
            snapshot: BTreeMap::new(),
            cache,
            inbox,
            _worker: Some(worker),
            scale_factor: 1.0,
            pointer_phys_x: 0.0,
            pointer_phys_y: 0.0,
            drag_start_x: 0.0,
            drag_start_y: 0.0,
            first_frame_rendered: false,
        }
    }

    fn viewport_phys(&self) -> (u32, u32) {
        self.renderer
            .as_ref()
            .map(|r| {
                let s = r.window.inner_size();
                (s.width, s.height)
            })
            .unwrap_or((1280, 800))
    }

    /// Build an owned snapshot of the current registry in display order,
    /// padded out to the full grid so empty tiles render as placeholders.
    /// Returns owned data so it doesn't hold a borrow on `self` while the
    /// renderer mutates.
    fn grid_apps_owned(&self) -> Vec<(String, Option<icons::UvRect>)> {
        let total = self.layout.total_tiles();
        let mut out: Vec<(String, Option<icons::UvRect>)> = Vec::with_capacity(total);
        for rec in self.registry.apps() {
            out.push((rec.name.clone(), rec.uv));
        }
        while out.len() < total {
            out.push((String::new(), None));
        }
        out
    }

    /// Recompute layout/bounds for the current window size and push tile +
    /// label + icon instance buffers to the GPU.
    fn relayout(&mut self) {
        let (w, _h) = self.viewport_phys();
        self.layout = grid::GridLayout::default().centered(w as f32);
        let bounds = self.layout.bounds(w as f32);
        if let Some(s) = self.scroller.as_mut() {
            s.set_bounds(bounds);
        }

        let owned = self.grid_apps_owned();
        let apps: Vec<grid::GridApp<'_>> = owned
            .iter()
            .map(|(name, uv)| grid::GridApp {
                name: name.as_str(),
                uv: *uv,
            })
            .collect();

        // Text labels.
        let scale = self.scale_factor;
        let dirty = if let Some(t) = self.text.as_mut() {
            let labels = self.layout.build_labels(w as f32, &apps);
            let quads = t.layout_labels(&labels, scale);
            let dirty = t.atlas_dirty;
            if let Some(r) = self.renderer.as_mut() {
                r.set_text_instances(&quads);
                if dirty {
                    r.upload_atlas(t.atlas_rgba());
                }
            }
            dirty
        } else {
            false
        };
        if dirty {
            if let Some(t) = self.text.as_mut() {
                t.atlas_dirty = false;
            }
        }

        if let Some(r) = self.renderer.as_mut() {
            r.rebuild_instances(&self.layout, &apps);
            let icon_instances = self.layout.build_icon_instances(w as f32, &apps);
            r.set_icon_instances(&icon_instances);
        }

        self.ensure_atlas_uploaded();
    }

    /// Upload (or grow) the GPU icon atlas to match the CPU atlas, then push
    /// the full pixel buffer once after the first allocation. Subsequent
    /// per-icon updates go through [`apply_icon`][Self::apply_icon].
    fn ensure_atlas_uploaded(&mut self) {
        let needed = self.registry.slot_count().max(1);
        let grew = self.atlas.ensure_capacity(needed);
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        let (gw, gh) = (self.atlas.width(), self.atlas.height());
        let (cur_w, cur_h) = r.icon_atlas_size();

        if !self.atlas_uploaded || grew || (cur_w, cur_h) != (gw, gh) {
            // (Re)allocate + full upload.
            r.upload_icon_atlas(self.atlas.rgba(), gw, gh);
            self.atlas_uploaded = true;
            self.timer
                .mark(prefix::STARTUP, "atlas + GPU texture upload");
        }
    }

    /// Apply one freshly-extracted (or cached) icon: write it into the slot,
    /// update the registry UV + state, push the cell to the GPU, and refresh
    /// the icon instance buffer so the new UV is picked up.
    fn apply_icon(&mut self, app_id: &AppId, image: DecodedIcon, from_cache: bool) {
        let Some(slot) = self.registry.get(app_id).map(|r| r.slot) else {
            return;
        };
        // `write_icon` may grow the CPU atlas if the slot is beyond the current
        // capacity. Detect that so we can re-upload the *whole* atlas to the
        // GPU (and reallocate the texture) instead of a partial cell write that
        // would overrun the old texture bounds. This is the Phase-5 grow path.
        let (gpu_w, gpu_h) = self
            .renderer
            .as_ref()
            .map(|r| r.icon_atlas_size())
            .unwrap_or((0, 0));
        let (x, y, uv) = self.atlas.write_icon(slot, &image);
        let atlas_grew = (self.atlas.width(), self.atlas.height()) != (gpu_w, gpu_h) || gpu_w == 0;
        if atlas_grew {
            // Full re-upload at the new dimensions.
            if let Some(r) = self.renderer.as_mut() {
                r.upload_icon_atlas(self.atlas.rgba(), self.atlas.width(), self.atlas.height());
            }
        } else if let Some(r) = self.renderer.as_ref() {
            // Same dimensions → cheap single-cell update.
            r.write_icon_cell(&image.rgba, x, y, image.w, image.h);
        }
        let state = if from_cache {
            IconState::Cached
        } else {
            IconState::Loaded
        };
        self.registry.update(app_id, |rec| {
            rec.uv = Some(uv);
            rec.icon_state = state;
        });
        // Refresh icon instances so the new UV is uploaded. Cheap (hundreds of
        // f32s) and keeps the draw call in sync with the atlas.
        let owned = self.grid_apps_owned();
        let apps: Vec<grid::GridApp<'_>> = owned
            .iter()
            .map(|(name, uv)| grid::GridApp {
                name: name.as_str(),
                uv: *uv,
            })
            .collect();
        let (w, _h) = self.viewport_phys();
        let icon_instances = self.layout.build_icon_instances(w as f32, &apps);
        if let Some(r) = self.renderer.as_mut() {
            r.set_icon_instances(&icon_instances);
        }
        self.request_redraw();
    }

    /// Mark an app's icon as failed (placeholder stays), logging the error.
    fn fail_icon(&mut self, app_id: &AppId, error: String) {
        eprintln!("icon-worker: failed app_id={app_id}: {error}");
        self.registry.update(app_id, |rec| {
            rec.icon_state = IconState::Failed;
        });
    }

    /// Populate the registry from a snapshot, applying cached icons where
    /// valid and queueing extraction requests for the rest. Called on the
    /// initial scan and on each refresh diff.
    fn ingest_snapshot(&mut self, new_snapshot: BTreeMap<AppId, SnapshotEntry>, is_initial: bool) {
        if is_initial {
            self.timer.mark_with(
                prefix::STARTUP,
                "app list enumeration",
                format!("({} apps)", new_snapshot.len()),
            );
        }
        self.snapshot = new_snapshot.clone();

        let mut requests: Vec<IconRequest> = Vec::new();
        let mut cached_applied = 0usize;

        // Insert every app from the snapshot (the registry dedupes; updates
        // happen via the diff path on subsequent scans).
        for (id, entry) in &new_snapshot {
            let exists = self.registry.get(id).is_some();
            if !exists {
                let slot = self.registry.alloc_slot();
                let rec = AppRecord {
                    app_id: id.clone(),
                    name: entry.name.clone(),
                    link_path: PathBuf::from(&entry.link_path),
                    resolved_target: PathBuf::from(&entry.target_path),
                    slot,
                    icon_state: IconState::Missing,
                    uv: None,
                };
                self.registry.insert(rec);
            }
        }

        // Now decide cache-vs-extract per app.
        for (id, entry) in &new_snapshot {
            let probe = CacheProbe {
                app_id: id,
                link_mtime: entry.link_mtime,
                target_path: &entry.target_path,
                target_mtime: entry.target_mtime,
                icon_location: &entry.icon_location,
                icon_index: entry.icon_index,
            };
            match self.cache.get_if_valid(&probe) {
                Ok(Some(cached)) => {
                    // Valid cache: apply immediately, mark Cached. Still queue
                    // nothing — but we *could* revalidate in the background.
                    // For now trust the cache fully (Phase 3 behavior).
                    let rec_state = self.registry.get(id).map(|r| r.icon_state);
                    if !matches!(rec_state, Some(IconState::Loaded | IconState::Cached)) {
                        self.apply_cached_icon(id, cached);
                        cached_applied += 1;
                    }
                }
                _ => {
                    // Miss or stale: queue extraction (unless already loading).
                    let already_loading = matches!(
                        self.registry.get(id).map(|r| r.icon_state),
                        Some(IconState::Loading) | Some(IconState::Loaded)
                    );
                    if !already_loading {
                        self.registry.update(id, |rec| {
                            rec.icon_state = IconState::Loading;
                        });
                        requests.push(IconRequest {
                            app_id: id.clone(),
                            name: entry.name.clone(),
                            link_path: PathBuf::from(&entry.link_path),
                            link_mtime: entry.link_mtime,
                            target_path: entry.target_path.clone(),
                            target_mtime: entry.target_mtime,
                            icon_location: entry.icon_location.clone(),
                            icon_index: entry.icon_index,
                            reason: if is_initial {
                                IconReason::Fresh
                            } else {
                                IconReason::Updated
                            },
                        });
                    }
                }
            }
        }

        if cached_applied > 0 {
            self.timer.mark_with(
                prefix::ICON_CACHE,
                "cached icon apply",
                format!("({cached_applied} icons)"),
            );
        }

        // Relayout once so new tiles + cached icons show up immediately.
        self.relayout();
        self.request_redraw();

        // Dispatch extraction requests to the worker.
        if !requests.is_empty() {
            self.timer.mark_with(
                prefix::ICON_WORKER,
                "queue extraction",
                format!("({} icons)", requests.len()),
            );
            if let Some(handle) = self._worker.as_ref() {
                for req in requests {
                    if handle.requests.send(req).is_err() {
                        eprintln!("icon-worker: request channel closed");
                        break;
                    }
                }
            }
        }
    }

    /// Apply a cache-served icon without going through the worker.
    fn apply_cached_icon(&mut self, app_id: &AppId, cached: CachedIcon) {
        self.apply_icon(app_id, cached.image, true);
    }

    /// Apply a refresh diff: add new apps, update changed ones (re-extracting
    /// icons whose cache key moved), and remove gone apps.
    fn apply_diff(&mut self, diff: AppDiff) {
        self.timer.mark_with(
            prefix::APP_REFRESH,
            "app list refresh",
            format!(
                "(added={} updated={} removed={})",
                diff.added.len(),
                diff.updated.len(),
                diff.removed.len()
            ),
        );

        // Removals.
        for id in &diff.removed {
            let slot = self.registry.get(id).map(|r| r.slot);
            self.registry.remove(id);
            if let Some(s) = slot {
                self.atlas.clear_slot(s);
                if let Some(r) = self.renderer.as_mut() {
                    r.upload_icon_atlas(self.atlas.rgba(), self.atlas.width(), self.atlas.height());
                }
            }
            let _ = self.cache.forget(id);
        }

        // Merge the added/updated entries into a mini-snapshot so we can reuse
        // the cache-probe + extract logic. We rebuild against the *current*
        // snapshot so probes reflect the latest fields.
        for entry in diff.added.iter().chain(diff.updated.iter()) {
            self.snapshot.insert(entry.app_id.clone(), entry.clone());
        }
        // Re-probe only the changed ids.
        let changed_ids: Vec<AppId> = diff
            .added
            .iter()
            .chain(diff.updated.iter())
            .map(|e| e.app_id.clone())
            .collect();

        // Ensure new apps exist in the registry with a slot.
        for entry in diff.added.iter().chain(diff.updated.iter()) {
            if self.registry.get(&entry.app_id).is_none() {
                let slot = self.registry.alloc_slot();
                self.registry.insert(AppRecord {
                    app_id: entry.app_id.clone(),
                    name: entry.name.clone(),
                    link_path: PathBuf::from(&entry.link_path),
                    resolved_target: PathBuf::from(&entry.target_path),
                    slot,
                    icon_state: IconState::Missing,
                    uv: None,
                });
            } else {
                // Existing app: update mutable fields.
                self.registry.update(&entry.app_id, |rec| {
                    rec.name = entry.name.clone();
                    rec.link_path = PathBuf::from(&entry.link_path);
                    rec.resolved_target = PathBuf::from(&entry.target_path);
                });
            }
        }

        // Re-extract (or serve from cache) the changed icons.
        let mut requests = Vec::new();
        for id in &changed_ids {
            let Some(entry) = self.snapshot.get(id) else {
                continue;
            };
            let probe = CacheProbe {
                app_id: id,
                link_mtime: entry.link_mtime,
                target_path: &entry.target_path,
                target_mtime: entry.target_mtime,
                icon_location: &entry.icon_location,
                icon_index: entry.icon_index,
            };
            match self.cache.get_if_valid(&probe) {
                Ok(Some(cached)) => {
                    self.apply_cached_icon(id, cached);
                }
                _ => {
                    self.registry.update(id, |rec| {
                        rec.icon_state = IconState::Loading;
                    });
                    requests.push(IconRequest {
                        app_id: id.clone(),
                        name: entry.name.clone(),
                        link_path: PathBuf::from(&entry.link_path),
                        link_mtime: entry.link_mtime,
                        target_path: entry.target_path.clone(),
                        target_mtime: entry.target_mtime,
                        icon_location: entry.icon_location.clone(),
                        icon_index: entry.icon_index,
                        reason: IconReason::Updated,
                    });
                }
            }
        }

        self.relayout();
        self.request_redraw();

        if let Some(handle) = self._worker.as_ref() {
            for req in requests {
                let _ = handle.requests.send(req);
            }
        }

        // GC cache rows we no longer reference.
        let present: Vec<AppId> = self.snapshot.keys().cloned().collect();
        if let Err(e) = self.cache.retain_and_touch(&present) {
            eprintln!("icon-cache: retain_and_touch failed: {e}");
        }
    }

    /// Drain the shared inbox and dispatch each message.
    fn drain_inbox(&mut self) {
        let messages: Vec<WorkerMessage> = {
            let mut g = self.inbox.lock().expect("inbox poisoned");
            std::mem::take(&mut *g)
        };
        for msg in messages {
            match msg {
                WorkerMessage::Icon(IconResult::Loaded { app_id, image }) => {
                    self.apply_icon(&app_id, image, false);
                }
                WorkerMessage::Icon(IconResult::Failed { app_id, error }) => {
                    self.fail_icon(&app_id, error);
                }
                WorkerMessage::Refresh(RefreshMessage::Initial(snap)) => {
                    self.ingest_snapshot(snap, true);
                }
                WorkerMessage::Refresh(RefreshMessage::Diff(diff)) => {
                    self.apply_diff(diff);
                }
            }
        }
    }

    fn handle_drag_start(&mut self, x_phys: f32, y_phys: f32) {
        self.drag_start_x = x_phys;
        self.drag_start_y = y_phys;
        if let Some(s) = self.scroller.as_mut() {
            s.drag_start(x_phys);
        }
        self.request_redraw();
    }

    fn handle_drag_move(&mut self, x_phys: f32) {
        if let Some(s) = self.scroller.as_mut() {
            s.drag_move(x_phys);
        }
        self.request_redraw();
    }

    fn handle_drag_end(&mut self) {
        if let Some(s) = self.scroller.as_mut() {
            s.drag_end();
        }
        self.request_redraw();
    }

    fn handle_pointer_release(&mut self) -> Option<AppLaunchInfo> {
        let x = self.pointer_phys_x;
        let y = self.pointer_phys_y;
        let dx = x - self.drag_start_x;
        let dy = y - self.drag_start_y;
        let is_click = dx * dx + dy * dy <= CLICK_SLOP_PHYS * CLICK_SLOP_PHYS;

        let launch = is_click.then(|| self.resolve_clicked_app(x, y)).flatten();
        self.handle_drag_end();
        launch
    }

    /// Hit-test the pointer, then resolve the clicked app **by stable id**
    /// (not positional index), so a rescan that shifted the list can't launch
    /// the wrong app. Returns an owned snapshot safe to use after the
    /// launcher dismisses.
    fn resolve_clicked_app(&self, x_phys: f32, y_phys: f32) -> Option<AppLaunchInfo> {
        let (w, _h) = self.viewport_phys();
        let scroll_x = self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        let app_index =
            self.layout
                .hit_test_app(w as f32, x_phys, y_phys, scroll_x, self.registry.len())?;
        // Map display index → stable id → launch snapshot. Going through the id
        // means even a concurrent mutation between pick and launch can't
        // resolve to the wrong app.
        let app_id = self.registry.apps().get(app_index)?.app_id.clone();
        self.registry.launch_info(&app_id)
    }

    fn request_redraw(&self) {
        if let Some(r) = self.renderer.as_ref() {
            r.window.request_redraw();
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::BackdropFrameArrived => {
                self.request_redraw();
            }
            // All worker/watcher traffic arrives via the shared inbox; these
            // events are just wakeups. Drain here on the UI thread.
            UserEvent::InboxWakeup
            | UserEvent::IconLoaded { .. }
            | UserEvent::IconFailed { .. }
            | UserEvent::AppListDiff(_) => {
                self.drain_inbox();
            }
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        self.timer.mark(prefix::STARTUP, "window creation");
        let attrs = Window::default_attributes()
            .with_title("Launchpad")
            .with_transparent(true)
            // Drop the classic HWND back buffer (WS_EX_NOREDIRECTIONBITMAP) so
            // the DWM composites only our DirectComposition swap chain. Without
            // this, alpha=0 pixels are filled with the window's white
            // background brush and transparency reads as solid white.
            .with_no_redirection_bitmap(true)
            // Borderless: the glass tiles own the visuals, so we drop the OS
            // title bar / frame. Closing via Esc/Alt-F4.
            .with_decorations(false)
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(winit::dpi::LogicalSize::new(640.0, 480.0));

        let window = event_loop.create_window(attrs).expect("create window");
        #[cfg(windows)]
        {
            if std::env::var_os("LAUNCHPAD_ALLOW_SCREENSHOT").is_some() {
                eprintln!("capture exclusion skipped: LAUNCHPAD_ALLOW_SCREENSHOT is set");
            } else {
                let exclusion = liquid_glass::windows_capture::exclude_window_from_capture(&window);
                if exclusion.attempted && !exclusion.success {
                    eprintln!("capture exclusion failed: {}", exclusion.message);
                } else if exclusion.attempted {
                    eprintln!("capture exclusion: {}", exclusion.message);
                }
            }
        }
        self.scale_factor = window.scale_factor() as f32;
        let (w, _h) = (window.inner_size().width, window.inner_size().height);
        self.layout = grid::GridLayout::default().centered(w as f32);

        let renderer = pollster::block_on(Renderer::new(
            window,
            &self.layout,
            self.event_proxy.clone(),
        ))
        .expect("init renderer");
        self.timer.mark(prefix::STARTUP, "renderer initialization");
        let bounds = self.layout.bounds(w as f32);
        let scroller = Scroller::new(bounds);
        let text = text::TextRenderer::new();

        self.renderer = Some(renderer);
        self.scroller = Some(scroller);
        self.text = Some(text);

        // First paint: empty/loading state, NO icon extraction. This is the
        // core Phase-1 win — the window is visible before any Shell/GDI work.
        self.relayout();
        self.request_redraw();
        self.timer.mark(prefix::STARTUP, "first redraw requested");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                let winit::keyboard::PhysicalKey::Code(key_code) = event.physical_key else {
                    return;
                };

                if key_code == winit::keyboard::KeyCode::Escape {
                    event_loop.exit();
                    return;
                }

                // M toggles the OS window frame on/off for easier debugging
                // (grab edges to resize, title bar to move) without rebuilding.
                if key_code == winit::keyboard::KeyCode::KeyM {
                    if let Some(r) = self.renderer.as_mut() {
                        r.toggle_decorations();
                        self.request_redraw();
                    }
                    return;
                }

                if let Some(r) = self.renderer.as_mut() {
                    if r.handle_liquid_glass_key(key_code) {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::Resized(new_size) => {
                if new_size.width == 0 || new_size.height == 0 {
                    return;
                }
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(new_size.width, new_size.height);
                }
                self.relayout();
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_factor = scale_factor as f32;
                self.relayout();
                self.request_redraw();
            }
            WindowEvent::Moved(_) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.notify_window_moved();
                }
                self.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                // If the button is still down we treat leaving as a release.
                let dragging = self
                    .scroller
                    .as_ref()
                    .map(|s| s.phase == Phase::Dragging)
                    .unwrap_or(false);
                if dragging {
                    self.handle_drag_end();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_phys_x = position.x as f32;
                self.pointer_phys_y = position.y as f32;
                let dragging = self
                    .scroller
                    .as_ref()
                    .map(|s| s.phase == Phase::Dragging)
                    .unwrap_or(false);
                if dragging {
                    self.handle_drag_move(position.x as f32);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button != MouseButton::Left {
                    return;
                }
                match state {
                    ElementState::Pressed => {
                        self.handle_drag_start(self.pointer_phys_x, self.pointer_phys_y);
                    }
                    ElementState::Released => {
                        if let Some(app) = self.handle_pointer_release() {
                            // Dismiss first, launch second. `set_visible(false)`
                            // hands the hide straight to the DWM (a few ms),
                            // while `ShellExecuteW` resolves the shortcut and
                            // spawns the target (tens to hundreds of ms). Doing
                            // them in this order makes the launcher feel like it
                            // vanishes the instant you click, instead of freezing
                            // on screen until the target app starts.
                            if let Some(r) = self.renderer.as_ref() {
                                r.window.set_visible(false);
                            }
                            event_loop.exit();
                            match launch::open_shortcut(&app.link_path) {
                                Ok(()) => eprintln!("launched {}", app.name),
                                Err(err) => eprintln!(
                                    "failed to launch {} ({}): {}",
                                    app.name,
                                    app.link_path.display(),
                                    err
                                ),
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let vp = self.viewport_phys();
                let animating;
                if let (Some(r), Some(s)) = (self.renderer.as_mut(), self.scroller.as_mut()) {
                    let dragging = s.phase == Phase::Dragging;
                    s.tick(now);
                    r.render(&DrawArgs {
                        scroll_x: s.position,
                        viewport: vp,
                        defer_backdrop_capture: dragging,
                    });
                    animating = s.is_animating();
                } else {
                    return;
                }
                if !self.first_frame_rendered {
                    self.first_frame_rendered = true;
                    self.timer.mark(prefix::STARTUP, "first frame rendered");
                }
                if animating {
                    self.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Keep the loop pumping while animating; otherwise winit blocks until
        // the next input or WGC FrameArrived user event.
        let animating = self
            .scroller
            .as_ref()
            .map(|s| s.is_animating())
            .unwrap_or(false);
        if animating {
            self.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::Wait);
    }
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

fn main() {
    let timer = StartupTimer::new();
    timer.mark(prefix::STARTUP, "process start");
    startup_timer::install(timer.clone());

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

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
    let worker = icon_worker::spawn(cache.clone(), result_tx);
    spawn_bridge(result_rx, merged_tx.clone(), WorkerMessage::Icon);

    // Spawn the Start Menu refresh watcher, bridging it the same way.
    let (refresh_tx, refresh_rx): (
        std::sync::mpsc::Sender<RefreshMessage>,
        std::sync::mpsc::Receiver<RefreshMessage>,
    ) = std::sync::mpsc::channel();
    refresh_watcher::spawn(refresh_tx, RefreshConfig::default());
    spawn_bridge(refresh_rx, merged_tx, WorkerMessage::Refresh);

    // Single forwarder for the merged channel into the shared inbox.
    forward_inbox(merged_rx, inbox.clone(), proxy.clone());

    let mut app = App::new(proxy, timer, cache, inbox, worker);
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
