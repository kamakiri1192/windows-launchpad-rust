// Text label shaders.
//
// One quad (two triangles) per glyph instance. The quad's screen position is
// offset by the same per-frame `scroll_x` as the tiles, so labels scroll in
// lockstep with icons. The fragment samples the glyph atlas and uses its
// alpha as coverage for each instance tint.

struct Uniforms {
    viewport: vec2<f32>,
    scroll_x: f32,
    // Global animation clock (seconds). Drives the edit-mode wiggle.
    time: f32,
    // Fixed page-frame center (physical px).
    frame_center: vec2<f32>,
    // Fixed page-frame half-size (physical px).
    frame_half_size: vec2<f32>,
    // Fixed page-frame corner radius (physical px).
    frame_radius: f32,
    // 1.0 while an edit-mode drag is in flight, else 0.0.
    drag_active: f32,
    // Pointer position (screen px) the dragged icon follows while dragging.
    drag_pos: vec2<f32>,
    // Open-transition values: (progress, opacity, start_scale, max_stagger).
    presentation: vec4<f32>,
    // Open-transition motion: (start_offset_fraction, _, _, _).
    presentation_motion: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) presentation_alpha: f32,
    @interpolate(flat) @location(3) motion_flags: u32,
};


@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) xywh: vec4<f32>,  // (x, y, w, h) top-left + size, content px
    @location(1) uvrect: vec4<f32>, // (u0, v0, u1, v1)
    @location(2) color: vec4<f32>,  // non-premultiplied RGBA tint
    @location(4) radius_pivot: vec4<f32>, // (clip radius, owner x/y, has owner)
    @location(5) motion: vec4<f32>, // owner (phase, lift, scale, flags)
    // Grid text shares the 96-byte GlyphQuad layout. Location 3 is unused;
    // location 4 carries the explicit owner pivot in the radius padding.
) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    let c = corners[vi];

    let origin = vec2<f32>(xywh.x + u.scroll_x, xywh.y);
    let glyph_center = select(
        vec2<f32>(origin.x + xywh.z * 0.5, origin.y + xywh.w * 0.5),
        radius_pivot.yz + vec2<f32>(u.scroll_x, 0.0),
        radius_pivot.w > 0.5,
    );
    let flags = u32(motion.w);
    let wiggling = (flags & 1u) != 0u;
    let dragged = (flags & 2u) != 0u && u.drag_active > 0.5;
    let owner_center = select(glyph_center, u.drag_pos, dragged);
    var local = vec2<f32>(origin.x + c.x * xywh.z, origin.y + (1.0 - c.y) * xywh.w) - glyph_center;
    if wiggling {
        // Identical phase, angular speed and vertical motion to shader_icon.
        // Rotate every glyph AND its shadows around the shared icon pivot.
        let wave = sin((u.time + motion.x) * 8.0);
        let angle = wave * 0.06;
        local *= motion.z;
        local = vec2<f32>(local.x * cos(angle) - local.y * sin(angle), local.x * sin(angle) + local.y * cos(angle));
        // Closed-folder miniatures use rigid group motion without extra lift.
        let lift = select(motion.y, 0.0, (flags & 8u) != 0u);
        local.y -= abs(wave) * 2.0 + lift;
    }
    let item_progress = select(launch_item_progress(glyph_center, vec4<f32>(u.frame_center, u.frame_half_size), u.presentation), 1.0, dragged);
    let reveal_scale = mix(u.presentation.z, 1.0, item_progress);
    let gather_offset = launch_gather_offset(owner_center, vec4<f32>(u.frame_center, u.frame_half_size), item_progress, u.presentation_motion.x);
    let world = owner_center + gather_offset + local * reveal_scale;

    let half_vp = u.viewport * 0.5;
    let clip = vec2<f32>(
        (world.x / half_vp.x) - 1.0,
        1.0 - (world.y / half_vp.y),
    );

    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.uv = vec2<f32>(
        mix(uvrect.x, uvrect.z, c.x),
        // c.y = 1 is the quad's top edge (glyph top) → map to atlas row v0
        // (the glyph's top row), which the CPU placed at atlas y = entry.y.
        mix(uvrect.w, uvrect.y, c.y),
    );
    out.color = color;
    out.presentation_alpha = u.presentation.y * item_progress;
    out.motion_flags = flags;
    return out;
}

// Signed distance to a rounded box centered at the origin (shared with the
// tile/icon shaders).
fn sdRoundBox(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(atlas, atlas_sampler, in.uv);
    // Clip labels to the fixed page frame so they never spill past its edge.
    let local = in.pos.xy - u.frame_center;
    let fd = sdRoundBox(local, u.frame_half_size, u.frame_radius);
    let frame_alpha = select(smoothstep(1.0, -1.0, fd), 1.0, (in.motion_flags & 2u) != 0u && u.drag_active > 0.5);
    // Atlas stores RGBA; alpha is coverage. Color stays non-premultiplied for
    // the pipeline's standard alpha blending.
    return vec4<f32>(in.color.rgb, sampled.a * in.color.a * frame_alpha * in.presentation_alpha);
}
