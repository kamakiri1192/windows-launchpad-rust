// Flatten the transparent pre-menu swapchain over the captured desktop.
//
// `scene_texture` contains premultiplied RGBA for every launcher layer drawn
// before the context menu. The output is an opaque capture-sized image, so a
// later blur retains launcher icons/folder content as well as the desktop.

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var backdrop_texture: texture_2d<f32>;
@group(0) @binding(2) var scene_sampler: sampler;

struct SceneFlattenUniforms {
    viewport: vec2<f32>,
    backdrop_origin: vec2<f32>,
    backdrop_extent: vec2<f32>,
    _pad: vec2<f32>,
};
@group(0) @binding(3) var<uniform> u: SceneFlattenUniforms;

struct VsOut {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    var out: VsOut;
    out.position = vec4<f32>(positions[vi], 0.0, 1.0);
    return out;
}

// The scene is linear-premultiplied when sampled through its sRGB view.
// Capture/blur textures deliberately store encoded SDR RGB for material math.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(c / 12.92, pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c > vec3<f32>(0.04045));
}
fn linear_to_srgb(rgb: vec3<f32>) -> vec3<f32> {
    let c = max(rgb, vec3<f32>(0.0));
    return select(c * 12.92, 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c > vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let backdrop_size_u = textureDimensions(backdrop_texture, 0);
    let backdrop_size = vec2<f32>(
        f32(backdrop_size_u.x),
        f32(backdrop_size_u.y),
    );
    let capture_uv = clamp(
        in.position.xy / max(backdrop_size, vec2<f32>(1.0)),
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    let screen_pixel = u.backdrop_origin + capture_uv * u.backdrop_extent;
    let scene_uv = clamp(
        screen_pixel / max(u.viewport, vec2<f32>(1.0)),
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );

    let scene = textureSample(scene_texture, scene_sampler, scene_uv);
    let desktop = textureSample(backdrop_texture, scene_sampler, capture_uv);
    let scene_alpha = clamp(scene.a, 0.0, 1.0);
    let flattened_rgb = scene.rgb + srgb_to_linear(desktop.rgb) * (1.0 - scene_alpha);
    return vec4<f32>(linear_to_srgb(flattened_rgb), 1.0);
}

