import AppKit
import CoreImage
import CoreImage.CIFilterBuiltins
import PolarTmuxHostClient
import SwiftUI

/// Native renderer for the Host-owned Companion pairing projection.
///
/// The view never creates credentials. It renders the local-only payload
/// returned by the Rust Host and keeps the QR code visible only while this
/// auxiliary window is presented.
public struct IPhoneCompanionView: View {
    public let pairing: CompanionPairingPayloadV1?
    public let isLoading: Bool
    public let errorMessage: String?
    public let backendActionsEnabled: Bool
    public let refresh: @MainActor () -> Void
    public let dismiss: @MainActor () -> Void

    @State private var didCopyPairingCode = false

    public init(
        pairing: CompanionPairingPayloadV1?,
        isLoading: Bool,
        errorMessage: String?,
        backendActionsEnabled: Bool = true,
        refresh: @escaping @MainActor () -> Void,
        dismiss: @escaping @MainActor () -> Void
    ) {
        self.pairing = pairing
        self.isLoading = isLoading
        self.errorMessage = errorMessage
        self.backendActionsEnabled = backendActionsEnabled
        self.refresh = refresh
        self.dismiss = dismiss
    }

    public var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "iphone.slash")
                .font(.largeTitle)
            Text("iPhone Companion — Unavailable")
                .font(.title2.bold())
            Text("Companion distribution is not enabled for this public build.")
                .multilineTextAlignment(.center)
            Button("Done", action: dismiss)
                .keyboardShortcut(.cancelAction)
        }
        .padding(24)
        .frame(minWidth: 390, minHeight: 240)
        .accessibilityIdentifier("iphone-companion-unavailable")
    }

    private var header: some View {
        HStack(spacing: 12) {
            Image(systemName: "iphone.radiowaves.left.and.right")
                .font(.title2)
                .foregroundStyle(.tint)
            VStack(alignment: .leading, spacing: 2) {
                Text("iPhone Companion")
                    .font(.title2.bold())
                Text("Secure control through this Polar Station Host")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Button("Refresh", action: refresh)
                .disabled(isLoading || !backendActionsEnabled)
            Button("Done", action: dismiss)
                .keyboardShortcut(.cancelAction)
                .accessibilityLabel("Close iPhone")
        }
        .padding(18)
        .background(.bar)
    }

    @ViewBuilder
    private var content: some View {
        if let pairing {
            pairingContent(pairing)
        } else if isLoading {
            ProgressView("Loading pairing state…")
                .frame(maxWidth: .infinity, minHeight: 360)
        } else {
            VStack(spacing: 14) {
                Image(systemName: "iphone.slash")
                    .font(.system(size: 42, weight: .light))
                    .foregroundStyle(.secondary)
                Text("Pairing Unavailable")
                    .font(.headline)
                Text(
                    errorMessage
                        ?? "The local Polar Station Host did not provide pairing state."
                )
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                Button("Try Again", action: refresh)
                    .disabled(!backendActionsEnabled)
            }
            .frame(maxWidth: .infinity, minHeight: 360)
        }
    }

    private func pairingContent(
        _ pairing: CompanionPairingPayloadV1
    ) -> some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(alignment: .top, spacing: 12) {
                Circle()
                    .fill(pairing.listenerReady ? .green : .orange)
                    .frame(width: 13, height: 13)
                    .padding(.top, 5)
                VStack(alignment: .leading, spacing: 4) {
                    Text("Mac Companion Server")
                        .font(.title3.bold())
                    Text(
                        pairing.listenerReady
                            ? "Ready for secure iPhone connections"
                            : "Listener is not ready"
                    )
                    .foregroundStyle(.secondary)
                }
                Spacer()
            }

            Label(
                connectionSummary(pairing.connectedDevices),
                systemImage:
                    pairing.connectedDevices > 0
                        ? "iphone.radiowaves.left.and.right"
                        : "iphone.slash"
            )
            .font(.headline)
            .accessibilityIdentifier("iphone-companion-status")

            Divider()

            if let image = qrCode(pairing.pairingCode) {
                Image(nsImage: image)
                    .interpolation(.none)
                    .frame(
                        width: image.size.width,
                        height: image.size.height
                    )
                    .padding(20)
                    .background(Color.white)
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                    .frame(maxWidth: .infinity)
                    .accessibilityLabel(
                        "Polar Station iPhone pairing QR code"
                    )
                    .accessibilityIdentifier(
                        "companion-pairing-qr-code"
                    )
            } else {
                Label(
                    "Pairing QR code could not be rendered",
                    systemImage: "exclamationmark.triangle"
                )
                .foregroundStyle(.red)
            }

            Button(action: { copy(pairing.pairingCode) }) {
                Label(
                    didCopyPairingCode
                        ? "Pairing Code Copied"
                        : "Copy Pairing Code",
                    systemImage:
                        didCopyPairingCode
                            ? "checkmark.circle.fill"
                            : "key"
                )
                .font(.headline)
                .frame(maxWidth: .infinity, minHeight: 42)
            }
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("copy-pairing-code")

            Text(
                "Scan with Polar Station on iPhone. Anyone who can scan this code can control this Host, so close this window when pairing is complete."
            )
            .font(.callout)
            .foregroundStyle(.secondary)
            .multilineTextAlignment(.center)

            Text("Host \(pairing.hostID)")
                .font(.caption.monospaced())
                .foregroundStyle(.tertiary)
                .textSelection(.enabled)
        }
        .padding(18)
        .background(
            Color.secondary.opacity(0.08),
            in: RoundedRectangle(cornerRadius: 18)
        )
    }

    private func connectionSummary(_ count: UInt32) -> String {
        switch count {
        case 0:
            "No iPhone connected"
        case 1:
            "1 iPhone connected"
        default:
            "\(count) iPhones connected"
        }
    }

    private func copy(_ pairingCode: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(
            pairingCode,
            forType: .string
        )
        didCopyPairingCode = true
    }

    private func qrCode(_ pairingCode: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(pairingCode.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else {
            return nil
        }
        let paddedExtent = output.extent.insetBy(dx: -4, dy: -4)
        let background = CIImage(color: .white)
            .cropped(to: paddedExtent)
        let scaled = output
            .composited(over: background)
            .transformed(
                by: CGAffineTransform(scaleX: 5, y: 5)
            )
        let context = CIContext()
        guard let cgImage = context.createCGImage(
            scaled,
            from: scaled.extent
        ) else {
            return nil
        }
        return NSImage(
            cgImage: cgImage,
            size: scaled.extent.size
        )
    }
}
