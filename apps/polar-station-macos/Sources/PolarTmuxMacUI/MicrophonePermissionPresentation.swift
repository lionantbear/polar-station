import AppKit
import AVFoundation

/// Native macOS privacy mechanics. Recording policy remains in shared Dictate.
public struct MicrophonePermissionPresentation {
    public enum Action: Equatable { case requestAccess, openSettings, none }
    public let label: String
    public let explanation: String
    public let action: Action
    public let actionLabel: String

    public init(status: AVAuthorizationStatus) {
        switch status {
        case .notDetermined:
            label = "Not requested"
            explanation = "macOS will ask for microphone access when you record. You can also request access here."
            action = .requestAccess
            actionLabel = "Request microphone access"
        case .authorized:
            label = "Allowed"
            explanation = "Choose an input device below and run a local microphone test."
            action = .none
            actionLabel = ""
        case .denied:
            label = "Access is off"
            explanation = "Allow Polar Station in System Settings → Privacy & Security → Microphone, then return and start recording again."
            action = .openSettings
            actionLabel = "Open microphone settings"
        case .restricted:
            label = "Restricted by macOS"
            explanation = "macOS restrictions prevent microphone access. This permission cannot be changed by the app; check the Mac’s restrictions or contact its administrator."
            action = .none
            actionLabel = ""
        @unknown default:
            label = "Status unavailable"
            explanation = "Check microphone access in macOS privacy settings, then try again."
            action = .openSettings
            actionLabel = "Open microphone settings"
        }
    }

    @MainActor public static func openSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone") {
            NSWorkspace.shared.open(url)
        }
    }

    @MainActor public static func showCaptureFailureIfNeeded() {
        let status = AVCaptureDevice.authorizationStatus(for: .audio)
        guard status == .denied || status == .restricted else { return }
        let presentation = Self(status: status)
        let alert = NSAlert()
        alert.messageText = "Microphone · " + presentation.label
        alert.informativeText = presentation.explanation
        if presentation.action == .openSettings {
            alert.addButton(withTitle: presentation.actionLabel)
            alert.addButton(withTitle: "Not now")
        } else {
            alert.addButton(withTitle: "OK")
        }
        if alert.runModal() == .alertFirstButtonReturn, presentation.action == .openSettings {
            openSettings()
        }
    }
}
