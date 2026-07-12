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
