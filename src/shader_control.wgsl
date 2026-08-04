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
//  10 = slider track (wide rounded bar) — params: (half_h, alpha, half_w, radius)
//  11 = slider knob (filled disk) — params: (radius, alpha, _, _)
//  12 = reset arrow (counterclockwise ↺) — params: (radius, alpha, stroke, _)
//  13 = pencil (context menu: edit home) — params: (size, alpha, stroke, _)
//  14 = eye-off (context menu: hide app) — params: (size, alpha, stroke, _)
//  15 = folder (context menu: reveal in Finder/Explorer) — params: (size, alpha, stroke, _)
//  16 = plus (context menu: larger icon) — params: (size, alpha, stroke, _)
//  17 = minus (context menu: smaller icon) — params: (size, alpha, stroke, _)
//  18 = info (context menu: app info) — params: (size, alpha, stroke, _)
struct InstanceIn {
    @location(0) center: vec2<f32>,  // physical px center of the element
    @location(1) params: vec4<f32>,  // (size_or_radius, alpha, active/extra, _pad)
    @location(2) color: vec4<f32>,   // rgba tint (non-premultiplied)
    @location(3) kind: vec4<f32>,    // (kind, a, b, c) element-specific
    @location(4) clip_rect: vec4<f32>,  // (min_x, min_y, width, height); width<=0 → no clip
    @location(5) clip_radius: vec4<f32>, // (radius, 0, 0, 0)
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,   // px relative to element center
    @location(1) params: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) kind: vec4<f32>,
    @location(4) pixel_pos: vec2<f32>,  // screen px position for clip test
    @location(5) clip_rect: vec4<f32>,
    @location(6) clip_radius: vec4<f32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @location(0) center: vec2<f32>,
    @location(1) params: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) kind: vec4<f32>,
    @location(4) clip_rect: vec4<f32>,
    @location(5) clip_radius: vec4<f32>,
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
    // Only the two edit-badge kinds carry animation data in `kind.yzw`.
    // Context-menu icons use kinds 13–18 and leave those fields at zero; if
    // they enter this branch they rotate around the screen origin using the
    // shared frame clock, making their position drift away from the label.
    if (kind.x > 3.5 && kind.x < 4.5)
        || (kind.x > 8.5 && kind.x < 9.5) {
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
    // Pass screen-space pixel position for per-instance clip test.
    out.pixel_pos = world;
    out.clip_rect = clip_rect;
    out.clip_radius = clip_radius;
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
    if kind > 9.5 && kind < 10.5 {
        // slider track: params.z carries half-width.
        return max(params.z, size) * 1.05;
    }
    if kind > 10.5 && kind < 11.5 {
        // slider knob: a filled disk of radius `size`.
        return size * 1.6;
    }
    if kind > 11.5 {
        // reset arrow: roughly a disk of radius `size`.
        return size * 1.6;
    }
    if kind > 6.5 {
        return size * 1.8;
    }
    // Context-menu glyphs (kinds 13–18): `size` is the full icon extent.
    if kind > 12.5 {
        return size * 1.2;
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
    } else if kind < 4.5 || (kind > 8.5 && kind < 9.5) {
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
    } else if kind < 8.5 {
        // Chevron pointing right.
        let r = in.params.x;
        let w = max(in.params.z, 1.0);
        let a = vec2<f32>(-0.22 * r, -0.46 * r);
        let b = vec2<f32>(0.28 * r, 0.0);
        let c = vec2<f32>(-0.22 * r, 0.46 * r);
        let d1 = sd_segment(p - a, b - a, w);
        let d2 = sd_segment(p - b, c - b, w);
        coverage = 1.0 - smoothstep(-1.0, 1.0, min(d1, d2));
    } else if kind < 10.5 {
        // Slider track: a wide, low rounded bar. params: (half_h, alpha, half_w, radius).
        let half_h = in.params.x;
        let half_w = max(in.params.z, 0.0);
        let radius = max(in.params.w, 0.0);
        let d = sd_round_box(p, vec2<f32>(half_w, half_h), radius);
        coverage = 1.0 - smoothstep(-1.0, 1.0, d);
    } else if kind < 11.5 {
        // Slider knob: a filled disk of radius `size`.
        let r = in.params.x;
        let d = sd_circle(p, r);
        coverage = 1.0 - smoothstep(-1.0, 1.0, d);
    } else if kind < 12.5 {
        // Reset arrow (↺): an open ring (≈270°) plus an arrowhead at the
        // upper-left opening. Local space is Y-down, so "upper-left" maps to
        // negative-x / negative-y. params: (radius, alpha, stroke, _).
        let r = in.params.x;
        let stroke = max(in.params.z, 1.0);
        // Ring annulus.
        let d_ring = abs(sd_circle(p, r)) - stroke * 0.5;
        let ring = 1.0 - smoothstep(-1.0, 1.0, d_ring);
        // Cut a small wedge at the opening so the ring reads as ↺ rather
        // than a full circle: zero out coverage where the angle is in the
        // opening sector around +x (right side), simulating an open top.
        // We carve the opening on the upper-right by rotating into that
        // frame; angle measured from +x axis, CCW.
        let ang = atan2(-p.y, p.x); // negate y because local is Y-down
        // Opening centered at 45° (upper-right), ~110° wide.
        let opening = smoothstep(0.55, 0.95, 1.0 - abs(ang - 0.7854) / 0.95);
        // Arrowhead: a small triangle/chevron at the opening's lower tip
        // (around angle ~ -10°), pointing back toward the ring center.
        // Place a short segment near (r*cos(-10°), -r*sin(-10°)).
        let tip_ang = -0.17;
        let tip = vec2<f32>(r * cos(tip_ang), r * sin(tip_ang) * -1.0);
        let aw = max(stroke * 0.9, 1.2);
        let al = r * 0.34;
        // Two short strokes forming a "V" opening downward-left (toward center).
        let dir1 = normalize(vec2<f32>(-0.6, -0.8));
        let dir2 = normalize(vec2<f32>(0.6, -0.8));
        let d_a1 = sd_segment(p - tip, dir1 * al, aw);
        let d_a2 = sd_segment(p - tip, dir2 * al, aw);
        let arrow = 1.0 - smoothstep(-1.0, 1.0, min(d_a1, d_a2));
        coverage = max(ring * (1.0 - opening), arrow);
    } else if kind < 13.5 {
        // Pencil: a diagonal body (rounded box) + triangular tip. Local space
        // is Y-down; the pencil points toward the lower-left (writing tip).
        // params: (size, alpha, stroke, _).
        let size = in.params.x;
        let stroke = max(in.params.z, 1.0);
        // Body: a rounded box rotated ~45°, occupying the upper-right half.
        let body_len = size * 0.62;
        let body_w = max(size * 0.16, stroke);
        // Rotate p by +45° (CW in Y-down) so the body lies along the new x axis.
        let ang = 0.785398; // 45°
        let ca = cos(ang);
        let sa = sin(ang);
        let rp = vec2<f32>(p.x * ca + p.y * sa, -p.x * sa + p.y * ca);
        let body = sd_round_box(
            rp - vec2<f32>(size * 0.06, -size * 0.06),
            vec2<f32>(body_len * 0.5, body_w),
            body_w * 0.5,
        );
        coverage = 1.0 - smoothstep(-1.0, 1.0, body);
    } else if kind < 14.5 {
        // Eye-off: an eye outline (two arcs forming a lens shape) with a
        // diagonal slash through it. params: (size, alpha, stroke, _).
        let size = in.params.x;
        let stroke = max(in.params.z, 1.0);
        // Eye outline: approximate with two arcs. We build the eye as the
        // region between an upper and lower parabola-ish boundary using sd_segment
        // approximations is complex; instead draw the eye as a wide rounded
        // lens: two horizontal segments forming top and bottom lids.
        let w = size * 0.5;
        let h = size * 0.28;
        // Top lid: arc from (-w,0) up to (0,-h) down to (w,0).
        let top_a = vec2<f32>(-w, 0.0);
        let top_b = vec2<f32>(0.0, -h);
        let top_c = vec2<f32>(w, 0.0);
        let d_top1 = sd_segment(p - top_a, top_b - top_a, stroke * 0.5);
        let d_top2 = sd_segment(p - top_b, top_c - top_b, stroke * 0.5);
        // Bottom lid mirrors.
        let bot_b = vec2<f32>(0.0, h);
        let d_bot1 = sd_segment(p - top_a, bot_b - top_a, stroke * 0.5);
        let d_bot2 = sd_segment(p - bot_b, top_c - bot_b, stroke * 0.5);
        // Pupil: a small filled disk.
        let pupil = sd_circle(p, size * 0.12);
        let eye = min(min(d_top1, d_top2), min(d_bot1, d_bot2));
        let eye_cov = 1.0 - smoothstep(-1.0, 1.0, eye);
        let pupil_cov = 1.0 - smoothstep(-1.0, 1.0, pupil);
        // Slash: a thick diagonal segment across the eye.
        let slash_a = vec2<f32>(-size * 0.5, -size * 0.5);
        let slash_b = vec2<f32>(size * 0.5, size * 0.5);
        let d_slash = sd_segment(p - slash_a, slash_b - slash_a, stroke);
        let slash = 1.0 - smoothstep(-1.0, 1.0, d_slash);
        coverage = max(max(eye_cov, pupil_cov), slash);
    } else if kind < 15.5 {
        // Folder: a rounded body with a tab on the upper-left. params:
        // (size, alpha, stroke, _). `size` is the folder's half-width.
        let size = in.params.x;
        let stroke = max(in.params.z, 1.0);
        let hw = size * 0.5;
        let hh = size * 0.4;
        let r = size * 0.1;
        // Main body: a rounded box centered slightly below origin.
        let body = sd_round_box(
            p - vec2<f32>(0.0, size * 0.06),
            vec2<f32>(hw, hh),
            r,
        );
        // Tab: a smaller rounded box on the upper-left.
        let tab = sd_round_box(
            p - vec2<f32>(-hw * 0.45, -hh * 0.85),
            vec2<f32>(hw * 0.5, hh * 0.3),
            r * 0.8,
        );
        // Outline stroke: we want just the border, so take abs of the union.
        let shape = min(body, tab);
        let outline = abs(shape) - stroke;
        coverage = 1.0 - smoothstep(-1.0, 1.0, outline);
    } else if kind < 16.5 {
        // Plus: two crossed segments (horizontal + vertical). params:
        // (size, alpha, stroke, _).
        let size = in.params.x;
        let w = max(in.params.z, 1.0);
        let len = size * 0.5;
        let dv = sd_segment(p - vec2<f32>(0.0, -len), vec2<f32>(0.0, 2.0 * len), w);
        let dh = sd_segment(p - vec2<f32>(-len, 0.0), vec2<f32>(2.0 * len, 0.0), w);
        coverage = 1.0 - smoothstep(-1.0, 1.0, min(dv, dh));
    } else if kind < 17.5 {
        // Minus: a single horizontal segment. params: (size, alpha, stroke, _).
        let size = in.params.x;
        let w = max(in.params.z, 1.0);
        let len = size * 0.5;
        let d = sd_segment(p - vec2<f32>(-len, 0.0), vec2<f32>(2.0 * len, 0.0), w);
        coverage = 1.0 - smoothstep(-1.0, 1.0, d);
    } else {
        // Info (i): a dot above a vertical stem. params: (size, alpha, stroke, _).
        let size = in.params.x;
        let w = max(in.params.z, 1.0);
        // Dot at the top.
        let dot_center = vec2<f32>(0.0, -size * 0.38);
        let dot = sd_circle(p - dot_center, size * 0.11);
        // Stem: a vertical segment below the dot.
        let stem_a = vec2<f32>(0.0, -size * 0.14);
        let stem_b = vec2<f32>(0.0, size * 0.42);
        let stem = sd_segment(p - stem_a, stem_b - stem_a, w * 0.6);
        coverage = max(
            1.0 - smoothstep(-1.0, 1.0, dot),
            1.0 - smoothstep(-1.0, 1.0, stem),
        );
    }

    // Only the edit-badge glyph (kind 4) is masked to the page frame. The
    // gear (kind 5) and all bottom-control ink are frame-independent.
    if kind > 3.5 && kind < 4.5 {
        coverage = coverage * frame_alpha(in.pos.xy);
    }

    let a = coverage * alpha;

    // Per-instance clip: discard fragments outside the clip rect.
    if (in.clip_rect.z > 0.0) {
        let p = in.pixel_pos;
        let inside = p.x >= in.clip_rect.x && p.y >= in.clip_rect.y
                  && p.x < in.clip_rect.x + in.clip_rect.z
                  && p.y < in.clip_rect.y + in.clip_rect.w;
        if (!inside) {
            discard;
        }
        // Rounded corners via SDF.
        let r = in.clip_radius.x;
        if (r > 0.0) {
            let half = vec2<f32>(in.clip_rect.z, in.clip_rect.w) * 0.5;
            let center = vec2<f32>(in.clip_rect.x + half.x, in.clip_rect.y + half.y);
            let local_clip = p - center;
            let sd = sd_round_box(local_clip, half, r);
            if (sd > 0.0) {
                discard;
            }
        }
    }

    if a <= 0.001 {
        discard;
    }
    return vec4<f32>(in.color.rgb * a, a);
}
