# Input passthrough implementation

This document records the implementation and repeatable verification for
`INPUT_PASSTHROUGH_REQUIREMENTS.md`. The requirements and technical research
remain the normative behavior and design references.

## Architecture

`src/input_routing.rs` owns the platform-independent state machine:

- one immutable `InputRoutingSnapshot` is published after app state changes;
- the 8 physical-pixel intent threshold resolves left click versus page drag;
- right drag becomes a terminal cancellation;
- wheel forwarding is enabled only while the launcher is visible, idle, and
  the pointer is in `OutsideTransparent`;
- modal, edit, folder, and active gesture states retain launcher ownership.

The native callbacks read a snapshot. They never keep references into the
mutable app, renderer, layout, or window state.

### Windows

The winit message hook intercepts the original `WM_MOUSEWHEEL` and
`WM_POINTERWHEEL`. It walks
downward from the launcher's `GA_ROOT` HWND with `GW_HWNDNEXT`, filters
invisible, disabled, cloaked, transparent, and same-process windows, then
selects one eligible top-level application sink. Resolving from `GA_ROOT` also
covers wheel messages addressed to a focused child/input sink. Hit testing uses
the signed screen point carried by wheel `lParam`, not the separately
DPI-virtualized `MSG.pt`. The unchanged message, `wParam`, and `lParam` are
delivered once. Confirmed clicks continue to use the deepest spatial child.

Chromium and Electron perform their own `WindowFromPoint` check after receiving
`WM_MOUSEWHEEL`; a successful `PostMessageW` can therefore be discarded when
the opaque launcher is still at that point. For the `Chrome_WidgetWin_1`
framework sink, the adapter uses a bounded `SendMessageTimeoutW` dispatch. Only
during that synchronous call, the launcher region has a one-physical-pixel
hit-test hole at the original wheel point; every other pixel remains composed
to avoid visible flashing. Its default region is restored before returning.
This does not hide, deactivate, move, reorder, recreate, or restyle the
launcher. If a future launcher window has a custom region, the compatibility
path returns `Unsupported` instead of overwriting it.

Confirmed clicks resolve and freeze the same target before the launcher hides.
After hide, the adapter verifies that the cursor, root HWND, PID, and deepest
child HWND are unchanged, then uses one private-tagged `SendInput` batch for
the complete button-down/button-up pair. Unlike direct button messages, this
preserves normal Win32 activation, capture, and `WM_CONTEXTMENU` behavior.
Wheel delivery does not call `SetWindowPos`, toggle Z-order, inject a global
mouse stream, or hide/show the launcher.

Before click injection, a harmless targeted `WM_NULL` performs a structured
UIPI preflight because `SendInput` itself does not identify UIPI blocking in
its return value. A target or cursor change cancels delivery instead of
clicking a different window.

Some receivers explicitly activate themselves from their wheel handler. This
is not handled by the rejected broad `forwarding_wheel` / summon-grace
workaround. A wheel queue arms a one-shot correlation before `PostMessageW`
(and removes it again if posting fails), so an immediately activating receiver
cannot race the guard. The correlation contains the launcher foreground state,
exact receiver root HWND, and queue time. A focus-loss auto-hide is suppressed
only when the new foreground window is that exact receiver root within the
bounded interval; the record is consumed on the first focus-loss check.
Unrelated or later focus changes keep normal auto-hide behavior.

### macOS

The transparent launcher remains event-opaque (`ignoresMouseEvents = false`).
This is required to distinguish a click from a page drag before a complete
click is allowed to reach another application.

An AppKit local event monitor retains the original button down/up `CGEvent`
while the pure router resolves click versus drag. It freezes the window below
the launcher with
`windowNumberAtPoint:belowWindowWithWindowNumber:`, maps that window to its
owner PID, and rejects same-process targets. After a confirmed click hides the
launcher, the adapter revalidates the cursor, target window, and PID, tags the
original retained events, then posts the complete down/up pair at the Core
Graphics HID event tap, before session annotation. This constrained global
posting is used only after the launcher is hidden, so normal WindowServer/AppKit
hit testing selects the now-exposed destination and the launcher cannot
self-receive. The adapter does not synthesize a new coordinate-only mouse event
and does not use `CGEventPostToPid`, whose delivery does not establish that an
AppKit target window accepted the mouse action.

Wheel forwarding uses an active session `CGEventTap`, installed for the
launcher's lifetime. For an eligible outside vertical sequence, the callback
resolves the window below the launcher and changes the original event's target
PID and window-under-pointer fields before returning that event to the event
system. The original precise delta, coordinates, modifiers, phase, momentum
phase, timestamp, and device metadata remain intact. A phase/momentum sequence
locks one target. Hover is outside the tap mask. If the tap is unavailable, an
outside wheel is consumed by the local monitor instead of becoming launcher
page input.

Private tags prevent the confirmed click pair from being captured and delivered
again. The session tap also verifies that the launcher is still the frontmost
window at the event point before retargeting, so events already destined for
another application are not rewritten.

The macOS lifecycle records a requested-focus state for initial creation and
re-summon. Only a `Focused(false)` received before the requested
`Focused(true)` is ignored; after acquisition, unrelated focus loss retains
the normal auto-hide behavior. Native QA snapshots expose the real
`NSWindow::windowNumber`, focused state, and Core Graphics Z-order.

macOS delivery requires both event-listening and event-posting approval. The
adapter uses `CGPreflightListenEventAccess` and
`CGPreflightPostEventAccess`, requests the missing approval once with
`CGRequestListenEventAccess` / `CGRequestPostEventAccess`, and emits an
actionable System Settings message. Permission denial is reported as
`DeliveryResult::PermissionDenied`; outside wheel events are still consumed
and never become launcher page input.

## Automated verification

`native_input_probe` creates a separate native GUI process:

- Windows: top-level Win32 window plus nested child;
- macOS: an AppKit window containing a real `NSButton` document view inside an
  `NSScrollView`;
- JSONL records include order, button, native deltas, phase/momentum, screen and
  local coordinates, focus/activation, PID, and native window identity.
- macOS UI-state records separately report the button action count,
  `rightMouseDown:` count, and actual clip-view content offset.

`input_routing_scenarios` generates physical-equivalent OS input through a path
different from product delivery:

- Windows generator: `SendInput`; product: targeted `PostMessageW` for wheel
  and separately tagged `SendInput` for confirmed clicks;
- macOS generator: HID `CGEventPost`; product: an active session-tap
  transformation for wheel and HID posting of retained click events only after
  hide.

The product scenarios cover:

1. left click waits for physical up, hides, and delivers one ordered pair;
2. left drag remains visible, moves the page, and delivers no button input;
3. right click waits for physical up, hides, delivers one ordered pair, and
   produces native context-menu dispatch;
4. right drag cancels and delivers nothing;
5. precise vertical wheel remains visible and is delivered once;
6. a test receiver that activates from its wheel handler cannot trigger
   Launchpad auto-hide;
7. hover updates launcher classification and is not delivered;
8. a headful Microsoft Edge page receives a downward wheel and reports an
   actual positive `scrollY`.

The runner also verifies exact wheel sink/root identity, coordinates, modifiers,
launcher PID/window continuity, focus, and Z-order. Windows observes beyond
the bounded receiver-activation interval; macOS compares the real on-screen
window order and native key-window focus before and after wheel delivery.
macOS accepts a raw probe record only when AppKit reports the probe's actual
window number; it no longer rewrites a tagged event's receiver or local
coordinates. A left click must change the real `NSButton` action counter, a
right click must invoke the real `rightMouseDown:`, and a wheel sequence must
change the real `NSScrollView` content offset. Hover and cancelled drags must
leave all semantic UI state unchanged. Raw event order, button, precise delta,
coordinates, modifiers, phase, momentum, receiver PID, and receiver window are
validated independently of those semantic effects.
The
one-shot target/time correlation and signed wheel-coordinate extraction are
covered by pure Windows unit tests. The Windows runner temporarily selects
focus-based OS wheel routing and restores the user's previous system setting,
which removes hosted-desktop hover-routing races without sharing the product's
targeted delivery path. The separate Edge compatibility scenario keeps the
machine's real routing setting, opens a guest-mode isolated profile to avoid
first-run/sync overlays, asserts `scrollY > 0`, and verifies the launcher's
window region, style, HWND, PID, visibility, foreground status, and Z-order are
unchanged. Both OS jobs run in
`.github/workflows/input-routing-e2e.yml`. A macOS machine without the required
TCC approvals for both process identities cannot run the full path: the scenario runner needs
post-event approval to generate independent input, while each Launchpad child
must report its own listen-event and post-event preflight as granted in JSONL.
A hosted runner without generator approval runs an explicit
permission-boundary contract and records that full semantic E2E did not run;
that result is never reported as successful product delivery. Full semantic
native E2E must also be run on an approved macOS machine.

Local commands:

```text
cargo build --bins
target/debug/input_routing_scenarios --request-permissions
target/debug/input_routing_scenarios --permission-status
target/debug/input_routing_scenarios
target/debug/input_routing_scenarios --product
target/debug/input_routing_scenarios --browser-compat
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features
```

On Windows, use `.exe` and backslash path separators for the scenario binary.
