import Foundation

public enum PolarTmuxLocalIPCError: Error, Equatable, LocalizedError, Sendable {
    case emptyFrame
    case frameTooLarge(Int)
    case invalidJSON
    case decodingFailed(frameType: String?, codingPath: String)
    case connectionClosed
    case connectionFailed(String)
    case unexpectedFrame(String)
    case protocolVersionMismatch(expected: UInt32, actual: UInt32)
    case requestIDMismatch(expected: String, actual: String)
    case executionTargetMismatch(expected: String, actual: String)
    case duplicateRequestID(String)
    case duplicateRegistration
    case unsupportedProtocolVersion(UInt32)
    case requestTimedOut(String)

    public var errorDescription: String? {
        switch self {
        case .emptyFrame:
            "Polar Tmux rejected an empty local IPC frame"
        case .frameTooLarge(let size):
            "Polar Tmux rejected a \(size)-byte local IPC frame"
        case .invalidJSON:
            "Polar Tmux received invalid local IPC JSON"
        case .decodingFailed(let frameType, let codingPath):
            "The Polar Tmux Host sent an incompatible "
                + "\(frameType ?? "unknown") frame at \(codingPath)"
        case .connectionClosed:
            "The Polar Tmux Host closed the local IPC connection"
        case .connectionFailed(let cause):
            "The Polar Tmux Host local IPC connection failed: \(cause)"
        case .unexpectedFrame(let type):
            "Polar Tmux received an unexpected \(type) frame"
        case .protocolVersionMismatch:
            "Polar Tmux received a response for another protocol version"
        case .requestIDMismatch:
            "Polar Tmux received a response for another request"
        case .executionTargetMismatch:
            "Polar Tmux received a response for another execution target"
        case .duplicateRequestID:
            "Polar Tmux rejected a duplicate in-flight request ID"
        case .duplicateRegistration:
            "Polar Tmux already has a registration request in flight"
        case .unsupportedProtocolVersion(let version):
            "Polar Tmux does not support Host protocol version \(version)"
        case .requestTimedOut(let requestID):
            "Polar Station backend request \(requestID) timed out"
        }
    }
}

public enum PolarTmuxFrameCodec {
    public static let maximumPayloadBytes = 1_048_576

    public static func encode(_ payload: Data) throws -> Data {
        guard !payload.isEmpty else {
            throw PolarTmuxLocalIPCError.emptyFrame
        }
        guard payload.count <= maximumPayloadBytes else {
            throw PolarTmuxLocalIPCError.frameTooLarge(payload.count)
        }
        let length = UInt32(payload.count)
        var frame = Data(capacity: payload.count + 4)
        frame.append(UInt8((length >> 24) & 0xff))
        frame.append(UInt8((length >> 16) & 0xff))
        frame.append(UInt8((length >> 8) & 0xff))
        frame.append(UInt8(length & 0xff))
        frame.append(payload)
        return frame
    }

    public static func encode(_ value: some Encodable) throws -> Data {
        try encode(JSONEncoder().encode(value))
    }
}

public struct PolarTmuxFrameDecoder: Sendable {
    private var buffer = Data()

    public init() {}

    public mutating func append(_ bytes: some DataProtocol) throws -> [Data] {
        buffer.append(contentsOf: bytes)
        var frames: [Data] = []
        while buffer.count >= 4 {
            let length = Int(buffer.prefix(4).reduce(UInt32(0)) {
                ($0 << 8) | UInt32($1)
            })
            guard length > 0 else {
                throw PolarTmuxLocalIPCError.emptyFrame
            }
            guard length <= PolarTmuxFrameCodec.maximumPayloadBytes else {
                throw PolarTmuxLocalIPCError.frameTooLarge(length)
            }
            guard buffer.count >= length + 4 else {
                break
            }
            frames.append(buffer.subdata(in: 4..<(length + 4)))
            buffer.removeSubrange(0..<(length + 4))
        }
        return frames
    }
}

public enum PolarTmuxSocketPath {
    public static func resolve(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        temporaryDirectory: URL = FileManager.default.temporaryDirectory
    ) -> URL {
        if let override = environment["POLAR_TMUX_IPC_SOCKET"]?
            .trimmingCharacters(in: .whitespacesAndNewlines),
           !override.isEmpty {
            return URL(fileURLWithPath: override)
        }
        return temporaryDirectory
            .appendingPathComponent("polar-tmux", isDirectory: true)
            .appendingPathComponent("host.sock", isDirectory: false)
    }
}
