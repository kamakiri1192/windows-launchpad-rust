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
pub mod widgets;

// Re-export the most commonly used types (used by external crate consumers
// via the lib crate; the bin crate imports from sub-modules directly).
#[allow(unused_imports)]
pub use context::{LayoutDirection, Ui};
#[allow(unused_imports)]
pub use interaction::{ElementState, InteractionPhase};
#[allow(unused_imports)]
pub use registry::Registry;
#[allow(unused_imports)]
pub use response::Response;
#[allow(unused_imports)]
pub use theme::{ControlSize, ScrollbarStyle, Theme, ToggleMotionStyle};

// Re-export widget types.
#[allow(unused_imports)]
pub use widgets::{
    Button, ButtonStyle, Divider, Heading, IconButton, Label, Slider, SliderResponse, Toggle,
    ToggleResponse, ToggleStyle,
};
