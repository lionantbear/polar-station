import Foundation
import CPolarStationDesktopClient

/// Voice RPC and ABI delivery only. Rust owns the prompt draft and save/close policy.
@MainActor public final class SettingsTextInteraction: ObservableObject {
    public struct Field:Decodable,Identifiable {
        public let field:String
        public let title:String
        public var id:String {field}
    }
    public struct ViewState: Decodable {
        public struct Actions:Decodable {
            public let loading:String
            public let save:String
            public let restore:String
            public let expand:String
            public let collapse:String
            public let focus:String
            public let cancel:String
            public let discard:String
            public let keep:String
            public let unsaved:String
            public let refresh:String
            public let rebase:String
        }
        public let fields:[Field]
        public let actions:Actions
        public let field: String?
        public let title: String
        public let text: String
        public let dirty: Bool
        public let pending: Bool
        public let loading:Bool
        public let expanded: Bool
        public let confirm_close: Bool
        public let confirmation:String
        public let can_edit: Bool
        public let can_save: Bool
        public let can_rebase:Bool
        public let error: String?
    }
    private final class Handle {
        let pointer = polar_station_settings_text_new()
        deinit { polar_station_settings_text_free(pointer) }
    }
    private let owner = Handle()
    private let voice = VoiceClient()
    private var closeCompletion:CheckedContinuation<Bool,Never>?
    /// Native lifecycle waits for the shared editor's close result, not a view disappearance.
    public func confirmClose() async -> Bool {
        guard state?.field != nil else {return true}
        guard closeCompletion == nil else {return false}
        return await withCheckedContinuation { continuation in
            closeCompletion=continuation
            dispatch(["type":"close"])
        }
    }
    private func finishClose(_ accepted:Bool) {
        let completion=closeCompletion;closeCompletion=nil;completion?.resume(returning:accepted)
    }
    @Published public private(set) var state: ViewState?
    public var expand: (@MainActor (String) -> Void)?
    public var collapse: (@MainActor () -> Void)?
    public var close: (@MainActor () -> Void)?
    public init() {dispatch(["type":"keep_editing"])}
    public func edit(_ field:String) { dispatch(["type":"edit","field":field]) }
    public func change(_ text:String) { dispatch(["type":"change","text":text]) }
    public func action(_ type:String) { dispatch(["type":type]) }
    private func dispatch(_ input:[String:Any]) {
        guard let bytes = try? JSONSerialization.data(withJSONObject:input) else { return }
        bytes.withUnsafeBytes { buffer in
            guard let pointer = polar_station_settings_text_dispatch(owner.pointer,buffer.bindMemory(to:UInt8.self).baseAddress,buffer.count) else { return }
            defer { polar_station_desktop_string_free(pointer) }
            let data = Data(String(cString:pointer).utf8)
            guard let output = try? JSONSerialization.jsonObject(with:data) as? [String:Any], let view=output["view"],
                  let viewData=try? JSONSerialization.data(withJSONObject:view), let next=try? JSONDecoder().decode(ViewState.self,from:viewData) else { return }
            state=next
            for effect in output["effects"] as? [[String:Any]] ?? [] {
                switch effect["type"] as? String {
                case "read_settings": Task { await readSettings() }
                case "save": if let settings=effect["settings"] { Task { await save(settings) } }
                case "expand": if let field=effect["field"] as? String { expand?(field) }
                case "collapse": collapse?()
                case "close":
                    if closeCompletion != nil {finishClose(true)} else {close?()}
                default: break
                }
            }
            if input["type"] as? String == "keep_editing",closeCompletion != nil {finishClose(false)}
        }
    }
    private func request(_ input:[String:Any]) async -> [String:Any]? {
        guard let bytes=try? JSONSerialization.data(withJSONObject:input) else { return nil }
        let bytesOut=await voice.request(bytes)
        return try? JSONSerialization.jsonObject(with:bytesOut) as? [String:Any]
    }
    private func readSettings() async {
        if let response=await request(["op":"voice_settings"]),response["ok"] as? Bool == true,let settings=response["payload"] as? [String:Any] {
            if settings["prompts"] == nil {
                dispatch(["type":"load_failed","error":"This Host does not support editable prompts. Update the Host first."])
            } else { dispatch(["type":"snapshot","settings":settings]) }
        } else { dispatch(["type":"load_failed","error":"Voice settings could not be loaded. Recheck the local Host connection."]) }
    }
    private func save(_ settings:Any) async {
        let response=await request(["op":"voice_settings_update","settings":settings])
        if response?["ok"] as? Bool == true,let saved=response?["payload"] {
            dispatch(["type":"complete","settings":saved,"error":NSNull()])
            NotificationCenter.default.post(name:Notification.Name("PolarStationVoiceSettingsChanged"),object:nil)
        } else { dispatch(["type":"complete","settings":NSNull(),"error":response?["error"] as? String ?? "The prompt was not saved. The draft is retained; check the local Host connection."]) }
    }
}
