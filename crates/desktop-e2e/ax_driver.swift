// AX driver: a persistent subprocess that drives the Agent Deck app's
// WKWebView via the macOS Accessibility API. Rust spawns this, then issues
// line-delimited JSON requests on stdin and reads line-delimited JSON
// responses on stdout.
//
// Why a swift subprocess: the Accessibility API lives in ApplicationServices
// (C framework). Calling it from Rust requires fragile FFI bindings; a small
// swift helper is the idiomatic, dependency-free bridge.
//
// Protocol: each request is one JSON line { "id": <int>, "op": "...", ... }.
// Each response is one JSON line { "id": <int>, "ok": <bool>, ... }.

import Cocoa
import ApplicationServices

// MARK: - AX tree helpers

/// Find the running Agent Deck app (by exact localizedName match).
func findApp() -> pid_t? {
    let apps = NSWorkspace.shared.runningApplications.filter {
        $0.localizedName == "Agent Deck" && $0.activationPolicy == .regular
    }
    return apps.first?.processIdentifier
}

/// Recursively locate the AXWebArea (the WKWebView content root).
func findWebArea(_ element: AXUIElement) -> AXUIElement? {
    var roleRef: CFTypeRef?
    AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &roleRef)
    if (roleRef as? String) == "AXWebArea" { return element }

    var kidsRef: CFTypeRef?
    AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &kidsRef)
    if let kids = kidsRef as? [AXUIElement] {
        for k in kids {
            if let found = findWebArea(k) { return found }
        }
    }
    return nil
}

func webAreaOfApp(_ pid: pid_t) -> AXUIElement? {
    let appEl = AXUIElementCreateApplication(pid)
    var windowsRef: CFTypeRef?
    AXUIElementCopyAttributeValue(appEl, kAXWindowsAttribute as CFString, &windowsRef)
    guard let wins = windowsRef as? [AXUIElement] else { return nil }
    for w in wins {
        if let web = findWebArea(w) { return web }
    }
    return nil
}

/// Read a string attribute, defaulting to "".
func strAttr(_ e: AXUIElement, _ key: String) -> String {
    var ref: CFTypeRef?
    AXUIElementCopyAttributeValue(e, key as CFString, &ref)
    if let s = ref as? String { return s }
    // AXCheckBox AXValue is an NSNumber (0/1); normalize to "0"/"1".
    if let n = ref as? NSNumber { return n.boolValue ? "1" : "0" }
    return ""
}

/// Match an element against a CSS-like locator spec. Supported forms:
///   "tag"                 → role matches (map: button→AXButton, input[type=checkbox]→AXCheckBox)
///   "#id"                 → AXDOMIdentifier == id
///   "tag#id"              → both
///   ".class"              → AXDOMClassList contains class
///   "[aria-label=val]"    → AXDescription == val (WKWebView exposes aria-label via AXDescription)
///   "tag[aria-label=val]" → both role and aria-label
func matches(_ e: AXUIElement, _ selector: String) -> Bool {
    let role = strAttr(e, "AXRole")
    let domId = strAttr(e, "AXDOMIdentifier")
    let domCls = strAttr(e, "AXDOMClassList")
    let desc = strAttr(e, "AXDescription")

    // Parse selector into tag / id / class / attr parts.
    var s = selector
    var tag = "", id = "", cls = "", ariaLabel = ""
    // Extract [attr=val]
    if let range = s.range(of: #"\[([^\]]+)\]"#, options: .regularExpression) {
        let inside = String(s[range]).dropFirst().dropLast() // strip [ ]
        let parts = inside.split(separator: "=", maxSplits: 1)
        if parts.count == 2 {
            ariaLabel = String(parts[1]).trimmingCharacters(in: CharacterSet(charactersIn: "'\""))
            s.removeSubrange(range)
        }
    }
    if let dot = s.firstIndex(of: ".") {
        cls = String(s[s.index(after: dot)...])
        s = String(s[..<dot])
    }
    if let hash = s.firstIndex(of: "#") {
        id = String(s[s.index(after: hash)...])
        s = String(s[..<hash])
    }
    tag = s

    // Tag → role mapping. `input` is special: WKWebView renders checkboxes as
    // AXCheckBox and text inputs as AXTextField, but neither carries a DOM
    // class in the AX tree — so we match `input` against either role and
    // disambiguate by id when needed.
    let expectedRole: String? = {
        switch tag {
        case "button": return "AXButton"
        case "input":
            if role == "AXCheckBox" || role == "AXTextField" { return role }
            return domCls.contains("checkbox") || selector.contains("checkbox") ? "AXCheckBox" : "AXTextField"
        case "div", "span", "p", "label": return "AXGroup"
        default: return nil
        }
    }()
    if let r = expectedRole, r != role && !(tag == "div" && role == "AXGroup") {
        if !(tag == "span" || tag == "p" || tag == "label") || role != "AXStaticText" {
            return false
        }
    }

    if !id.isEmpty && id != domId { return false }
    if !cls.isEmpty && !domCls.contains(cls) { return false }
    if !ariaLabel.isEmpty {
        // Support a trailing "*" as a prefix wildcard: "key-*" matches "key-0".
        if ariaLabel.hasSuffix("*") {
            let prefix = String(ariaLabel.dropLast())
            if !desc.hasPrefix(prefix) { return false }
        } else if desc != ariaLabel {
            return false
        }
    }
    return true
}

/// Depth-first search for the first element matching `selector` under `root`.
func findFirst(_ root: AXUIElement, _ selector: String) -> AXUIElement? {
    if matches(root, selector) { return root }
    var kidsRef: CFTypeRef?
    AXUIElementCopyAttributeValue(root, kAXChildrenAttribute as CFString, &kidsRef)
    if let kids = kidsRef as? [AXUIElement] {
        for k in kids {
            if let f = findFirst(k, selector) { return f }
        }
    }
    return nil
}

/// Count elements matching `selector` under `root`.
func countAll(_ root: AXUIElement, _ selector: String) -> Int {
    var n = matches(root, selector) ? 1 : 0
    var kidsRef: CFTypeRef?
    AXUIElementCopyAttributeValue(root, kAXChildrenAttribute as CFString, &kidsRef)
    if let kids = kidsRef as? [AXUIElement] {
        for k in kids { n += countAll(k, selector) }
    }
    return n
}

/// Snapshot the webview root fresh on every call (DOM is rebuilt by innerHTML
/// after each paint(), so cached AXUIElement handles may go stale).
func freshWebArea() -> AXUIElement? {
    guard let pid = findApp() else { return nil }
    return webAreaOfApp(pid)
}

// MARK: - Wait helper (block with sleep up to `ms` for the locator to appear)

func wait(_ selector: String, ms: Int) -> AXUIElement? {
    let deadline = Date().addingTimeInterval(TimeInterval(ms) / 1000.0)
    while Date() < deadline {
        if let web = freshWebArea(), let el = findFirst(web, selector) {
            return el
        }
        usleep(150_000)
    }
    return nil
}

// MARK: - Request dispatch

func respond(_ id: Int, _ payload: [String: Any]) {
    var line = payload
    line["id"] = id
    if let data = try? JSONSerialization.data(withJSONObject: line),
       let s = String(data: data, encoding: .utf8) {
        print(s)
        fflush(stdout)
    }
}

func respondOk(_ id: Int, _ extra: [String: Any] = [:]) {
    var p = extra
    p["ok"] = true
    respond(id, p)
}

func respondErr(_ id: Int, _ msg: String) {
    respond(id, ["ok": false, "error": msg])
}

func handle(_ req: [String: Any]) {
    guard let id = req["id"] as? Int, let op = req["op"] as? String else {
        respondErr(-1, "missing id/op")
        return
    }

    switch op {
    case "ping":
        respondOk(id)

    case "wait":
        let selector = req["selector"] as? String ?? ""
        let ms = req["ms"] as? Int ?? 8000
        if wait(selector, ms: ms) != nil {
            respondOk(id)
        } else {
            respondErr(id, "timeout waiting for '\(selector)'")
        }

    case "count":
        let selector = req["selector"] as? String ?? ""
        guard let web = freshWebArea() else { respondErr(id, "no webview"); return }
        respondOk(id, ["count": countAll(web, selector)])

    case "click":
        let selector = req["selector"] as? String ?? ""
        let ms = req["ms"] as? Int ?? 8000
        guard let el = wait(selector, ms: ms) else { respondErr(id, "not found: \(selector)"); return }
        let r = AXUIElementPerformAction(el, kAXPressAction as CFString)
        if r == .success {
            respondOk(id)
        } else {
            respondErr(id, "press failed (\(r.rawValue))")
        }

    case "get_value":
        let selector = req["selector"] as? String ?? ""
        let ms = req["ms"] as? Int ?? 8000
        guard let el = wait(selector, ms: ms) else { respondErr(id, "not found: \(selector)"); return }
        respondOk(id, ["value": strAttr(el, "AXValue"), "title": strAttr(el, "AXTitle")])

    case "alive":
        respondOk(id, ["running": freshWebArea() != nil])

    default:
        respondErr(id, "unknown op: \(op)")
    }
}

// MARK: - Read loop

while let line = readLine() {
    let trimmed = line.trimmingCharacters(in: .whitespaces)
    if trimmed.isEmpty { continue }
    guard let data = trimmed.data(using: .utf8),
          let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        respondErr(-1, "bad json: \(trimmed)")
        continue
    }
    handle(obj)
}
