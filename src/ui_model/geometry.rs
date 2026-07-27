#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn from_origin_size(origin: Point, size: Size) -> Self {
        Self {
            x: origin.x,
            y: origin.y,
            width: size.width,
            height: size.height,
        }
    }

    pub const fn min_x(&self) -> f32 {
        self.x
    }

    pub const fn min_y(&self) -> f32 {
        self.y
    }

    pub const fn max_x(&self) -> f32 {
        self.x + self.width
    }

    pub const fn max_y(&self) -> f32 {
        self.y + self.height
    }

    pub const fn contains(&self, point: Point) -> bool {
        point.x >= self.min_x()
            && point.x < self.max_x()
            && point.y >= self.min_y()
            && point.y < self.max_y()
    }

    pub const fn center(&self) -> Point {
        Point::new(self.x + self.width * 0.5, self.y + self.height * 0.5)
    }

    pub const fn inset(&self, insets: Insets) -> Self {
        Self {
            x: self.x + insets.left,
            y: self.y + insets.top,
            width: self.width - insets.left - insets.right,
            height: self.height - insets.top - insets.bottom,
        }
    }

    /// Returns the intersection of `self` and `other` (common rectangular
    /// area) using the same half-open boundary convention as [`Rect::contains`].
    /// Returns `None` if the intersection has zero area (width or height ≤ 0).
    pub fn intersection(self, other: Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let max_x = self.max_x().min(other.max_x());
        let max_y = self.max_y().min(other.max_y());
        let width = max_x - x;
        let height = max_y - y;
        if width > 0.0 && height > 0.0 {
            Some(Rect {
                x,
                y,
                width,
                height,
            })
        } else {
            None
        }
    }

    /// Returns `true` if `self` and `other` share any area (their
    /// intersection has positive width and height).
    pub fn intersects(self, other: Rect) -> bool {
        self.x.max(other.x) < self.max_x().min(other.max_x())
            && self.y.max(other.y) < self.max_y().min(other.max_y())
    }

    /// Returns `true` when `other` lies entirely inside `self`, using the
    /// same half-open boundary convention as [`Rect::contains`].
    pub const fn contains_rect(self, other: Rect) -> bool {
        other.min_x() >= self.min_x()
            && other.max_x() <= self.max_x()
            && other.min_y() >= self.min_y()
            && other.max_y() <= self.max_y()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Insets {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Insets {
    pub const fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    pub const fn all(value: f32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    pub const fn symmetric(horizontal: f32, vertical: f32) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Insets, Point, Rect};

    #[test]
    fn contains_includes_min_edges_and_excludes_max_edges() {
        let rect = Rect::new(10.0, 20.0, 100.0, 50.0);

        assert!(rect.contains(Point::new(10.0, 20.0)));
        assert!(rect.contains(Point::new(109.999, 69.999)));
        assert!(!rect.contains(Point::new(110.0, 20.0)));
        assert!(!rect.contains(Point::new(10.0, 70.0)));
        assert!(!rect.contains(Point::new(9.999, 20.0)));
        assert!(!rect.contains(Point::new(10.0, 19.999)));
    }

    #[test]
    fn center_returns_midpoint() {
        let rect = Rect::new(10.0, 20.0, 100.0, 50.0);

        assert_eq!(rect.center(), Point::new(60.0, 45.0));
    }

    #[test]
    fn inset_moves_edges_inward() {
        let rect = Rect::new(10.0, 20.0, 100.0, 50.0);

        assert_eq!(
            rect.inset(Insets::new(1.0, 2.0, 3.0, 4.0)),
            Rect::new(14.0, 21.0, 94.0, 46.0)
        );
    }

    #[test]
    fn inset_accepts_negative_values_to_expand_rect() {
        let rect = Rect::new(10.0, 20.0, 100.0, 50.0);

        assert_eq!(
            rect.inset(Insets::symmetric(-5.0, -10.0)),
            Rect::new(5.0, 10.0, 110.0, 70.0)
        );
    }

    #[test]
    fn intersection_overlapping_rects() {
        let a = Rect::new(10.0, 20.0, 100.0, 50.0);
        let b = Rect::new(30.0, 30.0, 60.0, 30.0);

        assert_eq!(a.intersection(b), Some(Rect::new(30.0, 30.0, 60.0, 30.0)));
        assert_eq!(b.intersection(a), Some(Rect::new(30.0, 30.0, 60.0, 30.0)));
    }

    #[test]
    fn intersection_touching_edges_returns_none() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(10.0, 0.0, 10.0, 10.0); // touches a's right edge

        assert_eq!(a.intersection(b), None);
    }

    #[test]
    fn intersection_one_rect_fully_contained() {
        let outer = Rect::new(0.0, 0.0, 100.0, 100.0);
        let inner = Rect::new(10.0, 20.0, 30.0, 40.0);

        assert_eq!(outer.intersection(inner), Some(inner));
    }

    #[test]
    fn intersection_disjoint_returns_none() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(20.0, 20.0, 10.0, 10.0);

        assert_eq!(a.intersection(b), None);
    }

    #[test]
    fn intersection_aligned_on_one_axis() {
        let a = Rect::new(0.0, 0.0, 20.0, 10.0);
        let b = Rect::new(5.0, 0.0, 10.0, 10.0); // same height, overlapping x

        assert_eq!(a.intersection(b), Some(Rect::new(5.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn intersects_returns_true_for_overlapping() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);

        assert!(a.intersects(b));
    }

    #[test]
    fn intersects_returns_false_for_disjoint() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(20.0, 20.0, 10.0, 10.0);

        assert!(!a.intersects(b));
    }

    #[test]
    fn intersects_returns_false_for_touching_edges() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(10.0, 0.0, 10.0, 10.0);

        assert!(!a.intersects(b));
    }

    #[test]
    fn contains_rect_fully_inside() {
        let outer = Rect::new(0.0, 0.0, 100.0, 100.0);
        let inner = Rect::new(10.0, 20.0, 30.0, 40.0);

        assert!(outer.contains_rect(inner));
    }

    #[test]
    fn contains_rect_same_rect() {
        let a = Rect::new(10.0, 20.0, 30.0, 40.0);

        assert!(a.contains_rect(a));
    }

    #[test]
    fn contains_rect_partially_outside() {
        let outer = Rect::new(0.0, 0.0, 100.0, 100.0);
        let half_out = Rect::new(80.0, 0.0, 30.0, 100.0); // extends past right edge

        assert!(!outer.contains_rect(half_out));
    }

    #[test]
    fn contains_rect_fully_outside() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(20.0, 20.0, 10.0, 10.0);

        assert!(!a.contains_rect(b));
    }

    #[test]
    fn contains_rect_touches_inner_edge() {
        let outer = Rect::new(0.0, 0.0, 100.0, 100.0);
        // inner rect at min-x edge (inclusive)
        let at_min = Rect::new(0.0, 10.0, 20.0, 30.0);
        assert!(outer.contains_rect(at_min));

        // inner rect at max-x edge (exclusive — still contained because
        // every point in `at_max` has x < 100.0)
        let at_max = Rect::new(80.0, 10.0, 20.0, 30.0);
        assert!(outer.contains_rect(at_max));
    }
}

/// Axis-aligned rounded-rectangle clip region, applied per-instance in the
/// shader (hard discard outside). `radius == 0.0` means sharp corners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipRegion {
    pub rect: Rect,
    pub radius: f32,
}

impl ClipRegion {
    pub const fn new(rect: Rect, radius: f32) -> Self {
        Self { rect, radius }
    }
}

/// UV rectangle of one icon inside the atlas, in 0..1 texture coordinates.
///
/// Stored as a 4-f32 pack so it slots directly into a `@location` instance
/// attribute in the icon shader. This is renderer-neutral data (texture
/// coordinates carry no feature semantics), so it lives in `ui_model` rather
/// than in any feature or worker module. Domain types such as
/// [`crate::domain::app_registry`] reference it without pulling in GPU or
/// worker dependencies.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UvRect {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}
