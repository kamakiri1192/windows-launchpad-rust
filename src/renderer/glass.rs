//! Liquid Glass shape submission for the fixed overlay surfaces.
//!
//! These are thin delegates into [`LiquidGlassRenderer`] that push a single
//! feature-owned glass shape (bottom control, corner gear, settings panel).
//! The base glass shapes (fixed page frame + scrolling tile halos) are rebuilt
//! via [`Renderer::rebuild_instances`].

use crate::liquid_glass::geometry::GlassShape;

use super::Renderer;

impl Renderer {
    /// Push the bottom-control's glass capsule shape into the Liquid Glass
    /// geometry buffer. `None` hides the control. Called every frame from the
    /// app (the geometry is tiny and rebuilt cheaply).
    pub fn set_control_glass_shape(&mut self, shape: Option<GlassShape>) {
        self.liquid_glass.set_control_shape(&self.device, shape);
    }

    /// Push the corner gear's glass capsule shape. `None` hides it.
    pub fn set_gear_glass_shape(&mut self, shape: Option<GlassShape>) {
        self.liquid_glass.set_gear_shape(&self.device, shape);
    }

    /// Push the settings overlay panel shape. `None` hides it.
    pub fn set_settings_panel_glass_shape(&mut self, shape: Option<GlassShape>) {
        self.liquid_glass
            .set_settings_panel_shape(&self.device, shape);
    }
}
