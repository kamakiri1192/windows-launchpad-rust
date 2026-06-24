// Dual-Kawase downsample pass (Marius Bjørge 13-tap).
// Reads a source at higher resolution and writes the next pyramid level
// (half width/height) with a mild H+V blur baked into the 13 samples.

@group(0) @binding(0) var source_texture: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

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
    let p = positions[vi];

    var out: VsOut;
    out.position = vec4<f32>(p, 0.0, 1.0);
    out.uv = p * 0.5 + vec2<f32>(0.5);
    return out;
}

fn sample_source(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(source_texture, source_sampler, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // texel size of the *source* texture (higher resolution level).
    let src_size = textureDimensions(source_texture, 0);
    let texel = vec2<f32>(1.0) / vec2<f32>(f32(src_size.x), f32(src_size.y));

    // Sample at half-texel offset so the 13-tap kernel straddles 2x2 source
    // blocks symmetrically (reduces temporal shimmer on motion).
    let uv = in.uv + texel * 0.5;

    let a = sample_source(uv + vec2<f32>(-0.875, -0.875) * texel);
    let b = sample_source(uv + vec2<f32>( 0.875, -0.875) * texel);
    let c = sample_source(uv + vec2<f32>(-0.875,  0.875) * texel);
    let d = sample_source(uv + vec2<f32>( 0.875,  0.875) * texel);

    let e = sample_source(uv + vec2<f32>(-2.0,   0.0)  * texel);
    let f = sample_source(uv + vec2<f32>( 2.0,   0.0)  * texel);
    let g = sample_source(uv + vec2<f32>( 0.0,  -2.0)  * texel);
    let h = sample_source(uv + vec2<f32>( 0.0,   2.0)  * texel);

    let i = sample_source(uv + vec2<f32>(-3.0,  -3.0)  * texel);
    let j = sample_source(uv + vec2<f32>( 3.0,  -3.0)  * texel);
    let k = sample_source(uv + vec2<f32>(-3.0,   3.0)  * texel);
    let l = sample_source(uv + vec2<f32>( 3.0,   3.0)  * texel);

    // Weights tuned for a near-Gaussian result after a down+up round trip.
    let center = (a + b + c + d) * 0.5;
    let cross  = (e + f + g + h) * 0.125;
    let diag   = (i + j + k + l) * 0.0625;

    return center + cross + diag;
}
