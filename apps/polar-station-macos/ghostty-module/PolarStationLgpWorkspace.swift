#if POLAR_STATION
import AppKit
import Foundation
import GhosttyKit
import PolarTmuxHostClient
import CPolarStationDesktopClient

/// Native adapter for the shared Rust conversation state. Ghostty displays
/// text; this owner supplies IME, input, browsing, focus and OS interaction.
/// All FFI calls are main-thread synchronous and effects contain no authority.
@MainActor
final class PolarStationLgpWorkspace: NSObject {
    let dictate = PolarStationDictate()
    var manageConnections: (() -> Void)?
    var focusInputSource: ((NSWindow,String) -> Void)?
    var terminalSelection: ((NSWindow, String) -> String?)?
    var retryHost: (() -> Void)?
    var sendSession: ((Data) async throws -> Void)?
    var sendFrame: ((Data) async throws -> Void)?
    var ensureTerminalVisible: ((String, String, [String:Any]) async throws -> Void)?
    var openPane: ((String, StationPaneContentV1, String?) async throws -> Void)?
    var workspaceIDForWindow: ((NSWindow) -> String?)?
    var saveSendMode: ((ChatSendModeV1) -> Void)?
    private var sendMode: ChatSendModeV1 = .enter
    private var pendingSelection: Set<String> = []
    private var ideaInstructionRequests: [String: UInt64] = [:]
    private let storage = Storage()
    private var panes: [String: Pane] = [:]
    var chooseIdeaSession: ((String,String,String) -> Void)?
    func bindIdeaSession(pane: String, view: String, owner: String, endpoint: String, thread: String, title: String) {
        input(["type":"idea_pick_session","pane_id":pane,"view_id":view,"owner_host_id":owner,"endpoint_id":endpoint,"thread_id":thread,"title":title]);scheduleDraftSave()
    }
    private var composers: [String: StationInputBarView] = [:]
    private var browser: BrowserPanel?
    private var activityPanel: ToolActivityPanel?
    private var generation: UInt64 = 0
    private let clockStart = ProcessInfo.processInfo.systemUptime
    private var leaseTimer: Timer?
    private var flushScheduled = false
    private var saveWork: DispatchWorkItem?
    private var inputQueue: [Data] = []
    private var dispatching = false
    private var stopped = false
    private struct Pane {
        let content: StationPaneContentV1
        let display: PolarStationDisplaySurface
        let windowID: String
    }
    private final class Storage: @unchecked Sendable {
        let handle = polar_station_input_new()
        deinit { if let handle { polar_station_input_free(handle) } }
    }

    private var referenceCollectionEnabled:Bool?
    override init() {
        super.init()
        dictate.selectedReference = { [weak self] in
            guard let self, let window=NSApp.keyWindow else { return "" }
            if let editor=window.firstResponder as? NSTextView {
                guard !(editor.delegate is NSSecureTextField) else { return "" }
                let text=editor.string as NSString, range=editor.selectedRange()
                return range.location != NSNotFound && NSMaxRange(range)<=text.length ? text.substring(with:range) : ""
            }
            guard let panel=self.composers.values.first(where:{$0.window === window}), let pane=panel.paneID else {return ""}
            return self.terminalSelection?(window,pane) ?? ""
        }
        dictate.shortcutTargets = { [weak self] in
            guard let self, NSApp.isActive, let window = NSApp.keyWindow else { return [] }
            if let panel = self.composers.values.first(where: { $0.window === window }) {
                return panel.dictateControls.targets?() ?? []
            }
            if let editor = window.firstResponder as? NSTextView,
               !(editor.delegate is NSSecureTextField) {
                return StationDictateTarget.editor(editor,includeContext:self.dictate.collectsContext).map { [$0] } ?? []
            }
            return []
        }
        dictate.changed = { [weak self] in
            if let self,self.referenceCollectionEnabled != self.dictate.collectsContext {
                self.referenceCollectionEnabled = self.dictate.collectsContext
                self.input(["type":"reference_collection","enabled":self.dictate.collectsContext])
            }
            for panel in self?.composers.values ?? Dictionary<String, StationInputBarView>().values { panel.dictateControls.refresh() }
        }
        dictate.currentProject = {[weak self] in
            guard NSApp.isActive,let window=NSApp.keyWindow else{return nil}
            return self?.composers.values.first(where:{$0.window === window})?.refineProject
        }
        dictate.currentConversation = {[weak self] in
            guard NSApp.isActive,let window=NSApp.keyWindow else{return nil}
            return self?.composers.values.first(where:{$0.window === window})?.conversationContext
        }
        dictate.currentTarget = {[weak self] editor,preserve in
            self?.composers.values.first(where:{$0.window === editor.window})?.dictateEditorTarget(editor,preserveSelection:preserve)
        }
        dictate.focusedTarget = {[weak self] editor in
            self?.composers.values.first(where:{$0.window === editor.window})?.dictateEditorTarget(editor)
                ?? StationDictateTarget.editor(editor)
        }
        restoreDrafts()
        // Only pending-request deadlines and lease maintenance. Message data
        // arrives via a real Host event feed, not this timer.
        leaseTimer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.input(["type": "tick", "now_ms": UInt64(max(0, ProcessInfo.processInfo.systemUptime - self.clockStart) * 1000)])
            }
        }
    }
    func stop() {
        guard !stopped else { return }
        dictate.stop()
        input(["type": "export_drafts"])
        input(["type": "disconnected", "generation": generation])
        stopped = true; leaseTimer?.invalidate(); leaseTimer = nil
        saveWork?.cancel(); saveWork = nil
        for panel in composers.values { panel.close() }
        composers.removeAll(); browser?.close(); browser = nil
        activityPanel?.onClose = nil; activityPanel?.close(); activityPanel = nil
        panes.removeAll()
    }
    func receive(_ data: Data) {
        if let value = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           value["type"] as? String == "connected", let g = value["generation"] as? NSNumber {
            generation = g.uint64Value
        }
        dispatch(data)
    }
    func created(_ paneID: String, content: StationPaneContentV1,
                 display: PolarStationDisplaySurface, window: NSWindow?) {
        let tracing = ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil
        if tracing { NSLog("Station input registration pane=%@ stopped=%d window=%d",paneID,stopped ? 1 : 0,window == nil ? 0 : 1) }
        guard !stopped, let window else { return }
        guard let windowID = attachWindow(window) else {
            if tracing { NSLog("Station input registration awaiting workspace pane=%@",paneID) }
            return
        }
        if tracing { NSLog("Station input registered pane=%@ window=%@",paneID,windowID) }
        panes[paneID] = Pane(content: content, display: display, windowID: windowID)
        if content.kind == .ideaTree {
            input(["type": "open_idea", "pane_id": paneID, "window_id": windowID, "view_id": content.viewID])
            display.subscribeIdea { [weak self] event in
                guard let self, self.panes[paneID]?.content.viewID == content.viewID else { return }
                if let state = event["state"] as? [String: Any] {
                    if self.composers[windowID]?.paneID == paneID {
                        self.composers[windowID]?.setIdeaReaderOpen(state["reader_open"] as? Bool ?? false)
                    }
                    self.input(["type": "idea_state", "pane_id": paneID, "view_id": content.viewID, "state": state])
                    if self.composers[windowID]?.paneID == paneID {
                        self.composers[windowID]?.updateInstructionPresentation(rect: state["instruction_editor_rect"] as? [String: Double], style: state["instruction_style"] as? [String: String])
                        let request = (state["instruction_request_id"] as? NSNumber)?.uint64Value ?? 0
                        if request > (self.ideaInstructionRequests[paneID] ?? 0) {
                            self.ideaInstructionRequests[paneID] = request
                            self.composers[windowID]?.editIdeaInstruction(in: display.view)
                        }
                    }
                } else if event["kind"] as? String == "open_source", let path = event["path"] as? String, path.hasPrefix("/"), FileManager.default.fileExists(atPath: path) {
                    NSWorkspace.shared.open(URL(fileURLWithPath: path))
                } else if event["kind"] as? String == "edit_node_instruction", self.composers[windowID]?.paneID == paneID {
                    self.composers[windowID]?.editIdeaInstruction(in: display.view)
                } else if event["kind"] as? String == "focus_search", self.composers[windowID]?.paneID == paneID {
                    self.composers[windowID]?.focusIdeaSearch()
                }
            }
            if pendingSelection.remove(paneID) != nil, (window.windowController as? TerminalController)?.focusedSurface === display.view { selected(paneID) }
            return
        }
        display.referenceAction = { [weak self, weak display] revision, row, column in
            guard let display else { return }
            let blocked = display.view.surface.map { ghostty_surface_has_selection($0) } ?? true
            self?.input(["type":"render_guard", "pane_id":paneID, "blocked":blocked])
            if let row { self?.input(["type":"activate_row", "pane_id":paneID, "revision":revision, "row":row, "column":column]) }
            else { self?.input(["type":"return_to_reading", "pane_id":paneID]) }
        }
        display.geometryChanged = { [weak self] in self?.refreshRenderGuards() }
        display.readingAction = { [weak self] retry in
            self?.input(["type": retry ? "retry_display" : "jump_to_latest", "pane_id": paneID])
        }
        input(["type": "open", "pane_id": paneID, "window_id": windowID, "scope": scope(content)])
        if pendingSelection.remove(paneID) != nil, (window.windowController as? TerminalController)?.focusedSurface === display.view { selected(paneID) }
    }
    @discardableResult
    func attachWindow(_ window: NSWindow, workspaceID: String? = nil) -> String? {
        guard let windowID = workspaceID ?? workspaceIDForWindow?(window) else { return nil }
        // Workspace identity survives process and native tab-group replacement.
        for (id, panel) in composers where id != windowID {
            if panel.window === window || (window.tabGroup != nil && panel.window?.tabGroup === window.tabGroup) {
                panel.commitDraft(); panel.close()
                input(["type": "window_closed", "window_id": id])
                composers.removeValue(forKey: id)
            }
        }
        if composers[windowID] == nil {
            let panel = StationInputBarView(dictate: dictate)
            panel.dictateControls.targets = { [weak self, weak panel] in
                guard let self, let panel else { return [] }
                if let editor = panel.window?.firstResponder as? NSTextView {
                    if editor.delegate is NSSecureTextField {
                        let target = StationDictateTarget(kind: "editor", context: "", insert: { _ in .unchanged })
                        return [target.protectedTarget()]
                    }
                    let pane = panel.paneID, revision = panel.dictateRevision, inputIdentity = panel.dictateInputIdentity
                    if let target = StationDictateTarget.editor(editor, valid: { [weak panel] in panel?.paneID == pane && panel?.dictateRevision == revision && panel?.dictateInputIdentity == inputIdentity }) { return [target] }
                }
                let canvas = panel.paneID.flatMap { self.panes[$0]?.display.view }
                let responder = panel.window?.firstResponder as? NSView
                let node = canvas.map { responder === $0 || responder?.isDescendant(of: $0) == true } ?? false
                return panel.dictateDefaultTarget(node: node).map { [$0] } ?? []
            }
            panel.onInquiry = { [weak self] action, token, value in
                var input: [String: Any] = ["type":"inquiry_action", "window_id":windowID, "action":action]
                if let token { input["token"] = token }; if let value { input["value"] = value }
                self?.input(input)
            }
            panel.onEnvironment = { [weak self] foreground, composing in
                self?.input(["type":"inquiry_environment", "window_id":windowID, "foreground":foreground, "composing":composing])
            }
            panel.onChooseIdea = { [weak self, weak panel] in
                guard let self, let id=panel?.paneID, let pane=self.panes[id], pane.content.kind == .ideaTree else { return }
                self.chooseIdeaSession?(windowID,id,pane.content.viewID)
            }
            panel.onIdeaInput = { [weak self] value in
                var value = value; value["window_id"] = windowID
                self?.input(value); self?.scheduleDraftSave()
            }
            panel.onDictated = {[weak self] lease,source,text in
                self?.input(["type":"dictated_input","lease":lease,"source":source,"text":text])
            }
            panel.onRefined = { [weak self] lease,original,text,english in
                self?.input(["type":"refined_input","lease":lease,"original":original,"text":text,"english":english])
            }
            panel.onDraft = { [weak self, weak panel] text in
                if let lease = panel?.genericLease {
                    self?.input(["type":"edit_input", "lease":lease, "text":text])
                } else {
                    self?.input(["type": "draft", "window_id": windowID, "text": text])
                }
                self?.scheduleDraftSave()
            }
            panel.onNativeIdeaCommand = { [weak self, weak panel] action, query in
                guard let self, let id = panel?.paneID, let pane = self.panes[id], pane.content.kind == .ideaTree else { return }
                pane.display.ideaCommand(action, query: query)
            }
            panel.onIdeaAction = { [weak self] action, query in
                self?.input(["type": "idea_action", "window_id": windowID, "action": action, "query": query])
            }
            panel.onSend = { [weak self, weak panel] in
                if let lease = panel?.genericLease {
                    self?.input(["type":"submit_input","lease":lease,"press_enter":true])
                } else {
                    self?.input(["type": "send", "window_id": windowID,
                        "created_at_secs": UInt64(Date().timeIntervalSince1970)])
                }
            }
            panel.onExit = { [weak self, weak panel] in
                guard let panel, let window=panel.window, let pane=panel.paneID else {return}
                self?.focusInputSource?(window,pane)
            }
            panel.onQuote = { [weak self, weak panel] in
                guard let panel, let window = panel.window, let pane = panel.paneID,
                      let lease = panel.genericLease, let text = self?.terminalSelection?(window, pane) else { return }
                self?.input(["type":"quote_selection","lease":lease,"text":text])
            }
            panel.onOlder = { [weak self, weak panel] in
                guard let id = panel?.paneID else { return }; self?.input(["type": "older", "pane_id": id])
            }
            panel.onRetry = { [weak self, weak panel] in
                if let lease = panel?.genericLease { self?.input(["type":"refresh_input","lease":lease]); return }
                guard let id = panel?.paneID else { return }; self?.input(["type": "retry", "pane_id": id])
            }
            panel.onReferences = { [weak self, weak panel] in
                guard let id = panel?.paneID else { return }; self?.input(["type": "references", "pane_id": id])
            }
            panel.onActivity = { [weak self, weak panel] in
                guard let id = panel?.paneID else { return }; self?.input(["type": "activity", "pane_id": id])
            }
            panel.onSearch = { [weak self, weak panel] query in
                guard let id = panel?.paneID else { return }; self?.input(["type": "search", "pane_id": id, "query": query])
            }
            panel.onChoose = { [weak self, weak panel] in
                guard let self, let panel else { return }
                if panel.picker == nil {
                    panel.picker = ConversationPicker(windowID: windowID)
                    panel.picker?.onManageConnections = { [weak self] in self?.manageConnections?() }
                    panel.picker?.onInput = { [weak self] in self?.input($0) }
                    panel.picker?.onOpen = { [weak self, weak panel] content, newPane in
                        guard let self, let panel else { return }
                        panel.commitDraft()
                        let replacement = newPane ? nil : panel.paneID
                        Task { @MainActor [weak self] in
                            do { try await self?.openPane?(windowID, content, replacement) }
                            catch { self?.showError(error.localizedDescription) }
                        }
                    }
                }
                panel.picker?.present(from: panel.targetButton, scope: panel.currentScope, channel: panel.channelID)
            }
            panel.onWindowClosed = { [weak self] in
                guard let self else { return }
                self.input(["type": "window_closed", "window_id": windowID])
                self.composers.removeValue(forKey: windowID)
            }
            panel.onMode = { [weak self] mode in self?.setSendMode(mode); self?.saveSendMode?(mode) }
            panel.setSendMode(sendMode)
            composers[windowID] = panel
        }
        // Registering a background restored tab must not move the workspace's
        // single footer away from its already focused native window.
        if window.isKeyWindow || composers[windowID]?.window?.isKeyWindow != true {
            composers[windowID]?.attach(to: window)
        }
        return windowID
    }
    func terminalSelected(_ paneID: String, in window: NSWindow, targetID:String, sessionID:String) {
        guard let id = attachWindow(window) else { return }
        composers[id]?.commitDraft()
        input(["type":"select_terminal", "window_id":id, "pane_id":paneID, "target_id":targetID, "session_id":sessionID])
    }
    func retired(_ paneID: String) {
        pendingSelection.remove(paneID)
        guard let pane = panes.removeValue(forKey: paneID) else { return }
        pane.display.subscribeIdea(nil)
        input(["type": "close", "pane_id": paneID])
    }
    func setCachePolicy(_ policy: ChatCachePolicy) {
        input(["type": "cache_policy", "policy": ["message_mib": policy.messageMiB, "presentation_mib": policy.presentationMiB]])
    }
    func setSendMode(_ mode: ChatSendModeV1) {
        sendMode = mode; for panel in composers.values { panel.setSendMode(mode) }
    }
    func refreshRenderGuards() {
        for (id, pane) in panes where pane.content.kind == .lgpText {
            let blocked = pane.display.pointerPending || (pane.display.view.surface.map { ghostty_surface_has_selection($0) } ?? false)
            if let surface = pane.display.view.surface {
                input(["type": "render_metrics", "pane_id": id, "columns": Int(ghostty_surface_size(surface).columns)])
            }
            input(["type": "render_guard", "pane_id": id, "blocked": blocked])
            if let scrollbar = pane.display.view.scrollbar, let snapshot = try? pane.display.snapshot() {
                input(["type": "viewport", "pane_id": id,
                    "at_bottom": scrollbar.offset + scrollbar.len >= scrollbar.total,
                    "top_row": scrollbar.offset, "visible_rows": scrollbar.len,
                    "revision": snapshot.revision,
                    "visible": pane.display.view.window?.isVisible == true && !pane.display.view.isHiddenOrHasHiddenAncestor])
            }
        }
        input(["type": "flush"])
    }
    func selected(_ paneID: String) {
        if ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil { NSLog("Station input selected pane=%@ registered=%d",paneID,panes[paneID] == nil ? 0 : 1) }
        guard let pane = panes[paneID] else { pendingSelection.insert(paneID); return }
        if let panel = composers[pane.windowID], panel.paneID == paneID, panel.editorHasFocus { return }
        composers[pane.windowID]?.commitDraft()
        if let window = pane.display.view.window { composers[pane.windowID]?.attach(to: window) }
        input(["type": "selection_changed", "pane_id": paneID])
        refreshRenderGuards()
    }
    func activateComposer(_ paneID:String) {
        guard panes[paneID] != nil else { return }
        input(["type":"activate_composer","pane_id":paneID])
    }
    func failed(_ paneID: String, _ error: String) {
        if let pane = panes[paneID] { composers[pane.windowID]?.showStatus(error) }
        else { showError(error) }
    }
    func showBrowser(relativeTo window: NSWindow?) {
        guard let window else { return }
        guard let id = attachWindow(window) else { return }
        composers[id]?.onChoose?()
    }
    private func scope(_ content: StationPaneContentV1) -> [String: Any] {
        ["target_id": content.targetID, "node_id": content.nodeID, "owner": content.owner,
         "thread": content.thread, "actor": content.actor]
    }
    func insertIdeaNode(in window: NSWindow) -> Bool {
        guard let panel = composers.values.first(where: { $0.window === window }),
              let id = panel.paneID, let pane = panes[id], pane.content.kind == .ideaTree,
              panel.acceptsIdeaShortcut(canvas: pane.display.view) else { return false }
        panel.insertIdeaNode(); return true
    }
    func editIdeaInstruction(in window: NSWindow) -> Bool {
        guard let panel = composers.values.first(where: { $0.window === window }),
              let id = panel.paneID, let pane = panes[id], pane.content.kind == .ideaTree,
              panel.acceptsIdeaShortcut(canvas: pane.display.view) else { return false }
        pane.display.ideaCommand("edit_instruction", query: ""); return true
    }
    func quoteSelection(in window: NSWindow) -> Bool {
        guard let panel = composers.values.first(where: { $0.window === window }),
              panel.genericLease != nil else { return false }
        panel.quoteSelection()
        return true
    }
    func openInquiry(in window: NSWindow) -> Bool {
        guard let panel = composers.values.first(where: { $0.window === window }), panel.hasInquiry else { return false }
        panel.publishInquiryEnvironment(); panel.onInquiry?("show", nil, nil); return true
    }
    private func input(_ object: [String: Any]) {
        if object["type"] as? String == "frame" {
            for panel in composers.values { panel.publishInquiryEnvironment() }
        }
        do { dispatch(try JSONSerialization.data(withJSONObject: object)) }
        catch { showError("Cannot encode native input") }
    }
    private func dispatch(_ data: Data) {
        guard !stopped, let handle = storage.handle, data.count <= 1_048_576 else { return }
        // Effects may synchronously request another input (e.g. Flush). Preserve
        // event order and never recursively enter Rust with a live borrow.
        guard inputQueue.count < 128 else { showError("Native input queue is full"); return }
        inputQueue.append(data)
        guard !dispatching else { return }
        dispatching = true
        defer { dispatching = false }
        while !inputQueue.isEmpty {
            let item = inputQueue.removeFirst()
            let result: UnsafeMutablePointer<CChar>? = item.withUnsafeBytes { bytes in
                polar_station_input_dispatch(handle, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count)
            }
            guard let result else { showError("LGP state is unavailable"); continue }
            let output = Data(bytes: result, count: strlen(result))
            polar_station_desktop_string_free(result)
            guard let object = try? JSONSerialization.jsonObject(with: output) as? [String: Any] else {
                showError("Invalid shared LGP response"); continue
            }
            guard object["ok"] as? Bool == true else { showError(object["error"] as? String ?? "LGP operation failed"); continue }
            let effects = object["effects"] as? [[String:Any]] ?? []
            for e in effects where e["type"] as? String == "inquiry" {
                if let id=e["window_id"] as? String, let view=e["view"] as? [String:Any], view["active"] as? Bool == true,
                   let composer=effects.last(where:{$0["type"] as? String == "composer" && $0["window_id"] as? String == id}), let text=composer["text"] as? String {
                    composers[id]?.setInquiryText(text)
                }
                consume(e)
            }
            for e in effects where e["type"] as? String != "inquiry" { consume(e) }
        }
    }
    private func markComposerTarget(_ window:String,selected:String?) {
            for (id,pane) in panes where pane.windowID == window {
                pane.display.view.wantsLayer = true
                pane.display.view.layer?.borderWidth = id == selected ? 2 : 0
                pane.display.view.layer?.borderColor = NSColor.controlAccentColor.cgColor
                pane.display.view.setAccessibilityLabel(pane.content.kind == .ideaTree ? "Idea Tree" : id == selected ? "Chat target" : "Conversation")
            }
    }
    private func consume(_ value: [String: Any]) {
        switch value["type"] as? String {
        case "insert_text":
            guard let lease = value["lease"] as? [String:Any], let window = lease["window"] as? String,
                  let text = value["text"] as? String else { return }
            composers[window]?.insertQuotedText(text, lease: lease)
        case "learn_refine":
            guard let sample=value["sample"] as? [String:Any],let intent=sample["intent"] as? String else{return}
            dictate.storeLearning(sample){[weak self] success in self?.input(["type":"learning_stored","intent":intent,"success":success])}
        case "session_request":
            guard let request = value["request"], let data = try? JSONSerialization.data(withJSONObject:request) else {return}
            Task { @MainActor [weak self] in
                guard let self else {return}
                do {
                    guard let send = self.sendSession else {throw PolarTmuxLocalIPCError.connectionClosed}
                    try await send(data)
                } catch {
                    if let request = request as? [String:Any], let id = request["request_id"] as? String {
                        self.input(["type":"session_completed", "request_id":id,"error":error.localizedDescription])
                    }
                }
            }
        case "request":
            guard let frame = value["frame"], let data = try? JSONSerialization.data(withJSONObject: frame) else { return }
            let currentGeneration = generation
            Task { @MainActor [weak self] in
                guard let self, let sendFrame = self.sendFrame, currentGeneration == self.generation else { return }
                do {
                    try await sendFrame(data)
                    if let frame = value["frame"] as? [String: Any],
                       let request = frame["request"] as? [String: Any],
                       let requestID = request["request_id"] as? String {
                        self.input(["type": "request_admitted", "generation": currentGeneration,
                                    "request_id": requestID])
                    }
                }
                catch {
                    guard let frame = value["frame"] as? [String: Any],
                          let request = frame["request"] as? [String: Any],
                          let requestID = request["request_id"] as? String else { return }
                    self.input(["type": "request_rejected", "generation": currentGeneration,
                                "request_id": requestID,
                                "reason": (error as? PolarTmuxDesktopSendError)?.reason ?? "not_ready"])
                }
            }
        case "reconnect_host": retryHost?()
        case "needs_render":
            guard !flushScheduled else { return }; flushScheduled = true
            DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(16)) { [weak self] in
                self?.flushScheduled = false; self?.refreshRenderGuards()
            }
        case "render":
            guard let id = value["pane_id"] as? String, let pane = panes[id],
                  let base = value["base_revision"] as? NSNumber,
                  let next = value["next_revision"] as? NSNumber,
                  let operation = value["operation"] as? NSNumber,
                  let text = value["text"] as? String else { return }
            do {
                let op = ps_display_operation(rawValue: operation.uint32Value)
                try pane.display.submit(contentEpoch: 1, baseRevision: base.uint64Value,
                    nextRevision: next.uint64Value, operation: op, text: text)
                input(["type": "render_result", "pane_id": id, "revision": next, "accepted": true])
            } catch {
                // A rejected batch changes neither pixels nor committed revision.
                input(["type": "render_result", "pane_id": id, "revision": next, "accepted": false])
                failed(id, "Display update failed; previous content preserved. " + error.localizedDescription)
            }
        case "reference_controls":
            guard let id = value["pane_id"] as? String, let revision = value["revision"] as? NSNumber else { return }
            panes[id]?.display.updateReferences(value["items"] as? [[String: Any]] ?? [], revision: revision.uint64Value,
                canReturn: value["can_return"] as? Bool ?? false)
        case "reading_hint":
            guard let id = value["pane_id"] as? String, let pane = panes[id] else { return }
            pane.display.updateReadingHint(value["label"] as? String ?? "Back to latest",
                visible: value["visible"] as? Bool ?? false, retry: value["retry"] as? Bool ?? false)
        case "scroll_to_row":
            guard let id = value["pane_id"] as? String, let row = value["row"] as? NSNumber else { return }
            panes[id]?.display.scrollToRow(row.uint64Value)
        case "scroll_latest":
            guard let id = value["pane_id"] as? String else { return }
            panes[id]?.display.scrollToLatest()
        case "idea_command", "idea_locate_reference":
            guard let id = value["pane_id"] as? String, let pane = panes[id],
                  pane.content.kind == .ideaTree, pane.content.viewID == value["view_id"] as? String,
                  let action = (value["action"] as? String) ?? (value["type"] as? String == "idea_locate_reference" ? "locate_reference" : nil) else { return }
            let reference = value["reference"].flatMap { try? JSONSerialization.data(withJSONObject:$0) }.flatMap { String(data:$0,encoding:.utf8) }
            pane.display.ideaCommand(action, query: reference ?? value["query"] as? String ?? "")
        case "idea_composer":
            guard let id = value["window_id"] as? String else { return }
            for (_, pane) in panes where pane.windowID == id {
                pane.display.view.layer?.borderWidth = 0
                pane.display.view.setAccessibilityLabel(pane.content.kind == .ideaTree ? "Idea Tree" : "Conversation")
            }
            composers[id]?.updateIdea(value)
            scheduleDraftSave()
        case "ensure_terminal_visible":
            guard let window = value["window_id"] as? String, let pane = value["pane_id"] as? String,
                  let token = value["token"] as? String, let target = (value["target"] as? [String:Any])?["client_view"] as? [String:Any] else { return }
            Task { @MainActor [weak self] in
                guard let self else { return }
                do {
                    guard let ensure = self.ensureTerminalVisible else { throw PolarTmuxLocalIPCError.connectionClosed }
                    try await ensure(window,pane,target)
                    self.input(["type":"terminal_view_result","window_id":window,"token":token,"ready":true])
                } catch {
                    self.input(["type":"terminal_view_result","window_id":window,"token":token,"ready":false,"error":error.localizedDescription])
                }
            }
        case "inquiry":
            guard let id = value["window_id"] as? String, let view = value["view"] as? [String:Any] else { return }
            composers[id]?.updateInquiry(view, focus: value["focus"] as? Bool == true)
        case "input_bar":
            guard let view = value["view"] as? [String:Any], let lease = view["lease"] as? [String:Any], let id = lease["window"] as? String else { return }
            composers[id]?.updateGeneric(view)
        case "composer":
            guard let id = value["window_id"] as? String else { return }
            markComposerTarget(id,selected:value["pane_id"] as? String)
            composers[id]?.update(value)
        case "focus_composer":
            guard let window = value["window_id"] as? String, let pane=value["pane_id"] as? String, composers[window]?.paneID == pane else { return }
            composers[window]?.activate()
        case "composer_closed":
            guard let id = value["window_id"] as? String else { return }; markComposerTarget(id,selected:nil); composers[id]?.clearSelection()
        case "status":
            if let id = value["pane_id"] as? String, let pane = panes[id] {
                if composers[pane.windowID]?.paneID == id { composers[pane.windowID]?.showStatus(value["message"] as? String ?? "") }
            }
        case "targets", "channels", "threads", "browse_error":
            browser?.consume(value)
            for panel in composers.values { panel.picker?.consume(value) }
        case "attachments":
            if let id = value["pane_id"] as? String, let pane = panes[id] {
                composers[pane.windowID]?.attachments = value["items"] as? [[String: Any]] ?? []
            }
        case "open_reference_thread":
            guard let window = value["window_id"] as? String,
                  let scope = value["scope"] as? [String: String],
                  let target = scope["target_id"], let node = scope["node_id"],
                  let owner = scope["owner"], let thread = scope["thread"], let actor = scope["actor"] else { return }
            let content = StationPaneContentV1(targetID: target, nodeID: node, owner: owner, thread: thread, actor: actor)
            Task { @MainActor [weak self] in
                do { try await self?.openPane?(window, content, nil) }
                catch { self?.showError(error.localizedDescription) }
            }
        case "copy_source":
            guard let id = value["pane_id"] as? String, panes[id] != nil,
                  let text = value["text"] as? String else { return }
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
        case "activity":
            if activityPanel == nil { activityPanel = ToolActivityPanel() }
            activityPanel?.onInput = { [weak self] input in self?.input(input) }
            let sourcePane = value["pane_id"] as? String
            activityPanel?.onClose = { [weak self] in
                guard let self, let sourcePane, let pane = self.panes[sourcePane],
                      let composer = self.composers[pane.windowID], composer.paneID == sourcePane else { return }
                composer.window?.makeKeyAndOrderFront(nil)
                composer.activate()
            }
            activityPanel?.show(value["items"] as? [String: Any] ?? [:])
        case "search_results":
            let rows = (value["items"] as? [String: Any])?["items"] as? [[String: Any]] ?? []
            let text = rows.map { ($0["text"] as? String ?? String(describing: $0)).prefix(500) }.joined(separator: "\n\n")
            let alert = NSAlert(); alert.messageText = "Search in loaded messages"
            alert.informativeText = text.isEmpty ? "No matching loaded messages" : String(text.prefix(8000))
            alert.runModal()
        case "drafts": if let drafts = value["value"] { saveDrafts(drafts) }
        case "error": showError(value["message"] as? String ?? "LGP operation failed")
        default: break
        }
    }
    private func showError(_ message: String) {
        browser?.showStatus(message)
        for panel in composers.values { panel.showStatus(message) }
        NSLog("Polar Station LGP: %@", message)
    }
    private var draftURL: URL? {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("PolarStation/lgp-drafts-v1.json")
    }
    private func scheduleDraftSave() {
        saveWork?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.input(["type": "export_drafts"]) }
        saveWork = work; DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(350), execute: work)
    }
    private func saveDrafts(_ value: Any) {
        let revision = (value as? [String:Any])?["snapshot_revision"] as? NSNumber
        var saved = false
        defer { if let revision { input(["type":"snapshot_stored","revision":revision,"success":saved]) } }
        guard let url = draftURL, let data = try? JSONSerialization.data(withJSONObject: value), data.count <= 1_048_576 else { return }
        do {
            let directory = url.deletingLastPathComponent()
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700])
            // Reject links: a persisted draft is private user data, not a path
            // supplied by a message or a remote Polar instance.
            if let attrs = try? FileManager.default.attributesOfItem(atPath: url.path), attrs[.type] as? FileAttributeType == .typeSymbolicLink {
                throw CocoaError(.fileWriteNoPermission)
            }
            try data.write(to: url, options: [.atomic])
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
            let handle = try FileHandle(forWritingTo:url)
            defer {try? handle.close()}
            try handle.synchronize()
            saved = true
        } catch { showError("Draft persistence failed: " + error.localizedDescription) }
    }
    private func restoreDrafts() {
        guard let url = draftURL,
              let attrs = try? FileManager.default.attributesOfItem(atPath: url.path),
              attrs[.type] as? FileAttributeType == .typeRegular,
              ((attrs[.size] as? NSNumber)?.intValue ?? Int.max) <= 1_048_576,
              let data = try? Data(contentsOf: url),
              let drafts = try? JSONSerialization.jsonObject(with: data) else { return }
        input(["type": "restore_drafts", "drafts": drafts])
    }
}

#endif
