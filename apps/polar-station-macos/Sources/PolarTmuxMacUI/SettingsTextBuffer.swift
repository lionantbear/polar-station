import AppKit
import Combine
import PolarTmuxHostClient
import SwiftUI

/// One physical plain-text editor moves between sidebar and Workspace Pane.
/// Rust owns its draft; AppKit owns marked text, selection and undo history.
@MainActor public final class SettingsTextBuffer: NSObject, NSTextViewDelegate {
    private final class TextView: NSTextView {
        let textUndo = UndoManager()
        override var undoManager: UndoManager? { textUndo }
    }
    private let editor = TextView(frame:.zero)
    private let scroll = NSScrollView(frame:.zero)
    private let interaction: SettingsTextInteraction
    private var observation: AnyCancellable?
    private var documentLoaded=false
    private var field: String?
    private var updating = false
    private weak var presentationWindow:NSWindow?
    public func beginEditing() {
        if interaction.state?.expanded != true {presentationWindow=NSApp.keyWindow}
    }
    public func setPresentationWindow(_ window:NSWindow?) {presentationWindow=window}
    public init(interaction: SettingsTextInteraction) {
        self.interaction=interaction
        super.init()
        editor.isRichText=false
        editor.allowsUndo=true
        editor.isAutomaticQuoteSubstitutionEnabled=false
        editor.isAutomaticDashSubstitutionEnabled=false
        editor.isAutomaticSpellingCorrectionEnabled=false
        editor.isVerticallyResizable=true
        editor.isHorizontallyResizable=false
        editor.minSize = .zero
        editor.maxSize = NSSize(width:CGFloat.greatestFiniteMagnitude,height:CGFloat.greatestFiniteMagnitude)
        editor.autoresizingMask = [.width]
        editor.textContainer?.widthTracksTextView=true
        editor.textContainerInset=NSSize(width:10,height:10)
        editor.drawsBackground=false
        editor.textColor = .labelColor
        editor.delegate=self
        scroll.hasVerticalScroller=true
        scroll.hasHorizontalScroller=false
        scroll.drawsBackground=false
        scroll.documentView=editor
        observation=interaction.$state.sink { [weak self] state in
            guard let state else {return}
            self?.synchronize(state)
        }
    }
    private func synchronize(_ state:SettingsTextInteraction.ViewState) {
        updating=true
        defer {updating=false}
        editor.isEditable=state.can_edit
        editor.setAccessibilityLabel(state.title)
        guard !editor.hasMarkedText() else {return}
        if field != state.field || (!documentLoaded && state.can_edit) {
            documentLoaded=state.can_edit
            editor.textUndo.disableUndoRegistration()
            editor.string=state.text
            editor.textUndo.enableUndoRegistration()
            editor.textUndo.removeAllActions()
            editor.setSelectedRange(NSRange(location:0,length:0))
            field=state.field
            if documentLoaded {(scroll.superview as? SettingsTextContainer)?.prepareFocus()}
        } else if editor.string != state.text {
            editor.breakUndoCoalescing()
            editor.insertText(state.text,replacementRange:NSRange(location:0,length:(editor.string as NSString).length))
            editor.breakUndoCoalescing()
        }
    }
    public func undoManager(for view:NSTextView)->UndoManager? {editor.textUndo}
    public func textDidChange(_ notification:Notification) {
        if !updating {interaction.change(editor.string)}
    }
    public func focus() {
        guard let window=editor.window else {return}
        window.makeFirstResponder(editor)
    }
    fileprivate func mount(in parent:NSView,scale:CGFloat,expanded:Bool) {
        guard interaction.state?.expanded == expanded,let window=parent.window,
              window === (presentationWindow ?? NSApp.keyWindow) else {return}
        editor.font=NSFont.systemFont(ofSize:NSFont.systemFontSize*scale)
        if scroll.superview !== parent {
            scroll.removeFromSuperview()
            scroll.autoresizingMask=[.width,.height]
            scroll.frame=parent.bounds
            parent.addSubview(scroll)
            if let parent=parent as? SettingsTextContainer {parent.prepareFocus()}
        }
    }
    fileprivate func attached(in parent:NSView) {
        guard scroll.superview === parent,editor.isEditable else {return}
        parent.scrollToVisible(parent.bounds)
        focus()
    }
}

private final class SettingsTextContainer:NSView {
    weak var buffer:SettingsTextBuffer?
    private var pendingFocus=false
    private var scale:CGFloat=1
    private var expanded=false
    func configure(scale:CGFloat,expanded:Bool) {
        self.scale=scale;self.expanded=expanded
        mountIfAttached()
    }
    private func mountIfAttached() {buffer?.mount(in:self,scale:scale,expanded:expanded)}
    init(buffer:SettingsTextBuffer) {self.buffer=buffer;super.init(frame:.zero)}
    required init?(coder:NSCoder) {fatalError("Settings editor is constructed by the native adapter")}
    func prepareFocus() {pendingFocus=true;needsLayout=true}
    override func viewDidMoveToWindow() {super.viewDidMoveToWindow();if window != nil {mountIfAttached();prepareFocus()}}
    override func layout() {
        super.layout()
        mountIfAttached()
        guard pendingFocus,window != nil,bounds.width>0,bounds.height>0 else {return}
        pendingFocus=false;buffer?.attached(in:self)
    }
}

struct SettingsTextBufferView: NSViewRepresentable {
    let buffer:SettingsTextBuffer
    let expanded:Bool
    @Environment(\.polarStationInterfaceTextScale) private var scale
    func makeNSView(context:Context)->NSView {SettingsTextContainer(buffer:buffer)}
    func updateNSView(_ view:NSView,context:Context) {(view as? SettingsTextContainer)?.configure(scale:scale,expanded:expanded)}
}
