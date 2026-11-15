import Foundation
import SwiftUI

public enum ApplicationShortcutEditorCommand: Equatable, Sendable {
    case newSession(String)
    case resumeCodex
    case bindIdeaCodex(pane: String, view: String)
    case newWorkspace
    case renameWorkspace
}

public struct ApplicationShortcutEditorRequest: Equatable, Identifiable, Sendable {
    public let id: UUID
    public let command: ApplicationShortcutEditorCommand
}

/// A window-scoped bridge into the existing SwiftUI editors. Keeping one
/// instance per managed window prevents a shortcut from opening hidden alerts
/// in every cached Control Center.
@MainActor
public final class ApplicationShortcutCommandState: ObservableObject {
    @Published public private(set) var request: ApplicationShortcutEditorRequest?
    @Published var sessionEditor: SessionEditorRequest?

    public var bindCodexSelection: (@MainActor (String,String,String,String,String,String) -> Void)?

    public init() {}

    public func send(_ command: ApplicationShortcutEditorCommand) {
        request = .init(id: UUID(), command: command)
    }
}
