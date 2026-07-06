use crate::ui_model::geometry::{Rect, Size};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::Color;

#[derive(Debug, Clone, PartialEq)]
pub struct TextView {
    pub id: UiId,
    pub text: String,
    pub rect: Rect,
    pub style: TextStyle,
    pub z: i16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub color: Color,
    pub weight: TextWeight,
    pub align: TextAlign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextWeight {
    Regular,
    Medium,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextMetrics {
    pub size: Size,
    pub baseline: f32,
}

pub trait TextMeasurer {
    fn measure_line(&mut self, text: &str, style: TextStyle) -> TextMetrics;
}
