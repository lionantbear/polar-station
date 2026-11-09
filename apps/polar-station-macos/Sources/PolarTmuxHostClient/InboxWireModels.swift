import Foundation

public struct InboxMessageIDV1: Codable, Equatable, Hashable, Sendable {
    public let rawValue: String
    public init(_ rawValue: String) { self.rawValue = rawValue }
    public init(from decoder: Decoder) throws { rawValue = try decoder.singleValueContainer().decode(String.self) }
    public func encode(to encoder: Encoder) throws { var c = encoder.singleValueContainer(); try c.encode(rawValue) }
}

public struct TerminalOriginHandleV1: Codable, Equatable, Sendable {
    public let workspaceID: String; public let tabID: String; public let paneID: String
    public let executionTargetID: String; public let tmuxSessionID: String
    public let consumerGeneration: UInt64; public let nativeRecoveryToken: String
    public init(workspaceID: String, tabID: String, paneID: String, executionTargetID: String, tmuxSessionID: String, consumerGeneration: UInt64, nativeRecoveryToken: String) {
        self.workspaceID = workspaceID; self.tabID = tabID; self.paneID = paneID
        self.executionTargetID = executionTargetID; self.tmuxSessionID = tmuxSessionID
        self.consumerGeneration = consumerGeneration; self.nativeRecoveryToken = nativeRecoveryToken
    }
    enum CodingKeys: String, CodingKey { case workspaceID = "workspace_id", tabID = "tab_id", paneID = "pane_id", executionTargetID = "execution_target_id", tmuxSessionID = "tmux_session_id", consumerGeneration = "consumer_generation", nativeRecoveryToken = "native_recovery_token" }
}

public enum InboxDraftStatusV1: String, Codable, Equatable, Sendable { case none, generating, ready, failed, approved }
public enum DeliveryReceiptStateV1: String, Codable, Equatable, Sendable { case restoring, delivered, failedBeforeWrite = "failed_before_write", deliveryUnknown = "delivery_unknown" }
public struct AIInvocationProjectionV1: Codable, Equatable, Sendable {
    public let provider: String; public let model: String?; public let durationMilliseconds: UInt64; public let error: String?
    enum CodingKeys: String, CodingKey { case provider, model, error; case durationMilliseconds = "duration_ms" }
}
public struct InboxMessageProjectionV1: Codable, Equatable, Identifiable, Sendable {
    public let id: InboxMessageIDV1; public let origin: TerminalOriginHandleV1
    public let sessionLabel: String; public let attention: String; public let summary: String
    public let source: String?; public let confidence: String?
    public let createdAtMilliseconds: UInt64; public let revision: UInt64; public let unread: Bool
    public let reply: String?; public let draft: String?; public let draftStatus: InboxDraftStatusV1
    public let staleContext: Bool; public let aiInvocation: AIInvocationProjectionV1?
    enum CodingKeys: String, CodingKey { case id, origin, attention, source, confidence, summary, revision, unread, reply, draft; case sessionLabel = "session_label", createdAtMilliseconds = "created_at_ms", draftStatus = "draft_status", staleContext = "stale_context", aiInvocation = "ai_invocation" }
}
public struct InboxPageV1: Codable, Equatable, Sendable {
    public let overview: InboxOverviewV1?
    public let messages: [InboxMessageProjectionV1]; public let nextBeforeCreatedAtMilliseconds: UInt64?; public let unreadCount: UInt32
    public init(messages: [InboxMessageProjectionV1], nextBeforeCreatedAtMilliseconds: UInt64?, unreadCount: UInt32, overview: InboxOverviewV1? = nil) { self.overview = overview; self.messages = messages; self.nextBeforeCreatedAtMilliseconds = nextBeforeCreatedAtMilliseconds; self.unreadCount = unreadCount }
    enum CodingKeys: String, CodingKey { case messages, overview; case nextBeforeCreatedAtMilliseconds = "next_before_created_at_ms"; case unreadCount = "unread_count" }
}
public enum InboxCommandV1: Encodable, Equatable, Sendable {
    case observeAttention(origin: TerminalOriginHandleV1)
    case getPage(before: UInt64?, limit: UInt16)
    case reply(messageID: InboxMessageIDV1, expectedRevision: UInt64, reply: String)
    case regenerate(messageID: InboxMessageIDV1, expectedRevision: UInt64)
    case editDraft(messageID: InboxMessageIDV1, expectedRevision: UInt64, draft: String)
    case approve(messageID: InboxMessageIDV1, expectedRevision: UInt64, deliveryMode: TerminalDeliveryModeV1, approvalNonce: String)
    case recordDelivery(messageID: InboxMessageIDV1, expectedRevision: UInt64, state: DeliveryReceiptStateV1)
    case acknowledge(messageID: InboxMessageIDV1, expectedRevision: UInt64)
    enum CodingKeys: String, CodingKey { case kind, origin, messageID = "message_id", expectedRevision = "expected_revision", before = "before_created_at_ms", limit, reply, draft, deliveryMode = "delivery_mode", approvalNonce = "approval_nonce", state }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .observeAttention(let origin): try c.encode("observe_attention", forKey: .kind); try c.encode(origin, forKey: .origin)
        case .getPage(let before, let limit): try c.encode("get_page", forKey: .kind); try c.encodeIfPresent(before, forKey: .before); try c.encode(limit, forKey: .limit)
        case .reply(let id, let revision, let reply): try c.encode("reply", forKey: .kind); try c.encode(id, forKey: .messageID); try c.encode(revision, forKey: .expectedRevision); try c.encode(reply, forKey: .reply)
        case .regenerate(let id, let revision): try c.encode("regenerate", forKey: .kind); try c.encode(id, forKey: .messageID); try c.encode(revision, forKey: .expectedRevision)
        case .editDraft(let id, let revision, let draft): try c.encode("edit_draft", forKey: .kind); try c.encode(id, forKey: .messageID); try c.encode(revision, forKey: .expectedRevision); try c.encode(draft, forKey: .draft)
        case .approve(let id, let revision, let mode, let nonce): try c.encode("approve", forKey: .kind); try c.encode(id, forKey: .messageID); try c.encode(revision, forKey: .expectedRevision); try c.encode(mode, forKey: .deliveryMode); try c.encode(nonce, forKey: .approvalNonce)
        case .recordDelivery(let id, let revision, let state): try c.encode("record_delivery", forKey: .kind); try c.encode(id, forKey: .messageID); try c.encode(revision, forKey: .expectedRevision); try c.encode(state, forKey: .state)
        case .acknowledge(let id, let revision): try c.encode("acknowledge", forKey: .kind); try c.encode(id, forKey: .messageID); try c.encode(revision, forKey: .expectedRevision)
        }
    }
}
public struct InboxRequestEnvelopeV1: Encodable, Equatable, Sendable {
    public let protocolVersion: UInt32 = polarTmuxProtocolVersion; public let api = "inbox_v1"; public let requestID: String; public let command: InboxCommandV1
    public init(requestID: String, command: InboxCommandV1) { self.requestID = requestID; self.command = command }
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version", api, requestID = "request_id", command }
}
public enum InboxResultV1: Decodable, Equatable, Sendable {
    case page(InboxPageV1); case message(InboxMessageProjectionV1)
    case deliveryRequested(InboxMessageIDV1, TerminalOriginHandleV1, String, TerminalDeliveryModeV1, String)
    case deliveryReceipt(InboxMessageIDV1, DeliveryReceiptStateV1)
    case error(PolarTmuxProtocolError, InboxMessageProjectionV1?)
    enum CodingKeys: String, CodingKey { case kind, page, message, messageID = "message_id", origin, text, deliveryMode = "delivery_mode", approvalNonce = "approval_nonce", state, error }
    public init(from decoder: Decoder) throws { let c = try decoder.container(keyedBy: CodingKeys.self); switch try c.decode(String.self, forKey: .kind) { case "page": self = try .page(c.decode(InboxPageV1.self, forKey: .page)); case "message": self = try .message(c.decode(InboxMessageProjectionV1.self, forKey: .message)); case "delivery_requested": self = try .deliveryRequested(c.decode(InboxMessageIDV1.self, forKey: .messageID), c.decode(TerminalOriginHandleV1.self, forKey: .origin), c.decode(String.self, forKey: .text), c.decode(TerminalDeliveryModeV1.self, forKey: .deliveryMode), c.decode(String.self, forKey: .approvalNonce)); case "delivery_receipt": self = try .deliveryReceipt(c.decode(InboxMessageIDV1.self, forKey: .messageID), c.decode(DeliveryReceiptStateV1.self, forKey: .state)); case "error": self = try .error(c.decode(PolarTmuxProtocolError.self, forKey: .error), c.decodeIfPresent(InboxMessageProjectionV1.self, forKey: .message)); default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "unknown Inbox result") } }
}
public struct InboxResponseEnvelopeV1: Decodable, Equatable, Sendable { public let protocolVersion: UInt32; public let api: String; public let requestID: String; public let eventRevision: UInt64; public let result: InboxResultV1; enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version", api, requestID = "request_id", eventRevision = "event_revision", result } }

public struct InboxOverviewV1: Codable, Equatable, Sendable {
    public let activity: [InboxOverviewRowV1]
    public let records: [InboxOverviewRowV1]
    public let errors: [String]
}
public struct InboxOverviewRowV1: Codable, Equatable, Identifiable, Sendable {
    public let id: String; public let source: String; public let executionTargetID: String
    public let title: String; public let taskStatus: String; public let taskLabel: String
    public let observationLabel: String; public let stale: Bool; public let summary: String
    public let createdAtMilliseconds: UInt64; public let session: PolarTmuxSessionKey?
    public let terminalMessageID: InboxMessageIDV1?
    public let codexOwnerID: String?; public let codexBindingID: String?
    public let deliveries: [CodexDeliveryV1]
    enum CodingKeys: String, CodingKey {
        case id, source, title, stale, summary, session, deliveries
        case executionTargetID = "execution_target_id", taskStatus = "task_status", taskLabel = "task_label"
        case observationLabel = "observation_label", createdAtMilliseconds = "created_at_ms"
        case terminalMessageID = "terminal_message_id", codexOwnerID = "codex_owner_id", codexBindingID = "codex_binding_id"
    }
}
