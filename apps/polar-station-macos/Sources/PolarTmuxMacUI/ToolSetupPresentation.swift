import SwiftUI
import PolarTmuxHostClient

private struct ToolSetupNativePortsKey:EnvironmentKey {
    static let defaultValue:ToolSetupNativePorts? = nil
}
public extension EnvironmentValues {
    var stationToolSetupPorts:ToolSetupNativePorts? {
        get {self[ToolSetupNativePortsKey.self]}
        set {self[ToolSetupNativePortsKey.self]=newValue}
    }
}
