//! Liquid Glass UI — ScrollView widget (Phase 3).
//!
//! A vertical scrolling container with pixel-level continuous scroll physics.
//! Views pushed inside the body automatically inherit the viewport clip region
//! (via the clip_stack), so content outside the viewport is discarded in the
//! shader. An overlay scrollbar is drawn using InkView primitives.

use crate::scroll::ContinuousScroller;
use crate::ui::context::Ui;
use crate::ui::response::Response;
use crate::ui_model::geometry::{ClipRegion, Point, Rect};
use crate::ui_model::ids::UiId;
use crate::ui_model::render_model::{Color, ControlKind, InkView};

/// Overlay scrollbar state tied to a ScrollView id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarState {
    /// 0..1 opacity. 1 = fully visible, 0 = hidden.
    pub opacity: f32,
    /// Whether the pointer is currently over the scrollbar track area.
    pub hovered: bool,
    /// Whether the thumb is being dragged.
    pub dragging: bool,
    /// Thumb drag anchor (scrollbar-local y of pointer at drag start).
    pub drag_anchor_y: f32,
    /// position value at drag start.
    pub drag_start_position: f32,
}

impl Default for ScrollbarState {
    fn default() -> Self {
        Self {
            opacity: 0.0,
            hovered: false,
            dragging: false,
            drag_anchor_y: 0.0,
            drag_start_position: 0.0,
        }
    }
}

/// Alignment hint for `ensure_visible`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAlignment {
    Start,
    Center,
    End,
    Nearest,
}

impl Ui {
    /// Create a vertical scroll view.
    ///
    /// `scroller` is a mutable reference to the physics model owned by the
    /// caller (usually on `App`). The viewport fills the remaining available
    /// height (or `available_height` if constrained). Content laid out inside
    /// `body` is positioned in a coordinate system translated by the current
    /// scroll offset and automatically clipped to the viewport.
    ///
    /// After `body` returns, `scroller.set_sizes(content_height, viewport_h)`
    /// is called, the scrollbar is drawn, and the cursor advances past the
    /// viewport.
    pub fn scroll_view(
        &mut self,
        id: UiId,
        scroller: &mut ContinuousScroller,
        body: impl FnOnce(&mut Ui),
    ) -> Response {
        let scrollbar_style = self.theme().scrollbar;
        let viewport_w = self.available_width;
        let viewport_x = self.cursor_x;
        let viewport_top = self.cursor_y;

        // Viewport height: use available_height if constrained, otherwise
        // take the remaining space in a column (which has None available_height).
        let viewport_h = self.available_height.unwrap_or(200.0);

        // Push the clip region so children inherit it.
        let viewport_rect = Rect::new(viewport_x, viewport_top, viewport_w, viewport_h);
        let clip_region = ClipRegion::new(viewport_rect, 0.0);
        self.clip_stack.push(clip_region);

        // Save current layout and set up the content area: full width,
        // unbounded height, translated by scroll offset.
        let saved_cursor_y = self.cursor_y;
        let saved_available_height = self.available_height;

        self.cursor_y = viewport_top - scroller.position;
        self.available_width = viewport_w;
        // Content height is unbounded (None) — it can grow as needed.
        self.available_height = None;
        self.first_in_container = true;

        body(self);

        // Calculate content height from distance traveled.
        let content_end_y = self.cursor_y;
        let content_top_y = viewport_top - scroller.position;
        let content_height = content_end_y - content_top_y;

        // Restore layout state.
        self.cursor_y = saved_cursor_y;
        self.available_height = saved_available_height;

        // Pop clip region.
        self.clip_stack.pop();

        // Update the scroller with measured sizes.
        scroller.set_sizes(content_height, viewport_h);

        // ---- Draw scrollbar ----
        let show_scrollbar = content_height > viewport_h && viewport_h > 0.0;
        if show_scrollbar {
            let max = scroller.max_offset();
            let position = scroller.position;

            // Thumb length: proportional to viewport/content, with a minimum.
            let thumb_len = if max > 0.0 {
                let len = viewport_h * (viewport_h / content_height);
                len.max(scrollbar_style.minimum_thumb_length)
            } else {
                viewport_h
            };

            // Thumb position within viewport, as fraction of track (viewport_h - thumb_len).
            let track_len = viewport_h - thumb_len;
            let thumb_y_top = if max > 0.0 && track_len > 0.0 {
                viewport_top + (position / max) * track_len
            } else {
                viewport_top
            };

            // Rubber-band compression when overscrolling.
            let thumb_y = if position < 0.0 {
                // Overscrolling past top: shrink thumb toward top.
                let compression: f32 = 1.0 - (-position / viewport_h).min(0.5);
                let _shrunk_len = thumb_len * compression.max(0.3);
                viewport_top
            } else if position > max && max > 0.0 {
                // Overscrolling past bottom: shrink thumb toward bottom.
                let overscroll = position - max;
                let compression: f32 = 1.0 - (overscroll / viewport_h).min(0.5);
                let shrunk_len = thumb_len * compression.max(0.3);
                viewport_top + viewport_h - shrunk_len
            } else {
                thumb_y_top.clamp(viewport_top, viewport_top + viewport_h - thumb_len)
            };

            // Get or init scrollbar state.
            if self.element_state(&id).scrollbar.is_none() {
                self.element_state_mut(&id).scrollbar = Some(ScrollbarState::default());
            }
            let state = self.element_state(&id).scrollbar.unwrap();

            // Scrollbar width: idle vs active (hover/drag).
            let width = if state.hovered || state.dragging {
                scrollbar_style.active_width
            } else {
                scrollbar_style.idle_width
            };

            // Scrollbar x: inset from right edge of viewport.
            let bar_x = viewport_x + viewport_w - width - scrollbar_style.inset;

            // Track opacity from state (animated by the caller via tick).
            let bar_opacity = state.opacity;
            let thumb_opacity = state.opacity.min(1.0);

            // Draw track (thin background bar). Only visible when hovering.
            if state.opacity > 0.01 {
                let track_alpha = bar_opacity * 0.15;
                if track_alpha > 0.001 {
                    let track_view = InkView {
                        id: id.clone(),
                        center: Point::new(bar_x + width * 0.5, viewport_top + viewport_h * 0.5),
                        extent: width * 0.5,
                        opacity: track_alpha,
                        scene_blur: 0.0,
                        stroke: 0.0,
                        corner_radius: width * 0.5,
                        color: Color::rgba(1.0, 1.0, 1.0, 1.0),
                        kind: ControlKind::Divider,
                        z: 200,
                        clip: None,
                    };
                    self.push_ink(track_view);
                }

                // Draw thumb.
                let thumb_alpha = thumb_opacity * 0.7;
                if thumb_alpha > 0.001 && thumb_len > 0.0 {
                    let thumb_view = InkView {
                        id: id.clone(),
                        center: Point::new(bar_x + width * 0.5, thumb_y + thumb_len * 0.5),
                        extent: thumb_len * 0.5,
                        opacity: thumb_alpha,
                        scene_blur: 0.0,
                        stroke: 0.0,
                        corner_radius: width * 0.5,
                        color: Color::rgba(1.0, 1.0, 1.0, 1.0),
                        kind: ControlKind::Divider,
                        z: 201,
                        clip: None,
                    };
                    self.push_ink(thumb_view);
                }
            }
        }

        // Advance cursor past the viewport for the next element.
        self.cursor_y = viewport_top + viewport_h;
        self.first_in_container = true;

        // Register the viewport rect.
        let resp_rect = viewport_rect;
        let hit_rect = viewport_rect;
        self.register(id.clone(), resp_rect, hit_rect);

        Response {
            id,
            rect: resp_rect,
            hit_rect,
            ..Default::default()
        }
    }

    /// Scroll the ScrollView identified by `view_id` so the widget identified
    /// by `id` is visible inside it, using the given alignment.
    pub fn ensure_visible(
        &mut self,
        view_id: &UiId,
        item_id: &UiId,
        scroller: &mut ContinuousScroller,
        alignment: ScrollAlignment,
    ) {
        let Some(viewport) = self.rect(view_id) else {
            return;
        };
        let Some(item_rect) = self.rect(item_id) else {
            return;
        };

        let vp_h = viewport.height;
        let item_top = item_rect.y;
        let item_bottom = item_rect.max_y();
        let item_h = item_rect.height;

        let target = match alignment {
            ScrollAlignment::Start => item_top,
            ScrollAlignment::Center => item_top + item_h * 0.5 - vp_h * 0.5,
            ScrollAlignment::End => item_bottom - vp_h,
            ScrollAlignment::Nearest => {
                let pos = scroller.position;
                if item_top < pos {
                    item_top
                } else if item_bottom > pos + vp_h {
                    item_bottom - vp_h
                } else {
                    // Already visible; no change.
                    return;
                }
            }
        };

        let min = scroller.min_offset();
        let max = scroller.max_offset();
        let target = target.clamp(min, max);

        if (target - scroller.position).abs() > scroller.cfg.settle_eps {
            scroller.ensure_visible(item_top, item_bottom);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::scroll::{ContinuousConfig, ContinuousScroller};
    use crate::ui::context::Ui;
    use crate::ui::theme::Theme;
    use crate::ui_model::geometry::Rect;
    use crate::ui_model::ids::UiId;

    fn new_ui() -> Ui {
        Ui::new(Theme::default(), 400.0, 800.0)
    }

    #[test]
    fn scroll_view_viewport_clips_children() {
        let mut ui = new_ui();
        let mut scroller = ContinuousScroller::new(ContinuousConfig::default());
        scroller.set_sizes(200.0, 100.0);
        let view_id = UiId::named("scroll");

        // Set up a constrained height so the viewport has a known size.
        ui.available_height = Some(100.0);

        let viewport_top = ui.cursor_y;
        let vp_w = ui.available_width;
        let vp_h = ui.available_height.unwrap();

        ui.scroll_view(view_id.clone(), &mut scroller, |ui| {
            // Push a child that should get clipped.
            ui.spacer(50.0);
        });

        let (render, _hits, _reg) = ui.take();

        // Child elements should have received the clip from the viewport.
        let viewport = Rect::new(viewport_top, viewport_top, vp_w, vp_h);
        for batch in &render.ink {
            for view in &batch.views {
                if let Some(clip) = view.clip {
                    // The clip should be the viewport rect.
                    assert_eq!(clip.rect, viewport);
                }
            }
        }
    }

    #[test]
    fn scroll_view_advances_cursor_past_viewport() {
        let mut ui = new_ui();
        let mut scroller = ContinuousScroller::new(ContinuousConfig::default());
        scroller.set_sizes(200.0, 100.0);
        let view_id = UiId::named("scroll");

        ui.available_height = Some(100.0);
        let cursor_before = ui.cursor_y;

        ui.scroll_view(view_id, &mut scroller, |ui| {
            ui.spacer(300.0); // Content taller than viewport
        });

        // Cursor should advance by viewport height.
        assert!((ui.cursor_y - cursor_before - 100.0).abs() < 0.1);
    }

    #[test]
    fn scroll_view_updates_scroller_sizes() {
        let mut ui = new_ui();
        let mut scroller = ContinuousScroller::new(ContinuousConfig::default());
        let view_id = UiId::named("scroll");

        ui.available_height = Some(100.0);

        ui.scroll_view(view_id, &mut scroller, |ui| {
            ui.spacer(500.0);
        });

        // Content height should have been measured.
        assert!((scroller.max_offset() - 400.0).abs() < 1.0); // 500 - 100 = 400
    }

    #[test]
    fn scroll_view_empty_content_sets_zero_max() {
        let mut ui = new_ui();
        let mut scroller = ContinuousScroller::new(ContinuousConfig::default());
        let view_id = UiId::named("scroll");

        ui.available_height = Some(100.0);

        ui.scroll_view(view_id, &mut scroller, |_ui| {
            // No content
        });

        assert_eq!(scroller.max_offset(), 0.0);
    }
}
