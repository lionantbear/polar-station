import Foundation

public enum SessionEditorRequestV1: Encodable, Equatable, Sendable {
    case history(target: String, endpoint: String, cursor: String?, search: String?, archived: Bool, subagents: Bool)
    case historyDetail(target: String, endpoint: String, thread: String)
    case launchContext(target: String, sourceSession: String?)
    case browseDirectory(target: String, path: String, endpoint: String?)
    case previewEnd(SessionRowProjectionV1.Identity)
    case previewEndAgent(target: String, owner: String, endpoint: String, thread: String)
    enum CodingKeys: String, CodingKey {
        case type, path, cursor, threads, detail
        case search = "search_term", archived, subagents = "include_subagents"
        case owner = "owner_host_id", thread = "thread_id"
        case target = "execution_target_id", source = "source_session_id"
        case endpoint = "endpoint_id", session = "tmux_session_id"
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case let .history(target, endpoint, cursor, search, archived, subagents):
            try c.encode("history", forKey: .type); try c.encode(target, forKey: .target); try c.encode(endpoint, forKey: .endpoint)
            try c.encodeIfPresent(cursor, forKey: .cursor); try c.encodeIfPresent(search, forKey: .search)
            try c.encode(archived, forKey: .archived); try c.encode(subagents, forKey: .subagents)
        case let .historyDetail(target, endpoint, thread):
            try c.encode("history_detail", forKey: .type); try c.encode(target, forKey: .target)
            try c.encode(endpoint, forKey: .endpoint); try c.encode(thread, forKey: .thread)
        case let .launchContext(target, source):
            try c.encode("launch_context", forKey: .type); try c.encode(target, forKey: .target)
            try c.encodeIfPresent(source, forKey: .source)
        case let .browseDirectory(target, path, endpoint):
            try c.encode("browse_directory", forKey: .type); try c.encode(target, forKey: .target)
            try c.encode(path, forKey: .path); try c.encodeIfPresent(endpoint, forKey: .endpoint)
        case let .previewEndAgent(target, owner, endpoint, thread):
            try c.encode("preview_end_agent", forKey: .type); try c.encode(target, forKey: .target)
            try c.encode(owner, forKey: .owner); try c.encode(endpoint, forKey: .endpoint); try c.encode(thread, forKey: .thread)
        case let .previewEnd(identity):
            try c.encode("preview_end", forKey: .type); try c.encode(identity.executionTargetID, forKey: .target)
            try c.encode(identity.tmuxSessionID, forKey: .session)
        }
    }
}

public struct SessionDirectoryV1: Codable, Equatable, Sendable {
    public let executionTargetID: String
    public let home: String
    public let path: String
    public let parent: String?
    public let directories: [String]
    enum CodingKeys: String, CodingKey {
        case executionTargetID = "execution_target_id", home, path, parent, directories
    }
}
public struct SessionEndPreviewV1: Codable, Equatable, Sendable {
    public let session: SessionRowProjectionV1.Identity?
    public let name: String
    public let targetLabel: String
    public let attachedClients: UInt32
    public let confirmation: ConfirmationProjectionV1
    enum CodingKeys: String, CodingKey {
        case session, name, confirmation
        case targetLabel = "target_label", attachedClients = "attached_clients"
    }
}
public enum SessionEditorResultV1: Decodable, Equatable, Sendable {
    case history([CodexHistoryThreadV1], String?)
    case historyDetail(CodexHistoryDetailV1)
    case launchContext(SessionDirectoryV1, String?)
    case directory(SessionDirectoryV1)
    case endPreview(SessionEndPreviewV1)
    enum CodingKeys: String, CodingKey { case type, directory, notice, preview, threads, detail, next_cursor }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .type) {
        case "history": self = try .history(c.decode([CodexHistoryThreadV1].self, forKey: .threads), c.decodeIfPresent(String.self, forKey: .next_cursor))
        case "history_detail": self = try .historyDetail(c.decode(CodexHistoryDetailV1.self, forKey: .detail))
        case "launch_context": self = try .launchContext(c.decode(SessionDirectoryV1.self, forKey: .directory), c.decodeIfPresent(String.self, forKey: .notice))
        case "directory": self = try .directory(c.decode(SessionDirectoryV1.self, forKey: .directory))
        case "end_preview": self = try .endPreview(c.decode(SessionEndPreviewV1.self, forKey: .preview))
        default: throw DecodingError.dataCorruptedError(forKey: .type, in: c, debugDescription: "Unsupported Session editor result")
        }
    }
}
public typealias SessionEditorReader = @MainActor (SessionEditorRequestV1) async throws -> SessionEditorResultV1
