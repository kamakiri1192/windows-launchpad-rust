use crate::ui_model::geometry::{Point, Rect, UvRect};
use crate::ui_model::grid::TileAnim;
use crate::ui_model::ids::UiId;
use crate::ui_model::text::TextView;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderModel {
    pub glass: Vec<GlassBatch>,
    pub tiles: Option<Vec<TileView>>,
    pub icons: Option<Vec<IconView>>,
    /// Fixed content composited after the generic modal glass lane.
    pub modal_tiles: Option<Vec<TileView>>,
    pub modal_icons: Option<Vec<IconView>>,
    pub text: Vec<TextView>,
    pub controls: Vec<ControlView>,
    /// Procedural renderer-neutral ink primitives, split into draw-order lanes.
    pub ink: Vec<InkBatch>,
    /// Shaped glyph geometry. Glyph rasterization/atlas upload remains a
    /// resource concern; frame submission uses these neutral quads.
    pub glyphs: Vec<GlyphBatch>,
}

impl RenderModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.glass.is_empty()
            && self.tiles.as_ref().is_none_or(Vec::is_empty)
            && self.icons.as_ref().is_none_or(Vec::is_empty)
            && self.modal_tiles.as_ref().is_none_or(Vec::is_empty)
            && self.modal_icons.as_ref().is_none_or(Vec::is_empty)
            && self.text.is_empty()
            && self.controls.is_empty()
            && self.ink.is_empty()
            && self.glyphs.is_empty()
    }

    pub fn set_glass_batch(&mut self, layer: GlassLayer, surfaces: Vec<GlassSurface>) {
        if let Some(batch) = self.glass.iter_mut().find(|batch| batch.layer == layer) {
            batch.surfaces = surfaces;
        } else {
            self.glass.push(GlassBatch { layer, surfaces });
        }
    }

    pub fn set_ink_batch(&mut self, lane: InkLane, views: Vec<InkView>) {
        if let Some(batch) = self.ink.iter_mut().find(|batch| batch.lane == lane) {
            batch.views = views;
        } else {
            self.ink.push(InkBatch { lane, views });
        }
    }

    pub fn set_glyph_batch(&mut self, lane: GlyphLane, views: Vec<GlyphView>) {
        if let Some(batch) = self.glyphs.iter_mut().find(|batch| batch.lane == lane) {
            batch.views = views;
        } else {
            self.glyphs.push(GlyphBatch { lane, views });
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlassSurface {
    pub id: UiId,
    pub rect: Rect,
    pub radius: f32,
    pub material: GlassMaterial,
    pub behavior: GlassBehavior,
    pub z: i16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlassBatch {
    pub layer: GlassLayer,
    pub surfaces: Vec<GlassSurface>,
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
    /// Glass surfaces composited above opaque grid fills but below grid icons
    /// and labels. This keeps nested glass boundaries distinct from the page
    /// frame's SDF union.
    GridOverlay,
    Overlay,
    Modal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TileView {
    pub id: UiId,
    pub rect: Rect,
    pub radius: f32,
    pub color: Color,
    pub has_icon: bool,
    pub motion: TileAnim,
    pub z: i16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IconView {
    pub id: UiId,
    pub rect: Rect,
    pub source: IconSource,
    pub motion: TileAnim,
    /// Optional common pivot for a rigid icon group, such as the 3x3
    /// miniatures inside a closed folder. The renderer keeps every child at
    /// its relative offset while the parent folder wiggles or follows a drag.
    pub motion_pivot: Option<Point>,
    pub z: i16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IconSource {
    AtlasCell(String),
    AtlasUv(UvRect),
    Placeholder,
}

/// Geometry behavior used by the Liquid Glass SDF without exposing its packed
/// numeric `shape_type` values to layout or feature code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlassBehavior {
    Scrolling,
    FixedFrame,
    Control,
    ClipOnly,
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

/// Draw-order lane for procedural foreground ink.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InkLane {
    Backdrop,
    BottomControl,
    Gear,
    Settings,
    EditBadge,
    Modal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InkBatch {
    pub lane: InkLane,
    pub views: Vec<InkView>,
}

/// Renderer-neutral procedural foreground primitive.
///
/// The named geometry fields deliberately avoid exposing the shader's packed
/// `ControlInstance` representation. The renderer owns that packing.
#[derive(Debug, Clone, PartialEq)]
pub struct InkView {
    pub id: UiId,
    pub center: Point,
    pub extent: f32,
    pub opacity: f32,
    /// Renderer-neutral request to blur the already-rendered lower scene
    /// inside this view's rounded geometry. Zero keeps the normal sharp scene.
    pub scene_blur: f32,
    pub stroke: f32,
    pub corner_radius: f32,
    pub color: Color,
    pub kind: ControlKind,
    pub z: i16,
}

/// Draw-order lane for already-shaped glyph geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlyphLane {
    Grid,
    BottomControl,
    Settings,
    Modal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlyphBatch {
    pub lane: GlyphLane,
    pub views: Vec<GlyphView>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GlyphView {
    pub id: UiId,
    pub rect: Rect,
    pub uv: UvRect,
    pub color: Color,
    pub z: i16,
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
