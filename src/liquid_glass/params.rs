#[derive(Debug, Clone, Copy)]
pub struct LiquidGlassParams {
    pub enabled: bool,
    pub thickness: f32,
    pub refractive_index: f32,
    pub chromatic_aberration: f32,
    pub blur_radius: f32,
    pub saturation: f32,
    pub glass_color: [f32; 4],
    pub light_direction: [f32; 2],
    pub light_intensity: f32,
    pub ambient_strength: f32,
    pub blend: f32,
}

impl Default for LiquidGlassParams {
    fn default() -> Self {
        Self {
            enabled: true,
            thickness: 18.0,
            refractive_index: 1.32,
            chromatic_aberration: 0.045,
            blur_radius: 12.0,
            saturation: 1.22,
            glass_color: [1.0, 1.0, 1.0, 0.08],
            light_direction: normalize2([-0.5, -0.8]),
            light_intensity: 0.9,
            ambient_strength: 0.35,
            blend: 18.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DebugOptions {
    pub show_backdrop_texture: bool,
    pub show_geometry_texture: bool,
    pub show_displacement: bool,
    pub show_alpha_mask: bool,
    pub show_final_glass_only: bool,
    pub disable_chromatic_aberration: bool,
    pub disable_edge_lighting: bool,
    pub disable_blur: bool,
}

impl DebugOptions {
    pub fn flags(self) -> u32 {
        let mut flags = 0;
        flags |= self.show_backdrop_texture as u32;
        flags |= (self.show_geometry_texture as u32) << 1;
        flags |= (self.show_displacement as u32) << 2;
        flags |= (self.show_alpha_mask as u32) << 3;
        flags |= (self.show_final_glass_only as u32) << 4;
        flags |= (self.disable_chromatic_aberration as u32) << 5;
        flags |= (self.disable_edge_lighting as u32) << 6;
        flags |= (self.disable_blur as u32) << 7;
        flags
    }
}

fn normalize2(v: [f32; 2]) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len <= f32::EPSILON {
        [1.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
}
