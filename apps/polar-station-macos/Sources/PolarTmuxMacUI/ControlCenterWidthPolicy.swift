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
    public static func clampMapWidth(_ width: CGFloat) -> CGFloat {
        min(max(width, minimumMapWidth), maximumMapWidth)
    }

    public static func adaptiveDefault(visibleLogicalWidth: CGFloat) -> CGFloat {
        clampMapWidth(floor(visibleLogicalWidth * 0.25))
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
        tab: ControlCenterTab,
        mapWidth: CGFloat,
        sessionsVisible: Bool
    ) -> CGFloat {
        switch tab {
        case .inbox, .settings: return clampMapWidth(mapWidth)
        case .layout:
            return clampMapWidth(mapWidth) + (sessionsVisible ? sessionsWidth + dividerWidth : 0)
        }
    }
}
