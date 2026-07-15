//! App shell state: the `App` struct, its constructor, runtime value types,
//! and pure (read-only) accessors.
//!
//! This module owns the durable runtime state that the handler, update,
//! command, frame, and render modules coordinate around. The struct fields and
//! constructor are moved here verbatim from the historical `main.rs::App` so
//! the move is mechanical and behavior-preserving. The pure accessors
//! (`viewport_phys`, `current_page`, `visible_app_ids`, …) read state without
//! mutating it and are grouped here so the input layer and tests can reason
//! about them deterministically.
//!
//! State-mutating methods live in [`super::update`], side-effect execution in
//! [`super::command`], per-frame work in [`super::frame`], and renderer/text
//! adapters in [`super::render`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::domain::app_diff::SnapshotEntry;
use crate::domain::app_id::AppId;
use crate::domain::app_registry::AppRegistry;
use crate::domain::launcher_item::LauncherItem;
use crate::domain::launcher_state::LauncherState;
use crate::domain::settings::{Settings, SettingsCategory, SortOrder};
use crate::icon_cache::IconCache;
use crate::renderer::icon_atlas::IconAtlas;
use crate::renderer::Renderer;
use crate::scroll::Scroller;
use crate::startup_timer::StartupTimer;
use crate::workers::icon_worker::WorkerHandle;
use crate::workers::refresh_watcher::RefreshMessage;
use winit::event_loop::EventLoopProxy;

use crate::app::event::UserEvent;

/// Press slop (physical px). A press that moves more than this is not a click
/// and not a long-press (it becomes a scroll drag).
pub const CLICK_SLOP_PHYS: f32 = 8.0;
pub const INITIAL_WINDOW_WIDTH: f64 = 1280.0;
pub const INITIAL_WINDOW_HEIGHT: f64 = 800.0;
pub const MIN_WINDOW_WIDTH: f64 = 640.0;
pub const MIN_WINDOW_HEIGHT: f64 = 480.0;
/// Grace window after `summon()` during which a `Focused(false)` is treated as
/// a focus-transition artifact and ignored (SetForegroundWindow can briefly
/// drop and re-acquire focus as the OS shuffles windows). Without this the
/// just-summoned launcher would instantly hide again on some machines.
pub const SUMMON_FOCUS_GRACE: Duration = Duration::from_millis(500);
/// How long a press must be held (without dragging past `CLICK_SLOP_PHYS`) to
/// enter edit mode. iOS home-screen long-press is ~450–500 ms; we use 500 ms to
/// avoid accidental triggers during a slow scroll.
pub const LONG_PRESS_THRESHOLD: Duration = Duration::from_millis(500);
/// While dragging an icon in edit mode, holding it this close to the page-frame
/// edge starts a one-page autoscroll. Re-declared in
/// [`crate::layout::edit_mode::EDIT_EDGE_SCROLL_ZONE`] as the source of truth
/// for the pure zone computation; kept here for the historical doc
/// cross-reference.
pub const EDIT_EDGE_SCROLL_ZONE: f32 = crate::layout::edit_mode::EDIT_EDGE_SCROLL_ZONE;

/// A grid press that hasn't yet been classified as a scroll drag, a click, or
/// a long-press into edit mode. While present, the scroller is *not* in
/// `Dragging` — we hold off until the gesture reveals its intent.
#[derive(Debug, Clone)]
pub struct PendingPress {
    /// When the press started (for long-press timing).
    pub start: Instant,
    /// Pointer position at press start (physical px).
    pub x: f32,
    pub y: f32,
    /// The app under the pointer at press start, if any. Entering edit mode
    /// lifts this app into a drag immediately.
    pub item_index: Option<usize>,
    /// Stable id of the app under the pointer at press start. Quick release
    /// launches this id, not whatever happens to be under the release point.
    pub item: Option<LauncherItem>,
    /// True when the press started outside the page-frame Liquid Glass. A
    /// stationary release there dismisses the launcher instead of interacting
    /// with the grid.
    pub outside_glass: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPressTarget {
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

impl PendingPress {
    /// True if `(release - start)` is within the click slop radius. A quick
    /// stationary release is a click; a drag past slop is not. Mirrors the
    /// historical `main.rs::pending_press_is_click`.
    pub(crate) fn is_click(&self, release_x: f32, release_y: f32) -> bool {
        let dx = release_x - self.x;
        let dy = release_y - self.y;
        dx * dx + dy * dy <= CLICK_SLOP_PHYS * CLICK_SLOP_PHYS
    }

    /// The press-time app id if the release is a click (within slop), else
    /// `None`. Launch uses the press-time id, not whatever moved under the
    /// release point. Mirrors `main.rs::pending_press_launch_id`.
    pub(crate) fn activated_item(&self, release_x: f32, release_y: f32) -> Option<&LauncherItem> {
        if !self.is_click(release_x, release_y) {
            return None;
        }
        self.item.as_ref()
    }

    /// True if the press started outside the page-frame glass and the release
    /// is a click. Such a release dismisses the launcher and replays the click
    /// to the underlying window. Mirrors `main.rs::pending_press_is_outside_glass_click`.
    pub(crate) fn is_outside_glass_click(&self, release_x: f32, release_y: f32) -> bool {
        self.outside_glass && self.is_click(release_x, release_y)
    }
}

/// Shared inbox the worker + watcher push into; the event loop drains it on
/// each wakeup via a `UserEvent` sentinel that just means "poll the inbox".
/// Using one mpsc + a single winit user event keeps proxy wiring simple even
/// though we have two background threads.
pub type Inbox = Mutex<Vec<WorkerMessage>>;

#[derive(Debug)]
pub enum WorkerMessage {
    Icon(crate::workers::icon_worker::IconResult),
    Refresh(RefreshMessage),
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedGridItem {
    pub item: LauncherItem,
    pub key: String,
    pub name: String,
    pub uv: Option<crate::icons::UvRect>,
    /// Ordered first-page preview slots. `None` retains an undiscovered
    /// child's position without drawing it.
    pub preview_uvs: Vec<Option<crate::icons::UvRect>>,
}

/// Owns the renderer (which owns the window) plus all app/icon state.
pub struct App {
    pub event_proxy: EventLoopProxy<UserEvent>,
    pub renderer: Option<Renderer>,
    pub scroller: Option<Scroller>,
    pub text: Option<crate::renderer::text_engine::TextRenderer>,
    pub layout: crate::grid::GridLayout,
    pub render_model: crate::ui_model::render_model::RenderModel,
    pub timer: StartupTimer,
    pub qa_runner: Option<crate::qa::QaRunner>,

    // ---- app + icon state ----
    pub registry: AppRegistry,
    /// User-owned launcher layout: top-level item order, folders, and hidden
    /// apps. Separated from `registry` so the user's arrangement survives app
    /// add/remove/re-detect cycles without corruption. Drives the visible grid
    /// order; the registry is now only the discovered-app dataset.
    pub launcher_state: LauncherState,
    /// CPU-side fixed-slot atlas; the GPU texture mirrors it.
    pub atlas: IconAtlas,
    /// True once the atlas texture has been allocated+uploaded at least once.
    pub atlas_uploaded: bool,
    /// Most recent Start Menu snapshot (for diffing on the UI thread when an
    /// inline scan is needed; the watcher also keeps its own).
    pub snapshot: BTreeMap<AppId, SnapshotEntry>,

    // ---- background plumbing ----
    pub cache: Arc<IconCache>,
    pub inbox: Arc<Inbox>,
    /// Kept to keep the worker thread alive; requests go through it.
    pub _worker: Option<WorkerHandle>,

    // ---- input ----
    pub scale_factor: f32,
    pub pointer_phys_x: f32,
    pub pointer_phys_y: f32,
    pub drag_start_x: f32,
    pub drag_start_y: f32,
    pub first_frame_rendered: bool,

    // ---- edit mode (iOS-style drag-to-reorder) ----
    /// True while the launcher is in the wiggling, reorderable state. Entered
    /// via long-press on an icon; exited via Esc / outside click / Done.
    pub editing: bool,
    /// A press is currently held down on the grid (not on the control) and we
    /// haven't yet decided whether it's a scroll drag, a click, or a long-press
    /// into edit mode. `Some` holds the press start time + pointer + app id.
    pub pending_press: Option<PendingPress>,
    /// The app currently being dragged in edit mode (lifted off the grid). Its
    /// tile is drawn at the pointer instead of its home cell.
    pub drag_item: Option<LauncherItem>,
    /// Pointer position the dragged tile follows (physical px, screen space).
    pub drag_x: f32,
    pub drag_y: f32,
    /// Accumulated wiggle animation phase (seconds). Only advances while
    /// `editing`.
    pub wiggle_phase: f32,
    /// Per-visible-app position springs. Each spring is keyed by `AppId` so it
    /// follows the app across reorder operations: the old cell remains the
    /// current spring value, and the app's new cell becomes the target.
    pub tile_springs: Vec<(LauncherItem, crate::scroll::Spring2)>,

    // ---- folder feature -------------------------------------------------
    pub folders: crate::features::folders::FolderFeatureState,
    pub folder_scroller: Option<Scroller>,
    pub folder_layout: Option<crate::layout::folder_panel::FolderPanelModel>,
    /// Per-child position springs used by the open-folder grid. Stable AppId
    /// keys preserve the visible position while preview order changes, then
    /// glide each non-dragged child to its new cell like the main grid.
    pub folder_child_springs: Vec<(AppId, crate::scroll::Spring2)>,
    /// Monotonic counters exposed only to QA telemetry. They let a sequence
    /// distinguish input-event cadence from layout/render cadence without
    /// changing either production path.
    pub folder_pointer_move_serial: u64,
    pub relayout_serial: u64,
    pub interaction_glass: Vec<crate::ui_model::render_model::GlassSurface>,

    // ---- bottom-center morphing control (search pill / page indicator /
    // search field) ----
    pub control: crate::features::bottom_control::BottomControl,
    /// Measured laid-out width (physical px) of the current search query, set
    /// once per frame in `render_bottom_control` (where we hold `&mut text`)
    /// and read back by `measure_query_width`. `None` = not measured this
    /// frame yet.
    pub cached_query_width: Option<f32>,
    /// Measured laid-out width (physical px) of the edit-mode Done label.
    pub cached_done_width: Option<f32>,
    /// 0 = normal bottom control width, 1 = edit-mode Done width.
    pub edit_control_progress: f32,
    /// Last settled page index, used to detect page changes for the indicator.
    pub last_page: i32,
    /// Whether the pointer is currently over the control capsule (hover),
    /// for hit-testing click vs. background.
    pub pointer_over_control: bool,
    /// True while the left button is held down *and* the press started on the
    /// control capsule. Such a release is a control click, not an app launch.
    pub pressed_on_control: bool,
    /// Settings overlay target pressed by the current pointer down, if any.
    pub pressed_on_settings: Option<SettingsPressTarget>,
    /// True while the settings overlay panel is shown on top of the grid.
    pub settings_open: bool,
    /// 0..1 presentation progress for the settings panel open/close animation.
    pub settings_panel_progress: f32,
    /// Persisted settings edited by the overlay.
    pub settings: Settings,
    /// Sidebar category currently shown by the settings overlay.
    pub settings_category: SettingsCategory,
    /// Timestamp of the last redraw, used to compute a real dt for the control
    /// animations (caret blink + morphs).
    pub last_redraw: Option<Instant>,
    pub last_frame_dt_ms: f32,

    // ---- resident-lifecycle state ----
    /// Whether the window is currently visible. `set_visible` doesn't query,
    /// so we track it ourselves to make `hide()` idempotent (avoids a hide
    /// storm when a focus-loss event races an app-launch hide).
    pub visible: bool,
    /// When the most recent `summon()` happened. A `Focused(false)` that
    /// arrives within `SUMMON_FOCUS_GRACE` of a summon is treated as a
    /// focus-transition artifact (SetForegroundWindow can briefly lose and
    /// re-acquire focus as the OS shuffles windows) and ignored, instead of
    /// instantly hiding the just-summoned window.
    pub last_summon: Option<Instant>,
    /// Set by `UserEvent::QuitRequested` (tray "Quit"); checked in
    /// `about_to_wait` to actually exit the loop. Decoupling the request from
    /// the exit lets the loop drain the current frame cleanly.
    pub should_quit: bool,
    /// Anchor keeping the OS-integration thread (hot key + tray) alive for
    /// the whole process. Underscore-prefixed because we never read it.
    #[cfg(windows)]
    pub _os: Option<crate::platform::windows::OsIntegrationHandle>,
}

use crate::domain::app_registry::AppLaunchInfo;

impl App {
    pub fn new(
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
            layout: crate::grid::GridLayout::default(),
            render_model: crate::ui_model::render_model::RenderModel::new(),
            timer,
            qa_runner: crate::qa::QaRunner::from_env(),
            registry: AppRegistry::new(),
            launcher_state: LauncherState::new(),
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
            drag_item: None,
            drag_x: 0.0,
            drag_y: 0.0,
            wiggle_phase: 0.0,
            tile_springs: Vec::new(),
            folders: crate::features::folders::FolderFeatureState::default(),
            folder_scroller: None,
            folder_layout: None,
            folder_child_springs: Vec::new(),
            folder_pointer_move_serial: 0,
            relayout_serial: 0,
            interaction_glass: Vec::new(),
            control: crate::features::bottom_control::BottomControl::new(),
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
            last_frame_dt_ms: 0.0,
            visible: true,
            last_summon: None,
            should_quit: false,
            #[cfg(windows)]
            _os: None,
        }
    }
}

impl App {
    pub(crate) fn viewport_phys(&self) -> (u32, u32) {
        self.renderer
            .as_ref()
            .map(|r| {
                let s = r.window.inner_size();
                (s.width, s.height)
            })
            .unwrap_or((1280, 800))
    }

    /// Current 0-based page index from the scroller position.
    pub(crate) fn current_page(&self) -> usize {
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
    pub(crate) fn frame_bottom_y(&self) -> f32 {
        let (w, _h) = self.viewport_phys();
        let (_cx, cy, _pw, panel_h) = self.layout.frame_panel_rect(w.max(1) as f32);
        cy + panel_h * 0.5
    }

    /// Resolve the control's frame geometry + layers for the current state.
    pub(crate) fn resolve_control(
        &self,
    ) -> Option<(
        crate::features::bottom_control::ControlGeometry,
        Vec<crate::features::bottom_control::ControlLayer>,
    )> {
        let viewport = self.viewport_phys();
        let frame_bottom = self.frame_bottom_y();
        let page = self.current_page();
        let page_count = self.layout.page_count;
        let edit_width =
            self.cached_done_width
                .map(|w| crate::features::bottom_control::EditWidth {
                    half_width: crate::features::bottom_control::done_half_width(
                        w,
                        self.scale_factor,
                    ),
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

    /// Build the [`crate::layout::bottom_control::BottomControlInput`] snapshot from
    /// current `App` state. This is the single place that assembles the values
    /// the layout layer needs to resolve bottom-control geometry and hit
    /// regions for one frame.
    pub(crate) fn bottom_control_input(&self) -> crate::layout::bottom_control::BottomControlInput {
        crate::layout::bottom_control::BottomControlInput {
            viewport: self.viewport_phys(),
            frame_bottom: self.frame_bottom_y(),
            scale_factor: self.scale_factor,
            page: self.current_page(),
            page_count: self.layout.page_count,
            mode: self.control.mode,
            expand: self.control.expand,
            indicator: self.control.indicator,
            editing: self.editing,
            edit_visual_progress: self.edit_visual_progress(),
            edit_control_progress: self.edit_control_progress,
            cached_done_width: self.cached_done_width,
            settings_open: self.settings_panel_active(),
        }
    }

    /// Build the bottom-control layout model (geometry snapshot + hit map) for
    /// the current frame. Pointer routing consumes [`hit_test`][lbcht] from
    /// this model instead of duplicating capsule/gear/close geometry inline.
    ///
    /// [`hit_test`]: crate::layout::bottom_control::hit_test
    pub(crate) fn bottom_control_model(&self) -> crate::layout::bottom_control::BottomControlModel {
        self.bottom_control_input().build()
    }

    /// Classify a physical-pixel pointer against the bottom-control hit map.
    /// Mirrors the previous inline hit-tests (`control.hit_test_scaled` +
    /// `edit_gear_hit` + close-button square) but sourced from a single layout
    /// pass.
    pub(crate) fn bottom_control_intent(
        &self,
        x: f32,
        y: f32,
    ) -> crate::layout::bottom_control::BottomControlPointerIntent {
        let model = self.bottom_control_model();
        crate::layout::bottom_control::hit_test(&model, crate::ui_model::geometry::Point::new(x, y))
    }

    /// Test whether a physical-pixel pointer is inside the control's capsule
    /// shape (inner rect + endcap circles), using the non-edit-width resolve.
    /// This is the exact test the previous `control.hit_test_scaled` performed
    /// on pointer release, and is kept separate from the intent-based hit map
    /// so the release gate treats the capsule shape (not the gear region) as
    /// the click boundary — preserving the behavior where a press on the gear
    /// that drifts off the capsule is dropped, while a press on the
    /// capsule/gear overlap still reaches `handle_control_click`.
    pub(crate) fn bottom_control_capsule_hit(&self, x: f32, y: f32) -> bool {
        self.control.hit_test_scaled(
            self.viewport_phys(),
            self.frame_bottom_y(),
            x,
            y,
            self.scale_factor,
        )
    }

    pub(crate) fn settings_panel_layout(
        &self,
    ) -> crate::layout::settings_panel::SettingsPanelLayout {
        crate::layout::settings_panel::panel_layout(self.viewport_phys(), self.scale_factor)
    }

    /// True when physical-px point `(x, y)` is inside the settings panel rect.
    pub(crate) fn settings_panel_contains(&self, x: f32, y: f32) -> bool {
        let layout = self.settings_panel_layout();
        crate::layout::settings_panel::contains(
            &layout,
            crate::ui_model::geometry::Point::new(x, y),
        )
    }

    /// True when `(x, y)` is over the panel's close (×) button.
    pub(crate) fn settings_panel_hit_close(&self, x: f32, y: f32) -> bool {
        let layout = self.settings_panel_layout();
        crate::layout::settings_panel::hit_close(
            &layout,
            self.scale_factor,
            crate::ui_model::geometry::Point::new(x, y),
        )
    }

    /// Measure the current visible search text's laid-out width in physical px (for caret
    /// placement). Returns the value measured this frame in
    /// `render_bottom_control` via cosmic-text shaping (cached so this can be
    /// called under `&self`). Falls back to 0 when not measured yet or when
    /// both the committed query and in-flight preedit are empty.
    pub(crate) fn measure_query_width(&self) -> f32 {
        if self.control.query.is_empty() && self.control.preedit.is_empty() {
            return 0.0;
        }
        self.cached_query_width.unwrap_or(0.0)
    }

    pub(crate) fn edit_visual_progress(&self) -> f32 {
        if self.editing {
            1.0
        } else if self.edit_control_progress > 0.001 {
            self.edit_control_progress.max(0.0)
        } else {
            0.0
        }
    }

    pub(crate) fn settings_panel_active(&self) -> bool {
        self.settings_open || self.settings_panel_progress > 0.001
    }

    /// Search text used for live filtering. This intentionally includes the
    /// active IME preedit so Japanese composition narrows the grid before the
    /// text is committed, while the committed query remains stored separately.
    pub(crate) fn visible_search_query(&self) -> String {
        let mut q = String::with_capacity(self.control.query.len() + self.control.preedit.len());
        q.push_str(&self.control.query);
        q.push_str(&self.control.preedit);
        q
    }

    pub(crate) fn matches_search(name: &str, query: &str) -> bool {
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

    /// Build the ordered list of app ids that should be considered for the
    /// visible grid, before search filtering. This is the Phase 7 source of the
    /// grid order: it follows `launcher_state.items` (app items only), and when
    /// `include_hidden` is true (the search-includes-hidden setting with a
    /// non-empty query) it also appends hidden apps that are not in `items`,
    /// sorted by display name, so a search can still surface hidden matches.
    /// Apps not currently discovered are skipped at the render step (no record),
    /// but their position in `items` is retained for re-detection.
    fn ordered_visible_candidate_ids(&self, include_hidden: bool) -> Vec<AppId> {
        let launcher_state = self.presentation_launcher_state();
        let mut ids: Vec<AppId> = launcher_state
            .items
            .iter()
            .filter_map(LauncherItem::as_app_id)
            .filter(|id| include_hidden || !launcher_state.is_hidden(id))
            .cloned()
            .collect();
        if include_hidden {
            // Hidden apps are not in `items` (hide_app removes them), so to let a
            // search surface them we append the discovered hidden ids not already
            // present, sorted by display name (matching the legacy registry.apps()
            // iteration order for the hidden tail).
            let present: std::collections::HashSet<AppId> = ids.iter().cloned().collect();
            let mut hidden_extra: Vec<AppId> = launcher_state
                .hidden_apps
                .iter()
                .filter(|id| !present.contains(*id))
                .filter(|id| self.registry.get(id).is_some())
                .cloned()
                .collect();
            hidden_extra.sort_by(|a, b| {
                let na = self.registry.lowercased_name_of(a).unwrap_or_default();
                let nb = self.registry.lowercased_name_of(b).unwrap_or_default();
                na.cmp(&nb)
            });
            ids.extend(hidden_extra);
        }
        ids
    }

    /// Build an owned snapshot of the currently visible app list in display order.
    /// Returns owned data so it doesn't hold a borrow on `self` while the
    /// renderer mutates.
    ///
    /// Order follows the user-owned [`LauncherState`] item list (apps only —
    /// folder items are not yet rendered in Phase 7). Apps the user hid via the
    /// edit-mode ✕ badge are excluded here unless `search_includes_hidden` is
    /// on and the query is non-empty (then hidden matches are appended at the
    /// tail, matching the legacy behavior). Apps referenced by the launcher
    /// state but not currently discovered are skipped (no record to render),
    /// but the launcher state keeps their position for re-detection.
    pub(crate) fn grid_apps_owned(&self) -> Vec<(AppId, String, Option<crate::icons::UvRect>)> {
        let query = self.visible_search_query();
        let include_hidden = self.settings.search_includes_hidden && !query.trim().is_empty();
        self.ordered_visible_candidate_ids(include_hidden)
            .into_iter()
            .filter_map(|id| {
                let rec = self.registry.get(&id)?;
                if Self::matches_search(&rec.name, &query) {
                    Some((rec.app_id.clone(), rec.name.clone(), rec.uv))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Build the production top-level item projection. With an empty search it
    /// preserves the exact app/folder interleave from `LauncherState.items`;
    /// with a query it intentionally returns flat discovered app results.
    pub(crate) fn grid_items_owned(&self) -> Vec<OwnedGridItem> {
        let query = self.visible_search_query();
        let launcher_state = self.presentation_launcher_state();
        if !query.trim().is_empty() {
            let include_hidden = self.settings.search_includes_hidden;
            return self
                .registry
                .apps()
                .iter()
                .filter(|record| include_hidden || !launcher_state.is_hidden(&record.app_id))
                .filter(|record| Self::matches_search(&record.name, &query))
                .map(|record| {
                    let item = LauncherItem::App(record.app_id.clone());
                    OwnedGridItem {
                        key: item.stable_key(),
                        item,
                        name: record.name.clone(),
                        uv: record.uv,
                        preview_uvs: Vec::new(),
                    }
                })
                .collect();
        }

        launcher_state
            .items
            .iter()
            .filter_map(|item| match item {
                LauncherItem::App(id) => {
                    let record = self.registry.get(id)?;
                    Some(OwnedGridItem {
                        item: item.clone(),
                        key: item.stable_key(),
                        name: record.name.clone(),
                        uv: record.uv,
                        preview_uvs: Vec::new(),
                    })
                }
                LauncherItem::Folder(folder_id) => {
                    let folder = launcher_state.folders.get(folder_id)?;
                    let preview_uvs = folder
                        .children
                        .iter()
                        .take(9)
                        .map(|child| {
                            (!launcher_state.is_hidden(child))
                                .then(|| self.registry.get(child).and_then(|record| record.uv))
                                .flatten()
                        })
                        .collect();
                    Some(OwnedGridItem {
                        item: item.clone(),
                        key: item.stable_key(),
                        name: folder.name.clone(),
                        uv: None,
                        preview_uvs,
                    })
                }
            })
            .collect()
    }

    pub(crate) fn visible_launcher_items(&self) -> Vec<LauncherItem> {
        self.grid_items_owned()
            .into_iter()
            .map(|entry| entry.item)
            .collect()
    }

    pub(crate) fn presentation_launcher_state(&self) -> &LauncherState {
        self.folders
            .child_exit_preview
            .as_ref()
            .map_or(&self.launcher_state, |preview| preview.launcher_state())
    }

    pub(crate) fn visible_app_ids(&self) -> Vec<AppId> {
        let query = self.visible_search_query();
        let include_hidden = self.settings.search_includes_hidden && !query.trim().is_empty();
        self.ordered_visible_candidate_ids(include_hidden)
            .into_iter()
            .filter(|id| {
                self.registry
                    .get(id)
                    .map(|rec| Self::matches_search(&rec.name, &query))
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Classify a screen-space pointer against the launcher grid using the
    /// layout layer's unified hit classifier. Returns whether the pointer is
    /// over a visible app cell (and which one), empty space inside the page
    /// frame, or the transparent launcher area outside the frame.
    pub(crate) fn grid_hit_at_pointer(&self, x: f32, y: f32) -> crate::layout::grid::GridHit {
        let (w, _h) = self.viewport_phys();
        let scroll_x = self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        let visible_count = self.visible_launcher_items().len();
        self.layout
            .classify(w as f32, x, y, scroll_x, visible_count)
    }

    pub(crate) fn pointer_over_page_glass(&self, x: f32, y: f32) -> bool {
        let (w, _h) = self.viewport_phys();
        self.layout.frame_contains_point(w as f32, x, y)
    }

    /// Resolve a tile-cell index for edit-mode drag/drop. Unlike app click
    /// hit-testing this excludes labels and allows the empty slot immediately
    /// after the last visible app, so dropping at the final page tail works.
    ///
    /// The hit decision comes from [`crate::layout::edit_mode::drop_cell_at`], a thin
    /// explicit wrapper over [`GridLayout::hit_test_tile_cell`] with
    /// `total_tiles` as the cell bound, so the rule that the label area is not a
    /// drop target lives in one place.
    pub(crate) fn edit_drop_index_at_pointer(&self, x: f32, y: f32) -> Option<usize> {
        let (w, _h) = self.viewport_phys();
        let scroll_x = self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        crate::layout::edit_mode::drop_cell_at(&self.layout, w as f32, x, y, scroll_x)
    }

    /// True if the pointer (physical px) is over the ✕ badge of the app at
    /// `idx`. The badge sits at the tile's top-left corner, with a radius
    /// matching the shader's badge radius (≈13% of the tile size, max 11px). A
    /// little slop is added so the hit area is forgiving on a touch screen.
    ///
    /// The geometry is resolved by [`crate::layout::edit_mode::badge_hit`] so the hit
    /// circle (radius + slop, centered at `tile + inset`) shares one calculation
    /// with the renderer's badge source geometry.
    pub(crate) fn badge_hit(&self, idx: usize, x: f32, y: f32) -> bool {
        let (w, _h) = self.viewport_phys();
        let scroll_x = self.scroller.as_ref().map(|s| s.position).unwrap_or(0.0);
        crate::layout::edit_mode::badge_hit(&self.layout, w as f32, x, y, scroll_x, idx)
    }

    /// Hit-test the pointer, then resolve the clicked app **by stable id**
    /// (not positional index), so a rescan that shifted the list can't launch
    /// the wrong app. Returns an owned snapshot safe to use after the
    /// launcher dismisses.
    pub(crate) fn resolve_clicked_app(&self, x_phys: f32, y_phys: f32) -> Option<AppLaunchInfo> {
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
}

#[cfg(test)]
mod pending_press_tests {
    use std::time::Instant;

    use super::{PendingPress, CLICK_SLOP_PHYS};
    use crate::domain::app_id::AppId;

    fn press(
        item_index: Option<usize>,
        app_id: Option<AppId>,
        outside_glass: bool,
    ) -> PendingPress {
        PendingPress {
            start: Instant::now(),
            x: 100.0,
            y: 100.0,
            item_index,
            item: app_id.map(crate::domain::launcher_item::LauncherItem::App),
            outside_glass,
        }
    }

    #[test]
    fn launches_only_pressed_app_id() {
        let pressed = AppId::from_normalized("pressed-app");
        let press = press(Some(3), Some(pressed.clone()), false);

        assert_eq!(
            press.activated_item(103.0, 102.0),
            Some(&crate::domain::launcher_item::LauncherItem::App(pressed))
        );
    }

    #[test]
    fn press_without_app_never_launches_release_target() {
        let press = press(None, None, false);

        assert_eq!(press.activated_item(103.0, 102.0), None);
    }

    #[test]
    fn movement_past_click_slop_does_not_launch() {
        let press = press(Some(0), Some(AppId::from_normalized("pressed-app")), false);

        assert_eq!(
            press.activated_item(100.0 + CLICK_SLOP_PHYS + 1.0, 100.0),
            None
        );
    }

    #[test]
    fn outside_glass_stationary_click_dismisses() {
        let press = press(None, None, true);

        assert!(press.is_outside_glass_click(102.0, 101.0));
    }

    #[test]
    fn outside_glass_drag_does_not_dismiss() {
        let press = press(None, None, true);

        assert!(!press.is_outside_glass_click(100.0 + CLICK_SLOP_PHYS + 1.0, 100.0));
    }

    #[test]
    fn inside_glass_empty_click_does_not_dismiss() {
        let press = press(None, None, false);

        assert!(!press.is_outside_glass_click(102.0, 101.0));
    }
}
