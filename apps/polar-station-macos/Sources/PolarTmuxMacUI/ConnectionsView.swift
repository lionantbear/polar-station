import SwiftUI
import PolarTmuxHostClient

/// Native window-local drafts; all validation, conflicts and references come from Host.
public struct ConnectionsView: View {
    let projection: ConnectionsProjectionV1
    let controlRevision: UInt64
    let close: (@MainActor () -> Void)?
    let requestCodex: CodexRequestHandler?
    let submit: @MainActor (ControlCenterIntentV1, @escaping @MainActor (String?) -> Void) -> Void
    @StateObject private var interaction: ConnectionsInteraction
    @Environment(\.dismiss) private var dismiss
    @State private var machine = MachineConnectionV1()
    @State private var polar = PolarConnectionV1()
    @State private var editingPolar = false
    @State private var existing = false
    @State private var draftRevision = ""
    @State private var submitting = false
    @State private var message: String?
    @State private var confirmingRemove = false
    @State private var newMachineBaseline: Set<String>?
    @State private var newPolarBaseline: Set<String>?
    @State private var savedRevision: String?
    @State private var checkedTarget: String?

    public init(interaction: ConnectionsInteraction? = nil, projection: ConnectionsProjectionV1, controlRevision: UInt64, close: (@MainActor () -> Void)? = nil, requestCodex: CodexRequestHandler? = nil,
                initialMachine: String = "local", initialPage: String = "Connection",
                submit: @escaping @MainActor (ControlCenterIntentV1, @escaping @MainActor (String?) -> Void) -> Void) {
        self._selectedMachine = State(initialValue: initialMachine)
        self._page = State(initialValue: initialPage)
        self._interaction = StateObject(wrappedValue: interaction ?? ConnectionsInteraction())
        self.projection = projection; self.controlRevision = controlRevision; self.close = close; self.requestCodex = requestCodex; self.submit = submit
    }
    @State private var routeEnabled = false
    @State private var routeExpanded = false
    @State private var lanAddress = ""
    @State private var lanPort = "22"
    @State private var wanHost = ""
    @State private var wanPort = "22888"
    @State private var doh = "google, cloudflare"
    @State private var selectedMachine = "local"
    @State private var page = "Connection"
    @State private var search = ""
    @State private var editing = false
    @State private var codexEditing = false

    private var machineTitle: String {
        projection.machines.first(where: { $0.id == selectedMachine })?.displayName ?? "Local"
    }
    private var instances: [PolarConnectionV1] {
        projection.polarInstances.filter { $0.executionTargetID == selectedMachine }
    }
    private func machineRow(_ machine: MachineConnectionV1) -> some View {
        let connection = projection.machineStatuses?.first(where: { $0.id == machine.id })?.connection
        return VStack(alignment: .leading, spacing: 3) {
            Text(machine.displayName).lineLimit(1)
            if let connection {
                HStack(spacing: 4) {
                    Image(systemName: connection.status == "connected" ? "checkmark.circle.fill" : "circle.fill")
                        .foregroundColor(connection.status == "connected" ? .green : (connection.status == "offline" ? .red : (connection.status == "inactive" ? .gray : .orange)))
                    Text(connection.label)
                    if let route = connection.route { Text(route.rawValue.uppercased()) }
                }.font(.caption).accessibilityLabel(connection.accessibilityLabel)
            }
        }
    }
    public var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 12) {
                Text("Machines").font(.headline)
                TextField("Search machines", text: $search)
                List(selection: Binding(get: { selectedMachine }, set: { id in
                    selectedMachine = id; page = "Connection"; selectMachine()
                })) {
                    Text("Local · This computer").tag("local")
                    ForEach(projection.machines.filter { search.isEmpty || $0.displayName.localizedCaseInsensitiveContains(search) }) {
                        machineRow($0).tag($0.id)
                    }
                }
                Button("+ Add machine") { beginMachine(nil); startEditing(); page = "Connection" }
            }.padding(16).frame(width: 200).disabled(editing || codexEditing || submitting)
            Divider()
            VStack(alignment: .leading, spacing: 16) {
                Text(editing ? (existing ? "Edit connection" : "Add connection") : machineTitle).font(.title2)
                if editing {
                    ScrollView { editor }.frame(maxHeight: .infinity)
                    if let message { Text(message).textSelection(.enabled) }
                    HStack {
                        Spacer()
                        Button("Cancel") { leave("cancel") }.disabled(submitting)
                        Button(submitting ? "Saving…" : "Save") {
                            saveDraft()
                        }.disabled(submitting || draftRevision.isEmpty).keyboardShortcut(.defaultAction)
                    }
                } else {
                    Picker("Connection type", selection: $page) {
                        Text("Connection").tag("Connection")
                        Text("Polar").tag("Polar")
                        Text("Codex").tag("Codex")
                    }.pickerStyle(.segmented).disabled(codexEditing)
                    if page == "Codex" {
                        if let requestCodex {
                            CodexEndpointsView(interaction: interaction, consume: consume, request: requestCodex, machines: projection.machines,
                                machineID: selectedMachine, onEditingChanged: { codexEditing = $0 }, close: { leave("close") }).id(selectedMachine)
                        } else { Text("Codex connection management is unavailable.") }
                    } else {
                    ScrollView {
                        VStack(alignment: .leading, spacing: 16) {
                            if page == "Connection" {
                                if selectedMachine == "local" {
                                    Text("This computer").font(.headline)
                                    Text("Local connections do not use SSH.").foregroundStyle(.secondary)
                                } else {
                                    Text("SSH").font(.headline)
                                    LabeledContent("SSH destination", value: machine.destination)
                                    if let route = machine.routeProfile {
                                        Text("Connection paths").font(.headline)
                                        LabeledContent("LAN", value: "\(route.lanAddress):\(route.lanPort)")
                                        LabeledContent("WAN", value: "\(route.wanHost):\(route.wanPort)")
                                    }
                                    DisclosureGroup("Connection details") {
                                        Text(machine.id).textSelection(.enabled)
                                        if machine.routeProfile != nil { Text("Explicit LAN/WAN routing configured") }
                                    }
                                }
                            } else if page == "Polar" {
                                HStack {
                                    Text("Polar instances").font(.headline)
                                    Spacer()
                                    Button("+ Add Polar") { beginPolar(nil); polar.executionTargetID = selectedMachine; startEditing() }
                                }
                                if instances.isEmpty { Text("No Polar instance on this machine. Add one to enable chat.").foregroundStyle(.secondary) }
                                ForEach(instances) { item in
                                    Button { beginPolar(item) } label: {
                                        HStack { Text(item.displayName); Spacer(); if polar.id == item.id { Image(systemName: "checkmark") } }
                                    }.buttonStyle(.bordered)
                                }
                                if editingPolar && existing && polar.executionTargetID == selectedMachine {
                                    Divider()
                                    Text(polar.displayName).font(.headline)
                                    DisclosureGroup("Connection details") {
                                        Text(polar.socketPath).textSelection(.enabled)
                                        Text("Expected node: " + polar.expectedNodeID).textSelection(.enabled)
                                        Text("Chat access is authorized by the Polar Vault on this machine.").foregroundStyle(.secondary)
                                    }
                                }
                            }
                        }.frame(maxWidth: .infinity, alignment: .leading)
                    }
                    Spacer(minLength: 0)
                    if page != "Codex", existing { Text(message ?? "Not checked").foregroundStyle(.secondary).textSelection(.enabled) }
                    HStack {
                        if page == "Connection" && selectedMachine != "local" || page == "Polar" && editingPolar && existing { connectionActions }
                        Spacer()
                        Button("Close") { leave("close") }.keyboardShortcut(.cancelAction)
                    }
                    }
                }
            }.padding(20).frame(minWidth: 440, maxWidth: .infinity, maxHeight: .infinity)
        }.frame(minWidth: 700, minHeight: 480)
        .interactiveDismissDisabled(editing || codexEditing || submitting)
        .onAppear { interaction.snapshot(projection); selectMachine(); interaction.requestClose = { leave("close") } }
        .onChange(of: page) { value in
            message = nil; checkedTarget = nil
            if value == "Connection" && !editing { selectMachine() }
        }
        .onChange(of: projection.revision) { _ in interaction.snapshot(projection); resolveCreatedSelection() }
        .onChange(of: projection.lastCheck) { value in
            if let value, value.targetID == checkedTarget, value.polar == editingPolar { message = value.summary }
        }
        .confirmationDialog(interaction.state?.confirmation ?? "", isPresented: Binding(get: { interaction.state?.confirm_discard == true }, set: { _ in })) {
            Button(interaction.state?.save_label ?? "Save") { saveDraft() }
            Button(interaction.state?.discard_label ?? "Discard", role: .destructive) {
                let effects = interaction.action("discard"); editing = false; selectMachine(); consume(effects)
            }
            Button(interaction.state?.keep_label ?? "Keep editing", role: .cancel) { _ = interaction.action("keep_editing") }
        }
        .confirmationDialog("Remove this connection?", isPresented: $confirmingRemove) {
            Button("Remove", role: .destructive) { save(editingPolar ? .removePolar(polar.id) : .removeMachine(machine.id)) }
        } message: { Text("Connections in use cannot be removed. Running sessions remain active.") }
    }
    @ViewBuilder private var connectionActions: some View {
        HStack {
            Button(submitting ? "Checking…" : "Check") {
                checkedTarget = editingPolar ? polar.id : machine.id
                submitting = true; message = "Checking…"
                submit(.init(controlRevision: controlRevision, operation: .checkConnection(targetID: checkedTarget!, polar: editingPolar))) { error in
                    submitting = false
                    if let error { message = error }
                }
            }.disabled(submitting)
            Button("Edit…") { startEditing() }.disabled(submitting)
            Spacer()
            Menu {
                Button("Remove…", role: .destructive) { confirmingRemove = true }
            } label: { Image(systemName: "ellipsis") }.menuStyle(.borderlessButton).frame(width: 26).disabled(submitting)
        }
    }
    @ViewBuilder private var editor: some View {
        Form {
            if editingPolar {
                TextField("Name", text: $polar.displayName)
                LabeledContent("Machine", value: machineTitle)
                TextField("Socket path", text: $polar.socketPath)
                Section("Identity verification") { TextField("Expected Node ID", text: $polar.expectedNodeID) }
            } else {
                TextField("Name", text: $machine.displayName)
                TextField("SSH destination", text: $machine.destination, prompt: Text("user@host or SSH alias"))
                DisclosureGroup("Connection paths · LAN / WAN", isExpanded: $routeExpanded) {
                    Toggle("Use explicit LAN/WAN route", isOn: $routeEnabled)
                    if routeEnabled {
                        TextField("LAN address", text: $lanAddress)
                        TextField("LAN port", text: $lanPort)
                        TextField("WAN host", text: $wanHost)
                        TextField("WAN port", text: $wanPort)
                        TextField("DNS providers", text: $doh)
                    }
                }
            }
        }.textFieldStyle(.roundedBorder).disabled(submitting)
    }
    private func startEditing() {
        var baseline = projection
        baseline.revision = draftRevision
        interaction.snapshot(baseline)
        interaction.begin(editingPolar ? .savePolar(polar) : .saveMachine(machine))
        editing = true
    }
    private func leave(_ action: String) {
        guard !submitting else { return }
        let effects = interaction.action(action)
        if interaction.state?.editing == false { editing = false; selectMachine() }
        consume(effects)
    }
    private func consume(_ effects: [ConnectionsInteraction.Effect]) {
        for effect in effects {
            if effect.type == "close" { finish() }
            else if effect.type == "save_codex", let settings = effect.settings,
                    let revision = effect.codex_revision, let token = effect.token, let requestCodex {
                submitting = true
                Task { @MainActor in
                    var error: String?
                    do {
                        let response = try await requestCodex(nil, .updateSettings(revision: revision, settings: settings))
                        switch response.result {
                        case .settings(let value): interaction.codexSnapshot(value, owner: response.ownerHostID)
                        case .error(let code): error = "Settings were not saved: \(code). Your draft is retained."
                        default: error = "Save outcome is unknown. Reload settings before retrying."
                        }
                    } catch let failure { error = failure.localizedDescription }
                    submitting = false
                    consume(interaction.complete(token, error: error))
                }
            }
            else if let change = effect.change, let token = effect.token, let revision = effect.expected_revision {
                save(change, revision: revision, token: token)
            }
        }
    }
    private func saveEdited(_ change: ConnectionChangeV1) {
        interaction.draft(change)
        consume(interaction.action("save"))
    }
    private func finish() { if let close { close() } else { dismiss() } }
    private func selectMachine() {
        beginMachine(projection.machines.first(where: { $0.id == selectedMachine }))
    }
    private func beginMachine(_ value: MachineConnectionV1?) {
        savedRevision = nil; checkedTarget = nil
        machine = value ?? .init(); editingPolar = false; existing = value != nil
        routeEnabled = machine.routeProfile != nil
        routeExpanded = routeEnabled
        lanAddress = machine.routeProfile?.lanAddress ?? ""
        lanPort = String(machine.routeProfile?.lanPort ?? 22)
        wanHost = machine.routeProfile?.wanHost ?? ""
        wanPort = String(machine.routeProfile?.wanPort ?? 22888)
        doh = machine.routeProfile?.dohProviders.joined(separator: ", ") ?? "google, cloudflare"
        draftRevision = projection.revision; message = nil
    }
    private func beginPolar(_ value: PolarConnectionV1?) {
        savedRevision = nil; checkedTarget = nil
        polar = value ?? .init(); editingPolar = true; existing = value != nil
        draftRevision = projection.revision; message = nil
    }
    private func saveDraft() {
        if interaction.state?.codex_draft != nil { interaction.saveCodexDraft?(); return }
        if editingPolar { saveEdited(.savePolar(polar)); return }
        if routeEnabled {
            guard let lan = UInt16(lanPort), lan > 0, let wan = UInt16(wanPort), wan > 0 else {
                _ = interaction.action("keep_editing"); message = "Ports must be numbers from 1 to 65535."; return
            }
            machine.routeProfile = .init(lanAddress: lanAddress, lanPort: lan, wanHost: wanHost,
                wanPort: wan, dohProviders: doh.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty })
        } else { machine.routeProfile = nil }
        saveEdited(.saveMachine(machine))
    }
    private func resolveCreatedSelection() {
        guard !submitting else { return }
        if let baseline = newMachineBaseline {
            let added = projection.machines.filter { !baseline.contains($0.id) }
            if added.count == 1 { selectedMachine = added[0].id; beginMachine(added[0]); newMachineBaseline = nil }
        }
        if let baseline = newPolarBaseline {
            let added = projection.polarInstances.filter { !baseline.contains($0.id) }
            if added.count == 1 { beginPolar(added[0]); newPolarBaseline = nil }
        }
    }
    private func save(_ change: ConnectionChangeV1, revision: String? = nil, token: UInt64? = nil) {
        if case .saveMachine(let value) = change, value.id.isEmpty { newMachineBaseline = Set(projection.machines.map(\.id)) }
        if case .savePolar(let value) = change, value.id.isEmpty { newPolarBaseline = Set(projection.polarInstances.map(\.id)) }
        checkedTarget = nil
        submitting = true
        submit(.init(controlRevision: controlRevision, operation: .updateConnection(expectedRevision: revision ?? draftRevision, change: change))) { error in
            submitting = false
            let effects = token.map { interaction.complete($0, error: error) } ?? []
            if let error { message = error }
            else {
                savedRevision = draftRevision
                editing = false
                if !editingPolar { selectedMachine = machine.id }
                existing = true
                resolveCreatedSelection()
                // A content-addressed revision does not change for a no-op save.
                // The correlated receipt completes the editor; a later snapshot
                // may update its baseline, but must not keep the form locked.
                message = "Saved. Check the connection to verify access."
            }
            consume(effects)
        }
    }
}
