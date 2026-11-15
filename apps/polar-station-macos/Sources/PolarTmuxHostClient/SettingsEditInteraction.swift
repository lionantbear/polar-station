import Foundation
import CPolarStationDesktopClient

/// Only delivers native edit observations to the original Settings domain.
@MainActor public final class SettingsEditInteraction {
    public enum Section:String { case appearance,attention,monitoring,sessionSummary="session_summary",voice,voiceRetention="voice_retention",imessage }
    private struct State:Decodable {
        struct Row:Decodable {let section:String;let replace:Bool;let revision:UInt64?}
        let revision:UInt64?
        let can_rebase:Bool
        let rebase_label:String
        let sections:[Row]
        let save_revision:UInt64?
    }
    private final class Handle {
        let pointer=polar_station_settings_edit_new()
        deinit {polar_station_settings_edit_free(pointer)}
    }
    private let owner=Handle()
    private var state:State?
    public init() {}
    public var canRebase:Bool {state?.can_rebase ?? false}
    public var rebaseLabel:String {state?.rebase_label ?? ""}
    public func rebase() {send(["type":"rebase"])}
    public func observe(_ revision:UInt64) {send(["type":"observe","revision":revision])}
    public func edited(_ section:Section) {send(["type":"edit","section":section.rawValue])}
    public func replaces(_ section:Section)->Bool {state?.sections.first{$0.section==section.rawValue}?.replace ?? true}
    public func begin(_ section:Section,request:String)->UInt64? {
        send(["type":"begin_save","section":section.rawValue,"request_id":request]);return state?.save_revision
    }
    public func complete(_ request:String,accepted:Bool,revision:UInt64?) {
        send(["type":"complete_save","request_id":request,"accepted":accepted,"revision":revision as Any? ?? NSNull()])
    }
    private func send(_ input:[String:Any]) {
        guard let bytes=try? JSONSerialization.data(withJSONObject:input) else{return}
        bytes.withUnsafeBytes {data in
            guard let result=polar_station_settings_edit_dispatch(owner.pointer,data.bindMemory(to:UInt8.self).baseAddress,data.count) else{return}
            defer {polar_station_desktop_string_free(result)}
            state=try? JSONDecoder().decode(State.self,from:Data(String(cString:result).utf8))
        }
    }
}
