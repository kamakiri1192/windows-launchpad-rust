// In-app targets contain linear-premultiplied RGBA when sampled. The OS
// compositor's explicit alpha modes apply to the stored, encoded SDR channels.
struct PresentationUniforms {
    alpha_mode: u32, // 0 = unspecified/opaque legacy, 1 = premultiplied, 2 = straight
    target_srgb: u32,
    _pad: vec2<u32>,
};
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: PresentationUniforms;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -3.0), vec2<f32>(-1.0, 1.0), vec2<f32>(3.0, 1.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}
fn encode_srgb(c: vec3<f32>) -> vec3<f32> {
    let v = max(c, vec3<f32>(0.0));
    return select(v * 12.92, 1.055 * pow(v, vec3<f32>(1.0 / 2.4)) - 0.055, v > vec3<f32>(0.0031308));
}
fn decode_srgb(c: vec3<f32>) -> vec3<f32> {
    let v = max(c, vec3<f32>(0.0));
    return select(v / 12.92, pow((v + 0.055) / 1.055, vec3<f32>(2.4)), v > vec3<f32>(0.04045));
}
@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let color = textureLoad(scene, vec2<i32>(position.xy), 0);
    // Auto/Inherit do not specify an alpha convention. Opaque ignores alpha.
    // Preserve their previous attachment behavior rather than guessing an OS contract.
    if u.alpha_mode == 0u { return color; }
    let alpha = clamp(color.a, 0.0, 1.0);
    if alpha <= 0.0 { return vec4<f32>(0.0); }
    var encoded = encode_srgb(color.rgb / alpha);
    if u.alpha_mode == 1u { encoded *= alpha; }
    // Cancel the attachment's hardware encoding, but only at this final,
    // unblended write. All preceding in-app source-over stays linear.
    if u.target_srgb != 0u { encoded = decode_srgb(encoded); }
    return vec4<f32>(encoded, alpha);
}
