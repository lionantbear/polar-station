import Foundation

public struct FileMappingSpecV1: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public var name: String
    public var enabled: Bool
    public var executionTargetID: String
    public var remotePath: String
    public var localPath: String

    public init(
        id: String = UUID().uuidString.lowercased(),
        name: String,
        enabled: Bool,
        executionTargetID: String,
        remotePath: String,
        localPath: String
    ) {
        self.id = id
        self.name = name
        self.enabled = enabled
        self.executionTargetID = executionTargetID
        self.remotePath = remotePath
        self.localPath = localPath
    }

    enum CodingKeys: String, CodingKey {
        case id, name, enabled
        case executionTargetID = "execution_target_id"
        case remotePath = "remote_path"
        case localPath = "local_path"
    }
}

public struct PortForwardSpecV1: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public var name: String
    public var enabled: Bool
    public var executionTargetID: String
    public var bindAddress: String
    public var allowNonLoopback: Bool
    public var localPortStart: UInt16
    public var localPortEnd: UInt16
    public var remoteHost: String
    public var remotePortStart: UInt16

    public init(
        id: String = UUID().uuidString.lowercased(),
        name: String,
        enabled: Bool,
        executionTargetID: String,
        bindAddress: String = "127.0.0.1",
        allowNonLoopback: Bool = false,
        localPortStart: UInt16,
        localPortEnd: UInt16,
        remoteHost: String = "127.0.0.1",
        remotePortStart: UInt16
    ) {
        self.id = id
        self.name = name
        self.enabled = enabled
        self.executionTargetID = executionTargetID
        self.bindAddress = bindAddress
        self.allowNonLoopback = allowNonLoopback
        self.localPortStart = localPortStart
        self.localPortEnd = localPortEnd
        self.remoteHost = remoteHost
        self.remotePortStart = remotePortStart
    }

    enum CodingKeys: String, CodingKey {
        case id, name, enabled
        case executionTargetID = "execution_target_id"
        case bindAddress = "bind_address"
        case allowNonLoopback = "allow_non_loopback"
        case localPortStart = "local_port_start"
        case localPortEnd = "local_port_end"
        case remoteHost = "remote_host"
        case remotePortStart = "remote_port_start"
    }
}

public struct AndroidMirrorSpecV1: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public var name: String
    public var enabled: Bool
    public var executionTargetID: String
    public var bindAddress: String
    public var allowNonLoopback: Bool
    public var localADBPort: UInt16
    public var remoteADBHost: String
    public var remoteADBPort: UInt16
    public var scrcpyArguments: [String]

    public init(
        id: String = UUID().uuidString.lowercased(),
        name: String,
        enabled: Bool,
        executionTargetID: String,
        bindAddress: String = "127.0.0.1",
        allowNonLoopback: Bool = false,
        localADBPort: UInt16 = 5555,
        remoteADBHost: String = "127.0.0.1",
        remoteADBPort: UInt16 = 5555,
        scrcpyArguments: [String] = []
    ) {
        self.id = id
        self.name = name
        self.enabled = enabled
        self.executionTargetID = executionTargetID
        self.bindAddress = bindAddress
        self.allowNonLoopback = allowNonLoopback
        self.localADBPort = localADBPort
        self.remoteADBHost = remoteADBHost
        self.remoteADBPort = remoteADBPort
        self.scrcpyArguments = scrcpyArguments
    }

    enum CodingKeys: String, CodingKey {
        case id, name, enabled
        case executionTargetID = "execution_target_id"
        case bindAddress = "bind_address"
        case allowNonLoopback = "allow_non_loopback"
        case localADBPort = "local_adb_port"
        case remoteADBHost = "remote_adb_host"
        case remoteADBPort = "remote_adb_port"
        case scrcpyArguments = "scrcpy_arguments"
    }
}

public struct ResourceCatalogV1: Codable, Equatable, Sendable {
    public var schemaVersion: UInt32
    public var revision: UInt64
    public var fileMappings: [FileMappingSpecV1]
    public var portForwards: [PortForwardSpecV1]
    public var androidMirrors: [AndroidMirrorSpecV1]

    public init(
        schemaVersion: UInt32 = 1,
        revision: UInt64,
        fileMappings: [FileMappingSpecV1],
        portForwards: [PortForwardSpecV1],
        androidMirrors: [AndroidMirrorSpecV1]
    ) {
        self.schemaVersion = schemaVersion
        self.revision = revision
        self.fileMappings = fileMappings
        self.portForwards = portForwards
        self.androidMirrors = androidMirrors
    }

    public static let empty = ResourceCatalogV1(
        revision: 0,
        fileMappings: [],
        portForwards: [],
        androidMirrors: []
    )

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case revision
        case fileMappings = "file_mappings"
        case portForwards = "port_forwards"
        case androidMirrors = "android_mirrors"
    }
}

public enum ResourceKindV1: String, Codable, Equatable, Sendable {
    case fileMapping = "file_mapping"
    case portForward = "port_forward"
    case androidMirror = "android_mirror"
}

public enum ResourceRuntimeStateV1: String, Codable, Equatable, Sendable {
    case disabled
    case stopped
    case waitingForSSH = "waiting_for_ssh"
    case starting
    case running
    case recovering
    case needsAttention = "needs_attention"
    case stopping
}

public struct ResourceRuntimeStatusV1: Codable, Equatable, Sendable {
    public let state: ResourceRuntimeStateV1
    public let generation: UInt64
    public let message: String
    public let canRestart: Bool
    public let canOpenMirror: Bool

    public init(
        state: ResourceRuntimeStateV1,
        generation: UInt64,
        message: String,
        canRestart: Bool,
        canOpenMirror: Bool
    ) {
        self.state = state
        self.generation = generation
        self.message = message
        self.canRestart = canRestart
        self.canOpenMirror = canOpenMirror
    }

    enum CodingKeys: String, CodingKey {
        case state, generation, message
        case canRestart = "can_restart"
        case canOpenMirror = "can_open_mirror"
    }
}

public struct ResourceEntryProjectionV1:
    Codable,
    Equatable,
    Sendable,
    Identifiable
{
    public let id: String
    public let kind: ResourceKindV1
    public let name: String
    public let enabled: Bool
    public let executionTargetID: String
    public let status: ResourceRuntimeStatusV1
    public let canRevealLocalPath: Bool?
    public let canOpenLog: Bool?
    public let actionDisabledReason: String?

    enum CodingKeys: String, CodingKey {
        case id, kind, name, enabled, status
        case executionTargetID = "execution_target_id"
        case canRevealLocalPath = "can_reveal_local_path"
        case canOpenLog = "can_open_log"
        case actionDisabledReason = "action_disabled_reason"
    }
}

public struct ResourcesControlProjectionV1:
    Codable,
    Equatable,
    Sendable
{
    public let schemaVersion: UInt32
    public let revision: UInt64
    public let catalog: ResourceCatalogV1
    public let entries: [ResourceEntryProjectionV1]
    public let availableTargetIDs: [String]
    public let hasPendingDraft: Bool
    public let applyLabel: String
    public let aggregateState: ResourceRuntimeStateV1?
    public let aggregateMessage: String?

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case revision, catalog, entries
        case availableTargetIDs = "available_target_ids"
        case hasPendingDraft = "has_pending_draft"
        case applyLabel = "apply_label"
        case aggregateState = "aggregate_state"
        case aggregateMessage = "aggregate_message"
    }
}

public enum ResourceLogLevelV1: String, Codable, Equatable, Sendable {
    case info
    case warning
    case error
}

public struct ResourceLogEventV1: Codable, Equatable, Sendable {
    public let timestampMS: UInt64
    public let level: ResourceLogLevelV1
    public let message: String

    enum CodingKeys: String, CodingKey {
        case timestampMS = "timestamp_ms"
        case level, message
    }
}

public struct ResourceLogTailV1: Codable, Equatable, Sendable {
    public let resourceID: String
    public let events: [ResourceLogEventV1]
    public let truncated: Bool

    enum CodingKeys: String, CodingKey {
        case resourceID = "resource_id"
        case events, truncated
    }
}

public enum ResourcesCommandV1: Codable, Equatable, Sendable {
    case getProjection
    case getLogTail(resourceID: String)
    case apply(catalog: ResourceCatalogV1)
    case restart(resourceID: String)
    case openMirror(resourceID: String)

    private enum CodingKeys: String, CodingKey {
        case type, catalog
        case resourceID = "resource_id"
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .type) {
        case "get_projection":
            self = .getProjection
        case "get_log_tail":
            self = try .getLogTail(
                resourceID: container.decode(
                    String.self,
                    forKey: .resourceID
                )
            )
        case "apply":
            self = try .apply(
                catalog: container.decode(
                    ResourceCatalogV1.self,
                    forKey: .catalog
                )
            )
        case "restart":
            self = try .restart(
                resourceID: container.decode(
                    String.self,
                    forKey: .resourceID
                )
            )
        case "open_mirror":
            self = try .openMirror(
                resourceID: container.decode(
                    String.self,
                    forKey: .resourceID
                )
            )
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .type,
                in: container,
                debugDescription: "unknown Resources command"
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .getProjection:
            try container.encode("get_projection", forKey: .type)
        case .getLogTail(let resourceID):
            try container.encode("get_log_tail", forKey: .type)
            try container.encode(resourceID, forKey: .resourceID)
        case .apply(let catalog):
            try container.encode("apply", forKey: .type)
            try container.encode(catalog, forKey: .catalog)
        case .restart(let resourceID):
            try container.encode("restart", forKey: .type)
            try container.encode(resourceID, forKey: .resourceID)
        case .openMirror(let resourceID):
            try container.encode("open_mirror", forKey: .type)
            try container.encode(resourceID, forKey: .resourceID)
        }
    }
}

public struct ResourcesRequestEnvelopeV1:
    Codable,
    Equatable,
    Sendable
{
    public let protocolVersion: UInt32
    public let api: String
    public let requestID: String
    public let expectedRevision: UInt64?
    public let command: ResourcesCommandV1

    public init(
        protocolVersion: UInt32 = polarTmuxProtocolVersion,
        requestID: String,
        expectedRevision: UInt64?,
        command: ResourcesCommandV1
    ) {
        self.protocolVersion = protocolVersion
        api = "resources_v1"
        self.requestID = requestID
        self.expectedRevision = expectedRevision
        self.command = command
    }

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case api
        case requestID = "request_id"
        case expectedRevision = "expected_revision"
        case command
    }
}

public enum ResourcesResultV1: Codable, Equatable, Sendable {
    case projection(ResourcesControlProjectionV1)
    case commandAccepted(ResourcesControlProjectionV1)
    case logTail(ResourceLogTailV1)
    case error(
        PolarTmuxProtocolError,
        ResourcesControlProjectionV1?
    )

    private enum CodingKeys: String, CodingKey {
        case type, projection, error, tail
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .type) {
        case "projection":
            self = try .projection(
                container.decode(
                    ResourcesControlProjectionV1.self,
                    forKey: .projection
                )
            )
        case "command_accepted":
            self = try .commandAccepted(
                container.decode(
                    ResourcesControlProjectionV1.self,
                    forKey: .projection
                )
            )
        case "log_tail":
            self = try .logTail(
                container.decode(ResourceLogTailV1.self, forKey: .tail)
            )
        case "error":
            self = try .error(
                container.decode(
                    PolarTmuxProtocolError.self,
                    forKey: .error
                ),
                container.decodeIfPresent(
                    ResourcesControlProjectionV1.self,
                    forKey: .projection
                )
            )
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .type,
                in: container,
                debugDescription: "unknown Resources result"
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .projection(let projection):
            try container.encode("projection", forKey: .type)
            try container.encode(projection, forKey: .projection)
        case .commandAccepted(let projection):
            try container.encode("command_accepted", forKey: .type)
            try container.encode(projection, forKey: .projection)
        case .logTail(let tail):
            try container.encode("log_tail", forKey: .type)
            try container.encode(tail, forKey: .tail)
        case .error(let error, let projection):
            try container.encode("error", forKey: .type)
            try container.encode(error, forKey: .error)
            try container.encodeIfPresent(
                projection,
                forKey: .projection
            )
        }
    }
}

public struct ResourcesResponseEnvelopeV1:
    Codable,
    Equatable,
    Sendable
{
    public let protocolVersion: UInt32
    public let api: String
    public let requestID: String
    public let revision: UInt64
    public let result: ResourcesResultV1

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case api
        case requestID = "request_id"
        case revision, result
    }
}
