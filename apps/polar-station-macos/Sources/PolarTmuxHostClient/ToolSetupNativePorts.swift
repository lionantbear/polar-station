import Foundation
import CPolarStationDesktopClient

/// Physical PTY ports only. Shared Rust interactions own attempt tokens and results.
@MainActor public struct ToolSetupNativePorts {
    public typealias Launch = @MainActor (ToolSetupLaunchV1, @escaping @MainActor (String?) -> Void, @escaping @MainActor (UInt32?) -> Void) -> Void
    public let launch: Launch
    public let focus: @MainActor (String) -> Void
    public init(launch:@escaping Launch,focus:@escaping @MainActor (String)->Void) {
        self.launch=launch;self.focus=focus
    }
}
public struct ToolSetupCopy:Decodable {
    public let restored_title:String
    public let restored_detail:String
    public let close_title:String
    public let close_detail:String
    public let keep:String
    public let stop:String
    public static func load()->ToolSetupCopy? {
        guard let raw=polar_station_tool_setup_copy() else {return nil}
        defer {polar_station_desktop_string_free(raw)}
        return try? JSONDecoder().decode(Self.self,from:Data(String(cString:raw).utf8))
    }
}
