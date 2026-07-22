@group(0) @binding(0) var composition_texture: texture_2d<f32>;

struct VsOut {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    var out: VsOut;
    out.position = vec4<f32>(positions[vi], 0.0, 1.0);
    return out;
}

fn load_composition(position: vec4<f32>) -> vec4<f32> {
    return textureLoad(composition_texture, vec2<i32>(position.xy), 0);
}

@fragment
fn fs_premultiplied(in: VsOut) -> @location(0) vec4<f32> {
    return load_composition(in.position);
}

@fragment
fn fs_straight(in: VsOut) -> @location(0) vec4<f32> {
    let color = load_composition(in.position);
    if color.a <= 1.0 / 255.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(clamp(color.rgb / color.a, vec3<f32>(0.0), vec3<f32>(1.0)), color.a);
}
