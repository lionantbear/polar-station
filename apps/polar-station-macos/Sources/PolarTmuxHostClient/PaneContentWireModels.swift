import CPolarStationDesktopClient
import Foundation

public enum PolarTmuxPaneContentAPI:
    String,
    Codable,
    Equatable,
    Sendable
{
    case paneContentV1 = "pane_content_v1"
}

public struct PolarTmuxPaneContentBinding:
    Codable,
    Equatable,
    Sendable
{
    public let executionTarget: String
    public let tmuxSessionID: String

    public init(
        executionTarget: String,
        tmuxSessionID: String
    ) {
        self.executionTarget = executionTarget
        self.tmuxSessionID = tmuxSessionID
    }

    enum CodingKeys: String, CodingKey {
        case executionTarget = "execution_target"
        case tmuxSessionID = "tmux_session_id"
    }
}

public enum PolarTmuxPaneContentCommand:
    Codable,
    Equatable,
    Sendable
{
    case observeRecovery, unobserveRecovery, probeRecovery, dismissRecovery
    case focus(focused: Bool, sequence: UInt64)
    case terminalControl(nativeInstanceID: String, focused: Bool, sequence: UInt64, cause: PolarStationTerminalControlCause)
    case probeTerminalControl
    case bind(PolarTmuxPaneContentBinding)
    case detach
    case retire
    case reconnect(PolarTmuxPaneContentBinding)

    private enum Operation: String, Codable {
        case unobserveRecovery = "unobserve_recovery"
        case observeRecovery = "observe_recovery"
        case probeRecovery = "probe_recovery"
        case dismissRecovery = "dismiss_recovery"
        case focus
        case terminalControl = "terminal_control"
        case probeTerminalControl = "probe_terminal_control"
        case bind
        case detach
        case retire
        case reconnect
    }

    private enum CodingKeys: String, CodingKey {
        case operation
        case binding
        case focused, sequence, cause
        case nativeInstanceID = "native_instance_id"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(
            keyedBy: CodingKeys.self
        )
        switch try container.decode(
            Operation.self,
            forKey: .operation
        ) {
        case .unobserveRecovery: self = .unobserveRecovery
        case .observeRecovery: self = .observeRecovery
        case .probeRecovery: self = .probeRecovery
        case .dismissRecovery: self = .dismissRecovery
        case .probeTerminalControl: self = .probeTerminalControl
        case .terminalControl:
            self = .terminalControl(nativeInstanceID: try container.decode(String.self, forKey: .nativeInstanceID), focused: try container.decode(Bool.self, forKey: .focused), sequence: try container.decode(UInt64.self, forKey: .sequence), cause: try container.decode(PolarStationTerminalControlCause.self, forKey: .cause))
        case .focus: self = .focus(focused: try container.decode(Bool.self, forKey: .focused), sequence: try container.decode(UInt64.self, forKey: .sequence))
        case .bind:
            self = .bind(try container.decode(
                PolarTmuxPaneContentBinding.self,
                forKey: .binding
            ))
        case .detach:
            self = .detach
        case .retire:
            self = .retire
        case .reconnect:
            self = .reconnect(try container.decode(
                PolarTmuxPaneContentBinding.self,
                forKey: .binding
            ))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(
            keyedBy: CodingKeys.self
        )
        switch self {
        case .unobserveRecovery: try container.encode(Operation.unobserveRecovery, forKey: .operation)
        case .observeRecovery: try container.encode(Operation.observeRecovery, forKey: .operation)
        case .probeRecovery: try container.encode(Operation.probeRecovery, forKey: .operation)
        case .dismissRecovery: try container.encode(Operation.dismissRecovery, forKey: .operation)
        case .probeTerminalControl: try container.encode(Operation.probeTerminalControl, forKey: .operation)
        case .terminalControl(let nativeInstanceID, let focused, let sequence, let cause):
            try container.encode(Operation.terminalControl, forKey: .operation)
            try container.encode(nativeInstanceID, forKey: .nativeInstanceID)
            try container.encode(focused, forKey: .focused)
            try container.encode(sequence, forKey: .sequence)
            try container.encode(cause, forKey: .cause)
        case .focus(let focused, let sequence):
            try container.encode(Operation.focus, forKey: .operation)
            try container.encode(focused, forKey: .focused)
            try container.encode(sequence, forKey: .sequence)
        case .bind(let binding):
            try container.encode(
                Operation.bind,
                forKey: .operation
            )
            try container.encode(binding, forKey: .binding)
        case .detach:
            try container.encode(
                Operation.detach,
                forKey: .operation
            )
        case .retire:
            try container.encode(
                Operation.retire,
                forKey: .operation
            )
        case .reconnect(let binding):
            try container.encode(
                Operation.reconnect,
                forKey: .operation
            )
            try container.encode(binding, forKey: .binding)
        }
    }
}

public struct PolarTmuxPaneContentRequest:
    Codable,
    Equatable,
    Sendable
{
    public let protocolVersion: UInt32
    public let api: PolarTmuxPaneContentAPI
    public let requestID: String
    public let channelID: String
    public let channelGeneration: UInt64
    public let contentRevision: UInt64
    public let command: PolarTmuxPaneContentCommand

    public init(
        requestID: String,
        channelID: String,
        channelGeneration: UInt64,
        contentRevision: UInt64,
        command: PolarTmuxPaneContentCommand
    ) {
        protocolVersion = polarTmuxProtocolVersion
        api = .paneContentV1
        self.requestID = requestID
        self.channelID = channelID
        self.channelGeneration = channelGeneration
        self.contentRevision = contentRevision
        self.command = command
    }

    public static func bind(
        requestID: String,
        channelID: String,
        channelGeneration: UInt64,
        contentRevision: UInt64,
        executionTarget: String,
        tmuxSessionID: String
    ) -> Self {
        .init(
            requestID: requestID,
            channelID: channelID,
            channelGeneration: channelGeneration,
            contentRevision: contentRevision,
            command: .bind(.init(
                executionTarget: executionTarget,
                tmuxSessionID: tmuxSessionID
            ))
        )
    }

    public static func detach(
        requestID: String,
        channelID: String,
        channelGeneration: UInt64,
        contentRevision: UInt64
    ) -> Self {
        .init(
            requestID: requestID,
            channelID: channelID,
            channelGeneration: channelGeneration,
            contentRevision: contentRevision,
            command: .detach
        )
    }

    public static func retire(
        requestID: String,
        channelID: String,
        channelGeneration: UInt64,
        contentRevision: UInt64
    ) -> Self {
        .init(
            requestID: requestID,
            channelID: channelID,
            channelGeneration: channelGeneration,
            contentRevision: contentRevision,
            command: .retire
        )
    }

    public static func reconnect(
        requestID: String,
        channelID: String,
        channelGeneration: UInt64,
        contentRevision: UInt64,
        executionTarget: String,
        tmuxSessionID: String
    ) -> Self {
        .init(
            requestID: requestID,
            channelID: channelID,
            channelGeneration: channelGeneration,
            contentRevision: contentRevision,
            command: .reconnect(.init(
                executionTarget: executionTarget,
                tmuxSessionID: tmuxSessionID
            ))
        )
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case api
        case requestID = "request_id"
        case channelID = "channel_id"
        case channelGeneration = "channel_generation"
        case contentRevision = "content_revision"
        case command
    }
}

public enum PolarTmuxPaneContentResult:
    Codable,
    Equatable,
    Sendable
{
    case recovery(PolarStationTerminalRecoveryView)
    case terminalControl(PolarStationTerminalControlStatus)
    case accepted(PolarTmuxPaneContentBinding)
    case ready(PolarTmuxPaneContentBinding)
    case detached
    case rejected(PolarTmuxProtocolError)

    public var bindingAccepted: Bool {
        switch self { case .accepted, .ready: return true; default: return false }
    }

    private enum Status: String, Codable {
        case recovery
        case terminalControl = "terminal_control"
        case accepted
        case ready
        case detached
        case rejected
    }

    private enum CodingKeys: String, CodingKey {
        case status
        case view
        case state
        case binding
        case focused, sequence
        case error
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(
            keyedBy: CodingKeys.self
        )
        switch try container.decode(Status.self, forKey: .status) {
        case .terminalControl: self = .terminalControl(try container.decode(PolarStationTerminalControlStatus.self, forKey: .state))
        case .recovery: self = .recovery(try container.decode(PolarStationTerminalRecoveryView.self, forKey: .view))
        case .accepted:
            self = .accepted(try container.decode(PolarTmuxPaneContentBinding.self, forKey: .binding))
        case .ready:
            self = .ready(try container.decode(
                PolarTmuxPaneContentBinding.self,
                forKey: .binding
            ))
        case .detached:
            self = .detached
        case .rejected:
            self = .rejected(try container.decode(
                PolarTmuxProtocolError.self,
                forKey: .error
            ))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(
            keyedBy: CodingKeys.self
        )
        switch self {
        case .terminalControl(let state):
            try container.encode(Status.terminalControl, forKey: .status)
            try container.encode(state, forKey: .state)
        case .recovery(let view):
            try container.encode(Status.recovery, forKey: .status)
            try container.encode(view, forKey: .view)
        case .accepted(let binding):
            try container.encode(Status.accepted, forKey: .status)
            try container.encode(binding, forKey: .binding)
        case .ready(let binding):
            try container.encode(Status.ready, forKey: .status)
            try container.encode(binding, forKey: .binding)
        case .detached:
            try container.encode(Status.detached, forKey: .status)
        case .rejected(let error):
            try container.encode(Status.rejected, forKey: .status)
            try container.encode(error, forKey: .error)
        }
    }
}

public struct PolarTmuxPaneContentResponse:
    Codable,
    Equatable,
    Sendable
{
    public let protocolVersion: UInt32
    public let api: PolarTmuxPaneContentAPI
    public let requestID: String
    public let channelID: String
    public let channelGeneration: UInt64
    public let contentRevision: UInt64
    public let result: PolarTmuxPaneContentResult

    public init(
        requestID: String,
        channelID: String,
        channelGeneration: UInt64,
        contentRevision: UInt64,
        result: PolarTmuxPaneContentResult
    ) {
        protocolVersion = polarTmuxProtocolVersion
        api = .paneContentV1
        self.requestID = requestID
        self.channelID = channelID
        self.channelGeneration = channelGeneration
        self.contentRevision = contentRevision
        self.result = result
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case api
        case requestID = "request_id"
        case channelID = "channel_id"
        case channelGeneration = "channel_generation"
        case contentRevision = "content_revision"
        case result
    }
}

public enum PolarTmuxPaneContentError:
    Error,
    Equatable,
    LocalizedError,
    Sendable
{
    case invalidRequest
    case responseIdentityMismatch

    public var errorDescription: String? {
        switch self {
        case .invalidRequest:
            "Polar Station rejected an invalid Pane content request"
        case .responseIdentityMismatch:
            "Polar Station ignored a Pane content response for another Surface"
        }
    }
}

public struct PolarStationTerminalRecoveryView: Codable, Equatable, Sendable {
    public static func unavailable() -> Self? {
        guard let json = polar_station_terminal_status_unavailable_json() else { return nil }
        defer { polar_station_desktop_string_free(json) }
        return try? JSONDecoder().decode(Self.self, from: Data(String(cString: json).utf8))
    }
    public let visible: Bool
    public let phase: String
    public let message: String
    public let detail: String
    public let retryEnabled: Bool
    public let inputWarning: Bool
    public let lastProbeUnixMs: UInt64?
    public let retryLabel: String
    public let detailsLabel: String
    public let dismissLabel: String
    public let inputWarningMessage: String
    public let probeTimeLabel: String
    enum CodingKeys: String, CodingKey {
        case visible, phase, message, detail
        case retryEnabled = "retry_enabled"
        case inputWarning = "input_warning"
        case lastProbeUnixMs = "last_probe_unix_ms"
        case retryLabel = "retry_label"
        case detailsLabel = "details_label"
        case dismissLabel = "dismiss_label"
        case inputWarningMessage = "input_warning_message"
        case probeTimeLabel = "probe_time_label"
    }
}

public extension Notification.Name {
    static let polarStationTerminalRecovery = Notification.Name("PolarStationTerminalRecovery")
}


public enum PolarStationTerminalControlCause: String, Codable, Equatable, Sendable {
    case selection, viewport, recovery, blur
}

public enum PolarStationTerminalControlPhase: String, Codable, Equatable, Sendable {
    case deferred, requested, applied, observed, superseded, failed
}

/// Host facts; Observed refers to tmux geometry, never program TTY or paint.
public struct PolarStationTerminalControlStatus: Codable, Equatable, Sendable {
    public let sequence: UInt64
    public let bindingGeneration: UInt64
    public let viewportRevision: UInt64
    public let phase: PolarStationTerminalControlPhase
    public let geometry: PolarStationTerminalGeometry?
    enum CodingKeys: String, CodingKey {
        case sequence, phase, geometry
        case bindingGeneration = "binding_generation"
        case viewportRevision = "viewport_revision"
    }
}
public struct PolarStationTerminalGeometry: Codable, Equatable, Sendable {
    public let windowID: String
    public let windowColumns: UInt16
    public let windowRows: UInt16
    public let clientColumns: UInt16
    public let clientRows: UInt16
    public let statusLines: UInt16
    public let windowPolicy: String
    enum CodingKeys: String, CodingKey {
        case windowID = "window_id"
        case windowColumns = "window_columns"
        case windowRows = "window_rows"
        case clientColumns = "client_columns"
        case clientRows = "client_rows"
        case statusLines = "status_lines"
        case windowPolicy = "window_policy"
    }
}
