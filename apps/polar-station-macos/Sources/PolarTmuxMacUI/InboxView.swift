import PolarTmuxHostClient
import SwiftUI

public struct InboxAttentionMessage: Identifiable, Equatable, Sendable {
    public let id: String
    public let tabID: String
    public let paneID: String
    public let sessionLabel: String
    public let attention: SessionAttentionStateV1
    public let summarySections: [String]
    public let revision: UInt64
    public let unread: Bool
    public let hostMessage: InboxMessageProjectionV1?

    public static func current(
        from projection: WorkspaceMapProjectionV1
    ) -> [Self] {
        current(from: projection.tabs)
    }

    public static func current(
        from tabs: [WorkspaceTabProjectionV1]
    ) -> [Self] {
        tabs.flatMap { tab in
            tab.panes.compactMap { pane in
                let attention = pane.attention
                guard attention.state != .idle
                        || !attention.summarySections.isEmpty
                else { return nil }
                return Self(
                    id: "live:\(tab.id):\(pane.id)",
                    tabID: tab.id,
                    paneID: pane.id,
                    sessionLabel: pane.sessionName,
                    attention: attention.state,
                    summarySections: attention.summarySections,
                    revision: attention.summaryRevision,
                    unread: attention.unread,
                    hostMessage: nil
                )
            }
        }
    }

    public static func persisted(_ messages: [InboxMessageProjectionV1]) -> [Self] {
        messages.map { message in
            Self(
                id: message.id.rawValue,
                tabID: message.origin.tabID,
                paneID: message.origin.paneID,
                sessionLabel: message.sessionLabel,
                attention: attentionState(message.attention),
                summarySections: [message.summary],
                revision: message.revision,
                unread: message.unread,
                hostMessage: message
            )
        }
    }

    private static func attentionState(_ value: String) -> SessionAttentionStateV1 {
        switch value {
        case "unknown": .unknown
        case "needsattention", "needs_attention": .needsAttention
        case "approvalrequired", "approval_required": .approvalRequired
        case "decisionrequired", "decision_required": .decisionRequired
        case "permissionrequired", "permission_required": .permissionRequired
        case "reviewready", "review_ready": .reviewReady
        case "failed": .failed
        case "working": .working
        default: .unknown
        }
    }
}

public struct InboxView: View {
    public let overview: InboxOverviewV1?
    public let openTerminalSession: @MainActor (PolarTmuxSessionKey) -> Void
    @State private var sourceFilter = ""
    @State private var targetFilter = ""
    @State private var statusFilter = ""
    @State private var originalBinding: CodexBindingV1?
    @State private var openingID: String?
    @State private var openError: String?
    public let requestCodex: CodexRequestHandler?
    public let codexRevision: UInt64
    public let openCodexSession: @MainActor (PolarTmuxSessionKey) -> Void
    public let liveMessages: [InboxAttentionMessage]
    public let historyMessages: [InboxAttentionMessage]
    public let backendAvailability: PolarStationBackendAvailability
    public let focusedMessageID: String?
    public let openPane: @MainActor (InboxAttentionMessage) -> Void
    public let submitReply: @MainActor (InboxAttentionMessage, String) -> Void
    public let editDraft: @MainActor (InboxMessageProjectionV1, String) -> Void
    public let approveDraft: @MainActor (InboxMessageProjectionV1) -> Void

    @State private var replyingTo: String?
    @State private var replyDraft = ""
    @State private var editingDraftFor: String?
    @State private var editedDraft = ""

    public init(
        overview: InboxOverviewV1? = nil,
        openTerminalSession: @escaping @MainActor (PolarTmuxSessionKey) -> Void = { _ in },
        requestCodex: CodexRequestHandler? = nil,
        codexRevision: UInt64 = 0,
        openCodexSession: @escaping @MainActor (PolarTmuxSessionKey) -> Void = { _ in },
        liveMessages: [InboxAttentionMessage],
        historyMessages: [InboxAttentionMessage],
        backendAvailability: PolarStationBackendAvailability,
        focusedMessageID: String? = nil,
        openPane: @escaping @MainActor (InboxAttentionMessage) -> Void,
        submitReply: @escaping @MainActor (InboxAttentionMessage, String) -> Void,
        editDraft: @escaping @MainActor (InboxMessageProjectionV1, String) -> Void,
        approveDraft: @escaping @MainActor (InboxMessageProjectionV1) -> Void
    ) {
        self.overview = overview
        self.openTerminalSession = openTerminalSession
        self.requestCodex = requestCodex
        self.codexRevision = codexRevision
        self.openCodexSession = openCodexSession
        self.liveMessages = liveMessages
        self.historyMessages = historyMessages
        self.backendAvailability = backendAvailability
        self.focusedMessageID = focusedMessageID
        self.openPane = openPane
        self.submitReply = submitReply
        self.editDraft = editDraft
        self.approveDraft = approveDraft
    }

    public var body: some View {
        VStack(spacing: 0) {
            HStack {
                Label("Inbox", systemImage: "tray.full")
                    .font(.headline)
                Spacer()
                Text("\(historyMessages.filter(\.unread).count) unread")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            .padding(12)
            Divider()

            if let overview {
                overviewBody(overview)
            } else if liveMessages.isEmpty && historyMessages.isEmpty && requestCodex == nil {
                VStack(spacing: 9) {
                    Image(systemName: "checkmark.circle")
                        .font(.system(size: 28))
                        .foregroundStyle(.secondary)
                    Text("No monitor messages").font(.headline)
                    Text("Working, review, approval, decision, permission, and failure updates will appear here.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }
                .padding(24)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 10) {
                            if let requestCodex {
                                CodexInboxView(request: requestCodex, revision: codexRevision, openSession: openCodexSession)
                            }
                            if !liveMessages.isEmpty {
                                sectionHeader("Live Pane Activity")
                                ForEach(liveMessages) { message in
                                    messageCard(message)
                                        .id(message.id)
                                }
                            }
                            if !historyMessages.isEmpty {
                                sectionHeader("History")
                                ForEach(historyMessages) { message in
                                    messageCard(message)
                                        .id(message.id)
                                }
                            }
                        }
                        .padding(10)
                    }
                    .onAppear {
                        if let focusedMessageID {
                            proxy.scrollTo(focusedMessageID, anchor: .center)
                        }
                    }
                    .onChange(of: focusedMessageID) { _ in
                        if let focusedMessageID {
                            withAnimation {
                                proxy.scrollTo(focusedMessageID, anchor: .center)
                            }
                        }
                    }
                }
            }
        }
        .sheet(isPresented: Binding(get: { originalBinding != nil }, set: { if !$0 { originalBinding = nil } })) {
            if let originalBinding, let requestCodex {
                CodexSessionControlView(session: originalBinding.session, stateRevision: codexRevision,
                    request: requestCodex, dismiss: { self.originalBinding = nil }, binding: originalBinding)
            }
        }
        .accessibilityIdentifier("polar-station-inbox")
    }

    private func visible(_ rows: [InboxOverviewRowV1]) -> [InboxOverviewRowV1] {
        rows.filter { row in
            (sourceFilter.isEmpty || row.source == sourceFilter)
                && (targetFilter.isEmpty || row.executionTargetID == targetFilter)
                && (statusFilter.isEmpty || (statusFilter == "disconnected" ? row.stale : row.taskStatus == statusFilter))
        }
    }
    private func overviewBody(_ overview: InboxOverviewV1) -> some View {
        let allRows = overview.activity + overview.records
        let targets = Array(Set(allRows.map(\.executionTargetID))).sorted()
        let statuses = Dictionary(allRows.map { ($0.taskStatus, $0.taskLabel) }, uniquingKeysWith: { first, _ in first })
        return VStack(spacing: 8) {
            HStack {
                Picker("Source", selection: $sourceFilter) {
                    Text("All sources").tag(""); Text("Codex").tag("codex"); Text("Terminal").tag("terminal")
                }
                Picker("Machine", selection: $targetFilter) {
                    Text("All machines").tag("")
                    ForEach(targets, id: \.self) { Text($0).tag($0) }
                }
            }
            Picker("Status", selection: $statusFilter) {
                Text("All states").tag(""); Text("Observation disconnected").tag("disconnected")
                ForEach(statuses.keys.sorted(), id: \.self) { Text(statuses[$0] ?? $0).tag($0) }
            }
            if let openError { Text(openError).foregroundStyle(.red).font(.caption) }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    ForEach(overview.errors, id: \.self) { Text($0).foregroundStyle(.red).font(.caption) }
                    sectionHeader("Current activity / needs attention")
                    if visible(overview.activity).isEmpty { Text("No matching current activity").font(.caption) }
                    ForEach(visible(overview.activity)) { overviewCard($0) }
                    sectionHeader("Recent records")
                    if visible(overview.records).isEmpty { Text("No matching records").font(.caption) }
                    ForEach(visible(overview.records)) { row in
                        if let id = row.terminalMessageID, let message = historyMessages.first(where: { $0.hostMessage?.id == id }) {
                            Text("Terminal · \(row.executionTargetID)").font(.caption).foregroundStyle(.secondary)
                            messageCard(message)
                        } else { overviewCard(row) }
                    }
                }.padding(10)
            }
        }.padding(.horizontal, 8)
    }
    private func overviewCard(_ row: InboxOverviewRowV1) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(row.title).font(.subheadline.bold())
            Text("\(row.source) · \(row.executionTargetID)").font(.caption).foregroundStyle(.secondary)
            Text(row.taskLabel).font(.callout)
            Text(row.observationLabel).font(.caption).foregroundStyle(row.stale ? .orange : .secondary)
            if row.createdAtMilliseconds > 0 {
                Text(Date(timeIntervalSince1970: Double(row.createdAtMilliseconds) / 1000), style: .time).font(.caption)
            }
            if !row.summary.isEmpty { Text(row.summary).font(.callout).textSelection(.enabled) }
            ForEach(Array(row.deliveries.enumerated()), id: \.offset) { _, delivery in
                Text("Notification: \(delivery.label)").font(.caption)
                if let code = delivery.error_code { Text(code).font(.caption).textSelection(.enabled) }
            }
            if row.codexBindingID != nil {
                Button(openingID == row.id ? "Opening…" : "Open original Thread") { Task { await openOriginal(row) } }
                    .disabled(requestCodex == nil || openingID != nil)
            } else if let session = row.session {
                Button("Open Session") { openTerminalSession(session) }
            }
        }.padding(10).frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
    }
    @MainActor private func openOriginal(_ row: InboxOverviewRowV1) async {
        guard let requestCodex, let owner = row.codexOwnerID, let bindingID = row.codexBindingID else { return }
        openingID = row.id; openError = nil
        defer { openingID = nil }
        do {
            let response = try await requestCodex(owner, .getBinding(id: bindingID))
            guard case .binding(let state?) = response.result else { openError = "Original Thread is unavailable. Refresh or check its connection."; return }
            guard state.binding.owner_host_id == owner, state.binding.binding_id == bindingID else { openError = "Original Thread identity changed."; return }
            originalBinding = state.binding
        } catch { openError = error.localizedDescription }
    }

    private func sectionHeader(_ title: String) -> some View {
        Text(title)
            .font(.caption.bold())
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 4)
    }

    private func messageCard(_ message: InboxAttentionMessage) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack(spacing: 7) {
                Circle().fill(color(message.attention)).frame(width: 8, height: 8)
                Text(message.sessionLabel).font(.subheadline.bold()).lineLimit(1)
                Spacer()
                if message.unread { Text("NEW").font(.caption2.bold()).foregroundStyle(.orange) }
            }
            ForEach(Array(message.summarySections.enumerated()), id: \.offset) { _, section in
                Text(section).font(.callout).frame(maxWidth: .infinity, alignment: .leading)
            }
            if let host = message.hostMessage {
                if let invocation = host.aiInvocation {
                    Text(invocation.error.map { "AI failed in \(invocation.durationMilliseconds) ms: \($0)" }
                        ?? "AI · \(invocation.model ?? "Auto") · \(invocation.durationMilliseconds) ms")
                        .font(.caption)
                        .foregroundStyle(invocation.error == nil ? Color.secondary : Color.red)
                }
                if let draft = host.draft {
                    Text("Draft").font(.caption.bold()).foregroundStyle(.secondary)
                    if editingDraftFor == message.id {
                        TextEditor(text: $editedDraft)
                            .font(.body.monospaced())
                            .frame(minHeight: 70)
                        HStack {
                            Button("Cancel Edit") {
                                editingDraftFor = nil
                                editedDraft = ""
                            }
                            Spacer()
                            Button("Save Draft") {
                                let value = editedDraft.trimmingCharacters(in: .whitespacesAndNewlines)
                                guard !value.isEmpty else { return }
                                editDraft(host, value)
                                editingDraftFor = nil
                            }
                            .disabled(editedDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        }
                    } else {
                        Text(draft).textSelection(.enabled)
                        Button("Edit Draft") {
                            editingDraftFor = message.id
                            editedDraft = draft
                        }
                    }
                    if host.draftStatus == .ready {
                        Button(host.staleContext ? "Restore & Insert/Send" : "Approve Insert/Send") {
                            approveDraft(host)
                        }
                    }
                }
            }
            if replyingTo == message.id {
                TextField("Reply to this message", text: $replyDraft, axis: .vertical)
                    .lineLimit(2...5)
                HStack {
                    Button("Cancel") { replyingTo = nil; replyDraft = "" }
                    Spacer()
                    Button("Generate Draft") {
                        let value = replyDraft.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !value.isEmpty else { return }
                        submitReply(message, value)
                    }
                    .disabled(replyDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !backendAvailability.hostActionsEnabled)
                }
            } else {
                HStack {
                    Button("Open Pane") { openPane(message) }
                    Spacer()
                    if message.hostMessage != nil {
                        Button("Reply") { replyingTo = message.id; replyDraft = "" }
                    }
                }
            }
        }
        .padding(11)
        .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 11))
        .overlay {
            RoundedRectangle(cornerRadius: 11)
                .stroke(message.id == focusedMessageID ? StationTheme.shared.accent : .clear, lineWidth: 2)
        }
    }

    private func color(_ state: SessionAttentionStateV1) -> Color {
        if state.isRed { return .red }
        if state.isOrange { return .orange }
        return state == .working ? .blue : .secondary
    }
}
