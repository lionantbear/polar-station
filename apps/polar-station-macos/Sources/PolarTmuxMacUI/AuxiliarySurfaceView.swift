import PolarTmuxHostClient
import SwiftUI

/// A privacy-safe placeholder for independently owned auxiliary projections.
///
/// Resources and iPhone can replace this body with their own projected native
/// controls without becoming pages in the main Control Center.
public struct AuxiliarySurfaceView: View {
    public let descriptor: AuxiliarySurfaceProjectionV1
    public let dismiss: @MainActor () -> Void

    public init(
        descriptor: AuxiliarySurfaceProjectionV1,
        dismiss: @escaping @MainActor () -> Void = {}
    ) {
        self.descriptor = descriptor
        self.dismiss = dismiss
    }

    public var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                Image(systemName: symbol)
                    .font(.title2)
                    .foregroundStyle(.tint)

                VStack(alignment: .leading, spacing: 2) {
                    Text(descriptor.title)
                        .font(.title2.bold())
                    Text(descriptor.state)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                Spacer()

                Button("Done", action: dismiss)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityLabel("Close \(descriptor.title)")
            }
            .padding(18)
            .background(.bar)

            Divider()

            VStack(spacing: 14) {
                Image(systemName: symbol)
                    .font(.system(size: 42, weight: .light))
                    .foregroundStyle(.secondary)

                Text(descriptor.message)
                    .font(.body)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 360)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(28)
        }
        .frame(minWidth: minimumWidth, minHeight: 300)
        .background(.regularMaterial)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(descriptor.title)
    }

    private var symbol: String {
        switch descriptor.id {
        case .resources:
            "externaldrive.connected.to.line.below"
        case .iphone:
            "iphone.gen3"
        }
    }

    private var minimumWidth: CGFloat {
        switch descriptor.id {
        case .resources:
            620
        case .iphone:
            420
        }
    }
}
