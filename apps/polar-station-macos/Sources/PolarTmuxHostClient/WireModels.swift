import Foundation

public let polarTmuxProtocolVersion: UInt32 = 1
public let polarTmuxRefreshSnapshotCapability =
    "refresh_snapshot_v1"
public let polarTmuxApplicationStateEventsCapability =
    "application_state_events_v1"
public let polarTmuxWorkspaceRelayCapability =
    "workspace_relay_v1"

public enum PolarTmuxApplicationStateAPI:
    String,
    Codable,
    Equatable,
    Hashable,
    Sendable
{
    case controlCenterV1 = "control_center_v1"
    case inboxV1 = "inbox_v1"
    case resourcesV1 = "resources_v1"
    case settingsV1 = "settings_v1"
    case notificationV1 = "notification_v1"
    case codexV1 = "codex_v1"
}
public struct PolarTmuxApplicationStateChange:
    Equatable,
    Sendable
{
    public let api: PolarTmuxApplicationStateAPI
    public let stateRevision: UInt64

    public init(
        api: PolarTmuxApplicationStateAPI,
        stateRevision: UInt64
    ) {
        self.api = api
        self.stateRevision = stateRevision
    }
}

public enum PolarTmuxJSONValue: Codable, Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([Self])
    case object([String: Self])

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([Self].self) {
            self = .array(value)
        } else {
            self = .object(try container.decode([String: Self].self))
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null:
            try container.encodeNil()
        case .bool(let value):
            try container.encode(value)
        case .number(let value):
            try container.encode(value)
        case .string(let value):
            try container.encode(value)
        case .array(let value):
            try container.encode(value)
        case .object(let value):
            try container.encode(value)
        }
    }
}

public struct PolarTmuxHostCapabilities: Codable, Equatable, Sendable {
    public var tmuxRead: Bool
    public var tmuxWrite: Bool
    public var frontendControl: Bool
    public var remoteTmux: Bool
    public var wslTmux: Bool

    public init(
        tmuxRead: Bool = false,
        tmuxWrite: Bool = false,
        frontendControl: Bool = false,
        remoteTmux: Bool = false,
        wslTmux: Bool = false
    ) {
        self.tmuxRead = tmuxRead
        self.tmuxWrite = tmuxWrite
        self.frontendControl = frontendControl
        self.remoteTmux = remoteTmux
        self.wslTmux = wslTmux
    }

    enum CodingKeys: String, CodingKey {
        case tmuxRead = "tmux_read"
        case tmuxWrite = "tmux_write"
        case frontendControl = "frontend_control"
        case remoteTmux = "remote_tmux"
        case wslTmux = "wsl_tmux"
    }
}

public struct PolarTmuxHostDescriptor: Codable, Equatable, Sendable {
    public let protocolVersion: UInt32
    public let hostID: String
    public let hostName: String
    public let platform: String
    public let hostVersion: String
    public let capabilities: PolarTmuxHostCapabilities

    public init(
        protocolVersion: UInt32,
        hostID: String,
        hostName: String,
        platform: String,
        hostVersion: String,
        capabilities: PolarTmuxHostCapabilities = .init()
    ) {
        self.protocolVersion = protocolVersion
        self.hostID = hostID
        self.hostName = hostName
        self.platform = platform
        self.hostVersion = hostVersion
        self.capabilities = capabilities
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case hostID = "host_id"
        case hostName = "host_name"
        case platform
        case hostVersion = "host_version"
        case capabilities
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        protocolVersion = try container.decode(UInt32.self, forKey: .protocolVersion)
        hostID = try container.decode(String.self, forKey: .hostID)
        hostName = try container.decode(String.self, forKey: .hostName)
        platform = try container.decode(String.self, forKey: .platform)
        hostVersion = try container.decode(String.self, forKey: .hostVersion)
        capabilities = try container.decodeIfPresent(
            PolarTmuxHostCapabilities.self,
            forKey: .capabilities
        ) ?? .init()
    }
}

public enum PolarTmuxExecutionTargetKind: String, Codable, Equatable, Sendable {
    case local
    case ssh
    case wsl
}

public struct PolarTmuxExecutionTargetDescriptor: Codable, Equatable, Identifiable, Sendable {
    public let id: String
    public let displayName: String
    public let kind: PolarTmuxExecutionTargetKind

    public init(
        id: String,
        displayName: String,
        kind: PolarTmuxExecutionTargetKind
    ) {
        self.id = id
        self.displayName = displayName
        self.kind = kind
    }

    enum CodingKeys: String, CodingKey {
        case id
        case displayName = "display_name"
        case kind
    }
}

public struct PolarTmuxAttachPlan: Codable, Equatable, Sendable {
    public let program: String
    public let arguments: [String]

    public init(program: String, arguments: [String]) {
        self.program = program
        self.arguments = arguments
    }

    enum CodingKeys: String, CodingKey {
        case program
        case arguments = "args"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(
            keyedBy: CodingKeys.self
        )
        program = try container.decode(
            String.self,
            forKey: .program
        )
        arguments = try container.decode(
            [String].self,
            forKey: .arguments
        )
        do {
            try validate()
        } catch {
            throw DecodingError.dataCorrupted(
                .init(
                    codingPath: decoder.codingPath,
                    debugDescription:
                        "attach plan violates the exact attach grammar"
                )
            )
        }
    }

    public func validate() throws {
        switch program {
        case "tmux":
            guard Self.isExactTmuxAttach(arguments)
            else { throw Self.invalidPlan }
        case "ssh":
            guard let payload = Self.sshPayload(arguments),
                  Self.isSafeSSHDestination(payload.destination),
                  Self.remoteTmuxAttachArguments(
                        payload.remoteCommand
                    ) != nil
            else { throw Self.invalidPlan }
        default:
            throw Self.invalidPlan
        }
    }

    /// Returns the stable tmux session ID carried by the already validated
    /// local or restricted SSH attach grammar.
    public func attachedSessionID() throws -> String {
        try validate()
        switch program {
        case "tmux":
            guard let sessionID = arguments.last else {
                throw Self.invalidPlan
            }
            return sessionID
        case "ssh":
            guard let payload = Self.sshPayload(arguments),
                  let remoteArguments =
                    Self.remoteTmuxAttachArguments(
                        payload.remoteCommand
                    ),
                  let sessionID = remoteArguments.last
            else {
                throw Self.invalidPlan
            }
            return sessionID
        default:
            throw Self.invalidPlan
        }
    }

    private static var invalidPlan:
        PolarTmuxAttachPlanValidationError
    {
        .invalidPlan
    }

    private static func isExactTmuxAttach(
        _ arguments: [String]
    ) -> Bool {
        if arguments.count == 3 {
            return arguments[0] == "attach-session"
                && arguments[1] == "-t"
                && isStableSessionID(arguments[2])
        }
        if arguments.count == 5 {
            return arguments[0] == "-L"
                && isSafeSocketName(arguments[1])
                && arguments[2] == "attach-session"
                && arguments[3] == "-t"
                && isStableSessionID(arguments[4])
        }
        return false
    }

    /// Accept the legacy direct SSH attach shape and the current Rust Host
    /// fail-closed ControlMaster shape. Both still carry exactly one inert,
    /// restricted tmux command; native code never evaluates shell text.
    private static func sshPayload(
        _ arguments: [String]
    ) -> (destination: String, remoteCommand: String)? {
        if arguments.count == 3,
           arguments[0] == "-tt"
        {
            return (arguments[1], arguments[2])
        }
        guard arguments.count == 7,
              arguments[0] == "-S",
              isCanonicalSSHControlSocket(arguments[1]),
              arguments[2] == "-o",
              arguments[3] == "ProxyCommand=/usr/bin/false",
              arguments[4] == "-tt"
        else {
            return nil
        }
        return (arguments[5], arguments[6])
    }

    private static func isStableSessionID(
        _ value: String
    ) -> Bool {
        let bytes = Array(value.utf8)
        return bytes.count >= 2
            && bytes[0] == 0x24
            && bytes.dropFirst().allSatisfy {
                (0x30...0x39).contains($0)
            }
    }

    private static func isSafeSocketName(
        _ value: String
    ) -> Bool {
        let bytes = Array(value.utf8)
        guard (1...96).contains(bytes.count),
              let first = bytes.first,
              isASCIIAlphaNumeric(first)
        else { return false }
        return bytes.allSatisfy {
            isASCIIAlphaNumeric($0)
                || $0 == 0x2e
                || $0 == 0x5f
                || $0 == 0x2d
        }
    }

    private static func isCanonicalSSHControlSocket(
        _ value: String
    ) -> Bool {
        let bytes = Array(value.utf8)
        guard (2...103).contains(bytes.count),
              bytes.first == 0x2f,
              bytes.last != 0x2f,
              !value.contains("//"),
              !value.split(separator: "/", omittingEmptySubsequences: false)
                .contains(where: { $0 == "." || $0 == ".." })
        else {
            return false
        }
        return bytes.allSatisfy {
            isASCIIAlphaNumeric($0)
                || $0 == 0x2f
                || $0 == 0x2e
                || $0 == 0x5f
                || $0 == 0x2d
        }
    }

    private static func isSafeSSHDestination(
        _ value: String
    ) -> Bool {
        let bytes = Array(value.utf8)
        guard (1...255).contains(bytes.count),
              bytes.first != 0x2d,
              bytes.contains(where: isASCIIAlphaNumeric)
        else { return false }
        let punctuation = Set("._-@:%[]+".utf8)
        return bytes.allSatisfy {
            isASCIIAlphaNumeric($0)
                || punctuation.contains($0)
        }
    }

    private static func isASCIIAlphaNumeric(
        _ byte: UInt8
    ) -> Bool {
        (0x30...0x39).contains(byte)
            || (0x41...0x5a).contains(byte)
            || (0x61...0x7a).contains(byte)
    }

    private static let remoteTmuxPathAssignment =
        "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"

    /// Project the Rust protocol's exact remote attach grammar. The only
    /// accepted environment prefix is the fixed PATH assignment published by
    /// `REMOTE_TMUX_PATH_ASSIGNMENT`; arbitrary assignments remain rejected.
    private static func remoteTmuxAttachArguments(
        _ command: String
    ) -> [String]? {
        guard let words = parseRestrictedRemoteCommand(command)
        else { return nil }
        let commandWords =
            words.first == remoteTmuxPathAssignment
                ? Array(words.dropFirst())
                : words
        guard commandWords.first == "tmux" else { return nil }
        let arguments = Array(commandWords.dropFirst())
        return isExactTmuxAttach(arguments) ? arguments : nil
    }

    /// Parse only the inert word forms emitted by Rust's `quote_posix`:
    /// ASCII bare words or whole single-quoted words, separated by spaces.
    /// Shell operators, expansions, escapes, concatenation, comments, and
    /// double quotes are rejected rather than interpreted.
    private static func parseRestrictedRemoteCommand(
        _ command: String
    ) -> [String]? {
        guard !command.isEmpty,
              command.first != " ",
              command.last != " "
        else { return nil }

        var words: [String] = []
        var index = command.startIndex
        while index < command.endIndex {
            var word = ""
            if command[index] == "'" {
                index = command.index(after: index)
                while index < command.endIndex,
                      command[index] != "'"
                {
                    let character = command[index]
                    guard !character.isASCIIControl
                    else { return nil }
                    word.append(character)
                    index = command.index(after: index)
                }
                guard index < command.endIndex,
                      command[index] == "'"
                else { return nil }
                index = command.index(after: index)
            } else {
                while index < command.endIndex,
                      command[index] != " "
                {
                    let character = command[index]
                    guard character.isRestrictedBareWordCharacter
                    else { return nil }
                    word.append(character)
                    index = command.index(after: index)
                }
            }
            guard !word.isEmpty else { return nil }
            words.append(word)
            guard index < command.endIndex else { break }
            guard command[index] == " " else { return nil }
            repeat {
                index = command.index(after: index)
            } while index < command.endIndex
                && command[index] == " "
            guard index < command.endIndex else { return nil }
        }
        return words
    }
}

private extension Character {
    var isASCIIControl: Bool {
        guard unicodeScalars.count == 1,
              let value = unicodeScalars.first?.value
        else { return true }
        return value < 0x20 || value == 0x7f
    }

    var isRestrictedBareWordCharacter: Bool {
        guard unicodeScalars.count == 1,
              let value = unicodeScalars.first?.value,
              value < 0x80
        else { return false }
        let byte = UInt8(value)
        return (0x30...0x39).contains(byte)
            || (0x41...0x5a).contains(byte)
            || (0x61...0x7a).contains(byte)
            || Set("_@%+=:,./-".utf8).contains(byte)
    }
}

public enum PolarTmuxAttachPlanValidationError:
    Error,
    Equatable,
    Sendable
{
    case invalidPlan
}

public struct PolarTmuxPreparedAttach: Equatable, Sendable {
    public let plan: PolarTmuxAttachPlan
    public let executionTarget: String
    public let revision: UInt64

    public init(
        plan: PolarTmuxAttachPlan,
        executionTarget: String,
        revision: UInt64
    ) {
        self.plan = plan
        self.executionTarget = executionTarget
        self.revision = revision
    }
}

public struct PolarTmuxSessionDescriptor:
    Codable,
    Equatable,
    Identifiable,
    Sendable
{
    public let id: String
    public let name: String
    public let windowCount: UInt32
    public let attached: Bool
    public let createdUnixSeconds: Int64?

    public init(
        id: String,
        name: String,
        windowCount: UInt32,
        attached: Bool,
        createdUnixSeconds: Int64? = nil
    ) {
        self.id = id
        self.name = name
        self.windowCount = windowCount
        self.attached = attached
        self.createdUnixSeconds = createdUnixSeconds
    }

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case windowCount = "window_count"
        case attached
        case createdUnixSeconds = "created_unix_seconds"
    }
}

public struct PolarTmuxSessionSnapshot:
    Codable,
    Equatable,
    Sendable
{
    public let sessions: [PolarTmuxSessionDescriptor]
    public let windows: [PolarTmuxWindowDescriptor]
    public let panes: [PolarTmuxPaneDescriptor]

    public init(
        sessions: [PolarTmuxSessionDescriptor],
        windows: [PolarTmuxWindowDescriptor] = [],
        panes: [PolarTmuxPaneDescriptor] = []
    ) {
        self.sessions = sessions
        self.windows = windows
        self.panes = panes
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(
            keyedBy: CodingKeys.self
        )
        sessions = try container.decode(
            [PolarTmuxSessionDescriptor].self,
            forKey: .sessions
        )
        windows = try container.decodeIfPresent(
            [PolarTmuxWindowDescriptor].self,
            forKey: .windows
        ) ?? []
        panes = try container.decodeIfPresent(
            [PolarTmuxPaneDescriptor].self,
            forKey: .panes
        ) ?? []
    }

    public func sessionID(forPane paneID: String)
        -> String?
    {
        guard let windowID = panes.first(
            where: { $0.id == paneID }
        )?.windowID
        else { return nil }
        return windows.first {
            $0.id == windowID
        }?.sessionID
    }

    private enum CodingKeys: String, CodingKey {
        case sessions
        case windows
        case panes
    }
}

public struct PolarTmuxWindowDescriptor:
    Codable,
    Equatable,
    Identifiable,
    Sendable
{
    public let id: String
    public let sessionID: String
    public let index: UInt32
    public let name: String
    public let active: Bool
    public let paneCount: UInt32
    public let layout: String

    public init(
        id: String,
        sessionID: String,
        index: UInt32,
        name: String,
        active: Bool,
        paneCount: UInt32,
        layout: String
    ) {
        self.id = id
        self.sessionID = sessionID
        self.index = index
        self.name = name
        self.active = active
        self.paneCount = paneCount
        self.layout = layout
    }

    enum CodingKeys: String, CodingKey {
        case id
        case sessionID = "session_id"
        case index
        case name
        case active
        case paneCount = "pane_count"
        case layout
    }
}

public struct PolarTmuxPaneDescriptor:
    Codable,
    Equatable,
    Identifiable,
    Sendable
{
    public let id: String
    public let windowID: String
    public let index: UInt32
    public let title: String
    public let active: Bool
    public let dead: Bool

    public init(
        id: String,
        windowID: String,
        index: UInt32,
        title: String,
        active: Bool,
        dead: Bool
    ) {
        self.id = id
        self.windowID = windowID
        self.index = index
        self.title = title
        self.active = active
        self.dead = dead
    }

    enum CodingKeys: String, CodingKey {
        case id
        case windowID = "window_id"
        case index
        case title
        case active
        case dead
    }
}

public protocol PolarTmuxAttachPlanProviding: Sendable {
    func prepareAttach(
        executionTarget: String,
        sessionID: String,
        expectedRevision: UInt64?
    ) async throws -> PolarTmuxPreparedAttach
}

public struct PolarTmuxRevisionedSnapshot: Codable, Equatable, Sendable {
    public let executionTarget: String
    public let revision: UInt64
    public let snapshot: PolarTmuxJSONValue

    public init(
        executionTarget: String,
        revision: UInt64,
        snapshot: PolarTmuxJSONValue
    ) {
        self.executionTarget = executionTarget
        self.revision = revision
        self.snapshot = snapshot
    }

    enum CodingKeys: String, CodingKey {
        case executionTarget = "execution_target"
        case revision
        case snapshot
    }
}

public struct PolarTmuxClientEnvelope: Codable, Equatable, Sendable {
    public let protocolVersion: UInt32
    public let requestID: String
    public let executionTarget: String
    public let expectedRevision: UInt64?
    public let command: PolarTmuxJSONValue

    public init(
        protocolVersion: UInt32 = polarTmuxProtocolVersion,
        requestID: String,
        executionTarget: String = "local",
        expectedRevision: UInt64? = nil,
        command: PolarTmuxJSONValue
    ) {
        self.protocolVersion = protocolVersion
        self.requestID = requestID
        self.executionTarget = executionTarget
        self.expectedRevision = expectedRevision
        self.command = command
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case requestID = "request_id"
        case executionTarget = "execution_target"
        case expectedRevision = "expected_revision"
        case command
    }

    public static func listExecutionTargets(
        requestID: String
    ) -> PolarTmuxClientEnvelope {
        .init(
            requestID: requestID,
            command: .object(["type": .string("list_execution_targets")])
        )
    }

    public static func getSnapshot(
        requestID: String,
        executionTarget: String
    ) -> PolarTmuxClientEnvelope {
        .init(
            requestID: requestID,
            executionTarget: executionTarget,
            command: .object(["type": .string("get_snapshot")])
        )
    }
}

public struct PolarTmuxHostEnvelope: Codable, Equatable, Sendable {
    public let protocolVersion: UInt32
    public let requestID: String
    public let executionTarget: String
    public let revision: UInt64
    public let result: PolarTmuxJSONValue

    public init(
        protocolVersion: UInt32,
        requestID: String,
        executionTarget: String,
        revision: UInt64,
        result: PolarTmuxJSONValue
    ) {
        self.protocolVersion = protocolVersion
        self.requestID = requestID
        self.executionTarget = executionTarget
        self.revision = revision
        self.result = result
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case requestID = "request_id"
        case executionTarget = "execution_target"
        case revision
        case result
    }

    public var resultType: String? {
        guard case .object(let object) = result,
              case .string(let type) = object["type"]
        else { return nil }
        return type
    }

    public func decodeExecutionTargets() throws
        -> [PolarTmuxExecutionTargetDescriptor]
    {
        let payload = try decodeResult(
            PolarTmuxExecutionTargetsPayload.self
        )
        guard payload.type == "execution_targets" else {
            throw PolarTmuxHostResultError.unexpectedType(payload.type)
        }
        return payload.targets
    }

    public func decodeAttachPlan() throws -> PolarTmuxAttachPlan {
        let payload = try decodeResult(PolarTmuxAttachPlanPayload.self)
        guard payload.type == "attach_plan" else {
            throw PolarTmuxHostResultError.unexpectedType(payload.type)
        }
        try payload.plan.validate()
        return payload.plan
    }

    public func decodeCompanionPairing() throws
        -> CompanionPairingPayloadV1
    {
        let payload = try decodeResult(
            CompanionPairingHostResult.self
        )
        guard payload.type == "companion_pairing" else {
            throw PolarTmuxHostResultError
                .unexpectedType(payload.type)
        }
        try payload.pairing.validate()
        return payload.pairing
    }

    public func decodeSessionSnapshot() throws
        -> PolarTmuxSessionSnapshot
    {
        let payload = try decodeResult(
            PolarTmuxSessionSnapshotPayload.self
        )
        guard [
            "snapshot",
            "mobile_snapshot",
            "sanitized_snapshot",
            "frontend_completed",
        ].contains(payload.type)
        else {
            throw PolarTmuxHostResultError
                .unexpectedType(payload.type)
        }
        return payload.snapshot
    }

    private func decodeResult<Value: Decodable>(
        _ type: Value.Type
    ) throws -> Value {
        do {
            return try JSONDecoder().decode(
                type,
                from: JSONEncoder().encode(result)
            )
        } catch let error as PolarTmuxHostResultError {
            throw error
        } catch {
            throw PolarTmuxHostResultError.invalidPayload
        }
    }
}

public enum PolarTmuxHostResultError:
    Error,
    Equatable,
    LocalizedError,
    Sendable
{
    case invalidPayload
    case unexpectedType(String)

    public var errorDescription: String? {
        switch self {
        case .invalidPayload:
            "The Polar Tmux Host returned an invalid result payload"
        case .unexpectedType(let type):
            "The Polar Tmux Host returned unexpected result type \(type)"
        }
    }
}

private struct PolarTmuxExecutionTargetsPayload: Decodable {
    let type: String
    let targets: [PolarTmuxExecutionTargetDescriptor]
}

private struct PolarTmuxAttachPlanPayload: Decodable {
    let type: String
    let plan: PolarTmuxAttachPlan
}

public struct CompanionPairingPayloadV1:
    Codable,
    Equatable,
    Sendable
{
    public let schema: String
    public let hostID: String
    public let pairingCode: String
    public let connectedDevices: UInt32
    public let listenerReady: Bool

    public init(
        schema: String,
        hostID: String,
        pairingCode: String,
        connectedDevices: UInt32,
        listenerReady: Bool
    ) {
        self.schema = schema
        self.hostID = hostID
        self.pairingCode = pairingCode
        self.connectedDevices = connectedDevices
        self.listenerReady = listenerReady
    }

    public func validate() throws {
        guard schema == "polar-tmux-pairing-payload-v1",
              !hostID.isEmpty,
              pairingCode.hasPrefix(
                "polar-tmux://pair?v=1&host_id=\(hostID)&key="
              ),
              pairingCode.count <= 1_024,
              !pairingCode.unicodeScalars.contains(where: {
                  CharacterSet.controlCharacters.contains($0)
              })
        else {
            throw PolarTmuxHostResultError.invalidPayload
        }
    }

    enum CodingKeys: String, CodingKey {
        case schema
        case hostID = "host_id"
        case pairingCode = "pairing_code"
        case connectedDevices = "connected_devices"
        case listenerReady = "listener_ready"
    }
}

private struct CompanionPairingHostResult: Decodable {
    let type: String
    let pairing: CompanionPairingPayloadV1
}

private struct PolarTmuxSessionSnapshotPayload: Decodable {
    let type: String
    let snapshot: PolarTmuxSessionSnapshot
}

func isPrivacyPreservingFrontendSnapshot(
    _ value: PolarTmuxJSONValue
) -> Bool {
    guard case .object(let snapshot) = value,
          Set(snapshot.keys) == Set([
              "sessions",
              "windows",
              "panes",
          ])
    else { return false }
    return snapshotObjectArray(
        snapshot["sessions"],
        exactKeys: [
            "id",
            "name",
            "window_count",
            "attached",
        ]
    ) && snapshotObjectArray(
        snapshot["windows"],
        exactKeys: [
            "id",
            "session_id",
            "index",
            "name",
            "active",
            "pane_count",
            "layout",
        ]
    ) && snapshotObjectArray(
        snapshot["panes"],
        exactKeys: [
            "id",
            "window_id",
            "index",
            "title",
            "active",
            "dead",
        ]
    )
}

private func snapshotObjectArray(
    _ value: PolarTmuxJSONValue?,
    exactKeys: Set<String>
) -> Bool {
    guard case .array(let values) = value else {
        return false
    }
    return values.allSatisfy {
        guard case .object(let object) = $0 else {
            return false
        }
        return Set(object.keys) == exactKeys
    }
}

public enum PolarTmuxFrontendAction: Codable, Equatable, Sendable {
    case activate
    case openSession(session: String)
    case focusPane(pane: String)
    case setDisplayMode(mode: String)
    case refresh
    case unsupported(type: String, payload: [String: PolarTmuxJSONValue])

    private enum CodingKeys: String, CodingKey {
        case type
        case session
        case pane
        case mode
    }

    public init(from decoder: Decoder) throws {
        let raw = try PolarTmuxJSONValue(from: decoder)
        guard case .object(let object) = raw,
              case .string(let type) = object["type"] else {
            throw DecodingError.dataCorrupted(
                .init(codingPath: decoder.codingPath, debugDescription: "frontend action has no type")
            )
        }
        switch type {
        case "activate":
            self = .activate
        case "open_session":
            guard case .string(let session) = object["session"] else {
                throw DecodingError.dataCorrupted(
                    .init(codingPath: decoder.codingPath, debugDescription: "open_session has no session")
                )
            }
            self = .openSession(session: session)
        case "focus_pane":
            guard case .string(let pane) = object["pane"] else {
                throw DecodingError.dataCorrupted(
                    .init(codingPath: decoder.codingPath, debugDescription: "focus_pane has no pane")
                )
            }
            self = .focusPane(pane: pane)
        case "set_display_mode":
            guard case .string(let mode) = object["mode"] else {
                throw DecodingError.dataCorrupted(
                    .init(codingPath: decoder.codingPath, debugDescription: "set_display_mode has no mode")
                )
            }
            self = .setDisplayMode(mode: mode)
        case "refresh":
            self = .refresh
        default:
            self = .unsupported(type: type, payload: object)
        }
    }

    public func encode(to encoder: Encoder) throws {
        let value: PolarTmuxJSONValue
        switch self {
        case .activate:
            value = .object(["type": .string("activate")])
        case .openSession(let session):
            value = .object(["type": .string("open_session"), "session": .string(session)])
        case .focusPane(let pane):
            value = .object(["type": .string("focus_pane"), "pane": .string(pane)])
        case .setDisplayMode(let mode):
            value = .object(["type": .string("set_display_mode"), "mode": .string(mode)])
        case .refresh:
            value = .object(["type": .string("refresh")])
        case .unsupported(_, let payload):
            value = .object(payload)
        }
        try value.encode(to: encoder)
    }
}

public struct PolarTmuxFrontendDispatch: Equatable, Sendable {
    public let dispatchID: String
    public let executionTarget: String
    public let revision: UInt64
    public let action: PolarTmuxFrontendAction
    public let attachPlan: PolarTmuxAttachPlan?
    public let snapshot: PolarTmuxSessionSnapshot?

    public init(
        dispatchID: String,
        executionTarget: String,
        revision: UInt64,
        action: PolarTmuxFrontendAction,
        attachPlan: PolarTmuxAttachPlan? = nil,
        snapshot: PolarTmuxSessionSnapshot? = nil
    ) {
        self.dispatchID = dispatchID
        self.executionTarget = executionTarget
        self.revision = revision
        self.action = action
        self.attachPlan = attachPlan
        self.snapshot = snapshot
    }
}

public struct PolarTmuxProtocolError: Codable, Equatable, Sendable {
    public let code: String
    public let message: String
    public let retryable: Bool

    public init(code: String, message: String, retryable: Bool) {
        self.code = code
        self.message = message
        self.retryable = retryable
    }
}

public enum PolarTmuxFrontendAcknowledgement:
    Encodable,
    Equatable,
    Sendable
{
    case accepted
    case error(PolarTmuxProtocolError)

    private enum CodingKeys: String, CodingKey {
        case type
        case error
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .accepted:
            try container.encode("accepted", forKey: .type)
        case .error(let error):
            try container.encode("error", forKey: .type)
            try container.encode(error, forKey: .error)
        }
    }
}
