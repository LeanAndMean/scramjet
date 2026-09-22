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

func sendKey(_ code: CGKeyCode, _ rawFlags: UInt64) {
    var flags = CGEventFlags(rawValue: rawFlags)
    // iTerm2's Kitty encoder requires device bits on physical modifier events.
    let modifiers: [(CGEventFlags, CGKeyCode)] = [(.maskControl.union(CGEventFlags(rawValue: 0x1)), 59), (.maskShift.union(CGEventFlags(rawValue: 0x2)), 56), (.maskAlternate.union(CGEventFlags(rawValue: 0x20)), 58), (.maskCommand.union(CGEventFlags(rawValue: 0x8)), 55)]
    for (flag, _) in modifiers where !flags.intersection(flag).isEmpty { flags.formUnion(flag) }
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
case "running":
    emit(NSWorkspace.shared.runningApplications.filter { $0.bundleIdentifier?.lowercased() == args[2].lowercased() }.map { $0.processIdentifier })
case "geometry-pid":
    emit(geometry(AXUIElementCreateApplication(pid_t(args[2])!)))
case "activate-pid":
    emit(["activated": NSRunningApplication(processIdentifier: pid_t(args[2])!)?.activate(options: [.activateIgnoringOtherApps]) ?? false])
case "close-windows-pid":
    let pid = pid_t(args[2])!
    if let app = NSRunningApplication(processIdentifier: pid), !app.isTerminated {
        let application = AXUIElementCreateApplication(pid)
        guard let windows = attribute(application, kAXWindowsAttribute) as? [AXUIElement] else { fatalError("Owned window enumeration failed") }
        for window in windows {
            guard let button = attribute(window, kAXCloseButtonAttribute), CFGetTypeID(button) == AXUIElementGetTypeID() else { fatalError("Owned window has no close button") }
            guard AXUIElementPerformAction(button as! AXUIElement, kAXPressAction as CFString) == .success else { fatalError("Owned window close failed") }
        }
        emit(["pid": pid, "closed": windows.count])
    } else {
        emit(["pid": pid, "closed": 0])
    }
case "windows-pid":
    let pid = Int(args[2])!
    let onScreenOnly = args.count == 4 && args[3] == "--on-screen"
    guard args.count == 3 || onScreenOnly else { fatalError("Unknown window query option") }
    let options: CGWindowListOption = onScreenOnly ? [.optionOnScreenOnly, .excludeDesktopElements] : [.optionAll, .excludeDesktopElements]
    guard let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else { fatalError("Window enumeration failed") }
    emit(windows.filter { $0[kCGWindowOwnerPID as String] as? Int == pid }.map { item -> [String: Any] in
        ["pid": pid, "window": item[kCGWindowNumber as String] as? Int ?? 0,
         "layer": item[kCGWindowLayer as String] as? Int ?? 0,
         "bounds": item[kCGWindowBounds as String] as? [String: Any] ?? [:]]
    })
case "press-pid":
    emit(["pressed": pressButton(AXUIElementCreateApplication(pid_t(args[2])!), title: args[3])])
case "press":
    let apps = NSWorkspace.shared.runningApplications.filter { $0.bundleIdentifier?.lowercased() == args[2].lowercased() }
    emit(["applications": apps.map { $0.localizedName ?? "unknown" }, "pressed": apps.contains { pressButton(AXUIElementCreateApplication($0.processIdentifier), title: args[3]) }])
case "key":
    sendKey(CGKeyCode(args[2])!, UInt64(args[3])!)
case "text":
    // Hosted fixtures use the runner's US keyboard layout and synthetic lowercase commands.
    let codes: [Character: CGKeyCode] = ["a": 0, "b": 11, "c": 8, "d": 2, "e": 14, "f": 3, "g": 5, "h": 4, "i": 34, "j": 38, "k": 40, "l": 37, "m": 46, "n": 45, "o": 31, "p": 35, "q": 12, "r": 15, "s": 1, "t": 17, "u": 32, "v": 9, "w": 13, "x": 7, "y": 16, "z": 6, "/": 44, " ": 49]
    for character in args[2] {
        guard let code = codes[character] else { fatalError("Unsupported synthetic character") }
        sendKey(code, 0)
    }
case "activate":
    let app = NSWorkspace.shared.runningApplications.first { $0.bundleIdentifier?.lowercased() == args[2].lowercased() }
    emit(["activated": app?.activate(options: [.activateIgnoringOtherApps]) ?? false])
case "frontmost":
    emit(["bundle": NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "", "pid": NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0])
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
