import Foundation

public protocol ControlCenterProjectionSource: Sendable {
    func loadProjection() throws -> ControlCenterProjectionV1
}

/// Optional deterministic behavior for a projection source used in local
/// fixture review.
///
/// A real Host IPC source does not conform to this protocol. It returns the
/// authoritative projection from the Host after accepting an intent instead.
public protocol ControlCenterFixtureInteractionSource:
    ControlCenterProjectionSource
{
    func nextProjection(
        after intent: ControlCenterIntentV1,
        current: ControlCenterProjectionV1
    ) throws -> ControlCenterProjectionV1?
}

public enum ControlCenterFixtureSourceError: Error, Equatable, Sendable {
    case resourceNotFound(name: String)
    case controlRevisionExhausted
}

public struct ControlCenterFixtureSource:
    ControlCenterFixtureInteractionSource
{
    private let bundle: Bundle
    private let resourceName: String

    public init(resourceName: String = "controller-sessions-closed") {
        bundle = Self.defaultBundle
        self.resourceName = resourceName
    }

    public init(
        bundle: Bundle,
        resourceName: String = "controller-sessions-closed"
    ) {
        self.bundle = bundle
        self.resourceName = resourceName
    }

    public static func load() throws -> ControlCenterProjectionV1 {
        try Self().loadProjection()
    }

    public func loadProjection() throws -> ControlCenterProjectionV1 {
        let data = try loadData()
        return try JSONDecoder().decode(
            ControlCenterProjectionV1.self,
            from: data
        )
    }

    public func loadData() throws -> Data {
        let subdirectories: [String?] = [
            "Resources/ControlCenterFixtures",
            "ControlCenterFixtures",
            nil,
        ]
        guard let url = subdirectories.lazy.compactMap({
            bundle.url(
                forResource: resourceName,
                withExtension: "json",
                subdirectory: $0
            )
        }).first else {
            throw ControlCenterFixtureSourceError.resourceNotFound(
                name: resourceName
            )
        }
        return try Data(contentsOf: url)
    }

    /// Simulates only the fixture's projected Sessions visibility action.
    ///
    /// This is development-review transport behavior, not Workspace business
    /// logic. Every other action remains an emitted typed intent awaiting the
    /// Rust Host.
    public func nextProjection(
        after intent: ControlCenterIntentV1,
        current: ControlCenterProjectionV1
    ) throws -> ControlCenterProjectionV1? {
        guard intent.controlRevision == current.controlRevision,
              case .invokeProjectedAction(let actionID) = intent.operation,
              actionID == "sessions.toggle",
              current.bottomBar.action(id: actionID)?.enabled == true
        else {
            return nil
        }

        let (nextRevision, overflow) =
            current.controlRevision.addingReportingOverflow(1)
        guard !overflow else {
            throw ControlCenterFixtureSourceError
                .controlRevisionExhausted
        }
        let sessionsVisible = !current.sessions.visible
        let sessions = SessionLibraryProjectionV1(
            visible: sessionsVisible,
            title: current.sessions.title,
            interactionHint: current.sessions.interactionHint,
            refreshAction: current.sessions.refreshAction,
            closeAction: ActionProjectionV1(
                id: current.sessions.closeAction.id,
                label: current.sessions.closeAction.label,
                role: current.sessions.closeAction.role,
                enabled: current.sessions.closeAction.enabled,
                selected: sessionsVisible,
                disabledReason: current.sessions.closeAction.disabledReason,
                confirmation: current.sessions.closeAction.confirmation,
                accessibilityLabel:
                    current.sessions.closeAction.accessibilityLabel
            ),
            sections: current.sessions.sections,
            remoteTargets: current.sessions.remoteTargets,
            rows: current.sessions.rows,
            selected: current.sessions.selected,
            localExecutionTargetID:
                current.sessions.localExecutionTargetID,
            remoteExecutionTargetID:
                current.sessions.remoteExecutionTargetID,
            localNewShell: current.sessions.localNewShell,
            localNewCommand: current.sessions.localNewCommand,
            remoteNewSession: current.sessions.remoteNewSession
        )
        let bottomBar = ControlBottomBarProjectionV1(
            actions: current.bottomBar.actions.map { action in
                guard action.id == actionID else {
                    return action
                }
                return ActionProjectionV1(
                    id: action.id,
                    label: action.label,
                    role: action.role,
                    enabled: action.enabled,
                    selected: sessionsVisible,
                    disabledReason: action.disabledReason,
                    confirmation: action.confirmation,
                    accessibilityLabel:
                        action.accessibilityLabel
                )
            },
            moreActions: current.bottomBar.moreActions
        )
        return ControlCenterProjectionV1(
            schemaVersion: current.schemaVersion,
            controlRevision: nextRevision,
            header: current.header,
            sessions: sessions,
            bottomBar: bottomBar,
            projectedActions: current.projectedActions,
            auxiliarySurfaces: current.auxiliarySurfaces,
            banner: current.banner
        )
    }

    private static var defaultBundle: Bundle {
#if SWIFT_PACKAGE
        .module
#else
        Bundle(for: ControlCenterResourceBundleToken.self)
#endif
    }
}

private final class ControlCenterResourceBundleToken {}
