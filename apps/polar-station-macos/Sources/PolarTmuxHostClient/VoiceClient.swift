import Foundation
import CPolarStationDesktopClient

/// Local Host transport only; recording and inference live in the Rust service.
public final class VoiceClient: @unchecked Sendable {
    private let handle: OpaquePointer?
    public init() { handle = polar_station_dictate_speech_new() }
    deinit { polar_station_dictate_speech_free(handle) }
    public func request(_ data: Data) async -> Data {
        await Task.detached { [self] in
            data.withUnsafeBytes { bytes in
                guard let reply = polar_station_dictate_service_request(handle,
                    bytes.bindMemory(to: UInt8.self).baseAddress, data.count) else {
                    return Data(#"{"ok":false,"error":"Voice client is unavailable"}"#.utf8)
                }
                defer { polar_station_desktop_string_free(reply) }
                return Data(bytes: reply, count: strlen(reply))
            }
        }.value
    }
}
