@group(0) @binding(0) var sharp_scene: texture_2d<f32>;
@group(0) @binding(1) var blurred_scene: texture_2d<f32>;
@group(0) @binding(2) var scene_sampler: sampler;

struct FocusBlurUniforms {
    viewport_mix_radius: vec4<f32>,
    frame: vec4<f32>,
};
@group(0) @binding(3) var<uniform> uniforms: FocusBlurUniforms;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    let position = positions[vi];
    var out: VsOut;
    out.position = vec4<f32>(position, 0.0, 1.0);
    // Render targets use a top-left texture origin while clip-space Y grows
    // upward. Flip once here when the scene texture returns to the swapchain.
    out.uv = vec2<f32>(position.x * 0.5 + 0.5, 0.5 - position.y * 0.5);
    return out;
}

fn rounded_rect_distance(point: vec2<f32>, center: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let safe_radius = min(max(radius, 0.0), min(half_size.x, half_size.y));
    let q = abs(point - center) - (half_size - vec2<f32>(safe_radius));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - safe_radius;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sharp = textureSample(sharp_scene, scene_sampler, in.uv);
    let blurred = textureSample(blurred_scene, scene_sampler, in.uv);
    let viewport = uniforms.viewport_mix_radius.xy;
    let point = in.uv * viewport;
    let distance = rounded_rect_distance(
        point,
        uniforms.frame.xy,
        uniforms.frame.zw,
        uniforms.viewport_mix_radius.w,
    );

    // Preserve the page glass rim and ramp to full blur over the first 12 px.
    // This avoids smearing the transparent surround into the rounded boundary.
    let inner_mask = smoothstep(0.0, 12.0, -distance);
    let blur_mix = clamp(uniforms.viewport_mix_radius.z, 0.0, 1.0) * inner_mask;
    return mix(sharp, blurred, blur_mix);
}
