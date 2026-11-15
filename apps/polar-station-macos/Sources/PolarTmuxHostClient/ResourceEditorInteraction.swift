import Foundation
import CPolarStationDesktopClient

/// The native form only adapts values and effects to the existing catalog API.
@MainActor public final class ResourceEditorInteraction: ObservableObject {
    public struct ViewState: Decodable {
        public let catalog: ResourceCatalogV1
        public let loaded: Bool
        public let dirty: Bool
        public let can_apply: Bool
        public let selected: String?
        public let confirmation: String?
        public let confirmation_title: String
        public let discard_label: String
        public let keep_label: String
        public let message: String?
    }
    public struct Effect: Decodable {
        public let type: String
        public let catalog: ResourceCatalogV1?
    }
    private struct Output: Decodable { let view: ViewState; let effects: [Effect] }
    private final class Handle {
        let pointer = polar_station_resource_editor_new()
        deinit { polar_station_resource_editor_free(pointer) }
    }
    private let owner = Handle()
    @Published public private(set) var state: ViewState?
    public init() { _ = send(["type":"keep_editing"]) }
    @discardableResult public func send(_ input: [String:Any]) -> [Effect] {
        guard let bytes = try? JSONSerialization.data(withJSONObject:input) else { return [] }
        return bytes.withUnsafeBytes { buffer in
            guard let result = polar_station_resource_editor_dispatch(owner.pointer, buffer.bindMemory(to:UInt8.self).baseAddress, buffer.count) else { return [] }
            defer { polar_station_desktop_string_free(result) }
            guard let output = try? JSONDecoder().decode(Output.self, from:Data(String(cString:result).utf8)) else { return [] }
            state = output.view
            return output.effects
        }
    }
    public func prefill(requestID:String, access:ResourceFileAccessV1) {
        guard let target=access.executionTargetID,let remote=access.remoteDirectory else { return }
        send(["type":"prefill","prefill":["request_id":requestID,"execution_target_id":target,"remote_directory":remote,"source_path":access.sourcePath ?? remote]])
    }
    public func snapshot(_ catalog: ResourceCatalogV1) { sendCatalog("snapshot", catalog) }
    public func change(_ catalog: ResourceCatalogV1) { sendCatalog("change", catalog) }
    private func sendCatalog(_ type: String, _ catalog: ResourceCatalogV1) {
        guard let bytes = try? JSONEncoder().encode(catalog), let value = try? JSONSerialization.jsonObject(with:bytes) else { return }
        send(["type":type,"catalog":value])
    }
}
