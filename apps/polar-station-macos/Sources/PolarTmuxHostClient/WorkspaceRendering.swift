import Foundation

public let polarTmuxWorkspaceRenderingCapability =
    "workspace_rendering"

public struct PolarTmuxRenderRatio: Codable, Equatable, Sendable {
    public let numerator: UInt32
    public let denominator: UInt32

    public init(numerator: UInt32, denominator: UInt32) {
        self.numerator = numerator
        self.denominator = denominator
    }
}

public enum PolarTmuxRenderLayoutRole:
    Codable,
    Equatable,
    Sendable
{
    case monitor
    case main
    case sidebar(slot: UInt8)

    private enum CodingKeys: String, CodingKey {
        case type
        case slot
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .type) {
        case "monitor":
            self = .monitor
        case "main":
            self = .main
        case "sidebar":
            self = try .sidebar(
                slot: container.decode(UInt8.self, forKey: .slot)
            )
        case let type:
            throw DecodingError.dataCorruptedError(
                forKey: .type,
                in: container,
                debugDescription: "unsupported render role \(type)"
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .monitor:
            try container.encode("monitor", forKey: .type)
        case .main:
            try container.encode("main", forKey: .type)
        case .sidebar(let slot):
            try container.encode("sidebar", forKey: .type)
            try container.encode(slot, forKey: .slot)
        }
    }
}

public struct PolarTmuxRenderLayoutSlot:
    Codable,
    Equatable,
    Sendable
{
    public let paneID: String
    public let canonicalIndex: Int
    public let visualIndex: Int
    public let column: Int
    public let row: Int
    public let width: PolarTmuxRenderRatio
    public let height: PolarTmuxRenderRatio
    public let role: PolarTmuxRenderLayoutRole

    public init(
        paneID: String,
        canonicalIndex: Int,
        visualIndex: Int,
        column: Int,
        row: Int,
        width: PolarTmuxRenderRatio,
        height: PolarTmuxRenderRatio,
        role: PolarTmuxRenderLayoutRole
    ) {
        self.paneID = paneID
        self.canonicalIndex = canonicalIndex
        self.visualIndex = visualIndex
        self.column = column
        self.row = row
        self.width = width
        self.height = height
        self.role = role
    }

    enum CodingKeys: String, CodingKey {
        case paneID = "pane_id"
        case canonicalIndex = "canonical_index"
        case visualIndex = "visual_index"
        case column
        case row
        case width
        case height
        case role
    }
}

public struct PolarTmuxRenderLayout:
    Codable,
    Equatable,
    Sendable
{
    public let paneOrder: [String]
    public let slots: [PolarTmuxRenderLayoutSlot]
    public let fingerprint: String

    public init(
        paneOrder: [String],
        slots: [PolarTmuxRenderLayoutSlot],
        fingerprint: String
    ) {
        self.paneOrder = paneOrder
        self.slots = slots
        self.fingerprint = fingerprint
    }

    enum CodingKeys: String, CodingKey {
        case paneOrder = "pane_order"
        case slots
        case fingerprint
    }
}

public struct PolarTmuxWorkspaceRenderPane:
    Codable,
    Equatable,
    Sendable
{
    public let id: String
    public let executionTargetID: String
    public let tmuxSessionID: String
    public let transportStatus: String
    public let attachFingerprint: String
    public let attachPlan: PolarTmuxAttachPlan

    public init(
        id: String,
        executionTargetID: String,
        tmuxSessionID: String,
        transportStatus: String,
        attachFingerprint: String,
        attachPlan: PolarTmuxAttachPlan
    ) {
        self.id = id
        self.executionTargetID = executionTargetID
        self.tmuxSessionID = tmuxSessionID
        self.transportStatus = transportStatus
        self.attachFingerprint = attachFingerprint
        self.attachPlan = attachPlan
    }

    enum CodingKeys: String, CodingKey {
        case id
        case executionTargetID = "execution_target_id"
        case tmuxSessionID = "tmux_session_id"
        case transportStatus = "transport_status"
        case attachFingerprint = "attach_fingerprint"
        case attachPlan = "attach_plan"
    }
}

public struct PolarTmuxWorkspaceRenderTab:
    Codable,
    Equatable,
    Sendable
{
    public let id: String
    public let title: String
    public let displayMode: String
    public let focusedPaneID: String?
    public let layout: PolarTmuxRenderLayout
    public let panes: [PolarTmuxWorkspaceRenderPane]

    public init(
        id: String,
        title: String,
        displayMode: String,
        focusedPaneID: String?,
        layout: PolarTmuxRenderLayout,
        panes: [PolarTmuxWorkspaceRenderPane]
    ) {
        self.id = id
        self.title = title
        self.displayMode = displayMode
        self.focusedPaneID = focusedPaneID
        self.layout = layout
        self.panes = panes
    }

    enum CodingKeys: String, CodingKey {
        case id
        case title
        case displayMode = "display_mode"
        case focusedPaneID = "focused_pane_id"
        case layout
        case panes
    }
}

public struct PolarTmuxWorkspaceRenderSnapshot:
    Codable,
    Equatable,
    Sendable
{
    public let schemaVersion: UInt32
    public let workspaceID: String
    public let workspaceName: String
    public let workspaceRevision: UInt64
    public let managedTerminalFontSizePoints: UInt8
    public let selectedTabID: String
    public let tabs: [PolarTmuxWorkspaceRenderTab]
    public let snapshotFingerprint: String
    public let error: PolarTmuxProtocolError?

    public init(
        schemaVersion: UInt32,
        workspaceID: String,
        workspaceName: String,
        workspaceRevision: UInt64,
        managedTerminalFontSizePoints: UInt8,
        selectedTabID: String,
        tabs: [PolarTmuxWorkspaceRenderTab],
        snapshotFingerprint: String,
        error: PolarTmuxProtocolError?
    ) {
        self.schemaVersion = schemaVersion
        self.workspaceID = workspaceID
        self.workspaceName = workspaceName
        self.workspaceRevision = workspaceRevision
        self.managedTerminalFontSizePoints =
            managedTerminalFontSizePoints
        self.selectedTabID = selectedTabID
        self.tabs = tabs
        self.snapshotFingerprint = snapshotFingerprint
        self.error = error
    }

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case workspaceID = "workspace_id"
        case workspaceName = "workspace_name"
        case workspaceRevision = "workspace_revision"
        case managedTerminalFontSizePoints =
            "managed_terminal_font_size_points"
        case selectedTabID = "selected_tab_id"
        case tabs
        case snapshotFingerprint = "snapshot_fingerprint"
        case error
    }
}

public struct PolarTmuxWorkspaceRenderRequest:
    Equatable,
    Sendable
{
    public let transactionID: String
    public let frontendInstanceID: String
    public let frontendGeneration: UInt64
    public let snapshot: PolarTmuxWorkspaceRenderSnapshot

    public init(
        transactionID: String,
        frontendInstanceID: String,
        frontendGeneration: UInt64,
        snapshot: PolarTmuxWorkspaceRenderSnapshot
    ) {
        self.transactionID = transactionID
        self.frontendInstanceID = frontendInstanceID
        self.frontendGeneration = frontendGeneration
        self.snapshot = snapshot
    }
}

public enum PolarTmuxRenderResult:
    Encodable,
    Equatable,
    Sendable
{
    case applied
    case rejected(PolarTmuxProtocolError)

    private enum CodingKeys: String, CodingKey {
        case status
        case error
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .applied:
            try container.encode("applied", forKey: .status)
        case .rejected(let error):
            try container.encode("rejected", forKey: .status)
            try container.encode(error, forKey: .error)
        }
    }
}

public struct PolarTmuxRenderReceipt:
    Encodable,
    Equatable,
    Sendable
{
    public let transactionID: String
    public let frontendInstanceID: String
    public let frontendGeneration: UInt64
    public let workspaceID: String
    public let workspaceRevision: UInt64
    public let snapshotFingerprint: String
    public let result: PolarTmuxRenderResult

    public init(
        transactionID: String,
        frontendInstanceID: String,
        frontendGeneration: UInt64,
        workspaceID: String,
        workspaceRevision: UInt64,
        snapshotFingerprint: String,
        result: PolarTmuxRenderResult
    ) {
        self.transactionID = transactionID
        self.frontendInstanceID = frontendInstanceID
        self.frontendGeneration = frontendGeneration
        self.workspaceID = workspaceID
        self.workspaceRevision = workspaceRevision
        self.snapshotFingerprint = snapshotFingerprint
        self.result = result
    }

    enum CodingKeys: String, CodingKey {
        case transactionID = "transaction_id"
        case frontendInstanceID = "frontend_instance_id"
        case frontendGeneration = "frontend_generation"
        case workspaceID = "workspace_id"
        case workspaceRevision = "workspace_revision"
        case snapshotFingerprint = "snapshot_fingerprint"
        case result
    }
}

public enum PolarTmuxLocalClientFrame:
    Encodable,
    Equatable,
    Sendable
{
    case register(
        frontendID: String,
        platform: String,
        supportedActions: [String],
        capabilities: [String],
        frontendInstanceID: String?,
        frontendGeneration: UInt64
    )
    case request(PolarTmuxClientEnvelope)
    case applicationRequest(ControlCenterRequestEnvelopeV1)
    case resourcesRequest(ResourcesRequestEnvelopeV1)
    case notificationRequest(NotificationRequestEnvelopeV1)
    case codexRequest(CodexRequestEnvelopeV1)
    case settingsRequest(SettingsRequestEnvelopeV1)
    case inboxRequest(InboxRequestEnvelopeV1)
    case subscribeSessionFeed(
        knownEpoch: String?,
        afterRevision: UInt64?
    )
    case frontendAck(
        dispatchID: String,
        result: PolarTmuxFrontendAcknowledgement
    )
    case workspaceRelayCompleted(
        dispatchID: String,
        response: PolarTmuxJSONValue
    )
    case pong(nonce: UInt64)

    private enum CodingKeys: String, CodingKey {
        case type
        case frontendID = "frontend_id"
        case platform
        case supportedActions = "supported_actions"
        case capabilities
        case frontendInstanceID = "frontend_instance_id"
        case frontendGeneration = "frontend_generation"
        case hostControlVersions = "host_control_versions"
        case extensions
        case request
        case knownEpoch = "known_epoch"
        case afterRevision = "after_revision"
        case dispatchID = "dispatch_id"
        case result
        case response
        case nonce
        case snapshot
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .register(
            let frontendID,
            let platform,
            let supportedActions,
            let capabilities,
            let frontendInstanceID,
            let frontendGeneration
        ):
            try container.encode("register", forKey: .type)
            try container.encode(["notification": 1, "codex": 2], forKey: .hostControlVersions)
            try container.encode([StationExtensionOffer()], forKey: .extensions)
            try container.encode(frontendID, forKey: .frontendID)
            try container.encode(platform, forKey: .platform)
            try container.encode(supportedActions, forKey: .supportedActions)
            try container.encode(capabilities, forKey: .capabilities)
            try container.encodeIfPresent(
                frontendInstanceID,
                forKey: .frontendInstanceID
            )
            if frontendGeneration > 0 {
                try container.encode(
                    frontendGeneration,
                    forKey: .frontendGeneration
                )
            }
        case .request(let request):
            try container.encode("request", forKey: .type)
            try container.encode(request, forKey: .request)
        case .applicationRequest(let request):
            try container.encode("application_request", forKey: .type)
            try container.encode(request, forKey: .request)
        case .resourcesRequest(let request):
            try container.encode("application_request", forKey: .type)
            try container.encode(request, forKey: .request)
        case .notificationRequest(let request):
            try container.encode("application_request", forKey: .type)
            try container.encode(request, forKey: .request)
        case .codexRequest(let request):
            try container.encode("application_request", forKey: .type)
            try container.encode(request, forKey: .request)
        case .settingsRequest(let request):
            try container.encode("application_request", forKey: .type)
            try container.encode(request, forKey: .request)
        case .inboxRequest(let request):
            try container.encode("application_request", forKey: .type)
            try container.encode(request, forKey: .request)
        case .subscribeSessionFeed(let knownEpoch, let afterRevision):
            try container.encode("subscribe_session_feed", forKey: .type)
            try container.encodeIfPresent(knownEpoch, forKey: .knownEpoch)
            try container.encodeIfPresent(afterRevision, forKey: .afterRevision)
        case .frontendAck(let dispatchID, let result):
            try container.encode("frontend_ack", forKey: .type)
            try container.encode(dispatchID, forKey: .dispatchID)
            try container.encode(result, forKey: .result)
        case .workspaceRelayCompleted(let dispatchID, let response):
            try container.encode(
                "workspace_relay_completed",
                forKey: .type
            )
            try container.encode(dispatchID, forKey: .dispatchID)
            try container.encode(response, forKey: .response)
        case .pong(let nonce):
            try container.encode("pong", forKey: .type)
            try container.encode(nonce, forKey: .nonce)
        }
    }
}

public enum PolarTmuxLocalHostFrame:
    Decodable,
    Equatable,
    Sendable
{
    case registered(
        registrationID: String,
        protocolVersion: UInt32,
        hostID: String,
        hostControlVersions: HostControlVersionsV1? = nil
    )
    case response(PolarTmuxHostEnvelope)
    case applicationResponse(ControlCenterResponseEnvelopeV1)
    case resourcesResponse(ResourcesResponseEnvelopeV1)
    case notificationResponse(NotificationResponseEnvelopeV1)
    case codexResponse(CodexResponseEnvelopeV1)
    case settingsResponse(SettingsResponseEnvelopeV1)
    case inboxResponse(InboxResponseEnvelopeV1)
    case applicationStateChanged(
        api: PolarTmuxApplicationStateAPI,
        stateRevision: UInt64
    )
    case sessionFeedSnapshot(PolarTmuxSessionFeedSnapshot)
    case sessionFeedEvent(PolarTmuxSessionFeedEvent)
    case frontendExecute(
        dispatchID: String,
        executionTarget: String,
        revision: UInt64,
        action: PolarTmuxFrontendAction,
        attachPlan: PolarTmuxAttachPlan?,
        snapshot: PolarTmuxSessionSnapshot?
    )
    case workspaceRelay(
        dispatchID: String,
        request: PolarTmuxJSONValue
    )
    case ping(nonce: UInt64)

    private enum CodingKeys: String, CodingKey {
        case type
        case registrationID = "registration_id"
        case protocolVersion = "protocol_version"
        case hostID = "host_id"
        case hostControlVersions = "host_control_versions"
        case response
        case api
        case stateRevision = "state_revision"
        case snapshot
        case event
        case dispatchID = "dispatch_id"
        case executionTarget = "execution_target"
        case revision
        case action
        case attachPlan = "attach_plan"
        case request
        case nonce
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .type) {
        case "registered":
            self = try .registered(
                registrationID: container.decode(
                    String.self,
                    forKey: .registrationID
                ),
                protocolVersion: container.decode(
                    UInt32.self,
                    forKey: .protocolVersion
                ),
                hostID: container.decode(String.self, forKey: .hostID),
                hostControlVersions: container.decodeIfPresent(HostControlVersionsV1.self, forKey: .hostControlVersions)
            )
        case "response":
            self = try .response(
                container.decode(
                    PolarTmuxHostEnvelope.self,
                    forKey: .response
                )
            )
        case "application_response":
            let discriminator = try container.decode(
                PolarTmuxCurrentApplicationDiscriminator.self,
                forKey: .response
            )
            switch discriminator.api {
            case "control_center_v1":
                self = try .applicationResponse(
                    container.decode(
                        ControlCenterResponseEnvelopeV1.self,
                        forKey: .response
                    )
                )
            case "resources_v1":
                self = try .resourcesResponse(
                    container.decode(
                        ResourcesResponseEnvelopeV1.self,
                        forKey: .response
                    )
                )
            case "notification_v1":
                self = try .notificationResponse(container.decode(NotificationResponseEnvelopeV1.self, forKey: .response))
            case "codex_v1":
                self = try .codexResponse(container.decode(CodexResponseEnvelopeV1.self, forKey: .response))
            case "settings_v1":
                self = try .settingsResponse(
                    container.decode(
                        SettingsResponseEnvelopeV1.self,
                        forKey: .response
                    )
                )
            case "inbox_v1":
                self = try .inboxResponse(
                    container.decode(
                        InboxResponseEnvelopeV1.self,
                        forKey: .response
                    )
                )
            default:
                throw DecodingError.dataCorruptedError(
                    forKey: .response,
                    in: container,
                    debugDescription:
                        "unknown Host application response API"
                )
            }
        case "application_state_changed":
            let revision = try container.decode(
                UInt64.self,
                forKey: .stateRevision
            )
            guard revision > 0 else {
                throw DecodingError.dataCorruptedError(
                    forKey: .stateRevision,
                    in: container,
                    debugDescription:
                        "application state revision must be non-zero"
                )
            }
            self = try .applicationStateChanged(
                api: container.decode(
                    PolarTmuxApplicationStateAPI.self,
                    forKey: .api
                ),
                stateRevision: revision
            )
        case "session_feed_snapshot":
            self = try .sessionFeedSnapshot(container.decode(
                PolarTmuxSessionFeedSnapshot.self,
                forKey: .snapshot
            ))
        case "session_feed_event":
            self = try .sessionFeedEvent(container.decode(
                PolarTmuxSessionFeedEvent.self,
                forKey: .event
            ))
        case "frontend_execute":
            guard container.contains(.attachPlan),
                  container.contains(.snapshot)
            else {
                throw DecodingError.dataCorruptedError(
                    forKey: container.contains(.attachPlan)
                        ? .snapshot : .attachPlan,
                    in: container,
                    debugDescription:
                        "frontend_execute must carry explicit attach_plan and snapshot fields"
                )
            }
            let action = try container.decode(
                PolarTmuxFrontendAction.self,
                forKey: .action
            )
            let attachPlan = try container.decodeIfPresent(
                PolarTmuxAttachPlan.self,
                forKey: .attachPlan
            )
            let rawSnapshot = try container.decodeIfPresent(
                PolarTmuxJSONValue.self,
                forKey: .snapshot
            )
            if let rawSnapshot,
               !isPrivacyPreservingFrontendSnapshot(
                    rawSnapshot
               )
            {
                throw DecodingError.dataCorruptedError(
                    forKey: .snapshot,
                    in: container,
                    debugDescription:
                        "frontend snapshot contains non-mobile fields"
                )
            }
            let snapshot = try container.decodeIfPresent(
                PolarTmuxSessionSnapshot.self,
                forKey: .snapshot
            )
            switch (action, attachPlan, snapshot) {
            case (
                .openSession(let session),
                .some(let plan),
                .none
            ):
                guard try plan.attachedSessionID() == session
                else {
                    throw DecodingError.dataCorruptedError(
                        forKey: .attachPlan,
                        in: container,
                        debugDescription:
                            "open_session attach plan targets another session"
                    )
                }
            case (.openSession, .none, .none):
                throw DecodingError.dataCorruptedError(
                    forKey: .attachPlan,
                    in: container,
                    debugDescription:
                        "open_session requires a Host attach plan"
                )
            case (.refresh, .none, .some):
                break
            case (.refresh, _, .none):
                throw DecodingError.dataCorruptedError(
                    forKey: .snapshot,
                    in: container,
                    debugDescription:
                        "refresh requires a privacy-preserving snapshot"
                )
            case (_, .some, _):
                throw DecodingError.dataCorruptedError(
                    forKey: .attachPlan,
                    in: container,
                    debugDescription:
                        "only open_session may carry an attach plan"
                )
            case (_, .none, .some):
                throw DecodingError.dataCorruptedError(
                    forKey: .snapshot,
                    in: container,
                    debugDescription:
                        "only refresh may carry a snapshot"
                )
            case (_, .none, .none):
                break
            }
            self = try .frontendExecute(
                dispatchID: container.decode(
                    String.self,
                    forKey: .dispatchID
                ),
                executionTarget: container.decode(
                    String.self,
                    forKey: .executionTarget
                ),
                revision: container.decode(
                    UInt64.self,
                    forKey: .revision
                ),
                action: action,
                attachPlan: attachPlan,
                snapshot: snapshot
            )
        case "workspace_relay":
            self = try .workspaceRelay(
                dispatchID: container.decode(
                    String.self,
                    forKey: .dispatchID
                ),
                request: container.decode(
                    PolarTmuxJSONValue.self,
                    forKey: .request
                )
            )
        case "ping":
            self = try .ping(
                nonce: container.decode(UInt64.self, forKey: .nonce)
            )
        case let type:
            throw DecodingError.dataCorruptedError(
                forKey: .type,
                in: container,
                debugDescription: "unsupported current Host frame \(type)"
            )
        }
    }
}

private struct PolarTmuxCurrentApplicationDiscriminator:
    Decodable
{
    let api: String
}

private struct StationExtensionOffer: Encodable {
    let name = "polar_access"
    let version: UInt16 = 1
    let max_payload_bytes: UInt32 = 65_536
}
