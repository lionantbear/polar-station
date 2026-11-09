import AppKit
import PolarTmuxHostClient
import SwiftUI

let polarStationPortFormat = IntegerFormatStyle<UInt16>.number.grouping(.never)

/// Native list-and-detail editor for the Host-owned global Resource catalog.
/// Draft edits remain local until Apply submits the complete revision-bound
/// candidate. Rust remains the owner of validation, lifecycle, and recovery.
public struct ResourcesControlView: View {
    public let projection: ResourcesControlProjectionV1?
    public let logTail: ResourceLogTailV1?
    public let isLoading: Bool
    public let errorMessage: String?
    public let backendActionsEnabled: Bool
    public let load: @MainActor () -> Void
    public let apply: @MainActor (ResourceCatalogV1) -> Void
    public let restart: @MainActor (String) -> Void
    public let openMirror: @MainActor (String) -> Void
    public let loadLog: @MainActor (String) -> Void
    public let dismiss: @MainActor () -> Void

    @State private var draft = ResourceCatalogV1.empty
    @State private var loadedRevision: UInt64?
    @State private var loadedCatalog: ResourceCatalogV1?
    @State private var conflictMessage: String?
    @State private var selectedKind = ResourceKind.ports
    @State private var selectedResourceID: String?
    @State private var showingAdvanced = false
    @State private var showingDiscardConfirmation = false
    @State private var showingLog = false
    @State private var selectedLogResourceID: String?

    public init(
        projection: ResourcesControlProjectionV1?,
        logTail: ResourceLogTailV1?,
        isLoading: Bool,
        errorMessage: String?,
        backendActionsEnabled: Bool = true,
        load: @escaping @MainActor () -> Void,
        apply: @escaping @MainActor (ResourceCatalogV1) -> Void,
        restart: @escaping @MainActor (String) -> Void,
        openMirror: @escaping @MainActor (String) -> Void,
        loadLog: @escaping @MainActor (String) -> Void,
        dismiss: @escaping @MainActor () -> Void
    ) {
        self.projection = projection
        self.logTail = logTail
        self.isLoading = isLoading
        self.errorMessage = errorMessage
        self.backendActionsEnabled = backendActionsEnabled
        self.load = load
        self.apply = apply
        self.restart = restart
        self.openMirror = openMirror
        self.loadLog = loadLog
        self.dismiss = dismiss
    }

    public var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if projection == nil, isLoading {
                ProgressView("Loading global Resources…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                editor
            }
        }
        .background(.regularMaterial)
        .onAppear {
            synchronizeDraft(projection)
            ensureSelection()
        }
        .onChange(of: projection) { next in
            synchronizeDraft(next)
            ensureSelection()
        }
        .onChange(of: selectedKind) { _ in
            selectedResourceID = nil
            ensureSelection()
        }
        .confirmationDialog(
            "Discard unapplied Resource changes?",
            isPresented: $showingDiscardConfirmation,
            titleVisibility: .visible
        ) {
            Button("Discard and Reload", role: .destructive) {
                if let projection {
                    draft = projection.catalog
                    loadedCatalog = projection.catalog
                    loadedRevision = projection.revision
                    conflictMessage = nil
                    ensureSelection()
                }
                load()
            }
            Button("Cancel", role: .cancel) {}
        }
        .sheet(isPresented: $showingLog) { resourceLogSheet }
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: "externaldrive.connected.to.line.below")
                .font(.system(size: 24))
                .foregroundStyle(.tint)
            VStack(alignment: .leading, spacing: 2) {
                Text("Resources").font(.title2.bold())
                Text(projection?.aggregateMessage ?? "Global Host resources")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer()
            Button("Reload", action: requestReload)
                .disabled(isLoading || !backendActionsEnabled)
            Button("Done", action: dismiss)
                .keyboardShortcut(.cancelAction)
                .accessibilityLabel("Close Resources")
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(.bar)
    }

    private var editor: some View {
        VStack(spacing: 0) {
            if targetIDs.isEmpty {
                Label(
                    "Add a registered SSH execution target before creating Resources.",
                    systemImage: "exclamationmark.triangle"
                )
                .font(.callout)
                .foregroundStyle(.orange)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
            }
            HSplitView {
                resourceList
                    .frame(minWidth: 220, idealWidth: 250, maxWidth: 300)
                selectedResourceDetail
                    .frame(minWidth: 420, maxWidth: .infinity, maxHeight: .infinity)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            Divider()
            footer
        }
    }

    private enum ResourceKind: String, CaseIterable, Identifiable {
        case ports = "Ports"
        case files = "Files"
        case android = "Android"

        var id: String { rawValue }
        var addLabel: String {
            switch self {
            case .ports: "Add Port Forward"
            case .files: "Add File Mapping"
            case .android: "Add Android Mirror"
            }
        }
    }

    private struct ResourceListItem: Identifiable {
        let id: String
        let name: String
        let summary: String
    }

    private var resourceList: some View {
        VStack(spacing: 0) {
            Picker("Resource type", selection: $selectedKind) {
                ForEach(ResourceKind.allCases) { kind in
                    Text(kind.rawValue).tag(kind)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(12)

            Divider()
            ScrollView {
                LazyVStack(spacing: 6) {
                    ForEach(resourceItems) { item in
                        Button {
                            selectedResourceID = item.id
                        } label: {
                            HStack(spacing: 10) {
                                Circle()
                                    .fill(status(item.id).color)
                                    .frame(width: 8, height: 8)
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(item.name.isEmpty ? "Untitled Resource" : item.name)
                                        .font(.headline)
                                        .lineLimit(1)
                                    Text(item.summary)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                }
                                Spacer(minLength: 0)
                            }
                            .padding(10)
                            .contentShape(Rectangle())
                            .background(
                                selectedResourceID == item.id
                                    ? StationTheme.shared.accent.opacity(0.18)
                                    : Color.secondary.opacity(0.06),
                                in: RoundedRectangle(cornerRadius: 8)
                            )
                        }
                        .buttonStyle(.plain)
                    }
                    if resourceItems.isEmpty {
                        Text("No \(selectedKind.rawValue) resources")
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 30)
                    }
                }
                .padding(10)
            }
            Button {
                addSelectedKind()
            } label: {
                Label(selectedKind.addLabel, systemImage: "plus")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)
            .padding(12)
            .disabled(targetIDs.isEmpty)
        }
        .background(Color.secondary.opacity(0.035))
    }

    @ViewBuilder
    private var selectedResourceDetail: some View {
        ScrollView {
            Group {
                switch selectedKind {
                case .ports:
                    if let index = draft.portForwards.firstIndex(where: {
                        $0.id == selectedResourceID
                    }) {
                        portForwardEditor($draft.portForwards[index])
                    } else {
                        emptyDetail("Select or add a Port Forward")
                    }
                case .files:
                    if let index = draft.fileMappings.firstIndex(where: {
                        $0.id == selectedResourceID
                    }) {
                        fileMappingEditor($draft.fileMappings[index])
                    } else {
                        emptyDetail("Select or add a File Mapping")
                    }
                case .android:
                    if let index = draft.androidMirrors.firstIndex(where: {
                        $0.id == selectedResourceID
                    }) {
                        androidEditor($draft.androidMirrors[index])
                    } else {
                        emptyDetail("Select or add an Android Mirror")
                    }
                }
            }
            .padding(20)
            .frame(maxWidth: 760, alignment: .topLeading)
            .frame(maxWidth: .infinity, alignment: .top)
        }
    }

    private func portForwardEditor(
        _ forward: Binding<PortForwardSpecV1>
    ) -> some View {
        VStack(alignment: .leading, spacing: 18) {
            resourceHeader(
                name: forward.name,
                enabled: forward.enabled,
                resourceID: forward.wrappedValue.id,
                remove: { removeResource(forward.wrappedValue.id) }
            )
            targetPicker(forward.executionTargetID)
            endpointGroup(
                title: "This Mac",
                explanation: "Applications on this computer connect to this bind address and local port range."
            ) {
                LabeledContent("Bind address") {
                    TextField("127.0.0.1", text: forward.bindAddress)
                        .textFieldStyle(.roundedBorder)
                }
                HStack {
                    LabeledContent("From") {
                        TextField("From", value: forward.localPortStart, format: polarStationPortFormat)
                            .textFieldStyle(.roundedBorder)
                    }
                    LabeledContent("To") {
                        TextField("To", value: forward.localPortEnd, format: polarStationPortFormat)
                            .textFieldStyle(.roundedBorder)
                    }
                }
            }
            Image(systemName: "arrow.down")
                .foregroundStyle(.tint)
                .frame(maxWidth: .infinity)
                .accessibilityLabel("Maps each local port by the same offset")
            endpointGroup(
                title: "Remote through target",
                explanation: "The Host reaches this address after connecting to the selected SSH target."
            ) {
                LabeledContent("Remote host") {
                    TextField("10.0.8.106", text: forward.remoteHost)
                        .textFieldStyle(.roundedBorder)
                }
                LabeledContent("Starting port") {
                    TextField("Port", value: forward.remotePortStart, format: polarStationPortFormat)
                        .textFieldStyle(.roundedBorder)
                }
                Text("Remote range: \(remoteRange(forward.wrappedValue))")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(
                        remoteRangeIsValid(forward.wrappedValue)
                            ? Color.secondary : Color.red
                    )
            }
            DisclosureGroup("Advanced", isExpanded: $showingAdvanced) {
                Toggle(
                    "Allow non-loopback exposure on this Mac",
                    isOn: forward.allowNonLoopback
                )
                .padding(.top, 6)
            }
            runtimeActions(forward.wrappedValue.id)
        }
    }

    private func fileMappingEditor(
        _ mapping: Binding<FileMappingSpecV1>
    ) -> some View {
        VStack(alignment: .leading, spacing: 18) {
            resourceHeader(
                name: mapping.name,
                enabled: mapping.enabled,
                resourceID: mapping.wrappedValue.id,
                remove: { removeResource(mapping.wrappedValue.id) }
            )
            targetPicker(mapping.executionTargetID)
            endpointGroup(
                title: "Remote on target",
                explanation: "Directory read through the selected SSH execution target."
            ) {
                LabeledContent("Remote path") {
                    TextField("/home/operator", text: mapping.remotePath)
                        .textFieldStyle(.roundedBorder)
                }
            }
            endpointGroup(
                title: "On this computer",
                explanation: "Local mount path exposed to applications on this Mac."
            ) {
                LabeledContent("Local path") {
                    TextField("~/Polar/remote", text: mapping.localPath)
                        .textFieldStyle(.roundedBorder)
                }
            }
            HStack {
                Button("Show Local") {
                    revealAppliedLocalPath(mapping.wrappedValue.id)
                }
                .disabled(
                    !backendActionsEnabled
                        || entry(mapping.wrappedValue.id)?.canRevealLocalPath != true
                )
                runtimeActions(mapping.wrappedValue.id)
            }
        }
    }

    private func androidEditor(
        _ mirror: Binding<AndroidMirrorSpecV1>
    ) -> some View {
        VStack(alignment: .leading, spacing: 18) {
            resourceHeader(
                name: mirror.name,
                enabled: mirror.enabled,
                resourceID: mirror.wrappedValue.id,
                remove: { removeResource(mirror.wrappedValue.id) }
            )
            targetPicker(mirror.executionTargetID)
            endpointGroup(
                title: "Local ADB",
                explanation: "ADB clients on this computer connect here."
            ) {
                endpointFields(host: mirror.bindAddress, port: mirror.localADBPort)
            }
            endpointGroup(
                title: "Remote ADB through target",
                explanation: "ADB server reached after connecting to the selected SSH target."
            ) {
                endpointFields(host: mirror.remoteADBHost, port: mirror.remoteADBPort)
            }
            LabeledContent("scrcpy arguments") {
                TextField(
                    "One argument per line",
                    text: Binding(
                        get: { mirror.wrappedValue.scrcpyArguments.joined(separator: "\n") },
                        set: {
                            mirror.wrappedValue.scrcpyArguments = $0
                                .split(whereSeparator: \.isNewline)
                                .map(String.init)
                        }
                    ),
                    axis: .vertical
                )
                .lineLimit(3...6)
                .textFieldStyle(.roundedBorder)
            }
            DisclosureGroup("Advanced", isExpanded: $showingAdvanced) {
                Toggle(
                    "Allow non-loopback local ADB exposure",
                    isOn: mirror.allowNonLoopback
                )
            }
            HStack {
                Button("Open Mirror") { openMirror(mirror.wrappedValue.id) }
                    .disabled(
                        !backendActionsEnabled
                            || !status(mirror.wrappedValue.id).canOpenMirror
                    )
                runtimeActions(mirror.wrappedValue.id)
            }
        }
    }

    private func endpointGroup<Content: View>(
        title: String,
        explanation: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 10) {
                Text(explanation).font(.caption).foregroundStyle(.secondary)
                content()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(4)
        } label: {
            Text(title).font(.headline)
        }
    }

    private func endpointFields(
        host: Binding<String>,
        port: Binding<UInt16>
    ) -> some View {
        HStack {
            TextField("Host", text: host).textFieldStyle(.roundedBorder)
            TextField("Port", value: port, format: polarStationPortFormat)
                .textFieldStyle(.roundedBorder)
                .frame(width: 100)
        }
    }

    private func resourceHeader(
        name: Binding<String>,
        enabled: Binding<Bool>,
        resourceID: String,
        remove: @escaping () -> Void
    ) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Toggle("Enabled", isOn: enabled)
                Spacer()
                ResourceStatusLabel(status: status(resourceID))
                Button(role: .destructive, action: remove) {
                    Image(systemName: "trash")
                }
                .buttonStyle(.borderless)
                .accessibilityLabel("Remove \(name.wrappedValue)")
            }
            TextField("Resource name", text: name)
                .font(.title2.bold())
                .textFieldStyle(.roundedBorder)
        }
    }

    private func targetPicker(_ target: Binding<String>) -> some View {
        LabeledContent("SSH execution target") {
            Picker("SSH execution target", selection: target) {
                ForEach(targetIDs, id: \.self) { Text($0).tag($0) }
            }
            .labelsHidden()
            .frame(maxWidth: .infinity)
        }
    }

    private func runtimeActions(_ resourceID: String) -> some View {
        HStack {
            Button("Restart") { restart(resourceID) }
                .disabled(!backendActionsEnabled || !status(resourceID).canRestart)
            Button("Log") { showLog(resourceID) }
                .disabled(
                    !backendActionsEnabled
                        || entry(resourceID)?.canOpenLog != true
                )
        }
    }

    private func emptyDetail(_ message: String) -> some View {
        VStack(spacing: 12) {
            Image(systemName: "externaldrive.badge.questionmark")
                .font(.system(size: 36))
                .foregroundStyle(.secondary)
            Text("No Resource Selected").font(.headline)
            Text(message).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, minHeight: 300)
    }

    private var footer: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                if let errorMessage {
                    Text(errorMessage).foregroundStyle(.red).textSelection(.enabled)
                } else if let conflictMessage {
                    Text(conflictMessage).foregroundStyle(.orange)
                } else {
                    Text(isDirty ? "Draft has changes" : "No unapplied changes")
                }
                Text("Revision \(draft.revision) · Runtime changes only after Apply")
                    .foregroundStyle(.secondary)
            }
            .font(.caption)
            .lineLimit(2)
            Spacer()
            Button("Reload") { requestReload() }
                .disabled(isLoading || !backendActionsEnabled)
            Button(projection?.applyLabel ?? "Apply") { apply(draft) }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
                .disabled(
                    isLoading || !backendActionsEnabled || projection == nil
                        || draft == projection?.catalog
                )
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(.bar)
    }

    private var resourceItems: [ResourceListItem] {
        switch selectedKind {
        case .ports:
            draft.portForwards.map {
                .init(
                    id: $0.id,
                    name: $0.name,
                    summary: "\($0.bindAddress):\($0.localPortStart)–\($0.localPortEnd) → \($0.remoteHost):\($0.remotePortStart)"
                )
            }
        case .files:
            draft.fileMappings.map {
                .init(id: $0.id, name: $0.name, summary: "\($0.remotePath) → \($0.localPath)")
            }
        case .android:
            draft.androidMirrors.map {
                .init(
                    id: $0.id,
                    name: $0.name,
                    summary: "\($0.bindAddress):\($0.localADBPort) → \($0.remoteADBHost):\($0.remoteADBPort)"
                )
            }
        }
    }

    private var targetIDs: [String] { projection?.availableTargetIDs ?? [] }
    private var isDirty: Bool {
        guard let loadedCatalog else { return false }
        return draft != loadedCatalog
    }

    private func entry(_ resourceID: String) -> ResourceEntryProjectionV1? {
        projection?.entries.first { $0.id == resourceID }
    }

    private func status(_ resourceID: String) -> ResourceRuntimeStatusV1 {
        entry(resourceID)?.status ?? .init(
            state: .stopped,
            generation: 0,
            message: "Pending Apply",
            canRestart: false,
            canOpenMirror: false
        )
    }

    private func ensureSelection() {
        if let selectedResourceID,
           resourceItems.contains(where: { $0.id == selectedResourceID }) {
            return
        }
        selectedResourceID = resourceItems.first?.id
    }

    private func addSelectedKind() {
        guard let target = targetIDs.first else { return }
        switch selectedKind {
        case .ports:
            let value = PortForwardSpecV1(
                name: "New Forward", enabled: false,
                executionTargetID: target, localPortStart: 8080,
                localPortEnd: 8080, remotePortStart: 8080
            )
            draft.portForwards.append(value)
            selectedResourceID = value.id
        case .files:
            let value = FileMappingSpecV1(
                name: "File Mapping", enabled: false,
                executionTargetID: target, remotePath: "/home/operator",
                localPath: "~/Polar/remote"
            )
            draft.fileMappings.append(value)
            selectedResourceID = value.id
        case .android:
            let value = AndroidMirrorSpecV1(
                name: "Android Mirror", enabled: false,
                executionTargetID: target
            )
            draft.androidMirrors.append(value)
            selectedResourceID = value.id
        }
    }

    private func removeResource(_ resourceID: String) {
        switch selectedKind {
        case .ports: draft.portForwards.removeAll { $0.id == resourceID }
        case .files: draft.fileMappings.removeAll { $0.id == resourceID }
        case .android: draft.androidMirrors.removeAll { $0.id == resourceID }
        }
        selectedResourceID = nil
        ensureSelection()
    }

    private func remoteRangeIsValid(_ forward: PortForwardSpecV1) -> Bool {
        guard forward.localPortEnd >= forward.localPortStart else { return false }
        let width = UInt32(forward.localPortEnd - forward.localPortStart)
        return UInt32(forward.remotePortStart) + width <= UInt32(UInt16.max)
    }

    private func remoteRange(_ forward: PortForwardSpecV1) -> String {
        guard remoteRangeIsValid(forward) else { return "Invalid range" }
        let end = UInt32(forward.remotePortStart)
            + UInt32(forward.localPortEnd - forward.localPortStart)
        return forward.localPortStart == forward.localPortEnd
            ? "\(forward.remotePortStart)"
            : "\(forward.remotePortStart)–\(end)"
    }

    private func synchronizeDraft(_ projection: ResourcesControlProjectionV1?) {
        guard let projection else { return }
        guard let loadedCatalog else {
            draft = projection.catalog
            self.loadedCatalog = projection.catalog
            loadedRevision = projection.revision
            return
        }
        guard loadedRevision != projection.revision else { return }
        if draft.sameConfiguration(as: projection.catalog) {
            draft = projection.catalog
            self.loadedCatalog = projection.catalog
            loadedRevision = projection.revision
            conflictMessage = nil
        } else if draft != loadedCatalog {
            conflictMessage = "The Host catalog changed. This draft is preserved; Reload discards it."
        } else {
            draft = projection.catalog
            self.loadedCatalog = projection.catalog
            loadedRevision = projection.revision
            conflictMessage = nil
        }
    }

    private func revealAppliedLocalPath(_ resourceID: String) {
        guard let path = projection?.catalog.fileMappings.first(where: {
            $0.id == resourceID
        })?.localPath else { return }
        NSWorkspace.shared.activateFileViewerSelecting([
            URL(fileURLWithPath: (path as NSString).expandingTildeInPath, isDirectory: true)
        ])
    }

    private func showLog(_ resourceID: String) {
        selectedLogResourceID = resourceID
        showingLog = true
        loadLog(resourceID)
    }

    private func requestReload() {
        if isDirty { showingDiscardConfirmation = true } else { load() }
    }

    private var resourceLogSheet: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Resource Log").font(.title2.bold())
                Spacer()
                Button("Refresh") {
                    if let selectedLogResourceID { loadLog(selectedLogResourceID) }
                }
                Button("Done") { showingLog = false }
            }
            Divider()
            if let tail = logTail, tail.resourceID == selectedLogResourceID {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 6) {
                        ForEach(Array(tail.events.enumerated()), id: \.offset) { _, event in
                            Text("\(event.level.rawValue.uppercased())  \(event.message)")
                                .font(.system(.caption, design: .monospaced))
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                }
            } else {
                ProgressView("Loading Resource log…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .padding(20)
        .frame(minWidth: 620, minHeight: 360)
    }
}

private extension ResourceCatalogV1 {
    func sameConfiguration(as other: ResourceCatalogV1) -> Bool {
        schemaVersion == other.schemaVersion
            && fileMappings == other.fileMappings
            && portForwards == other.portForwards
            && androidMirrors == other.androidMirrors
    }
}

private struct ResourceStatusLabel: View {
    let status: ResourceRuntimeStatusV1

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(status.color).frame(width: 8, height: 8)
            Text(status.message).lineLimit(1)
            if status.generation > 0 { Text("#\(status.generation)") }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .help(status.message)
    }
}

private extension ResourceRuntimeStatusV1 {
    var color: Color {
        switch state {
        case .disabled, .stopped, .stopping: .secondary
        case .waitingForSSH: .yellow
        case .starting, .recovering: .orange
        case .running: .green
        case .needsAttention: .red
        }
    }
}
