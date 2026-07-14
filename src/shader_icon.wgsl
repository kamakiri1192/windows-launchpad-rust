// Icon pipeline shaders.
//
// One unit quad (two triangles) is drawn per icon instance. The instance
// carries the tile's geometry (so we can reuse the rounded-rect mask from the
// tile shader) and the UV rect into the shared icon atlas. The fragment
// samples the atlas, masks it to the rounded squircle, and composites with
// premultiplied alpha so icons blend correctly over the color tiles / glass.

struct Uniforms {
    // (viewport_w, viewport_h) in physical px.
    viewport: vec2<f32>,
    // Horizontal content offset (px). Negative scrolls right.
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
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

// Per-instance icon data.
struct InstanceIn {
    @location(0) origin_size_r: vec4<f32>, // (x, y, size, radius)
    @location(1) uvrect: vec4<f32>,        // (u0, v0, u1, v1)
    // Edit-mode animation: (phase, lift, scale, flags).
    @location(2) extra: vec4<f32>,
    // Optional common rigid-group pivot: (x, y, enabled, padding).
    @location(3) motion_pivot: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // Local coords in px, origin at tile center (for the SDF mask).
    @location(0) local: vec2<f32>,
    @location(1) size_r: vec2<f32>,
    @location(2) uv: vec2<f32>,
    // Copy of the instance flags so the fragment shader can bypass the frame
    // clip for a dragged (lifted) icon.
    @location(3) flags: f32,
};

// Flag bit set in `extra.w` while edit mode is active (icon should wiggle).
const FLAG_WIGGLE: f32 = 1.0;
// Flag bit set in `extra.w` while this icon is the one being dragged.
const FLAG_DRAG: f32 = 2.0;
const FLAG_FIXED: f32 = 4.0;
const FLAG_GROUP_MOTION: f32 = 32.0;

// Unit quad: two triangles covering [0,1]x[0,1].
@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) origin_size_r: vec4<f32>,
    @location(1) uvrect: vec4<f32>,
    @location(2) extra: vec4<f32>,
    @location(3) motion_pivot: vec4<f32>,
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

    let origin = origin_size_r.xy;
    let size = origin_size_r.z;
    let radius = origin_size_r.w;

    let phase = extra.x;
    let lift = extra.y;
    let scale = extra.z;
    let flags = extra.w;
    let wiggling = (u32(flags) & u32(FLAG_WIGGLE)) != 0u;
    let dragged = (u32(flags) & u32(FLAG_DRAG)) != 0u;
    let fixed = (u32(flags) & u32(FLAG_FIXED)) != 0u;
    let grouped = (u32(flags) & u32(FLAG_GROUP_MOTION)) != 0u && motion_pivot.z > 0.5;

    // Effective size after the drag scale; re-anchor about the tile center.
    let eff_size = size * scale;
    let size_delta = (eff_size - size) * 0.5;

    // World-space top-left, shifted by the scroller + scale re-anchor. A rigid
    // group transforms each miniature around one folder pivot, so the preview
    // follows and scales with its parent instead of stacking at the pointer or
    // letting every child wiggle independently.
    var tl: vec2<f32>;
    var wiggle_rot = 0.0;
    var wiggle_dy = 0.0;
    var applied_lift = lift;
    if grouped {
        let t = u.time + phase;
        if wiggling && !dragged {
            wiggle_rot = sin(t * 8.0) * 0.06;
            wiggle_dy = abs(sin(t * 8.0)) * 2.0;
        }
        let pivot = motion_pivot.xy;
        var group_center = pivot;
        var relative_center = (origin + vec2<f32>(size * 0.5)) - pivot;
        relative_center = relative_center * scale;
        if dragged && u.drag_active > 0.5 {
            group_center = u.drag_pos;
        } else if !fixed {
            group_center.x = group_center.x + u.scroll_x;
        }
        let cosr = cos(wiggle_rot);
        let sinr = sin(wiggle_rot);
        let rotated_center = vec2<f32>(
            relative_center.x * cosr - relative_center.y * sinr,
            relative_center.x * sinr + relative_center.y * cosr,
        );
        let center = group_center + rotated_center - vec2<f32>(0.0, wiggle_dy);
        tl = center - vec2<f32>(eff_size * 0.5);
        applied_lift = 0.0;
        wiggle_dy = 0.0;
    } else if dragged && u.drag_active > 0.5 {
        tl = vec2<f32>(u.drag_pos.x - eff_size * 0.5, u.drag_pos.y - eff_size * 0.5);
    } else if fixed {
        tl = vec2<f32>(origin.x - size_delta, origin.y - size_delta);
    } else {
        tl = vec2<f32>(origin.x + u.scroll_x - size_delta, origin.y - size_delta);
    }
    // Local pixel coordinates relative to the tile's center (for the SDF), in
    // the effective (scaled) size.
    let local = vec2<f32>(
        c.x * eff_size - eff_size * 0.5,
        (1.0 - c.y) * eff_size - eff_size * 0.5,
    );

    // Non-group icons rotate around their own center. Group children already
    // received the shared parent rotation above, but reuse the same angle here
    // so their artwork orientation remains rigidly attached to the folder.
    if !grouped && wiggling && !dragged {
        let t = u.time + phase;
        wiggle_rot = sin(t * 8.0) * 0.06;
        wiggle_dy = abs(sin(t * 8.0)) * 2.0;
    }

    let cx = tl.x + c.x * eff_size;
    let cy = tl.y + (1.0 - c.y) * eff_size;
    let center = vec2<f32>(tl.x + eff_size * 0.5, tl.y + eff_size * 0.5);
    let rel = vec2<f32>(cx - center.x, cy - center.y);
    let cosr = cos(wiggle_rot);
    let sinr = sin(wiggle_rot);
    let rotated = vec2<f32>(rel.x * cosr - rel.y * sinr, rel.x * sinr + rel.y * cosr);
    let world = vec2<f32>(
        center.x + rotated.x,
        center.y + rotated.y - wiggle_dy - applied_lift,
    );

    // Physical px → clip space. Y is flipped so content origin is top-left.
    let half = u.viewport * 0.5;
    let clip = vec2<f32>(
        (world.x / half.x) - 1.0,
        1.0 - (world.y / half.y),
    );

    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.local = local;
    out.size_r = vec2<f32>(eff_size, radius);
    // Map quad corner → atlas UV. c.y=1 is the quad's top edge → v0.
    out.uv = vec2<f32>(
        mix(uvrect.x, uvrect.z, c.x),
        mix(uvrect.w, uvrect.y, c.y),
    );
    out.flags = flags;
    return out;
}

// Signed distance to a rounded box centered at the origin (same as tile shader).
fn sdRoundBox(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    let outer = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
    return outer;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let half_size = in.size_r.x * 0.5;
    let r = min(in.size_r.y, half_size);
    let d = sdRoundBox(in.local, vec2<f32>(half_size, half_size), r);

    // 1px AA edge on the squircle mask.
    let aa = 1.0;
    let mask = smoothstep(aa, -aa, d);

    // Clip the squircle mask to the fixed page frame's rounded rect. A dragged
    // (lifted) icon bypasses the clip so it can rise above the panel.
    let dragged = (u32(in.flags) & u32(FLAG_DRAG)) != 0u;
    let fixed = (u32(in.flags) & u32(FLAG_FIXED)) != 0u;
    let wiggling = (u32(in.flags) & u32(FLAG_WIGGLE)) != 0u;
    var frame_alpha = 1.0;
    if !dragged && !fixed {
        let local = in.pos.xy - u.frame_center;
        let fd = sdRoundBox(local, u.frame_half_size, u.frame_radius);
        frame_alpha = smoothstep(aa, -aa, fd);
    }
    let body_a = mask * frame_alpha;

    // Sample straight-alpha from the atlas, then premultiply for correct
    // blending (the pipeline uses PREMULTIPLIED_ALPHA_BLENDING over the tiles).
    let sampled = textureSample(atlas, atlas_sampler, in.uv);
    let out_a = sampled.a * body_a;
    var col = sampled.rgb * out_a;

    // Edit-mode delete badge: mirror the tile shader so icons with atlases also
    // show the same badge. Needs to live here because the icon pass draws after
    // the tile pass and would otherwise cover the badge. It bypasses the
    // squircle mask so the icon corner cannot cut it.
    var final_a = out_a;
    if false {
        let badge_r = min(in.size_r.x * 0.13, 11.0);
        // Top-left badge; local negative Y is the screen-space top edge.
        let badge_center = vec2<f32>(-half_size + badge_r, -half_size + badge_r);
        let bp = in.local - badge_center;
        let bd = length(bp) - badge_r;
        let badge_a = smoothstep(aa, -aa, bd) * frame_alpha;
        if badge_a > 0.001 {
            let p = bp / (badge_r * 0.55);
            let seg = min(abs(p.x - p.y), abs(p.x + p.y));
            let cross_a = smoothstep(aa, -aa, seg - 1.0) * badge_a;
            col = mix(col, vec3<f32>(0.85, 0.18, 0.18) * badge_a, badge_a * 0.95);
            col = mix(col, vec3<f32>(1.0, 1.0, 1.0) * badge_a, cross_a * 0.9);
            final_a = max(final_a, badge_a);
        }
    }
    if final_a <= 0.001 {
        discard;
    }

    return vec4<f32>(col, final_a);
}
