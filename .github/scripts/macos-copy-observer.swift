import AppKit
import ApplicationServices
import Foundation

let target = URL(fileURLWithPath: CommandLine.arguments[1])
let started = Date()
var samples: [[String: Any]] = []
var taps: [[String: Any]] = []
var previous: [UInt64] = []
let capabilities: [String: Any] = [
    "pid": ProcessInfo.processInfo.processIdentifier,
    "accessibilityTrusted": AXIsProcessTrusted(),
    "listenEventAccess": CGPreflightListenEventAccess(),
    "postEventAccess": CGPreflightPostEventAccess(),
    "screenCaptureAccess": CGPreflightScreenCaptureAccess(),
    "os": ProcessInfo.processInfo.operatingSystemVersionString,
    "permissionRequestsMade": false,
]
var tapCreated = false

func record() {
    let data = try! JSONSerialization.data(withJSONObject: ["capabilities": capabilities, "eventTapCreated": tapCreated, "samples": samples, "tapEvents": taps], options: [.sortedKeys])
    try! data.write(to: target, options: .atomic)
}

let mask = (1 << CGEventType.keyDown.rawValue) | (1 << CGEventType.keyUp.rawValue) | (1 << CGEventType.flagsChanged.rawValue)
let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .listenOnly, eventsOfInterest: CGEventMask(mask), callback: { _, type, event, _ in
    let key = event.getIntegerValueField(.keyboardEventKeycode)
    if key == 8 || type == .flagsChanged {
        taps.append(["seconds": Date().timeIntervalSince(started), "type": type.rawValue, "keyCode": key, "flags": event.flags.rawValue])
        if taps.count > 100 { taps.removeFirst() }
        record()
    }
    return Unmanaged.passUnretained(event)
}, userInfo: nil)
if let tap {
    tapCreated = true
    let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
    CFRunLoopAddSource(CFRunLoopGetCurrent(), source, .commonModes)
    CGEvent.tapEnable(tap: tap, enable: true)
}
let timer = Timer(timeInterval: 0.002, repeats: true) { _ in
    let c = CGEventSource.keyState(.combinedSessionState, key: 8)
    let hidC = CGEventSource.keyState(.hidSystemState, key: 8)
    let flags = CGEventSource.flagsState(.combinedSessionState).rawValue
    let hidFlags = CGEventSource.flagsState(.hidSystemState).rawValue
    let current = [c ? UInt64(1) : 0, hidC ? UInt64(1) : 0, flags, hidFlags]
    if current != previous {
        previous = current
        let foreground = NSWorkspace.shared.frontmostApplication
        samples.append(["seconds": Date().timeIntervalSince(started), "cDown": c, "hidCDown": hidC, "flags": flags, "hidFlags": hidFlags, "foregroundPid": foreground?.processIdentifier ?? -1, "foregroundBundle": foreground?.bundleIdentifier ?? "unknown"])
        if samples.count > 100 { samples.removeFirst() }
        record()
    }
}
RunLoop.current.add(timer, forMode: .common)
record()
RunLoop.current.run(until: started.addingTimeInterval(90))
record()
