use crate::ui_model::geometry::Rect;
use crate::ui_model::ids::UiId;
use crate::ui_model::text::TextView;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderModel {
    pub glass: Vec<GlassSurface>,
    pub tiles: Vec<TileView>,
    pub icons: Vec<IconView>,
    pub text: Vec<TextView>,
    pub controls: Vec<ControlView>,
}

impl RenderModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.glass.is_empty()
            && self.tiles.is_empty()
            && self.icons.is_empty()
            && self.text.is_empty()
            && self.controls.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlassSurface {
    pub id: UiId,
    pub rect: Rect,
    pub radius: f32,
    pub material: GlassMaterial,
    pub layer: GlassLayer,
    pub z: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassMaterial {
    Regular,
    Prominent,
}

/// Renderer-neutral compositing lane for a glass surface.
///
/// This describes how a surface participates in the frame, not which feature
/// produced it. The renderer must not infer settings/search/folder semantics
/// from [`UiId`] values in order to choose a GPU pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlassLayer {
    Base,
    Overlay,
    Modal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TileView {
    pub id: UiId,
    pub rect: Rect,
    pub radius: f32,
    pub color: Color,
    pub z: i16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IconView {
    pub id: UiId,
    pub rect: Rect,
    pub source: IconSource,
    pub z: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IconSource {
    AtlasCell(String),
    Placeholder,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlView {
    pub id: UiId,
    pub rect: Rect,
    pub kind: ControlKind,
    pub opacity: f32,
    pub z: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ControlKind {
    SearchPill,
    PageIndicator,
    SearchField,
    Magnifier,
    Dot,
    Caret,
    CloseButton,
    SettingsGear,
    EditBadge,
    RowBackground,
    Toggle,
    Checkmark,
    Chevron,
    Divider,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

#[cfg(test)]
mod tests {
    use super::RenderModel;

    #[test]
    fn new_render_model_is_empty() {
        assert!(RenderModel::new().is_empty());
    }
}
