import SwiftUI
import PolarTmuxHostClient

/// Edits the existing revisioned Host settings; credentials remain in the secure store.
public struct CodexEndpointsView: View {
    @ObservedObject var interaction: ConnectionsInteraction
    let consume: @MainActor ([ConnectionsInteraction.Effect]) -> Void
    let request: CodexRequestHandler
    let machines: [MachineConnectionV1]
    let close: @MainActor () -> Void
    let machineID: String?
    let onEditingChanged: @MainActor (Bool) -> Void
    @State private var draftEnabled = false
    @State private var settings = CodexControlSettingsV1()
    @State private var draft = CodexEndpointSettingsV1()
    @State private var owner = ""
    @State private var editing = false
    @State private var existing = false
    @State private var loaded = false
    @State private var busy = false
    @State private var error: String?
    @State private var removal = false
    public init(interaction: ConnectionsInteraction, consume: @escaping @MainActor ([ConnectionsInteraction.Effect]) -> Void, request: @escaping CodexRequestHandler, machines: [MachineConnectionV1], machineID: String? = nil, onEditingChanged: @escaping @MainActor (Bool) -> Void = { _ in }, close: @escaping @MainActor () -> Void) {
        self.interaction = interaction; self.consume = consume
        self.request = request; self.machines = machines; self.machineID = machineID; self.close = close; self.onEditingChanged = onEditingChanged
    }
    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Picker("App-server", selection: Binding(get: { existing ? draft.id : "" }, set: { id in
                    if let endpoint = settings.endpoints.first(where: { $0.id == id }) { draft = endpoint; existing = true }
                })) {
                    Text("Select app-server").tag("")
                    ForEach(settings.endpoints.filter { machineID == nil || $0.execution_target_id == machineID }, id: \.id) { Text($0.id).tag($0.id) }
                }.disabled(editing || busy)
                if editing {
                Form {
                    Toggle("Enable Codex control and monitoring", isOn: $draftEnabled)
                    TextField("Endpoint ID", text: $draft.id).disabled(existing)
                    Picker("Machine", selection: $draft.execution_target_id) {
                        Text("Local").tag("local")
                        ForEach(machines) { Text($0.displayName).tag($0.id) }
                    }
                    .disabled(machineID != nil)
                    Picker("Connection", selection: $draft.control_mode) {
                        Text("Local app-server").tag("local")
                        Text("App-server over SSH").tag("ssh_direct")
                        Text("Remote Station Host").tag("remote_host")
                    }
                    if draft.control_mode != "remote_host" { TextField("App-server socket on machine", text: $draft.socket_path) }
                    if draft.control_mode == "remote_host" {
                        Picker("Authorized Station Host", selection: Binding(get: { draft.peer_connection_ref ?? "" }, set: { id in
                            draft.peer_connection_ref = id.isEmpty ? nil : id
                            draft.owner_host_id = settings.host_connections.first(where: { $0.id == id })?.host_id ?? ""
                        })) {
                            Text("Select existing Host connection").tag("")
                            ForEach(settings.host_connections, id: \.id) { Text($0.host_id).tag($0.id) }
                        }
                        DisclosureGroup("Advanced · Remote Host mapping") {
                            TextField("Endpoint ID on remote Host", text: optional($draft.remote_endpoint_id))
                            TextField("Target ID on remote Host", text: optional($draft.remote_target_id))
                        }
                    }
                    Text("Monitoring uses original app-server events. Notifications are configured separately.").font(.caption).foregroundStyle(.secondary)
                }.textFieldStyle(.roundedBorder)
                } else {
                    VStack(alignment: .leading, spacing: 12) {
                        if existing {
                            Text(draft.id).font(.headline)
                            LabeledContent("Connection", value: draft.control_mode)
                            LabeledContent("Socket", value: draft.socket_path)
                            Text("Connection is checked when a session uses this app-server.").font(.caption).foregroundStyle(.secondary)
                        } else { Text("Select an app-server to view its connection, or add one.").foregroundStyle(.secondary) }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                }
            }.disabled(busy || !loaded)
            }.frame(maxHeight: .infinity)
            if let error = error ?? interaction.state?.message { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            HStack {
                if editing {
                    Button("Cancel") { consume(interaction.action("cancel")) }.disabled(busy)
                    Spacer()
                    Button("Save") { save() }.disabled(busy || !loaded || draft.id.isEmpty)
                } else {
                    Button("Add endpoint") { begin(id: nil) }.disabled(busy || !loaded)
                    if existing {
                        Button("Edit…") { begin(id: draft.id) }.disabled(busy)
                        Menu("More") { Button("Remove…", role: .destructive) { removal = true } }.disabled(busy)
                    }
                    Spacer()

                }
                Button("Close", action: close).keyboardShortcut(.cancelAction).disabled(busy)
            }
            if busy { ProgressView() }
        }.padding(machineID == nil ? 20 : 0).frame(minWidth: machineID == nil ? 710 : 0)
        .task { await load() }
        .onChange(of: editing) { onEditingChanged($0) }
        .onChange(of: draft) { _ in if editing { interaction.draftCodex(draft, enabled: draftEnabled) } }
        .onChange(of: draftEnabled) { _ in if editing { interaction.draftCodex(draft, enabled: draftEnabled) } }
        .onChange(of: interaction.state?.editing) { value in
            editing = value == true && interaction.state?.codex_draft != nil
            if !editing { refreshSelection() }
        }
        .onChange(of: interaction.state?.pending) { busy = $0 == true }
        .onChange(of: interaction.state?.codex_settings) { value in
            if let value { settings = value; if !editing { refreshSelection() } }
        }
        .onAppear { interaction.saveCodexDraft = { save() } }
        .onDisappear { interaction.saveCodexDraft = nil }
        .interactiveDismissDisabled(editing || busy)
        .confirmationDialog("Remove this app-server endpoint?", isPresented: $removal) {
            Button("Remove", role: .destructive) { consume(interaction.removeCodex(machine: draft.execution_target_id, id: draft.id)) }
        }
    }
    private func optional(_ value: Binding<String?>) -> Binding<String> {
        .init(get: { value.wrappedValue ?? "" }, set: { value.wrappedValue = $0.isEmpty ? nil : $0 })
    }
    private func reset() {
        if let endpoint = settings.endpoints.first(where: { machineID == nil || $0.execution_target_id == machineID }) { draft = endpoint; existing = true }
        else { draft = .init(); draft.execution_target_id = machineID ?? "local"; draft.owner_host_id = owner; existing = false }
    }
    @MainActor private func load() async {
        guard !busy else { return }; busy = true; error = nil
        defer { busy = false }
        do {
            let response = try await request(nil, .getSettings)
            guard case .settings(let value) = response.result else { error = "Could not load app-server settings."; return }
            settings = value; owner = response.ownerHostID; interaction.codexSnapshot(value, owner: owner); loaded = true; reset()
        } catch { self.error = error.localizedDescription }
    }
    private func begin(id: String?) {
        interaction.beginCodex(machine: machineID ?? draft.execution_target_id, id: id)
        guard let value = interaction.state?.codex_draft else { return }
        draft = value; draftEnabled = interaction.state?.codex_enabled ?? false
        existing = id != nil; editing = true; error = nil
    }
    private func refreshSelection() {
        if let saved = settings.endpoints.first(where: { $0.id == draft.id }) { draft = saved; existing = true }
        else { reset() }
    }
    private func save() {
        guard !busy, loaded else { return }
        interaction.draftCodex(draft, enabled: draftEnabled)
        consume(interaction.action("save"))
    }
}
