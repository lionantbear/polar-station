import AppKit
import Foundation
import PolarTmuxHostClient
import SwiftUI

public enum ControlCenterRegion: Equatable, Sendable {
    case header, sessions, workspaceMap, bottomBar
}

public enum ControlCenterWidthRequestDelivery: Equatable {
    case nextMainActorTurn
}

public enum ControlCenterInitialPresentationPolicy {
    public static func preferredWidth(
        sessionsVisible: Bool
    ) -> CGFloat {
        sessionsVisible
            ? ControlCenterPresentation.sessionsWidth
            : ControlCenterPresentation.compactWidth
    }
}

/// A deterministic description of the native shell.
///
/// Keeping these properties independent of SwiftUI lets fixture tests verify
/// region identity and ordering without rebuilding the Rust-owned semantics.
public struct ControlCenterPresentation {
    public static let compactWidth: CGFloat =
        ControlCenterWidthPolicy.maximumMapWidth
    public static let sessionsWidth: CGFloat =
        ControlCenterWidthPolicy.sessionsWidth +
        ControlCenterWidthPolicy.dividerWidth +
        ControlCenterWidthPolicy.maximumMapWidth

    public let projection: ControlCenterProjectionV1
    public let sessionsVisible: Bool

    public init(
        projection: ControlCenterProjectionV1,
        sessionsVisible: Bool = false
    ) {
        self.projection = projection
        self.sessionsVisible = sessionsVisible
    }

    public var componentOrder: [ControlCenterRegion] {
        [.header, .sessions, .workspaceMap, .bottomBar]
    }

    public var visibleRegions: [ControlCenterRegion] {
        componentOrder.filter {
            $0 != .sessions || sessionsVisible
        }
    }

    public var preferredWidth: CGFloat {
        sessionsVisible
            ? Self.sessionsWidth
            : Self.compactWidth
    }

    public var widthRequestDelivery: ControlCenterWidthRequestDelivery {
        .nextMainActorTurn
    }

    @MainActor
    public func deliverPreferredWidth(
        to request: @escaping @MainActor (CGFloat) -> Void
    ) {
        let width = preferredWidth
        DispatchQueue.main.async {
            request(width)
        }
    }

    public var workspaceMapIdentity: String {
        "" // Workspace identity is supplied by the native layout, never Host facts.
    }

    public var bottomActions: [ActionProjectionV1] {
        ControlBottomActionPolicy.visibleActions(
            in: projection.bottomBar
        )
    }

    public var auxiliaryDestinations: [AuxiliarySurfaceIDV1] {
        [
            projection.auxiliarySurfaces.resources.id,
            projection.auxiliarySurfaces.iphone.id,
        ]
    }

    public var accessibilityLabels: [String] {
        [
            projection.header.renameAction,
            projection.header.addTab,
            projection.header.refresh,
            projection.header.lifecycleAction,
        ]
        .compactMap { $0 }
        .map(\.accessibilityLabel)
        + bottomActions.map(\.accessibilityLabel)
    }

    public func intent(
        for action: ActionProjectionV1
    ) -> ControlCenterIntentV1 {
        ControlCenterIntentV1(
            controlRevision: projection.controlRevision,
            operation: action.intentOperation
        )
    }

    public func openAuxiliaryIntent(
        for surface: AuxiliarySurfaceIDV1
    ) -> ControlCenterIntentV1 {
        ControlCenterIntentV1(
            controlRevision: projection.controlRevision,
            operation: .openAuxiliary(surface: surface)
        )
    }
}

/// The native shell owns Sessions and Layout Map visibility together, so
/// backend refreshes and native Tab selection cannot mutate presentation.
public struct ControlCenterBodyPresentation: Equatable, Sendable {
    public let sessionsVisible: Bool
    public let layoutMapVisible: Bool
    public let inboxVisible: Bool
    public let mapWidth: CGFloat
    public let inboxWidth: CGFloat

    public init(
        sessionsVisible: Bool,
        layoutMapVisible: Bool,
        inboxVisible: Bool = false,
        mapWidth: CGFloat = ControlCenterWidthPolicy.maximumMapWidth,
        inboxWidth: CGFloat = ControlCenterWidthPolicy.defaultInboxWidth
    ) {
        self.sessionsVisible = sessionsVisible
        self.layoutMapVisible = layoutMapVisible
        self.inboxVisible = inboxVisible
        self.mapWidth = ControlCenterWidthPolicy.clampMapWidth(mapWidth)
        self.inboxWidth = ControlCenterWidthPolicy.clampInboxWidth(inboxWidth)
    }

    public var dividerVisible: Bool {
        sessionsVisible && layoutMapVisible
    }

    public var preferredWidth: CGFloat {
        ControlCenterWidthPolicy.controllerWidth(
            mapWidth: mapWidth,
            sessionsVisible: sessionsVisible,
            layoutMapVisible: layoutMapVisible,
            inboxVisible: inboxVisible,
            inboxWidth: inboxWidth
        )
    }
}

public struct ControlCenterView: View {
    public let projection: ControlCenterProjectionV1
    public let nativeLayoutView: NativeWorkspaceLayoutView?
    public let presentation: ControlCenterBodyPresentation
    public let backendAvailability: PolarStationBackendAvailability
    public let liveSessions: [PolarTmuxSessionProjection]
    public let inboxOverview: InboxOverviewV1?
    public let inboxMessages: [InboxMessageProjectionV1]?
    public let focusedInboxMessageID: String?
    public let submitInboxReply: @MainActor (InboxMessageProjectionV1, String) -> Void
    public let editInboxDraft: @MainActor (InboxMessageProjectionV1, String) -> Void
    public let approveInboxDraft: @MainActor (InboxMessageProjectionV1) -> Void
    public let emitPresentationAction:
        @MainActor (ControlCenterPresentationAction) -> Void
    public let emitWorkspaceOperation:
        @MainActor (ControlCenterIntentOperationV1) -> Void
    public let emitIntent: @MainActor (ControlCenterIntentV1) -> Void
    public let prepareSessionEditor: SessionEditorPrepare?
    public let sessionCreationRecovery: AgentSessionResumeV1?
    public let readSessionEditor: SessionEditorReader?
    public let requestCodex: CodexRequestHandler?
    public let codexStateRevision: UInt64
    @State private var codexSession: PolarTmuxSessionKey?
    @State private var codexBrowserOpen = false
    @State private var connectionsOpen = false
    public let submitSSHTarget:
        @MainActor (
            ControlCenterIntentV1,
            @escaping @MainActor (String?) -> Void
        ) -> Void
    @ObservedObject private var applicationShortcuts:
        ApplicationShortcutCommandState
    @State private var widthDragStart: CGFloat?
    @State private var inboxWidthDragStart: CGFloat?

    public init(
        projection: ControlCenterProjectionV1,
        nativeLayoutView: NativeWorkspaceLayoutView? = nil,
        presentation: ControlCenterBodyPresentation,
        backendAvailability: PolarStationBackendAvailability = .starting,
        liveSessions: [PolarTmuxSessionProjection] = [],
        inboxMessages: [InboxMessageProjectionV1]? = nil,
        inboxOverview: InboxOverviewV1? = nil,
        focusedInboxMessageID: String? = nil,
        submitInboxReply: @escaping @MainActor (InboxMessageProjectionV1, String) -> Void = { _, _ in },
        editInboxDraft: @escaping @MainActor (InboxMessageProjectionV1, String) -> Void = { _, _ in },
        approveInboxDraft: @escaping @MainActor (InboxMessageProjectionV1) -> Void = { _ in },
        emitPresentationAction:
        @escaping @MainActor (ControlCenterPresentationAction) -> Void,
        emitWorkspaceOperation:
        @escaping @MainActor (ControlCenterIntentOperationV1) -> Void,
        emitIntent: @escaping @MainActor (ControlCenterIntentV1) -> Void,
        submitSSHTarget:
        (@MainActor (
            ControlCenterIntentV1,
            @escaping @MainActor (String?) -> Void
        ) -> Void)? = nil,
        applicationShortcuts: ApplicationShortcutCommandState = .init(),
        requestCodex: CodexRequestHandler? = nil,
        codexStateRevision: UInt64 = 0,
        readSessionEditor: SessionEditorReader? = nil,
        prepareSessionEditor: SessionEditorPrepare? = nil,
        sessionCreationRecovery: AgentSessionResumeV1? = nil
    ) {
        self.prepareSessionEditor = prepareSessionEditor
        self.sessionCreationRecovery = sessionCreationRecovery
        self.readSessionEditor = readSessionEditor
        self.projection = projection
        self.nativeLayoutView = nativeLayoutView
        self.presentation = presentation
        self.backendAvailability = backendAvailability
        self.liveSessions = liveSessions
        self.inboxOverview = inboxOverview
        self.inboxMessages = inboxMessages
        self.focusedInboxMessageID = focusedInboxMessageID
        self.submitInboxReply = submitInboxReply
        self.editInboxDraft = editInboxDraft
        self.approveInboxDraft = approveInboxDraft
        self.emitPresentationAction = emitPresentationAction
        self.emitWorkspaceOperation = emitWorkspaceOperation
        self.emitIntent = emitIntent
        self.submitSSHTarget = submitSSHTarget ?? {
            intent, completion in
            emitIntent(intent)
            completion("The operation result is unavailable. Refresh Sessions before retrying.")
        }
        self.applicationShortcuts = applicationShortcuts
        self.requestCodex = requestCodex
        self.codexStateRevision = codexStateRevision
    }

    private var layoutRegion: some View {
        let bodyPresentation = presentation
        return VStack(spacing: 0) {
            HStack {
                Text("Layout").font(.headline).lineLimit(1)
                if requestCodex != nil {
                    Button { codexBrowserOpen = true } label: { Image(systemName: "bubble.left.and.bubble.right") }
                        .accessibilityLabel("Codex sessions")
                        .help("Codex sessions")
                }
                Spacer()
                Button {
                    emitPresentationAction(.toggleSessions)
                } label: {
                    Image(systemName: "terminal")
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Show or hide Sessions")
                .help("Show or hide Sessions")
                .accessibilityValue(bodyPresentation.sessionsVisible ? "Open" : "Closed")
            }
            .padding(8)
            HStack(spacing: 0) {
                    WorkspaceMapView(
                        controlRevision: projection.controlRevision,
                        nativeView: nativeLayoutView,
                        sessions: projection.sessions.rows,
                        liveSessions: liveSessions,
                        emitIntent: emitIntent
                    )
                    .frame(width: bodyPresentation.mapWidth)
                    .frame(maxHeight: .infinity)
                    .overlay(alignment: .trailing) {
                        Rectangle()
                            .fill(.clear)
                            .frame(width: 8)
                            .contentShape(Rectangle())
                            .onHover { hovering in
                                if hovering {
                                    NSCursor.resizeLeftRight.push()
                                } else {
                                    NSCursor.pop()
                                }
                            }
                            .gesture(
                                DragGesture(minimumDistance: 0, coordinateSpace: .global)
                                    .onChanged { value in
                                        if widthDragStart == nil {
                                            widthDragStart =
                                                bodyPresentation.mapWidth
                                        }
                                        guard let start = widthDragStart else {
                                            return
                                        }
                                        let selected =
                                            ControlCenterWidthPolicy.resizedWidth(
                                                start: start,
                                                translation: value.translation.width
                                            )
                                        if selected != bodyPresentation.mapWidth {
                                            emitPresentationAction(.resizeMap(
                                                width: selected,
                                                commit: false
                                            ))
                                        }
                                    }
                                    .onEnded { _ in
                                        emitPresentationAction(.resizeMap(
                                            width: bodyPresentation.mapWidth,
                                            commit: true
                                        ))
                                        widthDragStart = nil
                                    }
                            )
                            .accessibilityLabel("Resize Layout Map")
                            .help("Drag left or right to resize Layout Map")
                    }

            }
        }
        .frame(width: bodyPresentation.mapWidth)
    }

    public var body: some View {
        let bodyPresentation = presentation
        let liveTabs = NativeWorkspaceLayoutProjection.tabs(
            nativeView: nativeLayoutView,
            sessions: projection.sessions.rows,
            liveSessions: liveSessions
        )

        VStack(spacing: 0) {
            ControlHeaderView(
                projection: projection.header,
                nativeWorkspaceView: nativeLayoutView,
                deleteWorkspaceAction: projection.projectedActions.first { $0.id == "workspace.archive" },
                newWorkspaceWindowAction: projection.projectedActions.first { $0.id == "workspace.window.new" },
                attentionBadge: projection.attentionBadge,
                controlRevision: projection.controlRevision,
                backendAvailability: backendAvailability,
                applicationShortcuts: applicationShortcuts,
                emitIntent: emitIntent,
                openConnections: { connectionsOpen = true },
                submitSSHTarget: submitSSHTarget
            )

            Divider()
                .sheet(isPresented: $connectionsOpen) {
                    ConnectionsView(projection: projection.connections,
                        controlRevision: projection.controlRevision,
                        close: { connectionsOpen = false }, requestCodex: requestCodex,
                        submit: submitSSHTarget)
                }

            HStack(spacing: 0) {
                if bodyPresentation.sessionsVisible {
                    SessionLibraryView(
                        projection: projection.sessions,
                        connections: projection.connections,
                        endpointStates: projection.codexEndpointStates,
                        groups: projection.sessionGroups,
                        agentSessions: projection.agentSessions,
                        codexEndpoints: projection.codexEndpoints,
                        nativeLayoutView: nativeLayoutView,
                        controlRevision: projection.controlRevision,
                        dismiss: { emitPresentationAction(.toggleSessions) },
                        emitIntent: emitIntent,
                        submitOperation: submitSSHTarget,
                        readSessionEditor: readSessionEditor,
                        openCodex: requestCodex == nil ? nil : { @MainActor identity in
                            codexSession = PolarTmuxSessionKey(executionTarget: identity.executionTargetID, tmuxSessionID: identity.tmuxSessionID)
                        },
                        requestCodex: requestCodex,
                        codexRevision: codexStateRevision,
                        applicationShortcuts: applicationShortcuts,
                        prepareEditor: prepareSessionEditor,
                        creationRecovery: sessionCreationRecovery
                    )
                    .frame(width: ControlCenterWidthPolicy.sessionsWidth)
                    Divider()
                }
                if bodyPresentation.layoutMapVisible {
                    layoutRegion
                }

                if bodyPresentation.layoutMapVisible
                    && bodyPresentation.inboxVisible
                {
                    Divider()
                }

                if bodyPresentation.inboxVisible {
                    InboxView(
                        overview: inboxOverview,
                        openTerminalSession: { session in
                            if let pane = liveTabs.flatMap(\.panes).first(where: { $0.executionTargetID == session.executionTarget && $0.tmuxSessionID == session.tmuxSessionID }) {
                                emitWorkspaceOperation(.activatePane(paneID: pane.id))
                            } else {
                                emitWorkspaceOperation(.toggleSessionAssignment(identity: .init(executionTargetID: session.executionTarget, tmuxSessionID: session.tmuxSessionID)))
                            }
                        },
                        requestCodex: requestCodex,
                        codexRevision: codexStateRevision,
                        openCodexSession: { codexSession = $0 },
                        liveMessages: InboxAttentionMessage.current(from: liveTabs),
                        historyMessages: InboxAttentionMessage.persisted(
                            inboxMessages ?? []
                        ),
                        backendAvailability: backendAvailability,
                        focusedMessageID: focusedInboxMessageID,
                        openPane: { message in
                            emitWorkspaceOperation(.activatePane(
                                paneID: message.paneID
                            ))
                        },
                        submitReply: { message, reply in
                            guard let hostMessage = message.hostMessage else { return }
                            submitInboxReply(hostMessage, reply)
                        },
                        editDraft: editInboxDraft,
                        approveDraft: approveInboxDraft
                    )
                    .frame(width: bodyPresentation.inboxWidth)
                    .frame(maxHeight: .infinity)
                    .overlay(alignment: .trailing) {
                        Rectangle()
                            .fill(.clear)
                            .frame(width: 8)
                            .contentShape(Rectangle())
                            .gesture(
                                DragGesture(minimumDistance: 0, coordinateSpace: .global)
                                    .onChanged { value in
                                        if inboxWidthDragStart == nil {
                                            inboxWidthDragStart = bodyPresentation.inboxWidth
                                        }
                                        guard let start = inboxWidthDragStart else { return }
                                        let width = ControlCenterWidthPolicy.clampInboxWidth(
                                            start + value.translation.width
                                        )
                                        emitPresentationAction(.resizeInbox(
                                            width: width,
                                            commit: false
                                        ))
                                    }
                                    .onEnded { _ in
                                        emitPresentationAction(.resizeInbox(
                                            width: bodyPresentation.inboxWidth,
                                            commit: true
                                        ))
                                        inboxWidthDragStart = nil
                                    }
                            )
                            .accessibilityLabel("Resize Inbox")
                    }
                }
            }

            Divider()

            ControlBottomBar(
                projection: projection.bottomBar,
                nativeLayoutView: nativeLayoutView,
                auxiliarySurfaces: projection.auxiliarySurfaces,
                controlRevision: projection.controlRevision,
                layoutVisible: bodyPresentation.layoutMapVisible,
                inboxVisible: bodyPresentation.inboxVisible,
                inboxUnreadCount: inboxUnreadCount,
                applicationShortcuts: applicationShortcuts,
                emitPresentationAction: emitPresentationAction,
                emitWorkspaceOperation: emitWorkspaceOperation,
                emitIntent: emitIntent
            )
        }
        .frame(
            minWidth: ControlCenterWidthPolicy.minimumMapWidth,
            idealWidth: bodyPresentation.preferredWidth,
            minHeight: 440,
            idealHeight: 620
        )
        .background(.regularMaterial)
        .sheet(isPresented: Binding(get: { codexSession != nil }, set: { if !$0 { codexSession = nil } })) {
            if let session = codexSession, let requestCodex {
                CodexSessionControlView(session: session, stateRevision: codexStateRevision, request: requestCodex, dismiss: { codexSession = nil })
            }
        }
        .sheet(isPresented: $codexBrowserOpen) {
            if let requestCodex { CodexSessionsView(request: requestCodex, revision: codexStateRevision, dismiss: { codexBrowserOpen = false }) }
        }
        .accessibilityIdentifier("polar-station-control-center")
    }

    private var inboxUnreadCount: Int {
        let history = inboxMessages?.filter(\.unread).count ?? 0
        let tabs = NativeWorkspaceLayoutProjection.tabs(
            nativeView: nativeLayoutView,
            sessions: projection.sessions.rows,
            liveSessions: liveSessions
        )
        let live = InboxAttentionMessage.current(from: tabs).filter(\.unread).count
        return history + live
    }
}
