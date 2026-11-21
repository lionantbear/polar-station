import Foundation
import CPolarStationDesktopClient

public struct WorkspaceLifecycleDecision: Decodable {
    public let effect: String
    public let selected: String
    public let error: String?
    public let can_archive: Bool
}

/// Stateless local UI adapter. The caller retains the only native catalog.
public func decideWorkspaceLifecycle(action: String, ids: [String], archived: [String],
    open: [String], selected: String, bound: String?, target: String? = nil, tabCount: Int = 0, tabIndex: Int? = nil
) throws -> WorkspaceLifecycleDecision {
    struct Input: Encodable {
        let action: String; let ids: [String]; let archived: [String]; let open: [String]
        let selected: String; let bound: String?; let target: String?
        let tab_count: Int; let tab_index: Int?
    }
    let data = try JSONEncoder().encode(Input(action: action, ids: ids, archived: archived,
        open: open, selected: selected, bound: bound, target: target, tab_count: tabCount, tab_index: tabIndex))
    return try data.withUnsafeBytes { bytes in
        guard let result = polar_station_workspace_lifecycle(bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count) else {
            throw NSError(domain: "WorkspaceLifecycle", code: 1, userInfo: [NSLocalizedDescriptionKey: "Workspace action failed."])
        }
        defer { polar_station_desktop_string_free(result) }
        let value = try JSONDecoder().decode(WorkspaceLifecycleDecision.self, from: Data(String(cString: result).utf8))
        if let error = value.error { throw NSError(domain: "WorkspaceLifecycle", code: 1, userInfo: [NSLocalizedDescriptionKey: error]) }
        return value
    }
}

public struct PanePlacementMember: Encodable {
    public let id: String
    public let session: SessionRowProjectionV1.Identity?

    public init(id: String, session: SessionRowProjectionV1.Identity?) {
        self.id = id
        self.session = session
    }
}

public struct PanePlacementDecision: Decodable {
    public let additions: [SessionRowProjectionV1.Identity]
    public let existing: [String]
    public let full: Bool
}

public func planPanePlacement(panes: [PanePlacementMember],
                              sessions: [SessionRowProjectionV1.Identity],
                              reserved: Int = 0, unbound: Int = 0) throws -> PanePlacementDecision {
    struct Input: Encodable {
        let panes: [PanePlacementMember]
        let sessions: [SessionRowProjectionV1.Identity]
        let reserved: Int
        let unbound: Int
    }
    let data = try JSONEncoder().encode(Input(panes: panes, sessions: sessions, reserved: reserved, unbound: unbound))
    return try data.withUnsafeBytes { bytes in
        guard let result = polar_station_plan_pane_placement(bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count) else {
            throw NSError(domain: "PanePlacement", code: 1)
        }
        defer { polar_station_desktop_string_free(result) }
        return try JSONDecoder().decode(PanePlacementDecision.self, from: Data(String(cString: result).utf8))
    }
}
