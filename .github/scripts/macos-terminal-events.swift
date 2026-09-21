import AppKit
import ApplicationServices
import Foundation

func emit(_ value: Any) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    print(String(data: data, encoding: .utf8)!)
}

func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success ? value : nil
}

func geometry(_ element: AXUIElement, depth: Int = 0) -> [[String: Any]] {
    if depth > 10 { return [] }
    var result: [[String: Any]] = []
    if let role = attribute(element, kAXRoleAttribute) as? String,
       let position = attribute(element, kAXPositionAttribute),
       let size = attribute(element, kAXSizeAttribute),
       CFGetTypeID(position) == AXValueGetTypeID(), CFGetTypeID(size) == AXValueGetTypeID() {
        var point = CGPoint.zero
        var dimensions = CGSize.zero
        AXValueGetValue(position as! AXValue, .cgPoint, &point)
        AXValueGetValue(size as! AXValue, .cgSize, &dimensions)
        if ["AXWindow", "AXScrollArea", "AXTextArea"].contains(role) {
            var item: [String: Any] = ["role": role, "x": point.x, "y": point.y, "width": dimensions.width, "height": dimensions.height]
            if role == "AXTextArea", let text = attribute(element, kAXValueAttribute) as? String {
                let marker = (text as NSString).range(of: "ROW-001")
                if marker.location != NSNotFound {
                    var range = CFRange(location: marker.location, length: 1)
                    let parameter = AXValueCreate(.cfRange, &range)!
                    var bounds: CFTypeRef?
                    if AXUIElementCopyParameterizedAttributeValue(element, kAXBoundsForRangeParameterizedAttribute as CFString, parameter, &bounds) == .success,
                       let bounds = bounds, CFGetTypeID(bounds) == AXValueGetTypeID() {
                        var rect = CGRect.zero
                        AXValueGetValue(bounds as! AXValue, .cgRect, &rect)
                        item["firstCell"] = ["x": rect.origin.x, "y": rect.origin.y, "width": rect.width, "height": rect.height]
                    }
                }
            }
            result.append(item)
        }
    }
    for child in attribute(element, kAXChildrenAttribute) as? [AXUIElement] ?? [] {
        result += geometry(child, depth: depth + 1)
    }
    return result
}

func pressButton(_ element: AXUIElement, title: String, depth: Int = 0) -> Bool {
    if depth > 12 { return false }
    if ["AXButton", "AXCheckBox"].contains(attribute(element, kAXRoleAttribute) as? String ?? ""),
       [kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute].contains(where: { attribute(element, $0) as? String == title }) {
        return AXUIElementPerformAction(element, kAXPressAction as CFString) == .success
    }
    for child in attribute(element, kAXChildrenAttribute) as? [AXUIElement] ?? [] {
        if pressButton(child, title: title, depth: depth + 1) { return true }
    }
    return false
}

let args = CommandLine.arguments
switch args[1] {
case "capabilities":
    emit(["accessibility": AXIsProcessTrusted(), "postEvents": CGPreflightPostEventAccess(),
          "screenCapture": CGPreflightScreenCaptureAccess()])
case "geometry", "resize":
    guard let app = NSWorkspace.shared.runningApplications.first(where: { $0.bundleIdentifier?.lowercased() == (args.count > 2 ? args[2].lowercased() : "com.apple.terminal") }) else {
        fatalError("Terminal is not running")
    }
    let application = AXUIElementCreateApplication(app.processIdentifier)
    if args[1] == "resize" {
        guard let window = (attribute(application, kAXWindowsAttribute) as? [AXUIElement])?.first else { fatalError("No window") }
        var size = CGSize(width: Double(args[3])!, height: Double(args[4])!)
        let result = AXUIElementSetAttributeValue(window, kAXSizeAttribute as CFString, AXValueCreate(.cgSize, &size)!)
        emit(["result": result.rawValue])
    } else {
        emit(geometry(application))
    }
case "press-pid":
    emit(["pressed": pressButton(AXUIElementCreateApplication(pid_t(args[2])!), title: args[3])])
case "press":
    let apps = NSWorkspace.shared.runningApplications.filter { $0.bundleIdentifier?.lowercased() == args[2].lowercased() }
    emit(["applications": apps.map { $0.localizedName ?? "unknown" }, "pressed": apps.contains { pressButton(AXUIElementCreateApplication($0.processIdentifier), title: args[3]) }])
case "key":
    let code = CGKeyCode(args[2])!
    let flags = CGEventFlags(rawValue: UInt64(args[3])!)
    let modifiers: [(CGEventFlags, CGKeyCode)] = [(.maskControl, 59), (.maskShift, 56), (.maskAlternate, 58), (.maskCommand, 55)]
    var active = CGEventFlags()
    for (flag, modifier) in modifiers where flags.contains(flag) {
        active.insert(flag)
        let event = CGEvent(keyboardEventSource: nil, virtualKey: modifier, keyDown: true)!
        event.type = .flagsChanged
        event.flags = active
        event.post(tap: .cghidEventTap)
        usleep(20_000)
    }
    for down in [true, false] {
        let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: down)!
        event.flags = flags
        event.post(tap: .cghidEventTap)
        usleep(20_000)
    }
    for (flag, modifier) in modifiers.reversed() where flags.contains(flag) {
        active.remove(flag)
        let event = CGEvent(keyboardEventSource: nil, virtualKey: modifier, keyDown: false)!
        event.type = .flagsChanged
        event.flags = active
        event.post(tap: .cghidEventTap)
        usleep(20_000)
    }
case "mouse":
    let point = CGPoint(x: Double(args[3])!, y: Double(args[4])!)
    let actions: [String: (CGEventType, CGMouseButton)] = [
        "move": (.mouseMoved, .left), "down": (.leftMouseDown, .left),
        "drag": (.leftMouseDragged, .left), "up": (.leftMouseUp, .left),
        "rightDown": (.rightMouseDown, .right), "rightUp": (.rightMouseUp, .right)
    ]
    let (kind, button) = actions[args[2]]!
    let event = CGEvent(mouseEventSource: nil, mouseType: kind, mouseCursorPosition: point, mouseButton: button)!
    event.setIntegerValueField(.mouseEventClickState, value: 1)
    event.post(tap: .cghidEventTap)
case "wheel":
    let event = CGEvent(scrollWheelEvent2Source: nil, units: .line, wheelCount: 1,
                        wheel1: Int32(args[2])!, wheel2: 0, wheel3: 0)!
    event.post(tap: .cghidEventTap)
default:
    fatalError("Unknown event operation")
}
