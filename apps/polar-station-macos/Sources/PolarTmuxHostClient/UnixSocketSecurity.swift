#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif
import Foundation

public enum PolarTmuxSocketFileKind: Equatable, Sendable {
    case directory
    case unixSocket
    case symbolicLink
    case regularFile
    case other
}

public struct PolarTmuxSocketFileMetadata: Equatable, Sendable {
    public let ownerUserID: UInt32
    public let permissions: UInt32
    public let kind: PolarTmuxSocketFileKind

    public init(
        ownerUserID: UInt32,
        permissions: UInt32,
        kind: PolarTmuxSocketFileKind
    ) {
        self.ownerUserID = ownerUserID
        self.permissions = permissions
        self.kind = kind
    }
}

public protocol PolarTmuxSocketMetadataReading: Sendable {
    func metadata(at url: URL) throws
        -> PolarTmuxSocketFileMetadata
}

public struct PolarTmuxDarwinSocketMetadataReader:
    PolarTmuxSocketMetadataReading
{
    public init() {}

    public func metadata(at url: URL) throws
        -> PolarTmuxSocketFileMetadata
    {
        var value = stat()
        let result = url.path.withCString {
            lstat($0, &value)
        }
        guard result == 0 else {
            let code = POSIXErrorCode(rawValue: errno) ?? .EIO
            throw POSIXError(code)
        }

        let mode = UInt32(value.st_mode)
        let kind: PolarTmuxSocketFileKind
        switch mode & 0o170000 {
        case 0o040000:
            kind = .directory
        case 0o140000:
            kind = .unixSocket
        case 0o120000:
            kind = .symbolicLink
        case 0o100000:
            kind = .regularFile
        default:
            kind = .other
        }
        return .init(
            ownerUserID: UInt32(value.st_uid),
            permissions: mode & 0o7777,
            kind: kind
        )
    }
}

public enum PolarTmuxUnixSocketSecurityError:
    Error,
    Equatable,
    LocalizedError,
    Sendable
{
    case parentIsSymbolicLink
    case parentIsNotDirectory
    case parentOwnerMismatch(expected: UInt32, actual: UInt32)
    case parentModeMismatch(expected: UInt32, actual: UInt32)
    case socketIsSymbolicLink
    case socketIsNotUnixSocket
    case socketOwnerMismatch(expected: UInt32, actual: UInt32)
    case socketModeMismatch(expected: UInt32, actual: UInt32)

    public var errorDescription: String? {
        switch self {
        case .parentIsSymbolicLink:
            "The Polar Tmux socket directory must not be a symbolic link"
        case .parentIsNotDirectory:
            "The Polar Tmux socket parent is not a directory"
        case .parentOwnerMismatch:
            "The Polar Tmux socket directory belongs to another user"
        case .parentModeMismatch:
            "The Polar Tmux socket directory permissions are not 0700"
        case .socketIsSymbolicLink:
            "The Polar Tmux Host socket must not be a symbolic link"
        case .socketIsNotUnixSocket:
            "The Polar Tmux Host endpoint is not a Unix socket"
        case .socketOwnerMismatch:
            "The Polar Tmux Host socket belongs to another user"
        case .socketModeMismatch:
            "The Polar Tmux Host socket permissions are not 0600"
        }
    }
}

public struct PolarTmuxUnixSocketMetadataValidator: Sendable {
    private let metadataReader: any PolarTmuxSocketMetadataReading
    private let effectiveUserID: UInt32

    public init(
        metadataReader:
            any PolarTmuxSocketMetadataReading =
                PolarTmuxDarwinSocketMetadataReader(),
        effectiveUserID: UInt32 = UInt32(geteuid())
    ) {
        self.metadataReader = metadataReader
        self.effectiveUserID = effectiveUserID
    }

    public func validate(socketURL: URL) throws {
        let parentMetadata = try metadataReader.metadata(
            at: socketURL.deletingLastPathComponent()
        )
        switch parentMetadata.kind {
        case .symbolicLink:
            throw PolarTmuxUnixSocketSecurityError
                .parentIsSymbolicLink
        case .directory:
            break
        default:
            throw PolarTmuxUnixSocketSecurityError
                .parentIsNotDirectory
        }
        guard parentMetadata.ownerUserID == effectiveUserID else {
            throw PolarTmuxUnixSocketSecurityError
                .parentOwnerMismatch(
                    expected: effectiveUserID,
                    actual: parentMetadata.ownerUserID
                )
        }
        guard parentMetadata.permissions == 0o700 else {
            throw PolarTmuxUnixSocketSecurityError
                .parentModeMismatch(
                    expected: 0o700,
                    actual: parentMetadata.permissions
                )
        }

        let socketMetadata = try metadataReader.metadata(at: socketURL)
        switch socketMetadata.kind {
        case .symbolicLink:
            throw PolarTmuxUnixSocketSecurityError
                .socketIsSymbolicLink
        case .unixSocket:
            break
        default:
            throw PolarTmuxUnixSocketSecurityError
                .socketIsNotUnixSocket
        }
        guard socketMetadata.ownerUserID == effectiveUserID else {
            throw PolarTmuxUnixSocketSecurityError
                .socketOwnerMismatch(
                    expected: effectiveUserID,
                    actual: socketMetadata.ownerUserID
                )
        }
        guard socketMetadata.permissions == 0o600 else {
            throw PolarTmuxUnixSocketSecurityError
                .socketModeMismatch(
                    expected: 0o600,
                    actual: socketMetadata.permissions
                )
        }
    }
}
