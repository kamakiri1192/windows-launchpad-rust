import AppKit

final class AnimatedBackdropView: NSView {
    private var phase: CGFloat = 0

    override var isOpaque: Bool { true }

    func advance() {
        phase = (phase + 1.0 / 360.0).truncatingRemainder(dividingBy: 1.0)
        needsDisplay = true
        displayIfNeeded()
    }

    override func draw(_ dirtyRect: NSRect) {
        NSColor(
            calibratedHue: phase,
            saturation: 0.72,
            brightness: 0.78,
            alpha: 1.0
        ).setFill()
        dirtyRect.fill()

        let stripeWidth: CGFloat = 180
        let offset = phase * stripeWidth * 6
        NSColor(calibratedWhite: 1.0, alpha: 0.14).setFill()
        var x = -stripeWidth * 2 + offset.truncatingRemainder(dividingBy: stripeWidth * 2)
        while x < bounds.maxX + stripeWidth {
            NSBezierPath(rect: NSRect(x: x, y: 0, width: stripeWidth, height: bounds.height)).fill()
            x += stripeWidth * 2
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    private var timer: DispatchSourceTimer?
    private var activity: NSObjectProtocol?

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard let screen = NSScreen.main else {
            NSApp.terminate(nil)
            return
        }

        let view = AnimatedBackdropView(frame: screen.frame)
        let window = NSWindow(
            contentRect: screen.frame,
            styleMask: [.borderless],
            backing: .buffered,
            defer: false,
            screen: screen
        )
        window.contentView = view
        window.backgroundColor = .black
        window.isOpaque = true
        window.ignoresMouseEvents = true
        window.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary]
        window.level = .normal
        window.orderFront(nil)
        self.window = window

        // A Foundation Timer in an inactive accessory app is aggressively
        // coalesced by App Nap, which makes a nominally 60 Hz test background
        // fall to about 14 Hz. Keep the QA workload latency-sensitive and use
        // a GCD timer so the source remains dynamic while Launchpad is active.
        activity = ProcessInfo.processInfo.beginActivity(
            options: [.userInitiatedAllowingIdleSystemSleep, .latencyCritical],
            reason: "Launchpad animated-backdrop performance fixture"
        )
        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(
            deadline: .now(),
            repeating: .nanoseconds(16_666_667),
            leeway: .nanoseconds(0)
        )
        timer.setEventHandler {
            view.advance()
        }
        timer.resume()
        self.timer = timer
    }

    func applicationWillTerminate(_ notification: Notification) {
        timer?.cancel()
        if let activity {
            ProcessInfo.processInfo.endActivity(activity)
        }
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.setActivationPolicy(.accessory)
app.delegate = delegate
app.run()
