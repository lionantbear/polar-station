import PolarTmuxHostClient
import SwiftUI
import AppKit

public struct PolarStationSettingsView: View {
    private enum Page: String, CaseIterable, Identifiable {
        case appearance = "Appearance"
        case intelligence = "Intelligence"
        case voice = "Voice"
        case notifications = "Notifications"
        case workspace = "Workspace"
        case connections = "Connections"
        case resources = "Resources"
        var id: String { rawValue }
    }

    public let voicePlatform: VoicePlatformRequest?
    public let projection: SettingsProjectionV1?
    public let isLoading: Bool
    public let errorMessage: String?
    public let providerTestResult: String?
    public let codexNotifyStatus: CodexNotifyConfigStateV1?
    public let currentWorkspaceFontSize: UInt8?
    public let reload: () -> Void
    public let updateAppearance: (AppearanceSettingsV1) -> Void
    public let updateAttention: (AttentionSettingsV1) -> Void
    public let updateMonitoring: (MonitoringSettingsV1) -> Void
    public let updateSessionSummary: (SessionSummarySettingsV1) -> Void
    public let testSessionSummary: (SessionSummarySettingsV1) -> Void
    public let inspectCodexNotify: () -> Void
    public let configureCodexNotify: () -> Void
    public let updateWorkspaceFont: (UInt8) -> Void
    public let openConnections: () -> Void
    public let openResources: () -> Void
    public let dismiss: @MainActor () -> Void

    public let messagesSettings: ImessageSenderSettingsV1?
    public let requestConnector: ConnectorRequestHandler?
    public let requestCodex: CodexRequestHandler?
    @State private var connectorsOpen = false
    public let messagesStatus: NotificationSenderStatusV1?
    public let messagesError: String?
    public let isMessagesLoading: Bool
    public let reloadMessages: () -> Void
    public let authorizeMessages: () -> Void
    public let saveMessages: (ImessageSenderSettingsV1, String) -> Void
    @State private var messages = ImessageSenderSettingsV1()
    @State private var messagesRecipient = ""
    @State private var allowedSources = ""

    @State private var selectedPage: Page = .appearance
    @State private var widthText = ""
    @State private var inboxWidthText = ""
    @State private var appearance = AppearanceSettingsV1()
    @State private var summary = SessionSummarySettingsV1()
    @State private var monitoring = MonitoringSettingsV1()
    @State private var workspaceFont: Double = 13
    @State private var interfaceTextScale: Double = 100

    public init(
        projection: SettingsProjectionV1?, isLoading: Bool, errorMessage: String?,
        providerTestResult: String?, codexNotifyStatus: CodexNotifyConfigStateV1?, currentWorkspaceFontSize: UInt8?, reload: @escaping () -> Void,
        updateAppearance: @escaping (AppearanceSettingsV1) -> Void,
        updateAttention: @escaping (AttentionSettingsV1) -> Void,
        updateMonitoring: @escaping (MonitoringSettingsV1) -> Void,
        updateSessionSummary: @escaping (SessionSummarySettingsV1) -> Void,
        testSessionSummary: @escaping (SessionSummarySettingsV1) -> Void,
        inspectCodexNotify: @escaping () -> Void,
        configureCodexNotify: @escaping () -> Void,
        updateWorkspaceFont: @escaping (UInt8) -> Void,
        openConnections: @escaping () -> Void, openResources: @escaping () -> Void,
        dismiss: @escaping @MainActor () -> Void,
        messagesSettings: ImessageSenderSettingsV1? = nil,
        messagesStatus: NotificationSenderStatusV1? = nil,
        requestConnector: ConnectorRequestHandler? = nil,
        requestCodex: CodexRequestHandler? = nil,
        messagesError: String? = nil,
        isMessagesLoading: Bool = false,
        reloadMessages: @escaping () -> Void = {},
        authorizeMessages: @escaping () -> Void = {},
        saveMessages: @escaping (ImessageSenderSettingsV1, String) -> Void = { _, _ in },
        voicePlatform: VoicePlatformRequest? = nil,
        initialVoicePage: Bool = false
    ) {
        self._selectedPage = State(initialValue: initialVoicePage ? .voice : .appearance)
        self.voicePlatform = voicePlatform
        self.requestConnector = requestConnector; self.requestCodex = requestCodex
        self.projection = projection; self.isLoading = isLoading; self.errorMessage = errorMessage
        self.providerTestResult = providerTestResult; self.codexNotifyStatus = codexNotifyStatus; self.currentWorkspaceFontSize = currentWorkspaceFontSize
        self.reload = reload; self.updateAppearance = updateAppearance; self.updateAttention = updateAttention
        self.updateMonitoring = updateMonitoring
        self.updateSessionSummary = updateSessionSummary; self.testSessionSummary = testSessionSummary
        self.inspectCodexNotify = inspectCodexNotify; self.configureCodexNotify = configureCodexNotify
        self.updateWorkspaceFont = updateWorkspaceFont; self.openConnections = openConnections; self.openResources = openResources
        self.dismiss = dismiss
        self.messagesSettings = messagesSettings; self.messagesStatus = messagesStatus
        self.messagesError = messagesError; self.isMessagesLoading = isMessagesLoading
        self.reloadMessages = reloadMessages; self.authorizeMessages = authorizeMessages; self.saveMessages = saveMessages
    }

    public var body: some View {
        NavigationSplitView {
            List(Page.allCases, selection: $selectedPage) { page in Label(page.rawValue, systemImage: symbol(page)).tag(page) }
                .navigationSplitViewColumnWidth(min: 176, ideal: 176)
        } detail: {
            Form {
                if let errorMessage { Text(errorMessage).foregroundStyle(.red); Button("Retry", action: reload) }
                if projection == nil && isLoading { ProgressView("Loading Settings…") }
                pageBody
            }
            .formStyle(.grouped)
            .navigationTitle(selectedPage.rawValue)
        }
        .frame(minWidth: 720, minHeight: 500)
        .onReceive(NotificationCenter.default.publisher(for: Notification.Name("PolarStationOpenVoiceSettings"))) { _ in selectedPage = .voice }
        .task {
            applyProjection(); reload(); inspectCodexNotify()
            if let value = messagesSettings {
                messages = value
                allowedSources = value.allowedSourcePrincipals.joined(separator: "\n")
            }
        }
        .onChange(of: projection) { _ in applyProjection() }
        .onChange(of: messagesSettings) { value in
            if let value { messages = value; allowedSources = value.allowedSourcePrincipals.joined(separator: "\n") }
        }
        .onChange(of: selectedPage) { value in if value == .notifications { reloadMessages() } }
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Close", action: dismiss)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityLabel("Close Polar Station Settings")
            }
        }
        .stationTheme()
        .accessibilityIdentifier("polar-station-settings")
    }

    @ViewBuilder private var pageBody: some View {
        switch selectedPage {
        case .voice:
            VoiceSettingsView(platform: voicePlatform)
        case .appearance:
            Section("Theme") { StationThemePicker(projection: projection, save: updateAppearance) }
            Section("Monitor views") {
                Text("Layout and Inbox visibility are remembered separately for each Workspace. Use the panel buttons to change them.")
                    .foregroundStyle(.secondary)
                Picker("Approved terminal input", selection: $appearance.terminalDeliveryMode) {
                    Text("Insert only").tag(TerminalDeliveryModeV1.insert)
                    Text("Insert + Return").tag(TerminalDeliveryModeV1.send)
                }
            }
            Section("Layout width") {
                TextField("Adaptive, or 200–390", text: $widthText)
            }
            Section("Inbox width") {
                TextField("Adaptive, or 300–520", text: $inboxWidthText)
                HStack {
                    Button("Use Adaptive Widths") {
                        appearance.layoutMapWidth = nil
                        appearance.inboxWidth = nil
                        appearance.interfaceTextScalePercent = UInt16(interfaceTextScale)
                        updateAppearance(appearance)
                    }
                    Button("Save Appearance") {
                        appearance.layoutMapWidth = UInt16(widthText)
                        appearance.inboxWidth = UInt16(inboxWidthText)
                        appearance.interfaceTextScalePercent = UInt16(interfaceTextScale)
                        updateAppearance(appearance)
                    }
                    .disabled(
                        UInt16(widthText).map { !(200...390).contains($0) } ?? !widthText.isEmpty
                        || UInt16(inboxWidthText).map { !(300...520).contains($0) } ?? !inboxWidthText.isEmpty
                    )
                }
            }
            Section("Chat memory — in memory only") {
                TextField("Messages and index (MiB)", value: $appearance.lgpCache.messageMiB, format: .number)
                TextField("Rendered content (MiB)", value: $appearance.lgpCache.presentationMiB, format: .number)
                Button("Save Chat Memory") { updateAppearance(appearance) }
            }
            Section("Application text size") {
                Slider(value: $interfaceTextScale, in: 80...200, step: 10)
                HStack {
                    Text("\(Int(interfaceTextScale))%")
                        .monospacedDigit()
                    Spacer()
                    Button("Reset to 100%") { interfaceTextScale = 100 }
                }
                Text("Layout, Inbox, Sessions, Settings, and Resources preview")
                    .font(.system(size: 13 * interfaceTextScale / 100))
                Text("Terminal font remains a separate Workspace setting.")
                    .foregroundStyle(.secondary)
            }
        case .intelligence:
            Section("Session monitoring") {
                Toggle("Monitor tmux Sessions", isOn: $monitoring.enabled)
                Toggle("Use Codex pane-title signals", isOn: $monitoring.codexTitleDetection)
                Stepper("Analyze after quiet: \(monitoring.quietPeriodMilliseconds / 1000) seconds", value: $monitoring.quietPeriodMilliseconds, in: 1_000...30_000, step: 1_000)
                Button("Save Monitoring") { updateMonitoring(monitoring) }
                Text("tmux activity and bell events are event-driven. Pane output is captured only once after quiet when AI summaries are enabled.")
                    .foregroundStyle(.secondary)
            }
            Section("AI Session summaries") {
                Toggle("Use Codex app-server summaries", isOn: $summary.enabled)
                TextField("Codex executable", text: $summary.command)
                TextField("Model (Auto when empty)", text: Binding(get: { summary.model ?? "" }, set: { summary.model = $0.isEmpty ? nil : $0 }))
                Stepper("Timeout: \(summary.timeoutMilliseconds / 1000) seconds", value: $summary.timeoutMilliseconds, in: 1_000...60_000, step: 1_000)
                HStack { Button("Test Provider") { testSessionSummary(summary) }.disabled(!summary.enabled); Button("Save") { updateSessionSummary(summary) } }
                if let providerTestResult { Text(providerTestResult).foregroundStyle(.green) }
                Text("If AI is disabled or fails, generic activity settles to Unknown; direct Codex and bell signals remain visible.").foregroundStyle(.secondary)
            }
            Section("Codex completion signal") {
                Text(codexNotifyStatus.map { "Status: \($0.rawValue.capitalized)" } ?? "Status: Not checked")
                HStack {
                    Button("Check", action: inspectCodexNotify)
                    Button("Add Polar Station Notify", action: configureCodexNotify)
                        .disabled(codexNotifyStatus == .conflict || codexNotifyStatus == .invalid || codexNotifyStatus == .unsafe || codexNotifyStatus == .configured)
                }
                Text("An existing notify command is reported as Conflict and is never overwritten or chained.")
                    .foregroundStyle(.secondary)
            }
        case .notifications:
            if requestConnector != nil && requestCodex != nil {
                Section("Codex messages") { Button("Monitoring sources and connectors…") { connectorsOpen = true } }
                .sheet(isPresented: $connectorsOpen) {
                    if let requestConnector, let requestCodex {
                        ConnectorSettingsView(request: requestConnector, requestCodex: requestCodex, revision: 0, dismiss: { connectorsOpen = false })
                    }
                }
            }
            Section("iMessage completion notifications") {
                if isMessagesLoading { ProgressView("Connecting to this Mac’s sender…") }
                if let messagesError { Text(messagesError).foregroundStyle(.red) }
                if let status = messagesStatus {
                    Text(status.supported ? (status.authorized ? "Messages authorized" : "Open Messages and authorize this Mac to send notifications.") : "This Host cannot send iMessage.")
                        .foregroundStyle(.secondary)
                }
                Toggle("Enable this Mac as a sender", isOn: $messages.enabled)
                    .disabled(messagesSettings == nil || isMessagesLoading)
                TextField(messages.recipientCredentialRef == nil ? "Recipient phone number or Apple ID" : "Change recipient (leave blank to keep)", text: $messagesRecipient)
                Text("Recipient details are stored in this Mac’s credential store.").font(.caption).foregroundStyle(.secondary)
                TextField("Allowed execution Host IDs, one per line", text: $allowedSources, axis: .vertical).lineLimit(2...4)
                HStack {
                    Button("Open Messages") {
                        NSWorkspace.shared.open(URL(fileURLWithPath: "/System/Applications/Messages.app"))
                    }
                    Button("Authorize Messages", action: authorizeMessages)
                        .disabled(isMessagesLoading || messagesStatus?.supported != true)
                    Button("Refresh", action: reloadMessages).disabled(isMessagesLoading)
                    Button("Save iMessage") {
                        messages.allowedSourcePrincipals = allowedSources.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                        saveMessages(messages, messagesRecipient)
                        messagesRecipient = ""
                    }
                    .disabled(isMessagesLoading || messagesSettings == nil || (messages.enabled && messages.recipientCredentialRef == nil && messagesRecipient.isEmpty))
                }
            }

            Section("Attention") {
                Toggle("Show system notifications for urgent attention", isOn: Binding(
                    get: { projection?.attention.systemNotificationsEnabled ?? false },
                    set: { updateAttention(.init(systemNotificationsEnabled: $0)) }
                ))
                Text("Notifications use generic aggregate text and never include terminal output.").foregroundStyle(.secondary)
            }
        case .workspace:
            Section("Current Workspace") {
                Slider(value: $workspaceFont, in: 10...32, step: 1)
                Text("Terminal font: \(Int(workspaceFont)) pt")
                Button("Apply to Current Workspace") { updateWorkspaceFont(UInt8(workspaceFont)) }
                Text("Font remains part of the selected Workspace, not global Settings.").foregroundStyle(.secondary)
            }
        case .connections:
            Section { Button("Open Connections…", action: openConnections) } footer: { Text("Connection targets remain in the Host target registry.") }
        case .resources:
            Section { Button("Open Resources…", action: openResources) } footer: { Text("File mappings, port forwards, and Android mirrors remain in the Resources catalog.") }
        }
    }

    private func applyProjection() {
        widthText = projection?.appearance.layoutMapWidth.map(String.init) ?? ""
        inboxWidthText = projection?.appearance.inboxWidth.map(String.init) ?? ""
        if let value = projection?.appearance { appearance = value }
        interfaceTextScale = Double(projection?.appearance.interfaceTextScalePercent ?? 100)
        if let value = projection?.sessionSummary { summary = value }
        if let value = projection?.monitoring { monitoring = value }
        workspaceFont = Double(currentWorkspaceFontSize ?? 13)
    }

    private func symbol(_ page: Page) -> String {
        switch page { case .voice: "mic"; case .appearance: "paintbrush"; case .intelligence: "sparkles"; case .notifications: "bell"; case .workspace: "rectangle.3.group"; case .connections: "network"; case .resources: "externaldrive" }
    }
}
