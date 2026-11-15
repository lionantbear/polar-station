#if POLAR_STATION
    import AppKit
    import AVFoundation
    import CPolarStationDesktopClient
    import PolarTmuxHostClient
    import PolarTmuxMacUI
    import SwiftUI
    import UserNotifications

    @objc(PolarStationApplication)
    final class PolarStationApplication: NSApplication {
        override func restoreWindow(
            withIdentifier identifier: NSUserInterfaceItemIdentifier,
            state: NSCoder,
            completionHandler: @escaping (NSWindow?, Error?) -> Void
        ) -> Bool {
            if NativeWindowRestorationPolicy.shouldDiscardPersistedWindow(
                identifier: identifier.rawValue
            ) {
                completionHandler(nil, nil)
                return true
            }
            return super.restoreWindow(
                withIdentifier: identifier,
                state: state,
                completionHandler: completionHandler
            )
        }
    }

    private struct QuickCreatedSession {
        let workspaceID: String
        let tabID: String
        let paneID: String
        let sessions: [SessionRowProjectionV1.Identity]
    }

    private struct PendingLocalWorkspaceOperation {
        let window: PolarStationWindowContext
        let generation: UInt64
        let operation: ControlCenterIntentOperationV1?
        var work: (@MainActor () async throws -> Void)? = nil
        var requireWindow = true
        var workCompletion: (@MainActor (Error?) -> Void)? = nil
        var creation: QuickCreatedSession? = nil
        let completion: (@MainActor (String?) -> Void)?
    }

    @MainActor
    private final class PolarStationWindowContext: ObservableObject {
        let executor: PolarStationNativeExecutor
        let settingsNavigation = SettingsNavigation()
        let settingsEdits=SettingsEditInteraction()
        let settingsFormBuffer=SettingsFormBuffer()
        let voiceFormBuffer=VoiceFormBuffer()
        let connectorFormBuffer=ConnectorFormBuffer()
        var settingsFormHosts:[String:NSHostingView<PolarStationSettingsRoot>]=[:]
        let settingsConnections = ConnectionsInteraction()
        let connectionsFormBuffer = ConnectionsFormBuffer()
        let diagnostics=DiagnosticsInteraction()
        var toolSetup:ToolSetupNativePorts {
            ToolSetupNativePorts(launch:{[weak self] setup,started,exited in
                guard let self else {started("The originating workspace is unavailable.");return}
                executor.launchSetup(setup,started:started,exited:exited)
            },focus:{[weak self] in self?.executor.focusSetup($0)})
        }
        let presentation = ControlCenterPresentationState()
        let applicationShortcuts = ApplicationShortcutCommandState()
        @Published var layout: NativeWorkspaceLayoutView?
        var paneHost: NSHostingView<PolarStationPaneRoot>?
        var shortcutEditorHost: NSHostingView<PolarStationSessionShortcutRoot>?
        weak var activeContainer: TerminalViewContainer?
        var widthPreferenceLoaded = false
        init(executor: PolarStationNativeExecutor) {
            self.executor = executor
            layout = executor.layoutView()
        }
    }

    private struct PolarStationPaneRoot: View {
        @ObservedObject var windowContext: PolarStationWindowContext
        @ObservedObject var model: ControlCenterStore
        @ObservedObject var applicationShortcuts: ApplicationShortcutCommandState
        @ObservedObject var presentation: ControlCenterPresentationState
        let handlePresentationAction:
            @MainActor (ControlCenterPresentationAction) -> Void
        let dispatchIntent: @MainActor (ControlCenterIntentV1) -> Void
        let performLocalWorkspaceOperation:
            @MainActor (ControlCenterIntentOperationV1) -> Void

        var body: some View {
            VStack(spacing: 0) {
                    if let projection = model.projection {
                        ControlCenterView(
                            projection: projection,
                            nativeLayoutView: windowContext.layout,
                            presentation: presentation.body,
                            backendAvailability: model.backendAvailability,
                            liveSessions: Array(model.sessionRuntime.values),
                            inboxMessages: model.inboxPage.messages,
                            inboxOverview: model.inboxPage.overview,
                            focusedInboxMessageID: model.focusedInboxMessageID,
                            submitInboxReply: model.replyToInboxMessage,
                            editInboxDraft: model.editInboxDraft,
                            approveInboxDraft: model.approveInboxMessage,
                            emitPresentationAction: handlePresentationAction,
                            emitWorkspaceOperation:
                                performLocalWorkspaceOperation,
                            emitIntent: dispatchIntent,
                            submitSSHTarget: model.submitSSHTarget,
                            applicationShortcuts: applicationShortcuts,
                            requestCodex: model.requestCodex,
                            codexStateRevision: model.codexStateRevision,
                            readSessionEditor: model.readSessionEditor,
                            prepareSessionEditor: model.prepareSessionEditor,
                            sessionCreationRecovery: model.sessionCreationRecovery,
                            settingsNavigation:windowContext.settingsNavigation,
                            settingsContent:{ route in AnyView(settingsPage(route)) }
                        )
                    } else {
                        Color.clear
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .polarStationInterfaceTextScale(
                percent: model.settingsProjection?.appearance.interfaceTextScalePercent ?? 100
            )
            .stationTheme()
            .environment(\.stationToolSetupPorts,windowContext.toolSetup)
            .accessibilityIdentifier("polar-station-control-center")
        }

        @ViewBuilder private func settingsPage(_ route:SettingsNavigation.Route) -> some View {
            switch route.page {
            case "setup": PolarStationDiagnosticsRoot(model:model,remote:route.remote ?? false,context:windowContext)
            case "connections":
                if let projection = model.projection {
                    ConnectionsView(interaction:windowContext.settingsConnections,formBuffer:windowContext.connectionsFormBuffer,projection:projection.connections,controlRevision:projection.controlRevision,
                        close:windowContext.settingsNavigation.completeBack, requestCodex:model.requestCodex,
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
            default:
                RetainedSettingsForm(context:windowContext,page:route.page,root:PolarStationSettingsRoot(initialVoicePage:route.page == "voice",embedded:true,
                    initialPage:route.page == "notifications" ? "Notifications" : route.page == "workspace" ? "Workspace" : route.page == "intelligence" ? "Intelligence" : route.page == "voice" ? "Voice" : "Appearance",
                    promptEditor:AnyView(SettingsPromptControls(interaction:PolarStationRuntime.shared.settingsText,buffer:PolarStationRuntime.shared.settingsTextBuffer)),
                    voiceTools:PolarStationRuntime.shared.settingsVoiceTools,model:model,editLease:windowContext.settingsEdits,formBuffer:windowContext.settingsFormBuffer,voiceFormBuffer:windowContext.voiceFormBuffer,
                    updateWorkspaceFont:{ performLocalWorkspaceOperation(.setManagedTerminalFontSize(points:Int($0))) },
                    openResources:{ windowContext.settingsNavigation.open(.init(page:"resources")) },
                    openConnectors:{ windowContext.settingsNavigation.open(.init(page:"connectors")) },
                    dismiss:windowContext.settingsNavigation.back)).id(route.page)
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

    private struct PolarStationSessionShortcutRoot: View {
        @ObservedObject var context: PolarStationWindowContext
        @ObservedObject var model: ControlCenterStore
        var body: some View {
            if let projection = model.projection {
                SessionShortcutEditorHost(shortcuts: context.applicationShortcuts,
                    projection: projection, layout: context.layout,
                    requestCodex: model.requestCodex, read: model.readSessionEditor,
                    prepare: model.prepareSessionEditor, submit: model.submitSSHTarget,
                    recovery: model.sessionCreationRecovery)
                    .environment(\.stationToolSetupPorts,context.toolSetup)
            }
        }
    }

    private struct PolarStationDiagnosticsRoot: View {
        @ObservedObject var model: ControlCenterStore
        let remote: Bool
        let context:PolarStationWindowContext
        @ObservedObject private var interaction:DiagnosticsInteraction
        init(model:ControlCenterStore,remote:Bool,context:PolarStationWindowContext) {
            self.model=model;self.remote=remote;self.context=context;self.interaction=context.diagnostics;self.connectionsInteraction=context.settingsConnections
        }
        @ObservedObject private var connectionsInteraction:ConnectionsInteraction
        @State private var editor: DiagnosticsInteraction.Action?
        @State private var editorRow: String?
        @State private var editorGroup: String?
        @State private var voiceFacts: [String: Any] = [:]
        @State private var voice = VoiceClient()
        private var voiceTools:PolarStationVoiceTools { PolarStationRuntime.shared.settingsVoiceTools }
        @State private var retainedRow: DiagnosticsInteraction.Row?
        @State private var retainedHost: DiagnosticsInteraction.Host?

        private func object<T: Encodable>(_ value: T) -> Any {
            (try? JSONSerialization.jsonObject(with: JSONEncoder().encode(value))) ?? NSNull()
        }
        @MainActor private func updateFacts() {
            interaction.toolSetup=context.toolSetup
            interaction.read = model.readSessionEditor
            interaction.present = { action in
                switch action.type {
                case "reconnect_host": model.retryBackend()
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
            ScrollView {
                VStack(alignment:.leading, spacing:16) {
                    if let projection = interaction.projection {
                        HStack {
                            Text(remote ? projection.remote_title : projection.local_title).font(.title2.weight(.semibold))
                            Spacer()
                            Button(projection.refresh_label) { interaction.refresh(); Task { await sampleVoice() } }.disabled(interaction.busy)
                        }
                        if remote {
                            ForEach(projection.remote_setup) { row in render(row, group:"remote_setup") }
                            if projection.remote.isEmpty { Text(projection.empty_remote_label) }
                            ForEach(visibleHosts(projection.remote)) { host in
                                Text(host.name).font(.headline)
                                ForEach(visibleRows(host.rows, group:host.id)) { row in render(row, group:host.id) }
                            }
                        } else { ForEach(visibleRows(projection.local, group:"local")) { row in render(row, group:"local") } }
                    }
                    if let message = interaction.message { Text(message).foregroundStyle(.secondary).textSelection(.enabled) }
                    if interaction.busy { ProgressView() }
                }.padding(24).frame(maxWidth:.infinity, alignment:.leading)
            }
            .stationTheme()
            .polarStationInterfaceTextScale(percent:model.settingsProjection?.appearance.interfaceTextScalePercent ?? 100)
            .task { updateFacts(); model.loadResources(); await sampleVoice() }
            .onDisappear {interaction.present=nil}
            .onChange(of:model.backendAvailability) { _ in updateFacts() }
            .onChange(of:model.projection?.controlRevision) { _ in updateFacts() }
            .onChange(of:model.resourcesProjection?.revision) { _ in updateFacts() }
            .onReceive(NotificationCenter.default.publisher(for:NSApplication.didBecomeActiveNotification)) { _ in
                interaction.returned(); Task { await sampleVoice() }
            }
        }
        private func visibleHosts(_ hosts: [DiagnosticsInteraction.Host]) -> [DiagnosticsInteraction.Host] {
            guard editor != nil, let retainedHost, !hosts.contains(where: { $0.id == retainedHost.id }) else { return hosts }
            return hosts + [retainedHost]
        }
        private func visibleRows(_ rows: [DiagnosticsInteraction.Row], group: String) -> [DiagnosticsInteraction.Row] {
            guard editor != nil, editorGroup == group, let retainedRow, !rows.contains(where: { $0.id == retainedRow.id }) else { return rows }
            return rows + [retainedRow]
        }
        @ViewBuilder private func render(_ row: DiagnosticsInteraction.Row, group: String) -> some View {
            VStack(alignment:.leading, spacing:10) {
                HStack(alignment:.top, spacing:12) {
                    Image(systemName: row.state == "ready" ? "checkmark.circle.fill" : "info.circle")
                        .foregroundStyle(row.state == "ready" ? Color.green : Color.secondary)
                    VStack(alignment:.leading, spacing:4) { Text(row.title).font(.headline); Text(row.detail).foregroundStyle(.secondary) }
                    Spacer(minLength:8)
                    if let action = row.action, let label = row.action_label {
                        Button(label) {
                            retainedRow = row; retainedHost = interaction.projection?.remote.first(where: { $0.id == group })
                            editorRow = row.id; editorGroup = group; interaction.perform(action)
                        }.disabled(interaction.busy || editor != nil)
                    }
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
                        close:{editor=nil}, requestCodex:model.requestCodex, initialMachine:action.target_id ?? "local",
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

    private struct PolarStationIPhoneRoot: View {
        @ObservedObject var model: ControlCenterStore
        let dismiss: @MainActor () -> Void

        var body: some View {
            VStack(spacing: 0) {
                BackendStatusBanner(
                    availability: model.backendAvailability,
                    retry: model.retryBackend
                )
                IPhoneCompanionView(
                    pairing: model.companionPairing,
                    isLoading: model.isCompanionPairingLoading,
                    errorMessage: model.companionPairingError,
                    backendActionsEnabled:
                        model.backendAvailability.hostActionsEnabled,
                    refresh: model.loadCompanionPairing,
                    dismiss: dismiss
                )
            }
            .polarStationInterfaceTextScale(
                percent: model.settingsProjection?.appearance.interfaceTextScalePercent ?? 100
            )
        }
    }

    private struct PolarStationSettingsRoot: View {
        var initialVoicePage = false
        var embedded = false
        var initialPage: String? = nil
        var promptEditor:AnyView?=nil
        let voiceTools: PolarStationVoiceTools
        @ObservedObject var model: ControlCenterStore
        var editLease:SettingsEditInteraction?=nil
        let formBuffer:SettingsFormBuffer
        let voiceFormBuffer:VoiceFormBuffer
        let updateWorkspaceFont: @MainActor (UInt8) -> Void
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
                currentWorkspaceFontSize: model.nativeLayoutView.map { UInt8($0.managedTerminalFontSizePoints) },
                reload: model.loadSettings,
                updateAppearance: {model.updateAppearanceSettings($0,editLease:editLease)},
                updateAttention: {model.updateAttentionSettings($0,editLease:editLease)},
                updateMonitoring: {model.updateMonitoringSettings($0,editLease:editLease)},
                updateSessionSummary: {model.updateSessionSummarySettings($0,editLease:editLease)},
                testSessionSummary: model.testSessionSummaryProvider,
                inspectCodexNotify: model.inspectCodexNotify,
                configureCodexNotify: model.configureCodexNotify,
                updateWorkspaceFont: updateWorkspaceFont,
                openConnections: { PolarStationRuntime.shared.openSettingsRoute(.init(page:"connections")) },
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
                openSetup: { dismiss(); PolarStationRuntime.shared.openDiagnostics(.localDiagnostics) },
                embedded:embedded, initialPage:initialPage,promptEditor:promptEditor,editLease:editLease,formBuffer:formBuffer,voiceFormBuffer:voiceFormBuffer
            )
            .polarStationInterfaceTextScale(
                percent: model.settingsProjection?.appearance.interfaceTextScalePercent ?? 100
            )
        }
    }

    @MainActor
    private final class PolarStationAuxiliaryWindowCoordinator:
        NSObject,
        NSWindowDelegate
    {
        private let model: ControlCenterStore
        private var controllers: [String: NSWindowController] = [:]

        init(model: ControlCenterStore) {
            self.model = model
        }

        func show(
            _ descriptor: AuxiliarySurfaceProjectionV1,
            on screen: NSScreen?
        ) {
            guard descriptor.id == .iphone else { return }
            let key = descriptor.id.rawValue
            let controller: NSWindowController
            if let existing = controllers[key],
               let window = existing.window
            {
                window.title = descriptor.title
                controller = existing
            } else {
                let window = makeWindow(descriptor)
                position(window, on: screen)
                controller = NSWindowController(window: window)
                controllers[key] = controller
            }

            NSApp.activate(ignoringOtherApps: true)
            controller.showWindow(nil)
            controller.window?.makeKeyAndOrderFront(nil)
        }

        func hide(_ surface: AuxiliarySurfaceIDV1) {
            controllers[surface.rawValue]?.window?.orderOut(nil)
        }

        func hideAll() {
            controllers.values.forEach { $0.window?.orderOut(nil) }
        }

        private func makeWindow(
            _ descriptor: AuxiliarySurfaceProjectionV1
        ) -> NSWindow {
            let size=NSSize(width:420,height:529)
            let minimumSize=NSSize(width:390,height:460)
            let window = NSWindow(
                contentRect: NSRect(origin: .zero, size: size),
                styleMask: [
                    .titled,
                    .closable,
                    .miniaturizable,
                    .resizable,
                ],
                backing: .buffered,
                defer: false
            )
            window.identifier = NSUserInterfaceItemIdentifier(
                descriptor.id.rawValue
            )
            window.title = descriptor.title
            window.contentMinSize = minimumSize
            window.isReleasedWhenClosed = false
            window.isRestorable = false
            window.collectionBehavior = [
                .moveToActiveSpace,
                .fullScreenAuxiliary,
            ]
            window.delegate = self
            window.contentViewController = contentController(
                for: descriptor.id
            )
            return window
        }

        private func contentController(for surface:AuxiliarySurfaceIDV1) -> NSViewController {
            NSHostingController(rootView:PolarStationIPhoneRoot(model:model,dismiss:{ [weak self] in
                self?.model.send(.closeAuxiliary(surface:surface))
            }))
        }

        private func position(_ window: NSWindow, on screen: NSScreen?) {
            guard let visibleFrame = screen?.visibleFrame else {
                window.center()
                return
            }
            var frame = window.frame
            frame.size.width = min(frame.width, visibleFrame.width)
            frame.size.height = min(frame.height, visibleFrame.height)
            frame.origin.x = visibleFrame.midX - frame.width / 2
            frame.origin.y = visibleFrame.midY - frame.height / 2
            window.setFrame(frame, display: false)
        }

        func windowShouldClose(_ sender: NSWindow) -> Bool {
            guard let identifier = sender.identifier?.rawValue,
                  let surface = AuxiliarySurfaceIDV1(rawValue: identifier)
            else {
                sender.orderOut(nil)
                return false
            }
            if surface == .resources { model.cancelResourceFile() }
            model.send(.closeAuxiliary(surface: surface))
            return false
        }
    }

    @MainActor
    final class PolarStationRuntime: NSObject, UNUserNotificationCenterDelegate {
        static let shared = PolarStationRuntime()
        static let compactWidth: CGFloat = ControlCenterWidthPolicy.maximumMapWidth
        static let expandedWidth: CGFloat =
            ControlCenterWidthPolicy.sessionsWidth +
            ControlCenterWidthPolicy.dividerWidth +
            ControlCenterWidthPolicy.maximumMapWidth

        private let model: ControlCenterStore
        private let primaryWindow = PolarStationWindowContext(executor: PolarStationNativeExecutor())
        private var additionalWindows: [PolarStationWindowContext] = []
        private weak var lastActiveWindow: PolarStationWindowContext?
        private var windowRestorationStarted = false
        private var allWindows: [PolarStationWindowContext] { [primaryWindow] + additionalWindows }
        private var activeWindow: PolarStationWindowContext {
            if let window = NSApp.keyWindow ?? NSApp.mainWindow,
               let context = allWindows.first(where: { $0.executor.owns(window) }) { return context }
            return lastActiveWindow ?? primaryWindow
        }
        private var executor: PolarStationNativeExecutor { activeWindow.executor }
        private func context(for window: NSWindow) -> PolarStationWindowContext? {
            allWindows.first { $0.executor.owns(window) }
        }
        private func context(workspaceID: String) -> PolarStationWindowContext? {
            allWindows.first { $0.executor.initialControllerIsAttached && $0.executor.workspaceID == workspaceID }
        }
        private var pendingUnboundWindows = 0
        private let closingWindows = NSHashTable<NSWindow>.weakObjects()
        private func ensureContext(for controller: TerminalController) -> PolarStationWindowContext? {
            if let context = allWindows.first(where: { $0.executor.owns(controller) }) { return context }
            if !primaryWindow.executor.initialControllerIsAttached && additionalWindows.isEmpty {
                primaryWindow.executor.reset()
                if pendingUnboundWindows > 0 { pendingUnboundWindows -= 1; primaryWindow.executor.prepareUnboundWindow() }
                return primaryWindow
            }
            do {
                let context = PolarStationWindowContext(executor: try PolarStationNativeExecutor(shared: primaryWindow.executor))
                if pendingUnboundWindows > 0 { pendingUnboundWindows -= 1 }
                additionalWindows.append(context)
                configureExecutor(context)
                return context
            } catch {
                model.presentLocalError(error)
                return nil
            }
        }
        private func restoreWorkspaceWindowsIfReady() {
            guard !windowRestorationStarted, !stopping,
                  primaryWindow.executor.initialReadFinished,
                  primaryWindow.executor.initialControllerIsAttached,
                  startupGhostty != nil else { return }
            windowRestorationStarted = true
            // Only the selected Workspace is a launch target. Other saved
            // Workspaces are opened by an explicit user window/menu action.
            rememberOpenWorkspaceWindows()
        }

        private func rememberOpenWorkspaceWindows() {
            guard windowRestorationStarted, !stopping else { return }
            let ids = allWindows.filter { $0.executor.initialControllerIsAttached }.compactMap { $0.executor.workspaceID }
            primaryWindow.executor.rememberOpenWindows(ids)
        }
        private func refreshWindowLayouts(rememberWindows: Bool = true) {
            for context in allWindows {
                context.layout = context.executor.layoutView()
                if context.executor.workspaceID != nil {
                    context.presentation.restoreLegacyTab(inboxVisible: context.executor.panelVisibility.inbox)
                }
                if TerminalController.all.contains(where:{context.executor.owns($0) && $0.terminalViewContainer != nil}),
                   let remote=context.executor.consumeLegacySettingsScope() {
                    openSettingsRoute(.init(page:"setup",remote:remote),in:context)
                }
            }
            model.setNativeLayoutView(activeWindow.layout)
            if rememberWindows { rememberOpenWorkspaceWindows() }
        }
        private let lgpWorkspace = PolarStationLgpWorkspace()
        private let widthPreferences = ControlCenterWidthPreferences()
        private var presentation: ControlCenterPresentationState { activeWindow.presentation }
        private var applicationShortcuts: ApplicationShortcutCommandState { activeWindow.applicationShortcuts }
        private var quickCreateTabs: Set<String> = []
        private lazy var auxiliaryWindows =
            PolarStationAuxiliaryWindowCoordinator(model: model)
        fileprivate lazy var settingsVoiceTools: PolarStationVoiceTools = {
            let tools=PolarStationVoiceTools()
            tools.setShortcut={value,reply in PolarStationRuntime.shared.configureDictateShortcut(value,reply:reply)}
            tools.recoverRecording={id in PolarStationRuntime.shared.recoverDictateRecording(id)}
            return tools
        }()
        fileprivate let settingsText=SettingsTextInteraction()
        fileprivate lazy var settingsTextBuffer=SettingsTextBuffer(interaction:settingsText)
        private var settingsTextPaneID:String?
        private lazy var settingsTextConfirmation=SettingsTextConfirmationPresenter(interaction:settingsText) { [weak self] in
            guard let self else {return nil}
            let existing=self.promptPane()
            let context=existing?.context ?? self.activeWindow
            let tab=existing?.tab ?? context.executor.layoutView()?.selectedTabID
            let window=TerminalController.all.first(where:{context.executor.owns($0) && context.executor.nativeTabID($0)==tab})?.window
            if let window,!window.isKeyWindow {self.presentExplicitly(window)}
            return window
        }
        private var hostProcess: Process?
        private var hostLease: FileHandle?
        private var applicationPresentationState =
            ManagedApplicationPresentationState()
        private var bootstrapController: TerminalController?
        private var startupErrorAlert: NSAlert?
        private var startupGhostty: Ghostty.App?
        private var hostRestartAttempt = 0
        private var stopping = false
        private var statusItem: NSStatusItem?
        private var paneHost: NSHostingView<PolarStationPaneRoot>? {
            get { activeWindow.paneHost }
            set { activeWindow.paneHost = newValue }
        }
        private var activeContainer: TerminalViewContainer? {
            get { activeWindow.activeContainer }
            set { activeWindow.activeContainer = newValue }
        }
        private var widthPreferenceLoaded: Bool {
            get { activeWindow.widthPreferenceLoaded }
            set { activeWindow.widthPreferenceLoaded = newValue }
        }
        private var configured = false
        private lazy var shortcuts = PolarStationShortcuts(
            dispatch: { [weak self] action in
                self?.performShortcut(action) ?? false
            },
            badgesVisible: { [weak self] visible in
                self?.executor.setNumberBadgesVisible(visible)
            },
            context: { [weak self] in
                guard let layout=self?.executor.layoutView(),let tab=layout.tabs.first(where:{$0.tabID==layout.selectedTabID}) else {return StationShortcutHintContext(panes:0,tabs:0,idea:false)}
                let idea=tab.panes.first(where:{$0.paneID==layout.selectedPaneID})?.content?.kind == .ideaTree
                return StationShortcutHintContext(panes:UInt32(tab.panes.count),tabs:UInt32(layout.tabs.count),idea:idea)
            }
        )
        private var pendingControllerAttachments: [ObjectIdentifier: Bool] = [:]
        private var initialWindowPresentationCompleted = false
        private var controlCenterTransitionInFlight = false
        private var pendingAuxiliaryPresentation: AuxiliarySurfaceIDV1?
        private var localWorkspaceOperationInFlight = false
        private var nextManagedTabOrdinal = 2
        private var quitRequested = false
        private var quitSaveInFlight = false
        private var quitSaveCompleted = false
        private var pendingLocalWorkspaceOperations:
            [PendingLocalWorkspaceOperation] = []

        private static let hostRestartDelays: [TimeInterval] = [
            0.25,
            0.5,
            1.0,
            2.0,
        ]

        override private init() {
            model = ControlCenterStore(awaitsHostReadiness: true)
            super.init()
        }

        func configureDictateShortcut(_ text:String, reply:@escaping @MainActor ([String:Any])->Void) {lgpWorkspace.dictate.configureShortcut(text,reply:reply)}
        func recoverDictateRecording(_ id:String) {lgpWorkspace.dictate.recoverRecording(id)}
        func dictateKeyEvent(_ event: NSEvent) -> Bool { lgpWorkspace.dictate.handleShortcut(event) }
        func dictateShortcutStatus(_ status: String) { lgpWorkspace.dictate.shortcutStatus(status) }
        func dictateShortcutLost() { lgpWorkspace.dictate.cancel(); lgpWorkspace.dictate.shortcut(pressed: false) }

        func start(ghostty: Ghostty.App) {
            StationOperationTrace.record(1)
            stopping = false
            hostRestartAttempt = 0
            startupGhostty = ghostty
            UNUserNotificationCenter.current().delegate = self
            configureWorkspaceBridge()
            GlobalEventTap.shared.enableStationDictate()
            model.setSettingsProjectionSink { [weak self] projection in
                self?.applySettingsProjection(projection)
            }
            installNativeEntries()
            shortcuts.install(
                menuTarget: self,
                toggleSelector: #selector(toggleControlCenter(_:))
            )
            NotificationCenter.default.addObserver(
                self,
                selector: #selector(windowBecameKey(_:)),
                name: NSWindow.didBecomeKeyNotification,
                object: nil
            )
            NotificationCenter.default.addObserver(
                self,
                selector: #selector(windowWillClose(_:)),
                name: NSWindow.willCloseNotification,
                object: nil
            )
            NotificationCenter.default.addObserver(
                self,
                selector: #selector(windowEnteredFullScreen(_:)),
                name: NSWindow.didEnterFullScreenNotification,
                object: nil
            )
            NotificationCenter.default.addObserver(
                self,
                selector: #selector(windowExitedFullScreen(_:)),
                name: NSWindow.didExitFullScreenNotification,
                object: nil
            )
            NotificationCenter.default.addObserver(
                self,
                selector: #selector(applicationBecameActive(_:)),
                name: NSApplication.didBecomeActiveNotification,
                object: nil
            )
            let existing = existingManagedController()
            let startupCommands = applicationPresentationState.start(
                hasVisibleWindow: existing?.window != nil
            )
            executeApplicationPresentation(
                startupCommands.filter { $0 != .startAsynchronousRuntime },
                ghostty: ghostty
            )
            model.prepareForPresentation()
            model.startHostConnection()
            if !startHost() { scheduleHostRestart() }
        }

        func stop() {
            stopping = true
            lgpWorkspace.dictate.stop()
            if UNUserNotificationCenter.current().delegate === self {
                UNUserNotificationCenter.current().delegate = nil
            }
            applicationPresentationState.stop()
            NotificationCenter.default.removeObserver(self)
            shortcuts.stop()
            pendingControllerAttachments.removeAll()
            initialWindowPresentationCompleted = false
            for context in allWindows { context.executor.reset(); context.paneHost = nil }
            additionalWindows.removeAll()
            lastActiveWindow = nil
            controlCenterTransitionInFlight = false
            pendingAuxiliaryPresentation = nil
            auxiliaryWindows.hideAll()
            paneHost = nil
            hostLease?.closeFile()
            hostLease = nil
            hostProcess?.terminationHandler = nil
            hostProcess = nil
            bootstrapController = nil
            startupErrorAlert = nil
            StationTheme.shared.stop()
            startupGhostty = nil
        }

        func reopen(ghostty: Ghostty.App) {
            startupGhostty = ghostty
            let existing = existingManagedController()
            executeApplicationPresentation(
                applicationPresentationState.reopen(
                    hasVisibleWindow: existing?.window != nil
                ),
                ghostty: ghostty
            )
        }

        /// Routes every Polar Station New Tab entry through the same native
        /// Workspace FIFO used by the Control Center. Ordinary Ghostty builds
        /// do not compile this runtime.
        func requestManagedNewWindow() {
            guard !quitRequested else { return }
            dispatchControlCenterIntent(.init(controlRevision: 0, operation: .invokeProjectedAction(actionID: "workspace.window.new")))
        }
        func closeAllManagedWindows() {
            for context in allWindows {
                if let window = TerminalController.all.first(where: { context.executor.owns($0) })?.window { _ = closeManagedWindow(window) }
            }
        }

        func requestManagedNewTab() {
            performLocalWorkspaceOperation(.addTab(
                title: nextManagedTabTitle(),
                tabID: nil
            ))
        }

        private func nextManagedTabTitle() -> String {
            let nativeCount = model.nativeLayoutView?.tabs.count
                ?? 0
                ?? 1
            nextManagedTabOrdinal = max(
                nextManagedTabOrdinal,
                nativeCount + 1
            )
            defer { nextManagedTabOrdinal += 1 }
            return "Tab \(nextManagedTabOrdinal)"
        }

        @objc func toggleControlCenter(_ sender: Any?) {
            if let event = NSApp.currentEvent,
               event.type == .keyDown,
               event.isARepeat
            {
                return
            }
            if let button = sender as? NSStatusBarButton,
               let event = NSApp.currentEvent,
               event.type == .rightMouseDown
            {
                showStatusContextMenu(event: event, button: button)
                return
            }
            guard let controller = selectedControllerForControlCenter(),
                  let window = controller.window,
                  let container = controller.terminalViewContainer
            else { return }
            guard !controlCenterTransitionInFlight else { return }
            controlCenterTransitionInFlight = true
            defer {
                DispatchQueue.main.async { [weak self] in
                    self?.controlCenterTransitionInFlight = false
                }
            }

            if activeContainer === container, container.isPolarStationPaneVisible {
                container.hidePolarStationPane()
                activeContainer = nil
                if let focused = controller.focusedSurface,
                   window.firstResponder !== focused
                {
                    Ghostty.moveFocus(to: focused)
                }
                return
            }

            for item in TerminalController.all where executor.owns(item) {
                item.terminalViewContainer?.hidePolarStationPane()
            }
            if !NSApp.isActive || !window.isKeyWindow {
                presentExplicitly(window)
            }
            loadMapWidth(for: window)
            let host = controlCenterPaneHost()
            container.showPolarStationPane(
                host,
                width: presentation.body.preferredWidth
            )
            activeContainer = container
            model.prepareForPresentation()
        }

        nonisolated func userNotificationCenter(
            _ center: UNUserNotificationCenter,
            didReceive response: UNNotificationResponse,
            withCompletionHandler completionHandler: @escaping () -> Void
        ) {
            let messageID = response.notification.request.content.userInfo[
                "inbox_message_id"
            ] as? String
            let workspaceID = response.notification.request.content.userInfo["workspace_id"] as? String
            Task { @MainActor [weak self] in
                if let messageID {
                    if let workspaceID, let context = self?.context(workspaceID: workspaceID) {
                        self?.lastActiveWindow = context
                        if let window = context.activeContainer?.window { self?.presentExplicitly(window) }
                    }
                    self?.presentInbox(messageID: messageID)
                }
                completionHandler()
            }
        }

        private func presentInbox(messageID: String) {
            model.focusInboxMessage(messageID)
            presentation.send(.selectTab(.inbox))
            guard activeContainer?.isPolarStationPaneVisible != true else {
                activeContainer?.updatePolarStationPaneWidth(
                    presentation.body.preferredWidth
                )
                return
            }
            toggleControlCenter(nil)
        }

        private func showStatusContextMenu(
            event: NSEvent,
            button: NSStatusBarButton
        ) {
            let menu = NSMenu()
            menu.addItem(
                withTitle: activeContainer?.isPolarStationPaneVisible == true
                    ? "Hide Control Center"
                    : "Show Control Center",
                action: #selector(toggleControlCenterFromMenu(_:)),
                keyEquivalent: ""
            )
            menu.addItem(.separator())
            menu.addItem(withTitle:"Open Selected File",action:#selector(openSelectedFileFromMenu(_:)),keyEquivalent:"")
            menu.addItem(withTitle:"Local Diagnostics",action:#selector(openLocalDiagnostics(_:)),keyEquivalent:"")
            menu.addItem(withTitle:"Remote Hosts",action:#selector(openRemoteDiagnostics(_:)),keyEquivalent:"")
            menu.addItem(
                withTitle: "Resources…",
                action: #selector(showResourcesFromMenu(_:)),
                keyEquivalent: ""
            )
            menu.addItem(
                withTitle: "iPhone Companion…",
                action: #selector(showIPhoneFromMenu(_:)),
                keyEquivalent: ""
            )
            menu.addItem(
                withTitle: "Settings…",
                action: #selector(showSettingsFromMenu(_:)),
                keyEquivalent: ","
            )
            menu.addItem(.separator())
            menu.addItem(
                withTitle: "Quit Polar Station",
                action: #selector(quitFromMenu(_:)),
                keyEquivalent: "q"
            )
            menu.items.forEach { $0.target = self }
            NSMenu.popUpContextMenu(menu, with: event, for: button)
        }

        @objc private func toggleControlCenterFromMenu(_ sender: Any?) {
            toggleControlCenter(sender)
        }

        @objc private func openLocalDiagnostics(_: Any?) { openDiagnostics(.localDiagnostics) }
        @objc private func openRemoteDiagnostics(_: Any?) { openDiagnostics(.remoteHosts) }
        fileprivate func openDiagnostics(_ content: StationPaneContentV1) {
            openSettingsRoute(.init(page:"setup",remote:content.kind == .remoteHosts))
        }
        fileprivate func openSettingsRoute(_ route:SettingsNavigation.Route,in windowContext:PolarStationWindowContext? = nil) {
            let context=windowContext ?? activeWindow
            let selectedTab=context.executor.layoutView()?.selectedTabID
            guard let controller=TerminalController.all.first(where:{context.executor.owns($0) && context.executor.nativeTabID($0)==selectedTab}) ?? TerminalController.all.first(where:{context.executor.owns($0)}),
                  let window=controller.window,let container=controller.terminalViewContainer else { return }
            if !window.isKeyWindow { presentExplicitly(window) }
            loadMapWidth(for:window)
            context.settingsNavigation.open(route)
            _ = context.presentation.send(.selectTab(.settings))
            if !container.isPolarStationPaneVisible {
                context.activeContainer?.hidePolarStationPane()
                container.showPolarStationPane(controlCenterPaneHost(for:context),width:context.presentation.body.preferredWidth)
                context.activeContainer=container
                model.prepareForPresentation()
            } else {
                container.updatePolarStationPaneWidth(context.presentation.body.preferredWidth)
            }
        }

        private func promptPane()->(context:PolarStationWindowContext,tab:String,pane:String)? {
            for context in allWindows {
                guard let layout=context.executor.layoutView() else {continue}
                for tab in layout.tabs {
                    if let pane=tab.panes.first(where:{$0.content?.kind == .settingsText}) {
                        return (context,tab.tabID,pane.paneID)
                    }
                }
            }
            return nil
        }
        private func expandPrompt(_ field:String) {
            let existing=promptPane()
            let context=existing?.context ?? activeWindow
            let sameField=existing.map { current in context.executor.layoutView()?.tabs.flatMap(\.panes).first(where:{$0.paneID == current.pane})?.content?.settingsField == field } ?? false
            if let existing,sameField,context.executor.layoutView()?.selectedTabID == existing.tab {
                if let window=TerminalController.all.first(where:{context.executor.owns($0) && context.executor.nativeTabID($0)==existing.tab})?.window {
                    settingsTextBuffer.setPresentationWindow(window)
                    if !window.isKeyWindow {presentExplicitly(window)}
                }
                settingsTextBuffer.focus();return
            }
            context.executor.enqueueMutation?({ [weak self] in
                if let existing {try await context.executor.performImmediate(.selectTab(tabID:existing.tab))}
                if !sameField {try await context.executor.openConversation(.init(settingsField:field),replacing:existing?.pane)}
                if let window=TerminalController.all.first(where:{context.executor.owns($0) && context.executor.nativeTabID($0)==context.executor.layoutView()?.selectedTabID})?.window {
                    self?.settingsTextBuffer.setPresentationWindow(window)
                    if !window.isKeyWindow {self?.presentExplicitly(window)}
                }
                self?.settingsTextBuffer.focus()
            },true,{[weak self] error in if let error {self?.model.presentLocalError(error)}})
        }
        private func returnPromptToSidebar() {
            let existing=promptPane()
            openSettingsRoute(.init(page:"voice"),in:existing?.context)
            if let existing {
                existing.context.executor.enqueueMutation?({try await existing.context.executor.closeContentPane(existing.pane)},true,{[weak self] error in if let error {self?.model.presentLocalError(error)}})
            }
        }
        private func closePromptPane() {
            if let existing=promptPane() {
                existing.context.executor.enqueueMutation?({try await existing.context.executor.closeContentPane(existing.pane)},true,{[weak self] error in if let error {self?.model.presentLocalError(error)}})
            }
        }

        @objc private func openSelectedFileFromMenu(_:Any?) {
            guard let context=lastActiveWindow,let selected=context.executor.selectedResourceFile() else{return}
            _=openResourceFile(selected.value,surface:selected.surface)
        }
        func openResourceFile(_ value:String,surface:Ghostty.SurfaceView)->Bool {
            let bytes=Array(value.utf8)
            guard bytes.withUnsafeBufferPointer({polar_station_file_input_valid($0.baseAddress,$0.count)}) else{return false}
            guard let window=surface.window,let context=context(for:window),let source=context.executor.resourceFileSource(surface:surface) else{return false}
            model.bindResourceFileWindow(window)
            model.requestFileMapping={ [weak self, weak window] in
                guard let self, let window, self.context(for:window) != nil else{return}
                self.openSettingsRoute(.init(page:"resources"),in:self.context(for:window))
            }
            model.loadResources()
            model.openResourceFile(target:source.target,path:value,cwd:source.cwd,terminalSessionID:source.session)
            return true
        }

        @objc private func showResourcesFromMenu(_: Any?) {
            openSettingsRoute(.init(page:"resources"))
        }

        @objc private func showIPhoneFromMenu(_: Any?) {
            requestAuxiliaryPresentation(.iphone)
        }

        @objc private func showSettingsFromMenu(_: Any?) { openSettingsRoute(.init(page:"home")) }

        @objc private func quitFromMenu(_: Any?) {
            NSApp.terminate(nil)
        }

        private func selectedControllerForControlCenter() -> TerminalController? {
            if let preferred = TerminalController.preferredParent,
               preferred.window != nil,
               preferred.terminalViewContainer != nil
            {
                return preferred
            }
            return TerminalController.all.first {
                $0.window != nil && $0.terminalViewContainer != nil
            }
        }

        @objc private func windowBecameKey(_ notification: Notification) {
            guard let window = notification.object as? NSWindow,
                  !closingWindows.contains(window),
                  let controller = window.windowController as? TerminalController
            else { return }
            // Attachment establishes controller ownership before key-window
            // observation; otherwise an unbound context is created twice.
            attachWhenPresented(controller)
            guard let context = context(for: window) else { return }
            lastActiveWindow = context
            refreshWindowLayouts()
            if let selection = context.executor.observeNativeTabSelection(controller) {
                performLocalWorkspaceOperation(.selectTab(tabID: selection.tabID), in: context) { [weak context] error in
                    if error == nil { context?.executor.completeNativeTabSelection(selection) }
                }
            }
        }

        func closeNativeSurfaces(_ surfaces: [Ghostty.SurfaceView], controller: BaseTerminalController) -> Bool {
            guard let window = controller.window, let context = context(for: window),
                  !context.executor.isMutatingNativeTabs else { return false }
            let ids = surfaces.compactMap { context.executor.nativePaneID($0) }
            if ids.isEmpty { return nativeAction("close_tab", controller: controller) }
            context.executor.enqueueMutation?({ [executor = context.executor] in
                for id in ids { try await executor.closeContentPane(id) }
            }, true) { _ in }
            return true
        }

        /// Resolved Ghostty actions enter here regardless of their physical shortcut.
        @discardableResult
        func nativeAction(_ action: String, controller: BaseTerminalController, title: String? = nil) -> Bool {
            guard let controller = controller as? TerminalController, let window = controller.window,
                  let context = context(for: window), !context.executor.isMutatingNativeTabs else { return false }
            guard !quitRequested else { return true }
            let executor = context.executor
            if action == "native_tab_order_changed" { executor.nativeTabOrderChanged(); return true }
            let tabID = executor.nativeTabID(controller)
            do {
                let decision = try executor.nativeActionDecision(action, tabID: tabID)
                if decision.effect == "unhandled" { return false }
                if action == "new_tab" { performLocalWorkspaceOperation(.addTab(title: nextManagedTabTitle()), in: context); return true }
                guard let tabID else { return true }
                if action == "move_tab" {
                    presentTabDestination(tabID, in: context, window: window)
                } else if action == "rename_tab", title == nil {
                    let alert = NSAlert()
                    alert.messageText = "Rename Tab"
                    alert.informativeText = "Enter a non-empty Tab name."
                    let field = NSTextField(string: controller.titleOverride ?? window.title)
                    field.frame = NSRect(x: 0, y: 0, width: 280, height: 24)
                    alert.accessoryView = field
                    alert.addButton(withTitle: "Rename"); alert.addButton(withTitle: "Cancel")
                    alert.window.initialFirstResponder = field
                    alert.beginSheetModal(for: window) { [weak self, weak context] result in
                        guard result == .alertFirstButtonReturn, let self, let context else { return }
                        self.queueNativeAction("rename_tab", tabID: tabID, title: field.stringValue, in: context)
                    }
                } else {
                    queueNativeAction(action, tabID: tabID, title: title, in: context)
                }
            } catch { model.presentLocalError(error) }
            return true // Disabled and failed managed actions must never fall through.
        }
        private func presentTabDestination(_ tabID: String, in context: PolarStationWindowContext, window: NSWindow) {
            guard window.attachedSheet == nil else { return }
            let choices = context.executor.tabMoveDestinations
            let alert = NSAlert()
            alert.messageText = "Move Tab to Workspace"
            alert.informativeText = "Choose a Workspace, or explicitly create one. Cancel keeps the Tab here."
            let box = NSView(frame: NSRect(x: 0, y: 0, width: 320, height: 64))
            let picker = NSPopUpButton(frame: NSRect(x: 0, y: 34, width: 320, height: 26))
            picker.addItems(withTitles: choices.map(\.name) + ["Create Workspace…"])
            let name = NSTextField(string: "Workspace")
            name.frame = NSRect(x: 0, y: 0, width: 320, height: 24)
            name.placeholderString = "Name for a new Workspace"
            box.addSubview(picker); box.addSubview(name); alert.accessoryView = box
            alert.addButton(withTitle: "Move"); alert.addButton(withTitle: "Cancel")
            alert.beginSheetModal(for: window) { [weak self, weak context] response in
                guard response == .alertFirstButtonReturn, let self, let context else { return }
                let target = picker.indexOfSelectedItem < choices.count ? choices[picker.indexOfSelectedItem].id : nil
                context.executor.enqueueMutation?({ [self, context] in
                    let id = try await context.executor.moveNativeTab(tabID, to: target, newWorkspaceName: name.stringValue)
                    if let destination = self.context(workspaceID: id) {
                        destination.executor.refreshTransferredWorkspace()
                        if let native = TerminalController.all.first(where: { destination.executor.owns($0) })?.window { self.presentExplicitly(native) }
                    } else if let ghostty = self.startupGhostty {
                        self.pendingUnboundWindows += 1
                        let controller = TerminalController.newWindow(ghostty)
                        if let destination = self.ensureContext(for: controller) {
                            _ = destination.executor.attach(controller)
                            try await destination.executor.performImmediate(.selectWorkspace(workspaceID: id))
                        }
                    }
                }, true) { _ in }
            }
        }
        private func queueNativeAction(_ action: String, tabID: String, title: String?, in context: PolarStationWindowContext) {
            context.executor.enqueueMutation?({ [executor = context.executor] in
                try await executor.performNativeAction(action, tabID: tabID, title: title)
            }, true) { _ in }
        }
        func managedMenuValidation(_ item: NSMenuItem, controller: BaseTerminalController) -> Bool? {
            guard let selector = item.action else { return nil }
            let actions = ["closeTab:": "close_tab", "closeOtherTabs:": "close_other_tabs", "closeTabsOnTheRight:": "close_right_tabs",
                "changeTitle:": "rename_tab", "splitRight:": "split", "splitDown:": "split", "splitZoom:": "zoom_split",
                "equalizeSplits:": "equalize_splits", "moveSplitDividerUp:": "resize_split", "moveSplitDividerDown:": "resize_split",
                "moveSplitDividerLeft:": "resize_split", "moveSplitDividerRight:": "resize_split", "mergeAllWindows:": "merge_windows"]
            guard let action = actions[NSStringFromSelector(selector)] else { return nil }
            return managedNativeActionEnabled(action, controller: controller)
        }
        func managedNativeActionEnabled(_ action: String, controller: BaseTerminalController) -> Bool? {
            guard let controller = controller as? TerminalController, let window = controller.window,
                  let context = context(for: window) else { return nil }
            do { return try context.executor.nativeActionDecision(action, tabID: context.executor.nativeTabID(controller)).effect != "disabled" }
            catch { return false }
        }
        func prepareToTerminate() -> NSApplication.TerminateReply {
            if quitSaveCompleted { return .terminateNow }
            quitRequested = true
            sendNextLocalWorkspaceOperation()
            // terminateLater runs a modal loop that cannot drain our MainActor
            // Tasks. Keep the normal loop alive until the existing save finishes.
            return .terminateCancel
        }
        private func finishQuitIfDrained() {
            guard quitRequested, !quitSaveInFlight, !localWorkspaceOperationInFlight,
                  pendingLocalWorkspaceOperations.isEmpty else { return }
            quitSaveInFlight = true
            Task { @MainActor in
                do {
                    guard await settingsText.confirmClose() else {
                        quitRequested=false;quitSaveInFlight=false;return
                    }
                    for context in allWindows {
                        guard await context.executor.confirmSetupTermination() else {
                            quitRequested=false;quitSaveInFlight=false;return
                        }
                    }
                    try await primaryWindow.executor.flushPersistence()
                    quitSaveCompleted = true
                    NSApp.terminate(nil)
                } catch {
                    quitRequested = false
                    quitSaveInFlight = false
                    model.presentLocalError(error)
                }
            }
        }

        @discardableResult
        func closeManagedWindow(_ window: NSWindow) -> Bool {
            if closingWindows.contains(window) { return true }
            guard let context = context(for: window),
                  !context.executor.isMutatingNativeTabs else { return false }
            NSLog("Station lifecycle: user close for bound=%@", context.executor.workspaceID ?? "unbound")
            performLocalWorkspaceOperation(.invokeProjectedAction(actionID: "window.close"), in: context)
            return true
        }

        @objc private func windowWillClose(_ notification: Notification) {
            guard let window = notification.object as? NSWindow else { return }
            closingWindows.add(window)
            if let controller = window.windowController as? TerminalController {
                applicationPresentationState.windowClosed()
                pendingControllerAttachments.removeValue(
                    forKey: ObjectIdentifier(controller)
                )
                if let context = context(for: window) {
                    if context.activeContainer === controller.terminalViewContainer {
                        context.activeContainer?.hidePolarStationPane()
                        context.activeContainer = nil
                    }
                    let hasOtherTabs = window.tabbedWindows?.contains(where: { $0 !== window }) ?? false
                    if !hasOtherTabs && !context.executor.isMutatingNativeTabs {
                        context.paneHost = nil
                        context.settingsFormHosts.removeAll()
                        context.executor.reset(restoreSelection: false)
                        additionalWindows.removeAll { $0 === context }
                        if lastActiveWindow === context { lastActiveWindow = nil }
                        rememberOpenWorkspaceWindows()
                    }
                }
            }
        }

        @objc private func windowEnteredFullScreen(_ notification: Notification) {
            guard let window = notification.object as? NSWindow else { return }
            context(for: window)?.executor.windowDidEnterFullScreen(window)
        }

        func windowDidFailToEnterFullScreen(_ window: NSWindow) {
            context(for: window)?.executor.windowDidFailToEnterFullScreen(window)
        }

        @objc private func windowExitedFullScreen(_ notification: Notification) {
            guard let window = notification.object as? NSWindow else { return }
            context(for: window)?.executor.windowDidExitFullScreen(window)
        }

        @objc private func applicationBecameActive(_: Notification) {
            guard let controller = TerminalController.preferredParent
                ?? TerminalController.all.first
            else { return }
            attachWhenPresented(controller)
        }

        private func existingManagedController() -> TerminalController? {
            if let preferred = TerminalController.preferredParent,
               let window = preferred.window, !closingWindows.contains(window) { return preferred }
            return TerminalController.all.first { controller in
                controller.window.map { !closingWindows.contains($0) } ?? false
            }
        }

        private func executeApplicationPresentation(
            _ commands: [ManagedApplicationPresentationState.Command],
            ghostty: Ghostty.App
        ) {
            for command in commands {
                switch command {
                case .createAndPresentWindow:
                    materializeManagedWindow(ghostty: ghostty)
                case .presentExistingWindow:
                    guard let controller = existingManagedController(),
                          let window = controller.window
                    else {
                        materializeManagedWindow(ghostty: ghostty)
                        continue
                    }
                    presentExplicitly(window)
                    attachWhenPresented(controller)
                case .startAsynchronousRuntime:
                    break
                }
            }
        }

        private func materializeManagedWindow(ghostty: Ghostty.App) {
            if let controller = existingManagedController(),
               let window = controller.window
            {
                presentExplicitly(window)
                attachWhenPresented(controller)
                return
            }

            let controller = TerminalController(ghostty)
            bootstrapController = controller
            controller.showWindow(nil)
            guard let window = controller.window else {
                applicationPresentationState.windowClosed()
                showStartupWindowError()
                return
            }
            window.isRestorable = false
            initialWindowPresentationCompleted = true
            presentExplicitly(window)
            attachWhenPresented(controller)
            startupErrorAlert?.window.orderOut(nil)
            startupErrorAlert = nil
        }

        private func showStartupWindowError() {
            let alert = startupErrorAlert ?? NSAlert()
            alert.messageText = "Polar Station could not open its terminal window."
            alert.informativeText =
                "The background service is not required to open the interface. Retry the window or quit Polar Station."
            if alert.buttons.isEmpty {
                alert.addButton(withTitle: "Retry")
                alert.addButton(withTitle: "Quit")
                alert.buttons[0].target = self
                alert.buttons[0].action = #selector(retryStartupWindow(_:))
                alert.buttons[1].target = self
                alert.buttons[1].action = #selector(quitFromMenu(_:))
            }
            startupErrorAlert = alert
            NSApp.activate(ignoringOtherApps: true)
            alert.window.makeKeyAndOrderFront(nil)
        }

        @objc private func retryStartupWindow(_: Any?) {
            guard let startupGhostty else { return }
            materializeManagedWindow(ghostty: startupGhostty)
        }

        private func attachWhenPresented(
            _ controller: TerminalController,
            presentOnAttach: Bool = false
        ) {
            StationOperationTrace.record(23)
            let identity = ObjectIdentifier(controller)
            // Existing window/application notifications retry this intent when a
            // window exists. Never schedule a timer for presentation readiness.
            guard let window = controller.window else {
                pendingControllerAttachments[identity] =
                    (pendingControllerAttachments[identity] ?? false) || presentOnAttach
                return
            }

            guard !closingWindows.contains(window) else { return }

            let shouldPresent = pendingControllerAttachments.removeValue(
                forKey: identity
            ) ?? presentOnAttach
            if shouldPresent, !initialWindowPresentationCompleted {
                initialWindowPresentationCompleted = true
                presentExplicitly(window)
            }
            StationOperationTrace.record(24)
            let isInitialController = attach(controller, window: window)
            if isInitialController {
                scheduleStartupFullscreenIfReady()
            }
        }

        private func presentExplicitly(_ window: NSWindow) {
            guard !closingWindows.contains(window) else { return }
            window.deminiaturize(nil)
            NSApp.unhide(nil)
            NSApp.activate(ignoringOtherApps: true)
            window.makeKeyAndOrderFront(nil)
            StationOperationTrace.record(2)
        }

        @discardableResult
        private func attach(
            _ controller: TerminalController,
            window: NSWindow
        ) -> Bool {
            guard let context = ensureContext(for: controller) else { return false }
            let isInitialController = context.executor.attach(controller)
            if let content = window.contentView {
                let host = context.shortcutEditorHost ?? NSHostingView(
                    rootView: PolarStationSessionShortcutRoot(context: context, model: model))
                context.shortcutEditorHost = host
                if host.superview !== content {
                    host.removeFromSuperview()
                    host.frame = .zero
                    content.addSubview(host, positioned: .below, relativeTo: nil)
                }
            }
            if window.isKeyWindow { lastActiveWindow = context }
            window.tabbingIdentifier = context.executor.workspaceID ?? "PolarStation"
            lgpWorkspace.attachWindow(window, workspaceID: context.executor.workspaceID)
            context.executor.reconcileInputSelection()
            restoreWorkspaceWindowsIfReady()
            rememberOpenWorkspaceWindows()
            guard context.activeContainer != nil,
                  let destination = controller.terminalViewContainer,
                  destination !== context.activeContainer
            else { return isInitialController }
            context.activeContainer?.hidePolarStationPane()
            loadMapWidth(for: window)
            destination.showPolarStationPane(
                controlCenterPaneHost(for: context),
                width: context.presentation.body.preferredWidth
            )
            context.activeContainer = destination
            return isInitialController
        }

        private func configureExecutor(_ context: PolarStationWindowContext) {
            let executor = context.executor
            executor.enqueueMutation = { [weak self, weak context] work, requireWindow, completion in
                guard let self, let context, !self.quitSaveInFlight else { completion(PolarStationNativeExecutorError.workspaceUnavailable); return }
                pendingLocalWorkspaceOperations.append(.init(window: context, generation: context.executor.windowGeneration,
                    operation: nil, work: work, requireWindow: requireWindow, workCompletion: completion, completion: nil))
                sendNextLocalWorkspaceOperation()
            }
            executor.confirmContentRetirement = { [weak self] ids in
                guard let self,let pane=self.settingsTextPaneID,ids.contains(pane),self.settingsText.state?.expanded == true else {return true}
                return await self.settingsText.confirmClose()
            }
            executor.lifecycleIntent = { [weak self, weak context] operation in
                guard let self, let context else { return }
                self.performLocalWorkspaceOperation(operation, in: context)
            }
            executor.requestTabMove = { [weak self] controller in _ = self?.nativeAction("move_tab", controller: controller) }
            executor.toggleTabFocus = { [weak self, weak context] tabID in
                guard let context else { return }
                self?.performLocalWorkspaceOperation(.toggleDisplayMode(tabID: tabID), in: context)
            }
            executor.displayCreated = { [weak self, weak context] id, content, display, window in
                if content.kind == .toolSetup,let copy=ToolSetupCopy.load() {
                    let text=NSTextField(wrappingLabelWithString:copy.restored_title+"\n\n"+copy.restored_detail)
                    text.isSelectable=true
                    display.mountNativeContent(text)
                    return
                }
                if let self,content.kind == .settingsText {
                    self.settingsTextPaneID=id
                    self.settingsTextBuffer.setPresentationWindow(display.view.window ?? window)
                    self.lgpWorkspace.created(id,content:content,display:display,window:window)
                    if self.settingsText.state?.field != content.settingsField {self.settingsText.edit(content.settingsField)}
                    if self.settingsText.state?.expanded != true {self.settingsText.action("expand")}
                    display.mountNativeContent(NSHostingView(rootView:SettingsTextEditor(interaction:self.settingsText,buffer:self.settingsTextBuffer,expanded:true)))
                    context?.executor.reconcileInputSelection()
                    return
                }
                if let self,let context, content.kind == .localDiagnostics || content.kind == .remoteHosts {
                    display.mountNativeContent(NSHostingView(rootView: PolarStationDiagnosticsRoot(model: self.model, remote: content.kind == .remoteHosts,context:context)))
                }
                self?.lgpWorkspace.created(id, content: content, display: display, window: window)
                context?.executor.reconcileInputSelection()
            }
            executor.displayRetired = { [weak self] id in
                if let self,id == self.settingsTextPaneID,self.promptPane()==nil {
                    self.settingsTextPaneID=nil
                    if self.settingsText.state?.expanded == true {self.settingsText.action("collapse")}
                }
                self?.lgpWorkspace.retired(id)
            }
            executor.displayInteraction = { [weak self] in self?.lgpWorkspace.refreshRenderGuards() }
            executor.displayActivated = { [weak self] id in
                if let self,id == self.settingsTextPaneID {self.settingsTextBuffer.focus()}
                else {self?.lgpWorkspace.activateComposer(id)}
            }
            executor.displaySelected = { [weak self] id in
                if let self,id == self.settingsTextPaneID {self.settingsTextBuffer.focus()}
                else {self?.lgpWorkspace.selected(id)}
            }
            executor.setupSelected = {[weak self] window,id in self?.lgpWorkspace.setupSelected(id,in:window)}
            executor.terminalSelected = { [weak self] window, paneID, targetID, sessionID in self?.lgpWorkspace.terminalSelected(paneID, in: window, targetID:targetID, sessionID:sessionID) }
            executor.displayFailed = { [weak self] id, error in self?.lgpWorkspace.failed(id, error) }
            executor.requestContent = { [weak self] url, request in
                guard let self else { throw PolarTmuxLocalIPCError.connectionClosed }
                return try await self.model.requestPaneContent(socketURL: url, request: request)
            }
            executor.submitRecovery = { [weak self] url, request in
                guard let self else { throw PolarTmuxLocalIPCError.connectionClosed }
                try await self.model.sendPaneRecovery(socketURL: url, request: request)
            }
            executor.persistenceFailed = { [weak self] message in self?.model.reportNativeWorkspaceFailure(message) }
            executor.catalogChanged = { [weak self, weak context] in
                guard let self, let context else { return }
                restoreWorkspaceWindowsIfReady()
                refreshWindowLayouts()
                context.executor.refreshNumberBadges()
                for window in allWindows { window.executor.showWorkspaceChooser() }
            }
        }

        private func configureWorkspaceBridge() {
            _ = settingsTextConfirmation
            settingsText.expand = { [weak self] field in self?.expandPrompt(field) }
            settingsText.collapse = { [weak self] in self?.returnPromptToSidebar() }
            settingsText.close = { [weak self] in self?.closePromptPane() }
            guard !configured else { return }
            configured = true
            model.prepareSessionCreation = { [weak self] in
                guard let self, let target = self.executor.quickCreateContext() else { return nil }
                let paneID = UUID().uuidString.lowercased()
                let owner = self.executor
                return SessionCreationContext(validate: { [weak owner] count in
                    owner?.reserveSessionPanes(id: paneID, workspaceID: target.workspaceID, tabID: target.tabID, count: count)
                        ?? (owner == nil ? "The original window is unavailable." : nil)
                }, complete: { [weak self] sessions in
                    self?.completeQuickCreateBatch(workspaceID: target.workspaceID, tabID: target.tabID, paneID: paneID, sessions: sessions)
                }, release: { [weak owner] in owner?.releaseSessionPanes(id: paneID) }, contains: { [weak owner] identity in
                    guard let view = owner?.layoutView(), view.workspaceID == target.workspaceID,
                          let tab = view.tab(target.tabID) else { return false }
                    return tab.panes.contains { $0.executionTargetID == identity.executionTargetID && $0.tmuxSessionID == identity.tmuxSessionID }
                })
            }
            lgpWorkspace.chooseIdeaSession = { [weak self] workspaceID, pane, view in
                guard let self, let context=self.context(workspaceID:workspaceID) else { return }
                context.applicationShortcuts.bindCodexSelection = { [weak self] pane, view, owner, endpoint, thread, title in
                    self?.lgpWorkspace.bindIdeaSession(pane:pane,view:view,owner:owner,endpoint:endpoint,thread:thread,title:title)
                }
                context.applicationShortcuts.send(.bindIdeaCodex(pane:pane,view:view))
            }
            lgpWorkspace.dictate.showVoiceSettings = { [weak self] in self?.openSettingsRoute(.init(page:"voice")) }
            lgpWorkspace.manageConnections = { [weak self] in self?.openSettingsRoute(.init(page:"connections")) }
            lgpWorkspace.focusInputSource = { [weak self] window,pane in self?.context(for:window)?.executor.focusInputSource(paneID:pane,window:window) }
            lgpWorkspace.terminalSelection = { [weak self] window, pane in
                self?.context(for: window)?.executor.terminalSelection(paneID: pane, window: window)
            }
            lgpWorkspace.retryHost = { [weak self] in self?.model.retryBackend() }
            lgpWorkspace.sendSession = { [weak self] data in
                guard let self else {throw PolarTmuxLocalIPCError.connectionClosed}
                try await self.model.sendStationSession(data)
            }
            lgpWorkspace.sendFrame = { [weak self] data in
                guard let self else { throw PolarTmuxLocalIPCError.connectionClosed }
                try await self.model.sendExtensionFrame(data)
            }
            lgpWorkspace.workspaceIDForWindow = { [weak self] window in self?.context(for: window)?.executor.workspaceID }
            lgpWorkspace.ensureTerminalVisible = { [weak self] workspaceID, paneID, target in
                guard let self, let context = self.context(workspaceID:workspaceID) else { throw PolarTmuxLocalIPCError.connectionClosed }
                try await context.executor.ensureTerminalVisible(sourcePaneID:paneID,target:target)
            }
            lgpWorkspace.openPane = { [weak self] workspaceID, content, paneID in
                guard let self, let context = self.context(workspaceID: workspaceID) else { throw PolarTmuxLocalIPCError.connectionClosed }
                try await context.executor.openConversation(content, replacing: paneID)
            }
            lgpWorkspace.saveSendMode = { [weak self] mode in
                guard let self, var appearance = self.model.settingsProjection?.appearance else { return }
                appearance.chatSendMode = mode
                self.model.updateAppearanceSettings(appearance)
            }
            Task { [weak self] in
                guard let self else { return }
                await self.model.setExtensionInputHandler { [weak self] data in
                    await MainActor.run { self?.lgpWorkspace.receive(data) }
                }
            }
            model.setIntentSink { [weak self] intent in
                self?.handlePlatformIntent(intent)
            }
            model.inboxOriginSource = { [weak self] key in self?.allWindows.flatMap { $0.executor.inboxOrigins(for: key) } ?? [] }
            model.visibleLayoutWorkspaces = { [weak self] in
                self?.allWindows.filter { $0.presentation.body.layoutMapVisible && $0.activeContainer?.isPolarStationPaneVisible == true && $0.activeContainer?.window?.isVisible == true }
                    .compactMap { $0.executor.workspaceID } ?? []
            }

            configureExecutor(primaryWindow)
            model.setProjectionSink { [weak self] projection in
                guard let self else { return }
                for context in allWindows {
                    context.executor.apply(projection)
                    context.executor.refreshNumberBadges()
                }
                refreshWindowLayouts()
                scheduleStartupFullscreenIfReady()
                releasePendingAuxiliaryPresentationIfReady()
            }
            model.setSessionFeedSink { [weak self] update in
                guard let self else { return }
                for context in allWindows { context.executor.apply(update) }
                refreshWindowLayouts()
            }
            model.setWorkspaceRelaySink { [weak self] request in
                guard let self else {
                    return ControlCenterStore
                        .unavailableWorkspaceRelayResponse(request)
                }
                let scopedID: String?
                if case let .object(fields) = request,
                   case let .object(scope)? = fields["scope"],
                   case let .string(id)? = scope["workspace_id"] { scopedID = id } else { scopedID = nil }
                let target = scopedID.flatMap { self.context(workspaceID: $0) } ?? activeWindow
                let response = await target.executor.performWorkspaceRelay(request)
                refreshWindowLayouts()
                return response
            }
            model.setNativeIntentSink { [weak self] operation, completion in
                self?.performLocalWorkspaceOperation(
                    operation,
                    completion: completion
                )
            }
            model.setInboxDeliverySink { [weak self] origin, text, mode, completion in
                guard let self else {
                    completion("The native Pane runtime is unavailable.")
                    return
                }
                Task { @MainActor in
                    do {
                        guard let context = self.context(workspaceID: origin.workspaceID) else { throw PolarStationNativeExecutorError.workspaceUnavailable }
                        try await context.executor.deliverInboxDraft(origin: origin, text: text, mode: mode)
                        self.refreshWindowLayouts()
                        completion(nil)
                    } catch {
                        completion(error.localizedDescription)
                    }
                }
            }
            model.setInboxPageSink { [weak self] page in
                for context in self?.allWindows ?? [] { context.executor.captureInboxRecoverySnapshots(page) }
            }
        }

        private func requestAuxiliaryPresentation(
            _ surface: AuxiliarySurfaceIDV1
        ) {
            if surface == .resources { openSettingsRoute(.init(page:"resources")); return }
            showAuxiliary(surface)
            guard model.projection != nil else {
                pendingAuxiliaryPresentation = surface
                model.prepareForPresentation()
                return
            }
            model.send(.openAuxiliary(surface: surface))
        }

        private func releasePendingAuxiliaryPresentationIfReady() {
            guard model.projection != nil,
                  let surface = pendingAuxiliaryPresentation
            else { return }
            pendingAuxiliaryPresentation = nil
            model.send(.openAuxiliary(surface: surface))
        }

        private func handlePlatformIntent(_ intent: ControlCenterIntentV1) {
            switch intent.operation {
            case let .openAuxiliary(surface):
                showAuxiliary(surface)
            case let .closeAuxiliary(surface):
                auxiliaryWindows.hide(surface)
            default:
                break
            }
        }

        private func showAuxiliary(_ surface: AuxiliarySurfaceIDV1, on sourceScreen: NSScreen? = nil) {
            if surface == .resources { openSettingsRoute(.init(page:"resources")); return }
            let projectedDescriptor: AuxiliarySurfaceProjectionV1?
            switch surface {
            case .resources:
                projectedDescriptor =
                    model.projection?.auxiliarySurfaces.resources
                model.loadResources()
            case .iphone:
                projectedDescriptor =
                    model.projection?.auxiliarySurfaces.iphone
                model.loadCompanionPairing()
            }
            auxiliaryWindows.show(
                projectedDescriptor ?? fallbackAuxiliaryDescriptor(surface),
                on: sourceScreen ?? preferredAuxiliaryScreen()
            )
        }

        private func fallbackAuxiliaryDescriptor(
            _ surface: AuxiliarySurfaceIDV1
        ) -> AuxiliarySurfaceProjectionV1 {
            switch surface {
            case .resources:
                .init(
                    id: .resources,
                    title: "Global Resources",
                    state: "loading",
                    message: "Waiting for the Polar Station backend"
                )
            case .iphone:
                .init(
                    id: .iphone,
                    title: "iPhone Companion",
                    state: "loading",
                    message: "Waiting for the Polar Station backend"
                )
            }
        }

        private func preferredAuxiliaryScreen() -> NSScreen? {
            statusItem?.button?.window?.screen
                ?? selectedControllerForControlCenter()?.window?.screen
                ?? NSScreen.main
        }

        private func scheduleStartupFullscreenIfReady() {
            guard primaryWindow.executor.startupPresentationIsReady else { return }
            primaryWindow.executor.requestStartupFullscreen()
        }

        func nativeSplitResizeEnded() {
            guard let window=NSApp.keyWindow, let context=context(for:window) else{return}
            context.executor.saveSplitRatios()
        }

        private func performShortcut(
            _ action: PolarStationShortcutAction
        ) -> Bool {
            switch action {
            case .controlCenterToggle:
                toggleControlCenter(nil)
                return true
            case .workspaceModeToggle:
                guard let tabID = model.nativeLayoutView?.selectedTabID
                else { return false }
                performLocalWorkspaceOperation(.toggleDisplayMode(tabID: tabID))
                return true
            case .focusPrevious:
                return activateFocusTarget(.previous)
            case .focusNext:
                return activateFocusTarget(.next)
            case .previousTab, .nextTab:
                guard let layout = model.nativeLayoutView,
                      let index = layout.tabs.firstIndex(where: { $0.tabID == layout.selectedTabID }),
                      layout.tabs.count > 1 else { return true }
                let offset = action == .previousTab ? -1 : 1
                let next = (index + offset + layout.tabs.count) % layout.tabs.count
                performLocalWorkspaceOperation(.selectTab(tabID: layout.tabs[next].tabID))
                return true
            case let .focusPaneNumber(number):
                guard let paneID = executor.numberedPaneTarget(number) else {
                    return false
                }
                performLocalWorkspaceOperation(.activatePane(paneID: paneID))
                return true
            case .closeFocusedPane:
                if let pane = executor.quickCreateContext()?.source {
                    performLocalWorkspaceOperation(.removePane(paneID: pane.id))
                    return true
                }
                return true
            case .newShell: return beginQuickCreate(.shell)
            case .newCodex: return beginQuickCreate(.codex)
            case .resumeCodex: return beginQuickCreate(.codex, resume: true)
            case .openInquiry:
                guard let window = NSApp.mainWindow ?? NSApp.keyWindow else { return false }
                return lgpWorkspace.openInquiry(in: window)
            case .insertIdeaNode:
                guard let window = NSApp.keyWindow else { return false }
                return lgpWorkspace.insertIdeaNode(in: window)
            case .editIdeaInstruction:
                guard let window = NSApp.keyWindow else { return false }
                return lgpWorkspace.editIdeaInstruction(in: window)
            case .openFileReference:
                guard let selected=executor.selectedResourceFile() else{return false}
                return openResourceFile(selected.value,surface:selected.surface)
            case .quoteSelection:
                guard let window = NSApp.keyWindow else { return false }
                return lgpWorkspace.quoteSelection(in: window)
            case .openIdeaTree:
                Task { @MainActor in
                    do { try await executor.openConversation(.init(viewID: UUID().uuidString.lowercased())) }
                    catch { model.presentLocalError(error) }
                }
                return true
            case .newWorkspace:
                return presentShortcutEditor(
                    actionID: "workspace.create",
                    command: .newWorkspace
                )
            case .renameWorkspace:
                return presentShortcutEditor(
                    actionID: "workspace.rename",
                    command: .renameWorkspace
                )
            case .refreshSessions:
                guard projectedAction("sessions.refresh")?.enabled == true else {
                    return false
                }
                model.send(.invokeProjectedAction(actionID: "sessions.refresh"))
                return true
            }
        }

        private func beginQuickCreate(_ kind: QuickSessionKindV1, resume: Bool = false) -> Bool {
            guard let target = executor.quickCreateContext() else { return false }
            guard target.paneCount < 9 else {
                model.presentLocalError(NSError(domain: "PolarStation", code: 1, userInfo: [NSLocalizedDescriptionKey: "This Tab already has nine Panes."]))
                revealQuickCreateError()
                return true
            }
            let shortcuts = activeWindow.applicationShortcuts
            DispatchQueue.main.async { shortcuts.send(resume ? .resumeCodex : .newSession(kind == .codex ? "codex" : "shell")) }
            return true
        }

        private func completeQuickCreate(workspaceID: String, tabID: String, paneID: String, session: SessionRowProjectionV1.Identity) {
            completeQuickCreateBatch(workspaceID: workspaceID, tabID: tabID, paneID: paneID, sessions: [session])
        }

        private func completeQuickCreateBatch(workspaceID: String, tabID: String, paneID: String, sessions: [SessionRowProjectionV1.Identity]) {
            guard let context = context(workspaceID: workspaceID) else { return }
            pendingLocalWorkspaceOperations.append(.init(window: context, generation: context.executor.windowGeneration, operation: nil,
                creation: .init(workspaceID: workspaceID, tabID: tabID, paneID: paneID, sessions: sessions),
                completion: { [weak self] _ in self?.quickCreateTabs.remove("\(workspaceID)/\(tabID)") }))
            sendNextLocalWorkspaceOperation()
        }

        private func revealQuickCreateError() {
            if activeContainer?.isPolarStationPaneVisible != true { toggleControlCenter(nil) }
        }

        private func activateFocusTarget(
            _ direction: PolarTmuxFocusCycleDirection
        ) -> Bool {
            guard let paneID = executor.focusCycleTarget(direction) else {
                return false
            }
            performLocalWorkspaceOperation(.activatePane(paneID: paneID))
            return true
        }

        private func dispatchControlCenterIntent(
            _ intent: ControlCenterIntentV1,
            in context: PolarStationWindowContext? = nil
        ) {
            switch intent.operation {
            case .invokeProjectedAction(actionID: "workspace.window.new"):
                do { try (context ?? activeWindow).executor.authorizeLifecycle("new_window") }
                catch { model.presentLocalError(error); return }
                if let ghostty = startupGhostty {
                    pendingUnboundWindows += 1
                    _ = TerminalController.newWindow(ghostty)
                }
                return
            case .invokeProjectedAction(actionID: "window.close"), .invokeProjectedAction(actionID: "workspace.detach"),
                 .invokeConfirmedAction(actionID: "workspace.detach", confirmationID: _):
                let window = context ?? activeWindow
                if let native = TerminalController.all.first(where: { window.executor.owns($0) })?.window {
                    _ = closeManagedWindow(native)
                }
                return
            case .createSession, .createCodexSession:
                let window = context ?? activeWindow
                guard let target = window.executor.quickCreateContext() else {
                    model.reportNativeWorkspaceFailure("The originating Workspace is unavailable.")
                    return
                }
                let paneID = UUID().uuidString.lowercased()
                model.sendSessionCreation(intent) { [weak self] session in
                    self?.completeQuickCreate(workspaceID: target.workspaceID, tabID: target.tabID, paneID: paneID, session: session)
                }
                return
            default: break
            }
            if intent.operation.requiresNativeWorkspaceMutation {
                performLocalWorkspaceOperation(intent.operation, in: context)
            } else {
                if case let .openAuxiliary(surface) = intent.operation {
                    showAuxiliary(surface)
                }
                model.send(intent)
            }
        }

        private func performLocalWorkspaceOperation(
            _ operation: ControlCenterIntentOperationV1,
            in context: PolarStationWindowContext? = nil,
            completion: (@MainActor (String?) -> Void)? = nil
        ) {
            guard !quitRequested else { completion?("Workspace saves are finishing before Quit."); return }
            let window = context ?? activeWindow
            if case let .selectWorkspace(id) = operation,
               let existing = self.context(workspaceID: id), existing !== window,
               let native = TerminalController.all.first(where: { existing.executor.owns($0) })?.window {
                presentExplicitly(native)
                completion?(nil)
                return
            }
            let hydrated = model.prepareNativeWorkspaceOperation(operation, layout: window.executor.layoutView())
            pendingLocalWorkspaceOperations.append(.init(
                window: window, generation: window.executor.windowGeneration, operation: hydrated,
                completion: completion
            ))
            sendNextLocalWorkspaceOperation()
        }

        private func sendNextLocalWorkspaceOperation() {
            if !localWorkspaceOperationInFlight && pendingLocalWorkspaceOperations.isEmpty { finishQuitIfDrained(); return }
            guard !localWorkspaceOperationInFlight,
                  !pendingLocalWorkspaceOperations.isEmpty
            else { return }
            let pending = pendingLocalWorkspaceOperations.removeFirst()
            let executor = pending.window.executor
            guard !pending.requireWindow || (executor.initialControllerIsAttached && executor.windowGeneration == pending.generation) else {
                pending.workCompletion?(PolarStationNativeExecutorError.workspaceChanged)
                pending.completion?("The originating window has closed.")
                sendNextLocalWorkspaceOperation()
                return
            }
            localWorkspaceOperationInFlight = true
            Task {
                var failure: String?
                do {
                    try await StationMutationScope.$active.withValue(true) {
                    if let work = pending.work { try await work() }
                    else if let created = pending.creation {
                        try await executor.appendCreatedSessions(workspaceID: created.workspaceID, tabID: created.tabID, sessions: created.sessions, reservationID: created.paneID)
                    } else if let operation = pending.operation {
                        try await executor.performImmediate(operation)
                    }
                    }
                    pending.workCompletion?(nil)
                    // Metadata/feed jobs may refresh presentation, but must not
                    // enqueue another metadata job merely by completing.
                    refreshWindowLayouts(rememberWindows: pending.requireWindow)
                } catch {
                    pending.workCompletion?(error)
                    failure = error.localizedDescription
                    refreshWindowLayouts(rememberWindows: pending.requireWindow)
                    model.presentLocalError(error)
                    if pending.creation != nil { revealQuickCreateError() }
                    else if pending.completion == nil,
                            executor.windowGeneration == pending.generation,
                            let window = TerminalController.all.first(where: {
                                executor.owns($0)
                            })?.window {
                        let alert = NSAlert()
                        alert.messageText = "Workspace"
                        alert.informativeText = error.localizedDescription
                        alert.beginSheetModal(for: window) { _ in }
                    }
                    NSLog(
                        "Polar Station local Workspace command failed: %@",
                        error.localizedDescription
                    )
                }
                pending.completion?(failure)
                localWorkspaceOperationInFlight = false
                sendNextLocalWorkspaceOperation()
            }
        }

        private func projectedAction(_ id: String) -> ActionProjectionV1? {
            model.projection?.projectedActions.first { $0.id == id }
        }

        private func presentShortcutEditor(
            actionID: String,
            command: ApplicationShortcutEditorCommand
        ) -> Bool {
            guard projectedAction(actionID)?.enabled == true else { return false }
            guard selectedControllerForControlCenter()?.window != nil else {
                return false
            }
            if activeContainer?.isPolarStationPaneVisible != true {
                toggleControlCenter(nil)
            }
            let shortcuts = activeWindow.applicationShortcuts
            DispatchQueue.main.async { shortcuts.send(command) }
            return true
        }

        private func controlCenterPaneHost(for supplied: PolarStationWindowContext? = nil)
            -> NSHostingView<PolarStationPaneRoot> {
            let context = supplied ?? activeWindow
            if let paneHost = context.paneHost { return paneHost }
            let view = NSHostingView(
                rootView: PolarStationPaneRoot(
                    windowContext: context, model: model,
                    applicationShortcuts: context.applicationShortcuts,
                    presentation: context.presentation,
                    handlePresentationAction: { [weak self] action in
                        self?.handlePresentationAction(action, in: context)
                    },
                    dispatchIntent: { [weak self] intent in
                        self?.dispatchControlCenterIntent(intent, in: context)
                    },
                    performLocalWorkspaceOperation: { [weak self] operation in
                        self?.performLocalWorkspaceOperation(operation, in: context)
                    },
                )
            )
            context.settingsNavigation.onPresent = { [weak self, weak context] in
                guard let self, let context else { return }
                self.handlePresentationAction(.selectTab(.settings), in: context)
            }
            context.paneHost = view
            return view
        }

        private func handlePresentationAction(
            _ action: ControlCenterPresentationAction,
            in context: PolarStationWindowContext
        ) {
            let presentation = context.presentation
            if case let .selectTab(tab) = action, presentation.body.selectedTab == tab { return }
            let window = context.activeContainer?.window
            let available = window?.styleMask.contains(.fullScreen) == true || window?.isZoomed == true
                ? window?.contentView?.bounds.width : window?.screen?.visibleFrame.width
            guard presentation.send(action, availableWidth: available ?? .greatestFiniteMagnitude) else {
                model.presentLocalError(NSError(domain: "PolarStation", code: 1, userInfo: [NSLocalizedDescriptionKey: "There is not enough room for another panel. Close a panel or enlarge the window."]))
                return
            }
            if case .selectTab(.inbox) = action {
                model.loadInbox()
            }
            let shouldPersist: Bool
            switch action {
            case let .selectTab(tab):
                context.activeContainer?.window?.makeFirstResponder(nil)
                if tab != .settings { context.settingsNavigation.showLayout() }
                context.executor.savePanelVisibility(
                    layout: presentation.body.selectedTab != .inbox,
                    inbox: presentation.body.inboxVisible
                )
                shouldPersist = false
            case let .resize(_, commit):
                shouldPersist = commit
            case .toggleSessions:
                shouldPersist = false
            }
            if shouldPersist {
                let current = model.settingsProjection?.appearance
                model.updateAppearanceSettings(.init(
                    layoutMapWidth: UInt16(presentation.body.mapWidth),
                    layoutMapVisible: presentation.body.layoutMapVisible,
                    inboxVisible: presentation.body.inboxVisible,
                    inboxWidth: nil,
                    terminalDeliveryMode: current?.terminalDeliveryMode ?? .insert,
                    interfaceTextScalePercent: current?.interfaceTextScalePercent ?? 100,
                    chatSendMode: current?.chatSendMode ?? .enter,
                    lgpCache: current?.lgpCache ?? .init()
                ))
            }
            context.activeContainer?.updatePolarStationPaneWidth(
                presentation.body.preferredWidth
            )
        }

        private func loadMapWidth(for window: NSWindow) {
            guard !widthPreferenceLoaded else { return }
            let visibleWidth = max(
                window.screen?.visibleFrame.width ?? window.frame.width,
                1
            )
            let selected = model.settingsProjection?.appearance.layoutMapWidth
                .map(CGFloat.init)
                ?? widthPreferences.loadOrDefault(visibleLogicalWidth: visibleWidth)
            presentation.applyWidth(selected)
            presentation.restoreLegacyTab(inboxVisible: executor.panelVisibility.inbox)
            widthPreferenceLoaded = true
        }

        private func applySettingsProjection(_ projection: SettingsProjectionV1) {
            StationTheme.shared.reloadTerminal = { [weak self] in self?.startupGhostty?.reloadConfig(soft: false) }
            StationTheme.shared.update(projection)
            lgpWorkspace.setSendMode(projection.appearance.chatSendMode)
            lgpWorkspace.setCachePolicy(projection.appearance.lgpCache)
            guard let window = selectedControllerForControlCenter()?.window else {
                return
            }
            let visibleWidth = max(
                window.screen?.visibleFrame.width ?? window.frame.width,
                1
            )
            let selected = projection.appearance.layoutMapWidth
                .map(CGFloat.init)
                ?? ControlCenterWidthPolicy.adaptiveDefault(
                    visibleLogicalWidth: visibleWidth
                )
            presentation.applyWidth(selected)
            widthPreferenceLoaded = true
            activeContainer?.updatePolarStationPaneWidth(
                presentation.body.preferredWidth
            )
        }

        @objc private func openLgpConversation(_ sender: Any?) {
            lgpWorkspace.showBrowser(relativeTo: selectedControllerForControlCenter()?.window)
        }

        private func installNativeEntries() {
            if let menu = NSApp.mainMenu?.items.first?.submenu,
               !menu.items.contains(where: { $0.action == #selector(openLocalDiagnostics(_:)) }) {
                let local = NSMenuItem(title: "Local Diagnostics", action: #selector(openLocalDiagnostics(_:)), keyEquivalent: "")
                let remote = NSMenuItem(title: "Remote Hosts", action: #selector(openRemoteDiagnostics(_:)), keyEquivalent: "")
                local.target = self; remote.target = self
                menu.insertItem(local, at: min(3, menu.items.count))
                menu.insertItem(remote, at: min(4, menu.items.count))
            }
            if let applicationMenu = NSApp.mainMenu?.items.first?.submenu,
               !applicationMenu.items.contains(where: { $0.action == #selector(showSettingsFromMenu(_:)) }) {
                let item = NSMenuItem(title: "Settings…", action: #selector(showSettingsFromMenu(_:)), keyEquivalent: ",")
                item.target = self
                applicationMenu.insertItem(item, at: min(2, applicationMenu.items.count))
            }
            if let menu = NSApp.mainMenu?.items.first?.submenu,
               !menu.items.contains(where: { $0.action == #selector(openLgpConversation(_:)) }) {
                let item = NSMenuItem(title: "Open Polar Conversation…", action: #selector(openLgpConversation(_:)), keyEquivalent: "l")
                item.keyEquivalentModifierMask = [.command, .shift]; item.target = self
                menu.insertItem(item, at: min(3, menu.items.count))
            }
            let status = NSStatusBar.system.statusItem(
                withLength: NSStatusItem.variableLength
            )
            guard let button = status.button else {
                NSStatusBar.system.removeStatusItem(status)
                return
            }
            button.target = self
            button.action = #selector(toggleControlCenter(_:))
            button.sendAction(
                on: [.leftMouseDown, .rightMouseDown]
            )
            button.toolTip = "Polar Station"
            button.setAccessibilityLabel("Polar Station")
            button.setAccessibilityIdentifier("polar-station-status-item")
            button.title = "Polar"
            button.imagePosition = .imageLeading
            button.imageScaling = .scaleProportionallyDown
            if let image = Bundle.main.image(
                forResource: "PolarStationStatusTemplate"
            ) {
                image.isTemplate = true
                image.size = NSSize(width: 18, height: 18)
                button.image = image
            } else if let image = NSImage(
                systemSymbolName: "pawprint.fill",
                accessibilityDescription: "Polar Station"
            ) {
                image.isTemplate = true
                image.size = NSSize(width: 18, height: 18)
                button.image = image
            }
            button.needsDisplay = true
            statusItem = status
            status.autosaveName = "PolarStationStatusItem"
            status.isVisible = true
        }

        private func startHost() -> Bool {
            let helper = Bundle.main.bundleURL
                .appendingPathComponent("Contents/Helpers/polar-tmux")
            guard let terminfo = Bundle.main.resourceURL?
                .appendingPathComponent("terminfo", isDirectory: true),
                FileManager.default.fileExists(atPath: terminfo.path),
                FileManager.default.isExecutableFile(atPath: helper.path)
            else {
                model.hostStartupFailed(PolarTmuxLocalIPCError.connectionFailed(
                    "Host startup: bundled helper is not executable or bundled terminfo is missing (\(helper.path))"
                ))
                return false
            }
            let support = FileManager.default.urls(
                for: .applicationSupportDirectory,
                in: .userDomainMask
            )[0].appendingPathComponent("Polar Station", isDirectory: true)
            try? FileManager.default.createDirectory(
                at: support.appendingPathComponent("state", isDirectory: true),
                withIntermediateDirectories: true
            )
            let process = Process()
            process.executableURL = helper
            process.environment = ProcessInfo.processInfo.environment.merging(
                [
                    "TERMINFO": terminfo.path,
                ],
                uniquingKeysWith: { _, managed in managed }
            )
            process.arguments = [
                "host", "--listen", "127.0.0.1:47820",
                "--parent-lease-stdin", "--runtime-data-root",
                support.appendingPathComponent("runtime").path,
                "--target-config", support.appendingPathComponent("targets.json").path,
            ]
            let lease = Pipe()
            process.standardInput = lease
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            process.terminationHandler = { [weak self] terminated in
                DispatchQueue.main.async { [weak self, weak terminated] in
                    guard let self, let terminated else { return }
                    guard hostProcess === terminated else { return }
                    hostProcess = nil
                    hostLease?.closeFile()
                    hostLease = nil
                    let abnormalExit =
                        terminated.terminationReason != .exit ||
                        terminated.terminationStatus != 0
                    guard !stopping, abnormalExit else { return }
                    scheduleHostRestart()
                }
            }
            do {
                try process.run()
                lease.fileHandleForReading.closeFile()
                hostProcess = process
                hostLease = lease.fileHandleForWriting
                DispatchQueue.main.asyncAfter(deadline: .now() + 5.0) {
                    [weak self, weak process] in
                    guard let self, let process,
                          hostProcess === process
                    else { return }
                    hostRestartAttempt = 0
                }
                return true
            } catch {
                lease.fileHandleForReading.closeFile()
                lease.fileHandleForWriting.closeFile()
                model.hostStartupFailed(error)
                return false
            }
        }

        private func scheduleHostRestart() {
            guard !stopping, hostProcess == nil else { return }
            let delayIndex = min(
                hostRestartAttempt,
                Self.hostRestartDelays.count - 1
            )
            let delay = Self.hostRestartDelays[delayIndex]
            hostRestartAttempt = min(
                hostRestartAttempt + 1,
                Self.hostRestartDelays.count - 1
            )
            DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
                [weak self] in
                guard let self, !stopping, hostProcess == nil else { return }
                if !startHost() { scheduleHostRestart() }
            }
        }
    }
#endif
