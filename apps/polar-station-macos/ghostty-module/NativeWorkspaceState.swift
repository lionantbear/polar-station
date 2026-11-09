import CryptoKit
import Foundation
import PolarTmuxHostClient

/// The one logical Workspace table owned by the macOS adapter.
///
/// Rust supplies stable backend identity and display facts. This table owns
/// Tab/Pane order, Monitor/Focus state, the Focus main Pane, and the desired
/// Pane-to-Session binding used by the physical Ghostty executor.
struct PolarStationNativePane: Codable, Equatable, Sendable {
    let id: String
    let executionTargetID: String?
    let tmuxSessionID: String?
    var content: StationPaneContentV1? = nil
    var transportStatus: SessionRowProjectionV1.TransportStatus = .offline

    enum CodingKeys: String, CodingKey {
        case id, content
        case executionTargetID = "execution_target_id"
        case tmuxSessionID = "tmux_session_id"
    }
}

struct PolarStationNativeTab: Codable, Equatable, Sendable {
    let id: String
    var title: String
    var mode: PolarStationNativeDisplayMode
    var mainPaneID: String?
    var panes: [PolarStationNativePane]
    var visualPaneIDs: [String]

    /// Stable shortcut numbering follows canonical Pane order, not the
    /// Focus-only visual slot order that changes when MAIN is promoted.
    func paneID(number: Int) -> String? {
        guard (1 ... 9).contains(number), panes.indices.contains(number - 1)
        else { return nil }
        return panes[number - 1].id
    }

    init(
        id: String,
        title: String,
        mode: PolarStationNativeDisplayMode,
        mainPaneID: String?,
        panes: [PolarStationNativePane],
        visualPaneIDs: [String]? = nil
    ) {
        self.id = id
        self.title = title
        self.mode = mode
        self.mainPaneID = mainPaneID
        self.panes = panes
        let canonical = panes.map(\.id)
        if let visualPaneIDs {
            self.visualPaneIDs = visualPaneIDs
        } else if mode == .focus, let mainPaneID {
            self.visualPaneIDs = [mainPaneID]
                + canonical.filter { $0 != mainPaneID }
        } else {
            self.visualPaneIDs = canonical
        }
    }

    enum CodingKeys: String, CodingKey {
        case id
        case title
        case mode
        case mainPaneID = "main_pane_id"
        case panes
        case visualPaneIDs = "visual_pane_ids"
    }
}

public enum PolarTmuxFocusCycleDirection: Equatable, Sendable {
    case previous
    case next
}

/// Identity of the physical Ghostty Surface tree, deliberately excluding
/// projection fields that must not rebuild or rebind running TUI content.
struct PolarStationNativePhysicalTreeSignature: Equatable {
    struct Pane: Equatable {
        let id: String
        let executionTargetID: String?
        let tmuxSessionID: String?
        let content: StationPaneContentV1?
    }

    struct Tab: Equatable {
        let id: String
        let mode: PolarStationNativeDisplayMode
        let mainPaneID: String?
        let canonicalPaneIDs: [String]
        let visualPaneIDs: [String]
        let panes: [Pane]
    }

    let workspaceID: String
    let tabs: [Tab]
}

/// The exact native effects allowed when an authoritative projection arrives.
///
/// Revision-only changes are deliberately absent: a newer Host revision is not
/// permission to activate a window, replay focus, or invalidate Surface layout.
struct PolarStationNativeProjectionDelta: Equatable {
    static let none = Self()

    let physicalTreeChanged: Bool
    let metadataChanged: Bool
    let selectionChanged: Bool
    let fontSizeChanged: Bool

    init(
        physicalTreeChanged: Bool = false,
        metadataChanged: Bool = false,
        selectionChanged: Bool = false,
        fontSizeChanged: Bool = false
    ) {
        self.physicalTreeChanged = physicalTreeChanged
        self.metadataChanged = metadataChanged
        self.selectionChanged = selectionChanged
        self.fontSizeChanged = fontSizeChanged
    }

    init(
        previous: PolarStationNativeWorkspace?,
        candidate: PolarStationNativeWorkspace
    ) {
        guard let previous else {
            self.init(
                physicalTreeChanged: true,
                metadataChanged: true,
                selectionChanged: true,
                fontSizeChanged: true
            )
            return
        }
        let physicalTreeChanged = previous.physicalTreeSignature()
            != candidate.physicalTreeSignature()
        let metadataChanged = previous.name != candidate.name
            || previous.tabs.count != candidate.tabs.count
            || !zip(previous.tabs, candidate.tabs).allSatisfy { pair in
                pair.0.id == pair.1.id && pair.0.title == pair.1.title
            }
        let selectionChanged = previous.selectedTabID != candidate.selectedTabID
            || previous.selectedPaneID != candidate.selectedPaneID
        let fontSizeChanged = previous.managedTerminalFontSizePoints
            != candidate.managedTerminalFontSizePoints
        self.init(
            physicalTreeChanged: physicalTreeChanged,
            metadataChanged: metadataChanged,
            selectionChanged: selectionChanged,
            fontSizeChanged: fontSizeChanged
        )
    }
}

struct PolarStationNativeWorkspace: Codable, Equatable, Sendable {
    let id: String
    var name: String
    var revision: UInt64
    var structureFingerprint: String
    var selectedTabID: String
    var selectedPaneID: String?
    var managedTerminalFontSizePoints: UInt8
    var layoutMapVisible: Bool? = nil
    var inboxVisible: Bool? = nil
    var tabs: [PolarStationNativeTab]

    init(
        id: String,
        name: String,
        revision: UInt64,
        structureFingerprint: String = "",
        selectedTabID: String,
        selectedPaneID: String?,
        managedTerminalFontSizePoints: UInt8,
        tabs: [PolarStationNativeTab]
    ) {
        self.id = id
        self.name = name
        self.revision = revision
        self.structureFingerprint = structureFingerprint
        self.selectedTabID = selectedTabID
        self.selectedPaneID = selectedPaneID
        self.managedTerminalFontSizePoints =
            managedTerminalFontSizePoints
        self.tabs = tabs
    }

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case revision
        case layoutMapVisible = "layout_map_visible"
        case inboxVisible = "inbox_visible"
        case structureFingerprint = "structure_fingerprint"
        case selectedTabID = "selected_tab_id"
        case selectedPaneID = "selected_pane_id"
        case managedTerminalFontSizePoints =
            "managed_terminal_font_size_points"
        case tabs
    }

    init(
        map: WorkspaceMapProjectionV1,
        managedTerminalFontSizePoints: UInt8
    ) throws {
        guard !map.workspaceID.isEmpty,
              !map.tabs.isEmpty,
              map.tabs.contains(where: { $0.id == map.selectedTabID }),
              Set(map.tabs.map(\.id)).count == map.tabs.count
        else {
            throw PolarStationNativeWorkspaceError.invalidStructure
        }
        var allPaneIDs = Set<String>()
        let tabs = try map.tabs.map { source in
            let panes = try source.panes.map { pane in
                guard !pane.id.isEmpty,
                      (pane.content?.isValid == true || (pane.executionTargetID?.isEmpty == false && pane.tmuxSessionID?.isEmpty == false)),
                      allPaneIDs.insert(pane.id).inserted
                else {
                    throw PolarStationNativeWorkspaceError.invalidBinding
                }
                return PolarStationNativePane(
                    id: pane.id,
                    executionTargetID: pane.executionTargetID,
                    tmuxSessionID: pane.tmuxSessionID,
                    content: pane.content,
                    transportStatus: pane.transportStatus
                )
            }
            let mode: PolarStationNativeDisplayMode =
                source.displayMode == .focus ? .focus : .monitor
            let mainPaneID: String?
            switch mode {
            case .monitor:
                mainPaneID = nil
            case .focus where panes.isEmpty:
                mainPaneID = nil
            case .focus:
                guard let focusedPaneID = source.focusedPaneID,
                      panes.contains(where: { $0.id == focusedPaneID })
                else {
                    throw PolarStationNativeWorkspaceError.invalidFocusMain
                }
                mainPaneID = focusedPaneID
            }
            if !panes.isEmpty {
                _ = try PolarStationNativeLayoutFamily.layout(
                    orderedPaneIDs: source.visualPaneIDs,
                    mode: mode,
                    mainPaneID: mainPaneID
                )
            }
            return PolarStationNativeTab(
                id: source.id,
                title: source.title,
                mode: mode,
                mainPaneID: mainPaneID,
                panes: panes,
                visualPaneIDs: source.visualPaneIDs
            )
        }
        id = map.workspaceID
        name = map.workspaceName
        // Revision zero is the valid Host-owned initial Workspace state.
        // Native mutation advances it to one in the candidate; clamping here
        // would make the first CAS candidate stale before it reaches Rust.
        revision = map.structureRevision
        structureFingerprint = map.structureFingerprint
        selectedTabID = map.selectedTabID
        selectedPaneID = map.selectedPaneID ?? map.tabs
            .flatMap(\.panes)
            .first(where: \.selected)?
            .id
        self.managedTerminalFontSizePoints =
            managedTerminalFontSizePoints
        self.tabs = tabs
    }

    /// Merge Host runtime facts without accepting any Host copy of native
    /// Workspace structure or selection.
    mutating func mergeSessionRuntime(
        _ sessions: [PolarTmuxSessionProjection]
    ) {
        let statuses = Dictionary(
            sessions.map { session in
                let status: SessionRowProjectionV1.TransportStatus
                switch session.lifecycle {
                case .ready:
                    status = .online
                case .connecting, .recovering, .ending:
                    status = .recovering
                case .unavailable, .ended:
                    status = .offline
                }
                return (RuntimeSessionKey(
                    executionTargetID: session.key.executionTarget,
                    tmuxSessionID: session.key.tmuxSessionID
                ), status)
            },
            uniquingKeysWith: { _, latest in latest }
        )
        for tabIndex in tabs.indices {
            for paneIndex in tabs[tabIndex].panes.indices {
                let pane = tabs[tabIndex].panes[paneIndex]
                guard pane.content == nil, let target = pane.executionTargetID,
                      let session = pane.tmuxSessionID else { continue }
                let key = RuntimeSessionKey(executionTargetID: target, tmuxSessionID: session)
                if let status = statuses[key] {
                    tabs[tabIndex].panes[paneIndex].transportStatus = status
                }
            }
        }
    }

    func structureSnapshot() -> WorkspaceStructureSnapshotV1 {
        let structureTabs = tabs.map { tab in
            WorkspaceStructureTabV1(
                id: tab.id,
                title: tab.title,
                displayMode: tab.mode == .focus ? .focus : .monitor,
                mainPaneID: tab.mainPaneID,
                canonicalPaneIDs: tab.panes.map(\.id),
                visualPaneIDs: tab.visualPaneIDs,
                panes: tab.panes.map { pane in
                    WorkspaceStructurePaneV1(
                        id: pane.id,
                        executionTargetID: pane.executionTargetID,
                        tmuxSessionID: pane.tmuxSessionID,
                        content: pane.content
                    )
                }
            )
        }
        let unhashed = WorkspaceStructureSnapshotV1(
            workspaceID: id,
            workspaceName: name,
            structureRevision: revision,
            structureFingerprint: "",
            selectedTabID: selectedTabID,
            selectedPaneID: selectedPaneID,
            tabs: structureTabs
        )
        return WorkspaceStructureSnapshotV1(
            workspaceID: id,
            workspaceName: name,
            structureRevision: revision,
            structureFingerprint:
            PolarStationStructureFingerprint.compute(unhashed),
            selectedTabID: selectedTabID,
            selectedPaneID: selectedPaneID,
            tabs: structureTabs
        )
    }

    /// Returns only the identity that requires a Surface-tree operation.
    /// Selection, revision, font size, transport status, and display copy are
    /// intentionally absent so projection refresh cannot disturb a live TUI.
    func physicalTreeSignature() -> PolarStationNativePhysicalTreeSignature {
        PolarStationNativePhysicalTreeSignature(
            workspaceID: id,
            tabs: tabs.map { tab in
                PolarStationNativePhysicalTreeSignature.Tab(
                    id: tab.id,
                    mode: tab.mode,
                    mainPaneID: tab.mainPaneID,
                    canonicalPaneIDs: tab.panes.map(\.id),
                    visualPaneIDs: tab.visualPaneIDs,
                    panes: tab.panes.map { pane in
                        PolarStationNativePhysicalTreeSignature.Pane(
                            id: pane.id,
                            executionTargetID: pane.executionTargetID,
                            tmuxSessionID: pane.tmuxSessionID,
                            content: pane.content
                        )
                    }
                )
            }
        )
    }

    func candidate(
        replacing base: PolarStationNativeWorkspace
    ) -> WorkspaceStructureCandidateV1 {
        WorkspaceStructureCandidateV1(
            baseStructureRevision: base.revision,
            baseStructureFingerprint: base.structureFingerprint,
            structure: structureSnapshot()
        )
    }

    mutating func apply(
        _ mutation: PolarStationNativeWorkspaceMutation
    ) throws {
        switch mutation {
        case let .replaceLostSession(selection):
            guard id == selection.workspaceID else { throw PolarStationNativeWorkspaceError.workspaceChanged }
            let all = tabs.flatMap(\.panes)
            let old = all.filter { $0.executionTargetID == selection.identity.executionTargetID && $0.tmuxSessionID == selection.identity.tmuxSessionID }
            let existing = all.contains { $0.executionTargetID == selection.replacement.executionTargetID && $0.tmuxSessionID == selection.replacement.tmuxSessionID }
            guard old.isEmpty || !existing else { throw PolarStationNativeWorkspaceError.replacementAlreadyAssigned }
            if old.isEmpty { return }
            for tab in tabs.indices {
                for pane in tabs[tab].panes.indices where tabs[tab].panes[pane].executionTargetID == selection.identity.executionTargetID && tabs[tab].panes[pane].tmuxSessionID == selection.identity.tmuxSessionID {
                    tabs[tab].panes[pane] = .init(
                        id: tabs[tab].panes[pane].id,
                        executionTargetID: selection.replacement.executionTargetID,
                        tmuxSessionID: selection.replacement.tmuxSessionID,
                        transportStatus: .recovering
                    )
                }
            }
        case let .renameWorkspace(nextName):
            guard !nextName.trimmingCharacters(
                in: .whitespacesAndNewlines
            ).isEmpty
            else { throw PolarStationNativeWorkspaceError.invalidStructure }
            name = nextName
        case let .setManagedTerminalFontSize(points):
            guard (10 ... 32).contains(points)
            else { throw PolarStationNativeWorkspaceError.invalidStructure }
            managedTerminalFontSizePoints = points
        case let .addTab(id, title):
            guard !id.isEmpty,
                  !title.isEmpty,
                  !tabs.contains(where: { $0.id == id })
            else { throw PolarStationNativeWorkspaceError.invalidStructure }
            tabs.append(.init(
                id: id,
                title: title,
                mode: .monitor,
                mainPaneID: nil,
                panes: []
            ))
            selectedTabID = id
            selectedPaneID = nil
        case let .selectTab(id):
            guard tabs.contains(where: { $0.id == id })
            else { throw PolarStationNativeWorkspaceError.invalidStructure }
            selectedTabID = id
            selectedPaneID = selectedTab?.panes.first?.id
        case let .renameTab(id, title):
            guard !title.isEmpty, let index = tabIndex(id)
            else { throw PolarStationNativeWorkspaceError.invalidStructure }
            tabs[index].title = title
        case let .deleteTab(id, replacementTabID):
            guard let index = tabIndex(id)
            else { throw PolarStationNativeWorkspaceError.invalidStructure }
            if tabs.count == 1 {
                guard let replacementTabID,
                      !replacementTabID.isEmpty,
                      replacementTabID != id
                else { throw PolarStationNativeWorkspaceError.missingNativeID }
                tabs = [.init(
                    id: replacementTabID,
                    title: "Main",
                    mode: .monitor,
                    mainPaneID: nil,
                    panes: []
                )]
                selectedTabID = replacementTabID
                selectedPaneID = nil
            } else {
                guard replacementTabID == nil
                else { throw PolarStationNativeWorkspaceError.invalidStructure }
                tabs.remove(at: index)
                if selectedTabID == id {
                    let selectedIndex = min(index, tabs.count - 1)
                    selectedTabID = tabs[selectedIndex].id
                    selectedPaneID = tabs[selectedIndex].panes.first?.id
                }
            }
        case let .reorderTab(id, toIndex):
            guard let fromIndex = tabIndex(id), tabs.indices.contains(toIndex)
            else { throw PolarStationNativeWorkspaceError.invalidStructure }
            let tab = tabs.remove(at: fromIndex)
            tabs.insert(tab, at: toIndex)
        case let .reorderPane(tabID, paneID, toIndex):
            guard let tabIndex = tabIndex(tabID),
                  let fromIndex = tabs[tabIndex].panes.firstIndex(
                      where: { $0.id == paneID }
                  ),
                  tabs[tabIndex].panes.indices.contains(toIndex)
            else { throw PolarStationNativeWorkspaceError.invalidBinding }
            let pane = tabs[tabIndex].panes.remove(at: fromIndex)
            tabs[tabIndex].panes.insert(pane, at: toIndex)
            if let fromVisual = tabs[tabIndex].visualPaneIDs.firstIndex(of: paneID) {
                tabs[tabIndex].visualPaneIDs.remove(at: fromVisual)
                tabs[tabIndex].visualPaneIDs.insert(paneID, at: toIndex)
            }
            if tabs[tabIndex].mode == .focus {
                tabs[tabIndex].mainPaneID = tabs[tabIndex].visualPaneIDs.first
            }
        case let .movePane(paneID, fromTabID, toTabID, toIndex):
            guard fromTabID != toTabID,
                  let sourceIndex = tabIndex(fromTabID),
                  let targetIndex = tabIndex(toTabID),
                  let paneIndex = tabs[sourceIndex].panes.firstIndex(
                      where: { $0.id == paneID }
                  ),
                  tabs[targetIndex].panes.count < 9,
                  (0 ... tabs[targetIndex].panes.count).contains(toIndex)
            else { throw PolarStationNativeWorkspaceError.invalidBinding }
            let pane = tabs[sourceIndex].panes.remove(at: paneIndex)
            repairSelectionAfterRemoval(
                removedPaneID: paneID,
                tabIndex: sourceIndex,
                removedIndex: paneIndex
            )
            tabs[sourceIndex].visualPaneIDs.removeAll { $0 == paneID }
            if tabs[sourceIndex].mode == .focus { tabs[sourceIndex].mainPaneID = tabs[sourceIndex].visualPaneIDs.first }
            tabs[targetIndex].panes.insert(pane, at: toIndex)
            tabs[targetIndex].visualPaneIDs.insert(paneID, at: toIndex)
            selectedTabID = toTabID
            selectedPaneID = paneID
            if tabs[targetIndex].mode == .focus {
                tabs[targetIndex].mainPaneID = tabs[targetIndex].visualPaneIDs.first
            }
        case let .toggleMode(id):
            guard let index = tabIndex(id)
            else { throw PolarStationNativeWorkspaceError.invalidStructure }
            switch tabs[index].mode {
            case .monitor:
                let mainPaneID = selectedPane(in: tabs[index])
                    ?? tabs[index].panes.first?.id
                tabs[index].mode = .focus
                tabs[index].mainPaneID = mainPaneID
                if selectedTabID == id {
                    selectedPaneID = mainPaneID
                }
            case .focus:
                let formerMainPaneID = tabs[index].mainPaneID
                tabs[index].mode = .monitor
                tabs[index].mainPaneID = nil
                if selectedTabID == id {
                    selectedPaneID = formerMainPaneID
                }
            }
        case let .selectPane(id):
            guard let location = paneLocation(id),
                  tabs[location.tab].id == selectedTabID
            else { throw PolarStationNativeWorkspaceError.invalidBinding }
            selectedPaneID = id
        case let .activatePane(id):
            guard let location = paneLocation(id)
            else { throw PolarStationNativeWorkspaceError.invalidBinding }
            selectedTabID = tabs[location.tab].id
            selectedPaneID = id
            tabs[location.tab].mode = .focus
            try promoteFocusPane(id, in: location.tab)
        case let .cycleFocus(direction):
            guard let index = tabIndex(selectedTabID),
                  tabs[index].mode == .focus,
                  tabs[index].panes.count > 1,
                  let mainPaneID = tabs[index].mainPaneID,
                  let canonicalIndex = tabs[index].panes.firstIndex(
                      where: { $0.id == mainPaneID }
                  )
            else { return }
            let targetIndex: Int
            switch direction {
            case .previous:
                targetIndex = canonicalIndex == 0
                    ? tabs[index].panes.count - 1
                    : canonicalIndex - 1
            case .next:
                targetIndex = (canonicalIndex + 1)
                    % tabs[index].panes.count
            }
            let targetPaneID = tabs[index].panes[targetIndex].id
            try promoteFocusPane(targetPaneID, in: index)
            selectedPaneID = targetPaneID
        case let .replaceContent(paneID, content):
            guard let location = paneLocation(paneID), tabs[location.tab].id == selectedTabID,
                  tabs[location.tab].panes[location.pane].content != nil, content.isValid
            else { throw PolarStationNativeWorkspaceError.invalidBinding }
            tabs[location.tab].panes[location.pane].content = content
            selectedPaneID = paneID
        case let .openContent(tabID, paneID, content):
            guard let index = tabIndex(tabID), !paneID.isEmpty, content.isValid,
                  paneLocation(paneID) == nil, tabs[index].panes.count < 9
            else { throw PolarStationNativeWorkspaceError.invalidBinding }
            tabs[index].panes.append(PolarStationNativePane(id: paneID,
                executionTargetID: nil, tmuxSessionID: nil, content: content))
            selectedTabID = tabID; selectedPaneID = paneID
            if tabs[index].mode == .focus { tabs[index].mainPaneID = paneID }
        case let .removePane(id):
            guard let location = paneLocation(id)
            else { throw PolarStationNativeWorkspaceError.invalidBinding }
            tabs[location.tab].panes.remove(at: location.pane)
            repairSelectionAfterRemoval(
                removedPaneID: id,
                tabIndex: location.tab,
                removedIndex: location.pane
            )
        case let .addSessions(tabID, sessions):
            guard let index = tabIndex(tabID) else { throw PolarStationNativeWorkspaceError.invalidBinding }
            let tab = tabs[index]
            var seen = Set(tab.panes.map { "\($0.executionTargetID ?? "")\n\($0.tmuxSessionID ?? "")" })
            let additions = sessions.filter { seen.insert("\($0.executionTargetID)\n\($0.tmuxSessionID)").inserted }
            guard tab.panes.count + additions.count <= 9 else { throw PolarStationNativeWorkspaceError.invalidBinding }
            guard !additions.isEmpty else { return }
            let previousFocus = selectedPaneID
            for (index, session) in additions.enumerated() {
                try apply(.toggleSessionAssignment(tabID: tabID, executionTargetID: session.executionTargetID,
                    tmuxSessionID: session.tmuxSessionID, paneID: UUID().uuidString.lowercased(), focusCreated: tab.panes.isEmpty && index == 0))
            }
            if selectedTabID == tabID { selectedPaneID = tab.panes.isEmpty ? tabs[index].panes.first?.id : previousFocus }
        case let .toggleSessionAssignment(
            tabID,
            executionTargetID,
            tmuxSessionID,
            paneID,
            focusCreated
        ):
            guard let tab = tabIndex(tabID)
            else { throw PolarStationNativeWorkspaceError.invalidStructure }
            if let existing = tabs[tab].panes.firstIndex(where: {
                $0.executionTargetID == executionTargetID
                    && $0.tmuxSessionID == tmuxSessionID
            }) {
                let removed = tabs[tab].panes[existing].id
                tabs[tab].panes.remove(at: existing)
                repairSelectionAfterRemoval(
                    removedPaneID: removed,
                    tabIndex: tab,
                    removedIndex: existing
                )
            } else {
                guard !paneID.isEmpty,
                      tabs[tab].panes.count < 9,
                      paneLocation(paneID) == nil
                else { throw PolarStationNativeWorkspaceError.invalidBinding }
                let pane = PolarStationNativePane(
                    id: paneID,
                    executionTargetID: executionTargetID,
                    tmuxSessionID: tmuxSessionID,
                    transportStatus: .recovering
                )
                tabs[tab].panes.append(pane)
                if selectedTabID == tabID {
                    selectedPaneID = paneID
                }
                if focusCreated && tabs[tab].mode == .focus {
                    tabs[tab].mainPaneID = paneID
                }
                if tabs[tab].mode == .focus,
                   tabs[tab].mainPaneID == nil
                {
                    tabs[tab].mainPaneID = paneID
                }
            }
        }
        normalizeVisualBindings()
        revision &+= 1
        if revision == 0 { revision = 1 }
        try validateNativeLayout()
    }

    var terminalFocusPaneID: String? {
        guard let tab = tabs.first(where: { $0.id == selectedTabID })
        else { return nil }
        switch tab.mode {
        case .monitor:
            return selectedPane(in: tab)
        case .focus:
            return tab.mainPaneID
        }
    }

    private var selectedTab: PolarStationNativeTab? {
        tabs.first(where: { $0.id == selectedTabID })
    }

    private func tabIndex(_ id: String) -> Int? {
        tabs.firstIndex(where: { $0.id == id })
    }

    private func paneLocation(
        _ id: String
    ) -> (tab: Int, pane: Int)? {
        for tab in tabs.indices {
            if let pane = tabs[tab].panes.firstIndex(
                where: { $0.id == id }
            ) {
                return (tab, pane)
            }
        }
        return nil
    }

    private func selectedPane(
        in tab: PolarStationNativeTab
    ) -> String? {
        guard let selectedPaneID,
              tab.panes.contains(where: { $0.id == selectedPaneID })
        else { return nil }
        return selectedPaneID
    }

    private mutating func repairSelectionAfterRemoval(
        removedPaneID: String,
        tabIndex: Int,
        removedIndex: Int
    ) {
        let panes = tabs[tabIndex].panes
        let replacement = panes.isEmpty
            ? nil
            : panes[min(removedIndex, panes.count - 1)].id
        if selectedPaneID == removedPaneID {
            selectedPaneID = replacement
        }
        if tabs[tabIndex].mainPaneID == removedPaneID {
            tabs[tabIndex].mainPaneID = replacement
        }
    }

    private mutating func promoteFocusPane(
        _ paneID: String,
        in tabIndex: Int
    ) throws {
        guard tabs[tabIndex].mode == .focus,
              tabs[tabIndex].panes.contains(where: { $0.id == paneID }),
              let targetSlot = tabs[tabIndex].visualPaneIDs.firstIndex(of: paneID),
              !tabs[tabIndex].visualPaneIDs.isEmpty
        else { throw PolarStationNativeWorkspaceError.invalidFocusMain }
        tabs[tabIndex].visualPaneIDs.swapAt(0, targetSlot)
        tabs[tabIndex].mainPaneID = paneID
    }

    private mutating func normalizeVisualBindings() {
        for index in tabs.indices {
            let canonical = tabs[index].panes.map(\.id)
            switch tabs[index].mode {
            case .monitor:
                tabs[index].mainPaneID = nil
                tabs[index].visualPaneIDs = canonical
            case .focus:
                var visual = tabs[index].visualPaneIDs.filter {
                    canonical.contains($0)
                }
                visual.append(contentsOf: canonical.filter { !visual.contains($0) })
                tabs[index].visualPaneIDs = visual
                guard let main = tabs[index].mainPaneID,
                      let mainSlot = visual.firstIndex(of: main),
                      !visual.isEmpty
                else { continue }
                tabs[index].visualPaneIDs.swapAt(0, mainSlot)
            }
        }
    }

    mutating func preserveNativePresentation(
        from previous: PolarStationNativeWorkspace
    ) {
        for index in tabs.indices {
            guard let prior = previous.tabs.first(where: { $0.id == tabs[index].id }),
                  prior.mode == tabs[index].mode,
                  Set(prior.panes.map(\.id)) == Set(tabs[index].panes.map(\.id))
            else { continue }
            tabs[index].visualPaneIDs = prior.visualPaneIDs
            if tabs[index].mode == .focus {
                tabs[index].mainPaneID = prior.mainPaneID
            }
        }
        if previous.selectedTabID == selectedTabID,
           let previousSelection = previous.selectedPaneID,
           selectedTab?.panes.contains(where: { $0.id == previousSelection }) == true
        {
            selectedPaneID = previousSelection
        }
        normalizeVisualBindings()
    }

    var canCycleFocus: Bool {
        guard let selectedTab else { return false }
        return selectedTab.mode == .focus && selectedTab.panes.count > 1
    }

    func validateNativeLayout() throws {
        guard !id.isEmpty,
              !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              (10 ... 32).contains(managedTerminalFontSizePoints),
              !tabs.isEmpty,
              tabs.contains(where: { $0.id == selectedTabID }),
              Set(tabs.map(\.id)).count == tabs.count
        else { throw PolarStationNativeWorkspaceError.invalidStructure }
        let panes = tabs.flatMap(\.panes)
        guard Set(panes.map(\.id)).count == panes.count else { throw PolarStationNativeWorkspaceError.invalidBinding }
        for pane in panes {
            if let content = pane.content {
                guard content.isValid, pane.executionTargetID == nil, pane.tmuxSessionID == nil else { throw PolarStationNativeWorkspaceError.invalidBinding }
            } else {
                guard let target = pane.executionTargetID, !target.isEmpty,
                      let session = pane.tmuxSessionID, !session.isEmpty else { throw PolarStationNativeWorkspaceError.invalidBinding }
            }
        }
        if let selectedPaneID {
            guard selectedTab?.panes.contains(where: { $0.id == selectedPaneID }) == true
            else { throw PolarStationNativeWorkspaceError.invalidBinding }
        }
        for tab in tabs where !tab.panes.isEmpty {
            let canonical = tab.panes.map(\.id)
            guard tab.visualPaneIDs.count == canonical.count,
                  Set(tab.visualPaneIDs) == Set(canonical),
                  tab.mode == .monitor || tab.visualPaneIDs.first == tab.mainPaneID
            else { throw PolarStationNativeWorkspaceError.invalidFocusMain }
            _ = try PolarStationNativeLayoutFamily.layout(
                orderedPaneIDs: tab.visualPaneIDs,
                mode: tab.mode,
                mainPaneID: tab.mainPaneID
            )
        }
    }
}

private struct RuntimeSessionKey: Hashable {
    let executionTargetID: String
    let tmuxSessionID: String
}

/// The complete native-owned Workspace catalog. Exactly one Workspace is
/// selected, and every Workspace carries its own Tab/Pane/Session mapping.
/// Rust never receives or mirrors this table.
struct PolarStationNativeWorkspaceCatalog: Codable, Equatable, Sendable {
    var revision: UInt64
    var selectedWorkspaceID: String
    var workspaces: [PolarStationNativeWorkspace]
    var archivedWorkspaceIDs: [String]? = nil
    var openWorkspaceIDs: [String]? = nil

    enum CodingKeys: String, CodingKey {
        case revision = "catalog_revision"
        case selectedWorkspaceID = "selected_workspace_id"
        case workspaces
        case archivedWorkspaceIDs = "archived_workspace_ids"
        case openWorkspaceIDs = "open_workspace_ids"
    }

    var selectedWorkspace: PolarStationNativeWorkspace {
        workspaces.first { $0.id == selectedWorkspaceID }!
    }

    mutating func replaceSelected(
        with workspace: PolarStationNativeWorkspace,
        advanceRevision: Bool = true
    ) throws {
        try requireRevisionCapacity(if: advanceRevision)
        guard workspace.id == selectedWorkspaceID,
              let index = workspaces.firstIndex(where: {
                  $0.id == selectedWorkspaceID
              })
        else { throw PolarStationNativeWorkspaceError.invalidStructure }
        workspaces[index] = workspace
        try validate()
        if advanceRevision { revision += 1 }
    }

    mutating func createAndSelect(
        id: String,
        name: String,
        tabID: String,
        managedTerminalFontSizePoints: UInt8,
        advanceRevision: Bool = true
    ) throws {
        try requireRevisionCapacity(if: advanceRevision)
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !id.isEmpty,
              !tabID.isEmpty,
              !trimmed.isEmpty,
              !workspaces.contains(where: { $0.id == id })
        else { throw PolarStationNativeWorkspaceError.invalidStructure }
        var resolvedName = trimmed
        if trimmed == "Workspace" {
            var suffix = 2
            while workspaces.contains(where: { $0.name == resolvedName }) {
                resolvedName = "Workspace \(suffix)"
                suffix += 1
            }
        }
        let workspace = PolarStationNativeWorkspace(
            id: id,
            name: resolvedName,
            revision: 1,
            selectedTabID: tabID,
            selectedPaneID: nil,
            managedTerminalFontSizePoints: managedTerminalFontSizePoints,
            tabs: [.init(
                id: tabID,
                title: "Main",
                mode: .monitor,
                mainPaneID: nil,
                panes: []
            )]
        )
        workspaces.append(workspace)
        selectedWorkspaceID = id
        try validate()
        if advanceRevision { revision += 1 }
    }

    mutating func select(_ workspaceID: String, openingUnboundWindow: Bool = false) throws {
        guard !(archivedWorkspaceIDs ?? []).contains(workspaceID), workspaces.contains(where: { $0.id == workspaceID })
        else { throw PolarStationNativeWorkspaceError.invalidStructure }
        guard workspaceID != selectedWorkspaceID || openingUnboundWindow else { return }
        try requireRevisionCapacity(if: true)
        selectedWorkspaceID = workspaceID
        revision += 1
    }

    func lifecycle(_ action: String, bound: String?, target: String? = nil) throws -> WorkspaceLifecycleDecision {
        try decideWorkspaceLifecycle(action: action, ids: workspaces.map(\.id),
            archived: archivedWorkspaceIDs ?? [], open: openWorkspaceIDs ?? [],
            selected: selectedWorkspaceID, bound: bound, target: target)
    }

    /// One candidate owns both sides; a rejected transfer leaves the receiver unchanged.
    mutating func moveTab(_ tabID: String, from sourceID: String, to targetID: String, replacementTabID: String) throws {
        var next = self
        guard sourceID != targetID,
              !(archivedWorkspaceIDs ?? []).contains(targetID),
              let source = next.workspaces.firstIndex(where: { $0.id == sourceID }),
              let target = next.workspaces.firstIndex(where: { $0.id == targetID }),
              let tab = next.workspaces[source].tabs.first(where: { $0.id == tabID }),
              !next.workspaces[target].tabs.contains(where: { $0.id == tabID }),
              next.revision < UInt64.max, next.workspaces[target].revision < UInt64.max else {
            throw PolarStationNativeWorkspaceError.invalidStructure
        }
        try next.workspaces[source].apply(.deleteTab(tabID: tabID,
            replacementTabID: next.workspaces[source].tabs.count == 1 ? replacementTabID : nil))
        next.workspaces[target].tabs.append(tab)
        next.workspaces[target].selectedTabID = tabID
        next.workspaces[target].selectedPaneID = tab.mainPaneID ?? tab.panes.first?.id
        next.workspaces[target].revision += 1
        for index in [source, target] {
            next.workspaces[index].structureFingerprint = next.workspaces[index].structureSnapshot().structureFingerprint
        }
        next.revision += 1
        try next.validate()
        self = next
    }

    mutating func archive(_ workspaceID: String) throws {
        let decision = try lifecycle("archive", bound: workspaceID, target: workspaceID)
        try requireRevisionCapacity(if: true)
        archivedWorkspaceIDs = (archivedWorkspaceIDs ?? []) + [workspaceID]
        openWorkspaceIDs?.removeAll { $0 == workspaceID }
        selectedWorkspaceID = decision.selected
        revision += 1
        try validate()
    }

    mutating func restore(_ workspaceID: String) throws {
        _ = try lifecycle("restore", bound: nil, target: workspaceID)
        try requireRevisionCapacity(if: true)
        archivedWorkspaceIDs?.removeAll { $0 == workspaceID }
        revision += 1
        try validate()
    }

    // Old callers must fail rather than destroy a retained definition.
    mutating func deleteSelected(replacementWorkspaceID: String, replacementTabID: String) throws {
        _ = try lifecycle("delete", bound: selectedWorkspaceID)
    }

    func validate() throws {
        guard revision > 0,
              !workspaces.isEmpty,
              Set(workspaces.map(\.id)).count == workspaces.count,
              workspaces.contains(where: { $0.id == selectedWorkspaceID })
        else { throw PolarStationNativeWorkspaceError.invalidStructure }
        _ = try lifecycle("inspect", bound: nil)
        let open = openWorkspaceIDs ?? []
        guard Set(open).count == open.count, open.allSatisfy({ id in workspaces.contains { $0.id == id } && !(archivedWorkspaceIDs ?? []).contains(id) }) else {
            throw PolarStationNativeWorkspaceError.invalidStructure
        }
        try workspaces.forEach { try $0.validateNativeLayout() }
    }

    private func requireRevisionCapacity(if advancing: Bool) throws {
        if advancing, revision == UInt64.max {
            throw PolarStationNativeWorkspaceError.invalidStructure
        }
    }
}

struct PolarStationNativeWorkspacePersistence {
    private struct VersionEnvelope: Decodable {
        let schemaVersion: UInt32

        enum CodingKeys: String, CodingKey {
            case schemaVersion = "schema_version"
        }
    }

    private struct Envelope: Codable {
        let schemaVersion: UInt32
        let catalogRevision: UInt64
        let selectedWorkspaceID: String
        let workspaces: [PolarStationNativeWorkspace]
        var archivedWorkspaceIDs: [String]? = nil
        var openWorkspaceIDs: [String]? = nil

        enum CodingKeys: String, CodingKey {
            case schemaVersion = "schema_version"
            case catalogRevision = "catalog_revision"
            case selectedWorkspaceID = "selected_workspace_id"
            case workspaces
            case archivedWorkspaceIDs = "archived_workspace_ids"
            case openWorkspaceIDs = "open_workspace_ids"
        }
    }

    static let schemaVersion: UInt32 = 2
    let url: URL

    static func applicationDefault() throws -> Self {
        guard let root = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first else {
            throw PolarStationNativeWorkspacePersistenceError
                .applicationSupportUnavailable
        }
        let canonicalURL = root.appendingPathComponent("com.polarealm.PolarStation", isDirectory: true)
            .appendingPathComponent("native-layout.json")
        return Self(url: canonicalURL)
    }

    func load() throws -> PolarStationNativeWorkspaceCatalog? {
        guard FileManager.default.fileExists(atPath: url.path) else {
            return nil
        }
        let data = try Data(contentsOf: url)
        let decoder = JSONDecoder()
        let version = try decoder.decode(
            VersionEnvelope.self,
            from: data
        ).schemaVersion
        guard version == 1 || version == Self.schemaVersion else {
            throw PolarStationNativeWorkspacePersistenceError.unsupportedSchema(version)
        }
        let envelope = try decoder.decode(Envelope.self, from: data)
        var catalog = PolarStationNativeWorkspaceCatalog(
            revision: envelope.catalogRevision, selectedWorkspaceID: envelope.selectedWorkspaceID,
            workspaces: envelope.workspaces, archivedWorkspaceIDs: envelope.archivedWorkspaceIDs, openWorkspaceIDs: envelope.openWorkspaceIDs
        )
        if !catalog.workspaces.contains(where: { $0.id == catalog.selectedWorkspaceID && !(catalog.archivedWorkspaceIDs ?? []).contains($0.id) }),
           let first = catalog.workspaces.first(where: { !(catalog.archivedWorkspaceIDs ?? []).contains($0.id) }) {
            catalog.selectedWorkspaceID = first.id
        }
        try catalog.validate()
        return catalog
    }

    func save(_ catalog: PolarStationNativeWorkspaceCatalog) throws {
        try catalog.validate()
        let directory = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700]
        )
        let data = try JSONEncoder().encode(Envelope(
            schemaVersion: Self.schemaVersion,
            catalogRevision: catalog.revision,
            selectedWorkspaceID: catalog.selectedWorkspaceID,
            workspaces: catalog.workspaces, archivedWorkspaceIDs: catalog.archivedWorkspaceIDs, openWorkspaceIDs: catalog.openWorkspaceIDs
        ))
        try data.write(to: url, options: .atomic)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o600],
            ofItemAtPath: url.path
        )
        guard let readBack = try load(),
              Self.persistenceEquivalent(readBack, catalog)
        else {
            throw PolarStationNativeWorkspacePersistenceError
                .readBackMismatch
        }
    }

    private static func persistenceEquivalent(
        _ left: PolarStationNativeWorkspaceCatalog,
        _ right: PolarStationNativeWorkspaceCatalog
    ) -> Bool {
        persistenceProjection(left) == persistenceProjection(right)
    }

    private static func persistenceProjection(
        _ source: PolarStationNativeWorkspaceCatalog
    ) -> PolarStationNativeWorkspaceCatalog {
        var projected = source
        for workspaceIndex in projected.workspaces.indices {
            for tabIndex in projected.workspaces[workspaceIndex].tabs.indices {
                for paneIndex in projected.workspaces[workspaceIndex]
                    .tabs[tabIndex].panes.indices
                {
                    projected.workspaces[workspaceIndex].tabs[tabIndex]
                        .panes[paneIndex].transportStatus = .offline
                }
            }
        }
        return projected
    }
}

enum PolarStationNativeWorkspacePersistenceError: Error, Equatable {
    case applicationSupportUnavailable
    case unsupportedSchema(UInt32)
    case readBackMismatch
}

enum PolarStationNativeWorkspaceMutation: Equatable {
    case replaceLostSession(SessionReplacementSelectionV1)
    case renameWorkspace(String)
    case setManagedTerminalFontSize(UInt8)
    case addTab(id: String, title: String)
    case selectTab(String)
    case renameTab(id: String, title: String)
    case deleteTab(tabID: String, replacementTabID: String?)
    case reorderTab(tabID: String, toIndex: Int)
    case reorderPane(tabID: String, paneID: String, toIndex: Int)
    case movePane(
        paneID: String,
        fromTabID: String,
        toTabID: String,
        toIndex: Int
    )
    case toggleMode(String)
    case selectPane(String)
    case activatePane(String)
    case cycleFocus(PolarTmuxFocusCycleDirection)
    case removePane(String)
    case openContent(tabID: String, paneID: String, content: StationPaneContentV1)
    case replaceContent(paneID: String, content: StationPaneContentV1)
    case addSessions(tabID: String, sessions: [SessionRowProjectionV1.Identity])
    case toggleSessionAssignment(
        tabID: String,
        executionTargetID: String,
        tmuxSessionID: String,
        paneID: String,
        focusCreated: Bool = false
    )
}

extension PolarStationNativeWorkspace {
    func mutation(
        for operation: ControlCenterIntentOperationV1
    ) throws -> PolarStationNativeWorkspaceMutation? {
        switch operation {
        case let .useExistingSession(selection):
            return .replaceLostSession(selection)
        case let .renameWorkspace(name):
            return .renameWorkspace(name)
        case let .setManagedTerminalFontSize(points):
            guard let points = UInt8(exactly: points),
                  (10 ... 32).contains(points)
            else { throw PolarStationNativeWorkspaceError.invalidStructure }
            return .setManagedTerminalFontSize(points)
        case let .addTab(title, tabID):
            guard let tabID
            else { throw PolarStationNativeWorkspaceError.missingNativeID }
            return .addTab(id: tabID, title: title)
        case let .selectTab(tabID):
            return .selectTab(tabID)
        case let .renameTab(tabID, title):
            return .renameTab(id: tabID, title: title)
        case let .deleteTab(tabID, _, replacementTabID):
            return .deleteTab(
                tabID: tabID,
                replacementTabID: replacementTabID
            )
        case let .reorderTab(tabID, toIndex):
            return .reorderTab(tabID: tabID, toIndex: toIndex)
        case let .reorderPane(tabID, paneID, toIndex):
            return .reorderPane(
                tabID: tabID,
                paneID: paneID,
                toIndex: toIndex
            )
        case let .movePane(
            paneID,
            fromTabID,
            toTabID,
            toIndex
        ):
            return .movePane(
                paneID: paneID,
                fromTabID: fromTabID,
                toTabID: toTabID,
                toIndex: toIndex
            )
        case let .toggleDisplayMode(tabID):
            return .toggleMode(tabID)
        case let .addSessionAssignments(tabID, sessions):
            return .addSessions(tabID: tabID, sessions: sessions)
        case let .toggleSessionAssignment(identity, tabID, paneID):
            guard let tabID, !tabID.isEmpty, let paneID
            else { throw PolarStationNativeWorkspaceError.missingNativeID }
            return .toggleSessionAssignment(
                tabID: tabID,
                executionTargetID: identity.executionTargetID,
                tmuxSessionID: identity.tmuxSessionID,
                paneID: paneID
            )
        case let .selectPane(paneID):
            return .selectPane(paneID)
        case let .activatePane(paneID):
            return .activatePane(paneID)
        case let .removePane(paneID):
            return .removePane(paneID)
        case let .invokeProjectedAction(actionID)
            where actionID == "workspace.mode.toggle":
            return .toggleMode(selectedTabID)
        default:
            return nil
        }
    }
}

enum PolarStationNativeWorkspaceError: Error, Equatable, LocalizedError {
    case workspaceChanged
    var errorDescription: String? {
        switch self {
        case .workspaceChanged: "The selected Workspace changed. Choose the Session again."
        case .replacementAlreadyAssigned: "Already assigned in this Workspace"
        default: nil
        }
    }
    case replacementAlreadyAssigned
    case invalidStructure
    case invalidBinding
    case invalidFocusMain
    case missingNativeID
}

private enum PolarStationStructureFingerprint {
    static func compute(
        _ structure: WorkspaceStructureSnapshotV1
    ) -> String {
        var bytes = [UInt8]()
        append("polar-station-workspace-structure-v1", to: &bytes)
        append(structure.workspaceID, to: &bytes)
        append(structure.workspaceName, to: &bytes)
        append(structure.selectedTabID, to: &bytes)
        append(structure.selectedPaneID, to: &bytes)
        append(UInt64(structure.tabs.count), to: &bytes)
        for tab in structure.tabs {
            append(tab.id, to: &bytes)
            append(tab.title, to: &bytes)
            append(tab.displayMode == .focus ? "focus" : "monitor", to: &bytes)
            append(tab.mainPaneID, to: &bytes)
            append(tab.canonicalPaneIDs, to: &bytes)
            append(tab.visualPaneIDs, to: &bytes)
            append(UInt64(tab.panes.count), to: &bytes)
            for pane in tab.panes {
                append(pane.id, to: &bytes)
                if let content = pane.content {
                    for component in content.fingerprintComponents { append(component, to: &bytes) }
                } else {
                    append(pane.executionTargetID ?? "", to: &bytes)
                    append(pane.tmuxSessionID ?? "", to: &bytes)
                }
            }
        }
        return SHA256.hash(data: Data(bytes))
            .map { String(format: "%02x", $0) }
            .joined()
    }

    private static func append(_ value: UInt64, to bytes: inout [UInt8]) {
        withUnsafeBytes(of: value.bigEndian) {
            bytes.append(contentsOf: $0)
        }
    }

    private static func append(_ value: String, to bytes: inout [UInt8]) {
        let encoded = Array(value.utf8)
        append(UInt64(encoded.count), to: &bytes)
        bytes.append(contentsOf: encoded)
    }

    private static func append(_ value: String?, to bytes: inout [UInt8]) {
        guard let value else {
            bytes.append(0)
            return
        }
        bytes.append(1)
        append(value, to: &bytes)
    }

    private static func append(_ values: [String], to bytes: inout [UInt8]) {
        append(UInt64(values.count), to: &bytes)
        for value in values {
            append(value, to: &bytes)
        }
    }
}
