import Foundation

// Wire values only. The Host owns labels, permissions, revisions and routing.
public struct CodexEndpointSettingsV1: Codable, Equatable, Sendable {
    public var id = ""
    public var execution_target_id = "local"
    public var control_mode = "local"
    public var owner_host_id = ""
    public var socket_path = ""
    public var peer_connection_ref: String?
    public var remote_endpoint_id: String?
    public var remote_target_id: String?
    public init() {}
}
public struct CodexHostConnectionV1: Codable, Equatable, Sendable {
    public var id = ""
    public var host_id = ""
    public var ssh_target_id = ""
    public var port: UInt16 = 0
    public var execution_credential_ref: String?
    public var notification_credential_ref: String?
    public init() {}
}
public struct TelegramAllowedSourceV1: Codable, Equatable, Sendable {
    public var bot_id: Int64 = 0
    public var chat_id: Int64 = 0
    public var sender_id: Int64 = 0
    public init() {}
}
public struct TelegramDestinationV1: Codable, Equatable, Sendable {
    public var id = ""
    public var bot_id: Int64 = 0
    public var chat_id: Int64 = 0
    public init() {}
}
public struct TelegramControlSettingsV1: Codable, Equatable, Sendable {
    public var enabled = false
    public var bot_credential_ref: String?
    public var allowed_sources: [TelegramAllowedSourceV1] = []
    public var destinations: [TelegramDestinationV1] = []
    public init() {}
}
public struct ImessageDestinationV1: Codable, Equatable, Sendable {
    public var host_id = ""
    public var connection_ref: String?
    public var sender_generation = ""
    public var destination_ref = ""
    public init() {}
}
public struct CodexControlSettingsV1: Codable, Equatable, Sendable {
    public var revision: UInt64 = 0
    public var enabled = false
    public var endpoints: [CodexEndpointSettingsV1] = []
    public var host_connections: [CodexHostConnectionV1] = []
    public var telegram = TelegramControlSettingsV1()
    public var imessage_enabled = false
    public var imessage_sender: ImessageDestinationV1?
    public init() {}
}
public struct CodexBindingV1: Decodable, Equatable, Sendable {
    public let owner_host_id: String
    public let binding_id: String
    public let revision: UInt64
    public let session: PolarTmuxSessionKey?
    public let endpoint_id: String
    public let thread_id: String
    public let telegram_destination_ref: String?
    public let imessage_enabled: Bool
}
public struct CodexSessionPresentationV1: Decodable, Equatable, Sendable {
    public let connection_label: String
    public let task_label: String
    public let detail: String?
    public let input_label: String
    public let actions: [ActionProjectionV1]
}
public struct CodexStateV1: Decodable, Equatable, Sendable {
    public let thread_name: String?
    public let binding: CodexBindingV1
    public let connection: String
    public let activity: String
    public let turn_id: String?
    public let final_answer: String?
    public let error_code: String?
    public let can_queue: Bool
    public let can_steer: Bool
    public let can_interrupt: Bool
    public let presentation: CodexSessionPresentationV1?
}
public struct CodexLoadedThreadV1: Decodable, Equatable, Sendable {
    public let thread_id: String
    public let display_name: String
}
public typealias CodexRequestHandler = @MainActor (String?, CodexCommandV1) async throws -> CodexResponseEnvelopeV1

public struct CodexReceiptV1: Decodable, Equatable, Sendable {
    public let label: String?
    public let clear_input: Bool?
    public let intent_id: String
    public let status: String
    public let queue_id: String?
    public let turn_id: String?
    public let error_code: String?
}
public struct CodexCompletionCursor: Codable, Equatable, Sendable {
    public let created_at_ms: UInt64
    public let id: String
}
public struct CodexCompletionOrigin: Decodable, Equatable, Sendable {
    public let owner_host_id: String
    public let endpoint_id: String
    public let thread_id: String
    public let turn_id: String
    public let binding_id: String
    public let binding_revision: UInt64
    public let execution_target_id: String
    public let tmux_session_id: String?
}
public struct CodexCompletion: Decodable, Equatable, Sendable {
    public let id: String
    public let origin: CodexCompletionOrigin
    public let status: String
    public let summary: String
    public let created_at_ms: UInt64
}
public struct CodexDeliveryV1: Codable, Equatable, Sendable {
    public let channel: String
    public let status: String
    public let label: String
    public let created_at_ms: UInt64
    public let error_code: String?
}
public struct CodexCompletionProjectionV1: Decodable, Equatable, Sendable {
    public let completion: CodexCompletion
    public let deliveries: [CodexDeliveryV1]
}
public struct CodexChannelStatusV1: Decodable, Equatable, Sendable {
    public let state: String
    public let error_code: String?
}
public enum CodexCommandV1: Encodable, Equatable, Sendable {
    case getUi, getSettings, getTelegramStatus, getBindings
    case startLocalServer(endpoint:String)
    case updateSettings(revision: UInt64, settings: CodexControlSettingsV1)
    case listHistory(endpoint: String, cursor: String?, search: String?, archived: Bool, includeSubagents: Bool)
    case readHistoryDetail(endpoint: String, thread: String)
    case listLoaded(endpoint: String)
    case bind(session: PolarTmuxSessionKey, endpoint: String, thread: String)
    case getBinding(id: String)
    case unbind(id: String, revision: UInt64)
    case resume(id: String, revision: UInt64)
    case updateNotifications(id: String, revision: UInt64, telegram: String?, imessage: Bool)
    case queueText(id: String, revision: UInt64, intent: String, text: String)
    case steer(id: String, revision: UInt64, intent: String, turn: String, text: String)
    case interrupt(id: String, revision: UInt64, intent: String, turn: String)
    case getReceipt(intent: String)
    case getCompletions(endpoint: String?, before: CodexCompletionCursor?, limit: UInt16)
    enum CodingKeys: String, CodingKey {
        case kind, settings, session, text, before, limit, cursor, search_term, archived, include_subagents
        case endpoint_id, thread_id, binding_id, expected_revision, intent_id, expected_turn_id, turn_id
        case telegram_destination_ref, imessage_enabled
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .startLocalServer(let endpoint):try c.encode("start_local_server",forKey:.kind);try c.encode(endpoint,forKey:.endpoint_id)
        case .getUi: try c.encode("get_ui", forKey: .kind)
        case .getSettings: try c.encode("get_settings", forKey: .kind)
        case .getTelegramStatus: try c.encode("get_telegram_status", forKey: .kind)
        case .getBindings: try c.encode("get_bindings", forKey: .kind)
        case .updateSettings(let revision, let settings):
            try c.encode("update_settings", forKey: .kind); try c.encode(revision, forKey: .expected_revision); try c.encode(settings, forKey: .settings)
        case .listHistory(let endpoint, let cursor, let search, let archived, let includeSubagents):
            try c.encode("list_history", forKey: .kind)
            try c.encode(endpoint, forKey: .endpoint_id)
            try c.encodeIfPresent(cursor, forKey: .cursor)
            try c.encodeIfPresent(search, forKey: .search_term)
            try c.encode(archived, forKey: .archived)
            try c.encode(includeSubagents, forKey: .include_subagents)
        case .readHistoryDetail(let endpoint, let thread):
            try c.encode("read_history_detail", forKey: .kind); try c.encode(endpoint, forKey: .endpoint_id); try c.encode(thread, forKey: .thread_id)
        case .listLoaded(let endpoint):
            try c.encode("list_loaded", forKey: .kind); try c.encode(endpoint, forKey: .endpoint_id)
        case .bind(let session, let endpoint, let thread):
            try c.encode("bind", forKey: .kind); try c.encode(session, forKey: .session)
            try c.encode(endpoint, forKey: .endpoint_id); try c.encode(thread, forKey: .thread_id)
        case .getBinding(let id):
            try c.encode("get_binding", forKey: .kind); try c.encode(id, forKey: .binding_id)
        case .unbind(let id, let revision), .resume(let id, let revision):
            if case .unbind = self { try c.encode("unbind", forKey: .kind) } else { try c.encode("resume", forKey: .kind) }
            try c.encode(id, forKey: .binding_id); try c.encode(revision, forKey: .expected_revision)
        case .updateNotifications(let id, let revision, let telegram, let imessage):
            try c.encode("update_notifications", forKey: .kind); try c.encode(id, forKey: .binding_id)
            try c.encode(revision, forKey: .expected_revision); try c.encodeIfPresent(telegram, forKey: .telegram_destination_ref)
            try c.encode(imessage, forKey: .imessage_enabled)
        case .queueText(let id, let revision, let intent, let text):
            try c.encode("queue_text", forKey: .kind); try c.encode(id, forKey: .binding_id)
            try c.encode(revision, forKey: .expected_revision); try c.encode(intent, forKey: .intent_id); try c.encode(text, forKey: .text)
        case .steer(let id, let revision, let intent, let turn, let text):
            try c.encode("steer", forKey: .kind); try c.encode(id, forKey: .binding_id)
            try c.encode(revision, forKey: .expected_revision); try c.encode(intent, forKey: .intent_id)
            try c.encode(turn, forKey: .expected_turn_id); try c.encode(text, forKey: .text)
        case .interrupt(let id, let revision, let intent, let turn):
            try c.encode("interrupt", forKey: .kind); try c.encode(id, forKey: .binding_id)
            try c.encode(revision, forKey: .expected_revision); try c.encode(intent, forKey: .intent_id); try c.encode(turn, forKey: .turn_id)
        case .getReceipt(let intent):
            try c.encode("get_receipt", forKey: .kind); try c.encode(intent, forKey: .intent_id)
        case .getCompletions(let endpoint, let before, let limit):
            try c.encode("get_completions", forKey: .kind); try c.encodeIfPresent(endpoint, forKey: .endpoint_id)
            try c.encodeIfPresent(before, forKey: .before); try c.encode(limit, forKey: .limit)
        }
    }
}
public struct CodexRequestEnvelopeV1: Encodable, Equatable, Sendable {
    public let protocolVersion: UInt32 = polarTmuxProtocolVersion
    public let api = "codex_v1"
    public let requestID: String
    public let ownerHostID: String
    public let command: CodexCommandV1
    public init(requestID: String, ownerHostID: String, command: CodexCommandV1) {
        self.requestID = requestID; self.ownerHostID = ownerHostID; self.command = command
    }
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version", api, requestID = "request_id", ownerHostID = "owner_host_id", command }
}
public struct CodexHistoryDetailV1: Codable, Equatable, Sendable {
    public let thread: CodexHistoryThreadV1
    public let turn_id: String?
    public let turn_status: String?
    public let user_text: String?
    public let assistant_text: String?
}
public struct CodexHistoryThreadV1: Codable, Equatable, Sendable, Identifiable {
    public let preview: String?
    public let thread_id: String
    public let display_name: String
    public let cwd: String
    public let updated_at: Int64
    public let status: String
    public var id: String { thread_id }
}
public enum CodexResultV1: Decodable, Equatable, Sendable {
    case ui([String: String])
    case settings(CodexControlSettingsV1)
    case telegramStatus(CodexChannelStatusV1)
    case history([CodexHistoryThreadV1], String?)
    case historyDetail(CodexHistoryDetailV1)
    case loaded([CodexLoadedThreadV1])
    case bindings([CodexStateV1])
    case binding(CodexStateV1?)
    case receipt(CodexReceiptV1?)
    case completions([CodexCompletionProjectionV1], CodexCompletionCursor?)
    case error(String)
    enum CodingKeys: String, CodingKey { case kind, labels, settings, status, threads, bindings, binding, receipt, completions, next_before, next_cursor, code, detail }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "ui": self = try .ui(c.decode([String: String].self, forKey: .labels))
        case "settings": self = try .settings(c.decode(CodexControlSettingsV1.self, forKey: .settings))
        case "telegram_status": self = try .telegramStatus(c.decode(CodexChannelStatusV1.self, forKey: .status))
        case "history": self = try .history(c.decode([CodexHistoryThreadV1].self, forKey: .threads), c.decodeIfPresent(String.self, forKey: .next_cursor))
        case "history_detail": self = try .historyDetail(c.decode(CodexHistoryDetailV1.self, forKey: .detail))
        case "loaded": self = try .loaded(c.decode([CodexLoadedThreadV1].self, forKey: .threads))
        case "bindings": self = try .bindings(c.decode([CodexStateV1].self, forKey: .bindings))
        case "binding": self = try .binding(c.decodeIfPresent(CodexStateV1.self, forKey: .binding))
        case "receipt": self = try .receipt(c.decodeIfPresent(CodexReceiptV1.self, forKey: .receipt))
        case "completions": self = try .completions(c.decode([CodexCompletionProjectionV1].self, forKey: .completions), c.decodeIfPresent(CodexCompletionCursor.self, forKey: .next_before))
        case "error": self = try .error(c.decode(String.self, forKey: .code))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown Codex result")
        }
    }
}
public struct CodexResponseEnvelopeV1: Decodable, Equatable, Sendable {
    public let protocolVersion: UInt32
    public let api: String
    public let requestID: String
    public let ownerHostID: String
    public let result: CodexResultV1
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version", api, requestID = "request_id", ownerHostID = "owner_host_id", result }
}
