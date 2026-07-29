//! Context menu feature state.
//!
//! Owns the open/close animation for the app-icon right-click menu. The menu
//! morphs from a small seed state (40×40, corner radius 130) anchored at the
//! right-clicked app icon into the full panel (width 320, corner radius 70),
//! mirroring the per-property spring/easing channels of the `liquid-dom` menu
//! demo. This module is pure: it never touches windows, GPU resources, or
//! persistence. Side effects are requested by the app shell.
//!
//! Unlike [`crate::features::folders::FolderMotion`] (an omega/zeta spring
//! driving a single progress value), each animated property here owns an
//! independent channel with its own [`crate::spring_anim::Transition`], so
//! position can spring fast while corner radius eases slowly.

use crate::domain::app_id::AppId;
use crate::spring_anim::{self, Channel, Ease, Transition};

// Re-export the item enum from the pure layout layer so feature/app/render
// code can name it via `features::context_menu::ContextMenuItem` without the
// layout crate depending back on the binary-only features crate.
#[allow(unused_imports)]
pub use crate::layout::context_menu::ContextMenuItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuPhase {
    Closed,
    Opening,
    Open,
    Closing,
}

impl Default for ContextMenuPhase {
    fn default() -> Self {
        Self::Closed
    }
}

/// Animated property indices into [`ContextMenuState::channels`].
///
/// These are kept as plain indices rather than an enum-of-channels so the
/// hot per-frame tick can stay branch-light; the layout/render layer reads the
/// resolved values back through the accessor methods on [`ContextMenuState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
enum Prop {
    PosX = 0,
    PosY,
    Width,
    Height,
    Radius,
    ContentScale,
    ContentOpacity,
    ContentBlur,
    Activation,
}

const PROP_COUNT: usize = 9;

/// Reference demo constants (logical px at 1× DPI). The layout layer scales
/// these by the DPI factor into physical pixels before handing them to the
/// renderer.
pub const SEED_SIZE: f32 = 40.0;
pub const SEED_RADIUS: f32 = 130.0;
pub const MENU_WIDTH: f32 = 260.0;
pub const OPEN_RADIUS: f32 = 70.0;
pub const SEED_CONTENT_SCALE: f32 = 2.0;
pub const SEED_CONTENT_BLUR: f32 = 8.0;

// --- Transition presets, transcribed from MenuDemo.tsx ----------------------

/// `MENU_OPEN_POSITION_TRANSITION`: spring({ stiffness: 144, damping: 14, velocity: 2400 }).
const T_OPEN_POS: Transition = Transition::Spring {
    stiffness: 144.0,
    damping: 14.0,
    mass: 1.0,
    velocity: 2400.0,
};
/// `MENU_CLOSE_POSITION_TRANSITION`: spring({ stiffness: 130, damping: 18 }).
const T_CLOSE_POS: Transition = Transition::Spring {
    stiffness: 130.0,
    damping: 18.0,
    mass: 1.0,
    velocity: 0.0,
};
/// `MENU_OPEN_SIZE_TRANSITION`: easing({ duration: 0.3, ease: bezier(0.8, 0.3, 0.5, 0.8) }).
const T_OPEN_SIZE: Transition = Transition::Easing {
    duration: 0.3,
    ease: Ease::MENU_SIZE,
};
/// `MENU_CLOSE_SIZE_TRANSITION`: easing({ duration: 0.25, ease: easeOut }).
const T_CLOSE_SIZE: Transition = Transition::Easing {
    duration: 0.25,
    ease: Ease::EaseOut,
};
/// `MENU_OPEN/CLOSE_RADIUS_TRANSITION`: easing({ duration: 0.7, ease: easeOut }).
const T_RADIUS: Transition = Transition::Easing {
    duration: 0.7,
    ease: Ease::EaseOut,
};
/// `CONTENT_BLUR_TRANSITION` (also used for content scale).
const T_CONTENT_SHAPE: Transition = Transition::Easing {
    duration: 0.3,
    ease: Ease::EaseOut,
};
/// `CONTENT_TRANSITION`: spring({ stiffness: 137, damping: 20 }) — content opacity.
const T_CONTENT_OPACITY: Transition = Transition::Spring {
    stiffness: 137.0,
    damping: 20.0,
    mass: 1.0,
    velocity: 0.0,
};
/// `CONTENT_OPTICS_TRANSITION`: the easeIn decay applied to the activation bump.
const T_OPTICS: Transition = Transition::Easing {
    duration: 0.3,
    ease: Ease::EaseIn,
};

#[derive(Debug, Clone)]
pub struct ContextMenuState {
    pub active_app: Option<AppId>,
    pub phase: ContextMenuPhase,
    /// Anchor point (physical px) the seed springs from — typically the center
    /// of the right-clicked app icon.
    pub anchor: (f32, f32),
    channels: [Channel; PROP_COUNT],
    /// Per-channel elapsed time (seconds). Parallel array to `channels` so the
    /// tick loop can borrow both mutably without an enum-of-structs.
    elapsed: [f32; PROP_COUNT],
    /// Per-channel active transition. Retargeted on open/close.
    transitions: [Transition; PROP_COUNT],
}

impl Default for ContextMenuState {
    fn default() -> Self {
        Self {
            active_app: None,
            phase: ContextMenuPhase::Closed,
            anchor: (0.0, 0.0),
            channels: [Channel::rest(0.0); PROP_COUNT],
            elapsed: [0.0; PROP_COUNT],
            transitions: [Transition::Snap; PROP_COUNT],
        }
    }
}

impl ContextMenuState {
    /// Begin opening the menu for `app_id`, anchored at `(x, y)` (physical px).
    /// `target_rect` is the fully-open panel rectangle in physical px; the seed
    /// springs from the anchor toward it.
    pub fn open(&mut self, app_id: AppId, x: f32, y: f32, target_rect: MenuTarget) {
        self.active_app = Some(app_id);
        self.anchor = (x, y);
        self.phase = ContextMenuPhase::Opening;

        // Seed state: a 40×40 seed centered on the anchor, with content scaled
        // up and blurred (collapsed). Activation snaps high for the optics bump.
        let seed_x = x - SEED_SIZE * 0.5;
        let seed_y = y - SEED_SIZE * 0.5;
        let seed = [
            Channel::rest(seed_x),
            Channel::rest(seed_y),
            Channel::rest(SEED_SIZE),
            Channel::rest(SEED_SIZE),
            Channel::rest(SEED_RADIUS),
            Channel::rest(SEED_CONTENT_SCALE),
            Channel::rest(0.0),
            Channel::rest(SEED_CONTENT_BLUR),
            Channel::rest(1.0),
        ];
        self.channels = seed;
        self.elapsed = [0.0; PROP_COUNT];

        // Retarget every channel toward its open target with the open preset.
        self.transitions = [
            T_OPEN_POS,
            T_OPEN_POS,
            T_OPEN_SIZE,
            T_OPEN_SIZE,
            T_RADIUS,
            T_CONTENT_SHAPE,
            T_CONTENT_OPACITY,
            T_CONTENT_SHAPE,
            // Activation: snap high immediately, then decay on the next tick.
            Transition::Snap,
        ];
        let targets = [
            target_rect.x,
            target_rect.y,
            target_rect.width,
            target_rect.height,
            OPEN_RADIUS,
            1.0,
            1.0,
            0.0,
            1.0,
        ];
        for (i, ch) in self.channels.iter_mut().enumerate() {
            spring_anim::retarget(ch, targets[i], self.transitions[i], &mut self.elapsed[i]);
        }
    }

    /// Begin closing the menu. The channels retarget back toward the seed state
    /// at the anchor.
    pub fn close(&mut self) {
        if self.active_app.is_none() {
            return;
        }
        self.phase = ContextMenuPhase::Closing;

        let seed_x = self.anchor.0 - SEED_SIZE * 0.5;
        let seed_y = self.anchor.1 - SEED_SIZE * 0.5;
        self.transitions = [
            T_CLOSE_POS,
            T_CLOSE_POS,
            T_CLOSE_SIZE,
            T_CLOSE_SIZE,
            T_RADIUS,
            T_CONTENT_SHAPE,
            T_CONTENT_OPACITY,
            T_CONTENT_SHAPE,
            Transition::Easing {
                duration: 0.15,
                ease: Ease::EaseIn,
            },
        ];
        let targets = [
            seed_x,
            seed_y,
            SEED_SIZE,
            SEED_SIZE,
            SEED_RADIUS,
            SEED_CONTENT_SCALE,
            0.0,
            SEED_CONTENT_BLUR,
            0.0,
        ];
        for (i, ch) in self.channels.iter_mut().enumerate() {
            spring_anim::retarget(ch, targets[i], self.transitions[i], &mut self.elapsed[i]);
        }
    }

    /// Advance all channels by `dt` seconds. Returns `true` while any channel
    /// is still animating, mirroring [`FolderFeatureState::tick`].
    pub fn tick(&mut self, dt: f32) -> bool {
        if self.active_app.is_none() {
            return false;
        }
        let dt = dt.max(0.0);

        // After the activation bump snaps high on open, retarget it to decay
        // back toward the resting activation level on the first real tick.
        if self.phase == ContextMenuPhase::Opening {
            let bump_idx = Prop::Activation as usize;
            if self.transitions[bump_idx] == Transition::Snap {
                self.transitions[bump_idx] = T_OPTICS;
                self.elapsed[bump_idx] = 0.0;
                // Resting activation keeps a touch of glass emphasis while open.
                spring_anim::retarget(
                    &mut self.channels[bump_idx],
                    0.35,
                    T_OPTICS,
                    &mut self.elapsed[bump_idx],
                );
            }
        }

        let mut animating = false;
        for i in 0..PROP_COUNT {
            let still_going = spring_anim::step(
                &mut self.channels[i],
                self.transitions[i],
                &mut self.elapsed[i],
                dt,
            );
            animating |= still_going;
        }

        if !animating {
            match self.phase {
                ContextMenuPhase::Opening => self.phase = ContextMenuPhase::Open,
                ContextMenuPhase::Closing => {
                    self.phase = ContextMenuPhase::Closed;
                    self.active_app = None;
                }
                _ => {}
            }
        }
        animating
    }

    pub fn is_active(&self) -> bool {
        self.active_app.is_some()
    }

    pub fn is_open(&self) -> bool {
        self.phase == ContextMenuPhase::Open
    }

    // --- Resolved property accessors (physical px) ----------------------------

    pub fn pos_x(&self) -> f32 {
        self.channels[Prop::PosX as usize].current
    }
    pub fn pos_y(&self) -> f32 {
        self.channels[Prop::PosY as usize].current
    }
    pub fn width(&self) -> f32 {
        self.channels[Prop::Width as usize].current
    }
    pub fn height(&self) -> f32 {
        self.channels[Prop::Height as usize].current
    }
    pub fn radius(&self) -> f32 {
        self.channels[Prop::Radius as usize].current
    }
    pub fn content_scale(&self) -> f32 {
        self.channels[Prop::ContentScale as usize].current
    }
    pub fn content_opacity(&self) -> f32 {
        self.channels[Prop::ContentOpacity as usize].current
    }
    pub fn content_blur(&self) -> f32 {
        self.channels[Prop::ContentBlur as usize].current
    }
    pub fn activation(&self) -> f32 {
        self.channels[Prop::Activation as usize].current
    }
}

/// Resolved fully-open panel geometry in physical px. The app shell computes
/// this from the viewport, DPI, and item count before calling
/// [`ContextMenuState::open`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuTarget {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_at(x: f32, y: f32) -> MenuTarget {
        MenuTarget {
            x,
            y,
            width: MENU_WIDTH,
            height: 6.0 * 40.0 + 40.0,
        }
    }

    #[test]
    fn open_then_close_returns_to_closed() {
        let mut state = ContextMenuState::default();
        state.open(
            AppId::from_normalized("calc"),
            100.0,
            100.0,
            target_at(100.0, 100.0),
        );
        assert_eq!(state.phase, ContextMenuPhase::Opening);
        assert!(state.is_active());

        // Tick until open settles.
        let mut animating = true;
        let mut guard = 0;
        while animating && guard < 5_000 {
            animating = state.tick(1.0 / 60.0);
            guard += 1;
        }
        assert!(!animating);
        assert_eq!(state.phase, ContextMenuPhase::Open);
        assert!((state.width() - MENU_WIDTH).abs() < 0.5);
        assert!((state.content_opacity() - 1.0).abs() < 0.05);

        state.close();
        assert_eq!(state.phase, ContextMenuPhase::Closing);
        let mut animating = true;
        let mut guard = 0;
        while animating && guard < 5_000 {
            animating = state.tick(1.0 / 60.0);
            guard += 1;
        }
        assert!(!animating);
        assert_eq!(state.phase, ContextMenuPhase::Closed);
        assert!(!state.is_active());
        assert!(state.active_app.is_none());
    }

    #[test]
    fn radius_transitions_from_seed_to_open_value() {
        let mut state = ContextMenuState::default();
        state.open(
            AppId::from_normalized("calc"),
            0.0,
            0.0,
            target_at(0.0, 0.0),
        );
        // Seed starts at SEED_RADIUS.
        assert!((state.radius() - SEED_RADIUS).abs() < 0.5);
        // After enough ticks it should approach OPEN_RADIUS.
        for _ in 0..300 {
            state.tick(1.0 / 60.0);
        }
        assert!(
            (state.radius() - OPEN_RADIUS).abs() < 1.0,
            "radius settled at {}",
            state.radius()
        );
    }

    #[test]
    fn close_with_no_active_menu_is_noop() {
        let mut state = ContextMenuState::default();
        state.close();
        assert_eq!(state.phase, ContextMenuPhase::Closed);
        assert!(!state.tick(1.0 / 60.0));
    }
}
