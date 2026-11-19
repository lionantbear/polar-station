import Foundation
import Combine
import CPolarStationDesktopClient

/// Local C ABI adapter. Draft revision, pending tokens and leave decisions belong to Rust.
@MainActor public final class ConnectionsInteraction: ObservableObject {
    public struct State: Decodable {
        public var host_import_plan: HostImportPlanV1?
        public var host_import_decisions: [HostImportDecisionV1]
        public var host_import_can_apply: Bool
        public var editing: Bool
        public var dirty: Bool
        public var draft: ConnectionChangeV1?
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
        public let request: HostTransferRequestV1?
        public let bundle: HostExportBundleV1?
        public let type: String
        public let token: UInt64?
        public let expected_revision: String?
        public let settings: CodexControlSettingsV1?
        public let codex_revision: UInt64?
        private enum CodingKeys: String, CodingKey { case type, token, expected_revision, change, settings, request, bundle }
        public init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            request = try c.decodeIfPresent(HostTransferRequestV1.self, forKey: .request)
            bundle = try c.decodeIfPresent(HostExportBundleV1.self, forKey: .bundle)
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
    private var diagnosticRevision = ""
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
    public func exportHosts(_ ids: [String]) -> [Effect] { dispatch(["type":"export_hosts", "target_ids":ids]) }
    public func importHosts(_ bundle: HostExportBundleV1) -> [Effect] { send("import_hosts",value:bundle,key:"bundle") }
    public func compareHostImport(_ source: HostRefV1, target: String) -> [Effect] {
        guard let bytes = try? JSONEncoder().encode(source), let value = try? JSONSerialization.jsonObject(with: bytes) else { return [] }
        return dispatch(["type":"compare_host_import", "source":value, "target_id":target])
    }
    public func chooseHostImport(_ decision: HostImportDecisionV1) -> [Effect] { send("choose_host_import",value:decision,key:"decision") }
    public func transferComplete(_ token: UInt64, result: HostTransferResponseV1?, error: String?) -> [Effect] {
        var value: Any = NSNull()
        if let result, let bytes = try? JSONEncoder().encode(result), let object = try? JSONSerialization.jsonObject(with:bytes) { value=object }
        return dispatch(["type":"transfer_complete","token":token,"result":value,"error":error as Any? ?? NSNull()])
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
            if ProcessInfo.processInfo.environment["POLAR_STATION_TEST_PORTABILITY_TRACE"] == "1" {
                if let facts = object["facts"] as? [String: Any], let revision = facts["revision"] as? String { diagnosticRevision = revision }
                NSLog("Station private import owner=%@ input=%@ facts=%@ plan=%@ apply=%d pending=%d", String(describing:ObjectIdentifier(self)), object["type"] as? String ?? "", diagnosticRevision, output.view.host_import_plan?.revision ?? "", output.view.host_import_can_apply ? 1 : 0, output.view.pending ? 1 : 0)
            }
            state = output.view
            return output.effects
        }
    }
}
