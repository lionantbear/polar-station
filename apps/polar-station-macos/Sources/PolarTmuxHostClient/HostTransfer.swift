import Foundation

public struct HostRefV1: Codable, Equatable, Sendable, Hashable {
    public var catalog_id: String
    public var target_id: String
}
public struct HostSystemIdentityV1: Codable, Equatable, Sendable { public var kind: String; public var value: String }
public struct HostExecutionUserV1: Codable, Equatable, Sendable { public var name: String; public var uid: UInt32 }
public struct HostServerKeyV1: Codable, Equatable, Sendable { public var algorithm: String; public var fingerprint: String }
public struct HostIdentityObservationV1: Codable, Equatable, Sendable {
    public var system: HostSystemIdentityV1?
    public var user: HostExecutionUserV1?
    public var server_keys: [HostServerKeyV1]
    public var hostname: String?
    public var platform: String?
    public var observed_at: String?
}
public struct PortableHostConnectionV1: Codable, Equatable, Sendable {
    public var kind: String
    public var destination: String?
    public var route_profile: WorkspaceSSHTargetRouteProfileV1?
}
public struct PortableHostV1: Codable, Equatable, Sendable {
    public let source: HostRefV1
    public let display_name: String
    public let connection: PortableHostConnectionV1
    public let observation: HostIdentityObservationV1
    // The native adapter must not normalize absent optional fields into null (or vice versa).
    private let original: PolarTmuxJSONValue
    private enum CodingKeys: String, CodingKey { case source, display_name, connection, observation }
    public init(from decoder: Decoder) throws {
        original = try PolarTmuxJSONValue(from: decoder)
        let values = try decoder.container(keyedBy: CodingKeys.self)
        source = try values.decode(HostRefV1.self, forKey: .source)
        display_name = try values.decode(String.self, forKey: .display_name)
        connection = try values.decode(PortableHostConnectionV1.self, forKey: .connection)
        observation = try values.decode(HostIdentityObservationV1.self, forKey: .observation)
    }
    public func encode(to encoder: Encoder) throws { try original.encode(to: encoder) }
}

public struct HostExportBundleV1: Codable, Equatable, Sendable { public var schema: String; public var hosts: [PortableHostV1] }
public struct HostFieldComparisonV1: Codable, Equatable, Sendable {
    public var field: String; public var label: String
    public var local: String?; public var imported: String?; public var change: String
}
public struct HostImportCandidateV1: Codable, Equatable, Sendable {
    public var target_id: String; public var display_name: String
    public var identity_verified: Bool; public var different_user: Bool
    public var fields: [HostFieldComparisonV1]
}
public struct HostImportRowV1: Codable, Equatable, Sendable {
    public var fields: [HostFieldComparisonV1]
    public var host: PortableHostV1; public var disposition: String; public var candidates: [HostImportCandidateV1]
}
public struct HostImportTargetV1: Codable, Equatable, Sendable { public var target_id: String; public var display_name: String }
public struct HostImportPlanV1: Codable, Equatable, Sendable { public var targets: [HostImportTargetV1]; public var revision: String; public var rows: [HostImportRowV1] }
public struct HostImportDecisionV1: Codable, Equatable, Sendable {
    public var action: String; public var source: HostRefV1
    public var target_id: String?; public var fields: [String]?; public var allow_address_removal: Bool?
    public init(action: String, source: HostRefV1, target: String? = nil, fields: [String]? = nil, allowRemoval: Bool? = nil) {
        self.action=action; self.source=source; self.target_id=target; self.fields=fields; self.allow_address_removal=allowRemoval
    }
}
public struct HostImportAssociationV1: Codable, Equatable, Sendable { public var source: HostRefV1; public var target_id: String }
public struct HostImportResultV1: Codable, Equatable, Sendable {
    public var revision: String; public var associations: [HostImportAssociationV1]
    public var created: [String]; public var changed: [String]; public var skipped: [HostRefV1]
}
public struct HostTransferRequestV1: Codable, Equatable, Sendable {
    public var bindings: [PolarTmuxPaneContentBinding]?
    public var sources: [HostRefV1]?
    public var host: PortableHostV1?; public var target_id: String?
    public var type: String; public var target_ids: [String]?
    public var bundle: HostExportBundleV1?; public var expected_revision: String?; public var decisions: [HostImportDecisionV1]?
}
public struct HostTransferResponseV1: Codable, Equatable, Sendable {
    public var sources: [TerminalSourceV1]?
    public var associations: [HostImportAssociationV1]?
    public var source: HostRefV1?; public var revision: String?; public var candidate: HostImportCandidateV1?
    public var type: String; public var bundle: HostExportBundleV1?; public var plan: HostImportPlanV1?; public var result: HostImportResultV1?
}
public typealias HostTransferReader = @MainActor (HostTransferRequestV1) async throws -> HostTransferResponseV1

public struct ObservedTmuxSessionV1: Codable, Equatable, Sendable {
    public var server_generation: String
    public var session_id: String
    public var created_unix_seconds: Int64
}
public struct TerminalSourceV1: Codable, Equatable, Sendable {
    public var host: HostRefV1
    public var session: ObservedTmuxSessionV1?
    public var original_agent_id: String?
}

/// Scalar mirror of the validated shared portable-content contract.
public struct PortableContentV1: Codable, Equatable, Sendable {
    public var kind: String
    public var source: TerminalSourceV1?
    public var node_id: String?
    public var owner: String?
    public var thread: String?
    public var actor: String?
    public var source_workspace: String?
    public var view_id: String?
    public var paths: [String]?
    public var field: String?
    public var host: HostRefV1?
    public var setup_id: String?
    public var fingerprint: String {
        let encoder=JSONEncoder();encoder.outputFormatting=[.sortedKeys]
        return (try? encoder.encode(self)).flatMap { String(data:$0,encoding:.utf8) } ?? ""
    }
}
