// Probe-owned activator for scripts/probe.py's `activate_other` step (GPUI Phase 0
// R5, spec O4). A real loss of key status needs another app to become active; the
// probe must not launch (or leave running) the user's apps (Finder, Calculator) to get
// one, so it launches this instead: a minimal AppKit app that makes itself the
// active app with a 1-point key window, prints `active <pid>` once AppKit confirms it
// is active and key, and then waits. It exits when its stdin closes (the probe ends
// the step, or the probe itself dies) or on SIGTERM, so it can never outlive the
// run. A real deactivation test necessarily takes focus from the user's app briefly.
//
// Exit 3 with `inactive` on stdout if AppKit did not make it active within 5 s
// (the probe reports that as ENV-BLOCKED, not a product failure).

import AppKit
import Foundation

final class KeyWindow: NSWindow {
    // Borderless windows refuse key status by default; this one exists to take it.
    override var canBecomeKey: Bool { true }
}

final class Activator: NSObject, NSApplicationDelegate {
    var window: KeyWindow!
    var reported = false

    func applicationDidFinishLaunching(_ note: Notification) {
        window = KeyWindow(contentRect: NSRect(x: 0, y: 0, width: 1, height: 1),
                           styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        let nc = NotificationCenter.default
        nc.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in self.check() }
        nc.addObserver(forName: NSWindow.didBecomeKeyNotification, object: window, queue: .main) { _ in self.check() }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
            if !self.reported {
                print("inactive")
                fflush(stdout)
                exit(3)
            }
        }
        check()
    }

    func check() {
        guard !reported, NSApp.isActive, window.isKeyWindow else { return }
        reported = true
        print("active \(getpid())")
        fflush(stdout)
    }
}

// Lifetime is tied to the probe: stdin EOF means the step (or the probe) is done.
Thread.detachNewThread {
    while readLine() != nil {}
    exit(0)
}
signal(SIGTERM) { _ in exit(0) }

let app = NSApplication.shared
// Regular, so it can be the frontmost app (an accessory app cannot take activation
// from a regular one reliably).
app.setActivationPolicy(.regular)
let delegate = Activator()
app.delegate = delegate
app.run()
