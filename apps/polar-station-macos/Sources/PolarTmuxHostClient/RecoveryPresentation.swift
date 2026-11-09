import CPolarStationDesktopClient

/// Copies the shared Rust recovery policy; Swift owns only draft bytes and UI.
public struct PolarStationRecoveryPresentation: Equatable, Sendable {
    public let canSubmit: Bool
    public let localEnabled: Bool
    public let status: String
    public let saveLabel: String
    public let hostLabel: String
    public let localLabel: String
    public let destinationLabel: String

    public init(connected: Bool, targetKnown: Bool, pending: Bool) {
        let view = polar_station_recovery_view(connected, targetKnown, pending)
        canSubmit = view.can_submit
        localEnabled = view.local_enabled
        status = String(cString: view.status)
        saveLabel = String(cString: view.save_label)
        hostLabel = String(cString: view.host_label)
        localLabel = String(cString: view.local_label)
        destinationLabel = String(cString: view.destination_label)
    }
}
