//! Pure input-intent routing shared by the launcher shell and platform adapters.
//!
//! This module deliberately contains no `winit`, Win32, AppKit, or renderer
//! types. The application supplies a point classification derived from the
//! same layout geometry used for drawing, and the router resolves each pointer
//! gesture to exactly one owner.

use std::collections::{BTreeMap, BTreeSet};
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
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InputOwnedGeometry {
    pub viewport_owned: bool,
    pub page_frame: Option<InputRoundedRect>,
    pub bottom_capsule: Option<InputCapsule>,
    pub edit_gear: Option<InputCircle>,
}

impl InputOwnedGeometry {
    pub fn contains(self, point: PhysicalPoint) -> bool {
        self.viewport_owned
            || self.page_frame.is_some_and(|shape| shape.contains(point))
            || self
                .bottom_capsule
                .is_some_and(|shape| shape.contains(point))
            || self.edit_gear.is_some_and(|shape| shape.contains(point))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputRoundedRect {
    pub center: PhysicalPoint,
    pub half_width: f32,
    pub half_height: f32,
    pub radius: f32,
}

impl InputRoundedRect {
    pub fn contains(self, point: PhysicalPoint) -> bool {
        let radius = self
            .radius
            .max(0.0)
            .min(self.half_width)
            .min(self.half_height);
        let qx = (point.x - self.center.x).abs() - self.half_width + radius;
        let qy = (point.y - self.center.y).abs() - self.half_height + radius;
        let outside_x = qx.max(0.0);
        let outside_y = qy.max(0.0);
        let outside = (outside_x * outside_x + outside_y * outside_y).sqrt();
        let inside = qx.max(qy).min(0.0);
        outside + inside - radius <= 0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputCapsule {
    pub center: PhysicalPoint,
    pub half_width: f32,
    pub half_height: f32,
}

impl InputCapsule {
    pub fn contains(self, point: PhysicalPoint) -> bool {
        let half_height = self.half_height.max(0.0);
        let inner_half_width = (self.half_width - half_height).max(0.0);
        let dx = (point.x - self.center.x).abs();
        let dy = (point.y - self.center.y).abs();
        (dx <= inner_half_width && dy <= half_height) || {
            let end_dx = dx - inner_half_width;
            end_dx * end_dx + dy * dy <= half_height * half_height
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputCircle {
    pub center: PhysicalPoint,
    pub radius: f32,
}

impl InputCircle {
    pub fn contains(self, point: PhysicalPoint) -> bool {
        let dx = point.x - self.center.x;
        let dy = point.y - self.center.y;
        dx * dx + dy * dy <= self.radius.max(0.0).powi(2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputRoutingSnapshot {
    pub visible: bool,
    pub region: InputRegion,
    pub owned_geometry: InputOwnedGeometry,
    pub router_state: RouterState,
    pub generation: u64,
}

impl Default for InputRoutingSnapshot {
    fn default() -> Self {
        Self {
            visible: false,
            region: InputRegion::LaunchpadOwned,
            owned_geometry: InputOwnedGeometry::default(),
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

    pub fn region_at(self, point: PhysicalPoint) -> InputRegion {
        if self.owned_geometry.contains(point) {
            InputRegion::LaunchpadOwned
        } else {
            InputRegion::OutsideTransparent
        }
    }

    pub fn forwards_vertical_scroll_at(self, point: PhysicalPoint) -> bool {
        self.visible
            && matches!(self.region_at(point), InputRegion::OutsideTransparent)
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

/// Stable identifier shared by one physical contact and its following native
/// momentum sequence.
pub type GestureId = u64;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NativeScrollPhase {
    #[default]
    None,
    Began,
    Changed,
    Ended,
    Cancelled,
}

impl NativeScrollPhase {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Ended | Self::Cancelled)
    }
}

impl RawScrollEvent {
    /// Reject malformed platform packets before they can contaminate gesture
    /// ownership, settings scroll state, or a pager's physics state.
    pub fn is_valid(self) -> bool {
        self.delta_physical_px.0.is_finite()
            && self.delta_physical_px.1.is_finite()
            && self.scale_factor.is_finite()
            && self.scale_factor > 0.0
            && !(self.contact_phase != NativeScrollPhase::None
                && self.momentum_phase != NativeScrollPhase::None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollSource {
    Precise,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollPhaseCapability {
    /// The platform supplied physical-contact and momentum phases separately.
    Separate,
    /// winit supplied one phase after collapsing AppKit's two phase fields.
    CollapsedFallback,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawScrollEvent {
    pub timestamp_us: u64,
    pub delta_physical_px: (f32, f32),
    pub source: ScrollSource,
    pub contact_phase: NativeScrollPhase,
    pub momentum_phase: NativeScrollPhase,
    pub sequence_complete: bool,
    /// Native metadata indicating that the OS inverted this event for the
    /// user's natural-scroll preference. AppKit and winit macOS horizontal
    /// deltas already include that preference and must not be inverted again.
    pub direction_inverted_from_device: bool,
    pub scale_factor: f32,
    pub phase_capability: ScrollPhaseCapability,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollSample {
    pub gesture_id: GestureId,
    pub timestamp_us: u64,
    pub raw_dx: f32,
    pub raw_dy: f32,
    /// Canonical display-space delta. Positive x moves the visible grid right.
    pub canonical_dx: f32,
    pub canonical_dy: f32,
    pub source: ScrollSource,
    pub contact_phase: NativeScrollPhase,
    pub momentum_phase: NativeScrollPhase,
    pub sequence_complete: bool,
    pub scale_factor: f32,
    pub direction_inverted_from_device: bool,
    pub phase_capability: ScrollPhaseCapability,
}

impl ScrollSample {
    pub fn is_valid(self) -> bool {
        self.raw_dx.is_finite()
            && self.raw_dy.is_finite()
            && self.canonical_dx.is_finite()
            && self.canonical_dy.is_finite()
            && self.scale_factor.is_finite()
            && self.scale_factor > 0.0
            && (self.source == ScrollSource::Line || self.gesture_id != 0)
            && !(self.contact_phase != NativeScrollPhase::None
                && self.momentum_phase != NativeScrollPhase::None)
    }

    fn safe_cancel(gesture_id: GestureId, timestamp_us: u64) -> Self {
        Self {
            gesture_id,
            timestamp_us,
            raw_dx: 0.0,
            raw_dy: 0.0,
            canonical_dx: 0.0,
            canonical_dy: 0.0,
            source: ScrollSource::Precise,
            contact_phase: NativeScrollPhase::Cancelled,
            momentum_phase: NativeScrollPhase::None,
            sequence_complete: false,
            scale_factor: 1.0,
            direction_inverted_from_device: false,
            phase_capability: ScrollPhaseCapability::Separate,
        }
    }

    fn safe_momentum_cancel(gesture_id: GestureId, timestamp_us: u64) -> Self {
        let mut sample = Self::safe_cancel(gesture_id, timestamp_us);
        sample.contact_phase = NativeScrollPhase::None;
        sample.momentum_phase = NativeScrollPhase::Cancelled;
        sample
    }
}

/// `ContinuousScroller::apply_wheel` still exposes its historical
/// raw-platform sign contract. Convert the already canonical display-space y
/// delta at that single legacy boundary so natural scrolling is not applied a
/// second time.
pub fn continuous_scroller_input_from_canonical_y(canonical_dy: f32) -> f32 {
    -canonical_dy
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollapsedScrollPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// Assigns gesture IDs while preserving the platform's preference-adjusted
/// horizontal display-space delta sign.
///
/// Native adapters should call [`Self::adapt_native`] with separate contact
/// and momentum phases. [`Self::adapt_collapsed`] exists only for winit's
/// current macOS event contract, which cannot distinguish a new physical
/// contact from momentum `Began` after a contact has ended.
#[derive(Debug, Clone, Default)]
pub struct ScrollSampleAdapter {
    next_gesture_id: GestureId,
    active_contact: Option<GestureId>,
    awaiting_momentum: Option<GestureId>,
    active_momentum: Option<GestureId>,
    active_contact_contract: Option<GestureInputContract>,
    awaiting_momentum_contract: Option<GestureInputContract>,
    active_momentum_contract: Option<GestureInputContract>,
    last_contact_timestamp_us: Option<u64>,
    last_momentum_timestamp_us: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GestureInputContract {
    source: ScrollSource,
    phase_capability: ScrollPhaseCapability,
}

impl ScrollSampleAdapter {
    pub fn adapt_native(&mut self, mut event: RawScrollEvent) -> Option<ScrollSample> {
        // AppKit may report hasPreciseScrollingDeltas=false on a zero-delta
        // terminal packet. Source/capability belong to the gesture, not to an
        // individual packet, so inherit them before the Line fast path.
        let inherited_contract = if event.contact_phase != NativeScrollPhase::None
            && event.contact_phase != NativeScrollPhase::Began
        {
            self.active_contact_contract
        } else if event.momentum_phase != NativeScrollPhase::None || event.sequence_complete {
            self.active_momentum_contract
                .or(self.awaiting_momentum_contract)
        } else {
            None
        };
        if let Some(contract) = inherited_contract {
            event.source = contract.source;
            event.phase_capability = contract.phase_capability;
        }
        if event.source == ScrollSource::Line {
            return event.is_valid().then(|| Self::make_sample(0, event));
        }

        if event.contact_phase != NativeScrollPhase::None {
            let timestamp_reversed = event.contact_phase != NativeScrollPhase::Began
                && self
                    .last_contact_timestamp_us
                    .is_some_and(|last| event.timestamp_us < last);
            if !event.is_valid() || timestamp_reversed {
                return self.cancel_invalid_contact(event);
            }
            if event.contact_phase == NativeScrollPhase::Began {
                self.last_contact_timestamp_us = None;
            }
            self.last_contact_timestamp_us = Some(event.timestamp_us);
        } else if event.momentum_phase != NativeScrollPhase::None || event.sequence_complete {
            let starting_momentum =
                event.momentum_phase == NativeScrollPhase::Began && self.active_momentum.is_none();
            let timestamp_reversed = !starting_momentum
                && self
                    .last_momentum_timestamp_us
                    .is_some_and(|last| event.timestamp_us < last);
            if !event.is_valid() || timestamp_reversed {
                return self.cancel_invalid_momentum(event.timestamp_us);
            }
            if starting_momentum {
                self.last_momentum_timestamp_us = None;
            }
            self.last_momentum_timestamp_us = Some(event.timestamp_us);
        } else {
            return None;
        }

        let gesture_id = if event.contact_phase != NativeScrollPhase::None {
            self.contact_gesture_id(event.contact_phase)?
        } else if event.momentum_phase != NativeScrollPhase::None {
            self.momentum_gesture_id(event.momentum_phase)?
        } else if event.sequence_complete {
            self.active_momentum
                .take()
                .or_else(|| self.awaiting_momentum.take())?
        } else {
            return None;
        };

        let event_contract = GestureInputContract {
            source: event.source,
            phase_capability: event.phase_capability,
        };
        match event.contact_phase {
            NativeScrollPhase::Began => {
                self.active_contact_contract = Some(event_contract);
            }
            NativeScrollPhase::Ended => {
                self.awaiting_momentum_contract = self.active_contact_contract.take();
            }
            NativeScrollPhase::Cancelled => {
                self.active_contact_contract = None;
            }
            NativeScrollPhase::Changed | NativeScrollPhase::None => {}
        }
        match event.momentum_phase {
            NativeScrollPhase::Began => {
                self.active_momentum_contract = self.awaiting_momentum_contract.take();
            }
            NativeScrollPhase::Ended | NativeScrollPhase::Cancelled => {
                self.active_momentum_contract = None;
                self.awaiting_momentum_contract = None;
            }
            NativeScrollPhase::Changed | NativeScrollPhase::None => {}
        }

        if event.sequence_complete {
            if self.awaiting_momentum == Some(gesture_id) {
                self.awaiting_momentum = None;
            }
            if self.active_momentum == Some(gesture_id) {
                self.active_momentum = None;
            }
            self.active_momentum_contract = None;
            self.awaiting_momentum_contract = None;
        }

        Some(Self::make_sample(gesture_id, event))
    }

    /// Produce the one terminal packet needed by the current physical
    /// contact, then clear every continuation. Callers deliver the returned
    /// packet before resetting their router.
    pub fn cancel_active(&mut self, timestamp_us: u64) -> Option<ScrollSample> {
        let gesture_id = self.active_contact.take();
        self.awaiting_momentum = None;
        self.active_momentum = None;
        self.active_contact_contract = None;
        self.awaiting_momentum_contract = None;
        self.active_momentum_contract = None;
        self.last_contact_timestamp_us = None;
        self.last_momentum_timestamp_us = None;
        gesture_id.map(|gesture_id| ScrollSample::safe_cancel(gesture_id, timestamp_us))
    }

    pub fn reset(&mut self) {
        self.active_contact = None;
        self.awaiting_momentum = None;
        self.active_momentum = None;
        self.active_contact_contract = None;
        self.awaiting_momentum_contract = None;
        self.active_momentum_contract = None;
        self.last_contact_timestamp_us = None;
        self.last_momentum_timestamp_us = None;
    }

    pub fn adapt_collapsed(
        &mut self,
        timestamp_us: u64,
        delta_physical_px: (f32, f32),
        source: ScrollSource,
        phase: CollapsedScrollPhase,
        direction_inverted_from_device: bool,
        scale_factor: f32,
    ) -> Option<ScrollSample> {
        if source == ScrollSource::Line {
            return self.adapt_native(RawScrollEvent {
                timestamp_us,
                delta_physical_px,
                source,
                contact_phase: NativeScrollPhase::None,
                momentum_phase: NativeScrollPhase::None,
                sequence_complete: false,
                direction_inverted_from_device,
                scale_factor,
                phase_capability: ScrollPhaseCapability::CollapsedFallback,
            });
        }

        let (contact_phase, momentum_phase) = match phase {
            CollapsedScrollPhase::Started
                if self.awaiting_momentum.is_some() || self.active_momentum.is_some() =>
            {
                (NativeScrollPhase::None, NativeScrollPhase::Began)
            }
            CollapsedScrollPhase::Started => (NativeScrollPhase::Began, NativeScrollPhase::None),
            CollapsedScrollPhase::Moved if self.active_momentum.is_some() => {
                (NativeScrollPhase::None, NativeScrollPhase::Changed)
            }
            CollapsedScrollPhase::Moved => (NativeScrollPhase::Changed, NativeScrollPhase::None),
            CollapsedScrollPhase::Ended
                if self.active_momentum.is_some() || self.awaiting_momentum.is_some() =>
            {
                (NativeScrollPhase::None, NativeScrollPhase::Ended)
            }
            CollapsedScrollPhase::Ended => (NativeScrollPhase::Ended, NativeScrollPhase::None),
            CollapsedScrollPhase::Cancelled
                if self.active_momentum.is_some() || self.awaiting_momentum.is_some() =>
            {
                (NativeScrollPhase::None, NativeScrollPhase::Cancelled)
            }
            CollapsedScrollPhase::Cancelled => {
                (NativeScrollPhase::Cancelled, NativeScrollPhase::None)
            }
        };

        self.adapt_native(RawScrollEvent {
            timestamp_us,
            delta_physical_px,
            source,
            contact_phase,
            momentum_phase,
            sequence_complete: false,
            direction_inverted_from_device,
            scale_factor,
            phase_capability: ScrollPhaseCapability::CollapsedFallback,
        })
    }

    fn contact_gesture_id(&mut self, phase: NativeScrollPhase) -> Option<GestureId> {
        match phase {
            NativeScrollPhase::Began => {
                if self.active_contact.is_some() {
                    return None;
                }
                let gesture_id = self.allocate_gesture_id();
                self.active_contact = Some(gesture_id);
                Some(gesture_id)
            }
            NativeScrollPhase::Changed => self.active_contact,
            NativeScrollPhase::Ended => {
                let gesture_id = self.active_contact.take()?;
                self.awaiting_momentum = Some(gesture_id);
                Some(gesture_id)
            }
            NativeScrollPhase::Cancelled => self.active_contact.take(),
            NativeScrollPhase::None => None,
        }
    }

    fn cancel_invalid_contact(&mut self, event: RawScrollEvent) -> Option<ScrollSample> {
        // A malformed `Began` is not evidence that the currently active
        // contact owns that packet. Only continuation packets can cancel it.
        if event.contact_phase == NativeScrollPhase::Began {
            return None;
        }
        let gesture_id = self.active_contact.take()?;
        self.active_contact_contract = None;
        self.last_contact_timestamp_us = None;
        Some(ScrollSample::safe_cancel(gesture_id, event.timestamp_us))
    }

    fn cancel_invalid_momentum(&mut self, timestamp_us: u64) -> Option<ScrollSample> {
        // Momentum belongs to an already-ended generation. Clear only that
        // continuation and emit its own terminal packet; a newer physical
        // contact must remain untouched.
        let gesture_id = self
            .active_momentum
            .take()
            .or_else(|| self.awaiting_momentum.take())?;
        if self.awaiting_momentum == Some(gesture_id) {
            self.awaiting_momentum = None;
        }
        if self.active_momentum.is_none() {
            self.active_momentum_contract = None;
        }
        if self.awaiting_momentum.is_none() {
            self.awaiting_momentum_contract = None;
        }
        self.last_momentum_timestamp_us = None;
        Some(ScrollSample::safe_momentum_cancel(gesture_id, timestamp_us))
    }

    fn momentum_gesture_id(&mut self, phase: NativeScrollPhase) -> Option<GestureId> {
        match phase {
            NativeScrollPhase::Began => {
                let gesture_id = self
                    .active_momentum
                    .or_else(|| self.awaiting_momentum.take())?;
                self.active_momentum = Some(gesture_id);
                Some(gesture_id)
            }
            NativeScrollPhase::Changed => self.active_momentum,
            NativeScrollPhase::Ended | NativeScrollPhase::Cancelled => {
                let gesture_id = self
                    .active_momentum
                    .take()
                    .or_else(|| self.awaiting_momentum.take())?;
                if self.awaiting_momentum == Some(gesture_id) {
                    self.awaiting_momentum = None;
                }
                Some(gesture_id)
            }
            NativeScrollPhase::None => None,
        }
    }

    fn allocate_gesture_id(&mut self) -> GestureId {
        self.next_gesture_id = self.next_gesture_id.wrapping_add(1).max(1);
        self.next_gesture_id
    }

    fn make_sample(gesture_id: GestureId, event: RawScrollEvent) -> ScrollSample {
        // Keep the established settings-panel y bridge unchanged in this
        // horizontal paging fix. Its legacy ContinuousScroller boundary
        // compensates this sign separately.
        let legacy_y_sign = if event.direction_inverted_from_device {
            -1.0
        } else {
            1.0
        };
        ScrollSample {
            gesture_id,
            timestamp_us: event.timestamp_us,
            raw_dx: event.delta_physical_px.0,
            raw_dy: event.delta_physical_px.1,
            // Apple documents scrollingDeltaX/Y as already inverted according
            // to the user preference. winit forwards those same values. The
            // pager consumes x directly; do not undo the preference here.
            canonical_dx: event.delta_physical_px.0,
            canonical_dy: event.delta_physical_px.1 * legacy_y_sign,
            source: event.source,
            contact_phase: event.contact_phase,
            momentum_phase: event.momentum_phase,
            sequence_complete: event.sequence_complete,
            scale_factor: event.scale_factor,
            direction_inverted_from_device: event.direction_inverted_from_device,
            phase_capability: event.phase_capability,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderRoutePhase {
    Closed,
    Opening,
    Open,
    Closing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollRouteContext {
    pub settings_active: bool,
    pub folder_phase: FolderRoutePhase,
    pub blocking_interaction: bool,
    pub main_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollOwner {
    Settings,
    FolderPager,
    MainPager,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollRoute {
    Settings,
    FolderPager,
    MainPager,
    Blocked,
    Quarantined,
    Dropped,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutedScrollSample {
    pub route: ScrollRoute,
    pub sample: ScrollSample,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveScrollContact {
    pub gesture_id: GestureId,
    pub owner: ScrollOwner,
}

/// Owns app-level scroll routing. Physical contact and old momentum
/// quarantines are intentionally independent, so a new contact can take over
/// while old momentum remains isolated by gesture ID.
#[derive(Debug, Clone, Default)]
pub struct PagerInputRouter {
    active_contact: Option<ActiveScrollContact>,
    quarantined_gesture_ids: BTreeMap<GestureId, ScrollOwner>,
    settings_continuations: BTreeSet<GestureId>,
}

impl PagerInputRouter {
    pub const fn active_contact(&self) -> Option<ActiveScrollContact> {
        self.active_contact
    }

    pub fn quarantined_gesture_ids(&self) -> impl Iterator<Item = GestureId> + '_ {
        self.quarantined_gesture_ids.keys().copied()
    }

    pub fn reset(&mut self) {
        self.active_contact = None;
        self.quarantined_gesture_ids.clear();
        self.settings_continuations.clear();
    }

    pub fn active_cancel_sample(&self, timestamp_us: u64) -> Option<ScrollSample> {
        self.active_contact
            .map(|active| ScrollSample::safe_cancel(active.gesture_id, timestamp_us))
    }

    pub fn route(
        &mut self,
        mut sample: ScrollSample,
        context: ScrollRouteContext,
    ) -> RoutedScrollSample {
        if !sample.is_valid() {
            let Some(active) = self
                .active_contact
                .filter(|active| active.gesture_id == sample.gesture_id)
            else {
                return RoutedScrollSample {
                    route: ScrollRoute::Dropped,
                    sample,
                };
            };
            sample = ScrollSample::safe_cancel(active.gesture_id, sample.timestamp_us);
        }
        if sample.source == ScrollSource::Line {
            return RoutedScrollSample {
                route: if context.settings_active {
                    ScrollRoute::Settings
                } else {
                    ScrollRoute::Dropped
                },
                sample,
            };
        }

        let route = if sample.contact_phase != NativeScrollPhase::None {
            self.route_contact(sample, context)
        } else if sample.momentum_phase != NativeScrollPhase::None {
            self.route_momentum(sample)
        } else if sample.sequence_complete {
            self.finish_sequence(sample.gesture_id)
        } else {
            ScrollRoute::Dropped
        };

        if sample.sequence_complete {
            self.quarantined_gesture_ids.remove(&sample.gesture_id);
            self.settings_continuations.remove(&sample.gesture_id);
        }

        RoutedScrollSample { route, sample }
    }

    fn route_contact(&mut self, sample: ScrollSample, context: ScrollRouteContext) -> ScrollRoute {
        match sample.contact_phase {
            NativeScrollPhase::Began => {
                if self.active_contact.is_some() {
                    return ScrollRoute::Blocked;
                }
                let owner = Self::resolve_owner(context);
                self.active_contact = Some(ActiveScrollContact {
                    gesture_id: sample.gesture_id,
                    owner,
                });
                Self::owner_route(owner)
            }
            NativeScrollPhase::Changed => self.route_active(sample.gesture_id),
            NativeScrollPhase::Ended | NativeScrollPhase::Cancelled => {
                let Some(active) = self.active_contact else {
                    return ScrollRoute::Dropped;
                };
                if active.gesture_id != sample.gesture_id {
                    return ScrollRoute::Dropped;
                }
                self.active_contact = None;
                if sample.contact_phase == NativeScrollPhase::Ended {
                    match active.owner {
                        ScrollOwner::MainPager | ScrollOwner::FolderPager => {
                            self.quarantined_gesture_ids
                                .insert(active.gesture_id, active.owner);
                        }
                        ScrollOwner::Settings => {
                            self.settings_continuations.insert(active.gesture_id);
                        }
                        ScrollOwner::Blocked => {}
                    }
                }
                Self::owner_route(active.owner)
            }
            NativeScrollPhase::None => ScrollRoute::Dropped,
        }
    }

    fn route_active(&self, gesture_id: GestureId) -> ScrollRoute {
        match self.active_contact {
            Some(active) if active.gesture_id == gesture_id => Self::owner_route(active.owner),
            _ => ScrollRoute::Dropped,
        }
    }

    fn route_momentum(&mut self, sample: ScrollSample) -> ScrollRoute {
        if self
            .quarantined_gesture_ids
            .contains_key(&sample.gesture_id)
        {
            if sample.momentum_phase.is_terminal() {
                self.quarantined_gesture_ids.remove(&sample.gesture_id);
            }
            return ScrollRoute::Quarantined;
        }
        if self.settings_continuations.contains(&sample.gesture_id) {
            if sample.momentum_phase.is_terminal() {
                self.settings_continuations.remove(&sample.gesture_id);
            }
            return ScrollRoute::Settings;
        }
        ScrollRoute::Dropped
    }

    fn finish_sequence(&mut self, gesture_id: GestureId) -> ScrollRoute {
        let quarantined = self.quarantined_gesture_ids.remove(&gesture_id).is_some();
        let settings = self.settings_continuations.remove(&gesture_id);
        if quarantined {
            ScrollRoute::Quarantined
        } else if settings {
            ScrollRoute::Settings
        } else {
            ScrollRoute::Dropped
        }
    }

    const fn resolve_owner(context: ScrollRouteContext) -> ScrollOwner {
        if context.settings_active {
            ScrollOwner::Settings
        } else if matches!(
            context.folder_phase,
            FolderRoutePhase::Opening | FolderRoutePhase::Closing
        ) || context.blocking_interaction
        {
            ScrollOwner::Blocked
        } else if matches!(context.folder_phase, FolderRoutePhase::Open) {
            ScrollOwner::FolderPager
        } else if context.main_available {
            ScrollOwner::MainPager
        } else {
            ScrollOwner::Blocked
        }
    }

    const fn owner_route(owner: ScrollOwner) -> ScrollRoute {
        match owner {
            ScrollOwner::Settings => ScrollRoute::Settings,
            ScrollOwner::FolderPager => ScrollRoute::FolderPager,
            ScrollOwner::MainPager => ScrollRoute::MainPager,
            ScrollOwner::Blocked => ScrollRoute::Blocked,
        }
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
            owned_geometry: InputOwnedGeometry::default(),
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
    fn event_position_overrides_stale_pointer_snapshot_for_scroll_ownership() {
        let owned = PhysicalPoint::new(100.0, 80.0);
        let outside = PhysicalPoint::new(400.0, 300.0);
        let geometry = InputOwnedGeometry {
            viewport_owned: false,
            page_frame: Some(InputRoundedRect {
                center: owned,
                half_width: 80.0,
                half_height: 60.0,
                radius: 20.0,
            }),
            bottom_capsule: None,
            edit_gear: None,
        };

        let stale_outside = InputRoutingSnapshot {
            visible: true,
            region: OUTSIDE,
            owned_geometry: geometry,
            router_state: RouterState::Idle,
            generation: 1,
        };
        assert_eq!(stale_outside.region_at(owned), OWNED);
        assert!(!stale_outside.forwards_vertical_scroll_at(owned));

        let stale_owned = InputRoutingSnapshot {
            region: OWNED,
            ..stale_outside
        };
        assert_eq!(stale_owned.region_at(outside), OUTSIDE);
        assert!(stale_owned.forwards_vertical_scroll_at(outside));
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

    fn native_event(
        timestamp_us: u64,
        dx: f32,
        contact_phase: NativeScrollPhase,
        momentum_phase: NativeScrollPhase,
    ) -> RawScrollEvent {
        RawScrollEvent {
            timestamp_us,
            delta_physical_px: (dx, 0.0),
            source: ScrollSource::Precise,
            contact_phase,
            momentum_phase,
            sequence_complete: false,
            direction_inverted_from_device: false,
            scale_factor: 2.0,
            phase_capability: ScrollPhaseCapability::Separate,
        }
    }

    fn route_context() -> ScrollRouteContext {
        ScrollRouteContext {
            settings_active: false,
            folder_phase: FolderRoutePhase::Closed,
            blocking_interaction: false,
            main_available: true,
        }
    }

    #[test]
    fn native_adapter_keeps_contact_and_momentum_ids_separate_from_new_contact() {
        let mut adapter = ScrollSampleAdapter::default();
        let a_begin = adapter
            .adapt_native(native_event(
                0,
                0.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        let a_end = adapter
            .adapt_native(native_event(
                16_000,
                0.0,
                NativeScrollPhase::Ended,
                NativeScrollPhase::None,
            ))
            .unwrap();
        let a_momentum = adapter
            .adapt_native(native_event(
                17_000,
                -2.0,
                NativeScrollPhase::None,
                NativeScrollPhase::Began,
            ))
            .unwrap();
        let b_begin = adapter
            .adapt_native(native_event(
                18_000,
                0.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        let a_changed = adapter
            .adapt_native(native_event(
                19_000,
                -1.0,
                NativeScrollPhase::None,
                NativeScrollPhase::Changed,
            ))
            .unwrap();

        assert_eq!(a_begin.gesture_id, a_end.gesture_id);
        assert_eq!(a_begin.gesture_id, a_momentum.gesture_id);
        assert_eq!(a_begin.gesture_id, a_changed.gesture_id);
        assert_ne!(a_begin.gesture_id, b_begin.gesture_id);
        assert_eq!(b_begin.contact_phase, NativeScrollPhase::Began);
        assert_eq!(a_changed.momentum_phase, NativeScrollPhase::Changed);
    }

    #[test]
    fn appkit_preference_adjusted_x_is_not_inverted_again() {
        let mut adapter = ScrollSampleAdapter::default();
        let mut event = native_event(0, 12.0, NativeScrollPhase::Began, NativeScrollPhase::None);
        event.delta_physical_px.1 = -4.0;
        event.direction_inverted_from_device = true;
        let sample = adapter.adapt_native(event).unwrap();
        assert_eq!(sample.raw_dx, 12.0);
        assert_eq!(sample.raw_dy, -4.0);
        assert_eq!(sample.canonical_dx, 12.0);
        assert_eq!(
            sample.canonical_dy, 4.0,
            "the settings y contract is intentionally unchanged"
        );

        let mut router = PagerInputRouter::default();
        let routed = router.route(sample, route_context());
        assert_eq!(routed.route, ScrollRoute::MainPager);
        assert_eq!(routed.sample.canonical_dx, 12.0);
    }

    #[test]
    fn zero_delta_terminal_reaches_pager_and_creates_quarantine() {
        let mut adapter = ScrollSampleAdapter::default();
        let mut router = PagerInputRouter::default();
        let begin = adapter
            .adapt_native(native_event(
                0,
                0.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        let gesture_id = begin.gesture_id;
        assert_eq!(
            router.route(begin, route_context()).route,
            ScrollRoute::MainPager
        );
        let ended = adapter
            .adapt_native(native_event(
                16_000,
                0.0,
                NativeScrollPhase::Ended,
                NativeScrollPhase::None,
            ))
            .unwrap();
        assert_eq!(
            router.route(ended, route_context()).route,
            ScrollRoute::MainPager
        );
        assert_eq!(router.active_contact(), None);
        assert_eq!(
            router.quarantined_gesture_ids().collect::<Vec<_>>(),
            vec![gesture_id]
        );
    }

    #[test]
    fn old_momentum_cannot_mutate_new_active_contact() {
        let mut adapter = ScrollSampleAdapter::default();
        let mut router = PagerInputRouter::default();
        let a_begin = adapter
            .adapt_native(native_event(
                0,
                0.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        let a_id = a_begin.gesture_id;
        router.route(a_begin, route_context());
        let a_end = adapter
            .adapt_native(native_event(
                16_000,
                0.0,
                NativeScrollPhase::Ended,
                NativeScrollPhase::None,
            ))
            .unwrap();
        router.route(a_end, route_context());
        let a_momentum_begin = adapter
            .adapt_native(native_event(
                17_000,
                -3.0,
                NativeScrollPhase::None,
                NativeScrollPhase::Began,
            ))
            .unwrap();
        assert_eq!(
            router.route(a_momentum_begin, route_context()).route,
            ScrollRoute::Quarantined
        );

        let b_begin = adapter
            .adapt_native(native_event(
                18_000,
                0.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        let b_id = b_begin.gesture_id;
        router.route(b_begin, route_context());
        let active_before = router.active_contact();

        let a_momentum_end = adapter
            .adapt_native(native_event(
                19_000,
                0.0,
                NativeScrollPhase::None,
                NativeScrollPhase::Ended,
            ))
            .unwrap();
        assert_eq!(a_momentum_end.gesture_id, a_id);
        assert_eq!(
            router.route(a_momentum_end, route_context()).route,
            ScrollRoute::Quarantined
        );
        assert_eq!(router.active_contact(), active_before);
        assert_eq!(router.active_contact().unwrap().gesture_id, b_id);
        assert!(router.quarantined_gesture_ids().next().is_none());
    }

    #[test]
    fn malformed_old_momentum_clears_only_its_generation_not_new_contact() {
        let mut adapter = ScrollSampleAdapter::default();
        let mut router = PagerInputRouter::default();
        let origin = std::time::Instant::now();
        let mut scroller = crate::scroll::Scroller::new(crate::scroll::ScrollBounds {
            page_extent: 1000.0,
            page_count: 3,
        });

        let a_begin = adapter
            .adapt_native(native_event(
                0,
                0.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        router.route(a_begin, route_context());
        let a_end = adapter
            .adapt_native(native_event(
                16_000,
                0.0,
                NativeScrollPhase::Ended,
                NativeScrollPhase::None,
            ))
            .unwrap();
        router.route(a_end, route_context());
        let a_momentum = adapter
            .adapt_native(native_event(
                17_000,
                -5.0,
                NativeScrollPhase::None,
                NativeScrollPhase::Began,
            ))
            .unwrap();
        assert_eq!(
            router.route(a_momentum, route_context()).route,
            ScrollRoute::Quarantined
        );

        let b_begin = adapter
            .adapt_native(native_event(
                18_000,
                0.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        let b_id = b_begin.gesture_id;
        let routed = router.route(b_begin, route_context());
        scroller.apply_wheel_delta_scaled(
            routed.sample.canonical_dx,
            routed.sample.canonical_dy,
            routed.sample.scale_factor,
            origin + std::time::Duration::from_micros(routed.sample.timestamp_us),
            crate::scroll::WheelPhase::Started,
        );
        let b_changed = adapter
            .adapt_native(native_event(
                34_000,
                -120.0,
                NativeScrollPhase::Changed,
                NativeScrollPhase::None,
            ))
            .unwrap();
        let routed = router.route(b_changed, route_context());
        scroller.apply_wheel_delta_scaled(
            routed.sample.canonical_dx,
            routed.sample.canonical_dy,
            routed.sample.scale_factor,
            origin + std::time::Duration::from_micros(routed.sample.timestamp_us),
            crate::scroll::WheelPhase::Moved,
        );
        let before = (
            scroller.phase,
            scroller.position,
            scroller.wheel_diagnostics().filtered_velocity,
            router.active_contact(),
        );

        let mut invalid_a = native_event(
            16_500,
            f32::NAN,
            NativeScrollPhase::None,
            NativeScrollPhase::Changed,
        );
        invalid_a.scale_factor = f32::INFINITY;
        let a_cancel = adapter.adapt_native(invalid_a).unwrap();
        assert_eq!(a_cancel.gesture_id, a_begin.gesture_id);
        assert_eq!(a_cancel.contact_phase, NativeScrollPhase::None);
        assert_eq!(a_cancel.momentum_phase, NativeScrollPhase::Cancelled);
        assert_eq!(
            router.route(a_cancel, route_context()).route,
            ScrollRoute::Quarantined
        );
        assert_eq!(
            (
                scroller.phase,
                scroller.position,
                scroller.wheel_diagnostics().filtered_velocity,
                router.active_contact(),
            ),
            before
        );
        assert_eq!(router.active_contact().unwrap().gesture_id, b_id);

        let b_continues = adapter
            .adapt_native(native_event(
                50_000,
                -20.0,
                NativeScrollPhase::Changed,
                NativeScrollPhase::None,
            ))
            .unwrap();
        assert_eq!(b_continues.gesture_id, b_id);
        assert_eq!(
            router.route(b_continues, route_context()).route,
            ScrollRoute::MainPager
        );
    }

    #[test]
    fn scroll_owner_precedence_and_sticky_folder_owner() {
        let base = ScrollRouteContext {
            settings_active: false,
            folder_phase: FolderRoutePhase::Closed,
            blocking_interaction: false,
            main_available: true,
        };
        assert_eq!(
            PagerInputRouter::resolve_owner(ScrollRouteContext {
                settings_active: true,
                folder_phase: FolderRoutePhase::Open,
                blocking_interaction: true,
                ..base
            }),
            ScrollOwner::Settings
        );
        assert_eq!(
            PagerInputRouter::resolve_owner(ScrollRouteContext {
                folder_phase: FolderRoutePhase::Opening,
                ..base
            }),
            ScrollOwner::Blocked
        );
        assert_eq!(
            PagerInputRouter::resolve_owner(ScrollRouteContext {
                folder_phase: FolderRoutePhase::Open,
                ..base
            }),
            ScrollOwner::FolderPager
        );

        let mut adapter = ScrollSampleAdapter::default();
        let mut router = PagerInputRouter::default();
        let begin = adapter
            .adapt_native(native_event(
                0,
                0.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        let id = begin.gesture_id;
        assert_eq!(
            router
                .route(
                    begin,
                    ScrollRouteContext {
                        folder_phase: FolderRoutePhase::Open,
                        ..base
                    }
                )
                .route,
            ScrollRoute::FolderPager
        );
        let changed = ScrollSample {
            gesture_id: id,
            contact_phase: NativeScrollPhase::Changed,
            canonical_dx: 5.0,
            raw_dx: 5.0,
            ..begin
        };
        assert_eq!(
            router.route(changed, base).route,
            ScrollRoute::FolderPager,
            "owner remains the folder even after the visible phase changes"
        );
    }

    #[test]
    fn horizontal_fix_does_not_change_existing_settings_y_bridge() {
        fn resulting_content_delta(raw_dy: f32, direction_inverted: bool) -> f32 {
            let mut adapter = ScrollSampleAdapter::default();
            let mut event = native_event(1, 0.0, NativeScrollPhase::Began, NativeScrollPhase::None);
            event.delta_physical_px.1 = raw_dy;
            event.direction_inverted_from_device = direction_inverted;
            let canonical = adapter.adapt_native(event).unwrap().canonical_dy;
            // ContinuousScroller's legacy apply_wheel implementation negates
            // its input. The bridge must make its result canonical again.
            -continuous_scroller_input_from_canonical_y(canonical)
        }

        assert_eq!(resulting_content_delta(12.0, false), 12.0);
        assert_eq!(resulting_content_delta(12.0, true), -12.0);
    }

    #[test]
    fn malformed_native_packet_cancels_active_contact_once() {
        let mut adapter = ScrollSampleAdapter::default();
        let begin = adapter
            .adapt_native(native_event(
                1,
                0.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        let mut invalid = native_event(
            2,
            f32::NAN,
            NativeScrollPhase::Changed,
            NativeScrollPhase::None,
        );
        invalid.scale_factor = f32::INFINITY;
        let cancel = adapter.adapt_native(invalid).unwrap();
        assert_eq!(cancel.gesture_id, begin.gesture_id);
        assert_eq!(cancel.contact_phase, NativeScrollPhase::Cancelled);
        assert!(cancel.is_valid());
        assert!(adapter.adapt_native(invalid).is_none());
    }

    #[test]
    fn malformed_samples_never_reach_main_folder_or_settings_as_numbers() {
        let contexts = [
            (
                ScrollRouteContext {
                    folder_phase: FolderRoutePhase::Closed,
                    ..route_context()
                },
                ScrollRoute::MainPager,
            ),
            (
                ScrollRouteContext {
                    folder_phase: FolderRoutePhase::Open,
                    ..route_context()
                },
                ScrollRoute::FolderPager,
            ),
            (
                ScrollRouteContext {
                    settings_active: true,
                    ..route_context()
                },
                ScrollRoute::Settings,
            ),
        ];

        for (context, expected_route) in contexts {
            let mut adapter = ScrollSampleAdapter::default();
            let mut router = PagerInputRouter::default();
            let begin = adapter
                .adapt_native(native_event(
                    1,
                    0.0,
                    NativeScrollPhase::Began,
                    NativeScrollPhase::None,
                ))
                .unwrap();
            assert_eq!(router.route(begin, context).route, expected_route);
            let invalid = ScrollSample {
                canonical_dx: f32::NAN,
                raw_dx: f32::INFINITY,
                scale_factor: 0.0,
                contact_phase: NativeScrollPhase::Changed,
                ..begin
            };
            let routed = router.route(invalid, context);
            assert_eq!(routed.route, expected_route);
            assert_eq!(routed.sample.contact_phase, NativeScrollPhase::Cancelled);
            assert!(routed.sample.is_valid());
            assert_eq!(router.active_contact(), None);
            assert!(router.quarantined_gesture_ids().next().is_none());
        }

        let mut settings_router = PagerInputRouter::default();
        let invalid_line = ScrollSample {
            gesture_id: 0,
            timestamp_us: 1,
            raw_dx: 0.0,
            raw_dy: f32::NAN,
            canonical_dx: 0.0,
            canonical_dy: f32::NAN,
            source: ScrollSource::Line,
            contact_phase: NativeScrollPhase::None,
            momentum_phase: NativeScrollPhase::None,
            sequence_complete: false,
            scale_factor: 1.0,
            direction_inverted_from_device: false,
            phase_capability: ScrollPhaseCapability::Separate,
        };
        assert_eq!(
            settings_router
                .route(
                    invalid_line,
                    ScrollRouteContext {
                        settings_active: true,
                        ..route_context()
                    }
                )
                .route,
            ScrollRoute::Dropped
        );
    }

    #[test]
    fn lifecycle_cancel_then_reset_accepts_new_began_without_quarantine() {
        let mut adapter = ScrollSampleAdapter::default();
        let mut router = PagerInputRouter::default();
        let first = adapter
            .adapt_native(native_event(
                10,
                2.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        assert_eq!(
            router.route(first, route_context()).route,
            ScrollRoute::MainPager
        );
        let cancel = adapter.cancel_active(11).unwrap();
        assert_eq!(
            router.route(cancel, route_context()).route,
            ScrollRoute::MainPager
        );
        assert!(router.quarantined_gesture_ids().next().is_none());
        adapter.reset();
        router.reset();

        let second = adapter
            .adapt_native(native_event(
                12,
                -3.0,
                NativeScrollPhase::Began,
                NativeScrollPhase::None,
            ))
            .unwrap();
        assert_ne!(first.gesture_id, second.gesture_id);
        assert_eq!(
            router.route(second, route_context()).route,
            ScrollRoute::MainPager
        );
    }
}
