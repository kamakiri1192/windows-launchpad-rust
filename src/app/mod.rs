//! Application shell: winit lifecycle, state orchestration, action/command
//! flow, and per-frame ticking.
//!
//! This module is the Phase 5 app shell described in
//! `docs/DF_REARCHITECTURE_PLAN.md`. It consolidates the action/command shape
//! proven by Phases 1-4 (settings, bottom control + search, launcher grid,
//! edit mode) behind a small set of app-shell modules, leaving `main.rs` as
//! process startup + event-loop wiring.
//!
//! Sub-modules:
//! - [`state`]: the `App` struct, runtime value types, and pure accessors.
//! - [`event`]: `UserEvent`, `AppAction`, `AppCommand`, and input routing
//!   enums.
//! - [`input`]: pure functions that decide how a raw `WindowEvent`/`UserEvent`
//!   routes given the current shell flags (settings open, editing, control
//!   wants keyboard, …). These are the deterministic, testable surface.
//! - [`update`]: app state transitions invoked from the handler
//!   (`&mut self` methods — not a pure reducer).
//! - [`command`]: `AppCommand` execution at the app boundary (redraw, hide,
//!   summon, launch, persistence) plus the edit-mode command adapter.
//! - [`frame`]: per-frame tick and redraw orchestration.
//! - [`render`]: renderer/text/GPU-facing adapter code (relayout, control
//!   rendering, icon pipeline, springs, settings panel upload). The renderer
//!   facade split is Phase 6; this module adapts `LayoutResult` back into the
//!   existing renderer upload path.
//! - [`handler`]: the `ApplicationHandler<UserEvent>` implementation, kept as a
//!   thin dispatcher that routes through `input` → `update` → `command` →
//!   `frame`.
//!
//! The app shell is intentionally **not** a pure reducer. `update`, `command`,
//! `frame`, and `render` keep `&mut self` access to the renderer, scroller,
//! and text renderer because moving those behind a command queue would be a
//! larger behavior change than Phase 5 intends. Only `input` and `event` are
//! pure-data / pure-function surfaces.

pub mod command;
pub mod event;
pub mod frame;
pub mod handler;
pub mod input;
pub mod render;
pub mod state;
pub mod update;

// `UserEvent` is re-exported at the `app` root because `main.rs` and the
// event-loop wiring refer to it as `app::UserEvent`. The other shell types are
// reached through their sub-modules (`app::state::App`, `app::event::AppCommand`,
// …) so they do not need a root re-export.
pub use event::UserEvent;
