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
mod bottom_control;
mod grid;
mod icon_atlas;
mod icon_cache;
mod icon_pipeline;
mod icon_worker;
mod icons;
mod launch;
mod liquid_glass;
#[cfg(windows)]
mod platform_windows;
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
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Window, WindowId};

/// Cell edge (icon + padding) imported from the atlas module for readability.
const CELL: u32 = icon_atlas::CELL;
const CLICK_SLOP_PHYS: f32 = 8.0;
const INITIAL_WINDOW_WIDTH: f64 = 1280.0;
const INITIAL_WINDOW_HEIGHT: f64 = 800.0;
const MIN_WINDOW_WIDTH: f64 = 640.0;
const MIN_WINDOW_HEIGHT: f64 = 480.0;

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
    /// Summon the launcher window (global hot key / tray "Show").
    Summon,
    /// User asked to really quit (tray "Quit"). Ends the event loop.
    QuitRequested,
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

    // ---- bottom-center morphing control (search pill / page indicator /
    // search field) ----
    control: bottom_control::BottomControl,
    /// Last settled page index, used to detect page changes for the indicator.
    last_page: i32,
    /// Whether the pointer is currently over the control capsule (hover),
    /// for hit-testing click vs. background.
    pointer_over_control: bool,
    /// True while the left button is held down *and* the press started on the
    /// control capsule. Such a release is a control click, not an app launch.
    pressed_on_control: bool,
    /// Timestamp of the last redraw, used to compute a real dt for the control
    /// animations (caret blink + morphs).
    last_redraw: Option<Instant>,

    // ---- resident-lifecycle state ----
    /// Whether the window is currently visible. `set_visible` doesn't query,
    /// so we track it ourselves to make `hide()` idempotent (avoids a hide
    /// storm when a focus-loss event races an app-launch hide).
    visible: bool,
    /// Set by `UserEvent::QuitRequested` (tray "Quit"); checked in
    /// `about_to_wait` to actually exit the loop. Decoupling the request from
    /// the exit lets the loop drain the current frame cleanly.
    should_quit: bool,
    /// Anchor keeping the OS-integration thread (hot key + tray) alive for
    /// the whole process. Underscore-prefixed because we never read it.
    #[cfg(windows)]
    _os: Option<platform_windows::OsIntegrationHandle>,
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
            control: bottom_control::BottomControl::new(),
            last_page: 0,
            pointer_over_control: false,
            pressed_on_control: false,
            last_redraw: None,
            visible: true,
            should_quit: false,
            #[cfg(windows)]
            _os: None,
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

    /// Current 0-based page index from the scroller position.
    fn current_page(&self) -> usize {
        let (w, _h) = self.viewport_phys();
        let s = match self.scroller.as_ref() {
            Some(s) => s,
            None => return 0,
        };
        if w == 0 {
            return 0;
        }
        // position is the content offset; page = round(-position / page_extent).
        let p = (-s.position / w as f32).round() as i32;
        p.clamp(0, self.layout.page_count.saturating_sub(1) as i32) as usize
    }

    /// The Y coordinate of the bottom edge of the fixed page frame, in
    /// physical px. The bottom control sits a fixed margin below this.
    fn frame_bottom_y(&self) -> f32 {
        let (w, _h) = self.viewport_phys();
        let (_cx, cy, _pw, panel_h) = self.layout.frame_panel_rect(w.max(1) as f32);
        cy + panel_h * 0.5
    }

    /// Resolve the control's frame geometry + layers for the current state.
    fn resolve_control(
        &self,
    ) -> Option<(
        bottom_control::ControlGeometry,
        Vec<bottom_control::ControlLayer>,
    )> {
        let viewport = self.viewport_phys();
        let frame_bottom = self.frame_bottom_y();
        let page = self.current_page();
        let page_count = self.layout.page_count;
        Some(
            self.control
                .resolve(viewport, frame_bottom, page, page_count),
        )
    }

    /// Lay out and upload the bottom control's glass capsule + overlay shapes
    /// and text for the current frame. Call this once per redraw, after the
    /// control has been ticked.
    fn render_bottom_control(&mut self) {
        let (geom, layers) = match self.resolve_control() {
            Some(v) => v,
            None => return,
        };

        // Gather all the immutable data first (avoid overlapping borrows with
        // the mutable renderer/text borrows below).
        let scale = self.scale_factor;
        let query_width = self.measure_query_width();
        let caret_blink = caret_visibility(&self.control);

        // 1) Procedural overlay instances (magnifier, dots, caret, close).
        let instances =
            bottom_control::build_overlay_instances(&geom, &layers, query_width, caret_blink);

        // 2) Text glyphs (label / query / placeholder). Built via the shared
        // text renderer so they share the glyph atlas. Done before touching the
        // renderer so the atlas upload + dirty clear happen in one place.
        let (quads, atlas_dirty) = if let Some(t) = self.text.as_mut() {
            let q = self_layout_control_text(t, &geom, &layers, scale, &self.control);
            (q, t.atlas_dirty)
        } else {
            (Vec::new(), false)
        };
        if atlas_dirty {
            if let Some(t) = self.text.as_mut() {
                t.atlas_dirty = false;
            }
        }

        // 3) Push everything to the GPU.
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        let shape = bottom_control::glass_shape(&geom);
        r.set_control_glass_shape(shape);
        if atlas_dirty {
            if let Some(t) = self.text.as_ref() {
                r.upload_atlas(t.atlas_rgba());
            }
        }
        r.set_control_instances(&instances);
        r.set_control_text_instances(&quads);
    }

    /// Measure the current query's laid-out width in physical px (for caret
    /// placement). Falls back to 0 when there's no query.
    fn measure_query_width(&self) -> f32 {
        if self.control.query.is_empty() {
            return 0.0;
        }
        // Approximate via the text renderer's layout without committing to the
        // atlas: lay out a centered line at origin 0 and take the max right
        // edge. We can't borrow mutably here cheaply, so estimate from glyph
        // count instead. A precise measure would require a &mut TextRenderer;
        // the caret drift of a few px is acceptable for the MVP.
        // TODO: exact width once TextRenderer exposes a measure API.
        let chars = self.control.query.chars().count() as f32;
        chars * 9.0 * self.scale_factor
    }

    /// Build an owned snapshot of the current registry in display order.
    /// Returns owned data so it doesn't hold a borrow on `self` while the
    /// renderer mutates.
    fn grid_apps_owned(&self) -> Vec<(String, Option<icons::UvRect>)> {
        self.registry
            .apps()
            .iter()
            .map(|rec| (rec.name.clone(), rec.uv))
            .collect()
    }

    /// Recompute layout/bounds for the current window size and push tile +
    /// label + icon instance buffers to the GPU.
    fn relayout(&mut self) {
        let (w, _h) = self.viewport_phys();
        // Size pages to the current app count so every app is reachable by
        // scrolling (the grid grows pages as apps are added).
        self.layout = grid::GridLayout::for_app_count(self.registry.len()).centered(w as f32);
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
        // capacity. A grow recomputes *every* slot's UV (the cell grid gains
        // columns), so any app whose UV we cached before the grow now points at
        // the wrong cell → overlapping icons. When that happens we must
        // re-sync every app's UV from the new atlas before redrawing.
        let (gpu_w, gpu_h) = self
            .renderer
            .as_ref()
            .map(|r| r.icon_atlas_size())
            .unwrap_or((0, 0));
        let (x, y, this_uv) = self.atlas.write_icon(slot, &image);
        let atlas_grew = (self.atlas.width(), self.atlas.height()) != (gpu_w, gpu_h) || gpu_w == 0;

        if atlas_grew {
            // Full re-upload at the new dimensions, then re-sync every app's
            // UV from the freshly-grown atlas so none sample a stale cell.
            if let Some(r) = self.renderer.as_mut() {
                r.upload_icon_atlas(self.atlas.rgba(), self.atlas.width(), self.atlas.height());
            }
            self.resync_registry_uvs();
        } else if let Some(r) = self.renderer.as_ref() {
            // Same dimensions → cheap single-cell update.
            r.write_icon_cell(&image.rgba, x, y, image.w, image.h);
        }
        // Optional diagnostic: dump the atlas after every icon apply so we can
        // inspect the final packed layout for overlapping cells. Enabled by
        // env var. We always overwrite the same file → final state wins.
        if std::env::var_os("LAUNCHPAD_DUMP_ATLAS").is_some() {
            dump_atlas_png(&self.atlas);
        }

        let state = if from_cache {
            IconState::Cached
        } else {
            IconState::Loaded
        };
        // Use the post-grow UV (resync already updated the registry; for the
        // non-grow path this is just the UV write_icon returned).
        let new_uv = self.atlas.uv(slot).unwrap_or(this_uv);
        self.registry.update(app_id, |rec| {
            rec.uv = Some(new_uv);
            rec.icon_state = state;
        });
        // Refresh icon instances so the new UVs are uploaded. Cheap (hundreds
        // of f32s) and keeps the draw call in sync with the atlas.
        self.rebuild_icon_instances();
        self.request_redraw();
    }

    /// After an atlas grow, rewrite every app's cached UV from the new atlas
    /// layout. Without this, apps loaded before the grow keep stale UVs and
    /// sample the wrong cell, producing overlapping/garbled icons.
    fn resync_registry_uvs(&mut self) {
        // Collect (id, slot) first to avoid borrowing self.registry twice.
        let entries: Vec<(AppId, u32)> = self
            .registry
            .apps()
            .iter()
            .map(|r| (r.app_id.clone(), r.slot))
            .collect();
        for (id, slot) in entries {
            if let Some(uv) = self.atlas.uv(slot) {
                self.registry.update(&id, |rec| {
                    rec.uv = Some(uv);
                });
            }
        }
    }

    /// Rebuild the icon-instance buffer from the current registry + layout and
    /// push it to the GPU. Called whenever any app's UV may have changed.
    fn rebuild_icon_instances(&mut self) {
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
    }

    /// Mark an app's icon as failed (placeholder stays), logging the error.
    fn fail_icon(&mut self, app_id: &AppId, error: String) {
        eprintln!("icon-worker: failed app_id={app_id}: {error}");
        self.registry.update(app_id, |rec| {
            rec.icon_state = IconState::Failed;
        });
    }

    /// Manually reset the icon cache + re-extract every icon live (R key).
    ///
    /// Wipes the on-disk SQLite cache, clears every atlas cell back to
    /// transparent, resets each app's icon state to `Missing`, then re-queues
    /// the whole app set against the worker — so the visible page refills
    /// progressively without restarting the launcher. The current snapshot
    /// (names/targets/mtimes) is reused, so no Start Menu rescan is needed.
    fn reset_icons(&mut self) {
        eprintln!("icon-cache: manual reset requested — clearing cache + re-extracting");

        // (1) Wipe the on-disk cache so the next probe is always a miss.
        match self.cache.clear_all() {
            Ok(n) => eprintln!("icon-cache: cleared {n} cached rows"),
            Err(e) => eprintln!("icon-cache: clear_all failed: {e}"),
        }

        // (2) Clear every atlas slot and push the blanked atlas to the GPU so
        // stale icons vanish immediately.
        let max_slot = self.registry.max_slot();
        for slot in 0..=max_slot {
            self.atlas.clear_slot(slot);
        }
        if let Some(r) = self.renderer.as_mut() {
            r.upload_icon_atlas(self.atlas.rgba(), self.atlas.width(), self.atlas.height());
        }

        // (3) Reset every app's icon state + drop its UV so placeholders show.
        let ids: Vec<AppId> = self
            .registry
            .apps()
            .iter()
            .map(|r| r.app_id.clone())
            .collect();
        for id in &ids {
            self.registry.update(id, |rec| {
                rec.uv = None;
                rec.icon_state = IconState::Missing;
            });
        }
        self.rebuild_icon_instances();
        self.request_redraw();

        // (4) Re-queue extraction requests in display order (first page first)
        // using the cached snapshot, so the worker re-extracts everything.
        if let Some(handle) = self._worker.as_ref() {
            let mut queued = 0usize;
            for id in &ids {
                let Some(entry) = self.snapshot.get(id) else {
                    continue;
                };
                self.registry.update(id, |rec| {
                    rec.icon_state = IconState::Loading;
                });
                let req = IconRequest {
                    app_id: id.clone(),
                    name: entry.name.clone(),
                    link_path: PathBuf::from(&entry.link_path),
                    link_mtime: entry.link_mtime,
                    target_path: entry.target_path.clone(),
                    target_mtime: entry.target_mtime,
                    icon_location: entry.icon_location.clone(),
                    icon_index: entry.icon_index,
                    reason: IconReason::Fresh,
                };
                if handle.requests.send(req).is_err() {
                    eprintln!("icon-worker: request channel closed during reset");
                    break;
                }
                queued += 1;
            }
            eprintln!("icon-cache: re-queued {queued} icons for extraction");
        }
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

        // Display order = sorted by display name (matches the registry's sort).
        // Process in this order so cache hits for the FIRST PAGE land first —
        // the user sees the visible page's icons populate before off-screen
        // pages. This is the cache-read prioritization: we don't load all icons
        // at once in arbitrary map order; the visible page wins.
        let per_page = self.layout.cols * self.layout.rows;
        let mut display_order: Vec<&AppId> = new_snapshot.keys().collect();
        display_order.sort_by(|a, b| {
            let na = new_snapshot[*a].name.to_lowercase();
            let nb = new_snapshot[*b].name.to_lowercase();
            na.cmp(&nb)
        });

        // Pass 1: apply cached icons for the first page first.
        for id in display_order.iter().take(per_page) {
            cached_applied += self.apply_cache_if_available(id, &new_snapshot[id]);
        }
        // Pass 2: apply cached icons for the remaining pages.
        for id in display_order.iter().skip(per_page) {
            cached_applied += self.apply_cache_if_available(id, &new_snapshot[id]);
        }

        // Pass 3: queue extraction requests in display order (first-page misses
        // are extracted before off-screen misses, so the visible page fills in
        // first).
        for id in display_order {
            let entry = &new_snapshot[id];
            let already_loading = matches!(
                self.registry.get(id).map(|r| r.icon_state),
                Some(IconState::Loading | IconState::Loaded | IconState::Cached)
            );
            if already_loading {
                continue;
            }
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

        // Dispatch extraction requests to the worker (first page already
        // first, thanks to the display-order sort above).
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

    /// Apply a cached icon for `id` if the cache holds a valid entry and the
    /// app isn't already loaded. Returns 1 if applied, 0 otherwise.
    fn apply_cache_if_available(&mut self, id: &AppId, entry: &SnapshotEntry) -> usize {
        let probe = CacheProbe {
            app_id: id,
            link_mtime: entry.link_mtime,
            target_path: &entry.target_path,
            target_mtime: entry.target_mtime,
            icon_location: &entry.icon_location,
            icon_index: entry.icon_index,
        };
        let rec_state = self.registry.get(id).map(|r| r.icon_state);
        if matches!(rec_state, Some(IconState::Loaded | IconState::Cached)) {
            return 0;
        }
        match self.cache.get_if_valid(&probe) {
            Ok(Some(cached)) => {
                self.apply_cached_icon(id, cached);
                1
            }
            _ => 0,
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

    /// Hide the launcher window and reset transient UI state (search field,
    /// scroll position, IME), but keep the process + event loop alive so it
    /// can be summoned again. Idempotent: a no-op if already hidden.
    fn hide(&mut self) {
        if !self.visible {
            return;
        }
        if let Some(r) = self.renderer.as_ref() {
            r.window.set_visible(false);
            r.window.set_ime_allowed(false);
        }
        // Drop any in-progress search / IME composition so the next summon
        // starts clean.
        self.control.press_close();
        // Reset scroll to page 0 so the next appearance doesn't land mid-page.
        if let Some(s) = self.scroller.as_mut() {
            s.position = 0.0;
            s.velocity = 0.0;
            s.phase = Phase::Idle;
        }
        self.last_page = 0;
        self.visible = false;
        self.request_redraw();
    }

    /// Show the launcher window and steal focus. Counterpart to [`hide`].
    /// Re-centers on the primary monitor so a multi-monitor move doesn't
    /// strand the launcher on the wrong screen.
    fn summon(&mut self) {
        let Some(r) = self.renderer.as_ref() else {
            return;
        };
        r.window.set_visible(true);
        r.window.focus_window();
        self.visible = true;
        self.request_redraw();
    }

    /// Handle a click (press + release inside the capsule with no drag) on the
    /// bottom control. Decides whether it hit the close (×) button, and
    /// otherwise toggles the search field open/closed.
    fn handle_control_click(&mut self, x: f32, y: f32) {
        let viewport = self.viewport_phys();
        let frame_bottom = self.frame_bottom_y();
        // Close-button hit region (only meaningful when the field is open).
        let close_x = self.control.close_button_x(viewport, frame_bottom);
        let hit_close = close_x
            .map(|cx| (x - cx).abs() <= 12.0 && (y - self.frame_control_cy()).abs() <= 12.0)
            .unwrap_or(false);

        if hit_close {
            self.control.press_close();
        } else {
            match self.control.mode {
                bottom_control::Mode::Pill
                | bottom_control::Mode::Indicator
                | bottom_control::Mode::Collapsing => {
                    self.control.open_search();
                }
                bottom_control::Mode::Expanding | bottom_control::Mode::Field => {
                    // Clicking inside an open field does nothing (keep focus).
                    // A click outside the field's text area could move the
                    // caret; the MVP leaves the caret at the end.
                }
            }
        }
        self.request_redraw();
    }

    /// The center Y of the control capsule (for hit-testing the close button).
    fn frame_control_cy(&self) -> f32 {
        let (_cx, _cy, _w, panel_h) = self
            .layout
            .frame_panel_rect(self.viewport_phys().0.max(1) as f32);
        let (_, vh) = self.viewport_phys();
        let bottom = _cy + panel_h * 0.5;
        (bottom + 26.0 + 15.0)
            .min(vh as f32 - 15.0 - 8.0)
            .max(15.0 + 8.0)
    }

    /// Keep the OS IME in sync with the search field: enable it (and point the
    /// composition window at the caret) while the field is focused, disable it
    /// otherwise. Called every frame; `set_ime_allowed` is cheap.
    fn update_ime_state(&self) {
        let Some(r) = self.renderer.as_ref() else {
            return;
        };
        let want_ime = self.control.wants_keyboard();
        r.window.set_ime_allowed(want_ime);
        if want_ime {
            // Park the IME composition window at the caret so Japanese/IME
            // candidates appear right next to the typed text.
            let scale = self.scale_factor;
            let caret_x = self.control_caret_screen_x();
            let caret_y = self.frame_control_cy();
            r.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(caret_x as f64, caret_y as f64),
                winit::dpi::PhysicalSize::new(1.0, (16.0 * scale) as f64),
            );
        }
    }

    /// Screen-space X of the text caret inside the search field (physical px),
    /// used to anchor the IME composition window.
    fn control_caret_screen_x(&self) -> f32 {
        let Some((geom, _)) = self.resolve_control() else {
            return 0.0;
        };
        let origin = bottom_control::field_text_origin_x(&geom);
        origin + self.measure_query_width()
    }
}

fn initial_window_position(event_loop: &ActiveEventLoop) -> Option<PhysicalPosition<i32>> {
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
            UserEvent::Summon => {
                self.summon();
            }
            UserEvent::QuitRequested => {
                // Defer the actual exit to `about_to_wait` so the current
                // event drains cleanly.
                self.should_quit = true;
            }
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        self.timer.mark(prefix::STARTUP, "window creation");
        let mut attrs = Window::default_attributes()
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
            .with_inner_size(LogicalSize::new(
                INITIAL_WINDOW_WIDTH,
                INITIAL_WINDOW_HEIGHT,
            ))
            .with_min_inner_size(LogicalSize::new(MIN_WINDOW_WIDTH, MIN_WINDOW_HEIGHT));

        if let Some(position) = initial_window_position(event_loop) {
            attrs = attrs.with_position(position);
        }

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
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                // Borderless window has no close button in normal use, but
                // Alt+F4 still reaches here. Treat it as "hide" rather than
                // "quit" so the launcher stays resident; real quit is via the
                // tray menu.
                self.hide();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                let winit::keyboard::PhysicalKey::Code(key_code) = event.physical_key else {
                    return;
                };

                // While the search field has focus, the control eats most keys.
                if self.control.wants_keyboard() {
                    let handled = match key_code {
                        winit::keyboard::KeyCode::Escape => {
                            let c = self.control.handle_escape();
                            // If the field was open, Esc closes it instead of
                            // quitting; otherwise fall through to quit below.
                            if c {
                                self.request_redraw();
                                return;
                            }
                            false
                        }
                        winit::keyboard::KeyCode::Backspace => {
                            self.control.handle_backspace();
                            self.request_redraw();
                            true
                        }
                        winit::keyboard::KeyCode::ArrowLeft => {
                            self.control.handle_left();
                            self.request_redraw();
                            true
                        }
                        winit::keyboard::KeyCode::ArrowRight => {
                            self.control.handle_right();
                            self.request_redraw();
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        return;
                    }
                    // Otherwise, let printable text through (typed below).
                    // Direct (non-IME) printable characters arrive in event.text.
                    if let Some(text) = &event.text {
                        if self.control.wants_keyboard() {
                            let mut any = false;
                            for ch in text.chars() {
                                if self.control.handle_char(ch) {
                                    any = true;
                                }
                            }
                            if any {
                                self.request_redraw();
                                return;
                            }
                        }
                    }
                }

                if key_code == winit::keyboard::KeyCode::Escape {
                    // Esc with no open field: hide the launcher (stay resident).
                    self.hide();
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

                // R clears the icon cache and re-extracts every icon live, so
                // you can recover from a corrupted cache without restarting.
                if key_code == winit::keyboard::KeyCode::KeyR && !self.control.wants_keyboard() {
                    self.reset_icons();
                    return;
                }

                if let Some(r) = self.renderer.as_mut() {
                    if r.handle_liquid_glass_key(key_code) {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::Ime(event) => {
                use winit::event::Ime;
                if self.control.wants_keyboard() {
                    match event {
                        Ime::Preedit(s, _) => {
                            // Show the in-flight composition inline.
                            self.control.set_preedit(s);
                            self.request_redraw();
                        }
                        Ime::Commit(text) => {
                            // IME commit: finalize the composition into the query.
                            self.control.set_preedit(String::new());
                            for ch in text.chars() {
                                self.control.handle_char(ch);
                            }
                            self.request_redraw();
                        }
                        Ime::Enabled => {}
                        Ime::Disabled => {
                            self.control.set_preedit(String::new());
                        }
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
                // Drop a pending control press if the pointer leaves.
                self.pressed_on_control = false;
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
                        // If the press starts on the control capsule, mark it
                        // so the release is treated as a control click and NOT
                        // as a scroll drag.
                        let over_control = self.control.hit_test(
                            self.viewport_phys(),
                            self.frame_bottom_y(),
                            self.pointer_phys_x,
                            self.pointer_phys_y,
                        );
                        self.pressed_on_control = over_control;
                        if over_control {
                            return;
                        }
                        self.handle_drag_start(self.pointer_phys_x, self.pointer_phys_y);
                    }
                    ElementState::Released => {
                        if self.pressed_on_control {
                            self.pressed_on_control = false;
                            // Only count as a click if it stayed on the capsule.
                            if self.control.hit_test(
                                self.viewport_phys(),
                                self.frame_bottom_y(),
                                self.pointer_phys_x,
                                self.pointer_phys_y,
                            ) {
                                self.handle_control_click(self.pointer_phys_x, self.pointer_phys_y);
                            }
                            return;
                        }
                        if let Some(app) = self.handle_pointer_release() {
                            // Dismiss first, launch second. `hide()` hands the
                            // hide straight to the DWM (a few ms) and resets
                            // the UI, while `ShellExecuteW` resolves the
                            // shortcut and spawns the target (tens to hundreds
                            // of ms). Doing them in this order makes the
                            // launcher feel like it vanishes the instant you
                            // click, instead of freezing on screen until the
                            // target app starts.
                            let link_path = app.link_path.clone();
                            let name = app.name.clone();
                            self.hide();
                            // NOTE: no `event_loop.exit()` — we stay resident
                            // so the next hot key can summon us instantly.
                            match launch::open_shortcut(&link_path) {
                                Ok(()) => eprintln!("launched {}", name),
                                Err(err) => eprintln!(
                                    "failed to launch {} ({}): {}",
                                    name,
                                    link_path.display(),
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
                let scroll_x;
                let dragging;
                if let Some(s) = self.scroller.as_mut() {
                    dragging = s.phase == Phase::Dragging;
                    s.tick(now);
                    scroll_x = s.position;
                } else {
                    return;
                }
                let scroller_animating = self
                    .scroller
                    .as_ref()
                    .map(|s| s.is_animating())
                    .unwrap_or(false);

                // Detect a page change (settled page differs from the last
                // tracked one) and arm the transient page indicator.
                let page = self.current_page() as i32;
                if page != self.last_page && !scroller_animating {
                    self.last_page = page;
                    self.control.on_page_change(now);
                }

                // Advance the bottom-control's animations + timers. Use the
                // real elapsed dt (not a fixed 1/60) so the caret blink and
                // morph speeds are correct even when redraws fire faster than
                // 60 Hz (e.g. on backdrop-frame arrivals).
                let control_dt = match self.last_redraw {
                    Some(prev) => now.duration_since(prev).as_secs_f32().min(0.1),
                    None => 1.0 / 60.0,
                };
                self.last_redraw = Some(now);
                let control_animating = self.control.tick(now, control_dt);

                // Sync the OS IME with the search field (on while focused,
                // parked at the caret) so Japanese / other IME input works.
                self.update_ime_state();

                // Upload the control's capsule + overlays before the render.
                self.render_bottom_control();

                // Render the frame (consumes the uploaded buffers).
                if let Some(r) = self.renderer.as_mut() {
                    r.render(&DrawArgs {
                        scroll_x,
                        viewport: vp,
                        defer_backdrop_capture: dragging,
                    });
                }

                if !self.first_frame_rendered {
                    self.first_frame_rendered = true;
                    self.timer.mark(prefix::STARTUP, "first frame rendered");
                }
                if scroller_animating || control_animating {
                    self.request_redraw();
                }
            }
            WindowEvent::Focused(false) => {
                // Auto-hide when the launcher loses focus (clicking another
                // window, Alt-Tab, …). This is the macOS-Launchpad / Run-dialog
                // behavior. `hide()` is idempotent so the focus-loss that fires
                // right after we hide to launch an app is a harmless no-op.
                self.hide();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Real quit path: the tray "Quit" command set the flag; now that the
        // current event is fully handled we can terminate the loop.
        if self.should_quit {
            event_loop.exit();
            return;
        }

        // Keep the loop pumping while the scroller or the bottom control is
        // animating; otherwise winit blocks until the next input or WGC
        // FrameArrived user event.
        let scroller_animating = self
            .scroller
            .as_ref()
            .map(|s| s.is_animating())
            .unwrap_or(false);
        let control_animating = self.control.mode.is_morphing()
            || matches!(self.control.mode, bottom_control::Mode::Indicator)
            || matches!(self.control.mode, bottom_control::Mode::Field);
        if scroller_animating || control_animating {
            self.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

/// Scale a color's alpha by `a` (used to cross-fade control text layers).
fn mul_alpha(mut c: [f32; 4], a: f32) -> [f32; 4] {
    c[3] *= a.clamp(0.0, 1.0);
    c
}

/// Build the text glyph quads for the control's active layers. A free function
/// (not a method) so it can borrow `&mut TextRenderer` and `&BottomControl`
/// without colliding with the renderer borrow in `render_bottom_control`.
fn self_layout_control_text(
    t: &mut text::TextRenderer,
    geom: &bottom_control::ControlGeometry,
    layers: &[bottom_control::ControlLayer],
    scale: f32,
    control: &bottom_control::BottomControl,
) -> Vec<text::GlyphQuad> {
    let mut quads = Vec::new();
    const LABEL_FONT: &str = "Yu Gothic UI";
    const LABEL_SIZE: f32 = 13.0;
    const LABEL_LINE: f32 = 18.0;
    const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
    const PLACEHOLDER: [f32; 4] = [1.0, 1.0, 1.0, 0.45];
    /// Preedit (in-flight IME composition) is shown slightly dimmer to hint
    /// it isn't committed yet.
    const PREEDIT_INK: [f32; 4] = [0.85, 0.92, 1.0, 0.88];

    for layer in layers {
        let a = layer.alpha;
        if a <= 0.01 {
            continue;
        }
        match layer.visual {
            bottom_control::Visual::SearchPill => {
                // "検索" label to the right of the magnifier.
                let mag_size = 11.0;
                let mag_cx = geom.center.0 - geom.half_size.0 + mag_size + 8.0;
                let label_center_x = mag_cx + mag_size + 6.0 + 14.0;
                let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                    text: "検索",
                    font_size: LABEL_SIZE,
                    line_height: LABEL_LINE,
                    family: LABEL_FONT,
                    color: mul_alpha(INK, a),
                    center: (label_center_x, geom.center.1),
                    scale_factor: scale,
                });
                quads.append(&mut q);
            }
            bottom_control::Visual::PageIndicator => {
                // No text.
            }
            bottom_control::Visual::SearchField => {
                let origin_x = bottom_control::field_text_origin_x(geom);
                if control.query.is_empty() && control.preedit.is_empty() {
                    let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                        text: "検索",
                        font_size: LABEL_SIZE,
                        line_height: LABEL_LINE,
                        family: LABEL_FONT,
                        color: mul_alpha(PLACEHOLDER, a),
                        center: (origin_x + 14.0, geom.center.1),
                        scale_factor: scale,
                    });
                    quads.append(&mut q);
                } else {
                    // Render the committed query plus the in-flight IME
                    // preedit inline. The preedit is shown with an
                    // underline tint so the user can tell it's not yet
                    // committed.
                    let q_len = control.query.chars().count() as f32;
                    let p_len = control.preedit.chars().count() as f32;
                    // Committed text.
                    if !control.query.is_empty() {
                        let approx_half = q_len * 4.5;
                        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                            text: &control.query,
                            font_size: LABEL_SIZE,
                            line_height: LABEL_LINE,
                            family: LABEL_FONT,
                            color: mul_alpha(INK, a),
                            center: (origin_x + approx_half, geom.center.1),
                            scale_factor: scale,
                        });
                        quads.append(&mut q);
                    }
                    // Preedit, starting after the committed query.
                    if !control.preedit.is_empty() {
                        let preedit_origin = origin_x + q_len * 9.0;
                        let approx_half = p_len * 4.5;
                        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                            text: &control.preedit,
                            font_size: LABEL_SIZE,
                            line_height: LABEL_LINE,
                            family: LABEL_FONT,
                            color: mul_alpha(PREEDIT_INK, a),
                            center: (preedit_origin + approx_half, geom.center.1),
                            scale_factor: scale,
                        });
                        quads.append(&mut q);
                    }
                }
            }
        }
    }
    quads
}

/// Caret blink visibility for this frame. The caret shows ~57% of a ~1.06s
/// cycle, in sync with the control's `caret_phase`.
fn caret_visibility(control: &bottom_control::BottomControl) -> f32 {
    // Only blink when the field is the focus.
    if !matches!(control.mode, bottom_control::Mode::Field) {
        return 1.0;
    }
    let phase = control.caret_phase % 1.06;
    if phase < 0.6 {
        1.0
    } else {
        0.0
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

/// Diagnostic helper: write the current CPU-side icon atlas to
/// `target/atlas-dump.png` so we can eyeball that cells don't overlap after a
/// grow. Only runs when `LAUNCHPAD_DUMP_ATLAS` is set.
fn dump_atlas_png(atlas: &IconAtlas) {
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
    let timer = StartupTimer::new();
    timer.mark(prefix::STARTUP, "process start");
    startup_timer::install(timer.clone());

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

    // OS integration: global hot key (Win+Space) + tray icon. Spawned before
    // the event loop so the hot key works even during the very first frame.
    #[cfg(windows)]
    let os = platform_windows::OsIntegrationHandle::spawn(proxy.clone());

    let mut app = App::new(proxy, timer, cache, inbox, worker);
    // Anchor the OS-integration thread for the whole process lifetime.
    #[cfg(windows)]
    {
        app._os = Some(os);
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
