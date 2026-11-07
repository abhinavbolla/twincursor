// Lists one process's on-screen windows as JSON lines: name, layer and bounds in points.
// For checking the remote-control overlay. Usage: swift windows.swift PID
import CoreGraphics
import Foundation

let pid = Int32(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for w in list where (w[kCGWindowOwnerPID as String] as? Int32) == pid {
    let b = w[kCGWindowBounds as String] as? [String: Double] ?? [:]
    let entry: [String: Any] = [
        "name": w[kCGWindowName as String] as? String ?? "",
        "layer": w[kCGWindowLayer as String] as? Int ?? 0,
        "onscreen": w[kCGWindowIsOnscreen as String] as? Bool ?? false,
        "alpha": w[kCGWindowAlpha as String] as? Double ?? 1,
        "x": b["X"] ?? 0, "y": b["Y"] ?? 0, "w": b["Width"] ?? 0, "h": b["Height"] ?? 0,
    ]
    let data = try! JSONSerialization.data(withJSONObject: entry, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!)
}
