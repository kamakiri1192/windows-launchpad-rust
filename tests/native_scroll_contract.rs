use launchpad_windows::input_routing::{
    CollapsedScrollPhase, FolderRoutePhase, NativeScrollPhase, PagerInputRouter, RawScrollEvent,
    ScrollPhaseCapability, ScrollRoute, ScrollRouteContext, ScrollSampleAdapter, ScrollSource,
};
use launchpad_windows::scroll::{ScrollBounds, Scroller, WheelPhase};
use std::time::{Duration, Instant};

fn event(
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

fn main_context() -> ScrollRouteContext {
    ScrollRouteContext {
        settings_active: false,
        folder_phase: FolderRoutePhase::Closed,
        blocking_interaction: false,
        main_available: true,
    }
}

/// Production AppKit emits these as four events with separate `phase` and
/// `momentumPhase` fields. A collapsed TouchPhase cannot represent the overlap.
#[test]
fn appkit_equivalent_old_momentum_does_not_take_over_new_contact() {
    let mut adapter = ScrollSampleAdapter::default();
    let mut router = PagerInputRouter::default();

    let a_begin = adapter
        .adapt_native(event(
            0,
            0.0,
            NativeScrollPhase::Began,
            NativeScrollPhase::None,
        ))
        .unwrap();
    let a_id = a_begin.gesture_id;
    assert_eq!(
        router.route(a_begin, main_context()).route,
        ScrollRoute::MainPager
    );

    let a_end = adapter
        .adapt_native(event(
            16_000,
            0.0,
            NativeScrollPhase::Ended,
            NativeScrollPhase::None,
        ))
        .unwrap();
    assert_eq!(
        router.route(a_end, main_context()).route,
        ScrollRoute::MainPager
    );

    let a_momentum_begin = adapter
        .adapt_native(event(
            17_000,
            -5.0,
            NativeScrollPhase::None,
            NativeScrollPhase::Began,
        ))
        .unwrap();
    assert_eq!(a_momentum_begin.gesture_id, a_id);
    assert_eq!(
        router.route(a_momentum_begin, main_context()).route,
        ScrollRoute::Quarantined
    );

    let b_begin = adapter
        .adapt_native(event(
            18_000,
            3.0,
            NativeScrollPhase::Began,
            NativeScrollPhase::None,
        ))
        .unwrap();
    let b_id = b_begin.gesture_id;
    assert_ne!(a_id, b_id);
    assert_eq!(
        router.route(b_begin, main_context()).route,
        ScrollRoute::MainPager
    );

    let a_momentum_changed = adapter
        .adapt_native(event(
            19_000,
            -2.0,
            NativeScrollPhase::None,
            NativeScrollPhase::Changed,
        ))
        .unwrap();
    assert_eq!(a_momentum_changed.gesture_id, a_id);
    assert_eq!(
        router.route(a_momentum_changed, main_context()).route,
        ScrollRoute::Quarantined
    );
    assert_eq!(router.active_contact().unwrap().gesture_id, b_id);
}

fn visible_delta_after_native_packet(dx: f32, direction_inverted: bool) -> (f32, f32) {
    let mut adapter = ScrollSampleAdapter::default();
    let mut raw = event(0, dx, NativeScrollPhase::Began, NativeScrollPhase::None);
    raw.direction_inverted_from_device = direction_inverted;
    let sample = adapter.adapt_native(raw).unwrap();
    let canonical = sample.canonical_dx;
    let mut scroller = Scroller::new(ScrollBounds {
        page_extent: 1000.0,
        page_count: 3,
    });
    scroller.position = -1000.0;
    let before = scroller.position;
    scroller.apply_wheel_delta_scaled(
        sample.canonical_dx,
        sample.canonical_dy,
        sample.scale_factor,
        Instant::now() + Duration::from_secs(1),
        WheelPhase::Started,
    );
    (canonical, scroller.position - before)
}

fn visible_delta_after_winit_packet(dx: f32, direction_inverted: bool) -> (f32, f32) {
    let mut adapter = ScrollSampleAdapter::default();
    let sample = adapter
        .adapt_collapsed(
            0,
            (dx, 0.0),
            ScrollSource::Precise,
            CollapsedScrollPhase::Started,
            direction_inverted,
            2.0,
        )
        .unwrap();
    let canonical = sample.canonical_dx;
    let mut scroller = Scroller::new(ScrollBounds {
        page_extent: 1000.0,
        page_count: 3,
    });
    scroller.position = -1000.0;
    let before = scroller.position;
    scroller.apply_wheel_delta_scaled(
        sample.canonical_dx,
        sample.canonical_dy,
        sample.scale_factor,
        Instant::now() + Duration::from_secs(1),
        WheelPhase::Started,
    );
    (canonical, scroller.position - before)
}

#[test]
fn appkit_and_winit_preserve_macos_natural_scroll_direction() {
    // AppKit has already applied the preference to scrollingDeltaX. For the
    // same finger-right gesture, Natural ON reports display-space positive;
    // Natural OFF reports the opposite sign. winit forwards that AppKit delta.
    for path in [
        visible_delta_after_native_packet as fn(f32, bool) -> (f32, f32),
        visible_delta_after_winit_packet,
    ] {
        let (natural_canonical, natural_visible) = path(24.0, true);
        assert_eq!(natural_canonical, 24.0);
        assert!(
            natural_visible > 0.0,
            "Natural ON finger-right must move visible content right"
        );

        let (traditional_canonical, traditional_visible) = path(-24.0, false);
        assert_eq!(traditional_canonical, -24.0);
        assert!(
            traditional_visible < 0.0,
            "Natural OFF must preserve the OS-provided opposite direction"
        );
    }
}

#[test]
fn precise_contact_keeps_its_contract_when_zero_terminal_looks_like_line_input() {
    let mut adapter = ScrollSampleAdapter::default();
    let mut scroller = Scroller::new(ScrollBounds {
        page_extent: 1000.0,
        page_count: 3,
    });
    scroller.position = -1000.0;
    let origin = Instant::now() + Duration::from_secs(1);

    for (timestamp_us, dx, phase, wheel_phase) in [
        (0, 0.0, NativeScrollPhase::Began, WheelPhase::Started),
        (
            16_000,
            -240.0,
            NativeScrollPhase::Changed,
            WheelPhase::Moved,
        ),
    ] {
        let sample = adapter
            .adapt_native(event(timestamp_us, dx, phase, NativeScrollPhase::None))
            .unwrap();
        scroller.apply_wheel_delta_scaled(
            sample.canonical_dx,
            sample.canonical_dy,
            sample.scale_factor,
            origin + Duration::from_micros(timestamp_us),
            wheel_phase,
        );
    }
    assert_eq!(
        scroller.phase,
        launchpad_windows::scroll::Phase::WheelGesture
    );

    let mut terminal = event(
        32_000,
        0.0,
        NativeScrollPhase::Ended,
        NativeScrollPhase::None,
    );
    // Equivalent to AppKit toggling hasPreciseScrollingDeltas=false on the
    // zero-delta Ended packet.
    terminal.source = ScrollSource::Line;
    terminal.phase_capability = ScrollPhaseCapability::CollapsedFallback;
    let ended = adapter.adapt_native(terminal).unwrap();
    assert_eq!(ended.source, ScrollSource::Precise);
    assert_eq!(ended.phase_capability, ScrollPhaseCapability::Separate);
    assert_eq!(ended.contact_phase, NativeScrollPhase::Ended);
    assert_ne!(ended.gesture_id, 0);
    let gesture_id = ended.gesture_id;
    scroller.apply_wheel_delta_scaled(
        ended.canonical_dx,
        ended.canonical_dy,
        ended.scale_factor,
        origin + Duration::from_micros(ended.timestamp_us),
        WheelPhase::Ended,
    );
    assert_eq!(scroller.phase, launchpad_windows::scroll::Phase::Settling);

    let mut momentum = event(
        33_000,
        -12.0,
        NativeScrollPhase::None,
        NativeScrollPhase::Began,
    );
    momentum.source = ScrollSource::Line;
    momentum.phase_capability = ScrollPhaseCapability::CollapsedFallback;
    let momentum = adapter.adapt_native(momentum).unwrap();
    assert_eq!(momentum.gesture_id, gesture_id);
    assert_eq!(momentum.source, ScrollSource::Precise);
    assert_eq!(momentum.phase_capability, ScrollPhaseCapability::Separate);
}
