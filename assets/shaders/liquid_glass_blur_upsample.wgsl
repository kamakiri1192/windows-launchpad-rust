// Dual-Kawase upsample pass (Marius Bjørge style).
// Reads the lower-resolution source level and writes the next level up
// (double width/height), again with a mild H+V blur folded into the taps.
//
// 9-tap bilinear 3x3 kernel sampled on the low-res source. Weights are the
// classic 1-2-1 box-binomial (sum = 1) so the result stays energy-preserving
// across many pyramid levels.

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
    // texel size of the *source* texture (the lower-resolution level).
    let src_size = textureDimensions(source_texture, 0);
    let texel = vec2<f32>(1.0) / vec2<f32>(f32(src_size.x), f32(src_size.y));

    // Align the 3x3 phase with the downsample pass for a symmetric round trip.
    let uv = in.uv - texel * 0.5;

    let tl = sample_source(uv + vec2<f32>(-1.0, -1.0) * texel);
    let tm = sample_source(uv + vec2<f32>( 0.0, -1.0) * texel);
    let tr = sample_source(uv + vec2<f32>( 1.0, -1.0) * texel);
    let ml = sample_source(uv + vec2<f32>(-1.0,  0.0) * texel);
    let mm = sample_source(uv);
    let mr = sample_source(uv + vec2<f32>( 1.0,  0.0) * texel);
    let bl = sample_source(uv + vec2<f32>(-1.0,  1.0) * texel);
    let bm = sample_source(uv + vec2<f32>( 0.0,  1.0) * texel);
    let br = sample_source(uv + vec2<f32>( 1.0,  1.0) * texel);

    // 1-2-1 binomial per axis => 3x3 separable weights summing to 1.
    let w_c = vec4<f32>(1.0, 2.0, 1.0, 2.0);
    let w_r = vec4<f32>(4.0, 2.0, 1.0, 2.0);

    let top = tl * w_c.x + tm * w_c.y + tr * w_c.z;
    let mid = ml * w_c.w + mm * w_r.x + mr * w_r.y;
    let bot = bl * w_r.z + bm * w_r.w + br * w_c.z;

    return (top + mid + bot) * (1.0 / 16.0);
}
