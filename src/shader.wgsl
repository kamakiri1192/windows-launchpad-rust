// Launchpad MVP shaders.
//
// Vertex shader draws a unit quad (4 verts, 6 indices) once per tile instance.
// The scroller's horizontal offset is applied here, per-frame, via a tiny
// uniform — so the instance buffer (tile positions/colors) is written only
// once at startup and never touched while scrolling.

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

// Per-instance tile data.
struct InstanceIn {
    @location(0) origin: vec2<f32>,  // top-left in content px
    @location(1) size_r: vec2<f32>,  // (size, radius)
    @location(2) color: vec3<f32>,
    @location(3) icon_index: f32,
    // Edit-mode animation: (phase, lift, scale, flags).
    @location(4) extra: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,      // local coords in px, origin at center
    @location(1) size_r: vec2<f32>,
    @location(2) color: vec3<f32>,
    // Copy of the instance flags so the fragment shader can bypass the frame
    // clip for a dragged (lifted) icon.
    @location(3) flags: f32,
};

// Flag bit set in `extra.w` while edit mode is active (icon should wiggle).
const FLAG_WIGGLE: f32 = 1.0;
// Flag bit set in `extra.w` while this icon is the one being dragged.
const FLAG_DRAG: f32 = 2.0;
const FLAG_FIXED: f32 = 4.0;
const FLAG_NO_FILL: f32 = 16.0;

// Unit quad: two triangles covering [0,1]x[0,1].
@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) origin: vec2<f32>,
    @location(1) size_r: vec2<f32>,
    @location(2) color: vec3<f32>,
    @location(3) icon_index: f32,
    @location(4) extra: vec4<f32>,
) -> VsOut {
    // 0..1 unit quad corners.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    let c = corners[vi];

    let size = size_r.x;
    let phase = extra.x;
    let lift = extra.y;
    let scale = extra.z;
    let flags = extra.w;
    let wiggling = (u32(flags) & u32(FLAG_WIGGLE)) != 0u;
    let dragged = (u32(flags) & u32(FLAG_DRAG)) != 0u;
    let fixed = (u32(flags) & u32(FLAG_FIXED)) != 0u;

    // Effective size after the drag scale (wiggle doesn't change the size).
    let eff_size = size * scale;
    // The quad scales about the tile's center, so the world-space placement is
    // re-anchored: top-left shifts by half the size delta.
    let size_delta = (eff_size - size) * 0.5;

    // World-space top-left, shifted by the scroller + scale re-anchor. While
    // dragged, follow the pointer instead — the icon is lifted off the grid and
    // centered on `drag_pos` (so it tracks the finger rather than its home cell).
    var tl: vec2<f32>;
    if dragged && u.drag_active > 0.5 {
        tl = vec2<f32>(u.drag_pos.x - eff_size * 0.5, u.drag_pos.y - eff_size * 0.5);
    } else if fixed {
        tl = vec2<f32>(origin.x - size_delta, origin.y - size_delta);
    } else {
        tl = vec2<f32>(origin.x + u.scroll_x - size_delta, origin.y - size_delta);
    }

    // Local pixel coordinates relative to the tile's center (for SDF), in the
    // effective (scaled) size.
    let local = vec2<f32>(
        c.x * eff_size - eff_size * 0.5,
        (1.0 - c.y) * eff_size - eff_size * 0.5,
    );

    // Wiggle: a small rotation + vertical bob driven by the global clock and a
    // per-app phase offset, so icons wobble out of sync.
    var wiggle_rot = 0.0;
    var wiggle_dy = 0.0;
    // Keep the same phase after lift-off. Disabling wiggle for the dragged
    // frame would snap its current rotation/bob to zero on pointer-down.
    if wiggling {
        let t = u.time + phase;
        wiggle_rot = sin(t * 8.0) * 0.06;          // ±~3.4°
        wiggle_dy = abs(sin(t * 8.0)) * 2.0;        // gentle bob
    }

    let cx = tl.x + c.x * eff_size;
    let cy = tl.y + (1.0 - c.y) * eff_size;
    // Rotate about the tile center, then apply bob + drag lift.
    let center = vec2<f32>(tl.x + eff_size * 0.5, tl.y + eff_size * 0.5);
    let rel = vec2<f32>(cx - center.x, cy - center.y);
    let cosr = cos(wiggle_rot);
    let sinr = sin(wiggle_rot);
    let rotated = vec2<f32>(rel.x * cosr - rel.y * sinr, rel.x * sinr + rel.y * cosr);
    let world = vec2<f32>(center.x + rotated.x, center.y + rotated.y - wiggle_dy - lift);

    // Physical px → clip space. Y is flipped so that content origin is
    // top-left (matches our layout math).
    let half_vp = u.viewport * 0.5;
    let clip = vec2<f32>(
        (world.x / half_vp.x) - 1.0,
        1.0 - (world.y / half_vp.y),
    );

    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.uv = local;
    out.size_r = vec2<f32>(eff_size, size_r.y);
    out.color = color;
    out.flags = flags;
    return out;
}

// Signed distance to a rounded box centered at the origin.
// `p` is in pixels relative to the tile center; `b` is half-extent.
fn sdRoundBox(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    let outer = length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
    return outer;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if (u32(in.flags) & u32(FLAG_NO_FILL)) != 0u {
        discard;
    }
    let half_size = in.size_r.x * 0.5;
    let r = min(in.size_r.y, half_size);
    let d = sdRoundBox(in.uv, vec2<f32>(half_size, half_size), r);

    // 1px AA edge (assuming ~1px ≈ 1/dpiScale for now).
    let aa = 1.0;
    let alpha = smoothstep(aa, -aa, d);

    // Clip to the fixed page frame so tiles never spill past its rounded edge.
    // A dragged (lifted) icon bypasses the clip so it can rise above the panel.
    let dragged = (u32(in.flags) & u32(FLAG_DRAG)) != 0u;
    let fixed = (u32(in.flags) & u32(FLAG_FIXED)) != 0u;

    var frame_a = 1.0;
    if !dragged && !fixed {
        frame_a = frame_alpha(in.pos.xy);
    }
    let body_a = alpha * frame_a;
    // Subtle top→bottom sheen for a touch of depth.
    let sheen = mix(1.08, 0.86, clamp(in.uv.y / in.size_r.x + 0.5, 0.0, 1.0));
    var col = in.color * sheen;
    var a = body_a;

    // Edit-mode delete badge: a red circle with a white ✕ in the top-left
    // corner. Shown on every wiggling (editing) tile that isn't being dragged.
    // The badge bypasses the squircle mask so the icon corner cannot cut it.
    if false {
        // Badge center sits at the tile's top-left corner. In local coords,
        // negative Y is the top edge because screen-space Y grows downward.
        let badge_r = min(in.size_r.x * 0.13, 11.0);
        let badge_center = vec2<f32>(-half_size + badge_r, -half_size + badge_r);
        let bp = in.uv - badge_center;
        let bd = length(bp) - badge_r;
        let badge_a = smoothstep(aa, -aa, bd) * frame_a;
        if badge_a > 0.001 {
            // Two short crossing segments for the ✕, centered on the badge.
            let p = bp / (badge_r * 0.55);
            let seg = min(
                abs(p.x - p.y),
                abs(p.x + p.y),
            );
            let cross_a = smoothstep(aa, -aa, seg - 1.0) * badge_a;
            // Red badge fill.
            col = mix(col, vec3<f32>(0.85, 0.18, 0.18), badge_a * 0.95);
            // White ✕ on top.
            col = mix(col, vec3<f32>(1.0, 1.0, 1.0), cross_a * 0.9);
            a = max(a, badge_a);
        }
    }

    if a <= 0.001 {
        discard;
    }

    return vec4<f32>(col, a);
}

// Alpha against the fixed page frame's rounded rect. `frag` is the fragment's
// physical-pixel position.
fn frame_alpha(frag: vec2<f32>) -> f32 {
    let local = frag - u.frame_center;
    let fd = sdRoundBox(local, u.frame_half_size, u.frame_radius);
    // 1px AA edge around the frame border.
    let faa = 1.0;
    return smoothstep(faa, -faa, fd);
}
