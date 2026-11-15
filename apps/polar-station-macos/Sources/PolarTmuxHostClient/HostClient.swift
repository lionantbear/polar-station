import Foundation

public enum PolarTmuxLocalIPCConnectionState: Equatable, Sendable {
    case connecting(generation: UInt64, attempt: Int)
    case ready(generation: UInt64)
    case unavailable(
        generation: UInt64,
        attempt: Int,
        message: String,
        retryAfterMilliseconds: UInt64
    )
}

public actor PolarTmuxHostClient {
    public typealias TransportFactory =
        @Sendable (URL) -> any PolarTmuxLocalIPCTransport
    public typealias FrontendHandler =
        @Sendable (PolarTmuxFrontendDispatch) async
            -> PolarTmuxFrontendAcknowledgement
    public typealias ResponseHandler =
        @Sendable (PolarTmuxHostEnvelope) async -> Void
    public typealias ApplicationStateHandler =
        @Sendable (PolarTmuxApplicationStateChange) async -> Void
    public typealias SessionFeedHandler =
        @Sendable (PolarTmuxSessionFeedUpdate) async -> Void
    public typealias WorkspaceRelayHandler =
        @Sendable (PolarTmuxJSONValue) async -> PolarTmuxJSONValue
    public typealias ConnectionStateHandler =
        @Sendable (PolarTmuxLocalIPCConnectionState) async -> Void

    public typealias ExtensionInputHandler = @Sendable (Data) async -> Void
    private var extensionInputHandler: ExtensionInputHandler?
    private var extensionRegistrationInput: Data?
    private var ideaHostInput: Data?
    public func setExtensionInputHandler(_ handler: @escaping ExtensionInputHandler) async {
        extensionInputHandler = handler
        if let input = extensionRegistrationInput { await handler(input) }
        if let input = ideaHostInput { await handler(input) }
    }
    public func sendStationSession(_ payload: Data) async throws {
        try await ensureTransport()
        guard let rust = transport as? PolarTmuxRustDesktopTransport else { throw PolarTmuxDesktopSendError(code:5) }
        try rust.sendSession(payload)
    }

    public func sendExtensionFrame(_ payload: Data) async throws {
        guard let transport, activeRegistration != nil else { throw PolarTmuxLocalIPCError.connectionClosed }
        guard payload.count <= 1_048_576,
              let object = try JSONSerialization.jsonObject(with: payload) as? [String: Any],
              let type = object["type"] as? String,
              (["extension_request", "extension_cancel"].contains(type) || (type == "application_request" && (object["request"] as? [String:Any])?["api"] as? String == "codex_v1")) else { throw PolarTmuxLocalIPCError.invalidJSON }
        try await transport.send(payload)
    }
    private func extensionInput(_ object: [String: Any]) async {
        guard let data = try? JSONSerialization.data(withJSONObject: object) else { return }
        if object["type"] as? String == "connected" { extensionRegistrationInput = data }
        if object["type"] as? String == "disconnected" { extensionRegistrationInput = nil; ideaHostInput = nil }
        if object["type"] as? String == "idea_host" { ideaHostInput = data }
        await extensionInputHandler?(data)
    }

    // Delivery bindings only. Rust owns admission, identity, deadlines and cancellation.
    private enum ResponsePayload: Sendable { case host(PolarTmuxLocalHostFrame), content(PolarTmuxPaneContentResponse) }
    private struct ResponseBinding {
        let generation: UInt64?
        let complete: @Sendable (Result<ResponsePayload, any Error>) -> Void
        var cancelled = false
    }
    private var responseWaiters: [String: ResponseBinding] = [:]

    private struct RegistrationDescriptor: Equatable, Sendable {
        let frontendID: String
        let supportedActions: [String]
    }

    private struct ActiveRegistration: Sendable {
        let desktopGeneration: UInt64?
        let frame: PolarTmuxLocalHostFrame
    }

    private struct SessionFeedCursor: Equatable, Sendable {
        let epoch: String
        let revision: UInt64
    }

    private let socketURL: URL
    private let transportFactory: TransportFactory
    public func sendPaneRecovery(socketURL: URL, request: PolarTmuxPaneContentRequest) throws {
        guard let rust = transport as? PolarTmuxRustDesktopTransport else { throw PolarTmuxLocalIPCError.connectionClosed }
        try rust.sendContent(socketURL: socketURL, request: request)
    }

    private var transport: (any PolarTmuxLocalIPCTransport)?
    private var readTask: Task<Void, Never>?
    private var readPumpID: UInt64 = 0
    private var desktopGeneration: UInt64 = 0
    private var registrationDescriptor: RegistrationDescriptor?
    private var activeRegistration: ActiveRegistration?
    private var bootstrapSent = false
    private var registrationWaiters:
        [CheckedContinuation<PolarTmuxLocalHostFrame, any Error>] = []
    private var frontendHandler: FrontendHandler?
    private var responseHandler: ResponseHandler?
    private var applicationStateHandler:
        ApplicationStateHandler?
    private var sessionFeedHandler: SessionFeedHandler?
    private var workspaceRelayHandler: WorkspaceRelayHandler?
    private var connectionStateHandler: ConnectionStateHandler?
    private var connectionStateDeliveryTail: Task<Void, Never>?
    private var sessionFeedCursor: SessionFeedCursor?
    private var inFlightDispatchIDs: Set<String> = []
    private var inFlightWorkspaceRelayIDs: Set<String> = []

    public init(
        socketURL: URL = PolarTmuxSocketPath.resolve(),
        transportFactory: @escaping TransportFactory = {
            PolarTmuxRustDesktopTransport(socketURL: $0)
        }
    ) {
        self.socketURL = socketURL
        self.transportFactory = transportFactory
    }

    public var localSocketURL: URL {
        socketURL
    }

    public func setFrontendHandler(
        _ handler: @escaping FrontendHandler
    ) {
        frontendHandler = handler
    }

    public func setResponseHandler(
        _ handler: @escaping ResponseHandler
    ) {
        responseHandler = handler
    }

    public func setApplicationStateHandler(
        _ handler: @escaping ApplicationStateHandler
    ) {
        applicationStateHandler = handler
    }

    public func setSessionFeedHandler(
        _ handler: @escaping SessionFeedHandler
    ) {
        sessionFeedHandler = handler
    }

    public func setWorkspaceRelayHandler(
        _ handler: @escaping WorkspaceRelayHandler
    ) {
        workspaceRelayHandler = handler
    }

    public func setConnectionStateHandler(
        _ handler: @escaping ConnectionStateHandler
    ) {
        connectionStateHandler = handler
    }

    public func register(
        frontendID: String,
        supportedActions: [String]
    ) async throws -> PolarTmuxLocalHostFrame {
        let descriptor = RegistrationDescriptor(
            frontendID: frontendID,
            supportedActions: supportedActions
        )
        if let registrationDescriptor,
           registrationDescriptor != descriptor
        {
            throw PolarTmuxLocalIPCError.duplicateRegistration
        }
        registrationDescriptor = descriptor
        try await ensureConnected()
        let frame = try await ensureRegisteredOnCurrentConnection()
        guard case .registered(_, let version, _, _) = frame else {
            throw PolarTmuxLocalIPCError.unexpectedFrame(
                String(describing: frame)
            )
        }
        guard version == polarTmuxProtocolVersion else {
            throw PolarTmuxLocalIPCError.unsupportedProtocolVersion(version)
        }
        return frame
    }

    public func request(_ request: PolarTmuxClientEnvelope) async throws -> PolarTmuxHostEnvelope {
        let response = try await awaitResponse(id: request.requestID, frame: .request(request))
        guard case .response(let value) = response else { throw PolarTmuxLocalIPCError.unexpectedFrame("Rust response family") }
        return value
    }

    public func requestControlCenter(_ request: ControlCenterRequestEnvelopeV1) async throws -> ControlCenterResponseEnvelopeV1 {
        let response = try await awaitResponse(id: request.requestID, frame: .applicationRequest(request))
        guard case .applicationResponse(let value) = response else { throw PolarTmuxLocalIPCError.unexpectedFrame("Rust response family") }
        return value
    }

    public func requestResources(_ request: ResourcesRequestEnvelopeV1) async throws -> ResourcesResponseEnvelopeV1 {
        let response = try await awaitResponse(id: request.requestID, frame: .resourcesRequest(request))
        guard case .resourcesResponse(let value) = response else { throw PolarTmuxLocalIPCError.unexpectedFrame("Rust response family") }
        return value
    }

    public func requestSettings(_ request: SettingsRequestEnvelopeV1) async throws -> SettingsResponseEnvelopeV1 {
        let response = try await awaitResponse(id: request.requestID, frame: .settingsRequest(request))
        guard case .settingsResponse(let value) = response else { throw PolarTmuxLocalIPCError.unexpectedFrame("Rust response family") }
        return value
    }

    public func requestNotification(_ request: NotificationRequestEnvelopeV1) async throws -> NotificationResponseEnvelopeV1 {
        let response = try await awaitResponse(id: request.requestID, frame: .notificationRequest(request))
        guard case .notificationResponse(let value) = response else { throw PolarTmuxLocalIPCError.unexpectedFrame("Rust response family") }
        return value
    }

    public func requestCodex(ownerHostID: String?, command: CodexCommandV1) async throws -> CodexResponseEnvelopeV1 {
        try await ensureConnected()
        guard let registration = activeRegistration,
              case .registered(_, _, let localHostID, _) = registration.frame else {
            throw PolarTmuxLocalIPCError.unexpectedFrame("Host registration is unavailable")
        }
        return try await requestCodex(.init(requestID: UUID().uuidString, ownerHostID: ownerHostID ?? localHostID, command: command))
    }

    public func requestCodex(_ request: CodexRequestEnvelopeV1) async throws -> CodexResponseEnvelopeV1 {
        let response = try await awaitResponse(id: request.requestID, frame: .codexRequest(request))
        guard case .codexResponse(let value) = response else { throw PolarTmuxLocalIPCError.unexpectedFrame("Rust response family") }
        return value
    }

    public func requestInbox(_ request: InboxRequestEnvelopeV1) async throws -> InboxResponseEnvelopeV1 {
        let response = try await awaitResponse(id: request.requestID, frame: .inboxRequest(request))
        guard case .inboxResponse(let value) = response else { throw PolarTmuxLocalIPCError.unexpectedFrame("Rust response family") }
        return value
    }

    public func companionPairing()
        async throws -> CompanionPairingPayloadV1
    {
        let response = try await request(.init(
            requestID:
                "pairing-\(UUID().uuidString.lowercased())",
            command: .object([
                "type": .string("get_companion_pairing"),
            ])
        ))
        return try response.decodeCompanionPairing()
    }

    public func acknowledge(
        dispatchID: String,
        result: PolarTmuxFrontendAcknowledgement
    ) async throws {
        try await ensureConnected()
        try await send(.frontendAck(
            dispatchID: dispatchID,
            result: result
        ))
    }

    public func disconnect() async {
        readPumpID &+= 1
        let activeTransport = transport
        transport = nil
        readTask?.cancel()
        readTask = nil
        activeRegistration = nil
        bootstrapSent = false
        connectionStateDeliveryTail?.cancel()
        connectionStateDeliveryTail = nil
        failAllWaiters(with: PolarTmuxLocalIPCError.connectionClosed)
        if let activeTransport {
            await activeTransport.close()
        }
    }

    public func retryNow() async {
        await transport?.retryNow()
    }

    private func ensureTransport() async throws {
        if transport == nil {
            let newTransport = transportFactory(socketURL)
            try await newTransport.connect()
            transport = newTransport
            desktopGeneration = 0
            readPumpID &+= 1
            let generation = readPumpID
            readTask = Task { [weak self] in
                await self?.readLoop(
                    generation: generation,
                    transport: newTransport
                )
            }
        }
    }
    private func ensureConnected() async throws {
        try await ensureTransport()
        if registrationDescriptor != nil { _ = try await ensureRegisteredOnCurrentConnection() }
    }

    public func requestPaneContent(socketURL: URL, request: PolarTmuxPaneContentRequest) async throws -> PolarTmuxPaneContentResponse {
        try await ensureTransport()
        guard let descriptor = registrationDescriptor else { throw PolarTmuxDesktopSendError(code: 1) }
        if !bootstrapSent { bootstrapSent = true; await sendRegistration(descriptor, generation: readPumpID) }
        guard let rust = transport as? PolarTmuxRustDesktopTransport else { throw PolarTmuxDesktopSendError(code: 5) }
        guard responseWaiters[request.requestID] == nil else { throw PolarTmuxLocalIPCError.duplicateRequestID(request.requestID) }
        return try await withCheckedThrowingContinuation { continuation in
            responseWaiters[request.requestID] = ResponseBinding(generation: nil, complete: { result in
                switch result {
                case .success(.content(let response)): continuation.resume(returning: response)
                case .failure(let error): continuation.resume(throwing: error)
                default: continuation.resume(throwing: PolarTmuxLocalIPCError.unexpectedFrame("Pane receipt"))
                }
            })
            do { try rust.sendContent(socketURL: socketURL, request: request) }
            catch { responseWaiters.removeValue(forKey: request.requestID)?.complete(.failure(error)) }
        }
    }

    private func ensureRegisteredOnCurrentConnection()
        async throws -> PolarTmuxLocalHostFrame
    {
        guard let descriptor = registrationDescriptor else {
            throw PolarTmuxLocalIPCError.duplicateRegistration
        }
        let generation = readPumpID
        if let activeRegistration {
            return activeRegistration.frame
        }
        return try await withCheckedThrowingContinuation {
            (continuation:
                CheckedContinuation<PolarTmuxLocalHostFrame, any Error>) in
            registrationWaiters.append(continuation)
            guard !bootstrapSent else { return }
            bootstrapSent = true
            Task { [weak self] in
                await self?.sendRegistration(
                    descriptor,
                    generation: generation
                )
            }
        }
    }

    private func sendRegistration(
        _ descriptor: RegistrationDescriptor,
        generation: UInt64
    ) async {
        guard generation == readPumpID else { return }
        do {
            try await send(.register(
                frontendID: descriptor.frontendID,
                platform: "macos",
                supportedActions: descriptor.supportedActions,
                capabilities: [
                    polarTmuxRefreshSnapshotCapability,
                    polarTmuxApplicationStateEventsCapability,
                    polarTmuxSessionFeedCapability,
                    polarTmuxWorkspaceRelayCapability,
                    "session_lifecycle_v2",
                ],
                frontendInstanceID: nil,
                frontendGeneration: 0
            ))
        } catch {
            await handleConnectionFailure(
                error,
                generation: generation
            )
        }
    }

    private func readLoop(
        generation: UInt64,
        transport: any PolarTmuxLocalIPCTransport
    ) async {
        do {
            while !Task.isCancelled {
                let event = try await transport.receiveEvent(
                    maximumBytes:
                        PolarTmuxFrameCodec.maximumPayloadBytes + 4
                )
                guard generation == readPumpID else { return }
                let bytes: Data
                let eventGeneration: UInt64?
                switch event {
                case .requestFailed(let generation, let failure):
                    if responseWaiters[failure.request_id]?.generation == generation {
                        finishResponse(failure.request_id, result: .failure(failure))
                    }
                    continue
                case .session(let data):
                    await extensionInputHandler?(data)
                    continue
                case .content(let response):
                    if let binding = responseWaiters.removeValue(forKey: response.requestID), binding.generation == nil {
                        binding.complete(.success(.content(response))); continue
                    }
                    await MainActor.run { NotificationCenter.default.post(name: .polarStationTerminalRecovery, object: response) }
                    continue
                case .connecting(let observed, let attempt):
                    guard observed >= desktopGeneration else { continue }
                    desktopGeneration = observed
                    activeRegistration = nil
                    await extensionInput(["type": "disconnected", "generation": observed])
                    enqueueConnectionState(.connecting(
                        generation: observed,
                        attempt: attempt
                    ))
                    continue
                case .unavailable(
                    let observed,
                    let attempt,
                    let message,
                    let retryAfterMilliseconds
                ):
                    guard observed >= desktopGeneration else { continue }
                    desktopGeneration = observed
                    activeRegistration = nil
                    await extensionInput(["type": "disconnected", "generation": observed])
                    failRegistration(PolarTmuxLocalIPCError.connectionFailed(message))
                    failPendingRequests(with: PolarTmuxLocalIPCError.connectionFailed(message), controlOnly: true)
                    enqueueConnectionState(.unavailable(
                        generation: observed,
                        attempt: attempt,
                        message: message,
                        retryAfterMilliseconds: retryAfterMilliseconds
                    ))
                    continue
                case .ready(let observed, let frame):
                    guard observed >= desktopGeneration else { continue }
                    desktopGeneration = observed
                    bytes = frame
                    eventGeneration = observed
                case .frame(let observed, let frame):
                    if let observed {
                        guard observed == desktopGeneration,
                              activeRegistration?.desktopGeneration == observed
                        else { continue }
                    }
                    bytes = frame
                    eventGeneration = observed
                }
                guard !bytes.isEmpty else {
                    throw PolarTmuxLocalIPCError.connectionClosed
                }
                for payload in [bytes] {
                    let raw = try JSONSerialization.jsonObject(with: payload) as? [String: Any]
                    let frameType = raw?["type"] as? String
                    if ["extension_response", "extension_event"].contains(frameType ?? ""), let raw {
                        await extensionInput(["type": "frame", "generation": eventGeneration ?? desktopGeneration, "frame": raw])
                        continue
                    }
                    if let raw, (frameType == "application_response" && (raw["response"] as? [String:Any])?["api"] as? String == "codex_v1") || (frameType == "application_state_changed" && raw["api"] as? String == "codex_v1") {
                        await extensionInput(["type":"frame", "generation":eventGeneration ?? desktopGeneration, "frame":raw])
                    }
                    let frame: PolarTmuxLocalHostFrame
                    do {
                        frame = try JSONDecoder().decode(
                            PolarTmuxLocalHostFrame.self,
                            from: payload
                        )
                    } catch let error as DecodingError {
                        throw Self.decodingFailure(
                            error,
                            payload: payload
                        )
                    } catch {
                        throw PolarTmuxLocalIPCError.invalidJSON
                    }
                    try await handle(
                        frame,
                        desktopGeneration: eventGeneration
                    )
                    if frameType == "registered" {
                        let grants = raw?["extensions"] as? [[String: Any]] ?? []
                        await extensionInput(["type": "connected", "generation": eventGeneration ?? desktopGeneration,
                            "extensions": grants.compactMap { $0["name"] as? String }])
                        if let host = raw?["host_id"] as? String {
                            let versions = raw?["host_control_versions"] as? [String:Any]
                            await extensionInput(["type":"idea_host","host_id":host,"supported":(versions?["codex"] as? NSNumber)?.intValue == 2])
                        }
                    }
                }
            }
        } catch is CancellationError {
            return
        } catch {
            await extensionInput(["type": "disconnected", "generation": desktopGeneration])
            await handleConnectionFailure(
                error,
                generation: generation
            )
            guard generation == readPumpID,
                  self.transport != nil
            else { return }
            readTask = Task { [weak self] in
                await self?.readLoop(
                    generation: generation,
                    transport: transport
                )
            }
        }
    }

    private func handle(
        _ frame: PolarTmuxLocalHostFrame,
        desktopGeneration observedDesktopGeneration: UInt64? = nil
    ) async throws {
        switch frame {
        case .registered(_, let version, _, _):
            guard version == polarTmuxProtocolVersion else {
                throw PolarTmuxLocalIPCError.unsupportedProtocolVersion(version)
            }
            activeRegistration = ActiveRegistration(desktopGeneration: observedDesktopGeneration, frame: frame)
            enqueueConnectionState(.ready(
                generation: observedDesktopGeneration
                    ?? readPumpID
            ))
            let waiters = registrationWaiters
            registrationWaiters.removeAll()
            waiters.forEach { $0.resume(returning: frame) }
            try await send(.subscribeSessionFeed(
                knownEpoch: sessionFeedCursor?.epoch,
                afterRevision: sessionFeedCursor?.revision
            ))
        case .response(let response):
            if let waiter = responseWaiters.removeValue(forKey: response.requestID) {
                waiter.complete(.success(.host(frame))); await responseHandler?(response)
            }
        case .applicationResponse(let response):
            if let waiter = responseWaiters.removeValue(forKey: response.requestID) { waiter.complete(.success(.host(frame))) }
        case .resourcesResponse(let response):
            if let waiter = responseWaiters.removeValue(forKey: response.requestID) { waiter.complete(.success(.host(frame))) }
        case .settingsResponse(let response):
            if let waiter = responseWaiters.removeValue(forKey: response.requestID) { waiter.complete(.success(.host(frame))) }
        case .notificationResponse(let response):
            if let waiter = responseWaiters.removeValue(forKey: response.requestID) { waiter.complete(.success(.host(frame))) }
        case .codexResponse(let response):
            if let waiter = responseWaiters.removeValue(forKey: response.requestID) { waiter.complete(.success(.host(frame))) }
        case .inboxResponse(let response):
            if let waiter = responseWaiters.removeValue(forKey: response.requestID) { waiter.complete(.success(.host(frame))) }
        case .applicationStateChanged(
            let api,
            let stateRevision
        ):
            if let applicationStateHandler {
                let change = PolarTmuxApplicationStateChange(
                    api: api,
                    stateRevision: stateRevision
                )
                Task {
                    await applicationStateHandler(change)
                }
            }
        case .sessionFeedSnapshot(let snapshot):
            guard !snapshot.epoch.isEmpty,
                  snapshot.revision > 0,
                  Set(snapshot.sessions.map(\.key)).count
                    == snapshot.sessions.count
            else {
                throw PolarTmuxLocalIPCError.unexpectedFrame(
                    "invalid Session feed snapshot"
                )
            }
            sessionFeedCursor = SessionFeedCursor(
                epoch: snapshot.epoch,
                revision: snapshot.revision
            )
            if let sessionFeedHandler {
                await sessionFeedHandler(.snapshot(snapshot))
            }
        case .sessionFeedEvent(let event):
            guard !event.epoch.isEmpty,
                  event.revision > 0,
                  !event.changes.isEmpty
            else {
                throw PolarTmuxLocalIPCError.unexpectedFrame(
                    "invalid Session feed event"
                )
            }
            guard let cursor = sessionFeedCursor,
                  cursor.epoch == event.epoch,
                  event.revision == cursor.revision + 1
            else {
                if let cursor = sessionFeedCursor,
                   cursor.epoch == event.epoch,
                   event.revision <= cursor.revision
                {
                    return
                }
                try await send(.subscribeSessionFeed(
                    knownEpoch: sessionFeedCursor?.epoch,
                    afterRevision: sessionFeedCursor?.revision
                ))
                return
            }
            sessionFeedCursor = SessionFeedCursor(
                epoch: event.epoch,
                revision: event.revision
            )
            if let sessionFeedHandler {
                await sessionFeedHandler(.event(event))
            }
        case .ping(let nonce):
            try await send(.pong(nonce: nonce))
        case .frontendExecute(
            let dispatchID,
            let executionTarget,
            let revision,
            let action,
            let attachPlan,
            let snapshot
        ):
            let dispatch = PolarTmuxFrontendDispatch(
                dispatchID: dispatchID,
                executionTarget: executionTarget,
                revision: revision,
                action: action,
                attachPlan: attachPlan,
                snapshot: snapshot
            )
            guard inFlightDispatchIDs.insert(dispatchID).inserted else {
                try await send(.frontendAck(
                    dispatchID: dispatchID,
                    result: .error(.init(
                        code: "duplicate_dispatch",
                        message: "Frontend action is already in flight",
                        retryable: false
                    ))
                ))
                return
            }
            guard let frontendHandler else {
                inFlightDispatchIDs.remove(dispatchID)
                try await send(.frontendAck(
                    dispatchID: dispatchID,
                    result: .error(.init(
                        code: "frontend_unavailable",
                        message: "Frontend action handler is unavailable",
                        retryable: true
                    ))
                ))
                return
            }
            Task { [weak self] in
                let result = await frontendHandler(dispatch)
                await self?.completeFrontendDispatch(
                    dispatchID: dispatchID,
                    result: result
                )
            }
        case .workspaceRelay(let dispatchID, let request):
            guard inFlightWorkspaceRelayIDs.insert(dispatchID).inserted else {
                try await send(.workspaceRelayCompleted(
                    dispatchID: dispatchID,
                    response: Self.workspaceRelayError(
                        request: request,
                        code: "duplicate_dispatch",
                        message: "Workspace relay is already in flight",
                        retryable: false
                    )
                ))
                return
            }
            guard let workspaceRelayHandler else {
                inFlightWorkspaceRelayIDs.remove(dispatchID)
                try await send(.workspaceRelayCompleted(
                    dispatchID: dispatchID,
                    response: Self.workspaceRelayError(
                        request: request,
                        code: "desktop_workspace_unavailable",
                        message: "Native Workspace handler is unavailable",
                        retryable: true
                    )
                ))
                return
            }
            Task { [weak self] in
                let response = await workspaceRelayHandler(request)
                await self?.completeWorkspaceRelay(
                    dispatchID: dispatchID,
                    response: response
                )
            }
        }
    }

    private func completeFrontendDispatch(
        dispatchID: String,
        result: PolarTmuxFrontendAcknowledgement
    ) async {
        guard inFlightDispatchIDs.remove(dispatchID) != nil else {
            return
        }
        do {
            try await send(.frontendAck(
                dispatchID: dispatchID,
                result: result
            ))
        } catch {
            await handleConnectionFailure(
                error,
                generation: readPumpID
            )
        }
    }

    private func completeWorkspaceRelay(
        dispatchID: String,
        response: PolarTmuxJSONValue
    ) async {
        guard inFlightWorkspaceRelayIDs.remove(dispatchID) != nil else {
            return
        }
        do {
            try await send(.workspaceRelayCompleted(
                dispatchID: dispatchID,
                response: response
            ))
        } catch {
            await handleConnectionFailure(
                error,
                generation: readPumpID
            )
        }
    }

    private static func workspaceRelayError(
        request: PolarTmuxJSONValue,
        code: String,
        message: String,
        retryable: Bool
    ) -> PolarTmuxJSONValue {
        guard case .object(let fields) = request,
              case .string(let requestID) = fields["request_id"],
              let scope = fields["scope"]
        else {
            return .object([:])
        }
        return .object([
            "protocol_version": .number(1),
            "api": .string("workspace_v1"),
            "request_id": .string(requestID),
            "scope": scope,
            "revision": .number(0),
            "result": .object([
                "type": .string("error"),
                "error": .object([
                    "code": .string(code),
                    "message": .string(message),
                    "retryable": .bool(retryable),
                ]),
            ]),
        ])
    }

    private func send(_ frame: PolarTmuxLocalClientFrame) async throws {
        guard let transport else {
            throw PolarTmuxLocalIPCError.connectionClosed
        }
        try await transport.send(JSONEncoder().encode(frame))
    }

    private static func decodingFailure(
        _ error: DecodingError,
        payload: Data
    ) -> PolarTmuxLocalIPCError {
        let codingPath: [any CodingKey]
        switch error {
        case .dataCorrupted(let context),
             .keyNotFound(_, let context),
             .typeMismatch(_, let context),
             .valueNotFound(_, let context):
            codingPath = context.codingPath
        @unknown default:
            codingPath = []
        }
        let path = codingPath.isEmpty
            ? "$"
            : codingPath.map(\.stringValue).joined(separator: ".")
        return .decodingFailed(
            frameType: safeFrameType(payload),
            codingPath: path
        )
    }

    private static func safeFrameType(_ payload: Data) -> String? {
        guard
            let object = try? JSONSerialization.jsonObject(with: payload),
            let type = (object as? [String: Any])?["type"] as? String,
            (1...64).contains(type.utf8.count),
            type.utf8.allSatisfy({
                ($0 >= 97 && $0 <= 122)
                    || ($0 >= 48 && $0 <= 57)
                    || $0 == 95
            })
        else {
            return nil
        }
        return type
    }

    private func failRegistration(_ error: any Error) {
        let waiters = registrationWaiters
        registrationWaiters.removeAll()
        waiters.forEach { $0.resume(throwing: error) }
    }

    private func awaitResponse(id: String, frame: PolarTmuxLocalClientFrame) async throws -> PolarTmuxLocalHostFrame {
        try Task.checkCancellation()
        try await ensureConnected()
        try Task.checkCancellation()
        guard responseWaiters[id] == nil else { throw PolarTmuxLocalIPCError.duplicateRequestID(id) }
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                responseWaiters[id] = ResponseBinding(generation: desktopGeneration, complete: { result in
                    switch result {
                    case .success(.host(let frame)): continuation.resume(returning: frame)
                    case .failure(let error): continuation.resume(throwing: error)
                    default: continuation.resume(throwing: PolarTmuxLocalIPCError.unexpectedFrame("control response"))
                    }
                })
                Task { [weak self] in
                    guard let self else { return }
                    if await self.bindingCancelled(id) {
                        await self.finishResponse(id, result: .failure(CancellationError())); return
                    }
                    do {
                        try await self.send(frame)
                        if await self.bindingCancelled(id) { await self.cancelResponse(id) }
                    }
                    catch { await self.finishResponse(id, result: .failure(error)) }
                }
            }
        } onCancel: {
            Task { [weak self] in await self?.cancelResponse(id) }
        }
    }

    private func finishResponse(_ id: String, result: Result<PolarTmuxLocalHostFrame, any Error>) {
        responseWaiters.removeValue(forKey: id)?.complete(result.map(ResponsePayload.host))
    }

    private func bindingCancelled(_ id: String) -> Bool { responseWaiters[id]?.cancelled ?? true }
    private func cancelResponse(_ id: String) async {
        guard let binding = responseWaiters[id] else { return }
        responseWaiters[id]?.cancelled = true
        if let generation = binding.generation { await transport?.cancelRequest(id: id, generation: generation) }
    }

    private func handleConnectionFailure(
        _ error: any Error,
        generation: UInt64
    ) async {
        guard generation == readPumpID else { return }
        failRegistration(error)
        failPendingRequests(with: error, controlOnly: true)
        await transport?.retryNow()
    }

    private func enqueueConnectionState(
        _ state: PolarTmuxLocalIPCConnectionState
    ) {
        guard let connectionStateHandler else { return }
        let previous = connectionStateDeliveryTail
        connectionStateDeliveryTail = Task {
            await previous?.value
            guard !Task.isCancelled else { return }
            await connectionStateHandler(state)
        }
    }

    private func failAllWaiters(with error: any Error) {
        failRegistration(error)
        failPendingRequests(with: error)
    }

    private func failPendingRequests(with error: any Error, controlOnly: Bool = false) {
        let ids = responseWaiters.keys.filter { !controlOnly || responseWaiters[$0]?.generation != nil }
        for id in ids { responseWaiters.removeValue(forKey: id)?.complete(.failure(error)) }
        inFlightDispatchIDs.removeAll()
        inFlightWorkspaceRelayIDs.removeAll()
    }

}

extension PolarTmuxHostClient: PolarTmuxAttachPlanProviding {
    public func prepareAttach(
        executionTarget: String,
        sessionID: String,
        expectedRevision: UInt64?
    ) async throws -> PolarTmuxPreparedAttach {
        let response = try await request(.init(
            requestID: UUID().uuidString,
            executionTarget: executionTarget,
            expectedRevision: expectedRevision,
            command: .object([
                "type": .string("prepare_attach"),
                "session": .string(sessionID),
            ])
        ))
        return .init(
            plan: try response.decodeAttachPlan(),
            executionTarget: response.executionTarget,
            revision: response.revision
        )
    }
}
