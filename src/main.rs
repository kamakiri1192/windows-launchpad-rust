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
mod app_icon;
mod app_id;
mod app_registry;
mod app_scan;
mod bottom_control;
mod debug_logger;
mod grid;
mod icon_atlas;
mod icon_cache;
mod icon_pipeline;
mod icon_worker;
mod icons;
mod launch;
mod layout;
mod liquid_glass;
#[cfg(windows)]
mod platform_windows;
mod refresh_watcher;
mod renderer;
mod scroll;
mod settings;
mod startup_timer;
mod text;
mod ui_model;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
use settings::{Settings, SettingsCategory, SortOrder};
use startup_timer::{prefix, StartupTimer};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::WindowAttributesExtWindows;
use winit::window::{Icon, Window, WindowId};

/// Cell edge (icon + padding) imported from the atlas module for readability.
const CELL: u32 = icon_atlas::CELL;
const CLICK_SLOP_PHYS: f32 = 8.0;
const INITIAL_WINDOW_WIDTH: f64 = 1280.0;
const INITIAL_WINDOW_HEIGHT: f64 = 800.0;
const MIN_WINDOW_WIDTH: f64 = 640.0;
const MIN_WINDOW_HEIGHT: f64 = 480.0;
/// Grace window after `summon()` during which a `Focused(false)` is treated as
/// a focus-transition artifact and ignored (SetForegroundWindow can briefly
/// drop and re-acquire focus as the OS shuffles windows). Without this the
/// just-summoned launcher would instantly hide again on some machines.
const SUMMON_FOCUS_GRACE: Duration = Duration::from_millis(500);
/// How long a press must be held (without dragging past `CLICK_SLOP_PHYS`) to
/// enter edit mode. iOS home-screen long-press is ~450–500 ms; we use 500 ms to
/// avoid accidental triggers during a slow scroll.
const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(500);
/// While dragging an icon in edit mode, holding it this close to the page-frame
/// edge starts a one-page autoscroll.
const EDIT_EDGE_SCROLL_ZONE: f32 = 72.0;

/// A grid press that hasn't yet been classified as a scroll drag, a click, or a
/// long-press into edit mode. While present, the scroller is *not* in
/// `Dragging` — we hold off until the gesture reveals its intent.
#[derive(Debug, Clone)]
struct PendingPress {
    /// When the press started (for long-press timing).
    start: Instant,
    /// Pointer position at press start (physical px).
    x: f32,
    y: f32,
    /// The app under the pointer at press start, if any. Entering edit mode
    /// lifts this app into a drag immediately.
    app_index: Option<usize>,
    /// Stable id of the app under the pointer at press start. Quick release
    /// launches this id, not whatever happens to be under the release point.
    app_id: Option<AppId>,
    /// True when the press started outside the page-frame Liquid Glass. A
    /// stationary release there dismisses the launcher instead of interacting
    /// with the grid.
    outside_glass: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsPressTarget {
    Close,
    Category(SettingsCategory),
    Sort(SortOrder),
    FrequentToggle,
    SearchHiddenToggle,
    ResetCache,
    ResetSettings,
    Inside,
    Outside,
}

type SettingsPanelLayout = layout::settings_panel::SettingsPanelLayout;

fn pending_press_launch_id(press: &PendingPress, release_x: f32, release_y: f32) -> Option<&AppId> {
    if !pending_press_is_click(press, release_x, release_y) {
        return None;
    }
    press.app_id.as_ref()
}

fn pending_press_is_click(press: &PendingPress, release_x: f32, release_y: f32) -> bool {
    let dx = release_x - press.x;
    let dy = release_y - press.y;
    dx * dx + dy * dy <= CLICK_SLOP_PHYS * CLICK_SLOP_PHYS
}

fn pending_press_is_outside_glass_click(
    press: &PendingPress,
    release_x: f32,
    release_y: f32,
) -> bool {
    press.outside_glass && pending_press_is_click(press, release_x, release_y)
}

fn settings_category_id(category: SettingsCategory) -> layout::settings_panel::SettingsCategoryId {
    match category {
        SettingsCategory::Apps => layout::settings_panel::SettingsCategoryId::Apps,
        SettingsCategory::Search => layout::settings_panel::SettingsCategoryId::Search,
        SettingsCategory::System => layout::settings_panel::SettingsCategoryId::System,
        SettingsCategory::About => layout::settings_panel::SettingsCategoryId::About,
    }
}

fn settings_category_from_id(
    category: layout::settings_panel::SettingsCategoryId,
) -> SettingsCategory {
    match category {
        layout::settings_panel::SettingsCategoryId::Apps => SettingsCategory::Apps,
        layout::settings_panel::SettingsCategoryId::Search => SettingsCategory::Search,
        layout::settings_panel::SettingsCategoryId::System => SettingsCategory::System,
        layout::settings_panel::SettingsCategoryId::About => SettingsCategory::About,
    }
}

fn sort_order_id(order: SortOrder) -> layout::settings_panel::SortOrderId {
    match order {
        SortOrder::Name => layout::settings_panel::SortOrderId::Name,
        SortOrder::Manual => layout::settings_panel::SortOrderId::Manual,
        SortOrder::Recent => layout::settings_panel::SortOrderId::Recent,
        SortOrder::Frequent => layout::settings_panel::SortOrderId::Frequent,
    }
}

fn sort_order_from_id(order: layout::settings_panel::SortOrderId) -> SortOrder {
    match order {
        layout::settings_panel::SortOrderId::Name => SortOrder::Name,
        layout::settings_panel::SortOrderId::Manual => SortOrder::Manual,
        layout::settings_panel::SortOrderId::Recent => SortOrder::Recent,
        layout::settings_panel::SortOrderId::Frequent => SortOrder::Frequent,
    }
}

fn settings_panel_copy<'a>(
    hidden_count_label: &'a str,
) -> layout::settings_panel::SettingsPanelCopy<'a> {
    layout::settings_panel::SettingsPanelCopy {
        title: SETTINGS_TITLE,
        categories: [
            (
                layout::settings_panel::SettingsCategoryId::Apps,
                SettingsCategory::Apps.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::Search,
                SettingsCategory::Search.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::System,
                SettingsCategory::System.label(),
            ),
            (
                layout::settings_panel::SettingsCategoryId::About,
                SettingsCategory::About.label(),
            ),
        ],
        sort_orders: [
            (
                layout::settings_panel::SortOrderId::Name,
                SortOrder::Name.label(),
            ),
            (
                layout::settings_panel::SortOrderId::Manual,
                SortOrder::Manual.label(),
            ),
            (
                layout::settings_panel::SortOrderId::Recent,
                SortOrder::Recent.label(),
            ),
            (
                layout::settings_panel::SortOrderId::Frequent,
                SortOrder::Frequent.label(),
            ),
        ],
        sort_label: "並び順",
        frequent_apps_label: "よく使うアプリ",
        frequent_apps_detail: "ホーム画面に表示するための準備設定",
        hidden_apps_label: "非表示アプリ",
        hidden_count_label,
        search_hidden_label: "検索時に非表示アプリを含める",
        search_hidden_detail: "検索中だけ、隠したアプリも結果に表示します",
        reset_cache_label: "キャッシュをリセット",
        reset_cache_detail: "アイコンを再抽出します",
        reset_settings_label: "設定をリセット",
        reset_settings_detail: "並び順、非表示、設定値を初期状態に戻します",
        version_label: "バージョン",
        version_value: env!("CARGO_PKG_VERSION"),
    }
}

fn settings_press_target_from_layout_hit(
    hit: layout::settings_panel::SettingsPanelHit,
) -> SettingsPressTarget {
    match hit {
        layout::settings_panel::SettingsPanelHit::Close => SettingsPressTarget::Close,
        layout::settings_panel::SettingsPanelHit::Category(category) => {
            SettingsPressTarget::Category(settings_category_from_id(category))
        }
        layout::settings_panel::SettingsPanelHit::Sort(order) => {
            SettingsPressTarget::Sort(sort_order_from_id(order))
        }
        layout::settings_panel::SettingsPanelHit::FrequentToggle => {
            SettingsPressTarget::FrequentToggle
        }
        layout::settings_panel::SettingsPanelHit::SearchHiddenToggle => {
            SettingsPressTarget::SearchHiddenToggle
        }
        layout::settings_panel::SettingsPanelHit::ResetCache => SettingsPressTarget::ResetCache,
        layout::settings_panel::SettingsPanelHit::ResetSettings => {
            SettingsPressTarget::ResetSettings
        }
        layout::settings_panel::SettingsPanelHit::Inside => SettingsPressTarget::Inside,
        layout::settings_panel::SettingsPanelHit::Outside => SettingsPressTarget::Outside,
    }
}

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
    /// Toggle the settings overlay (tray "Settings" / gear button).
    ToggleSettings,
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

    // ---- edit mode (iOS-style drag-to-reorder) ----
    /// True while the launcher is in the wiggling, reorderable state. Entered
    /// via long-press on an icon; exited via Esc / outside click / Done.
    editing: bool,
    /// A press is currently held down on the grid (not on the control) and we
    /// haven't yet decided whether it's a scroll drag, a click, or a long-press
    /// into edit mode. `Some` holds the press start time + pointer + app id.
    pending_press: Option<PendingPress>,
    /// The app currently being dragged in edit mode (lifted off the grid). Its
    /// tile is drawn at the pointer instead of its home cell.
    drag_app: Option<AppId>,
    /// Pointer position the dragged tile follows (physical px, screen space).
    drag_x: f32,
    drag_y: f32,
    /// Accumulated wiggle animation phase (seconds). Only advances while
    /// `editing`.
    wiggle_phase: f32,
    /// Per-visible-app position springs. Each spring is keyed by `AppId` so it
    /// follows the app across reorder operations: the old cell remains the
    /// current spring value, and the app's new cell becomes the target.
    tile_springs: Vec<(AppId, scroll::Spring2)>,

    // ---- bottom-center morphing control (search pill / page indicator /
    // search field) ----
    control: bottom_control::BottomControl,
    /// Measured laid-out width (physical px) of the current search query, set
    /// once per frame in `render_bottom_control` (where we hold `&mut text`)
    /// and read back by `measure_query_width`. `None` = not measured this
    /// frame yet.
    cached_query_width: Option<f32>,
    /// Measured laid-out width (physical px) of the edit-mode Done label.
    cached_done_width: Option<f32>,
    /// 0 = normal bottom control width, 1 = edit-mode Done width.
    edit_control_progress: f32,
    /// Last settled page index, used to detect page changes for the indicator.
    last_page: i32,
    /// Whether the pointer is currently over the control capsule (hover),
    /// for hit-testing click vs. background.
    pointer_over_control: bool,
    /// True while the left button is held down *and* the press started on the
    /// control capsule. Such a release is a control click, not an app launch.
    pressed_on_control: bool,
    /// Settings overlay target pressed by the current pointer down, if any.
    pressed_on_settings: Option<SettingsPressTarget>,
    /// True while the settings overlay panel is shown on top of the grid.
    settings_open: bool,
    /// 0..1 presentation progress for the settings panel open/close animation.
    settings_panel_progress: f32,
    /// Persisted settings edited by the overlay.
    settings: Settings,
    /// Sidebar category currently shown by the settings overlay.
    settings_category: SettingsCategory,
    /// Timestamp of the last redraw, used to compute a real dt for the control
    /// animations (caret blink + morphs).
    last_redraw: Option<Instant>,

    // ---- resident-lifecycle state ----
    /// Whether the window is currently visible. `set_visible` doesn't query,
    /// so we track it ourselves to make `hide()` idempotent (avoids a hide
    /// storm when a focus-loss event races an app-launch hide).
    visible: bool,
    /// When the most recent `summon()` happened. A `Focused(false)` that
    /// arrives within `SUMMON_FOCUS_GRACE` of a summon is treated as a
    /// focus-transition artifact (SetForegroundWindow can briefly lose and
    /// re-acquire focus as the OS shuffles windows) and ignored, instead of
    /// instantly hiding the just-summoned window.
    last_summon: Option<Instant>,
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
            editing: false,
            pending_press: None,
            drag_app: None,
            drag_x: 0.0,
            drag_y: 0.0,
            wiggle_phase: 0.0,
            tile_springs: Vec::new(),
            control: bottom_control::BottomControl::new(),
            cached_query_width: None,
            cached_done_width: None,
            edit_control_progress: 0.0,
            last_page: 0,
            pointer_over_control: false,
            pressed_on_control: false,
            pressed_on_settings: None,
            settings_open: false,
            settings_panel_progress: 0.0,
            settings: Settings::default(),
            settings_category: SettingsCategory::Apps,
            last_redraw: None,
            visible: true,
            last_summon: None,
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
        // position is the content offset; page = round(-position / page_extent),
        // where page_extent is the content (liquid-glass) page width.
        let page_w = self.layout.page_width(w.max(1) as f32);
        if page_w <= 0.0 || !page_w.is_finite() {
            return 0;
        }
        let p = (-s.position / page_w).round() as i32;
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
        let edit_width = self.cached_done_width.map(|w| bottom_control::EditWidth {
            half_width: bottom_control::done_half_width(w, self.scale_factor),
            progress: self.edit_control_progress,
        });
        Some(self.control.resolve_scaled_with_edit_width(
            viewport,
            frame_bottom,
            page,
            page_count,
            self.scale_factor,
            edit_width,
        ))
    }

    /// Lay out and upload the bottom control's glass capsule + overlay shapes
    /// and text for the current frame. Call this once per redraw, after the
    /// control has been ticked.
    fn render_bottom_control(&mut self) {
        // Gather all the immutable data first (avoid overlapping borrows with
        // the mutable renderer/text borrows below).
        let scale = self.scale_factor;
        // Measure the query width exactly via cosmic-text shaping (same pass
        // as drawing), so the caret and IME anchor line up with the glyphs
        // regardless of ASCII/CJK widths. Cached for the frame so
        // `measure_query_width` can read it back under `&self`.
        if let Some(m) = self.text.as_mut() {
            self.cached_query_width = {
                let mut measure = |s: &str| -> f32 {
                    if s.is_empty() {
                        return 0.0;
                    }
                    let spec = text::CenteredLineSpec {
                        text: s,
                        font_size: QUERY_LABEL_SIZE,
                        line_height: QUERY_LABEL_LINE,
                        family: QUERY_LABEL_FONT,
                        color: [1.0, 1.0, 1.0, 1.0],
                        center: (0.0, 0.0),
                        scale_factor: scale,
                    };
                    m.measure_text(&spec)
                };
                // The caret sits after *all visible text*: the committed query
                // plus the in-flight IME preedit. Without the preedit width,
                // the caret stays put while the user types Japanese.
                let w = measure(&self.control.query) + measure(&self.control.preedit);
                Some(w)
            };
            let spec = text::CenteredLineSpec {
                text: DONE_LABEL,
                font_size: QUERY_LABEL_SIZE,
                line_height: QUERY_LABEL_LINE,
                family: QUERY_LABEL_FONT,
                color: [1.0, 1.0, 1.0, 1.0],
                center: (0.0, 0.0),
                scale_factor: scale,
            };
            self.cached_done_width = Some(m.measure_text(&spec));
        }
        let (geom, layers) = match self.resolve_control() {
            Some(v) => v,
            None => return,
        };
        let query_width = self.measure_query_width();
        let caret_blink = caret_visibility(&self.control);
        let edit_visual_progress = self.edit_visual_progress();

        // 1) Procedural overlay instances (magnifier, dots, caret, close).
        // While the Done-width morph is active, keep normal pill contents hidden
        // so they don't overflow the narrower capsule.
        let instances = if edit_visual_progress > 0.0 {
            Vec::new()
        } else {
            bottom_control::build_overlay_instances(&geom, &layers, query_width, caret_blink)
        };

        // 2) Text glyphs (label / query / placeholder). Built via the shared
        // text renderer so they share the glyph atlas. Done before touching the
        // renderer so the atlas upload + dirty clear happen in one place.
        let (quads, atlas_dirty) = if let Some(t) = self.text.as_mut() {
            let q = self_layout_control_text(
                t,
                &geom,
                &layers,
                scale,
                &self.control,
                edit_visual_progress,
            );
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

    /// Lay out and upload the edit-mode settings gear capsule (the second
    /// capsule shown beside the Done button in edit mode). Hidden at all other
    /// times. See `bottom_control::edit_gear_geometry`.
    fn render_gear(&mut self) {
        // The gear only appears in edit mode, alongside the Done capsule.
        let edit_progress = self.edit_visual_progress();
        let show = self.visible && edit_progress > 0.0 && !self.settings_panel_active();
        if !show {
            if let Some(r) = self.renderer.as_mut() {
                r.set_gear_glass_shape(None);
                r.set_gear_instances(&[]);
            }
            return;
        }
        let viewport = self.viewport_phys();
        let frame_bottom = self.frame_bottom_y();
        let scale = self.scale_factor;
        let done_hw = self
            .cached_done_width
            .map(|w| bottom_control::done_half_width(w, scale))
            .unwrap_or_else(|| bottom_control::done_half_width(0.0, scale));
        let Some((geom, alpha)) = bottom_control::edit_gear_geometry(
            viewport,
            frame_bottom,
            scale,
            done_hw,
            edit_progress,
        ) else {
            if let Some(r) = self.renderer.as_mut() {
                r.set_gear_glass_shape(None);
                r.set_gear_instances(&[]);
            }
            return;
        };
        let instance = bottom_control::edit_gear_instance(&geom, alpha);
        let shape = bottom_control::edit_gear_glass_shape(&geom);
        if let Some(r) = self.renderer.as_mut() {
            r.set_gear_glass_shape(Some(shape));
            r.set_gear_instances(&[instance]);
        }
    }

    fn settings_panel_layout(&self) -> SettingsPanelLayout {
        layout::settings_panel::panel_layout(self.viewport_phys(), self.scale_factor)
    }

    /// True when physical-px point `(x, y)` is inside the settings panel rect.
    fn settings_panel_contains(&self, x: f32, y: f32) -> bool {
        let layout = self.settings_panel_layout();
        layout::settings_panel::contains(&layout, ui_model::geometry::Point::new(x, y))
    }

    /// True when `(x, y)` is over the panel's close (×) button.
    fn settings_panel_hit_close(&self, x: f32, y: f32) -> bool {
        let layout = self.settings_panel_layout();
        layout::settings_panel::hit_close(
            &layout,
            self.scale_factor,
            ui_model::geometry::Point::new(x, y),
        )
    }

    fn settings_hit_target(&self, x: f32, y: f32) -> SettingsPressTarget {
        let layout = self.settings_panel_layout();
        let hit = layout::settings_panel::hit_test(
            &layout,
            self.scale_factor,
            settings_category_id(self.settings_category),
            ui_model::geometry::Point::new(x, y),
        );
        settings_press_target_from_layout_hit(hit)
    }

    fn handle_settings_click(&mut self, target: SettingsPressTarget) {
        match target {
            SettingsPressTarget::Close => self.close_settings(),
            SettingsPressTarget::Category(category) => {
                self.settings_category = category;
                self.request_redraw();
            }
            SettingsPressTarget::Sort(order) => {
                self.settings.sort_order = order;
                if order == SortOrder::Name {
                    self.registry.set_order(Vec::new());
                    self.persist_user_order();
                    self.relayout();
                }
                self.persist_settings();
                self.request_redraw();
            }
            SettingsPressTarget::FrequentToggle => {
                self.settings.frequent_apps_enabled = !self.settings.frequent_apps_enabled;
                self.persist_settings();
                self.request_redraw();
            }
            SettingsPressTarget::SearchHiddenToggle => {
                self.settings.search_includes_hidden = !self.settings.search_includes_hidden;
                self.persist_settings();
                self.search_input_changed();
            }
            SettingsPressTarget::ResetCache => {
                self.reset_icons();
                self.request_redraw();
            }
            SettingsPressTarget::ResetSettings => {
                self.settings = Settings::default();
                self.registry.set_order(Vec::new());
                self.registry.set_hidden(Vec::new());
                self.persist_settings();
                self.persist_user_order();
                self.persist_hidden();
                self.relayout();
                self.request_redraw();
            }
            SettingsPressTarget::Inside | SettingsPressTarget::Outside => {}
        }
    }

    /// Lay out and upload the settings overlay panel (glass + title text +
    /// close ×) for the current frame. No-op when the overlay is closed.
    fn render_settings_panel(&mut self) {
        if !self.settings_panel_active() {
            if let Some(r) = self.renderer.as_mut() {
                r.set_settings_panel_glass_shape(None);
                r.set_settings_instances(&[]);
                r.set_settings_text_instances(&[]);
            }
            return;
        }

        let scale = self.scale_factor;
        let hidden_count = self.registry.hidden().len();
        let hidden_count_label = format!("{hidden_count} 件");
        let copy = settings_panel_copy(&hidden_count_label);
        let model = layout::settings_panel::build_with_copy(
            layout::settings_panel::SettingsPanelInput {
                viewport: self.viewport_phys(),
                scale_factor: scale,
                category: settings_category_id(self.settings_category),
                sort_order: sort_order_id(self.settings.sort_order),
                frequent_apps_enabled: self.settings.frequent_apps_enabled,
                search_includes_hidden: self.settings.search_includes_hidden,
                hidden_count,
                progress: self.settings_panel_progress,
            },
            &copy,
        );
        let layout = model.layout;
        let visual_scale = model.visual_scale;
        let visual_alpha = model.visual_alpha;

        let shape = crate::liquid_glass::geometry::GlassShape::control_rounded_rect(
            [layout.cx, layout.cy],
            [
                layout.hw * 2.0 * visual_scale,
                layout.hh * 2.0 * visual_scale,
            ],
            layout.radius * visual_scale,
        );

        // Close × glyph at the top-right inset.
        let btn_r = layout::settings_panel::CLOSE_HALF * scale;
        let close = control_icon(
            layout.left + layout.hw * 2.0 - btn_r * 2.0,
            layout.top + btn_r * 2.0,
            btn_r,
            bottom_control::KIND_CLOSE,
            layout::settings_panel::INK,
        );

        let mut instances = Vec::new();
        let mut quads = Vec::new();
        let current_settings = self.settings.clone();
        let current_category = self.settings_category;

        build_settings_panel_instances(
            &layout,
            scale,
            current_category,
            &current_settings,
            hidden_count,
            &mut instances,
        );
        instances.push(close);

        if let Some(t) = self.text.as_mut() {
            build_settings_panel_text_views(t, &model.result.render.text, scale, &mut quads);
        }

        transform_settings_instances(
            &mut instances,
            [layout.cx, layout.cy],
            visual_scale,
            visual_alpha,
        );
        transform_settings_quads(
            &mut quads,
            [layout.cx, layout.cy],
            visual_scale,
            visual_alpha,
        );

        if let Some(r) = self.renderer.as_mut() {
            r.set_settings_panel_glass_shape(Some(shape));
            r.set_settings_instances(&instances);
            // Upload the atlas if the title added any glyphs this frame.
            if let Some(t) = self.text.as_ref() {
                if t.atlas_dirty {
                    r.upload_atlas(t.atlas_rgba());
                }
            }
            r.set_settings_text_instances(&quads);
        }
        if let Some(t) = self.text.as_mut() {
            t.atlas_dirty = false;
        }
    }

    /// Measure the current visible search text's laid-out width in physical px (for caret
    /// placement). Returns the value measured this frame in
    /// `render_bottom_control` via cosmic-text shaping (cached so this can be
    /// called under `&self`). Falls back to 0 when not measured yet or when
    /// both the committed query and in-flight preedit are empty.
    fn measure_query_width(&self) -> f32 {
        if self.control.query.is_empty() && self.control.preedit.is_empty() {
            return 0.0;
        }
        self.cached_query_width.unwrap_or(0.0)
    }

    fn step_edit_control_width(&mut self, dt: f32) -> bool {
        let target = if self.editing { 1.0 } else { 0.0 };
        let duration = if self.editing {
            bottom_control::EXPAND_DURATION
        } else {
            bottom_control::COLLAPSE_DURATION
        };
        let before = self.edit_control_progress;
        self.edit_control_progress =
            advance_unit_toward(self.edit_control_progress, target, dt, duration);
        (self.edit_control_progress - before).abs() > 0.0001
            || (self.edit_control_progress - target).abs() > 0.0001
    }

    fn edit_visual_progress(&self) -> f32 {
        if self.editing {
            1.0
        } else if self.edit_control_progress > 0.001 {
            self.edit_control_progress.max(0.0)
        } else {
            0.0
        }
    }

    fn settings_panel_active(&self) -> bool {
        self.settings_open || self.settings_panel_progress > 0.001
    }

    fn step_settings_panel(&mut self, dt: f32) -> bool {
        let target = if self.settings_open { 1.0 } else { 0.0 };
        let duration = if self.settings_open {
            layout::settings_panel::OPEN_DURATION
        } else {
            layout::settings_panel::CLOSE_DURATION
        };
        let before = self.settings_panel_progress;
        self.settings_panel_progress =
            advance_unit_toward(self.settings_panel_progress, target, dt, duration);
        if !self.settings_open && self.settings_panel_progress < 0.001 {
            self.settings_panel_progress = 0.0;
        }
        (self.settings_panel_progress - before).abs() > 0.0001
            || (self.settings_panel_progress - target).abs() > 0.0001
    }

    /// Search text used for live filtering. This intentionally includes the
    /// active IME preedit so Japanese composition narrows the grid before the
    /// text is committed, while the committed query remains stored separately.
    fn visible_search_query(&self) -> String {
        let mut q = String::with_capacity(self.control.query.len() + self.control.preedit.len());
        q.push_str(&self.control.query);
        q.push_str(&self.control.preedit);
        q
    }

    fn matches_search(name: &str, query: &str) -> bool {
        let query = query.trim();
        if query.is_empty() {
            return true;
        }

        let haystack = name.to_lowercase();
        query
            .split_whitespace()
            .map(str::to_lowercase)
            .all(|needle| haystack.contains(&needle))
    }

    /// Rebuild visible search results and redraw immediately after any input
    /// mutation. Keeps text input, IME composition, tiles, labels, click
    /// resolution, and scroll bounds in one state transition.
    fn search_input_changed(&mut self) {
        self.relayout();
        let (w, _h) = self.viewport_phys();
        let bounds = self.layout.bounds(w as f32);
        if let Some(s) = self.scroller.as_mut() {
            s.position = bounds.snap_target(s.position);
            s.velocity = 0.0;
            s.phase = Phase::Idle;
        }
        self.last_page = self.current_page() as i32;
        self.request_redraw();
    }

    /// Build an owned snapshot of the currently visible app list in display order.
    /// Returns owned data so it doesn't hold a borrow on `self` while the
    /// renderer mutates.
    ///
    /// Apps the user hid via the edit-mode ✕ badge are excluded here (same path
    /// as the search filter), so they never reach the grid or click resolution.
    fn grid_apps_owned(&self) -> Vec<(String, Option<icons::UvRect>)> {
        let query = self.visible_search_query();
        let include_hidden = self.settings.search_includes_hidden && !query.trim().is_empty();
        self.registry
            .apps()
            .iter()
            .filter(|rec| include_hidden || !self.registry.is_hidden(&rec.app_id))
            .filter(|rec| Self::matches_search(&rec.name, &query))
            .map(|rec| (rec.name.clone(), rec.uv))
            .collect()
    }

    fn visible_app_ids(&self) -> Vec<AppId> {
        let query = self.visible_search_query();
        let include_hidden = self.settings.search_includes_hidden && !query.trim().is_empty();
        self.registry
            .apps()
            .iter()
            .filter(|rec| include_hidden || !self.registry.is_hidden(&rec.app_id))
            .filter(|rec| Self::matches_search(&rec.name, &query))
            .map(|rec| rec.app_id.clone())
            .collect()
    }

    /// Recompute layout/bounds for the current window size and push tile +
    /// label + icon instance buffers to the GPU.
    fn relayout(&mut self) {
        let (w, _h) = self.viewport_phys();
        let owned = self.grid_apps_owned();
        // Size pages to the current visible app count so every filtered app is
        // reachable and blank trailing pages disappear during search.
        self.layout = grid::GridLayout::for_app_count(owned.len())
            .with_scale_factor(self.scale_factor)
            .centered(w as f32);
        let bounds = self.layout.bounds(w as f32);
        if let Some(s) = self.scroller.as_mut() {
            s.set_bounds(bounds);
        }

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

        let visible_ids = self.visible_app_ids();
        let anim = self.edit_anim(&visible_ids);
        // Update the per-tile position springs to the new home cells (keeping
        // each spring's current value so tiles glide from where they were).
        self.update_tile_springs(&visible_ids, w as f32);
        // Build the instances and override each tile's position with its spring
        // value so a reorder (or relayout) animates the icons sliding into place
        // rather than snapping. Done before the renderer borrow so we can read
        // the springs under &self.
        let mut tile_instances = self.layout.build_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut tile_instances);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut icon_instances);
        // While dragging, lift the dragged app off the grid: remove it from the
        // normal instance list and append a pointer-following copy at the end so
        // it draws on top of everything else.
        self.lift_dragged_instances(&mut tile_instances, &mut icon_instances, &visible_ids);
        if let Some(r) = self.renderer.as_mut() {
            // The liquid-glass shape rebuild uses the resting positions (the
            // glass doesn't need to follow the slide); the tile/icon instance
            // buffers carry the spring-adjusted positions.
            r.rebuild_instances(&self.layout, &apps, &anim);
            r.set_tile_instances(&tile_instances);
            r.set_icon_instances(&icon_instances);
        }

        let atlas_grew = self.ensure_atlas_uploaded();
        if atlas_grew {
            // Growing the atlas changes UVs for icons that were already cached
            // before this relayout, so refresh the icon instance buffer once
            // more after the registry has been re-synced.
            self.rebuild_icon_instances();
        }
    }

    /// Upload (or grow) the GPU icon atlas to match the CPU atlas, then push
    /// the full pixel buffer once after the first allocation. Subsequent
    /// per-icon updates go through [`apply_icon`][Self::apply_icon].
    fn ensure_atlas_uploaded(&mut self) -> bool {
        let needed = self.registry.slot_count().max(1);
        let grew = self.atlas.ensure_capacity(needed);
        if grew {
            self.resync_registry_uvs();
        }
        let Some(r) = self.renderer.as_mut() else {
            return grew;
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
        grew
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
        let visible_ids = self.visible_app_ids();
        let anim = self.edit_anim(&visible_ids);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut icon_instances);
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

    // ---- edit mode (iOS-style reorder) -----------------------------------

    /// Begin a grid press. Instead of immediately starting a scroll drag (the
    /// old behavior), we record the press and wait to see whether it becomes a
    /// drag (→ scroll), a quick release (→ click/launch), or a long-press
    /// (→ enter edit mode). The scroller stays `Idle` until intent is clear.
    fn begin_grid_press(&mut self, now: Instant) {
        let x = self.pointer_phys_x;
        let y = self.pointer_phys_y;
        let app_index = self.app_index_at_pointer(x, y);
        let app_id = app_index.and_then(|idx| self.visible_app_ids().get(idx).cloned());
        let outside_glass = !self.pointer_over_page_glass(x, y);
        debug_log!(
            "edit-press: pending x={x:.1} y={y:.1} app_index={app_index:?} outside_glass={outside_glass}"
        );
        self.pending_press = Some(PendingPress {
            start: now,
            x,
            y,
            app_index,
            app_id,
            outside_glass,
        });
        self.request_redraw();
    }

    fn pointer_over_page_glass(&self, x: f32, y: f32) -> bool {
        let (w, _h) = self.viewport_phys();
        self.layout.frame_contains_point(w as f32, x, y)
    }

    /// Resolve the visible-grid index under the pointer (or `None` if it's over
    /// empty space / the label gap that isn't a real cell). Used both for
    /// long-press target and drop-target detection.
    fn app_index_at_pointer(&self, x: f32, y: f32) -> Option<usize> {
        let (w, _h) = self.viewport_phys();
        let scroll_x = self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        let visible_ids = self.visible_app_ids();
        self.layout
            .hit_test_app(w as f32, x, y, scroll_x, visible_ids.len())
    }

    /// Resolve a tile-cell index for edit-mode drag/drop. Unlike app click
    /// hit-testing this excludes labels and allows the empty slot immediately
    /// after the last visible app, so dropping at the final page tail works.
    fn edit_drop_index_at_pointer(&self, x: f32, y: f32) -> Option<usize> {
        let (w, _h) = self.viewport_phys();
        let scroll_x = self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        self.layout
            .hit_test_tile_cell(w as f32, x, y, scroll_x, self.layout.total_tiles())
    }

    /// True if the pointer (physical px) is over the ✕ badge of the app at
    /// `idx`. The badge sits at the tile's top-left corner, with a radius
    /// matching the shader's badge radius (≈13% of the tile size, max 11px). A
    /// little slop is added so the hit area is forgiving on a touch screen.
    fn badge_hit(&self, idx: usize, x: f32, y: f32) -> bool {
        let (w, _h) = self.viewport_phys();
        let scroll_x = self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        let (tx, ty) = self.layout.tile_position(w as f32, idx);
        let radius = self.layout.edit_badge_radius();
        let hit_r = radius + self.layout.edit_badge_hit_slop();
        let inset = radius * 0.45;
        let cx = tx + scroll_x + inset;
        let cy = ty + inset;
        let dx = x - cx;
        let dy = y - cy;
        dx * dx + dy * dy <= hit_r * hit_r
    }

    /// Hide an app from the launcher (the ✕ badge action): removes it from the
    /// visible stream, persists the hidden list, and relayouts. Reversible by
    /// clearing the hidden list later. Stays a no-op if already hidden.
    fn hide_app(&mut self, id: &AppId) {
        if self.registry.is_hidden(id) {
            return;
        }
        self.registry.hide(id);
        // Drop the app from the user order too so it doesn't linger invisibly.
        let mut order: Vec<AppId> = self
            .registry
            .order()
            .iter()
            .filter(|x| *x != id)
            .cloned()
            .collect();
        order.push(id.clone());
        self.registry.set_order(order);
        self.persist_hidden();
        self.persist_user_order();
        // Drop any in-flight drag of the just-hidden app.
        if self.drag_app.as_ref() == Some(id) {
            self.drag_app = None;
        }
        self.relayout();
        self.request_redraw();
    }

    /// A press is pending; check whether movement past `CLICK_SLOP_PHYS`
    /// promotes it to a real scroll drag. Returns true if it was promoted.
    fn maybe_promote_press_to_drag(&mut self) -> bool {
        let Some(p) = self.pending_press.as_ref() else {
            return false;
        };
        let dx = self.pointer_phys_x - p.x;
        let dy = self.pointer_phys_y - p.y;
        if dx * dx + dy * dy <= CLICK_SLOP_PHYS * CLICK_SLOP_PHYS {
            return false;
        }
        let start_x = p.x;
        let start_y = p.y;
        // Promote: start the scroll drag from the original anchor, then apply
        // the current pointer so the page follows the gesture from here.
        self.pending_press = None;
        self.handle_drag_start(start_x, start_y);
        if self.scroller.as_ref().map(|s| s.phase) == Some(Phase::Dragging) {
            self.handle_drag_move(self.pointer_phys_x);
        }
        true
    }

    /// Check whether the pending press has been held long enough to enter edit
    /// mode. Called from `about_to_wait`. Returns true if edit mode was entered.
    fn maybe_long_press_into_edit(&mut self, now: Instant) -> bool {
        let Some(p) = self.pending_press.as_ref() else {
            return false;
        };
        if p.outside_glass {
            return false;
        }
        // Only a press that hasn't moved past slop can become a long-press.
        let dx = self.pointer_phys_x - p.x;
        let dy = self.pointer_phys_y - p.y;
        if dx * dx + dy * dy > CLICK_SLOP_PHYS * CLICK_SLOP_PHYS {
            return false;
        }
        if now.duration_since(p.start) < LONG_PRESS_THRESHOLD {
            return false;
        }
        // Enter edit mode and immediately lift the pressed app into a drag.
        let app_index = p.app_index;
        self.enter_edit_mode(app_index);
        true
    }

    /// Enter edit mode, optionally lifting `app_index` straight into a drag
    /// (the long-press path). Edit mode is idempotent.
    fn enter_edit_mode(&mut self, app_index: Option<usize>) {
        let was_editing = self.editing;
        self.editing = true;
        self.pending_press = None;
        self.wiggle_phase = 0.0;
        // Cancel any in-flight scroll so the page sits still while editing.
        if let Some(s) = self.scroller.as_mut() {
            if s.phase != Phase::Idle {
                s.phase = Phase::Idle;
                s.velocity = 0.0;
            }
        }
        // Lift the long-pressed app (if any) into a drag.
        if let Some(idx) = app_index {
            let visible = self.visible_app_ids();
            if let Some(id) = visible.get(idx).cloned() {
                self.drag_app = Some(id);
                self.drag_x = self.pointer_phys_x;
                self.drag_y = self.pointer_phys_y;
            }
        }
        if !was_editing {
            debug_log!("edit-mode: entered");
        }
        self.relayout();
        self.request_redraw();
    }

    /// Exit edit mode. Commits any in-progress drag (if the lifted app was
    /// dropped on a valid cell) and persists the resulting order. Safe to call
    /// when not editing.
    fn exit_edit_mode(&mut self) {
        if !self.editing {
            return;
        }
        // If a drag was in flight, finalize it as a drop at the current cell.
        if self.drag_app.is_some() {
            self.commit_reorder();
        }
        self.editing = false;
        self.drag_app = None;
        self.pending_press = None;
        self.relayout();
        debug_log!("edit-mode: exited");
        self.request_redraw();
    }

    /// Open the settings overlay. Dismisses edit mode and the search field so
    /// they cannot be interacted with underneath the panel.
    fn open_settings(&mut self) {
        if self.editing {
            self.exit_edit_mode();
        }
        if self.control.wants_keyboard() {
            self.control.press_close();
            self.search_input_changed();
        }
        self.pending_press = None;
        self.pressed_on_control = false;
        self.settings_open = true;
        debug_log!("settings: opened");
        self.request_redraw();
    }

    /// Close the settings overlay if it is open. Safe to call when closed.
    fn close_settings(&mut self) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        debug_log!("settings: closed");
        self.request_redraw();
    }

    /// Toggle the settings overlay.
    fn toggle_settings(&mut self) {
        if self.settings_open {
            self.close_settings();
        } else {
            self.open_settings();
        }
    }

    /// Update the dragged tile's follow position during an edit-mode move.
    fn handle_edit_drag_move(&mut self) {
        if self.drag_app.is_some() {
            self.drag_x = self.pointer_phys_x;
            self.drag_y = self.pointer_phys_y;
            // Live reorder: move the dragged app to the cell under the pointer
            // so other icons shift around it.
            self.live_reorder();
            self.maybe_autoscroll_edit_drag();
            self.request_redraw();
        }
    }

    /// Move `drag_app` to the visible cell currently under the pointer (if it's
    /// a different cell). No-op when the pointer is off the grid.
    fn live_reorder(&mut self) {
        let Some(drag_id) = self.drag_app.clone() else {
            return;
        };
        let Some(target_idx) = self.edit_drop_index_at_pointer(self.drag_x, self.drag_y) else {
            return;
        };
        let visible = self.visible_app_ids();
        let Some(drag_pos) = visible.iter().position(|id| id == &drag_id) else {
            return;
        };
        let insert_idx = target_idx.min(visible.len());
        if insert_idx == drag_pos {
            return;
        }
        debug_log!(
            "edit-reorder: moving drag_pos={drag_pos} target_idx={target_idx} insert_idx={insert_idx}"
        );
        self.reorder_by_index(&drag_id, insert_idx);
    }

    /// Start a one-page autoscroll if the lifted edit-mode icon is held near a
    /// page-frame edge. Returns true when a new page glide was started.
    fn maybe_autoscroll_edit_drag(&mut self) -> bool {
        if !self.editing || self.drag_app.is_none() {
            return false;
        }

        let (w, _h) = self.viewport_phys();
        let (cx, cy, panel_w, panel_h) = self.layout.frame_panel_rect(w.max(1) as f32);
        let top = cy - panel_h * 0.5;
        let bottom = cy + panel_h * 0.5;
        if self.drag_y < top || self.drag_y > bottom {
            return false;
        }

        let zone = self
            .layout
            .scaled(EDIT_EDGE_SCROLL_ZONE)
            .min(panel_w * 0.25)
            .max(24.0);
        let left = cx - panel_w * 0.5;
        let right = cx + panel_w * 0.5;
        let grid_left = self.layout.margin_left;
        let grid_right = self.layout.margin_left + self.layout.grid_w();
        let left_zone = zone.min((grid_left - left).max(0.0));
        let right_zone = zone.min((right - grid_right).max(0.0));
        let current = self.current_page();
        let target = if left_zone > 0.0 && self.drag_x <= left + left_zone && current > 0 {
            Some(current - 1)
        } else if right_zone > 0.0
            && self.drag_x >= right - right_zone
            && current + 1 < self.layout.page_count
        {
            Some(current + 1)
        } else {
            None
        };

        let Some(target) = target else {
            return false;
        };
        let Some(scroller) = self.scroller.as_mut() else {
            return false;
        };
        if scroller.phase != Phase::Idle {
            return false;
        }
        if scroller.settle_to_page(target) {
            self.request_redraw();
            true
        } else {
            false
        }
    }

    /// Finalize the in-flight drag: drop the dragged app onto the current cell
    /// and persist the new order.
    fn commit_reorder(&mut self) {
        self.live_reorder();
        self.settings.sort_order = SortOrder::Manual;
        self.persist_settings();
        self.persist_user_order();
    }

    /// Reorder the registry so that `drag_id` moves to `insert_idx` in the
    /// visible order, shifting the apps between them. Hidden apps are preserved
    /// after the visible stream.
    fn reorder_by_index(&mut self, drag_id: &AppId, insert_idx: usize) {
        // Build the current visible order from the registry's display order,
        // then move drag_id to the requested visible insertion index.
        let mut order: Vec<AppId> = self
            .visible_app_ids()
            .into_iter()
            // Append hidden apps at the end so they're preserved but never
            // repositioned visibly.
            .chain(self.registry.hidden().iter().cloned())
            .collect();
        let Some(drag_pos) = order.iter().position(|i| i == drag_id) else {
            return;
        };
        let id = order.remove(drag_pos);
        order.insert(insert_idx.min(order.len()), id);
        self.registry.set_order(order);
        self.relayout();
    }

    /// Hit-test the pointer, then resolve the clicked app **by stable id**
    /// (not positional index), so a rescan that shifted the list can't launch
    /// the wrong app. Returns an owned snapshot safe to use after the
    /// launcher dismisses.
    fn resolve_clicked_app(&self, x_phys: f32, y_phys: f32) -> Option<AppLaunchInfo> {
        let (w, _h) = self.viewport_phys();
        let scroll_x = self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        let visible_ids = self.visible_app_ids();
        let app_index =
            self.layout
                .hit_test_app(w as f32, x_phys, y_phys, scroll_x, visible_ids.len())?;
        // Map display index → stable id → launch snapshot. Going through the id
        // means even a concurrent mutation between pick and launch can't
        // resolve to the wrong app.
        let app_id = visible_ids.get(app_index)?;
        self.registry.launch_info(app_id)
    }

    fn request_redraw(&self) {
        if let Some(r) = self.renderer.as_ref() {
            r.window.request_redraw();
        }
    }

    /// Build the per-app edit-mode animation parameters. Each visible app gets
    /// a `TileAnim`; outside edit mode they're all `IDLE`.
    ///
    /// - In edit mode, every app wiggles (FLAG_WIGGLE + a per-app phase offset
    ///   so they don't all swing in lockstep).
    /// - The app being dragged (if any) is lifted (`lift`), enlarged (`scale`),
    ///   and flagged `FLAG_DRAG` so the shader bypasses the frame clip and
    ///   follows the pointer instead of its home cell.
    fn edit_anim(&self, visible_ids: &[AppId]) -> Vec<grid::TileAnim> {
        if !self.editing {
            return Vec::new();
        }
        let drag_id = self.drag_app.as_ref();
        visible_ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let is_drag = drag_id.map(|d| d == id).unwrap_or(false);
                if is_drag {
                    grid::TileAnim {
                        phase: self.wiggle_phase + i as f32 * 0.37,
                        lift: 24.0 * self.scale_factor.max(1.0),
                        scale: 1.15,
                        flags: grid::TileAnim::FLAG_WIGGLE | grid::TileAnim::FLAG_DRAG,
                    }
                } else {
                    grid::TileAnim {
                        phase: self.wiggle_phase + i as f32 * 0.37,
                        lift: 0.0,
                        scale: 1.0,
                        flags: grid::TileAnim::FLAG_WIGGLE,
                    }
                }
            })
            .collect()
    }

    /// Realign `tile_springs` with the current visible app set. Existing
    /// springs are matched by `AppId`, not position, so a reordered app keeps
    /// its previous cell as the spring value and glides to its new home cell.
    fn update_tile_springs(&mut self, visible_ids: &[AppId], viewport_w: f32) {
        let mut old = std::mem::take(&mut self.tile_springs);
        self.tile_springs.reserve(visible_ids.len());
        for (i, id) in visible_ids.iter().enumerate() {
            let (x, y) = self.layout.tile_position(viewport_w, i);
            if let Some(pos) = old.iter().position(|(spring_id, _)| spring_id == id) {
                let (_, mut spring) = old.swap_remove(pos);
                spring.glide_to(x, y);
                self.tile_springs.push((id.clone(), spring));
            } else {
                self.tile_springs
                    .push((id.clone(), scroll::Spring2::at(x, y)));
            }
        }
    }

    /// Override each instance's position with its spring value, so the tile
    /// slides from where it was toward its home cell. Works for both
    /// `TileInstance` and `IconInstance` via the [`SpringPos`] trait.
    fn apply_spring_positions<T: SpringPos>(&self, visible_ids: &[AppId], instances: &mut [T]) {
        for (id, inst) in visible_ids.iter().zip(instances.iter_mut()) {
            if let Some((_, spring)) = self
                .tile_springs
                .iter()
                .find(|(spring_id, _)| spring_id == id)
            {
                inst.set_pos(spring.x.value, spring.y.value);
            }
        }
    }

    /// While an edit-mode drag is in flight, move the dragged app's tile + icon
    /// to the end of the instance lists so it draws on top of everything else —
    /// but keep it as the *same* instance, not a duplicate. The shader uses
    /// `drag_pos` to make that trailing instance follow the pointer.
    fn lift_dragged_instances(
        &self,
        tile_instances: &mut Vec<grid::TileInstance>,
        icon_instances: &mut Vec<crate::icon_pipeline::IconInstance>,
        _visible_ids: &[AppId],
    ) {
        let is_drag = |flags: f32| (flags as u32 & grid::TileAnim::FLAG_DRAG) != 0;

        if let Some(pos) = tile_instances.iter().position(|t| is_drag(t.extra[3])) {
            let item = tile_instances.swap_remove(pos);
            tile_instances.push(item);
        }
        if let Some(pos) = icon_instances.iter().position(|i| is_drag(i.extra[3])) {
            let item = icon_instances.swap_remove(pos);
            icon_instances.push(item);
        }
    }

    /// Advance every tile position spring by `dt`. Returns `true` while any
    /// spring is still animating (so the caller keeps redrawing).
    fn step_tile_springs(&mut self, dt: f32) -> bool {
        let cfg = self.scroller.as_ref().map(|s| s.cfg).unwrap_or_default();
        let mut any = false;
        for (_, s) in &mut self.tile_springs {
            if s.step(dt, &cfg) {
                any = true;
            }
        }
        any
    }

    /// Rebuild + re-push the tile/icon instance buffers using the current
    /// spring positions, without recomputing the layout. Called every frame
    /// while the springs are animating so the slide is visible.
    fn refresh_spring_instances(&mut self) {
        let owned = self.grid_apps_owned();
        let apps: Vec<grid::GridApp<'_>> = owned
            .iter()
            .map(|(name, uv)| grid::GridApp {
                name: name.as_str(),
                uv: *uv,
            })
            .collect();
        let (w, _h) = self.viewport_phys();
        let visible_ids = self.visible_app_ids();
        let anim = self.edit_anim(&visible_ids);
        let mut tile_instances = self.layout.build_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut tile_instances);
        let mut icon_instances = self.layout.build_icon_instances(w as f32, &apps, &anim);
        self.apply_spring_positions(&visible_ids, &mut icon_instances);
        self.lift_dragged_instances(&mut tile_instances, &mut icon_instances, &visible_ids);
        if let Some(r) = self.renderer.as_mut() {
            r.set_tile_instances(&tile_instances);
            r.set_icon_instances(&icon_instances);
        }
    }

    /// Load the persisted user customization (drag-to-reorder result + hidden
    /// apps) into the registry. Called once at startup, before the first scan
    /// is ingested, so apps are placed in the user's arrangement from the first
    /// frame. A missing or corrupt store is a no-op (registry stays name-sorted
    /// with nothing hidden).
    fn load_customization(&mut self) {
        self.settings = self.cache.get_settings();
        let order = self.cache.get_app_order();
        if !order.is_empty() {
            self.registry.set_order(order);
        }
        let hidden = self.cache.get_hidden_ids();
        if !hidden.is_empty() {
            self.registry.set_hidden(hidden);
        }
    }

    /// Persist the current display order so it survives across launches. Called
    /// after a drag-to-reorder completes (and on hide). Cheap: one small blob
    /// upsert. Errors are logged but never panic the UI.
    fn persist_user_order(&self) {
        if let Err(e) = self.cache.put_app_order(self.registry.order()) {
            eprintln!("layout: failed to persist app order: {e}");
        }
    }

    /// Persist the current hidden-app list. Called after a hide/unhide change.
    fn persist_hidden(&self) {
        let ids: Vec<AppId> = self.registry.hidden().iter().cloned().collect();
        if let Err(e) = self.cache.put_hidden_ids(&ids) {
            eprintln!("layout: failed to persist hidden ids: {e}");
        }
    }

    fn persist_settings(&self) {
        if let Err(e) = self.cache.put_settings(&self.settings) {
            eprintln!("settings: failed to persist settings: {e}");
        }
    }

    /// Hide the launcher window and reset transient UI state (search field,
    /// scroll position, IME), but keep the process + event loop alive so it
    /// can be summoned again. Idempotent: a no-op if already hidden.
    fn hide(&mut self) {
        if !self.visible {
            debug_log!("hide: already hidden, no-op");
            return;
        }
        debug_log!("hide: hiding window");
        if let Some(r) = self.renderer.as_ref() {
            r.window.set_visible(false);
            r.window.set_ime_allowed(false);
        }
        // Exit edit mode if active, persisting any reorder before we vanish.
        if self.editing {
            self.exit_edit_mode();
        }
        // Close the settings overlay so a re-summon starts clean.
        self.settings_open = false;
        self.settings_panel_progress = 0.0;
        self.pending_press = None;
        // Drop any in-progress search / IME composition so the next summon
        // starts clean.
        self.control.press_close();
        self.relayout();
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

    /// Hide the launcher after a transparent-area click and, on Windows, send
    /// a best-effort replacement click to whatever is now under the cursor.
    fn hide_with_click_passthrough(&mut self) {
        self.hide();
        #[cfg(windows)]
        {
            if platform_windows::replay_left_click_at_cursor() {
                debug_log!("outside-click: replayed click to underlying window");
            } else {
                debug_log!("outside-click: failed to replay click to underlying window");
            }
        }
    }

    /// Show the launcher window and steal focus. Counterpart to [`hide`].
    /// Re-centers on the primary monitor so a multi-monitor move doesn't
    /// strand the launcher on the wrong screen.
    fn summon(&mut self) {
        let Some(r) = self.renderer.as_ref() else {
            return;
        };
        debug_log!("summon: showing window (visible was {})", self.visible);
        r.window.set_visible(true);
        // Steal focus. focus_window() can be silently denied by Windows when
        // the foreground already belongs to another app (common after hide()),
        // so we also allow-set-foreground + re-assert focus. If it still fails
        // the user at least sees the window appear (visible=true above) even
        // if it's not topmost.
        #[cfg(windows)]
        {
            // ASFW_ANY (-1) lifts the SetForegroundWindow restriction so any
            // process (incl. ours) can come to the front. This is what lets a
            // hotkey-triggered summon reliably raise the window instead of
            // just flashing the taskbar after the window was hidden.
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
                const ASFW_ANY: u32 = u32::MAX; // -1 as the Win32 ASFW_ANY sentinel
                let _ = AllowSetForegroundWindow(ASFW_ANY);
            }
        }
        r.window.focus_window();
        self.visible = true;
        // Record the summon time so a focus-transition artifact in the next
        // SUMMON_FOCUS_GRACE is ignored instead of instantly hiding us.
        self.last_summon = Some(Instant::now());
        self.request_redraw();
        debug_log!("summon: window shown + focus requested");
    }

    /// Handle a click (press + release inside the capsule with no drag) on the
    /// bottom control. Decides whether it hit the close (×) button, and
    /// otherwise toggles the search field open/closed.
    fn handle_control_click(&mut self, x: f32, y: f32) {
        // In edit mode the bottom control shows two capsules: [完了] on the
        // left and a settings gear [⚙] on the right. Decide which was clicked.
        if self.editing {
            let viewport = self.viewport_phys();
            let frame_bottom = self.frame_bottom_y();
            let scale = self.scale_factor;
            let done_hw = self
                .cached_done_width
                .map(|w| bottom_control::done_half_width(w, scale))
                .unwrap_or_else(|| bottom_control::done_half_width(0.0, scale));
            if let Some((gear, _)) = bottom_control::edit_gear_geometry(
                viewport,
                frame_bottom,
                scale,
                done_hw,
                self.edit_visual_progress(),
            ) {
                if bottom_control::edit_gear_hit(&gear, x, y) {
                    self.open_settings();
                    return;
                }
            }
            // Otherwise it's the Done capsule.
            self.exit_edit_mode();
            return;
        }
        let viewport = self.viewport_phys();
        let frame_bottom = self.frame_bottom_y();
        // Close-button hit region (only meaningful when the field is open).
        let close_x = self
            .control
            .close_button_x_scaled(viewport, frame_bottom, self.scale_factor);
        let hit_close = close_x
            .map(|cx| {
                let hit_radius = 12.0 * self.scale_factor.max(1.0);
                (x - cx).abs() <= hit_radius && (y - self.frame_control_cy()).abs() <= hit_radius
            })
            .unwrap_or(false);

        if hit_close {
            self.control.press_close();
            self.search_input_changed();
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
            self.request_redraw();
        }
    }

    /// The center Y of the control capsule (for hit-testing the close button).
    fn frame_control_cy(&self) -> f32 {
        self.resolve_control()
            .map(|(geom, _)| geom.center.1)
            .unwrap_or(0.0)
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

fn load_window_icon() -> Option<Icon> {
    let icon = app_icon::load_rgba(Some(256))?;
    Icon::from_rgba(icon.rgba, icon.width, icon.height).ok()
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
                debug_log!("user_event: Summon received (visible={})", self.visible);
                self.summon();
            }
            UserEvent::QuitRequested => {
                debug_log!("user_event: QuitRequested received → process::exit(0)");
                // Force-exit the process. We previously used event_loop.exit(),
                // but the debug log showed the os-integration thread (tray +
                // hook) kept the process alive for >1.8s after the call — the
                // tray was still clickable, so 'Quit' appeared to need two
                // clicks. A hard exit terminates all threads immediately; the
                // OS releases the LL hook and removes the tray icon on process
                // teardown, so no manual cleanup is needed.
                std::process::exit(0);
            }
            UserEvent::ToggleSettings => {
                // Tray "Settings": ensure the window is visible first so the
                // overlay is actually shown.
                if !self.visible {
                    self.summon();
                }
                self.toggle_settings();
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

        if let Some(icon) = load_window_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }

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
        self.layout = grid::GridLayout::default()
            .with_scale_factor(self.scale_factor)
            .centered(w as f32);

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

                let key_code = match event.physical_key {
                    winit::keyboard::PhysicalKey::Code(code) => Some(code),
                    winit::keyboard::PhysicalKey::Unidentified(_) => None,
                };

                // The settings overlay takes precedence over everything: Esc
                // closes it (doesn't hide the launcher), mirroring how edit mode
                // and the search field swallow Esc rather than quitting.
                if self.settings_open && key_code == Some(winit::keyboard::KeyCode::Escape) {
                    self.close_settings();
                    return;
                }

                // Edit mode takes precedence over everything except the search
                // field: Esc exits edit mode (doesn't hide), Enter/Done would
                // too. This branch sits before `wants_keyboard` so an open
                // search field still defers to edit-mode Esc.
                if self.editing && key_code == Some(winit::keyboard::KeyCode::Escape) {
                    self.exit_edit_mode();
                    return;
                }

                // While the search field has focus, the control eats most keys.
                if self.control.wants_keyboard() {
                    let handled = match key_code {
                        Some(winit::keyboard::KeyCode::Escape) => {
                            let c = self.control.wants_keyboard();
                            // If the field was open, Esc clears search and
                            // closes it instead of hiding the launcher.
                            if c {
                                self.control.press_close();
                                self.search_input_changed();
                                return;
                            }
                            false
                        }
                        Some(winit::keyboard::KeyCode::Backspace) => {
                            if self.control.preedit.is_empty() {
                                self.control.handle_backspace();
                                self.search_input_changed();
                            } else {
                                self.request_redraw();
                            }
                            true
                        }
                        Some(winit::keyboard::KeyCode::ArrowLeft) => {
                            if self.control.preedit.is_empty() {
                                self.control.handle_left();
                            }
                            self.request_redraw();
                            true
                        }
                        Some(winit::keyboard::KeyCode::ArrowRight) => {
                            if self.control.preedit.is_empty() {
                                self.control.handle_right();
                            }
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
                    if self.control.preedit.is_empty() {
                        if let Some(text) = &event.text {
                            if self.control.wants_keyboard() {
                                let mut any = false;
                                for ch in text.chars() {
                                    if self.control.handle_char(ch) {
                                        any = true;
                                    }
                                }
                                if any {
                                    self.search_input_changed();
                                    return;
                                }
                            }
                        }
                    }
                }

                if key_code == Some(winit::keyboard::KeyCode::Escape) {
                    // Esc with no open field: hide the launcher (stay resident).
                    self.hide();
                    return;
                }

                // M toggles the OS window frame on/off for easier debugging
                // (grab edges to resize, title bar to move) without rebuilding.
                if key_code == Some(winit::keyboard::KeyCode::KeyM) {
                    if let Some(r) = self.renderer.as_mut() {
                        r.toggle_decorations();
                        self.request_redraw();
                    }
                    return;
                }

                // R clears the icon cache and re-extracts every icon live, so
                // you can recover from a corrupted cache without restarting.
                if key_code == Some(winit::keyboard::KeyCode::KeyR)
                    && !self.control.wants_keyboard()
                {
                    self.reset_icons();
                    return;
                }

                if let (Some(r), Some(key_code)) = (self.renderer.as_mut(), key_code) {
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
                            self.search_input_changed();
                        }
                        Ime::Commit(text) => {
                            // IME commit: finalize the composition into the query.
                            self.control.set_preedit(String::new());
                            let mut any = false;
                            for ch in text.chars() {
                                if self.control.handle_char(ch) {
                                    any = true;
                                }
                            }
                            if any || !text.is_empty() {
                                self.search_input_changed();
                            }
                        }
                        Ime::Enabled => {}
                        Ime::Disabled => {
                            self.control.set_preedit(String::new());
                            self.search_input_changed();
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
                // An edit-mode drag whose pointer leaves the window is finalized
                // where it last was (iOS keeps the icon where you let go).
                if self.editing && self.drag_app.is_some() {
                    self.commit_reorder();
                    self.drag_app = None;
                    self.relayout();
                }
                // A pending long-press is cancelled when the pointer leaves.
                self.pending_press = None;
                // Drop a pending control press if the pointer leaves.
                self.pressed_on_control = false;
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer_phys_x = position.x as f32;
                self.pointer_phys_y = position.y as f32;
                // Edit-mode drag: follow the pointer and live-reorder.
                if self.editing && self.drag_app.is_some() {
                    self.handle_edit_drag_move();
                    return;
                }
                // A pending press may promote to a real scroll drag once it
                // moves past slop. If it does, the scroller is now Dragging and
                // the move has already been applied.
                if self.pending_press.is_some() && self.maybe_promote_press_to_drag() {
                    return;
                }
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
                        // While the settings overlay is open, presses are
                        // consumed by the overlay: an inside-panel click may
                        // hit the close button, an outside click closes the
                        // overlay. No grid interaction is possible underneath.
                        if self.settings_open {
                            self.pressed_on_settings = Some(
                                self.settings_hit_target(self.pointer_phys_x, self.pointer_phys_y),
                            );
                            // Always swallow: outside clicks are handled on
                            // release (close), inside clicks may hit the ×.
                            return;
                        }
                        // If the press starts on the control capsule (or, in
                        // edit mode, the adjacent settings gear capsule), mark
                        // it so the release is treated as a control click and
                        // NOT as a scroll drag.
                        let mut over_control = self.control.hit_test_scaled(
                            self.viewport_phys(),
                            self.frame_bottom_y(),
                            self.pointer_phys_x,
                            self.pointer_phys_y,
                            self.scale_factor,
                        );
                        // In edit mode the gear capsule sits beside the Done
                        // capsule but is not part of the control shape, so hit
                        // it explicitly.
                        if !over_control && self.editing {
                            let scale = self.scale_factor;
                            let done_hw = self
                                .cached_done_width
                                .map(|w| bottom_control::done_half_width(w, scale))
                                .unwrap_or_else(|| bottom_control::done_half_width(0.0, scale));
                            if let Some((gear, _)) = bottom_control::edit_gear_geometry(
                                self.viewport_phys(),
                                self.frame_bottom_y(),
                                scale,
                                done_hw,
                                self.edit_visual_progress(),
                            ) {
                                over_control = bottom_control::edit_gear_hit(
                                    &gear,
                                    self.pointer_phys_x,
                                    self.pointer_phys_y,
                                );
                            }
                        }
                        self.pressed_on_control = over_control;
                        if over_control {
                            return;
                        }
                        // Edit mode: clicking an icon lifts it into a drag;
                        // clicking its ✕ badge hides it; clicking empty space
                        // exits edit mode (outside click).
                        if self.editing {
                            let px = self.pointer_phys_x;
                            let py = self.pointer_phys_y;
                            let idx = self.app_index_at_pointer(px, py);
                            if let Some(idx) = idx {
                                let visible = self.visible_app_ids();
                                if let Some(id) = visible.get(idx).cloned() {
                                    debug_log!("edit-drag: press idx={idx}");
                                    // ✕ badge hit takes precedence over a drag.
                                    if self.badge_hit(idx, px, py) {
                                        self.hide_app(&id);
                                        return;
                                    }
                                    self.drag_app = Some(id);
                                    self.drag_x = px;
                                    self.drag_y = py;
                                    self.relayout();
                                    self.request_redraw();
                                    return;
                                }
                            }
                            // Empty space → exit edit mode (and persist).
                            self.exit_edit_mode();
                            return;
                        }
                        // Normal mode: defer the scroll drag until the gesture
                        // resolves (drag past slop, or quick release, or long-
                        // press into edit mode).
                        self.begin_grid_press(Instant::now());
                    }
                    ElementState::Released => {
                        // Settings overlay open: handle close-button + outside
                        // clicks. Nothing underneath is reachable.
                        if self.settings_open {
                            let pressed = self.pressed_on_settings.take();
                            let px = self.pointer_phys_x;
                            let py = self.pointer_phys_y;
                            let released = self.settings_hit_target(px, py);
                            if pressed == Some(SettingsPressTarget::Outside)
                                && released == SettingsPressTarget::Outside
                            {
                                self.close_settings();
                                return;
                            }
                            if pressed == Some(released) {
                                self.handle_settings_click(released);
                            } else if pressed == Some(SettingsPressTarget::Outside)
                                && released == SettingsPressTarget::Outside
                            {
                                // Outside the panel → dismiss (like a modal).
                                self.close_settings();
                            }
                            return;
                        }
                        if self.pressed_on_control {
                            self.pressed_on_control = false;
                            // Only count as a click if it stayed on the capsule.
                            if self.control.hit_test_scaled(
                                self.viewport_phys(),
                                self.frame_bottom_y(),
                                self.pointer_phys_x,
                                self.pointer_phys_y,
                                self.scale_factor,
                            ) {
                                self.handle_control_click(self.pointer_phys_x, self.pointer_phys_y);
                            }
                            return;
                        }
                        // Edit-mode drag release: drop the icon here and persist.
                        if self.editing && self.drag_app.is_some() {
                            self.commit_reorder();
                            self.drag_app = None;
                            self.relayout();
                            self.request_redraw();
                            return;
                        }
                        // A pending press that released without dragging and
                        // without a long-press is a click → launch the app.
                        if let Some(press) = self.pending_press.take() {
                            if pending_press_is_outside_glass_click(
                                &press,
                                self.pointer_phys_x,
                                self.pointer_phys_y,
                            ) {
                                self.hide_with_click_passthrough();
                                return;
                            }
                            if let Some(app) = pending_press_launch_id(
                                &press,
                                self.pointer_phys_x,
                                self.pointer_phys_y,
                            )
                            .and_then(|id| self.registry.launch_info(id))
                            {
                                let link_path = app.link_path.clone();
                                let name = app.name.clone();
                                self.hide();
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
                            return;
                        }
                        if let Some(app) = self.handle_pointer_release() {
                            // Dismiss first, launch second. `hide()` hands the
                            // hide straight to the DWM (a few ms) and resets the
                            // UI, while `ShellExecuteW` resolves the shortcut and
                            // spawns the target (tens to hundreds of ms). Doing
                            // them in this order makes the launcher feel like it
                            // vanishes the instant you click, instead of freezing
                            // on screen until the target app starts.
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
                let auto_scroll_started = self.maybe_autoscroll_edit_drag();
                if self.editing && self.drag_app.is_some() {
                    self.live_reorder();
                }

                // Advance the wiggle animation phase while editing. dt is taken
                // from the redraw cadence (clamped like the control's).
                let anim_dt = match self.last_redraw {
                    Some(prev) => now.duration_since(prev).as_secs_f32().min(0.1),
                    None => 1.0 / 60.0,
                };
                if self.editing {
                    self.wiggle_phase += anim_dt;
                }

                // Advance the per-tile position springs and re-push the
                // instance buffers if any are still sliding (reorder animation).
                let springs_animating = self.step_tile_springs(anim_dt);
                if springs_animating {
                    self.refresh_spring_instances();
                }

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
                let edit_control_animating = self.step_edit_control_width(control_dt);
                let settings_animating = self.step_settings_panel(control_dt);

                // Upload the control's capsule + overlays before the render.
                // This also measures query + preedit width for the IME cursor.
                self.render_bottom_control();
                // Upload the corner gear capsule + glyph (if shown).
                self.render_gear();
                // Upload the settings overlay panel (if open).
                self.render_settings_panel();

                // Sync the OS IME with the search field (on while focused,
                // parked at the caret) so Japanese / other IME input works.
                self.update_ime_state();

                // Render the frame (consumes the uploaded buffers).
                if let Some(r) = self.renderer.as_mut() {
                    r.render(&DrawArgs {
                        scroll_x,
                        viewport: vp,
                        defer_backdrop_capture: dragging,
                        time: self.wiggle_phase,
                        drag_active: if self.drag_app.is_some() { 1.0 } else { 0.0 },
                        drag_pos: (self.drag_x, self.drag_y),
                    });
                }

                if !self.first_frame_rendered {
                    self.first_frame_rendered = true;
                    self.timer.mark(prefix::STARTUP, "first frame rendered");
                }
                if scroller_animating
                    || auto_scroll_started
                    || control_animating
                    || edit_control_animating
                    || settings_animating
                    || springs_animating
                    || self.editing
                {
                    self.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                debug_log!("window_event: Focused({})", focused);
                // Auto-hide when the launcher loses focus (clicking another
                // window, Alt-Tab, …). This is the macOS-Launchpad / Run-dialog
                // behavior. `hide()` is idempotent so the focus-loss that fires
                // right after we hide to launch an app is a harmless no-op.
                //
                // BUT: ignore a focus loss that happens within
                // SUMMON_FOCUS_GRACE of a summon. SetForegroundWindow can
                // briefly drop and re-acquire focus as the OS shuffles
                // windows, and without this guard the just-summoned launcher
                // would vanish within ~75ms on some machines.
                if !focused {
                    let in_grace = self
                        .last_summon
                        .map(|t| t.elapsed() < SUMMON_FOCUS_GRACE)
                        .unwrap_or(false);
                    // While editing we don't auto-hide on focus loss: clicking
                    // outside the launcher to dismiss edit mode would itself
                    // blur the window, and we want to exit edit mode cleanly
                    // (persisting the reorder) rather than vanish mid-edit.
                    // The settings overlay gets the same treatment so it isn't
                    // dismissed by a momentary focus shuffle.
                    if self.editing {
                        debug_log!("window_event: Focused(false) ignored (editing)");
                    } else if self.settings_panel_active() {
                        debug_log!("window_event: Focused(false) ignored (settings open)");
                    } else if in_grace {
                        debug_log!("window_event: Focused(false) ignored (within summon grace)");
                    } else {
                        self.hide();
                    }
                }
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

        // Long-press timer: if a press is still pending, keep redrawing so we
        // notice when it crosses LONG_PRESS_THRESHOLD and enter edit mode.
        let long_press_pending = self.pending_press.is_some();
        if long_press_pending {
            self.maybe_long_press_into_edit(Instant::now());
        }

        // Edit mode keeps redrawing so the wiggle animation advances and the
        // dragged tile tracks the pointer smoothly.
        if scroller_animating || control_animating || self.editing || long_press_pending {
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

/// Trait for instance types that carry an `(x, y)` position we can rewrite in
/// place — used by the reorder animation to override a tile/icon's home cell
/// with its spring value.
trait SpringPos {
    fn set_pos(&mut self, x: f32, y: f32);
}

impl SpringPos for grid::TileInstance {
    fn set_pos(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
    }
}

impl SpringPos for crate::icon_pipeline::IconInstance {
    fn set_pos(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
    }
}

// Shared font metrics for the bottom-control text (label / query / placeholder
// / preedit), so measuring and drawing use identical shaping parameters.
const QUERY_LABEL_FONT: &str = "Yu Gothic UI";
const QUERY_LABEL_SIZE: f32 = 13.0;
const QUERY_LABEL_LINE: f32 = 18.0;
const DONE_LABEL: &str = "完了";

// ---- settings overlay (placeholder panel) ----------------------------------

/// Title shown in the placeholder settings panel.
const SETTINGS_TITLE: &str = "設定";
/// Title font for the settings panel.
const SETTINGS_TITLE_FONT: &str = "Yu Gothic UI";
const SETTINGS_SIDEBAR_W: f32 = 210.0;
const SETTINGS_SIDEBAR_TOP: f32 = 78.0;
const SETTINGS_SIDEBAR_ROW_H: f32 = 38.0;
const SETTINGS_SIDEBAR_STEP: f32 = 44.0;
const SETTINGS_CONTENT_PAD: f32 = 34.0;
const SETTINGS_CONTENT_TOP: f32 = 92.0;
const SETTINGS_ROW_H: f32 = 46.0;
const SETTINGS_ROW_STEP: f32 = 62.0;
const SETTINGS_SEGMENT_H: f32 = 32.0;
const SETTINGS_SEGMENT_GAP: f32 = 8.0;
const SETTINGS_INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
const SETTINGS_MUTED: [f32; 4] = [1.0, 1.0, 1.0, 0.58];
const SETTINGS_DIM: [f32; 4] = [1.0, 1.0, 1.0, 0.34];
const SETTINGS_ACCENT: [f32; 4] = [0.35, 0.68, 1.0, 0.42];
const SETTINGS_GREEN: [f32; 4] = [0.28, 0.82, 0.48, 0.78];

fn transform_settings_instances(
    instances: &mut [bottom_control::ControlInstance],
    origin: [f32; 2],
    scale: f32,
    alpha: f32,
) {
    for instance in instances {
        instance.center[0] = origin[0] + (instance.center[0] - origin[0]) * scale;
        instance.center[1] = origin[1] + (instance.center[1] - origin[1]) * scale;
        instance.params[0] *= scale;
        instance.params[2] *= scale;
        instance.params[3] *= scale;
        instance.params[1] *= alpha;
        instance.color[3] *= alpha;
    }
}

fn transform_settings_quads(
    quads: &mut [text::GlyphQuad],
    origin: [f32; 2],
    scale: f32,
    alpha: f32,
) {
    for quad in quads {
        quad.x = origin[0] + (quad.x - origin[0]) * scale;
        quad.y = origin[1] + (quad.y - origin[1]) * scale;
        quad.w *= scale;
        quad.h *= scale;
        quad.color[3] *= alpha;
    }
}

fn control_icon(
    x: f32,
    y: f32,
    radius: f32,
    kind: f32,
    color: [f32; 4],
) -> bottom_control::ControlInstance {
    bottom_control::ControlInstance {
        center: [x, y],
        params: [radius, color[3], 1.6, 0.0],
        color,
        kind: [kind, 0.0, 0.0, 0.0],
    }
}

fn round_rect_instance(
    center: [f32; 2],
    half_width: f32,
    half_height: f32,
    radius: f32,
    color: [f32; 4],
) -> bottom_control::ControlInstance {
    bottom_control::ControlInstance {
        center,
        params: [half_height, color[3], half_width, radius],
        color,
        kind: [bottom_control::KIND_ROUND_RECT, 0.0, 0.0, 0.0],
    }
}

fn divider_instance(
    center: [f32; 2],
    half_width: f32,
    half_height: f32,
) -> bottom_control::ControlInstance {
    round_rect_instance(center, half_width, half_height, half_height, SETTINGS_DIM)
}

fn toggle_instances(
    center: [f32; 2],
    enabled: bool,
    scale: f32,
    instances: &mut Vec<bottom_control::ControlInstance>,
) {
    let track_hw = 22.0 * scale;
    let track_hh = 11.0 * scale;
    let track_color = if enabled {
        SETTINGS_GREEN
    } else {
        [1.0, 1.0, 1.0, 0.14]
    };
    instances.push(round_rect_instance(
        center,
        track_hw,
        track_hh,
        track_hh,
        track_color,
    ));
    if enabled {
        instances.push(control_icon(
            center[0] + 10.0 * scale,
            center[1],
            6.0 * scale,
            bottom_control::KIND_DOT,
            [1.0, 1.0, 1.0, 0.78],
        ));
    }
}

fn build_settings_panel_instances(
    layout: &SettingsPanelLayout,
    scale: f32,
    category: SettingsCategory,
    settings: &Settings,
    _hidden_count: usize,
    instances: &mut Vec<bottom_control::ControlInstance>,
) {
    let panel_right = layout.left + layout.hw * 2.0;
    let panel_bottom = layout.top + layout.hh * 2.0;
    instances.push(divider_instance(
        [layout.right_left, layout.cy],
        0.55 * scale,
        layout.hh - 28.0 * scale,
    ));

    for (i, item) in SettingsCategory::ALL.iter().copied().enumerate() {
        if item == category {
            let row_top = layout.top
                + SETTINGS_SIDEBAR_TOP * scale
                + i as f32 * SETTINGS_SIDEBAR_STEP * scale;
            instances.push(round_rect_instance(
                [
                    layout.left + layout.sidebar_w * 0.5,
                    row_top + SETTINGS_SIDEBAR_ROW_H * scale * 0.5,
                ],
                layout.sidebar_w * 0.5 - 14.0 * scale,
                SETTINGS_SIDEBAR_ROW_H * scale * 0.5,
                10.0 * scale,
                SETTINGS_ACCENT,
            ));
        }
    }

    let content_left = layout.right_left + SETTINGS_CONTENT_PAD * scale;
    let content_right = panel_right - SETTINGS_CONTENT_PAD * scale;
    let row_w = content_right - content_left;
    let row_h = SETTINGS_ROW_H * scale;
    let first_top = layout.top + SETTINGS_CONTENT_TOP * scale;

    match category {
        SettingsCategory::Apps => {
            let segment_top = first_top + 44.0 * scale;
            let gap = SETTINGS_SEGMENT_GAP * scale;
            let each_w = (row_w - gap * 3.0) / 4.0;
            for (i, order) in SortOrder::ALL.iter().copied().enumerate() {
                let left = content_left + i as f32 * (each_w + gap);
                let selected = settings.sort_order == order;
                instances.push(round_rect_instance(
                    [
                        left + each_w * 0.5,
                        segment_top + SETTINGS_SEGMENT_H * scale * 0.5,
                    ],
                    each_w * 0.5,
                    SETTINGS_SEGMENT_H * scale * 0.5,
                    10.0 * scale,
                    if selected {
                        SETTINGS_ACCENT
                    } else {
                        [1.0, 1.0, 1.0, 0.14]
                    },
                ));
                if selected {
                    instances.push(control_icon(
                        left + 15.0 * scale,
                        segment_top + SETTINGS_SEGMENT_H * scale * 0.5,
                        8.0 * scale,
                        bottom_control::KIND_CHECK,
                        SETTINGS_INK,
                    ));
                }
            }
            settings_row_backgrounds(
                content_left,
                first_top + SETTINGS_ROW_STEP * scale,
                row_w,
                row_h,
                scale,
                instances,
                2,
            );
            toggle_instances(
                [
                    content_right - 28.0 * scale,
                    first_top + SETTINGS_ROW_STEP * scale + row_h * 0.5,
                ],
                settings.frequent_apps_enabled,
                scale,
                instances,
            );
            instances.push(control_icon(
                content_right - 14.0 * scale,
                first_top + SETTINGS_ROW_STEP * 2.0 * scale + row_h * 0.5,
                9.0 * scale,
                bottom_control::KIND_CHEVRON,
                SETTINGS_MUTED,
            ));
        }
        SettingsCategory::Search => {
            settings_row_backgrounds(content_left, first_top, row_w, row_h, scale, instances, 1);
            toggle_instances(
                [content_right - 28.0 * scale, first_top + row_h * 0.5],
                settings.search_includes_hidden,
                scale,
                instances,
            );
        }
        SettingsCategory::System => {
            settings_row_backgrounds(content_left, first_top, row_w, row_h, scale, instances, 2);
            for i in 0..2 {
                instances.push(control_icon(
                    content_right - 14.0 * scale,
                    first_top + i as f32 * SETTINGS_ROW_STEP * scale + row_h * 0.5,
                    9.0 * scale,
                    bottom_control::KIND_CHEVRON,
                    SETTINGS_MUTED,
                ));
            }
        }
        SettingsCategory::About => {
            settings_row_backgrounds(content_left, first_top, row_w, row_h, scale, instances, 1);
        }
    }

    instances.push(divider_instance(
        [layout.cx, panel_bottom - 56.0 * scale],
        layout.hw - 26.0 * scale,
        0.45 * scale,
    ));
}

fn settings_row_backgrounds(
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    scale: f32,
    instances: &mut Vec<bottom_control::ControlInstance>,
    count: usize,
) {
    for i in 0..count {
        instances.push(round_rect_instance(
            [
                left + width * 0.5,
                top + i as f32 * SETTINGS_ROW_STEP * scale + height * 0.5,
            ],
            width * 0.5,
            height * 0.5,
            12.0 * scale,
            [1.0, 1.0, 1.0, 0.12],
        ));
    }
}

fn build_settings_panel_text_views(
    t: &mut text::TextRenderer,
    views: &[ui_model::text::TextView],
    scale: f32,
    quads: &mut Vec<text::GlyphQuad>,
) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    for view in views {
        let color = [
            view.style.color.r,
            view.style.color.g,
            view.style.color.b,
            view.style.color.a,
        ];
        let center_y = view.rect.center().y;
        let line_height = view.rect.height / scale;
        match view.style.align {
            ui_model::text::TextAlign::Start => push_text_left(
                t,
                quads,
                &view.text,
                view.rect.x,
                center_y,
                view.style.size,
                line_height,
                color,
                scale,
            ),
            ui_model::text::TextAlign::End => push_text_right(
                t,
                quads,
                &view.text,
                view.rect.x,
                center_y,
                view.style.size,
                line_height,
                color,
                scale,
            ),
            ui_model::text::TextAlign::Center => {
                quads.append(&mut t.layout_centered_line(&text::CenteredLineSpec {
                    text: &view.text,
                    font_size: view.style.size,
                    line_height,
                    family: SETTINGS_TITLE_FONT,
                    color,
                    center: (view.rect.center().x, center_y),
                    scale_factor: scale,
                }));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_text_left(
    t: &mut text::TextRenderer,
    quads: &mut Vec<text::GlyphQuad>,
    value: &str,
    left: f32,
    center_y: f32,
    font_size: f32,
    line_height: f32,
    color: [f32; 4],
    scale: f32,
) {
    let width = t.measure_text(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (0.0, 0.0),
        scale_factor: scale,
    });
    quads.append(&mut t.layout_centered_line(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (left + width * 0.5, center_y),
        scale_factor: scale,
    }));
}

#[allow(clippy::too_many_arguments)]
fn push_text_right(
    t: &mut text::TextRenderer,
    quads: &mut Vec<text::GlyphQuad>,
    value: &str,
    right: f32,
    center_y: f32,
    font_size: f32,
    line_height: f32,
    color: [f32; 4],
    scale: f32,
) {
    let width = t.measure_text(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (0.0, 0.0),
        scale_factor: scale,
    });
    quads.append(&mut t.layout_centered_line(&text::CenteredLineSpec {
        text: value,
        font_size,
        line_height,
        family: SETTINGS_TITLE_FONT,
        color,
        center: (right - width * 0.5, center_y),
        scale_factor: scale,
    }));
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
    edit_visual_progress: f32,
) -> Vec<text::GlyphQuad> {
    let mut quads = Vec::new();
    const INK: [f32; 4] = [1.0, 1.0, 1.0, 0.92];
    const PLACEHOLDER: [f32; 4] = [1.0, 1.0, 1.0, 0.45];
    /// Preedit (in-flight IME composition) is shown slightly dimmer to hint
    /// it isn't committed yet.
    const PREEDIT_INK: [f32; 4] = [0.85, 0.92, 1.0, 0.88];

    // While edit-mode width is morphing, keep the Done label centered and skip
    // normal pill/indicator/field content so it cannot overflow the capsule.
    if edit_visual_progress > 0.0 {
        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
            text: DONE_LABEL,
            font_size: QUERY_LABEL_SIZE,
            line_height: QUERY_LABEL_LINE,
            family: QUERY_LABEL_FONT,
            color: mul_alpha(INK, edit_visual_progress.clamp(0.0, 1.0)),
            center: (geom.center.0, geom.center.1),
            scale_factor: scale,
        });
        quads.append(&mut q);
        return quads;
    }

    for layer in layers {
        let a = layer.alpha;
        if a <= 0.01 {
            continue;
        }
        match layer.visual {
            bottom_control::Visual::SearchPill => {
                // "検索" label to the right of the magnifier.
                let (_, label_center_x) = bottom_control::search_pill_content_centers(geom);
                let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                    text: "検索",
                    font_size: QUERY_LABEL_SIZE,
                    line_height: QUERY_LABEL_LINE,
                    family: QUERY_LABEL_FONT,
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
                        font_size: QUERY_LABEL_SIZE,
                        line_height: QUERY_LABEL_LINE,
                        family: QUERY_LABEL_FONT,
                        color: mul_alpha(PLACEHOLDER, a),
                        center: (origin_x + 14.0 * scale, geom.center.1),
                        scale_factor: scale,
                    });
                    quads.append(&mut q);
                } else {
                    // Render the committed query plus the in-flight IME
                    // preedit inline. The preedit is shown with an
                    // underline tint so the user can tell it's not yet
                    // committed. Widths are measured exactly (same shaping as
                    // drawing) so the caret / preedit line up with the glyphs.
                    let query_w = if control.query.is_empty() {
                        0.0
                    } else {
                        t.measure_text(&text::CenteredLineSpec {
                            text: &control.query,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: INK,
                            center: (0.0, 0.0),
                            scale_factor: scale,
                        })
                    };
                    let preedit_w = if control.preedit.is_empty() {
                        0.0
                    } else {
                        t.measure_text(&text::CenteredLineSpec {
                            text: &control.preedit,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: PREEDIT_INK,
                            center: (0.0, 0.0),
                            scale_factor: scale,
                        })
                    };
                    // Committed text: left-anchored at origin_x, so center on
                    // its own half-width.
                    if query_w > 0.0 {
                        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                            text: &control.query,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: mul_alpha(INK, a),
                            center: (origin_x + query_w * 0.5, geom.center.1),
                            scale_factor: scale,
                        });
                        quads.append(&mut q);
                    }
                    // Preedit, starting right after the committed query.
                    if preedit_w > 0.0 {
                        let preedit_origin = origin_x + query_w;
                        let mut q = t.layout_centered_line(&text::CenteredLineSpec {
                            text: &control.preedit,
                            font_size: QUERY_LABEL_SIZE,
                            line_height: QUERY_LABEL_LINE,
                            family: QUERY_LABEL_FONT,
                            color: mul_alpha(PREEDIT_INK, a),
                            center: (preedit_origin + preedit_w * 0.5, geom.center.1),
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

fn advance_unit_toward(v: f32, target: f32, dt: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return target;
    }
    let dir = if target >= v { 1.0 } else { -1.0 };
    let next = v + dir * dt.max(0.0) / duration;
    if dir > 0.0 {
        next.min(target)
    } else {
        next.max(target)
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
    #[cfg(windows)]
    let _single_instance = match platform_windows::SingleInstanceGuard::acquire() {
        Ok(guard) => guard,
        Err(e) if e.is_already_running() => {
            crate::debug_log!("single-instance: existing instance signaled");
            return;
        }
        Err(e) => {
            eprintln!("single-instance: {e}");
            std::process::exit(1);
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
    // Restore the user's saved layout (drag-to-reorder + hidden apps) before the
    // first scan lands, so apps appear in the user's arrangement from frame one.
    app.load_customization();
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
mod pending_press_tests {
    use std::time::Instant;

    use super::{
        pending_press_is_outside_glass_click, pending_press_launch_id, PendingPress,
        CLICK_SLOP_PHYS,
    };
    use crate::app_id::AppId;

    fn press(app_index: Option<usize>, app_id: Option<AppId>, outside_glass: bool) -> PendingPress {
        PendingPress {
            start: Instant::now(),
            x: 100.0,
            y: 100.0,
            app_index,
            app_id,
            outside_glass,
        }
    }

    #[test]
    fn launches_only_pressed_app_id() {
        let pressed = AppId::from_normalized("pressed-app");
        let press = press(Some(3), Some(pressed.clone()), false);

        assert_eq!(
            pending_press_launch_id(&press, 103.0, 102.0),
            Some(&pressed)
        );
    }

    #[test]
    fn press_without_app_never_launches_release_target() {
        let press = press(None, None, false);

        assert_eq!(pending_press_launch_id(&press, 103.0, 102.0), None);
    }

    #[test]
    fn movement_past_click_slop_does_not_launch() {
        let press = press(Some(0), Some(AppId::from_normalized("pressed-app")), false);

        assert_eq!(
            pending_press_launch_id(&press, 100.0 + CLICK_SLOP_PHYS + 1.0, 100.0),
            None
        );
    }

    #[test]
    fn outside_glass_stationary_click_dismisses() {
        let press = press(None, None, true);

        assert!(pending_press_is_outside_glass_click(&press, 102.0, 101.0));
    }

    #[test]
    fn outside_glass_drag_does_not_dismiss() {
        let press = press(None, None, true);

        assert!(!pending_press_is_outside_glass_click(
            &press,
            100.0 + CLICK_SLOP_PHYS + 1.0,
            100.0
        ));
    }

    #[test]
    fn inside_glass_empty_click_does_not_dismiss() {
        let press = press(None, None, false);

        assert!(!pending_press_is_outside_glass_click(&press, 102.0, 101.0));
    }
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
