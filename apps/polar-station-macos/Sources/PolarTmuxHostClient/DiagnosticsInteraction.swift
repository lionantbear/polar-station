import Foundation
import Combine
import CPolarStationDesktopClient

/// Native task delivery only; the shared Rust engine owns tool facts and workflow.
@MainActor public final class DiagnosticsInteraction: ObservableObject {
    public struct Action: Codable, Equatable {
        public let type: String
        public let target_id: String?
        public let destination: String?
        public let connection_id: String?
        public let endpoint_id: String?
        public let resource_id: String?
        public let section: String?
        public let tmux: Bool?
        public let codex: Bool?
        public let codex_service: Bool?
        public let claude: Bool?
        public let claude_login: Bool?
        public let setup_id:String?
    }
    public struct Row: Decodable, Identifiable, Equatable {
        public let id: String
        public let title: String
        public let state: String
        public let detail: String
        public let action: Action?
        public let action_label: String?
    }
    public struct Host: Decodable, Identifiable, Equatable {
        public let id: String
        public let name: String
        public let rows: [Row]
    }
    public struct Projection: Decodable, Equatable {
        public let local_title: String
        public let remote_title: String
        public let refresh_label: String
        public let empty_remote_label: String
        public let local: [Row]
        public let remote_setup: [Row]
        public let remote: [Host]
        public let capability_sections: [String: String]?
    }
    private struct Effect: Decodable {
        let type: String
        let token: UInt64?
        let request: SessionEditorInteraction.Request?
        let action: Action?
        let expected_revision: String?
        let machine: MachineConnectionV1?
        let setup:ToolSetupLaunchV1?
        let setup_id:String?
    }
    private struct Output: Decodable {
        let projection: Projection
        let busy: Bool
        let message: String?
        let effects: [Effect]
    }
    private final class Handle: @unchecked Sendable {
        let pointer = polar_station_diagnostics_new()
        deinit { polar_station_diagnostics_free(pointer) }
    }
    private let owner = Handle()
    @Published public private(set) var projection: Projection?
    @Published public private(set) var busy = false
    @Published public private(set) var message: String?
    public var saveMachine: (@MainActor (String, MachineConnectionV1) -> Void)?
    public var toolSetup:ToolSetupNativePorts?
    public var read: SessionEditorReader?
    public var present: (@MainActor (Action) -> Void)?
    public init() {}
    public func facts(generation: UInt64, value: [String: Any]) {
        dispatch(["type":"facts", "generation":generation, "facts":value])
    }
    public func showOperationError(_ error: String?) { message = error }
    public func connectDestination(_ destination: String) {
        dispatch(["type": "connect_destination", "destination": destination])
    }
    public func refresh() { dispatch(["type":"refresh"]) }
    public func dictateTrialSucceeded() { dispatch(["type":"dictate_trial_succeeded"]) }
    public func dictateTrialProgress(_ phase: String, error: String? = nil) {
        dispatch(["type":"dictate_trial_progress", "phase":phase, "error":error as Any? ?? NSNull()])
    }
    public func returned() { dispatch(["type":"returned"]) }
    public func perform(_ action: Action) {
        guard let value = try? JSONSerialization.jsonObject(with: JSONEncoder().encode(action)) else { return }
        dispatch(["type":"action", "action":value])
    }
    private func dispatch(_ input: [String: Any]) {
        guard let bytes = try? JSONSerialization.data(withJSONObject: input) else { return }
        let output: Output? = bytes.withUnsafeBytes { buffer in
            guard let result = polar_station_diagnostics_dispatch(owner.pointer, buffer.bindMemory(to: UInt8.self).baseAddress, bytes.count) else { return nil }
            defer { polar_station_desktop_string_free(result) }
            do {return try JSONDecoder().decode(Output.self, from: Data(String(cString:result).utf8))}
            catch {
                if ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil {NSLog("Station diagnostics decode failure: %@",String(describing:error))}
                return nil
            }
        }
        guard let output else { return }
        if ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil {
            NSLog("Station diagnostics input=%@ token=%@ effects=%@",input["type"] as? String ?? "",String(describing:input["token"]),output.effects.map(\.type).joined(separator:","))
        }
        projection = output.projection; busy = output.busy; message = output.message
        for effect in output.effects {
            if effect.type == "launch_setup",let setup=effect.setup,let token=effect.token {
                guard let toolSetup else {
                    dispatch(["type":"setup_started","token":token,"error":"The native setup Pane is unavailable."]);continue
                }
                toolSetup.launch(setup,{[weak self] error in
                    self?.dispatch(["type":"setup_started","token":token,"error":error as Any? ?? NSNull()])
                },{[weak self] code in
                    self?.dispatch(["type":"setup_exited","token":token,"exit_code":code as Any? ?? NSNull()])
                })
            }
            if effect.type == "save_machine", let revision = effect.expected_revision, let machine = effect.machine { saveMachine?(revision, machine) }
            if effect.type == "focus_setup",let id=effect.setup_id {toolSetup?.focus(id)}
            if effect.type == "present", let action = effect.action { present?(action) }
            if effect.type == "read", let token = effect.token, let wire = effect.request?.wire {
                let reader = read
                Task { [weak self] in
                    do {
                        guard let reader else { throw PolarTmuxLocalIPCError.connectionClosed }
                        let result = try await reader(wire)
                        self?.dispatch(["type":"complete", "token":token, "result":result.wireObject])
                    } catch { self?.dispatch(["type":"complete", "token":token, "error":error.localizedDescription]) }
                }
            }
        }
    }
}
