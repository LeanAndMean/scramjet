import AppKit
import ApplicationServices
import Carbon
import Foundation

let target = URL(fileURLWithPath: CommandLine.arguments[1])
let started = Date()
var events: [[String: Any]] = []
var hotKey: EventHotKeyRef?
var requested = ""
var registration: OSStatus = -1
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
    let data = try! JSONSerialization.data(withJSONObject: ["capabilities": capabilities, "registration": registration, "armed": hotKey != nil, "events": events], options: [.sortedKeys])
    try! data.write(to: target, options: .atomic)
}
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
app.finishLaunching()
var eventType = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
var handler: EventHandlerRef?
let installed = InstallEventHandler(GetApplicationEventTarget(), { _, event, _ in
    var key = EventHotKeyID()
    let result = GetEventParameter(event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil, MemoryLayout<EventHotKeyID>.size, nil, &key)
    events.append(["seconds": Date().timeIntervalSince(started), "kind": "hotkey", "lease": requested, "result": result, "id": key.id, "foregroundPid": NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0, "foregroundBundle": NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? ""])
    record()
    return noErr
}, 1, &eventType, nil, &handler)
events.append(["kind": "handler", "status": installed])
let timer = Timer(timeInterval: 0.02, repeats: true) { _ in
    let command = (try? String(contentsOfFile: target.path + ".control", encoding: .utf8)) ?? ""
    if command == requested { return }
    requested = command
    if let ref = hotKey { UnregisterEventHotKey(ref); hotKey = nil }
    if command.hasPrefix("arm") {
        registration = RegisterEventHotKey(UInt32(kVK_ANSI_C), UInt32(cmdKey), EventHotKeyID(signature: 0x5343524D, id: 608), GetApplicationEventTarget(), 0, &hotKey)
    }
    record()
}
RunLoop.current.add(timer, forMode: .common)
let stopTimer = Timer(timeInterval: 180, repeats: false) { _ in
    if let ref = hotKey { UnregisterEventHotKey(ref) }
    record()
    exit(0)
}
RunLoop.current.add(stopTimer, forMode: .common)
record()
app.run()
