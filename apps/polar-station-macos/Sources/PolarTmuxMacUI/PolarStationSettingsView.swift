import PolarTmuxHostClient
import SwiftUI
import AppKit

@MainActor public struct PolarStationSettingsView: View {
    @ObservedObject private var formBuffer:SettingsFormBuffer
    private enum Page: String, CaseIterable, Identifiable {
        case appearance = "Appearance"
        case intelligence = "Intelligence"
        case voice = "Voice"
        case notifications = "Notifications"
        case connections = "Connections"
        case resources = "Resources"
        var id: String { rawValue }
    }

    private let embedded: Bool
    private let initialPage: String?
    public let voicePlatform: VoicePlatformRequest?
    public let openSetup: (@MainActor () -> Void)?
    public let promptEditor:AnyView?
    private let voiceFormBuffer:VoiceFormBuffer
    public let editLease:SettingsEditInteraction?
    public let projection: SettingsProjectionV1?
    public let isLoading: Bool
    public let errorMessage: String?
    public let providerTestResult: String?
    public let codexNotifyStatus: CodexNotifyConfigStateV1?
    public let reload: () -> Void
    public let updateAppearance: (AppearanceSettingsV1) -> Void
    public let updateAttention: (AttentionSettingsV1) -> Void
    public let updateMonitoring: (MonitoringSettingsV1) -> Void
    public let updateSessionSummary: (SessionSummarySettingsV1) -> Void
    public let testSessionSummary: (SessionSummarySettingsV1) -> Void
    public let inspectCodexNotify: () -> Void
    public let configureCodexNotify: () -> Void
    public let openConnections: () -> Void
    public let openResources: () -> Void
    public let dismiss: @MainActor () -> Void

    public let messagesSettings: ImessageSenderSettingsV1?
    public let requestConnector: ConnectorRequestHandler?
    public let requestCodex: CodexRequestHandler?
    public let openConnectors: () -> Void
    public let messagesStatus: NotificationSenderStatusV1?
    public let messagesError: String?
    public let isMessagesLoading: Bool
    public let reloadMessages: () -> Void
    public let authorizeMessages: () -> Void
    public let saveMessages: (ImessageSenderSettingsV1, String) -> Void
    @Binding private var messages: ImessageSenderSettingsV1
    @Binding private var messagesRecipient: String
    @Binding private var allowedSources: String

    @State private var selectedPage: Page = .appearance
    @Binding private var widthText: String
    @Binding private var appearance: AppearanceSettingsV1
    @Binding private var summary: SessionSummarySettingsV1
    @Binding private var monitoring: MonitoringSettingsV1
    @Binding private var interfaceTextScale: Double

    public init(
        projection: SettingsProjectionV1?, isLoading: Bool, errorMessage: String?,
        providerTestResult: String?, codexNotifyStatus: CodexNotifyConfigStateV1?, reload: @escaping () -> Void,
        updateAppearance: @escaping (AppearanceSettingsV1) -> Void,
        updateAttention: @escaping (AttentionSettingsV1) -> Void,
        updateMonitoring: @escaping (MonitoringSettingsV1) -> Void,
        updateSessionSummary: @escaping (SessionSummarySettingsV1) -> Void,
        testSessionSummary: @escaping (SessionSummarySettingsV1) -> Void,
        inspectCodexNotify: @escaping () -> Void,
        configureCodexNotify: @escaping () -> Void,
        openConnections: @escaping () -> Void, openResources: @escaping () -> Void,
        openConnectors: @escaping () -> Void = {},
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
        initialVoicePage: Bool = false,
        openSetup: (@MainActor () -> Void)? = nil,
        embedded: Bool = false,
        initialPage: String? = nil,
        promptEditor:AnyView?=nil,
        editLease:SettingsEditInteraction?=nil,
        formBuffer:SettingsFormBuffer,
        voiceFormBuffer:VoiceFormBuffer
    ) {
        self.embedded = embedded; self.initialPage = initialPage
        self.voiceFormBuffer=voiceFormBuffer
        self.formBuffer=formBuffer
        self.editLease=editLease
        self._messages=Binding(get:{formBuffer.messages},set:{formBuffer.messagesEdits.edited(.imessage);formBuffer.messages=$0})
        self._messagesRecipient=Binding(get:{formBuffer.messagesRecipient},set:{formBuffer.messagesEdits.edited(.imessage);formBuffer.messagesRecipient=$0})
        self._allowedSources=Binding(get:{formBuffer.allowedSources},set:{formBuffer.messagesEdits.edited(.imessage);formBuffer.allowedSources=$0})
        self._widthText=Binding(get:{formBuffer.widthText},set:{formBuffer.widthText=$0})
        self._appearance=Binding(get:{formBuffer.appearance},set:{formBuffer.appearance=$0})
        self._summary=Binding(get:{formBuffer.summary},set:{formBuffer.summary=$0})
        self._monitoring=Binding(get:{formBuffer.monitoring},set:{formBuffer.monitoring=$0})
        self._interfaceTextScale=Binding(get:{formBuffer.interfaceTextScale},set:{formBuffer.interfaceTextScale=$0})
        self.promptEditor=promptEditor
        self._selectedPage = State(initialValue: initialPage.flatMap(Page.init(rawValue:)) ?? (initialVoicePage ? .voice : .appearance))
        self.voicePlatform = voicePlatform
        self.openSetup = openSetup
        self.requestConnector = requestConnector; self.requestCodex = requestCodex
        self.projection = projection; self.isLoading = isLoading; self.errorMessage = errorMessage
        self.providerTestResult = providerTestResult; self.codexNotifyStatus = codexNotifyStatus
        self.reload = reload; self.updateAppearance = updateAppearance; self.updateAttention = updateAttention
        self.updateMonitoring = updateMonitoring
        self.updateSessionSummary = updateSessionSummary; self.testSessionSummary = testSessionSummary
        self.inspectCodexNotify = inspectCodexNotify; self.configureCodexNotify = configureCodexNotify
        self.openConnections = openConnections; self.openResources = openResources
        self.dismiss = dismiss;self.openConnectors=openConnectors
        self.messagesSettings = messagesSettings; self.messagesStatus = messagesStatus
        self.messagesError = messagesError; self.isMessagesLoading = isMessagesLoading
        self.reloadMessages = reloadMessages; self.authorizeMessages = authorizeMessages; self.saveMessages = saveMessages
    }

    public var body: some View {
        Group {
            if embedded { settingsForm } else {
                NavigationSplitView {
                    List(Page.allCases, selection: $selectedPage) { page in Label(page.rawValue, systemImage: symbol(page)).tag(page) }
                        .navigationSplitViewColumnWidth(min: 176, ideal: 176)
                } detail: { settingsForm }
                .frame(minWidth:720,minHeight:500)
            }
        }
        .onChange(of:initialPage) { page in if let page, let value = Page(rawValue:page) { selectedPage = value } }
        .onReceive(NotificationCenter.default.publisher(for: Notification.Name("PolarStationOpenVoiceSettings"))) { _ in selectedPage = .voice }
        .task {
            applyProjection(); reload(); inspectCodexNotify()
            applyMessages()
        }
        .onChange(of: projection) { _ in applyProjection() }
        .onChange(of:widthText) { value in if value != (projection?.appearance.layoutMapWidth.map(String.init) ?? "") {editLease?.edited(.appearance)} }
        .onChange(of:appearance) { value in if value != projection?.appearance {editLease?.edited(.appearance)} }
        .onChange(of:interfaceTextScale) { value in if value != Double(projection?.appearance.interfaceTextScalePercent ?? 100) {editLease?.edited(.appearance)} }
        .onChange(of:summary) { value in if value != projection?.sessionSummary {editLease?.edited(.sessionSummary)} }
        .onChange(of:monitoring) { value in if value != projection?.monitoring {editLease?.edited(.monitoring)} }
        .onChange(of: messagesSettings) { _ in applyMessages() }
        .onChange(of: selectedPage) { value in if value == .notifications { reloadMessages() } }
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                if !embedded { Button("Close", action: dismiss)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityLabel("Close Polar Station Settings") }
            }
        }
        .stationTheme()
        .accessibilityIdentifier("polar-station-settings")
    }

    private func applyMessages() {
        guard let value=messagesSettings else{return}
        formBuffer.messagesEdits.observe(value.revision)
        if formBuffer.messagesEdits.replaces(.imessage) {
            formBuffer.messages=value
            formBuffer.allowedSources=value.allowedSourcePrincipals.joined(separator:"\n")
        }
    }
    private var settingsForm: some View {
        Form {
            if let errorMessage { Text(errorMessage).foregroundStyle(.red); Button("Retry",action:reload) }
            if let editLease,editLease.canRebase {
                Button(editLease.rebaseLabel) {editLease.rebase();reload()}
            }
            if projection == nil && isLoading { ProgressView("Loading Settings…") }
            pageBody
        }.formStyle(.grouped).navigationTitle(selectedPage.rawValue)
    }

    @ViewBuilder private var pageBody: some View {
        switch selectedPage {
        case .voice:
            VoiceSettingsView(platform: voicePlatform, openSetup:openSetup,promptEditor:promptEditor,formBuffer:voiceFormBuffer)
        case .appearance:
            Section("Theme") { StationThemePicker(projection: projection, save: updateAppearance) }
            Section("Monitor views") {
                Text("Layout, Inbox and Settings share one width. Sessions adds a column only on Layout.")
                    .foregroundStyle(.secondary)
                Picker("Approved terminal input", selection: $appearance.terminalDeliveryMode) {
                    Text("Insert only").tag(TerminalDeliveryModeV1.insert)
                    Text("Insert + Return").tag(TerminalDeliveryModeV1.send)
                }
            }
            Section("Control Center width") {
                TextField("Adaptive, or 200–390", text: $widthText)
                HStack {
                    Button("Use Adaptive Width") {
                        appearance.layoutMapWidth = nil
                        appearance.inboxWidth = nil
                        appearance.interfaceTextScalePercent = UInt16(interfaceTextScale)
                        updateAppearance(appearance)
                    }
                    Button("Save Appearance") {
                        appearance.layoutMapWidth = UInt16(widthText)
                        appearance.inboxWidth = nil
                        appearance.interfaceTextScalePercent = UInt16(interfaceTextScale)
                        updateAppearance(appearance)
                    }
                    .disabled(
                        UInt16(widthText).map { !(200...390).contains($0) } ?? !widthText.isEmpty
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
                Section("Codex messages") { Button("Monitoring sources and connectors…",action:openConnectors) }
            }
            Section("iMessage completion notifications") {
                if formBuffer.messagesEdits.canRebase {
                    Button(formBuffer.messagesEdits.rebaseLabel) {formBuffer.messagesEdits.rebase();reloadMessages()}
                }
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
        case .connections:
            Section { Button("Open Connections…", action: openConnections) } footer: { Text("Connection targets remain in the Host target registry.") }
        case .resources:
            Section { Button("Open Resources…", action: openResources) } footer: { Text("File mappings, port forwards, and Android mirrors remain in the Resources catalog.") }
        }
    }

    private func applyProjection() {
        guard let projection else {return}
        editLease?.observe(projection.revision)
        if editLease?.replaces(.appearance) != false {
            widthText = projection.appearance.layoutMapWidth.map(String.init) ?? ""
            appearance = projection.appearance
            interfaceTextScale = Double(projection.appearance.interfaceTextScalePercent)
        }
        if editLease?.replaces(.sessionSummary) != false { summary = projection.sessionSummary }
        if editLease?.replaces(.monitoring) != false { monitoring = projection.monitoring }
    }

    private func symbol(_ page: Page) -> String {
        switch page { case .voice: "mic"; case .appearance: "paintbrush"; case .intelligence: "sparkles"; case .notifications: "bell"; case .connections: "network"; case .resources: "externaldrive" }
    }
}
