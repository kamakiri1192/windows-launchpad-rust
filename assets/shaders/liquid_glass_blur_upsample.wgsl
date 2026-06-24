struct BlurUniforms {
    texel_step: vec2<f32>,
    radius: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: BlurUniforms;
@group(0) @binding(1) var source_texture: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;

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
    let radius = max(u.radius, 0.0);
    if radius < 0.5 {
        return sample_source(in.uv);
    }

    let step = u.texel_step * radius * 0.22;
    var color = sample_source(in.uv) * 0.227027;
    color += sample_source(in.uv + step * 1.384615) * 0.316216;
    color += sample_source(in.uv - step * 1.384615) * 0.316216;
    color += sample_source(in.uv + step * 3.230769) * 0.070270;
    color += sample_source(in.uv - step * 3.230769) * 0.070270;
    return color;
}
