// Separable two-channel Gaussian blur for the full-resolution text shadow
// mask.
//
// Both channels are blurred (the previous design left R unblurred, which let
// sharp glyph corners — the apex of "A", the tip of "フ" — lose shadow area
// because the +1,+1 offset overlapped the white body). Now R is a tight blur
// (σ ≈ 0.75 logical px, the "1px 1px 2px" main shadow) and A is a wider blur
// (σ ≈ 1.75 logical px, the "0 0 4px" halo).
//
// Concept:
//   coverage ─┬─ narrow horizontal → narrow vertical → R (main shadow)
//             └─ wide horizontal   → wide vertical   → A (halo)
//
// Each axis pass computes both channels from its input in one fragment. Bilinear
// tap pairing collapses contiguous discrete taps into one sample.

@group(0) @binding(0) var shadow_mask: texture_2d<f32>;
@group(0) @binding(1) var shadow_sampler: sampler;

struct BlurUniforms {
    // (physical px per logical px, narrow sigma logical, wide sigma logical, _)
    sample_scale: vec4<f32>,
};

@group(0) @binding(2) var<uniform> uniforms: BlurUniforms;

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

fn gaussian_weight(distance: f32, sigma: f32) -> f32 {
    return exp(-0.5 * distance * distance / (sigma * sigma));
}

// Bilinear-paired sample of two symmetric taps at integer offsets `first`
// (positive) and `second` (positive, larger), along `direction` (a unit axis).
// `texel` is the source texel size in UV. Returns weighted coverage and the
// combined weight so the caller can normalize.
fn paired_sample(uv: vec2<f32>, texel: vec2<f32>, direction: vec2<f32>, first: f32, first_weight: f32, second_weight: f32) -> vec2<f32> {
    let combined_weight = first_weight + second_weight;
    if (combined_weight <= 0.000001) {
        return vec2<f32>(0.0);
    }
    // Linear filtering at the weighted-average offset reproduces both taps.
    let offset = first + second_weight / combined_weight;
    let positive = textureSample(shadow_mask, shadow_sampler, uv + direction * texel * offset);
    let negative = textureSample(shadow_mask, shadow_sampler, uv - direction * texel * offset);
    // Both taps share the combined weight; read the alpha channel (input is
    // RGBA where .a is the coverage we blur).
    return vec2<f32>((positive.a + negative.a) * combined_weight, 2.0 * combined_weight);
}

// 1D Gaussian for a single channel at integer tap distances. `direction` is the
// axis (horizontal or vertical), `texel` its UV step. Returns normalized
// coverage from the input's alpha channel. ±8 physical taps (4 bilinear pairs)
// keep the tail under 1.5% even for the wide sigma (1.75 logical → 3.5 physical
// at 2× DPI), so no coverage leaks past the kernel edge.
fn channel_blur(uv: vec2<f32>, texel: vec2<f32>, direction: vec2<f32>, sigma: f32) -> f32 {
    let center = textureSample(shadow_mask, shadow_sampler, uv);
    let w0 = gaussian_weight(0.0, sigma);
    let w1 = gaussian_weight(1.0, sigma);
    let w2 = gaussian_weight(2.0, sigma);
    let w3 = gaussian_weight(3.0, sigma);
    let w4 = gaussian_weight(4.0, sigma);
    let w5 = gaussian_weight(5.0, sigma);
    let w6 = gaussian_weight(6.0, sigma);
    let w7 = gaussian_weight(7.0, sigma);
    let w8 = gaussian_weight(8.0, sigma);

    var acc = vec2<f32>(center.a * w0, w0);
    acc += paired_sample(uv, texel, direction, 1.0, w1, w2);
    acc += paired_sample(uv, texel, direction, 3.0, w3, w4);
    acc += paired_sample(uv, texel, direction, 5.0, w5, w6);
    acc += paired_sample(uv, texel, direction, 7.0, w7, w8);

    return acc.x / max(acc.y, 0.000001);
}

// The vertical pass samples an intermediate RGBA whose .r/.a are the
// horizontally-blurred narrow/wide channels. This mirrors channel_blur but
// reads the chosen channel instead of always alpha.
fn channel_blur_rgba(uv: vec2<f32>, texel: vec2<f32>, direction: vec2<f32>, sigma: f32, channel: f32) -> f32 {
    let center = textureSample(shadow_mask, shadow_sampler, uv);
    let center_val = select(center.r, center.a, channel >= 0.5);
    let w0 = gaussian_weight(0.0, sigma);
    let w1 = gaussian_weight(1.0, sigma);
    let w2 = gaussian_weight(2.0, sigma);
    let w3 = gaussian_weight(3.0, sigma);
    let w4 = gaussian_weight(4.0, sigma);
    let w5 = gaussian_weight(5.0, sigma);
    let w6 = gaussian_weight(6.0, sigma);
    let w7 = gaussian_weight(7.0, sigma);
    let w8 = gaussian_weight(8.0, sigma);

    var acc = vec2<f32>(center_val * w0, w0);

    let combined12 = w1 + w2;
    let combined34 = w3 + w4;
    if (combined12 > 0.000001) {
        let offset12 = 1.0 + w2 / combined12;
        let p12 = textureSample(shadow_mask, shadow_sampler, uv + direction * texel * offset12);
        let n12 = textureSample(shadow_mask, shadow_sampler, uv - direction * texel * offset12);
        let pv12 = select(p12.r, p12.a, channel >= 0.5);
        let nv12 = select(n12.r, n12.a, channel >= 0.5);
        acc += vec2<f32>((pv12 + nv12) * combined12, 2.0 * combined12);
    }
    if (combined34 > 0.000001) {
        let offset34 = 3.0 + w4 / combined34;
        let p34 = textureSample(shadow_mask, shadow_sampler, uv + direction * texel * offset34);
        let n34 = textureSample(shadow_mask, shadow_sampler, uv - direction * texel * offset34);
        let pv34 = select(p34.r, p34.a, channel >= 0.5);
        let nv34 = select(n34.r, n34.a, channel >= 0.5);
        acc += vec2<f32>((pv34 + nv34) * combined34, 2.0 * combined34);
    }
    let combined56 = w5 + w6;
    if (combined56 > 0.000001) {
        let offset56 = 5.0 + w6 / combined56;
        let p56 = textureSample(shadow_mask, shadow_sampler, uv + direction * texel * offset56);
        let n56 = textureSample(shadow_mask, shadow_sampler, uv - direction * texel * offset56);
        let pv56 = select(p56.r, p56.a, channel >= 0.5);
        let nv56 = select(n56.r, n56.a, channel >= 0.5);
        acc += vec2<f32>((pv56 + nv56) * combined56, 2.0 * combined56);
    }
    let combined78 = w7 + w8;
    if (combined78 > 0.000001) {
        let offset78 = 7.0 + w8 / combined78;
        let p78 = textureSample(shadow_mask, shadow_sampler, uv + direction * texel * offset78);
        let n78 = textureSample(shadow_mask, shadow_sampler, uv - direction * texel * offset78);
        let pv78 = select(p78.r, p78.a, channel >= 0.5);
        let nv78 = select(n78.r, n78.a, channel >= 0.5);
        acc += vec2<f32>((pv78 + nv78) * combined78, 2.0 * combined78);
    }

    return acc.x / max(acc.y, 0.000001);
}

@fragment
fn fs_horizontal(in: VsOut) -> @location(0) vec4<f32> {
    let dimensions = vec2<f32>(textureDimensions(shadow_mask));
    let texel = vec2<f32>(1.0) / max(dimensions, vec2<f32>(1.0));
    let scale = clamp(uniforms.sample_scale.x, 0.5, 4.0);
    let narrow_sigma = max(uniforms.sample_scale.y, 0.1) * scale;
    let wide_sigma = max(uniforms.sample_scale.z, 0.1) * scale;

    let narrow = channel_blur(in.uv, texel, vec2<f32>(1.0, 0.0), narrow_sigma);
    let wide = channel_blur(in.uv, texel, vec2<f32>(1.0, 0.0), wide_sigma);
    // Pack narrow→R, wide→A for the vertical pass.
    return vec4<f32>(narrow, 0.0, 0.0, wide);
}

@fragment
fn fs_vertical(in: VsOut) -> @location(0) vec4<f32> {
    let dimensions = vec2<f32>(textureDimensions(shadow_mask));
    let texel = vec2<f32>(1.0) / max(dimensions, vec2<f32>(1.0));
    let scale = clamp(uniforms.sample_scale.x, 0.5, 4.0);
    let narrow_sigma = max(uniforms.sample_scale.y, 0.1) * scale;
    let wide_sigma = max(uniforms.sample_scale.z, 0.1) * scale;

    let narrow = channel_blur_rgba(in.uv, texel, vec2<f32>(0.0, 1.0), narrow_sigma, 0.0);
    let wide = channel_blur_rgba(in.uv, texel, vec2<f32>(0.0, 1.0), wide_sigma, 1.0);
    return vec4<f32>(narrow, 0.0, 0.0, wide);
}
