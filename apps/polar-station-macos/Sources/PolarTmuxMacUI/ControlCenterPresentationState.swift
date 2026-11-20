import CoreGraphics
import SwiftUI

public enum ControlCenterTab: Equatable, Sendable {
    case layout, inbox, settings
}

public enum ControlCenterPresentationAction: Equatable, Sendable {
    case selectTab(ControlCenterTab)
    case pressLayout
    case expandLayout
    case toggleSessions
    case resize(width: CGFloat, commit: Bool)
}

/// Native geometry only; the existing page owners retain their content/drafts.
public struct ControlCenterBodyPresentation: Equatable, Sendable {
    public fileprivate(set) var selectedTab: ControlCenterTab
    public fileprivate(set) var sessionsVisible: Bool
    public fileprivate(set) var mapWidth: CGFloat

    public init(selectedTab: ControlCenterTab = .layout, sessionsVisible: Bool = false,
                mapWidth: CGFloat = ControlCenterWidthPolicy.maximumMapWidth) {
        self.selectedTab = selectedTab
        self.sessionsVisible = sessionsVisible
        self.mapWidth = ControlCenterWidthPolicy.clampMapWidth(mapWidth)
    }
    public var layoutMapVisible: Bool { selectedTab == .layout }
    public var inboxVisible: Bool { selectedTab == .inbox }
    public var dividerVisible: Bool { false }
    public var preferredWidth: CGFloat {
        ControlCenterWidthPolicy.controllerWidth(tab: selectedTab, mapWidth: mapWidth,
            sessionsVisible: false)
    }
}

/// One per-window owner for selection, measurement and the outer resize gesture.
@MainActor
public final class ControlCenterPresentationState: ObservableObject {
    @Published public private(set) var body: ControlCenterBodyPresentation
    private var restoredTab = false
    private var resizing = false
    private var observedMapWidth: CGFloat?

    public init(sessionsVisible: Bool = false,
                inboxVisible: Bool = false,
                mapWidth: CGFloat = ControlCenterWidthPolicy.maximumMapWidth) {
        body = ControlCenterBodyPresentation(selectedTab: inboxVisible ? .inbox : .layout,
            sessionsVisible: sessionsVisible, mapWidth: mapWidth)
    }

    /// Legacy visibility is input once, never a refresh-time page selection.
    public func restoreLegacyTab(inboxVisible: Bool) {
        guard !restoredTab else { return }
        restoredTab = true
        body.selectedTab = inboxVisible ? .inbox : .layout
    }

    /// An unrelated settings refresh must not replay the old width over a drag.
    public func applyWidth(_ mapWidth: CGFloat) {
        guard !resizing else { return }
        if observedMapWidth != mapWidth {
            observedMapWidth = mapWidth
            body.mapWidth = ControlCenterWidthPolicy.clampMapWidth(mapWidth)
        }
    }

    @discardableResult
    public func send(_ action: ControlCenterPresentationAction,
                     availableWidth: CGFloat = .greatestFiniteMagnitude) -> Bool {
        var next = body
        switch action {
        case let .selectTab(tab):
            next.selectedTab = tab
            if tab != .layout { next.sessionsVisible = false }
        case .pressLayout:
            next.sessionsVisible = next.selectedTab == .layout && !next.sessionsVisible
            next.selectedTab = .layout
        case .expandLayout:
            next.selectedTab = .layout
            next.sessionsVisible = true
        case .toggleSessions:
            next.selectedTab = .layout
            next.sessionsVisible.toggle()
        case let .resize(width, _):
            let requested = min(width, availableWidth - 321)
            let extra = body.dividerVisible
                    ? ControlCenterWidthPolicy.sessionsWidth + ControlCenterWidthPolicy.dividerWidth : 0
            next.mapWidth = ControlCenterWidthPolicy.clampMapWidth(requested - extra)
        }
        guard next.preferredWidth + 321 <= availableWidth else { return false }
        switch action {
        case .selectTab, .pressLayout, .expandLayout: restoredTab = true
        case let .resize(_, commit): resizing = !commit
        case .toggleSessions: break
        }
        body = next
        return true
    }
}
