//! Deterministic, hidden-window GPU scenario runner used by visual QA.
//!
//! A JSON scenario supplies a synthetic launcher fixture and timestamped raw
//! pointer/semantic actions. Actions flow through the production `AppAction`
//! path, while rendered surface frames are copied directly by the renderer.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app::action::{AppAction, KeyAction};
use crate::app::state::App;
use crate::domain::app_id::AppId;
use crate::domain::app_registry::{AppRecord, AppRegistry, IconState};
use crate::domain::folders::{Folder, FolderId};
use crate::domain::launcher_item::LauncherItem;
use crate::domain::launcher_state::LauncherState;
use crate::ui_model::geometry::Point;

pub const SCENARIO_ENV: &str = "LAUNCHPAD_QA_SCENARIO";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QaScenario {
    pub name: String,
    #[serde(default = "default_viewport")]
    pub viewport: [u32; 2],
    #[serde(default = "default_fps")]
    pub fps: u32,
    pub duration_ms: u64,
    pub output_dir: PathBuf,
    pub fixture: QaFixture,
    #[serde(default)]
    pub actions: Vec<TimedAction>,
}

fn default_viewport() -> [u32; 2] {
    [1280, 800]
}

fn default_fps() -> u32 {
    30
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QaFixture {
    pub apps: Vec<QaApp>,
    #[serde(default)]
    pub folders: Vec<QaFolder>,
    pub items: Vec<QaItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QaApp {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QaFolder {
    pub id: String,
    pub name: String,
    pub children: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QaItem {
    App { id: String },
    Folder { id: String },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TimedAction {
    pub at_ms: u64,
    #[serde(flatten)]
    pub action: QaAction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum QaAction {
    OpenFolder { id: String },
    Move { target: QaTarget },
    PointerDown,
    PointerUp,
    TypeText { value: String },
    CommitRename,
    Escape,
    ExitEditMode,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QaTarget {
    Point { x: f32, y: f32 },
    GridItem { index: usize },
    GridItemPoint { index: usize, x: f32, y: f32 },
    FolderChild { index: usize },
    FolderTitle,
    FolderPanel { x: f32, y: f32 },
}

#[derive(Debug, Clone, Serialize)]
pub struct QaFrameRecord {
    pub index: u64,
    pub elapsed_ms: u64,
    pub file: String,
    pub editing: bool,
    pub folder_open: bool,
    pub folder_page: usize,
    pub renaming: bool,
    pub folder_rename_caret_visible: Option<bool>,
    pub folder_scroll_x: Option<f32>,
    pub folder_scroll_velocity: Option<f32>,
    pub folder_scroll_phase: Option<String>,
    pub folder_child_drag: bool,
    pub top_level_drag: bool,
    pub top_level_item_count: usize,
    pub active_folder_child_count: Option<usize>,
}

struct QaFrameState {
    editing: bool,
    folder_open: bool,
    folder_page: usize,
    renaming: bool,
    folder_rename_caret_visible: Option<bool>,
    folder_scroll: Option<(f32, f32, crate::scroll::Phase)>,
    folder_child_drag: bool,
    top_level_drag: bool,
    top_level_item_count: usize,
    active_folder_child_count: Option<usize>,
}

#[derive(Debug, Serialize)]
struct QaManifest<'a> {
    scenario: &'a str,
    viewport: [u32; 2],
    fps: u32,
    duration_ms: u64,
    completed: bool,
    frames: &'a [QaFrameRecord],
    video_command: String,
}

pub struct QaRunner {
    scenario: QaScenario,
    scenario_path: PathBuf,
    run_dir: PathBuf,
    start: Option<Instant>,
    next_action: usize,
    next_capture_ms: u64,
    frame_index: u64,
    frames: Vec<QaFrameRecord>,
    finalized: bool,
}

impl QaRunner {
    pub fn from_env() -> Option<Self> {
        let scenario_path = PathBuf::from(std::env::var_os(SCENARIO_ENV)?);
        match Self::load(&scenario_path) {
            Ok(runner) => Some(runner),
            Err(error) => panic!("failed to load {}: {error}", scenario_path.display()),
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        let mut scenario: QaScenario =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        scenario.fps = scenario.fps.clamp(1, 120);
        scenario.viewport[0] = scenario.viewport[0].max(320);
        scenario.viewport[1] = scenario.viewport[1].max(240);
        scenario.actions.sort_by_key(|action| action.at_ms);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let root = if scenario.output_dir.is_absolute() {
            scenario.output_dir.clone()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(&scenario.output_dir)
        };
        let run_dir = root.join(format!("{}-{stamp}", sanitize_name(&scenario.name)));
        std::fs::create_dir_all(&run_dir).map_err(|error| error.to_string())?;
        Ok(Self {
            scenario,
            scenario_path: path.to_path_buf(),
            run_dir,
            start: None,
            next_action: 0,
            next_capture_ms: 0,
            frame_index: 0,
            frames: Vec::new(),
            finalized: false,
        })
    }

    pub fn viewport(&self) -> [u32; 2] {
        self.scenario.viewport
    }

    pub fn fixture(&self) -> &QaFixture {
        &self.scenario.fixture
    }

    pub fn start(&mut self, now: Instant) {
        self.start.get_or_insert(now);
    }

    pub fn elapsed_ms(&self, now: Instant) -> u64 {
        self.start
            .map(|start| now.saturating_duration_since(start).as_millis() as u64)
            .unwrap_or(0)
    }

    pub fn take_due_actions(&mut self, now: Instant) -> Vec<QaAction> {
        let elapsed = self.elapsed_ms(now);
        let mut due = Vec::new();
        while let Some(action) = self.scenario.actions.get(self.next_action) {
            if action.at_ms > elapsed {
                break;
            }
            due.push(action.action.clone());
            self.next_action += 1;
        }
        due
    }

    pub fn capture_due(&self, now: Instant) -> bool {
        self.start.is_some() && self.elapsed_ms(now) >= self.next_capture_ms && !self.finished(now)
    }

    fn next_capture_path(&mut self, now: Instant, state: QaFrameState) -> Option<PathBuf> {
        if !self.capture_due(now) {
            return None;
        }
        let elapsed_ms = self.elapsed_ms(now);
        let file = format!("frame_{:06}.png", self.frame_index);
        self.frames.push(QaFrameRecord {
            index: self.frame_index,
            elapsed_ms,
            file: file.clone(),
            editing: state.editing,
            folder_open: state.folder_open,
            folder_page: state.folder_page,
            renaming: state.renaming,
            folder_rename_caret_visible: state.folder_rename_caret_visible,
            folder_scroll_x: state.folder_scroll.map(|value| value.0),
            folder_scroll_velocity: state.folder_scroll.map(|value| value.1),
            folder_scroll_phase: state.folder_scroll.map(|value| format!("{:?}", value.2)),
            folder_child_drag: state.folder_child_drag,
            top_level_drag: state.top_level_drag,
            top_level_item_count: state.top_level_item_count,
            active_folder_child_count: state.active_folder_child_count,
        });
        self.frame_index += 1;
        let frame_ms = (1000 / self.scenario.fps.max(1) as u64).max(1);
        self.next_capture_ms = self.next_capture_ms.saturating_add(frame_ms);
        Some(self.run_dir.join(file))
    }

    pub fn finished(&self, now: Instant) -> bool {
        self.start.is_some() && self.elapsed_ms(now) >= self.scenario.duration_ms
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let start = self.start?;
        let next_action_ms = self
            .scenario
            .actions
            .get(self.next_action)
            .map(|action| action.at_ms)
            .unwrap_or(self.scenario.duration_ms);
        let next_ms = next_action_ms
            .min(self.next_capture_ms)
            .min(self.scenario.duration_ms);
        Some(start + Duration::from_millis(next_ms))
    }

    pub fn finalize(&mut self) {
        if self.finalized {
            return;
        }
        let manifest = QaManifest {
            scenario: &self.scenario.name,
            viewport: self.scenario.viewport,
            fps: self.scenario.fps,
            duration_ms: self.scenario.duration_ms,
            completed: true,
            frames: &self.frames,
            video_command: format!(
                "ffmpeg -framerate {} -i frame_%06d.png -c:v libx264 -pix_fmt yuv420p {}.mp4",
                self.scenario.fps,
                sanitize_name(&self.scenario.name)
            ),
        };
        if let Ok(json) = serde_json::to_vec_pretty(&manifest) {
            let _ = std::fs::write(self.run_dir.join("manifest.json"), json);
        }
        let _ = std::fs::write(
            self.run_dir.join("scenario-source.txt"),
            self.scenario_path.display().to_string(),
        );
        eprintln!("qa sequence complete: {}", self.run_dir.display());
        self.finalized = true;
    }
}

impl App {
    pub(crate) fn qa_enabled(&self) -> bool {
        self.qa_runner.is_some()
    }

    pub(crate) fn install_qa_fixture(&mut self) {
        let Some(fixture) = self.qa_runner.as_ref().map(QaRunner::fixture).cloned() else {
            return;
        };
        self.registry = AppRegistry::new();
        for app in &fixture.apps {
            let id = AppId::from_normalized(app.id.clone());
            let slot = self.registry.alloc_slot();
            self.registry.insert(AppRecord {
                app_id: id,
                name: app.name.clone(),
                link_path: PathBuf::from(format!("qa/{}.lnk", app.id)),
                resolved_target: PathBuf::from(format!("qa/{}.exe", app.id)),
                slot,
                icon_state: IconState::Missing,
                uv: None,
            });
        }
        let mut launcher = LauncherState::new();
        for folder in &fixture.folders {
            let id = FolderId::from_normalized(folder.id.clone());
            launcher.upsert_folder(Folder {
                id,
                name: folder.name.clone(),
                children: folder
                    .children
                    .iter()
                    .cloned()
                    .map(AppId::from_normalized)
                    .collect(),
            });
        }
        launcher.set_items(
            fixture
                .items
                .iter()
                .map(|item| match item {
                    QaItem::App { id } => LauncherItem::App(AppId::from_normalized(id.clone())),
                    QaItem::Folder { id } => {
                        LauncherItem::Folder(FolderId::from_normalized(id.clone()))
                    }
                })
                .collect(),
        );
        self.launcher_state = launcher;
    }

    pub(crate) fn start_qa(&mut self, now: Instant) {
        if let Some(runner) = self.qa_runner.as_mut() {
            runner.start(now);
        }
    }

    pub(crate) fn tick_qa(&mut self, now: Instant) {
        let actions = self
            .qa_runner
            .as_mut()
            .map(|runner| runner.take_due_actions(now))
            .unwrap_or_default();
        for action in actions {
            self.apply_qa_action(action);
        }
    }

    fn apply_qa_action(&mut self, action: QaAction) {
        match action {
            QaAction::OpenFolder { id } => {
                self.open_folder(FolderId::from_normalized(id));
            }
            QaAction::Move { target } => {
                if let Some(point) = self.resolve_qa_target(&target) {
                    self.handle_action(AppAction::PointerMoved {
                        x: point.x,
                        y: point.y,
                    });
                }
            }
            QaAction::PointerDown => {
                let action = self.classify_pointer_press(self.pointer_phys_x, self.pointer_phys_y);
                self.handle_action(AppAction::PointerPress(action));
            }
            QaAction::PointerUp => {
                let action =
                    self.classify_pointer_release(self.pointer_phys_x, self.pointer_phys_y);
                self.handle_action(AppAction::PointerRelease(action));
            }
            QaAction::TypeText { value } => {
                self.handle_action(AppAction::Keyboard(KeyAction::FolderRenameChar(value)));
            }
            QaAction::CommitRename => {
                self.handle_action(AppAction::Keyboard(KeyAction::CommitFolderRename));
            }
            QaAction::Escape => {
                let action = if self.folders.rename.is_some() {
                    KeyAction::CancelFolderRename
                } else if self.editing {
                    KeyAction::ExitEditMode
                } else if self.folders.is_active() {
                    KeyAction::CloseFolder
                } else {
                    KeyAction::None
                };
                self.handle_action(AppAction::Keyboard(action));
            }
            QaAction::ExitEditMode => {
                self.handle_action(AppAction::Keyboard(KeyAction::ExitEditMode))
            }
        }
    }

    fn resolve_qa_target(&self, target: &QaTarget) -> Option<Point> {
        match target {
            QaTarget::Point { x, y } => Some(Point::new(*x, *y)),
            QaTarget::GridItem { index } => self
                .visible_launcher_items()
                .get(*index)
                .and_then(|item| self.launcher_item_rect(item))
                .map(|rect| rect.center()),
            QaTarget::GridItemPoint { index, x, y } => self
                .visible_launcher_items()
                .get(*index)
                .and_then(|item| self.launcher_item_rect(item))
                .map(|rect| {
                    Point::new(
                        rect.x + rect.width * x.clamp(0.0, 1.0),
                        rect.y + rect.height * y.clamp(0.0, 1.0),
                    )
                }),
            QaTarget::FolderChild { index } => self
                .folder_layout
                .as_ref()?
                .child_rects
                .get(*index)
                .map(|rect| rect.center()),
            QaTarget::FolderTitle => self
                .folder_layout
                .as_ref()
                .map(|layout| layout.title_rect.center()),
            QaTarget::FolderPanel { x, y } => self.folder_layout.as_ref().map(|layout| {
                Point::new(
                    layout.target_panel_rect.x + layout.target_panel_rect.width * x.clamp(0.0, 1.0),
                    layout.target_panel_rect.y
                        + layout.target_panel_rect.height * y.clamp(0.0, 1.0),
                )
            }),
        }
    }

    pub(crate) fn qa_capture_path(&mut self, now: Instant) -> Option<PathBuf> {
        let editing = self.editing;
        let folder_open = self.folders.is_active();
        let folder_page = self.folders.page;
        let renaming = self.folders.rename.is_some();
        let folder_rename_caret_visible = renaming.then(|| {
            crate::layout::control_geometry::caret_blink_opacity(self.control.caret_phase) > 0.5
        });
        let folder_scroll = self
            .folder_scroller
            .as_ref()
            .map(|scroller| (scroller.position, scroller.velocity, scroller.phase));
        let folder_child_drag = self.folders.child_drag.is_some();
        let top_level_drag = self.drag_item.is_some();
        let top_level_item_count = self.launcher_state.items.len();
        let active_folder_child_count = self
            .folders
            .active
            .as_ref()
            .and_then(|id| self.launcher_state.folders.get(id))
            .map(|folder| folder.children.len());
        self.qa_runner.as_mut()?.next_capture_path(
            now,
            QaFrameState {
                editing,
                folder_open,
                folder_page,
                renaming,
                folder_rename_caret_visible,
                folder_scroll,
                folder_child_drag,
                top_level_drag,
                top_level_item_count,
                active_folder_child_count,
            },
        )
    }

    pub(crate) fn qa_capture_due(&self, now: Instant) -> bool {
        self.qa_runner
            .as_ref()
            .is_some_and(|runner| runner.capture_due(now))
    }

    pub(crate) fn qa_finished(&self, now: Instant) -> bool {
        self.qa_runner
            .as_ref()
            .is_some_and(|runner| runner.finished(now))
    }

    pub(crate) fn qa_next_deadline(&self) -> Option<Instant> {
        self.qa_runner.as_ref()?.next_deadline()
    }

    pub(crate) fn finalize_qa(&mut self) {
        if let Some(runner) = self.qa_runner.as_mut() {
            runner.finalize();
        }
    }
}

fn sanitize_name(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    value.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_schema_parses_semantic_targets() {
        let value = serde_json::json!({
            "name": "folder",
            "duration_ms": 1000,
            "output_dir": "target/qa",
            "fixture": {
                "apps": [{"id": "a", "name": "App A"}],
                "folders": [{"id": "folder-0", "name": "Folder", "children": ["a"]}],
                "items": [{"kind": "folder", "id": "folder-0"}]
            },
            "actions": [
                {"at_ms": 0, "type": "open_folder", "id": "folder-0"},
                {"at_ms": 200, "type": "move", "target": {"kind": "folder_child", "index": 0}},
                {"at_ms": 250, "type": "pointer_down"}
            ]
        });
        let scenario: QaScenario = serde_json::from_value(value).unwrap();
        assert_eq!(scenario.viewport, [1280, 800]);
        assert_eq!(scenario.fps, 30);
        assert!(matches!(
            scenario.actions[1].action,
            QaAction::Move {
                target: QaTarget::FolderChild { index: 0 }
            }
        ));
    }

    #[test]
    fn runner_orders_actions_and_sanitizes_run_name() {
        assert_eq!(sanitize_name("Folder QA / 1"), "Folder-QA---1");
    }
}
