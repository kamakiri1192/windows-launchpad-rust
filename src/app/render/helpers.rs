//! Shared helper utilities for the render adapter: color blending, the
//! SpringPos trait, and the linear animation advance helper.

use crate::grid;
use crate::renderer::icon_pipeline;

pub(crate) fn mul_alpha(mut c: [f32; 4], a: f32) -> [f32; 4] {
    c[3] *= a.clamp(0.0, 1.0);
    c
}

pub(crate) trait SpringPos {
    fn set_pos(&mut self, x: f32, y: f32);
}

impl SpringPos for grid::TileInstance {
    fn set_pos(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
    }
}

impl SpringPos for icon_pipeline::IconInstance {
    fn set_pos(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
    }
}

pub(crate) fn advance_unit_toward(v: f32, target: f32, dt: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return target;
    }
    let dir = if target >= v { 1.0 } else { -1.0 };
    let next = v + dir * dt.max(0.0) / duration;
    if dir > 0.0 {
        next.min(target)
    } else {
        next.max(target)
    }
}
