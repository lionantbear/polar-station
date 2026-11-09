import CoreTransferable
import PolarTmuxHostClient
import SwiftUI
import UniformTypeIdentifiers

public struct WorkspaceTabDragPayload: Codable, Hashable {
    let workspaceID: String
    let tabID: String

}

public struct PaneTabMoveDragPayload:
    Codable,
    Hashable,
    Sendable,
    Transferable
{
    public let sourceTabID: String
    public let paneID: String

    public init(sourceTabID: String, paneID: String) {
        self.sourceTabID = sourceTabID
        self.paneID = paneID
    }

    public static var transferRepresentation: some TransferRepresentation {
        CodableRepresentation(contentType: .polarStationPaneMove)
    }
}

extension UTType {
    static let polarStationTabMove = UTType(exportedAs: "com.polarealm.polar-station.tab-move", conformingTo: .json)
    static let polarStationPaneMove = UTType(
        exportedAs: "com.polarealm.polar-station.pane-move", conformingTo: .json
    )
}

public struct TabPanePlacement: Equatable, Sendable {
    public let column: Int
    public let row: Int
    public let columnSpan: Int
    public let rowSpan: Int

    public init(
        column: Int,
        row: Int,
        columnSpan: Int,
        rowSpan: Int
    ) {
        self.column = column
        self.row = row
        self.columnSpan = columnSpan
        self.rowSpan = rowSpan
    }
}

/// Local native presentation facts supplied by the macOS executor. This is
/// deliberately separate from the Rust projection: it contains only the
/// visual order needed to make the Layout Map match Ghostty.
public struct NativePaneLayoutView: Equatable, Sendable {
    public let paneID: String
    public let executionTargetID: String?
    public let tmuxSessionID: String?
    public let content: StationPaneContentV1?
    public let placement: TabPanePlacement

    public init(
        paneID: String,
        executionTargetID: String?,
        tmuxSessionID: String?,
        content: StationPaneContentV1? = nil,
        placement: TabPanePlacement
    ) {
        self.paneID = paneID
        self.executionTargetID = executionTargetID
        self.tmuxSessionID = tmuxSessionID
        self.content = content
        self.placement = placement
    }
}

public struct NativeTabLayoutView: Equatable, Sendable {
    public let tabID: String
    public let title: String
    public let displayMode: WorkspaceDisplayModeV1
    public let visualPaneIDs: [String]
    public let mainPaneID: String?
    public let panes: [NativePaneLayoutView]

    public init(
        tabID: String,
        title: String,
        displayMode: WorkspaceDisplayModeV1,
        visualPaneIDs: [String],
        mainPaneID: String?,
        panes: [NativePaneLayoutView]
    ) {
        self.tabID = tabID
        self.title = title
        self.displayMode = displayMode
        self.visualPaneIDs = visualPaneIDs
        self.mainPaneID = mainPaneID
        self.panes = panes
    }
}

public struct NativeWorkspaceChoiceView: Equatable, Sendable {
    public let workspaceID: String
    public let name: String
    public let selected: Bool
    public let archived: Bool

    public init(workspaceID: String, name: String, selected: Bool, archived: Bool = false) {
        self.workspaceID = workspaceID
        self.name = name
        self.selected = selected
        self.archived = archived
    }
}

public struct NativeWorkspaceLayoutView: Equatable, Sendable {
    public let workspaceID: String
    public let workspaceName: String
    public let managedTerminalFontSizePoints: Int
    public let selectedTabID: String
    public let selectedPaneID: String?
    public let workspaceChoices: [NativeWorkspaceChoiceView]
    public let tabs: [NativeTabLayoutView]

    public init(
        workspaceID: String,
        workspaceName: String,
        managedTerminalFontSizePoints: Int,
        selectedTabID: String,
        selectedPaneID: String?,
        workspaceChoices: [NativeWorkspaceChoiceView] = [],
        tabs: [NativeTabLayoutView]
    ) {
        self.workspaceID = workspaceID
        self.workspaceName = workspaceName
        self.managedTerminalFontSizePoints =
            managedTerminalFontSizePoints
        self.selectedTabID = selectedTabID
        self.selectedPaneID = selectedPaneID
        self.workspaceChoices = workspaceChoices
        self.tabs = tabs
    }

    public func tab(_ tabID: String) -> NativeTabLayoutView? {
        tabs.first { $0.tabID == tabID }
    }
}

public enum NativeWorkspaceLayoutProjection {
    public static func tabs(
        nativeView: NativeWorkspaceLayoutView?,
        sessions: [SessionRowProjectionV1],
        liveSessions: [PolarTmuxSessionProjection] = []
    ) -> [WorkspaceTabProjectionV1] {
        let liveRuntime = Dictionary(
            uniqueKeysWithValues: liveSessions.map { ($0.key, $0) }
        )
        guard let nativeView else { return [] }
        let sessionRows = Dictionary(
            uniqueKeysWithValues: sessions.map {
                (SessionRowProjectionV1.Identity(
                    executionTargetID: $0.identity.executionTargetID,
                    tmuxSessionID: $0.identity.tmuxSessionID
                ), $0)
            }
        )
        return nativeView.tabs.map { nativeTab in
            let panesByID = Dictionary(
                uniqueKeysWithValues: nativeTab.panes.map {
                    ($0.paneID, $0)
                }
            )
            let panes: [WorkspacePaneProjectionV1] =
                nativeTab.visualPaneIDs.compactMap { paneID in
                    guard let nativePane = panesByID[paneID]
                    else { return nil }
                    if let content = nativePane.content {
                        return WorkspacePaneProjectionV1(id: nativePane.paneID,
                            executionTargetID: nil, tmuxSessionID: nil, content: content,
                            targetLabel: content.kind == .ideaTree ? "Local" : content.targetID,
                            sessionName: content.kind == .ideaTree ? "Idea Tree" : content.thread,
                            transportStatus: .offline, selected: nativeView.selectedPaneID == nativePane.paneID)
                    }
                    guard let target = nativePane.executionTargetID, let session = nativePane.tmuxSessionID else { return nil }
                    let identity = SessionRowProjectionV1.Identity(
                        executionTargetID: target,
                        tmuxSessionID: session
                    )
                    let row = sessionRows[identity]
                    let runtime = liveRuntime[PolarTmuxSessionKey(
                        executionTarget: target,
                        tmuxSessionID: session
                    )]
                    return WorkspacePaneProjectionV1(
                        id: nativePane.paneID,
                        executionTargetID: target,
                        tmuxSessionID: session,
                        targetLabel: row?.targetLabel
                            ?? target,
                        targetKind: row?.section == .remote ? .ssh : .local,
                        sessionName: runtime?.displayName ?? row?.displayName
                            ?? session,
                        transportStatus: runtime.map {
                            transportStatus($0.lifecycle)
                        } ?? row?.transportStatus ?? .recovering,
                        sessionPresence: runtime.map {
                            $0.lifecycle == .ended ? .ended : $0.lifecycle == .ready ? .present : .unknown
                        } ?? .unknown,
                        selected: nativeView.selectedPaneID == nativePane.paneID,
                        attention: runtime?.attention ?? SessionAttentionProjectionV1(
                            executionTargetID: target,
                            tmuxSessionID: session
                        )
                    )
                }
            return WorkspaceTabProjectionV1(
                id: nativeTab.tabID,
                title: nativeTab.title,
                selected: nativeView.selectedTabID == nativeTab.tabID,
                displayMode: nativeTab.displayMode,
                focusedPaneID: nativeTab.mainPaneID,
                renameAction: nativeRenameAction,
                deleteAction: nativeDeleteAction(tabID: nativeTab.tabID),
                panes: panes
            )
        }
    }

    private static func overlay(
        _ pane: WorkspacePaneProjectionV1,
        with runtime: [PolarTmuxSessionKey: PolarTmuxSessionProjection]
    ) -> WorkspacePaneProjectionV1 {
        guard pane.content == nil, let target = pane.executionTargetID, let sessionID = pane.tmuxSessionID else { return pane }
        let key = PolarTmuxSessionKey(executionTarget: target, tmuxSessionID: sessionID)
        guard let session = runtime[key] else { return pane }
        return WorkspacePaneProjectionV1(
            id: pane.id,
            executionTargetID: pane.executionTargetID,
            tmuxSessionID: pane.tmuxSessionID,
            targetLabel: pane.targetLabel,
            targetKind: pane.targetKind,
            sessionName: session.displayName,
            transportStatus: transportStatus(session.lifecycle),
            selected: pane.selected,
            attention: session.attention ?? SessionAttentionProjectionV1(
                executionTargetID: target,
                tmuxSessionID: sessionID
            )
        )
    }

    private static func transportStatus(
        _ lifecycle: PolarTmuxSessionLifecycle
    ) -> SessionRowProjectionV1.TransportStatus {
        switch lifecycle {
        case .ready: .online
        case .connecting, .recovering, .ending: .recovering
        case .unavailable, .ended: .offline
        }
    }

    private static let nativeRenameAction = ActionProjectionV1(
        id: "workspace.tab.rename",
        label: "Rename Tab",
        role: .secondary,
        enabled: true,
        selected: false,
        disabledReason: nil,
        confirmation: nil,
        accessibilityLabel: "Rename Tab"
    )

    private static func nativeDeleteAction(
        tabID: String
    ) -> ActionProjectionV1 {
        ActionProjectionV1(
            id: "workspace.tab.delete",
            label: "Delete Tab",
            role: .destructive,
            enabled: true,
            selected: false,
            disabledReason: nil,
            confirmation: ConfirmationProjectionV1(
                id: "native-delete-tab:\(tabID)",
                title: "Delete Tab?",
                message: "The tmux Sessions remain available.",
                confirmLabel: "Delete Tab",
                destructive: true
            ),
            accessibilityLabel: "Delete Tab"
        )
    }
}

public enum TabCardLayoutPolicy {
    public static let headerMinimumHeight: CGFloat = 44
    public static let focusRowHeight: CGFloat = 48
    public static let monitorRowHeight: CGFloat = 56
    public static let focusMinimumBodyHeight: CGFloat = 200
    public static let monitorMinimumBodyHeight: CGFloat = 170
    public static let minimumPaneWidth: CGFloat = 96
    public static let gridGap: CGFloat = 7
    /// The body receives the 390-point Map width after the ScrollView and card
    /// keep their fixed horizontal insets: 390 - 24 - 22 = 344.
    public static let referenceBodyWidth: CGFloat = 344

    public static func bodyScale(availableBodyWidth: CGFloat) -> CGFloat {
        min(1, max(1, availableBodyWidth) / referenceBodyWidth)
    }

    public static func scaledBodyHeight(
        displayMode: WorkspaceDisplayModeV1,
        rowSpan: Int,
        availableBodyWidth: CGFloat
    ) -> CGFloat {
        bodyHeight(displayMode: displayMode, rowSpan: rowSpan)
            * bodyScale(availableBodyWidth: availableBodyWidth)
    }

    public static func scaledGridGap(
        availableBodyWidth: CGFloat
    ) -> CGFloat {
        gridGap * bodyScale(availableBodyWidth: availableBodyWidth)
    }

    public static func maximumReadableColumns(
        availableWidth: CGFloat
    ) -> Int {
        max(
            1,
            Int(
                floor(
                    (max(1, availableWidth) + gridGap)
                        / (minimumPaneWidth + gridGap)
                )
            )
        )
    }

    public static func bodyHeight(
        displayMode: WorkspaceDisplayModeV1,
        rowSpan: Int
    ) -> CGFloat {
        let rows = CGFloat(max(1, rowSpan))
        switch displayMode {
        case .focus:
            return max(
                focusMinimumBodyHeight,
                rows * focusRowHeight
            )
        case .monitor:
            return max(
                monitorMinimumBodyHeight,
                rows * monitorRowHeight
            )
        }
    }

    public static func placements(
        for tab: WorkspaceTabProjectionV1
    ) -> [String: TabPanePlacement] {
        placements(for: tab, nativeView: nil)
    }

    public static func placements(
        for tab: WorkspaceTabProjectionV1,
        nativeView: NativeTabLayoutView?
    ) -> [String: TabPanePlacement] {
        guard !tab.panes.isEmpty else { return [:] }
        if let nativeView {
            let paneIDs = Set(tab.panes.map(\.id))
            let nativeIDs = Set(nativeView.panes.map(\.paneID))
            if paneIDs == nativeIDs,
               nativeView.panes.count == tab.panes.count
            {
                return Dictionary(
                    uniqueKeysWithValues: nativeView.panes.map {
                        ($0.paneID, $0.placement)
                    }
                )
            }
        }
        let displayMode = nativeView?.displayMode ?? tab.displayMode
        let populations = populations(
            paneCount: tab.panes.count,
            displayMode: displayMode
        )
        let panesByID = Dictionary(
            uniqueKeysWithValues: tab.panes.map { ($0.id, $0) }
        )
        let projectedVisualIDs: [String]
        if displayMode == .focus,
           let main = tab.panes.first(where: {
               $0.id == tab.focusedPaneID
           })
        {
            projectedVisualIDs = [main.id]
                + tab.panes.map(\.id).filter { $0 != main.id }
        } else {
            projectedVisualIDs = tab.panes.map(\.id)
        }
        let candidateVisualIDs = nativeView?.visualPaneIDs ?? projectedVisualIDs
        let visualIDs = candidateVisualIDs.count == tab.panes.count
            && Set(candidateVisualIDs) == Set(tab.panes.map(\.id))
            ? candidateVisualIDs
            : projectedVisualIDs
        let visualPanes = visualIDs.compactMap { panesByID[$0] }
        guard visualPanes.count == tab.panes.count else { return [:] }
        let columnWeights: [Int]
        switch displayMode {
        case .monitor:
            columnWeights = Array(repeating: 1, count: populations.count)
        case .focus where tab.panes.count <= 3:
            columnWeights = tab.panes.count == 1 ? [1] : [4, 1]
        case .focus:
            columnWeights = [3, 1, 1]
        }
        let rowUnits = populations.reduce(1, leastCommonMultiple)
        var result: [String: TabPanePlacement] = [:]
        var paneIndex = 0
        var columnOrigin = 0
        for (column, population) in populations.enumerated() {
            let rowSpan = rowUnits / population
            for row in 0 ..< population {
                let pane = visualPanes[paneIndex]
                result[pane.id] = .init(
                    column: columnOrigin,
                    row: row * rowSpan,
                    columnSpan: columnWeights[column],
                    rowSpan: rowSpan
                )
                paneIndex += 1
            }
            columnOrigin += columnWeights[column]
        }
        return result
    }

    private static func populations(
        paneCount: Int,
        displayMode: WorkspaceDisplayModeV1
    ) -> [Int] {
        let monitor = [
            [1], [1, 1], [1, 2], [1, 2, 1], [1, 2, 2],
            [2, 2, 2], [2, 3, 2], [2, 3, 3], [3, 3, 3],
        ]
        let focus = [
            [1], [1, 1], [1, 2], [1, 2, 1], [1, 2, 2],
            [1, 2, 3], [1, 3, 3], [1, 3, 4], [1, 4, 4],
        ]
        switch displayMode {
        case .monitor:
            return monitor[paneCount - 1]
        case .focus:
            return focus[paneCount - 1]
        }
    }

    private static func leastCommonMultiple(
        _ left: Int,
        _ right: Int
    ) -> Int {
        left / greatestCommonDivisor(left, right) * right
    }

    private static func greatestCommonDivisor(
        _ leftValue: Int,
        _ rightValue: Int
    ) -> Int {
        var left = leftValue
        var right = rightValue
        while right != 0 {
            (left, right) = (right, left % right)
        }
        return max(1, left)
    }
}

public struct WorkspaceMapView: View {
    public let controlRevision: UInt64
    public let nativeView: NativeWorkspaceLayoutView?
    public let sessions: [SessionRowProjectionV1]
    public let liveSessions: [PolarTmuxSessionProjection]
    public let emitIntent: @MainActor (ControlCenterIntentV1) -> Void

    @State private var renamingTab: WorkspaceTabProjectionV1?
    @State private var tabTitleDraft = ""
    @State private var summaryPane: WorkspacePaneProjectionV1?
    @State private var hoveredPaneID: String?
    @FocusState private var focusedPaneID: String?

    public init(
        controlRevision: UInt64,
        nativeView: NativeWorkspaceLayoutView? = nil,
        sessions: [SessionRowProjectionV1] = [],
        liveSessions: [PolarTmuxSessionProjection] = [],
        emitIntent: @escaping @MainActor (ControlCenterIntentV1) -> Void
    ) {
        self.controlRevision = controlRevision
        self.nativeView = nativeView
        self.sessions = sessions
        self.liveSessions = liveSessions
        self.emitIntent = emitIntent
    }

    public var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 12) {
                ForEach(renderedTabs) { tab in
                    tabCard(tab)
                }
            }
            .padding(12)
        }
        .background(StationTheme.shared.surface)
        .overlay {
            if renderedTabs.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "rectangle.split.3x1")
                        .font(.largeTitle)
                        .foregroundStyle(.secondary)
                    Text((nativeView?.workspaceName ?? "Workspace"))
                        .font(.headline)
                    Text("This Workspace has no projected tabs.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(nativeView?.workspaceName ?? "Workspace")
        .popover(item: $summaryPane) { pane in
            PaneAttentionSummaryView(
                pane: pane,
                controlRevision: controlRevision,
                emitIntent: emitIntent
            )
        }
        .onChange(of: nativeView?.selectedTabID) { _ in summaryPane = nil }
        .onChange(of: nativeView.flatMap { $0.tab($0.selectedTabID)?.mainPaneID }) { _ in
            summaryPane = nil
        }
        .alert(
            "Rename Tab",
            isPresented: Binding(
                get: { renamingTab != nil },
                set: { if !$0 { renamingTab = nil } }
            )
        ) {
            TextField("Tab title", text: $tabTitleDraft)
            Button("Cancel", role: .cancel) {
                renamingTab = nil
            }
            Button("Rename") {
                guard let tab = renamingTab else { return }
                emit(.renameTab(
                    tabID: tab.id,
                    title: tabTitleDraft
                ))
                renamingTab = nil
            }
            .disabled(
                tabTitleDraft
                    .trimmingCharacters(
                        in: .whitespacesAndNewlines
                    )
                    .isEmpty
            )
        }
    }

    private var renderedTabs: [WorkspaceTabProjectionV1] {
        NativeWorkspaceLayoutProjection.tabs(
            nativeView: nativeView,
            sessions: sessions,
            liveSessions: liveSessions
        )
    }

    private func tabCard(
        _ tab: WorkspaceTabProjectionV1
    ) -> some View {
        let nativeTab = nativeView?.tab(tab.id)
        let displayMode = nativeTab?.displayMode ?? tab.displayMode
        let acceptsPane: (PaneTabMoveDragPayload) -> Bool = { payload in
            guard let source = renderedTabs.first(where: {
                      $0.id == payload.sourceTabID
                  }),
                  source.panes.contains(where: {
                      $0.id == payload.paneID
                  }),
                  payload.sourceTabID == tab.id || tab.panes.count < 9
            else { return false }
            return true
        }
        let dropPane: (PaneTabMoveDragPayload, Bool) -> Void = { payload, _ in
            if payload.sourceTabID == tab.id {
                emit(.reorderPane(tabID: tab.id, paneID: payload.paneID, toIndex: tab.panes.count - 1))
                return
            }
            emit(.movePane(
                paneID: payload.paneID,
                fromTabID: payload.sourceTabID,
                toTabID: tab.id,
                toIndex: tab.panes.count
            ))
        }
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                HStack {
                    Image(systemName: tab.selected ? "rectangle.inset.filled" : "rectangle")
                    Text(tab.title).font(.headline).lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(tab.title)
                .accessibilityAddTraits(.isButton)
                .accessibilityAddTraits(tab.selected ? .isSelected : [])
                .accessibilityAction { emit(.selectTab(tabID: tab.id)) }
                .overlay {
                    GeometryReader { geometry in
                        TabDragSourceView(
                            visualFrame: geometry.frame(in: .global),
                            payload: WorkspaceTabDragPayload(workspaceID: nativeView?.workspaceID ?? "", tabID: tab.id),
                            onSelect: { emit(.selectTab(tabID: tab.id)) }
                        )
                    }
                }

                Text("\(tab.panes.count)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                Menu {
                    if let index = renderedTabs.firstIndex(where: { $0.id == tab.id }) {
                        Button("Move Up") { emit(.reorderTab(tabID: tab.id, toIndex: index - 1)) }.disabled(index == 0)
                        Button("Move Down") { emit(.reorderTab(tabID: tab.id, toIndex: index + 1)) }.disabled(index + 1 == renderedTabs.count)
                    }
                    Divider()
                    Button("Rename…") { tabTitleDraft = tab.title; renamingTab = tab }
                    if let action = tab.deleteAction {
                        Button(action.label, role: .destructive) {
                            if let id = action.confirmation?.id { emit(.deleteTab(tabID: tab.id, confirmationID: id)) }
                        }.disabled(!action.enabled)
                    }
                } label: { Image(systemName: "ellipsis").frame(width: 28, height: 28) }
                .menuStyle(.borderlessButton).fixedSize().accessibilityLabel("Tab actions").help("Tab actions")
            }
            .padding(.horizontal, 1)
            .frame(
                minHeight:
                TabCardLayoutPolicy.headerMinimumHeight
            )


            Divider()

            if tab.panes.isEmpty {
                ProportionalPaneLayout(displayMode: displayMode) {
                    VStack(spacing: 6) {
                        Image(systemName: "rectangle.dashed")
                            .foregroundStyle(.secondary)
                        Text("Add a Session to this Tab")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                .clipped()
            } else {
                projectedGeometry(for: tab, nativeView: nativeTab)
            }
        }
        .padding(11)
        .background(
            .thinMaterial,
            in: RoundedRectangle(cornerRadius: 12)
        )
        .overlay {
            RoundedRectangle(cornerRadius: 12)
                .stroke(
                    tab.selected
                        ? StationTheme.shared.accent.opacity(0.7)
                        : Color.secondary.opacity(0.18),
                    lineWidth: tab.selected ? 1.5 : 1
                )
        }
        .shadow(
            color: .black.opacity(tab.selected ? 0.10 : 0.04),
            radius: tab.selected ? 5 : 2,
            y: 2
        )
        .overlay {
            GeometryReader { geometry in
                PaneDropDestinationView(
                    visualFrame: geometry.frame(in: .global),
                    canAccept: acceptsPane,
                    onDrop: dropPane
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .frame(height: tab.panes.isEmpty ? nil : 66)
            .frame(maxHeight: .infinity, alignment: .top)
        }
        .overlay {
            // Tab insertion uses the header midpoint even for an empty card;
            // the independent Pane destination keeps its full empty-card area.
            GeometryReader { geometry in
                PaneDropDestinationView(
                    visualFrame: geometry.frame(in: .global),
                    canAccept: acceptsPane,
                    onDrop: dropPane,
                    canAcceptTab: { payload in
                        payload.workspaceID == nativeView?.workspaceID && payload.tabID != tab.id && renderedTabs.contains(where: { $0.id == payload.tabID })
                    },
                    onTabDrop: { payload, after in
                        guard let index = renderedTabs.firstIndex(where: { $0.id == tab.id }) else { return }
                        let source = renderedTabs.firstIndex(where: { $0.id == payload.tabID }) ?? index
                        let insertion = index + (after ? 1 : 0)
                        emit(.reorderTab(tabID: payload.tabID, toIndex: insertion - (source < insertion ? 1 : 0)))
                    }
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .frame(height: 66)
            .frame(maxHeight: .infinity, alignment: .top)
        }

        .contextMenu {
            if let renameAction = tab.renameAction {
                Button {
                    tabTitleDraft = tab.title
                    renamingTab = tab
                } label: {
                    Label(renameAction.label, systemImage: "pencil")
                }
                .disabled(!renameAction.enabled)
            }
            if let deleteAction = tab.deleteAction {
                Button(role: .destructive) {
                    guard let confirmationID = deleteAction.confirmation?.id
                    else { return }
                    emit(.deleteTab(
                        tabID: tab.id,
                        confirmationID: confirmationID
                    ))
                } label: {
                    Label(deleteAction.label, systemImage: "trash")
                }
                .disabled(!deleteAction.enabled)
            }
            Divider()
            if let tabIndex = renderedTabs.firstIndex(
                where: { $0.id == tab.id }
            ) {
                Button("Move Tab Earlier") {
                    emit(.reorderTab(
                        tabID: tab.id,
                        toIndex: tabIndex - 1
                    ))
                }
                .disabled(tabIndex == 0)

                Button("Move Tab Later") {
                    emit(.reorderTab(
                        tabID: tab.id,
                        toIndex: tabIndex + 1
                    ))
                }
                .disabled(tabIndex + 1 >= renderedTabs.count)
            }
        }
    }

    private func projectedGeometry(
        for tab: WorkspaceTabProjectionV1,
        nativeView: NativeTabLayoutView?
    ) -> some View {
        let placements = TabCardLayoutPolicy.placements(
            for: tab,
            nativeView: nativeView
        )
        return ProportionalPaneLayout(
            displayMode: nativeView?.displayMode ?? tab.displayMode
        ) {
            ForEach(tab.panes) { pane in
                if let placement = placements[pane.id] {
                    paneTile(pane, in: tab)
                        .layoutValue(
                            key: PanePlacementLayoutValueKey.self,
                            value: placement
                        )
                        .clipped()
                }
            }
        }
        .clipped()
    }

    private func paneTile(
        _ pane: WorkspacePaneProjectionV1,
        in tab: WorkspaceTabProjectionV1
    ) -> some View {
        return VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                if pane.content?.kind != .ideaTree {
                    Circle()
                        .fill(pane.transportStatus == .online ? Color.green : Color.red)
                        .frame(width: 8, height: 8)
                }

                Text(pane.title)
                    .font(.subheadline.weight(.semibold))
                    .lineLimit(1)
                if pane.sessionPresence == .ended {
                    Text("Ended")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.red)
                        .accessibilityLabel("Session ended")
                }
            }
            Text(pane.sourceLabel)
                .font(.caption.weight(.medium))
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .frame(
            maxWidth: .infinity,
            maxHeight: .infinity,
            alignment: .topLeading
        )
        .padding(8)
        .contentShape(Rectangle())
        .focusable()
        .focused($focusedPaneID, equals: pane.id)
        .onHover { hovering in
            if hovering {
                hoveredPaneID = pane.id
            } else if hoveredPaneID == pane.id {
                hoveredPaneID = nil
            }
        }
        .accessibilityLabel(
            "\(pane.title), \(pane.sourceLabel), \(pane.runtimeState)"
        )
        .accessibilityAddTraits(.isButton)
        .accessibilityAddTraits(pane.selected ? .isSelected : [])
        .accessibilityAction {
            emitIntent(ControlCenterIntentV1(
                controlRevision: controlRevision,
                operation: .selectPane(paneID: pane.id)
            ))
        }
        .accessibilityAction(named: "Focus as main") {
            summaryPane = nil
            emitIntent(ControlCenterIntentV1(
                controlRevision: controlRevision,
                operation: .activatePane(paneID: pane.id)
            ))
        }
        .background {
            GeometryReader { geometry in
                PaneClickArbitrationView(
                    visualFrame: geometry.frame(in: .global),
                    dragPayload: PaneTabMoveDragPayload(
                        sourceTabID: tab.id,
                        paneID: pane.id
                    ),
                    onSelect: {
                        summaryPane = pane
                        emitIntent(ControlCenterIntentV1(
                            controlRevision: controlRevision,
                            operation: .selectPane(paneID: pane.id)
                        ))
                        if pane.attention.summaryRevision > 0 {
                            emitIntent(ControlCenterIntentV1(
                                controlRevision: controlRevision,
                                operation: .acknowledgePaneSummary(
                                    paneID: pane.id,
                                    summaryRevision:
                                    pane.attention.summaryRevision
                                )
                            ))
                        }
                    },
                    onActivate: {
                        summaryPane = nil
                        emitIntent(ControlCenterIntentV1(
                            controlRevision: controlRevision,
                            operation: .activatePane(paneID: pane.id)
                        ))
                    }
                )
            }
        }
        .overlay {
            GeometryReader { geometry in
                PaneDropDestinationView(
                    visualFrame: geometry.frame(in: .global),
                    canAccept: { payload in
                        payload.paneID != pane.id
                            && (payload.sourceTabID == tab.id || tab.panes.count < 9)
                            && renderedTabs.first(where: { $0.id == payload.sourceTabID })?.panes.contains(where: { $0.id == payload.paneID }) == true
                    },
                    onDrop: { payload, after in
                        let visual = nativeView?.tab(tab.id)?.visualPaneIDs ?? tab.panes.map(\.id)
                        let remaining = visual.filter { $0 != payload.paneID }
                        guard let anchor = remaining.firstIndex(of: pane.id) else { return }
                        let destination = anchor + (after ? 1 : 0)
                        guard visual.firstIndex(of: payload.paneID) != destination else { return }
                        if payload.sourceTabID == tab.id {
                            emit(.reorderPane(tabID: tab.id, paneID: payload.paneID, toIndex: destination))
                        } else {
                            emit(.movePane(paneID: payload.paneID, fromTabID: payload.sourceTabID, toTabID: tab.id, toIndex: destination))
                        }
                    }
                )
            }
        }
        .contextMenu {
            Button(role: .destructive) {
                emitIntent(ControlCenterIntentV1(
                    controlRevision: controlRevision,
                    operation: .removePane(paneID: pane.id)
                ))
            } label: {
                Label(
                    tab.panes.count == 1
                        ? "Remove Pane (Keep Empty Tab)"
                        : "Remove from Tab",
                    systemImage: "trash"
                )
            }
        }
        .background {
            RoundedRectangle(cornerRadius: 8)
                .fill(
                    pane.selected
                        ? StationTheme.shared.accent.opacity(0.16)
                        : Color.secondary.opacity(0.08)
                )
                .overlay {
                    RoundedRectangle(cornerRadius: 8)
                        .fill(attentionColor(for: pane).opacity(0.16))
                }
        }
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(attentionColor(for: pane), lineWidth: 3)
                .opacity(pane.attention.state == .idle ? 0 : 0.9)
            RoundedRectangle(cornerRadius: 8)
                .stroke(
                    pane.selected
                        ? StationTheme.shared.accent.opacity(0.55)
                        : Color.secondary.opacity(0.12)
                )
        }
        .overlay {
            ZStack(alignment: .topTrailing) {
                if pane.attention.state != .idle {
                    Text(pane.attention.state.rawValue.replacingOccurrences(
                        of: "_",
                        with: " "
                    ))
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(attentionColor(for: pane))
                    .padding(.horizontal, 5)
                    .padding(.vertical, 2)
                    .background(.thinMaterial, in: Capsule())
                    .accessibilityLabel(
                        "Attention: \(pane.attention.state.rawValue)"
                    )
                }
            }
        }
    }

    private func attentionColor(
        for pane: WorkspacePaneProjectionV1
    ) -> Color {
        if pane.attention.state.isRed { return .red }
        if pane.attention.state.isOrange { return .orange }
        if pane.attention.state == .working { return .blue }
        return .clear
    }

    private func emit(
        _ operation: ControlCenterIntentOperationV1
    ) {
        emitIntent(ControlCenterIntentV1(
            controlRevision: controlRevision,
            operation: operation
        ))
    }
}

private struct PaneAttentionSummaryView: View {
    let pane: WorkspacePaneProjectionV1
    let controlRevision: UInt64
    let emitIntent: @MainActor (ControlCenterIntentV1) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(pane.title)
                .font(.headline)
            Text(pane.sourceLabel)
                .font(.caption)
                .foregroundStyle(.secondary)
            if pane.attention.summarySections.isEmpty {
                Text("No summary is available yet.")
                    .font(.body)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(pane.attention.summarySections, id: \.self) { section in
                    Text(section)
                        .font(.body)
                }
            }
        }
        .padding(14)
        .frame(minWidth: 240, maxWidth: 340, alignment: .leading)
        .onAppear {
            guard pane.attention.summaryRevision > 0 else { return }
            emitIntent(ControlCenterIntentV1(
                controlRevision: controlRevision,
                operation: .markAttentionNotificationHandled(
                    paneID: pane.id,
                    summaryRevision: pane.attention.summaryRevision
                )
            ))
        }
    }
}

private struct GeometryExtents {
    let columns: Int
    let rows: Int

    init(placements: [TabPanePlacement]) {
        columns = max(
            1,
            placements.map {
                max(0, $0.column)
                    + max(1, $0.columnSpan)
            }.max() ?? 1
        )
        rows = max(
            1,
            placements.map {
                max(0, $0.row)
                    + max(1, $0.rowSpan)
            }.max() ?? 1
        )
    }
}

private struct PanePlacementLayoutValueKey: LayoutValueKey {
    static let defaultValue = TabPanePlacement(
        column: 0,
        row: 0,
        columnSpan: 1,
        rowSpan: 1
    )
}

/// Measures and places one native Pane topology as a single proportional body.
/// `sizeThatFits` is the important ownership boundary: the enclosing Tab card
/// receives the scaled height before SwiftUI lays out the next sibling card.
private struct ProportionalPaneLayout: Layout {
    let displayMode: WorkspaceDisplayModeV1

    func sizeThatFits(
        proposal: ProposedViewSize,
        subviews: Subviews,
        cache _: inout Void
    ) -> CGSize {
        let width = max(
            1,
            proposal.width ?? TabCardLayoutPolicy.referenceBodyWidth
        )
        let extents = GeometryExtents(
            placements: subviews.map {
                $0[PanePlacementLayoutValueKey.self]
            }
        )
        return CGSize(
            width: width,
            height: TabCardLayoutPolicy.scaledBodyHeight(
                displayMode: displayMode,
                rowSpan: extents.rows,
                availableBodyWidth: width
            )
        )
    }

    func placeSubviews(
        in bounds: CGRect,
        proposal _: ProposedViewSize,
        subviews: Subviews,
        cache _: inout Void
    ) {
        let placements = subviews.map {
            $0[PanePlacementLayoutValueKey.self]
        }
        let extents = GeometryExtents(placements: placements)
        let gap = TabCardLayoutPolicy.scaledGridGap(
            availableBodyWidth: bounds.width
        )
        let columnWidth = max(
            1,
            (
                bounds.width
                    - CGFloat(max(0, extents.columns - 1)) * gap
            ) / CGFloat(extents.columns)
        )
        let rowHeight = max(
            1,
            (
                bounds.height
                    - CGFloat(max(0, extents.rows - 1)) * gap
            ) / CGFloat(extents.rows)
        )

        for (subview, placement) in zip(subviews, placements) {
            let width =
                columnWidth * CGFloat(max(1, placement.columnSpan))
                    + gap * CGFloat(max(0, placement.columnSpan - 1))
            let height =
                rowHeight * CGFloat(max(1, placement.rowSpan))
                    + gap * CGFloat(max(0, placement.rowSpan - 1))
            subview.place(
                at: CGPoint(
                    x: bounds.minX
                        + CGFloat(max(0, placement.column))
                        * (columnWidth + gap),
                    y: bounds.minY
                        + CGFloat(max(0, placement.row))
                        * (rowHeight + gap)
                ),
                anchor: .topLeading,
                proposal: ProposedViewSize(width: width, height: height)
            )
        }
    }
}
