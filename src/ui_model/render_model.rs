use crate::ui_model::geometry::{ClipRegion, Point, Rect, UvRect};
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
    /// Fixed content for the context menu lane (background tiles + icons),
    /// kept separate from `modal_tiles`/`modal_icons` so the menu and an open
    /// folder panel can coexist without overwriting each other's content.
    pub context_menu_tiles: Option<Vec<TileView>>,
    pub context_menu_icons: Option<Vec<IconView>>,
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
            && self.context_menu_tiles.as_ref().is_none_or(Vec::is_empty)
            && self.context_menu_icons.as_ref().is_none_or(Vec::is_empty)
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
    pub clip: Option<ClipRegion>,
    /// Per-surface glass activation level (0.0 = idle, 1.0 = fully active).
    /// Controls blur/edge-light/saturation/chromatic-aberration intensity per-shape.
    pub activation: f32,
    /// Optional per-surface backdrop blur radius. `None` uses the renderer's
    /// global glass blur setting.
    pub blur_radius: Option<f32>,
    /// How strongly this surface replaces the transparent window backdrop
    /// with the filtered RGB selected by its compositing lane. That source
    /// may be the native desktop capture or a flattened completed scene.
    /// `0` keeps ordinary translucent glass compositing; `1` prevents the real
    /// desktop from bleeding back through an opaque backdrop material.
    pub backdrop_replacement: f32,
    /// Optional per-surface tint override applied to the glass color.
    pub tint: Option<Color>,
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
    /// Pointer-following Liquid Glass for a lifted top-level folder. It is
    /// isolated from `GridOverlay` so overlapping closed folders never enter
    /// the same SDF union, and is composited immediately before drag content.
    DragOverlay,
    Overlay,
    Modal,
    /// Context menu glass. Isolated from `Modal` so the menu's glass never
    /// smooth-unions with an open folder panel's glass — they stay visually
    /// distinct even when overlapping. Composited above `Modal`.
    ContextMenu,
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
    /// Horizontal slider track (rounded bar). Drawn by `shader_control.wgsl`
    /// kind 10.
    SliderTrack,
    /// Slider knob (filled disk). Drawn by kind 11.
    SliderKnob,
    /// Per-row reset arrow (↺). Drawn by kind 12.
    ResetIcon,
    /// Pencil glyph (context menu: edit home). Drawn by kind 13.
    Pencil,
    /// Eye-with-slash glyph (context menu: hide app). Drawn by kind 14.
    EyeOff,
    /// Folder glyph (context menu: reveal in Finder/Explorer). Drawn by kind 15.
    FolderIcon,
    /// ChatGPT logo (context menu: ChatGPT help). Drawn by kind 19 from a
    /// dedicated rasterized SVG texture (not a procedural SDF).
    ChatGptLogo,
    /// Plus glyph (context menu: larger icon). Drawn by kind 16.
    Plus,
    /// Minus glyph (context menu: smaller icon). Drawn by kind 17.
    Minus,
    /// Info glyph (context menu: app info). Drawn by kind 18.
    Info,
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
    /// Context menu procedural ink. Isolated from `Modal` so the menu's ink
    /// (icons, row backgrounds) never collides with folder panel ink.
    ContextMenu,
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
    pub clip: Option<ClipRegion>,
}

/// Draw-order lane for already-shaped glyph geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlyphLane {
    Grid,
    BottomControl,
    Settings,
    Modal,
    /// Context menu label text. Isolated from `Modal` so it never collides
    /// with folder panel glyphs.
    ContextMenu,
    /// FPS overlay text, drawn last so it sits above all modal content.
    Overlay,
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
    pub clip: Option<ClipRegion>,
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
