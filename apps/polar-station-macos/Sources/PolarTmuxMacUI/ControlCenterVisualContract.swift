import Foundation

public struct ControlCenterVisualUnit:
    Equatable,
    Sendable
{
    public let id: String
    public let symbol: String

    public init(id: String, symbol: String) {
        self.id = id
        self.symbol = symbol
    }
}

public enum ControlCenterVisualPresentation:
    Equatable,
    Sendable
{
    case iconOnly
    case iconOverLabel
}

public enum ControlHeaderVisualContract {
    public static let units: [ControlCenterVisualUnit] = [
        .init(id: "workspace", symbol: "chevron.up.chevron.down"),
        .init(id: "rename", symbol: "pencil"),
        .init(id: "connection", symbol: "status+label"),
        .init(id: "pin", symbol: "pin.fill"),
        .init(id: "add-tab", symbol: "plus"),
        .init(id: "refresh", symbol: "arrow.clockwise"),
        .init(id: "lifecycle", symbol: "play.fill"),
    ]
    public static let presentation:
        ControlCenterVisualPresentation = .iconOnly

    public static func symbol(for id: String) -> String {
        units.first { $0.id == id }?.symbol ?? "circle"
    }
}

public enum ControlBottomVisualContract {
    public static let units: [ControlCenterVisualUnit] = [
        .init(id: "sessions", symbol: "terminal"),
        .init(id: "mode", symbol: "rectangle.split.2x1"),
        .init(id: "resources", symbol: "externaldrive"),
        .init(id: "iphone", symbol: "iphone"),
        .init(id: "more", symbol: "ellipsis.circle"),
    ]
    public static let presentation:
        ControlCenterVisualPresentation = .iconOverLabel

    public static func symbol(for id: String) -> String {
        units.first { $0.id == id }?.symbol ?? "circle"
    }
}
