#if POLAR_STATION
    import AppKit
    import AVFoundation
    import CPolarStationDesktopClient
    import PolarTmuxHostClient
    import PolarTmuxMacUI
    import SwiftUI

    struct PolarStationSettings: View {
        let route: SettingsNavigation.Route
        @ObservedObject var windowContext: PolarStationWindowContext
        @ObservedObject var model: ControlCenterStore

        var body: some View {
            switch route.page {
            case "station_backend":
                Color.clear.onAppear {
                    PolarStationRuntime.shared.openBackendSettings()
                    windowContext.settingsNavigation.completeBack()
                }
            case "workspace_layout":
                Button("Open Layout") {
                    PolarStationRuntime.shared.previewWorkspace(windowContext.executor.workspaceID, in: windowContext)
                }
            case "setup": PolarStationDiagnosticsRoot(model:model,route:route,context:windowContext)
            case "connections":
                if let projection = model.projection {
                    ConnectionsView(interaction:windowContext.settingsConnections,formBuffer:windowContext.connectionsFormBuffer,projection:projection.connections,controlRevision:projection.controlRevision,
                        close:windowContext.settingsNavigation.completeBack, requestCodex:model.requestCodex, requestHostTransfer:model.requestHostTransfer,
                        initialMachine:route.target ?? "local", initialPage:route.section == "codex" ? "Codex" : route.section == "polar" ? "Polar" : "Connection",
                        initialPolar:route.section == "polar" ? route.object : nil,initialEndpoint:route.section == "codex" ? route.object : nil,embedded:true,
                        submit:model.submitSSHTarget)
                        .onAppear {
                            windowContext.settingsNavigation.requestEditorClose = { [weak interaction=windowContext.settingsConnections] in interaction?.requestClose?() }
                        }
                        .onDisappear { windowContext.settingsNavigation.requestEditorClose=nil }
                }
            case "connectors":
                ConnectorSettingsView(request:model.requestConnector,requestCodex:model.requestCodex,revision:model.codexStateRevision,
                    embedded:true,formBuffer:windowContext.connectorFormBuffer,dismiss:windowContext.settingsNavigation.back)
            case "resources":
                VStack(spacing:0) {
                    BackendStatusBanner(availability:model.backendAvailability,retry:model.retryBackend)
                    ResourcesControlView(resourceEditor:model.resourceEditor,initialResourceID:route.resource,embedded:true,projection:model.resourcesProjection,
                        logTail:model.resourceLogTail,isLoading:model.isResourcesLoading,errorMessage:model.resourcesError,
                        backendActionsEnabled:model.backendAvailability.hostActionsEnabled,load:model.loadResources,
                        apply:model.applyResources,restart:model.restartResource,openMirror:model.openAndroidMirror,
                        loadLog:model.loadResourceLog,dismiss:windowContext.settingsNavigation.back)
                }
            case "workspace":
                WorkspaceFontSettings(context: windowContext)
            case "tmux_status_bar":
                TmuxStatusBarSettings(model: model, windowContext: windowContext, formBuffer: windowContext.settingsFormBuffer, editLease: windowContext.settingsEdits)
            case "notifications", "intelligence", "voice", "appearance":
                RetainedSettingsForm(context:windowContext,page:route.page,root:PolarStationSettingsRoot(initialVoicePage:route.page == "voice",embedded:true,
                    initialPage:route.page == "notifications" ? "Notifications" : route.page == "intelligence" ? "Intelligence" : route.page == "voice" ? "Voice" : "Appearance",
                    promptEditor:AnyView(SettingsPromptControls(interaction:PolarStationRuntime.shared.settingsText,buffer:PolarStationRuntime.shared.settingsTextBuffer)),
                    voiceTools:PolarStationRuntime.shared.settingsVoiceTools,model:model,windowContext:windowContext,editLease:windowContext.settingsEdits,formBuffer:windowContext.settingsFormBuffer,voiceFormBuffer:windowContext.voiceFormBuffer,
                    openResources:{ windowContext.settingsNavigation.open(.init(page:"resources")) },
                    openConnectors:{ windowContext.settingsNavigation.open(.init(page:"connectors")) },
                    dismiss:windowContext.settingsNavigation.back)).id(route.page)
            default:
                Text("Setting unavailable").foregroundStyle(.secondary)
            }
        }

    }

    /// Retain the original form tree and native fields across directory navigation.
    private struct RetainedSettingsForm:NSViewRepresentable {
        let context:PolarStationWindowContext
        let page:String
        let root:PolarStationSettingsRoot
        func makeNSView(context:Context)->NSHostingView<PolarStationSettingsRoot> {
            if let host=self.context.settingsFormHosts[page] {host.rootView=root;return host}
            let host=NSHostingView(rootView:root);self.context.settingsFormHosts[page]=host;return host
        }
        func updateNSView(_ view:NSHostingView<PolarStationSettingsRoot>,context:Context) {view.rootView=root}
    }

    struct PolarStationSettingsRoot: View {
        var initialVoicePage = false
        var embedded = false
        var initialPage: String? = nil
        var promptEditor:AnyView?=nil
        let voiceTools: PolarStationVoiceTools
        @ObservedObject var model: ControlCenterStore
        @ObservedObject var windowContext: PolarStationWindowContext
        var editLease:SettingsEditInteraction?=nil
        let formBuffer:SettingsFormBuffer
        let voiceFormBuffer:VoiceFormBuffer
        let openResources: @MainActor () -> Void
        let openConnectors:@MainActor () -> Void
        let dismiss: @MainActor () -> Void

        var body: some View {
            PolarStationSettingsView(
                projection: model.settingsProjection,
                isLoading: model.isSettingsLoading,
                errorMessage: model.settingsError,
                providerTestResult: model.settingsProviderTestResult,
                codexNotifyStatus: model.codexNotifyStatus,
                reload: model.loadSettings,
                updateAppearance: {model.updateAppearanceSettings($0,editLease:editLease)},
                updateAttention: {model.updateAttentionSettings($0,editLease:editLease)},
                updateMonitoring: {model.updateMonitoringSettings($0,editLease:editLease)},
                updateSessionSummary: {model.updateSessionSummarySettings($0,editLease:editLease)},
                testSessionSummary: model.testSessionSummaryProvider,
                inspectCodexNotify: model.inspectCodexNotify,
                configureCodexNotify: model.configureCodexNotify,
                openConnections: { PolarStationRuntime.shared.openSettingsRoute(.init(page:"connections"), in: windowContext) },
                openResources: openResources,
                openConnectors:openConnectors,
                dismiss: dismiss,
                messagesSettings: model.messagesSettings,
                messagesStatus: model.messagesStatus,
                requestConnector: model.requestConnector,
                requestCodex: model.requestCodex,
                messagesError: model.messagesError,
                isMessagesLoading: model.isMessagesLoading,
                reloadMessages: model.loadMessagesSender,
                authorizeMessages: model.authorizeMessagesSender,
                saveMessages: { model.saveMessagesSender($0,recipient:$1,editLease:formBuffer.messagesEdits) { clean in if clean {formBuffer.messagesRecipient=""} } },
                voicePlatform: voiceTools.request, initialVoicePage: initialVoicePage,
                openSetup: { section in PolarStationRuntime.shared.openSettingsRoute(.init(page:"setup", remote:false, section:section), in: windowContext) },
                embedded:embedded, initialPage:initialPage,promptEditor:promptEditor,editLease:editLease,formBuffer:formBuffer,voiceFormBuffer:voiceFormBuffer
            )
            .polarStationInterfaceTextScale(
                percent: model.settingsProjection?.appearance.interfaceTextScalePercent ?? 100
            )
        }
    }

    @MainActor
    struct PolarStationDiagnosticsRoot: View {
        @ObservedObject var model: ControlCenterStore
        let route: SettingsNavigation.Route
        private var remote: Bool { route.remote ?? false }
        @ObservedObject var context:PolarStationWindowContext
        @ObservedObject private var interaction:DiagnosticsInteraction
        init(model:ControlCenterStore,route:SettingsNavigation.Route,context:PolarStationWindowContext) {
            self.model=model;self.route=route;self.context=context;self.interaction=context.diagnostics;self.connectionsInteraction=context.settingsConnections
            _selectedMachine = State(initialValue: route.target)
        }
        @ObservedObject private var connectionsInteraction:ConnectionsInteraction
        @State private var editor: DiagnosticsInteraction.Action?
        @State private var editorRow: String?
        @State private var editorGroup: String?
        @State private var manualDestination = ""
        @State private var selectedAlias: String?
        @State private var selectedMachine: String?
        @State private var testTranscript = ""
        @State private var trialID: UUID?
        @State private var voiceFacts: [String: Any] = [:]
        @State private var voice = VoiceClient()
        private var voiceTools:PolarStationVoiceTools { PolarStationRuntime.shared.settingsVoiceTools }
        @State private var retainedRow: DiagnosticsInteraction.Row?
        @State private var retainedHost: DiagnosticsInteraction.Host?
        @State private var revealedSection: String?

        private func object<T: Encodable>(_ value: T) -> Any {
            (try? JSONSerialization.jsonObject(with: JSONEncoder().encode(value))) ?? NSNull()
        }
        @MainActor private func updateFacts() {
            interaction.toolSetup=context.toolSetup
            interaction.read = model.readSessionEditor
            interaction.saveMachine = { revision, machine in
                guard let controlRevision = model.projection?.controlRevision else { return }
                model.submitSSHTarget(.init(controlRevision: controlRevision,
                    operation: .updateConnection(expectedRevision: revision, change: .saveMachine(machine)))) { error in
                    interaction.showOperationError(error)
                }
            }
            interaction.present = { action in
                switch action.type {
                case "reconnect_host": model.retryBackend()
                case "try_dictate":
                    testTranscript = ""
                    startTrial()
                case "request_microphone_access":
                    AVCaptureDevice.requestAccess(for: .audio) { _ in Task { @MainActor in await sampleVoice() } }
                case "open_microphone_settings": MicrophonePermissionPresentation.openSettings()
                case "request_external_input_access", "open_external_input_settings":
                    if let url = URL(string:"x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") { NSWorkspace.shared.open(url) }
                default: editor = action
                }
            }
            let generation: UInt64
            if case .ready(let value) = model.backendAvailability { generation = value } else { generation = 0 }
            interaction.facts(generation:generation, value:[
                "host":model.backendAvailability.hostActionsEnabled ? "connected" : "offline",
                "connections":model.projection.map { object($0.connections) } ?? NSNull(),
                "resources":model.resourcesProjection.map(object) ?? NSNull(),
                "inspection_supported":model.projection.map { $0.targetToolsInspection as Any } ?? NSNull(),
                "voice":voiceFacts])
        }
        @MainActor private func sampleVoice() async {
            let permissions: [AVAuthorizationStatus: String] = [.notDetermined:"not_requested", .authorized:"granted", .denied:"denied", .restricted:"restricted"]
            voiceFacts["microphone"] = permissions[AVCaptureDevice.authorizationStatus(for:.audio)] ?? "unknown"
            voiceFacts["external_input_requested"] = true
            voiceFacts["external_input_permission"] = AXIsProcessTrusted() ? "granted" : "denied"
            let data = await voice.request(Data(#"{"op":"voice_status"}"#.utf8))
            if let result = try? JSONSerialization.jsonObject(with:data) as? [String:Any], result["ok"] as? Bool == true,
               let status = result["payload"] as? [String:Any] {
                voiceFacts["account_signed_in"] = status["signed_in"]
                voiceFacts["service_available"] = true
            } else {
                voiceFacts.removeValue(forKey:"account_signed_in")
                voiceFacts["service_available"] = false
            }
            updateFacts()
        }
        var body: some View {
            ScrollViewReader { scroll in
            ScrollView {
                VStack(alignment:.leading, spacing:16) {
                    if let projection = interaction.projection {
                        HStack {
                            Text(remote ? projection.remote_title : projection.local_title).font(.title2.weight(.semibold))
                            Spacer()
                            Button(projection.refresh_label) { interaction.refresh(); Task { await sampleVoice() } }.disabled(interaction.busy)
                        }
                        if let returnToSessionEditor = context.returnToSessionEditor {
                            Button("Back to Add Session") {
                                context.returnToSessionEditor = nil
                                returnToSessionEditor()
                            }
                        }
                        if remote {
                            let aliases = projection.remote_setup.filter { $0.id.hasPrefix("ssh:") }
                            Text("Add a host").font(.headline)
                            if !aliases.isEmpty {
                                Picker("SSH alias", selection: selection($selectedAlias, ids: aliases.map(\.id))) {
                                    ForEach(aliases) { row in Text(row.title).tag(row.id) }
                                }
                                if let row = aliases.first(where: { $0.id == selectedAlias }) ?? aliases.first {
                                    render(row, group: "remote_setup")
                                }
                            }
                            TextField("SSH destination", text: $manualDestination)
                                .disabled(interaction.busy || editor != nil)
                            Button("Connect") {
                                let destination = manualDestination.trimmingCharacters(in: .whitespacesAndNewlines)
                                selectedAlias = "ssh:" + destination
                                interaction.connectDestination(destination)
                            }.disabled(interaction.busy || editor != nil || manualDestination.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                            ForEach(projection.remote_setup.filter { !$0.id.hasPrefix("ssh:") }) { row in render(row, group:"remote_setup") }
                            let hosts = visibleHosts(projection.remote)
                            if hosts.isEmpty { Text(projection.empty_remote_label) }
                            else {
                                Picker("Managed host", selection: selection($selectedMachine, ids: hosts.map(\.id))) {
                                    ForEach(hosts) { host in Text(host.name).tag(host.id) }
                                }.disabled(editor != nil)
                                if let host = hosts.first(where: { $0.id == selectedMachine }) ?? hosts.first {
                                    renderCards(host.rows, group: host.id, sections: projection.capability_sections ?? [:])
                                }
                            }
                        } else {
                            renderCards(projection.local, group: "local", sections: projection.capability_sections ?? [:])
                        }
                    }
                    if let message = interaction.message { Text(message).foregroundStyle(.secondary).textSelection(.enabled) }
                    if interaction.busy { ProgressView() }
                }.padding(24).frame(maxWidth:.infinity, alignment:.leading)
            }
            .onAppear { revealSection(scroll) }
            .onChange(of: route) { _ in
                revealedSection = nil
                selectedMachine = route.target
                revealSection(scroll)
            }
            .onChange(of: interaction.projection) { _ in revealSection(scroll) }
            }
            .stationTheme()
            .polarStationInterfaceTextScale(percent:model.settingsProjection?.appearance.interfaceTextScalePercent ?? 100)
            .task { updateFacts(); interaction.refresh(); model.loadResources(); await sampleVoice() }
            .onDisappear {
                interaction.present=nil; interaction.saveMachine=nil
                if trialID != nil && testTranscript.isEmpty {
                    interaction.dictateTrialProgress("cancelled")
                }
                trialID = nil
            }
            .onChange(of:model.backendAvailability) { _ in updateFacts() }
            .onChange(of:model.projection?.controlRevision) { _ in updateFacts() }
            .onChange(of:model.resourcesProjection?.revision) { _ in updateFacts() }
            .onReceive(NotificationCenter.default.publisher(for:NSApplication.didBecomeActiveNotification)) { _ in
                interaction.returned(); Task { await sampleVoice() }
            }
        }
        private func revealSection(_ scroll: ScrollViewProxy) {
            guard let id = route.section, revealedSection != id,
                  let projection = interaction.projection else { return }
            let host = remote ? projection.remote.first(where: { $0.id == selectedMachine }) : nil
            guard let row = (remote ? host?.rows : projection.local)?.first(where: { $0.id == id }) else { return }
            revealedSection = id
            scroll.scrollTo(id, anchor: .top)
            if row.action?.type == "edit_voice_setup" {
                retainedRow = row
                editorRow = id
                editorGroup = host?.id ?? "local"
                editor = row.action
            }
        }
        private func selection(_ value: Binding<String?>, ids: [String]) -> Binding<String> {
            Binding(get: { value.wrappedValue.flatMap { ids.contains($0) ? $0 : nil } ?? ids.first ?? "" },
                    set: { value.wrappedValue = $0 })
        }
        private func visibleHosts(_ hosts: [DiagnosticsInteraction.Host]) -> [DiagnosticsInteraction.Host] {
            guard editor != nil, let retainedHost, !hosts.contains(where: { $0.id == retainedHost.id }) else { return hosts }
            return hosts + [retainedHost]
        }
        private func visibleRows(_ rows: [DiagnosticsInteraction.Row], group: String) -> [DiagnosticsInteraction.Row] {
            guard editor != nil, editorGroup == group, let retainedRow, !rows.contains(where: { $0.id == retainedRow.id }) else { return rows }
            return rows + [retainedRow]
        }
        private struct DiagnosticsCard: Identifiable {
            let id: String
            let title: String?
            var rows: [DiagnosticsInteraction.Row]
        }

        private func renderCards(
            _ rows: [DiagnosticsInteraction.Row], group: String, sections: [String: String]
        ) -> some View {
            var cards: [DiagnosticsCard] = []
            for row in visibleRows(rows, group: group) {
                let section = sections[row.id]
                let id = section.map { "section/" + $0 } ?? row.id
                if let index = cards.firstIndex(where: { $0.id == id }) {
                    cards[index].rows.append(row)
                } else {
                    cards.append(DiagnosticsCard(id: id, title: section, rows: [row]))
                }
            }
            return ForEach(cards) { card in
                VStack(alignment: .leading, spacing: 10) {
                    if let title = card.title { Text(title).font(.headline) }
                    ForEach(card.rows) { row in render(row, group: group).id(row.id) }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
            }
        }
        private func startTrial() {
            let attempt = UUID()
            trialID = attempt
            let started = PolarStationRuntime.shared.startSetupDictate(progress: { phase, error in
                guard trialID == attempt else { return }
                interaction.dictateTrialProgress(phase, error:error)
            }) { text in
                guard trialID == attempt, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return .unchanged }
                testTranscript = text
                interaction.dictateTrialSucceeded()
                return .applied
            }
            if !started {
                interaction.dictateTrialProgress("failed", error:"Dictate is unavailable. Check audio input or finish the current recording.")
            }
        }
        @ViewBuilder private func render(_ row: DiagnosticsInteraction.Row, group: String) -> some View {
            VStack(alignment:.leading, spacing:10) {
                HStack(alignment:.top, spacing:12) {
                    Image(systemName: row.state == "ready" ? "checkmark.circle.fill" : "info.circle")
                        .foregroundStyle(row.state == "ready" ? Color.green : Color.secondary)
                    VStack(alignment:.leading, spacing:4) { Text(row.title).font(.headline); Text(row.detail).foregroundStyle(.secondary) }
                }
                if let action = row.action, let label = row.action_label {
                    Button(label) {
                        retainedRow = row
                        retainedHost = interaction.projection?.remote.first(where: { $0.id == group })
                        editorRow = row.id
                        editorGroup = group
                        interaction.perform(action)
                    }.disabled(interaction.busy || editor != nil)
                }
                if row.id == "voice_trial", row.state == "ready", !testTranscript.isEmpty {
                    Text(testTranscript).textSelection(.enabled)
                }
                if editorRow == row.id, editorGroup == group, let action = editor {
                    setupEditor(action)
                    Button("Close details") {
                        if ["edit_connection", "edit_polar_connection", "edit_codex_endpoint", "add_machine_connection"].contains(action.type) {
                            connectionsInteraction.requestClose?()
                        } else { editor = nil }
                    }
                }
            }.frame(maxWidth:.infinity, alignment:.leading)
        }
        @ViewBuilder private func setupEditor(_ action: DiagnosticsInteraction.Action) -> some View {
            switch action.type {
            case "edit_voice_setup":
                VoiceSettingsView(platform:voiceTools.request, setupSection:action.section,formBuffer:context.voiceFormBuffer)
            case "edit_connection", "edit_polar_connection", "edit_codex_endpoint", "add_machine_connection":
                if let projection = model.projection {
                    ConnectionsView(interaction:connectionsInteraction, formBuffer:context.connectionsFormBuffer, projection:projection.connections, controlRevision:projection.controlRevision,
                        close:{editor=nil}, requestCodex:model.requestCodex, requestHostTransfer:model.requestHostTransfer, initialMachine:action.target_id ?? "local",
                        initialPage:action.type == "edit_codex_endpoint" ? "Codex" : action.type == "edit_polar_connection" ? "Polar" : "Connection",
                        initialPolar:action.connection_id, initialEndpoint:action.endpoint_id,
                        embedded:true, newMachine:action.type == "add_machine_connection",
                        submit:model.submitSSHTarget).frame(minHeight:380)
                }
            case "edit_resource":
                ResourcesControlView(resourceEditor:model.resourceEditor,initialResourceID:action.resource_id, embedded:true, projection:model.resourcesProjection,
                    logTail:model.resourceLogTail, isLoading:model.isResourcesLoading, errorMessage:model.resourcesError,
                    backendActionsEnabled:model.backendAvailability.hostActionsEnabled, load:model.loadResources,
                    apply:model.applyResources, restart:model.restartResource, openMirror:model.openAndroidMirror,
                    loadLog:model.loadResourceLog, dismiss:{editor=nil}).frame(minHeight:380)
            default: EmptyView()
            }
        }
    }

    private struct TmuxStatusBarSettings: View {
        @ObservedObject var model: ControlCenterStore
        @ObservedObject var windowContext: PolarStationWindowContext
        @ObservedObject var formBuffer: SettingsFormBuffer
        let editLease: SettingsEditInteraction
        @State private var preview: Preview?
        @State private var purposeDraft: String?

        private struct Selection: Equatable {
            let binding: PolarTmuxPaneContentBinding
            let workspaceName: String?
        }

        private struct PreviewRequest: Equatable {
            let selection: Selection?
            let revision: UInt64?
        }

        private struct Preview {
            let selection: Selection
            let facts: [String: String]?
        }

        private struct PreviewInput: Encodable {
            let fields: Set<String>
            let facts: [String: String]?
            let purpose: String?
        }

        private struct Projection: Decodable {
            struct Field: Decodable, Identifiable {
                let id: String
                let label: String
                let selected: Bool
            }
            let fields: [Field]
            let preview: String
        }

        private func projection(for draft: Set<String>, facts: [String: String]?) -> Projection? {
            guard let input = try? JSONEncoder().encode(PreviewInput(fields: draft, facts: facts, purpose: purposeDraft)) else { return nil }
            return input.withUnsafeBytes { buffer in
                guard let output = polar_station_tmux_status_bar_view(
                    buffer.bindMemory(to: UInt8.self).baseAddress, buffer.count
                ) else { return nil }
                defer { polar_station_desktop_string_free(output) }
                return try? JSONDecoder().decode(Projection.self, from: Data(String(cString: output).utf8))
            }
        }

        var body: some View {
            let binding = windowContext.executor.selectedTerminalBinding()
            let workspaceName = windowContext.executor.workspaceName
            let selection = binding.map { Selection(binding: $0, workspaceName: workspaceName) }
            let currentPreview = preview.flatMap { $0.selection == selection ? $0 : nil }
            let request = PreviewRequest(
                selection: model.backendAvailability.hostActionsEnabled ? selection : nil,
                revision: model.settingsProjection?.revision
            )
            Form {
                if let draft = formBuffer.tmuxStatusFieldsDraft,
                   let projection = projection(for: draft, facts: currentPreview?.facts) {
                    Text(previewText(binding: binding, current: currentPreview, draft: draft, line: projection.preview))
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)
                    Text("Save updates open panes in this workspace and sets the default for new sessions.")
                        .foregroundStyle(.secondary)
                    if let facts = currentPreview?.facts {
                        TextField("Task or purpose for this pane", text: Binding(
                            get: { purposeDraft ?? facts["purpose"] ?? "" },
                            set: { value in
                                let text = String(value.unicodeScalars.prefix(128))
                                purposeDraft = text == (facts["purpose"] ?? "") ? nil : text
                                editLease.edited(.tmuxStatusBar)
                            }
                        )).disabled(model.isSettingsLoading)
                    }
                    Button("Save") {
                        let purpose = binding.flatMap { pane in
                            purposeDraft.map { TmuxStatusPurposeEditV1(pane: pane, text: $0) }
                        }
                        model.updateTmuxStatusBar(
                            draft.sorted(), workspaceSessions: windowContext.executor.workspaceTerminalBindings(),
                            workspaceName: workspaceName, purpose: purpose, editLease: editLease
                        )
                    }
                    .disabled(model.isSettingsLoading)
                    ForEach(projection.fields) { field in
                        Toggle(field.label, isOn: Binding(
                            get: { field.selected },
                            set: { selected in
                                editLease.edited(.tmuxStatusBar)
                                if selected { formBuffer.tmuxStatusFieldsDraft?.insert(field.id) }
                                else { formBuffer.tmuxStatusFieldsDraft?.remove(field.id) }
                            }
                        )).disabled(model.isSettingsLoading)
                    }
                } else {
                    Text("tmux Status Bar settings are unavailable.")
                }
                if let error = model.settingsError { Text(error).foregroundStyle(.secondary) }
                if let warning = model.tmuxStatusWarning { Text(warning).foregroundStyle(.secondary) }
                if editLease.canRebase {
                    Button(editLease.rebaseLabel) {
                        editLease.rebase()
                        model.loadSettings()
                    }
                }
            }
            .formStyle(.grouped)
            .onChange(of: selection) { purposeDraft = nil }
            .task(id: request) {
                guard let selection = request.selection else {
                    preview = nil
                    return
                }
                let facts = await model.readTmuxStatusFacts(selection.binding, workspaceName: selection.workspaceName)
                guard !Task.isCancelled else { return }
                if let draft = purposeDraft, draft.trimmingCharacters(in: .whitespaces) == (facts?["purpose"] ?? "") {
                    purposeDraft = nil
                }
                preview = Preview(selection: selection, facts: facts)
            }
            .onChange(of: model.settingsProjection?.tmuxStatusBar, initial: true) {
                if formBuffer.tmuxStatusFieldsDraft == nil || editLease.replaces(.tmuxStatusBar) {
                    formBuffer.tmuxStatusFieldsDraft = model.settingsProjection?.tmuxStatusBar.map { Set($0) }
                }
            }
        }

        private func previewText(binding: PolarTmuxPaneContentBinding?, current: Preview?, draft: Set<String>, line: String) -> String {
            guard binding != nil else { return "Select a tmux pane to preview." }
            guard model.backendAvailability.hostActionsEnabled else { return "Preview unavailable for this pane." }
            guard let current else { return "Loading preview…" }
            guard current.facts != nil else { return "Preview unavailable for this pane." }
            if draft.isEmpty { return "Status bar hidden" }
            return line.isEmpty ? "No selected fields available." : line
        }
    }

    private struct WorkspaceFontSettings: View {
        @ObservedObject var context: PolarStationWindowContext
        @State private var preference: Preference?
        @State private var draft = 0
        @State private var saving = false
        @State private var message: String?

        private struct Preference: Decodable {
            let workspace_id: String
            let points: Int
            let min: Int
            let max: Int
        }

        var body: some View {
            Form {
                Section("Current Workspace Font") {
                    if let preference {
                        Stepper(value: $draft, in: preference.min...preference.max) {
                            Text("\(draft) pt").monospacedDigit()
                        }
                        .disabled(saving)
                        HStack {
                            Button("Cancel") { draft = preference.points; message = nil }
                                .disabled(saving || draft == preference.points)
                            Button("Apply") { apply(preference.workspace_id, points: draft) }
                                .disabled(saving)
                        }
                    } else {
                        Text("Open a Workspace to edit its terminal font.").foregroundStyle(.secondary)
                    }
                    if let message { Text(message).foregroundStyle(.secondary) }
                }
            }
            .formStyle(.grouped)
            .onChange(of: context.layout?.workspaceID, initial: true) { refresh() }
            .onChange(of: context.layout?.managedTerminalFontSizePoints) { refresh() }
        }

        private func refresh() {
            guard let layout = context.layout, !layout.workspaceID.isEmpty else {
                preference = nil
                message = nil
                return
            }
            let prior = preference
            do {
                let projection = try WorkspaceManagerWindow.transfer([
                    "action": "workspace_font", "workspace_id": layout.workspaceID,
                    "points": layout.managedTerminalFontSizePoints,
                ])
                let value = try JSONDecoder().decode(Preference.self, from: JSONSerialization.data(withJSONObject: projection))
                if prior?.workspace_id != value.workspace_id || prior?.points == draft {
                    draft = value.points
                    message = nil
                }
                preference = value
            } catch {
                preference = nil
                message = error.localizedDescription
            }
        }

        private func apply(_ id: String, points: Int) {
            saving = true
            message = nil
            Task {
                defer { saving = false }
                do {
                    let failures = try await context.executor.saveTerminalFont(workspaceID: id, points: points)
                    if preference?.workspace_id == id {
                        refresh()
                        message = failures == 0 ? "Workspace font saved" : "Workspace font saved. Some terminals could not apply it."
                    }
                } catch {
                    if preference?.workspace_id == id { message = error.localizedDescription }
                }
            }
        }
    }
#endif
