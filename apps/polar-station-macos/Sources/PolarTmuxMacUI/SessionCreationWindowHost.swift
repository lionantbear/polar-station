import AppKit
import SwiftUI

/// Owns presentation only; the supplied submit closure retains the captured Tab.
struct SessionCreationWindowHost<Content: View>: NSViewRepresentable {
    let request: SessionEditorRequest?
    let close: @MainActor () -> Void
    @ViewBuilder let content: (SessionEditorRequest) -> Content
    func makeNSView(context: Context) -> NSView { NSView() }
    func makeCoordinator() -> Coordinator { Coordinator() }
    func updateNSView(_ view: NSView, context: Context) {
        guard let request else { context.coordinator.dismiss(); return }
        let owner = context.coordinator
        if owner.id == request.id {
            (owner.controller?.contentViewController as? NSHostingController<Content>)?.rootView = content(request)
            return
        }
        owner.dismiss()
        let window = SessionCreationWindow(contentViewController: NSHostingController(rootView: content(request)))
        window.title = request.bindPane == nil ? "Add Session" : "Bind Codex session"
        window.identifier = .init("polar-station-session-editor-" + request.id.uuidString)
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.isReleasedWhenClosed = false
        owner.id = request.id; owner.onClose = close
        owner.controller = NSWindowController(window: window)
        window.delegate = owner
        if let parent = view.window {
            window.setFrameOrigin(parent.frame.origin)
            owner.parentObserver = NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: parent, queue: .main) { [weak owner] _ in
                MainActor.assumeIsolated { owner?.onClose?(); owner?.dismiss() }
            }
        }
        owner.controller?.showWindow(nil)
        window.makeKeyAndOrderFront(nil)
    }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) { coordinator.dismiss() }
    @MainActor final class Coordinator: NSObject, NSWindowDelegate {
        var id: UUID?
        var controller: NSWindowController?
        var onClose: (@MainActor () -> Void)?
        var parentObserver: NSObjectProtocol?
        func dismiss() {
            if let parentObserver { NotificationCenter.default.removeObserver(parentObserver) }
            parentObserver = nil
            controller?.window?.delegate = nil
            controller?.close(); controller = nil; id = nil
        }
        func windowShouldClose(_ sender: NSWindow) -> Bool { !sender.isDocumentEdited }
        func windowWillClose(_ notification: Notification) { onClose?() }
    }
}

/// Native responder fallback when a SwiftUI text field owns the editor.
private final class SessionCreationWindow: NSWindow {
    override func cancelOperation(_ sender: Any?) {
        guard !isDocumentEdited, attachedSheet == nil else { return }
        if let editor = firstResponder as? NSTextView, editor.hasMarkedText() {
            super.cancelOperation(sender)
            return
        }
        performClose(sender)
    }
}
