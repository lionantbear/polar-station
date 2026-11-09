#if POLAR_STATION
import AppKit
import OSLog
import PolarTmuxHostClient

/// Owns every managed Ghostty Tab controller and the AppKit presentation state
/// that must stay coherent across selection and native-fullscreen transitions.
@MainActor
final class PolarStationNativeTabGroupCoordinator: NSObject {
    private var focusButtons: [String: NSButton] = [:]
    private let toggleTabFocus: @MainActor (String) -> Void

    private struct FullscreenEntryWatchdog: Equatable, Sendable {
        let windowToken: String
        let generation: UInt64
        let ticket: UInt64
    }

    private struct StartupPresentationSettle: Equatable, Sendable {
        let windowToken: String
        let readyUptime: TimeInterval
    }

    private weak var initialController: TerminalController?
    private var controllers: [String: TerminalController] = [:]
    private var expectedTabIDs: [String] = []
    private var tabOrderObservation: NSKeyValueObservation?
    private weak var observedTabGroup: NSWindowTabGroup?
    var nativeTransferRequested: (@MainActor (TerminalController) -> Void)?
    var nativeOrderChanged: (@MainActor ([String]) -> Void)?

    func tabID(for controller: TerminalController) -> String? {
        controllers.first { $0.value === controller }?.key
    }

    func nativeTabOrder() -> [String]? {
        guard !isMutatingNativeTabs,
              let group = controllers.values.compactMap({ $0.window?.tabGroup }).first else { return nil }
        let order = group.windows.compactMap { window in
            controllers.first(where: { $0.value.window === window })?.key
        }
        guard order.count == group.windows.count,
              order.count == expectedTabIDs.count,
              Set(order) == Set(expectedTabIDs) else { return nil }
        return order
    }

    private func observeTabOrder() {
        guard let group = controllers.values.compactMap({ $0.window?.tabGroup }).first,
              observedTabGroup !== group else { return }
        tabOrderObservation = nil
        observedTabGroup = group
        tabOrderObservation = group.observe(\.windows, options: [.new]) { [weak self] group, _ in
            // AppKit has already previewed a transfer. Restore the source before
            // asking for its destination; callbacks from our own repair are ignored.
            MainActor.assumeIsolated {
                guard let self, !self.isMutatingNativeTabs else { return }
                DispatchQueue.main.async { [weak self, weak group] in
                    guard let self, let group, !self.isMutatingNativeTabs,
                          let moved = self.controllers.values.first(where: { controller in
                              guard let window = controller.window else { return false }
                              return !group.windows.contains(window)
                          }) else { return }
                    self.nativeTransferRequested?(moved)
                }
            }
            // AppKit delivers its tab-group KVO on the main UI thread.
            MainActor.assumeIsolated {
                guard let self, let order = self.nativeTabOrder(),
                      order != self.expectedTabIDs else { return }
                self.nativeOrderChanged?(order)
            }
        }
    }
    private var presentationState = NativeTabGroupPresentationState()
    private var selectionIngressState = NativeTabSelectionIngressState()
    private var tabTreeMutationDepth = 0
    private var creatingNativeTab = false
    var isMutatingNativeTabs: Bool { tabTreeMutationDepth > 0 }
    private var startupFullscreenRequested = false
    private var presentationReconcileScheduled = false
    private var presentationReconcileAttempt = 0
    private var presentationFrame: NSRect?
    private var pendingFocusRequest: NativeTerminalFocusRequest?
    private var activeFullscreenEntryWatchdog: FullscreenEntryWatchdog?
    private var nextFullscreenEntryWatchdogTicket: UInt64 = 0
    private var startupPresentationSettle: StartupPresentationSettle?
    private var lastLoggedPresentationPhase:
        NativeTabGroupPresentationState.Phase?
    private let surfaceForPane: @MainActor (String) -> Ghostty.SurfaceView?
    private let presentationLogger = Logger(
        subsystem: "com.polarealm.polar-station",
        category: "native-presentation"
    )

    private static let maxPresentationReconcileAttempts = 80
    private static let presentationReconcileRetryDelay: TimeInterval = 0.05
    private static let startupPresentationQuietInterval: TimeInterval = 0.75
    private static let fullscreenEntryWatchdogDelay: TimeInterval = 3

    init(
        surfaceForPane: @escaping @MainActor (String) -> Ghostty.SurfaceView?,
        toggleTabFocus: @escaping @MainActor (String) -> Void
    ) {
        self.surfaceForPane = surfaceForPane
        self.toggleTabFocus = toggleTabFocus
        super.init()
        NotificationCenter.default.addObserver(
            self, selector: #selector(tabAccessoryFrameChanged(_:)),
            name: NSView.frameDidChangeNotification, object: nil
        )
        NotificationCenter.default.addObserver(
            self, selector: #selector(terminalSurfaceMounted(_:)),
            name: .init("PolarStationTerminalMounted"), object: nil
        )
    }

    deinit { NotificationCenter.default.removeObserver(self) }

    @objc private func tabAccessoryFrameChanged(_ notification: Notification) {
        guard let button = notification.object as? NSButton,
              focusButtons.values.contains(where: { $0 === button }),
              !isMutatingNativeTabs else { return }
        // Reuse Ghostty TerminalController's native tab reorder signal. AppKit
        // does not consistently issue windows KVO for mouse-only reorders.
        DispatchQueue.main.async { [weak self] in
            guard let self, let order = self.nativeTabOrder(),
                  order != self.expectedTabIDs else { return }
            self.observeTabOrder()
            self.nativeOrderChanged?(order)
        }
    }

    @objc private func terminalSurfaceMounted(_ notification: Notification) {
        guard let request = pendingFocusRequest,
              let surface = notification.object as? Ghostty.SurfaceView,
              surfaceForPane(request.paneID) === surface else { return }
        schedulePresentationReconcile()
    }

    var initialControllerIsAttached: Bool {
        initialController?.window != nil
            || controllers.values.contains { $0.window != nil }
    }

    func selectedManagedTabID() -> String? {
        if let key = controllers.first(where: {
            $0.value.window?.isKeyWindow == true
        })?.key { return key }
        let selectedWindow = controllers.values
            .compactMap(\.window)
            .compactMap { $0.tabGroup?.selectedWindow }
            .first
        return selectedWindow.flatMap { selected in
            controllers.first(where: { $0.value.window === selected })?.key
        }
    }

    private func newManagedTab(from controller: TerminalController) -> TerminalController? {
        creatingNativeTab = true
        defer { creatingNativeTab = false }
        // The coordinator owns selection. The user-facing factory otherwise
        // activates every newly created Tab asynchronously after restoration.
        return TerminalController.newTab(
            controller.ghostty, from: controller.window, activate: false
        )
    }

    func owns(_ controller: TerminalController) -> Bool {
        if creatingNativeTab { return true }
        if controller === initialController || controllers.values.contains(where: { $0 === controller }) { return true }
        guard let group = controller.window?.tabGroup else { return false }
        return initialController?.window?.tabGroup === group
            || controllers.values.contains { $0.window?.tabGroup === group }
    }

    @discardableResult
    func attach(_ controller: TerminalController) -> Bool {
        if initialController == nil {
            initialController = controller
            presentationFrame = controller.window?.frame
        }
        schedulePresentationReconcile()
        return controller === initialController
    }

    func reset() {
        for controller in controllers.values {
            controller.window?.tab.accessoryView = nil
        }
        focusButtons.removeAll()
        initialController = nil
        controllers.removeAll()
        tabOrderObservation = nil
        observedTabGroup = nil
        expectedTabIDs.removeAll()
        presentationState = NativeTabGroupPresentationState()
        selectionIngressState = NativeTabSelectionIngressState()
        tabTreeMutationDepth = 0
        startupFullscreenRequested = false
        presentationReconcileScheduled = false
        presentationReconcileAttempt = 0
        presentationFrame = nil
        pendingFocusRequest = nil
        activeFullscreenEntryWatchdog = nil
        startupPresentationSettle = nil
        lastLoggedPresentationPhase = nil
    }

    /// Keep one native container for the chooser while releasing every bound Tab.
    func unbind(prepare: (TerminalController) -> Void) {
        guard let keeper = controllers.values.first(where: { $0.window?.isKeyWindow == true })
            ?? controllers.values.first(where: { $0.window != nil })
            ?? initialController.flatMap({ $0.window != nil ? $0 : nil }) else { return }
        let retired = controllers
        tabTreeMutationDepth += 1
        for controller in retired.values { prepare(controller) }
        if !retired.values.contains(where: { $0 === keeper }) { prepare(keeper) }
        closeRetiredControllers(retired, retained: ["unbound": keeper])
        tabTreeMutationDepth -= 1
        reset()
        _ = attach(keeper)
    }

    @discardableResult
    func reconcileTabs(
        _ tabs: [PolarStationNativeTab],
        retireAbsent: Bool,
        install: (
            _ tab: PolarStationNativeTab,
            _ controller: TerminalController,
            _ hasInstalledController: Bool
        ) -> Void
    ) -> [String: TerminalController] {
        let nextExpectedTabIDs = tabs.map(\.id)
        if let effectiveTabID = selectionIngressState
            .reconcileAvailableTabIDs(Set(nextExpectedTabIDs))
        {
            presentationState.requestSelection(tabID: effectiveTabID)
        }
        if expectedTabIDs != nextExpectedTabIDs {
            expectedTabIDs = nextExpectedTabIDs
            presentationReconcileAttempt = 0
        }

        guard let creationController = nextExpectedTabIDs.compactMap({
            controllers[$0]
        }).first(where: { $0.window != nil })
            ?? controllers.values.first(where: { $0.window != nil })
            ?? initialController,
              creationController.window != nil
        else { return [:] }

        tabTreeMutationDepth += 1
        defer { tabTreeMutationDepth -= 1 }

        // Switching Workspace replaces its Tab identities, not its native Window.
        // Retain the current backing controller so AppKit cannot retire the whole
        // fullscreen Space while the new Workspace is being installed.
        let replacingWorkspace = retireAbsent && !controllers.isEmpty
            && !tabs.contains(where: { controllers[$0.id] != nil })
        let reusableController = replacingWorkspace
            ? (controllers.values.first(where: { $0.window?.isKeyWindow == true })
                ?? controllers.values.first(where: { $0.window != nil })) : nil
        var nextControllers: [String: TerminalController] = [:]
        for (index, tab) in tabs.enumerated() {
            let controller: TerminalController
            let hasInstalledController: Bool
            if let existing = controllers[tab.id] {
                controller = existing
                hasInstalledController = true
            } else if index == 0, let reusableController {
                controller = reusableController
                hasInstalledController = false
            } else if index == 0,
                      controllers.isEmpty,
                      let initialController,
                      initialController.window != nil {
                controller = initialController
                hasInstalledController = false
            } else if let created = newManagedTab(from: creationController) {
                controller = created
                hasInstalledController = false
            } else {
                continue
            }

            nextControllers[tab.id] = controller
            controller.titleOverride = tab.title
            updateFocusButton(tab, controller: controller)
            install(tab, controller, hasInstalledController)
        }

        let retired = controllers.filter { nextControllers[$0.key] == nil }
        handoffSelectionBeforeRetirement(
            retired,
            retained: nextControllers
        )
        controllers = nextControllers
        initialController = tabs.first.flatMap { nextControllers[$0.id] }
        focusButtons = focusButtons.filter { nextControllers[$0.key] != nil }
        closeRetiredControllers(retired, retained: nextControllers)
        presentationLogger.notice("Workspace backing windows retained=\(nextControllers.count) visible=\(nextControllers.values.filter { $0.window?.isVisible == true }.count)")

        reorderTabs(tabs.compactMap { nextControllers[$0.id] })
        observeTabOrder()
        schedulePresentationReconcile()
        return nextControllers
    }

    func updateTabMetadata(_ tabs: [PolarStationNativeTab]) {
        for tab in tabs {
            controllers[tab.id]?.titleOverride = tab.title
            if let controller = controllers[tab.id] {
                updateFocusButton(tab, controller: controller)
            }
        }
    }

    private func updateFocusButton(_ tab: PolarStationNativeTab, controller: TerminalController) {
        guard let window = controller.window else { return }
        let button: NSButton
        if let existing = focusButtons[tab.id] {
            button = existing
        } else {
            button = NSButton(title: "", target: self, action: #selector(focusClicked(_:)))
            button.postsFrameChangedNotifications = true
            button.identifier = NSUserInterfaceItemIdentifier(tab.id)
            button.setButtonType(.pushOnPushOff)
            button.bezelStyle = .accessoryBarAction
            button.imagePosition = .imageOnly
            button.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                button.widthAnchor.constraint(equalToConstant: 24),
                button.heightAnchor.constraint(equalToConstant: 22)
            ])
            focusButtons[tab.id] = button
        }
        let focused = tab.mode == .focus
        button.state = focused ? .on : .off
        button.image = NSImage(
            systemSymbolName: focused ? "rectangle.inset.filled" : "square.grid.2x2",
            accessibilityDescription: "Focus"
        )
        button.toolTip = focused ? "Focus on — switch to Monitor" : "Focus off — switch to Focus"
        button.setAccessibilityLabel("Focus for \(tab.title)")
        window.tab.accessoryView = button
    }

    @objc private func focusClicked(_ sender: NSButton) {
        guard let tabID = sender.identifier?.rawValue,
              controllers[tabID] != nil else { return }
        // Rendering follows the committed native mode, not NSButton's eager toggle.
        sender.state = sender.state == .on ? .off : .on
        toggleTabFocus(tabID)
    }

    func requestSelection(
        tabID: String,
        paneID: String?,
        focusIfActive: Bool,
        preservePendingFocus: Bool = false
    ) {
        guard let effectiveTabID = selectionIngressState
            .requestProjectedSelection(tabID: tabID)
        else { return }
        presentationState.requestSelection(
            tabID: effectiveTabID,
            presentation: focusIfActive ? .activeUser : .passive
        )
        if focusIfActive, effectiveTabID == tabID, let paneID {
            StationOperationTrace.record(3, identity: paneID)
            pendingFocusRequest = NativeTerminalFocusRequest(
                tabID: tabID,
                paneID: paneID
            )
        } else if !preservePendingFocus,
                  pendingFocusRequest?.tabID != effectiveTabID {
            pendingFocusRequest = nil
        }
        schedulePresentationReconcile()
    }

    /// Converts a real AppKit Tab choice into one ordered native intent before
    /// attach-time presentation reconciliation can classify it as drift.
    func observeNativeTabSelection(
        _ controller: TerminalController
    ) -> NativeTabSelectionIngressState.Intent? {
        guard tabTreeMutationDepth == 0,
              let window = controller.window,
              window.isKeyWindow,
              let tabID = controllers.first(where: {
                  $0.value === controller
              })?.key
        else { return nil }
        let selectedWindow = window.tabGroup?.selectedWindow ?? window
        guard selectedWindow === window,
              let intent = selectionIngressState.observeNativeSelection(
                  tabID: tabID
              )
        else { return nil }

        pendingFocusRequest = nil
        presentationState.requestSelection(tabID: intent.tabID)
        schedulePresentationReconcile()
        return intent
    }

    /// Resolves only the matching newest native intent. An older Host reply
    /// cannot clear or overwrite a later native Tab choice.
    func completeNativeTabSelection(
        _ intent: NativeTabSelectionIngressState.Intent
    ) {
        guard let effectiveTabID = selectionIngressState
            .completeNativeSelection(intent)
        else { return }
        presentationState.requestSelection(tabID: effectiveTabID)
        schedulePresentationReconcile()
    }

    func requestStartupFullscreen() {
        guard !startupFullscreenRequested else { return }
        startupFullscreenRequested = true
        presentationReconcileAttempt = 0
        presentationLogger.notice("startup fullscreen requested")
        schedulePresentationReconcile()
    }

    func windowDidEnterFullScreen(_ window: NSWindow) {
        let windowToken = token(for: window)
        let accepted = fullscreenCompletionIsCurrent(windowToken: windowToken)
        presentationLogger.notice(
            "didEnterFullScreen accepted=\(accepted, privacy: .public)"
        )
        let commands = presentationState.didEnterFullscreen(
            windowToken: windowToken
        )
        if accepted {
            activeFullscreenEntryWatchdog = nil
        }
        execute(commands)
        schedulePresentationReconcile()
    }

    func windowDidFailToEnterFullScreen(_ window: NSWindow) {
        let windowToken = token(for: window)
        let accepted = fullscreenCompletionIsCurrent(windowToken: windowToken)
        presentationLogger.error(
            "didFailToEnterFullScreen accepted=\(accepted, privacy: .public)"
        )
        let commands = presentationState.didFailToEnterFullscreen(
            windowToken: windowToken
        )
        if accepted {
            activeFullscreenEntryWatchdog = nil
        }
        execute(commands)
        if accepted {
            NSLog(
                "Polar Station: native fullscreen transition failed; staying windowed"
            )
        }
        schedulePresentationReconcile()
    }

    func windowDidExitFullScreen(_ window: NSWindow) {
        presentationLogger.notice("didExitFullScreen")
        presentationState.didExitFullscreen(windowToken: token(for: window))
        presentationFrame = window.frame
        schedulePresentationReconcile()
    }

    func controller(for tabID: String) -> TerminalController? {
        controllers[tabID]
    }

    var allControllers: [TerminalController] {
        Array(controllers.values)
    }

    private func schedulePresentationReconcile(
        after delay: TimeInterval = 0
    ) {
        guard !presentationReconcileScheduled else { return }
        presentationReconcileScheduled = true
        if delay == 0 {
            DispatchQueue.main.async { [weak self] in
                self?.runScheduledPresentationReconcile()
            }
        } else {
            DispatchQueue.main.asyncAfter(
                deadline: .now() + delay
            ) { [weak self] in
                self?.runScheduledPresentationReconcile()
            }
        }
    }

    private func runScheduledPresentationReconcile() {
        presentationReconcileScheduled = false
        reconcilePresentation()
    }

    private func reconcilePresentation() {
        guard let observation = presentationObservation() else {
            scheduleNextStartupObservationIfNeeded()
            return
        }
        let commands = presentationState.reconcile(
            observation,
            startupFullscreenRequested: startupFullscreenRequested
        )
        logPresentationPhaseIfChanged()
        execute(commands)
        logPresentationPhaseIfChanged()
        if let currentObservation = presentationObservation() {
            applyPendingFocusIfReady(observation: currentObservation)
        }
        if pendingFocusRequest != nil, !commands.isEmpty {
            schedulePresentationReconcile()
        }

        if commands.contains(where: { command in
            if case .enterFullscreen = command { return true }
            return false
        }) {
            return
        }
        switch presentationState.phase {
        case .enteringFullscreen, .readyFullscreen,
                .readyWindowedAfterUserExit, .windowedFallback:
            return
        default:
            if !commands.isEmpty || startupFullscreenRequested {
                scheduleNextStartupObservationIfNeeded()
            }
        }
    }

    private func scheduleNextStartupObservationIfNeeded() {
        guard startupFullscreenRequested else { return }
        presentationReconcileAttempt += 1
        guard presentationReconcileAttempt <
                Self.maxPresentationReconcileAttempts
        else {
            presentationState.timedOut()
            logPresentationPhaseIfChanged()
            presentationLogger.error(
                "startup presentation observation timed out"
            )
            NSLog(
                "Polar Station: Tab group did not stabilize before fullscreen"
            )
            return
        }
        schedulePresentationReconcile(
            after: Self.presentationReconcileRetryDelay
        )
    }

    private func presentationObservation()
        -> NativeTabGroupPresentationObservation? {
        guard !expectedTabIDs.isEmpty else { return nil }
        let windowTokensByTabID = Dictionary(
            uniqueKeysWithValues: expectedTabIDs.compactMap { tabID in
                controllers[tabID]?.window.map { (tabID, token(for: $0)) }
            }
        )
        guard windowTokensByTabID.count == expectedTabIDs.count else {
            return nil
        }
        let expectedWindows = expectedTabIDs.compactMap {
            controllers[$0]?.window
        }
        guard expectedWindows.count == expectedTabIDs.count,
              let firstWindow = expectedWindows.first
        else { return nil }

        let group = firstWindow.tabGroup
        let groupWindows = group?.windows ?? [firstWindow]
        let groupWindowTokens = groupWindows.map(token(for:))
        let groupTokensByWindowToken = Dictionary(
            uniqueKeysWithValues: expectedWindows.map { window in
                let owner = window.tabGroup.map {
                    token(for: $0)
                } ?? "single-\(token(for: window))"
                return (token(for: window), owner)
            }
        )
        let selectedWindow = group?.selectedWindow ?? firstWindow
        let referenceWindowToken: String
        if let initialWindow = initialController?.window,
           expectedWindows.contains(where: { $0 === initialWindow }) {
            referenceWindowToken = token(for: initialWindow)
        } else {
            referenceWindowToken = token(for: selectedWindow)
        }
        let frames = Dictionary(
            uniqueKeysWithValues: expectedWindows.map { window in
                (token(for: window), nativeFrame(window.frame))
            }
        )
        let fullscreenWindowTokens = Set(expectedWindows.compactMap { window in
            window.styleMask.contains(.fullScreen) ? token(for: window) : nil
        })
        let settledPresentationWindowToken =
            startupPresentationSettle.flatMap { settle in
                ProcessInfo.processInfo.systemUptime >= settle.readyUptime
                    ? settle.windowToken : nil
            }

        return NativeTabGroupPresentationObservation(
            expectedTabIDs: expectedTabIDs,
            windowTokensByTabID: windowTokensByTabID,
            groupTokensByWindowToken: groupTokensByWindowToken,
            groupWindowTokens: groupWindowTokens,
            selectedWindowToken: token(for: selectedWindow),
            referenceWindowToken: referenceWindowToken,
            frames: frames,
            fullscreenWindowTokens: fullscreenWindowTokens,
            transitioningWindowTokens: [],
            applicationIsActive: NSApp.isActive,
            visibleWindowTokens: Set(expectedWindows.compactMap { window in
                window.isVisible ? token(for: window) : nil
            }),
            keyWindowToken: expectedWindows.first(where: \.isKeyWindow)
                .map(token(for:)),
            settledPresentationWindowToken:
                settledPresentationWindowToken
        )
    }

    private func execute(
        _ commands: [NativeTabGroupPresentationState.Command]
    ) {
        for command in commands {
            switch command {
            case .normalizeFrames(let referenceWindowToken):
                normalizeFrames(referenceWindowToken: referenceWindowToken)
            case .selectWindow(_, let windowToken, let presentation):
                selectWindow(
                    windowToken: windowToken,
                    presentation: presentation
                )
            case .presentWindow(_, let windowToken):
                presentForStartupFullscreen(windowToken: windowToken)
            case .enterFullscreen(let windowToken, let generation):
                guard let controller = controller(windowToken: windowToken)
                else {
                    execute(
                        presentationState.didFailToEnterFullscreen(
                            windowToken: windowToken
                        )
                    )
                    continue
                }
                scheduleFullscreenEntryWatchdog(
                    windowToken: windowToken,
                    generation: generation
                )
                presentationLogger.notice(
                    "issuing one native fullscreen transition generation=\(generation, privacy: .public)"
                )
                controller.toggleFullscreen(mode: .native)
            case .applyDeferredSelection:
                schedulePresentationReconcile()
            }
        }
    }

    private func normalizeFrames(referenceWindowToken: String) {
        guard let reference = window(windowToken: referenceWindowToken) else {
            return
        }
        let frame = presentationFrame ?? reference.frame
        presentationFrame = frame
        NSAnimationContext.beginGrouping()
        NSAnimationContext.current.duration = 0
        for controller in controllers.values {
            guard let window = controller.window,
                  !window.styleMask.contains(.fullScreen)
            else { continue }
            window.setFrame(frame, display: false, animate: false)
        }
        NSAnimationContext.endGrouping()
    }

    private func selectWindow(
        windowToken: String,
        presentation: NativeTabSelectionPresentation
    ) {
        guard let target = window(windowToken: windowToken) else { return }
        let selected = target.tabGroup?.selectedWindow
            ?? controllers.values.compactMap(\.window).first
        if let selected, selected !== target {
            let frame = selected.frame
            target.setFrame(frame, display: false, animate: false)
            presentationFrame = frame
        }
        if target.tabGroup?.selectedWindow !== target {
            target.tabGroup?.selectedWindow = target
        }
        if presentation == .activeUser, NSApp.isActive {
            target.makeKeyAndOrderFront(nil)
        }
    }

    private func presentForStartupFullscreen(windowToken: String) {
        guard startupFullscreenRequested,
              let target = window(windowToken: windowToken)
        else { return }
        let frame = presentationFrame ?? target.frame
        if !target.styleMask.contains(.fullScreen) {
            target.setFrame(frame, display: false, animate: false)
        }
        _ = NSRunningApplication.current.activate(options: [
            .activateAllWindows,
            .activateIgnoringOtherApps,
        ])
        NSApp.activate(ignoringOtherApps: true)
        target.makeKeyAndOrderFront(nil)
        startupPresentationSettle = StartupPresentationSettle(
            windowToken: windowToken,
            readyUptime: ProcessInfo.processInfo.systemUptime
                + Self.startupPresentationQuietInterval
        )
        presentationLogger.notice(
            "presented exact startup window; waiting for quiet interval"
        )
    }

    private func scheduleFullscreenEntryWatchdog(
        windowToken: String,
        generation: UInt64
    ) {
        nextFullscreenEntryWatchdogTicket &+= 1
        if nextFullscreenEntryWatchdogTicket == 0 {
            nextFullscreenEntryWatchdogTicket = 1
        }
        let watchdog = FullscreenEntryWatchdog(
            windowToken: windowToken,
            generation: generation,
            ticket: nextFullscreenEntryWatchdogTicket
        )
        activeFullscreenEntryWatchdog = watchdog
        DispatchQueue.main.asyncAfter(
            deadline: .now() + Self.fullscreenEntryWatchdogDelay
        ) { [weak self] in
            self?.fullscreenEntryWatchdog(watchdog)
        }
    }

    private func fullscreenEntryWatchdog(
        _ watchdog: FullscreenEntryWatchdog
    ) {
        guard activeFullscreenEntryWatchdog == watchdog,
              case .enteringFullscreen(
                  let targetWindowToken,
                  let currentGeneration
              ) = presentationState.phase,
              targetWindowToken == watchdog.windowToken,
              currentGeneration == watchdog.generation
        else { return }

        activeFullscreenEntryWatchdog = nil
        if window(windowToken: watchdog.windowToken)?.styleMask.contains(
            .fullScreen
        ) == true {
            execute(
                presentationState.didEnterFullscreen(
                    windowToken: watchdog.windowToken
                )
            )
            NSLog(
                "Polar Station: fullscreen watchdog accepted observed fullscreen"
            )
            presentationLogger.notice(
                "fullscreen watchdog accepted observed fullscreen"
            )
        } else {
            execute(
                presentationState.didFailToEnterFullscreen(
                    windowToken: watchdog.windowToken
                )
            )
            NSLog(
                "Polar Station: fullscreen entry timed out; staying windowed"
            )
            presentationLogger.error(
                "fullscreen entry timed out; staying windowed"
            )
        }
        schedulePresentationReconcile()
    }

    private func fullscreenCompletionIsCurrent(
        windowToken: String
    ) -> Bool {
        guard case .enteringFullscreen(let targetWindowToken, _) =
                presentationState.phase
        else { return false }
        return targetWindowToken == windowToken
    }

    private func logPresentationPhaseIfChanged() {
        let phase = presentationState.phase
        guard lastLoggedPresentationPhase != phase else { return }
        lastLoggedPresentationPhase = phase
        presentationLogger.notice(
            "native presentation phase=\(self.presentationPhaseName(phase), privacy: .public)"
        )
    }

    private func presentationPhaseName(
        _ phase: NativeTabGroupPresentationState.Phase
    ) -> String {
        switch phase {
        case .awaitingInitialController:
            "awaiting-initial-controller"
        case .buildingNormalTree:
            "building-normal-tree"
        case .stabilizingSelection:
            "stabilizing-selection"
        case .enteringFullscreen:
            "entering-fullscreen"
        case .readyFullscreen:
            "ready-fullscreen"
        case .readyWindowedAfterUserExit:
            "ready-windowed-after-user-exit"
        case .windowedFallback:
            "windowed-fallback"
        }
    }

    private func applyPendingFocusIfReady(
        observation: NativeTabGroupPresentationObservation
    ) {
        guard tabTreeMutationDepth == 0,
              var request = pendingFocusRequest,
              presentationState.requestedSelectedTabID == request.tabID,
              let expectedWindowToken =
                observation.windowTokensByTabID[request.tabID],
              observation.selectedWindowToken == expectedWindowToken
        else { return }
        guard NSApp.isActive,
              let controller = controllers[request.tabID],
              controller.window?.isKeyWindow == true,
              let window = controller.window,
              let surface = surfaceForPane(request.paneID),
              surface.window === window,
              controller.surfaceTree.contains(where: { $0 === surface })
        else { return }
        // focusSurface/moveFocus enqueue unowned future focus work (and can
        // activate another window). This mounted, selected view is ready now.
        if controller.focusedSurface !== surface {
            controller.focusedSurfaceDidChange(to: surface)
        }
        if window.firstResponder !== surface {
            _ = window.makeFirstResponder(surface)
        }
        if request.observe(
            selectedTabID: selectedManagedTabID(),
            focusedPaneID: controller.focusedSurface === surface ? request.paneID : nil,
            firstResponderPaneID: window.firstResponder === surface ? request.paneID : nil
        ) {
            StationOperationTrace.record(4, identity: request.paneID)
            pendingFocusRequest = nil
        } else {
            pendingFocusRequest = request
            // Reuse the presentation turn already responsible for mounting
            // AppKit trees; no independent timer or Host readiness gate.
            if request.observedFocused { schedulePresentationReconcile() }
        }
    }

    private func handoffSelectionBeforeRetirement(
        _ retired: [String: TerminalController],
        retained: [String: TerminalController]
    ) {
        guard !retired.isEmpty,
              let requestedTabID = presentationState.requestedSelectedTabID
        else { return }

        let selectedWindow = controllers.values
            .compactMap(\.window)
            .compactMap { $0.tabGroup?.selectedWindow }
            .first
        let selectedTabID = selectedWindow.flatMap { selected in
            controllers.first(where: { $0.value.window === selected })?.key
        }
        let keyTabID = controllers.first(where: {
            $0.value.window?.isKeyWindow == true
        })?.key
        let visibleTabIDs = Set(controllers.compactMap { tabID, controller in
            controller.window?.isVisible == true ? tabID : nil
        })
        guard let handoff = NativeTabRetirementHandoff.plan(
            requestedTabID: requestedTabID,
            selectedTabID: selectedTabID,
            keyTabID: keyTabID,
            visibleTabIDs: visibleTabIDs,
            retiringTabIDs: Set(retired.keys),
            applicationIsActive: NSApp.isActive
        ),
        let target = retained[handoff.targetTabID],
        let targetWindow = target.window,
        let sourceWindow = controllers[handoff.frameSourceTabID]?.window
        else { return }

        if targetWindow !== sourceWindow,
           !targetWindow.styleMask.contains(.fullScreen)
        {
            targetWindow.setFrame(
                sourceWindow.frame,
                display: false,
                animate: false
            )
            presentationFrame = sourceWindow.frame
        }
        if targetWindow.tabGroup?.selectedWindow !== targetWindow {
            targetWindow.tabGroup?.selectedWindow = targetWindow
        }
        if handoff.makeKey {
            targetWindow.makeKeyAndOrderFront(nil)
        } else if handoff.orderFront {
            targetWindow.orderFront(nil)
        }
    }

    private func closeRetiredControllers(
        _ retired: [String: TerminalController],
        retained: [String: TerminalController]
    ) {
        for (_, controller) in retired {
            guard !retained.values.contains(where: { $0 === controller })
            else { continue }
            presentationLogger.notice("Retiring native Tab; group windows=\(controller.window?.tabGroup?.windows.count ?? 0)")
            if controller.window?.tabGroup?.windows.count ?? 0 > 1 {
                controller.closeTabImmediately(registerRedo: false)
            } else {
                controller.closeWindowImmediately()
            }
        }
    }

    private func reorderTabs(_ desired: [TerminalController]) {
        guard let anchor = desired.first?.window, desired.count > 1 else {
            return
        }
        let desiredWindows = desired.compactMap(\.window)
        if let current = anchor.tabGroup?.windows,
           current.count == desiredWindows.count,
           zip(current, desiredWindows).allSatisfy({ $0 === $1 }) {
            return
        }
        NSAnimationContext.beginGrouping()
        NSAnimationContext.current.duration = 0
        for controller in desired.dropFirst() {
            guard let window = controller.window else { continue }
            window.tabGroup?.removeWindow(window)
            _ = anchor.tabGroup?.windows.last?
                .addTabbedWindowSafely(window, ordered: .above)
                ?? anchor.addTabbedWindowSafely(window, ordered: .above)
        }
        NSAnimationContext.endGrouping()
    }

    private func controller(
        windowToken: String
    ) -> TerminalController? {
        controllers.values.first {
            $0.window.map { token(for: $0) } == windowToken
        }
    }

    private func window(windowToken: String) -> NSWindow? {
        controller(windowToken: windowToken)?.window
    }

    private func token(for object: AnyObject) -> String {
        String(describing: ObjectIdentifier(object))
    }

    private func nativeFrame(_ frame: NSRect) -> NativeWindowFrame {
        NativeWindowFrame(
            x: frame.origin.x,
            y: frame.origin.y,
            width: frame.size.width,
            height: frame.size.height
        )
    }
}
#endif
