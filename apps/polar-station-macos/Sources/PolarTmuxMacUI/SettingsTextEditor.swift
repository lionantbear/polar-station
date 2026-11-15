import AppKit
import Combine
import PolarTmuxHostClient
import SwiftUI

public struct SettingsPromptControls:View {
    @ObservedObject private var interaction:SettingsTextInteraction
    private let buffer:SettingsTextBuffer
    public init(interaction:SettingsTextInteraction,buffer:SettingsTextBuffer) {
        self.interaction=interaction;self.buffer=buffer
    }
    public var body:some View {
        VStack(alignment:.leading,spacing:8) {
            if let state=interaction.state {
                ForEach(state.fields) { field in
                    Button { buffer.beginEditing();interaction.edit(field.field) } label: {Label(field.title,systemImage:"pencil")}
                        .disabled(state.pending || state.loading)
                }
            }
            SettingsTextEditor(interaction:interaction,buffer:buffer)
        }
    }
}

/// Native plain text only; the same interaction is retained when presentation moves.
public struct SettingsTextEditor: View {
    @ObservedObject private var interaction:SettingsTextInteraction
    private let buffer:SettingsTextBuffer
    private let expanded:Bool
    public init(interaction:SettingsTextInteraction,buffer:SettingsTextBuffer,expanded:Bool=false) {
        self.interaction=interaction;self.buffer=buffer;self.expanded=expanded
    }
    public var body:some View {
        if let state=interaction.state,state.field != nil,(!expanded || state.expanded) {
            VStack(alignment:.leading,spacing:12) {
                HStack {
                    Text(state.title).font(.headline)
                    Spacer(minLength:0)
                    if state.dirty { Text(state.actions.unsaved).foregroundStyle(.secondary) }
                }
                if state.loading {ProgressView(state.actions.loading)}
                if state.expanded && !expanded {
                    Button(state.actions.focus) { interaction.action("expand") }
                } else {
                    SettingsTextBufferView(buffer:buffer,expanded:expanded)
                        .frame(minHeight:expanded ? 0 : 120,maxHeight:expanded ? .infinity : 180)
                        .accessibilityLabel(state.title)
                    HStack {
                        Button(state.actions.restore) { interaction.action("restore_default") }.disabled(!state.can_edit)
                        Spacer(minLength:0)
                        Button(expanded ? state.actions.collapse : state.actions.expand) { interaction.action(expanded ? "collapse" : "expand") }
                            .disabled(!state.can_edit && !state.expanded)
                    }
                    if let error=state.error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
                    if state.error != nil || state.can_rebase {
                        Button(state.actions.refresh) {interaction.action("refresh")}.disabled(state.pending || state.loading)
                    }
                    if state.can_rebase { Button(state.actions.rebase) {interaction.action("rebase")} }
                    HStack {
                        Button(state.actions.cancel) { interaction.action("close") }.disabled(state.pending)
                        Spacer()
                        Button(state.actions.save) { interaction.action("save") }.disabled(!state.can_save)
                    }
                }
            }.padding(12)
            .onAppear {if expanded {buffer.focus()}}

        }
    }
}

/// One native confirmation presenter also covers hidden Tabs and application Quit.
/// The shared editor supplies every label and determines the result.
@MainActor public final class SettingsTextConfirmationPresenter {
    private let interaction:SettingsTextInteraction
    private let parent:()->NSWindow?
    private var observation:AnyCancellable?
    private var alert:NSAlert?
    public init(interaction:SettingsTextInteraction,parent:@escaping ()->NSWindow?) {
        self.interaction=interaction;self.parent=parent
        observation=interaction.$state.sink { [weak self] state in
            guard let state else {return}
            self?.present(state)
        }
    }
    private func present(_ state:SettingsTextInteraction.ViewState) {
        guard state.confirm_close,!state.pending,alert == nil else {return}
        guard let window=parent(),window.attachedSheet == nil else {
            Task {interaction.action("keep_editing")};return
        }
        let prompt=NSAlert();alert=prompt
        prompt.messageText=state.confirmation
        prompt.informativeText=state.error ?? ""
        prompt.alertStyle = .warning
        prompt.addButton(withTitle:state.actions.save).isEnabled=state.can_save
        prompt.addButton(withTitle:state.actions.discard)
        prompt.addButton(withTitle:state.actions.keep).keyEquivalent="\u{1b}"
        prompt.beginSheetModal(for:window) { [weak self] response in
            guard let self else {return}
            self.alert=nil
            self.interaction.action(response == .alertFirstButtonReturn ? "save" : response == .alertSecondButtonReturn ? "discard" : "keep_editing")
        }
    }
}
