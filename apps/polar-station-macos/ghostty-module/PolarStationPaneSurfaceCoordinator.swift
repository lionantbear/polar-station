#if POLAR_STATION
import AppKit
import GhosttyKit
import PolarTmuxHostClient

/// Owns managed Pane SurfaceViews from creation through exact Ghostty close.
/// A Pane removed from the physical tree moves into a retirement lease; no
/// callback-facing object is released merely because the Pane host detached.
@MainActor
final class PolarStationPaneSurfaceCoordinator: NSObject {
    var backendSocket: (() -> URL?)?
    private var backendSuspended = false
    var displayCreated: ((String, StationPaneContentV1, PolarStationDisplaySurface, NSWindow?) -> Void)?
    var displayRetired: ((String) -> Void)?
    var displayInteraction: (() -> Void)?
    var displayActivated: ((String) -> Void)?
    var displaySelected: ((String) -> Void)?
    var setupSelected:((NSWindow,String)->Void)?
    var terminalSelected: ((NSWindow, String, String, String) -> Void)?
    var displayFailed: ((String, String) -> Void)?
    private struct DisplayPane {
        let content: StationPaneContentV1
        let display: PolarStationDisplaySurface
    }
    func focusInputSource(paneID:String, window:NSWindow) {
        if let surface=residentPanes[paneID]?.surface ?? setupPanes[paneID]?.view, surface.window === window { window.makeFirstResponder(surface) }
        else if let surface=displayPanes[paneID]?.display.view, surface.window === window { window.makeFirstResponder(surface) }
    }
    func terminalSelection(paneID: String, window: NSWindow) -> String? {
        guard let resident = residentPanes[paneID], resident.surface.window === window,
              let surface = resident.surface.surface else { return nil }
        var selected = ghostty_text_s()
        guard ghostty_surface_read_selection(surface, &selected) else { return nil }
        defer { ghostty_surface_free_text(surface, &selected) }
        guard selected.text_len > 0, selected.text_len <= 1024 * 1024 else { return nil }
        return String(cString: selected.text)
    }
    private var setupPanes:[String:PolarStationToolSetupSurface]=[:]
    private struct PendingSetup {
        let setup:ToolSetupLaunchV1
        let started:@MainActor (String?)->Void
        let exited:@MainActor (UInt32?)->Void
    }
    private var pendingSetup:PendingSetup?
    func stageSetup(_ setup:ToolSetupLaunchV1,started:@escaping @MainActor (String?)->Void,exited:@escaping @MainActor (UInt32?)->Void) throws {
        guard pendingSetup == nil else {throw PolarStationNativeExecutorError.workspaceUnavailable}
        pendingSetup=PendingSetup(setup:setup,started:started,exited:exited)
    }
    func failPendingSetup(_ message:String) {
        let pending=pendingSetup;pendingSetup=nil;pending?.started(message)
    }
    func confirmSetupRetirement(retaining:Set<String>) async -> Bool {
        for (id,setup) in setupPanes where !retaining.contains(id) {
            guard await setup.confirmClose() else {return false}
        }
        return true
    }
    private func retireSetup(_ id:String) {
        setupPanes.removeValue(forKey:id)?.retire()
    }
    private var displayPanes: [String: DisplayPane] = [:]
    private var displayGeneration: UInt64 = 1
    var surfaceLost: ((String) -> Void)?
    var requestContent: ((URL, PolarTmuxPaneContentRequest) async throws -> PolarTmuxPaneContentResponse)?
    var submitRecovery: ((URL, PolarTmuxPaneContentRequest) async throws -> Void)?
    private var recoveryOverlays: [String: PolarStationTerminalRecoveryOverlay] = [:]
    private var recoverySubscriptions: Set<String> = []
    private var inputMonitor: Any?
    private var focusedPaneID: String?
    private static var selectionSequence: UInt64 = 0
    private static let nativeInstanceID = "native-\(UUID().uuidString)"
    private var readyBindingGenerations: [String: UInt64] = [:]

    private struct ViewReadyWaiter {
        let paneID: String
        let binding: Binding
        let continuation: CheckedContinuation<Void, Error>
        let deadline: Task<Void, Never>
    }
    private var viewReadyWaiters: [UUID: ViewReadyWaiter] = [:]

    func waitForTerminalView(paneID: String, binding: Binding) async throws {
        guard let resident = residentPanes[paneID], resident.binding == binding else {
            throw NSError(domain:"PolarStation.SessionView",code:1,userInfo:[NSLocalizedDescriptionKey:"The target pane binding changed. Task not sent."])
        }
        if readyBindingGenerations[resident.channelID] == resident.contentRevision { return }
        let id = UUID()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                let deadline = Task { @MainActor [weak self] in
                    do { try await Task.sleep(for:.seconds(30)) } catch { return }
                    self?.finishViewWaiter(id, error:NSError(domain:"PolarStation.SessionView",code:2,userInfo:[NSLocalizedDescriptionKey:"The target terminal did not become ready. Task not sent."]))
                }
                viewReadyWaiters[id] = ViewReadyWaiter(paneID:paneID,binding:binding,continuation:continuation,deadline:deadline)
                if Task.isCancelled { finishViewWaiter(id,error:CancellationError()) }
            }
        } onCancel: {
            Task { @MainActor [weak self] in self?.finishViewWaiter(id,error:CancellationError()) }
        }
    }
    private func finishViewWaiter(_ id: UUID, error: Error?) {
        guard let waiter = viewReadyWaiters.removeValue(forKey:id) else { return }
        waiter.deadline.cancel()
        if let error { waiter.continuation.resume(throwing:error) } else { waiter.continuation.resume() }
    }
    private func completeViewWaiters(paneID: String) {
        guard let resident = residentPanes[paneID], readyBindingGenerations[resident.channelID] == resident.contentRevision else { return }
        for (id,waiter) in viewReadyWaiters where waiter.paneID == paneID {
            let error: Error? = waiter.binding == resident.binding ? nil : NSError(domain:"PolarStation.SessionView",code:1,userInfo:[NSLocalizedDescriptionKey:"The target pane changed. Task not sent."])
            finishViewWaiter(id,error:error)
        }
    }

    struct Binding: Equatable {
        let executionTargetID: String
        let tmuxSessionID: String
        var source: TerminalSourceV1? = nil
    }

    private struct ResidentPane {
        let surface: Ghostty.SurfaceView
        let ghostty: Ghostty.App
        var binding: Binding
        let channelID: String
        let channelGeneration: UInt64
        let controlSocketURL: URL
        var contentRevision: UInt64
        let backendPath: String
    }

    private struct RetirementLease {
        let id: String
        let paneID: String
        let resident: ResidentPane

        var surface: Ghostty.SurfaceView { resident.surface }
    }

    private var residentPanes: [String: ResidentPane] = [:]
    private var placeholders: [String: Ghostty.SurfaceView] = [:]
    private var backgroundPlaceholders: [String: Ghostty.SurfaceView] = [:]
    private var retirementLeases: [String: RetirementLease] = [:]
    private var failedRetirementLeaseIDs: Set<String> = []
    private var retirementState = NativeSurfaceRetirementState()
    private var retirementRetryInFlight = false
    private var chatPointer: (id: String, start: NSPoint, dragged: Bool)?

    override init() {
        super.init()
        NotificationCenter.default.addObserver(self, selector: #selector(recoveryChanged(_:)), name: .polarStationTerminalRecovery, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(recoveryTransportReady(_:)), name: Notification.Name("PolarStationRecoveryTransportReady"), object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(windowFocused(_:)), name: NSWindow.didBecomeKeyNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(surfaceFocused(_:)), name: .init("PolarStationTerminalFocused"), object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(surfacePointerDown(_:)), name: .init("PolarStationSurfacePointerDown"), object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(windowBlurred(_:)), name: NSWindow.didResignKeyNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(surfaceBlurred(_:)), name: .init("PolarStationTerminalBlurred"), object: nil)
        inputMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .leftMouseUp, .leftMouseDragged, .keyUp]) { [weak self] event in
            MainActor.assumeIsolated { self?.humanInteraction(event) }
            return event
        }
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(ghosttyDidCloseSurface(_:)),
            name: Ghostty.Notification.ghosttyCloseSurface,
            object: nil
        )
    }

    func reset() {
        for id in Array(setupPanes.keys) {retireSetup(id)}
        failPendingSetup("The originating workspace closed before setup started.")
        for id in Array(displayPanes.keys) { retireDisplay(id) }
        focusedPaneID = nil
        for id in residentPanes.keys { sendRecovery(paneID: id, command: .unobserveRecovery) }
        for overlay in recoveryOverlays.values { overlay.removeFromSuperview() }
        recoveryOverlays.removeAll()
        recoverySubscriptions.removeAll()
        readyBindingGenerations.removeAll()
        residentPanes.removeAll()
        placeholders.removeAll()
        backgroundPlaceholders.removeAll()
        retirementLeases.removeAll()
        failedRetirementLeaseIDs.removeAll()
        retirementState = NativeSurfaceRetirementState()
        retirementRetryInFlight = false
    }

    func binding(
        executionTargetID: String,
        tmuxSessionID: String,
        source: TerminalSourceV1? = nil
    ) -> Binding {
        Binding(
            executionTargetID: executionTargetID,
            tmuxSessionID: tmuxSessionID,
            source: source
        )
    }

    func setIdeaPresentation(scope: String, visiblePaneIDs: Set<String>) {
        for (id, pane) in displayPanes where !visiblePaneIDs.contains(id) {
            pane.display.setIdeaPresentation(scope: scope, visible: false)
        }
        for (id, pane) in displayPanes where visiblePaneIDs.contains(id) {
            pane.display.setIdeaPresentation(scope: scope, visible: true)
        }
    }

    func surface(for paneID: String) -> Ghostty.SurfaceView? {
        residentPanes[paneID]?.surface ?? setupPanes[paneID]?.view ?? displayPanes[paneID]?.display.view
    }

    var residentTerminalPaneIDs: Set<String> { Set(residentPanes.keys) }

    struct TerminalTransfer {
        let from: String
        let to: String
        let surface: Ghostty.SurfaceView
    }

    func terminalTransfers(to workspace: PolarStationNativeWorkspace) -> [TerminalTransfer] {
        guard let backendPath = backendSocket?()?.path, !backendSuspended else { return [] }
        let panes = workspace.tabs.flatMap(\.panes)
        let desiredIDs = Set(panes.map(\.id))
        let terminals = panes.compactMap { pane -> (id: String, binding: Binding)? in
            guard pane.content == nil, let target = pane.executionTargetID, let session = pane.tmuxSessionID
            else { return nil }
            return (pane.id, binding(executionTargetID: target, tmuxSessionID: session, source: pane.source))
        }
        return terminals.compactMap { terminal in
            guard residentPanes[terminal.id] == nil,
                  !terminals.contains(where: { $0.id != terminal.id && $0.binding == terminal.binding })
            else { return nil }
            let sources = residentPanes.filter { id, resident in
                !desiredIDs.contains(id) && resident.binding == terminal.binding && resident.backendPath == backendPath
            }
            guard sources.count == 1, let source = sources.first else { return nil }
            return .init(from: source.key, to: terminal.id, surface: source.value.surface)
        }
    }

    func commitTerminalTransfers(_ transfers: [TerminalTransfer]) {
        for transfer in transfers {
            guard let resident = residentPanes.removeValue(forKey: transfer.from) else { continue }
            residentPanes[transfer.to] = resident
            if let overlay = recoveryOverlays.removeValue(forKey: transfer.from) {
                let paneID = transfer.to
                overlay.action = { [weak self] command in self?.sendRecovery(paneID: paneID, command: command) }
                recoveryOverlays[paneID] = overlay
            }
            if focusedPaneID == transfer.from { focusedPaneID = transfer.to }
        }
    }

    @discardableResult
    func synchronizeFontSize(_ points: UInt8, in controller: TerminalController) -> Int {
        var failures = 0
        for view in controller.surfaceTree {
            guard let surface = view.surface,
                  ghostty_surface_synchronize_polar_font_size(surface, Float32(points))
            else { failures += 1; continue }
        }
        return failures
    }

    func surfaces(
        for panes: [PolarStationNativePane],
        controller: TerminalController,
        fontSize: UInt8
    ) -> [String: Ghostty.SurfaceView] {
        var result: [String: Ghostty.SurfaceView] = [:]
        for pane in panes {
            if let content=pane.content,content.kind == .toolSetup {
                if let existing=setupPanes[pane.id],existing.content == content {
                    result[pane.id]=existing.view;continue
                }
                if let pending=pendingSetup,pending.setup.id == content.setupID,
                   pending.setup.execution_target_id == content.executionTargetID {
                    pendingSetup=nil
                    retireDisplay(pane.id);retireSetup(pane.id)
                    do {
                        guard let app=controller.ghostty.app else {throw PolarStationNativeExecutorError.workspaceUnavailable}
                        let setup=try PolarStationToolSetupSurface(app:app,setup:pending.setup,fontSize:fontSize,exited:pending.exited)
                        setupPanes[pane.id]=setup;result[pane.id]=setup.view
                        DispatchQueue.main.async {[weak self,weak setup] in
                            guard let setup,self?.setupPanes[pane.id] === setup,setup.view.window != nil else {
                                pending.started("The setup terminal could not be attached to its workspace.");return
                            }
                            pending.started(nil)
                        }
                    } catch {pending.started(error.localizedDescription)}
                    continue
                }
                // No launch material after restore. Use the existing processless
                // display for an informational placeholder; never launch a shell.
            }
            if let content = pane.content {
                retireSetup(pane.id)
                if let existing = displayPanes[pane.id], existing.content == content {
                    result[pane.id] = existing.display.view
                    continue
                }
                retireDisplay(pane.id)
                if let leaseID = beginRetirement(paneID: pane.id) {
                    Task { [weak self] in await self?.retireLease(leaseID) }
                }
                do {
                    guard let app = controller.ghostty.app, displayGeneration < UInt64.max else {
                        throw PolarStationNativeExecutorError.workspaceUnavailable
                    }
                    let display = try PolarStationDisplaySurface(app: app,
                        fontSize: Float32(fontSize), generation: displayGeneration)
                    displayGeneration += 1
                    try display.bind(contentEpoch: 1)
                    if content.kind == .ideaTree { try display.mountIdeaTree(viewID: content.viewID) }
                    displayPanes[pane.id] = DisplayPane(content: content, display: display)
                    result[pane.id] = display.view
                    // Defer effects until the physical tree has installed the view.
                    DispatchQueue.main.async { [weak self, weak display] in
                        guard let self, let display,
                              self.displayPanes[pane.id]?.display === display else { return }
                        self.displayCreated?(pane.id, content, display, display.view.window ?? controller.window)
                    }
                } catch { displayFailed?(pane.id, error.localizedDescription) }
                continue
            }
            retireSetup(pane.id)
            retireDisplay(pane.id)
            guard !backendSuspended, backendSocket?() != nil else { continue }
            guard let target = pane.executionTargetID, let session = pane.tmuxSessionID else {
                displayFailed?(pane.id, "Invalid Pane content binding")
                continue
            }
            let expectedBinding = binding(executionTargetID: target, tmuxSessionID: session, source: pane.source)
            if let resident = residentPanes[pane.id],
               resident.binding == expectedBinding,
               resident.backendPath == backendSocket?()?.path {
                observeRecovery(paneID: pane.id, resident: resident)
                result[pane.id] = resident.surface
                continue
            }
            if let leaseID = beginRetirement(paneID: pane.id) {
                Task { [weak self] in await self?.retireLease(leaseID) }
            }
            let resident = makePaneSurface(
                binding: expectedBinding,
                controller: controller,
                fontSize: fontSize
            )
            residentPanes[pane.id] = resident
            observeRecovery(paneID: pane.id, resident: resident)
            result[pane.id] = resident.surface
        }
        return result
    }

    func captureInitialPlaceholderIfNeeded(
        tabID: String,
        controller: TerminalController
    ) {
        guard placeholders[tabID] == nil,
              let shell = controller.surfaceTree.first
        else { return }
        placeholders[tabID] = shell
    }

    func placeholder(
        for tabID: String,
        controller: TerminalController,
        fontSize: UInt8,
        processless: Bool = false
    ) -> Ghostty.SurfaceView {
        if processless {
            if let existing = backgroundPlaceholders[tabID] { return existing }
            guard let app = controller.ghostty.app else { preconditionFailure("Ghostty runtime is unavailable") }
            var configuration = Ghostty.SurfaceConfiguration()
            configuration.fontSize = Float32(fontSize)
            let view = Ghostty.SurfaceView(app, baseConfig: configuration, stationDisplayOnly: true)
            backgroundPlaceholders[tabID] = view
            placeholders[tabID] = nil
            return view
        }
        backgroundPlaceholders[tabID] = nil
        if let existing = placeholders[tabID] {
            return existing
        }
        let shell = makeShellSurface(controller, fontSize: fontSize)
        placeholders[tabID] = shell
        return shell
    }

    func removePlaceholder(for tabID: String) {
        placeholders[tabID] = nil
        backgroundPlaceholders[tabID] = nil
    }

    func placeholderSnapshot() -> [String: Ghostty.SurfaceView] {
        placeholders
    }

    func restorePlaceholders(
        _ snapshot: [String: Ghostty.SurfaceView]
    ) {
        placeholders = snapshot
    }

    func prunePlaceholders(retaining tabIDs: Set<String>) {
        placeholders = placeholders.filter { tabIDs.contains($0.key) }
        backgroundPlaceholders = backgroundPlaceholders.filter { tabIDs.contains($0.key) }
    }

    func rebindChangedPaneContent(
        from previous: PolarStationNativeWorkspace,
        to candidate: PolarStationNativeWorkspace
    ) async throws {
        let prior = Dictionary(uniqueKeysWithValues: previous.tabs
            .flatMap(\.panes).map { ($0.id, $0) })
        for pane in candidate.tabs.flatMap(\.panes) {
            guard pane.content == nil, let old = prior[pane.id], old.content == nil,
                  old.executionTargetID != pane.executionTargetID
                    || old.tmuxSessionID != pane.tmuxSessionID || old.source != pane.source
            else { continue }
            try await rebindPaneContent(paneID: pane.id, to: pane)
        }
    }

    func restorePaneContent(
        to workspace: PolarStationNativeWorkspace
    ) async throws {
        for pane in workspace.tabs.flatMap(\.panes) {
            guard pane.content == nil, let target = pane.executionTargetID, let session = pane.tmuxSessionID,
                  let resident = residentPanes[pane.id],
                  resident.binding != binding(executionTargetID: target, tmuxSessionID: session, source: pane.source)
            else { continue }
            try await rebindPaneContent(paneID: pane.id, to: pane)
        }
    }

    private func requestPaneContent(socketURL: URL, request: PolarTmuxPaneContentRequest) async throws -> PolarTmuxPaneContentResponse {
        guard let requestContent else { throw PolarTmuxLocalIPCError.connectionClosed }
        return try await requestContent(socketURL, request)
    }

    func reconnectSavedPaneContent(
        to workspace: PolarStationNativeWorkspace
    ) async throws {
        for pane in workspace.tabs.flatMap(\.panes) {
            if pane.content != nil { continue }
            guard let target = pane.executionTargetID, let session = pane.tmuxSessionID,
                  var resident = residentPanes[pane.id] else {
                throw PolarStationNativeExecutorError.workspaceUnavailable
            }
            let nextRevision = resident.contentRevision + 1
            let response = try await requestPaneContent(socketURL: resident.controlSocketURL, request: .reconnect(
                requestID: "mac-restore-\(UUID().uuidString.lowercased())",
                channelID: resident.channelID,
                channelGeneration: resident.channelGeneration,
                contentRevision: nextRevision,
                executionTarget: target,
                tmuxSessionID: session,
                source: pane.source
            ))
            guard response.result.bindingAccepted else {
                throw PolarStationNativeExecutorError.contentBindingRejected
            }
            resident.binding = binding(
                executionTargetID: target,
                tmuxSessionID: session,
                source: pane.source
            )
            resident.contentRevision = nextRevision
            residentPanes[pane.id] = resident
        }
    }

    func retireAbsentPaneContent(
        from previous: PolarStationNativeWorkspace,
        to current: PolarStationNativeWorkspace
    ) async {
        let retainedPaneIDs = Set(current.tabs.flatMap(\.panes).map(\.id))
        for pane in previous.tabs.flatMap(\.panes)
        where !retainedPaneIDs.contains(pane.id) {
            for (id,waiter) in viewReadyWaiters where waiter.paneID == pane.id {
                finishViewWaiter(id,error:CancellationError())
            }
            retireSetup(pane.id)
            retireDisplay(pane.id)
            guard let leaseID = beginRetirement(paneID: pane.id) else {
                continue
            }
            await retireLease(leaseID)
        }
    }

    func suspendBackend() async {
        backendSuspended = true
        for id in Array(setupPanes.keys) { retireSetup(id) }
        for id in Array(displayPanes.keys) { retireDisplay(id) }
        for id in Array(residentPanes.keys) {
            if let lease = beginRetirement(paneID:id) { await retireLease(lease) }
        }
    }
    func resumeBackend() { backendSuspended = false }

    func retryFailedRetirements() async {
        guard !retirementRetryInFlight,
              !failedRetirementLeaseIDs.isEmpty
        else { return }
        retirementRetryInFlight = true
        defer { retirementRetryInFlight = false }
        let leaseIDs = failedRetirementLeaseIDs.sorted()
        failedRetirementLeaseIDs.removeAll()
        for leaseID in leaseIDs {
            guard retirementState.retryContentRetirement(
                leaseID: leaseID
            ) else { continue }
            await retireLease(leaseID)
        }
    }

    var allActiveSurfaces: [Ghostty.SurfaceView] {
        residentPanes.values.map(\.surface) + setupPanes.values.map(\.view) + displayPanes.values.map { $0.display.view }
    }

    private func retireDisplay(_ paneID: String) {
        guard let pane = displayPanes.removeValue(forKey: paneID) else { return }
        displayRetired?(paneID)
        try? pane.display.close()
    }

    private func beginRetirement(paneID: String) -> String? {
        sendRecovery(paneID: paneID, command: .unobserveRecovery)
        recoveryOverlays.removeValue(forKey: paneID)?.removeFromSuperview()
        if let resident = residentPanes[paneID] { recoverySubscriptions.remove(resident.channelID) }
        guard let resident = residentPanes.removeValue(forKey: paneID) else {
            return nil
        }
        let leaseID = UUID().uuidString.lowercased()
        let lease = RetirementLease(
            id: leaseID,
            paneID: paneID,
            resident: resident
        )
        retirementLeases[leaseID] = lease
        retirementState.register(
            leaseID: leaseID,
            paneID: paneID,
            surfaceToken: token(for: resident.surface)
        )
        retirementState.detachFromTree(leaseID: leaseID)
        retirementState.beginContentRetirement(leaseID: leaseID)
        return leaseID
    }

    private func retireLease(_ leaseID: String) async {
        guard let lease = retirementLeases[leaseID] else { return }
        do {
            let response = try await requestPaneContent(socketURL: lease.resident.controlSocketURL, request: .retire(
                requestID: "mac-retire-\(UUID().uuidString.lowercased())",
                channelID: lease.resident.channelID,
                channelGeneration: lease.resident.channelGeneration,
                contentRevision: lease.resident.contentRevision + 1
            ))
            guard case .detached = response.result else {
                throw PolarStationNativeExecutorError
                    .contentRetirementRejected
            }
            failedRetirementLeaseIDs.remove(leaseID)
            if retirementState.markContentDetached(leaseID: leaseID) {
                scheduleRelease(leaseID: leaseID)
            } else {
                requestGhosttyClose(leaseID: leaseID)
            }
        } catch {
            retirementState.markRetirementFailed(leaseID: leaseID)
            failedRetirementLeaseIDs.insert(leaseID)
            NSLog(
                "Polar Station retained failed Pane retirement %@: %@",
                lease.paneID,
                error.localizedDescription
            )
        }
    }

    private func requestGhosttyClose(leaseID: String) {
        guard let lease = retirementLeases[leaseID],
              let surface = lease.surface.surface
        else {
            NSLog("Polar Station retained a Pane Surface without a close handle")
            return
        }
        lease.resident.ghostty.requestClose(surface: surface)
    }

    @objc private func ghosttyDidCloseSurface(_ notification: Notification) {
        guard let surface = notification.object as? Ghostty.SurfaceView else {
            return
        }
        if let paneID = residentPanes.first(where: { $0.value.surface === surface })?.key {
            residentPanes.removeValue(forKey: paneID)
            surfaceLost?(paneID)
        }
        let exactLeaseIDs = retirementLeases.compactMap { leaseID, candidate in
            candidate.surface === surface ? leaseID : nil
        }
        guard !exactLeaseIDs.isEmpty else { return }
        let processAlive = notification.userInfo?["process_alive"] as? Bool
            ?? true
        let releasable = Set(retirementState.observeClose(
            surfaceToken: token(for: surface),
            processAlive: processAlive
        ))
        for leaseID in exactLeaseIDs where releasable.contains(leaseID) {
            scheduleRelease(leaseID: leaseID)
        }
    }

    private func scheduleRelease(leaseID: String) {
        DispatchQueue.main.async { [weak self] in
            guard let self,
                  self.retirementState.release(leaseID: leaseID)
            else { return }
            self.failedRetirementLeaseIDs.remove(leaseID)
            self.retirementLeases.removeValue(forKey: leaseID)
        }
    }

    private func rebindPaneContent(
        paneID: String,
        to pane: PolarStationNativePane
    ) async throws {
        guard pane.content == nil, let target = pane.executionTargetID, let session = pane.tmuxSessionID,
              var resident = residentPanes[paneID] else {
            throw PolarStationNativeExecutorError.workspaceUnavailable
        }
        let nextRevision = resident.contentRevision + 1
        let response = try await requestPaneContent(socketURL: resident.controlSocketURL, request: .bind(
            requestID: "mac-rebind-\(UUID().uuidString.lowercased())",
            channelID: resident.channelID,
            channelGeneration: resident.channelGeneration,
            contentRevision: nextRevision,
            executionTarget: target,
            tmuxSessionID: session,
            source: pane.source
        ))
        guard response.result.bindingAccepted else {
            throw PolarStationNativeExecutorError.contentBindingRejected
        }
        resident.binding = binding(
            executionTargetID: target,
            tmuxSessionID: session,
            source: pane.source
        )
        resident.contentRevision = nextRevision
        residentPanes[paneID] = resident
    }

    private func makePaneSurface(
        binding initialBinding: Binding,
        controller: TerminalController,
        fontSize: UInt8
    ) -> ResidentPane {
        let identifier = UUID().uuidString.lowercased()
        let channelID = "macos-surface-\(identifier)"
        let terminalConfig = terminalConfiguration(initialBinding, channelID: channelID)
        var configuration = Ghostty.SurfaceConfiguration()
        configuration.fontSize = Float32(fontSize)
        let surface = makeSurface(controller, configuration: configuration, terminalConfig: terminalConfig)
        if let handle = surface.surface { ghostty_surface_set_polar_native_control(handle, true) }
        return ResidentPane(
            surface: surface,
            ghostty: controller.ghostty,
            binding: initialBinding,
            channelID: channelID,
            channelGeneration: 1,
            controlSocketURL: URL(string: "station-terminal:")!,
            contentRevision: 1,
            backendPath: backendSocket?()?.path ?? "/unavailable-station-backend.sock"
        )
    }

    private func observeRecovery(paneID: String, resident: ResidentPane) {
        if recoveryOverlays[paneID] == nil {
            let overlay = PolarStationTerminalRecoveryOverlay()
            if let initial = PolarStationTerminalRecoveryView.unavailable() { overlay.render(initial) }
            overlay.action = { [weak self] command in self?.sendRecovery(paneID: paneID, command: command) }
            resident.surface.addSubview(overlay)
            overlay.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                overlay.topAnchor.constraint(equalTo: resident.surface.topAnchor, constant: 8),
                overlay.trailingAnchor.constraint(equalTo: resident.surface.trailingAnchor, constant: -8),
                overlay.leadingAnchor.constraint(greaterThanOrEqualTo: resident.surface.leadingAnchor, constant: 8),
                overlay.widthAnchor.constraint(lessThanOrEqualToConstant: 360),
            ])
            recoveryOverlays[paneID] = overlay
        }
        guard !recoverySubscriptions.contains(resident.channelID) else { return }
        recoverySubscriptions.insert(resident.channelID)
        sendRecovery(paneID: paneID, command: .observeRecovery)
    }

    private func sendRecovery(paneID: String, command: PolarTmuxPaneContentCommand) {
        guard let resident = residentPanes[paneID], let submitRecovery else { return }
        let request = PolarTmuxPaneContentRequest(requestID: "recovery-\(UUID().uuidString)", channelID: resident.channelID, channelGeneration: resident.channelGeneration, contentRevision: resident.contentRevision, command: command)
        if case .terminalControl(_, let focused, let sequence, _) = command {
            StationOperationTrace.record(focused ? 52 : 53, identity: resident.channelID, generation: sequence)
        }
        Task { [weak self] in
            do { try await submitRecovery(resident.controlSocketURL, request) }
            catch { self?.recoverySubscriptions.remove(resident.channelID) }
        }
    }

    @objc private func recoveryTransportReady(_ notification: Notification) {
        recoverySubscriptions.removeAll()
        for (id, resident) in residentPanes { observeRecovery(paneID: id, resident: resident) }
    }

    @objc private func recoveryChanged(_ notification: Notification) {
        guard let response = notification.object as? PolarTmuxPaneContentResponse,
              case .recovery(let view) = response.result,
              let entry = residentPanes.first(where: { $0.value.channelID == response.channelID && $0.value.channelGeneration == response.channelGeneration }) else { return }
        guard response.contentRevision >= entry.value.contentRevision else { return }
        defer {
            if entry.value.binding.source != nil, focusedPaneID == entry.key, let window = entry.value.surface.window {
                notifyTerminalSelection(window:window,paneID:entry.key,resident:entry.value)
            }
        }
        recoveryOverlays[entry.key]?.render(view)
        if view.phase != "hidden" {
            readyBindingGenerations.removeValue(forKey: response.channelID)
        } else if readyBindingGenerations[response.channelID] != response.contentRevision {
            readyBindingGenerations[response.channelID] = response.contentRevision
            completeViewWaiters(paneID:entry.key)
            let surface = entry.value.surface
            if surface.window?.isKeyWindow == true && surface.window?.firstResponder === surface {
                focusPane(entry.key, clicked: true)
            }
        }
    }

    private func notifyTerminalSelection(window:NSWindow, paneID:String, resident:ResidentPane) {
        if backendSuspended || (resident.binding.source != nil && readyBindingGenerations[resident.channelID] != resident.contentRevision) {
            setupSelected?(window,paneID)
        } else {
            terminalSelected?(window,paneID,resident.binding.executionTargetID,resident.binding.tmuxSessionID)
        }
    }

    private func focusPane(_ paneID: String?, clicked: Bool = false) {
        if focusedPaneID == paneID && !clicked { return }
        if let old = focusedPaneID, old != paneID {
            Self.selectionSequence += 1
            sendRecovery(paneID: old, command: .terminalControl(nativeInstanceID: Self.nativeInstanceID, focused: false, sequence: Self.selectionSequence, cause: .blur))
        }
        focusedPaneID = paneID
        if let paneID {
            Self.selectionSequence += 1
            sendRecovery(paneID: paneID, command: .terminalControl(nativeInstanceID: Self.nativeInstanceID, focused: true, sequence: Self.selectionSequence, cause: .selection))
        }
    }

    func selectRestoredInput(paneID: String, window: NSWindow) {
        if setupPanes[paneID] != nil || displayPanes[paneID]?.content.kind == .toolSetup {
            setupSelected?(window,paneID)
        } else if displayPanes[paneID] != nil {
            displaySelected?(paneID)
        } else if let resident = residentPanes[paneID] {
            notifyTerminalSelection(window:window, paneID:paneID, resident:resident)
        }
    }

    @objc private func surfaceFocused(_ notification: Notification) {
        if let surface=notification.object as? Ghostty.SurfaceView,let window=surface.window,window.isKeyWindow,
           let entry=setupPanes.first(where:{$0.value.view === surface}) {
            focusPane(nil);setupSelected?(window,entry.key);return
        }
        if let surface=notification.object as? Ghostty.SurfaceView,let window=surface.window,window.isKeyWindow,
           let entry=displayPanes.first(where:{$0.value.display.view === surface && $0.value.content.kind == .toolSetup}) {
            focusPane(nil);setupSelected?(window,entry.key);return
        }
        if let surface = notification.object as? Ghostty.SurfaceView,
           surface.window?.isKeyWindow == true,
           let entry = displayPanes.first(where: { $0.value.display.view === surface }) {
            focusPane(nil)
            // Idea graphics owns its mouse events and native focus; it never
            // enters the transcript's deferred pointer-release selection path.
            if entry.value.content.kind != .lgpText || NSEvent.pressedMouseButtons & 1 == 0 {
                displaySelected?(entry.key)
            }
            return
        }
        guard let surface = notification.object as? Ghostty.SurfaceView,
              surface.window?.isKeyWindow == true,
              let entry = residentPanes.first(where: { $0.value.surface === surface }) else { return }
        observeRecovery(paneID: entry.key, resident: entry.value)
        focusPane(entry.key)
        if let window = surface.window { notifyTerminalSelection(window:window, paneID:entry.key, resident:entry.value) }
    }

    @objc private func windowFocused(_ notification: Notification) {
        guard let window = notification.object as? NSWindow else { return }
        for (id, pane) in displayPanes where pane.display.view.window === window {
            let nativeFocused = pane.content.kind != .lgpText &&
                (window.firstResponder as? NSView)?.isDescendant(of: pane.display.view) == true
            guard window.firstResponder === pane.display.view || nativeFocused else { continue }
            if pane.content.kind != .lgpText || NSEvent.pressedMouseButtons & 1 == 0 { displaySelected?(id) }
        }
        for (id, resident) in residentPanes where resident.surface.window === window && window.firstResponder === resident.surface {
            observeRecovery(paneID: id, resident: resident)
            focusPane(id)
            notifyTerminalSelection(window:window, paneID:id, resident:resident)
        }
    }

    @objc private func surfaceBlurred(_ notification: Notification) {
        guard let surface = notification.object as? Ghostty.SurfaceView, let id = focusedPaneID,
              residentPanes[id]?.surface === surface else { return }
        focusPane(nil)
    }

    @objc private func windowBlurred(_ notification: Notification) {
        if let window = notification.object as? NSWindow {
            for (id, pane) in displayPanes where pane.display.view.window === window {
                pane.display.cancelPointer()
                if chatPointer?.id == id { chatPointer = nil }
            }
        }
        guard let window = notification.object as? NSWindow, let id = focusedPaneID,
              residentPanes[id]?.surface.window === window else { return }
        focusPane(nil)
    }

    @objc private func surfacePointerDown(_ notification: Notification) {
        guard let surface = notification.object as? Ghostty.SurfaceView,
              let event = notification.userInfo?["event"] as? NSEvent,
              event.type == .leftMouseDown,
              let entry = displayPanes.first(where: { $0.value.display.view === surface }) else { return }
        // The Surface owns hit testing and may consume this focus-transfer
        // click. Track it before that happens; only an undragged release below
        // selects the conversation, preserving transcript selection gestures.
        displaySelected?(entry.key)
        entry.value.display.beginPointer()
        chatPointer = (entry.key, event.locationInWindow, false)
        focusPane(nil)
    }

    private func humanInteraction(_ event: NSEvent) {
        if event.type == .leftMouseDown, ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil {
            let content = event.window?.contentView
            let hit = content.map { $0.hitTest($0.convert(event.locationInWindow, from: nil)) }
            NSLog("Station input click window=%d key=%d point=%@ hit=%@ responder=%@", event.window?.windowNumber ?? -1, event.window?.isKeyWindow == true ? 1 : 0, NSStringFromPoint(event.locationInWindow), String(describing: hit), String(describing: event.window?.firstResponder))
        }
        guard event.window?.isKeyWindow == true else { return }
        if event.type == .leftMouseUp || event.type == .keyUp {
            DispatchQueue.main.async { [weak self] in self?.displayInteraction?() }
        }
        if event.type == .keyUp { return }
        if event.type == .leftMouseDragged {
            if let pointer = chatPointer {
                chatPointer = (pointer.id, pointer.start, true)
                displayPanes[pointer.id]?.display.cancelPointer()
            }
            return
        }
        if event.type == .leftMouseUp {
            let pointer = chatPointer; chatPointer = nil
            if let pointer, let display = displayPanes[pointer.id]?.display {
                let single = !pointer.dragged && event.clickCount == 1 &&
                    abs(event.locationInWindow.x-pointer.start.x) <= 4 && abs(event.locationInWindow.y-pointer.start.y) <= 4
                _ = display.endPointer(at: event.locationInWindow, single: single)
            }
            if let pointer, !pointer.dragged, event.clickCount == 1,
               abs(event.locationInWindow.x-pointer.start.x) <= 4,
               abs(event.locationInWindow.y-pointer.start.y) <= 4,
               let surface = displayPanes[pointer.id]?.display.view,
               surface.window === event.window {
                DispatchQueue.main.async { [weak self, weak surface] in
                    guard let self, let surface,
                          self.displayPanes[pointer.id]?.display.view === surface,
                          surface.window?.isKeyWindow == true,
                          surface.window?.firstResponder === surface else { return }
                    self.displaySelected?(pointer.id)
                    self.displayActivated?(pointer.id)
                }
            }
            return
        }
        if let window = event.window, let root = window.contentView,
           let hit = root.hitTest(root.convert(event.locationInWindow, from: nil)) {
            for (id, pane) in displayPanes where pane.display.view.window === window {
                guard pane.content.kind == .localDiagnostics || pane.content.kind == .remoteHosts || pane.content.kind == .settingsText || pane.content.kind == .settingsPane else { continue }
                guard hit === pane.display.view || hit.isDescendant(of: pane.display.view) else { continue }
                focusPane(nil)
                displaySelected?(id)
                return
            }
        }
        for (id, resident) in residentPanes where resident.surface.window === event.window {
            guard resident.surface.bounds.contains(resident.surface.convert(event.locationInWindow, from: nil)) else { continue }
            StationOperationTrace.record(70, identity: resident.channelID, generation: Self.selectionSequence + 1)
            observeRecovery(paneID: id, resident: resident)
            focusPane(id, clicked: true)
            break
        }
    }

    private func makeShellSurface(
        _ controller: TerminalController,
        fontSize: UInt8
    ) -> Ghostty.SurfaceView {
        var configuration = Ghostty.SurfaceConfiguration()
        configuration.fontSize = Float32(fontSize)
        return makeSurface(controller, configuration: configuration)
    }

    private func makeSurface(
        _ controller: TerminalController,
        configuration: Ghostty.SurfaceConfiguration,
        terminalConfig: String? = nil
    ) -> Ghostty.SurfaceView {
        guard let app = controller.ghostty.app else {
            preconditionFailure("Ghostty runtime is unavailable")
        }
        return Ghostty.SurfaceView(app, baseConfig: configuration, stationTerminalConfig: terminalConfig)
    }

    private func terminalConfiguration(_ binding: Binding, channelID: String) -> String {
        struct Config: Encodable {
            struct Channel: Encodable {
                let id: String
                let generation: UInt64 = 1
            }
            let binding: PolarTmuxPaneContentBinding
            let size = ["columns": 80, "rows": 24]
            let channel: Channel
        }
        let content = PolarTmuxPaneContentBinding(
            executionTarget: binding.executionTargetID,
            tmuxSessionID: binding.tmuxSessionID,
            source: binding.source
        )
        do {
            return String(decoding: try JSONEncoder().encode(Config(binding: content, channel: .init(id: channelID))), as: UTF8.self)
        } catch {
            preconditionFailure("Invalid direct Station terminal binding")
        }
    }

    private func token(for object: AnyObject) -> String {
        String(describing: ObjectIdentifier(object))
    }
}

@MainActor
private final class PolarStationTerminalRecoveryOverlay: NSVisualEffectView {
    var action: ((PolarTmuxPaneContentCommand) -> Void)?
    private let spinner = NSProgressIndicator()
    private let message = NSTextField(wrappingLabelWithString: "")
    private let detail = NSTextField(wrappingLabelWithString: "")
    private lazy var retry = NSButton(title: "", target: self, action: #selector(retryClicked))
    private lazy var details = NSButton(title: "", target: self, action: #selector(detailsClicked))
    private var expanded = false
    private(set) var retryAllowed = false

    init() {
        super.init(frame: .zero)
        material = .popover
        state = .active
        wantsLayer = true
        layer?.cornerRadius = 8
        spinner.style = .spinning
        spinner.controlSize = .small
        let controls = NSStackView(views: [retry, details])
        controls.spacing = 8
        let stack = NSStackView(views: [NSStackView(views: [spinner, message]), controls, detail])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        addSubview(stack)
        stack.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor, constant: 10), stack.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -10),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10), stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
        ])
        detail.isHidden = true
        message.setAccessibilityRole(.staticText)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    func render(_ view: PolarStationTerminalRecoveryView) {
        isHidden = !view.visible || view.phase == "hidden"
        retryAllowed = view.retryEnabled
        message.stringValue = view.message
        retry.title = view.retryLabel
        details.title = view.detailsLabel
        let busy = view.phase == "probing" || view.phase == "attaching"
        spinner.isHidden = !busy
        if busy { spinner.startAnimation(nil) } else { spinner.stopAnimation(nil) }
        message.textColor = view.phase == "recovered" ? .systemGreen
            : view.phase == "ended" ? .secondaryLabelColor : .systemOrange
        retry.isHidden = ["hidden", "ended", "recovered"].contains(view.phase)
        retry.isEnabled = view.retryEnabled
        let formatter = DateFormatter()
        formatter.timeZone = TimeZone(secondsFromGMT: 0)
        formatter.dateFormat = "HH:mm:ss"
        let date = view.lastProbeUnixMs.map { formatter.string(from: Date(timeIntervalSince1970: Double($0) / 1000)) } ?? "—"
        detail.stringValue = "\(view.detail)\n\(view.probeTimeLabel)：\(date)"
        detail.isHidden = !expanded
    }
    @objc private func retryClicked() { action?(.probeRecovery) }
    @objc private func detailsClicked() { expanded.toggle(); detail.isHidden = !expanded }
}
#endif
