// Dual-Kawase downsample pass (Marius Bjørge 13-tap).
// Reads a source at higher resolution and writes the next pyramid level
// (half width/height) with a mild H+V blur baked into the 13 samples.
//
// The raw 0.5 / 1.0 / 0.25 ring weights plus the center weight total 8.0.
// Normalize by that total so a constant-color scene keeps the same energy at
// every pyramid level.

@group(0) @binding(0) var source_texture: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

const KERNEL_NORMALIZATION: f32 = 1.0 / 8.0;

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

    // Sample at half-texel offset so the kernel straddles 2x2 source blocks
    // symmetrically (reduces temporal shimmer on motion).
    let uv = in.uv + texel * 0.5;

    // 4 corners of the inner ring (weight 0.5 / 8 each).
    let a = sample_source(uv + vec2<f32>(-1.0, -1.0) * texel);
    let b = sample_source(uv + vec2<f32>( 1.0, -1.0) * texel);
    let c = sample_source(uv + vec2<f32>(-1.0,  1.0) * texel);
    let d = sample_source(uv + vec2<f32>( 1.0,  1.0) * texel);

    // 4 edge midpoints of the inner ring (weight 1.0 / 8 each).
    let e = sample_source(uv + vec2<f32>(-2.0,  0.0) * texel);
    let f = sample_source(uv + vec2<f32>( 2.0,  0.0) * texel);
    let g = sample_source(uv + vec2<f32>( 0.0, -2.0) * texel);
    let h = sample_source(uv + vec2<f32>( 0.0,  2.0) * texel);

    // 4 outer corners (weight 0.25 / 8 each).
    let i = sample_source(uv + vec2<f32>(-3.0, -3.0) * texel);
    let j = sample_source(uv + vec2<f32>( 3.0, -3.0) * texel);
    let k = sample_source(uv + vec2<f32>(-3.0,  3.0) * texel);
    let l = sample_source(uv + vec2<f32>( 3.0,  3.0) * texel);

    // Marius Bjørge dual-Kawase 13-tap. Ring weights total 7.0 and the center
    // contributes 1.0, so applying 1 / 8 preserves a normalized total of 1.0.
    let ring = (a + b + c + d) * (0.5 * KERNEL_NORMALIZATION)
             + (e + f + g + h) * KERNEL_NORMALIZATION
             + (i + j + k + l) * (0.25 * KERNEL_NORMALIZATION);

    let center = sample_source(uv) * KERNEL_NORMALIZATION;

    return ring + center;
}
