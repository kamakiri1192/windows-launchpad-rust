//! Renderer/text/GPU-facing adapter code, split by feature.
//!
//! This module adapts the layout-layer `LayoutResult` back into the existing
//! renderer upload path. Each sub-module owns one feature's adapter logic:
//!
//! - [`controls`]: bottom control, edit gear, IME, caret.
//! - [`settings`]: settings panel ink, text, and animation.
//! - [`grid`]: grid relayout, tile springs, edit-mode animation.
//! - [`icons`]: icon cache integration, worker results, app-list diff.
//! - [`helpers`]: shared utilities (color blend, SpringPos trait, animation).

mod controls;
mod folders;
mod grid;
mod helpers;
mod icons;
mod settings;

pub(crate) use settings::{settings_category_id, settings_press_target_from_layout_hit};
