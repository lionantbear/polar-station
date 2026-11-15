import PolarTmuxHostClient
import SwiftUI

private struct WorkspaceChoice: Equatable, Identifiable {
    let id: String
    let name: String
    let selected: Bool
    let archived: Bool
}

public struct ControlHeaderView: View {
    public let projection: ControlHeaderProjectionV1
    public let deleteWorkspaceAction: ActionProjectionV1?
    public let newWorkspaceWindowAction: ActionProjectionV1?
    public let nativeWorkspaceView: NativeWorkspaceLayoutView?
    public let attentionBadge: AttentionBadgeProjectionV1
    public let controlRevision: UInt64
    public let backendAvailability: PolarStationBackendAvailability
    @ObservedObject private var applicationShortcuts:
        ApplicationShortcutCommandState
    public let emitIntent: @MainActor (ControlCenterIntentV1) -> Void
    public let submitSSHTarget:
        @MainActor (
            ControlCenterIntentV1,
            @escaping @MainActor (String?) -> Void
        ) -> Void
    @State private var workspaceNameDraft = ""
    @State private var tabTitleDraft = ""
    @State private var renamingWorkspace = false
    @State private var addingTab = false
    public let openConnections: @MainActor () -> Void
    @State private var awaitingDetachConfirmation = false
    @State private var awaitingDeleteConfirmation = false
    @State private var archiveTarget: String?

    public init(
        projection: ControlHeaderProjectionV1,
        nativeWorkspaceView: NativeWorkspaceLayoutView? = nil,
        deleteWorkspaceAction: ActionProjectionV1? = nil,
        newWorkspaceWindowAction: ActionProjectionV1? = nil,
        attentionBadge: AttentionBadgeProjectionV1 = .init(),
        controlRevision: UInt64,
        backendAvailability: PolarStationBackendAvailability = .starting,
        applicationShortcuts: ApplicationShortcutCommandState = .init(),
        emitIntent: @escaping @MainActor (ControlCenterIntentV1) -> Void,
        openConnections: @escaping @MainActor () -> Void = {},
        submitSSHTarget:
        @escaping @MainActor (
            ControlCenterIntentV1,
            @escaping @MainActor (String?) -> Void
        ) -> Void
    ) {
        self.projection = projection
        self.deleteWorkspaceAction = deleteWorkspaceAction
        self.newWorkspaceWindowAction = newWorkspaceWindowAction
        self.nativeWorkspaceView = nativeWorkspaceView
        self.attentionBadge = attentionBadge
        self.controlRevision = controlRevision
        self.backendAvailability = backendAvailability
        self.applicationShortcuts = applicationShortcuts
        self.openConnections = openConnections
        self.emitIntent = emitIntent
        self.submitSSHTarget = submitSSHTarget
    }

    public var body: some View {
        HStack(spacing: 4) {
            workspacePicker

            connectionBadge(projection.connection, attentionBadge: attentionBadge)
            Menu {
                if let renameAction = projection.renameAction {
                    editorButton(
                        renameAction,
                        systemImage: "pencil"
                    ) {
                        workspaceNameDraft = selectedWorkspaceName
                        renamingWorkspace = true
                    }
                }


                editorButton(
                    projection.addTab,
                    systemImage: "plus"
                ) {
                    tabTitleDraft = nextTabTitle
                    addingTab = true
                }

                ProjectedActionButton(
                    action: projection.refresh,
                    systemImage: "arrow.uturn.backward.circle",
                    showsLabel: true,
                    emitIntent: invoke
                )

                if let lifecycleAction = projection.lifecycleAction {
                    ProjectedActionButton(
                        action: lifecycleAction,
                        systemImage: lifecycleAction.enabled
                            ? "play.fill"
                            : "hourglass",
                        showsLabel: true,
                        emitIntent: invoke
                    )
                }
            } label: { Image(systemName: "ellipsis") }
            .help("Workspace actions")

        }
        .padding(.horizontal, 6)
        .padding(.vertical, 7)
        .background(.bar)
        .alert(
            "Rename Workspace",
            isPresented: $renamingWorkspace
        ) {
            TextField("Workspace name", text: $workspaceNameDraft)
            Button("Cancel", role: .cancel) {}
            Button("Rename") {
                emit(.renameWorkspace(name: workspaceNameDraft))
            }
            .disabled(trimmed(workspaceNameDraft).isEmpty)
        } message: {
            Text("Choose the name shown in Polar Station.")
        }
        .onReceive(applicationShortcuts.$request.compactMap { $0 }) { request in
            guard request.command == .renameWorkspace else { return }
            guard projection.renameAction?.enabled == true else { return }
            workspaceNameDraft = selectedWorkspaceName
            renamingWorkspace = true
        }
        .alert("Add Tab", isPresented: $addingTab) {
            TextField("Tab title", text: $tabTitleDraft)
            Button("Cancel", role: .cancel) {}
            Button("Add") {
                emit(.addTab(title: tabTitleDraft))
            }
            .disabled(trimmed(tabTitleDraft).isEmpty)
        } message: {
            Text("The new tab becomes the selected Layout Map tab.")
        }
        .confirmationDialog(deleteWorkspaceAction?.confirmation?.title ?? "Archive Workspace?",
                            isPresented: $awaitingDeleteConfirmation, titleVisibility: .visible) {
            if let action = deleteWorkspaceAction, let confirmation = action.confirmation {
                Button(confirmation.confirmLabel, role: .destructive) {
                    if let archiveTarget { emit(.invokeConfirmedAction(actionID: "workspace.archive", confirmationID: archiveTarget)) }
                }
                Button("Cancel", role: .cancel) {}
            }
        } message: {
            Text(selectedWorkspaceName)
            Text(deleteWorkspaceAction?.confirmation?.message ?? "")
        }
        .confirmationDialog(
            projection.workspaceMenuDetachAction?.confirmation?.title
                ?? "Detach Workspace?",
            isPresented: $awaitingDetachConfirmation,
            titleVisibility: .visible
        ) {
            if let action = projection.workspaceMenuDetachAction,
               let confirmation = action.confirmation
            {
                Button(
                    confirmation.confirmLabel,
                    role: confirmation.destructive ? .destructive : nil
                ) {
                    invoke(action)
                }
                Button("Cancel", role: .cancel) {}
            }
        } message: {
            if let message = projection.workspaceMenuDetachAction?
                .confirmation?.message
            {
                Text(message)
            }
        }
    }

    private var workspacePicker: some View {
        Menu {
            ForEach(workspaceChoices.filter { !$0.archived }, id: \.id) { workspace in
                let displayName = workspace.name
                Button {
                    emitIntent(ControlCenterIntentV1(
                        controlRevision: controlRevision,
                        operation: .selectWorkspace(workspaceID: workspace.id)
                    ))
                } label: {
                    if workspace.selected {
                        Label(displayName, systemImage: "checkmark")
                    } else {
                        Text(displayName)
                    }
                }
                .accessibilityLabel(displayName)
            }
            if workspaceChoices.contains(where: \.archived) {
                Menu("Archived Workspaces") {
                    ForEach(workspaceChoices.filter(\.archived)) { workspace in
                        Button("Restore \(workspace.name)") {
                            emit(.invokeConfirmedAction(actionID: "workspace.unarchive", confirmationID: workspace.id))
                        }
                    }
                }
            }
            Divider()
            if let action = newWorkspaceWindowAction {
                Button(action.label) { invoke(action) }.disabled(!action.enabled)
            }
            if let action = deleteWorkspaceAction {
                Button(action.label) { archiveTarget = nativeWorkspaceView?.workspaceID; awaitingDeleteConfirmation = true }
                    .disabled(!canArchive)
            }
            if let detach = projection.workspaceMenuDetachAction {
                Divider()
                Button {
                    invoke(detach)
                } label: {
                    Label(
                        detach.label,
                        systemImage: "rectangle.portrait.and.arrow.forward"
                    )
                }
                .disabled(!detach.enabled)
                .help(detach.disabledReason ?? detach.accessibilityLabel)
            }
        } label: {
            HStack(spacing: 5) {
                Text(selectedWorkspaceName)
                    .font(.headline)
                    .lineLimit(1)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.caption2.bold())
                    .foregroundStyle(.secondary)
            }
            .frame(
                minWidth: 0,
                maxWidth: 92,
                alignment: .leading
            )
        }
        .menuStyle(.borderlessButton)
        .accessibilityLabel(selectedWorkspaceName)
        .help(selectedWorkspaceName)
    }

    private func connectionBadge(
        _ connection: ConnectionStatusProjectionV1?,
        attentionBadge: AttentionBadgeProjectionV1
    ) -> some View {
        Button {
            openConnections()
        } label: {
            VStack(spacing: 0) {
                Image(systemName: connectionSymbol(connection))
                    .foregroundStyle(connectionColor(connection))
                if attentionBadge.unresolvedRed > 0
                    || attentionBadge.unreadOrange > 0
                {
                    Text("\(attentionBadge.unresolvedRed + attentionBadge.unreadOrange)")
                        .font(.system(size: 8, weight: .bold))
                        .foregroundStyle(
                            attentionBadge.unresolvedRed > 0 ? .red : .orange
                        )
                }
            }
            .font(.system(size: 12, weight: .semibold))
            .padding(.horizontal, 4)
            .frame(minWidth: 30, minHeight: 30)
            .background(
                Color.secondary.opacity(0.12),
                in: RoundedRectangle(cornerRadius: 8)
            )
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("connections-manager")
        .accessibilityLabel(
            connection?.accessibilityLabel ?? "Connection unavailable"
        )
        .help(connection?.accessibilityLabel ?? "Connection unavailable")
    }

    private func connectionSymbol(
        _ connection: ConnectionStatusProjectionV1?
    ) -> String {
        let status = connection?.status.lowercased()
        if status == "connected" || status == "online" {
            if connection?.route == .lan {
                return "network"
            }
            if connection?.route == .wan {
                return "globe"
            }
            return "checkmark.circle.fill"
        }
        switch status {
        case "connecting", "reconnecting", "recovering", "partial":
            return "arrow.triangle.2.circlepath"
        case "disconnected", "offline":
            return "xmark.circle.fill"
        case "inactive":
            return "minus.circle.fill"
        default:
            return "minus.circle.fill"
        }
    }

    private func connectionColor(
        _ connection: ConnectionStatusProjectionV1?
    ) -> Color {
        switch connection?.status.lowercased() {
        case "connected", "online":
            .green
        case "connecting", "reconnecting", "recovering", "partial":
            .orange
        case "disconnected", "offline":
            .red
        case "inactive":
            .secondary
        default:
            .secondary
        }
    }

    private var canArchive: Bool {
        guard let view = nativeWorkspaceView, !view.workspaceID.isEmpty,
              let selected = workspaceChoices.first(where: { !$0.archived })?.id else { return false }
        return (try? decideWorkspaceLifecycle(action: "inspect", ids: workspaceChoices.map(\.id),
            archived: workspaceChoices.filter(\.archived).map(\.id), open: [], selected: selected,
            bound: view.workspaceID).can_archive) ?? false
    }

    private var selectedWorkspaceName: String {
        nativeWorkspaceView?.workspaceName ?? "Workspace"
    }

    private var workspaceChoices: [WorkspaceChoice] {
        if let nativeWorkspaceView,
           !nativeWorkspaceView.workspaceChoices.isEmpty
        {
            return nativeWorkspaceView.workspaceChoices.map {
                WorkspaceChoice(
                    id: $0.workspaceID,
                    name: $0.name,
                    selected: $0.selected, archived: $0.archived
                )
            }
        }
        return []
    }

    private func invoke(_ action: ActionProjectionV1) {
        emit(action.intentOperation)
    }

    private func emit(_ operation: ControlCenterIntentOperationV1) {
        emitIntent(
            ControlCenterIntentV1(
                controlRevision: controlRevision,
                operation: operation
            )
        )
    }

    private func editorButton(
        _ action: ActionProjectionV1,
        systemImage: String,
        perform: @escaping @MainActor () -> Void
    ) -> some View {
        Button(action: perform) {
            Label(action.label, systemImage: systemImage)
        }
        .buttonStyle(.plain)
        .disabled(!action.enabled)
        .accessibilityLabel(action.accessibilityLabel)
        .help(action.disabledReason ?? action.accessibilityLabel)
    }

    private var nextTabTitle: String {
        "Tab"
    }

    private func trimmed(_ value: String) -> String {
        value.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

public struct WorkspaceSSHTargetEditorView: View {
    // Keep the reviewed identity/revision paired with the editor's draft.
    @State private var projection: WorkspaceSSHTargetEditorProjectionV1
    let controlRevision: UInt64
    let backendAvailability: PolarStationBackendAvailability
    let submitSSHTarget:
        @MainActor (
            ControlCenterIntentV1,
            @escaping @MainActor (String?) -> Void
        ) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var destination: String
    @State private var routeEnabled: Bool
    @State private var lanAddress: String
    @State private var lanPort: String
    @State private var wanHost: String
    @State private var wanPort: String
    @State private var dohPrimary: String
    @State private var dohSecondary: String
    @State private var submitting = false
    @State private var error: String?

    public init(
        projection: WorkspaceSSHTargetEditorProjectionV1,
        controlRevision: UInt64,
        backendAvailability: PolarStationBackendAvailability,
        submitSSHTarget:
        @escaping @MainActor (
            ControlCenterIntentV1,
            @escaping @MainActor (String?) -> Void
        ) -> Void
    ) {
        _projection = State(initialValue: projection)
        self.controlRevision = controlRevision
        self.backendAvailability = backendAvailability
        self.submitSSHTarget = submitSSHTarget
        _destination = State(initialValue: projection.destination)
        _routeEnabled = State(initialValue: projection.routeProfile != nil)
        _lanAddress = State(initialValue: projection.routeProfile?.lanAddress ?? "")
        _lanPort = State(
            initialValue: projection.routeProfile.map { String($0.lanPort) } ?? "22"
        )
        _wanHost = State(initialValue: projection.routeProfile?.wanHost ?? "")
        _wanPort = State(
            initialValue: projection.routeProfile.map { String($0.wanPort) } ?? "22888"
        )
        _dohPrimary = State(
            initialValue: projection.routeProfile?.dohProviders.first ?? "google"
        )
        _dohSecondary = State(
            initialValue: projection.routeProfile?.dohProviders.dropFirst().first ?? "cloudflare"
        )
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(projection.title)
                .font(.headline)
                .font(.subheadline)
                .foregroundStyle(.secondary)
            TextField(
                recovery.destinationLabel,
                text: $destination,
                prompt: Text(projection.destinationPlaceholder)
            )
            .textFieldStyle(.roundedBorder)
            .accessibilityIdentifier(
                "workspace-ssh-target-destination"
            )
            .disabled(submitting)
            Toggle("Use explicit LAN/WAN route", isOn: $routeEnabled)
                .accessibilityIdentifier("workspace-ssh-target-route-enabled")
                .disabled(submitting)
            TextField("LAN address", text: $lanAddress)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("workspace-ssh-target-lan-address")
                .disabled(submitting || !routeEnabled)
            TextField("LAN SSH port", text: $lanPort)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("workspace-ssh-target-lan-port")
                .disabled(submitting || !routeEnabled)
            TextField("WAN DNS host", text: $wanHost)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("workspace-ssh-target-wan-host")
                .disabled(submitting || !routeEnabled)
            TextField("WAN SSH port", text: $wanPort)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("workspace-ssh-target-wan-port")
                .disabled(submitting || !routeEnabled)
            TextField("DoH provider 1", text: $dohPrimary)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("workspace-ssh-target-doh-primary")
                .disabled(submitting || !routeEnabled)
            TextField("DoH provider 2", text: $dohSecondary)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("workspace-ssh-target-doh-secondary")
                .disabled(submitting || !routeEnabled)
            Text(recovery.status)
                .font(.caption)
                .foregroundStyle(.secondary)
            if let error {
                Text(error)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
            HStack {
                Spacer()
                Button(projection.cancelLabel) {
                    dismiss()
                }
                .accessibilityIdentifier(
                    "cancel-workspace-ssh-target"
                )
                .disabled(submitting)
                Button(recovery.saveLabel) {
                    save()
                }
                .accessibilityIdentifier(
                    "save-workspace-ssh-target"
                )
                .keyboardShortcut(.defaultAction)
                .disabled(!recovery.canSubmit || trimmedDestination.isEmpty)
            }
        }
        .padding(20)
        .frame(width: 460)
        .interactiveDismissDisabled(submitting)
    }

    private var trimmedDestination: String {
        destination.trimmingCharacters(
            in: .whitespacesAndNewlines
        )
    }

    private var recovery: PolarStationRecoveryPresentation {
        .init(
            connected: backendAvailability.hostActionsEnabled,
            targetKnown: true,
            pending: submitting
        )
    }

    private func save() {
        guard recovery.canSubmit else { return }
        submitting = true
        error = nil
        let routeProfile = routeEnabled
            ? WorkspaceSSHTargetRouteProfileV1(
                lanAddress: lanAddress.trimmingCharacters(in: .whitespacesAndNewlines),
                lanPort: UInt16(lanPort.trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0,
                wanHost: wanHost.trimmingCharacters(in: .whitespacesAndNewlines),
                wanPort: UInt16(wanPort.trimmingCharacters(in: .whitespacesAndNewlines)) ?? 0,
                dohProviders: [
                    dohPrimary.trimmingCharacters(in: .whitespacesAndNewlines),
                    dohSecondary.trimmingCharacters(in: .whitespacesAndNewlines),
                ].filter { !$0.isEmpty }
            )
            : nil
        let intent = ControlCenterIntentV1(
            controlRevision: controlRevision,
            operation: .setWorkspaceSSHTarget(
                targetID: projection.targetID,
                targetRevision: projection.targetRevision,
                destination: trimmedDestination,
                routeProfile: routeProfile
            )
        )
        submitSSHTarget(intent) { submissionError in
            submitting = false
            if let submissionError {
                error = submissionError
            } else {
                dismiss()
            }
        }
    }
}

extension ActionProjectionV1 {
    var intentOperation: ControlCenterIntentOperationV1 {
        if let confirmation {
            .invokeConfirmedAction(
                actionID: id,
                confirmationID: confirmation.id
            )
        } else {
            .invokeProjectedAction(actionID: id)
        }
    }
}

struct ProjectedActionButton: View {
    let action: ActionProjectionV1
    let systemImage: String
    var showsLabel = true
    var prominent = false
    var plainIcon = false
    let emitIntent: @MainActor (ActionProjectionV1) -> Void

    @State private var awaitingConfirmation = false

    var body: some View {
        Group {
            if prominent {
                projectedButton.buttonStyle(.borderedProminent)
            } else if showsLabel {
                projectedButton.buttonStyle(.bordered)
            } else {
                projectedButton.buttonStyle(.plain)
            }
        }
        .controlSize(.small)
        .disabled(!action.enabled)
        .accessibilityLabel(action.accessibilityLabel)
        .accessibilityAddTraits(action.selected ? .isSelected : [])
        .help(action.disabledReason ?? action.accessibilityLabel)
        .confirmationDialog(
            action.confirmation?.title ?? action.label,
            isPresented: $awaitingConfirmation,
            titleVisibility: .visible
        ) {
            if let confirmation = action.confirmation {
                Button(
                    confirmation.confirmLabel,
                    role: confirmation.destructive ? .destructive : nil
                ) {
                    emitIntent(action)
                }
                Button("Cancel", role: .cancel) {}
            }
        } message: {
            if let message = action.confirmation?.message {
                Text(message)
            }
        }
    }

    private var projectedButton: some View {
        Button(role: action.role == .destructive ? .destructive : nil) {
            if action.confirmation == nil {
                emitIntent(action)
            } else {
                awaitingConfirmation = true
            }
        } label: {
            if showsLabel {
                Label(action.label, systemImage: systemImage)
                    .frame(minWidth: 28, minHeight: 28)
            } else if plainIcon {
                Label(action.label, systemImage: systemImage)
                    .labelStyle(.iconOnly)
            } else {
                Label(action.label, systemImage: systemImage)
                    .labelStyle(.iconOnly)
                    .frame(width: 30, height: 30)
                    .background(
                        Color.secondary.opacity(0.12),
                        in: RoundedRectangle(cornerRadius: 8)
                    )
            }
        }
    }
}
