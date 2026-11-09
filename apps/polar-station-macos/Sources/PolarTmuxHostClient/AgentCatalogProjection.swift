import Foundation
import CPolarStationDesktopClient

public struct AgentCatalogRow: Decodable {
    public let root: Int
    public let parent: Int?
    public let key: String
    public let group: String
    public let matches: Bool
    public let rank: Int?
}
public func projectAgentCatalog(agents: [AgentSessionCatalogRowV1], groups: [SessionTargetGroupV1], ranks: [Int?], query: String, filter: String) -> [AgentCatalogRow] {
    struct Input: Encodable {
        let agents: [AgentSessionCatalogRowV1]; let groups: [SessionTargetGroupV1]
        let ranks: [Int?]; let query: String; let filter: String
    }
    guard let data = try? JSONEncoder().encode(Input(agents: agents, groups: groups, ranks: ranks, query: query, filter: filter)) else { return [] }
    return data.withUnsafeBytes { bytes in
        guard let result = polar_station_agent_catalog_project(bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count) else { return [] }
        defer { polar_station_desktop_string_free(result) }
        return (try? JSONDecoder().decode([AgentCatalogRow].self, from: Data(String(cString: result).utf8))) ?? []
    }
}
