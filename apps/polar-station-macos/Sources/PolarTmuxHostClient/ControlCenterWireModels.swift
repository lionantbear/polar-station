import Foundation

public enum ActionRoleV1: String, Codable, Equatable, Sendable {
    case primary
    case secondary
    case toggle
    case recovery
    case destructive
}

public struct ConfirmationProjectionV1: Codable, Equatable, Sendable {
    public let id: String
    public let title: String
    public let message: String
    public let confirmLabel: String
    public let destructive: Bool

    public init(
        id: String,
        title: String,
        message: String,
        confirmLabel: String,
        destructive: Bool
    ) {
        self.id = id
        self.title = title
        self.message = message
        self.confirmLabel = confirmLabel
        self.destructive = destructive
    }

    enum CodingKeys: String, CodingKey {
        case id
        case title
        case message
        case confirmLabel = "confirm_label"
        case destructive
    }
}

public struct ActionProjectionV1: Codable, Equatable, Identifiable, Sendable {
    public let id: String
    public let label: String
    public let role: ActionRoleV1
    public let enabled: Bool
    public let selected: Bool
    public let disabledReason: String?
    public let confirmation: ConfirmationProjectionV1?
    public let accessibilityLabel: String

    public init(
        id: String,
        label: String,
        role: ActionRoleV1,
        enabled: Bool,
        selected: Bool,
        disabledReason: String?,
        confirmation: ConfirmationProjectionV1?,
        accessibilityLabel: String
    ) {
        self.id = id
        self.label = label
        self.role = role
        self.enabled = enabled
        self.selected = selected
        self.disabledReason = disabledReason
        self.confirmation = confirmation
        self.accessibilityLabel = accessibilityLabel
    }

    enum CodingKeys: String, CodingKey {
        case id
        case label
        case role
        case enabled
        case selected
        case disabledReason = "disabled_reason"
        case confirmation
        case accessibilityLabel = "accessibility_label"
    }
}

public struct WorkspaceChoiceProjectionV1:
    Codable, Equatable, Identifiable, Sendable
{
    public let id: String
    public let name: String
    public let selected: Bool

    public init(id: String, name: String, selected: Bool) {
        self.id = id
        self.name = name
        self.selected = selected
    }
}

public struct ConnectionStatusProjectionV1:
    Codable, Equatable, Sendable
{
    public let label: String
    public let route: ConnectionRouteV1?
    public let status: String
    public let accessibilityLabel: String

    public init(
        label: String,
        route: ConnectionRouteV1? = nil,
        status: String,
        accessibilityLabel: String
    ) {
        self.label = label
        self.route = route
        self.status = status
        self.accessibilityLabel = accessibilityLabel
    }

    enum CodingKeys: String, CodingKey {
        case label
        case route
        case status
        case accessibilityLabel = "accessibility_label"
    }
}

public enum ConnectionRouteV1: String, Codable, Equatable, Sendable {
    case lan
    case wan
}

public struct WorkspaceSSHTargetRouteProfileV1:
    Codable, Equatable, Sendable
{
    public let lanAddress: String
    public let lanPort: UInt16
    public let wanHost: String
    public let wanPort: UInt16
    public let dohProviders: [String]

    public init(
        lanAddress: String,
        lanPort: UInt16,
        wanHost: String,
        wanPort: UInt16,
        dohProviders: [String]
    ) {
        self.lanAddress = lanAddress
        self.lanPort = lanPort
        self.wanHost = wanHost
        self.wanPort = wanPort
        self.dohProviders = dohProviders
    }

    enum CodingKeys: String, CodingKey {
        case lanAddress = "lan_address"
        case lanPort = "lan_port"
        case wanHost = "wan_host"
        case wanPort = "wan_port"
        case dohProviders = "doh_providers"
    }
}

public struct ControlHeaderProjectionV1: Codable, Equatable, Sendable {
    public let connection: ConnectionStatusProjectionV1?
    public let sshTargetEditor: WorkspaceSSHTargetEditorProjectionV1
    public let renameAction: ActionProjectionV1?
    public let addTab: ActionProjectionV1
    public let refresh: ActionProjectionV1
    public let workspaceMenuDetachAction: ActionProjectionV1?
    public let lifecycleAction: ActionProjectionV1?

    public init(
        connection: ConnectionStatusProjectionV1?,
        sshTargetEditor: WorkspaceSSHTargetEditorProjectionV1,
        renameAction: ActionProjectionV1?,
        addTab: ActionProjectionV1,
        refresh: ActionProjectionV1,
        workspaceMenuDetachAction: ActionProjectionV1? = nil,
        lifecycleAction: ActionProjectionV1?
    ) {
        self.connection = connection
        self.sshTargetEditor = sshTargetEditor
        self.renameAction = renameAction
        self.addTab = addTab
        self.refresh = refresh
        self.workspaceMenuDetachAction = workspaceMenuDetachAction
        self.lifecycleAction = lifecycleAction
    }

    enum CodingKeys: String, CodingKey {
        case connection
        case sshTargetEditor = "ssh_target_editor"
        case renameAction = "rename_action"
        case addTab = "add_tab"
        case refresh
        case workspaceMenuDetachAction = "workspace_menu_detach_action"
        case lifecycleAction = "lifecycle_action"
    }
}

public struct SessionAssignmentActionsV1: Codable, Equatable, Sendable {
    public let add: ActionProjectionV1
    public let remove: ActionProjectionV1
    public let tabFull: ActionProjectionV1
    public let assignedElsewhere: ActionProjectionV1
    public let noManagedTab: ActionProjectionV1

    public init(
        add: ActionProjectionV1,
        remove: ActionProjectionV1,
        tabFull: ActionProjectionV1,
        assignedElsewhere: ActionProjectionV1,
        noManagedTab: ActionProjectionV1
    ) {
        self.add = add
        self.remove = remove
        self.tabFull = tabFull
        self.assignedElsewhere = assignedElsewhere
        self.noManagedTab = noManagedTab
    }

    enum CodingKeys: String, CodingKey {
        case add
        case remove
        case tabFull = "tab_full"
        case assignedElsewhere = "assigned_elsewhere"
        case noManagedTab = "no_managed_tab"
    }
}

public struct SessionReplacementV1: Codable, Equatable, Sendable {
    public let executionTargetID: String
    public let tmuxSessionID: String
    public let serverGeneration: String
    public let createdUnixSeconds: Int64
    public var identity: SessionRowProjectionV1.Identity {
        .init(executionTargetID: executionTargetID, tmuxSessionID: tmuxSessionID)
    }
    public init(executionTargetID: String, tmuxSessionID: String, serverGeneration: String, createdUnixSeconds: Int64) {
        self.executionTargetID = executionTargetID; self.tmuxSessionID = tmuxSessionID
        self.serverGeneration = serverGeneration; self.createdUnixSeconds = createdUnixSeconds
    }
    enum CodingKeys: String, CodingKey {
        case executionTargetID = "execution_target_id", tmuxSessionID = "tmux_session_id"
        case serverGeneration = "server_generation", createdUnixSeconds = "created_unix_seconds"
    }
}

public struct SessionReplacementSelectionV1: Codable, Equatable, Sendable {
    public let identity: SessionRowProjectionV1.Identity
    public let replacement: SessionReplacementV1
    public let workspaceID: String
    public let confirmationID: String
    public init(identity: SessionRowProjectionV1.Identity, replacement: SessionReplacementV1, workspaceID: String, confirmationID: String) {
        self.identity = identity; self.replacement = replacement
        self.workspaceID = workspaceID; self.confirmationID = confirmationID
    }
}

public struct SessionRowProjectionV1:
    Codable, Equatable, Identifiable, Sendable
{
    public struct Identity:
        Codable, Equatable, Hashable, Sendable
    {
        public let executionTargetID: String
        public let tmuxSessionID: String

        public init(
            executionTargetID: String,
            tmuxSessionID: String
        ) {
            self.executionTargetID = executionTargetID
            self.tmuxSessionID = tmuxSessionID
        }

        enum CodingKeys: String, CodingKey {
            case executionTargetID = "execution_target_id"
            case tmuxSessionID = "tmux_session_id"
        }
    }

    public enum Section: String, Codable, Equatable, Sendable {
        case local
        case remote
    }

    public enum TransportStatus:
        String, Codable, Equatable, Sendable
    {
        case online
        case offline
        case recovering
    }

    /// Non-terminal agent identities are listed separately; these are only CLI planes.
    public enum SessionKind: String, Codable, Equatable, Sendable {
        case terminal
        case codexPlane = "codex_plane"
        case claudePlane = "claude_plane"
    }

    public let identity: Identity
    public let sessionKind: SessionKind?
    public let originalAgentID: String?
    public let displayName: String
    public let detailLabel: String?
    public let targetLabel: String
    public let section: Section
    public let attachedClients: UInt32
    public let transportStatus: TransportStatus
    public let selected: Bool
    public let assignedToSelectedTab: Bool
    public let assignmentAction: ActionProjectionV1
    public let assignmentActions: SessionAssignmentActionsV1?
    public let renameAction: ActionProjectionV1?
    public let killAction: ActionProjectionV1?
    public let restartAction: ActionProjectionV1?
    public let replacement: SessionReplacementV1?
    public let replacementAction: ActionProjectionV1?

    public var id: String {
        "\(identity.executionTargetID):\(identity.tmuxSessionID)"
    }

    public var actions: [ActionProjectionV1] {
        [assignmentAction, renameAction, killAction, restartAction, replacementAction].compactMap { $0 }
    }

    public init(
        identity: Identity,
        sessionKind: SessionKind? = nil,
        originalAgentID: String? = nil,
        displayName: String,
        detailLabel: String? = nil,
        targetLabel: String,
        section: Section,
        attachedClients: UInt32,
        transportStatus: TransportStatus,
        selected: Bool,
        assignedToSelectedTab: Bool,
        assignmentAction: ActionProjectionV1,
        assignmentActions: SessionAssignmentActionsV1? = nil,
        renameAction: ActionProjectionV1? = nil,
        killAction: ActionProjectionV1? = nil,
        restartAction: ActionProjectionV1? = nil,
        replacement: SessionReplacementV1? = nil,
        replacementAction: ActionProjectionV1? = nil
    ) {
        self.identity = identity
        self.sessionKind = sessionKind
        self.originalAgentID = originalAgentID
        self.displayName = displayName
        self.detailLabel = detailLabel
        self.targetLabel = targetLabel
        self.section = section
        self.attachedClients = attachedClients
        self.transportStatus = transportStatus
        self.selected = selected
        self.assignedToSelectedTab = assignedToSelectedTab
        self.assignmentAction = assignmentAction
        self.assignmentActions = assignmentActions
        self.renameAction = renameAction
        self.killAction = killAction
        self.restartAction = restartAction
        self.replacement = replacement
        self.replacementAction = replacementAction
    }

    enum CodingKeys: String, CodingKey {
        case identity
        case sessionKind = "session_kind"
        case originalAgentID = "original_agent_id"
        case displayName = "display_name"
        case detailLabel = "detail_label"
        case targetLabel = "target_label"
        case section
        case attachedClients = "attached_clients"
        case transportStatus = "transport_status"
        case selected
        case assignedToSelectedTab = "assigned_to_selected_tab"
        case assignmentAction = "assignment_action"
        case assignmentActions = "assignment_actions"
        case renameAction = "rename_action"
        case killAction = "kill_action"
        case restartAction = "restart_action"
        case replacement
        case replacementAction = "replacement_action"
    }
}

public struct SessionLibraryProjectionV1: Codable, Equatable, Sendable {
    public let visible: Bool
    public let title: String
    public let interactionHint: String
    public let refreshAction: ActionProjectionV1
    public let closeAction: ActionProjectionV1
    public let sections: [SessionSectionProjectionV1]
    public let remoteTargets: [SessionTargetProjectionV1]
    public let rows: [SessionRowProjectionV1]
    public let selected: SessionRowProjectionV1.Identity?
    public let localExecutionTargetID: String
    public let remoteExecutionTargetID: String?
    public let localNewShell: ActionProjectionV1?
    public let localNewCommand: ActionProjectionV1?
    public let remoteNewSession: ActionProjectionV1?

    public init(
        visible: Bool,
        title: String,
        interactionHint: String,
        refreshAction: ActionProjectionV1,
        closeAction: ActionProjectionV1,
        sections: [SessionSectionProjectionV1],
        remoteTargets: [SessionTargetProjectionV1],
        rows: [SessionRowProjectionV1],
        selected: SessionRowProjectionV1.Identity?,
        localExecutionTargetID: String,
        remoteExecutionTargetID: String?,
        localNewShell: ActionProjectionV1? = nil,
        localNewCommand: ActionProjectionV1? = nil,
        remoteNewSession: ActionProjectionV1? = nil
    ) {
        self.visible = visible
        self.title = title
        self.interactionHint = interactionHint
        self.refreshAction = refreshAction
        self.closeAction = closeAction
        self.sections = sections
        self.remoteTargets = remoteTargets
        self.rows = rows
        self.selected = selected
        self.localExecutionTargetID = localExecutionTargetID
        self.remoteExecutionTargetID = remoteExecutionTargetID
        self.localNewShell = localNewShell
        self.localNewCommand = localNewCommand
        self.remoteNewSession = remoteNewSession
    }

    enum CodingKeys: String, CodingKey {
        case visible
        case title
        case interactionHint = "interaction_hint"
        case refreshAction = "refresh_action"
        case closeAction = "close_action"
        case sections
        case remoteTargets = "remote_targets"
        case rows
        case selected
        case localExecutionTargetID = "local_execution_target_id"
        case remoteExecutionTargetID = "remote_execution_target_id"
        case localNewShell = "local_new_shell"
        case localNewCommand = "local_new_command"
        case remoteNewSession = "remote_new_session"
    }
}

public struct SessionSectionProjectionV1:
    Codable, Equatable, Identifiable, Sendable
{
    public enum State: String, Codable, Equatable, Sendable {
        case loading
        case populated
        case empty
        case targetMissing = "target_missing"
        case error
    }

    public let section: SessionRowProjectionV1.Section
    public let title: String
    public let state: State
    public let message: String?
    public let rowCount: Int

    public var id: SessionRowProjectionV1.Section { section }

    enum CodingKeys: String, CodingKey {
        case section
        case title
        case state
        case message
        case rowCount = "row_count"
    }
}

public struct SessionTargetProjectionV1:
    Codable, Equatable, Identifiable, Sendable
{
    public let id: String
    public let label: String
}

public enum WorkspaceDisplayModeV1: String, Codable, Equatable, Sendable {
    case monitor
    case focus
}

public enum WorkspaceExecutionTargetKindV1:
    String, Codable, Equatable, Sendable
{
    case local
    case ssh
    case wsl
}

public enum SessionAttentionStateV1:
    String, Codable, Equatable, Sendable
{
    case unknown
    case idle
    case working
    case needsAttention = "needs_attention"
    case reviewReady = "review_ready"
    case approvalRequired = "approval_required"
    case decisionRequired = "decision_required"
    case permissionRequired = "permission_required"
    case failed

    public var isRed: Bool {
        switch self {
        case .approvalRequired, .decisionRequired,
             .permissionRequired, .failed:
            true
        default:
            false
        }
    }

    public var isOrange: Bool { self == .reviewReady || self == .needsAttention }
}

public enum SessionAttentionSourceV1: String, Codable, Equatable, Sendable {
    case unspecified
    case tmuxActivity = "tmux_activity"
    case tmuxBell = "tmux_bell"
    case paneTitle = "pane_title"
    case agentHook = "agent_hook"
    case aiClassification = "ai_classification"
}

public enum SessionAttentionConfidenceV1: String, Codable, Equatable, Sendable {
    case direct
    case inferred
}

public enum SessionMonitorHealthV1: String, Codable, Equatable, Sendable {
    case connected
    case degraded
    case disconnected
}

public enum SessionSummaryFreshnessV1:
    String, Codable, Equatable, Sendable
{
    case analyzing
    case fresh
    case stale
    case unavailable
}

public struct SessionAttentionProjectionV1:
    Codable, Equatable, Sendable
{
    public let executionTargetID: String
    public let tmuxSessionID: String
    public let summaryRevision: UInt64
    public let summarySections: [String]
    public let state: SessionAttentionStateV1
    public let freshness: SessionSummaryFreshnessV1
    public let primarySource: SessionAttentionSourceV1
    public let confidence: SessionAttentionConfidenceV1
    public let monitorHealth: SessionMonitorHealthV1
    public let monitorDetail: String?
    public let sourceEventID: String?
    public let attentionSinceMS: UInt64?
    public let unread: Bool
    public let notificationPending: Bool

    public init(
        executionTargetID: String = "local",
        tmuxSessionID: String = "unknown",
        summaryRevision: UInt64 = 0,
        summarySections: [String] = [],
        state: SessionAttentionStateV1 = .idle,
        freshness: SessionSummaryFreshnessV1 = .unavailable,
        primarySource: SessionAttentionSourceV1 = .unspecified,
        confidence: SessionAttentionConfidenceV1 = .inferred,
        monitorHealth: SessionMonitorHealthV1 = .disconnected,
        monitorDetail: String? = nil,
        sourceEventID: String? = nil,
        attentionSinceMS: UInt64? = nil,
        unread: Bool = false,
        notificationPending: Bool = false
    ) {
        self.executionTargetID = executionTargetID
        self.tmuxSessionID = tmuxSessionID
        self.summaryRevision = summaryRevision
        self.summarySections = summarySections
        self.state = state
        self.freshness = freshness
        self.primarySource = primarySource
        self.confidence = confidence
        self.monitorHealth = monitorHealth
        self.monitorDetail = monitorDetail
        self.sourceEventID = sourceEventID
        self.attentionSinceMS = attentionSinceMS
        self.unread = unread
        self.notificationPending = notificationPending
    }

    /// Source-compatible initializer retained for native modules that do not
    /// yet project monitor provenance. New fields receive conservative values.
    public init(
        executionTargetID: String,
        tmuxSessionID: String,
        summaryRevision: UInt64,
        summarySections: [String],
        state: SessionAttentionStateV1,
        freshness: SessionSummaryFreshnessV1,
        attentionSinceMS: UInt64?,
        unread: Bool,
        notificationPending: Bool
    ) {
        self.init(
            executionTargetID: executionTargetID,
            tmuxSessionID: tmuxSessionID,
            summaryRevision: summaryRevision,
            summarySections: summarySections,
            state: state,
            freshness: freshness,
            primarySource: .unspecified,
            confidence: .inferred,
            monitorHealth: .disconnected,
            attentionSinceMS: attentionSinceMS,
            unread: unread,
            notificationPending: notificationPending
        )
    }

    enum CodingKeys: String, CodingKey {
        case executionTargetID = "execution_target_id"
        case tmuxSessionID = "tmux_session_id"
        case summaryRevision = "summary_revision"
        case summarySections = "summary_sections"
        case state
        case freshness
        case primarySource = "primary_source"
        case confidence
        case monitorHealth = "monitor_health"
        case monitorDetail = "monitor_detail"
        case sourceEventID = "source_event_id"
        case attentionSinceMS = "attention_since_ms"
        case unread
        case notificationPending = "notification_pending"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            executionTargetID: try container.decode(
                String.self,
                forKey: .executionTargetID
            ),
            tmuxSessionID: try container.decode(
                String.self,
                forKey: .tmuxSessionID
            ),
            summaryRevision: try container.decode(
                UInt64.self,
                forKey: .summaryRevision
            ),
            summarySections: try container.decodeIfPresent(
                [String].self,
                forKey: .summarySections
            ) ?? [],
            state: try container.decode(
                SessionAttentionStateV1.self,
                forKey: .state
            ),
            freshness: try container.decode(
                SessionSummaryFreshnessV1.self,
                forKey: .freshness
            ),
            primarySource: try container.decodeIfPresent(
                SessionAttentionSourceV1.self,
                forKey: .primarySource
            ) ?? .unspecified,
            confidence: try container.decodeIfPresent(
                SessionAttentionConfidenceV1.self,
                forKey: .confidence
            ) ?? .inferred,
            monitorHealth: try container.decodeIfPresent(
                SessionMonitorHealthV1.self,
                forKey: .monitorHealth
            ) ?? .disconnected,
            monitorDetail: try container.decodeIfPresent(
                String.self,
                forKey: .monitorDetail
            ),
            sourceEventID: try container.decodeIfPresent(
                String.self,
                forKey: .sourceEventID
            ),
            attentionSinceMS: try container.decodeIfPresent(
                UInt64.self,
                forKey: .attentionSinceMS
            ),
            unread: try container.decode(Bool.self, forKey: .unread),
            notificationPending: try container.decode(
                Bool.self,
                forKey: .notificationPending
            )
        )
    }
}

public struct AttentionBadgeProjectionV1:
    Codable, Equatable, Sendable
{
    public let unresolvedRed: UInt32
    public let unreadOrange: UInt32
    public let highestState: SessionAttentionStateV1

    public init(
        unresolvedRed: UInt32 = 0,
        unreadOrange: UInt32 = 0,
        highestState: SessionAttentionStateV1 = .idle
    ) {
        self.unresolvedRed = unresolvedRed
        self.unreadOrange = unreadOrange
        self.highestState = highestState
    }

    enum CodingKeys: String, CodingKey {
        case unresolvedRed = "unresolved_red"
        case unreadOrange = "unread_orange"
        case highestState = "highest_state"
    }
}

public struct WorkspacePaneProjectionV1:
    Codable, Equatable, Identifiable, Sendable
{
    public enum SessionPresence: String, Codable, Equatable, Sendable {
        case present
        case ended
        case unknown
    }

    public let id: String
    public let executionTargetID: String?
    public let tmuxSessionID: String?
    public let content: StationPaneContentV1?
    public let targetLabel: String
    public let targetKind: WorkspaceExecutionTargetKindV1
    public let sessionName: String
    public let transportStatus:
        SessionRowProjectionV1.TransportStatus
    public let sessionPresence: SessionPresence
    public let selected: Bool
    public let attention: SessionAttentionProjectionV1

    public var title: String { sessionName }
    public var runtimeState: String { content?.kind == .ideaTree ? "local preview" : transportStatus.rawValue }
    public var sourceLabel: String {
        if content?.kind == .ideaTree { return "Local" }
        if content != nil { return "Polar" }
        return switch targetKind {
        case .local: "Local"
        case .ssh: "SSH"
        case .wsl: "WSL"
        }
    }
    public var actions: [ActionProjectionV1] { [] }

    public init(
        id: String,
        executionTargetID: String?,
        tmuxSessionID: String?,
        content: StationPaneContentV1? = nil,
        targetLabel: String,
        targetKind: WorkspaceExecutionTargetKindV1 = .local,
        sessionName: String,
        transportStatus:
            SessionRowProjectionV1.TransportStatus,
        sessionPresence: SessionPresence = .unknown,
        selected: Bool,
        attention: SessionAttentionProjectionV1 = .init()
    ) {
        self.id = id
        self.executionTargetID = executionTargetID
        self.tmuxSessionID = tmuxSessionID
        self.content = content
        self.targetLabel = targetLabel
        self.targetKind = targetKind
        self.sessionName = sessionName
        self.transportStatus = transportStatus
        self.sessionPresence = sessionPresence
        self.selected = selected
        self.attention = attention
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(
            keyedBy: CodingKeys.self
        )
        id = try container.decode(String.self, forKey: .id)
        content = try container.decodeIfPresent(StationPaneContentV1.self, forKey: .content)
        executionTargetID = try container.decodeIfPresent(
            String.self,
            forKey: .executionTargetID
        )
        tmuxSessionID = try container.decodeIfPresent(
            String.self,
            forKey: .tmuxSessionID
        )
        targetLabel = try container.decode(
            String.self,
            forKey: .targetLabel
        )
        targetKind = try container.decodeIfPresent(
            WorkspaceExecutionTargetKindV1.self,
            forKey: .targetKind
        ) ?? .local
        sessionName = try container.decode(
            String.self,
            forKey: .sessionName
        )
        transportStatus = try container.decode(
            SessionRowProjectionV1.TransportStatus.self,
            forKey: .transportStatus
        )
        sessionPresence = try container.decodeIfPresent(
            SessionPresence.self,
            forKey: .sessionPresence
        ) ?? .unknown
        selected = try container.decode(Bool.self, forKey: .selected)
        attention = try container.decodeIfPresent(
            SessionAttentionProjectionV1.self,
            forKey: .attention
        ) ?? SessionAttentionProjectionV1()
    }

    enum CodingKeys: String, CodingKey {
        case id, content
        case executionTargetID = "execution_target_id"
        case tmuxSessionID = "tmux_session_id"
        case targetLabel = "target_label"
        case targetKind = "target_kind"
        case sessionName = "session_name"
        case transportStatus = "transport_status"
        case sessionPresence = "session_presence"
        case selected
        case attention
    }
}

public struct WorkspaceTabProjectionV1:
    Codable, Equatable, Identifiable, Sendable
{
    public let id: String
    public let title: String
    public let selected: Bool
    public let displayMode: WorkspaceDisplayModeV1
    public let focusedPaneID: String?
    public let visualPaneIDs: [String]
    public let renameAction: ActionProjectionV1?
    public let deleteAction: ActionProjectionV1?
    public let panes: [WorkspacePaneProjectionV1]

    public init(
        id: String,
        title: String,
        selected: Bool,
        displayMode: WorkspaceDisplayModeV1,
        focusedPaneID: String?,
        visualPaneIDs: [String]? = nil,
        renameAction: ActionProjectionV1? = nil,
        deleteAction: ActionProjectionV1? = nil,
        panes: [WorkspacePaneProjectionV1]
    ) {
        self.id = id
        self.title = title
        self.selected = selected
        self.displayMode = displayMode
        self.focusedPaneID = focusedPaneID
        self.visualPaneIDs = visualPaneIDs ?? panes.map(\.id)
        self.renameAction = renameAction
        self.deleteAction = deleteAction
        self.panes = panes
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        title = try container.decode(String.self, forKey: .title)
        selected = try container.decode(Bool.self, forKey: .selected)
        displayMode = try container.decode(
            WorkspaceDisplayModeV1.self,
            forKey: .displayMode
        )
        focusedPaneID = try container.decodeIfPresent(
            String.self,
            forKey: .focusedPaneID
        )
        renameAction = try container.decodeIfPresent(
            ActionProjectionV1.self,
            forKey: .renameAction
        )
        deleteAction = try container.decodeIfPresent(
            ActionProjectionV1.self,
            forKey: .deleteAction
        )
        panes = try container.decode(
            [WorkspacePaneProjectionV1].self,
            forKey: .panes
        )
        visualPaneIDs = try container.decodeIfPresent(
            [String].self,
            forKey: .visualPaneIDs
        ) ?? panes.map(\.id)
    }

    enum CodingKeys: String, CodingKey {
        case id
        case title
        case selected
        case displayMode = "display_mode"
        case focusedPaneID = "focused_pane_id"
        case visualPaneIDs = "visual_pane_ids"
        case renameAction = "rename_action"
        case deleteAction = "delete_action"
        case panes
    }
}

/// Persisted display identity only. The Host must verify it on each binding.
public struct StationPaneContentV1: Codable, Equatable, Sendable {
    public enum Kind: String, Codable, Sendable { case lgpText = "lgp_text", ideaTree = "idea_tree" }
    public let kind: Kind
    public let targetID: String
    public let nodeID: String
    public let owner: String
    public let thread: String
    public let actor: String
    public let viewID: String
    public init(targetID: String, nodeID: String, owner: String, thread: String, actor: String) {
        self.kind = .lgpText; self.targetID = targetID; self.nodeID = nodeID
        self.owner = owner; self.thread = thread; self.actor = actor; self.viewID = ""
    }
    public init(viewID: String) {
        self.kind = .ideaTree; self.viewID = viewID
        self.targetID = ""; self.nodeID = ""; self.owner = ""; self.thread = ""; self.actor = ""
    }
    public var fingerprintComponents: [String] {
        ["polar-station-pane-content-v1", kind.rawValue] + (kind == .ideaTree ? [viewID] : [targetID, nodeID, owner, thread, actor])
    }
    public var isValid: Bool {
        (kind == .ideaTree ? [viewID] : [targetID, nodeID, owner, thread, actor]).allSatisfy {
            !$0.isEmpty && $0.utf8.count <= 256 && !$0.unicodeScalars.contains { $0.value < 32 || $0.value == 127 }
        }
    }
    enum CodingKeys: String, CodingKey {
        case kind, owner, thread, actor
        case targetID = "target_id", nodeID = "node_id", viewID = "view_id"
    }
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decode(Kind.self, forKey: .kind) {
        case .ideaTree:
            for key: CodingKeys in [.targetID, .nodeID, .owner, .thread, .actor] where values.contains(key) {
                throw DecodingError.dataCorruptedError(forKey: key, in: values, debugDescription: "Idea Tree has no conversation identity")
            }
            self.init(viewID: try values.decode(String.self, forKey: .viewID))
        case .lgpText:
            guard !values.contains(.viewID) else {
                throw DecodingError.dataCorruptedError(forKey: .viewID, in: values, debugDescription: "Conversation has no Idea Tree identity")
            }
            self.init(targetID: try values.decode(String.self, forKey: .targetID),
                      nodeID: try values.decode(String.self, forKey: .nodeID),
                      owner: try values.decode(String.self, forKey: .owner),
                      thread: try values.decode(String.self, forKey: .thread),
                      actor: try values.decode(String.self, forKey: .actor))
        }
    }
    public func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encode(kind, forKey: .kind)
        if kind == .ideaTree { try values.encode(viewID, forKey: .viewID); return }
        try values.encode(targetID, forKey: .targetID); try values.encode(nodeID, forKey: .nodeID)
        try values.encode(owner, forKey: .owner); try values.encode(thread, forKey: .thread)
        try values.encode(actor, forKey: .actor)
    }
}
public struct WorkspaceStructurePaneV1: Codable, Equatable, Sendable {
    public let id: String
    public let executionTargetID: String?
    public let tmuxSessionID: String?
    public let content: StationPaneContentV1?
    public init(id: String, executionTargetID: String? = nil, tmuxSessionID: String? = nil,
                content: StationPaneContentV1? = nil) {
        self.id = id; self.executionTargetID = executionTargetID
        self.tmuxSessionID = tmuxSessionID; self.content = content
    }
    enum CodingKeys: String, CodingKey {
        case id, content
        case executionTargetID = "execution_target_id"
        case tmuxSessionID = "tmux_session_id"
    }
}

public struct WorkspaceStructureTabV1:
    Codable, Equatable, Sendable
{
    public let id: String
    public let title: String
    public let displayMode: WorkspaceDisplayModeV1
    public let mainPaneID: String?
    public let canonicalPaneIDs: [String]
    public let visualPaneIDs: [String]
    public let panes: [WorkspaceStructurePaneV1]

    public init(
        id: String,
        title: String,
        displayMode: WorkspaceDisplayModeV1,
        mainPaneID: String?,
        canonicalPaneIDs: [String],
        visualPaneIDs: [String],
        panes: [WorkspaceStructurePaneV1]
    ) {
        self.id = id
        self.title = title
        self.displayMode = displayMode
        self.mainPaneID = mainPaneID
        self.canonicalPaneIDs = canonicalPaneIDs
        self.visualPaneIDs = visualPaneIDs
        self.panes = panes
    }

    enum CodingKeys: String, CodingKey {
        case id
        case title
        case displayMode = "display_mode"
        case mainPaneID = "main_pane_id"
        case canonicalPaneIDs = "canonical_pane_ids"
        case visualPaneIDs = "visual_pane_ids"
        case panes
    }
}

public struct WorkspaceStructureSnapshotV1:
    Codable, Equatable, Sendable
{
    public let schemaVersion: UInt16
    public let workspaceID: String
    public let workspaceName: String
    public let structureRevision: UInt64
    public let structureFingerprint: String
    public let selectedTabID: String
    public let selectedPaneID: String?
    public let tabs: [WorkspaceStructureTabV1]

    public init(
        schemaVersion: UInt16 = 1,
        workspaceID: String,
        workspaceName: String,
        structureRevision: UInt64,
        structureFingerprint: String,
        selectedTabID: String,
        selectedPaneID: String?,
        tabs: [WorkspaceStructureTabV1]
    ) {
        self.schemaVersion = schemaVersion
        self.workspaceID = workspaceID
        self.workspaceName = workspaceName
        self.structureRevision = structureRevision
        self.structureFingerprint = structureFingerprint
        self.selectedTabID = selectedTabID
        self.selectedPaneID = selectedPaneID
        self.tabs = tabs
    }

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case workspaceID = "workspace_id"
        case workspaceName = "workspace_name"
        case structureRevision = "structure_revision"
        case structureFingerprint = "structure_fingerprint"
        case selectedTabID = "selected_tab_id"
        case selectedPaneID = "selected_pane_id"
        case tabs
    }
}

public struct WorkspaceStructureCandidateV1:
    Codable, Equatable, Sendable
{
    public let baseStructureRevision: UInt64
    public let baseStructureFingerprint: String
    public let structure: WorkspaceStructureSnapshotV1

    public init(
        baseStructureRevision: UInt64,
        baseStructureFingerprint: String,
        structure: WorkspaceStructureSnapshotV1
    ) {
        self.baseStructureRevision = baseStructureRevision
        self.baseStructureFingerprint = baseStructureFingerprint
        self.structure = structure
    }

    enum CodingKeys: String, CodingKey {
        case baseStructureRevision = "base_structure_revision"
        case baseStructureFingerprint = "base_structure_fingerprint"
        case structure
    }
}

public struct WorkspaceMapProjectionV1: Codable, Equatable, Sendable {
    public let workspaceID: String
    public let workspaceName: String
    public let desiredRevision: UInt64
    public let structureRevision: UInt64
    public let structureFingerprint: String
    public let selectedTabID: String
    public let selectedPaneID: String?
    public let tabs: [WorkspaceTabProjectionV1]

    public init(
        workspaceID: String,
        workspaceName: String,
        desiredRevision: UInt64,
        structureRevision: UInt64? = nil,
        structureFingerprint: String = "",
        selectedTabID: String,
        selectedPaneID: String? = nil,
        tabs: [WorkspaceTabProjectionV1]
    ) {
        self.workspaceID = workspaceID
        self.workspaceName = workspaceName
        self.desiredRevision = desiredRevision
        self.structureRevision = structureRevision ?? desiredRevision
        self.structureFingerprint = structureFingerprint
        self.selectedTabID = selectedTabID
        self.selectedPaneID = selectedPaneID
        self.tabs = tabs
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        workspaceID = try container.decode(String.self, forKey: .workspaceID)
        workspaceName = try container.decode(String.self, forKey: .workspaceName)
        desiredRevision = try container.decode(UInt64.self, forKey: .desiredRevision)
        structureRevision = try container.decodeIfPresent(
            UInt64.self,
            forKey: .structureRevision
        ) ?? desiredRevision
        structureFingerprint = try container.decodeIfPresent(
            String.self,
            forKey: .structureFingerprint
        ) ?? ""
        selectedTabID = try container.decode(String.self, forKey: .selectedTabID)
        selectedPaneID = try container.decodeIfPresent(
            String.self,
            forKey: .selectedPaneID
        )
        tabs = try container.decode(
            [WorkspaceTabProjectionV1].self,
            forKey: .tabs
        )
    }

    enum CodingKeys: String, CodingKey {
        case workspaceID = "workspace_id"
        case workspaceName = "workspace_name"
        case desiredRevision = "desired_revision"
        case structureRevision = "structure_revision"
        case structureFingerprint = "structure_fingerprint"
        case selectedTabID = "selected_tab_id"
        case selectedPaneID = "selected_pane_id"
        case tabs
    }
}

public struct ControlBottomBarProjectionV1:
    Codable, Equatable, Sendable
{
    public let actions: [ActionProjectionV1]
    public let moreActions: [ActionProjectionV1]?

    public init(
        actions: [ActionProjectionV1],
        moreActions: [ActionProjectionV1]? = nil
    ) {
        self.actions = actions
        self.moreActions = moreActions
    }

    public func action(id: String) -> ActionProjectionV1? {
        actions.first { $0.id == id }
    }

    enum CodingKeys: String, CodingKey {
        case actions
        case moreActions = "more_actions"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(
            keyedBy: CodingKeys.self
        )
        actions = try container.decode(
            [ActionProjectionV1].self,
            forKey: .actions
        )
        moreActions = try container.decodeIfPresent(
            [ActionProjectionV1].self,
            forKey: .moreActions
        )
    }
}

public enum AuxiliarySurfaceIDV1:
    String, Codable, Equatable, Sendable
{
    case resources
    case iphone
}

public typealias AuxiliarySurfaceV1 = AuxiliarySurfaceIDV1

public struct AuxiliarySurfaceProjectionV1:
    Codable, Equatable, Identifiable, Sendable
{
    public let id: AuxiliarySurfaceIDV1
    public let title: String
    public let state: String
    public let message: String

    public init(
        id: AuxiliarySurfaceIDV1,
        title: String,
        state: String,
        message: String
    ) {
        self.id = id
        self.title = title
        self.state = state
        self.message = message
    }
}

public struct AuxiliarySurfacesProjectionV1:
    Codable, Equatable, Sendable
{
    public let resources: AuxiliarySurfaceProjectionV1
    public let iphone: AuxiliarySurfaceProjectionV1

    public init(
        resources: AuxiliarySurfaceProjectionV1,
        iphone: AuxiliarySurfaceProjectionV1
    ) {
        self.resources = resources
        self.iphone = iphone
    }

    public subscript(
        surface: AuxiliarySurfaceIDV1
    ) -> AuxiliarySurfaceProjectionV1 {
        switch surface {
        case .resources:
            resources
        case .iphone:
            iphone
        }
    }
}

public struct ControlCenterBannerV1: Codable, Equatable, Sendable {
    public enum Level: String, Codable, Equatable, Sendable {
        case information
        case warning
        case error
    }

    public let level: Level
    public let message: String

    public init(level: Level, message: String) {
        self.level = level
        self.message = message
    }
}

public struct CodexThreadMetadataV1: Codable, Equatable, Sendable {
    public let preview: String?
    public let cwd: String?
    public let created_at: Int64?
    public let updated_at: Int64?
    public let parent_thread_id: String?
    public let agent_role: String?
    public let agent_nickname: String?
}

public struct AgentSessionCatalogRowV1: Codable, Equatable, Sendable, Identifiable {
    public let terminals: [SessionRowProjectionV1.Identity]?
    public let metadata: CodexThreadMetadataV1?
    public let execution_target_id: String
    public let owner_host_id: String
    public let endpoint_id: String
    public let thread_id: String
    public let display_name: String
    public let status: String
    public let loaded: Bool
    public var id: String { "\(owner_host_id)/\(endpoint_id)/\(thread_id)" }
}

public struct ControlCenterProjectionV1: Codable, Equatable, Sendable {
    public let codexEndpointStates: [String: String]
    public let agentSessions: [AgentSessionCatalogRowV1]
    public let sessionGroups: [SessionTargetGroupV1]
    public let codexEndpoints: [CodexEndpointSettingsV1]
    public let connections: ConnectionsProjectionV1
    public let schemaVersion: UInt16
    public let controlRevision: UInt64
    public let header: ControlHeaderProjectionV1
    public let sessions: SessionLibraryProjectionV1
    public let bottomBar: ControlBottomBarProjectionV1
    public let projectedActions: [ActionProjectionV1]
    public let auxiliarySurfaces: AuxiliarySurfacesProjectionV1
    public let banner: ControlCenterBannerV1?
    public let attentionBadge: AttentionBadgeProjectionV1

    public init(
        schemaVersion: UInt16,
        controlRevision: UInt64,
        header: ControlHeaderProjectionV1,
        sessions: SessionLibraryProjectionV1,
        bottomBar: ControlBottomBarProjectionV1,
        projectedActions: [ActionProjectionV1],
        auxiliarySurfaces: AuxiliarySurfacesProjectionV1,
        banner: ControlCenterBannerV1?,
        attentionBadge: AttentionBadgeProjectionV1 = .init(),
        connections: ConnectionsProjectionV1 = .init(),
        sessionGroups: [SessionTargetGroupV1] = [],
        codexEndpoints: [CodexEndpointSettingsV1] = [],
        agentSessions: [AgentSessionCatalogRowV1] = [],
        codexEndpointStates: [String: String] = [:]
    ) {
        self.codexEndpointStates = codexEndpointStates
        self.agentSessions = agentSessions
        self.codexEndpoints = codexEndpoints
        self.sessionGroups = sessionGroups
        self.connections = connections
        self.schemaVersion = schemaVersion
        self.controlRevision = controlRevision
        self.header = header
        self.sessions = sessions
        self.bottomBar = bottomBar
        self.projectedActions = projectedActions
        self.auxiliarySurfaces = auxiliarySurfaces
        self.banner = banner
        self.attentionBadge = attentionBadge
    }

    enum CodingKeys: String, CodingKey {
        case connections
        case codexEndpointStates = "codex_endpoint_states"
        case agentSessions = "agent_sessions"
        case codexEndpoints = "codex_endpoints"
        case sessionGroups = "session_groups"
        case schemaVersion = "schema_version"
        case controlRevision = "control_revision"
        case header
        case sessions
        case bottomBar = "bottom_bar"
        case projectedActions = "projected_actions"
        case auxiliarySurfaces = "auxiliary_surfaces"
        case banner
        case attentionBadge = "attention_badge"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            schemaVersion: try container.decode(
                UInt16.self,
                forKey: .schemaVersion
            ),
            controlRevision: try container.decode(
                UInt64.self,
                forKey: .controlRevision
            ),
            header: try container.decode(
                ControlHeaderProjectionV1.self,
                forKey: .header
            ),
            sessions: try container.decode(
                SessionLibraryProjectionV1.self,
                forKey: .sessions
            ),
            bottomBar: try container.decode(
                ControlBottomBarProjectionV1.self,
                forKey: .bottomBar
            ),
            projectedActions: try container.decode(
                [ActionProjectionV1].self,
                forKey: .projectedActions
            ),
            auxiliarySurfaces: try container.decode(
                AuxiliarySurfacesProjectionV1.self,
                forKey: .auxiliarySurfaces
            ),
            banner: try container.decodeIfPresent(
                ControlCenterBannerV1.self,
                forKey: .banner
            ),
            attentionBadge: try container.decodeIfPresent(
                AttentionBadgeProjectionV1.self,
                forKey: .attentionBadge
            ) ?? .init(),
            connections: try container.decodeIfPresent(ConnectionsProjectionV1.self, forKey: .connections) ?? .init(),
            sessionGroups: try container.decodeIfPresent([SessionTargetGroupV1].self, forKey: .sessionGroups) ?? [],
            codexEndpoints: try container.decodeIfPresent([CodexEndpointSettingsV1].self, forKey: .codexEndpoints) ?? [],
            agentSessions: try container.decodeIfPresent([AgentSessionCatalogRowV1].self, forKey: .agentSessions) ?? [],
            codexEndpointStates: try container.decodeIfPresent([String: String].self, forKey: .codexEndpointStates) ?? [:]
        )
    }
}

public struct WorkspaceSSHTargetEditorProjectionV1:
    Codable, Equatable, Sendable
{
    public let title: String
    public let targetID: String
    public let targetRevision: UInt64
    public let destination: String
    public let routeProfile: WorkspaceSSHTargetRouteProfileV1?
    public let destinationLabel: String
    public let destinationPlaceholder: String
    public let cancelLabel: String
    public let saveLabel: String

    enum CodingKeys: String, CodingKey {
        case title
        case targetID = "target_id"
        case targetRevision = "target_revision"
        case destination
        case routeProfile = "route_profile"
        case destinationLabel = "destination_label"
        case destinationPlaceholder = "destination_placeholder"
        case cancelLabel = "cancel_label"
        case saveLabel = "save_label"
    }
}

public enum ControlCenterIntentOperationV1: Equatable, Sendable {
    case checkConnection(targetID: String, polar: Bool)
    case updateConnection(expectedRevision: String, change: ConnectionChangeV1)
    case invokeProjectedAction(actionID: String)
    case invokeConfirmedAction(actionID: String, confirmationID: String)
    case setManagedTerminalFontSize(points: Int)
    case createWorkspace(name: String)
    case selectWorkspace(workspaceID: String)
    case renameWorkspace(name: String)
    case setWorkspaceSSHTarget(
        targetID: String,
        targetRevision: UInt64,
        destination: String,
        routeProfile: WorkspaceSSHTargetRouteProfileV1?
    )
    case deleteWorkspace(confirmationID: String)
    case addTab(title: String, tabID: String? = nil)
    case selectTab(tabID: String)
    case renameTab(tabID: String, title: String)
    case deleteTab(
        tabID: String,
        confirmationID: String,
        replacementTabID: String? = nil
    )
    case reorderTab(tabID: String, toIndex: Int)
    case reorderPane(tabID: String, paneID: String, toIndex: Int)
    case movePane(
        paneID: String,
        fromTabID: String,
        toTabID: String,
        toIndex: Int
    )
    case toggleDisplayMode(tabID: String)
    case useExistingSession(selection: SessionReplacementSelectionV1)
    case selectSession(identity: SessionRowProjectionV1.Identity)
    case addSessionAssignments(tabID: String, sessions: [SessionRowProjectionV1.Identity])
    case toggleSessionAssignment(
        identity: SessionRowProjectionV1.Identity,
        tabID: String? = nil,
        paneID: String? = nil
    )
    case createCodexSession(executionTargetID: String, endpointID: String, name: String, workingDirectory: String?, yolo: Bool = false)
    case createAgentSession(executionTargetID: String, provider: String, mode: String, name: String, workingDirectory: String?, endpointID: String?, sessionID: String?, yolo: Bool = false, openLocalCLI: Bool = false, sessionIDs: [String] = [])
    case createSession(
        executionTargetID: String,
        name: String,
        workingDirectory: String?,
        command: String?,
        autoRestart: Bool = false
    )
    case endAgentSession(ownerHostID: String, endpointID: String, threadID: String, confirmationID: String)
    case renameAgentSession(ownerHostID: String, endpointID: String, threadID: String, name: String)
    case renameSession(
        identity: SessionRowProjectionV1.Identity,
        name: String
    )
    case killSession(
        identity: SessionRowProjectionV1.Identity,
        confirmationID: String
    )
    case selectPane(paneID: String)
    case activatePane(paneID: String)
    case removePane(paneID: String)
    case acknowledgePaneSummary(paneID: String, summaryRevision: UInt64)
    case markAttentionNotificationHandled(
        paneID: String,
        summaryRevision: UInt64
    )
    case openAuxiliary(surface: AuxiliarySurfaceIDV1)
    case closeAuxiliary(surface: AuxiliarySurfaceIDV1)
}

extension ControlCenterIntentOperationV1: Codable {
    private enum CodingKeys: String, CodingKey {
        case type, change, polar
        case expectedRevision = "expected_revision"
        case actionID = "action_id"
        case confirmationID = "confirmation_id"
        case workspaceID = "workspace_id"
        case targetID = "target_id"
        case targetRevision = "target_revision"
        case executionTargetID = "execution_target_id"
        case name
        case title
        case ownerHostID = "owner_host_id", threadID = "thread_id"
        case endpointID = "endpoint_id"
        case provider, mode
        case sessionID = "session_id"
        case openLocalCLI = "open_local_cli"
        case sessionIDs = "session_ids"
        case sessions
        case openTerminal = "open_terminal"
        case workingDirectory = "working_directory"
        case yolo
        case command
        case autoRestart = "auto_restart"
        case tabID = "tab_id"
        case replacementTabID = "replacement_tab_id"
        case fromTabID = "from_tab_id"
        case toTabID = "to_tab_id"
        case toIndex = "to_index"
        case identity
        case selection
        case paneID = "pane_id"
        case surface
        case points
        case destination
        case routeProfile = "route_profile"
        case summaryRevision = "summary_revision"
    }

    private enum Kind: String, Codable {
        case checkConnection = "check_connection"
        case updateConnection = "update_connection"
        case invokeProjectedAction = "invoke_projected_action"
        case invokeConfirmedAction = "invoke_confirmed_action"
        case setManagedTerminalFontSize =
            "set_managed_terminal_font_size"
        case createWorkspace = "create_workspace"
        case selectWorkspace = "select_workspace"
        case renameWorkspace = "rename_workspace"
        case setWorkspaceSSHTarget = "set_workspace_ssh_target"
        case deleteWorkspace = "delete_workspace"
        case addTab = "add_tab"
        case selectTab = "select_tab"
        case renameTab = "rename_tab"
        case deleteTab = "delete_tab"
        case reorderTab = "reorder_tab"
        case reorderPane = "reorder_pane"
        case movePane = "move_pane"
        case toggleDisplayMode = "toggle_display_mode"
        case useExistingSession = "use_existing_session"
        case selectSession = "select_session"
        case addSessionAssignments = "add_session_assignments"
        case toggleSessionAssignment = "toggle_session_assignment"
        case createCodexSession = "create_codex_session"
        case createAgentSession = "create_agent_session"
        case createSession = "create_session"
        case endAgentSession = "end_agent_session"
        case renameAgentSession = "rename_agent_session"
        case renameSession = "rename_session"
        case killSession = "kill_session"
        case selectPane = "select_pane"
        case activatePane = "activate_pane"
        case removePane = "remove_pane"
        case acknowledgePaneSummary = "acknowledge_pane_summary"
        case markAttentionNotificationHandled =
            "mark_attention_notification_handled"
        case openAuxiliary = "open_auxiliary"
        case closeAuxiliary = "close_auxiliary"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(Kind.self, forKey: .type) {
        case .checkConnection:
            self = .checkConnection(targetID: try container.decode(String.self, forKey: .targetID), polar: try container.decode(Bool.self, forKey: .polar))
        case .updateConnection:
            self = .updateConnection(expectedRevision: try container.decode(String.self, forKey: .expectedRevision), change: try container.decode(ConnectionChangeV1.self, forKey: .change))
        case .invokeProjectedAction:
            self = .invokeProjectedAction(
                actionID: try container.decode(String.self, forKey: .actionID)
            )
        case .invokeConfirmedAction:
            self = .invokeConfirmedAction(
                actionID: try container.decode(String.self, forKey: .actionID),
                confirmationID: try container.decode(
                    String.self,
                    forKey: .confirmationID
                )
            )
        case .setManagedTerminalFontSize:
            self = .setManagedTerminalFontSize(
                points: try container.decode(
                    Int.self,
                    forKey: .points
                )
            )
        case .createWorkspace:
            self = .createWorkspace(
                name: try container.decode(String.self, forKey: .name)
            )
        case .selectWorkspace:
            self = .selectWorkspace(
                workspaceID: try container.decode(
                    String.self,
                    forKey: .workspaceID
                )
            )
        case .renameWorkspace:
            self = .renameWorkspace(
                name: try container.decode(String.self, forKey: .name)
            )
        case .setWorkspaceSSHTarget:
            self = .setWorkspaceSSHTarget(
                targetID: try container.decode(
                    String.self,
                    forKey: .targetID
                ),
                targetRevision: try container.decode(
                    UInt64.self,
                    forKey: .targetRevision
                ),
                destination: try container.decode(
                    String.self,
                    forKey: .destination
                ),
                routeProfile: try container.decode(
                    WorkspaceSSHTargetRouteProfileV1?.self,
                    forKey: .routeProfile
                )
            )
        case .deleteWorkspace:
            self = .deleteWorkspace(
                confirmationID: try container.decode(
                    String.self,
                    forKey: .confirmationID
                )
            )
        case .addTab:
            self = .addTab(
                title: try container.decode(String.self, forKey: .title),
                tabID: try container.decodeIfPresent(
                    String.self,
                    forKey: .tabID
                )
            )
        case .selectTab:
            self = .selectTab(
                tabID: try container.decode(String.self, forKey: .tabID)
            )
        case .renameTab:
            self = .renameTab(
                tabID: try container.decode(String.self, forKey: .tabID),
                title: try container.decode(String.self, forKey: .title)
            )
        case .deleteTab:
            self = .deleteTab(
                tabID: try container.decode(String.self, forKey: .tabID),
                confirmationID: try container.decode(
                    String.self,
                    forKey: .confirmationID
                ),
                replacementTabID: try container.decodeIfPresent(
                    String.self,
                    forKey: .replacementTabID
                )
            )
        case .reorderTab:
            self = .reorderTab(
                tabID: try container.decode(String.self, forKey: .tabID),
                toIndex: try container.decode(Int.self, forKey: .toIndex)
            )
        case .reorderPane:
            self = .reorderPane(
                tabID: try container.decode(String.self, forKey: .tabID),
                paneID: try container.decode(String.self, forKey: .paneID),
                toIndex: try container.decode(Int.self, forKey: .toIndex)
            )
        case .movePane:
            self = .movePane(
                paneID: try container.decode(String.self, forKey: .paneID),
                fromTabID: try container.decode(String.self, forKey: .fromTabID),
                toTabID: try container.decode(String.self, forKey: .toTabID),
                toIndex: try container.decode(Int.self, forKey: .toIndex)
            )
        case .toggleDisplayMode:
            self = .toggleDisplayMode(
                tabID: try container.decode(String.self, forKey: .tabID)
            )
        case .useExistingSession:
            self = .useExistingSession(selection: try container.decode(SessionReplacementSelectionV1.self, forKey: .selection))
        case .selectSession:
            self = .selectSession(
                identity: try container.decode(
                    SessionRowProjectionV1.Identity.self,
                    forKey: .identity
                )
            )
        case .addSessionAssignments:
            self = try .addSessionAssignments(tabID: container.decode(String.self, forKey: .tabID), sessions: container.decode([SessionRowProjectionV1.Identity].self, forKey: .sessions))
        case .toggleSessionAssignment:
            self = .toggleSessionAssignment(
                identity: try container.decode(
                    SessionRowProjectionV1.Identity.self,
                    forKey: .identity
                ),
                tabID: try container.decodeIfPresent(
                    String.self,
                    forKey: .tabID
                ),
                paneID: try container.decodeIfPresent(
                    String.self,
                    forKey: .paneID
                )
            )
        case .createCodexSession:
            self = .createCodexSession(executionTargetID: try container.decode(String.self, forKey: .executionTargetID), endpointID: try container.decode(String.self, forKey: .endpointID), name: try container.decode(String.self, forKey: .name), workingDirectory: try container.decodeIfPresent(String.self, forKey: .workingDirectory), yolo: try container.decodeIfPresent(Bool.self, forKey: .yolo) ?? false)
        case .createAgentSession:
            self = .createAgentSession(executionTargetID: try container.decode(String.self, forKey: .executionTargetID), provider: try container.decode(String.self, forKey: .provider), mode: try container.decode(String.self, forKey: .mode), name: try container.decode(String.self, forKey: .name), workingDirectory: try container.decodeIfPresent(String.self, forKey: .workingDirectory), endpointID: try container.decodeIfPresent(String.self, forKey: .endpointID), sessionID: try container.decodeIfPresent(String.self, forKey: .sessionID), yolo: try container.decodeIfPresent(Bool.self, forKey: .yolo) ?? false, openLocalCLI: try container.decodeIfPresent(Bool.self, forKey: .openLocalCLI) ?? false, sessionIDs: try container.decodeIfPresent([String].self, forKey: .sessionIDs) ?? [])
        case .createSession:
            self = .createSession(
                executionTargetID: try container.decode(
                    String.self,
                    forKey: .executionTargetID
                ),
                name: try container.decode(String.self, forKey: .name),
                workingDirectory: try container.decodeIfPresent(
                    String.self,
                    forKey: .workingDirectory
                ),
                command: try container.decodeIfPresent(
                    String.self,
                    forKey: .command
                ),
                autoRestart: try container.decodeIfPresent(Bool.self, forKey: .autoRestart) ?? false
            )
        case .endAgentSession:
            self = .endAgentSession(ownerHostID: try container.decode(String.self, forKey: .ownerHostID), endpointID: try container.decode(String.self, forKey: .endpointID), threadID: try container.decode(String.self, forKey: .threadID), confirmationID: try container.decode(String.self, forKey: .confirmationID))
        case .renameAgentSession:
            self = .renameAgentSession(ownerHostID: try container.decode(String.self, forKey: .ownerHostID), endpointID: try container.decode(String.self, forKey: .endpointID), threadID: try container.decode(String.self, forKey: .threadID), name: try container.decode(String.self, forKey: .name))
        case .renameSession:
            self = .renameSession(
                identity: try container.decode(
                    SessionRowProjectionV1.Identity.self,
                    forKey: .identity
                ),
                name: try container.decode(String.self, forKey: .name)
            )
        case .killSession:
            self = .killSession(
                identity: try container.decode(
                    SessionRowProjectionV1.Identity.self,
                    forKey: .identity
                ),
                confirmationID: try container.decode(
                    String.self,
                    forKey: .confirmationID
                )
            )
        case .selectPane:
            self = .selectPane(
                paneID: try container.decode(String.self, forKey: .paneID)
            )
        case .activatePane:
            self = .activatePane(
                paneID: try container.decode(String.self, forKey: .paneID)
            )
        case .removePane:
            self = .removePane(
                paneID: try container.decode(String.self, forKey: .paneID)
            )
        case .acknowledgePaneSummary:
            self = .acknowledgePaneSummary(
                paneID: try container.decode(String.self, forKey: .paneID),
                summaryRevision: try container.decode(
                    UInt64.self,
                    forKey: .summaryRevision
                )
            )
        case .markAttentionNotificationHandled:
            self = .markAttentionNotificationHandled(
                paneID: try container.decode(String.self, forKey: .paneID),
                summaryRevision: try container.decode(
                    UInt64.self,
                    forKey: .summaryRevision
                )
            )
        case .openAuxiliary:
            self = .openAuxiliary(
                surface: try container.decode(
                    AuxiliarySurfaceIDV1.self,
                    forKey: .surface
                )
            )
        case .closeAuxiliary:
            self = .closeAuxiliary(
                surface: try container.decode(
                    AuxiliarySurfaceIDV1.self,
                    forKey: .surface
                )
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .checkConnection(let id, let polar):
            try container.encode(Kind.checkConnection, forKey: .type)
            try container.encode(id, forKey: .targetID)
            try container.encode(polar, forKey: .polar)
        case .updateConnection(let revision, let change):
            try container.encode(Kind.updateConnection, forKey: .type)
            try container.encode(revision, forKey: .expectedRevision)
            try container.encode(change, forKey: .change)
        case .invokeProjectedAction(let actionID):
            try container.encode(Kind.invokeProjectedAction, forKey: .type)
            try container.encode(actionID, forKey: .actionID)
        case .invokeConfirmedAction(let actionID, let confirmationID):
            try container.encode(Kind.invokeConfirmedAction, forKey: .type)
            try container.encode(actionID, forKey: .actionID)
            try container.encode(confirmationID, forKey: .confirmationID)
        case .setManagedTerminalFontSize(let points):
            try container.encode(
                Kind.setManagedTerminalFontSize,
                forKey: .type
            )
            try container.encode(points, forKey: .points)
        case .createWorkspace(let name):
            try container.encode(Kind.createWorkspace, forKey: .type)
            try container.encode(name, forKey: .name)
        case .selectWorkspace(let workspaceID):
            try container.encode(Kind.selectWorkspace, forKey: .type)
            try container.encode(workspaceID, forKey: .workspaceID)
        case .renameWorkspace(let name):
            try container.encode(Kind.renameWorkspace, forKey: .type)
            try container.encode(name, forKey: .name)
        case .setWorkspaceSSHTarget(
            let targetID,
            let targetRevision,
            let destination,
            let routeProfile
        ):
            try container.encode(Kind.setWorkspaceSSHTarget, forKey: .type)
            try container.encode(targetID, forKey: .targetID)
            try container.encode(targetRevision, forKey: .targetRevision)
            try container.encode(destination, forKey: .destination)
            try container.encode(routeProfile, forKey: .routeProfile)
        case .deleteWorkspace(let confirmationID):
            try container.encode(Kind.deleteWorkspace, forKey: .type)
            try container.encode(confirmationID, forKey: .confirmationID)
        case .addTab(let title, let tabID):
            try container.encode(Kind.addTab, forKey: .type)
            try container.encode(title, forKey: .title)
            try container.encodeIfPresent(tabID, forKey: .tabID)
        case .selectTab(let tabID):
            try container.encode(Kind.selectTab, forKey: .type)
            try container.encode(tabID, forKey: .tabID)
        case .renameTab(let tabID, let title):
            try container.encode(Kind.renameTab, forKey: .type)
            try container.encode(tabID, forKey: .tabID)
            try container.encode(title, forKey: .title)
        case .deleteTab(let tabID, let confirmationID, let replacementTabID):
            try container.encode(Kind.deleteTab, forKey: .type)
            try container.encode(tabID, forKey: .tabID)
            try container.encode(confirmationID, forKey: .confirmationID)
            try container.encodeIfPresent(
                replacementTabID,
                forKey: .replacementTabID
            )
        case .reorderTab(let tabID, let toIndex):
            try container.encode(Kind.reorderTab, forKey: .type)
            try container.encode(tabID, forKey: .tabID)
            try container.encode(toIndex, forKey: .toIndex)
        case .reorderPane(let tabID, let paneID, let toIndex):
            try container.encode(Kind.reorderPane, forKey: .type)
            try container.encode(tabID, forKey: .tabID)
            try container.encode(paneID, forKey: .paneID)
            try container.encode(toIndex, forKey: .toIndex)
        case .movePane(let paneID, let fromTabID, let toTabID, let toIndex):
            try container.encode(Kind.movePane, forKey: .type)
            try container.encode(paneID, forKey: .paneID)
            try container.encode(fromTabID, forKey: .fromTabID)
            try container.encode(toTabID, forKey: .toTabID)
            try container.encode(toIndex, forKey: .toIndex)
        case .toggleDisplayMode(let tabID):
            try container.encode(Kind.toggleDisplayMode, forKey: .type)
            try container.encode(tabID, forKey: .tabID)
        case .useExistingSession(let selection):
            try container.encode(Kind.useExistingSession, forKey: .type)
            try container.encode(selection, forKey: .selection)
        case .selectSession(let identity):
            try container.encode(Kind.selectSession, forKey: .type)
            try container.encode(identity, forKey: .identity)
        case .addSessionAssignments(let tabID, let sessions):
            try container.encode(Kind.addSessionAssignments, forKey: .type)
            try container.encode(tabID, forKey: .tabID); try container.encode(sessions, forKey: .sessions)
        case .toggleSessionAssignment(let identity, let tabID, let paneID):
            try container.encode(Kind.toggleSessionAssignment, forKey: .type)
            try container.encode(identity, forKey: .identity)
            try container.encodeIfPresent(tabID, forKey: .tabID)
            try container.encodeIfPresent(paneID, forKey: .paneID)
        case let .createCodexSession(target, endpoint, name, directory, yolo):
            try container.encode(yolo, forKey: .yolo)
            try container.encode(Kind.createCodexSession, forKey: .type)
            try container.encode(target, forKey: .executionTargetID)
            try container.encode(endpoint, forKey: .endpointID)
            try container.encode(name, forKey: .name)
            try container.encodeIfPresent(directory, forKey: .workingDirectory)
        case let .createAgentSession(target, provider, mode, name, directory, endpoint, sessionID, yolo, openLocalCLI, sessionIDs):
            try container.encode(Kind.createAgentSession, forKey: .type)
            try container.encode(target, forKey: .executionTargetID)
            try container.encode(provider, forKey: .provider)
            try container.encode(mode, forKey: .mode)
            try container.encode(name, forKey: .name)
            try container.encodeIfPresent(directory, forKey: .workingDirectory)
            try container.encodeIfPresent(endpoint, forKey: .endpointID)
            try container.encodeIfPresent(sessionID, forKey: .sessionID)
            try container.encode(yolo, forKey: .yolo)
            try container.encode(openLocalCLI, forKey: .openLocalCLI)
            try container.encode(sessionIDs, forKey: .sessionIDs)
            try container.encode(true, forKey: .openTerminal)
        case .createSession(
            let executionTargetID,
            let name,
            let workingDirectory,
            let command,
            let autoRestart
        ):
            try container.encode(Kind.createSession, forKey: .type)
            try container.encode(executionTargetID, forKey: .executionTargetID)
            try container.encode(name, forKey: .name)
            try container.encodeIfPresent(
                workingDirectory,
                forKey: .workingDirectory
            )
            try container.encodeIfPresent(command, forKey: .command)
            try container.encode(autoRestart, forKey: .autoRestart)
        case let .endAgentSession(owner, endpoint, thread, confirmation):
            try container.encode(Kind.endAgentSession, forKey: .type)
            try container.encode(owner, forKey: .ownerHostID)
            try container.encode(endpoint, forKey: .endpointID)
            try container.encode(thread, forKey: .threadID)
            try container.encode(confirmation, forKey: .confirmationID)
        case let .renameAgentSession(owner, endpoint, thread, name):
            try container.encode(Kind.renameAgentSession, forKey: .type)
            try container.encode(owner, forKey: .ownerHostID)
            try container.encode(endpoint, forKey: .endpointID)
            try container.encode(thread, forKey: .threadID)
            try container.encode(name, forKey: .name)
        case .renameSession(let identity, let name):
            try container.encode(Kind.renameSession, forKey: .type)
            try container.encode(identity, forKey: .identity)
            try container.encode(name, forKey: .name)
        case .killSession(let identity, let confirmationID):
            try container.encode(Kind.killSession, forKey: .type)
            try container.encode(identity, forKey: .identity)
            try container.encode(confirmationID, forKey: .confirmationID)
        case .selectPane(let paneID):
            try container.encode(Kind.selectPane, forKey: .type)
            try container.encode(paneID, forKey: .paneID)
        case .activatePane(let paneID):
            try container.encode(Kind.activatePane, forKey: .type)
            try container.encode(paneID, forKey: .paneID)
        case .removePane(let paneID):
            try container.encode(Kind.removePane, forKey: .type)
            try container.encode(paneID, forKey: .paneID)
        case .acknowledgePaneSummary(let paneID, let summaryRevision):
            try container.encode(
                Kind.acknowledgePaneSummary,
                forKey: .type
            )
            try container.encode(paneID, forKey: .paneID)
            try container.encode(summaryRevision, forKey: .summaryRevision)
        case .markAttentionNotificationHandled(
            let paneID,
            let summaryRevision
        ):
            try container.encode(
                Kind.markAttentionNotificationHandled,
                forKey: .type
            )
            try container.encode(paneID, forKey: .paneID)
            try container.encode(summaryRevision, forKey: .summaryRevision)
        case .openAuxiliary(let surface):
            try container.encode(Kind.openAuxiliary, forKey: .type)
            try container.encode(surface, forKey: .surface)
        case .closeAuxiliary(let surface):
            try container.encode(Kind.closeAuxiliary, forKey: .type)
            try container.encode(surface, forKey: .surface)
        }
    }
}

public struct ControlCenterIntentV1: Codable, Equatable, Sendable {
    public let controlRevision: UInt64
    public let operation: ControlCenterIntentOperationV1

    public init(
        controlRevision: UInt64,
        operation: ControlCenterIntentOperationV1
    ) {
        self.controlRevision = controlRevision
        self.operation = operation
    }

    enum CodingKeys: String, CodingKey {
        case controlRevision = "control_revision"
        case operation
    }
}

/// Exact Rust `ControlCenterIntentV1` wire shape.
///
/// The native renderer's auxiliary close operation is AppKit-only and
/// deliberately has no Host equivalent.
public enum QuickSessionKindV1: String, Codable, Equatable, Sendable { case shell, codex }

public enum ControlCenterHostIntentV1: Encodable, Equatable, Sendable {
    case checkConnection(targetID: String, polar: Bool)
    case updateConnection(expectedRevision: String, change: ConnectionChangeV1)
    case quickCreateSession(executionTargetID: String, sourceSessionID: String?, kind: QuickSessionKindV1)
    case refreshSessions
    case setSSHTarget(targetID: String, targetRevision: UInt64, destination: String, routeProfile: WorkspaceSSHTargetRouteProfileV1?)
    case createCodexSession(executionTargetID: String, endpointID: String, name: String, workingDirectory: String?, yolo: Bool = false)
    case createAgentSession(executionTargetID: String, provider: String, mode: String, name: String, workingDirectory: String?, endpointID: String?, sessionID: String?, yolo: Bool = false, openLocalCLI: Bool = false, sessionIDs: [String] = [])
    case createSession(executionTargetID: String, name: String, workingDirectory: String?, command: String?, autoRestart: Bool = false)
    case useExistingSession(identity: SessionRowProjectionV1.Identity, replacement: SessionReplacementV1, confirmationID: String, retire: Bool)
    case restartSession(identity: SessionRowProjectionV1.Identity)
    case endAgentSession(ownerHostID: String, endpointID: String, threadID: String, confirmationID: String)
    case renameAgentSession(ownerHostID: String, endpointID: String, threadID: String, name: String)
    case renameSession(identity: SessionRowProjectionV1.Identity, name: String)
    case killSession(identity: SessionRowProjectionV1.Identity, confirmationID: String)
    case acknowledgeSessionSummary(identity: SessionRowProjectionV1.Identity, summaryRevision: UInt64)
    case markAttentionNotificationHandled(identity: SessionRowProjectionV1.Identity, summaryRevision: UInt64)

    private enum CodingKeys: String, CodingKey {
        case change, polar
        case expectedRevision = "expected_revision"
        case intent, name, destination, command, replacement, retire, kind
        case sourceSessionID = "source_session_id"
        case targetID = "target_id", targetRevision = "target_revision", routeProfile = "route_profile"
        case executionTargetID = "execution_target_id", tmuxSessionID = "tmux_session_id"
        case ownerHostID = "owner_host_id", threadID = "thread_id"
        case endpointID = "endpoint_id"
        case provider, mode
        case sessionID = "session_id"
        case openLocalCLI = "open_local_cli"
        case sessionIDs = "session_ids"
        case openTerminal = "open_terminal"
        case workingDirectory = "working_directory"
        case yolo, autoRestart = "auto_restart"
        case confirmationID = "confirmation_id", summaryRevision = "summary_revision"
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .checkConnection(let id, let polar):
            try c.encode("check_connection", forKey: .intent)
            try c.encode(id, forKey: .targetID)
            try c.encode(polar, forKey: .polar)
        case .updateConnection(let revision, let change):
            try c.encode("update_connection", forKey: .intent)
            try c.encode(revision, forKey: .expectedRevision)
            try c.encode(change, forKey: .change)
        case let .quickCreateSession(target, source, kind):
            try c.encode("quick_create_session", forKey: .intent)
            try c.encode(target, forKey: .executionTargetID)
            try c.encode(source, forKey: .sourceSessionID)
            try c.encode(kind, forKey: .kind)
        case .refreshSessions:
            try c.encode("refresh_sessions", forKey: .intent)
        case let .setSSHTarget(id, revision, destination, profile):
            try c.encode("set_ssh_target", forKey: .intent)
            try c.encode(id, forKey: .targetID)
            try c.encode(revision, forKey: .targetRevision)
            try c.encode(destination, forKey: .destination)
            try c.encode(profile, forKey: .routeProfile)
        case let .createCodexSession(target, endpoint, name, directory, yolo):
            try c.encode(yolo, forKey: .yolo)
            try c.encode("create_codex_session", forKey: .intent)
            try c.encode(target, forKey: .executionTargetID)
            try c.encode(endpoint, forKey: .endpointID)
            try c.encode(name, forKey: .name)
            try c.encodeIfPresent(directory, forKey: .workingDirectory)
        case let .createAgentSession(target, provider, mode, name, directory, endpoint, sessionID, yolo, openLocalCLI, sessionIDs):
            try c.encode("create_agent_session", forKey: .intent)
            try c.encode(target, forKey: .executionTargetID)
            try c.encode(provider, forKey: .provider)
            try c.encode(mode, forKey: .mode)
            try c.encode(name, forKey: .name)
            try c.encodeIfPresent(directory, forKey: .workingDirectory)
            try c.encodeIfPresent(endpoint, forKey: .endpointID)
            try c.encodeIfPresent(sessionID, forKey: .sessionID)
            try c.encode(yolo, forKey: .yolo)
            try c.encode(openLocalCLI, forKey: .openLocalCLI)
            try c.encode(sessionIDs, forKey: .sessionIDs)
            try c.encode(true, forKey: .openTerminal)
        case let .createSession(target, name, directory, command, autoRestart):
            try c.encode("create_session", forKey: .intent)
            try c.encode(target, forKey: .executionTargetID)
            try c.encode(name, forKey: .name)
            try c.encode(directory, forKey: .workingDirectory)
            try c.encode(command, forKey: .command)
            try c.encode(autoRestart, forKey: .autoRestart)
        case let .useExistingSession(identity, replacement, confirmation, retire):
            try c.encode("use_existing_session", forKey: .intent)
            try encode(identity, into: &c)
            try c.encode(replacement, forKey: .replacement)
            try c.encode(confirmation, forKey: .confirmationID)
            try c.encode(retire, forKey: .retire)
        case let .restartSession(identity):
            try c.encode("restart_session", forKey: .intent)
            try encode(identity, into: &c)
        case let .endAgentSession(owner, endpoint, thread, confirmation):
            try c.encode("end_agent_session", forKey: .intent)
            try c.encode(owner, forKey: .ownerHostID)
            try c.encode(endpoint, forKey: .endpointID)
            try c.encode(thread, forKey: .threadID)
            try c.encode(confirmation, forKey: .confirmationID)
        case let .renameAgentSession(owner, endpoint, thread, name):
            try c.encode("rename_agent_session", forKey: .intent)
            try c.encode(owner, forKey: .ownerHostID)
            try c.encode(endpoint, forKey: .endpointID)
            try c.encode(thread, forKey: .threadID)
            try c.encode(name, forKey: .name)
        case let .renameSession(identity, name):
            try c.encode("rename_session", forKey: .intent)
            try encode(identity, into: &c)
            try c.encode(name, forKey: .name)
        case let .killSession(identity, confirmation):
            try c.encode("kill_session", forKey: .intent)
            try encode(identity, into: &c)
            try c.encode(confirmation, forKey: .confirmationID)
        case let .acknowledgeSessionSummary(identity, revision):
            try c.encode("acknowledge_session_summary", forKey: .intent)
            try encode(identity, into: &c)
            try c.encode(revision, forKey: .summaryRevision)
        case let .markAttentionNotificationHandled(identity, revision):
            try c.encode("mark_attention_notification_handled", forKey: .intent)
            try encode(identity, into: &c)
            try c.encode(revision, forKey: .summaryRevision)
        }
    }
    private func encode(_ identity: SessionRowProjectionV1.Identity, into c: inout KeyedEncodingContainer<CodingKeys>) throws {
        try c.encode(identity.executionTargetID, forKey: .executionTargetID)
        try c.encode(identity.tmuxSessionID, forKey: .tmuxSessionID)
    }
}

public extension ControlCenterIntentOperationV1 {
    /// Native layout and presentation commands have no Host equivalent.
    var hostIntent: ControlCenterHostIntentV1? {
        switch self {
        case .checkConnection(let id, let polar): return .checkConnection(targetID: id, polar: polar)
        case .updateConnection(let revision, let change):
            return .updateConnection(expectedRevision: revision, change: change)
        case let .setWorkspaceSSHTarget(id, revision, destination, profile):
            return .setSSHTarget(targetID: id, targetRevision: revision, destination: destination, routeProfile: profile)
        case let .createCodexSession(target, endpoint, name, directory, yolo):
            return .createCodexSession(executionTargetID: target, endpointID: endpoint, name: name, workingDirectory: directory, yolo: yolo)
        case let .createAgentSession(target, provider, mode, name, directory, endpoint, sessionID, yolo, openLocalCLI, sessionIDs):
            return .createAgentSession(executionTargetID: target, provider: provider, mode: mode, name: name, workingDirectory: directory, endpointID: endpoint, sessionID: sessionID, yolo: yolo, openLocalCLI: openLocalCLI, sessionIDs: sessionIDs)
        case let .createSession(target, name, directory, command, autoRestart):
            return .createSession(executionTargetID: target, name: name, workingDirectory: directory, command: command, autoRestart: autoRestart)
        case let .endAgentSession(owner, endpoint, thread, confirmation):
            return .endAgentSession(ownerHostID: owner, endpointID: endpoint, threadID: thread, confirmationID: confirmation)
        case let .renameAgentSession(owner, endpoint, thread, name):
            return .renameAgentSession(ownerHostID: owner, endpointID: endpoint, threadID: thread, name: name)
        case let .renameSession(identity, name):
            return .renameSession(identity: identity, name: name)
        case let .killSession(identity, confirmation):
            return .killSession(identity: identity, confirmationID: confirmation)
        case .invokeProjectedAction("sessions.refresh"):
            return .refreshSessions
        default: return nil
        }
    }
}

public enum ControlCenterCommandV1: Encodable, Equatable, Sendable {
    case getProjection
    case sessionEditor(SessionEditorRequestV1)
    case submitIntent(ControlCenterHostIntentV1)

    private enum CodingKeys: String, CodingKey {
        case type
        case intent
        case request
        case candidate
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .getProjection:
            try container.encode("get_projection", forKey: .type)
        case .sessionEditor(let request):
            try container.encode("session_editor", forKey: .type)
            try container.encode(request, forKey: .request)
        case .submitIntent(let intent):
            try container.encode("submit_intent", forKey: .type)
            try container.encode(intent, forKey: .intent)

        }
    }
}

public struct ControlCenterRequestEnvelopeV1:
    Encodable, Equatable, Sendable
{
    public let protocolVersion: UInt32
    public let api = "control_center_v1"
    public let requestID: String
    public let frontendSessionID: String
    public let expectedControlRevision: UInt64?
    public let command: ControlCenterCommandV1

    public init(
        requestID: String,
        frontendSessionID: String,
        expectedControlRevision: UInt64?,
        command: ControlCenterCommandV1
    ) {
        protocolVersion = polarTmuxProtocolVersion
        self.requestID = requestID
        self.frontendSessionID = frontendSessionID
        self.expectedControlRevision = expectedControlRevision
        self.command = command
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case api
        case requestID = "request_id"
        case frontendSessionID = "frontend_session_id"
        case expectedControlRevision =
            "expected_control_revision"
        case command
    }
}

public struct AgentSessionResumeV1: Codable, Equatable, Sendable {
    public let execution_target_id: String
    public let endpoint_id: String
    public let thread_id: String
}

public struct ControlCenterErrorV1:
    Decodable, Equatable, Error, Sendable
{
    public let code: String
    public let message: String
    public var resume: AgentSessionResumeV1? = nil
}

extension ControlCenterErrorV1: LocalizedError {
    public var errorDescription: String? { message }
}

public struct SessionCreationItemV1: Decodable, Equatable, Sendable {
    public let thread_id: String
    public let session: SessionRowProjectionV1.Identity?
    public let error: ControlCenterErrorV1?
}
public enum ControlCenterResultV1: Decodable, Equatable, Sendable {
    case sessionEditor(SessionEditorResultV1)
    case projection(ControlCenterProjectionV1)
    case intentAccepted(ControlCenterProjectionV1)
    case sessionCreated(ControlCenterProjectionV1, SessionRowProjectionV1.Identity)
    case sessionBatchCreated(ControlCenterProjectionV1, [SessionCreationItemV1])
    case error(
        error: ControlCenterErrorV1,
        projection: ControlCenterProjectionV1?
    )

    private enum CodingKeys: String, CodingKey {
        case result
        case items
        case type
        case session
        case projection
        case structure
        case current
        case error
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .type) {
        case "session_editor":
            self = try .sessionEditor(container.decode(SessionEditorResultV1.self, forKey: .result))
        case "projection":
            self = try .projection(
                container.decode(
                    ControlCenterProjectionV1.self,
                    forKey: .projection
                )
            )
        case "session_batch_created":
            self = try .sessionBatchCreated(container.decode(ControlCenterProjectionV1.self, forKey: .projection), container.decode([SessionCreationItemV1].self, forKey: .items))
        case "session_created":
            self = try .sessionCreated(container.decode(ControlCenterProjectionV1.self, forKey: .projection), container.decode(SessionRowProjectionV1.Identity.self, forKey: .session))
        case "intent_accepted":
            self = try .intentAccepted(
                container.decode(
                    ControlCenterProjectionV1.self,
                    forKey: .projection
                )
            )
        case "error":
            self = try .error(
                error: container.decode(
                    ControlCenterErrorV1.self,
                    forKey: .error
                ),
                projection: container.decodeIfPresent(
                    ControlCenterProjectionV1.self,
                    forKey: .projection
                )
            )
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .type,
                in: container,
                debugDescription:
                    "unknown Control Center result type"
            )
        }
    }
}

public struct ControlCenterResponseEnvelopeV1:
    Decodable, Equatable, Sendable
{
    public let protocolVersion: UInt32
    public let api: String
    public let requestID: String
    public let controlRevision: UInt64
    public let result: ControlCenterResultV1

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case api
        case requestID = "request_id"
        case controlRevision = "control_revision"
        case result
    }
}
