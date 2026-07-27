use crate::ui_model::geometry::{Point, Rect};
use crate::ui_model::ids::UiId;

/// Widget operation result returned by every widget and layout container.
///
/// Carries the widget's layout rectangle, hit-test region, and current input
/// state for the frame.  `UiId` is *not* `Copy` (it wraps a `String`), so
/// `Response` is only `Clone` — not `Copy`.
#[derive(Clone, Debug, PartialEq)]
pub struct Response {
    pub id: UiId,
    /// Visual ("ink") rectangle occupied by the widget.
    pub rect: Rect,
    /// Hit-test rectangle (may be larger than `rect` for touch-friendly taps).
    pub hit_rect: Rect,
    pub hovered: bool,
    pub pressed: bool,
    pub clicked: bool,
    pub focused: bool,
    /// `true` when the widget's value changed this frame (e.g. toggle toggled).
    pub changed: bool,
}

impl Default for Response {
    fn default() -> Self {
        Self {
            id: UiId::named(""),
            rect: Rect::default(),
            hit_rect: Rect::default(),
            hovered: false,
            pressed: false,
            clicked: false,
            focused: false,
            changed: false,
        }
    }
}

impl Response {
    /// Alias for the visual rectangle — convenience so callers can write
    /// `resp.rect()` instead of `resp.rect`.
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// Returns `true` when `point` lies inside the hit-test rectangle.
    pub fn contains(&self, point: Point) -> bool {
        self.hit_rect.contains(point)
    }
}

#[cfg(test)]
mod tests {
    use super::Response;
    use crate::ui_model::geometry::{Point, Rect};
    use crate::ui_model::ids::UiId;

    #[test]
    fn default_response_has_empty_id() {
        let r = Response::default();
        assert_eq!(r.id, UiId::named(""));
    }

    #[test]
    fn contains_uses_hit_rect() {
        let r = Response {
            id: UiId::named("btn"),
            rect: Rect::new(10.0, 10.0, 20.0, 20.0),
            hit_rect: Rect::new(5.0, 5.0, 30.0, 30.0),
            ..Default::default()
        };

        // Inside hit_rect but outside visual rect
        assert!(r.contains(Point::new(6.0, 6.0)));
        // Outside hit_rect
        assert!(!r.contains(Point::new(0.0, 0.0)));
    }

    #[test]
    fn rect_alias_returns_visual_rect() {
        let r = Response {
            id: UiId::named("btn"),
            rect: Rect::new(1.0, 2.0, 3.0, 4.0),
            hit_rect: Rect::new(0.0, 0.0, 5.0, 6.0),
            ..Default::default()
        };

        assert_eq!(r.rect(), Rect::new(1.0, 2.0, 3.0, 4.0));
    }
}
