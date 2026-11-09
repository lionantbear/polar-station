import CoreGraphics

/// Native-only width policy for the in-window Control Center.
///
/// Rust carries Workspace facts and visibility intent. This type owns only
/// physical Map geometry, so the same projection can be rendered on a host
/// with a different logical display width without changing the wire model.
public enum ControlCenterWidthPolicy {
    public static let minimumMapWidth: CGFloat = 200
    public static let maximumMapWidth: CGFloat = 390
    public static let sessionsWidth: CGFloat = 280
    public static let dividerWidth: CGFloat = 1
    public static let minimumInboxWidth: CGFloat = 300
    public static let maximumInboxWidth: CGFloat = 520
    public static let defaultInboxWidth: CGFloat = 360

    public static func clampMapWidth(_ width: CGFloat) -> CGFloat {
        min(max(width, minimumMapWidth), maximumMapWidth)
    }

    public static func adaptiveDefault(visibleLogicalWidth: CGFloat) -> CGFloat {
        clampMapWidth(floor(visibleLogicalWidth * 0.25))
    }

    public static func clampInboxWidth(_ width: CGFloat) -> CGFloat {
        min(max(width, minimumInboxWidth), maximumInboxWidth)
    }

    /// Dragging the terminal boundary in either direction changes the native
    /// Map width, with the product bounds applied before presentation.
    public static func resizedWidth(
        start: CGFloat,
        translation: CGFloat
    ) -> CGFloat {
        let startWidth = clampMapWidth(start)
        return clampMapWidth(startWidth + translation)
    }

    public static func controllerWidth(
        mapWidth: CGFloat,
        sessionsVisible: Bool,
        layoutMapVisible: Bool,
        inboxVisible: Bool = false,
        inboxWidth: CGFloat = defaultInboxWidth
    ) -> CGFloat {
        let selectedMapWidth = clampMapWidth(mapWidth)
        let selectedInboxWidth = clampInboxWidth(inboxWidth)
        var widths: [CGFloat] = []
        if sessionsVisible { widths.append(sessionsWidth) }
        if layoutMapVisible { widths.append(selectedMapWidth) }
        if inboxVisible { widths.append(selectedInboxWidth) }
        return max(minimumMapWidth, widths.reduce(0, +) + CGFloat(max(0, widths.count - 1)) * dividerWidth)
    }
}
