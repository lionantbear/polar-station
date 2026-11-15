import Foundation

public enum TerminalDeliveryModeV1: String, Codable, Equatable, Sendable {
    case insert
    case send
}

public enum ChatSendModeV1: String, Codable, Equatable, Sendable { case enter; case modifiedEnter = "modified_enter" }

public struct ChatCachePolicy: Codable, Equatable, Sendable {
    public var messageMiB: UInt32 = 512
    public var presentationMiB: UInt32 = 32
    public init() {}
    enum CodingKeys: String, CodingKey { case messageMiB = "message_mib"; case presentationMiB = "presentation_mib" }
}

public struct ThemeSettingsV1: Codable, Equatable, Sendable {
    public var mode: String = "system"
    public var preset: String = "catppuccin_mocha"
    public var terminal_theme: String? = nil
    public init() {}
}
public struct ThemePaletteV1: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var name: String
    public var dark: Bool
    public var background: String
    public var foreground: String
    public var surface: String
    public var accent: String
    public var selection_background: String
    public var selection_foreground: String
}
public struct ThemeProjectionV1: Codable, Equatable, Sendable {
    public var choices: [ThemePaletteV1]
    public var light: ThemePaletteV1
    public var dark: ThemePaletteV1
}

public struct AppearanceSettingsV1: Codable, Equatable, Sendable {
    public var theme: ThemeSettingsV1
    public var layoutMapWidth: UInt16?
    public var layoutMapVisible: Bool
    public var inboxVisible: Bool
    public var inboxWidth: UInt16?
    public var lgpCache: ChatCachePolicy
    public var chatSendMode: ChatSendModeV1
    public var terminalDeliveryMode: TerminalDeliveryModeV1
    public var interfaceTextScalePercent: UInt16
    public init(
        layoutMapWidth: UInt16? = nil,
        layoutMapVisible: Bool = true,
        inboxVisible: Bool = false,
        inboxWidth: UInt16? = nil,
        terminalDeliveryMode: TerminalDeliveryModeV1 = .insert,
        interfaceTextScalePercent: UInt16 = 100,
        chatSendMode: ChatSendModeV1 = .enter,
        lgpCache: ChatCachePolicy = .init(),
        theme: ThemeSettingsV1 = .init()
    ) {
        self.theme = theme
        self.layoutMapWidth = layoutMapWidth
        self.layoutMapVisible = layoutMapVisible
        self.inboxVisible = inboxVisible
        self.inboxWidth = inboxWidth
        self.lgpCache = lgpCache
        self.chatSendMode = chatSendMode
        self.terminalDeliveryMode = terminalDeliveryMode
        self.interfaceTextScalePercent = interfaceTextScalePercent
    }
    enum CodingKeys: String, CodingKey {
        case theme
        case layoutMapWidth = "layout_map_width"
        case layoutMapVisible = "layout_map_visible"
        case inboxVisible = "inbox_visible"
        case inboxWidth = "inbox_width"
        case lgpCache = "lgp_cache"
        case chatSendMode = "chat_send_mode"
        case terminalDeliveryMode = "terminal_delivery_mode"
        case interfaceTextScalePercent = "interface_text_scale_percent"
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            layoutMapWidth: try c.decodeIfPresent(UInt16.self, forKey: .layoutMapWidth),
            layoutMapVisible: try c.decodeIfPresent(Bool.self, forKey: .layoutMapVisible) ?? true,
            inboxVisible: try c.decodeIfPresent(Bool.self, forKey: .inboxVisible) ?? false,
            inboxWidth: try c.decodeIfPresent(UInt16.self, forKey: .inboxWidth),
            terminalDeliveryMode: try c.decodeIfPresent(TerminalDeliveryModeV1.self, forKey: .terminalDeliveryMode) ?? .insert,
            interfaceTextScalePercent: try c.decodeIfPresent(UInt16.self, forKey: .interfaceTextScalePercent) ?? 100,
            chatSendMode: try c.decodeIfPresent(ChatSendModeV1.self, forKey: .chatSendMode) ?? .enter,
            lgpCache: try c.decodeIfPresent(ChatCachePolicy.self, forKey: .lgpCache) ?? .init(),
            theme: try c.decodeIfPresent(ThemeSettingsV1.self, forKey: .theme) ?? .init()
        )
    }
}

public struct AttentionSettingsV1: Codable, Equatable, Sendable {
    public var systemNotificationsEnabled: Bool
    public init(systemNotificationsEnabled: Bool = false) { self.systemNotificationsEnabled = systemNotificationsEnabled }
    enum CodingKeys: String, CodingKey { case systemNotificationsEnabled = "system_notifications_enabled" }
}

public struct MonitoringSettingsV1: Codable, Equatable, Sendable {
    public var enabled: Bool
    public var quietPeriodMilliseconds: UInt64
    public var codexTitleDetection: Bool
    public init(enabled: Bool = true, quietPeriodMilliseconds: UInt64 = 5_000, codexTitleDetection: Bool = true) {
        self.enabled = enabled
        self.quietPeriodMilliseconds = quietPeriodMilliseconds
        self.codexTitleDetection = codexTitleDetection
    }
    enum CodingKeys: String, CodingKey {
        case enabled
        case quietPeriodMilliseconds = "quiet_period_ms"
        case codexTitleDetection = "codex_title_detection"
    }
}

public struct SessionSummarySettingsV1: Codable, Equatable, Sendable {
    public var enabled: Bool
    public var command: String
    public var model: String?
    public var timeoutMilliseconds: UInt64
    public var quietPeriodMilliseconds: UInt64
    public init(enabled: Bool = false, command: String = "codex", model: String? = nil, timeoutMilliseconds: UInt64 = 15_000, quietPeriodMilliseconds: UInt64 = 5_000) {
        self.enabled = enabled; self.command = command; self.model = model; self.timeoutMilliseconds = timeoutMilliseconds; self.quietPeriodMilliseconds = quietPeriodMilliseconds
    }
    enum CodingKeys: String, CodingKey { case enabled, command, model; case timeoutMilliseconds = "timeout_ms"; case quietPeriodMilliseconds = "quiet_period_ms" }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            enabled: try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? false,
            command: try c.decodeIfPresent(String.self, forKey: .command) ?? "codex",
            model: try c.decodeIfPresent(String.self, forKey: .model),
            timeoutMilliseconds: try c.decodeIfPresent(UInt64.self, forKey: .timeoutMilliseconds) ?? 15_000,
            quietPeriodMilliseconds: try c.decodeIfPresent(UInt64.self, forKey: .quietPeriodMilliseconds) ?? 5_000
        )
    }
}

public struct SettingsProjectionV1: Codable, Equatable, Sendable {
    public var themeProjection: ThemeProjectionV1? = nil
    public let schemaVersion: UInt32
    public let revision: UInt64
    public var appearance: AppearanceSettingsV1
    public var attention: AttentionSettingsV1
    public var monitoring: MonitoringSettingsV1
    public var sessionSummary: SessionSummarySettingsV1
    public init(
        schemaVersion: UInt32 = 1,
        revision: UInt64 = 0,
        appearance: AppearanceSettingsV1 = .init(),
        attention: AttentionSettingsV1 = .init(),
        monitoring: MonitoringSettingsV1 = .init(),
        sessionSummary: SessionSummarySettingsV1 = .init()
    ) {
        self.schemaVersion = schemaVersion
        self.revision = revision
        self.appearance = appearance
        self.attention = attention
        self.monitoring = monitoring
        self.sessionSummary = sessionSummary
    }
    enum CodingKeys: String, CodingKey { case schemaVersion = "schema_version"; case revision, appearance, attention, monitoring; case sessionSummary = "session_summary" }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            schemaVersion: try c.decode(UInt32.self, forKey: .schemaVersion),
            revision: try c.decode(UInt64.self, forKey: .revision),
            appearance: try c.decode(AppearanceSettingsV1.self, forKey: .appearance),
            attention: try c.decode(AttentionSettingsV1.self, forKey: .attention),
            monitoring: try c.decodeIfPresent(MonitoringSettingsV1.self, forKey: .monitoring) ?? .init(),
            sessionSummary: try c.decode(SessionSummarySettingsV1.self, forKey: .sessionSummary)
        )
    }
}

public enum CodexNotifyConfigStateV1: String, Codable, Equatable, Sendable {
    case missing, configured, conflict, invalid, unsafe, unsupported
}

public enum SettingsCommandV1: Encodable, Equatable, Sendable {
    case getProjection
    case updateAppearance(AppearanceSettingsV1)
    case updateAttention(AttentionSettingsV1)
    case updateMonitoring(MonitoringSettingsV1)
    case updateSessionSummary(SessionSummarySettingsV1)
    case testSessionSummaryProvider(SessionSummarySettingsV1)
    case getCodexNotifyStatus(executionTargetID: String)
    case configureCodexNotify(executionTargetID: String)
    enum CodingKeys: String, CodingKey { case kind, value; case executionTargetID = "execution_target_id" }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .getProjection: try c.encode("get_projection", forKey: .kind)
        case .updateAppearance(let value): try c.encode("update_appearance", forKey: .kind); try c.encode(value, forKey: .value)
        case .updateAttention(let value): try c.encode("update_attention", forKey: .kind); try c.encode(value, forKey: .value)
        case .updateMonitoring(let value): try c.encode("update_monitoring", forKey: .kind); try c.encode(value, forKey: .value)
        case .updateSessionSummary(let value): try c.encode("update_session_summary", forKey: .kind); try c.encode(value, forKey: .value)
        case .testSessionSummaryProvider(let value): try c.encode("test_session_summary_provider", forKey: .kind); try c.encode(value, forKey: .value)
        case .getCodexNotifyStatus(let target): try c.encode("get_codex_notify_status", forKey: .kind); try c.encode(target, forKey: .executionTargetID)
        case .configureCodexNotify(let target): try c.encode("configure_codex_notify", forKey: .kind); try c.encode(target, forKey: .executionTargetID)
        }
    }
}

public struct SettingsRequestEnvelopeV1: Encodable, Equatable, Sendable {
    public let protocolVersion: UInt32 = polarTmuxProtocolVersion
    public let api = "settings_v1"
    public let requestID: String
    public let expectedRevision: UInt64?
    public let command: SettingsCommandV1
    public init(requestID: String, expectedRevision: UInt64? = nil, command: SettingsCommandV1) { self.requestID = requestID; self.expectedRevision = expectedRevision; self.command = command }
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version", api; case requestID = "request_id"; case expectedRevision = "expected_revision"; case command }
}

public enum SettingsResultV1: Codable, Equatable, Sendable {
    case projection(SettingsProjectionV1)
    case providerTested(String)
    case codexNotifyStatus(String, CodexNotifyConfigStateV1)
    case error(PolarTmuxProtocolError, SettingsProjectionV1?)
    enum CodingKeys: String, CodingKey { case kind, projection, summary, error, status; case themeProjection = "theme_projection"; case executionTargetID = "execution_target_id" }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "projection":
            var value = try c.decode(SettingsProjectionV1.self, forKey: .projection)
            value.themeProjection = try c.decodeIfPresent(ThemeProjectionV1.self, forKey: .themeProjection)
            self = .projection(value)
        case "provider_tested": self = try .providerTested(c.decode(String.self, forKey: .summary))
        case "codex_notify_status": self = try .codexNotifyStatus(c.decode(String.self, forKey: .executionTargetID), c.decode(CodexNotifyConfigStateV1.self, forKey: .status))
        case "error": self = try .error(c.decode(PolarTmuxProtocolError.self, forKey: .error), c.decodeIfPresent(SettingsProjectionV1.self, forKey: .projection))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "unknown Settings result")
        }
    }
    public func encode(to encoder: Encoder) throws { fatalError("Settings responses are decode-only") }
}

public struct SettingsResponseEnvelopeV1: Codable, Equatable, Sendable {
    public let protocolVersion: UInt32
    public let api: String
    public let requestID: String
    public let revision: UInt64
    public let result: SettingsResultV1
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version", api; case requestID = "request_id"; case revision, result }
}
