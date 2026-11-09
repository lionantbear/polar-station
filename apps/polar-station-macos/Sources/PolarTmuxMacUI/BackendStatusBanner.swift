import PolarTmuxHostClient
import SwiftUI

public struct BackendStatusBanner: View {
    public let availability: PolarStationBackendAvailability
    public let retry: @MainActor () -> Void

    public init(
        availability: PolarStationBackendAvailability,
        retry: @escaping @MainActor () -> Void
    ) {
        self.availability = availability
        self.retry = retry
    }

    public var body: some View {
        if case .ready = availability {
            EmptyView()
        } else {
            HStack(spacing: 10) {
                Image(systemName: iconName)
                    .foregroundStyle(iconColor)
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(.callout.weight(.semibold))
                    if let message = availability.failureMessage {
                        Text(message)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 12)
                Button("Retry", action: retry)
                    .disabled(isStarting)
                    .accessibilityLabel("Retry Polar Station backend")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(Color.orange.opacity(0.14))
            .overlay(alignment: .bottom) {
                Divider()
            }
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("polar-station-backend-status")
        }
    }

    private var title: String {
        switch availability {
        case .starting:
            "Connecting to the Polar Station backend…"
        case .ready:
            "Backend connected"
        case .reconnecting(_, let attempt):
            "Reconnecting to the backend (attempt \(attempt))…"
        case .unavailable:
            "Backend unavailable — the interface remains usable"
        }
    }

    private var iconName: String {
        switch availability {
        case .starting, .reconnecting:
            "arrow.triangle.2.circlepath"
        case .unavailable:
            "exclamationmark.triangle.fill"
        case .ready:
            "checkmark.circle.fill"
        }
    }

    private var iconColor: Color {
        if case .unavailable = availability { return .orange }
        return .secondary
    }

    private var isStarting: Bool {
        if case .starting = availability { return true }
        return false
    }
}
