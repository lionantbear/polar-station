import Foundation
import CPolarStationDesktopClient

/// Native adapter for the shared Rust directory and destination policy.
@MainActor public final class SettingsNavigation: ObservableObject {
    public struct Route: Codable, Equatable {
        public var page: String
        public var remote: Bool?
        public var section: String?
        public var target: String?
        public var object: String?
        public var resource: String?
        public init(page: String, remote: Bool? = nil, section: String? = nil, target: String? = nil, object: String? = nil, resource: String? = nil) {
            self.page = page; self.remote = remote; self.section = section
            self.target = target; self.object = object; self.resource = resource
        }
    }
    public struct Entry: Decodable, Identifiable {
        public let title: String
        public let subtitle: String
        public let symbol: String
        public let route: Route
        public var id: String { "\(route.page):\(route.remote.map(String.init) ?? "")" }
    }
    public struct ViewState: Decodable {
        public let is_directory: Bool
        public let visible: Bool
        public let route: Route
        public let title: String
        public let can_back: Bool
        public let back_label:String
        public let scopes: [Entry]
        public let entries: [Entry]
    }
    private final class Handle {
        let pointer = polar_station_settings_navigation_new()
        deinit { polar_station_settings_navigation_free(pointer) }
    }
    private let owner = Handle()
    @Published public private(set) var state: ViewState?
    public var onPresent: (@MainActor () -> Void)?
    public var requestEditorClose: (@MainActor () -> Void)?
    public init() { dispatch(["type":"show_layout"]) }
    public func open(_ route: Route) {
        guard let object = try? JSONSerialization.jsonObject(with: JSONEncoder().encode(route)) else { return }
        dispatch(["type":"open", "route":object])
        onPresent?()
    }
    public func navigate(_ route:Route) {
        guard let object = try? JSONSerialization.jsonObject(with:JSONEncoder().encode(route)) else { return }
        dispatch(["type":"navigate","route":object])
    }
    public func back() {
        if let requestEditorClose { requestEditorClose() }
        else { completeBack() }
    }
    public func completeBack() { dispatch(["type":"back"]) }
    public func show() { dispatch(["type":"show"]); onPresent?() }
    public func showLayout() { dispatch(["type":"show_layout"]) }
    private func dispatch(_ input: [String:Any]) {
        guard let bytes = try? JSONSerialization.data(withJSONObject:input) else { return }
        bytes.withUnsafeBytes { buffer in
            guard let pointer = polar_station_settings_navigation_dispatch(owner.pointer, buffer.bindMemory(to:UInt8.self).baseAddress, buffer.count) else { return }
            defer { polar_station_desktop_string_free(pointer) }
            if let next = try? JSONDecoder().decode(ViewState.self, from:Data(String(cString:pointer).utf8)) { state = next }
        }
    }
}
