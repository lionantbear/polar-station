import SwiftUI
import AppKit
import UniformTypeIdentifiers
import PolarTmuxHostClient

/// Native window-local drafts; all validation, conflicts and references come from Host.
public struct ConnectionsView: View {
    let projection: ConnectionsProjectionV1
    let controlRevision: UInt64
    let initialPolar: String?
    let initialEndpoint: String?
    let initialMachine: String
    let initialPage: String
    let embedded: Bool
    let newMachine: Bool
    let close: (@MainActor () -> Void)?
    let requestHostTransfer: HostTransferReader?
    @State private var exportSelection = Set<String>()
    @State private var choosingExport = false
    let requestCodex: CodexRequestHandler?
    let submit: @MainActor (ControlCenterIntentV1, @escaping @MainActor (String?) -> Void) -> Void
    @StateObject private var interaction: ConnectionsInteraction
    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var formBuffer: ConnectionsFormBuffer
    @Binding private var machine: MachineConnectionV1
    @Binding private var polar: PolarConnectionV1
    @Binding private var editingPolar: Bool
    @Binding private var existing: Bool
    @Binding private var draftRevision: String
    @Binding private var submitting: Bool
    @Binding private var message: String?
    @Binding private var confirmingRemove: Bool
    @Binding private var newMachineBaseline: Set<String>?
    @Binding private var newPolarBaseline: Set<String>?
    @Binding private var savedRevision: String?
    @Binding private var checkedTarget: String?

    public init(interaction: ConnectionsInteraction? = nil, formBuffer: ConnectionsFormBuffer, projection: ConnectionsProjectionV1, controlRevision: UInt64, close: (@MainActor () -> Void)? = nil, requestCodex: CodexRequestHandler? = nil, requestHostTransfer: HostTransferReader? = nil,
                initialMachine: String = "local", initialPage: String = "Connection",
                initialPolar: String? = nil, initialEndpoint: String? = nil, embedded: Bool = false, newMachine: Bool = false,
                submit: @escaping @MainActor (ControlCenterIntentV1, @escaping @MainActor (String?) -> Void) -> Void) {
        self.embedded = embedded; self.newMachine = newMachine; self.initialPolar = initialPolar; self.initialEndpoint = initialEndpoint
        self.initialMachine = initialMachine; self.initialPage = initialPage
        let buffer = formBuffer
        self._formBuffer = ObservedObject(wrappedValue: buffer)
        self._machine = Binding(get:{buffer.machine},set:{buffer.machine=$0})
        self._polar = Binding(get:{buffer.polar},set:{buffer.polar=$0})
        self._editingPolar = Binding(get:{buffer.editingPolar},set:{buffer.editingPolar=$0})
        self._existing = Binding(get:{buffer.existing},set:{buffer.existing=$0})
        self._draftRevision = Binding(get:{buffer.draftRevision},set:{buffer.draftRevision=$0})
        self._submitting = Binding(get:{buffer.submitting},set:{buffer.submitting=$0})
        self._message = Binding(get:{buffer.message},set:{buffer.message=$0})
        self._confirmingRemove = Binding(get:{buffer.confirmingRemove},set:{buffer.confirmingRemove=$0})
        self._newMachineBaseline = Binding(get:{buffer.newMachineBaseline},set:{buffer.newMachineBaseline=$0})
        self._newPolarBaseline = Binding(get:{buffer.newPolarBaseline},set:{buffer.newPolarBaseline=$0})
        self._savedRevision = Binding(get:{buffer.savedRevision},set:{buffer.savedRevision=$0})
        self._checkedTarget = Binding(get:{buffer.checkedTarget},set:{buffer.checkedTarget=$0})
        self._routeEnabled = Binding(get:{buffer.routeEnabled},set:{buffer.routeEnabled=$0})
        self._routeExpanded = Binding(get:{buffer.routeExpanded},set:{buffer.routeExpanded=$0})
        self._lanAddress = Binding(get:{buffer.lanAddress},set:{buffer.lanAddress=$0})
        self._lanPort = Binding(get:{buffer.lanPort},set:{buffer.lanPort=$0})
        self._wanHost = Binding(get:{buffer.wanHost},set:{buffer.wanHost=$0})
        self._wanPort = Binding(get:{buffer.wanPort},set:{buffer.wanPort=$0})
        self._doh = Binding(get:{buffer.doh},set:{buffer.doh=$0})
        self._selectedMachine = Binding(get:{buffer.selectedMachine},set:{buffer.selectedMachine=$0})
        self._page = Binding(get:{buffer.page},set:{buffer.page=$0})
        self._search = Binding(get:{buffer.search},set:{buffer.search=$0})
        self._editing = Binding(get:{buffer.editing},set:{buffer.editing=$0})
        self._codexEditing = Binding(get:{buffer.codexEditing},set:{buffer.codexEditing=$0})
        self._interaction = StateObject(wrappedValue: interaction ?? ConnectionsInteraction())
        self.projection = projection; self.controlRevision = controlRevision; self.close = close; self.requestCodex = requestCodex; self.requestHostTransfer = requestHostTransfer; self.submit = submit
    }
    @Binding private var routeEnabled: Bool
    @Binding private var routeExpanded: Bool
    @Binding private var lanAddress: String
    @Binding private var lanPort: String
    @Binding private var wanHost: String
    @Binding private var wanPort: String
    @Binding private var doh: String
    @Binding private var selectedMachine: String
    @Binding private var page: String
    @Binding private var search: String
    @Binding private var editing: Bool
    @Binding private var codexEditing: Bool

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
            if !embedded {
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
            }
            VStack(alignment: .leading, spacing: 16) {
                if requestHostTransfer != nil {
                    HStack {
                        Button("Import Hosts…") { importHostsFile() }
                        Button("Export Hosts…") { exportSelection=[selectedMachine]; choosingExport=true }
                    }.disabled(editing || codexEditing || submitting || projection.hostTransferSupported != true)
                }
                if embedded && !newMachine {
                    Picker("Machine",selection:Binding(get:{selectedMachine},set:{id in selectedMachine=id; page="Connection"; selectMachine()})) {
                        Text("Local · This computer").tag("local")
                        ForEach(projection.machines) { machine in Text(machine.displayName).tag(machine.id) }
                    }.disabled(editing || codexEditing || submitting)
                    Button("+ Add machine") { beginMachine(nil); startEditing(); page="Connection" }.disabled(editing || codexEditing || submitting)
                }
                Text(editing ? (existing ? "Edit connection" : "Add connection") : machineTitle).font(.title2)
                if editing {
                    scrolling { editor }
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
                                machineID: selectedMachine, initialEndpoint: initialEndpoint, embedded: embedded, onEditingChanged: { codexEditing = $0 }, close: { leave("close") }).id(selectedMachine)
                        } else { Text("Codex connection management is unavailable.") }
                    } else {
                    scrolling {
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
            }.padding(20).frame(minWidth: embedded ? 0 : 440, maxWidth: .infinity, maxHeight: .infinity)
        }.frame(minWidth: embedded ? 0 : 700, minHeight: embedded ? 0 : 480)
        .sheet(isPresented:Binding(get:{interaction.state?.host_import_plan != nil},set:{if !$0 { consume(interaction.action("cancel_host_import")) }})) {
            HostImportReviewView(interaction:interaction,consume:consume)
        }
        .sheet(isPresented:$choosingExport) {
            VStack(alignment:.leading,spacing:14) {
                Text("Export Hosts").font(.title2)
                Text("Connection configuration only. No passwords, private keys or OAuth.").foregroundStyle(.secondary)
                List {
                    exportToggle("local","This computer")
                    ForEach(projection.machines) { item in exportToggle(item.id,item.displayName) }
                }.frame(minHeight:180)
                HStack {
                    Button("Cancel") { choosingExport=false }; Spacer()
                    Button("Export…") { choosingExport=false; consume(interaction.exportHosts(exportSelection.sorted())) }.disabled(exportSelection.isEmpty)
                }
            }.padding(24).frame(width:520)
        }
        .interactiveDismissDisabled(editing || codexEditing || submitting)
        .onAppear {
            interaction.snapshot(projection)
            if !editing && !codexEditing {
                let route = [initialMachine,initialPage,initialPolar ?? "",initialEndpoint ?? "",String(newMachine)]
                if formBuffer.presentedRoute != route {
                    selectedMachine = initialMachine; page = initialPage
                    formBuffer.presentedRoute = route
                }
                selectMachine()
                if newMachine { beginMachine(nil); startEditing() }
                if let initialPolar, let item = instances.first(where: { $0.id == initialPolar }) { beginPolar(item) }
            }
            formBuffer.initialized = true
            interaction.requestClose = { leave("close") }
        }
        .onChange(of: editableCandidate) { value in if editing { interaction.draft(value) } }
        .onDisappear { interaction.requestClose = nil }
        .onChange(of: page) { value in
            message = nil; checkedTarget = nil
            if value == "Connection" && !editing { selectMachine() }
        }
        .onChange(of: projection) { latest in interaction.snapshot(latest); resolveCreatedSelection() }
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
    @ViewBuilder private func scrolling<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        if embedded { content() } else { ScrollView { content() } }
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
    // Same raw-to-wire conversion as GTK's original routeProfile: invalid
    // ports remain in the raw field and become 0 for existing domain checks.
    private var editableCandidate: ConnectionChangeV1 {
        if editingPolar { return .savePolar(polar) }
        var candidate = machine
        candidate.routeProfile = routeEnabled ? .init(lanAddress:lanAddress,
            lanPort:UInt16(lanPort) ?? 0, wanHost:wanHost, wanPort:UInt16(wanPort) ?? 0,
            dohProviders:doh.split(separator:",").map{$0.trimmingCharacters(in:.whitespaces)}.filter{!$0.isEmpty}) : nil
        return .saveMachine(candidate)
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
        if editing { interaction.draft(editableCandidate) }
        let effects = interaction.action(action)
        if interaction.state?.editing == false { editing = false; selectMachine() }
        consume(effects)
    }
    private func consume(_ effects: [ConnectionsInteraction.Effect]) {
        for effect in effects {
            if effect.type == "host_transfer", let token=effect.token, let request=effect.request, let requestHostTransfer {
                submitting=true
                Task { @MainActor in
                    do { let result=try await requestHostTransfer(request); submitting=false; consume(interaction.transferComplete(token,result:result,error:nil)) }
                    catch { submitting=false; consume(interaction.transferComplete(token,result:nil,error:error.localizedDescription)) }
                    message=interaction.state?.message
                }
            }
            else if effect.type == "write_host_export", let bundle=effect.bundle { saveHostsFile(bundle) }
            else if effect.type == "close" { finish() }
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
    private func exportToggle(_ id:String,_ title:String) -> some View {
        Toggle(title,isOn:Binding(get:{exportSelection.contains(id)},set:{if $0 {exportSelection.insert(id)} else {exportSelection.remove(id)}}))
    }
    private func importHostsFile() {
        let panel=NSOpenPanel(); panel.canChooseDirectories=false; panel.allowsMultipleSelection=false; panel.allowedContentTypes=[.json]
        panel.begin { response in
            guard response == .OK, let url=panel.url else { return }
            do {
                let file=try FileHandle(forReadingFrom:url); defer { try? file.close() }
                let bytes=try file.read(upToCount:524289) ?? Data()
                guard bytes.count <= 524288 else { message="Host import file is too large."; return }
                consume(interaction.importHosts(try JSONDecoder().decode(HostExportBundleV1.self,from:bytes)))
            } catch { message="Cannot read Host import: " + error.localizedDescription }
        }
    }
    private func saveHostsFile(_ bundle:HostExportBundleV1) {
        let panel=NSSavePanel(); panel.nameFieldStringValue="hosts.polar-hosts.json"; panel.allowedContentTypes=[.json]
        panel.begin { response in
            guard response == .OK, let url=panel.url else { return }
            do { let encoder=JSONEncoder(); encoder.outputFormatting=[.prettyPrinted,.sortedKeys]; try encoder.encode(bundle).write(to:url,options:.atomic); try FileManager.default.setAttributes([.posixPermissions:0o600],ofItemAtPath:url.path); message="Hosts exported." }
            catch { message="Host export was not saved: " + error.localizedDescription }
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
