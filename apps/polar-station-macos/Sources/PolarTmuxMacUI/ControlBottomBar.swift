import AppKit
import PolarTmuxHostClient
import SwiftUI

public enum ControlBottomActionPolicy {
    public static func visibleActions(
        in projection: ControlBottomBarProjectionV1
    ) -> [ActionProjectionV1] {
        ["presentation.layout.toggle", "presentation.inbox.toggle", "resources.open", "more.open"].compactMap {
            projection.action(id: $0)
        }
    }
}

public struct ControlBottomBar: View {
    @Environment(\.polarStationInterfaceTextScale) private var interfaceTextScale
    public let projection: ControlBottomBarProjectionV1
    public let nativeLayoutView: NativeWorkspaceLayoutView?
    public let auxiliarySurfaces: AuxiliarySurfacesProjectionV1
    public let controlRevision: UInt64
    public let layoutVisible: Bool
    public let inboxVisible: Bool
    public let inboxUnreadCount: Int
    public let emitPresentationAction:
        @MainActor (ControlCenterPresentationAction) -> Void
    public let emitWorkspaceOperation:
        @MainActor (ControlCenterIntentOperationV1) -> Void
    public let emitIntent: @MainActor (ControlCenterIntentV1) -> Void
    @ObservedObject private var applicationShortcuts:
        ApplicationShortcutCommandState

    @State private var creatingWorkspace = false
    @State private var workspaceNameDraft = ""
    @State private var pendingConfirmation: ActionProjectionV1?
    @State private var editingFontSize = false
    @State private var fontSizeDraft = 14

    public init(
        projection: ControlBottomBarProjectionV1,
        nativeLayoutView: NativeWorkspaceLayoutView?,
        auxiliarySurfaces: AuxiliarySurfacesProjectionV1,
        controlRevision: UInt64,
        layoutVisible: Bool,
        inboxVisible: Bool,
        inboxUnreadCount: Int = 0,
        applicationShortcuts: ApplicationShortcutCommandState = .init(),
        emitPresentationAction:
        @escaping @MainActor (ControlCenterPresentationAction) -> Void,
        emitWorkspaceOperation:
        @escaping @MainActor (ControlCenterIntentOperationV1) -> Void,
        emitIntent: @escaping @MainActor (ControlCenterIntentV1) -> Void
    ) {
        self.projection = projection
        self.nativeLayoutView = nativeLayoutView
        self.auxiliarySurfaces = auxiliarySurfaces
        self.controlRevision = controlRevision
        self.layoutVisible = layoutVisible
        self.inboxVisible = inboxVisible
        self.inboxUnreadCount = inboxUnreadCount
        self.applicationShortcuts = applicationShortcuts
        self.emitPresentationAction = emitPresentationAction
        self.emitWorkspaceOperation = emitWorkspaceOperation
        self.emitIntent = emitIntent
    }

    public var body: some View {
        HStack(spacing: 5) {
            ForEach(orderedActions) { action in
                bottomControl(action)
                    .frame(maxWidth: .infinity)
            }
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 6)
        .background(.bar)
        .alert("New Workspace", isPresented: $creatingWorkspace) {
            TextField("Workspace name", text: $workspaceNameDraft)
            Button("Cancel", role: .cancel) {}
            Button("Create") {
                emit(.createWorkspace(name: workspaceNameDraft))
            }
            .disabled(
                workspaceNameDraft
                    .trimmingCharacters(
                        in: .whitespacesAndNewlines
                    )
                    .isEmpty
            )
        } message: {
            Text("Create and select a new Workspace.")
        }
        .onReceive(applicationShortcuts.$request.compactMap { $0 }) { request in
            guard request.command == .newWorkspace else { return }
            guard newWorkspaceAction?.enabled == true else { return }
            workspaceNameDraft = "Workspace"
            creatingWorkspace = true
        }
        .confirmationDialog(
            pendingConfirmation?.confirmation?.title
                ?? pendingConfirmation?.label
                ?? "Confirm",
            isPresented: Binding(
                get: { pendingConfirmation != nil },
                set: { if !$0 { pendingConfirmation = nil } }
            ),
            titleVisibility: .visible
        ) {
            if let action = pendingConfirmation,
               let confirmation = action.confirmation
            {
                Button(
                    confirmation.confirmLabel,
                    role: confirmation.destructive
                        ? .destructive
                        : nil
                ) {
                    confirm(action, confirmationID: confirmation.id)
                    pendingConfirmation = nil
                }
                Button("Cancel", role: .cancel) {
                    pendingConfirmation = nil
                }
            }
        } message: {
            if let message =
                    pendingConfirmation?.confirmation?.message
            {
                Text(message)
            }
        }
        .sheet(isPresented: $editingFontSize) {
            managedTerminalFontSizeSheet
        }
    }

    @ViewBuilder
    private func bottomControl(
        _ action: ActionProjectionV1
    ) -> some View {
        switch action.id {
        case "presentation.layout.toggle":
            bottomButton(
                action: action,
                systemImage: symbol(for: action.id),
                selectedOverride: layoutVisible,
                enabledOverride: true
            ) {
                emitPresentationAction(.setLayoutMapVisible(!layoutVisible))
            }
        case "presentation.inbox.toggle":
            ZStack(alignment: .topTrailing) {
                bottomButton(
                    action: action,
                    systemImage: symbol(for: action.id),
                    selectedOverride: inboxVisible,
                    enabledOverride: true
                ) {
                    emitPresentationAction(.setInboxVisible(!inboxVisible))
                }
                if inboxUnreadCount > 0 && !inboxVisible {
                    Text(inboxUnreadCount > 99 ? "99+" : "\(inboxUnreadCount)")
                        .font(.system(size: 9 * interfaceTextScale, weight: .bold))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 5)
                        .frame(minWidth: 18, minHeight: 18)
                        .background(Color.red, in: Capsule())
                        .accessibilityLabel("\(inboxUnreadCount) unread Inbox messages")
                }
            }
        case "resources.open":
            bottomButton(
                action: action,
                systemImage: "externaldrive.connected.to.line.below"
            ) {
                requestAuxiliary(
                    auxiliarySurfaces.resources.id,
                    from: action
                )
            }
        case "more.open":
            moreMenu(action)
        default:
            bottomButton(
                action: action,
                systemImage: symbol(for: action.id)
            ) {
                performBottomAction(action)
            }
        }
    }

    private func bottomButton(
        action: ActionProjectionV1,
        systemImage: String,
        labelOverride: String? = nil,
        selectedOverride: Bool? = nil,
        enabledOverride: Bool? = nil,
        perform: @escaping @MainActor () -> Void
    ) -> some View {
        let selected = selectedOverride ?? action.selected
        return Button(action: perform) {
            VStack(spacing: 1) {
                Image(systemName: systemImage)
                    .font(.system(size: 16 * interfaceTextScale, weight: .medium))
            }
            .frame(maxWidth: .infinity, minHeight: 44)
            .background(
                selected
                    ? StationTheme.shared.accent.opacity(0.15)
                    : Color.clear,
                in: RoundedRectangle(cornerRadius: 7)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!(enabledOverride ?? action.enabled))
        .accessibilityLabel(labelOverride ?? action.accessibilityLabel)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .help(
            action.disabledReason
                ?? labelOverride
                ?? action.accessibilityLabel
        )
    }

    private func moreMenu(
        _ action: ActionProjectionV1
    ) -> some View {
        Menu {
            Button {
                fontSizeDraft =
                    nativeLayoutView?.managedTerminalFontSizePoints
                        ?? 14
                editingFontSize = true
            } label: {
                Label(
                    fontSizeAction?.label
                        ?? "Ghostty Font Size…",
                    systemImage: "textformat.size"
                )
            }
            .disabled(fontSizeAction?.enabled == false)

            Divider()

            Button {
                requestAuxiliary(auxiliarySurfaces.iphone.id, from: action)
            } label: {
                Label("iPhone Companion…", systemImage: "iphone")
            }

            Divider()

            Button {
                workspaceNameDraft = "Workspace"
                creatingWorkspace = true
            } label: {
                Label(
                    "New Workspace",
                    systemImage:
                        "plus.rectangle.on.rectangle"
                )
            }
            .disabled(newWorkspaceAction?.enabled == false)

        } label: {
            VStack(spacing: 1) {
                Image(systemName: "ellipsis.circle")
                    .font(.system(size: 16 * interfaceTextScale, weight: .medium))
            }
            .frame(maxWidth: .infinity, minHeight: 44)
            .contentShape(Rectangle())
        }
        .menuStyle(.borderlessButton)
        .controlSize(.small)
        .disabled(!action.enabled)
        .accessibilityLabel(action.accessibilityLabel)
        .accessibilityAddTraits(action.selected ? .isSelected : [])
        .help(action.disabledReason ?? action.accessibilityLabel)
    }

    private var fontSizeAction: ActionProjectionV1? {
        projection.moreActions?.first {
            $0.id == "managed.terminal.font.size"
        }
    }

    private var newWorkspaceAction: ActionProjectionV1? {
        projection.moreActions?.first {
            $0.id == "workspace.create"
        }
    }

    private var managedTerminalFontSizeSheet: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Ghostty Font Size")
                .font(.headline)

            Stepper(
                value: $fontSizeDraft,
                in: 10...32,
                step: 1
            ) {
                Text("\(fontSizeDraft) pt")
                    .monospacedDigit()
            }

            HStack {
                Spacer()
                Button("Cancel", role: .cancel) {
                    editingFontSize = false
                }
                Button("Apply") {
                    emit(
                        .setManagedTerminalFontSize(
                            points: fontSizeDraft
                        )
                    )
                    editingFontSize = false
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(22)
        .frame(width: 300)
    }

    private var orderedActions: [ActionProjectionV1] {
        ControlBottomActionPolicy.visibleActions(in: projection)
    }

    private func requestAuxiliary(
        _ surface: AuxiliarySurfaceIDV1,
        from _: ActionProjectionV1
    ) {
        emitIntent(ControlCenterIntentV1(
            controlRevision: controlRevision,
            operation: .openAuxiliary(surface: surface)
        ))
    }

    private func invoke(_ action: ActionProjectionV1) {
        emit(action.intentOperation)
    }

    private func performBottomAction(
        _ action: ActionProjectionV1
    ) {
        if action.confirmation != nil {
            pendingConfirmation = action
        } else {
            invoke(action)
        }
    }

    private func emit(
        _ operation: ControlCenterIntentOperationV1
    ) {
        emitIntent(ControlCenterIntentV1(
            controlRevision: controlRevision,
            operation: operation
        ))
    }

    private func performMoreAction(
        _ action: ActionProjectionV1
    ) {
        if action.id == "workspace.create" {
            workspaceNameDraft = "Workspace"
            creatingWorkspace = true
        } else if action.confirmation != nil {
            pendingConfirmation = action
        } else {
            invoke(action)
        }
    }

    private func confirm(
        _ action: ActionProjectionV1,
        confirmationID: String
    ) {
        if action.id == "workspace.archive" {
            emit(.invokeConfirmedAction(actionID: "workspace.archive", confirmationID: confirmationID))
        } else {
            emit(.invokeConfirmedAction(
                actionID: action.id,
                confirmationID: confirmationID
            ))
        }
    }

    private func symbol(for actionID: String) -> String {
        switch actionID {
        case "presentation.layout.toggle":
            "rectangle.split.2x1"
        case "presentation.inbox.toggle":
            "tray.full"
        default:
            "circle"
        }
    }

    private func resourceSymbol(
        for action: ActionProjectionV1
    ) -> String {
        if !action.enabled {
            return "externaldrive.badge.exclamationmark"
        }
        if action.selected {
            return "externaldrive.fill.badge.checkmark"
        }
        return "externaldrive"
    }
}
