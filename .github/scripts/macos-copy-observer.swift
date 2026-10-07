import AppKit
import ApplicationServices
import Foundation

let target = URL(fileURLWithPath: CommandLine.arguments[1])
let started = Date()
var samples: [[String: Any]] = []
var ports: [CFMachPort] = []
var created: [[String: Any]] = []
var suppress = false
let capabilities: [String: Any] = [
    "pid": ProcessInfo.processInfo.processIdentifier,
    "accessibilityTrusted": AXIsProcessTrusted(),
    "listenEventAccess": CGPreflightListenEventAccess(),
    "postEventAccess": CGPreflightPostEventAccess(),
    "screenCaptureAccess": CGPreflightScreenCaptureAccess(),
    "os": ProcessInfo.processInfo.operatingSystemVersionString,
    "permissionRequestsMade": false,
]
func record() {
    let data = try! JSONSerialization.data(withJSONObject: ["capabilities": capabilities, "tapCreation": created, "events": samples], options: [.sortedKeys])
    try! data.write(to: target, options: .atomic)
}
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
app.finishLaunching()
let monitor = NSEvent.addGlobalMonitorForEvents(matching: [.scrollWheel]) { event in
    samples.append(["source": "appkit", "deltaY": event.scrollingDeltaY, "seconds": Date().timeIntervalSince(started)])
    record()
}
for (index, location) in [CGEventTapLocation.cgSessionEventTap, .cgAnnotatedSessionEventTap, .cghidEventTap].enumerated() {
    for (kind, options) in [("active", CGEventTapOptions.defaultTap), ("passive", CGEventTapOptions.listenOnly)] {
        let identity = index * 2 + (kind == "active" ? 0 : 1) + 1
        let tap = CGEvent.tapCreate(tap: location, place: .headInsertEventTap, options: options, eventsOfInterest: 1 << CGEventType.scrollWheel.rawValue, callback: { _, type, event, info in
            let identity = Int(bitPattern: info)
            let foreground = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? ""
            let discard = suppress && identity % 2 == 1 && ["com.apple.Terminal", "com.googlecode.iterm2"].contains(foreground)
            samples.append(["source": "cg", "identity": identity, "type": type.rawValue, "discard": discard, "deltaY": event.getIntegerValueField(.scrollWheelEventDeltaAxis1), "targetPid": event.getIntegerValueField(.eventTargetUnixProcessID), "seconds": Date().timeIntervalSince(started)])
            if samples.count > 200 { samples.removeFirst() }
            record()
            return discard ? nil : Unmanaged.passUnretained(event)
        }, userInfo: UnsafeMutableRawPointer(bitPattern: identity))
        created.append(["identity": identity, "location": index, "kind": kind, "created": tap != nil])
        if let tap {
            ports.append(tap)
            CFRunLoopAddSource(CFRunLoopGetCurrent(), CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0), .commonModes)
            CGEvent.tapEnable(tap: tap, enable: true)
        }
    }
}
let timer = Timer(timeInterval: 0.05, repeats: true) { _ in
    suppress = (try? String(contentsOfFile: target.path + ".control", encoding: .utf8)) == "suppress"
}
RunLoop.current.add(timer, forMode: .common)
let stopTimer = Timer(timeInterval: 180, repeats: false) { _ in
    for port in ports { CGEvent.tapEnable(tap: port, enable: false) }
    record()
    exit(0)
}
RunLoop.current.add(stopTimer, forMode: .common)
record()
app.run()
