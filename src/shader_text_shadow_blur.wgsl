// Separable 9-tap Gaussian blur for the full-resolution text shadow mask.
// The normalized kernel approximates sigma = 2 physical pixels.

@group(0) @binding(0) var shadow_mask: texture_2d<f32>;
@group(0) @binding(1) var shadow_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let position = positions[vertex_index];

    var out: VsOut;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = vec2<f32>(
        (position.x + 1.0) * 0.5,
        (1.0 - position.y) * 0.5,
    );
    return out;
}

fn gaussian_alpha(uv: vec2<f32>, direction: vec2<f32>) -> f32 {
    let dimensions = vec2<f32>(textureDimensions(shadow_mask));
    let texel = direction / max(dimensions, vec2<f32>(1.0));
    var alpha = textureSample(shadow_mask, shadow_sampler, uv).a * 0.20416369;
    alpha += textureSample(shadow_mask, shadow_sampler, uv + texel * 1.0).a * 0.18017382;
    alpha += textureSample(shadow_mask, shadow_sampler, uv - texel * 1.0).a * 0.18017382;
    alpha += textureSample(shadow_mask, shadow_sampler, uv + texel * 2.0).a * 0.12383154;
    alpha += textureSample(shadow_mask, shadow_sampler, uv - texel * 2.0).a * 0.12383154;
    alpha += textureSample(shadow_mask, shadow_sampler, uv + texel * 3.0).a * 0.06628224;
    alpha += textureSample(shadow_mask, shadow_sampler, uv - texel * 3.0).a * 0.06628224;
    alpha += textureSample(shadow_mask, shadow_sampler, uv + texel * 4.0).a * 0.02763055;
    alpha += textureSample(shadow_mask, shadow_sampler, uv - texel * 4.0).a * 0.02763055;
    return alpha;
}

@fragment
fn fs_horizontal(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, gaussian_alpha(in.uv, vec2<f32>(1.0, 0.0)));
}

@fragment
fn fs_vertical(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, gaussian_alpha(in.uv, vec2<f32>(0.0, 1.0)));
}
