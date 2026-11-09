#if POLAR_STATION
import Foundation
import PolarTmuxMacUI

/// Read-only compatibility access to the pre-Settings width preference.
///
/// New writes go through SettingsV1. This reader exists only so a missing
/// Settings document can be seeded once without losing the user's old width.
final class ControlCenterWidthPreferences {
    private static let key = "PolarStation.controlCenter.layoutMapWidth.v1"
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func loadOrDefault(visibleLogicalWidth: CGFloat) -> CGFloat {
        guard let stored = defaults.object(forKey: Self.key) as? NSNumber else {
            return ControlCenterWidthPolicy.adaptiveDefault(
                visibleLogicalWidth: visibleLogicalWidth
            )
        }
        let value = stored.doubleValue
        guard value.isFinite else {
            return ControlCenterWidthPolicy.adaptiveDefault(
                visibleLogicalWidth: visibleLogicalWidth
            )
        }
        return ControlCenterWidthPolicy.clampMapWidth(CGFloat(value))
    }

    func legacyStoredWidth() -> UInt16? {
        guard let stored = defaults.object(forKey: Self.key) as? NSNumber else {
            return nil
        }
        let value = stored.doubleValue
        guard value.isFinite else { return nil }
        return UInt16(ControlCenterWidthPolicy.clampMapWidth(CGFloat(value)))
    }
}
#endif
