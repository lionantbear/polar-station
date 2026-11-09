#if POLAR_STATION
    import AppKit
    import PolarTmuxHostClient
    import PolarTmuxMacUI
    import SwiftUI

    extension Notification.Name {
        static let polarStationSurfaceGeometryDidChange =
            Notification.Name("PolarStationSurfaceGeometryDidChange")
    }

    private final class PolarStationPaneNumberBadge: NSView {
        var number = 0 {
            didSet { needsDisplay = true }
        }

        override func hitTest(_ point: NSPoint) -> NSView? { nil }

        override func setFrameSize(_ newSize: NSSize) {
            super.setFrameSize(newSize)
            needsDisplay = true
        }

        override func draw(_ dirtyRect: NSRect) {
            guard number > 0, bounds.width > 0, bounds.height > 0 else { return }
            let glyphSize = min(896, max(24, min(bounds.width, bounds.height) * 0.70))
            let glyph = NSAttributedString(
                string: String(number),
                attributes: [
                    .font: NSFont.boldSystemFont(ofSize: glyphSize),
                    .foregroundColor: NSColor(
                        calibratedRed: 114 / 255,
                        green: 220 / 255,
                        blue: 186 / 255,
                        alpha: 0.66
                    ),
                    .strokeColor: NSColor(
                        calibratedRed: 8 / 255,
                        green: 18 / 255,
                        blue: 32 / 255,
                        alpha: 0.66
                    ),
                    .strokeWidth: -2,
                ]
            )
            let size = glyph.size()
            glyph.draw(at: NSPoint(
                x: (bounds.width - size.width) / 2,
                y: (bounds.height - size.height) / 2
            ))
        }
    }

    extension Ghostty.SurfaceView {
        func polarStationRefreshSurfaceLayouts() {
            needsLayout = true
            superview?.needsLayout = true
            NotificationCenter.default.post(
                name: .polarStationSurfaceGeometryDidChange,
                object: self
            )
        }

        /// Delivers approved Inbox text through Ghostty's native Surface API.
        /// This deliberately bypasses the clipboard and synthetic key events.
        func polarStationSendText(_ text: String) {
            surfaceModel?.sendText(text)
        }
    }

    enum StationMutationScope {
        @TaskLocal static var active = false
    }

    /// Applies the native-owned Workspace table directly to managed Ghostty.
    @MainActor
    final class PolarStationNativeExecutor {
        private(set) var windowGeneration: UInt64 = 0
        // Reuse the runtime FIFO for every writer, including non-UI callbacks.
        var enqueueMutation: ((@escaping @MainActor () async throws -> Void, Bool, @escaping @MainActor (Error?) -> Void) -> Void)?
        private func serialized(_ work: @escaping @MainActor () async throws -> Void, requireWindow: Bool = true) async throws {
            guard let enqueueMutation else { throw PolarStationNativeExecutorError.workspaceUnavailable }
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                enqueueMutation(work, requireWindow) { error in
                    if let error { continuation.resume(throwing: error) } else { continuation.resume() }
                }
            }
        }
        var lifecycleIntent: ((ControlCenterIntentOperationV1) -> Void)?
        private var chooserView: NSView?
        private var chooserChoices: [NativeWorkspaceChoiceView]?
        func showWorkspaceChooser() {
            guard workspace == nil, let layout = layoutView(),
                  let controller = TerminalController.all.first(where: { owns($0) }),
                  let content = controller.window?.contentView else { return }
            if chooserView?.superview === content, chooserChoices == layout.workspaceChoices { return }
            chooserChoices = layout.workspaceChoices
            chooserView?.removeFromSuperview()
            let view = NSHostingView(rootView: WorkspaceChooserView(choices: layout.workspaceChoices) { [weak self] in self?.lifecycleIntent?($0) })
            view.frame = content.bounds
            view.autoresizingMask = [.width, .height]
            content.addSubview(view)
            chooserView = view
            controller.titleOverride = "Polar Station — Open Workspace"
        }
        var catalogChanged: (@MainActor () -> Void)?
        var requestTabMove: ((TerminalController) -> Void)?
        var toggleTabFocus: (@MainActor (String) -> Void)?
        private var numberBadgesVisible = false
        private struct InstalledTabStructure: Equatable {
            let paneIDs: [String]
            let bindings: [WorkspaceStructurePaneV1]
            let visualPaneIDs: [String]
            let mode: PolarStationNativeDisplayMode
            let mainPaneID: String?

            init(tab: PolarStationNativeTab) {
                paneIDs = tab.panes.map(\.id)
                bindings = tab.panes.map {
                    WorkspaceStructurePaneV1(id: $0.id, executionTargetID: $0.executionTargetID,
                        tmuxSessionID: $0.tmuxSessionID, content: $0.content)
                }
                visualPaneIDs = tab.visualPaneIDs
                mode = tab.mode
                mainPaneID = tab.mainPaneID
            }
        }

        private var catalogOwner: PolarStationNativeExecutor?
        private var ownedCatalog: PolarStationNativeWorkspaceCatalog?
        private var selectedWorkspaceID: String?
        private var catalog: PolarStationNativeWorkspaceCatalog? {
            get {
                var value = (catalogOwner ?? self).ownedCatalog
                if let selectedWorkspaceID { value?.selectedWorkspaceID = selectedWorkspaceID }
                return value
            }
            set {
                (catalogOwner ?? self).ownedCatalog = newValue
                selectedWorkspaceID = newValue?.selectedWorkspaceID
            }
        }
        var workspaceID: String? { workspace?.id }
        var panelVisibility: (layout: Bool, inbox: Bool) {
            (workspace?.layoutMapVisible ?? true, workspace?.inboxVisible ?? false)
        }

        func savePanelVisibility(layout: Bool, inbox: Bool) {
            let id = workspaceID
            enqueueMutation?({ [self] in
                guard let id, var saved = catalog,
                      let index = saved.workspaces.firstIndex(where: { $0.id == id }) else { return }
                guard saved.workspaces[index].layoutMapVisible != layout || saved.workspaces[index].inboxVisible != inbox else { return }
                saved.workspaces[index].layoutMapVisible = layout
                saved.workspaces[index].inboxVisible = inbox
                guard saved.revision < UInt64.max else { throw PolarStationNativeExecutorError.workspaceChanged }
                saved.revision += 1
                try await persistLifecycle(saved)
                if workspaceID == id { workspace = saved.workspaces[index] }
            }, false) { [weak self] error in
                if error != nil { self?.persistenceFailed?("Panel visibility could not be saved.") }
            }
        }

        private(set) var initialReadFinished = false
        var restoredWorkspaceIDs: [String] { catalog?.openWorkspaceIDs ?? [] }
        var isMutatingNativeTabs: Bool { tabGroupCoordinator.isMutatingNativeTabs }

        func owns(_ controller: TerminalController) -> Bool { tabGroupCoordinator.owns(controller) }
        func owns(_ window: NSWindow) -> Bool {
            guard let controller = window.windowController as? TerminalController else { return false }
            return owns(controller)
        }

        private var workspace: PolarStationNativeWorkspace?
        private var pendingWorkspace: PolarStationNativeWorkspace?
        private let surfaceCoordinator = PolarStationPaneSurfaceCoordinator()
        var displayCreated: ((String, StationPaneContentV1, PolarStationDisplaySurface, NSWindow?) -> Void)? {
            didSet { surfaceCoordinator.displayCreated = displayCreated }
        }
        var displayRetired: ((String) -> Void)? { didSet { surfaceCoordinator.displayRetired = displayRetired } }
        var displayInteraction: (() -> Void)? { didSet { surfaceCoordinator.displayInteraction = displayInteraction } }
        var displayActivated: ((String) -> Void)? { didSet { surfaceCoordinator.displayActivated = displayActivated } }
        var displaySelected: ((String) -> Void)? { didSet { surfaceCoordinator.displaySelected = displaySelected } }
        var terminalSelected: ((NSWindow, String, String, String) -> Void)? { didSet { surfaceCoordinator.terminalSelected = terminalSelected } }
        func focusInputSource(paneID:String, window:NSWindow) { surfaceCoordinator.focusInputSource(paneID:paneID,window:window) }
        func terminalSelection(paneID: String, window: NSWindow) -> String? {
            surfaceCoordinator.terminalSelection(paneID: paneID, window: window)
        }
        var displayFailed: ((String, String) -> Void)? { didSet { surfaceCoordinator.displayFailed = displayFailed } }
        func openConversation(_ content: StationPaneContentV1, replacing paneID: String? = nil) async throws {
            if !StationMutationScope.active {
                return try await serialized { [self] in try await openConversation(content, replacing: paneID) }
            }
            guard let current = workspace else { throw PolarStationNativeExecutorError.workspaceUnavailable }
            if let paneID, let tab = current.tabs.first(where: { $0.id == current.selectedTabID }),
               tab.panes.contains(where: { $0.id == paneID && $0.content != nil }) {
                try await performMutation(.replaceContent(paneID: paneID, content: content))
                return
            }
            try await performMutation(.openContent(tabID: current.selectedTabID,
                paneID: UUID().uuidString.lowercased(), content: content))
        }
        func closeContentPane(_ paneID: String) async throws {
            try await performMutation(.removePane(paneID))
        }
        var requestContent: ((URL, PolarTmuxPaneContentRequest) async throws -> PolarTmuxPaneContentResponse)? {
            didSet { surfaceCoordinator.requestContent = requestContent }
        }
        var submitRecovery: ((URL, PolarTmuxPaneContentRequest) async throws -> Void)? {
            didSet { surfaceCoordinator.submitRecovery = submitRecovery }
        }
        private lazy var tabGroupCoordinator: PolarStationNativeTabGroupCoordinator = {
            let coordinator = PolarStationNativeTabGroupCoordinator(
                surfaceForPane: { [weak self] paneID in
                    self?.surfaceCoordinator.surface(for: paneID)
                },
                toggleTabFocus: { [weak self] tabID in
                    self?.toggleTabFocus?(tabID)
                }
            )
            coordinator.nativeTransferRequested = { [weak self] controller in
                self?.restoreNativePresentation()
                self?.requestTabMove?(controller)
            }
            coordinator.nativeOrderChanged = { [weak self] order in
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    do { try await adoptNativeTabOrder(order) }
                    catch { persistenceFailed?("Tab order could not be saved.") }
                }
            }
            return coordinator
        }()
        private var installedTabStructures: [String: InstalledTabStructure] = [:]
        private var appliedFontSize: UInt8?
        private let ownedPersistence: PolarStationNativeWorkspacePersistence?
        private var persistence: PolarStationNativeWorkspacePersistence? { (catalogOwner ?? self).ownedPersistence }
        private let ownedPersistenceQueue = DispatchQueue(label: "com.polarealm.PolarStation.native-state", qos: .utility)
        private var persistenceQueue: DispatchQueue { (catalogOwner ?? self).ownedPersistenceQueue }
        private var ownedMutationSerial: UInt64 = 0
        private var nativeMutationSerial: UInt64 {
            get { (catalogOwner ?? self).ownedMutationSerial }
            set { (catalogOwner ?? self).ownedMutationSerial = newValue }
        }
        var persistenceFailed: (@MainActor (String) -> Void)?
        private var sessionRuntime: [
            PolarTmuxSessionKey: PolarTmuxSessionProjection
        ] = [:]
        private var inboxRecoverySnapshots: [
            String: PolarStationNativeWorkspace
        ] = [:]

        init(
            persistence: PolarStationNativeWorkspacePersistence? =
                try? .applicationDefault(),
            createInitialWorkspace: Bool = true
        ) {
            self.ownedPersistence = persistence
            initialReadFinished = persistence == nil
            surfaceCoordinator.surfaceLost = { [weak self] paneID in
                Task { @MainActor [weak self] in
                    guard let self, let workspace = self.workspace,
                          workspace.tabs.contains(where: { tab in
                              tab.panes.contains(where: { $0.id == paneID })
                          })
                    else { return }
                    self.installedTabStructures.removeAll()
                    _ = self.applyPhysical(workspace, focusIfActive: false)
                }
            }
            guard createInitialWorkspace else { return }
            let workspaceID = UUID().uuidString.lowercased()
            let tabID = UUID().uuidString.lowercased()
            let empty = PolarStationNativeWorkspace(
                id: workspaceID, name: "Workspace", revision: 1, selectedTabID: tabID, selectedPaneID: nil,
                managedTerminalFontSizePoints: 14,
                tabs: [.init(id: tabID, title: "Main", mode: .monitor, mainPaneID: nil, panes: [])]
            )
            catalog = .init(revision: 1, selectedWorkspaceID: workspaceID, workspaces: [empty])
            workspace = empty
            pendingWorkspace = empty
            // Load on the same serial persistence queue. A user mutation wins
            // over a late read; disk state never overwrites newer native intent.
            if let persistence {
                persistenceQueue.async { [weak self] in
                    let loaded = Result { try persistence.load() }
                    Task { @MainActor [weak self] in
                        guard let self else { return }
                        initialReadFinished = true
                        defer { catalogChanged?() }
                        guard nativeMutationSerial == 0 else { return }
                        switch loaded {
                        case .success(let saved?):
                            catalog = saved
                            workspace = saved.selectedWorkspace
                            pendingWorkspace = initialControllerIsAttached ? nil : saved.selectedWorkspace
                            if initialControllerIsAttached { _ = applyPhysical(saved.selectedWorkspace, focusIfActive: false) }
                        case .failure:
                            persistenceFailed?("Could not load native layout; existing tmux Sessions are unchanged.")
                        case .success(nil): break
                        }
                    }
                }
            }
        }

        /// Another native tab group shares this process catalog and serial
        /// persistence lane, while owning its physical surfaces and selection.
        convenience init(shared owner: PolarStationNativeExecutor, workspaceID: String? = nil) throws {
            self.init(persistence: nil, createInitialWorkspace: false)
            let root = owner.catalogOwner ?? owner
            catalogOwner = root
            guard var candidate = root.catalog else { throw PolarStationNativeExecutorError.workspaceUnavailable }
            if let workspaceID {
                try candidate.select(workspaceID)
                workspace = candidate.selectedWorkspace
                selectedWorkspaceID = workspaceID
            } else {
                workspace = nil
                selectedWorkspaceID = nil
            }
            pendingWorkspace = workspace
            sessionRuntime = owner.sessionRuntime
        }

        func rememberOpenWindows(_ ids: [String]) {
            enqueueMutation?({ [self] in
                guard var saved = catalog, saved.openWorkspaceIDs != ids else { return }
                saved.openWorkspaceIDs = ids.filter { id in saved.workspaces.contains { $0.id == id } }
                guard saved.revision < UInt64.max else { throw PolarStationNativeExecutorError.workspaceChanged }
                saved.revision += 1
                let bound = selectedWorkspaceID
                try await persistLifecycle(saved)
                selectedWorkspaceID = bound
            }, false) { [weak self] error in
                if error != nil { self?.persistenceFailed?("Open windows could not be saved.") }
            }
        }

        func flushPersistence() async throws {
            guard let saved = catalog, let persistence else { return }
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                persistenceQueue.async {
                    do { try persistence.save(saved); continuation.resume() }
                    catch { continuation.resume(throwing: error) }
                }
            }
        }

        @discardableResult
        func attach(_ controller: TerminalController) -> Bool {
            let isInitial = tabGroupCoordinator.attach(controller)
            guard let pendingWorkspace else { showWorkspaceChooser(); return isInitial }
            self.pendingWorkspace = nil
            applyPhysical(pendingWorkspace, focusIfActive: true)
            return isInitial
        }

        func observeNativeTabSelection(
            _ controller: TerminalController
        ) -> NativeTabSelectionIngressState.Intent? {
            tabGroupCoordinator.observeNativeTabSelection(controller)
        }

        func completeNativeTabSelection(
            _ intent: NativeTabSelectionIngressState.Intent
        ) {
            tabGroupCoordinator.completeNativeTabSelection(intent)
        }

        var initialControllerIsAttached: Bool {
            tabGroupCoordinator.initialControllerIsAttached
        }

        /// Startup presentation is eligible only after every projected Tab has a
        /// live native controller and its exact physical Surface structure is the
        /// one currently installed. Host lifecycle actions do not describe this
        /// AppKit-owned readiness boundary.
        var startupPresentationIsReady: Bool {
            guard pendingWorkspace == nil,
                  let workspace,
                  !workspace.tabs.isEmpty
            else { return false }
            return physicalPresentationMatches(workspace)
        }

        private func physicalPresentationMatches(
            _ candidate: PolarStationNativeWorkspace
        ) -> Bool {
            !candidate.tabs.isEmpty && candidate.tabs.allSatisfy { tab in
                installedTabStructures[tab.id] == InstalledTabStructure(tab: tab)
                    && tabGroupCoordinator.controller(for: tab.id)?.window != nil
            }
        }

        func requestStartupFullscreen() {
            tabGroupCoordinator.requestStartupFullscreen()
        }

        func windowDidEnterFullScreen(_ window: NSWindow) {
            tabGroupCoordinator.windowDidEnterFullScreen(window)
        }

        func windowDidFailToEnterFullScreen(_ window: NSWindow) {
            tabGroupCoordinator.windowDidFailToEnterFullScreen(window)
        }

        func windowDidExitFullScreen(_ window: NSWindow) {
            tabGroupCoordinator.windowDidExitFullScreen(window)
        }

        func authorizeLifecycle(_ action: String) throws {
            guard let catalog else { throw PolarStationNativeExecutorError.workspaceUnavailable }
            _ = try catalog.lifecycle(action, bound: workspaceID)
        }
        func prepareUnboundWindow() {
            workspace = nil
            pendingWorkspace = nil
            selectedWorkspaceID = nil
        }
        func reset(restoreSelection: Bool = true) {
            windowGeneration &+= 1
            chooserView?.removeFromSuperview()
            chooserView = nil
            workspace = restoreSelection ? catalog?.selectedWorkspace : nil
            pendingWorkspace = workspace
            installedTabStructures.removeAll()
            appliedFontSize = nil
            tabGroupCoordinator.reset()
            surfaceCoordinator.reset()
        }

        func apply(_ projection: ControlCenterProjectionV1) {
            _ = projection
            // Session facts cannot import or mutate native layout.
        }

        func apply(_ update: PolarTmuxSessionFeedUpdate) {
            if !StationMutationScope.active {
                enqueueMutation?({ [self] in apply(update) }, false) { _ in }
                return
            }
            switch update {
            case let .snapshot(snapshot):
                sessionRuntime = Dictionary(
                    uniqueKeysWithValues: snapshot.sessions.map {
                        ($0.key, $0)
                    }
                )
            case let .event(event):
                for change in event.changes {
                    switch change {
                    case let .upsert(session):
                        sessionRuntime[session.key] = session
                    case let .remove(key):
                        sessionRuntime.removeValue(forKey: key)
                    }
                }
            }
            guard var catalog else { return }
            let boundID = workspace?.id
            let sessions = Array(sessionRuntime.values)
            for index in catalog.workspaces.indices {
                catalog.workspaces[index].mergeSessionRuntime(sessions)
            }
            self.catalog = catalog
            selectedWorkspaceID = boundID
            workspace = boundID.flatMap { id in catalog.workspaces.first { $0.id == id } }
            Task { @MainActor [weak self] in
                guard let self else { return }
                await StationMutationScope.$active.withValue(false) { await self.retireEndedSessionPanes() }
            }
        }

        private func retireEndedSessionPanes() async {
            let panes = self.workspace?.tabs.flatMap(\.panes) ?? []
            for pane in panes {
                guard pane.content == nil, let target = pane.executionTargetID, let reference = pane.tmuxSessionID,
                      self.sessionRuntime.values.contains(where: { $0.key.executionTarget == target && $0.key.tmuxSessionID == reference && $0.lifecycle == .ended }),
                      self.workspace?.tabs.flatMap(\.panes).contains(where: { $0.id == pane.id && $0.executionTargetID == target && $0.tmuxSessionID == reference }) == true else { continue }
                do { try await self.performMutation(.removePane(pane.id)) }
                catch { self.persistenceFailed?("The Session ended, but its Pane could not be removed: \(error)") }
            }
        }

        func performImmediate(
            _ operation: ControlCenterIntentOperationV1
        ) async throws {
            if !StationMutationScope.active {
                return try await serialized { [self] in try await performImmediate(operation) }
            }
            switch operation {
            case let .deleteTab(tabID, _, _):
                try await performNativeAction("close_tab", tabID: tabID)
                return
            case let .renameTab(tabID, title):
                try await performNativeAction("rename_tab", tabID: tabID, title: title)
                return
            case .invokeProjectedAction(actionID: "window.close"):
                try authorizeLifecycle("close")
                await unbindWorkspace()
                if let controller = TerminalController.all.first(where: { owns($0) }),
                   let window = controller.window {
                    controller.closeWindowImmediately()
                    // Fullscreen teardown may order its closing window front.
                    // Complete native close without allowing it to reappear.
                    window.orderOut(nil)
                }
                return
            case let .invokeConfirmedAction(actionID, confirmationID)
                where actionID == "workspace.restore":
                guard !confirmationID.isEmpty else {
                    throw PolarStationNativeWorkspaceError.invalidStructure
                }
                try await restoreSavedLayout()
                return
            case let .createWorkspace(name):
                try await performCreateWorkspace(name: name)
                return
            case let .selectWorkspace(workspaceID):
                try await performSelectWorkspace(workspaceID)
                return
            case .deleteWorkspace:
                _ = try catalog?.lifecycle("delete", bound: workspaceID)
                return
            case let .invokeConfirmedAction(actionID, confirmationID) where actionID == "workspace.archive":
                guard confirmationID == workspaceID else { throw PolarStationNativeExecutorError.workspaceChanged }
                try await performArchiveWorkspace(confirmationID)
                return
            case let .invokeConfirmedAction(actionID, confirmationID) where actionID == "workspace.unarchive":
                try await performRestoreWorkspace(confirmationID)
                return
            default:
                break
            }
            guard let current = workspace else {
                throw PolarStationNativeExecutorError.workspaceUnavailable
            }
            guard let mutation = try current.mutation(for: operation) else {
                return
            }
            try await performMutation(mutation)
            if let id=workspace?.selectedPaneID, let pane=workspace?.tabs.flatMap(\.panes).first(where:{$0.id==id}), pane.content?.kind == .lgpText {
                displayActivated?(id)
            }
        }

        private var reservedSessionPanes: [String: (tabID: String, count: Int)] = [:]
        func reserveSessionPanes(id: String, workspaceID: String, tabID: String, count: Int) -> String? {
            guard let current = workspace, current.id == workspaceID, let tab = current.tabs.first(where: { $0.id == tabID }) else { return "The Tab that opened Add Session no longer exists." }
            let reserved = reservedSessionPanes.filter { $0.key != id && $0.value.tabID == tabID }.values.reduce(0) { $0 + $1.count }
            guard tab.panes.count + reserved + count <= 9 else { return "This Tab has insufficient room (maximum nine Panes)." }
            reservedSessionPanes[id] = (tabID, count)
            return nil
        }
        func releaseSessionPanes(id: String) { reservedSessionPanes.removeValue(forKey: id) }
        private func validateReservedPanes(_ candidate: PolarStationNativeWorkspace, excluding: String? = nil) throws {
            for tab in candidate.tabs {
                let reserved = reservedSessionPanes.filter { $0.key != excluding && $0.value.tabID == tab.id }.values.reduce(0) { $0 + $1.count }
                if tab.panes.count + reserved > 9 { throw PolarStationNativeWorkspaceError.invalidBinding }
            }
        }

        func quickCreateContext() -> (workspaceID: String, tabID: String, source: PolarStationNativePane?, paneCount: Int)? {
            guard let current = workspace,
                  let selected = tabGroupCoordinator.selectedManagedTabID(),
                  let tab = current.tabs.first(where: { $0.id == selected }),
                  let controller = tabGroupCoordinator.controller(for: selected),
                  controller.window != nil else { return nil }
            let source: PolarStationNativePane?
            if tab.mode == .focus { source = tab.panes.first(where: { $0.id == tab.mainPaneID }) }
            else { source = tab.panes.first(where: { surfaceCoordinator.surface(for: $0.id) === controller.focusedSurface }) }
            return (current.id, tab.id, source, tab.panes.count)
        }

        func ensureTerminalVisible(sourcePaneID: String, target: [String:Any]) async throws {
            if !StationMutationScope.active {
                return try await serialized { [self] in try await ensureTerminalVisible(sourcePaneID: sourcePaneID, target: target) }
            }
            guard var candidate = workspace, var saved = catalog,
                  let tab = candidate.tabs.first(where: { $0.panes.contains(where: { $0.id == sourcePaneID }) }),
                  tabGroupCoordinator.selectedManagedTabID() == tab.id,
                  let controller = tabGroupCoordinator.controller(for:tab.id),
                  let window = controller.window, window.isVisible,
                  let execution = target["execution_target_id"] as? String,
                  let session = target["tmux_session_id"] as? String else {
                throw NSError(domain:"PolarStation.SessionView",code:1,userInfo:[NSLocalizedDescriptionKey:"The originating tab or target is unavailable. Task not sent."])
            }
            let responder = window.firstResponder
            var changed = false
            if !tab.panes.contains(where: { $0.executionTargetID == execution && $0.tmuxSessionID == session }) {
                guard tab.panes.count < 9 else { throw NSError(domain:"PolarStation.SessionView",code:2,userInfo:[NSLocalizedDescriptionKey:"This tab is full. Task not sent."]) }
                try candidate.apply(.addSessions(tabID:tab.id,sessions:[.init(executionTargetID:execution,tmuxSessionID:session)])); changed = true
            }
            if tab.mode == .focus { try candidate.apply(.toggleMode(tab.id)); changed = true }
            if changed {
                try validateReservedPanes(candidate,excluding:nil)
                candidate.structureFingerprint = candidate.structureSnapshot().structureFingerprint
                try saved.replaceSelected(with:candidate)
                try await stageCatalog(saved)
                if let source = surfaceCoordinator.surface(for:sourcePaneID) {
                    controller.focusedSurfaceDidChange(to:source)
                }
                if let responder { window.makeFirstResponder(responder) }
            }
            guard let pane = workspace?.tabs.first(where:{$0.id == tab.id})?.panes.first(where:{$0.executionTargetID == execution && $0.tmuxSessionID == session}) else {
                throw PolarStationNativeWorkspaceError.invalidBinding
            }
            try await surfaceCoordinator.waitForTerminalView(paneID:pane.id,binding:.init(executionTargetID:execution,tmuxSessionID:session))
        }

        func appendCreatedSessions(workspaceID: String, tabID: String, sessions: [SessionRowProjectionV1.Identity], reservationID: String? = nil) async throws {
            if !StationMutationScope.active {
                return try await serialized { [self] in try await appendCreatedSessions(workspaceID: workspaceID, tabID: tabID, sessions: sessions, reservationID: reservationID) }
            }
            defer { if let reservationID { releaseSessionPanes(id: reservationID) } }
            guard var candidate = workspace, candidate.id == workspaceID, var candidateCatalog = catalog,
                  let tab = candidate.tabs.first(where: { $0.id == tabID })
            else { throw PolarStationNativeWorkspaceError.invalidBinding }
            var seen = Set(tab.panes.map { "\($0.executionTargetID ?? "")\n\($0.tmuxSessionID ?? "")" })
            let additions = sessions.filter { seen.insert("\($0.executionTargetID)\n\($0.tmuxSessionID)").inserted }
            guard tab.panes.count + additions.count <= 9 else { throw PolarStationNativeWorkspaceError.invalidBinding }
            guard !additions.isEmpty else { return }
            try candidate.apply(.addSessions(tabID: tabID, sessions: additions))
            try validateReservedPanes(candidate, excluding: reservationID)
            candidate.structureFingerprint = candidate.structureSnapshot().structureFingerprint
            try candidateCatalog.replaceSelected(with: candidate)
            try await stageCatalog(candidateCatalog)
        }

        func inboxOrigins(for key: PolarTmuxSessionKey) -> [TerminalOriginHandleV1] {
            guard let catalog else { return [] }
            return catalog.workspaces.flatMap { workspace in
                let token = "\(workspace.id):\(workspace.revision)"
                inboxRecoverySnapshots[token] = workspace
                return workspace.tabs.flatMap { tab in
                    tab.panes.filter { $0.executionTargetID == key.executionTarget && $0.tmuxSessionID == key.tmuxSessionID }.map { pane in
                        TerminalOriginHandleV1(workspaceID: workspace.id, tabID: tab.id, paneID: pane.id,
                            executionTargetID: key.executionTarget, tmuxSessionID: key.tmuxSessionID,
                            consumerGeneration: workspace.revision, nativeRecoveryToken: token)
                    }
                }
            }
        }

        func deliverInboxDraft(
            origin: TerminalOriginHandleV1,
            text: String,
            mode: TerminalDeliveryModeV1
        ) async throws {
            guard let current = workspace,
                  current.id == origin.workspaceID
            else {
                throw PolarStationNativeExecutorError.workspaceUnavailable
            }
            if !current.tabs.contains(where: { tab in
                tab.id == origin.tabID && tab.panes.contains(where: {
                    $0.id == origin.paneID && $0.executionTargetID == origin.executionTargetID
                    && $0.tmuxSessionID == origin.tmuxSessionID
                })
            }) {
                try await restoreInboxOrigin(origin)
            }
            guard let restored = workspace,
                  restored.id == origin.workspaceID,
                  restored.tabs.contains(where: { tab in
                      tab.id == origin.tabID && tab.panes.contains(where: {
                          $0.id == origin.paneID && $0.executionTargetID == origin.executionTargetID
                          && $0.tmuxSessionID == origin.tmuxSessionID
                      })
                  }),
                  let surface = surfaceCoordinator.surface(for: origin.paneID)
            else {
                throw PolarStationNativeExecutorError.presentationRestoreFailed
            }
            try await performImmediate(.activatePane(paneID: origin.paneID))
            surface.polarStationSendText(text + (mode == .send ? "\r" : ""))
        }

        func captureInboxRecoverySnapshots(_ page: InboxPageV1) {
            guard let workspace else { return }
            for message in page.messages where
                message.origin.workspaceID == workspace.id
                && workspace.tabs.contains(where: { tab in
                    tab.id == message.origin.tabID
                    && tab.panes.contains(where: { $0.id == message.origin.paneID })
                })
            {
                inboxRecoverySnapshots[message.origin.nativeRecoveryToken] = workspace
            }
        }

        private func restoreInboxOrigin(
            _ origin: TerminalOriginHandleV1
        ) async throws {
            if !StationMutationScope.active {
                return try await serialized { [self] in try await restoreInboxOrigin(origin) }
            }
            guard var candidateCatalog = catalog,
                  candidateCatalog.workspaces.contains(where: { $0.id == origin.workspaceID }),
                  let snapshot = inboxRecoverySnapshots[origin.nativeRecoveryToken],
                  snapshot.id == origin.workspaceID,
                  snapshot.tabs.contains(where: { tab in
                      tab.id == origin.tabID
                      && tab.panes.contains(where: { $0.id == origin.paneID })
                  })
            else {
                throw PolarStationNativeExecutorError.presentationRestoreFailed
            }
            try candidateCatalog.replaceSelected(with: snapshot)
            try await stageCatalog(candidateCatalog)
        }

        private func restoreSavedLayout() async throws {
            guard let saved = catalog?.selectedWorkspace,
                  initialControllerIsAttached
            else {
                throw PolarStationNativeExecutorError.workspaceUnavailable
            }
            try await surfaceCoordinator.reconnectSavedPaneContent(to: saved)
            installedTabStructures.removeAll()
            workspace = saved
            applyPhysical(saved, focusIfActive: true)
            guard startupPresentationIsReady else {
                throw PolarStationNativeExecutorError.presentationRestoreFailed
            }
        }

        var tabMoveDestinations: [(id: String, name: String)] {
            (catalog?.workspaces ?? []).filter { $0.id != workspaceID && !(catalog?.archivedWorkspaceIDs ?? []).contains($0.id) }.map { ($0.id, $0.name) }
        }
        func restoreNativePresentation() {
            if let workspace { _ = applyPhysical(workspace, focusIfActive: false) }
        }
        func refreshTransferredWorkspace() {
            guard let id = workspaceID, let saved = catalog?.workspaces.first(where: { $0.id == id }) else { return }
            let previous = workspace
            workspace = saved
            if let previous { _ = commitPhysicalWorkspace(previous: previous, candidate: saved); scheduleRetirement(from: previous, to: saved) }
            else { _ = applyPhysical(saved, focusIfActive: true) }
            catalogChanged?()
        }
        func moveNativeTab(_ tabID: String, to targetID: String?, newWorkspaceName: String?) async throws -> String {
            guard StationMutationScope.active, let source = workspaceID, var candidate = catalog else { throw PolarStationNativeExecutorError.workspaceUnavailable }
            let target: String
            if let targetID { target = targetID }
            else {
                target = UUID().uuidString.lowercased()
                try candidate.createAndSelect(id: target, name: newWorkspaceName ?? "Workspace", tabID: UUID().uuidString.lowercased(),
                    managedTerminalFontSizePoints: workspace?.managedTerminalFontSizePoints ?? 14, advanceRevision: false)
                candidate.selectedWorkspaceID = source
            }
            try candidate.moveTab(tabID, from: source, to: target, replacementTabID: UUID().uuidString.lowercased())
            try await persistLifecycle(candidate)
            refreshTransferredWorkspace()
            return target
        }

        func nativePaneID(_ surface: Ghostty.SurfaceView) -> String? {
            workspace?.tabs.flatMap(\.panes).first { surfaceCoordinator.surface(for: $0.id) === surface }?.id
        }
        func nativeTabID(_ controller: TerminalController) -> String? { tabGroupCoordinator.tabID(for: controller) }
        func nativeActionDecision(_ action: String, tabID: String?) throws -> WorkspaceLifecycleDecision {
            guard let saved = catalog else { throw PolarStationNativeExecutorError.workspaceUnavailable }
            return try decideWorkspaceLifecycle(action: "native." + action, ids: saved.workspaces.map(\.id),
                archived: saved.archivedWorkspaceIDs ?? [], open: saved.openWorkspaceIDs ?? [],
                selected: saved.selectedWorkspaceID, bound: workspaceID,
                tabCount: workspace?.tabs.count ?? 0, tabIndex: workspace?.tabs.firstIndex { $0.id == tabID })
        }
        func performNativeAction(_ action: String, tabID: String, title: String? = nil) async throws {
            if !StationMutationScope.active {
                return try await serialized { [self] in try await performNativeAction(action, tabID: tabID, title: title) }
            }
            let decision = try nativeActionDecision(action, tabID: tabID)
            if decision.effect == "close" {
                try await performImmediate(.invokeProjectedAction(actionID: "window.close"))
                return
            }
            guard var candidate = workspace, var saved = catalog,
                  let index = candidate.tabs.firstIndex(where: { $0.id == tabID }) else { throw PolarStationNativeExecutorError.workspaceChanged }
            switch decision.effect {
            case "rename_tab":
                let title = (title ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
                try candidate.apply(.renameTab(id: tabID, title: title))
            case "close_tab", "close_other_tabs", "close_right_tabs":
                let ids = candidate.tabs.enumerated().filter { i, tab in
                    decision.effect == "close_tab" ? tab.id == tabID : (decision.effect == "close_other_tabs" ? tab.id != tabID : i > index)
                }.map { $0.element.id }
                for id in ids { try candidate.apply(.deleteTab(tabID: id, replacementTabID: nil)) }
            default: throw PolarStationNativeWorkspaceError.invalidStructure
            }
            candidate.structureFingerprint = candidate.structureSnapshot().structureFingerprint
            try saved.replaceSelected(with: candidate)
            try await stageCatalog(saved)
        }

        private func adoptNativeTabOrder(_ order: [String]) async throws {
            if !StationMutationScope.active {
                return try await serialized { [self] in try await adoptNativeTabOrder(order) }
            }
            guard var candidate = workspace, var candidateCatalog = catalog,
                  order != candidate.tabs.map(\.id),
                  order.count == candidate.tabs.count,
                  Set(order) == Set(candidate.tabs.map(\.id)) else { return }
            // Reuse the Layout mutation and persistence lane, retaining selected
            // Tab/Pane IDs and the already-moved native surfaces.
            for (index, id) in order.enumerated() {
                try candidate.apply(.reorderTab(tabID: id, toIndex: index))
            }
            candidate.structureFingerprint = candidate.structureSnapshot().structureFingerprint
            try candidateCatalog.replaceSelected(with: candidate)
            try await stageCatalog(candidateCatalog)
        }

        private func performMutation(
            _ mutation: PolarStationNativeWorkspaceMutation
        ) async throws {
            if !StationMutationScope.active {
                return try await serialized { [self] in try await performMutation(mutation) }
            }
            guard var candidate = workspace,
                  var candidateCatalog = catalog
            else {
                throw PolarStationNativeExecutorError.workspaceUnavailable
            }
            try candidate.apply(mutation)
            try validateReservedPanes(candidate)
            if case .replaceLostSession = mutation, candidate == workspace { return }
            candidate.structureFingerprint =
                candidate.structureSnapshot().structureFingerprint
            try candidateCatalog.replaceSelected(with: candidate)
            try await stageCatalog(candidateCatalog)
        }

        private func performCreateWorkspace(
            name: String
        ) async throws {
            guard var candidateCatalog = catalog
            else {
                throw PolarStationNativeExecutorError.workspaceUnavailable
            }
            _ = try candidateCatalog.lifecycle("create", bound: workspaceID)
            try candidateCatalog.createAndSelect(
                id: UUID().uuidString.lowercased(),
                name: name,
                tabID: UUID().uuidString.lowercased(),
                managedTerminalFontSizePoints:
                workspace?.managedTerminalFontSizePoints ?? 14
            )
            var selected = candidateCatalog.selectedWorkspace
            selected.structureFingerprint =
                selected.structureSnapshot().structureFingerprint
            try candidateCatalog.replaceSelected(
                with: selected,
                advanceRevision: false
            )
            try await stageCatalog(candidateCatalog)
        }

        private func performSelectWorkspace(
            _ workspaceID: String
        ) async throws {
            guard var candidateCatalog = catalog else {
                throw PolarStationNativeExecutorError.workspaceUnavailable
            }
            _ = try candidateCatalog.lifecycle("open", bound: self.workspaceID, target: workspaceID)
            let previousID = self.workspaceID
            if previousID == workspaceID { return }
            try candidateCatalog.select(workspaceID, openingUnboundWindow: workspace == nil)
            try await stageCatalog(candidateCatalog)
            if previousID != workspaceID { windowGeneration &+= 1 }
            await retireEndedSessionPanes()
        }

        private func persistLifecycle(_ candidate: PolarStationNativeWorkspaceCatalog) async throws {
            guard StationMutationScope.active, let revision = catalog?.revision,
                  revision < UInt64.max, candidate.revision == revision + 1 else { throw PolarStationNativeExecutorError.workspaceChanged }
            if let persistence {
                try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                    persistenceQueue.async {
                        do { try persistence.save(candidate); continuation.resume() }
                        catch { continuation.resume(throwing: error) }
                    }
                }
            }
            nativeMutationSerial &+= 1
            catalog = candidate
        }

        private func performArchiveWorkspace(_ id: String) async throws {
            guard var candidate = catalog, workspaceID == id else { throw PolarStationNativeExecutorError.workspaceChanged }
            try candidate.archive(id)
            try await persistLifecycle(candidate)
            await unbindWorkspace()
            catalogChanged?()
        }

        private func performRestoreWorkspace(_ id: String) async throws {
            guard var candidate = catalog else { throw PolarStationNativeExecutorError.workspaceUnavailable }
            try candidate.restore(id)
            let boundID = workspaceID
            try await persistLifecycle(candidate)
            selectedWorkspaceID = boundID
            catalogChanged?()
            showWorkspaceChooser()
        }

        func unbindWorkspace() async {
            let previous = workspace
            workspace = nil
            pendingWorkspace = nil
            selectedWorkspaceID = nil
            tabGroupCoordinator.unbind { [self] controller in
                let placeholder = surfaceCoordinator.placeholder(for: "unbound-window", controller: controller, fontSize: 14)
                replaceManagedSurfaceTree(.init(view: placeholder), in: controller)
            }
            if let previous {
                var empty = previous
                empty.tabs = [] // physical retirement set only; never persisted as a Workspace
                await surfaceCoordinator.retireAbsentPaneContent(from: previous, to: empty)
            }
            installedTabStructures.removeAll()
            showWorkspaceChooser()
        }

        private func stageCatalog(
            _ candidateCatalog: PolarStationNativeWorkspaceCatalog,
            persist: Bool = true
        ) async throws {
            let previous = workspace
            guard let revision = catalog?.revision, revision < UInt64.max,
                  candidateCatalog.revision == revision + 1 else {
                throw PolarStationNativeExecutorError.workspaceChanged
            }
            var candidateCatalog = candidateCatalog
            let candidate = candidateCatalog.selectedWorkspace
            let priorOpen = catalog?.openWorkspaceIDs ?? [previous?.id].compactMap { $0 }
            var open: [String] = []
            for id in priorOpen {
                let next = id == previous?.id ? candidate.id : id
                if candidateCatalog.workspaces.contains(where: { $0.id == next }) && !open.contains(next) { open.append(next) }
            }
            if !open.contains(candidate.id) { open.append(candidate.id) }
            candidateCatalog.openWorkspaceIDs = open
            if persist { try await persistLifecycle(candidateCatalog) }
            else { catalog = candidateCatalog }
            workspace = candidate
            chooserView?.removeFromSuperview()
            chooserView = nil
            catalogChanged?()
            let physicallyApplied: Bool
            if let previous {
                physicallyApplied = commitPhysicalWorkspace(previous: previous, candidate: candidate)
                scheduleRetirement(from: previous, to: candidate)
            } else {
                physicallyApplied = applyPhysical(candidate, focusIfActive: true)
            }
            if !physicallyApplied { throw PolarStationNativeExecutorError.physicalRepairPending }
        }

        /// The only one-way boundary in a native Workspace mutation. Everything
        /// above this call is fallible and reversible; AppKit tree closure and
        /// Pane process retirement are never rolled back afterward.
        private func commitPhysicalWorkspace(
            previous: PolarStationNativeWorkspace,
            candidate: PolarStationNativeWorkspace
        ) -> Bool {
            detachRetiredSurfaceTrees(from: previous, to: candidate)
            return applyPhysical(
                candidate,
                retireAbsent: true,
                focusIfActive: true,
                preservePendingFocus: true
            )
        }

        private func detachRetiredSurfaceTrees(
            from previous: PolarStationNativeWorkspace,
            to candidate: PolarStationNativeWorkspace
        ) {
            let retainedTabIDs = Set(candidate.tabs.map(\.id))
            for tab in previous.tabs where !retainedTabIDs.contains(tab.id) {
                guard let controller = tabGroupCoordinator.controller(for: tab.id)
                else { continue }
                let placeholder = surfaceCoordinator.placeholder(
                    for: tab.id,
                    controller: controller,
                    fontSize: candidate.managedTerminalFontSizePoints
                )
                replaceManagedSurfaceTree(.init(view: placeholder), in: controller)
            }
        }

        private func scheduleRetirement(
            from previous: PolarStationNativeWorkspace,
            to candidate: PolarStationNativeWorkspace
        ) {
            Task { [weak self] in
                await self?.surfaceCoordinator.retireAbsentPaneContent(
                    from: previous,
                    to: candidate
                )
            }
        }

        /// Executes an allowlisted Companion Workspace request against the same
        /// native single-writer table used by direct AppKit interaction. The Rust
        /// Host only relays this envelope and never mirrors the resulting layout.
        func performWorkspaceRelay(
            _ value: PolarTmuxJSONValue
        ) async -> PolarTmuxJSONValue {
            let request: NativeWorkspaceRelayRequest
            do {
                request = try NativeWorkspaceRelayRequest(value)
            } catch {
                return NativeWorkspaceRelayRequest.invalidEnvelopeResponse(value)
            }
            guard let currentCatalog = catalog,
                  let current = workspace
            else {
                return request.errorResponse(
                    code: "desktop_workspace_unavailable",
                    message: "Polar Station has not loaded its native Workspace",
                    retryable: true,
                    revision: 0
                )
            }
            let scopedWorkspace: PolarStationNativeWorkspace
            if let workspaceID = request.scopeWorkspaceID {
                guard let scoped = currentCatalog.workspaces.first(where: {
                    $0.id == workspaceID
                }) else {
                    return request.errorResponse(
                        code: "workspace_not_found",
                        message: "The native Workspace does not exist",
                        retryable: false,
                        revision: currentCatalog.revision
                    )
                }
                scopedWorkspace = scoped
                if scoped.id != current.id, !request.command.isRead {
                    return request.errorResponse(
                        code: "workspace_not_found",
                        message: "The native Workspace is not selected",
                        retryable: false,
                        revision: scoped.revision
                    )
                }
            } else {
                scopedWorkspace = current
            }
            guard request.accepts(
                catalogRevision: currentCatalog.revision,
                workspaceRevision: scopedWorkspace.revision
            ) else {
                return request.errorResponse(
                    code: "workspace_revision_conflict",
                    message: "The native Workspace changed; refresh and retry",
                    retryable: true,
                    revision: request.scopeWorkspaceID == nil
                        ? currentCatalog.revision : scopedWorkspace.revision
                )
            }

            do {
                if case let .selectWorkspace(workspaceID) = request.command {
                    guard catalog?.workspaces.contains(where: {
                        $0.id == workspaceID
                    }) == true else {
                        throw NativeWorkspaceRelayError.workspaceNotFound
                    }
                    try await performSelectWorkspace(workspaceID)
                } else if let mutation = try relayMutation(
                    for: request.command,
                    workspace: current
                ) {
                    try await performMutation(mutation)
                } else if request.command == .repair {
                    applyPhysical(
                        current,
                        retireAbsent: true,
                        focusIfActive: false
                    )
                }
                guard let acceptedCatalog = catalog,
                      let accepted = workspace
                else {
                    throw PolarStationNativeExecutorError.workspaceUnavailable
                }
                return request.successResponse(
                    projection: companionProjection(
                        acceptedCatalog,
                        scopeWorkspaceID: request.scopeWorkspaceID
                    ),
                    revision: request.scopeWorkspaceID == nil
                        ? acceptedCatalog.revision : accepted.revision
                )
            } catch let error as NativeWorkspaceRelayError {
                return request.errorResponse(
                    code: error.code,
                    message: error.message,
                    retryable: false,
                    revision: request.scopeWorkspaceID == nil
                        ? currentCatalog.revision : scopedWorkspace.revision
                )
            } catch {
                return request.errorResponse(
                    code: "desktop_workspace_apply_failed",
                    message: "The native Workspace change could not be applied",
                    retryable: true,
                    revision: request.scopeWorkspaceID == nil
                        ? (catalog?.revision ?? currentCatalog.revision)
                        : (workspace?.revision ?? scopedWorkspace.revision)
                )
            }
        }

        private func relayMutation(
            for command: NativeWorkspaceRelayCommand,
            workspace: PolarStationNativeWorkspace
        ) throws -> PolarStationNativeWorkspaceMutation? {
            switch command {
            case .getCatalog, .getWorkspace, .open, .repair:
                return nil
            case .selectWorkspace:
                return nil
            case let .selectTab(tabID):
                return .selectTab(tabID)
            case let .setDisplayMode(tabID, mode):
                guard let tab = workspace.tabs.first(where: { $0.id == tabID })
                else { throw NativeWorkspaceRelayError.tabNotFound }
                return tab.mode == mode ? nil : .toggleMode(tabID)
            case let .selectFocusPane(tabID, paneID):
                guard let tab = workspace.tabs.first(where: { $0.id == tabID })
                else { throw NativeWorkspaceRelayError.tabNotFound }
                guard tab.panes.contains(where: { $0.id == paneID }) else {
                    throw NativeWorkspaceRelayError.paneNotFound
                }
                return .activatePane(paneID)
            case .focusPrevious:
                return .cycleFocus(.previous)
            case .focusNext:
                return .cycleFocus(.next)
            case let .focusSidebarSlot(slot):
                guard let tab = workspace.tabs.first(where: {
                    $0.id == workspace.selectedTabID
                }), tab.mode == .focus, tab.visualPaneIDs.indices.contains(slot)
                else { throw NativeWorkspaceRelayError.paneNotFound }
                return .activatePane(tab.visualPaneIDs[slot])
            }
        }

        private func companionProjection(
            _ catalog: PolarStationNativeWorkspaceCatalog,
            scopeWorkspaceID: String?
        ) -> PolarTmuxJSONValue {
            let sourceWorkspaces = scopeWorkspaceID.map { workspaceID in
                catalog.workspaces.filter { $0.id == workspaceID }
            } ?? catalog.workspaces
            let workspaces = sourceWorkspaces.map {
                workspace -> PolarTmuxJSONValue in
                let tabs = workspace.tabs.map { tab -> PolarTmuxJSONValue in
                    let canonicalPaneIDs = tab.panes.map(\.id)
                    let layout = try? PolarStationNativeLayoutFamily.layout(
                        orderedPaneIDs: canonicalPaneIDs,
                        mode: tab.mode,
                        mainPaneID: tab.mainPaneID
                    )
                    let slots = layout?.slots.map { slot -> PolarTmuxJSONValue in
                        let canonicalIndex = canonicalPaneIDs.firstIndex(
                            of: slot.surfaceID
                        ) ?? slot.visualIndex
                        let role: PolarTmuxJSONValue
                        switch slot.role {
                        case .monitor:
                            role = .object(["type": .string("monitor")])
                        case .main:
                            role = .object(["type": .string("main")])
                        case let .sidebar(slot):
                            role = .object([
                                "type": .string("sidebar"),
                                "slot": .number(Double(slot)),
                            ])
                        }
                        return .object([
                            "pane_id": .string(slot.surfaceID),
                            "canonical_index": .number(Double(canonicalIndex)),
                            "visual_index": .number(Double(slot.visualIndex)),
                            "column": .number(Double(slot.column)),
                            "row": .number(Double(slot.row)),
                            "width": .object([
                                "numerator": .number(Double(slot.width.numerator)),
                                "denominator": .number(Double(slot.width.denominator)),
                            ]),
                            "height": .object([
                                "numerator": .number(Double(slot.height.numerator)),
                                "denominator": .number(Double(slot.height.denominator)),
                            ]),
                            "role": role,
                        ])
                    } ?? []
                    let panes = tab.panes.map { pane -> PolarTmuxJSONValue in
                        if let content = pane.content {
                            return .object(["id": .string(pane.id), "content_kind": .string(content.kind.rawValue),
                                "target_label": .string(content.targetID), "session_name": .string(content.thread),
                                "transport_status": .string("offline")])
                        }
                        guard let target = pane.executionTargetID, let session = pane.tmuxSessionID else { return .null }
                        let key = PolarTmuxSessionKey(executionTarget: target, tmuxSessionID: session)
                        let runtime = sessionRuntime[key]
                        return .object([
                            "id": .string(pane.id),
                            "target_label": .string(
                                target == "local" ? "This computer" : target
                            ),
                            "session_name": .string(
                                runtime?.displayName ?? session
                            ),
                            "transport_status": .string(
                                pane.transportStatus.rawValue
                            ),
                        ])
                    }
                    return .object([
                        "id": .string(tab.id),
                        "title": .string(tab.title),
                        "display_mode": .string(tab.mode.rawValue),
                        "focused_pane_id": tab.mainPaneID.map {
                            .string($0)
                        } ?? .null,
                        "pane_order": .array(canonicalPaneIDs.map {
                            .string($0)
                        }),
                        "layout": .object([
                            "pane_order": .array(canonicalPaneIDs.map {
                                .string($0)
                            }),
                            "slots": .array(slots),
                            "fingerprint": .string(
                                "native-\(workspace.revision)-\(tab.id)"
                            ),
                        ]),
                        "panes": .array(panes),
                    ])
                }
                return .object([
                    "id": .string(workspace.id),
                    "name": .string(workspace.name),
                    "desired_revision": .number(Double(workspace.revision)),
                    "applied_revision": .number(Double(workspace.revision)),
                    "runtime_health": .string("healthy"),
                    "pending_impact": .string("none"),
                    "selected_tab_id": .string(workspace.selectedTabID),
                    "tabs": .array(tabs),
                ])
            }
            return .object([
                "schema_version": .number(1),
                "revision": .number(Double(catalog.revision)),
                "selected_workspace_id": .string(catalog.selectedWorkspaceID),
                "workspaces": .array(workspaces),
                "pending_operation": .bool(false),
            ])
        }

        func layoutView() -> NativeWorkspaceLayoutView? {
            guard let catalog else { return nil }
            let choices = catalog.workspaces.map { NativeWorkspaceChoiceView(workspaceID: $0.id, name: $0.name,
                selected: $0.id == workspaceID, archived: (catalog.archivedWorkspaceIDs ?? []).contains($0.id)) }
            guard let workspace else {
                return NativeWorkspaceLayoutView(workspaceID: "", workspaceName: "Open Workspace", managedTerminalFontSizePoints: 14,
                    selectedTabID: "", selectedPaneID: nil, workspaceChoices: choices, tabs: [])
            }
            let tabs = workspace.tabs.compactMap(nativeTabLayoutView)
            guard tabs.count == workspace.tabs.count else { return nil }
            return NativeWorkspaceLayoutView(
                workspaceID: workspace.id,
                workspaceName: workspace.name,
                managedTerminalFontSizePoints: Int(
                    workspace.managedTerminalFontSizePoints
                ),
                selectedTabID: workspace.selectedTabID,
                selectedPaneID: workspace.selectedPaneID,
                workspaceChoices: choices,
                tabs: tabs
            )
        }

        private func nativeTabLayoutView(
            _ tab: PolarStationNativeTab
        ) -> NativeTabLayoutView? {
            if tab.panes.isEmpty {
                return NativeTabLayoutView(
                    tabID: tab.id,
                    title: tab.title,
                    displayMode: tab.mode == .focus ? .focus : .monitor,
                    visualPaneIDs: [],
                    mainPaneID: nil,
                    panes: []
                )
            }
            guard let layout = try? PolarStationNativeLayoutFamily.layout(
                orderedPaneIDs: tab.panes.map(\.id),
                mode: tab.mode,
                mainPaneID: tab.mainPaneID
            ) else { return nil }
            let rowUnits = layout.columnPopulations.reduce(
                1,
                leastCommonMultiple
            )
            var columnOrigins: [Int] = []
            var nextColumn = 0
            for column in layout.columns {
                columnOrigins.append(nextColumn)
                nextColumn += column.width.numerator
            }
            let placements = Dictionary(
                uniqueKeysWithValues: layout.slots.map { slot in
                    let population = layout.columnPopulations[slot.column]
                    let rowSpan = rowUnits / population
                    return (
                        slot.surfaceID,
                        TabPanePlacement(
                            column: columnOrigins[slot.column],
                            row: slot.row * rowSpan,
                            columnSpan: slot.width.numerator,
                            rowSpan: rowSpan
                        )
                    )
                }
            )
            return NativeTabLayoutView(
                tabID: tab.id,
                title: tab.title,
                displayMode: tab.mode == .focus ? .focus : .monitor,
                visualPaneIDs: layout.visualPaneIDs,
                mainPaneID: tab.mainPaneID,
                panes: tab.panes.compactMap { pane in
                    guard let placement = placements[pane.id] else { return nil }
                    return NativePaneLayoutView(
                        paneID: pane.id,
                        executionTargetID: pane.executionTargetID,
                        tmuxSessionID: pane.tmuxSessionID,
                        content: pane.content,
                        placement: placement
                    )
                }
            )
        }

        private func leastCommonMultiple(_ left: Int, _ right: Int) -> Int {
            left / greatestCommonDivisor(left, right) * right
        }

        private func greatestCommonDivisor(_ first: Int, _ second: Int) -> Int {
            var left = first
            var right = second
            while right != 0 {
                (left, right) = (right, left % right)
            }
            return max(1, left)
        }

        func focusCycleTarget(
            _ direction: PolarTmuxFocusCycleDirection
        ) -> String? {
            guard let workspace,
                  let tab = workspace.tabs.first(where: {
                      $0.id == workspace.selectedTabID
                  }),
                  tab.mode == .focus,
                  tab.panes.count > 1,
                  let mainPaneID = tab.mainPaneID,
                  let index = tab.panes.firstIndex(where: { $0.id == mainPaneID })
            else { return nil }
            switch direction {
            case .previous:
                return tab.panes[index == 0 ? tab.panes.count - 1 : index - 1].id
            case .next:
                return tab.panes[(index + 1) % tab.panes.count].id
            }
        }

        func focusSidebarTarget(_ slot: Int) -> String? {
            guard slot >= 1,
                  let workspace,
                  let tab = workspace.tabs.first(where: {
                      $0.id == workspace.selectedTabID
                  }),
                  tab.mode == .focus,
                  slot < tab.panes.count
            else { return nil }
            return tab.panes[slot].id
        }

        func numberedPaneTarget(_ number: Int) -> String? {
            guard let workspace else { return nil }
            let selectedTabID = tabGroupCoordinator.selectedManagedTabID()
                ?? workspace.selectedTabID
            guard
                  let tab = workspace.tabs.first(where: { $0.id == selectedTabID })
            else { return nil }
            return tab.paneID(number: number)
        }

        func setNumberBadgesVisible(_ visible: Bool) {
            numberBadgesVisible = visible
            guard let workspace else { return }
            let selectedTabID = tabGroupCoordinator.selectedManagedTabID()
                ?? workspace.selectedTabID
            for tab in workspace.tabs {
                for (index, pane) in tab.panes.enumerated() {
                    guard let surface = surfaceCoordinator.surface(for: pane.id) else {
                        continue
                    }
                    let shouldShow = visible && tab.id == selectedTabID
                    let existing = surface.subviews.compactMap {
                        $0 as? PolarStationPaneNumberBadge
                    }.first
                    let badge: PolarStationPaneNumberBadge
                    if let existing {
                        badge = existing
                    } else if shouldShow {
                        badge = PolarStationPaneNumberBadge(frame: .zero)
                        badge.translatesAutoresizingMaskIntoConstraints = false
                        surface.addSubview(badge)
                        NSLayoutConstraint.activate([
                            badge.centerXAnchor.constraint(equalTo: surface.centerXAnchor),
                            badge.centerYAnchor.constraint(equalTo: surface.centerYAnchor),
                            badge.widthAnchor.constraint(equalTo: surface.widthAnchor),
                            badge.heightAnchor.constraint(equalTo: surface.heightAnchor),
                        ])
                    } else {
                        continue
                    }
                    badge.number = index + 1
                    badge.isHidden = !shouldShow
                }
            }
        }

        func refreshNumberBadges() {
            if numberBadgesVisible { setNumberBadgesVisible(true) }
        }

        @discardableResult
        func reconcileInputSelection(_ installed: PolarStationNativeWorkspace? = nil) {
            guard let workspace = installed ?? workspace, let paneID = workspace.terminalFocusPaneID,
                  let window = tabGroupCoordinator.controller(for: workspace.selectedTabID)?.window
            else { return }
            surfaceCoordinator.selectRestoredInput(paneID: paneID, window: window)
        }

        private func applyPhysical(
            _ workspace: PolarStationNativeWorkspace,
            retireAbsent: Bool = true,
            focusIfActive: Bool,
            preservePendingFocus: Bool = false
        ) -> Bool {
            guard initialControllerIsAttached else {
                pendingWorkspace = workspace
                return false
            }

            // Freeze the desired selection before AppKit creates or reorders any
            // backing windows. Synchronous didBecomeKey notifications caused by
            // our own physical mutation must not become reverse user intents.
            tabGroupCoordinator.requestSelection(
                tabID: workspace.selectedTabID,
                paneID: workspace.terminalFocusPaneID,
                focusIfActive: focusIfActive,
                preservePendingFocus: preservePendingFocus
            )
            var installedAll = true
            let controllers = tabGroupCoordinator.reconcileTabs(
                workspace.tabs,
                retireAbsent: retireAbsent
            ) { [unowned self] tab, controller, hasInstalledController in
                if !hasInstalledController, tab.panes.isEmpty {
                    surfaceCoordinator.captureInitialPlaceholderIfNeeded(
                        tabID: tab.id,
                        controller: controller
                    )
                }
                let structure = InstalledTabStructure(tab: tab)
                let expectedSurfaceCount = max(tab.panes.count, 1)
                let physicalTreeIntact = controller.surfaceTree.count == expectedSurfaceCount
                    && tab.panes.allSatisfy { pane in
                    guard let surface = surfaceCoordinator.surface(for: pane.id) else {
                        return false
                    }
                    return controller.surfaceTree.contains { $0 === surface }
                }
                if !hasInstalledController
                    || installedTabStructures[tab.id] != structure
                    || !physicalTreeIntact
                {
                    let installed = install(
                        tab: tab,
                        in: controller,
                        fontSize: workspace.managedTerminalFontSizePoints
                    )
                    if installed {
                        installedTabStructures[tab.id] = structure
                    } else {
                        installedAll = false
                    }
                } else {
                    installedTabStructures[tab.id] = structure
                }
            }

            if retireAbsent {
                let liveTabIDs = Set(workspace.tabs.map(\.id))
                surfaceCoordinator.prunePlaceholders(retaining: liveTabIDs)
                installedTabStructures = installedTabStructures.filter {
                    liveTabIDs.contains($0.key)
                }
            }
            applyFontSize(workspace.managedTerminalFontSizePoints)
            reconcileSurfaceGeometry()
            refreshNumberBadges()
            reconcileInputSelection(workspace)
            return installedAll && controllers.count == workspace.tabs.count
        }

        private func updateTabMetadata(
            _ workspace: PolarStationNativeWorkspace
        ) {
            tabGroupCoordinator.updateTabMetadata(workspace.tabs)
        }

        private func install(
            tab: PolarStationNativeTab,
            in controller: TerminalController,
            fontSize: UInt8
        ) -> Bool {
            guard !tab.panes.isEmpty else {
                let placeholder = surfaceCoordinator.placeholder(
                    for: tab.id,
                    controller: controller,
                    fontSize: fontSize
                )
                replaceManagedSurfaceTree(
                    .init(view: placeholder),
                    in: controller
                )
                return true
            }

            surfaceCoordinator.removePlaceholder(for: tab.id)
            let tabSurfaces = surfaceCoordinator.surfaces(
                for: tab.panes,
                controller: controller,
                fontSize: fontSize
            )

            do {
                let layout = try PolarStationNativeLayoutFamily.layout(
                    orderedPaneIDs: tab.visualPaneIDs,
                    mode: tab.mode,
                    mainPaneID: tab.mainPaneID
                )
                guard let tree = splitTree(layout: layout, surfaces: tabSurfaces)
                else { return false }
                replaceManagedSurfaceTree(
                    tree,
                    in: controller
                )
                return true
            } catch {
                NSLog("Polar Station could not build native layout: %@",
                      error.localizedDescription)
                return false
            }
        }

        private func replaceManagedSurfaceTree(
            _ tree: SplitTree<Ghostty.SurfaceView>,
            in controller: TerminalController
        ) {
            // The Tab coordinator alone delivers terminal focus after mounting.
            controller.replaceSurfaceTree(tree)
            controller.undoManager?.removeAllActions(withTarget: controller)
        }

        private func splitTree(
            layout: PolarStationNativeLayout,
            surfaces: [String: Ghostty.SurfaceView]
        ) -> SplitTree<Ghostty.SurfaceView>? {
            let columns = layout.columns.compactMap { column -> WeightedNode? in
                let rows = column.surfaceIDs.compactMap { id in
                    surfaces[id].map { WeightedNode(node: .leaf(view: $0), weight: 1) }
                }
                guard let node = rightAssociated(rows, direction: .vertical) else {
                    return nil
                }
                return WeightedNode(
                    node: node,
                    weight: Double(column.width.numerator)
                        / Double(column.width.denominator)
                )
            }
            guard let root = rightAssociated(columns, direction: .horizontal) else {
                return nil
            }
            return SplitTree(root: root, zoomed: nil)
        }

        private struct WeightedNode {
            let node: SplitTree<Ghostty.SurfaceView>.Node
            let weight: Double
        }

        private func rightAssociated(
            _ values: [WeightedNode],
            direction: SplitTree<Ghostty.SurfaceView>.Direction
        ) -> SplitTree<Ghostty.SurfaceView>.Node? {
            guard let first = values.first else { return nil }
            guard values.count > 1 else { return first.node }
            let remaining = Array(values.dropFirst())
            guard let right = rightAssociated(remaining, direction: direction) else {
                return first.node
            }
            let total = values.reduce(0) { $0 + $1.weight }
            return .split(.init(
                direction: direction,
                ratio: first.weight / total,
                left: first.node,
                right: right
            ))
        }

        private func applyFontSize(_ points: UInt8) {
            guard appliedFontSize != points else { return }
            appliedFontSize = points
            for tab in workspace?.tabs ?? [] {
                guard let controller = tabGroupCoordinator.controller(for: tab.id)
                else { continue }
                for pane in tab.panes {
                    guard let surface = surfaceCoordinator.surface(for: pane.id)
                    else { continue }
                    controller.performAction("set_font_size:\(points)", on: surface)
                }
            }
        }

        private func reconcileSurfaceGeometry() {
            for controller in tabGroupCoordinator.allControllers {
                controller.terminalViewContainer?
                    .reconcilePolarStationTerminalGeometry()
            }
            for surface in surfaceCoordinator.allActiveSurfaces {
                surface.polarStationRefreshSurfaceLayouts()
            }
        }
    }

    private enum NativeWorkspaceRelayCommand: Equatable {
        case getCatalog
        case getWorkspace
        case selectWorkspace(String)
        case selectTab(String)
        case setDisplayMode(String, PolarStationNativeDisplayMode)
        case selectFocusPane(String, String)
        case open
        case repair
        case focusPrevious
        case focusNext
        case focusSidebarSlot(Int)

        var isRead: Bool {
            self == .getCatalog || self == .getWorkspace
        }

        var isLifecycle: Bool {
            self == .open || self == .repair
        }
    }

    private struct NativeWorkspaceRelayRequest {
        let requestID: String
        let scope: PolarTmuxJSONValue
        let scopeWorkspaceID: String?
        let expectedCatalogRevision: UInt64?
        let expectedWorkspaceRevision: UInt64?
        let command: NativeWorkspaceRelayCommand

        init(_ value: PolarTmuxJSONValue) throws {
            guard case let .object(request) = value,
                  request["protocol_version"]?.uint64Value == 1,
                  request["api"]?.stringValue == "workspace_v1",
                  let requestID = request["request_id"]?.stringValue,
                  !requestID.isEmpty,
                  let scope = request["scope"],
                  case let .object(commandObject) = request["command"],
                  let type = commandObject["type"]?.stringValue
            else { throw NativeWorkspaceRelayError.invalidRequest }
            self.requestID = requestID
            self.scope = scope
            guard case let .object(scopeObject) = scope,
                  let scopeType = scopeObject["type"]?.stringValue
            else { throw NativeWorkspaceRelayError.invalidRequest }
            switch scopeType {
            case "catalog":
                scopeWorkspaceID = nil
            case "workspace":
                guard let workspaceID = scopeObject["workspace_id"]?.stringValue,
                      !workspaceID.isEmpty
                else { throw NativeWorkspaceRelayError.invalidRequest }
                scopeWorkspaceID = workspaceID
            default:
                throw NativeWorkspaceRelayError.invalidRequest
            }
            expectedCatalogRevision = request["expected_catalog_revision"]?
                .optionalUInt64Value
            expectedWorkspaceRevision = request["expected_workspace_revision"]?
                .optionalUInt64Value
            switch type {
            case "get_catalog": command = .getCatalog
            case "get_workspace": command = .getWorkspace
            case "select_workspace":
                guard let id = commandObject["workspace_id"]?.stringValue
                else { throw NativeWorkspaceRelayError.invalidRequest }
                command = .selectWorkspace(id)
            case "select_tab":
                guard let id = commandObject["tab_id"]?.stringValue
                else { throw NativeWorkspaceRelayError.invalidRequest }
                command = .selectTab(id)
            case "set_display_mode":
                guard let id = commandObject["tab_id"]?.stringValue,
                      let rawMode = commandObject["display_mode"]?.stringValue,
                      let mode = PolarStationNativeDisplayMode(rawValue: rawMode)
                else { throw NativeWorkspaceRelayError.invalidRequest }
                command = .setDisplayMode(id, mode)
            case "select_focus_pane":
                guard let tabID = commandObject["tab_id"]?.stringValue,
                      let paneID = commandObject["pane_id"]?.stringValue
                else { throw NativeWorkspaceRelayError.invalidRequest }
                command = .selectFocusPane(tabID, paneID)
            case "open": command = .open
            case "repair": command = .repair
            case "focus_previous": command = .focusPrevious
            case "focus_next": command = .focusNext
            case "focus_sidebar_slot":
                guard let slot = commandObject["slot"]?.uint64Value,
                      (1 ... 8).contains(slot)
                else { throw NativeWorkspaceRelayError.invalidRequest }
                command = .focusSidebarSlot(Int(slot))
            default:
                throw NativeWorkspaceRelayError.commandNotAllowed
            }
        }

        func accepts(
            catalogRevision: UInt64,
            workspaceRevision: UInt64
        ) -> Bool {
            if let expectedCatalogRevision,
               expectedCatalogRevision != catalogRevision
            {
                return false
            }
            if let expectedWorkspaceRevision,
               expectedWorkspaceRevision != workspaceRevision
            {
                return false
            }
            return true
        }

        func successResponse(
            projection: PolarTmuxJSONValue,
            revision: UInt64
        ) -> PolarTmuxJSONValue {
            let result: PolarTmuxJSONValue
            if command.isRead {
                result = .object([
                    "type": .string("companion_catalog"),
                    "projection": projection,
                ])
            } else if command.isLifecycle {
                result = .object([
                    "type": .string("companion_operation_completed"),
                    "operation_id": .string(requestID),
                    "projection": projection,
                    "warnings": .array([]),
                ])
            } else {
                result = .object([
                    "type": .string("companion_transition"),
                    "projection": projection,
                    "impact": .string("none"),
                    "automatic_apply_candidate": .bool(false),
                ])
            }
            return response(revision: revision, result: result)
        }

        func errorResponse(
            code: String,
            message: String,
            retryable: Bool,
            revision: UInt64
        ) -> PolarTmuxJSONValue {
            response(
                revision: revision,
                result: .object([
                    "type": .string("error"),
                    "error": .object([
                        "code": .string(code),
                        "message": .string(message),
                        "retryable": .bool(retryable),
                    ]),
                ])
            )
        }

        private func response(
            revision: UInt64,
            result: PolarTmuxJSONValue
        ) -> PolarTmuxJSONValue {
            .object([
                "protocol_version": .number(1),
                "api": .string("workspace_v1"),
                "request_id": .string(requestID),
                "scope": scope,
                "revision": .number(Double(revision)),
                "result": result,
            ])
        }

        static func invalidEnvelopeResponse(
            _ value: PolarTmuxJSONValue
        ) -> PolarTmuxJSONValue {
            let fields = value.objectValue ?? [:]
            let requestID = fields["request_id"]?.stringValue
                ?? "invalid-workspace-relay"
            let scope = fields["scope"]
                ?? .object(["type": .string("catalog")])
            return .object([
                "protocol_version": .number(1),
                "api": .string("workspace_v1"),
                "request_id": .string(requestID),
                "scope": scope,
                "revision": .number(0),
                "result": .object([
                    "type": .string("error"),
                    "error": .object([
                        "code": .string("workspace_relay_invalid"),
                        "message": .string("The relayed Workspace request is invalid"),
                        "retryable": .bool(false),
                    ]),
                ]),
            ])
        }
    }

    private enum NativeWorkspaceRelayError: Error {
        case invalidRequest
        case commandNotAllowed
        case workspaceNotFound
        case tabNotFound
        case paneNotFound

        var code: String {
            switch self {
            case .invalidRequest: "workspace_relay_invalid"
            case .commandNotAllowed: "command_not_allowed"
            case .workspaceNotFound: "workspace_not_found"
            case .tabNotFound: "tab_not_found"
            case .paneNotFound: "pane_not_found"
            }
        }

        var message: String {
            switch self {
            case .invalidRequest: "The relayed Workspace request is invalid"
            case .commandNotAllowed: "The Workspace command is not allowed for Companion"
            case .workspaceNotFound: "The native Workspace does not exist"
            case .tabNotFound: "The native Tab does not exist"
            case .paneNotFound: "The native Pane does not exist"
            }
        }
    }

    private extension PolarTmuxJSONValue {
        var objectValue: [String: PolarTmuxJSONValue]? {
            guard case let .object(value) = self else { return nil }
            return value
        }

        var stringValue: String? {
            guard case let .string(value) = self else { return nil }
            return value
        }

        var uint64Value: UInt64? {
            guard case let .number(value) = self, value.isFinite,
                  value.rounded(.towardZero) == value,
                  value >= 0
            else { return nil }
            return UInt64(exactly: value)
        }

        var optionalUInt64Value: UInt64? {
            if case .null = self { return nil }
            return uint64Value
        }
    }

    enum PolarStationNativeExecutorError: LocalizedError {
        case workspaceUnavailable
        case workspaceChanged
        case contentBindingRejected
        case contentRetirementRejected
        case presentationRestoreFailed
        case physicalRepairPending

        var errorDescription: String? {
            switch self {
            case .workspaceUnavailable:
                "Polar Station has not loaded the Workspace."
            case .workspaceChanged:
                "The Workspace changed while saving. Review its current state and retry."
            case .contentBindingRejected:
                "Polar Station could not switch Pane content."
            case .contentRetirementRejected:
                "Polar Station could not retire Pane content."
            case .presentationRestoreFailed:
                "Polar Station could not restore the complete saved Layout."
            case .physicalRepairPending:
                "Layout saved; terminal view is waiting for repair."
            }
        }
    }
#endif
