struct UiUniforms {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: UiUniforms;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local_pos: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) radius: f32,
    @location(3) color: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) center: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) radius: f32,
    @location(3) color: vec4<f32>,
) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];
    let half_size = size * 0.5;
    let pixel_pos = center + corner * half_size;
    let clip = vec2<f32>(
        pixel_pos.x / uniforms.viewport.x * 2.0 - 1.0,
        1.0 - pixel_pos.y / uniforms.viewport.y * 2.0,
    );

    var out: VsOut;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.local_pos = corner * half_size;
    out.half_size = half_size;
    out.radius = radius;
    out.color = color;
    return out;
}

fn sd_round_box(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let radius = min(in.radius, min(in.half_size.x, in.half_size.y));
    let dist = sd_round_box(in.local_pos, in.half_size, radius);
    let coverage = clamp(0.5 - dist, 0.0, 1.0);
    let alpha = in.color.a * coverage;
    return vec4<f32>(in.color.rgb * alpha, alpha);
}
