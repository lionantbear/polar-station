#if POLAR_STATION
    import AppKit
    import CPolarStationDesktopClient

    enum PolarStationShortcutAction: Equatable {
        case controlCenterToggle
        case workspaceModeToggle
        case focusPrevious
        case focusNext
        case previousTab
        case nextTab
        case focusPaneNumber(Int)
        case newShell
        case newCodex
        case openIdeaTree
        case resumeCodex
        case openInquiry
        case insertIdeaNode
        case editIdeaInstruction
        case quoteSelection
        case openFileReference
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

        private let context: @MainActor () -> StationShortcutHintContext
        private let dispatch: Dispatch
        private let badgesVisible: BadgesVisible
        private var keyMonitor: Any?
        private var flagsMonitor: Any?
        private var modifiersHeld = false
        private var hintView: PolarStationShortcutHints?

        init(dispatch: @escaping Dispatch, badgesVisible: @escaping BadgesVisible, context: @escaping @MainActor () -> StationShortcutHintContext) {
            self.context = context
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
                self.showHints(self.modifiersHeld)
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
            let view = PolarStationShortcutHints(frame: content.bounds, context: context())
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
            case 17: action = .quoteSelection
            case 18: action = .previousTab
            case 19: action = .nextTab
            case 20: action = .openFileReference
            default: return nil
            }
            return Match(action: action, repeatable: entry.repeatable)
        }
    }
    /// Draw-only overlay: focus and mouse hit testing remain with the terminal.
    @MainActor
    private final class PolarStationShortcutHints: NSView {
        private let context: StationShortcutHintContext
        init(frame:NSRect,context:StationShortcutHintContext){self.context=context;super.init(frame:frame)}
        required init?(coder:NSCoder){return nil}
        override var isFlipped: Bool { true }
        override func hitTest(_ point: NSPoint) -> NSView? { nil }
        override var acceptsFirstResponder: Bool { false }
        override func draw(_ dirtyRect: NSRect) {
            NSColor(calibratedRed:8/255,green:18/255,blue:32/255,alpha:0.74).setFill();bounds.fill()
            let title:[NSAttributedString.Key:Any]=[.font:NSFont.systemFont(ofSize:36,weight:.semibold),.foregroundColor:NSColor.white]
            ("⌘ + ⌥ · Shortcuts" as NSString).draw(at:NSPoint(x:32,y:28),withAttributes:title)
            let text:[NSAttributedString.Key:Any]=[.font:NSFont.systemFont(ofSize:40),.foregroundColor:NSColor.white]
            let key:[NSAttributedString.Key:Any]=[.font:NSFont.monospacedSystemFont(ofSize:48,weight:.semibold),.foregroundColor:NSColor(calibratedRed:114/255,green:220/255,blue:186/255,alpha:1)]
            let columns=bounds.width>=1050 ? 2 : 1
            let capacity=max(1,Int((bounds.height-150)/82))
            let count=min(Int(polar_station_shortcut_hint_count(context)),columns*capacity)
            let rows=max(1,min(capacity,(count+columns-1)/columns))
            let width=(bounds.width-64)/CGFloat(columns)
            for index in 0..<count {
                let entry=polar_station_shortcut_hint_at(context,index)
                let x:CGFloat=32+CGFloat(index/rows)*width,y:CGFloat=124+CGFloat(index%rows)*82
                (String(cString:entry.key) as NSString).draw(at:NSPoint(x:x,y:y),withAttributes:key)
                (String(cString:entry.label) as NSString).draw(in:NSRect(x:x+164,y:y+8,width:width-174,height:60),withAttributes:text)
            }
        }
    }

#endif
