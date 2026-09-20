// Shared by tile, icon, label, and folder-glass presentation. Centers are
// screen-space owner pivots AFTER scrolling, never individual glyph/child centers.
fn launch_item_progress(center: vec2<f32>, frame: vec4<f32>, timing: vec4<f32>) -> f32 {
    if timing.x >= 1.0 { return 1.0; }
    let normalized = (center - frame.xy) / max(frame.zw, vec2<f32>(1.0));
    let delay = clamp(length(normalized), 0.0, 1.0) * timing.w;
    let local = clamp((timing.x - delay) / max(1.0 - delay, 0.001), 0.0, 1.0);
    return smoothstep(0.0, 1.0, local);
}

fn launch_gather_offset(center: vec2<f32>, frame: vec4<f32>, progress: f32, fraction: f32) -> vec2<f32> {
    if progress >= 1.0 { return vec2<f32>(0.0); }
    let normalized = (center - frame.xy) / max(frame.zw, vec2<f32>(1.0));
    let radial = length(normalized);
    if radial <= 0.001 { return vec2<f32>(0.0); }
    return normalized / radial * min(frame.z, frame.w) * fraction * (1.0 - progress);
}
