import Foundation

// Wire projections only; admission, routing and deduplication belong to Rust.
public struct MessageConnectorV1: Codable, Equatable, Sendable {
    public var id = ""
    public var host_id = ""
    public var provider = "telegram"
    public var connection_ref: String?
    public var enabled = true
    public var allowed_source_hosts: [String] = []
    public init() {}
}
public struct CodexMessageRouteV1: Codable, Equatable, Sendable {
    public var endpoint_id = ""
    public var thread_id: String?
    public var connector_id = ""
    public var destination_ref = "current"
    public var enabled = true
    public init() {}
}
public struct MessageConnectorSettingsV1: Codable, Equatable, Sendable {
    public var revision: UInt64 = 0
    public var connectors: [MessageConnectorV1] = []
    public var routes: [CodexMessageRouteV1] = []
    public var disabled_monitors: [String] = []
    public init() {}
}
public struct ConnectorStreamV1: Decodable, Equatable, Sendable {
    public let id, connector_id, destination_ref, thread_id, title: String
    public let return_host_id, return_endpoint_id, status: String
    public let platform_id, error_code: String?
}
public struct ConnectorResultV1: Decodable, Equatable, Sendable {
    public let host_id: String?
    public let kind: String
    public let settings: MessageConnectorSettingsV1?
    public let streams: [ConnectorStreamV1]?
    public let code: String?
}
public enum ConnectorCommandV1: Encodable, Equatable, Sendable {
    case getSettings
    case updateSettings(MessageConnectorSettingsV1)
    case getStreams(String)
    case setReturnRoute(stream: String, expectedHost: String, host: String, endpoint: String)
    enum CodingKeys: String, CodingKey { case kind, settings, expected_revision, connector_id, stream_id, expected_host_id, host_id, endpoint_id }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .getSettings: try c.encode("get_settings", forKey: .kind)
        case .updateSettings(let settings):
            try c.encode("update_settings", forKey: .kind)
            try c.encode(settings.revision, forKey: .expected_revision)
            try c.encode(settings, forKey: .settings)
        case .getStreams(let id):
            try c.encode("get_streams", forKey: .kind); try c.encode(id, forKey: .connector_id)
        case .setReturnRoute(let stream, let expectedHost, let host, let endpoint):
            try c.encode("set_return_route", forKey: .kind); try c.encode(stream, forKey: .stream_id)
            try c.encode(expectedHost, forKey: .expected_host_id); try c.encode(host, forKey: .host_id); try c.encode(endpoint, forKey: .endpoint_id)
        }
    }
}
public typealias ConnectorRequestHandler = @MainActor (ConnectorCommandV1) async throws -> ConnectorResultV1
