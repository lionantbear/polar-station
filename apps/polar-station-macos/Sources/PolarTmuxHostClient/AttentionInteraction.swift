import Foundation
import CPolarStationDesktopClient

@MainActor public final class AttentionInteraction {
    public struct Observation: Encodable {
        let origin: TerminalOriginHandleV1
        let state: SessionAttentionStateV1
        let freshness: SessionSummaryFreshnessV1
        let attention_since_ms: UInt64?
        public init(origin: TerminalOriginHandleV1, attention: SessionAttentionProjectionV1) {
            self.origin = origin; state = attention.state; freshness = attention.freshness; attention_since_ms = attention.attentionSinceMS
        }
    }
    public struct Notice: Decodable { public let workspace_id: String; public let message_id: String; public let count: Int }
    private struct Output: Decodable { let observe: [TerminalOriginHandleV1]; let notify: [Notice] }
    private final class Handle: @unchecked Sendable {
        let pointer = polar_station_attention_new()
        deinit { polar_station_attention_free(pointer) }
    }
    private let owner = Handle()
    public init() {}
    private func object<T: Encodable>(_ value: T) -> Any { (try? JSONSerialization.jsonObject(with: JSONEncoder().encode(value))) ?? NSNull() }
    private func dispatch(_ input: [String: Any]) -> Output? {
        guard let data = try? JSONSerialization.data(withJSONObject: input) else { return nil }
        return data.withUnsafeBytes { bytes in
            guard let result = polar_station_attention_dispatch(owner.pointer, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count) else { return nil }
            defer { polar_station_desktop_string_free(result) }
            return try? JSONDecoder().decode(Output.self, from: Data(String(cString: result).utf8))
        }
    }
    public func reconnect() { _ = dispatch(["type":"reconnect"]) }
    public func observe(_ observations: [Observation]) -> [TerminalOriginHandleV1] {
        dispatch(["type":"observe", "observations":object(observations)])?.observe ?? []
    }
    public func messages(_ page: InboxPageV1, visibleWorkspaces: [String], enabled: Bool) -> [Notice] {
        dispatch(["type":"messages", "messages":object(page.messages), "visible_workspaces":visibleWorkspaces, "enabled":enabled])?.notify ?? []
    }
}
