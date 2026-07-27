use std::collections::HashMap;

use crate::ui_model::geometry::Rect;
use crate::ui_model::ids::UiId;

/// `UiId` → final layout rectangle registry.
///
/// After a frame is built, tooling (tutorials, overlays, accessibility) can
/// look up the on-screen position of a named element via
/// [`Registry::rect`] / [`Registry::hit_rect`].
#[derive(Clone, Debug, Default)]
pub struct Registry {
    rects: HashMap<UiId, Rect>,
    hit_rects: HashMap<UiId, Rect>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or overwrite) the visual and hit-test rectangles for `id`.
    pub fn register(&mut self, id: UiId, rect: Rect, hit_rect: Rect) {
        self.rects.insert(id.clone(), rect);
        self.hit_rects.insert(id, hit_rect);
    }

    /// Visual ("ink") rectangle registered for `id`.
    pub fn rect(&self, id: &UiId) -> Option<Rect> {
        self.rects.get(id).copied()
    }

    /// Hit-test rectangle registered for `id` (may be larger than the visual
    /// rect for touch-friendly targets).
    pub fn hit_rect(&self, id: &UiId) -> Option<Rect> {
        self.hit_rects.get(id).copied()
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.rects.clear();
        self.hit_rects.clear();
    }

    /// Number of registered entries.
    pub fn len(&self) -> usize {
        self.rects.len()
    }

    /// Returns `true` when no entry is registered.
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::Registry;
    use crate::ui_model::geometry::Rect;
    use crate::ui_model::ids::UiId;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(x, y, w, h)
    }

    #[test]
    fn register_and_retrieve_rect() {
        let mut reg = Registry::new();
        let id = UiId::named("test-btn");
        reg.register(
            id.clone(),
            r(10.0, 20.0, 100.0, 40.0),
            r(8.0, 18.0, 104.0, 44.0),
        );

        assert_eq!(reg.rect(&id), Some(r(10.0, 20.0, 100.0, 40.0)));
        assert_eq!(reg.hit_rect(&id), Some(r(8.0, 18.0, 104.0, 44.0)));
    }

    #[test]
    fn re_register_same_id_overwrites() {
        let mut reg = Registry::new();
        let id = UiId::named("btn");
        reg.register(id.clone(), r(0.0, 0.0, 10.0, 10.0), r(0.0, 0.0, 10.0, 10.0));
        reg.register(id.clone(), r(1.0, 2.0, 30.0, 40.0), r(1.0, 2.0, 32.0, 42.0));

        assert_eq!(reg.rect(&id), Some(r(1.0, 2.0, 30.0, 40.0)));
        assert_eq!(reg.hit_rect(&id), Some(r(1.0, 2.0, 32.0, 42.0)));
    }

    #[test]
    fn unknown_id_returns_none() {
        let reg = Registry::new();
        assert_eq!(reg.rect(&UiId::named("nope")), None);
        assert_eq!(reg.hit_rect(&UiId::named("nope")), None);
    }

    #[test]
    fn clear_removes_all_entries() {
        let mut reg = Registry::new();
        reg.register(
            UiId::named("a"),
            r(0.0, 0.0, 1.0, 1.0),
            r(0.0, 0.0, 1.0, 1.0),
        );
        reg.register(
            UiId::named("b"),
            r(0.0, 0.0, 1.0, 1.0),
            r(0.0, 0.0, 1.0, 1.0),
        );
        assert_eq!(reg.len(), 2);

        reg.clear();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert_eq!(reg.rect(&UiId::named("a")), None);
    }
}
