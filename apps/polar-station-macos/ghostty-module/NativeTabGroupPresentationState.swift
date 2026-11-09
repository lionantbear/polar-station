import Foundation

struct NativeWindowFrame: Equatable, Sendable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double

    func isApproximatelyEqual(
        to other: NativeWindowFrame,
        tolerance: Double = 0.5
    ) -> Bool {
        abs(x - other.x) <= tolerance
            && abs(y - other.y) <= tolerance
            && abs(width - other.width) <= tolerance
            && abs(height - other.height) <= tolerance
    }

    fileprivate var stabilityValue: NativeWindowFrame {
        NativeWindowFrame(
            x: Self.roundForStability(x),
            y: Self.roundForStability(y),
            width: Self.roundForStability(width),
            height: Self.roundForStability(height)
        )
    }

    private static func roundForStability(_ value: Double) -> Double {
        (value * 2).rounded() / 2
    }
}

struct NativeTabGroupPresentationObservation: Equatable, Sendable {
    let expectedTabIDs: [String]
    let windowTokensByTabID: [String: String]
    let groupTokensByWindowToken: [String: String]
    let groupWindowTokens: [String]
    var selectedWindowToken: String?
    let referenceWindowToken: String
    var frames: [String: NativeWindowFrame]
    let fullscreenWindowTokens: Set<String>
    let transitioningWindowTokens: Set<String>
    var applicationIsActive: Bool
    var visibleWindowTokens: Set<String>
    var keyWindowToken: String?
    var settledPresentationWindowToken: String?
}

enum NativeTabSelectionPresentation: Equatable, Sendable {
    case passive
    case activeUser
}

/// Orders native AppKit Tab choices while their existing Workspace CAS
/// transactions are in flight.
///
/// The authoritative projected selection remains distinct from the newest
/// native choice. Presentation follows the native override immediately so a
/// passive reconcile cannot bounce the window backward; only the exact latest
/// completion clears that override and exposes the accepted or rolled-back
/// projection.
struct NativeTabSelectionIngressState: Equatable, Sendable {
    struct Intent: Equatable, Sendable {
        let tabID: String
        let generation: UInt64
    }

    private(set) var projectedSelectedTabID: String?
    private(set) var pendingNativeSelection: Intent?
    private var nextGeneration: UInt64 = 0

    var effectiveSelectedTabID: String? {
        pendingNativeSelection?.tabID ?? projectedSelectedTabID
    }

    @discardableResult
    mutating func requestProjectedSelection(tabID: String) -> String? {
        guard !tabID.isEmpty else { return effectiveSelectedTabID }
        projectedSelectedTabID = tabID
        return effectiveSelectedTabID
    }

    mutating func observeNativeSelection(tabID: String) -> Intent? {
        guard !tabID.isEmpty, tabID != effectiveSelectedTabID else {
            return nil
        }
        nextGeneration &+= 1
        if nextGeneration == 0 {
            nextGeneration = 1
        }
        let intent = Intent(tabID: tabID, generation: nextGeneration)
        pendingNativeSelection = intent
        return intent
    }

    /// Removes a native override whose backing Tab no longer belongs to the
    /// accepted Workspace. A retired window must never remain the effective
    /// selection while its surviving replacement is already projected.
    @discardableResult
    mutating func reconcileAvailableTabIDs(
        _ availableTabIDs: Set<String>
    ) -> String? {
        if let pendingNativeSelection,
           !availableTabIDs.contains(pendingNativeSelection.tabID)
        {
            self.pendingNativeSelection = nil
        }
        if let projectedSelectedTabID,
           !availableTabIDs.contains(projectedSelectedTabID)
        {
            self.projectedSelectedTabID = nil
        }
        return effectiveSelectedTabID
    }

    @discardableResult
    mutating func completeNativeSelection(_ intent: Intent) -> String? {
        guard pendingNativeSelection == intent else {
            return effectiveSelectedTabID
        }
        pendingNativeSelection = nil
        return effectiveSelectedTabID
    }
}

/// Describes the synchronous AppKit presentation work required before a
/// selected, key, or visible Tab controller may leave its native Tab group.
/// The coordinator executes this plan before removeWindow or orderOut so a
/// persisted deletion cannot temporarily leave Polar Station without a
/// presented survivor.
struct NativeTabRetirementHandoff: Equatable, Sendable {
    let targetTabID: String
    let frameSourceTabID: String
    let orderFront: Bool
    let makeKey: Bool

    static func plan(
        requestedTabID: String,
        selectedTabID: String?,
        keyTabID: String?,
        visibleTabIDs: Set<String>,
        retiringTabIDs: Set<String>,
        applicationIsActive: Bool
    ) -> Self? {
        guard !requestedTabID.isEmpty,
              !retiringTabIDs.contains(requestedTabID)
        else { return nil }

        let selectedIsRetiring = selectedTabID.map(
            retiringTabIDs.contains
        ) == true
        let keyIsRetiring = keyTabID.map(retiringTabIDs.contains) == true
        let visibleRetiringTabID = visibleTabIDs
            .intersection(retiringTabIDs)
            .sorted()
            .first
        guard selectedIsRetiring || keyIsRetiring
                || visibleRetiringTabID != nil
        else { return nil }

        let frameSourceTabID = keyIsRetiring
            ? keyTabID
            : selectedIsRetiring ? selectedTabID : visibleRetiringTabID
        guard let frameSourceTabID else { return nil }
        return Self(
            targetTabID: requestedTabID,
            frameSourceTabID: frameSourceTabID,
            orderFront: true,
            makeKey: applicationIsActive
                && (keyIsRetiring || selectedIsRetiring)
        )
    }
}

/// One existing native focus request, confirmed after a later presentation
/// turn so that AppKit/SwiftUI tree replacement cannot erase its delivery.
struct NativeTerminalFocusRequest: Equatable, Sendable {
    let tabID: String
    let paneID: String
    private(set) var observedFocused = false

    mutating func observe(
        selectedTabID: String?,
        focusedPaneID: String?,
        firstResponderPaneID: String?
    ) -> Bool {
        let matches = selectedTabID == tabID
            && focusedPaneID == paneID && firstResponderPaneID == paneID
        let complete = matches && observedFocused
        observedFocused = matches
        return complete
    }
}

struct NativeTabGroupPresentationState: Equatable, Sendable {
    enum Phase: Equatable, Sendable {
        case awaitingInitialController
        case buildingNormalTree
        case stabilizingSelection
        case enteringFullscreen(
            targetWindowToken: String,
            generation: UInt64
        )
        case readyFullscreen(windowToken: String)
        case readyWindowedAfterUserExit
        case windowedFallback
    }

    enum Command: Equatable, Sendable {
        case normalizeFrames(referenceWindowToken: String)
        case selectWindow(
            tabID: String,
            windowToken: String,
            presentation: NativeTabSelectionPresentation
        )
        case presentWindow(tabID: String, windowToken: String)
        case enterFullscreen(windowToken: String, generation: UInt64)
        case applyDeferredSelection(tabID: String)
    }

    static let requiredStableObservations = 3

    private(set) var phase: Phase = .awaitingInitialController
    private(set) var requestedSelectedTabID: String?
    private(set) var requestedSelectionPresentation:
        NativeTabSelectionPresentation = .passive
    private(set) var deferredSelectedTabID: String?
    private var deferredSelectionPresentation:
        NativeTabSelectionPresentation = .passive
    private var stableObservationCount = 0
    private var lastStableSignature: StableSignature?
    private var nextFullscreenGeneration: UInt64 = 0
    private var managedWindowTokens: Set<String> = []
    private var managedWindowOrder: [String] = []
    private var startupPresentedWindowToken: String?

    mutating func requestSelection(
        tabID: String,
        presentation: NativeTabSelectionPresentation = .passive
    ) {
        guard !tabID.isEmpty else { return }
        if case .enteringFullscreen = phase {
            deferredSelectedTabID = tabID
            deferredSelectionPresentation = presentation
        } else {
            if requestedSelectedTabID == tabID,
               requestedSelectionPresentation == .activeUser,
               presentation == .passive
            {
                return
            }
            requestedSelectedTabID = tabID
            requestedSelectionPresentation = presentation
        }
    }

    mutating func reconcile(
        _ observation: NativeTabGroupPresentationObservation,
        startupFullscreenRequested: Bool
    ) -> [Command] {
        if case .enteringFullscreen = phase {
            return []
        }

        guard !observation.expectedTabIDs.isEmpty,
              let requestedSelectedTabID,
              let requestedWindowToken =
                observation.windowTokensByTabID[requestedSelectedTabID]
        else {
            resetStability(phase: .awaitingInitialController)
            return []
        }

        let expectedWindowTokens = observation.expectedTabIDs.compactMap {
            observation.windowTokensByTabID[$0]
        }
        guard expectedWindowTokens.count == observation.expectedTabIDs.count,
              observation.groupWindowTokens == expectedWindowTokens,
              expectedWindowTokens.allSatisfy({
                  observation.groupTokensByWindowToken[$0] != nil
              }),
              Set(expectedWindowTokens.compactMap {
                  observation.groupTokensByWindowToken[$0]
              }).count == 1,
              observation.transitioningWindowTokens.isEmpty
        else {
            resetStability(phase: .buildingNormalTree)
            return []
        }
        if managedWindowOrder != expectedWindowTokens {
            managedWindowOrder = expectedWindowTokens
            startupPresentedWindowToken = nil
        }
        managedWindowTokens = Set(expectedWindowTokens)

        if case .readyFullscreen = phase {
            guard observation.selectedWindowToken != requestedWindowToken else {
                return []
            }
            let presentation = requestedSelectionPresentation
            requestedSelectionPresentation = .passive
            return [
                .selectWindow(
                    tabID: requestedSelectedTabID,
                    windowToken: requestedWindowToken,
                    presentation: presentation
                ),
            ]
        }

        let expectedTokenSet = Set(expectedWindowTokens)
        if !observation.fullscreenWindowTokens.isEmpty {
            if observation.fullscreenWindowTokens == expectedTokenSet {
                phase = .readyFullscreen(windowToken: requestedWindowToken)
            } else {
                resetStability(phase: .windowedFallback)
            }
            // A retained fullscreen Window can gain new backing Tabs whose
            // fullscreen flags settle later. Its saved selection still applies.
            if observation.selectedWindowToken != requestedWindowToken {
                let presentation = requestedSelectionPresentation
                requestedSelectionPresentation = .passive
                return [.selectWindow(tabID: requestedSelectedTabID,
                    windowToken: requestedWindowToken, presentation: presentation)]
            }
            return []
        }

        let referenceWindowToken = expectedTokenSet.contains(
            observation.referenceWindowToken
        ) ? observation.referenceWindowToken : expectedWindowTokens[0]
        guard let referenceFrame = observation.frames[referenceWindowToken],
              expectedWindowTokens.allSatisfy({ token in
                  observation.frames[token]?.isApproximatelyEqual(
                      to: referenceFrame
                  ) == true
              })
        else {
            resetStability(phase: .buildingNormalTree)
            return [
                .normalizeFrames(
                    referenceWindowToken: referenceWindowToken
                ),
            ]
        }

        guard observation.selectedWindowToken == requestedWindowToken else {
            resetStability(phase: .stabilizingSelection)
            let presentation = requestedSelectionPresentation
            requestedSelectionPresentation = .passive
            return [
                .selectWindow(
                    tabID: requestedSelectedTabID,
                    windowToken: requestedWindowToken,
                    presentation: presentation
                ),
            ]
        }

        if startupFullscreenRequested,
           phaseCanEnterFullscreen,
           startupPresentedWindowToken != requestedWindowToken {
            resetStability(phase: .stabilizingSelection)
            startupPresentedWindowToken = requestedWindowToken
            return [
                .presentWindow(
                    tabID: requestedSelectedTabID,
                    windowToken: requestedWindowToken
                ),
            ]
        }

        if startupFullscreenRequested,
           phaseCanEnterFullscreen,
           !observation.applicationIsActive
               || !observation.visibleWindowTokens.contains(
                   requestedWindowToken
               )
               || observation.keyWindowToken != requestedWindowToken
               || observation.settledPresentationWindowToken
                   != requestedWindowToken {
            resetStability(phase: .stabilizingSelection)
            return []
        }

        let signature = StableSignature(
            windowTokens: expectedWindowTokens,
            selectedWindowToken: requestedWindowToken,
            frames: expectedWindowTokens.compactMap {
                observation.frames[$0]?.stabilityValue
            }
        )
        if signature == lastStableSignature {
            stableObservationCount += 1
        } else {
            lastStableSignature = signature
            stableObservationCount = 1
        }
        if !keepsWindowedDecision {
            phase = .stabilizingSelection
        }

        guard startupFullscreenRequested,
              phaseCanEnterFullscreen,
              stableObservationCount >= Self.requiredStableObservations
        else { return [] }

        nextFullscreenGeneration &+= 1
        if nextFullscreenGeneration == 0 {
            nextFullscreenGeneration = 1
        }
        let generation = nextFullscreenGeneration
        phase = .enteringFullscreen(
            targetWindowToken: requestedWindowToken,
            generation: generation
        )
        return [
            .enterFullscreen(
                windowToken: requestedWindowToken,
                generation: generation
            ),
        ]
    }

    mutating func didEnterFullscreen(
        windowToken: String
    ) -> [Command] {
        guard case .enteringFullscreen(let target, _) = phase,
              target == windowToken
        else { return [] }
        phase = .readyFullscreen(windowToken: target)
        guard let deferredSelectedTabID else { return [] }
        self.deferredSelectedTabID = nil
        requestedSelectedTabID = deferredSelectedTabID
        requestedSelectionPresentation = deferredSelectionPresentation
        deferredSelectionPresentation = .passive
        return [.applyDeferredSelection(tabID: deferredSelectedTabID)]
    }

    @discardableResult
    mutating func didFailToEnterFullscreen(
        windowToken: String
    ) -> [Command] {
        guard case .enteringFullscreen(let target, _) = phase,
              target == windowToken
        else { return [] }
        resetStability(phase: .windowedFallback)
        guard let deferredSelectedTabID else { return [] }
        self.deferredSelectedTabID = nil
        requestedSelectedTabID = deferredSelectedTabID
        requestedSelectionPresentation = deferredSelectionPresentation
        deferredSelectionPresentation = .passive
        return [.applyDeferredSelection(tabID: deferredSelectedTabID)]
    }

    mutating func didExitFullscreen(windowToken: String) {
        guard case .readyFullscreen = phase,
              managedWindowTokens.contains(windowToken)
        else { return }
        resetStability(phase: .readyWindowedAfterUserExit)
    }

    mutating func timedOut() {
        switch phase {
        case .readyFullscreen, .readyWindowedAfterUserExit, .windowedFallback:
            return
        default:
            resetStability(phase: .windowedFallback)
        }
    }

    private var phaseCanEnterFullscreen: Bool {
        switch phase {
        case .readyFullscreen, .readyWindowedAfterUserExit,
                .windowedFallback, .enteringFullscreen:
            false
        default:
            true
        }
    }

    private var keepsWindowedDecision: Bool {
        switch phase {
        case .readyWindowedAfterUserExit, .windowedFallback:
            true
        default:
            false
        }
    }

    private mutating func resetStability(phase: Phase) {
        if !keepsWindowedDecision {
            self.phase = phase
        }
        stableObservationCount = 0
        lastStableSignature = nil
    }

    private struct StableSignature: Equatable, Sendable {
        let windowTokens: [String]
        let selectedWindowToken: String
        let frames: [NativeWindowFrame]
    }
}
