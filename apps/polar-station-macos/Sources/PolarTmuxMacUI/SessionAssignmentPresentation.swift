import PolarTmuxHostClient

/// Native, click-time assignment state for one Session. The Host projects
/// copy and policy, while this value joins those facts to the live Workspace
/// table owned by the macOS presentation adapter.
public enum SessionAssignmentPresentation: Equatable, Sendable {
    case noManagedTab
    case available(tabID: String)
    case assignedHere(tabID: String, paneID: String)
    case assignedElsewhere(tabID: String)
    case tabFull(tabID: String)

    public static func classify(
        executionTargetID: String,
        tmuxSessionID: String,
        nativeView: NativeWorkspaceLayoutView?
    ) -> Self {
        guard let nativeView,
              let selectedTab = nativeView.tab(nativeView.selectedTabID)
        else { return .noManagedTab }

        if let pane = selectedTab.panes.first(where: {
            $0.executionTargetID == executionTargetID
                && $0.tmuxSessionID == tmuxSessionID
        }) {
            return .assignedHere(
                tabID: selectedTab.tabID,
                paneID: pane.paneID
            )
        }

        if selectedTab.panes.count >= 9 {
            return .tabFull(tabID: selectedTab.tabID)
        }
        return .available(tabID: selectedTab.tabID)
    }

    var assignedToSelectedTab: Bool {
        if case .assignedHere = self { return true }
        return false
    }

    func action(
        policy: SessionAssignmentActionsV1?,
        fallback: ActionProjectionV1
    ) -> ActionProjectionV1 {
        guard let policy else { return fallback }
        switch self {
        case .available:
            return policy.add
        case .assignedHere:
            return policy.assignedElsewhere
        case .assignedElsewhere:
            return policy.assignedElsewhere
        case .tabFull:
            return policy.tabFull
        case .noManagedTab:
            return policy.noManagedTab
        }
    }

    var target: (tabID: String, paneID: String?)? {
        switch self {
        case let .available(tabID), let .tabFull(tabID):
            return (tabID, nil)
        case let .assignedHere(tabID, paneID):
            return (tabID, paneID)
        case .assignedElsewhere, .noManagedTab:
            return nil
        }
    }
}
