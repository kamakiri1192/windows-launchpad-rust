use crate::ui_model::geometry::{Point, Rect};
use crate::ui_model::hit::HitTarget;
use crate::ui_model::ids::UiId;

#[derive(Debug, Clone, PartialEq)]
pub enum HitShape {
    Rect { rect: Rect, inclusive: bool },
    Circle { center: Point, radius: f32 },
}

impl HitShape {
    pub const fn rect(rect: Rect) -> Self {
        Self::Rect {
            rect,
            inclusive: false,
        }
    }

    pub const fn rect_inclusive(rect: Rect) -> Self {
        Self::Rect {
            rect,
            inclusive: true,
        }
    }

    pub const fn circle(center: Point, radius: f32) -> Self {
        Self::Circle { center, radius }
    }

    pub fn contains(&self, point: Point) -> bool {
        match self {
            Self::Rect { rect, inclusive } => {
                if *inclusive {
                    point.x >= rect.min_x()
                        && point.x <= rect.max_x()
                        && point.y >= rect.min_y()
                        && point.y <= rect.max_y()
                } else {
                    rect.contains(point)
                }
            }
            Self::Circle { center, radius } => {
                let dx = point.x - center.x;
                let dy = point.y - center.y;
                dx * dx + dy * dy <= radius * radius
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HitRegion {
    pub id: UiId,
    pub rect: Rect,
    pub shape: HitShape,
    pub target: HitTarget,
    pub z: i16,
}

impl HitRegion {
    pub fn new(id: UiId, rect: Rect, target: HitTarget, z: i16) -> Self {
        Self {
            id,
            rect,
            shape: HitShape::rect(rect),
            target,
            z,
        }
    }

    pub fn rect_inclusive(id: UiId, rect: Rect, target: HitTarget, z: i16) -> Self {
        Self {
            id,
            rect,
            shape: HitShape::rect_inclusive(rect),
            target,
            z,
        }
    }

    pub fn circle(id: UiId, center: Point, radius: f32, target: HitTarget, z: i16) -> Self {
        let rect = Rect::new(
            center.x - radius,
            center.y - radius,
            radius * 2.0,
            radius * 2.0,
        );
        Self {
            id,
            rect,
            shape: HitShape::circle(center, radius),
            target,
            z,
        }
    }

    pub fn contains(&self, point: Point) -> bool {
        self.shape.contains(point)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HitMap {
    regions: Vec<HitRegion>,
}

impl HitMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_regions(regions: Vec<HitRegion>) -> Self {
        Self { regions }
    }

    pub fn push(&mut self, region: HitRegion) {
        self.regions.push(region);
    }

    pub fn regions(&self) -> &[HitRegion] {
        &self.regions
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn hit_test(&self, point: Point) -> Option<&HitRegion> {
        self.regions
            .iter()
            .enumerate()
            .filter(|(_, region)| region.contains(point))
            .max_by(|(left_index, left), (right_index, right)| {
                left.z
                    .cmp(&right.z)
                    .then_with(|| left_index.cmp(right_index))
            })
            .map(|(_, region)| region)
    }
}

#[cfg(test)]
mod tests {
    use super::{HitMap, HitRegion};
    use crate::ui_model::geometry::{Point, Rect};
    use crate::ui_model::hit::HitTarget;
    use crate::ui_model::ids::UiId;

    fn region(key: &str, rect: Rect, z: i16) -> HitRegion {
        HitRegion::new(
            UiId::launcher_item(key),
            rect,
            HitTarget::launcher_item(key),
            z,
        )
    }

    #[test]
    fn hit_test_returns_none_when_no_region_contains_point() {
        let map = HitMap::with_regions(vec![region("calc", Rect::new(0.0, 0.0, 10.0, 10.0), 0)]);

        assert!(map.hit_test(Point::new(20.0, 20.0)).is_none());
    }

    #[test]
    fn hit_test_uses_rect_containment() {
        let map = HitMap::with_regions(vec![region("calc", Rect::new(10.0, 20.0, 30.0, 40.0), 0)]);

        assert_eq!(
            map.hit_test(Point::new(10.0, 20.0))
                .map(|hit| hit.id.as_str()),
            Some("launcher-item:calc")
        );
        assert!(map.hit_test(Point::new(40.0, 20.0)).is_none());
        assert!(map.hit_test(Point::new(10.0, 60.0)).is_none());
    }

    #[test]
    fn hit_test_can_use_inclusive_rect_containment() {
        let map = HitMap::with_regions(vec![HitRegion::rect_inclusive(
            UiId::launcher_item("calc"),
            Rect::new(10.0, 20.0, 30.0, 40.0),
            HitTarget::launcher_item("calc"),
            0,
        )]);

        assert_eq!(
            map.hit_test(Point::new(40.0, 60.0))
                .map(|hit| hit.id.as_str()),
            Some("launcher-item:calc")
        );
    }

    #[test]
    fn hit_test_can_use_circle_containment() {
        let map = HitMap::with_regions(vec![HitRegion::circle(
            UiId::launcher_item("calc"),
            Point::new(20.0, 20.0),
            10.0,
            HitTarget::launcher_item("calc"),
            0,
        )]);

        assert_eq!(
            map.hit_test(Point::new(30.0, 20.0))
                .map(|hit| hit.id.as_str()),
            Some("launcher-item:calc")
        );
        assert!(map.hit_test(Point::new(29.0, 29.0)).is_none());
    }

    #[test]
    fn hit_test_returns_highest_z_region() {
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let map = HitMap::with_regions(vec![region("back", rect, 0), region("front", rect, 10)]);

        assert_eq!(
            map.hit_test(Point::new(50.0, 50.0))
                .map(|hit| hit.id.as_str()),
            Some("launcher-item:front")
        );
    }

    #[test]
    fn hit_test_returns_later_region_when_z_matches() {
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let map = HitMap::with_regions(vec![region("first", rect, 5), region("second", rect, 5)]);

        assert_eq!(
            map.hit_test(Point::new(50.0, 50.0))
                .map(|hit| hit.id.as_str()),
            Some("launcher-item:second")
        );
    }

    #[test]
    fn push_preserves_region_order_for_equal_z_tie_breaks() {
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let mut map = HitMap::new();

        map.push(region("first", rect, 0));
        map.push(region("second", rect, 0));

        assert_eq!(map.len(), 2);
        assert_eq!(
            map.hit_test(Point::new(50.0, 50.0))
                .map(|hit| hit.id.as_str()),
            Some("launcher-item:second")
        );
    }
}
