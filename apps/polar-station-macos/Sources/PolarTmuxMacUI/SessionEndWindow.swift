import AppKit
import Combine
import PolarTmuxHostClient
import SwiftUI

@MainActor
final class SessionEndDraft: ObservableObject, Identifiable {
    let id = UUID()
    let target: SessionEditorRequestV1
    let interaction = SessionEditorInteraction()
    private var observation: AnyCancellable?
    var preview: SessionEndPreviewV1? { interaction.state?.end_preview }
    var pending: Bool { interaction.state?.pending ?? false }
    var message: String { interaction.state?.message ?? "" }
    var canSubmit: Bool { interaction.state?.can_submit ?? false }
    let read: SessionEditorReader?
    let submit: @MainActor (ControlCenterIntentOperationV1, @escaping @MainActor (String?) -> Void) -> Void
    init(identity: SessionRowProjectionV1.Identity, read: SessionEditorReader?,
         submit: @escaping @MainActor (ControlCenterIntentOperationV1, @escaping @MainActor (String?) -> Void) -> Void) {
        self.target = .previewEnd(identity); self.read = read; self.submit = submit
    }
    init(agent: AgentSessionCatalogRowV1, read: SessionEditorReader?,
         submit: @escaping @MainActor (ControlCenterIntentOperationV1, @escaping @MainActor (String?) -> Void) -> Void) {
        self.target = .previewEndAgent(target: agent.execution_target_id, owner: agent.owner_host_id, endpoint: agent.endpoint_id, thread: agent.thread_id)
        self.read = read; self.submit = submit
    }
    func load() {
        if observation == nil {
            observation = interaction.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }
            interaction.submit = { [weak self] operation, completion in
                self?.submit(operation) { error in completion(error, []) }
            }
            interaction.openEnd(target, read: read)
        } else { interaction.action("refresh_end") }
    }
    func close() { interaction.action("close") }
    func confirm() { interaction.action("submit") }

}

private struct SessionEndDialog: View {
    @ObservedObject var draft: SessionEndDraft
    let close: @MainActor () -> Void
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(draft.preview?.confirmation.title ?? "End Session?").font(.title3.bold())
            if let preview = draft.preview { Text(preview.confirmation.message).fixedSize(horizontal: false, vertical: true) }
            if !draft.message.isEmpty { Text(draft.message).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true) }
            if draft.pending { ProgressView() }
            HStack {
                Button("Refresh") { draft.load() }.disabled(draft.pending)
                Spacer()
                Button("Close", action: close).keyboardShortcut(.cancelAction)
                Button(draft.preview?.confirmation.confirmLabel ?? "End Session", role: .destructive) { draft.confirm() }
                    .disabled(!draft.canSubmit)
            }
        }.padding(20).frame(width: 430)
        .onAppear { draft.load() }
        .onReceive(draft.interaction.$state) { state in if state?.end_completed == true { close() } }
        .onDisappear { draft.close() }
    }
}

/// A modeless child window: no nested menu grab or blocked Workspace event loop.
struct SessionEndWindowHost: NSViewRepresentable {
    var draft: SessionEndDraft?
    let close: @MainActor () -> Void
    func makeNSView(context: Context) -> NSView { NSView() }
    func makeCoordinator() -> Coordinator { Coordinator() }
    func updateNSView(_ view: NSView, context: Context) {
        guard let draft else { context.coordinator.dismiss(); return }
        guard context.coordinator.id != draft.id else { return }
        context.coordinator.dismiss()
        let content = NSHostingController(rootView: SessionEndDialog(draft: draft, close: close))
        let window = NSWindow(contentViewController: content)
        window.title = "End Session"
        window.identifier = .init("polar-station-session-end")
        window.styleMask = [.titled, .closable]
        window.isReleasedWhenClosed = false
        window.center()
        context.coordinator.id = draft.id
        context.coordinator.onClose = close
        context.coordinator.controller = NSWindowController(window: window)
        window.delegate = context.coordinator
        if let parent = view.window { parent.addChildWindow(window, ordered: .above) }
        context.coordinator.controller?.showWindow(nil)
        window.makeKeyAndOrderFront(nil)
    }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) { coordinator.dismiss() }
    @MainActor final class Coordinator: NSObject, NSWindowDelegate {
        var id: UUID?
        var controller: NSWindowController?
        var onClose: (@MainActor () -> Void)?
        func dismiss() {
            if let window = controller?.window {
                window.delegate = nil
                window.parent?.removeChildWindow(window)
                window.close(); window.contentViewController = nil
            }
            controller = nil; id = nil
        }
        func windowWillClose(_ notification: Notification) { onClose?() }
    }
}
