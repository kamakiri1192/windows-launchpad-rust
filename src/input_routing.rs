//! Pure input-intent routing shared by the launcher shell and platform adapters.
//!
//! This module deliberately contains no `winit`, Win32, AppKit, or renderer
//! types. The application supplies a point classification derived from the
//! same layout geometry used for drawing, and the router resolves each pointer
//! gesture to exactly one owner.

use std::sync::{Arc, RwLock};

/// Click/drag intent threshold in physical pixels.
pub const CLICK_SLOP_PHYS: f32 = 8.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PhysicalPoint {
    pub x: f32,
    pub y: f32,
}

impl PhysicalPoint {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn distance_squared(self, other: Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputRegion {
    LaunchpadOwned,
    OutsideTransparent,
    ModalDismiss,
}

impl InputRegion {
    pub const fn is_launchpad_owned(self) -> bool {
        !matches!(self, Self::OutsideTransparent)
    }
}

/// Classify a point from layout-owned hit results.
///
/// `viewport_owned` is used for modal, editing, icon-drag, page-drag, and
/// transition states. `modal_dismiss` is kept distinct so the app can close a
/// modal without ever confusing that action with passthrough.
pub const fn classify_region(
    viewport_owned: bool,
    modal_dismiss: bool,
    page_frame_contains: bool,
    bottom_control_contains: bool,
) -> InputRegion {
    if modal_dismiss {
        InputRegion::ModalDismiss
    } else if viewport_owned || page_frame_contains || bottom_control_contains {
        InputRegion::LaunchpadOwned
    } else {
        InputRegion::OutsideTransparent
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RouterState {
    Idle,
    LaunchpadOwned { button: PointerButton },
    LeftPending { press: PhysicalPoint },
    PageDrag { press: PhysicalPoint },
    RightPending { press: PhysicalPoint },
    RightCancelled,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RouterAction {
    None,
    LaunchpadOwns,
    BeginPending {
        button: PointerButton,
        press: PhysicalPoint,
    },
    BeginPageDrag {
        press: PhysicalPoint,
        current: PhysicalPoint,
    },
    ContinuePageDrag {
        current: PhysicalPoint,
    },
    EndPageDrag,
    /// Defensive release path for platforms that coalesce away the threshold
    /// crossing move. The app must start at `press`, catch up to `current`,
    /// then end the drag without producing a click.
    FinishPageDrag {
        press: PhysicalPoint,
        current: PhysicalPoint,
    },
    DeliverClick {
        button: PointerButton,
        point: PhysicalPoint,
    },
    CancelRightGesture,
    ForwardVerticalScroll,
    Consume,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputRouter {
    state: RouterState,
    click_slop_phys: f32,
}

impl Default for InputRouter {
    fn default() -> Self {
        Self::new(CLICK_SLOP_PHYS)
    }
}

impl InputRouter {
    pub const fn new(click_slop_phys: f32) -> Self {
        Self {
            state: RouterState::Idle,
            click_slop_phys,
        }
    }

    pub const fn state(&self) -> RouterState {
        self.state
    }

    pub const fn is_idle(&self) -> bool {
        matches!(self.state, RouterState::Idle)
    }

    pub fn reset(&mut self) {
        self.state = RouterState::Idle;
    }

    pub fn press(
        &mut self,
        button: PointerButton,
        point: PhysicalPoint,
        region: InputRegion,
    ) -> RouterAction {
        if !self.is_idle() {
            return RouterAction::Consume;
        }
        if region.is_launchpad_owned() {
            self.state = RouterState::LaunchpadOwned { button };
            return RouterAction::LaunchpadOwns;
        }
        self.state = match button {
            PointerButton::Left => RouterState::LeftPending { press: point },
            PointerButton::Right => RouterState::RightPending { press: point },
        };
        RouterAction::BeginPending {
            button,
            press: point,
        }
    }

    pub fn pointer_moved(&mut self, point: PhysicalPoint) -> RouterAction {
        match self.state {
            RouterState::LeftPending { press } if self.past_slop(press, point) => {
                self.state = RouterState::PageDrag { press };
                RouterAction::BeginPageDrag {
                    press,
                    current: point,
                }
            }
            RouterState::PageDrag { .. } => RouterAction::ContinuePageDrag { current: point },
            RouterState::RightPending { press } if self.past_slop(press, point) => {
                self.state = RouterState::RightCancelled;
                RouterAction::CancelRightGesture
            }
            RouterState::LaunchpadOwned { .. } => RouterAction::LaunchpadOwns,
            RouterState::LeftPending { .. }
            | RouterState::RightPending { .. }
            | RouterState::RightCancelled
            | RouterState::Idle => RouterAction::None,
        }
    }

    pub fn release(&mut self, button: PointerButton, point: PhysicalPoint) -> RouterAction {
        let state = self.state;
        let action = match state {
            RouterState::LeftPending { press } if button == PointerButton::Left => {
                if self.past_slop(press, point) {
                    RouterAction::FinishPageDrag {
                        press,
                        current: point,
                    }
                } else {
                    RouterAction::DeliverClick { button, point }
                }
            }
            RouterState::PageDrag { .. } if button == PointerButton::Left => {
                RouterAction::EndPageDrag
            }
            RouterState::RightPending { press } if button == PointerButton::Right => {
                if self.past_slop(press, point) {
                    RouterAction::CancelRightGesture
                } else {
                    RouterAction::DeliverClick { button, point }
                }
            }
            RouterState::RightCancelled if button == PointerButton::Right => {
                RouterAction::CancelRightGesture
            }
            RouterState::LaunchpadOwned {
                button: owned_button,
            } if owned_button == button => RouterAction::LaunchpadOwns,
            _ => return RouterAction::Consume,
        };
        self.state = RouterState::Idle;
        action
    }

    pub const fn vertical_scroll(&self, region: InputRegion) -> RouterAction {
        if !matches!(self.state, RouterState::Idle) || region.is_launchpad_owned() {
            RouterAction::Consume
        } else {
            RouterAction::ForwardVerticalScroll
        }
    }

    fn past_slop(&self, press: PhysicalPoint, point: PhysicalPoint) -> bool {
        press.distance_squared(point) > self.click_slop_phys * self.click_slop_phys
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryResult {
    Delivered,
    Queued,
    NoTarget,
    PermissionDenied,
    Unsupported,
    Failed { os_error: i64 },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InjectionTag {
    pub source_process: u32,
    pub generation: u64,
}

/// Immutable state consumed by native input callbacks. The app replaces this
/// value after state transitions; callbacks never retain references into
/// mutable `App`, window, layout, or renderer state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputRoutingSnapshot {
    pub visible: bool,
    pub region: InputRegion,
    pub router_state: RouterState,
    pub generation: u64,
}

impl Default for InputRoutingSnapshot {
    fn default() -> Self {
        Self {
            visible: false,
            region: InputRegion::LaunchpadOwned,
            router_state: RouterState::Idle,
            generation: 0,
        }
    }
}

impl InputRoutingSnapshot {
    pub const fn forwards_vertical_scroll(self) -> bool {
        self.visible
            && matches!(self.region, InputRegion::OutsideTransparent)
            && matches!(self.router_state, RouterState::Idle)
    }
}

#[derive(Debug, Clone, Default)]
pub struct InputRoutingPublisher(Arc<RwLock<InputRoutingSnapshot>>);

impl InputRoutingPublisher {
    pub fn publish(&self, snapshot: InputRoutingSnapshot) {
        if let Ok(mut current) = self.0.write() {
            *current = snapshot;
        }
    }

    pub fn snapshot(&self) -> InputRoutingSnapshot {
        self.0.read().map(|value| *value).unwrap_or_default()
    }
}

impl InjectionTag {
    pub const fn is_self_delivery(self, process: u32, current_generation: u64) -> bool {
        self.source_process == process
            && self.generation != 0
            && self.generation <= current_generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollPhase {
    None,
    MayBegin,
    Began,
    Changed,
    Ended,
    Cancelled,
    MomentumBegan,
    MomentumChanged,
    MomentumEnded,
}

/// Locks one native scroll sequence to one OS target. Target resolution is
/// supplied by the platform adapter and therefore remains outside the router.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrollTargetLock<T> {
    target: Option<T>,
}

impl<T: Copy> ScrollTargetLock<T> {
    pub const fn target(&self) -> Option<T> {
        self.target
    }

    pub fn target_for(
        &mut self,
        phase: ScrollPhase,
        resolve: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let target = match phase {
            ScrollPhase::None => resolve(),
            ScrollPhase::MayBegin | ScrollPhase::Began | ScrollPhase::MomentumBegan => {
                if self.target.is_none() {
                    self.target = resolve();
                }
                self.target
            }
            ScrollPhase::Changed
            | ScrollPhase::Ended
            | ScrollPhase::Cancelled
            | ScrollPhase::MomentumChanged
            | ScrollPhase::MomentumEnded => self.target.or_else(resolve),
        };
        if matches!(phase, ScrollPhase::Cancelled | ScrollPhase::MomentumEnded)
            || (phase == ScrollPhase::Ended && self.target.is_none())
        {
            self.target = None;
        }
        target
    }

    pub fn finish_gesture_without_momentum(&mut self) {
        self.target = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTSIDE: InputRegion = InputRegion::OutsideTransparent;
    const OWNED: InputRegion = InputRegion::LaunchpadOwned;

    #[test]
    fn region_classifier_uses_shared_geometry_and_exclusive_states() {
        assert_eq!(
            classify_region(false, false, false, false),
            InputRegion::OutsideTransparent
        );
        assert_eq!(
            classify_region(false, false, true, false),
            InputRegion::LaunchpadOwned
        );
        assert_eq!(
            classify_region(false, false, false, true),
            InputRegion::LaunchpadOwned
        );
        assert_eq!(
            classify_region(true, false, false, false),
            InputRegion::LaunchpadOwned
        );
        assert_eq!(
            classify_region(true, true, false, false),
            InputRegion::ModalDismiss
        );
    }

    #[test]
    fn left_click_waits_for_release_and_delivers_once() {
        let mut router = InputRouter::default();
        let press = PhysicalPoint::new(10.0, 20.0);
        assert!(matches!(
            router.press(PointerButton::Left, press, OUTSIDE),
            RouterAction::BeginPending { .. }
        ));
        assert!(matches!(router.state(), RouterState::LeftPending { .. }));
        assert_eq!(
            router.release(PointerButton::Left, PhysicalPoint::new(14.0, 22.0)),
            RouterAction::DeliverClick {
                button: PointerButton::Left,
                point: PhysicalPoint::new(14.0, 22.0),
            }
        );
        assert!(router.is_idle());
        assert_eq!(
            router.release(PointerButton::Left, press),
            RouterAction::Consume
        );
    }

    #[test]
    fn left_drag_promotes_after_eight_physical_pixels_and_catches_up() {
        let mut router = InputRouter::default();
        let press = PhysicalPoint::new(-100.0, 50.0);
        router.press(PointerButton::Left, press, OUTSIDE);
        assert_eq!(
            router.pointer_moved(PhysicalPoint::new(-92.0, 50.0)),
            RouterAction::None
        );
        let current = PhysicalPoint::new(-91.0, 50.0);
        assert_eq!(
            router.pointer_moved(current),
            RouterAction::BeginPageDrag { press, current }
        );
        assert_eq!(
            router.pointer_moved(PhysicalPoint::new(-80.0, 51.0)),
            RouterAction::ContinuePageDrag {
                current: PhysicalPoint::new(-80.0, 51.0)
            }
        );
        assert_eq!(
            router.release(PointerButton::Left, PhysicalPoint::new(-80.0, 51.0)),
            RouterAction::EndPageDrag
        );
    }

    #[test]
    fn diagonal_distance_uses_combined_physical_displacement() {
        let mut router = InputRouter::default();
        let press = PhysicalPoint::new(0.0, 0.0);
        router.press(PointerButton::Left, press, OUTSIDE);
        let current = PhysicalPoint::new(6.0, 6.0);
        assert_eq!(
            router.pointer_moved(current),
            RouterAction::BeginPageDrag { press, current }
        );
    }

    #[test]
    fn coalesced_left_release_cannot_turn_a_drag_into_a_click() {
        let mut router = InputRouter::default();
        let press = PhysicalPoint::new(0.0, 0.0);
        let current = PhysicalPoint::new(20.0, 0.0);
        router.press(PointerButton::Left, press, OUTSIDE);
        assert_eq!(
            router.release(PointerButton::Left, current),
            RouterAction::FinishPageDrag { press, current }
        );
    }

    #[test]
    fn right_click_and_right_drag_are_mutually_exclusive() {
        let mut router = InputRouter::default();
        let press = PhysicalPoint::new(100.0, 100.0);
        router.press(PointerButton::Right, press, OUTSIDE);
        assert!(matches!(
            router.release(PointerButton::Right, PhysicalPoint::new(103.0, 103.0)),
            RouterAction::DeliverClick {
                button: PointerButton::Right,
                ..
            }
        ));

        router.press(PointerButton::Right, press, OUTSIDE);
        assert_eq!(
            router.pointer_moved(PhysicalPoint::new(109.0, 100.0)),
            RouterAction::CancelRightGesture
        );
        assert_eq!(
            router.release(PointerButton::Right, PhysicalPoint::new(109.0, 100.0)),
            RouterAction::CancelRightGesture
        );
        assert!(router.is_idle());
    }

    #[test]
    fn wheel_only_forwards_while_idle_and_outside() {
        let mut router = InputRouter::default();
        assert_eq!(
            router.vertical_scroll(OUTSIDE),
            RouterAction::ForwardVerticalScroll
        );
        assert_eq!(router.vertical_scroll(OWNED), RouterAction::Consume);
        router.press(PointerButton::Left, PhysicalPoint::new(1.0, 1.0), OUTSIDE);
        assert_eq!(router.vertical_scroll(OUTSIDE), RouterAction::Consume);
    }

    #[test]
    fn gesture_ownership_does_not_change_when_pointer_crosses_region() {
        let mut router = InputRouter::default();
        let press = PhysicalPoint::new(0.0, 0.0);
        router.press(PointerButton::Left, press, OUTSIDE);
        router.pointer_moved(PhysicalPoint::new(20.0, 0.0));
        assert!(matches!(router.state(), RouterState::PageDrag { .. }));
        assert_eq!(
            router.vertical_scroll(OWNED),
            RouterAction::Consume,
            "page drag remains launcher-owned"
        );
    }

    #[test]
    fn self_injection_tag_is_rejected_by_generation() {
        let tag = InjectionTag {
            source_process: 42,
            generation: 7,
        };
        assert!(tag.is_self_delivery(42, 7));
        assert!(tag.is_self_delivery(42, 8));
        assert!(!tag.is_self_delivery(41, 8));
        assert!(!InjectionTag::default().is_self_delivery(42, 8));
    }

    #[test]
    fn published_snapshot_is_owned_and_wheel_gated() {
        let publisher = InputRoutingPublisher::default();
        let snapshot = InputRoutingSnapshot {
            visible: true,
            region: OUTSIDE,
            router_state: RouterState::Idle,
            generation: 9,
        };
        publisher.publish(snapshot);
        assert_eq!(publisher.snapshot(), snapshot);
        assert!(publisher.snapshot().forwards_vertical_scroll());

        publisher.publish(InputRoutingSnapshot {
            router_state: RouterState::RightPending {
                press: PhysicalPoint::new(1.0, 2.0),
            },
            ..snapshot
        });
        assert!(!publisher.snapshot().forwards_vertical_scroll());
    }

    #[test]
    fn scroll_target_stays_locked_through_momentum() {
        let mut lock = ScrollTargetLock::default();
        assert_eq!(
            lock.target_for(ScrollPhase::Began, || Some(11_u64)),
            Some(11)
        );
        assert_eq!(lock.target_for(ScrollPhase::Changed, || Some(22)), Some(11));
        assert_eq!(lock.target_for(ScrollPhase::Ended, || Some(22)), Some(11));
        assert_eq!(
            lock.target_for(ScrollPhase::MomentumBegan, || Some(22)),
            Some(11)
        );
        assert_eq!(
            lock.target_for(ScrollPhase::MomentumEnded, || Some(22)),
            Some(11)
        );
        assert_eq!(lock.target(), None);
    }
}
