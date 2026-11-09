import Foundation
import Combine
import CPolarStationDesktopClient

/// Only JSON/C ABI delivery and native task bindings. All editor decisions live in Rust.
@MainActor public final class SessionEditorInteraction: ObservableObject {
    public struct Draft: Codable, Equatable {
        public var bind_only = false
        public var target = ""
        public var endpoint = ""
        public var session_type = "shell"
        public var mode = "new"
        public var name = ""
        public var directory = ""
        public var command = ""
        public var auto_restart = false
        public var yolo = false
        public var open_local_cli = false
        public var original_id = ""
        public var search = ""
        public var archived = false
        public var subagents = false
        public var rename: SessionRowProjectionV1.Identity?
        public init() {}
    }
    public struct State: Decodable {
        public let end_preview: SessionEndPreviewV1?
        public let end_loading: Bool
        public let end_completed: Bool
        public let draft: Draft
        public let history: [CodexHistoryThreadV1]
        public let selected: [String]
        public let manual: Bool
        public let detail: CodexHistoryDetailV1?
        public let history_loading: Bool
        public let detail_loading: Bool
        public let directory_loading: Bool
        public let history_error: String?
        public let detail_error: String?
        public let message: String?
        public let can_more: Bool
        public let can_submit: Bool
        public let pending: Bool
        public let submit_label: String
        public let local_cli_supported: Bool
    }
    private struct Effect: Decodable {
        let type: String
        let token: UInt64
        let request: Request?
        let operation: ControlCenterIntentOperationV1?
        let endpoint_id: String?
        let thread_id: String?
        let owner_host_id: String?
        let title: String?
    }
    // This is only a decode adapter for the existing read-only Host contract.
    private struct Request: Decodable {
        let type: String
        let execution_target_id: String
        let endpoint_id: String?
        let thread_id: String?
        let source_session_id: String?
        let owner_host_id: String?
        let tmux_session_id: String?
        let cursor: String?
        let search_term: String?
        let archived: Bool?
        let include_subagents: Bool?
        var wire: SessionEditorRequestV1? {
            switch type {
            case "history": .history(target: execution_target_id, endpoint: endpoint_id ?? "", cursor: cursor,
                search: search_term, archived: archived ?? false, subagents: include_subagents ?? false)
            case "history_detail": .historyDetail(target: execution_target_id, endpoint: endpoint_id ?? "", thread: thread_id ?? "")
            case "launch_context": .launchContext(target: execution_target_id, sourceSession: source_session_id)
            case "preview_end": .previewEnd(.init(executionTargetID: execution_target_id, tmuxSessionID: tmux_session_id ?? ""))
            case "preview_end_agent": .previewEndAgent(target: execution_target_id, owner: owner_host_id ?? "", endpoint: endpoint_id ?? "", thread: thread_id ?? "")
            default: nil
            }
        }
    }
    private struct Output: Decodable { let view: State; let effects: [Effect] }
    private final class Handle: @unchecked Sendable {
        let pointer = polar_station_session_editor_new()
        deinit { polar_station_session_editor_free(pointer) }
    }
    private let owner = Handle()
    private var reads: [UInt64: Task<Void, Never>] = [:]
    private var reader: SessionEditorReader?
    public var pick: (@MainActor (String, String, String, String) -> Void)?
    public var submit: (@MainActor (ControlCenterIntentOperationV1, @escaping @MainActor (String?, [String]) -> Void) -> Void)?
    @Published public private(set) var state: State?
    public init() {}
    public func open(_ draft: Draft, endpoints: [CodexEndpointSettingsV1], source: SessionRowProjectionV1.Identity?, read: SessionEditorReader?) {
        reader = read
        dispatch(["type":"open", "draft":object(draft), "endpoints":object(endpoints), "source":source.map(object) ?? NSNull()])
    }
    public func openEnd(_ request: SessionEditorRequestV1, read: SessionEditorReader?) {
        reader = read
        dispatch(["type":"open_end", "request":object(request)])
    }
    public func endpoints(_ endpoints: [CodexEndpointSettingsV1]) { dispatch(["type":"endpoints", "endpoints":object(endpoints)]) }
    public func edit(_ change: (inout Draft) -> Void) {
        guard var draft = state?.draft else { return }; change(&draft)
        dispatch(["type":"edit", "draft":object(draft)])
    }
    public func select(_ id: String, checked: Bool? = nil) {
        dispatch(["type":"select", "id":id, "checked":checked as Any? ?? NSNull()])
    }
    public func manual(_ id: String) { dispatch(["type":"manual", "id":id]) }
    public func action(_ type: String) { dispatch(["type":type]) }
    private func object<T: Encodable>(_ value: T) -> Any {
        (try? JSONSerialization.jsonObject(with: JSONEncoder().encode(value))) ?? NSNull()
    }
    private func complete(_ token: UInt64, result: SessionEditorResultV1?, error: String?) {
        reads.removeValue(forKey: token)
        var value: Any = NSNull()
        switch result {
        case .history(let rows, let next): value = ["type":"history", "threads":object(rows), "next_cursor":next as Any? ?? NSNull()]
        case .historyDetail(let detail): value = ["type":"history_detail", "detail":object(detail)]
        case .launchContext(let directory, let notice): value = ["type":"launch_context", "directory":object(directory), "notice":notice as Any? ?? NSNull()]
        case .endPreview(let preview): value = ["type":"end_preview", "preview":object(preview)]
        default: break
        }
        dispatch(["type":"complete", "token":token, "result":value, "error":error as Any? ?? NSNull()])
    }
    private func dispatch(_ input: [String: Any]) {
        guard let handle = owner.pointer, let bytes = try? JSONSerialization.data(withJSONObject: input) else { return }
        let output: Output? = bytes.withUnsafeBytes { buffer in
            guard let result = polar_station_session_editor_dispatch(handle, buffer.bindMemory(to: UInt8.self).baseAddress, bytes.count) else { return nil }
            defer { polar_station_desktop_string_free(result) }
            guard var object = try? JSONSerialization.jsonObject(with: Data(String(cString: result).utf8)) as? [String: Any] else { return nil }
            if let effects = object["effects"] as? [[String: Any]] {
                object["effects"] = effects.map { effect in
                    var effect = effect
                    if var operation = effect["operation"] as? [String: Any] {
                        operation["type"] = operation.removeValue(forKey: "intent")
                        if ["rename_session", "kill_session"].contains(operation["type"] as? String ?? "") {
                            operation["identity"] = ["execution_target_id":operation.removeValue(forKey: "execution_target_id") ?? NSNull(),
                                "tmux_session_id":operation.removeValue(forKey: "tmux_session_id") ?? NSNull()]
                        }
                        effect["operation"] = operation
                    }
                    return effect
                }
            }
            guard let data = try? JSONSerialization.data(withJSONObject: object) else { return nil }
            return try? JSONDecoder().decode(Output.self, from: data)
        }
        guard let output else { return }
        state = output.view
        for effect in output.effects {
            switch effect.type {
            case "cancel": reads.removeValue(forKey: effect.token)?.cancel()
            case "read":
                guard let request = effect.request else { continue }
                let read = reader
                reads[effect.token] = Task { [weak self] in
                    do {
                        guard let read else { throw PolarTmuxLocalIPCError.connectionClosed }
                        guard let wire = request.wire else { throw PolarTmuxLocalIPCError.unexpectedFrame("Unknown editor read") }
                        let result = try await read(wire)
                        self?.complete(effect.token, result: result, error: nil)
                    } catch { self?.complete(effect.token, result: nil, error: error.localizedDescription) }
                }
            case "picked":
                guard let owner = effect.owner_host_id, let endpoint = effect.endpoint_id, let thread = effect.thread_id, let pick else { continue }
                pick(owner, endpoint, thread, effect.title ?? thread)
                dispatch(["type":"submitted", "token":effect.token, "error":NSNull(), "completed":[String]()])
            case "submit":
                guard let operation = effect.operation, let submit else {
                    dispatch(["type":"submitted", "token":effect.token, "error":"The Session service is unavailable.", "completed":[String]()]); continue
                }
                submit(operation) { [weak self] error, completed in
                    self?.dispatch(["type":"submitted", "token":effect.token, "error":error as Any? ?? NSNull(), "completed":completed])
                }
            default: break
            }
        }
    }
}
