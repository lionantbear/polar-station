import Foundation

public struct MachineConnectionV1: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var displayName: String
    public var destination: String
    public var routeProfile: WorkspaceSSHTargetRouteProfileV1?
    public init(id: String = "", displayName: String = "", destination: String = "", routeProfile: WorkspaceSSHTargetRouteProfileV1? = nil) {
        self.id = id; self.displayName = displayName; self.destination = destination; self.routeProfile = routeProfile
    }
    enum CodingKeys: String, CodingKey { case id, destination; case displayName = "display_name", routeProfile = "route_profile" }
}
public struct PolarConnectionV1: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var displayName: String
    public var executionTargetID: String
    public var socketPath: String
    public var expectedNodeID: String
    public var expectedPeerUID: UInt32?
    public init(id: String = "", displayName: String = "", executionTargetID: String = "local", socketPath: String = "", expectedNodeID: String = "", expectedPeerUID: UInt32? = nil) {
        self.id = id; self.displayName = displayName; self.executionTargetID = executionTargetID; self.socketPath = socketPath; self.expectedNodeID = expectedNodeID; self.expectedPeerUID = expectedPeerUID
    }
    enum CodingKeys: String, CodingKey { case id; case displayName = "display_name", executionTargetID = "execution_target_id", socketPath = "socket_path", expectedNodeID = "expected_node_id", expectedPeerUID = "expected_peer_uid" }
}
public struct MachineConnectionStatusV1: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let connection: ConnectionStatusProjectionV1
}
public struct ConnectionsProjectionV1: Codable, Equatable, Sendable {
    public var revision: String
    public var machineStatuses: [MachineConnectionStatusV1]? = nil
    public var lastCheck: ConnectionCheckV1? = nil
    public var machines: [MachineConnectionV1]
    public var polarInstances: [PolarConnectionV1]
    public init(revision: String = "", machines: [MachineConnectionV1] = [], polarInstances: [PolarConnectionV1] = []) {
        self.revision = revision; self.machines = machines; self.polarInstances = polarInstances
    }
    enum CodingKeys: String, CodingKey { case revision, machines; case machineStatuses = "machine_statuses"; case lastCheck = "last_check"; case polarInstances = "polar_instances" }
}
public enum ConnectionChangeV1: Codable, Equatable, Sendable {
    case saveMachine(MachineConnectionV1), savePolar(PolarConnectionV1), removeMachine(String), removePolar(String)
    enum CodingKeys: String, CodingKey { case type, machine, polar, id }
    enum Kind: String, Codable { case saveMachine = "save_machine", savePolar = "save_polar", removeMachine = "remove_machine", removePolar = "remove_polar" }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(Kind.self, forKey: .type) {
        case .saveMachine: self = .saveMachine(try c.decode(MachineConnectionV1.self, forKey: .machine))
        case .savePolar: self = .savePolar(try c.decode(PolarConnectionV1.self, forKey: .polar))
        case .removeMachine: self = .removeMachine(try c.decode(String.self, forKey: .id))
        case .removePolar: self = .removePolar(try c.decode(String.self, forKey: .id))
        }
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .saveMachine(let m): try c.encode(Kind.saveMachine, forKey: .type); try c.encode(m, forKey: .machine)
        case .savePolar(let p): try c.encode(Kind.savePolar, forKey: .type); try c.encode(p, forKey: .polar)
        case .removeMachine(let id): try c.encode(Kind.removeMachine, forKey: .type); try c.encode(id, forKey: .id)
        case .removePolar(let id): try c.encode(Kind.removePolar, forKey: .type); try c.encode(id, forKey: .id)
        }
    }
}

public struct SessionTargetGroupV1: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let title: String
    public let state: String
    public let message: String?
    public let stale: Bool
}

public struct ConnectionCheckV1: Codable, Equatable, Sendable {
    public let requestID: String
    public let targetID: String
    public let polar: Bool
    public let ready: Bool
    public let summary: String
    enum CodingKeys: String, CodingKey { case requestID = "request_id", targetID = "target_id", polar, ready, summary }
}
