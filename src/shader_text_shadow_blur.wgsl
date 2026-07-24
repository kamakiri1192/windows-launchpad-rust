// Separable Gaussian blur for the full-resolution text shadow mask.
// R carries the unblurred main shadow. A carries the wider halo (sigma ~= 2
// logical px). On Retina, the 17 contiguous physical-pixel weights are paired
// into 9 bilinear samples; no diagonal/corner coverage is skipped.

@group(0) @binding(0) var shadow_mask: texture_2d<f32>;
@group(0) @binding(1) var shadow_sampler: sampler;

struct BlurUniforms {
    // Physical pixels per logical CSS pixel.
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

fn gaussian_weight(distance: f32, sigma: f32, radius: f32) -> f32 {
    if distance > radius {
        return 0.0;
    }
    return exp(-0.5 * distance * distance / (sigma * sigma));
}

fn paired_halo_sample(
    uv: vec2<f32>,
    texel: vec2<f32>,
    first_distance: f32,
    first_weight: f32,
    second_weight: f32,
) -> f32 {
    let combined_weight = first_weight + second_weight;
    if combined_weight <= 0.000001 {
        return 0.0;
    }
    // Linear filtering at this weighted fractional offset reproduces both
    // adjacent discrete taps with one sample on each side.
    let offset = first_distance + second_weight / combined_weight;
    let positive = textureSample(shadow_mask, shadow_sampler, uv + texel * offset).a;
    let negative = textureSample(shadow_mask, shadow_sampler, uv - texel * offset).a;
    return (positive + negative) * combined_weight;
}

fn gaussian_channels(uv: vec2<f32>, direction: vec2<f32>) -> vec2<f32> {
    let dimensions = vec2<f32>(textureDimensions(shadow_mask));
    let texel = direction / max(dimensions, vec2<f32>(1.0));
    let scale = clamp(uniforms.sample_scale.x, 0.5, 2.0);
    let sigma = 2.0 * scale;
    let radius = ceil(4.0 * scale);
    let center = textureSample(shadow_mask, shadow_sampler, uv);
    let w0 = gaussian_weight(0.0, sigma, radius);
    let w1 = gaussian_weight(1.0, sigma, radius);
    let w2 = gaussian_weight(2.0, sigma, radius);
    let w3 = gaussian_weight(3.0, sigma, radius);
    let w4 = gaussian_weight(4.0, sigma, radius);
    let w5 = gaussian_weight(5.0, sigma, radius);
    let w6 = gaussian_weight(6.0, sigma, radius);
    let w7 = gaussian_weight(7.0, sigma, radius);
    let w8 = gaussian_weight(8.0, sigma, radius);
    let normalization = w0 + 2.0 * (w1 + w2 + w3 + w4 + w5 + w6 + w7 + w8);

    var halo = center.a * w0;
    halo += paired_halo_sample(uv, texel, 1.0, w1, w2);
    halo += paired_halo_sample(uv, texel, 3.0, w3, w4);
    halo += paired_halo_sample(uv, texel, 5.0, w5, w6);
    halo += paired_halo_sample(uv, texel, 7.0, w7, w8);

    return vec2<f32>(center.r, halo / max(normalization, 0.000001));
}

@fragment
fn fs_horizontal(in: VsOut) -> @location(0) vec4<f32> {
    let channels = gaussian_channels(in.uv, vec2<f32>(1.0, 0.0));
    return vec4<f32>(channels.x, 0.0, 0.0, channels.y);
}

@fragment
fn fs_vertical(in: VsOut) -> @location(0) vec4<f32> {
    let channels = gaussian_channels(in.uv, vec2<f32>(0.0, 1.0));
    return vec4<f32>(channels.x, 0.0, 0.0, channels.y);
}
