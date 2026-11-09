import Foundation
import Combine
import CPolarStationDesktopClient

/// Local C ABI adapter. Draft revision, pending tokens and leave decisions belong to Rust.
@MainActor public final class ConnectionsInteraction: ObservableObject {
    public struct State: Decodable {
        public var editing: Bool
        public var codex_settings: CodexControlSettingsV1?
        public var codex_draft: CodexEndpointSettingsV1?
        public var codex_enabled: Bool
        public var pending: Bool
        public var confirm_discard: Bool
        public var message: String?
        public var confirmation: String
        public var save_label: String
        public var discard_label: String
        public var keep_label: String
    }
    public struct Effect: Decodable {
        public let type: String
        public let token: UInt64?
        public let expected_revision: String?
        public let settings: CodexControlSettingsV1?
        public let codex_revision: UInt64?
        private enum CodingKeys: String, CodingKey { case type, token, expected_revision, change, settings }
        public init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            type = try c.decode(String.self, forKey: .type)
            token = try c.decodeIfPresent(UInt64.self, forKey: .token)
            settings = try c.decodeIfPresent(CodexControlSettingsV1.self, forKey: .settings)
            change = try c.decodeIfPresent(ConnectionChangeV1.self, forKey: .change)
            if type == "save_codex" {
                codex_revision = try c.decode(UInt64.self, forKey: .expected_revision); expected_revision = nil
            } else {
                expected_revision = try c.decodeIfPresent(String.self, forKey: .expected_revision); codex_revision = nil
            }
        }
        public let change: ConnectionChangeV1?
    }
    private struct Output: Decodable { let view: State; let effects: [Effect] }
    // The private owner is only borrowed by this MainActor adapter. Its final
    // release needs no actor hop; no callback can outlive its owning adapter.
    private final class Handle: @unchecked Sendable {
        let pointer = polar_station_connections_new()
        deinit { polar_station_connections_free(pointer) }
    }
    private let owner = Handle()
    @Published public private(set) var state: State?
    public var requestClose: (() -> Void)?
    public var saveCodexDraft: (() -> Void)?
    public init() {}
    public func codexSnapshot(_ settings: CodexControlSettingsV1, owner: String) {
        guard let bytes = try? JSONEncoder().encode(settings), let value = try? JSONSerialization.jsonObject(with: bytes) else { return }
        _ = dispatch(["type":"codex_snapshot", "settings":value, "owner":owner])
    }
    public func removeCodex(machine: String, id: String) -> [Effect] {
        dispatch(["type":"remove_codex", "machine":machine, "id":id])
    }
    public func beginCodex(machine: String, id: String?) {
        _ = dispatch(["type":"edit_codex", "machine":machine, "id":id as Any? ?? NSNull()])
    }
    public func draftCodex(_ endpoint: CodexEndpointSettingsV1, enabled: Bool) {
        guard let bytes = try? JSONEncoder().encode(endpoint), let value = try? JSONSerialization.jsonObject(with: bytes) else { return }
        _ = dispatch(["type":"draft_codex", "endpoint":value, "enabled":enabled])
    }
    public func snapshot(_ facts: ConnectionsProjectionV1) { _ = send("snapshot", value: facts, key: "facts") }
    public func begin(_ change: ConnectionChangeV1) { _ = send("edit", value: change, key: "change") }
    public func draft(_ change: ConnectionChangeV1) { _ = send("draft", value: change, key: "change") }
    public func complete(_ token: UInt64, error: String?) -> [Effect] {
        dispatch(["type":"complete", "token":token, "error":error as Any? ?? NSNull()])
    }
    public func action(_ type: String) -> [Effect] { dispatch(["type":type]) }
    private func send<T: Encodable>(_ type: String, value: T, key: String) -> [Effect] {
        guard let bytes = try? JSONEncoder().encode(value), let object = try? JSONSerialization.jsonObject(with: bytes) else { return [] }
        return dispatch(["type":type, key:object])
    }
    private func dispatch(_ object: [String:Any]) -> [Effect] {
        guard let handle = owner.pointer, let bytes = try? JSONSerialization.data(withJSONObject: object) else { return [] }
        return bytes.withUnsafeBytes { buffer in
            guard let result = polar_station_connections_dispatch(handle, buffer.bindMemory(to: UInt8.self).baseAddress, bytes.count) else { return [] }
            defer { polar_station_desktop_string_free(result) }
            guard let output = try? JSONDecoder().decode(Output.self, from: Data(String(cString: result).utf8)) else { return [] }
            state = output.view
            return output.effects
        }
    }
}
