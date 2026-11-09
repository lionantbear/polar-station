import SwiftUI

private struct PolarStationInterfaceTextScaleKey: EnvironmentKey {
    static let defaultValue: CGFloat = 1
}

public extension EnvironmentValues {
    var polarStationInterfaceTextScale: CGFloat {
        get { self[PolarStationInterfaceTextScaleKey.self] }
        set { self[PolarStationInterfaceTextScaleKey.self] = newValue }
    }
}

public extension View {
    func polarStationInterfaceTextScale(percent: UInt16) -> some View {
        let bounded = min(max(percent, 80), 200)
        return environment(
            \.polarStationInterfaceTextScale,
            CGFloat(bounded) / 100
        )
        .environment(\.sizeCategory, Self.sizeCategory(for: bounded))
        .stationTheme()
    }

    private static func sizeCategory(for percent: UInt16) -> ContentSizeCategory {
        switch percent {
        case ..<90: .small
        case ..<110: .medium
        case ..<120: .large
        case ..<130: .extraLarge
        case ..<140: .extraExtraLarge
        case ..<150: .extraExtraExtraLarge
        case ..<170: .accessibilityMedium
        case ..<190: .accessibilityLarge
        default: .accessibilityExtraLarge
        }
    }
}
