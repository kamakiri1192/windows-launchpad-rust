// Native compositor QA: a known background, or one Escape key via the same
// CoreGraphics HID event path as the native input scenario runner.
import AppKit
import CoreGraphics

if CommandLine.arguments.contains("--escape") {
    guard CGPreflightPostEventAccess() else {
        fputs("Accessibility/post-event permission is required\n", stderr)
        exit(1)
    }
    for down in [true, false] {
        CGEvent(keyboardEventSource: CGEventSource(stateID: .hidSystemState),
                virtualKey: 53, keyDown: down)?.post(tap: .cghidEventTap)
        Thread.sleep(forTimeInterval: 0.05)
    }
    exit(0)
}

final class BackdropView: NSView {
    var phase: CGFloat = 0.59
    override var isOpaque: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        NSColor(calibratedHue: phase, saturation: 0.72, brightness: 0.78, alpha: 1).setFill()
        dirtyRect.fill()
        NSColor(calibratedWhite: 1, alpha: 0.14).setFill()
        let offset = (phase * 1080).truncatingRemainder(dividingBy: 360)
        var x = -360 + offset
        while x < bounds.maxX + 180 {
            NSBezierPath(rect: NSRect(x: x, y: 0, width: 180, height: bounds.height)).fill()
            x += 360
        }
    }
}

final class Delegate: NSObject, NSApplicationDelegate {
    var windows: [NSWindow] = []
    var timer: DispatchSourceTimer?
    var activity: NSObjectProtocol?
    func applicationDidFinishLaunching(_ notification: Notification) {
        for screen in NSScreen.screens {
            let window = NSWindow(contentRect: screen.frame, styleMask: [.borderless],
                                  backing: .buffered, defer: false, screen: screen)
            window.contentView = BackdropView(frame: screen.frame)
            window.isOpaque = true
            window.ignoresMouseEvents = true
            window.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
            // Above ordinary apps, below PROFILE_KEEP_VISIBLE's floating launcher.
            window.level = NSWindow.Level(rawValue: 2)
            window.orderFrontRegardless()
            windows.append(window)
        }
        guard CommandLine.arguments.contains("--dynamic") else { return }
        activity = ProcessInfo.processInfo.beginActivity(
            options: [.userInitiatedAllowingIdleSystemSleep, .latencyCritical],
            reason: "Launchpad reveal color QA")
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now(), repeating: .nanoseconds(16_666_667), leeway: .nanoseconds(0))
        timer.setEventHandler { [weak self] in
            for window in self?.windows ?? [] {
                guard let view = window.contentView as? BackdropView else { continue }
                view.phase = (view.phase + 1.0 / 360.0).truncatingRemainder(dividingBy: 1)
                view.needsDisplay = true
                view.displayIfNeeded()
            }
        }
        timer.resume()
        self.timer = timer
    }
}

let app = NSApplication.shared
let delegate = Delegate()
app.setActivationPolicy(.accessory)
app.delegate = delegate
app.run()
