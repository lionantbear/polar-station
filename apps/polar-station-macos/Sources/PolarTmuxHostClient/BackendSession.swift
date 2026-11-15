import Foundation

public enum PolarStationBackendAvailability: Equatable, Sendable {
    case starting
    case ready(generation: UInt64)
    case reconnecting(message: String, attempt: Int)
    case unavailable(message: String)

    public var failureMessage: String? {
        switch self {
        case .starting, .ready:
            nil
        case .reconnecting(let message, _), .unavailable(let message):
            message
        }
    }

    public var hostActionsEnabled: Bool {
        if case .ready = self { return true }
        return false
    }
}

public enum PolarStationBackendMutationResult<Response: Sendable>: Sendable {
    case accepted(Response)
    case rejected(String)
    case outcomeUnknown(String)
}

/// Thin desktop session boundary around the authenticated local Host client.
///
/// Presentation code submits intent and observes typed state. This actor owns
/// a projection of Rust connection availability. The shared Rust desktop client
/// owns mutation ordering, connection generations and reconnect backoff; this
/// actor never owns product projections or native window lifetime.
public actor PolarStationBackendSession {
    public typealias AvailabilityHandler =
        @Sendable (PolarStationBackendAvailability) async -> Void

    private struct Registration: Equatable, Sendable {
        let frontendID: String
        let supportedActions: [String]
    }

    private let client: PolarTmuxHostClient
    private var registration: Registration?
    private var availability: PolarStationBackendAvailability = .starting
    private var lastFailureMessage: String?
    private var availabilityHandler: AvailabilityHandler?
    private var connectionStateHandlerInstalled = false
    private var startTask: Task<Void, any Error>?

    public init(client: PolarTmuxHostClient = PolarTmuxHostClient()) {
        self.client = client
    }

    public func setExtensionInputHandler(_ handler: @escaping PolarTmuxHostClient.ExtensionInputHandler) async {
        await client.setExtensionInputHandler(handler)
    }
    public func sendStationSession(_ payload: Data) async throws { try await client.sendStationSession(payload) }
    public func sendExtensionFrame(_ payload: Data) async throws { try await client.sendExtensionFrame(payload) }

    public func sendPaneRecovery(socketURL: URL, request: PolarTmuxPaneContentRequest) async throws {
        try await client.sendPaneRecovery(socketURL: socketURL, request: request)
    }

    public func requestPaneContent(socketURL: URL, request: PolarTmuxPaneContentRequest) async throws -> PolarTmuxPaneContentResponse {
        try await client.requestPaneContent(socketURL: socketURL, request: request)
    }

    public func setAvailabilityHandler(
        _ handler: @escaping AvailabilityHandler
    ) async {
        availabilityHandler = handler
        await handler(availability)
    }

    public func setApplicationStateHandler(
        _ handler: @escaping PolarTmuxHostClient.ApplicationStateHandler
    ) async {
        await client.setApplicationStateHandler(handler)
    }

    public func setSessionFeedHandler(
        _ handler: @escaping PolarTmuxHostClient.SessionFeedHandler
    ) async {
        await client.setSessionFeedHandler(handler)
    }

    public func setWorkspaceRelayHandler(
        _ handler: @escaping PolarTmuxHostClient.WorkspaceRelayHandler
    ) async {
        await client.setWorkspaceRelayHandler(handler)
    }

    public func setFrontendHandler(
        _ handler: @escaping PolarTmuxHostClient.FrontendHandler
    ) async {
        await client.setFrontendHandler(handler)
    }

    public func start(
        frontendID: String,
        supportedActions: [String]
    ) async throws {
        let nextRegistration = Registration(
            frontendID: frontendID,
            supportedActions: supportedActions
        )
        if registration == nextRegistration,
           case .ready = availability
        {
            return
        }
        if let startTask {
            try await startTask.value
            return
        }
        registration = nextRegistration
        if !connectionStateHandlerInstalled {
            await client.setConnectionStateHandler { [weak self] state in
                await self?.clientConnectionStateChanged(state)
            }
            connectionStateHandlerInstalled = true
        }
        if lastFailureMessage == nil {
            await publish(.starting)
        }
        let task = Task { [client] in
            _ = try await client.register(
                frontendID: frontendID,
                supportedActions: supportedActions
            )
        }
        startTask = task
        do {
            try await task.value
            startTask = nil
        } catch {
            startTask = nil
            throw error
        }
    }

    public func retryNow() async {
        guard registration != nil else { return }
        await client.retryNow()
    }

    public func stop() async {
        await client.disconnect()
        await publish(.unavailable(
            message: "Polar Station backend session stopped"
        ))
    }

    public func requestControlCenter(
        _ request: ControlCenterRequestEnvelopeV1
    ) async throws -> ControlCenterResponseEnvelopeV1 {
        do {
            return try await client.requestControlCenter(request)
        } catch {
            throw error
        }
    }

    public func requestResources(
        _ request: ResourcesRequestEnvelopeV1
    ) async throws -> ResourcesResponseEnvelopeV1 {
        do {
            return try await client.requestResources(request)
        } catch {
            throw error
        }
    }

    public func requestSettings(
        _ request: SettingsRequestEnvelopeV1
    ) async throws -> SettingsResponseEnvelopeV1 {
        do { return try await client.requestSettings(request) }
        catch { throw error }
    }

    public func requestNotification(
        _ request: NotificationRequestEnvelopeV1
    ) async throws -> NotificationResponseEnvelopeV1 {
        try await client.requestNotification(request)
    }

    public func requestCodex(ownerHostID: String?, command: CodexCommandV1) async throws -> CodexResponseEnvelopeV1 {
        try await client.requestCodex(ownerHostID: ownerHostID, command: command)
    }

    public func requestCodex(
        _ request: CodexRequestEnvelopeV1
    ) async throws -> CodexResponseEnvelopeV1 {
        try await client.requestCodex(request)
    }

    public func requestInbox(
        _ request: InboxRequestEnvelopeV1
    ) async throws -> InboxResponseEnvelopeV1 {
        do { return try await client.requestInbox(request) }
        catch { throw error }
    }

    public func submitSettingsMutation(
        _ request: SettingsRequestEnvelopeV1
    ) async -> PolarStationBackendMutationResult<SettingsResponseEnvelopeV1> {
        do { return .accepted(try await client.requestSettings(request)) }
        catch { return mutationFailure(error) }
    }

    public func companionPairing()
        async throws -> CompanionPairingPayloadV1
    {
        do {
            return try await client.companionPairing()
        } catch {
            throw error
        }
    }

    public func submitControlCenterMutation(
        _ request: ControlCenterRequestEnvelopeV1
    ) async -> PolarStationBackendMutationResult<
        ControlCenterResponseEnvelopeV1
    > {
        await submitControlCenterMutation { request }
    }

    public func submitControlCenterMutation(
        buildRequest: @escaping @Sendable () async throws
            -> ControlCenterRequestEnvelopeV1
    ) async -> PolarStationBackendMutationResult<
        ControlCenterResponseEnvelopeV1
    > {
        do { return await performControlCenterMutation(try await buildRequest()) }
        catch { return .rejected(error.localizedDescription) }
    }

    public func submitResourcesMutation(
        _ request: ResourcesRequestEnvelopeV1
    ) async -> PolarStationBackendMutationResult<ResourcesResponseEnvelopeV1> {
        await performResourcesMutation(request)
    }

    private func performControlCenterMutation(
        _ request: ControlCenterRequestEnvelopeV1
    ) async -> PolarStationBackendMutationResult<
        ControlCenterResponseEnvelopeV1
    > {
        do {
            return .accepted(try await client.requestControlCenter(request))
        } catch {
            return mutationFailure(error)
        }
    }

    private func performResourcesMutation(
        _ request: ResourcesRequestEnvelopeV1
    ) async -> PolarStationBackendMutationResult<
        ResourcesResponseEnvelopeV1
    > {
        do {
            return .accepted(try await client.requestResources(request))
        } catch {
            return mutationFailure(error)
        }
    }

    private func mutationFailure<Response: Sendable>(_ error: any Error) -> PolarStationBackendMutationResult<Response> {
        if error is PolarTmuxDesktopSendError || error is CancellationError {
            return .rejected(error.localizedDescription)
        }
        if let failure = error as? DesktopRequestFailure, !failure.outcome_unknown {
            return .rejected(failure.message)
        }
        return .outcomeUnknown("The Host did not confirm this action. Its outcome is unknown; refresh before retrying. \(error.localizedDescription)")
    }

    private func clientConnectionStateChanged(
        _ state: PolarTmuxLocalIPCConnectionState
    ) async {
        switch state {
        case .connecting(_, let attempt):
            await publish(.reconnecting(
                message: lastFailureMessage
                    ?? "Connecting to the Polar Station backend",
                attempt: attempt
            ))
        case .ready(let generation):
            await publish(.ready(generation: generation))
        case .unavailable(_, _, let message, _):
            await publish(.unavailable(message: message))
        }
    }

    private func publish(
        _ next: PolarStationBackendAvailability
    ) async {
        switch next {
        case .unavailable(let message):
            lastFailureMessage = message
        case .ready:
            lastFailureMessage = nil
        case .starting, .reconnecting:
            break
        }
        availability = next
        if let availabilityHandler {
            await availabilityHandler(next)
        }
    }
}
