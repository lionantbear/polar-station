import AppKit
import CPolarStationDesktopClient
import Combine
import Foundation
import PolarTmuxHostClient
import PolarTmuxMacUI
import UserNotifications

@MainActor
struct SessionCreationContext {
    let validate: (Int) -> String?
    let complete: ([SessionRowProjectionV1.Identity]) -> Void
    let release: () -> Void
    var contains: (SessionRowProjectionV1.Identity) -> Bool = { _ in false }
}

/// Thin application state for the native Control Center renderer.
///
/// Production always loads and mutates through the authenticated local Rust
/// Host.
@MainActor
final class ControlCenterStore: ObservableObject {
    @Published var sessionCreationRecovery: AgentSessionResumeV1?
    private var creationRequestIDs: [Data: String] = [:]
    func prepareSessionEditor() -> SessionCreationSubmit {
        let captured = prepareSessionCreation?()
        creationRequestIDs.removeAll()
        return { [weak self] intent, completion in
            var completed: [String] = []
            self?.submitHostIntent(intent.operation, completion: { error in completion(error, completed) },
                preparedCreation: captured, completedThreads: { completed.append(contentsOf: $0) })
        }
    }
    var prepareSessionCreation: (@MainActor () -> SessionCreationContext?)?
    typealias IntentSink =
        @MainActor (ControlCenterIntentV1) -> Void
    typealias ProjectionSink =
        @MainActor (ControlCenterProjectionV1) -> Void
    typealias NativeIntentSink = @MainActor (
        ControlCenterIntentOperationV1,
        (@MainActor (String?) -> Void)?
    ) -> Void
    typealias SessionFeedSink =
        @MainActor (PolarTmuxSessionFeedUpdate) -> Void
    typealias WorkspaceRelaySink =
        @MainActor (PolarTmuxJSONValue) async -> PolarTmuxJSONValue
    typealias SettingsProjectionSink = @MainActor (SettingsProjectionV1) -> Void
    typealias InboxPageSink = @MainActor (InboxPageV1) -> Void
    typealias InboxDeliverySink = @MainActor (
        TerminalOriginHandleV1,
        String,
        TerminalDeliveryModeV1,
        @escaping @MainActor (String?) -> Void
    ) -> Void

    @Published private(set) var projection:
        ControlCenterProjectionV1?
    @Published private(set) var nativeLayoutView:
        NativeWorkspaceLayoutView?
    @Published private(set) var sessionRuntime:
        [PolarTmuxSessionKey: PolarTmuxSessionProjection] = [:]
    @Published private(set) var isLoading = false
    @Published private(set) var loadError: String?
    @Published private(set) var lastIntent:
        ControlCenterIntentV1?
    @Published private(set) var companionPairing:
        CompanionPairingPayloadV1?
    @Published private(set) var isCompanionPairingLoading = false
    @Published private(set) var companionPairingError: String?
    @Published private(set) var resourcesProjection:
        ResourcesControlProjectionV1?
    @Published private(set) var isResourcesLoading = false
    @Published private(set) var resourcesError: String?
    @Published private(set) var resourceLogTail: ResourceLogTailV1?
    @Published private(set) var codexStateRevision: UInt64 = 0
    @Published private(set) var messagesSettings: ImessageSenderSettingsV1?
    @Published private(set) var messagesStatus: NotificationSenderStatusV1?
    @Published private(set) var messagesError: String?
    @Published private(set) var isMessagesLoading = false
    @Published private(set) var settingsProjection: SettingsProjectionV1? = .init()
    @Published private(set) var settingsError: String?
    @Published private(set) var settingsProviderTestResult: String?
    @Published private(set) var codexNotifyStatus: CodexNotifyConfigStateV1?
    @Published private(set) var inboxPage: InboxPageV1 = .init(
        messages: [],
        nextBeforeCreatedAtMilliseconds: nil,
        unreadCount: 0
    )
    @Published private(set) var inboxError: String?
    @Published private(set) var focusedInboxMessageID: String?
    @Published private(set) var isInboxLoading = false
    @Published private(set) var isSettingsLoading = false
    @Published private(set) var backendAvailability:
        PolarStationBackendAvailability = .starting

    var inboxOriginSource: (@MainActor (PolarTmuxSessionKey) -> [TerminalOriginHandleV1])?

    func setExtensionInputHandler(_ handler: @escaping PolarTmuxHostClient.ExtensionInputHandler) async {
        await backendSession.setExtensionInputHandler(handler)
    }
    func sendStationSession(_ payload: Data) async throws { try await backendSession.sendStationSession(payload) }
    func sendExtensionFrame(_ payload: Data) async throws { try await backendSession.sendExtensionFrame(payload) }

    func sendPaneRecovery(socketURL: URL, request: PolarTmuxPaneContentRequest) async throws {
        try await backendSession.sendPaneRecovery(socketURL: socketURL, request: request)
    }
    func requestPaneContent(socketURL: URL, request: PolarTmuxPaneContentRequest) async throws -> PolarTmuxPaneContentResponse {
        try await backendSession.requestPaneContent(socketURL: socketURL, request: request)
    }

    private let backendSession: PolarStationBackendSession
    private let frontendSessionID: String
    private var intentSink: IntentSink
    private var projectionSink: ProjectionSink = { _ in }
    private var nativeIntentSink: NativeIntentSink = { _, completion in
        completion?("The native Workspace runtime is not ready.")
    }
    private var sessionFeedSink: SessionFeedSink = { _ in }
    private var settingsProjectionSink: SettingsProjectionSink = { _ in }
    private var inboxPageSink: InboxPageSink = { _ in }
    private var inboxDeliverySink: InboxDeliverySink = { _, _, _, completion in
        completion("The native Pane runtime is not ready.")
    }
    private var workspaceRelaySink: WorkspaceRelaySink = {
        ControlCenterStore.unavailableWorkspaceRelayResponse($0)
    }

    private var backendSessionStarted = false
    private var hostIsReady: Bool
    private var projectionLoadInFlight = false
    private var reloadAfterCurrentProjectionLoad = false
    private var reloadAfterCurrentResourcesLoad = false
    private var reloadAfterCurrentInboxLoad = false
    private var resourcesLoadRequested = false
    private var backendSessionHandlersInstalled = false
    private var lastApplicationStateRevisions:
        [PolarTmuxApplicationStateAPI: UInt64] = [:]
    private let attentionInteraction = AttentionInteraction()
    var visibleLayoutWorkspaces: (@MainActor () -> [String])?

    init(
        backendSession: PolarStationBackendSession =
            PolarStationBackendSession(),
        awaitsHostReadiness: Bool = false,
        intentSink: @escaping IntentSink = { _ in }
    ) {
        self.backendSession = backendSession
        hostIsReady = !awaitsHostReadiness
        frontendSessionID =
            "mac-control-\(UUID().uuidString.lowercased())"
        self.intentSink = intentSink
    }

    func setIntentSink(
        _ intentSink: @escaping IntentSink
    ) {
        self.intentSink = intentSink
    }

    func setProjectionSink(
        _ projectionSink: @escaping ProjectionSink
    ) {
        self.projectionSink = projectionSink
        if let projection {
            projectionSink(projection)
        }
    }

    func setNativeLayoutView(
        _ view: NativeWorkspaceLayoutView?
    ) {
        guard nativeLayoutView != view else { return }
        nativeLayoutView = view
        let update = PolarTmuxSessionFeedUpdate.snapshot(.init(epoch: "native-layout", revision: 0, sessions: Array(sessionRuntime.values)))
        if shouldSynchronizeInbox(for: update) { loadInbox() }
    }

    func setNativeIntentSink(
        _ nativeIntentSink: @escaping NativeIntentSink
    ) {
        self.nativeIntentSink = nativeIntentSink
    }

    func setSessionFeedSink(
        _ sessionFeedSink: @escaping SessionFeedSink
    ) {
        self.sessionFeedSink = sessionFeedSink
    }

    func setSettingsProjectionSink(
        _ settingsProjectionSink: @escaping SettingsProjectionSink
    ) {
        self.settingsProjectionSink = settingsProjectionSink
        if let settingsProjection {
            settingsProjectionSink(settingsProjection)
        }
    }

    func setInboxDeliverySink(_ sink: @escaping InboxDeliverySink) {
        inboxDeliverySink = sink
    }

    func setInboxPageSink(_ sink: @escaping InboxPageSink) {
        inboxPageSink = sink
        sink(inboxPage)
    }

    func focusInboxMessage(_ messageID: String?) {
        focusedInboxMessageID = messageID
    }

    func setWorkspaceRelaySink(
        _ workspaceRelaySink: @escaping WorkspaceRelaySink
    ) {
        self.workspaceRelaySink = workspaceRelaySink
    }

    func reportNativeWorkspaceFailure(_ message: String) { loadError = message }

    func dismissLoadError() {
        loadError = nil
    }

    func load() {
        guard hostIsReady else {
            if projection == nil {
                isLoading = true
            }
            return
        }
        guard !projectionLoadInFlight else {
            reloadAfterCurrentProjectionLoad = true
            return
        }
        projectionLoadInFlight = true
        isLoading = true
        Task {
            defer { finishProjectionLoad() }
            do {
                try await ensureBackendSession()
                let response = try await backendSession.requestControlCenter(
                    ControlCenterRequestEnvelopeV1(
                        requestID: requestID(),
                        frontendSessionID: frontendSessionID,
                        expectedControlRevision: nil,
                        command: .getProjection
                    )
                )
                try accept(projection(from: response))
            } catch {
                failProjectionLoad(error)
            }
        }
    }

    /// Start the shared client immediately. Only its authenticated Ready event
    /// enables Host actions and triggers initial projection/settings loads.
    func startHostConnection() {
        Task { try? await ensureBackendSession() }
    }

    /// Marks the local shell visible immediately. If Host IPC is not ready,
    /// the renderer keeps a compact loading state while the shared client connects.
    func prepareForPresentation() {
        guard projection == nil else { return }
        isLoading = true
        guard hostIsReady else { return }
        load()
    }

    func hostStartupFailed(_ error: Error) {
        hostIsReady = false
        projectionLoadInFlight = false
        isLoading = false
        loadError = error.localizedDescription
        backendAvailability = .unavailable(
            message: error.localizedDescription
        )
    }

    func retryBackend() {
        Task {
            // Register even after cold Host startup failure, so Retry has a
            // shared Rust worker to wake. Do not wait for registration first.
            let start = Task { try await ensureBackendSession() }
            await backendSession.retryNow()
            do {
                try await start.value
            } catch {
                failProjectionLoad(error)
            }
        }
    }

    func loadCompanionPairing() {
        guard !isCompanionPairingLoading else { return }
        isCompanionPairingLoading = true
        companionPairingError = nil
        Task {
            defer { isCompanionPairingLoading = false }
            do {
                try await ensureBackendSession()
                companionPairing =
                    try await backendSession.companionPairing()
            } catch {
                companionPairingError =
                    error.localizedDescription
            }
        }
    }

    @Published var resourceFilePrefill:ResourceFileAccessV1?
    var requestFileMapping:(()->Void)?
    private let resourceFileFlow = SharedResourceFileOpen()
    private let resourceFilePresentation = PolarStationFileOpenPresentation()
    func bindResourceFileWindow(_ window:NSWindow) {
        resourceFilePresentation.bind(to:window)
        resourceFilePresentation.sendInput = { [weak self] input in self?.fileOpenInput(input) }
    }
    func openResourceFile(target:String,path:String,cwd:String?,terminalSessionID:String?=nil) {
        fileOpenInput(["type":"open","request_id":requestID(),"text":path,"origin":["execution_target_id":target,"cwd":cwd as Any? ?? NSNull(),"terminal_session_id":terminalSessionID as Any? ?? NSNull()]])
    }
    func cancelResourceFile(){resourceFilePrefill=nil;fileOpenInput(["type":"cancel"])}
    private func resolveResourceFile(){fileOpenInput(["type":"refresh","request_id":requestID()])}
    private func fileOpenInput(_ value:[String:Any]) {
        for effect in resourceFileFlow.dispatch(value) {
            if resourceFilePresentation.present(effect) {continue}
            switch effect["type"] as? String {
            case "resolve_open", "plan_access":
                guard let id=effect["request_id"] as? String else{continue}
                do {
                    let command:ResourcesCommandV1
                    if effect["type"] as? String == "resolve_open" {
                        let source=try JSONDecoder().decode(ResourceFileOriginV1.self,from:JSONSerialization.data(withJSONObject:effect["origin"] as Any))
                        let input=try JSONDecoder().decode(PolarTmuxJSONValue.self,from:JSONSerialization.data(withJSONObject:effect["input"] as Any))
                        command = .resolveFileOpen(origin:source,input:input)
                    } else {
                        let file=try JSONDecoder().decode(PolarTmuxJSONValue.self,from:JSONSerialization.data(withJSONObject:effect["file"] as Any))
                        command = .planFileAccess(file:file)
                    }
                    Task {
                        do {
                            try await ensureBackendSession()
                            let response=try await backendSession.requestResources(.init(requestID:id,expectedRevision:nil,command:command))
                            switch response.result {
                            case .fileOpen(let resolution):
                                let data=try JSONEncoder().encode(resolution)
                                fileOpenInput(["type":"resolved","request_id":id,"resolution":try JSONSerialization.jsonObject(with:data)])
                            case .error(let error,_):fileOpenInput(["type":"failed","request_id":id,"message":error.message])
                            default:fileOpenInput(["type":"failed","request_id":id,"message":"Unexpected file-open response."])
                            }
                        }catch{fileOpenInput(["type":"failed","request_id":id,"message":error.localizedDescription])}
                    }
                }catch{fileOpenInput(["type":"failed","request_id":id,"message":error.localizedDescription])}
            case "start_resource":if let id=effect["resource_id"] as? String{restartResource(id)}
            case "create_mapping":
                if let access=effect["access"],let data=try? JSONSerialization.data(withJSONObject:access),let prefill=try? JSONDecoder().decode(ResourceFileAccessV1.self,from:data){resourceFilePrefill=prefill;requestFileMapping?()}
            default:break
            }
        }
    }

    func loadResources() {
        resourcesLoadRequested = true
        guard hostIsReady else { return }
        guard !isResourcesLoading else {
            reloadAfterCurrentResourcesLoad = true
            return
        }
        resourcesLoadRequested = false
        isResourcesLoading = true
        resourcesError = nil
        Task {
            defer { finishResourcesLoad() }
            do {
                try await ensureBackendSession()
                let response = try await backendSession.requestResources(
                    ResourcesRequestEnvelopeV1(
                        requestID: requestID(),
                        expectedRevision: nil,
                        command: .getProjection
                    )
                )
                acceptResources(response)
            } catch {
                resourcesError = error.localizedDescription
                resourcesLoadRequested = true
            }
        }
    }

    func applyResources(_ catalog: ResourceCatalogV1) {
        sendResources(
            .apply(catalog: catalog),
            expectedRevision: resourcesProjection?.revision
        )
    }

    func loadMessagesSender() {
        guard hostIsReady, !isMessagesLoading else { return }
        isMessagesLoading = true
        messagesError = nil
        Task {
            defer { isMessagesLoading = false }
            do {
                try await requestMessages(.getSenderSettings)
                try await requestMessages(.getSenderStatus)
            } catch { messagesError = error.localizedDescription }
        }
    }

    func authorizeMessagesSender() {
        guard hostIsReady, !isMessagesLoading else { return }
        isMessagesLoading = true
        messagesError = nil
        Task {
            defer { isMessagesLoading = false }
            do { try await requestMessages(.requestMessagesAuthorization) }
            catch { messagesError = error.localizedDescription }
        }
    }

    func saveMessagesSender(_ proposed: ImessageSenderSettingsV1, recipient: String) {
        guard hostIsReady, !isMessagesLoading, let current = messagesSettings else { return }
        isMessagesLoading = true
        messagesError = nil
        Task {
            defer { isMessagesLoading = false }
            do {
                if !recipient.isEmpty {
                    try await requestMessages(.setSenderRecipient(expectedRevision: current.revision, recipient: recipient))
                }
                guard let latest = messagesSettings else { return }
                var next = proposed
                next.recipientCredentialRef = latest.recipientCredentialRef
                next.senderGeneration = latest.senderGeneration
                try await requestMessages(.updateSenderSettings(expectedRevision: latest.revision, settings: next))
                try await requestMessages(.getSenderStatus)
            } catch { messagesError = error.localizedDescription }
        }
    }

    private func requestMessages(_ command: NotificationCommandV1) async throws {
        let response = try await backendSession.requestNotification(.init(requestID: requestID(), command: command))
        switch response.result {
        case .settings(let value): messagesSettings = value
        case .status(let value): messagesStatus = value
        case .error(let code): messagesError = code; throw PolarTmuxLocalIPCError.unexpectedFrame(code)
        case .receipt, .connector: break
        }
    }

    func loadSettings() {
        guard hostIsReady else {
            settingsError = "Background services are unavailable. Settings remain visible; retry after reconnecting."
            return
        }
        guard !isSettingsLoading else { return }
        isSettingsLoading = true
        settingsError = nil
        Task {
            defer { isSettingsLoading = false }
            do {
                try await ensureBackendSession()
                acceptSettings(try await backendSession.requestSettings(
                    SettingsRequestEnvelopeV1(requestID: requestID(), command: .getProjection)
                ))
            } catch { settingsError = error.localizedDescription }
        }
    }

    func loadInbox() {
        guard hostIsReady else { return }
        guard !isInboxLoading else {
            reloadAfterCurrentInboxLoad = true
            return
        }
        isInboxLoading = true
        inboxError = nil
        Task {
            defer { finishInboxLoad() }
            do {
                let response = try await backendSession.requestInbox(.init(
                    requestID: requestID(),
                    command: .getPage(before: nil, limit: 100)
                ))
                acceptInbox(response)
            } catch { inboxError = error.localizedDescription }
        }
    }

    func replyToInboxMessage(_ message: InboxMessageProjectionV1, reply: String) {
        guard hostIsReady, !isInboxLoading else { return }
        isInboxLoading = true
        inboxError = nil
        Task {
            defer { finishInboxLoad() }
            do {
                acceptInbox(try await backendSession.requestInbox(.init(
                    requestID: requestID(),
                    command: .reply(
                        messageID: message.id,
                        expectedRevision: message.revision,
                        reply: reply
                    )
                )))
                loadInbox()
            } catch { inboxError = error.localizedDescription }
        }
    }

    func approveInboxMessage(_ message: InboxMessageProjectionV1) {
        guard hostIsReady, !isInboxLoading, message.draftStatus == .ready else { return }
        isInboxLoading = true
        inboxError = nil
        let mode = settingsProjection?.appearance.terminalDeliveryMode ?? .insert
        Task {
            defer { finishInboxLoad() }
            do {
                acceptInbox(try await backendSession.requestInbox(.init(
                    requestID: requestID(),
                    command: .approve(
                        messageID: message.id,
                        expectedRevision: message.revision,
                        deliveryMode: mode,
                        approvalNonce: "approval-\(UUID().uuidString.lowercased())"
                    )
                )))
            } catch { inboxError = error.localizedDescription }
        }
    }

    func editInboxDraft(_ message: InboxMessageProjectionV1, draft: String) {
        guard hostIsReady, !isInboxLoading, message.draftStatus == .ready else { return }
        isInboxLoading = true
        inboxError = nil
        Task {
            defer { finishInboxLoad() }
            do {
                acceptInbox(try await backendSession.requestInbox(.init(
                    requestID: requestID(),
                    command: .editDraft(
                        messageID: message.id,
                        expectedRevision: message.revision,
                        draft: draft
                    )
                )))
                loadInbox()
            } catch { inboxError = error.localizedDescription }
        }
    }

    func updateAppearanceSettings(_ value: AppearanceSettingsV1) {
        sendSettings(.updateAppearance(value))
    }

    func updateAttentionSettings(_ value: AttentionSettingsV1) {
        guard value.systemNotificationsEnabled else {
            sendSettings(.updateAttention(value))
            return
        }
        Task { @MainActor [weak self] in
            guard let self else { return }
            let center = UNUserNotificationCenter.current()
            let settings = await center.notificationSettings()
            switch settings.authorizationStatus {
            case .authorized, .provisional, .ephemeral:
                self.sendSettings(.updateAttention(value))
            case .denied:
                self.settingsError = "Notifications are denied for Polar Station in System Settings. Polar Station will not ask again automatically."
            case .notDetermined:
                do {
                    if try await center.requestAuthorization(options: [.alert, .sound]) {
                        self.sendSettings(.updateAttention(value))
                    } else {
                        self.settingsError = "Notification permission was not granted."
                    }
                } catch {
                    self.settingsError = error.localizedDescription
                }
            @unknown default:
                self.settingsError = "Notification authorization state is unavailable."
            }
        }
    }

    func updateSessionSummarySettings(_ value: SessionSummarySettingsV1) {
        sendSettings(.updateSessionSummary(value))
    }

    func updateMonitoringSettings(_ value: MonitoringSettingsV1) {
        sendSettings(.updateMonitoring(value))
    }

    func testSessionSummaryProvider(_ value: SessionSummarySettingsV1) {
        guard hostIsReady, !isSettingsLoading else { return }
        isSettingsLoading = true
        settingsError = nil
        settingsProviderTestResult = nil
        Task {
            defer { isSettingsLoading = false }
            do {
                let response = try await backendSession.requestSettings(
                    SettingsRequestEnvelopeV1(requestID: requestID(), command: .testSessionSummaryProvider(value))
                )
                acceptSettings(response)
            } catch { settingsError = error.localizedDescription }
        }
    }

    func requestConnector(_ command: ConnectorCommandV1) async throws -> ConnectorResultV1 {
        let response = try await backendSession.requestNotification(.init(requestID: requestID(), command: .connector(command)))
        guard case .connector(let result) = response.result else { throw PolarTmuxLocalIPCError.unexpectedFrame("Connector unavailable") }
        if let code = result.code { throw PolarTmuxLocalIPCError.unexpectedFrame(code) }
        return result
    }

    func requestCodex(owner: String?, command: CodexCommandV1) async throws -> CodexResponseEnvelopeV1 {
        try await backendSession.requestCodex(ownerHostID: owner, command: command)
    }

    func inspectCodexNotify() {
        requestCodexNotify(.getCodexNotifyStatus(executionTargetID: "local"))
    }

    func configureCodexNotify() {
        requestCodexNotify(.configureCodexNotify(executionTargetID: "local"))
    }

    private func requestCodexNotify(_ command: SettingsCommandV1) {
        guard hostIsReady, !isSettingsLoading else { return }
        isSettingsLoading = true
        settingsError = nil
        Task {
            defer { isSettingsLoading = false }
            do {
                acceptSettings(try await backendSession.requestSettings(
                    .init(requestID: requestID(), command: command)
                ))
            } catch { settingsError = error.localizedDescription }
        }
    }

    func restartResource(_ resourceID: String) {
        sendResources(
            .restart(resourceID: resourceID),
            expectedRevision: resourcesProjection?.revision
        )
    }

    func openAndroidMirror(_ resourceID: String) {
        sendResources(
            .openMirror(resourceID: resourceID),
            expectedRevision: resourcesProjection?.revision
        )
    }

    func loadResourceLog(_ resourceID: String) {
        guard hostIsReady, !isResourcesLoading else { return }
        isResourcesLoading = true
        resourcesError = nil
        Task {
            defer { isResourcesLoading = false }
            do {
                try await ensureBackendSession()
                let response = try await backendSession.requestResources(
                    ResourcesRequestEnvelopeV1(
                        requestID: requestID(),
                        expectedRevision: nil,
                        command: .getLogTail(resourceID: resourceID)
                    )
                )
                acceptResources(response)
            } catch {
                resourcesError = error.localizedDescription
            }
        }
    }

    /// Accepts a newer authoritative projection from the configured
    /// transport. It never derives or patches product state locally.
    func accept(_ projection: ControlCenterProjectionV1) {
        guard self.projection != projection else {
            loadError = nil
            return
        }
        self.projection = projection
        loadError = nil
        projectionSink(projection)
    }

    /// Re-stamps a renderer-produced operation with the latest authoritative
    /// revision before it crosses the application boundary.
    /// The native window captures its Workspace before this request suspends.
    func sendSessionCreation(
        _ intent: ControlCenterIntentV1,
        created: @escaping @MainActor (SessionRowProjectionV1.Identity) -> Void
    ) {
        submitHostIntent(intent.operation, preparedCreation: SessionCreationContext(validate: { _ in nil }, complete: { sessions in if let session = sessions.first { created(session) } }, release: {} ))
    }

    func send(_ intent: ControlCenterIntentV1) {
        send(intent.operation)
    }

    func send(_ operation: ControlCenterIntentOperationV1) {
        if case let .useExistingSession(selection) = operation {
            submitSessionReplacement(selection, completion: { _ in })
            return
        }
        if operation.requiresNativeWorkspaceMutation {
            let hydrated = hydrateNativeIdentity(
                in: operation,
                projection: projection
            )
            if let projection {
                lastIntent = ControlCenterIntentV1(
                    controlRevision: projection.controlRevision,
                    operation: hydrated
                )
            }
            nativeIntentSink(hydrated, nil)
            return
        }
        guard let projection else { return }
        let intent = ControlCenterIntentV1(
            controlRevision: projection.controlRevision,
            operation: operation
        )
        lastIntent = intent

        submitHostIntent(
            hydrateNativeIdentity(
                in: intent.operation,
                projection: projection
            )
        )
    }

    func submitSSHTarget(
        _ intent: ControlCenterIntentV1,
        completion: @escaping @MainActor (String?) -> Void
    ) {
        submitCompleting(intent.operation, completion: completion)
    }

    /// Resolves native-only identity defaults without submitting any Host
    /// request. The managed application executes the returned operation on its
    /// local Workspace command path.
    func prepareNativeWorkspaceOperation(
        _ operation: ControlCenterIntentOperationV1,
        layout: NativeWorkspaceLayoutView? = nil
    ) -> ControlCenterIntentOperationV1 {
        hydrateNativeIdentity(in: operation, projection: projection, layout: layout)
    }

    /// Sends an AppKit-native Tab choice through the single-writer native
    /// Workspace queue. The completion resolves the exact native generation.
    func submitNativeTabSelection(
        tabID: String,
        completion: @escaping @MainActor (String?) -> Void
    ) {
        submitCompleting(
            .selectTab(tabID: tabID),
            completion: completion
        )
    }

    private func submitCompleting(
        _ operation: ControlCenterIntentOperationV1,
        completion: @escaping @MainActor (String?) -> Void
    ) {
        if case let .useExistingSession(selection) = operation {
            submitSessionReplacement(selection, completion: completion)
            return
        }
        if operation.requiresNativeWorkspaceMutation {
            let hydratedOperation = hydrateNativeIdentity(
                in: operation,
                projection: projection
            )
            if let projection {
                lastIntent = ControlCenterIntentV1(
                    controlRevision: projection.controlRevision,
                    operation: hydratedOperation
                )
            }
            nativeIntentSink(hydratedOperation, completion)
            return
        }
        guard let projection else {
            completion("The Control Center is not ready.")
            return
        }
        let hydratedOperation = hydrateNativeIdentity(
            in: operation,
            projection: projection
        )
        let currentIntent = ControlCenterIntentV1(
            controlRevision: projection.controlRevision,
            operation: hydratedOperation
        )
        lastIntent = currentIntent
        submitHostIntent(
            currentIntent.operation,
            completion: completion
        )
    }

    private func replacementFailure(_ result: PolarStationBackendMutationResult<ControlCenterResponseEnvelopeV1>) -> String {
        switch result {
        case .accepted: ""
        case .rejected(let message), .outcomeUnknown(let message): message
        }
    }

    private func submitSessionReplacement(
        _ selection: SessionReplacementSelectionV1,
        completion: @escaping @MainActor (String?) -> Void
    ) {
        guard let view = nativeLayoutView, view.workspaceID == selection.workspaceID else {
            completion("The selected Workspace changed. Choose the Session again.")
            return
        }
        loadError = nil
        Task {
            try? await ensureBackendSession()
            func request(_ retire: Bool) async -> PolarStationBackendMutationResult<ControlCenterResponseEnvelopeV1> {
                await backendSession.submitControlCenterMutation {
                    try await MainActor.run {
                        guard let projection = self.projection else { throw ControlCenterStoreError.controlCenterNotReady }
                        return ControlCenterRequestEnvelopeV1(
                            requestID: self.requestID(), frontendSessionID: self.frontendSessionID,
                            expectedControlRevision: projection.controlRevision,
                            command: .submitIntent(.useExistingSession(identity: selection.identity, replacement: selection.replacement, confirmationID: selection.confirmationID, retire: retire))
                        )
                    }
                }
            }
            let validated = await request(false)
            switch validated {
            case .accepted(let response):
                do { accept(try projection(from: response)) }
                catch { loadError = error.localizedDescription; completion(loadError); return }
            default:
                // Reuse ordinary Host error handling below through the result helper.
                loadError = replacementFailure(validated)
                completion(loadError); return
            }
            let nativeError: String? = await withCheckedContinuation { continuation in
                nativeIntentSink(.useExistingSession(selection: selection)) { error in continuation.resume(returning: error) }
            }
            if let nativeError { loadError = nativeError; completion(nativeError); return }
            let retired = await request(true)
            switch retired {
            case .accepted(let response):
                do { accept(try projection(from: response)); completion(nil) }
                catch { loadError = error.localizedDescription; completion(loadError) }
            default:
                loadError = "Workspace saved; the lost record could not be removed. " + replacementFailure(retired)
                completion(loadError)
            }
        }
    }

    func readSessionEditor(_ editor: SessionEditorRequestV1) async throws -> SessionEditorResultV1 {
        try await ensureBackendSession()
        let response = try await backendSession.requestControlCenter(.init(
            requestID: requestID(), frontendSessionID: frontendSessionID,
            expectedControlRevision: nil, command: .sessionEditor(editor)))
        switch response.result {
        case .sessionEditor(let value): return value
        case .error(let error, _): throw error
        default: throw PolarTmuxLocalIPCError.unexpectedFrame("No Session editor result")
        }
    }

    private func submitHostIntent(
        _ operation: ControlCenterIntentOperationV1,
        completion: (@MainActor (String?) -> Void)? = nil,
        preparedCreation: SessionCreationContext? = nil,
        completedThreads: (@MainActor ([String]) -> Void)? = nil
    ) {
        if case .closeAuxiliary = operation {
            guard let projection else { return }
            intentSink(.init(
                controlRevision: projection.controlRevision,
                operation: operation
            ))
            return
        }
        let hostIntent: ControlCenterHostIntentV1?
        switch operation {
        case let .acknowledgePaneSummary(paneID, revision):
            hostIntent = nativeSessionIdentity(paneID).map { .acknowledgeSessionSummary(identity: $0, summaryRevision: revision) }
        case let .markAttentionNotificationHandled(paneID, revision):
            hostIntent = nativeSessionIdentity(paneID).map { .markAttentionNotificationHandled(identity: $0, summaryRevision: revision) }
        case let .invokeProjectedAction(actionID):
            if let row = projection?.sessions.rows.first(where: { $0.restartAction?.id == actionID }) {
                hostIntent = .restartSession(identity: row.identity)
            } else { hostIntent = operation.hostIntent }
        default: hostIntent = operation.hostIntent
        }
        guard var hostIntent = hostIntent else {
            if let projection {
                intentSink(.init(controlRevision: projection.controlRevision, operation: operation))
            }
            completion?(nil)
            return
        }
        let createdSession: SessionCreationContext?
        switch operation {
        case .createSession, .createCodexSession, .createAgentSession: createdSession = preparedCreation ?? prepareSessionCreation?()
        default: createdSession = nil
        }
        if let createdSession {
            if case let .createAgentSession(target, provider, mode, name, directory, endpoint, selectedID, yolo, localCLI, ids) = hostIntent,
               provider == "codex", mode == "resume" {
                let chosen = ids.isEmpty ? selectedID.map { [$0] } ?? [] : ids
                let remaining = chosen.filter { id in
                    !(projection?.agentSessions ?? []).contains { agent in
                        agent.execution_target_id == target && agent.endpoint_id == endpoint && agent.thread_id == id
                            && (agent.terminals ?? []).contains(where: createdSession.contains)
                    }
                }
                completedThreads?(chosen.filter { !remaining.contains($0) })
                if remaining.isEmpty { completion?(nil); return }
                hostIntent = .createAgentSession(executionTargetID: target, provider: provider, mode: mode, name: name,
                    workingDirectory: directory, endpointID: endpoint, sessionID: remaining[0], yolo: yolo,
                    openLocalCLI: localCLI, sessionIDs: remaining)
            }
            let count: Int
            if case let .createAgentSession(_, _, _, _, _, _, _, _, _, ids) = hostIntent { count = max(1, ids.count) } else { count = 1 }
            if let error = createdSession.validate(count) { completion?(error); return }
        }
        loadError = nil
        sessionCreationRecovery = nil
        let creationKey: Data? = createdSession == nil ? nil : {
            let encoder = JSONEncoder(); encoder.outputFormatting = .sortedKeys
            return try? encoder.encode(hostIntent)
        }()
        let submittedHostIntent = hostIntent
        Task {
            try? await ensureBackendSession()
            let result = await backendSession.submitControlCenterMutation {
                try await MainActor.run {
                    guard let projection = self.projection else {
                        throw ControlCenterStoreError.controlCenterNotReady
                    }
                    let requestID: String
                    if let creationKey {
                        requestID = self.creationRequestIDs[creationKey] ?? self.requestID()
                        self.creationRequestIDs[creationKey] = requestID
                    } else { requestID = self.requestID() }
                    if case .createSession = operation { StationOperationTrace.record(46, identity: requestID, generation: projection.controlRevision) }
                    return ControlCenterRequestEnvelopeV1(
                        requestID: requestID,
                        frontendSessionID: self.frontendSessionID,
                        expectedControlRevision:
                            projection.controlRevision,
                        command: .submitIntent(submittedHostIntent)
                    )
                }
            }
            switch result {
            case .accepted(let response):
                do {
                    if case .createSession = operation { StationOperationTrace.record(47, identity: response.requestID, generation: response.controlRevision) }
                    if case .error(let failure, _) = response.result { sessionCreationRecovery = failure.resume }
                    let acceptedProjection = try projection(from: response)
                    if let creationKey { creationRequestIDs.removeValue(forKey: creationKey) }
                    accept(acceptedProjection)
                    if case let .sessionCreated(_, session) = response.result { createdSession?.complete([session]) }
                    if case let .sessionBatchCreated(_, items) = response.result {
                        createdSession?.complete(items.compactMap(\.session))
                        completedThreads?(items.filter { $0.session != nil }.map(\.thread_id))
                        let failures = items.compactMap { item in item.error.map { "\(item.thread_id): \($0.message)" } }
                        if !failures.isEmpty { completion?(failures.joined(separator: "\n")); return }
                    }
                    if case .createSession = operation { StationOperationTrace.record(48, identity: response.requestID, generation: acceptedProjection.controlRevision) }
                    let intent = ControlCenterIntentV1(
                        controlRevision:
                            acceptedProjection.controlRevision,
                        operation: operation
                    )
                    lastIntent = intent
                    intentSink(intent)
                    completion?(nil)
                } catch {
                    createdSession?.release()
                    loadError = error.localizedDescription
                    completion?(error.localizedDescription)
                }
            case .rejected(let message),
                 .outcomeUnknown(let message):
                createdSession?.release()
                loadError = message
                NSLog(
                    "Polar Station Control Center intent failed: %@",
                    message
                )
                completion?(message)
            }
        }
    }

    private func nativeSessionIdentity(_ paneID: String) -> SessionRowProjectionV1.Identity? {
        guard let pane = nativeLayoutView?.tabs.flatMap(\.panes).first(where: { $0.paneID == paneID }) else { return nil }
        guard pane.content == nil, let target = pane.executionTargetID, let session = pane.tmuxSessionID else { return nil }
        return .init(executionTargetID: target, tmuxSessionID: session)
    }

    private func hydrateNativeIdentity(
        in operation: ControlCenterIntentOperationV1,
        projection: ControlCenterProjectionV1?,
        layout: NativeWorkspaceLayoutView? = nil
    ) -> ControlCenterIntentOperationV1 {
        let nativeLayoutView = layout ?? self.nativeLayoutView
        switch operation {
        case .addTab(let title, nil):
            return .addTab(
                title: title,
                tabID: UUID().uuidString.lowercased()
            )
        case let .toggleSessionAssignment(identity, suppliedTabID, suppliedPaneID):
            let selectedTabID = suppliedTabID
                ?? nativeLayoutView?.selectedTabID
                ?? ""
            let nativeTab = nativeLayoutView?.tab(selectedTabID)
            let existing = nativeTab?.panes.first {
                $0.executionTargetID == identity.executionTargetID
                    && $0.tmuxSessionID == identity.tmuxSessionID
            }
            return .toggleSessionAssignment(
                identity: identity,
                tabID: selectedTabID,
                paneID: suppliedPaneID
                    ?? existing?.paneID
                    ?? UUID().uuidString.lowercased()
            )
        case .deleteTab(let tabID, let confirmationID, nil)
            where (nativeLayoutView?.tabs.count
                ?? 0) == 1:
            return .deleteTab(
                tabID: tabID,
                confirmationID: confirmationID,
                replacementTabID: UUID().uuidString.lowercased()
            )
        default:
            return operation
        }
    }

    private func ensureBackendSession() async throws {
        if !backendSessionHandlersInstalled {
            await backendSession.setAvailabilityHandler {
                [weak self] availability in
                await self?.backendAvailabilityChanged(availability)
            }
            await backendSession.setApplicationStateHandler {
                [weak self] change in
                await self?.applicationStateChanged(change)
            }
            await backendSession.setSessionFeedHandler {
                [weak self] update in
                await self?.sessionFeedChanged(update)
            }
            await backendSession.setWorkspaceRelayHandler {
                [weak self] request in
                guard let self else {
                    return Self.unavailableWorkspaceRelayResponse(request)
                }
                return await self.workspaceRelaySink(request)
            }
            backendSessionHandlersInstalled = true
        }
        guard !backendSessionStarted else { return }
        try await backendSession.start(
            frontendID: "polar-station-macos",
            supportedActions: [
                "open_session",
                "refresh",
            ]
        )
        backendSessionStarted = true
    }

    private func backendAvailabilityChanged(
        _ availability: PolarStationBackendAvailability
    ) {
        backendAvailability = availability
        if case .ready = availability {
            attentionInteraction.reconnect()
            NotificationCenter.default.post(name: Notification.Name("PolarStationRecoveryTransportReady"), object: nil)
            hostIsReady = true
            backendSessionStarted = true
            load()
            loadSettings()
            loadInbox()
            if resourcesLoadRequested {
                loadResources()
            }
        }
    }

    private func sessionFeedChanged(
        _ update: PolarTmuxSessionFeedUpdate
    ) {
        switch update {
        case .snapshot(let snapshot):
            sessionRuntime = Dictionary(
                uniqueKeysWithValues: snapshot.sessions.map { ($0.key, $0) }
            )
        case .event(let event):
            var next = sessionRuntime
            for change in event.changes {
                switch change {
                case .upsert(let session):
                    next[session.key] = session
                case .remove(let key):
                    next.removeValue(forKey: key)
                }
            }
            sessionRuntime = next
        }
        sessionFeedSink(update)
        if shouldSynchronizeInbox(for: update) {
            loadInbox()
        }
    }

    private func shouldSynchronizeInbox(
        for update: PolarTmuxSessionFeedUpdate
    ) -> Bool {
        let observations = sessionRuntime.values.flatMap { session -> [AttentionInteraction.Observation] in
            guard let attention = session.attention else { return [] }
            return (inboxOriginSource?(session.key) ?? []).map { .init(origin: $0, attention: attention) }
        }
        let origins = attentionInteraction.observe(observations)
        for origin in origins {
            Task {
                do {
                    let response = try await backendSession.requestInbox(.init(requestID: requestID(), command: .observeAttention(origin: origin)))
                    acceptInbox(response)
                } catch { inboxError = error.localizedDescription }
            }
        }
        return !origins.isEmpty
    }

    private func updateInboxNotifications(_ page: InboxPageV1) {
        let notices = attentionInteraction.messages(page, visibleWorkspaces: visibleLayoutWorkspaces?() ?? [],
            enabled: settingsProjection?.attention.systemNotificationsEnabled == true)
        for notice in notices {
            let content = UNMutableNotificationContent()
            content.title = "Polar Station"
            content.body = "A session needs your attention."
            content.sound = .default
            content.userInfo = ["inbox_message_id": notice.message_id, "workspace_id": notice.workspace_id]
            UNUserNotificationCenter.current().add(.init(identifier: "polar-station-attention-\(notice.workspace_id)", content: content, trigger: nil))
        }
    }

    nonisolated static func unavailableWorkspaceRelayResponse(
        _ request: PolarTmuxJSONValue
    ) -> PolarTmuxJSONValue {
        guard case let .object(requestFields) = request,
              case let .string(requestID) = requestFields["request_id"],
              let scope = requestFields["scope"]
        else { return .object([:]) }
        return .object([
            "protocol_version": .number(1),
            "api": .string("workspace_v1"),
            "request_id": .string(requestID),
            "scope": scope,
            "revision": .number(0),
            "result": .object([
                "type": .string("error"),
                "error": .object([
                    "code": .string("desktop_workspace_unavailable"),
                    "message": .string("Native Workspace owner is unavailable"),
                    "retryable": .bool(true),
                ]),
            ]),
        ])
    }

    private func projection(
        from response: ControlCenterResponseEnvelopeV1
    ) throws -> ControlCenterProjectionV1 {
        switch response.result {
        case .sessionEditor: throw PolarTmuxLocalIPCError.unexpectedFrame("Expected a Control Center projection")
        case let .projection(projection),
             let .intentAccepted(projection),
             let .sessionCreated(projection, _), let .sessionBatchCreated(projection, _):
            return projection
        case let .error(error, freshProjection):
            if let freshProjection {
                accept(freshProjection)
            }
            throw error

        }
    }

    func quickCreateSession(
        executionTargetID: String, sourceSessionID: String?, kind: QuickSessionKindV1,
        completion: @escaping @MainActor (SessionRowProjectionV1.Identity?) -> Void
    ) {
        Task {
            try? await ensureBackendSession()
            let result = await backendSession.submitControlCenterMutation {
                try await MainActor.run {
                    guard let projection = self.projection else { throw ControlCenterStoreError.controlCenterNotReady }
                    return ControlCenterRequestEnvelopeV1(requestID: self.requestID(), frontendSessionID: self.frontendSessionID,
                        expectedControlRevision: projection.controlRevision,
                        command: .submitIntent(.quickCreateSession(executionTargetID: executionTargetID, sourceSessionID: sourceSessionID, kind: kind)))
                }
            }
            switch result {
            case .accepted(let response):
                if case let .error(error, fresh) = response.result {
                    if let fresh { accept(fresh) }
                    loadError = error.message; completion(nil); return
                }
                guard case let .sessionCreated(projection, session) = response.result else {
                    loadError = "Creation result is unknown. Refresh Sessions before retrying."
                    completion(nil); return
                }
                accept(projection)
                completion(session)
            case .rejected(let message), .outcomeUnknown(let message):
                loadError = message; completion(nil)
            }
        }
    }

    private func requestID() -> String {
        "mac-\(UUID().uuidString.lowercased())"
    }

    private func failProjectionLoad(_ error: Error) {
        loadError = error.localizedDescription
    }

    func presentLocalError(_ error: Error) {
        loadError = error.localizedDescription
    }

    private func finishProjectionLoad() {
        projectionLoadInFlight = false
        guard reloadAfterCurrentProjectionLoad else {
            isLoading = false
            return
        }
        reloadAfterCurrentProjectionLoad = false
        load()
    }

    private func applicationStateChanged(
        _ change: PolarTmuxApplicationStateChange
    ) {
        guard change.stateRevision
            > (lastApplicationStateRevisions[change.api] ?? 0)
        else {
            return
        }
        lastApplicationStateRevisions[change.api] =
            change.stateRevision
        switch change.api {
        case .controlCenterV1:
            load()
        case .resourcesV1:
            guard resourcesProjection != nil
                || isResourcesLoading
            else {
                return
            }
            loadResources()
        case .codexV1:
            codexStateRevision = change.stateRevision
            load()
            loadInbox()
        case .notificationV1:
            if messagesSettings != nil { loadMessagesSender() }
        case .settingsV1:
            guard settingsProjection != nil || isSettingsLoading else { return }
            loadSettings()
        case .inboxV1:
            loadInbox()
        }
    }

    private func finishResourcesLoad() {
        isResourcesLoading = false
        guard reloadAfterCurrentResourcesLoad else {
            return
        }
        reloadAfterCurrentResourcesLoad = false
        loadResources()
    }

    private func finishInboxLoad() {
        isInboxLoading = false
        guard reloadAfterCurrentInboxLoad else {
            return
        }
        reloadAfterCurrentInboxLoad = false
        loadInbox()
    }

    private func sendResources(
        _ command: ResourcesCommandV1,
        expectedRevision: UInt64?
    ) {
        guard let expectedRevision,
              !isResourcesLoading
        else { return }
        isResourcesLoading = true
        resourcesError = nil
        Task {
            defer { isResourcesLoading = false }
            try? await ensureBackendSession()
            let result = await backendSession.submitResourcesMutation(
                ResourcesRequestEnvelopeV1(
                    requestID: requestID(),
                    expectedRevision: expectedRevision,
                    command: command
                )
            )
            switch result {
            case .accepted(let response):
                acceptResources(response)
            case .rejected(let message),
                 .outcomeUnknown(let message):
                resourcesError = message
            }
        }
    }

    private func acceptResources(
        _ response: ResourcesResponseEnvelopeV1
    ) {
        switch response.result {
        case let .projection(projection),
             let .commandAccepted(projection):
            resourcesProjection = projection
            resourcesError = nil
            resolveResourceFile()
        case .fileAccess, .fileOpen:break
        case let .logTail(tail):
            resourceLogTail = tail
            resourcesError = nil
        case let .error(error, projection):
            if let projection {
                resourcesProjection = projection
            }
            resourcesError = error.message
        }
    }

    private func sendSettings(_ command: SettingsCommandV1) {
        guard hostIsReady else {
            settingsError = "Background services are unavailable, so this change was not saved."
            return
        }
        guard let revision = settingsProjection?.revision, !isSettingsLoading else { return }
        isSettingsLoading = true
        settingsError = nil
        Task {
            defer { isSettingsLoading = false }
            let result = await backendSession.submitSettingsMutation(
                SettingsRequestEnvelopeV1(requestID: requestID(), expectedRevision: revision, command: command)
            )
            switch result {
            case .accepted(let response): acceptSettings(response)
            case .rejected(let message), .outcomeUnknown(let message): settingsError = message
            }
        }
    }

    private func acceptSettings(_ response: SettingsResponseEnvelopeV1) {
        switch response.result {
        case .projection(let projection):
            settingsProjection = projection
            settingsProjectionSink(projection)
            settingsError = nil
            settingsProviderTestResult = nil
        case .providerTested(let summary):
            settingsProviderTestResult = summary
            settingsError = nil
        case .codexNotifyStatus(_, let status):
            codexNotifyStatus = status
            settingsError = nil
        case .error(let error, let projection):
            if let projection {
                settingsProjection = projection
                settingsProjectionSink(projection)
            }
            settingsError = error.message
        }
    }

    private func acceptInbox(_ response: InboxResponseEnvelopeV1) {
        switch response.result {
        case .page(let page):
            updateInboxNotifications(page)
            inboxPage = page
            inboxPageSink(page)
            inboxError = nil
        case .message:
            inboxError = nil
        case .deliveryRequested(let messageID, let origin, let text, let mode, _):
            inboxDeliverySink(origin, text, mode) { [weak self] error in
                guard let self else { return }
                self.inboxError = error
                let approvedRevision = (self.inboxPage.messages.first { $0.id == messageID }?.revision ?? 0) + 1
                Task {
                    do {
                        _ = try await self.backendSession.requestInbox(.init(
                            requestID: self.requestID(),
                            command: .recordDelivery(
                                messageID: messageID,
                                expectedRevision: approvedRevision,
                                state: error == nil ? .delivered : .failedBeforeWrite
                            )
                        ))
                        self.loadInbox()
                    } catch {
                        self.inboxError = error.localizedDescription
                    }
                }
            }
        case .deliveryReceipt:
            loadInbox()
        case .error(let error, _):
            inboxError = error.message
        }
    }
}

private enum ControlCenterStoreError: LocalizedError {
    case unexpectedStructureResult
    case controlCenterNotReady

    var errorDescription: String? {
        switch self {
        case .unexpectedStructureResult:
            "The Host returned an unexpected Workspace structure result."
        case .controlCenterNotReady:
            "The Control Center has no authoritative projection yet."
        }
    }
}

extension ControlCenterIntentOperationV1 {
    var requiresNativeWorkspaceMutation: Bool {
        switch self {
        case let .invokeConfirmedAction(actionID, _)
            where ["workspace.restore", "workspace.archive", "workspace.unarchive"].contains(actionID):
            true
        case .setManagedTerminalFontSize,
             .createWorkspace,
             .selectWorkspace,
             .renameWorkspace,
             .deleteWorkspace,
             .addTab,
             .selectTab,
             .renameTab,
             .deleteTab,
             .reorderTab,
             .reorderPane,
             .movePane,
             .toggleDisplayMode,
             .toggleSessionAssignment,
             .addSessionAssignments,
             .selectPane,
             .activatePane,
             .removePane:
            true
        default:
            false
        }
    }

    var isPaneSelection: Bool {
        if case .selectPane = self { return true }
        return false
    }

    var isPaneActivation: Bool {
        if case .activatePane = self { return true }
        return false
    }
}

/// UI-thread transport adapter. All pending/open semantics live in Resource Rust.
private final class SharedResourceFileOpen {
    private let handle=polar_station_resource_file_open_new()
    deinit{polar_station_resource_file_open_free(handle)}
    func dispatch(_ value:[String:Any])->[[String:Any]] {
        guard let data=try? JSONSerialization.data(withJSONObject:value) else{return []}
        let bytes=Array(data)
        guard let output=bytes.withUnsafeBufferPointer({polar_station_resource_file_open_dispatch(handle,$0.baseAddress,$0.count)}) else{return []}
        defer{polar_station_desktop_string_free(output)}
        guard let data=String(cString:output).data(using:.utf8),let result=(try? JSONSerialization.jsonObject(with:data)) as? [String:Any] else{return []}
        return result["effects"] as? [[String:Any]] ?? []
    }
}
