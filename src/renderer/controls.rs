//! Procedural overlay instance buffers: bottom control, corner gear, and the
//! settings overlay (which reuses the control pipelines).
//!
//! Each buffer is capacity-managed ([`InstanceBuffer`]): an empty list sets
//! the logical draw count to zero (the pass skips it) but keeps the buffer
//! allocated for reuse, so a surface that disappears and reappears does not
//! churn allocations. The `ControlUniforms` struct is the small
//! viewport/scroll/frame uniform shared by the control shape and text shaders.

use crate::features::bottom_control::ControlInstance;
use crate::renderer::text_engine::GlyphQuad;

use super::counters::Category;
use super::Renderer;

/// Uniform for the bottom-control overlay + text shaders. The bottom control
/// uses only the viewport; edit badges also use scroll and the page frame clip.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ControlUniforms {
    pub(super) viewport_scroll: [f32; 4],
    pub(super) frame_center_radius: [f32; 4],
    pub(super) frame_half_size: [f32; 4],
}

impl Renderer {
    /// Replace the procedural overlay instances (magnifier, dots, caret,
    /// close ×) for the bottom control.
    pub fn set_control_instances(&mut self, instances: &[ControlInstance]) {
        let outcome = self
            .control_instance_buffer
            .set(&self.device, &self.queue, instances);
        if outcome.allocated {
            self.counters.record_creation(Category::Control);
        }
    }

    /// Replace the corner gear ink instances. Drawn in the same control
    /// overlay pass as the bottom-control ink (they share the pipeline).
    pub fn set_gear_instances(&mut self, instances: &[ControlInstance]) {
        let outcome = self
            .gear_instance_buffer
            .set(&self.device, &self.queue, instances);
        if outcome.allocated {
            self.counters.record_creation(Category::Gear);
        }
    }

    /// Replace the settings overlay ink instances (close ×). Drawn in a final
    /// overlay pass on top of the panel glass.
    pub fn set_settings_instances(&mut self, instances: &[ControlInstance]) {
        let outcome = self
            .settings_instance_buffer
            .set(&self.device, &self.queue, instances);
        if outcome.allocated {
            self.counters.record_creation(Category::Settings);
        }
    }

    /// Replace the settings overlay text quads (title).
    pub fn set_settings_text_instances(&mut self, quads: &[GlyphQuad]) {
        let outcome = self
            .settings_text_instance_buffer
            .set(&self.device, &self.queue, quads);
        if outcome.allocated {
            self.counters.record_creation(Category::SettingsText);
        }
    }

    /// Replace the text glyph quads for the bottom control (label / query /
    /// placeholder).
    pub fn set_control_text_instances(&mut self, quads: &[GlyphQuad]) {
        let outcome = self
            .control_text_instance_buffer
            .set(&self.device, &self.queue, quads);
        if outcome.allocated {
            self.counters.record_creation(Category::ControlText);
        }
    }
}
