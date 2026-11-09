import AppKit
import SwiftUI

/// Physical list focus only. Rust owns movement, checked IDs and submission.
struct SessionHistoryKeyboard: NSViewRepresentable {
    let focusRequest: Int
    let action: (String) -> Void

    func makeNSView(context: Context) -> Responder { Responder() }
    func updateNSView(_ view: Responder, context: Context) {
        view.action = action
        if view.focusRequest != focusRequest {
            view.focusRequest = focusRequest
            view.requestFocus()
        }
    }

    final class Responder: NSView {
        var action: ((String) -> Void)?
        var focusRequest: Int?
        override var acceptsFirstResponder: Bool { true }
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if window != nil { requestFocus() }
        }
        func requestFocus() {
            DispatchQueue.main.async { [weak self] in
                guard let self, let window = self.window, window.isKeyWindow else { return }
                window.makeFirstResponder(self)
            }
        }
        override func keyDown(with event: NSEvent) {
            guard event.modifierFlags.intersection([.command, .option, .control, .shift]).isEmpty else {
                super.keyDown(with: event); return
            }
            switch event.keyCode {
            case 126: action?("previous")
            case 125: action?("next")
            case 49: action?("toggle_selected")
            case 36, 76: if !event.isARepeat { action?("submit") }
            case 53: action?("cancel")
            default: super.keyDown(with: event)
            }
        }
    }
}
