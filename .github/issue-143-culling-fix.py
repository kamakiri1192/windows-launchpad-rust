from pathlib import Path

path = Path("src/liquid_glass/renderer.rs")
text = path.read_text(encoding="utf-8")

old = """    if !shape.is_scrolling() || shape.has_tint_override() {
        return true;
    }
    let bounds = shape.screen_bounds(scroll_x);"""
new = """    if !shape.is_scrolling() {
        return true;
    }
    let bounds = shape.screen_bounds(scroll_x);"""
if text.count(old) != 1:
    raise RuntimeError("unexpected base culling guard")
text = text.replace(old, new)

old = """    if intersect_bounds(influence_bounds, frame.screen_bounds(0.0)).is_none() {
        return false;
    }

    // A scrolling rounded rect"""
new = """    if intersect_bounds(influence_bounds, frame.screen_bounds(0.0)).is_none() {
        return false;
    }
    if shape.has_tint_override() {
        return true;
    }

    // A scrolling rounded rect"""
if text.count(old) != 1:
    raise RuntimeError("unexpected frame intersection guard")
text = text.replace(old, new)

old = """        let far_page = GlassShape::rounded_rect([1_200.0, 350.0], [100.0, 100.0], 30.0);
        let swallowed = GlassShape::rounded_rect([500.0, 350.0], [100.0, 100.0], 30.0);"""
new = """        let far_page = GlassShape::rounded_rect([1_200.0, 350.0], [100.0, 100.0], 30.0);
        let tinted_far_page = far_page.with_tint(Some([1.0, 0.5, 0.25, 0.75]));
        let swallowed = GlassShape::rounded_rect([500.0, 350.0], [100.0, 100.0], 30.0);"""
if text.count(old) != 1:
    raise RuntimeError("unexpected culling test setup")
text = text.replace(old, new)

old = """        assert!(!base_shape_may_affect_frame(far_page, 0.0, frame, 26.0));
        assert!(!base_shape_may_affect_frame(swallowed, 0.0, frame, 26.0));"""
new = """        assert!(!base_shape_may_affect_frame(far_page, 0.0, frame, 26.0));
        assert!(!base_shape_may_affect_frame(
            tinted_far_page,
            0.0,
            frame,
            26.0
        ));
        assert!(!base_shape_may_affect_frame(swallowed, 0.0, frame, 26.0));"""
if text.count(old) != 1:
    raise RuntimeError("unexpected culling assertions")
text = text.replace(old, new)

path.write_text(text, encoding="utf-8", newline="\n")
print("Refined tinted shape culling")
