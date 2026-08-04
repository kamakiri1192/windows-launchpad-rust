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
use crate::icons::normalize::{DecodedIcon, TARGET};
use crate::ui_model::geometry::Point;

pub const SCENARIO_ENV: &str = "LAUNCHPAD_QA_SCENARIO";
pub const HEADLESS_ENV: &str = "LAUNCHPAD_QA_HEADLESS";

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
    pub scroll_expectations: Option<QaScrollExpectations>,
    #[serde(default)]
    pub context_menu_expectations: Option<QaContextMenuExpectations>,
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
pub struct QaScrollExpectations {
    pub min_samples: usize,
    pub expected_terminal_contacts: u32,
    pub expected_horizontal_releases: u32,
    pub expected_target_decisions: u32,
    pub expected_spring_generations: u32,
    pub expected_releases: Vec<QaReleaseExpectation>,
    #[serde(default)]
    pub min_zero_crossings: u32,
    pub required_surfaces: Vec<QaPagerSurface>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QaReleaseExpectation {
    pub gesture_id: u64,
    pub surface: QaPagerSurface,
    pub min_filtered_velocity: f32,
    pub max_filtered_velocity: f32,
    pub target_x: f32,
    #[serde(default = "default_target_tolerance")]
    pub target_tolerance: f32,
    #[serde(default)]
    pub min_release_position_x: Option<f32>,
    #[serde(default)]
    pub max_release_position_x: Option<f32>,
    #[serde(default)]
    pub settled_position_x: Option<f32>,
    #[serde(default)]
    pub max_settle_duration_ms: Option<u64>,
}

fn default_target_tolerance() -> f32 {
    1.0
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QaContextMenuExpectations {
    pub expected_open_count: u32,
    #[serde(default = "default_min_context_menu_open_frames")]
    pub min_open_frames: u32,
    /// Number of opening attempts, including attempts dismissed before Open.
    #[serde(default)]
    pub expected_opening_count: Option<u32>,
    /// Number of transitions into the Closing phase.
    #[serde(default)]
    pub expected_closing_count: u32,
    /// Number of completed transitions back into Closed.
    #[serde(default)]
    pub expected_closed_count: u32,
    #[serde(default = "default_min_context_menu_closing_frames")]
    pub min_closing_frames: u32,
    /// Maximum frames allowed for any single Closing streak. This catches a
    /// visually absent menu whose lifecycle is kept alive by a slow spring or
    /// by repeated dismiss events restarting its elapsed timers.
    #[serde(default)]
    pub max_closing_frames: Option<u32>,
    #[serde(default)]
    pub require_final_closed: bool,
}

fn default_min_context_menu_open_frames() -> u32 {
    10
}

fn default_min_context_menu_closing_frames() -> u32 {
    3
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum QaPagerSurface {
    Main,
    Folder,
}

impl QaPagerSurface {
    fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Folder => "folder",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QaFixture {
    #[serde(default)]
    pub apps: Vec<QaApp>,
    #[serde(default)]
    pub generated_apps: Vec<QaGeneratedApps>,
    #[serde(default)]
    pub folders: Vec<QaFolder>,
    #[serde(default)]
    pub items: Vec<QaItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QaGeneratedApps {
    pub prefix: String,
    pub name_prefix: String,
    pub count: usize,
    #[serde(default = "default_true")]
    pub top_level: bool,
}

fn default_true() -> bool {
    true
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
    OpenFolder {
        id: String,
    },
    Move {
        target: QaTarget,
    },
    /// Press and release the right button through the production input router
    /// at the current pointer position.
    RightClick,
    PointerDown,
    LongPress,
    PointerUp,
    TypeText {
        value: String,
    },
    CommitRename,
    Escape,
    ExitEditMode,
    /// Replay a production-normalized scroll sample. Deltas use the pager's
    /// canonical display convention: positive x moves the visible grid right.
    ScrollSample {
        #[serde(default)]
        gesture_id: Option<u64>,
        timestamp_us: u64,
        canonical_dx: f32,
        canonical_dy: f32,
        #[serde(default)]
        source: QaScrollSource,
        #[serde(default)]
        contact_phase: QaScrollPhase,
        #[serde(default)]
        momentum_phase: QaScrollPhase,
        #[serde(default)]
        sequence_complete: bool,
    },
    /// Replay a raw native packet through the production `ScrollSampleAdapter`.
    /// `expected_canonical_*` makes preservation of AppKit's already
    /// preference-adjusted delta observable.
    NativeScrollSample {
        timestamp_us: u64,
        raw_dx: f32,
        raw_dy: f32,
        expected_canonical_dx: f32,
        expected_canonical_dy: f32,
        #[serde(default)]
        source: QaScrollSource,
        #[serde(default)]
        contact_phase: QaScrollPhase,
        #[serde(default)]
        momentum_phase: QaScrollPhase,
        #[serde(default)]
        sequence_complete: bool,
        #[serde(default)]
        direction_inverted_from_device: bool,
        #[serde(default = "default_scale_factor")]
        scale_factor: f32,
    },
}

fn default_scale_factor() -> f32 {
    1.0
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QaScrollSource {
    #[default]
    Precise,
    Line,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QaScrollPhase {
    #[default]
    None,
    Began,
    Changed,
    Ended,
    Cancelled,
}

impl QaScrollPhase {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Ended | Self::Cancelled)
    }
}

fn expand_generated_apps(fixture: &mut QaFixture) -> Result<(), String> {
    use std::collections::BTreeSet;

    let mut ids = fixture
        .apps
        .iter()
        .map(|app| app.id.clone())
        .collect::<BTreeSet<_>>();
    for generated in std::mem::take(&mut fixture.generated_apps) {
        if generated.prefix.is_empty() {
            return Err("generated_apps prefix cannot be empty".to_owned());
        }
        for index in 0..generated.count {
            let id = format!("{}-{index:03}", generated.prefix);
            if !ids.insert(id.clone()) {
                return Err(format!("generated_apps produced duplicate id={id}"));
            }
            fixture.apps.push(QaApp {
                id: id.clone(),
                name: format!("{} {index:03}", generated.name_prefix),
            });
            if generated.top_level {
                fixture.items.push(QaItem::App { id });
            }
        }
    }
    Ok(())
}

fn normalize_scroll_actions(actions: &mut [TimedAction]) -> Result<(), String> {
    use std::collections::BTreeSet;

    let mut next_gesture_id = 1_u64;
    let mut active_contact = None;
    let mut quarantined = BTreeSet::new();
    let mut completed = BTreeSet::new();
    let mut previous_timestamp = None;

    for (index, timed) in actions.iter_mut().enumerate() {
        let QaAction::ScrollSample {
            gesture_id,
            timestamp_us,
            canonical_dx,
            canonical_dy,
            contact_phase,
            momentum_phase,
            sequence_complete,
            ..
        } = &mut timed.action
        else {
            continue;
        };

        if !canonical_dx.is_finite() || !canonical_dy.is_finite() {
            return Err(format!(
                "scroll_sample action {index} has a non-finite canonical delta"
            ));
        }
        if previous_timestamp.is_some_and(|previous| *timestamp_us < previous) {
            return Err(format!(
                "scroll_sample action {index} timestamp_us={} moves backwards",
                *timestamp_us
            ));
        }
        previous_timestamp = Some(*timestamp_us);

        let has_contact = *contact_phase != QaScrollPhase::None;
        let has_momentum = *momentum_phase != QaScrollPhase::None;
        if has_contact && has_momentum {
            return Err(format!(
                "scroll_sample action {index} cannot carry contact and momentum phases together"
            ));
        }
        if *sequence_complete
            && (has_contact || has_momentum || *canonical_dx != 0.0 || *canonical_dy != 0.0)
        {
            return Err(format!(
                "scroll_sample action {index} sequence_complete must be a zero-delta phase-less terminal signal"
            ));
        }
        if !has_contact && !has_momentum && !*sequence_complete {
            return Err(format!(
                "scroll_sample action {index} must carry a contact phase, momentum phase, or sequence_complete"
            ));
        }

        let resolved = if has_contact {
            match *contact_phase {
                QaScrollPhase::Began => {
                    if active_contact.is_some() {
                        return Err(format!(
                            "scroll_sample action {index} begins a second active contact"
                        ));
                    }
                    let id = gesture_id.unwrap_or(next_gesture_id);
                    if quarantined.contains(&id) || completed.contains(&id) {
                        return Err(format!(
                            "scroll_sample action {index} reuses terminal gesture_id={id}"
                        ));
                    }
                    next_gesture_id = next_gesture_id.max(id.saturating_add(1));
                    active_contact = Some(id);
                    id
                }
                QaScrollPhase::Changed | QaScrollPhase::Ended | QaScrollPhase::Cancelled => {
                    let active = active_contact.ok_or_else(|| {
                        format!(
                            "scroll_sample action {index} has {:?} without an active contact",
                            *contact_phase
                        )
                    })?;
                    let id = gesture_id.unwrap_or(active);
                    if id != active {
                        return Err(format!(
                            "scroll_sample action {index} gesture_id={id} does not match active contact {active}"
                        ));
                    }
                    if *contact_phase == QaScrollPhase::Ended {
                        active_contact = None;
                        quarantined.insert(id);
                    } else if *contact_phase == QaScrollPhase::Cancelled {
                        active_contact = None;
                        completed.insert(id);
                    }
                    id
                }
                QaScrollPhase::None => unreachable!(),
            }
        } else {
            let id = if let Some(id) = *gesture_id {
                id
            } else if quarantined.len() == 1 {
                *quarantined.first().expect("length checked")
            } else {
                return Err(format!(
                    "scroll_sample action {index} must name gesture_id because {} gestures await momentum completion",
                    quarantined.len()
                ));
            };
            if !quarantined.contains(&id) {
                return Err(format!(
                    "scroll_sample action {index} references non-quarantined gesture_id={id}"
                ));
            }
            if *sequence_complete || momentum_phase.is_terminal() {
                quarantined.remove(&id);
                completed.insert(id);
            }
            id
        };
        *gesture_id = Some(resolved);
    }
    Ok(())
}

fn validate_native_scroll_actions(actions: &[TimedAction]) -> Result<(), String> {
    let mut previous_timestamp = None;
    for (index, timed) in actions.iter().enumerate() {
        let QaAction::NativeScrollSample {
            timestamp_us,
            raw_dx,
            raw_dy,
            expected_canonical_dx,
            expected_canonical_dy,
            contact_phase,
            momentum_phase,
            sequence_complete,
            direction_inverted_from_device,
            scale_factor,
            ..
        } = &timed.action
        else {
            continue;
        };
        if [raw_dx, raw_dy, expected_canonical_dx, expected_canonical_dy]
            .iter()
            .any(|value| !value.is_finite())
            || !scale_factor.is_finite()
            || *scale_factor <= 0.0
        {
            return Err(format!(
                "native_scroll_sample action {index} has non-finite deltas or invalid scale"
            ));
        }
        if previous_timestamp.is_some_and(|previous| *timestamp_us < previous) {
            return Err(format!(
                "native_scroll_sample action {index} timestamp_us={timestamp_us} moves backwards"
            ));
        }
        previous_timestamp = Some(*timestamp_us);
        let has_contact = *contact_phase != QaScrollPhase::None;
        let has_momentum = *momentum_phase != QaScrollPhase::None;
        if has_contact && has_momentum {
            return Err(format!(
                "native_scroll_sample action {index} cannot carry contact and momentum phases together"
            ));
        }
        if *sequence_complete && (has_contact || has_momentum || *raw_dx != 0.0 || *raw_dy != 0.0) {
            return Err(format!(
                "native_scroll_sample action {index} sequence_complete must be a zero-delta phase-less terminal signal"
            ));
        }
        if !has_contact && !has_momentum && !*sequence_complete {
            return Err(format!(
                "native_scroll_sample action {index} must carry a contact phase, momentum phase, or sequence_complete"
            ));
        }
        let legacy_y_sign = if *direction_inverted_from_device {
            -1.0
        } else {
            1.0
        };
        if (*expected_canonical_dx - *raw_dx).abs() > 0.001
            || (*expected_canonical_dy - *raw_dy * legacy_y_sign).abs() > 0.001
        {
            return Err(format!(
                "native_scroll_sample action {index} disagrees with the production x/y contracts"
            ));
        }
    }
    Ok(())
}

fn validate_scroll_expectations(scenario: &QaScenario) -> Result<(), String> {
    use std::collections::BTreeSet;

    let sample_count = scenario
        .actions
        .iter()
        .filter(|action| {
            matches!(
                action.action,
                QaAction::ScrollSample { .. } | QaAction::NativeScrollSample { .. }
            )
        })
        .count();
    let Some(expected) = scenario.scroll_expectations.as_ref() else {
        if sample_count > 0 {
            return Err("scroll_sample actions require a scroll_expectations contract".to_owned());
        }
        return Ok(());
    };
    if sample_count == 0 {
        return Err("scroll_expectations requires at least one scroll_sample action".to_owned());
    }
    if expected.min_samples == 0 {
        return Err("scroll_expectations min_samples must be greater than zero".to_owned());
    }
    if expected.min_samples > sample_count {
        return Err(format!(
            "scroll_expectations min_samples={} exceeds the {} declared scroll_sample actions",
            expected.min_samples, sample_count
        ));
    }
    if expected.required_surfaces.is_empty() {
        return Err("scroll_expectations required_surfaces cannot be empty".to_owned());
    }
    let unique = expected
        .required_surfaces
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if unique.len() != expected.required_surfaces.len() {
        return Err("scroll_expectations required_surfaces cannot contain duplicates".to_owned());
    }
    if expected.expected_horizontal_releases > expected.expected_terminal_contacts {
        return Err(
            "scroll_expectations horizontal releases cannot exceed terminal contacts".to_owned(),
        );
    }
    if expected.expected_target_decisions != expected.expected_horizontal_releases
        || expected.expected_spring_generations != expected.expected_horizontal_releases
    {
        return Err(
            "scroll_expectations requires exactly one target decision and spring generation per horizontal release"
                .to_owned(),
        );
    }
    if expected.expected_releases.len() != expected.expected_horizontal_releases as usize {
        return Err(
            "scroll_expectations expected_releases must contain one entry per horizontal release"
                .to_owned(),
        );
    }
    let mut release_ids = BTreeSet::new();
    for release in &expected.expected_releases {
        if !release_ids.insert(release.gesture_id) {
            return Err(
                "scroll_expectations expected_releases cannot contain duplicate gesture_id values"
                    .to_owned(),
            );
        }
        if !expected.required_surfaces.contains(&release.surface) {
            return Err(format!(
                "scroll_expectations release gesture_id={} references a surface not listed in required_surfaces",
                release.gesture_id
            ));
        }
        if !release.min_filtered_velocity.is_finite()
            || !release.max_filtered_velocity.is_finite()
            || release.min_filtered_velocity > release.max_filtered_velocity
        {
            return Err(format!(
                "scroll_expectations release gesture_id={} has an invalid filtered velocity range",
                release.gesture_id
            ));
        }
        if !release.target_x.is_finite()
            || !release.target_tolerance.is_finite()
            || release.target_tolerance < 0.0
        {
            return Err(format!(
                "scroll_expectations release gesture_id={} has an invalid target contract",
                release.gesture_id
            ));
        }
        match (
            release.min_release_position_x,
            release.max_release_position_x,
        ) {
            (Some(min), Some(max)) if min.is_finite() && max.is_finite() && min <= max => {}
            (None, None) => {}
            _ => {
                return Err(format!(
                    "scroll_expectations release gesture_id={} has an invalid release position range",
                    release.gesture_id
                ));
            }
        }
        match (release.settled_position_x, release.max_settle_duration_ms) {
            (Some(position), Some(duration)) if position.is_finite() && duration > 0 => {}
            (None, None) => {}
            _ => {
                return Err(format!(
                    "scroll_expectations release gesture_id={} must declare settled_position_x and max_settle_duration_ms together",
                    release.gesture_id
                ));
            }
        }
    }
    Ok(())
}

fn validate_context_menu_expectations(scenario: &QaScenario) -> Result<(), String> {
    let Some(expected) = scenario.context_menu_expectations.as_ref() else {
        return Ok(());
    };
    if expected.expected_open_count == 0 {
        return Err(
            "context_menu_expectations expected_open_count must be greater than zero".to_owned(),
        );
    }
    if expected.min_open_frames == 0 {
        return Err(
            "context_menu_expectations min_open_frames must be greater than zero".to_owned(),
        );
    }
    if expected.expected_opening_count == Some(0) {
        return Err(
            "context_menu_expectations expected_opening_count must be greater than zero".to_owned(),
        );
    }
    if expected.expected_closed_count > expected.expected_closing_count {
        return Err(
            "context_menu_expectations expected_closed_count cannot exceed expected_closing_count"
                .to_owned(),
        );
    }
    if expected.expected_closing_count > 0 && expected.min_closing_frames == 0 {
        return Err(
            "context_menu_expectations min_closing_frames must be greater than zero".to_owned(),
        );
    }
    if expected.max_closing_frames == Some(0) {
        return Err(
            "context_menu_expectations max_closing_frames must be greater than zero".to_owned(),
        );
    }
    if expected.require_final_closed && expected.expected_closed_count == 0 {
        return Err(
            "context_menu_expectations require_final_closed needs expected_closed_count".to_owned(),
        );
    }
    if !scenario
        .actions
        .iter()
        .any(|action| matches!(action.action, QaAction::RightClick))
    {
        return Err(
            "context_menu_expectations requires at least one right_click action".to_owned(),
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QaTarget {
    Point { x: f32, y: f32 },
    GridItem { index: usize },
    GridItemPoint { index: usize, x: f32, y: f32 },
    ContextMenuRow { index: usize },
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
    pub context_menu_active: bool,
    pub context_menu_phase: String,
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
    pub frame_dt_ms: f32,
    pub pointer_x: f32,
    pub folder_scroll_input_target_x: Option<f32>,
    pub folder_scroll_input_error_x: Option<f32>,
    pub folder_scroll_settle_target_x: Option<f32>,
    pub folder_scroll_sample_count: Option<usize>,
    pub folder_scroll_frame_delta_x: Option<f32>,
    pub folder_pointer_move_serial: u64,
    pub folder_pointer_move_delta: u64,
    pub relayout_serial: u64,
    pub relayout_delta: u64,
    pub folder_child_page_target: Option<usize>,
    pub folder_child_page_hover_progress: Option<f32>,
    pub pager_surface: Option<String>,
    pub pager_state: Option<String>,
    pub pager_axis: Option<String>,
    pub pager_signed_displacement: Option<f32>,
    pub pager_position_x: Option<f32>,
    pub pager_velocity: Option<f32>,
    pub pager_filtered_velocity: Option<f32>,
    pub pager_settle_target_x: Option<f32>,
    pub pager_target_decision_count: Option<u32>,
    pub pager_spring_generation_count: Option<u32>,
    pub pager_reanchor_count: Option<u32>,
    pub pager_spring_id: Option<u64>,
    pub text_atlas_width: Option<u32>,
    pub text_atlas_height: Option<u32>,
    pub text_atlas_cached_glyphs: Option<usize>,
    pub text_atlas_cache_hits: Option<u64>,
    pub text_atlas_cache_misses: Option<u64>,
    pub text_atlas_grows: Option<u64>,
    pub text_atlas_drops: Option<u64>,
}

struct QaFrameState {
    editing: bool,
    context_menu_active: bool,
    context_menu_phase: String,
    folder_open: bool,
    folder_page: usize,
    renaming: bool,
    folder_rename_caret_visible: Option<bool>,
    folder_scroll: Option<(f32, f32, crate::scroll::Phase)>,
    folder_child_drag: bool,
    top_level_drag: bool,
    top_level_item_count: usize,
    active_folder_child_count: Option<usize>,
    frame_dt_ms: f32,
    pointer_x: f32,
    folder_scroll_diagnostics: Option<crate::scroll::ScrollDiagnostics>,
    folder_pointer_move_serial: u64,
    relayout_serial: u64,
    folder_child_page_target: Option<usize>,
    folder_child_page_hover_progress: Option<f32>,
    pager: Option<QaPagerSnapshot>,
    text_atlas: Option<crate::renderer::text_engine::TextAtlasStats>,
}

#[derive(Debug, Clone, Serialize)]
struct QaPagerSnapshot {
    surface: String,
    state: String,
    axis: String,
    signed_displacement: f32,
    position_x: f32,
    velocity: f32,
    filtered_velocity: f32,
    settle_target_x: Option<f32>,
    target_decision_count: u32,
    spring_generation_count: u32,
    reanchor_count: u32,
    spring_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct QaScrollTraceRecord {
    index: usize,
    gesture_id: u64,
    /// Scenario-relative time retained for readable, deterministic fixtures.
    timestamp_us: u64,
    /// The same sample translated into `App::scroll_clock_origin`'s epoch.
    dispatch_timestamp_us: u64,
    raw_dx: f32,
    raw_dy: f32,
    canonical_dx: f32,
    canonical_dy: f32,
    expected_canonical_dx: f32,
    expected_canonical_dy: f32,
    direction_inverted_from_device: bool,
    source: QaScrollSource,
    contact_phase: QaScrollPhase,
    momentum_phase: QaScrollPhase,
    sequence_complete: bool,
    before: Option<QaPagerSnapshot>,
    after: Option<QaPagerSnapshot>,
}

#[derive(Debug, Default, Serialize)]
struct QaScrollAssertions {
    passed: bool,
    sample_count: usize,
    snapshot_missing_count: u32,
    snapshot_surface_mismatch_count: u32,
    canonical_normalization_mismatch_count: u32,
    required_surface_missing_count: u32,
    zero_crossing_count: u32,
    zero_crossing_violation_count: u32,
    nonzero_input_stall_count: u32,
    pre_terminal_target_decision_count: u32,
    pre_terminal_spring_generation_count: u32,
    pre_terminal_reanchor_count: u32,
    release_count: u32,
    horizontal_release_count: u32,
    release_contract_violation_count: u32,
    release_expectation_missing_count: u32,
    release_expectation_mismatch_count: u32,
    release_position_mismatch_count: u32,
    settle_completion_missing_count: u32,
    release_target_decision_count: u32,
    release_spring_generation_count: u32,
    momentum_mutation_count: u32,
}

#[derive(Debug, Default, Serialize)]
struct QaContextMenuAssertions {
    passed: bool,
    opening_count: u32,
    open_entry_count: u32,
    open_frame_count: u32,
    closing_count: u32,
    closing_frame_count: u32,
    max_closing_streak_frames: u32,
    closed_entry_count: u32,
    final_phase: String,
}

#[derive(Debug, Serialize)]
struct QaManifest<'a> {
    scenario: &'a str,
    viewport: [u32; 2],
    fps: u32,
    duration_ms: u64,
    completed: bool,
    frames: &'a [QaFrameRecord],
    scroll_trace: &'a [QaScrollTraceRecord],
    scroll_expectations: Option<&'a QaScrollExpectations>,
    scroll_assertions: QaScrollAssertions,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_menu_expectations: Option<&'a QaContextMenuExpectations>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_menu_assertions: Option<QaContextMenuAssertions>,
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
    scroll_trace: Vec<QaScrollTraceRecord>,
    frame_ready: bool,
    animation_advanced: bool,
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
        expand_generated_apps(&mut scenario.fixture)?;
        scenario.actions.sort_by_key(|action| action.at_ms);
        normalize_scroll_actions(&mut scenario.actions)?;
        validate_native_scroll_actions(&scenario.actions)?;
        validate_scroll_expectations(&scenario)?;
        validate_context_menu_expectations(&scenario)?;
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
            scroll_trace: Vec::new(),
            frame_ready: false,
            animation_advanced: false,
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

    fn wall_elapsed_ms(&self, now: Instant) -> u64 {
        self.start
            .map(|start| now.saturating_duration_since(start).as_millis() as u64)
            .unwrap_or(0)
    }

    /// Prepare one fixed-step scenario frame once its wall-clock pacing
    /// deadline is reached. Scenario actions use the frame's virtual timestamp,
    /// so a slow render cannot skip ahead to later actions or end the run before
    /// animations have received their expected number of steps.
    pub fn prepare_due_frame(&mut self, now: Instant) -> Vec<QaAction> {
        if self.frame_ready
            || self.finished()
            || self.start.is_none()
            || self.wall_elapsed_ms(now) < self.next_capture_ms
        {
            return Vec::new();
        }
        self.frame_ready = true;
        let elapsed = self.next_capture_ms;
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

    pub fn capture_due(&self) -> bool {
        self.frame_ready
    }

    /// Advance app animations once per captured QA frame. Hidden-window QA can
    /// receive several redraw callbacks within one OS scheduling slice; using
    /// the wall time between those callbacks would make animation progress
    /// depend on callback frequency and could leave a close transition in
    /// `Closing` indefinitely. The scenario FPS is the deterministic clock.
    pub fn animation_dt(&mut self) -> f32 {
        if self.capture_due() && !self.animation_advanced {
            self.animation_advanced = true;
            1.0 / self.scenario.fps.max(1) as f32
        } else {
            0.0
        }
    }

    fn next_capture_path(&mut self, state: QaFrameState) -> Option<PathBuf> {
        if !self.capture_due() {
            return None;
        }
        let elapsed_ms = self.next_capture_ms;
        let file = format!("frame_{:06}.png", self.frame_index);
        let previous = self.frames.last();
        let folder_scroll_frame_delta_x = state.folder_scroll.map(|scroll| {
            scroll.0
                - previous
                    .and_then(|frame| frame.folder_scroll_x)
                    .unwrap_or(scroll.0)
        });
        let folder_pointer_move_delta = state.folder_pointer_move_serial.saturating_sub(
            previous
                .map(|frame| frame.folder_pointer_move_serial)
                .unwrap_or(state.folder_pointer_move_serial),
        );
        let relayout_delta = state.relayout_serial.saturating_sub(
            previous
                .map(|frame| frame.relayout_serial)
                .unwrap_or(state.relayout_serial),
        );
        let folder_scroll_input_target_x = state
            .folder_scroll_diagnostics
            .and_then(|diagnostics| diagnostics.input_target);
        let folder_scroll_input_error_x = state
            .folder_scroll
            .and_then(|scroll| folder_scroll_input_target_x.map(|target| scroll.0 - target));
        self.frames.push(QaFrameRecord {
            index: self.frame_index,
            elapsed_ms,
            file: file.clone(),
            editing: state.editing,
            context_menu_active: state.context_menu_active,
            context_menu_phase: state.context_menu_phase,
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
            frame_dt_ms: state.frame_dt_ms,
            pointer_x: state.pointer_x,
            folder_scroll_input_target_x,
            folder_scroll_input_error_x,
            folder_scroll_settle_target_x: state
                .folder_scroll_diagnostics
                .and_then(|diagnostics| diagnostics.settle_target),
            folder_scroll_sample_count: state
                .folder_scroll_diagnostics
                .map(|diagnostics| diagnostics.velocity_sample_count),
            folder_scroll_frame_delta_x,
            folder_pointer_move_serial: state.folder_pointer_move_serial,
            folder_pointer_move_delta,
            relayout_serial: state.relayout_serial,
            relayout_delta,
            folder_child_page_target: state.folder_child_page_target,
            folder_child_page_hover_progress: state.folder_child_page_hover_progress,
            pager_surface: state.pager.as_ref().map(|pager| pager.surface.clone()),
            pager_state: state.pager.as_ref().map(|pager| pager.state.clone()),
            pager_axis: state.pager.as_ref().map(|pager| pager.axis.clone()),
            pager_signed_displacement: state.pager.as_ref().map(|pager| pager.signed_displacement),
            pager_position_x: state.pager.as_ref().map(|pager| pager.position_x),
            pager_velocity: state.pager.as_ref().map(|pager| pager.velocity),
            pager_filtered_velocity: state.pager.as_ref().map(|pager| pager.filtered_velocity),
            pager_settle_target_x: state.pager.as_ref().and_then(|pager| pager.settle_target_x),
            pager_target_decision_count: state
                .pager
                .as_ref()
                .map(|pager| pager.target_decision_count),
            pager_spring_generation_count: state
                .pager
                .as_ref()
                .map(|pager| pager.spring_generation_count),
            pager_reanchor_count: state.pager.as_ref().map(|pager| pager.reanchor_count),
            pager_spring_id: state.pager.as_ref().and_then(|pager| pager.spring_id),
            text_atlas_width: state.text_atlas.map(|stats| stats.width),
            text_atlas_height: state.text_atlas.map(|stats| stats.height),
            text_atlas_cached_glyphs: state.text_atlas.map(|stats| stats.cached_glyphs),
            text_atlas_cache_hits: state.text_atlas.map(|stats| stats.cache_hits),
            text_atlas_cache_misses: state.text_atlas.map(|stats| stats.cache_misses),
            text_atlas_grows: state.text_atlas.map(|stats| stats.grows),
            text_atlas_drops: state.text_atlas.map(|stats| stats.atlas_drops),
        });
        self.complete_capture();
        Some(self.run_dir.join(file))
    }

    fn complete_capture(&mut self) {
        self.frame_index += 1;
        self.frame_ready = false;
        self.animation_advanced = false;
        let frame_ms = (1000 / self.scenario.fps.max(1) as u64).max(1);
        self.next_capture_ms = self.next_capture_ms.saturating_add(frame_ms);
    }

    pub fn finished(&self) -> bool {
        self.start.is_some()
            && !self.frame_ready
            && self.next_capture_ms >= self.scenario.duration_ms
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let start = self.start?;
        let next_ms = self.next_capture_ms.min(self.scenario.duration_ms);
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
            scroll_trace: &self.scroll_trace,
            scroll_expectations: self.scenario.scroll_expectations.as_ref(),
            scroll_assertions: evaluate_scroll_assertions(
                &self.scroll_trace,
                &self.frames,
                self.scenario.scroll_expectations.as_ref(),
            ),
            context_menu_expectations: self.scenario.context_menu_expectations.as_ref(),
            context_menu_assertions: self
                .scenario
                .context_menu_expectations
                .as_ref()
                .map(|expected| evaluate_context_menu_assertions(&self.frames, expected)),
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

    fn record_scroll_sample(&mut self, mut record: QaScrollTraceRecord) {
        record.index = self.scroll_trace.len();
        self.scroll_trace.push(record);
    }
}

fn qa_timestamp_in_app_epoch(
    app_scroll_clock_origin: Instant,
    qa_start: Instant,
    scenario_timestamp_us: u64,
) -> u64 {
    let qa_epoch_offset_us = qa_start
        .saturating_duration_since(app_scroll_clock_origin)
        .as_micros();
    let qa_epoch_offset_us = u64::try_from(qa_epoch_offset_us).unwrap_or(u64::MAX);
    qa_epoch_offset_us.saturating_add(scenario_timestamp_us)
}

fn counter_delta(before: u32, after: u32) -> u32 {
    after.saturating_sub(before)
}

fn snapshots_bitwise_equal(before: &QaPagerSnapshot, after: &QaPagerSnapshot) -> bool {
    before.surface == after.surface
        && before.state == after.state
        && before.axis == after.axis
        && before.signed_displacement.to_bits() == after.signed_displacement.to_bits()
        && before.position_x.to_bits() == after.position_x.to_bits()
        && before.velocity.to_bits() == after.velocity.to_bits()
        && before.filtered_velocity.to_bits() == after.filtered_velocity.to_bits()
        && before.settle_target_x.map(f32::to_bits) == after.settle_target_x.map(f32::to_bits)
        && before.target_decision_count == after.target_decision_count
        && before.spring_generation_count == after.spring_generation_count
        && before.reanchor_count == after.reanchor_count
        && before.spring_id == after.spring_id
}

fn evaluate_scroll_assertions(
    trace: &[QaScrollTraceRecord],
    frames: &[QaFrameRecord],
    expected: Option<&QaScrollExpectations>,
) -> QaScrollAssertions {
    use std::collections::BTreeSet;

    let mut result = QaScrollAssertions {
        sample_count: trace.len(),
        ..QaScrollAssertions::default()
    };
    let mut observed_surfaces = BTreeSet::new();

    for sample in trace {
        if (sample.canonical_dx - sample.expected_canonical_dx).abs() > 0.001
            || (sample.canonical_dy - sample.expected_canonical_dy).abs() > 0.001
        {
            result.canonical_normalization_mismatch_count += 1;
        }
        let (Some(before), Some(after)) = (&sample.before, &sample.after) else {
            result.snapshot_missing_count += 1;
            continue;
        };
        observed_surfaces.insert(before.surface.as_str());
        observed_surfaces.insert(after.surface.as_str());
        if before.surface != after.surface {
            result.snapshot_surface_mismatch_count += 1;
        }
        let target_delta = counter_delta(before.target_decision_count, after.target_decision_count);
        let spring_delta = counter_delta(
            before.spring_generation_count,
            after.spring_generation_count,
        );
        let reanchor_delta = counter_delta(before.reanchor_count, after.reanchor_count);

        if matches!(
            sample.contact_phase,
            QaScrollPhase::Began | QaScrollPhase::Changed
        ) {
            result.pre_terminal_target_decision_count += target_delta;
            result.pre_terminal_spring_generation_count += spring_delta;
            result.pre_terminal_reanchor_count += reanchor_delta;
        }
        if sample.contact_phase.is_terminal() {
            result.release_count += 1;
            result.release_target_decision_count += target_delta;
            result.release_spring_generation_count += spring_delta;
            let accepted_horizontal_release = before.state == "WheelGesture"
                && after.state == "Settling"
                && after.axis == "Horizontal";
            if accepted_horizontal_release {
                result.horizontal_release_count += 1;
                if target_delta != 1
                    || spring_delta != 1
                    || reanchor_delta != 0
                    || after.spring_id.is_none()
                {
                    result.release_contract_violation_count += 1;
                }
            }
        }
        if sample.momentum_phase != QaScrollPhase::None && !snapshots_bitwise_equal(before, after) {
            result.momentum_mutation_count += 1;
        }

        let crossed_zero = sample.contact_phase == QaScrollPhase::Changed
            && before.signed_displacement != 0.0
            && before.signed_displacement != after.signed_displacement
            && (after.signed_displacement == 0.0
                || before.signed_displacement.signum() != after.signed_displacement.signum());
        if crossed_zero {
            result.zero_crossing_count += 1;
            let unexplained_position_jump =
                (after.position_x - before.position_x).abs() > sample.canonical_dx.abs() + 1.0;
            let changed_phase = before.state != after.state;
            if unexplained_position_jump
                || changed_phase
                || target_delta != 0
                || spring_delta != 0
                || reanchor_delta != 0
            {
                result.zero_crossing_violation_count += 1;
            }
        }

        if sample.contact_phase == QaScrollPhase::Changed
            && sample.canonical_dx != 0.0
            && after.axis != "Vertical"
            && before.position_x.to_bits() == after.position_x.to_bits()
        {
            result.nonzero_input_stall_count += 1;
        }
    }

    if let Some(expected) = expected {
        result.required_surface_missing_count = expected
            .required_surfaces
            .iter()
            .filter(|surface| !observed_surfaces.contains(surface.as_str()))
            .count() as u32;
        for release in &expected.expected_releases {
            let observed = trace.iter().find(|sample| {
                if sample.gesture_id != release.gesture_id || !sample.contact_phase.is_terminal() {
                    return false;
                }
                let (Some(before), Some(after)) = (&sample.before, &sample.after) else {
                    return false;
                };
                before.surface == release.surface.as_str()
                    && after.surface == release.surface.as_str()
                    && before.state == "WheelGesture"
                    && after.state == "Settling"
                    && after.axis == "Horizontal"
            });
            let Some(observed) = observed else {
                result.release_expectation_missing_count += 1;
                continue;
            };
            let after = observed
                .after
                .as_ref()
                .expect("release expectation lookup requires an after snapshot");
            let velocity_matches = after.filtered_velocity >= release.min_filtered_velocity
                && after.filtered_velocity <= release.max_filtered_velocity;
            let target_matches = after.settle_target_x.is_some_and(|target| {
                (target - release.target_x).abs() <= release.target_tolerance
            });
            if !velocity_matches || !target_matches {
                result.release_expectation_mismatch_count += 1;
            }
            if let (Some(min), Some(max)) = (
                release.min_release_position_x,
                release.max_release_position_x,
            ) {
                if !(min..=max).contains(&after.position_x) {
                    result.release_position_mismatch_count += 1;
                }
            }
            if let (Some(settled_position), Some(max_duration_ms)) =
                (release.settled_position_x, release.max_settle_duration_ms)
            {
                let release_ms = observed.timestamp_us / 1_000;
                let deadline_ms = release_ms.saturating_add(max_duration_ms);
                let completed_in_time = frames.iter().any(|frame| {
                    frame.elapsed_ms >= release_ms
                        && frame.elapsed_ms <= deadline_ms
                        && frame.pager_surface.as_deref() == Some(release.surface.as_str())
                        && frame.pager_state.as_deref() == Some("Idle")
                        && frame.pager_position_x.is_some_and(|position| {
                            (position - settled_position).abs() <= release.target_tolerance
                        })
                });
                if !completed_in_time {
                    result.settle_completion_missing_count += 1;
                }
            }
        }
        result.passed = !trace.is_empty()
            && result.sample_count >= expected.min_samples
            && result.snapshot_missing_count == 0
            && result.snapshot_surface_mismatch_count == 0
            && result.canonical_normalization_mismatch_count == 0
            && result.required_surface_missing_count == 0
            && result.zero_crossing_count >= expected.min_zero_crossings
            && result.zero_crossing_violation_count == 0
            && result.nonzero_input_stall_count == 0
            && result.pre_terminal_target_decision_count == 0
            && result.pre_terminal_spring_generation_count == 0
            && result.pre_terminal_reanchor_count == 0
            && result.release_count == expected.expected_terminal_contacts
            && result.horizontal_release_count == expected.expected_horizontal_releases
            && result.release_contract_violation_count == 0
            && result.release_expectation_missing_count == 0
            && result.release_expectation_mismatch_count == 0
            && result.release_position_mismatch_count == 0
            && result.settle_completion_missing_count == 0
            && result.release_target_decision_count == expected.expected_target_decisions
            && result.release_spring_generation_count == expected.expected_spring_generations
            && result.momentum_mutation_count == 0;
    }
    result
}

fn evaluate_context_menu_assertions(
    frames: &[QaFrameRecord],
    expected: &QaContextMenuExpectations,
) -> QaContextMenuAssertions {
    evaluate_context_menu_phases(
        frames.iter().map(|frame| frame.context_menu_phase.as_str()),
        expected,
    )
}

fn evaluate_context_menu_phases<'a>(
    phases: impl IntoIterator<Item = &'a str>,
    expected: &QaContextMenuExpectations,
) -> QaContextMenuAssertions {
    let mut result = QaContextMenuAssertions::default();
    let mut previous_phase = "Closed";
    let mut closing_streak_frames = 0_u32;
    for phase in phases {
        if phase == "Opening" && previous_phase != "Opening" {
            result.opening_count += 1;
        }
        if phase == "Open" {
            result.open_frame_count += 1;
            if previous_phase != "Open" {
                result.open_entry_count += 1;
            }
        }
        if phase == "Closing" {
            result.closing_frame_count += 1;
            closing_streak_frames += 1;
            result.max_closing_streak_frames =
                result.max_closing_streak_frames.max(closing_streak_frames);
            if previous_phase != "Closing" {
                result.closing_count += 1;
            }
        } else {
            closing_streak_frames = 0;
        }
        if phase == "Closed" && previous_phase != "Closed" {
            result.closed_entry_count += 1;
        }
        previous_phase = phase;
    }
    result.final_phase = previous_phase.to_owned();
    let expected_opening_count = expected
        .expected_opening_count
        .unwrap_or(expected.expected_open_count);
    result.passed = result.opening_count == expected_opening_count
        && result.open_entry_count == expected.expected_open_count
        && result.open_frame_count >= expected.min_open_frames;
    result.passed = result.passed
        && result.closing_count == expected.expected_closing_count
        && result.closed_entry_count == expected.expected_closed_count
        && result.closing_frame_count
            >= expected
                .expected_closing_count
                .saturating_mul(expected.min_closing_frames)
        && expected
            .max_closing_frames
            .is_none_or(|max_frames| result.max_closing_streak_frames <= max_frames)
        && (!expected.require_final_closed || result.final_phase == "Closed");
    result
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
        self.atlas = crate::renderer::icon_atlas::IconAtlas::new(64);
        self.atlas_uploaded = false;
        for (index, app) in fixture.apps.iter().enumerate() {
            let id = AppId::from_normalized(app.id.clone());
            let slot = self.registry.alloc_slot();
            let (r, g, b) = crate::layout::grid::app_color(index);
            let to_byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
            let rgba = [to_byte(r), to_byte(g), to_byte(b), 255];
            let mut pixels = vec![0u8; (TARGET * TARGET * 4) as usize];
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.copy_from_slice(&rgba);
            }
            let icon = DecodedIcon {
                rgba: pixels,
                w: TARGET,
                h: TARGET,
            };
            let (_, _, uv) = self.atlas.write_icon(slot, &icon);
            self.registry.insert(AppRecord {
                app_id: id,
                name: app.name.clone(),
                link_path: PathBuf::from(format!("qa/{}.lnk", app.id)),
                resolved_target: PathBuf::from(format!("qa/{}.exe", app.id)),
                slot,
                icon_state: IconState::Loaded,
                uv: Some(uv),
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
            .map(|runner| runner.prepare_due_frame(now))
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
            QaAction::RightClick => {
                self.handle_routed_pointer_button(crate::input_routing::PointerButton::Right, true);
                self.handle_routed_pointer_button(
                    crate::input_routing::PointerButton::Right,
                    false,
                );
            }
            QaAction::PointerDown => {
                let action = self.classify_pointer_press(self.pointer_phys_x, self.pointer_phys_y);
                self.handle_action(AppAction::PointerPress(action));
            }
            QaAction::LongPress => {
                let action = self.classify_pointer_press(self.pointer_phys_x, self.pointer_phys_y);
                self.handle_action(AppAction::PointerPress(action));
                if let Some(ready_at) = self
                    .pending_press
                    .as_ref()
                    .map(|press| press.start + crate::features::edit_mode::LONG_PRESS_THRESHOLD)
                {
                    // Keep deterministic QA independent of shader warm-up and
                    // runner speed while exercising the production long-press
                    // threshold, slop, and edit-entry path.
                    self.maybe_long_press_into_edit(ready_at);
                }
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
                } else if self.context_menu.is_active() {
                    KeyAction::CloseContextMenu
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
            QaAction::ScrollSample {
                gesture_id,
                timestamp_us,
                canonical_dx,
                canonical_dy,
                source,
                contact_phase,
                momentum_phase,
                sequence_complete,
            } => {
                let gesture_id =
                    gesture_id.expect("scroll gesture ids are resolved while loading the scenario");
                let dispatch_timestamp_us = self
                    .qa_runner
                    .as_ref()
                    .and_then(|runner| runner.start)
                    .map(|qa_start| {
                        qa_timestamp_in_app_epoch(self.scroll_clock_origin, qa_start, timestamp_us)
                    })
                    .unwrap_or(timestamp_us);
                let before = self.qa_pager_snapshot();

                self.handle_action(AppAction::ScrollSample(
                    crate::input_routing::ScrollSample {
                        gesture_id,
                        timestamp_us: dispatch_timestamp_us,
                        raw_dx: canonical_dx,
                        raw_dy: canonical_dy,
                        canonical_dx,
                        canonical_dy,
                        source: match source {
                            QaScrollSource::Precise => crate::input_routing::ScrollSource::Precise,
                            QaScrollSource::Line => crate::input_routing::ScrollSource::Line,
                        },
                        contact_phase: qa_native_scroll_phase(contact_phase),
                        momentum_phase: qa_native_scroll_phase(momentum_phase),
                        sequence_complete,
                        scale_factor: 1.0,
                        direction_inverted_from_device: false,
                        phase_capability: crate::input_routing::ScrollPhaseCapability::Separate,
                    },
                ));

                let after = self.qa_pager_snapshot();
                if let Some(runner) = self.qa_runner.as_mut() {
                    runner.record_scroll_sample(QaScrollTraceRecord {
                        index: 0,
                        gesture_id,
                        timestamp_us,
                        dispatch_timestamp_us,
                        raw_dx: canonical_dx,
                        raw_dy: canonical_dy,
                        canonical_dx,
                        canonical_dy,
                        expected_canonical_dx: canonical_dx,
                        expected_canonical_dy: canonical_dy,
                        direction_inverted_from_device: false,
                        source,
                        contact_phase,
                        momentum_phase,
                        sequence_complete,
                        before,
                        after,
                    });
                }
            }
            QaAction::NativeScrollSample {
                timestamp_us,
                raw_dx,
                raw_dy,
                expected_canonical_dx,
                expected_canonical_dy,
                source,
                contact_phase,
                momentum_phase,
                sequence_complete,
                direction_inverted_from_device,
                scale_factor,
            } => {
                let dispatch_timestamp_us = self
                    .qa_runner
                    .as_ref()
                    .and_then(|runner| runner.start)
                    .map(|qa_start| {
                        qa_timestamp_in_app_epoch(self.scroll_clock_origin, qa_start, timestamp_us)
                    })
                    .unwrap_or(timestamp_us);
                let before = self.qa_pager_snapshot();
                let Some(sample) =
                    self.scroll_sample_adapter
                        .adapt_native(crate::input_routing::RawScrollEvent {
                            timestamp_us: dispatch_timestamp_us,
                            delta_physical_px: (raw_dx, raw_dy),
                            source: match source {
                                QaScrollSource::Precise => {
                                    crate::input_routing::ScrollSource::Precise
                                }
                                QaScrollSource::Line => crate::input_routing::ScrollSource::Line,
                            },
                            contact_phase: qa_native_scroll_phase(contact_phase),
                            momentum_phase: qa_native_scroll_phase(momentum_phase),
                            sequence_complete,
                            direction_inverted_from_device,
                            scale_factor,
                            phase_capability: crate::input_routing::ScrollPhaseCapability::Separate,
                        })
                else {
                    return;
                };
                self.handle_action(AppAction::ScrollSample(sample));
                let after = self.qa_pager_snapshot();
                if let Some(runner) = self.qa_runner.as_mut() {
                    runner.record_scroll_sample(QaScrollTraceRecord {
                        index: 0,
                        gesture_id: sample.gesture_id,
                        timestamp_us,
                        dispatch_timestamp_us,
                        raw_dx,
                        raw_dy,
                        canonical_dx: sample.canonical_dx,
                        canonical_dy: sample.canonical_dy,
                        expected_canonical_dx,
                        expected_canonical_dy,
                        direction_inverted_from_device,
                        source,
                        contact_phase,
                        momentum_phase,
                        sequence_complete,
                        before,
                        after,
                    });
                }
            }
        }
    }

    fn qa_pager_snapshot(&self) -> Option<QaPagerSnapshot> {
        let (surface, scroller) = if self.folders.is_active() {
            ("folder", self.folder_scroller.as_ref()?)
        } else {
            ("main", self.scroller.as_ref()?)
        };
        let diagnostics = scroller.wheel_diagnostics();
        Some(QaPagerSnapshot {
            surface: surface.to_owned(),
            state: format!("{:?}", scroller.phase),
            axis: format!("{:?}", diagnostics.axis),
            signed_displacement: diagnostics.signed_displacement,
            position_x: scroller.position,
            velocity: scroller.velocity,
            filtered_velocity: diagnostics.filtered_velocity,
            settle_target_x: diagnostics.settle_target,
            target_decision_count: diagnostics.target_decision_count,
            spring_generation_count: diagnostics.spring_generation_count,
            reanchor_count: diagnostics.reanchor_count,
            spring_id: diagnostics.spring_id,
        })
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
            QaTarget::ContextMenuRow { index } => self
                .context_menu_layout
                .as_ref()?
                .rows
                .get(*index)
                .map(|row| row.rect.center()),
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

    pub(crate) fn qa_capture_path(&mut self) -> Option<PathBuf> {
        let editing = self.editing;
        let context_menu_active = self.context_menu.is_active();
        let context_menu_phase = format!("{:?}", self.context_menu.phase);
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
        let folder_scroll_diagnostics = self
            .folder_scroller
            .as_ref()
            .map(|scroller| scroller.diagnostics(self.pointer_phys_x));
        let folder_child_page_target = self
            .folders
            .child_page_hover
            .as_ref()
            .map(|hover| hover.target);
        let folder_child_page_hover_progress =
            self.folders.child_page_hover.as_ref().map(|hover| {
                (hover.elapsed / crate::features::folders::CHILD_PAGE_EDGE_DWELL).clamp(0.0, 1.0)
            });
        let pager = self.qa_pager_snapshot();
        let text_atlas = self.text.as_ref().map(|text| text.atlas_stats());
        self.qa_runner.as_mut()?.next_capture_path(QaFrameState {
            editing,
            context_menu_active,
            context_menu_phase,
            folder_open,
            folder_page,
            renaming,
            folder_rename_caret_visible,
            folder_scroll,
            folder_child_drag,
            top_level_drag,
            top_level_item_count,
            active_folder_child_count,
            frame_dt_ms: self.last_frame_dt_ms,
            pointer_x: self.pointer_phys_x,
            folder_scroll_diagnostics,
            folder_pointer_move_serial: self.folder_pointer_move_serial,
            relayout_serial: self.relayout_serial,
            folder_child_page_target,
            folder_child_page_hover_progress,
            pager,
            text_atlas,
        })
    }

    pub(crate) fn qa_capture_due(&self) -> bool {
        self.qa_runner.as_ref().is_some_and(QaRunner::capture_due)
    }

    pub(crate) fn qa_finished(&self) -> bool {
        self.qa_runner.as_ref().is_some_and(QaRunner::finished)
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

fn qa_native_scroll_phase(phase: QaScrollPhase) -> crate::input_routing::NativeScrollPhase {
    match phase {
        QaScrollPhase::None => crate::input_routing::NativeScrollPhase::None,
        QaScrollPhase::Began => crate::input_routing::NativeScrollPhase::Began,
        QaScrollPhase::Changed => crate::input_routing::NativeScrollPhase::Changed,
        QaScrollPhase::Ended => crate::input_routing::NativeScrollPhase::Ended,
        QaScrollPhase::Cancelled => crate::input_routing::NativeScrollPhase::Cancelled,
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
                {"at_ms": 250, "type": "pointer_down"},
                {"at_ms": 300, "type": "long_press"}
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
        assert!(matches!(scenario.actions[3].action, QaAction::LongPress));
    }

    #[test]
    fn runner_orders_actions_and_sanitizes_run_name() {
        assert_eq!(sanitize_name("Folder QA / 1"), "Folder-QA---1");
    }

    #[test]
    fn slow_wall_clock_cannot_overtake_the_fixed_step_scenario_clock() {
        let mut runner = QaRunner {
            scenario: QaScenario {
                name: "fixed-step-clock".to_owned(),
                viewport: [1280, 800],
                fps: 30,
                duration_ms: 150,
                output_dir: PathBuf::new(),
                fixture: QaFixture {
                    apps: Vec::new(),
                    generated_apps: Vec::new(),
                    folders: Vec::new(),
                    items: Vec::new(),
                },
                scroll_expectations: None,
                context_menu_expectations: None,
                actions: vec![TimedAction {
                    at_ms: 100,
                    action: QaAction::RightClick,
                }],
            },
            scenario_path: PathBuf::new(),
            run_dir: PathBuf::new(),
            start: None,
            next_action: 0,
            next_capture_ms: 0,
            frame_index: 0,
            frames: Vec::new(),
            scroll_trace: Vec::new(),
            frame_ready: false,
            animation_advanced: false,
            finalized: false,
        };
        let start = Instant::now();
        let slow_now = start + Duration::from_secs(5);
        runner.start(start);

        // Even though five wall-clock seconds have elapsed, the first four
        // fixed frames are still 0, 33, 66, and 99 ms in scenario time.
        for expected_ms in [0, 33, 66, 99] {
            assert!(runner.prepare_due_frame(slow_now).is_empty());
            assert_eq!(runner.next_capture_ms, expected_ms);
            assert!(runner.capture_due());
            assert!(!runner.finished());
            assert!((runner.animation_dt() - 1.0 / 30.0).abs() < f32::EPSILON);
            assert_eq!(runner.animation_dt(), 0.0);
            runner.complete_capture();
        }

        let actions = runner.prepare_due_frame(slow_now);
        assert!(matches!(actions.as_slice(), [QaAction::RightClick]));
        assert_eq!(runner.next_capture_ms, 132);
        assert!(!runner.finished());
        runner.complete_capture();
        assert!(runner.finished());
    }

    fn scroll_action(
        at_ms: u64,
        gesture_id: Option<u64>,
        timestamp_us: u64,
        contact_phase: QaScrollPhase,
        momentum_phase: QaScrollPhase,
    ) -> TimedAction {
        TimedAction {
            at_ms,
            action: QaAction::ScrollSample {
                gesture_id,
                timestamp_us,
                canonical_dx: 0.0,
                canonical_dy: 0.0,
                source: QaScrollSource::Precise,
                contact_phase,
                momentum_phase,
                sequence_complete: false,
            },
        }
    }

    fn gesture_id(action: &TimedAction) -> Option<u64> {
        match action.action {
            QaAction::ScrollSample { gesture_id, .. } => gesture_id,
            _ => None,
        }
    }

    #[test]
    fn scroll_schema_assigns_ids_and_keeps_old_momentum_separate() {
        let mut actions = vec![
            scroll_action(0, None, 0, QaScrollPhase::Began, QaScrollPhase::None),
            scroll_action(
                16,
                None,
                16_000,
                QaScrollPhase::Changed,
                QaScrollPhase::None,
            ),
            scroll_action(32, None, 32_000, QaScrollPhase::Ended, QaScrollPhase::None),
            scroll_action(
                48,
                Some(2),
                48_000,
                QaScrollPhase::Began,
                QaScrollPhase::None,
            ),
            scroll_action(
                64,
                Some(2),
                64_000,
                QaScrollPhase::Changed,
                QaScrollPhase::None,
            ),
            scroll_action(
                72,
                Some(1),
                72_000,
                QaScrollPhase::None,
                QaScrollPhase::Changed,
            ),
            scroll_action(
                80,
                Some(1),
                80_000,
                QaScrollPhase::None,
                QaScrollPhase::Ended,
            ),
        ];

        normalize_scroll_actions(&mut actions).unwrap();
        assert_eq!(
            actions.iter().map(gesture_id).collect::<Vec<_>>(),
            vec![
                Some(1),
                Some(1),
                Some(1),
                Some(2),
                Some(2),
                Some(1),
                Some(1)
            ]
        );
    }

    #[test]
    fn scroll_schema_rejects_second_active_contact_and_timestamp_reversal() {
        let mut overlapping = vec![
            scroll_action(0, Some(7), 0, QaScrollPhase::Began, QaScrollPhase::None),
            scroll_action(1, Some(8), 1, QaScrollPhase::Began, QaScrollPhase::None),
        ];
        assert!(normalize_scroll_actions(&mut overlapping)
            .unwrap_err()
            .contains("second active contact"));

        let mut reversed_time = vec![
            scroll_action(0, Some(7), 10, QaScrollPhase::Began, QaScrollPhase::None),
            scroll_action(1, Some(7), 9, QaScrollPhase::Changed, QaScrollPhase::None),
        ];
        assert!(normalize_scroll_actions(&mut reversed_time)
            .unwrap_err()
            .contains("moves backwards"));
    }

    #[test]
    fn cancelled_contact_completes_without_momentum_quarantine() {
        let mut valid = vec![
            scroll_action(0, Some(7), 0, QaScrollPhase::Began, QaScrollPhase::None),
            scroll_action(1, Some(7), 1, QaScrollPhase::Cancelled, QaScrollPhase::None),
            scroll_action(2, Some(8), 2, QaScrollPhase::Began, QaScrollPhase::None),
        ];
        normalize_scroll_actions(&mut valid).unwrap();

        let mut stale_momentum = vec![
            scroll_action(0, Some(7), 0, QaScrollPhase::Began, QaScrollPhase::None),
            scroll_action(1, Some(7), 1, QaScrollPhase::Cancelled, QaScrollPhase::None),
            scroll_action(2, Some(7), 2, QaScrollPhase::None, QaScrollPhase::Changed),
        ];
        assert!(normalize_scroll_actions(&mut stale_momentum)
            .unwrap_err()
            .contains("non-quarantined"));
    }

    #[test]
    fn sequence_complete_must_be_phase_less_and_zero_delta() {
        let mut actions = vec![TimedAction {
            at_ms: 0,
            action: QaAction::ScrollSample {
                gesture_id: Some(1),
                timestamp_us: 0,
                canonical_dx: 1.0,
                canonical_dy: 0.0,
                source: QaScrollSource::Precise,
                contact_phase: QaScrollPhase::None,
                momentum_phase: QaScrollPhase::None,
                sequence_complete: true,
            },
        }];
        assert!(normalize_scroll_actions(&mut actions)
            .unwrap_err()
            .contains("zero-delta phase-less terminal"));
    }

    fn main_scroll_contract(
        min_samples: usize,
        terminal_contacts: u32,
        horizontal_releases: u32,
    ) -> QaScrollExpectations {
        QaScrollExpectations {
            min_samples,
            expected_terminal_contacts: terminal_contacts,
            expected_horizontal_releases: horizontal_releases,
            expected_target_decisions: horizontal_releases,
            expected_spring_generations: horizontal_releases,
            expected_releases: (1..=u64::from(horizontal_releases))
                .map(|gesture_id| QaReleaseExpectation {
                    gesture_id,
                    surface: QaPagerSurface::Main,
                    min_filtered_velocity: -f32::MAX,
                    max_filtered_velocity: f32::MAX,
                    target_x: 0.0,
                    target_tolerance: 1.0,
                    min_release_position_x: None,
                    max_release_position_x: None,
                    settled_position_x: None,
                    max_settle_duration_ms: None,
                })
                .collect(),
            min_zero_crossings: 0,
            required_surfaces: vec![QaPagerSurface::Main],
        }
    }

    fn pager_snapshot(
        state: &str,
        target_decisions: u32,
        spring_generations: u32,
        spring_id: Option<u64>,
    ) -> QaPagerSnapshot {
        QaPagerSnapshot {
            surface: "main".to_owned(),
            state: state.to_owned(),
            axis: "Horizontal".to_owned(),
            signed_displacement: 0.0,
            position_x: 0.0,
            velocity: 0.0,
            filtered_velocity: 0.0,
            settle_target_x: (state == "Settling").then_some(0.0),
            target_decision_count: target_decisions,
            spring_generation_count: spring_generations,
            reanchor_count: 0,
            spring_id,
        }
    }

    fn terminal_trace(
        before: Option<QaPagerSnapshot>,
        after: Option<QaPagerSnapshot>,
    ) -> QaScrollTraceRecord {
        QaScrollTraceRecord {
            index: 0,
            gesture_id: 1,
            timestamp_us: 0,
            dispatch_timestamp_us: 0,
            raw_dx: 0.0,
            raw_dy: 0.0,
            canonical_dx: 0.0,
            canonical_dy: 0.0,
            expected_canonical_dx: 0.0,
            expected_canonical_dy: 0.0,
            direction_inverted_from_device: false,
            source: QaScrollSource::Precise,
            contact_phase: QaScrollPhase::Ended,
            momentum_phase: QaScrollPhase::None,
            sequence_complete: false,
            before,
            after,
        }
    }

    #[test]
    fn empty_trace_and_missing_snapshots_cannot_pass_scroll_contract() {
        let contract = main_scroll_contract(1, 0, 0);
        let empty = evaluate_scroll_assertions(&[], &[], Some(&contract));
        assert!(!empty.passed);
        assert_eq!(empty.required_surface_missing_count, 1);

        let missing = evaluate_scroll_assertions(
            &[terminal_trace(
                None,
                Some(pager_snapshot("Idle", 0, 0, None)),
            )],
            &[],
            Some(&contract),
        );
        assert!(!missing.passed);
        assert_eq!(missing.snapshot_missing_count, 1);
    }

    #[test]
    fn release_requires_exactly_one_target_and_spring() {
        let contract = main_scroll_contract(1, 1, 1);
        for generated in [0, 2] {
            let result = evaluate_scroll_assertions(
                &[terminal_trace(
                    Some(pager_snapshot("WheelGesture", 0, 0, None)),
                    Some(pager_snapshot(
                        "Settling",
                        generated,
                        generated,
                        (generated > 0).then_some(1),
                    )),
                )],
                &[],
                Some(&contract),
            );
            assert!(!result.passed, "generated={generated} must fail");
            assert_eq!(result.release_contract_violation_count, 1);
        }

        let valid = evaluate_scroll_assertions(
            &[terminal_trace(
                Some(pager_snapshot("WheelGesture", 0, 0, None)),
                Some(pager_snapshot("Settling", 1, 1, Some(1))),
            )],
            &[],
            Some(&contract),
        );
        assert!(valid.passed);
    }

    #[test]
    fn release_velocity_and_target_contract_prevents_false_green() {
        let mut contract = main_scroll_contract(1, 1, 1);
        contract.expected_releases[0] = QaReleaseExpectation {
            gesture_id: 1,
            surface: QaPagerSurface::Main,
            min_filtered_velocity: -6_000.0,
            max_filtered_velocity: -4_000.0,
            target_x: -1_440.0,
            target_tolerance: 0.5,
            min_release_position_x: None,
            max_release_position_x: None,
            settled_position_x: None,
            max_settle_duration_ms: None,
        };

        let mut zero_velocity = pager_snapshot("Settling", 1, 1, Some(1));
        zero_velocity.settle_target_x = Some(-1_440.0);
        let result = evaluate_scroll_assertions(
            &[terminal_trace(
                Some(pager_snapshot("WheelGesture", 0, 0, None)),
                Some(zero_velocity),
            )],
            &[],
            Some(&contract),
        );
        assert!(!result.passed);
        assert_eq!(result.release_expectation_mismatch_count, 1);

        let mut wrong_target = pager_snapshot("Settling", 1, 1, Some(1));
        wrong_target.filtered_velocity = -5_000.0;
        wrong_target.settle_target_x = Some(0.0);
        let result = evaluate_scroll_assertions(
            &[terminal_trace(
                Some(pager_snapshot("WheelGesture", 0, 0, None)),
                Some(wrong_target),
            )],
            &[],
            Some(&contract),
        );
        assert!(!result.passed);
        assert_eq!(result.release_expectation_mismatch_count, 1);
    }

    #[test]
    fn native_sign_and_settle_deadline_prevent_false_green() {
        let mut contract = main_scroll_contract(1, 1, 1);
        contract.expected_releases[0].settled_position_x = Some(0.0);
        contract.expected_releases[0].max_settle_duration_ms = Some(500);

        let mut sign_reversed = terminal_trace(
            Some(pager_snapshot("WheelGesture", 0, 0, None)),
            Some(pager_snapshot("Settling", 1, 1, Some(1))),
        );
        sign_reversed.canonical_dx = -24.0;
        sign_reversed.expected_canonical_dx = 24.0;
        let result = evaluate_scroll_assertions(&[sign_reversed], &[], Some(&contract));
        assert!(!result.passed);
        assert_eq!(result.canonical_normalization_mismatch_count, 1);
        assert_eq!(result.settle_completion_missing_count, 1);
    }

    #[test]
    fn qa_timestamp_is_translated_to_the_app_scroll_epoch() {
        let app_origin = Instant::now();
        let qa_start = app_origin + Duration::from_millis(3_250);

        assert_eq!(
            qa_timestamp_in_app_epoch(app_origin, qa_start, 1_500_000),
            4_750_000
        );
    }

    #[test]
    fn scroll_actions_require_an_expectation_contract() {
        let mut scenario: QaScenario = serde_json::from_value(serde_json::json!({
            "name": "missing-contract",
            "duration_ms": 100,
            "output_dir": "target/qa",
            "fixture": {},
            "actions": [{
                "at_ms": 0,
                "type": "scroll_sample",
                "timestamp_us": 0,
                "canonical_dx": 0.0,
                "canonical_dy": 0.0,
                "contact_phase": "began"
            }]
        }))
        .unwrap();
        normalize_scroll_actions(&mut scenario.actions).unwrap();
        assert!(validate_scroll_expectations(&scenario)
            .unwrap_err()
            .contains("require a scroll_expectations contract"));
    }

    #[test]
    fn context_menu_contract_rejects_opening_without_open() {
        let expected = QaContextMenuExpectations {
            expected_open_count: 1,
            min_open_frames: 3,
            expected_opening_count: None,
            expected_closing_count: 0,
            expected_closed_count: 0,
            min_closing_frames: 3,
            max_closing_frames: None,
            require_final_closed: false,
        };
        let result = evaluate_context_menu_phases(
            ["Closed", "Opening", "Opening", "Closing", "Closed"],
            &expected,
        );
        assert!(!result.passed);
        assert_eq!(result.opening_count, 1);
        assert_eq!(result.open_entry_count, 0);
        assert_eq!(result.open_frame_count, 0);
    }

    #[test]
    fn context_menu_contract_requires_a_sustained_open_state() {
        let expected = QaContextMenuExpectations {
            expected_open_count: 1,
            min_open_frames: 3,
            expected_opening_count: None,
            expected_closing_count: 0,
            expected_closed_count: 0,
            min_closing_frames: 3,
            max_closing_frames: None,
            require_final_closed: false,
        };
        let result = evaluate_context_menu_phases(
            ["Closed", "Opening", "Open", "Open", "Closed"],
            &expected,
        );
        assert!(!result.passed);
        assert_eq!(result.open_entry_count, 1);
        assert_eq!(result.open_frame_count, 2);
    }

    #[test]
    fn context_menu_contract_covers_dismissal_during_and_after_opening() {
        let expected = QaContextMenuExpectations {
            expected_open_count: 2,
            min_open_frames: 3,
            expected_opening_count: Some(4),
            expected_closing_count: 4,
            expected_closed_count: 4,
            min_closing_frames: 1,
            max_closing_frames: None,
            require_final_closed: true,
        };
        let result = evaluate_context_menu_phases(
            [
                "Closed", "Opening", "Closing", "Closed", "Opening", "Open", "Open", "Open",
                "Closing", "Closed", "Opening", "Closing", "Closed", "Opening", "Open", "Open",
                "Open", "Closing", "Closed",
            ],
            &expected,
        );
        assert!(result.passed);
        assert_eq!(result.opening_count, 4);
        assert_eq!(result.open_entry_count, 2);
        assert_eq!(result.closing_count, 4);
        assert_eq!(result.closed_entry_count, 4);
        assert_eq!(result.final_phase, "Closed");
    }

    #[test]
    fn context_menu_contract_rejects_an_invisible_closing_tail() {
        let expected = QaContextMenuExpectations {
            expected_open_count: 1,
            min_open_frames: 3,
            expected_opening_count: Some(1),
            expected_closing_count: 1,
            expected_closed_count: 1,
            min_closing_frames: 1,
            max_closing_frames: Some(2),
            require_final_closed: true,
        };
        let result = evaluate_context_menu_phases(
            [
                "Closed", "Opening", "Open", "Open", "Open", "Closing", "Closing", "Closing",
                "Closed",
            ],
            &expected,
        );

        assert!(!result.passed);
        assert_eq!(result.max_closing_streak_frames, 3);
    }

    #[test]
    fn context_menu_scenario_has_an_open_contract() {
        let scenario: QaScenario =
            serde_json::from_str(include_str!("../qa/context_menu_open.json")).unwrap();
        validate_context_menu_expectations(&scenario).unwrap();
        assert_eq!(
            scenario
                .context_menu_expectations
                .unwrap()
                .expected_open_count,
            1
        );
        assert!(scenario
            .actions
            .iter()
            .any(|action| matches!(action.action, QaAction::RightClick)));
    }

    #[test]
    fn context_menu_dismiss_scenario_has_a_close_contract() {
        let scenario: QaScenario =
            serde_json::from_str(include_str!("../qa/context_menu_dismiss.json")).unwrap();
        validate_context_menu_expectations(&scenario).unwrap();
        let expected = scenario.context_menu_expectations.unwrap();
        assert_eq!(expected.expected_opening_count, Some(5));
        assert_eq!(expected.expected_closing_count, 5);
        assert_eq!(expected.expected_closed_count, 5);
        assert!(expected.require_final_closed);
        assert_eq!(
            scenario
                .actions
                .iter()
                .filter(|action| matches!(action.action, QaAction::RightClick))
                .count(),
            5
        );
    }

    #[test]
    fn trackpad_secondary_click_scenario_ignores_zero_delta_scroll_lifecycle() {
        let scenario: QaScenario = serde_json::from_str(include_str!(
            "../qa/context_menu_trackpad_secondary_click.json"
        ))
        .unwrap();
        validate_scroll_expectations(&scenario).unwrap();
        validate_context_menu_expectations(&scenario).unwrap();
        let expected = scenario.context_menu_expectations.unwrap();
        assert_eq!(expected.expected_open_count, 1);
        assert_eq!(expected.expected_closed_count, 1);
        assert_eq!(
            scenario
                .actions
                .iter()
                .filter(|action| matches!(action.action, QaAction::ScrollSample { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn bundled_trackpad_scenarios_parse_and_validate() {
        for source in [
            include_str!("../qa/trackpad_reversal_main.json"),
            include_str!("../qa/trackpad_edge_and_folders.json"),
            include_str!("../qa/trackpad_old_momentum_new_contact.json"),
            include_str!("../qa/trackpad_native_sign_and_stay.json"),
        ] {
            let mut scenario: QaScenario = serde_json::from_str(source).unwrap();
            expand_generated_apps(&mut scenario.fixture).unwrap();
            scenario.actions.sort_by_key(|action| action.at_ms);
            normalize_scroll_actions(&mut scenario.actions).unwrap();
            validate_native_scroll_actions(&scenario.actions).unwrap();
            validate_scroll_expectations(&scenario).unwrap();
            assert!(scenario.scroll_expectations.is_some());
            assert!(!scenario.fixture.apps.is_empty());
            assert!(!scenario.fixture.items.is_empty());
            assert!(scenario.actions.iter().any(|action| {
                matches!(
                    action.action,
                    QaAction::ScrollSample { .. } | QaAction::NativeScrollSample { .. }
                )
            }));
        }
    }
}
