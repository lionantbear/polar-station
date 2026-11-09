import Foundation

public let polarTmuxSessionFeedCapability = "session_feed_v1"

public struct PolarTmuxSessionKey:
    Codable, Equatable, Hashable, Sendable
{
    public let executionTarget: String
    public let tmuxSessionID: String

    public init(executionTarget: String, tmuxSessionID: String) {
        self.executionTarget = executionTarget
        self.tmuxSessionID = tmuxSessionID
    }

    enum CodingKeys: String, CodingKey {
        case executionTarget = "execution_target"
        case tmuxSessionID = "tmux_session_id"
    }
}

public enum PolarTmuxSessionLifecycle:
    String, Codable, Equatable, Sendable
{
    case connecting
    case ready
    case recovering
    case unavailable
    case ending
    case ended
}

public struct PolarTmuxSSHTransportGeneration:
    Codable, Equatable, Sendable
{
    public let supervisorInstanceID: UInt64
    public let generation: UInt64

    public init(supervisorInstanceID: UInt64, generation: UInt64) {
        self.supervisorInstanceID = supervisorInstanceID
        self.generation = generation
    }

    enum CodingKeys: String, CodingKey {
        case supervisorInstanceID = "supervisor_instance_id"
        case generation
    }
}

public struct PolarTmuxSessionProjection:
    Codable, Equatable, Sendable
{
    public let key: PolarTmuxSessionKey
    public let displayName: String
    public let lifecycle: PolarTmuxSessionLifecycle
    public let transportGeneration: PolarTmuxSSHTransportGeneration?
    public let revision: UInt64
    public let attention: SessionAttentionProjectionV1?

    public init(
        key: PolarTmuxSessionKey,
        displayName: String,
        lifecycle: PolarTmuxSessionLifecycle,
        transportGeneration: PolarTmuxSSHTransportGeneration?,
        revision: UInt64,
        attention: SessionAttentionProjectionV1?
    ) {
        self.key = key
        self.displayName = displayName
        self.lifecycle = lifecycle
        self.transportGeneration = transportGeneration
        self.revision = revision
        self.attention = attention
    }

    enum CodingKeys: String, CodingKey {
        case key
        case displayName = "display_name"
        case lifecycle
        case transportGeneration = "transport_generation"
        case revision
        case attention
    }
}

public struct PolarTmuxSessionFeedSnapshot:
    Codable, Equatable, Sendable
{
    public let epoch: String
    public let revision: UInt64
    public let sessions: [PolarTmuxSessionProjection]

    public init(
        epoch: String,
        revision: UInt64,
        sessions: [PolarTmuxSessionProjection]
    ) {
        self.epoch = epoch
        self.revision = revision
        self.sessions = sessions
    }
}

public enum PolarTmuxSessionFeedChange:
    Codable, Equatable, Sendable
{
    case upsert(PolarTmuxSessionProjection)
    case remove(PolarTmuxSessionKey)

    private enum CodingKeys: String, CodingKey {
        case type
        case session
        case key
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .type) {
        case "upsert":
            self = try .upsert(container.decode(
                PolarTmuxSessionProjection.self,
                forKey: .session
            ))
        case "remove":
            self = try .remove(container.decode(
                PolarTmuxSessionKey.self,
                forKey: .key
            ))
        case let type:
            throw DecodingError.dataCorruptedError(
                forKey: .type,
                in: container,
                debugDescription: "unknown Session feed change \(type)"
            )
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .upsert(let session):
            try container.encode("upsert", forKey: .type)
            try container.encode(session, forKey: .session)
        case .remove(let key):
            try container.encode("remove", forKey: .type)
            try container.encode(key, forKey: .key)
        }
    }
}

public struct PolarTmuxSessionFeedEvent:
    Codable, Equatable, Sendable
{
    public let epoch: String
    public let revision: UInt64
    public let changes: [PolarTmuxSessionFeedChange]

    public init(
        epoch: String,
        revision: UInt64,
        changes: [PolarTmuxSessionFeedChange]
    ) {
        self.epoch = epoch
        self.revision = revision
        self.changes = changes
    }
}

public enum PolarTmuxSessionFeedUpdate: Equatable, Sendable {
    case snapshot(PolarTmuxSessionFeedSnapshot)
    case event(PolarTmuxSessionFeedEvent)
}
