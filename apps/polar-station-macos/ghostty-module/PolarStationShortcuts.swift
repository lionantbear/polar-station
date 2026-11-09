#if POLAR_STATION
    import AppKit
    import CPolarStationDesktopClient

    enum PolarStationShortcutAction: Equatable {
        case controlCenterToggle
        case workspaceModeToggle
        case focusPrevious
        case focusNext
        case focusPaneNumber(Int)
        case newShell
        case newCodex
        case openIdeaTree
        case resumeCodex
        case openInquiry
        case insertIdeaNode
        case editIdeaInstruction
        case newWorkspace
        case renameWorkspace
        case refreshSessions
        case closeFocusedPane
    }

    /// Native event adapter; action keys and copy come from the shared Rust catalog.
    @MainActor
    final class PolarStationShortcuts: NSObject {
        typealias Dispatch = @MainActor (PolarStationShortcutAction) -> Bool
        typealias BadgesVisible = @MainActor (Bool) -> Void

        private let dispatch: Dispatch
        private let badgesVisible: BadgesVisible
        private var keyMonitor: Any?
        private var flagsMonitor: Any?
        private var modifiersHeld = false
        private var hintView: PolarStationShortcutHints?

        init(dispatch: @escaping Dispatch, badgesVisible: @escaping BadgesVisible) {
            self.dispatch = dispatch
            self.badgesVisible = badgesVisible
            super.init()
        }

        func install(menuTarget: AnyObject, toggleSelector: Selector) {
            let item = NSMenuItem(
                title: "Polar Station…",
                action: toggleSelector,
                keyEquivalent: String(cString: polar_station_shortcut_at(0).key).lowercased()
            )
            item.keyEquivalentModifierMask = [.command, .option]
            item.target = menuTarget
            NSApp.mainMenu?.items.first?.submenu?.insertItem(item, at: 1)

            installContentMenu()

            keyMonitor = NSEvent.addLocalMonitorForEvents(
                matching: .keyDown
            ) { [weak self] event in
                guard let self, NSApp.modalWindow == nil,
                      let window = NSApp.keyWindow, window.attachedSheet == nil,
                      window.windowController is TerminalController,
                      let match = Self.match(event)
                else { return event }
                if let input = window.firstResponder as? NSTextInputClient, input.hasMarkedText() {
                    return event
                }
                if event.isARepeat && !match.repeatable {
                    return nil
                }
                let handled = dispatch(match.action)
                if (match.action == .insertIdeaNode || match.action == .editIdeaInstruction) && !handled { return event }
                return nil // A resolved Station action is consumed even when disabled.
            }
            flagsMonitor = NSEvent.addLocalMonitorForEvents(
                matching: .flagsChanged
            ) { [weak self] event in
                guard let self else { return event }
                let flags = event.modifierFlags
                    .intersection(.deviceIndependentFlagsMask)
                modifiersHeld = flags.subtracting([.numericPad, .function, .capsLock]) == [.command, .option]
                showHints(modifiersHeld && NSApp.isActive && NSApp.keyWindow != nil)
                return event
            }
            NotificationCenter.default.addObserver(
                self, selector: #selector(windowResignedKey(_:)),
                name: NSWindow.didResignKeyNotification, object: nil
            )
            NotificationCenter.default.addObserver(
                self, selector: #selector(windowBecameKey(_:)),
                name: NSWindow.didBecomeKeyNotification, object: nil
            )
            NotificationCenter.default.addObserver(
                self, selector: #selector(applicationResignedActive(_:)),
                name: NSApplication.didResignActiveNotification, object: nil
            )
        }

        private func installContentMenu() {
            for index in 0..<polar_station_shortcut_count() {
                let entry = polar_station_shortcut_at(index)
                guard entry.action == 12 || entry.action == 13 || entry.action == 14 else { continue }
                let item = NSMenuItem(title: String(cString: entry.label),
                    action: #selector(contentMenu(_:)), keyEquivalent: String(cString: entry.key).lowercased())
                item.keyEquivalentModifierMask = [.command, .option]
                item.target = self; item.tag = Int(entry.action)
                NSApp.mainMenu?.items.first?.submenu?.insertItem(item, at: 2)
            }
        }
        @objc private func contentMenu(_ sender: NSMenuItem) {
            _ = dispatch(sender.tag == 12 ? .openIdeaTree : sender.tag == 13 ? .resumeCodex : .openInquiry)
        }

        @objc private func windowResignedKey(_ note: Notification) {
            showHints(false)
        }

        @objc private func windowBecameKey(_ note: Notification) {
            modifiersHeld = NSEvent.modifierFlags.intersection([.command, .option, .control, .shift]) == [.command, .option]
            showHints(modifiersHeld && NSApp.isActive)
        }

        @objc private func applicationResignedActive(_ note: Notification) {
            modifiersHeld = false
            showHints(false)
        }

        func stop() {
            if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
            if let flagsMonitor { NSEvent.removeMonitor(flagsMonitor) }
            NotificationCenter.default.removeObserver(self)
            keyMonitor = nil
            flagsMonitor = nil
            modifiersHeld = false
            showHints(false)
        }

        private func showHints(_ visible: Bool) {
            hintView?.removeFromSuperview()
            hintView = nil
            guard visible, NSApp.modalWindow == nil, let window = NSApp.keyWindow,
                  window.attachedSheet == nil,
                  let content = window.contentView,
                  window.windowController is TerminalController else {
                badgesVisible(false)
                return
            }
            badgesVisible(true)
            let view = PolarStationShortcutHints(frame: content.bounds)
            view.autoresizingMask = [.width, .height]
            content.addSubview(view, positioned: .above, relativeTo: nil)
            hintView = view
        }

        private struct Match {
            let action: PolarStationShortcutAction
            let repeatable: Bool
        }

        private static func match(_ event: NSEvent) -> Match? {
            let normalizedModifiers = event.modifierFlags
                .intersection(.deviceIndependentFlagsMask)
                .subtracting(.numericPad)
                .subtracting(.function)
                .subtracting(.capsLock)

            guard normalizedModifiers == [.command, .option] else { return nil }
            let index = polar_station_shortcut_lookup(1, UInt32(event.keyCode))
            guard index != 0 else { return nil }
            let entry = polar_station_shortcut_at(Int(index - 1))
            let action: PolarStationShortcutAction
            switch entry.action {
            case 1: action = .controlCenterToggle
            case 2: action = .workspaceModeToggle
            case 3: action = .focusPrevious
            case 4: action = .focusNext
            case 5: action = .focusPaneNumber(Int(entry.number))
            case 6: action = .newShell
            case 7: action = .newCodex
            case 8: action = .newWorkspace
            case 9: action = .renameWorkspace
            case 10: action = .refreshSessions
            case 11: action = .closeFocusedPane
            case 12: action = .openIdeaTree
            case 13: action = .resumeCodex
            case 14: action = .openInquiry
            case 15: action = .insertIdeaNode
            case 16: action = .editIdeaInstruction
            default: return nil
            }
            return Match(action: action, repeatable: entry.repeatable)
        }
    }
    /// Draw-only overlay: focus and mouse hit testing remain with the terminal.
    @MainActor
    private final class PolarStationShortcutHints: NSView {
        override var isFlipped: Bool { true }
        override func hitTest(_ point: NSPoint) -> NSView? { nil }
        override var acceptsFirstResponder: Bool { false }
        override func draw(_ dirtyRect: NSRect) {
            NSColor(calibratedRed: 8/255, green: 18/255, blue: 32/255, alpha: 0.20).setFill()
            bounds.fill()
            var rows: [(String, String)] = []
            for index in 0..<polar_station_shortcut_count() {
                let entry = polar_station_shortcut_at(index)
                if entry.action == 5 && entry.number != 1 { continue }
                rows.append((entry.action == 5 ? "1–9" : String(cString: entry.key), String(cString: entry.label)))
            }
            let terminal = terminalRows()
            let fontSize = max(10, min(30, min((bounds.width - 72) / 58, (bounds.height - 100) / CGFloat(max(rows.count, terminal.count) + 2) / 1.5)))
            let lineHeight = fontSize * 1.5
            let text: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: fontSize), .foregroundColor: NSColor.white.withAlphaComponent(0.88)]
            let key: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedSystemFont(ofSize: fontSize, weight: .semibold), .foregroundColor: NSColor(calibratedRed: 114/255, green: 220/255, blue: 186/255, alpha: 0.88)]
            ("Hold ⌘ + ⌥ · release either to hide" as NSString).draw(at: NSPoint(x: 24, y: 20), withAttributes: text)
            for (column, section) in [("POLAR STATION", rows), ("GHOSTTY", terminal)].enumerated() {
                let x = 24 + CGFloat(column) * (bounds.width / 2)
                (section.0 as NSString).draw(at: NSPoint(x: x, y: 28 + lineHeight), withAttributes: text)
                for (index, row) in section.1.enumerated() {
                    let y = 28 + lineHeight * CGFloat(index + 2)
                    (row.0 as NSString).draw(at: NSPoint(x: x, y: y), withAttributes: key)
                    (row.1 as NSString).draw(at: NSPoint(x: x + fontSize * (column == 0 ? 3 : 6), y: y), withAttributes: text)
                }
            }
        }

        // Native menu equivalents already reflect Ghostty's current configuration.
        private func terminalRows() -> [(String, String)] {
            var items: [String: NSMenuItem] = [:]
            func collect(_ menu: NSMenu) {
                for item in menu.items {
                    if let submenu = item.submenu { collect(submenu) }
                    if let action = item.action, !item.keyEquivalent.isEmpty {
                        items[NSStringFromSelector(action)] = item
                    }
                }
            }
            if let menu = NSApp.mainMenu { collect(menu) }
            return [("selectPreviousTab:", "Previous Tab"), ("selectNextTab:", "Next Tab"),
                    ("newTab:", "New Tab"), ("closeTab:", "Close Tab"), ("find:", "Search"),
                    ("toggleCommandPalette:", "Command palette"), ("copy:", "Copy"), ("paste:", "Paste")].compactMap { selector, label in
                guard let item = items[selector] else { return nil }
                let flags = item.keyEquivalentModifierMask
                let prefix = (flags.contains(.control) ? "⌃" : "") + (flags.contains(.option) ? "⌥" : "") + (flags.contains(.shift) ? "⇧" : "") + (flags.contains(.command) ? "⌘" : "")
                return (prefix + item.keyEquivalent.uppercased(), label)
            }
        }
    }

    /// Physical capture only. No recording, provider, focus, or editor calls.
    @MainActor
    final class StationDictateKeyCapture {
        private var held = false
        func handle(_ event: NSEvent, binding text: String, emit: (UInt32) -> Void) -> Bool {
            let binding = Self.binding(text) ?? (63, NSEvent.ModifierFlags.function)
            let modifiers = event.modifierFlags.intersection([.control, .option, .shift, .command, .function])
            if event.keyCode == binding.0 {
                if (event.type == .flagsChanged || event.type == .keyDown), modifiers == binding.1 {
                    if !held { held = true; emit(1) }
                    return true
                }
                if held, event.type == .flagsChanged || event.type == .keyUp {
                    held = false; emit(2); return true
                }
            }
            if held, event.type == .flagsChanged, !modifiers.isSuperset(of: binding.1) {
                held = false; emit(2); return false
            }
            // Fn navigation is passed through after emitting cancellation.
            if held, event.type == .keyDown || event.type == .flagsChanged {
                held = false; emit(4)
            }
            return false
        }
        static func binding(_ text: String) -> (UInt16, NSEvent.ModifierFlags)? {
            switch text {
            case "Fn": return (63, .function)
            case "RightOption": return (61, .option)
            case "Control+Option+Space": return (49, [.control, .option])
            case "Control+Shift+Space": return (49, [.control, .shift])
            default: return nil
            }
        }
    }
#endif
