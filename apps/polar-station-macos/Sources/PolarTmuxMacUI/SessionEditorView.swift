import AppKit
import SwiftUI
import PolarTmuxHostClient

struct SessionTargetChoice: Identifiable, Hashable {
    let id: String
    let label: String
}

struct SessionEditorRequest: Identifiable {
    enum Kind {
        case createShell
        case createCommand
        case createRemote
        case rename(SessionRowProjectionV1)
    }

    let id = UUID()
    let kind: Kind
    var bindPane: String? = nil
    var bindView: String? = nil
    var initialType: String = "shell"
    var initialMode: String = "new"
    var resumeAgent: AgentSessionCatalogRowV1? = nil

    static let createShell = Self(kind: .createShell)
    static let createCommand = Self(kind: .createCommand)
    static let createRemote = Self(kind: .createRemote)

    static func rename(_ row: SessionRowProjectionV1) -> Self {
        Self(kind: .rename(row))
    }
}

struct SessionEditorSheet: View {
    let endpointStates: [String: String]
    let connections: ConnectionsProjectionV1?
    let controlRevision: UInt64
    let submitConnections: SessionEditorSubmit
    @State private var manageOpen = false
    let recovery: AgentSessionResumeV1?
    let request: SessionEditorRequest
    let destination: String
    let endpoints: [CodexEndpointSettingsV1]
    let remoteTargets: [SessionTargetChoice]
    let localTargetID: String
    let sourceSession: SessionRowProjectionV1.Identity?
    let requestCodex: CodexRequestHandler?
    let readSessionEditor: SessionEditorReader?
    let pick: (@MainActor (String,String,String,String) -> Void)?
    let cancel: @MainActor () -> Void
    let submit: @MainActor (ControlCenterIntentOperationV1, @escaping @MainActor (String?, [String]) -> Void) -> Void
    @FocusState private var nameFocused: Bool
    @State private var capturedDestination = ""
    @StateObject private var editor = SessionEditorInteraction()
    @State private var directoryOpen = false
    @State private var manualOpen = false
    @State private var manualDraft = ""
    @State private var previewExpanded = false
    @State private var optionsOpen = true
    @State private var historyFocusRequest = 0

    private func field<T>(_ key: WritableKeyPath<SessionEditorInteraction.Draft, T>) -> Binding<T> {
        Binding(get: { (editor.state?.draft ?? .init())[keyPath: key] },
                set: { value in editor.edit { $0[keyPath: key] = value } })
    }

    private var autoRestart: Bool {
        get { editor.state?.draft.auto_restart ?? false }
        nonmutating set { editor.edit { $0.auto_restart = newValue } }
    }
    private var yolo: Bool {
        get { editor.state?.draft.yolo ?? false }
        nonmutating set { editor.edit { $0.yolo = newValue } }
    }
    private var endpointID: String {
        get { editor.state?.draft.endpoint ?? "" }
        nonmutating set { editor.edit { $0.endpoint = newValue } }
    }
    private var sessionType: String {
        get { editor.state?.draft.session_type ?? "" }
        nonmutating set { editor.edit { $0.session_type = newValue } }
    }
    private var agentMode: String {
        get { editor.state?.draft.mode ?? "" }
        nonmutating set { editor.edit { $0.mode = newValue } }
    }
    private var originalSessionID: String {
        get { editor.state?.draft.original_id ?? "" }
        nonmutating set { editor.edit { $0.original_id = newValue } }
    }
    private var openLocalCLI: Bool {
        get { editor.state?.draft.open_local_cli ?? false }
        nonmutating set { editor.edit { $0.open_local_cli = newValue } }
    }
    private var historySearch: String {
        get { editor.state?.draft.search ?? "" }
        nonmutating set { editor.edit { $0.search = newValue } }
    }
    private var historyArchived: Bool {
        get { editor.state?.draft.archived ?? false }
        nonmutating set { editor.edit { $0.archived = newValue } }
    }
    private var historySubagents: Bool {
        get { editor.state?.draft.subagents ?? false }
        nonmutating set { editor.edit { $0.subagents = newValue } }
    }
    private var name: String {
        get { editor.state?.draft.name ?? "" }
        nonmutating set { editor.edit { $0.name = newValue } }
    }
    private var workingDirectory: String {
        get { editor.state?.draft.directory ?? "" }
        nonmutating set { editor.edit { $0.directory = newValue } }
    }
    private var command: String {
        get { editor.state?.draft.command ?? "" }
        nonmutating set { editor.edit { $0.command = newValue } }
    }
    private var selectedTargetID: String {
        get { editor.state?.draft.target ?? "" }
        nonmutating set { editor.edit { $0.target = newValue } }
    }
    private var pending: Bool { editor.state?.pending ?? false }
    private var readingDirectory: Bool { editor.state?.directory_loading ?? false }
    private var submissionError: String? { editor.state?.message }
    private var history: [CodexHistoryThreadV1] { editor.state?.history ?? [] }
    private var selectedThreads: [String] { editor.state?.selected ?? [] }
    private var manualID: Bool { editor.state?.manual ?? false }
    private var historyLoading: Bool { editor.state?.history_loading ?? false }
    private var historyError: String? { editor.state?.history_error }
    private var detail: CodexHistoryDetailV1? { editor.state?.detail }
    private var detailLoading: Bool { editor.state?.detail_loading ?? false }
    private var detailError: String? { editor.state?.detail_error }

    init(
        request: SessionEditorRequest,
        destination: String,
        remoteTargets: [SessionTargetChoice],
        localTargetID: String,
        endpoints: [CodexEndpointSettingsV1],
        sourceSession: SessionRowProjectionV1.Identity?,
        requestCodex: CodexRequestHandler?,
        readSessionEditor: SessionEditorReader?,
        recovery: AgentSessionResumeV1?,
        connections: ConnectionsProjectionV1?,
        endpointStates: [String: String],
        controlRevision: UInt64,
        submitConnections: @escaping SessionEditorSubmit,
        pick: (@MainActor (String,String,String,String) -> Void)? = nil,
        cancel: @escaping @MainActor () -> Void,
        submit:
            @escaping @MainActor (ControlCenterIntentOperationV1, @escaping @MainActor (String?, [String]) -> Void) -> Void
    ) {
        self.endpointStates = endpointStates
        self.connections = connections
        self.controlRevision = controlRevision
        self.submitConnections = submitConnections
        self.recovery = recovery
        self.request = request
        self.destination = destination
        self.endpoints = endpoints
        self.remoteTargets = remoteTargets
        self.localTargetID = localTargetID
        self.sourceSession = sourceSession
        self.requestCodex = requestCodex
        self.readSessionEditor = readSessionEditor
        self.pick = pick
        self.cancel = cancel
        self.submit = submit

    }

    private var binding: Bool { request.bindPane != nil }
    private var availableEndpoints: [CodexEndpointSettingsV1] { endpoints }
    private var browsingHistory: Bool { showsCreationOptions && sessionType == "codex" && agentMode == "resume" }

    var body: some View {
        VStack(spacing: 0) {
            editorHeader.padding(.horizontal, 20).padding(.vertical, 12)
            Divider()
            if browsingHistory {
                historyPicker.padding(.horizontal, 20).padding(.vertical, 12)
            } else {
                HSplitView {
                    if showsCreationOptions, !readingDirectory, let readSessionEditor {
                        SessionDirectoryPicker(target: selectedTargetID,
                            endpoint: sessionType == "codex" ? optional(endpointID) : nil,
                            initialPath: workingDirectory, embedded: true, read: readSessionEditor,
                            choose: { workingDirectory = $0 }, cancel: {})
                            .id(selectedTargetID + "|" + endpointID)
                            .frame(minWidth: 340, maxWidth: .infinity, maxHeight: .infinity)
                    }
                    ScrollView { newSessionForm.padding(24).frame(maxWidth: 720, alignment: .leading) }
                        .frame(minWidth: 420, maxWidth: .infinity)
                }
            }
            if let submissionError {
                Text(submissionError).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true).padding(.horizontal, 20)
            }
            if showsCreationOptions && sessionType == "codex" {
                Text(endpointID.isEmpty ? "Not configured" : (endpointStates[endpointID] ?? "Unknown"))
                    .font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 20)
            }
            if pending { ProgressView("Opening Session…") }
            Divider()
            HStack(spacing: 12) {
                if browsingHistory {
                    Button("Open using ID…") { manualDraft = originalSessionID; manualOpen = true }
                        .popover(isPresented: $manualOpen) { manualIDEditor }
                    if manualID { Text("Using entered ID").font(.caption).foregroundStyle(.secondary) }
                    if !binding { Menu("More options") {
                        Toggle("Open CLI on this Station Host", isOn: field(\.open_local_cli)).disabled(!localCLIEndpointSupported)
                    }.fixedSize() }
                }
                Text(capturedDestination.isEmpty ? destination : capturedDestination).lineLimit(1).font(.caption).foregroundStyle(.secondary).help(destination)
                Spacer(minLength: 0)
                Button("Cancel", role: .cancel, action: cancel).keyboardShortcut(.cancelAction).disabled(pending)
                Button(submitLabel, action: submitOnce).keyboardShortcut(.defaultAction).buttonStyle(.borderedProminent)
                    .disabled(!canSubmit || pending || (readingDirectory && agentMode != "resume"))
            }.padding(.horizontal, 20).padding(.vertical, 12)
        }
        .frame(minWidth: showsCreationOptions ? 800 : 420,
               maxWidth: .infinity, minHeight: showsCreationOptions ? 494 : 180,
               maxHeight: .infinity)
        .stationTheme()
        .background(SessionEditorWindowSizing(enabled: showsCreationOptions, pending: pending))
        .disabled(pending)
        .sheet(isPresented: $manageOpen) {
            if let connections {
                ConnectionsView(projection: connections, controlRevision: controlRevision, close: { manageOpen = false }, requestCodex: requestCodex, initialMachine: selectedTargetID, initialPage: "Codex", submit: submitConnections)
            }
        }
        .onAppear {
            optionsOpen = true
            if capturedDestination.isEmpty { capturedDestination = destination }
            nameFocused = request.initialMode != "resume" && request.resumeAgent == nil
            editor.submit = submit
            editor.pick = pick
            var draft = SessionEditorInteraction.Draft()
            draft.bind_only = binding
            draft.target = request.resumeAgent?.execution_target_id ?? sourceSession?.executionTargetID ?? localTargetID
            draft.session_type = request.initialType; draft.mode = request.initialMode
            if case .createCommand = request.kind { draft.session_type = "command" }
            if case .rename(let row) = request.kind { draft.name = row.displayName; draft.rename = row.identity }
            if let agent = request.resumeAgent {
                draft.mode = "resume"; draft.endpoint = agent.endpoint_id; draft.original_id = agent.thread_id
            }
            if binding { draft.target = localTargetID }
            editor.startCodex=requestCodex
            editor.open(draft, endpoints: availableEndpoints, source: sourceSession, read: readSessionEditor)
        }
        .onDisappear { editor.action("close") }
        .onChange(of: endpoints) { _ in editor.endpoints(availableEndpoints) }
        .onChange(of: recovery) { value in
            guard let value else { return }
            editor.edit { $0.target = value.execution_target_id; $0.session_type = "codex"; $0.endpoint = value.endpoint_id; $0.mode = "resume"; $0.original_id = value.thread_id }
            editor.manual(value.thread_id)
        }
        .sheet(isPresented: $directoryOpen) {
            if let readSessionEditor {
                SessionDirectoryPicker(target: selectedTargetID, endpoint: sessionType == "codex" ? optional(endpointID) : nil,
                    initialPath: workingDirectory, read: readSessionEditor,
                    choose: { workingDirectory = $0; directoryOpen = false }, cancel: { directoryOpen = false })
            }
        }
        .interactiveDismissDisabled(pending)
    }

    private var editorHeader: some View {
        HStack(spacing: 10) {
            if !showsCreationOptions { Text(title).font(.headline).fixedSize() }
            if showsCreationOptions {
                Picker("Type", selection: field(\.session_type)) {
                    Text("Codex").tag("codex"); Text("Claude").tag("claude")
                    Text("Shell").tag("shell"); Text("Command").tag("command")
                }.labelsHidden().frame(width: 100).accessibilityLabel("Type").disabled(binding)
                Picker("Host", selection: field(\.target)) {
                    Text("This computer").tag(localTargetID)
                    ForEach(remoteTargets) { Text($0.label).tag($0.id) }
                }.labelsHidden().frame(minWidth: 120, maxWidth: 180).accessibilityLabel("Host")
                if sessionType == "codex" {
                    Picker("App server", selection: field(\.endpoint)) {
                        Text("Select app server").tag("")
                        ForEach(availableEndpoints.filter { $0.execution_target_id == selectedTargetID }, id: \.id) { Text($0.id).tag($0.id) }
                    }.labelsHidden().frame(minWidth: 120, maxWidth: 200).accessibilityLabel("App server")
                    if editor.state?.can_start_local_server == true || editor.state?.server_starting == true {
                        Button{editor.action("start_local_server")}label:{Image(systemName:editor.state?.server_starting == true ? "hourglass" : "power")}
                            .help(editor.state?.server_start_label ?? "Start local Codex app server").accessibilityLabel(editor.state?.server_start_label ?? "Start local Codex app server").disabled(editor.state?.can_start_local_server != true)
                    }
                    Button { manageOpen = true } label: { Image(systemName: "gearshape") }
                        .help("Manage app servers").accessibilityLabel("Manage…").disabled(connections == nil)
                }
                Spacer(minLength: 0)
                if isAgent && !binding { modeTab("New", value: "new"); modeTab("Resume", value: "resume") }
            }
        }.controlSize(.small)
    }

    private func modeTab(_ label: String, value: String) -> some View {
        Button { agentMode = value } label: {
            Text(label).fontWeight(agentMode == value ? .semibold : .regular)
                .foregroundStyle(agentMode == value ? StationTheme.shared.accent : Color.secondary)
                .padding(.vertical, 5)
                .overlay(alignment: .bottom) {
                    if agentMode == value { Rectangle().fill(StationTheme.shared.accent).frame(height: 2) }
                }
        }.buttonStyle(.plain).accessibilityAddTraits(agentMode == value ? .isSelected : [])
    }

    private var newSessionForm: some View {
        VStack(alignment: .leading, spacing: 16) {
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 14) {
                GridRow {
                    Text("Session name").foregroundStyle(.secondary).frame(width: 120, alignment: .trailing)
                    TextField("Session name", text: field(\.name)).focused($nameFocused).onSubmit(submitOnce)
                }
                if showsCreationOptions {
                    GridRow {
                        Text("Working directory").foregroundStyle(.secondary).frame(width: 120, alignment: .trailing)
                        HStack {
                            TextField("Working directory", text: field(\.directory)).onSubmit(submitOnce)
                        }
                    }
                }
                if sessionType == "claude" && agentMode == "resume" {
                    GridRow {
                        Text("Session UUID").foregroundStyle(.secondary).frame(width: 120, alignment: .trailing)
                        TextField("Claude session UUID", text: field(\.original_id)).onSubmit(submitOnce)
                    }
                }
                if requiresCommand {
                    GridRow {
                        Text("Command").foregroundStyle(.secondary).frame(width: 120, alignment: .trailing)
                        TextEditor(text: field(\.command)).font(.body.monospaced()).frame(minHeight: 120).accessibilityLabel("Command")
                    }
                }
            }
            if readingDirectory { ProgressView("Reading startup directory…").controlSize(.small) }
            if requiresCommand { Toggle("Automatically restart command after server loss", isOn: field(\.auto_restart)) }
            if showsCreationOptions && sessionType == "codex" {
                DisclosureGroup("More options", isExpanded: $optionsOpen) {
                    VStack(alignment: .leading, spacing: 10) {
                        Toggle("YOLO · Skip approvals and sandbox", isOn: field(\.yolo))
                        Toggle("Open CLI on this Station Host", isOn: field(\.open_local_cli)).disabled(!localCLIEndpointSupported)
                        Text("By default the CLI opens on the selected Host.").font(.caption).foregroundStyle(.secondary)
                    }.padding(.top, 8)
                }
            }
        }.frame(maxWidth: .infinity, alignment: .leading)
    }

    private func selectHistory(_ id: String, checked: Bool) {
        editor.select(id, checked: checked)
        historyFocusRequest += 1
    }

    private var historyPicker: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 10) {
                TextField("Search conversations", text: field(\.search)).submitScope()
                Menu {
                    Toggle("Archived", isOn: field(\.archived))
                    Toggle("Include subagents", isOn: field(\.subagents))
                } label: { Text("Filters") }.fixedSize()
                Text("\(selectedThreads.count) selected").font(.caption).foregroundStyle(.secondary)
            }
            if let historyError {
                HStack { Text(historyError).foregroundStyle(.red); Button("Retry") { editor.action("retry_history") } }
            }
            HSplitView {
                GeometryReader { viewport in
                    ScrollViewReader { scroller in
                    ScrollView {
                        LazyVStack(spacing: 0) {
                            ForEach(history) { row in
                                HStack(spacing: 8) {
                                    Toggle("Select \(row.display_name)", isOn: Binding(
                                        get: { selectedThreads.contains(row.id) },
                                        set: { selectHistory(row.id, checked: $0) }))
                                        .labelsHidden().toggleStyle(.checkbox)
                                    HStack(spacing: 8) {
                                        Text(row.display_name).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
                                        Text(URL(fileURLWithPath: row.cwd).lastPathComponent)
                                            .font(.caption).foregroundStyle(.secondary).lineLimit(1).frame(maxWidth: 90)
                                    }.frame(height: 32).contentShape(Rectangle())
                                        .gesture(TapGesture(count: 2).exclusively(before: TapGesture(count: 1)).onEnded { result in
                                            switch result {
                                            case .first: editor.activate(row.id)
                                            case .second: editor.select(row.id); historyFocusRequest += 1
                                            }
                                        })
                                }.padding(.horizontal, 8).frame(height: 32)
                                    .background(originalSessionID == row.id ? StationTheme.shared.accent.opacity(0.12) : Color.clear)
                                    .id(row.id)
                                    .help(row.display_name + "\n" + row.cwd)
                            }
                            Color.clear.frame(height: 1).background(GeometryReader { marker in
                                Color.clear.preference(key: SessionHistoryBottom.self,
                                    value: marker.frame(in: .named("session-history")).maxY)
                            })
                        }
                    }.coordinateSpace(name: "session-history")
                        .onPreferenceChange(SessionHistoryBottom.self) { bottom in
                            if bottom > 0 && bottom <= viewport.size.height + 160 && editor.state?.can_more == true {
                                editor.action("more")
                            }
                        }
                        .onChange(of: originalSessionID) { id in scroller.scrollTo(id) }
                    }
                    .background(SessionHistoryKeyboard(focusRequest: historyFocusRequest) { action in
                        if action == "cancel" { if !pending { cancel() } }
                        else { editor.action(action) }
                    })
                }.frame(minWidth: 280, idealWidth: 340, maxWidth: 480)
                historyDetails.frame(minWidth: 360, maxWidth: .infinity)
            }.frame(maxHeight: .infinity)
            if historyLoading { ProgressView("Loading conversations…").controlSize(.small) }
            else if history.isEmpty && historyError == nil { Text("No conversations found.").foregroundStyle(.secondary) }
        }
    }

    private func messageCard(_ title: String, icon: String, text: String) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(title, systemImage: icon).font(.subheadline.weight(.semibold)).foregroundStyle(.secondary)
            Text(text).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
        }.padding(16).frame(maxWidth: .infinity, alignment: .leading)
            .background(StationTheme.shared.surface, in: RoundedRectangle(cornerRadius: 10))
    }

    private var historyDetails: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let row = history.first(where: { $0.thread_id == originalSessionID }) {
                VStack(alignment: .leading, spacing: 10) {
                    HStack(alignment: .top) {
                        Text(row.display_name).font(.title3.weight(.semibold)).lineLimit(2).help(row.display_name).textSelection(.enabled)
                        Spacer(minLength: 12)
                        Text(row.status).font(.caption).padding(.horizontal, 10).padding(.vertical, 5)
                            .background(StationTheme.shared.accent.opacity(0.12), in: Capsule())
                    }
                    HStack {
                        Label(row.cwd, systemImage: "folder").lineLimit(1).truncationMode(.middle).help(row.cwd)
                        Button { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(row.cwd, forType: .string) }
                            label: { Image(systemName: "doc.on.doc") }.help("Copy directory").accessibilityLabel("Copy directory")
                    }.font(.callout).foregroundStyle(.secondary)
                    HStack(spacing: 12) {
                        Image(systemName: "clock")
                        Text(Date(timeIntervalSince1970: Double(row.updated_at)), style: .relative)
                            .help(Date(timeIntervalSince1970: Double(row.updated_at)).formatted())
                        Spacer()
                        Text(String(row.thread_id.prefix(8))).font(.caption.monospaced()).help(row.thread_id)
                        Button("Copy ID") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(row.thread_id, forType: .string) }
                    }.font(.caption).foregroundStyle(.secondary)
                }.padding(16)
                Divider()
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        if let preview = row.preview, !preview.isEmpty, preview != row.display_name {
                            VStack(alignment: .leading, spacing: 8) {
                                Label("Opening preview", systemImage: "text.bubble").font(.subheadline.weight(.semibold))
                                Text(preview).lineLimit(previewExpanded ? nil : 4).textSelection(.enabled)
                                Button(previewExpanded ? "Show less" : "Show more") { previewExpanded.toggle() }.buttonStyle(.plain).font(.caption)
                            }.padding(16).frame(maxWidth: .infinity, alignment: .leading)
                                .background(StationTheme.shared.surface, in: RoundedRectangle(cornerRadius: 10))
                        }
                        if detailLoading { ProgressView("Loading latest exchange…").frame(maxWidth: .infinity, minHeight: 100) }
                        if let detail, detail.thread.id == row.id {
                            if let text = detail.user_text { messageCard("You", icon: "person.crop.circle", text: text) }
                            if let text = detail.assistant_text { messageCard("Codex", icon: "sparkles", text: text) }
                            if detail.user_text == nil && detail.assistant_text == nil { Label("No messages yet", systemImage: "bubble.left.and.bubble.right").foregroundStyle(.secondary).padding(24) }
                        }
                        if let detailError {
                            Label(detailError, systemImage: "exclamationmark.triangle").foregroundStyle(.red)
                            Button("Retry") { editor.action("retry_detail") }
                        }
                    }.padding(16).frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                Spacer()
                Label("Select a conversation to see its details", systemImage: "bubble.left.and.text.bubble.right")
                    .foregroundStyle(.secondary).frame(maxWidth: .infinity)
                Spacer()
            }
        }.onChange(of: originalSessionID) { _ in previewExpanded = false }
    }

    private var manualIDEditor: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Open using original ID").font(.headline)
            TextField("Codex Thread ID", text: $manualDraft).submitScope()
            HStack { Spacer(); Button("Cancel") { manualOpen = false }
                Button("Use ID") { editor.manual(manualDraft); manualOpen = false }
                    .disabled(trimmed(manualDraft).isEmpty)
            }
        }.padding(16).frame(width: 390)
    }

    private func submitOnce() { editor.action("submit") }

    private var title: String {
        switch request.kind {
        case .createShell, .createCommand, .createRemote:
            "Add Session"
        case .rename:
            "Rename Session"
        }
    }

    private var submitLabel: String { editor.state?.submit_label ?? "New & Open" }

    private var showsCreationOptions: Bool {
        if case .rename = request.kind { false } else { true }
    }

    private var requiresCommand: Bool {
        showsCreationOptions && sessionType == "command"
    }

    private var isAgent: Bool {
        showsCreationOptions && (sessionType == "codex" || sessionType == "claude")
    }

    private var localCLIEndpointSupported: Bool { editor.state?.local_cli_supported ?? false }
    private var canSubmit: Bool { editor.state?.can_submit ?? false }

    private func optional(_ value: String) -> String? {
        let value = trimmed(value)
        return value.isEmpty ? nil : value
    }

    private func trimmed(_ value: String) -> String {
        value.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

// Geometry is a native preference, independent of Session/Tab identity.
private struct SessionEditorWindowSizing: NSViewRepresentable {
    let enabled: Bool
    let pending: Bool
    func makeNSView(context: Context) -> SizingView { SizingView() }
    func updateNSView(_ view: SizingView, context: Context) {
        view.enabled = enabled
        view.window?.isDocumentEdited = pending
        DispatchQueue.main.async { [weak view] in view?.configure() }
    }
    static func dismantleNSView(_ view: SizingView, coordinator: ()) { view.stopObserving() }

    final class SizingView: NSView {
        var enabled = false
        private var configured = false
        private let defaults = ProcessInfo.processInfo.environment["GHOSTTY_USER_DEFAULTS_SUITE"]
            .flatMap(UserDefaults.init(suiteName:)) ?? .standard
        private var key: String { "PolarStation.sessionEditor.golden.v3.size" }
        @objc private func saveUserSize() {
            guard configured, let window else { return }
            let size = window.contentLayoutRect.size
            defaults.set(window.isZoomed, forKey: key + ".maximized")
            if !window.isZoomed { defaults.set([size.width, size.height], forKey: key) }
        }
        func stopObserving() {
            NotificationCenter.default.removeObserver(self)
        }
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            stopObserving()
            if let window {
                NotificationCenter.default.addObserver(self, selector: #selector(saveUserSize),
                    name: NSWindow.didEndLiveResizeNotification, object: window)
                NotificationCenter.default.addObserver(self, selector: #selector(saveUserSize),
                    name: NSWindow.willCloseNotification, object: window)
            }
            DispatchQueue.main.async { [weak self] in self?.configure() }
        }
        func configure() {
            guard enabled, let window else { return }
            guard !configured else { return }; configured = true
            let available = (window.screen ?? NSScreen.main)?.visibleFrame.size ?? NSSize(width: 1280, height: 800)
            let stored = defaults.array(forKey: key) as? [Double]
            let goldenWidth = min(max(800, available.width * 0.618), (available.height - 64) * 1.618)
            var size = NSSize(width: goldenWidth, height: goldenWidth / 1.618)
            if let stored, stored.count == 2, stored.allSatisfy({ $0.isFinite && $0 > 0 }) { size = NSSize(width: stored[0], height: stored[1]) }
            size.width = min(max(800, size.width), available.width - 32)
            size.height = min(max(494, size.height), available.height - 64)
            window.styleMask.insert(.resizable)
            window.contentMinSize = NSSize(width: min(800, available.width - 32), height: min(494, available.height - 64))
            window.setContentSize(size)
            if let frame = (window.screen ?? NSScreen.main)?.visibleFrame {
                window.setFrameOrigin(NSPoint(x: frame.minX + max(0, (frame.width - window.frame.width) / 2),
                                              y: frame.minY + max(0, (frame.height - window.frame.height) / 2)))
            }
            if defaults.bool(forKey: key + ".maximized") { window.zoom(nil) }
        }
    }
}

private struct SessionHistoryBottom: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}
