// A small plain window made for a bench to share (bench/engine-e2e.sh), so a bench never
// captures a window that belongs to someone. It never takes focus or activates, sits behind
// every other window, and writes its window number (the CGWindowID screen capture uses) to
// OUT once it is on screen. It stays up until killed.
//
// It isn't part of tc-spike on purpose: TwinCursor's own windows are never offered for sharing
// and are left out of every capture (media::capture::select::is_twincursor), so a tc-spike
// window couldn't be shared without weakening that rule.
//
// Usage: swiftc -O -o test-window test-window.swift && ./test-window OUT [TITLE]
import AppKit

let args = CommandLine.arguments
guard args.count >= 2 else {
    FileHandle.standardError.write("usage: test-window OUT [TITLE]\n".data(using: .utf8)!)
    exit(2)
}
let title = args.count >= 3 ? args[2] : "TwinCursor bench window"

let app = NSApplication.shared
// No Dock icon, no menu bar, and never the active app: whatever has focus keeps it.
app.setActivationPolicy(.accessory)

let window = NSWindow(
    contentRect: NSRect(x: 0, y: 0, width: 240, height: 160),
    styleMask: [.titled],
    backing: .buffered,
    defer: false)
window.title = title
window.isReleasedWhenClosed = false
window.backgroundColor = .systemTeal
// Behind everything: screen capture of one window sees it whatever covers it.
window.orderBack(nil)

do {
    try "\(window.windowNumber)\n".write(toFile: args[1], atomically: true, encoding: .utf8)
} catch {
    FileHandle.standardError.write("can't write \(args[1]): \(error)\n".data(using: .utf8)!)
    exit(1)
}
signal(SIGTERM) { _ in exit(0) }
signal(SIGINT) { _ in exit(0) }
app.run()
