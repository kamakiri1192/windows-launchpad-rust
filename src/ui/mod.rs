//! Liquid Glass UI — immediate-mode component foundation.
//!
//! This module provides the `Ui` context, layout containers (`column`,
//! `row`, `spacer`, `padding`, `with_clip`), theme values, and supporting
//! types. Widgets (buttons, toggles, sliders, scroll views) are built in
//! later phases on top of this foundation.

pub mod context;
pub mod interaction;
pub mod layout;
pub mod registry;
pub mod response;
pub mod theme;

// Re-export the most commonly used types.
pub use context::{LayoutDirection, Ui};
pub use interaction::{ElementState, InteractionPhase};
pub use registry::Registry;
pub use response::Response;
pub use theme::{ControlSize, ScrollbarStyle, Theme, ToggleMotionStyle};
