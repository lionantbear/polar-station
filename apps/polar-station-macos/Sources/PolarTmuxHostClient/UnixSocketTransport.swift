import Foundation

public struct DesktopRequestFailure: Decodable, LocalizedError, Sendable {
    public let request_id: String
    public let api: String
    public let code: String
    public let message: String
    public let outcome_unknown: Bool
    public var errorDescription: String? { message }
}
public enum PolarTmuxLocalIPCTransportEvent: Sendable {
    case requestFailed(generation: UInt64, failure: DesktopRequestFailure)
    case content(PolarTmuxPaneContentResponse)
    case session(Data)
    case connecting(generation: UInt64, attempt: Int)
    case ready(generation: UInt64, frame: Data)
    case frame(generation: UInt64?, data: Data)
    case unavailable(
        generation: UInt64,
        attempt: Int,
        message: String,
        retryAfterMilliseconds: UInt64
    )
}

public protocol PolarTmuxLocalIPCTransport: Sendable {
    func connect() async throws
    func send(_ data: Data) async throws
    func receive(maximumBytes: Int) async throws -> Data
    func receiveEvent(maximumBytes: Int) async throws
        -> PolarTmuxLocalIPCTransportEvent
    func cancelRequest(id: String, generation: UInt64) async
    func retryNow() async
    func close() async
}

extension PolarTmuxLocalIPCTransport {

    public func receiveEvent(maximumBytes: Int) async throws
        -> PolarTmuxLocalIPCTransportEvent
    {
        .frame(
            generation: nil,
            data: try await receive(maximumBytes: maximumBytes)
        )
    }

    public func cancelRequest(id: String, generation: UInt64) async {}
    public func retryNow() async {}
}
