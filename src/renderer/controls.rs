//! Procedural overlay instance buffers: bottom control, corner gear, and the
//! settings overlay (which reuses the control pipelines).
//!
//! Each buffer is optional: an empty list clears the buffer so the draw pass
//! skips it. The `ControlUniforms` struct is the small viewport/scroll/frame
//! uniform shared by the control shape and text shaders.

use wgpu::util::DeviceExt;

use crate::bottom_control::ControlInstance;
use crate::text::GlyphQuad;

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
        self.control_instance_count = instances.len() as u32;
        if instances.is_empty() {
            self.control_instance_buffer = None;
            return;
        }
        self.control_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("control instance buffer"),
                contents: bytemuck::cast_slice(instances),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Replace the corner gear ink instances. Drawn in the same control
    /// overlay pass as the bottom-control ink (they share the pipeline).
    pub fn set_gear_instances(&mut self, instances: &[ControlInstance]) {
        self.gear_instance_count = instances.len() as u32;
        if instances.is_empty() {
            self.gear_instance_buffer = None;
            return;
        }
        self.gear_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("gear instance buffer"),
                contents: bytemuck::cast_slice(instances),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Replace the settings overlay ink instances (close ×). Drawn in a final
    /// overlay pass on top of the panel glass.
    pub fn set_settings_instances(&mut self, instances: &[ControlInstance]) {
        self.settings_instance_count = instances.len() as u32;
        if instances.is_empty() {
            self.settings_instance_buffer = None;
            return;
        }
        self.settings_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("settings instance buffer"),
                contents: bytemuck::cast_slice(instances),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Replace the settings overlay text quads (title).
    pub fn set_settings_text_instances(&mut self, quads: &[GlyphQuad]) {
        self.settings_text_instance_count = quads.len() as u32;
        if quads.is_empty() {
            self.settings_text_instance_buffer = None;
            return;
        }
        self.settings_text_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("settings text instance buffer"),
                contents: bytemuck::cast_slice(quads),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }

    /// Replace the text glyph quads for the bottom control (label / query /
    /// placeholder).
    pub fn set_control_text_instances(&mut self, quads: &[GlyphQuad]) {
        self.control_text_instance_count = quads.len() as u32;
        if quads.is_empty() {
            self.control_text_instance_buffer = None;
            return;
        }
        self.control_text_instance_buffer = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("control text instance buffer"),
                contents: bytemuck::cast_slice(quads),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            },
        ));
    }
}
