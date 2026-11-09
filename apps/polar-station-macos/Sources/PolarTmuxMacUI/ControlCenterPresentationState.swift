import CoreGraphics
import SwiftUI

public enum ControlCenterPresentationAction: Equatable, Sendable {
    case toggleSessions
    case setLayoutMapVisible(Bool)
    case setInboxVisible(Bool)
    case resizeMap(width: CGFloat, commit: Bool)
    case resizeInbox(width: CGFloat, commit: Bool)
}

/// The single native owner of Control Center body presentation.
///
/// Workspace selection and Session assignment deliberately do not enter this
/// state. A cached Control Center host can therefore move between native Tabs
/// without changing which body regions are visible.
@MainActor
public final class ControlCenterPresentationState: ObservableObject {
    @Published public private(set) var body: ControlCenterBodyPresentation

    public init(
        sessionsVisible: Bool = false,
        layoutMapVisible: Bool = true,
        inboxVisible: Bool = false,
        mapWidth: CGFloat = ControlCenterWidthPolicy.maximumMapWidth,
        inboxWidth: CGFloat = ControlCenterWidthPolicy.defaultInboxWidth
    ) {
        body = ControlCenterBodyPresentation(
            sessionsVisible: sessionsVisible,
            layoutMapVisible: layoutMapVisible,
            inboxVisible: inboxVisible,
            mapWidth: mapWidth,
            inboxWidth: inboxWidth
        )
    }

    @discardableResult
    public func send(_ action: ControlCenterPresentationAction, availableWidth: CGFloat = .greatestFiniteMagnitude) -> Bool {
        var next = body
        switch action {
        case .toggleSessions:
            next = ControlCenterBodyPresentation(
                sessionsVisible: !body.sessionsVisible,
                layoutMapVisible: body.layoutMapVisible,
                inboxVisible: body.inboxVisible,
                mapWidth: body.mapWidth,
                inboxWidth: body.inboxWidth
            )
        case let .setLayoutMapVisible(visible):
            next = ControlCenterBodyPresentation(
                sessionsVisible: body.sessionsVisible,
                layoutMapVisible: visible,
                inboxVisible: body.inboxVisible,
                mapWidth: body.mapWidth,
                inboxWidth: body.inboxWidth
            )
        case let .setInboxVisible(visible):
            next = ControlCenterBodyPresentation(
                sessionsVisible: body.sessionsVisible,
                layoutMapVisible: body.layoutMapVisible,
                inboxVisible: visible,
                mapWidth: body.mapWidth,
                inboxWidth: body.inboxWidth
            )
        case let .resizeMap(width, _):
            next = ControlCenterBodyPresentation(
                sessionsVisible: body.sessionsVisible,
                layoutMapVisible: body.layoutMapVisible,
                inboxVisible: body.inboxVisible,
                mapWidth: width,
                inboxWidth: body.inboxWidth
            )
        case let .resizeInbox(width, _):
            next = ControlCenterBodyPresentation(
                sessionsVisible: body.sessionsVisible,
                layoutMapVisible: body.layoutMapVisible,
                inboxVisible: body.inboxVisible,
                mapWidth: body.mapWidth,
                inboxWidth: width
            )
        }
        guard next.preferredWidth + 321 <= availableWidth else { return false }
        body = next
        return true
    }
}
