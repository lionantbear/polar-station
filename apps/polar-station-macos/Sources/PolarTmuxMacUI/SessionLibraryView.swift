import AppKit
import PolarTmuxHostClient
import SwiftUI

public typealias SessionEditorSubmit = @MainActor (ControlCenterIntentV1, @escaping @MainActor (String?) -> Void) -> Void
public typealias SessionCreationSubmit = @MainActor (ControlCenterIntentV1, @escaping @MainActor (String?, [String]) -> Void) -> Void
public typealias SessionEditorPrepare = @MainActor () -> SessionCreationSubmit

public struct SessionLibraryView: View {
    public let endpointStates: [String: String]
    public let connections: ConnectionsProjectionV1?
    @State private var selecting = false
    @State private var selectedSessions: [SessionRowProjectionV1.Identity] = []
    @State private var batchTabID: String?
    @State private var sessionSearch = ""
    @State private var sessionFilter = "all"
    @State private var collapsedGroups: Set<String> = []
    public let prepareEditor: SessionEditorPrepare?
    public let creationRecovery: AgentSessionResumeV1?
    public let codexEndpoints: [CodexEndpointSettingsV1]
    public let agentSessions: [AgentSessionCatalogRowV1]
    public let groups: [SessionTargetGroupV1]
    public let projection: SessionLibraryProjectionV1
    public let nativeLayoutView: NativeWorkspaceLayoutView?
    public let controlRevision: UInt64
    public let dismiss: @MainActor () -> Void
    public let submitOperation: @MainActor (ControlCenterIntentV1, @escaping @MainActor (String?) -> Void) -> Void
    public let emitIntent: @MainActor (ControlCenterIntentV1) -> Void
    public let readSessionEditor: SessionEditorReader?
    public let openCodex: (@MainActor (SessionRowProjectionV1.Identity) -> Void)?
    public let requestCodex: CodexRequestHandler?
    public let codexRevision: UInt64

    @ObservedObject private var applicationShortcuts: ApplicationShortcutCommandState
    private var editor: SessionEditorRequest? {
        get { applicationShortcuts.sessionEditor }
        nonmutating set { applicationShortcuts.sessionEditor = newValue }
    }
    @State private var pendingKill: SessionEndDraft?
    @State private var pendingReplacement: SessionRowProjectionV1?
    @State private var codexAgents: [CodexStateV1] = []
    @State private var selectedCodex: CodexBindingV1?
    @State private var codexError: String?
    @State private var expandedCatalogGroups: Set<String> = []
    @State private var renameAgent: AgentSessionCatalogRowV1?
    @State private var detailAgent: AgentSessionCatalogRowV1?
    @State private var expandedAgents: Set<String> = []

    public init(
        projection: SessionLibraryProjectionV1,
        connections: ConnectionsProjectionV1? = nil,
        endpointStates: [String: String] = [:],
        groups: [SessionTargetGroupV1] = [],
        agentSessions: [AgentSessionCatalogRowV1] = [],
        codexEndpoints: [CodexEndpointSettingsV1] = [],
        nativeLayoutView: NativeWorkspaceLayoutView? = nil,
        controlRevision: UInt64,
        dismiss: @escaping @MainActor () -> Void,
        emitIntent: @escaping @MainActor (ControlCenterIntentV1) -> Void,
        submitOperation: @escaping @MainActor (ControlCenterIntentV1, @escaping @MainActor (String?) -> Void) -> Void,
        readSessionEditor: SessionEditorReader? = nil,
        openCodex: (@MainActor (SessionRowProjectionV1.Identity) -> Void)? = nil,
        requestCodex: CodexRequestHandler? = nil,
        codexRevision: UInt64 = 0,
        applicationShortcuts: ApplicationShortcutCommandState = .init(),
        prepareEditor: SessionEditorPrepare? = nil,
        creationRecovery: AgentSessionResumeV1? = nil
    ) {
        self.endpointStates = endpointStates
        self.connections = connections
        self.prepareEditor = prepareEditor
        self.creationRecovery = creationRecovery
        self.applicationShortcuts = applicationShortcuts
        self.codexEndpoints = codexEndpoints
        self.agentSessions = agentSessions
        self.groups = groups
        self.projection = projection
        self.nativeLayoutView = nativeLayoutView
        self.controlRevision = controlRevision
        self.dismiss = dismiss
        self.emitIntent = emitIntent
        self.submitOperation = submitOperation
        self.readSessionEditor = readSessionEditor
        self.openCodex = openCodex
        self.requestCodex = requestCodex
        self.codexRevision = codexRevision
    }

    public var body: some View {
        let catalog = AgentCatalogIndex(agents: agentSessions, groups: groups, layout: nativeLayoutView, query: sessionSearch, filter: sessionFilter)
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Text(projection.title)
                    .font(.headline)

                Spacer(minLength: 4)

                Button {
                    selecting.toggle(); selectedSessions = []; batchTabID = nativeLayoutView?.selectedTabID
                } label: { Image(systemName: selecting ? "checkmark.circle.fill" : "checkmark.circle") }
                .accessibilityLabel(selecting ? "Done" : "Select")
                .help(selecting ? "Done" : "Select sessions")
                .controlSize(.small)
                if selecting {
                    Button {
                        guard let tabID = batchTabID else { return }
                        submitOperation(.init(controlRevision: controlRevision, operation: .addSessionAssignments(tabID: tabID, sessions: selectedSessions))) { error in
                            codexError = error
                            if error == nil { selectedSessions = []; selecting = false }
                        }
                    } label: { Image(systemName: "arrow.right") }
                    .accessibilityLabel("Add \(selectedSessions.count)")
                    .help("Add \(selectedSessions.count) to Tab")
                    .disabled(selectedSessions.isEmpty)
                }
                Button { editor = .createShell } label: { Image(systemName: "plus") }
                    .buttonStyle(.bordered)
                    .accessibilityLabel("New Session")
                    .help("New Session")
                ProjectedActionButton(
                    action: projection.refreshAction,
                    systemImage: "arrow.clockwise",
                    showsLabel: false,
                    emitIntent: invoke
                )

                Button(action: dismiss) {
                    Image(systemName: "xmark")
                }
                .buttonStyle(.plain)
                .accessibilityLabel(projection.closeAction.accessibilityLabel)
                .help(projection.closeAction.accessibilityLabel)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)

            Text(projection.interactionHint)
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
                .padding(.horizontal, 10)
                .padding(.bottom, 8)

            TextField("Search sessions", text: $sessionSearch).padding(.horizontal, 10)
            Picker("Type", selection: $sessionFilter) {
                Text("All types").tag("all"); Text("Codex").tag("codex")
                Text("Claude").tag("claude"); Text("Terminal").tag("terminal")
            }.padding(10)
            Divider()

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 8) {
                    ForEach(groups) { group in
                        let hostKey = "host:" + group.id
                        let hostExpanded = !sessionSearch.isEmpty || !expandedCatalogGroups.contains(hostKey)
                        Button {
                            if !expandedCatalogGroups.insert(hostKey).inserted { expandedCatalogGroups.remove(hostKey) }
                        } label: {
                            HStack { Image(systemName: hostExpanded ? "chevron.down" : "chevron.right"); Text(group.title); Spacer() }.font(.caption.bold())
                        }.buttonStyle(.plain)
                        if hostExpanded {
                            ForEach(catalog.groups.filter { key in catalog.roots[key]?.first?.execution_target_id == group.id }, id: \.self) { key in
                                let roots = catalog.roots[key] ?? []
                                let expanded = !sessionSearch.isEmpty || (expandedCatalogGroups.contains(key) != roots.contains { catalog.ranks[$0.id, default: Int.max] != Int.max })
                                Button {
                                    if !expandedCatalogGroups.insert(key).inserted { expandedCatalogGroups.remove(key) }
                                } label: {
                                    HStack { Image(systemName: expanded ? "chevron.down" : "chevron.right"); Text(catalogGroupTitle(key)).lineLimit(1); Spacer(); Text("\(catalog.counts[key, default: 0])") }.font(.caption)
                                }.buttonStyle(.plain)
                                if expanded { ForEach(roots) { agent in agentTree(agent, catalog: catalog, visited: []) } }
                            }
                            ForEach(projection.rows.filter { row in
                                row.identity.executionTargetID == group.id && !catalog.terminals.contains(row.identity) && matches([row.displayName, row.detailLabel, row.targetLabel].compactMap { $0 }.joined(separator: " "), kind: row.sessionKind?.rawValue ?? "terminal")
                            }) { row in sessionRow(row) }
                            if let message = group.message, group.state != "empty" || !catalog.roots.values.contains(where: { $0.first?.execution_target_id == group.id }) { Text(message).font(.caption).foregroundStyle(.secondary) }
                        }
                    }
                    if let codexError { Text(codexError).font(.caption).foregroundStyle(.red) }
                }
                .padding(10)
            }
        }
        .frame(maxHeight: .infinity)
        .background(StationTheme.shared.background)
        .background {
            if SessionListPaintProbe.enabled { SessionListPaintProbe(revision: controlRevision) }
        }
        .sheet(isPresented: Binding(get: { selectedCodex != nil }, set: { if !$0 { selectedCodex = nil } })) {
            if let selectedCodex, let requestCodex {
                CodexSessionControlView(session: selectedCodex.session, stateRevision: codexRevision, request: requestCodex, dismiss: { self.selectedCodex = nil }, binding: selectedCodex)
            }
        }
        .sheet(item: $renameAgent) { agent in
            AgentRenameSheet(agent: agent, close: { renameAgent = nil }) { name, completion in
                submitOperation(.init(controlRevision: controlRevision, operation: .renameAgentSession(ownerHostID: agent.owner_host_id, endpointID: agent.endpoint_id, threadID: agent.thread_id, name: name)), completion)
            }
        }
        .sheet(item: $detailAgent) { agent in
            VStack(alignment: .leading, spacing: 12) {
                Text(agent.display_name).font(.headline)
                Text(agent.metadata?.preview ?? "No preview yet").textSelection(.enabled)
                Text([agent.metadata?.cwd, agent.metadata?.agent_role, agent.metadata?.agent_nickname, agent.status, agent.owner_host_id, agent.endpoint_id, agent.thread_id].compactMap { $0 }.joined(separator: "\n")).font(.callout).textSelection(.enabled)
                HStack { Spacer(); Button("Done") { detailAgent = nil }.keyboardShortcut(.defaultAction) }
            }.padding(24).frame(width: 520)
        }
        .confirmationDialog(
            pendingReplacement?.replacementAction?.confirmation?.title ?? "Use existing Session?",
            isPresented: Binding(get: { pendingReplacement != nil }, set: { if !$0 { pendingReplacement = nil } }),
            titleVisibility: .visible
        ) {
            if let row = pendingReplacement, let replacement = row.replacement,
               let confirmation = row.replacementAction?.confirmation, let view = nativeLayoutView {
                Button(confirmation.confirmLabel) {
                    let selection = SessionReplacementSelectionV1(identity: row.identity, replacement: replacement, workspaceID: view.workspaceID, confirmationID: confirmation.id)
                    pendingReplacement = nil
                    submitOperation(.init(controlRevision: controlRevision, operation: .useExistingSession(selection: selection))) { _ in }
                }
                Button("Cancel", role: .cancel) { pendingReplacement = nil }
            }
        } message: {
            if let message = pendingReplacement?.replacementAction?.confirmation?.message { Text(message) }
        }
        .background(SessionEndWindowHost(draft: pendingKill, close: { pendingKill = nil }).frame(width: 0, height: 0))
    }

    @ViewBuilder
    private func sessionSection(
        _ section: SessionSectionProjectionV1
    ) -> some View {
        if section.section == .remote {
            Divider()
                .padding(.vertical, 4)
        }

        sectionTitle(section.title)

        if section.section == .local {
            localCreationControls
        } else {
            remoteCreationControls
        }

        if let message = section.message {
            Text(message)
                .font(.caption)
                .foregroundStyle(
                    section.state == .error ? .red : .secondary
                )
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 6)
        }

        ForEach(rows(in: section.section)) { row in
            sessionRow(row)
        }
    }

    private var localCreationControls: some View {
        Menu {
            if let action = projection.localNewShell {
                Button {
                    editor = .createShell
                } label: {
                    Label(action.label, systemImage: "terminal")
                }
                .disabled(!action.enabled)
            }
            if let action = projection.localNewCommand {
                Button {
                    editor = .createCommand
                } label: {
                    Label(action.label, systemImage: "chevron.left.forwardslash.chevron.right")
                }
                .disabled(!action.enabled)
            }
        } label: {
            Label("New Local Session", systemImage: "plus")
                .labelStyle(.iconOnly)
        }
        .menuStyle(.borderlessButton)
        .controlSize(.small)
        .accessibilityLabel("New local session")
    }

    @ViewBuilder
    private var remoteCreationControls: some View {
        if let action = projection.remoteNewSession {
            Menu {
                Button {
                    editor = .createRemote
                } label: {
                    Label(action.label, systemImage: "terminal")
                }
                .disabled(!action.enabled || remoteTargets.isEmpty)
            } label: {
                Label("New Remote Session", systemImage: "plus")
                    .labelStyle(.iconOnly)
            }
            .menuStyle(.borderlessButton)
            .controlSize(.small)
            .disabled(!action.enabled || remoteTargets.isEmpty)
            .help(
                remoteTargets.isEmpty
                    ? "No managed remote target is currently projected."
                    : action.disabledReason ?? action.accessibilityLabel
            )
        }
    }

    private func sessionRow(
        _ row: SessionRowProjectionV1
    ) -> some View {
        let presentation = SessionAssignmentPresentation.classify(
            executionTargetID: row.identity.executionTargetID,
            tmuxSessionID: row.identity.tmuxSessionID,
            nativeView: nativeLayoutView
        )
        let assignmentAction = presentation.action(
            policy: row.assignmentActions,
            fallback: row.assignmentAction
        )
        return HStack(spacing: 6) {
            Image(systemName: row.sessionKind?.rawValue == "codex_plane" ? "bubble.left.and.text.bubble.right" : row.sessionKind?.rawValue == "claude_plane" ? "sparkles" : "terminal")

            VStack(alignment: .leading, spacing: 1) {
                Text(row.displayName)
                    .lineLimit(1)
                    .truncationMode(.tail)
                if let detailLabel = row.detailLabel {
                    Text(detailLabel)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

            }

            Spacer(minLength: 2)

            if selecting { Image(systemName: selectedSessions.contains(row.identity) ? "checkmark.square.fill" : "square") }
            if presentation.assignedToSelectedTab {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(.secondary)
                    .accessibilityLabel("Assigned to selected tab")
            }



        }
        .font(.system(size: 14))
        .padding(.horizontal, 10)
        .frame(maxWidth: .infinity, minHeight: 44)
        .background(
            row.selected
                ? StationTheme.shared.accent.opacity(0.16)
                : Color.secondary.opacity(0.08),
            in: RoundedRectangle(cornerRadius: 9)
        )
        .overlay {
            RoundedRectangle(cornerRadius: 9)
                .stroke(
                    row.selected
                        ? StationTheme.shared.accent.opacity(0.55)
                        : Color.clear,
                    lineWidth: 1
                )
        }
        .contentShape(Rectangle())
        .onTapGesture {
            if assignmentAction.enabled {
                toggleAssignment(row, presentation: presentation)
            }
        }
        .contextMenu { sessionActions(row) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(row.displayName)
        .accessibilityAddTraits(
            row.selected ? [.isButton, .isSelected] : [.isButton]
        )
        .accessibilityAction(
            named: Text(assignmentAction.accessibilityLabel)
        ) {
            if assignmentAction.enabled {
                toggleAssignment(row, presentation: presentation)
            }
        }
        .help(
            assignmentAction.disabledReason
                ?? assignmentAction.accessibilityLabel
        )
    }

    @ViewBuilder
    private func sessionActions(_ row: SessionRowProjectionV1, includeRename: Bool = true, includeEnd: Bool = true) -> some View {
        if let view = nativeLayoutView, let tab = view.tab(view.selectedTabID),
           let pane = tab.panes.first(where: { $0.executionTargetID == row.identity.executionTargetID && $0.tmuxSessionID == row.identity.tmuxSessionID }) {
            Button("Remove from current Tab") { emit(.toggleSessionAssignment(identity: row.identity, tabID: view.selectedTabID, paneID: pane.paneID)) }
        }
        if let agentID = row.originalAgentID {
            Button("Copy original Agent ID") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(agentID, forType: .string)
            }
        }
        if let openCodex {
            Button("Codex…") { openCodex(row.identity) }
        }
        if let action = row.replacementAction {
            Button(action.label) { pendingReplacement = row }
                .disabled(!action.enabled || replacementAlreadyAssigned(row))
                .help(replacementAlreadyAssigned(row) ? "Already assigned in this Workspace" : action.accessibilityLabel)
            if replacementAlreadyAssigned(row), let replacement = row.replacement,
               let pane = nativeLayoutView?.tabs.flatMap(\.panes).first(where: { $0.executionTargetID == replacement.executionTargetID && $0.tmuxSessionID == replacement.tmuxSessionID }) {
                Button("Show existing Session") { emit(.activatePane(paneID: pane.paneID)) }
            }
        }
        if let action = row.restartAction {
            Button(action.label) {
                emit(.invokeProjectedAction(actionID: action.id))
            }
            .disabled(!action.enabled)
        }
        if includeRename, let renameAction = row.renameAction {
            Button {
                editor = .rename(row)
            } label: {
                Label {
                    Text(renameAction.label)
                } icon: {
                    Image(systemName: "square.and.pencil")
                }
            }
            .disabled(!renameAction.enabled)
        }
        if includeEnd, let killAction = row.killAction {
            Button(role: .destructive) {
                pendingKill = SessionEndDraft(identity: row.identity, read: readSessionEditor) { operation, completion in
                    submitOperation(.init(controlRevision: controlRevision, operation: operation), completion)
                }
            } label: {
                Label(killAction.label, systemImage: "trash")
            }
            .disabled(!killAction.enabled)
        }
    }

    private func catalogGroupTitle(_ key: String) -> String {
        if key == "current" { return "Current Workspace" }
        guard let separator = key.range(of: " / ") else { return key }
        return URL(fileURLWithPath: String(key[separator.upperBound...])).lastPathComponent
    }

    private func workspaceRank(_ identities: [SessionRowProjectionV1.Identity]) -> Int {
        guard let view = nativeLayoutView else { return Int.max }
        let tabs = view.tabs.sorted { ($0.tabID == view.selectedTabID ? 0 : 1) < ($1.tabID == view.selectedTabID ? 0 : 1) }
        for (index, tab) in tabs.enumerated() {
            for (paneIndex, pane) in tab.panes.enumerated() where identities.contains(where: {
                $0.executionTargetID == pane.executionTargetID && $0.tmuxSessionID == pane.tmuxSessionID
            }) { return index * 10 + paneIndex }
        }
        return Int.max
    }

    private func agentTree(_ agent: AgentSessionCatalogRowV1, catalog: AgentCatalogIndex, visited: Set<String>) -> AnyView {
        guard !visited.contains(agent.id) else { return AnyView(EmptyView()) }
        let children = (catalog.children[agent.id] ?? []).filter { catalog.visible.contains($0.id) && !visited.contains($0.id) }
        let expanded = !sessionSearch.isEmpty || (expandedAgents.contains(agent.id) != children.contains { catalog.ranks[$0.id, default: Int.max] != Int.max })
        return AnyView(VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 4) {
                if !children.isEmpty {
                    Button {
                        if !expandedAgents.insert(agent.id).inserted { expandedAgents.remove(agent.id) }
                    } label: { Image(systemName: expanded ? "chevron.down" : "chevron.right") }.buttonStyle(.plain)
                }
                Button {
                    if let identity = agent.terminals?.min(by: { workspaceRank([$0]) < workspaceRank([$1]) }),
                       let row = projection.rows.first(where: { $0.identity == identity }) {
                        toggleAssignment(row, presentation: .classify(executionTargetID: identity.executionTargetID, tmuxSessionID: identity.tmuxSessionID, nativeView: nativeLayoutView))
                    } else if !selecting { openAgent(agent) }
                } label: {
                    VStack(alignment: .leading, spacing: 1) {
                        HStack {
                            if let view = nativeLayoutView, let tab = view.tab(view.selectedTabID), tab.panes.contains(where: { pane in (agent.terminals ?? []).contains { $0.executionTargetID == pane.executionTargetID && $0.tmuxSessionID == pane.tmuxSessionID } }) { Image(systemName: "checkmark") }
                            if selecting, let identity = agent.terminals?.first { Image(systemName: selectedSessions.contains(identity) ? "checkmark.square.fill" : "square") }
                            Text(agent.display_name).lineLimit(1); Spacer(minLength: 0); if !children.isEmpty { Text("\(children.count)").font(.caption) } }
                        HStack(spacing: 4) {
                            Text("\(agent.metadata?.cwd.map { URL(fileURLWithPath: $0).lastPathComponent } ?? "Unknown project") · \(agent.status)").lineLimit(1)
                            Spacer(minLength: 0)
                            if let updated = agent.metadata?.updated_at { Text(Date(timeIntervalSince1970: Double(updated)), style: .relative).lineLimit(1) }
                        }.font(.caption).foregroundStyle(.secondary)
                    }.frame(maxWidth: .infinity, minHeight: 44, alignment: .leading).contentShape(Rectangle())
                }.buttonStyle(.plain)
                .help([agent.display_name, agent.metadata?.cwd, agent.metadata?.preview, agent.metadata?.agent_role, agent.thread_id].compactMap { $0 }.joined(separator: "\n"))
                .contextMenu {
                    Button("Rename…") { renameAgent = agent }
                    Button("Details…") { detailAgent = agent }
                    Button("Copy ID") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(agent.thread_id, forType: .string) }
                    Button("End Session…", role: .destructive) {
                        pendingKill = SessionEndDraft(agent: agent, read: readSessionEditor) { operation, completion in
                            submitOperation(.init(controlRevision: controlRevision, operation: operation), completion)
                        }
                    }
                    if let identity = agent.terminals?.min(by: { workspaceRank([$0]) < workspaceRank([$1]) }), let row = projection.rows.first(where: { $0.identity == identity }) { sessionActions(row, includeRename: false, includeEnd: false) }
                }
            }
            if expanded {
                ForEach(children) { child in agentTree(child, catalog: catalog, visited: visited.union([agent.id])).padding(.leading, 10) }
            }
        })
    }

    private func pick(_ identity: SessionRowProjectionV1.Identity) {
        guard let tabID = batchTabID, let tab = nativeLayoutView?.tab(tabID) else { return }
        if tab.panes.contains(where: { $0.executionTargetID == identity.executionTargetID && $0.tmuxSessionID == identity.tmuxSessionID }) { return }
        if let index = selectedSessions.firstIndex(of: identity) { selectedSessions.remove(at: index) }
        else { selectedSessions.append(identity) }
    }

    private func toggleAssignment(
        _ row: SessionRowProjectionV1,
        presentation: SessionAssignmentPresentation
    ) {
        guard let view = nativeLayoutView, let tab = view.tab(view.selectedTabID) else { return }
        if selecting { pick(row.identity); return }
        if let pane = tab.panes.first(where: { $0.executionTargetID == row.identity.executionTargetID && $0.tmuxSessionID == row.identity.tmuxSessionID }) {
            emit(.toggleSessionAssignment(identity: row.identity, tabID: view.selectedTabID, paneID: pane.paneID))
        } else { emit(.toggleSessionAssignment(identity: row.identity, tabID: view.selectedTabID, paneID: nil)) }
    }

    private func matches(_ name: String, kind: String) -> Bool {
        (sessionFilter == "all" || sessionFilter == kind || kind == sessionFilter + "_plane") && (sessionSearch.isEmpty || name.localizedCaseInsensitiveContains(sessionSearch))
    }

    private func openAgent(_ agent: AgentSessionCatalogRowV1) {
        if agent.loaded {
            submitOperation(.init(controlRevision: controlRevision, operation: .createAgentSession(
                executionTargetID: agent.execution_target_id, provider: "codex", mode: "resume", name: agent.display_name,
                workingDirectory: nil, endpointID: agent.endpoint_id, sessionID: agent.thread_id, yolo: false, openLocalCLI: false))) { codexError = $0 }
        } else { editor = .init(kind: .createShell, initialType: "codex", resumeAgent: agent) }
    }

    private func invoke(_ action: ActionProjectionV1) {
        emit(action.intentOperation)
    }

    private func replacementAlreadyAssigned(_ row: SessionRowProjectionV1) -> Bool {
        guard let replacement = row.replacement, let panes = nativeLayoutView?.tabs.flatMap(\.panes) else { return false }
        return panes.contains { $0.executionTargetID == row.identity.executionTargetID && $0.tmuxSessionID == row.identity.tmuxSessionID }
            && panes.contains { $0.executionTargetID == replacement.executionTargetID && $0.tmuxSessionID == replacement.tmuxSessionID }
    }

    private func emit(
        _ operation: ControlCenterIntentOperationV1
    ) {
        emitIntent(ControlCenterIntentV1(
            controlRevision: controlRevision,
            operation: operation
        ))
    }

    private var currentTerminal: SessionRowProjectionV1.Identity? {
        guard let view = nativeLayoutView, let tab = view.tab(view.selectedTabID) else { return nil }
        let paneID = tab.displayMode == .focus ? tab.mainPaneID : view.selectedPaneID
        guard let pane = tab.panes.first(where: { $0.paneID == paneID }),
              let target = pane.executionTargetID, let session = pane.tmuxSessionID else { return nil }
        return .init(executionTargetID: target, tmuxSessionID: session)
    }

    private var remoteTargets: [SessionTargetChoice] {
        projection.remoteTargets.map { target in
            return SessionTargetChoice(
                id: target.id,
                label: target.label
            )
        }
    }

    private func rows(
        in section: SessionRowProjectionV1.Section
    ) -> [SessionRowProjectionV1] {
        projection.rows.filter { $0.section == section }
    }

    private func sectionTitle(_ title: String) -> some View {
        Text(title.uppercased())
            .font(.caption2.weight(.semibold))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 3)
            .accessibilityAddTraits(.isHeader)
    }

    @MainActor private func reloadCodexAgents() async {
        guard let requestCodex else { return }
        do {
            var states: [CodexStateV1] = []
            for owner in Set(codexEndpoints.map(\.owner_host_id)) where !owner.isEmpty {
                let response = try await requestCodex(owner, .getBindings)
                if case .bindings(let values) = response.result { states.append(contentsOf: values) }
                if case .error(let code) = response.result { codexError = code; return }
            }
            codexAgents = states
            codexError = nil
        } catch { codexError = error.localizedDescription }
    }

}


// An opt-in AppKit draw callback for the list revision, not a presentation claim.
private struct SessionListPaintProbe: NSViewRepresentable {
    static let enabled = ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil
    let revision: UInt64
    func makeNSView(context: Context) -> PaintView { PaintView() }
    func updateNSView(_ view: PaintView, context: Context) {
        guard view.revision != revision else { return }
        view.revision = revision
        view.pending = true
        view.needsDisplay = true
    }
    final class PaintView: NSView {
        var revision: UInt64 = .max
        var pending = false
        override func hitTest(_ point: NSPoint) -> NSView? { nil }
        override func draw(_ dirtyRect: NSRect) {
            guard pending else { return }
            pending = false
            StationOperationTrace.record(49, generation: revision)
        }
    }
}

/// Window-scoped shortcut presentation stays mounted independently of the panel.
public struct SessionShortcutEditorHost: View {
    @ObservedObject private var shortcuts: ApplicationShortcutCommandState
    private let projection: ControlCenterProjectionV1
    private let layout: NativeWorkspaceLayoutView?
    private let requestCodex: CodexRequestHandler
    private let read: SessionEditorReader
    private let prepare: SessionEditorPrepare
    private let submit: SessionEditorSubmit
    private let recovery: AgentSessionResumeV1?
    private var editor: SessionEditorRequest? {
        get { shortcuts.sessionEditor }
        nonmutating set { shortcuts.sessionEditor = newValue }
    }
    @State private var editorSubmit: SessionCreationSubmit?

    public init(shortcuts: ApplicationShortcutCommandState,
                projection: ControlCenterProjectionV1, layout: NativeWorkspaceLayoutView?,
                requestCodex: @escaping CodexRequestHandler, read: @escaping SessionEditorReader,
                prepare: @escaping SessionEditorPrepare, submit: @escaping SessionEditorSubmit,
                recovery: AgentSessionResumeV1? = nil) {
        self.shortcuts = shortcuts; self.projection = projection; self.layout = layout
        self.requestCodex = requestCodex; self.read = read; self.prepare = prepare; self.submit = submit
        self.recovery = recovery
    }

    public var body: some View {
        Color.clear.frame(width: 0, height: 0)
            .onReceive(shortcuts.$request.compactMap { $0 }) { request in
                let type: String, mode: String
                switch request.command {
                case .newSession(let value): type = value; mode = "new"
                case .resumeCodex, .bindIdeaCodex: type = "codex"; mode = "resume"
                default: return
                }
                if let editor {
                    NSApp.windows.first { $0.identifier?.rawValue == "polar-station-session-editor-" + editor.id.uuidString }?.makeKeyAndOrderFront(nil)
                    return
                }
                var next = SessionEditorRequest(kind: .createShell, initialType: type, initialMode: mode)
                if case .bindIdeaCodex(let pane, let view) = request.command { next.bindPane=pane; next.bindView=view }
                editor = next
            }
            .background(SessionCreationWindowHost(request: editor, close: { editor = nil }) { request in
                SessionEditorSheet(
                    request: request,
                    destination: request.bindPane != nil ? "Binds to the originating Idea Pane; no new Pane" : layout.map { "Opens in \($0.workspaceName) / \($0.tab($0.selectedTabID)?.title ?? "Tab")" } ?? "Opens in the current Tab",
                    remoteTargets: projection.sessions.remoteTargets.map { .init(id: $0.id, label: $0.label) },
                    localTargetID: projection.sessions.localExecutionTargetID,
                    endpoints: projection.codexEndpoints, sourceSession: currentTerminal,
                    requestCodex: requestCodex, readSessionEditor: read, recovery: recovery,
                    connections: projection.connections, endpointStates: projection.codexEndpointStates,
                    controlRevision: projection.controlRevision, submitConnections: submit,
                    pick: { owner, endpoint, thread, title in
                        guard let pane=request.bindPane, let view=request.bindView else { return }
                        shortcuts.bindCodexSelection?(pane,view,owner,endpoint,thread,title);editor=nil
                    },
                    cancel: { editor = nil },
                    submit: { operation, completion in
                        let finish: @MainActor (String?, [String]) -> Void = { error, completed in
                            completion(error, completed)
                            if error == nil { editor = nil }
                        }
                        let intent = ControlCenterIntentV1(controlRevision: projection.controlRevision, operation: operation)
                        if let editorSubmit { editorSubmit(intent, finish) }
                        else { submit(intent) { finish($0, []) } }
                    }).onAppear { if request.bindPane == nil { editorSubmit = prepare() } }
            })
    }

    private var currentTerminal: SessionRowProjectionV1.Identity? {
        guard let view = layout, let tab = view.tab(view.selectedTabID) else { return nil }
        let paneID = tab.displayMode == .focus ? tab.mainPaneID : view.selectedPaneID
        guard let pane = tab.panes.first(where: { $0.paneID == paneID }),
              let target = pane.executionTargetID, let session = pane.tmuxSessionID else { return nil }
        return .init(executionTargetID: target, tmuxSessionID: session)
    }
}
