import Foundation

/// Keeps application startup and warm reopen under one native lifecycle owner.
/// Window presentation must complete before asynchronous Workspace or Host work.
struct ManagedApplicationPresentationState: Equatable, Sendable {
    enum Phase: Equatable, Sendable {
        case stopped
        case runningWithWindow
        case runningWithoutWindow
    }

    enum Command: Equatable, Sendable {
        case createAndPresentWindow
        case presentExistingWindow
        case startAsynchronousRuntime
    }

    private(set) var phase: Phase = .stopped

    mutating func start(hasVisibleWindow: Bool) -> [Command] {
        guard phase == .stopped else { return [] }
        phase = .runningWithWindow
        return [
            hasVisibleWindow
                ? .presentExistingWindow
                : .createAndPresentWindow,
            .startAsynchronousRuntime,
        ]
    }

    mutating func windowClosed() {
        guard phase != .stopped else { return }
        phase = .runningWithoutWindow
    }

    mutating func reopen(hasVisibleWindow: Bool) -> [Command] {
        guard phase != .stopped else { return [] }
        phase = .runningWithWindow
        return [
            hasVisibleWindow
                ? .presentExistingWindow
                : .createAndPresentWindow,
        ]
    }

    mutating func stop() {
        phase = .stopped
    }
}
