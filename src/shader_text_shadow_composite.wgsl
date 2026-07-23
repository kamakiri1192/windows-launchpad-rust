// Composite a strong +1,+1 px drop shadow and a softer zero-offset halo.
// The render pipeline uses standard alpha-over blending, so this shader emits
// only the combined black shadow contribution.

struct CompositeUniforms {
    // (offset x px, offset y px, main alpha, halo alpha)
    offset_alpha: vec4<f32>,
};

@group(0) @binding(0) var blurred_shadow: texture_2d<f32>;
@group(0) @binding(1) var shadow_sampler: sampler;
@group(0) @binding(2) var<uniform> uniforms: CompositeUniforms;

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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let dimensions = vec2<f32>(textureDimensions(blurred_shadow));
    let texel_offset = uniforms.offset_alpha.xy
        / max(dimensions, vec2<f32>(1.0));
    let main_alpha = textureSample(
        blurred_shadow,
        shadow_sampler,
        in.uv - texel_offset,
    ).a * uniforms.offset_alpha.z;
    let halo_alpha = textureSample(
        blurred_shadow,
        shadow_sampler,
        in.uv,
    ).a * uniforms.offset_alpha.w;
    let combined_alpha = 1.0
        - (1.0 - clamp(main_alpha, 0.0, 1.0))
        * (1.0 - clamp(halo_alpha, 0.0, 1.0));
    return vec4<f32>(0.0, 0.0, 0.0, combined_alpha);
}
