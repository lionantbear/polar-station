import Foundation

public struct HostControlVersionsV1: Codable, Equatable, Sendable {
    public var codex: UInt16?
    public var notification: UInt16?
}

public struct ImessageSenderSettingsV1: Codable, Equatable, Sendable {
    public var revision: UInt64 = 0
    public var enabled = false
    public var recipientCredentialRef: String?
    public var senderGeneration = ""
    public var allowedSourcePrincipals: [String] = []
    public init() {}
    enum CodingKeys: String, CodingKey {
        case revision, enabled
        case recipientCredentialRef = "recipient_credential_ref"
        case senderGeneration = "sender_generation"
        case allowedSourcePrincipals = "allowed_source_principals"
    }
}
public struct NotificationSenderStatusV1: Decodable, Equatable, Sendable {
    public let hostID: String
    public let supported: Bool
    public let enabled: Bool
    public let authorized: Bool
    public let senderGeneration: String
    public let errorCode: String?
    enum CodingKeys: String, CodingKey {
        case supported, enabled, authorized
        case hostID = "host_id", senderGeneration = "sender_generation", errorCode = "error_code"
    }
}
public struct NotificationReceiptV1: Decodable, Equatable, Sendable {
    public let requestID: String
    public let senderGeneration: String
    public let status: String
    public let errorCode: String?
    enum CodingKeys: String, CodingKey { case status; case requestID = "request_id", senderGeneration = "sender_generation", errorCode = "error_code" }
}
public enum NotificationCommandV1: Encodable, Equatable, Sendable {
    case connector(ConnectorCommandV1)
    case getSenderSettings
    case setSenderRecipient(expectedRevision: UInt64, recipient: String)
    case updateSenderSettings(expectedRevision: UInt64, settings: ImessageSenderSettingsV1)
    case getSenderStatus
    case requestMessagesAuthorization
    enum CodingKeys: String, CodingKey { case kind, settings, recipient, command; case expectedRevision = "expected_revision" }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .connector(let command): try c.encode("connector", forKey: .kind); try c.encode(command, forKey: .command)
        case .setSenderRecipient(let revision, let recipient):
            try c.encode("set_sender_recipient", forKey: .kind)
            try c.encode(revision, forKey: .expectedRevision)
            try c.encode(recipient, forKey: .recipient)
        case .getSenderSettings: try c.encode("get_sender_settings", forKey: .kind)
        case .getSenderStatus: try c.encode("get_sender_status", forKey: .kind)
        case .requestMessagesAuthorization: try c.encode("request_messages_authorization", forKey: .kind)
        case .updateSenderSettings(let revision, let settings):
            try c.encode("update_sender_settings", forKey: .kind)
            try c.encode(revision, forKey: .expectedRevision)
            try c.encode(settings, forKey: .settings)
        }
    }
}
public struct NotificationRequestEnvelopeV1: Encodable, Equatable, Sendable {
    public let protocolVersion: UInt32 = polarTmuxProtocolVersion
    public let api = "notification_v1"
    public let requestID: String
    public let command: NotificationCommandV1
    public init(requestID: String, command: NotificationCommandV1) { self.requestID = requestID; self.command = command }
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version", api; case requestID = "request_id", command }
}
public enum NotificationResultV1: Decodable, Equatable, Sendable {
    case connector(ConnectorResultV1)
    case settings(ImessageSenderSettingsV1)
    case status(NotificationSenderStatusV1)
    case receipt(NotificationReceiptV1)
    case error(String)
    enum CodingKeys: String, CodingKey { case kind, settings, status, receipt, code, result }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "connector": self = try .connector(c.decode(ConnectorResultV1.self, forKey: .result))
        case "settings": self = try .settings(c.decode(ImessageSenderSettingsV1.self, forKey: .settings))
        case "status": self = try .status(c.decode(NotificationSenderStatusV1.self, forKey: .status))
        case "receipt": self = try .receipt(c.decode(NotificationReceiptV1.self, forKey: .receipt))
        case "error": self = try .error(c.decode(String.self, forKey: .code))
        default: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "Unknown notification result")
        }
    }
}
public struct NotificationResponseEnvelopeV1: Decodable, Equatable, Sendable {
    public let protocolVersion: UInt32
    public let api: String
    public let requestID: String
    public let result: NotificationResultV1
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol_version", api; case requestID = "request_id", result }
}
