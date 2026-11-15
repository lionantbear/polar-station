import CPolarStationDesktopClient
import Foundation

/// Native milestones feed the shared bounded recorder; identities contain no terminal text.
public struct PolarTmuxDesktopSendError: LocalizedError, Sendable {
    public let reason: String
    public var errorDescription: String? {
        switch reason {
        case "not_ready": return "Station Host is reconnecting. Retry when it is ready."
        case "queue_full": return "Station Host request queue is full. Retry shortly."
        case "unsupported": return "Station Host did not negotiate this capability."
        case "invalid_request": return "Station Host rejected an invalid request."
        default: return "Station Host client is unavailable."
        }
    }
    public init(code: UInt32) {
        reason = [1:"not_ready", 2:"queue_full", 3:"unsupported", 4:"invalid_request", 5:"unavailable"][code] ?? "unavailable"
    }
}

public enum StationOperationTrace {
    public static func record(_ stage: UInt32, identity: String = "", generation: UInt64 = 0) {
        let bytes = Array(identity.utf8.prefix(128))
        bytes.withUnsafeBufferPointer {
            polar_station_trace_native(stage, $0.baseAddress, $0.count, generation)
        }
    }
}

private let polarStationRustWake:
    @convention(c) (UnsafeMutableRawPointer?) -> Void = { context in
        guard let context else { return }
        Unmanaged<PolarTmuxRustDesktopTransport>
            .fromOpaque(context)
            .takeUnretainedValue()
            .wakeReceiver()
    }

/// JSON delivery adapter over the single shared Rust desktop client.
/// Socket framing, request lifecycle and registration readiness belong to Rust.
public final class PolarTmuxRustDesktopTransport:
    PolarTmuxLocalIPCTransport,
    @unchecked Sendable
{
    private let socketURL: URL
    private let condition = NSCondition()
    private final class Handle: @unchecked Sendable {
        let pointer: OpaquePointer
        init(_ pointer: OpaquePointer) { self.pointer = pointer }
        deinit { polar_station_desktop_client_stop(pointer); polar_station_desktop_client_free(pointer) }
    }
    // A captured owner keeps the C handle alive across native send/cancel/close races.
    private var handle: Handle?
    private var connected = false
    private var closed = false
    private var wakeSerial: UInt64 = 0

    public init(socketURL: URL) {
        self.socketURL = socketURL
    }


    deinit {
        closeNow()
    }

    public func connect() async throws {
        markConnected()
    }

    public func send(_ data: Data) async throws {
        let payload = data

        let (active, isConnected) = connectionSnapshot()
        guard isConnected else {
            throw PolarTmuxLocalIPCError.connectionClosed
        }
        if let active {
            let json = String(decoding: payload, as: UTF8.self)
            let sent = json.withCString {
                polar_station_desktop_client_send_json_status(active.pointer, $0)
            }
            guard sent == 0 else { throw PolarTmuxDesktopSendError(code: sent) }
            return
        }

        let registration = try JSONSerialization.jsonObject(with: payload)
        guard let fields = registration as? [String: Any],
              fields["type"] as? String == "register"
        else {
            throw PolarTmuxLocalIPCError.duplicateRegistration
        }
        let config: [String: Any] = [
            "socket_path": socketURL.path,
            "registration": fields,
            "event_capacity": 256,
        ]
        let configData = try JSONSerialization.data(withJSONObject: config)
        let configJSON = String(decoding: configData, as: UTF8.self)
        let context = Unmanaged.passUnretained(self).toOpaque()
        guard let created = configJSON.withCString({ pointer in
            polar_station_desktop_client_new(
                pointer,
                polarStationRustWake,
                context
            )
        }) else {
            throw PolarTmuxLocalIPCError.connectionFailed(
                "shared Rust desktop client rejected its configuration"
            )
        }
        guard polar_station_desktop_client_start(created) else {
            polar_station_desktop_client_free(created)
            throw PolarTmuxLocalIPCError.connectionFailed(
                "shared Rust desktop client could not start"
            )
        }
        if !install(created) {
            polar_station_desktop_client_stop(created)
            polar_station_desktop_client_free(created)
            throw PolarTmuxLocalIPCError.connectionClosed
        }
    }

    public func receive(maximumBytes: Int) async throws -> Data {
        while true {
            switch try await receiveEvent(maximumBytes: maximumBytes) {
            case .ready(_, let frame), .frame(_, let frame):
                return frame
            case .connecting, .unavailable, .content, .requestFailed, .session:
                continue
            }
        }
    }

    public func receiveEvent(maximumBytes: Int) async throws
        -> PolarTmuxLocalIPCTransportEvent
    {
        try await Task.detached(priority: .userInitiated) { [self] in
            let event = try decodeDesktopEvent(waitForEventJSON())
            let frameSize: Int
            switch event {
            case .ready(_, let frame), .frame(_, let frame), .session(let frame):
                frameSize = frame.count
            case .connecting, .unavailable, .content, .requestFailed:
                frameSize = 0
            }
            guard frameSize <= maximumBytes else {
                throw PolarTmuxLocalIPCError.frameTooLarge(frameSize)
            }
            return event
        }.value
    }

    public func sendSession(_ payload: Data) throws {
        guard let active = activeHandle() else { throw PolarTmuxLocalIPCError.connectionClosed }
        let accepted = String(decoding:payload,as:UTF8.self).withCString { polar_station_desktop_client_session_json(active.pointer,$0) }
        guard accepted else { throw PolarTmuxDesktopSendError(code:2) }
    }

    public func sendContent(socketURL: URL, request: PolarTmuxPaneContentRequest) throws {
        guard let active = activeHandle() else { throw PolarTmuxLocalIPCError.connectionClosed }
        let data = try JSONSerialization.data(withJSONObject: ["socket_path": socketURL.path, "request": JSONSerialization.jsonObject(with: JSONEncoder().encode(request))])
        let accepted = String(decoding: data, as: UTF8.self).withCString { polar_station_desktop_client_send_content_json(active.pointer, $0) }
        guard accepted else { throw PolarTmuxLocalIPCError.connectionClosed }
    }

    public func cancelRequest(id: String, generation: UInt64) async {
        if let active = activeHandle() {
            id.withCString { polar_station_desktop_client_cancel_request(active.pointer, generation, $0) }
        }
    }

    public func retryNow() async {
        if let active = activeHandle() {
            _ = polar_station_desktop_client_retry_now(active.pointer)
        }
    }

    public func close() async {
        closeNow()
    }

    fileprivate func wakeReceiver() {
        condition.lock()
        wakeSerial &+= 1
        condition.broadcast()
        condition.unlock()
    }

    private func markConnected() {
        condition.lock()
        closed = false
        connected = true
        condition.unlock()
    }

    private func connectionSnapshot() -> (Handle?, Bool) {
        condition.lock()
        defer { condition.unlock() }
        return (handle, connected && !closed)
    }

    private func activeHandle() -> Handle? {
        condition.lock()
        defer { condition.unlock() }
        return closed ? nil : handle
    }

    private func install(_ created: OpaquePointer) -> Bool {
        condition.lock()
        defer { condition.unlock() }
        guard !closed, handle == nil else { return false }
        handle = Handle(created)
        condition.broadcast()
        return true
    }

    private func waitForEventJSON() throws -> String {
        while true {
            condition.lock()
            while handle == nil && !closed { condition.wait() }
            guard !closed, let active = handle else {
                condition.unlock(); throw PolarTmuxLocalIPCError.connectionClosed
            }
            let observedWake = wakeSerial
            condition.unlock()
            // Rust cancellation can invoke wake while holding its client lock.
            // Never hold the native condition across a Rust call; the serial
            // prevents a wake between pop and wait from being lost.
            if let raw = polar_station_desktop_client_pop_event_json(active.pointer) {
                let json = String(cString: raw)
                polar_station_desktop_string_free(raw)
                return json
            }
            condition.lock()
            if !closed && observedWake == wakeSerial { condition.wait() }
            condition.unlock()
        }
    }

    private func decodeDesktopEvent(_ json: String) throws
        -> PolarTmuxLocalIPCTransportEvent
    {
        let data = Data(json.utf8)
        if let object = try JSONSerialization.jsonObject(with:data) as? [String:Any], object["type"] as? String == "session_completed" { return .session(data) }
        if let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
           object["type"] as? String == "content_completed", let response = object["response"] {
            return .content(try JSONDecoder().decode(PolarTmuxPaneContentResponse.self, from: JSONSerialization.data(withJSONObject: response)))
        }
        guard let event = try JSONSerialization.jsonObject(with: data)
                as? [String: Any],
              let type = event["type"] as? String,
              let generation = (event["generation"] as? NSNumber)?.uint64Value
        else {
            throw PolarTmuxLocalIPCError.invalidJSON
        }
        switch type {
        case "request_failed":
            let failure = try JSONDecoder().decode(DesktopRequestFailure.self,
                from: JSONSerialization.data(withJSONObject: event["failure"] as Any))
            return .requestFailed(generation: generation, failure: failure)
        case "connecting":
            guard let attempt = (event["attempt"] as? NSNumber)?.intValue else {
                throw PolarTmuxLocalIPCError.invalidJSON
            }
            return .connecting(generation: generation, attempt: attempt)
        case "ready":
            guard let registrationID = event["registration_id"] as? String,
                  let hostID = event["host_id"] as? String
            else {
                throw PolarTmuxLocalIPCError.invalidJSON
            }
            var hostFrame: [String: Any] = [
                "type": "registered",
                "registration_id": registrationID,
                "protocol_version": polarTmuxProtocolVersion,
                "host_id": hostID,
            ]
            if let versions = event["host_control_versions"] { hostFrame["host_control_versions"] = versions }
            if let extensions = event["extensions"] { hostFrame["extensions"] = extensions }
            return .ready(
                generation: generation,
                frame: try JSONSerialization.data(withJSONObject: hostFrame)
            )
        case "frame":
            guard let frame = event["frame"] as? [String: Any] else {
                throw PolarTmuxLocalIPCError.invalidJSON
            }
            return .frame(
                generation: generation,
                data: try JSONSerialization.data(withJSONObject: frame)
            )
        case "unavailable":
            guard let attempt = (event["attempt"] as? NSNumber)?.intValue,
                  let message = event["message"] as? String,
                  let retryAfter = (event["retry_after_ms"] as? NSNumber)?
                    .uint64Value
            else {
                throw PolarTmuxLocalIPCError.invalidJSON
            }
            return .unavailable(
                generation: generation,
                attempt: attempt,
                message: message,
                retryAfterMilliseconds: retryAfter
            )
        default:
            throw PolarTmuxLocalIPCError.invalidJSON
        }
    }

    private func closeNow() {
        condition.lock()
        guard !closed else {
            condition.unlock()
            return
        }
        closed = true
        connected = false
        let active = handle
        handle = nil
        condition.broadcast()
        condition.unlock()
        withExtendedLifetime(active) {}
    }
}
