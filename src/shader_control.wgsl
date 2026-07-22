// Bottom-control overlay shader.
//
// Draws the procedural content layers of the morphing bottom-center control on
// top of its Liquid Glass capsule:
//   - the magnifier glass + handle (search pill / field),
//   - the page-indicator dots (transient),
//   - the text caret (search field),
//   - the close (×) button (search field).
//
// Everything is drawn in **physical pixels** centered on the capsule. Each
// instance is one element; the fragment shader interprets it by `kind`. The
// capsule glass itself comes from the Liquid Glass pass — this shader only
// paints the foreground ink.

struct Uniforms {
    viewport_scroll: vec4<f32>,
    frame_center_radius: vec4<f32>,
    frame_half_size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// kind values:
//   0 = magnifier (ring + handle)
//   1 = indicator dot
//   2 = caret (vertical bar)
//   3 = close button (×)
//   4 = edit badge close glyph (scroll-coupled, frame-masked)
//   9 = modal edit badge close glyph (fixed, not frame-masked)
//   5 = settings gear (ring + radial teeth)
//   6 = rounded rectangle
//   7 = check mark
//   8 = chevron
struct InstanceIn {
    @location(0) center: vec2<f32>,  // physical px center of the element
    @location(1) params: vec4<f32>,  // (size_or_radius, alpha, active/extra, _pad)
    @location(2) color: vec4<f32>,   // rgba tint (non-premultiplied)
    @location(3) kind: vec4<f32>,    // (kind, a, b, c) element-specific
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,   // px relative to element center
    @location(1) params: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) kind: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) center: vec2<f32>,
    @location(1) params: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) kind: vec4<f32>,
) -> VsOut {
    // Local extent for the unit quad. We size the quad generously per element
    // so the SDF (ring/dot/X) fits; `size` is the element's radius.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, -1.0),
    );
    let c = corners[vi];
    // Half-extent of the bounding box for this element (px). For the
    // magnifier we add room for the handle; for dots/caret/close it is the
    // radius.
    let extent = element_extent(kind.x, params);

    var element_center = center;
    // Both badge kinds share the tile's GPU wiggle. Only the top-level badge
    // (kind 4) receives main-page scroll; modal folder badges (kind 9) already
    // carry their folder-page position in screen coordinates.
    if (kind.x > 3.5 && kind.x < 4.5) || kind.x > 8.5 {
        let t = u.viewport_scroll.w + kind.w;
        let rot = sin(t * 8.0) * 0.06;
        let dy = abs(sin(t * 8.0)) * 2.0;
        let pivot = kind.yz;
        let rel = element_center - pivot;
        let cosr = cos(rot);
        let sinr = sin(rot);
        element_center = pivot + vec2<f32>(
            rel.x * cosr - rel.y * sinr,
            rel.x * sinr + rel.y * cosr - dy,
        );
        if kind.x < 4.5 {
            element_center.x = element_center.x + u.viewport_scroll.z;
        }
    }
    let world = vec2<f32>(element_center.x + c.x * extent, element_center.y - c.y * extent);
    let local = vec2<f32>(c.x * extent, -c.y * extent);

    let half_vp = u.viewport_scroll.xy * 0.5;
    let clip = vec2<f32>(
        (world.x / half_vp.x) - 1.0,
        1.0 - (world.y / half_vp.y),
    );

    var out: VsOut;
    out.pos = vec4<f32>(clip, 0.0, 1.0);
    out.local = local;
    out.params = params;
    out.color = color;
    out.kind = kind;
    return out;
}

// Bounding-box half-extent (px) for each element kind, given its base size.
fn element_extent(kind: f32, params: vec4<f32>) -> f32 {
    let size = params.x;
    if kind < 0.5 {
        // magnifier: ring radius + handle length.
        return size * 2.4;
    }
    if kind > 4.5 && kind < 5.5 {
        // gear: teeth extend just past the ring radius.
        return size * 1.4;
    }
    if kind > 5.5 && kind < 6.5 {
        // rounded rectangle: params.z carries half-width.
        return max(params.z, size) * 1.05;
    }
    if kind > 6.5 {
        return size * 1.8;
    }
    // dot / caret / close: a square of side ~2*size fits the shape.
    return size * 1.6;
}

// Signed distance to a circle of radius `r` centered at origin.
fn sd_circle(p: vec2<f32>, r: f32) -> f32 {
    return length(p) - r;
}

// Signed distance to a rounded line segment from (0,0) to `b` with radius `r`.
fn sd_segment(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let pa = p;
    let ba = b;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h) - r;
}

fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let rr = min(r, min(b.x, b.y));
    let q = abs(p) - b + vec2<f32>(rr);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - rr;
}

fn frame_alpha(pixel: vec2<f32>) -> f32 {
    let local = pixel - u.frame_center_radius.xy;
    let d = sd_round_box(local, u.frame_half_size.xy, u.frame_center_radius.z);
    return smoothstep(1.0, -1.0, d);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.local;
    let alpha = in.params.y;
    let kind = in.kind.x;

    var coverage: f32 = 0.0;

    if kind < 0.5 {
        // Magnifier: ring (annulus) + handle.
        let size = in.params.x;
        let ring_r = size * 0.5;
        let ring_w = max(size * 0.13, 1.2);
        let ring_in = ring_r - ring_w;
        let d_outer = sd_circle(p, ring_r);
        let d_inner = sd_circle(p, ring_in);
        // Annulus coverage: inside outer, outside inner.
        let ring = (1.0 - smoothstep(-1.0, 1.0, d_outer)) * smoothstep(-1.0, 1.0, d_inner);
        // Handle: a short thick segment down-right from the ring edge.
        // Local space is Y-down (matches screen coords), so (1, 1) points to
        // the lower-right — the classic 🔍 handle direction.
        let h_len = size * 0.62;
        let dir = normalize(vec2<f32>(1.0, 1.0));
        let b: vec2<f32> = dir * h_len;
        // Shift the handle start to the ring's lower-right edge.
        let hp = p - dir * (ring_r * 0.7);
        let d_h = sd_segment(hp, b, ring_w * 0.85);
        let handle = 1.0 - smoothstep(-1.0, 1.0, d_h);
        coverage = max(ring, handle);
    } else if kind < 1.5 {
        // Indicator dot.
        let r = in.params.x;
        let d = sd_circle(p, r);
        coverage = 1.0 - smoothstep(-1.0, 1.0, d);
    } else if kind < 2.5 {
        // Caret: a thin vertical rounded bar.
        let h = in.params.x; // half-height
        let w = max(in.params.z, 1.0); // half-width
        let q = abs(p) - vec2<f32>(w, h);
        let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
        coverage = 1.0 - smoothstep(-1.0, 1.0, d);
    } else if kind < 3.5 {
        // Close button: an × made of two crossed segments, each centered at
        // the origin. sd_segment measures distance to [0, b], so we shift p by
        // +b/2 to center the segment on the origin.
        let r = in.params.x;
        let w = max(in.params.z, 1.0);
        let len = r * 0.62;
        let b1 = vec2<f32>(len, len);    // diagonal: top-left → bottom-right
        let b2 = vec2<f32>(-len, len);   // diagonal: top-right → bottom-left
        let d1 = sd_segment(p + b1, 2.0 * b1, w);
        let d2 = sd_segment(p + b2, 2.0 * b2, w);
        coverage = 1.0 - smoothstep(-1.0, 1.0, min(d1, d2));
    } else if kind < 4.5 || kind > 8.5 {
        // Edit badge: the glass disk is rendered by Liquid Glass; this pass
        // only paints the iOS-style close glyph.
        let r = in.params.x;
        let w = max(in.params.z, 1.0);
        let len = r * 0.50;
        let b1 = vec2<f32>(len, len);
        let b2 = vec2<f32>(-len, len);
        let d1 = sd_segment(p + b1, 2.0 * b1, w);
        let d2 = sd_segment(p + b2, 2.0 * b2, w);
        let close = 1.0 - smoothstep(-1.0, 1.0, min(d1, d2));
        let disk = (1.0 - smoothstep(-1.0, 1.0, sd_circle(p, r * 1.02))) * 0.34;
        let ring_d = abs(sd_circle(p, r * 0.82)) - max(w * 0.45, 0.7);
        let ring = (1.0 - smoothstep(-1.0, 1.0, ring_d)) * 0.38;
        coverage = max(close, max(ring, disk));
    } else if kind < 5.5 {
        // Settings gear: an annulus (ring) plus 8 short radial teeth. `size`
        // is the outer tooth-tip radius; the ring sits at 0.62*size.
        let size = in.params.x;
        let tooth_r = size;
        let ring_r = size * 0.62;
        let ring_w = max(size * 0.16, 1.2);
        let d_outer = sd_circle(p, ring_r + ring_w * 0.5);
        let d_inner = sd_circle(p, ring_r - ring_w * 0.5);
        let ring = (1.0 - smoothstep(-1.0, 1.0, d_outer)) * smoothstep(-1.0, 1.0, d_inner);
        // Teeth: 8 rounded boxes radiating from the origin. Each tooth is a
        // thin radial bar centered just outside the ring. Accumulate the
        // union of all tooth coverages, then union with the ring.
        let tooth_len = (tooth_r - ring_r) * 0.95;
        let tooth_w = max(size * 0.09, 0.9);
        var tooth_union: f32 = 0.0;
        for (var i = 0; i < 8; i = i + 1) {
            let ang = f32(i) * (6.2831853 / 8.0);
            let ca = cos(ang);
            let sa = sin(ang);
            // Rotate p into the tooth's local frame (long axis = x), then
            // translate to the tooth center and test a rounded box.
            let rx = p.x * ca + p.y * sa;
            let ry = -p.x * sa + p.y * ca;
            let q = vec2<f32>(rx, ry) - vec2<f32>(ring_r + tooth_len * 0.5, 0.0);
            let d = sd_round_box(q, vec2<f32>(tooth_len * 0.5, tooth_w), tooth_w * 0.4);
            let t = 1.0 - smoothstep(-1.0, 1.0, d);
            tooth_union = max(tooth_union, t);
        }
        coverage = max(ring, tooth_union);
    } else if kind < 6.5 {
        // Rounded rectangle. params: (half-height, alpha, half-width, radius).
        let half_h = in.params.x;
        let half_w = max(in.params.z, 0.0);
        let radius = max(in.params.w, 0.0);
        let d = sd_round_box(p, vec2<f32>(half_w, half_h), radius);
        coverage = 1.0 - smoothstep(-1.0, 1.0, d);
    } else if kind < 7.5 {
        // Check mark.
        let r = in.params.x;
        let w = max(in.params.z, 1.0);
        let a = vec2<f32>(-0.42 * r, -0.02 * r);
        let b = vec2<f32>(-0.12 * r, 0.32 * r);
        let c = vec2<f32>(0.48 * r, -0.36 * r);
        let d1 = sd_segment(p - a, b - a, w);
        let d2 = sd_segment(p - b, c - b, w);
        coverage = 1.0 - smoothstep(-1.0, 1.0, min(d1, d2));
    } else {
        // Chevron pointing right.
        let r = in.params.x;
        let w = max(in.params.z, 1.0);
        let a = vec2<f32>(-0.22 * r, -0.46 * r);
        let b = vec2<f32>(0.28 * r, 0.0);
        let c = vec2<f32>(-0.22 * r, 0.46 * r);
        let d1 = sd_segment(p - a, b - a, w);
        let d2 = sd_segment(p - b, c - b, w);
        coverage = 1.0 - smoothstep(-1.0, 1.0, min(d1, d2));
    }

    // Only the edit-badge glyph (kind 4) is masked to the page frame. The
    // gear (kind 5) and all bottom-control ink are frame-independent.
    if kind > 3.5 && kind < 4.5 {
        coverage = coverage * frame_alpha(in.pos.xy);
    }

    let a = coverage * alpha;
    if a <= 0.001 {
        discard;
    }
    return vec4<f32>(in.color.rgb * a, a);
}
