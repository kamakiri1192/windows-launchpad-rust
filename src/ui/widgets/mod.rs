//! Liquid Glass UI Widgets — Phase 2: functional Ink-based widgets.
//!
//! These widgets are built on the Phase 0/1 foundation (Ui, Response,
//! ElementState, RenderModel, HitMap) and match the visual appearance of the
//! existing settings panel render code (`app/render/settings.rs`,
//! `layout/settings_panel.rs`).

pub mod button;
pub mod divider;
pub mod label;
pub mod scroll_view;
pub mod slider;
pub mod toggle;

pub use button::{Button, ButtonStyle, IconButton};
pub use divider::Divider;
pub use label::{Heading, Label};
pub use slider::{Slider, SliderResponse};
pub use toggle::{Toggle, ToggleResponse, ToggleStyle};

use crate::ui::theme::ControlSize;
use crate::ui_model::render_model::Color;

// ---------------------------------------------------------------------------
// Common constants
// ---------------------------------------------------------------------------

/// Default z-order for interactive controls (matches `Z_CONTROL` in
/// `settings_panel.rs`).
pub const Z_CONTROL: i16 = 100;

// ---------------------------------------------------------------------------
// Common helpers
// ---------------------------------------------------------------------------

/// Returns a scale multiplier for the given control size.
///
/// Regular = 1.0, Small = 0.9, Mini = 0.8.
pub fn control_size_multiplier(size: ControlSize) -> f32 {
    match size {
        ControlSize::Regular => 1.0,
        ControlSize::Small => 0.9,
        ControlSize::Mini => 0.8,
    }
}

/// Convert a `[f32; 4]` RGBA array to a `Color`.
pub fn color_from_array(c: [f32; 4]) -> Color {
    Color::rgba(c[0], c[1], c[2], c[3])
}

/// Scale the alpha of a `[f32; 4]` colour by `factor`.
pub fn scale_color_alpha(c: [f32; 4], factor: f32) -> [f32; 4] {
    [c[0], c[1], c[2], c[3] * factor]
}
