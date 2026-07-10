//! Liquid Glass shape submission, grouped by render lane.
//!
//! The renderer does not know whether a glass surface came from the bottom
//! control, the edit-mode gear, the settings panel, or a future folder. It
//! only knows the *rendering characteristics* a surface needs:
//!
//! - [`overlay_glass`][Renderer::set_overlay_glass]: fixed glass surfaces that
//!   share one Liquid Glass SDF field so they merge and separate smoothly
//!   (today: the bottom-control capsule + the edit-mode settings gear). Both
//!   shapes are submitted in one call so the field is computed once.
//! - the modal lane (settings panel) is submitted through
//!   [`prepare`][Renderer::prepare], which routes a renderer-neutral
//!   `GlassSurface` list.
//!
//! The base glass shapes (fixed page frame + scrolling tile halos) are still
//! rebuilt via [`Renderer::rebuild_instances`], which is the base/scrolling
//! lane.

use crate::liquid_glass::geometry::GlassShape;

use super::Renderer;

impl Renderer {
    /// Submit the fixed overlay glass surfaces for this frame. The bottom
    /// control and the edit-mode gear are submitted together because the
    /// Liquid Glass shader composites them in one SDF field (this is what
    /// makes the gear merge into / separate from the capsule smoothly).
    ///
    /// Pass `None` for either slot to hide that surface. Replaces the former
    /// `set_control_glass_shape` / `set_gear_glass_shape` pair so that adding
    /// a future overlay surface does not grow a new feature-named method — it
    /// just contributes another shape to this lane.
    ///
    /// Behavior preservation: this is exactly the pair of
    /// `LiquidGlassRenderer::set_control_shape` + `set_gear_shape` calls the
    /// old setters made, issued together.
    pub fn set_overlay_glass(&mut self, control: Option<GlassShape>, gear: Option<GlassShape>) {
        self.liquid_glass.set_control_shape(&self.device, control);
        self.liquid_glass.set_gear_shape(&self.device, gear);
    }
}
