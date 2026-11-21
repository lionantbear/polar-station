import Foundation

public enum ToolSetupDeliveryV1:String,Codable,Sendable {
    case externalTerminal="external_terminal",nativePane="native_pane"
}
public struct ToolSetupLaunchV1:Codable,Equatable,Sendable {
    public let id:String
    public let execution_target_id:String
    public let title:String
    public let command:String
}
public enum SessionEditorRequestV1: Encodable, Equatable, Sendable {
    case inspectTools(target:String)
    case probeSshDestination(String)
    case prepareSshLogin(String)
    case prepareTools(target:String, tmux:Bool, codex:Bool, codexService:Bool, claude:Bool = false, claudeLogin:Bool = false, delivery:ToolSetupDeliveryV1 = .externalTerminal)
    case setCreationYolo(Bool)
    case checkEndpoint(target: String, endpoint: String)
    case ensureLocalCodex(endpoint: String)
    case history(target: String, endpoint: String, cursor: String?, search: String?, archived: Bool, subagents: Bool)
    case historyDetail(target: String, endpoint: String, thread: String)
    case launchContext(target: String, sourceSession: String?)
    case browseDirectory(target: String, path: String, endpoint: String?)
    case previewEnd(SessionRowProjectionV1.Identity)
    case previewEndAgent(target: String, owner: String, endpoint: String, thread: String)
    enum CodingKeys: String, CodingKey {
        case type, destination, path, cursor, threads, detail, yolo, tmux, codex, codex_service, claude, claude_login, delivery
        case search = "search_term", archived, subagents = "include_subagents"
        case owner = "owner_host_id", thread = "thread_id"
        case target = "execution_target_id", source = "source_session_id"
        case endpoint = "endpoint_id", session = "tmux_session_id"
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .prepareSshLogin(let destination):
            try c.encode("prepare_ssh_login", forKey: .type)
            try c.encode(destination, forKey: .destination)
        case .probeSshDestination(let destination):
            try c.encode("probe_ssh_destination", forKey: .type)
            try c.encode(destination, forKey: .destination)
        case let .inspectTools(target):
            try c.encode("inspect_tools", forKey:.type); try c.encode(target,forKey:.target)
        case let .prepareTools(target, tmux, codex, service, claude, claudeLogin, delivery):
            try c.encode("prepare_tools", forKey:.type);try c.encode(target,forKey:.target)
            try c.encode(tmux,forKey:.tmux);try c.encode(codex,forKey:.codex);try c.encode(service,forKey:.codex_service)
            if claude { try c.encode(true, forKey:.claude) }
            if claudeLogin { try c.encode(true, forKey:.claude_login) }
            if delivery == .nativePane {try c.encode(delivery,forKey:.delivery)}

        case let .setCreationYolo(yolo):
            try c.encode("set_creation_yolo", forKey: .type); try c.encode(yolo, forKey: .yolo)
        case let .checkEndpoint(target, endpoint):
            try c.encode("check_endpoint", forKey: .type)
            try c.encode(target, forKey: .target)
            try c.encode(endpoint, forKey: .endpoint)
        case let .ensureLocalCodex(endpoint):
            try c.encode("ensure_local_codex", forKey: .type)
            try c.encode(endpoint, forKey: .endpoint)
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
public struct SessionSetupChecksV1: Codable, Equatable, Sendable {
    public let codex_auth: String?
    public let codex_server_ready: Bool?
    public let claude_auth: String?
    public let ssh_aliases: [String]?
    public let tmux_archive_bytes: Int?
}
public struct SessionTargetToolsV1:Codable,Equatable,Sendable {
    public let execution_target_id:String
    public let tmux:String
    public let codex:String
    public let codex_control_enabled:Bool
    public let claude:String?
    public var setup_checks: SessionSetupChecksV1? = nil
    public var native_setup:Bool? = nil
    public var default_local_codex_endpoint:String? = nil
}
public enum SessionEditorResultV1: Decodable, Equatable, Sendable {
    case tools([SessionTargetToolsV1])
    case sshDestinationVerified(MachineConnectionV1)
    case history([CodexHistoryThreadV1], String?)
    case historyDetail(CodexHistoryDetailV1)
    case launchContext(SessionDirectoryV1, String?, Bool, [SessionTargetToolsV1]?, [CodexEndpointSettingsV1]?)
    case toolsLaunched(String)
    case toolsPrepared(ToolSetupLaunchV1)
    case creationYolo(Bool)
    case endpointReady
    case endpointLoginRequired
    case directory(SessionDirectoryV1)
    case endPreview(SessionEndPreviewV1)
    enum CodingKeys: String, CodingKey { case type, machine, directory, notice, preview, threads, detail, next_cursor, yolo, tools, codex_endpoints, execution_target_id, setup }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .type) {
        case "ssh_destination_verified": self = try .sshDestinationVerified(c.decode(MachineConnectionV1.self, forKey: .machine))
        case "tools": self = try .tools(c.decode([SessionTargetToolsV1].self, forKey:.tools))
        case "history": self = try .history(c.decode([CodexHistoryThreadV1].self, forKey: .threads), c.decodeIfPresent(String.self, forKey: .next_cursor))
        case "history_detail": self = try .historyDetail(c.decode(CodexHistoryDetailV1.self, forKey: .detail))
        case "launch_context": self = try .launchContext(c.decode(SessionDirectoryV1.self, forKey: .directory), c.decodeIfPresent(String.self, forKey: .notice), c.decodeIfPresent(Bool.self, forKey: .yolo) ?? true, c.decodeIfPresent([SessionTargetToolsV1].self, forKey:.tools), c.decodeIfPresent([CodexEndpointSettingsV1].self,forKey:.codex_endpoints))
        case "tools_prepared": self = try .toolsPrepared(c.decode(ToolSetupLaunchV1.self,forKey:.setup))
        case "tools_launched": self = try .toolsLaunched(c.decode(String.self,forKey:.execution_target_id))
        case "creation_yolo": self = try .creationYolo(c.decode(Bool.self, forKey: .yolo))
        case "endpoint_ready": self = .endpointReady
        case "endpoint_login_required": self = .endpointLoginRequired
        case "directory": self = try .directory(c.decode(SessionDirectoryV1.self, forKey: .directory))
        case "end_preview": self = try .endPreview(c.decode(SessionEndPreviewV1.self, forKey: .preview))
        default: throw DecodingError.dataCorruptedError(forKey: .type, in: c, debugDescription: "Unsupported Session editor result")
        }
    }
}
public typealias SessionEditorReader = @MainActor (SessionEditorRequestV1) async throws -> SessionEditorResultV1
