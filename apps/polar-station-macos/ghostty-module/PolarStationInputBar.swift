#if POLAR_STATION
import AppKit
import Foundation
import GhosttyKit
import PolarTmuxHostClient
import CPolarStationDesktopClient

/// Native shared input shell and its content-specific controls. Business state stays in Rust.
@MainActor
final class StationInputBarView: NSView, NSTextViewDelegate, NSPopoverDelegate {
    let dictateControls: StationDictateControls
    var onIdeaInput: (([String:Any]) -> Void)?
    private var canOpenIdeaSession = false
    var onChooseIdea: (() -> Void)?
    private var hasIdeaBinding = false
    private var ideaReferences: [[String:Any]] = []
    private let ideaAttach = NSButton(title: "+", target: nil, action: nil)
    private let ideaReferenceMenu = NSPopUpButton(frame: .zero, pullsDown: true)
    var onIdeaAction: ((String, String) -> Void)?
    private(set) var refineProject:String?
    private(set) var conversationContext:String?
    var genericLease: [String:Any]?
    var dictateInputIdentity: Data? {
        if inquiryActive {
            return try? JSONSerialization.data(withJSONObject:["pane":paneID ?? "","inquiry":inquiryView["token"] as? String ?? ""],options:.sortedKeys)
        }
        guard let genericLease else { return nil }
        return try? JSONSerialization.data(withJSONObject: genericLease, options: [.sortedKeys])
    }
    var onExit: (() -> Void)?
    var onQuote: (() -> Void)?
    var onDraft: ((String) -> Void)?; var onSend: (() -> Void)?
    var onReferences: (() -> Void)?
    var onActivity: (() -> Void)?
    var onOlder: (() -> Void)?; var onRetry: (() -> Void)?; var onSearch: ((String) -> Void)?
    var onWindowClosed: (() -> Void)?
    var onChoose: (() -> Void)?; var onMode: ((ChatSendModeV1) -> Void)?
    var onInquiry: ((String,String?,String?) -> Void)?
    var onEnvironment: ((Bool,Bool) -> Void)?
    var onDictated: (([String:Any],[String:Any],String)->Void)?
    var onRefined: (([String:Any],String,String,Bool)->Void)?
    var onNativeIdeaCommand: ((String, String) -> Void)?
    private let inquiryButton = NSButton(title:"Inquiry",target:nil,action:nil)
    private let inquiryPopover = NSPopover()
    private var inquiryView: [String:Any] = [:]
    private var inquiryButtons: [InquiryButton] = []
    private var inquiryMonitor: Any?
    private var inquiryActive = false
    private var inquirySignature:Data?
    private var inquiryExtra:NSTextField?
    private let inquiryText = NSTextView()
    private var inquiryTaskPreview:NSTextView?
    private var inquiryPreviewHeight:NSLayoutConstraint?
    private var inquiryDetailScroll:NSScrollView?
    var hasInquiry: Bool { inquiryActive || (inquiryView["count"] as? Int ?? 0) > 0 }
    var paneID: String?
    private(set) var dictateRevision: UInt64 = 0
    var attachments: [[String: Any]] = []
    var picker: ConversationPicker?
    var currentScope: [String: Any] = [:]
    var channelID = ""
    let targetButton = NSButton(title: "Choose Channel / Thread…", target: nil, action: nil)
    private let draftEditor = StationDraftEditor()
    private var text: NSTextView { draftEditor.text }
    private let status = NSTextField(labelWithString: "")
    private let statusButton = NSButton(title: "●", target: nil, action: nil)
    private let statusPopover = NSPopover()
    private let send = NSButton(title: "Send", target: nil, action: nil)
    private let quote = NSButton(title: "❞", target: nil, action: nil)
    private let retry = NSButton(title: "Reconnect", target: nil, action: nil)
    private let older = NSMenuItem(title: "Load older messages", action: #selector(olderPressed), keyEquivalent: "")
    private let toggle = NSButton(title: "▾ Chat", target: nil, action: nil)
    private let mode = NSPopUpButton(frame: .zero, pullsDown: true)
    private var footerLayout: StationFooterLayout!
    private var editorRow: NSView { footerLayout.slot }
    private var footerDensityKey = ""
    private let ideaRow = NSStackView()
    private let ideaMenu = NSPopUpButton(frame: .zero, pullsDown: true)
    private let ideaSearchButton = NSButton(title: "⌕", target: nil, action: nil)
    private let ideaSearchPopover = NSPopover()
    private var ideaDocument: NSStackView { draftEditor.document }
    private var ideaGeneralHeight: NSLayoutConstraint? { get { draftEditor.generalHeight } set { draftEditor.generalHeight = newValue } }
    private var ideaBlockEditors: [String: IdeaNodeInstructionEditor] { get { draftEditor.blocks } set { draftEditor.blocks = newValue } }
    private var ideaViewID = ""
    private weak var lastIdeaEditor: NSTextView?
    private var ideaReaderOpen = false
    private var ideaSelectedNode: [String: String]?
    private var ideaDetailEditor: IdeaNodeInstructionEditor?
    private var ideaDetailBox: NSView?

    private var lastIdeaComposerHeight: CGFloat = -1
    private weak var ideaDetailCanvas: NSView?
    private var ideaEditorRect: [String: Double]?
    private var ideaInstructionStyle: [String: String]?
    private var ideaAnchorX: NSLayoutConstraint?
    private var ideaAnchorTop: NSLayoutConstraint?
    private var ideaAnchorHeight: NSLayoutConstraint?
    private var ideaAnchorWidth: NSLayoutConstraint?
    private var ideaSessionTitle = "Codex ▾"
    private var ideaDocumentText = ""
    private let ideaCopyDraft = NSMenuItem(title: "Copy draft", action: #selector(copyIdeaDraft), keyEquivalent: "")
    private var ideaGroups: [String: NSMenu] = [:]

    func acceptsIdeaShortcut(canvas: NSView) -> Bool {
        guard isIdea, let window, window.attachedSheet == nil else { return false }
        if let responder = window.firstResponder as? NSView {
            return responder === text || responder === canvas || responder.isDescendant(of: canvas) || responder.isDescendant(of: self)
                || (ideaDetailBox.map { responder.isDescendant(of: $0) } ?? false)
        }
        return false
    }
    private var editorScroll: NSScrollView { draftEditor.scroll }
    private let ideaStatus = NSMenuItem(title: "", action: nil, keyEquivalent: "")
    private var ideaCompact = false
    private weak var ideaPreviousResponder: NSResponder?
    private var ideaPopoverPane: String?
    private let ideaSearch = NSSearchField()
    private let ideaMore = NSPopUpButton(frame: .zero, pullsDown: true)
    private let ideaTheme = NSPopUpButton(frame: .zero, pullsDown: true)
    private let tools = NSPopUpButton(frame: .zero, pullsDown: true)
    private var ideaItems: [String: NSMenuItem] = [:]
    private var isIdea = false
    private var changing = false
    private var sendMode: ChatSendModeV1 = .enter
    private weak var container: TerminalViewContainer?
    private var focusWork: DispatchWorkItem?
    private var expanded = true
    private var closedObserver: NSObjectProtocol?
    private var activeTextEditor: NSTextView { inquiryActive ? inquiryText : text }
    var editorHasFocus: Bool { activeTextEditor.window?.firstResponder === activeTextEditor }
    required init?(coder: NSCoder) { return nil }
    init(dictate: PolarStationDictate) {
        dictateControls = StationDictateControls(dictate: dictate)
        super.init(frame: .zero)
        dictateControls.activateInput = { [weak self] in
            guard let self, let window = self.window, self.paneID != nil, window.isVisible,
                  !self.text.isHiddenOrHasHiddenAncestor else { return }
            self.activeTextEditor.window?.makeFirstResponder(self.activeTextEditor)
        }
        dictateControls.refineContext = { [weak self] in self?.paneID ?? "" }
        dictateControls.refine = { [weak self, weak dictate] in
            guard let self, let dictate, self.paneID != nil else { return }
            let editor = self.isIdea ? self.lastIdeaEditor ?? self.text : self.activeTextEditor
            guard editor.isEditable, !editor.hasMarkedText(), !editor.string.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
            let pane = self.paneID, revision = self.dictateRevision, identity = self.dictateInputIdentity
            editor.window?.makeFirstResponder(editor)
            editor.setSelectedRange(NSRange(location: 0, length: (editor.string as NSString).length))
            let lease=self.genericLease,original=editor.string,english=dictate.refineEnglish(contextID:self.paneID ?? "")
            guard let target = StationDictateTarget.editor(editor, applied:{[weak self] result in
                if let lease {self?.onRefined?(lease,original,result,english)}
            }, valid: { [weak self] in
                self?.paneID == pane && self?.dictateRevision == revision && self?.dictateInputIdentity == identity
            }) else { return }
            dictate.refine(target: target, text: editor.string, contextID:self.paneID ?? "",project:self.refineProject)
        }
        text.delegate = self
        inquiryText.isRichText = false; inquiryText.allowsUndo = true; inquiryText.delegate = self
        inquiryText.font = .systemFont(ofSize:14); inquiryText.textContainer?.widthTracksTextView = true
        inquiryText.autoresizingMask = [.width]
        let scroll = editorScroll
        scroll.translatesAutoresizingMaskIntoConstraints = false
        send.target = self; send.action = #selector(sendPressed); send.isEnabled = false
        toggle.target = self; toggle.action = #selector(toggleExpanded)
        targetButton.target = self; targetButton.action = #selector(choosePressed)
        targetButton.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        targetButton.lineBreakMode = .byTruncatingMiddle
        quote.target = self; quote.action = #selector(quotePressed); quote.isHidden = true
        quote.toolTip = "Quote selection · ⌘⌥G"
        quote.setAccessibilityLabel("Quote selection")
        retry.target = self; retry.action = #selector(retryPressed); retry.isHidden = true
        retry.toolTip = "Reconnect this conversation; messages are not resent"
        status.lineBreakMode = .byTruncatingTail
        tools.setAccessibilityLabel("More actions")
        tools.addItem(withTitle: "⋯"); older.target = self; older.isEnabled = false
        tools.menu?.autoenablesItems = false; tools.menu?.addItem(older)
        let references = NSMenuItem(title: "Message references…", action: #selector(referencesPressed), keyEquivalent: "")
        references.target = self; tools.menu?.addItem(references)
        let activity = NSMenuItem(title: "Tool activity…", action: #selector(activityPressed), keyEquivalent: "")
        activity.target = self; tools.menu?.addItem(activity)
        let search = NSMenuItem(title: "Search loaded messages…", action: #selector(searchPressed), keyEquivalent: "")
        search.target = self; tools.menu?.addItem(search)
        statusButton.target = self; statusButton.action = #selector(connectionPressed)
        let details = NSStackView(views: [status]); details.orientation = .vertical
        details.edgeInsets = NSEdgeInsets(top: 10, left: 10, bottom: 10, right: 10)
        let controller = NSViewController(); controller.view = details
        statusPopover.contentViewController = controller; statusPopover.behavior = .transient
        mode.addItems(withTitles: ["▾", "Enter sends", "Cmd+Enter sends"]); mode.target = self; mode.action = #selector(modePressed)
        inquiryButton.target = self; inquiryButton.action = #selector(inquiryEntryPressed); inquiryButton.isHidden = true
        ideaAttach.target = self; ideaAttach.action = #selector(attachIdeaReference)
        ideaAttach.title = "Insert node"
        ideaAttach.toolTip = "Insert selected node title and UUID · Cmd+Option+I"
        ideaAttach.setAccessibilityLabel("Insert node")
        ideaReferenceMenu.addItem(withTitle:"References")
        ideaAttach.isHidden = true; ideaReferenceMenu.isHidden = true
        footerLayout = StationFooterLayout(leading: [ideaMenu, targetButton], trailing: [ideaAttach, quote, tools, dictateControls], primary: send, viewport: editorScroll, target: targetButton)
        footerLayout.onDensityChanged = { [weak self] compact in self?.renderDensity(compact) }
        addSubview(footerLayout)
        footerLayout.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([footerLayout.leadingAnchor.constraint(equalTo: leadingAnchor), footerLayout.trailingAnchor.constraint(equalTo: trailingAnchor), footerLayout.topAnchor.constraint(equalTo: topAnchor), footerLayout.bottomAnchor.constraint(equalTo: bottomAnchor)])
        ideaRow.orientation = .horizontal; ideaRow.spacing = 6; ideaRow.isHidden = true
        ideaSearch.placeholderString = "Search nodes…"
        ideaSearch.target = self; ideaSearch.action = #selector(ideaSearchChanged)
        ideaSearch.sendsSearchStringImmediately = true
        ideaSearch.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        ideaMore.addItem(withTitle: "View"); ideaMore.toolTip = "Idea view and motion preferences"
        ideaMore.menu?.autoenablesItems = false
        ideaTheme.addItem(withTitle: "Theme"); ideaTheme.menu?.autoenablesItems = false
        ideaMenu.addItem(withTitle: "Idea"); ideaMenu.menu?.autoenablesItems = false
        ideaSearchButton.target = self; ideaSearchButton.action = #selector(openIdeaSearch)
        ideaSearchButton.toolTip = "Search nodes"; ideaSearchButton.setAccessibilityLabel("Search nodes")
        for button in [ideaSearchButton] { button.bezelStyle = .rounded }
        for view in [ideaMenu, ideaSearchButton, ideaMore, ideaTheme] { view.isHidden = true }
        let searchController = NSViewController()
        let searchBox = NSStackView(views: [ideaSearch]); searchBox.edgeInsets = NSEdgeInsets(top: 10, left: 10, bottom: 10, right: 10)
        searchController.view = searchBox
        ideaSearch.widthAnchor.constraint(equalToConstant: 280).isActive = true
        ideaSearchPopover.contentViewController = searchController
        ideaSearchPopover.behavior = .transient; ideaSearchPopover.delegate = self
        for (label, action) in [("Connection status…", #selector(connectionPressed)), ("Refresh / reconnect", #selector(retryPressed)), ("Inquiries…", #selector(inquiryEntryPressed))] {
            let item = NSMenuItem(title: label, action: action, keyEquivalent: "")
            item.target = self; tools.menu?.addItem(item)
        }
        let shortcut = NSMenuItem(title: "Send shortcut", action: nil, keyEquivalent: "")
        let shortcuts = NSMenu(); shortcut.submenu = shortcuts
        for (title, value) in [("Enter sends", "enter"), ("Cmd+Enter sends", "modified_enter")] {
            let item = NSMenuItem(title: title, action: #selector(footerShortcutPressed(_:)), keyEquivalent: "")
            item.target = self; item.representedObject = value; shortcuts.addItem(item)
        }
        tools.menu?.addItem(shortcut)
    }
    func attach(to window: NSWindow) {
        guard let target = (window.windowController as? TerminalController)?.terminalViewContainer else { return }
        if container !== target {
            container?.hideStationComposer(); container = target; lastIdeaComposerHeight = -1
            if let closedObserver { NotificationCenter.default.removeObserver(closedObserver) }
            closedObserver = NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: window, queue: .main) { [weak self, weak window] _ in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    // Closing one native tab must preserve its group's editor.
                    if let remaining = window?.tabbedWindows?.first(where: { $0 !== window }) {
                        self.attach(to: remaining)
                    } else {
                        self.commitDraft(); self.close(); self.onWindowClosed?()
                    }
                }
            }
        }
        resizeEditor()
    }
    func close() {
        focusWork?.cancel(); focusWork=nil
        closeIdeaInstruction()
        closeIdeaPopovers()
        hideInquiry()
        picker?.close(); statusPopover.close(); container?.hideStationComposer(); container = nil
        if let closedObserver { NotificationCenter.default.removeObserver(closedObserver) }; closedObserver = nil
    }
    func clearSelection() {
        genericLease = nil; refineProject = nil; conversationContext = nil; targetButton.isEnabled = true; quote.isHidden = true; retry.title = "Reconnect"
        focusWork?.cancel(); focusWork=nil
        hideInquiry()
        dictateRevision = 0
        setIdeaMode(false)
        paneID = nil; currentScope = [:]; channelID = ""
        changing = true; text.string = ""; changing = false
        targetButton.title = "Choose Channel / Thread…"
        send.isEnabled = false; older.isEnabled = false; retry.isHidden = true
        showStatus("Choose a conversation"); resizeEditor()
    }
    func activate() {
        expanded = true; editorRow.isHidden = false; send.isHidden = inquiryActive; mode.isHidden = isIdea || inquiryActive; toggle.title = isIdea ? "▾ Idea" : "▾ Chat"
        resizeEditor()
        let selected = paneID
        focusWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self, self.expanded, self.paneID == selected, self.window?.isKeyWindow == true else { return }
            if let responder=self.window?.firstResponder as? NSView, responder !== self.text && !(responder is Ghostty.SurfaceView) { return }
            self.window?.makeFirstResponder(self.text)
        }
        focusWork=work; DispatchQueue.main.async(execute:work)
    }
    func publishInquiryEnvironment() {
        let foreground = window?.isKeyWindow == true || inquiryPopover.contentViewController?.view.window?.isKeyWindow == true
        onEnvironment?(foreground, activeTextEditor.hasMarkedText())
    }
    func setInquiryText(_ value:String) {
        guard !inquiryText.hasMarkedText(), inquiryText.string != value else {return}
        changing = true; inquiryText.string = value; changing = false
    }
    private func closeInquiryPanel() {
        let returning = inquiryPopover.contentViewController?.view.window?.isKeyWindow == true

        inquiryPopover.close()
        if returning, let window, window.isVisible {
            window.makeKey(); window.makeFirstResponder(text)
        }
    }
    private func hideInquiry() {
        closeInquiryPanel(); inquiryButton.isHidden = true; inquiryActive = false; text.isEditable = true
        if let monitor = inquiryMonitor { NSEvent.removeMonitor(monitor) }; inquiryMonitor = nil
    }
    @objc private func inquiryEntryPressed() {
        commitDraft(); publishInquiryEnvironment(); onInquiry?("inbox", nil, nil)
    }
    private func inquiryLater() { onInquiry?("later", inquiryView["token"] as? String, nil) }
    @objc private func inquiryControlPressed(_ sender: InquiryButton) {
        if let event=NSApp.currentEvent, event.type == .keyDown, event.isARepeat { return }
        onInquiry?(sender.inquiryAction, sender.inquiryToken, sender.inquiryValue)
    }
    func updateInquiry(_ value: [String:Any], focus: Bool) {
        inquiryView = value; inquiryActive = value["active"] as? Bool == true
        send.isHidden=inquiryActive;mode.isHidden=inquiryActive
        let count = value["count"] as? Int ?? 0
        inquiryButton.isHidden = !inquiryActive && count == 0 && value["has_history"] as? Bool != true
        inquiryButton.title = value["entry_title"] as? String ?? "Inquiry"
        text.isEditable = !inquiryActive
        inquiryText.isEditable = inquiryActive && value["editable"] as? Bool == true
        // Inquiry actions live in its native panel; the hidden footer action
        // retains the normal composer caption for restoration.
        if !inquiryActive { setSendMode(sendMode) }
        renderFooterControls()
        let stage = value["stage"] as? String ?? "badge"
        guard stage != "badge", window?.isKeyWindow == true || inquiryPopover.isShown else { closeInquiryPanel(); return }
        let token = value["token"] as? String ?? ""
        var body:[String:Any]=[:]
        for key in ["stage","token","title","question","choices","status","editable","invitations","history"] { if let v=value[key] { body[key]=v } }
        let signature=try? JSONSerialization.data(withJSONObject:body,options:.sortedKeys)
        if inquirySignature == signature && !focus && inquiryPopover.isShown {
            inquiryExtra?.stringValue=value["others_label"] as? String ?? ""; inquiryExtra?.isHidden=count<=1
            updateInquiryPreview(value["task_preview"] as? String ?? "")
            return
        }

        inquirySignature=signature
        // An inactive popover retains its last responder. Rebuilding the task
        // preview while typing must not promote that stale candidate focus.
        let previousIndex = inquiryPopover.contentViewController?.view.window?.isKeyWindow == true
            ? inquiryButtons.firstIndex(where: { $0.window?.firstResponder === $0 }) : nil
        inquiryButtons = []
        inquiryTaskPreview = nil; inquiryPreviewHeight = nil
        let panelWidth = min(760,max(240,(window?.contentView?.bounds.width ?? 800)*0.618))
        let popupWidth = panelWidth
        let listing = stage == "list"
        let prompt = stage == "prompt"
        let routing = value["routing"] as? Bool == true
        func stack()->NSStackView {
            let v=NSStackView();v.orientation = .vertical;v.alignment = .leading;v.spacing=8
            v.edgeInsets=NSEdgeInsets(top:8,left:12,bottom:8,right:12)
            v.widthAnchor.constraint(equalToConstant:panelWidth).isActive=true
            return v
        }
        func label(_ text:String,dim:Bool=false)->NSTextField {
            let v=NSTextField(wrappingLabelWithString:text);v.font = .systemFont(ofSize:12)
            v.preferredMaxLayoutWidth=panelWidth-24;v.widthAnchor.constraint(lessThanOrEqualToConstant:panelWidth-24).isActive=true
            v.isHidden=text.isEmpty;if dim {v.textColor = .secondaryLabelColor};return v
        }
        func button(_ title:String,_ action:String,_ selectedValue:String? = nil)->InquiryButton {
            let b = InquiryButton(title:title,target:self,action:#selector(inquiryControlPressed(_:)))
            b.inquiryAction=action;b.inquiryToken=token;b.inquiryValue=selectedValue;b.setAccessibilityLabel(title)
            inquiryButtons.append(b);return b
        }
        let header=stack(), root=stack(), footer=stack()
        let navigation=NSStackView();navigation.spacing=6
        if listing {
            navigation.addArrangedSubview(button("Pending","inbox"));navigation.addArrangedSubview(button("History","history"))
        } else {navigation.addArrangedSubview(button(value["inbox_label"] as? String ?? "All Inquiries","inbox"))}
        header.addArrangedSubview(navigation)
        let title=label(value["title"] as? String ?? "Inquiry");title.font = .boldSystemFont(ofSize:14);header.addArrangedSubview(title)
        header.addArrangedSubview(label(value["details"] as? String ?? "",dim:true))
        if !listing {
            if routing {root.addArrangedSubview(label(value["task_label"] as? String ?? "",dim:true))}
            if !routing && !prompt {root.addArrangedSubview(label(value["question"] as? String ?? ""))}
            if value["editable"] as? Bool == true {
                let answerScroll = NSScrollView(); answerScroll.hasVerticalScroller = true
                inquiryText.removeFromSuperview(); answerScroll.documentView = inquiryText
                root.addArrangedSubview(answerScroll)
                answerScroll.widthAnchor.constraint(equalToConstant:panelWidth-24).isActive = true
                answerScroll.heightAnchor.constraint(equalToConstant:110).isActive = true
            } else {
                let task=label(value["task_preview"] as? String ?? "");task.isSelectable=true;root.addArrangedSubview(task)
            }
        }
        if !prompt {
            if routing {root.addArrangedSubview(label(value["question"] as? String ?? "",dim:true))}
            for choice in value["choices"] as? [[String:Any]] ?? [] {
                let number=choice["number"] as? Int ?? 0
                let prefix = routing || listing ? "" : (choice["selected"] as? Bool == true ? "● " : "○ ")
                let title="\(prefix)\(number). \(choice["label"] as? String ?? "")"
                let b=button(title,"choose",choice["value"] as? String);b.isEnabled=choice["enabled"] as? Bool == true
                b.widthAnchor.constraint(lessThanOrEqualToConstant:popupWidth-24).isActive=true
                b.cell?.lineBreakMode = .byTruncatingMiddle
                let row=NSStackView();row.orientation = .vertical;row.alignment = .leading;row.spacing=3
                if routing {b.setButtonType(.radio);b.state=choice["selected"] as? Bool == true ? .on : .off}
                else if listing {b.bezelStyle = .inline}
                row.addArrangedSubview(b)
                if routing {
                    row.addArrangedSubview(label(choice["subtitle"] as? String ?? "",dim:true))
                    row.addArrangedSubview(label(choice["reason_text"] as? String ?? ""))
                    if choice["selected"] as? Bool == true {
                        let directory=label(choice["directory"] as? String ?? "",dim:true);directory.isSelectable=true;row.addArrangedSubview(directory)
                        row.addArrangedSubview(label(choice["context"] as? String ?? ""))
                    }
                } else if listing {
                    row.addArrangedSubview(label(choice["context"] as? String ?? ""))
                    row.addArrangedSubview(label(choice["detail"] as? String ?? "",dim:true))
                }
                root.addArrangedSubview(row)
            }
            if routing {
                let alternatives=NSStackView();alternatives.spacing=6
                let other=button(value["recommend_label"] as? String ?? "Other Sessions…","recommend");other.isEnabled=value["can_recommend"] as? Bool == true;alternatives.addArrangedSubview(other)
                if value["can_retry_view"] as? Bool == true {alternatives.addArrangedSubview(button(value["retry_view_label"] as? String ?? "Retry opening","retry_view"))}
                root.addArrangedSubview(alternatives)
                root.addArrangedSubview(label(value["delivery_hint"] as? String ?? "",dim:true))
            }
        }
        let separator=NSBox();separator.boxType = .separator;separator.widthAnchor.constraint(equalToConstant:panelWidth-24).isActive=true;footer.addArrangedSubview(separator)
        if routing && !listing {footer.addArrangedSubview(label(value["destination"] as? String ?? ""))}
        footer.addArrangedSubview(label(value["status"] as? String ?? ""))
        let actions=NSStackView();actions.spacing=6
        if listing {
            if value["history"] as? Bool != true {let dismiss=button("Dismiss pending","dismiss_all");dismiss.isEnabled=count>0;actions.addArrangedSubview(dismiss)}
            actions.addArrangedSubview(button("Load older","older"))
        } else if value["can_restore"] as? Bool == true {actions.addArrangedSubview(button("Restore","restore"))}
        else if value["history"] as? Bool != true {actions.addArrangedSubview(button("Dismiss","dismiss"))}
        actions.addArrangedSubview(button(value["later_label"] as? String ?? "Later","later"))
        let spacer=NSView();spacer.setContentHuggingPriority(.defaultLow,for:.horizontal);actions.addArrangedSubview(spacer)
        if !listing && value["history"] as? Bool != true {
            let primary=button(value[prompt ? "open_label" : "primary_label"] as? String ?? "Open",prompt ? "open" : "submit")
            primary.isEnabled=prompt || value["can_next"] as? Bool == true;actions.addArrangedSubview(primary)
        }
        actions.widthAnchor.constraint(equalToConstant:panelWidth-24).isActive=true;footer.addArrangedSubview(actions)
        header.layoutSubtreeIfNeeded();footer.layoutSubtreeIfNeeded();root.layoutSubtreeIfNeeded()
        let headerHeight=header.fittingSize.height, footerHeight=footer.fittingSize.height
        let bodyHeight=min(max(100,(window?.contentView?.bounds.height ?? 900)-headerHeight-footerHeight-100),max(80,root.fittingSize.height))
        let panelHeight=headerHeight+bodyHeight+footerHeight
        let content=NSView(frame:NSRect(x:0,y:0,width:panelWidth,height:panelHeight))
        for (view,y,height) in [(header,footerHeight+bodyHeight,headerHeight),(footer,CGFloat(0),footerHeight)] {
            view.translatesAutoresizingMaskIntoConstraints=true;view.frame=NSRect(x:0,y:y,width:panelWidth,height:height);content.addSubview(view)
        }
        let scroll=NSScrollView(frame:NSRect(x:0,y:footerHeight,width:panelWidth,height:bodyHeight))
        scroll.drawsBackground=false;scroll.hasVerticalScroller=true
        scroll.documentView=InquiryDocumentView(stack:root,width:panelWidth,minimumHeight:bodyHeight)
        content.addSubview(scroll);inquiryDetailScroll=scroll;inquiryExtra=nil
        let controller=NSViewController();controller.view=content;inquiryPopover.contentViewController=controller
        // Editing in the positioning window must keep this interaction open.
        // Rust's Later/Escape and pane/window lifecycle own its dismissal.
        inquiryPopover.delegate = self
        inquiryPopover.behavior = .applicationDefined
        inquiryPopover.contentSize=content.frame.size
        if !inquiryPopover.isShown && !activeTextEditor.hasMarkedText() { inquiryPopover.show(relativeTo:tools.bounds,of:tools,preferredEdge:.maxY) }
        if focus || previousIndex != nil {
            DispatchQueue.main.async { [weak self] in
                guard let self, self.inquiryView["token"] as? String == token, !self.activeTextEditor.hasMarkedText(), self.inquiryPopover.isShown else { return }
                guard self.window?.isKeyWindow == true || self.inquiryPopover.contentViewController?.view.window?.isKeyWindow == true else { return }
                if focus && self.inquiryView["editable"] as? Bool == true { self.activeTextEditor.window?.makeKey(); self.activeTextEditor.window?.makeFirstResponder(self.activeTextEditor) }
                else { let i=focus ? (self.inquiryButtons.firstIndex(where:{$0.inquiryAction=="open" || $0.inquiryAction=="choose" || $0.inquiryAction=="inspect"}) ?? 0) : (previousIndex ?? 0); if self.inquiryButtons.indices.contains(i) { let b=self.inquiryButtons[i]; b.window?.makeKey(); b.window?.makeFirstResponder(b); b.scrollToVisible(b.bounds) } }
            }
        }
        if inquiryMonitor == nil {
            inquiryMonitor=NSEvent.addLocalMonitorForEvents(matching:.keyDown) { [weak self] event in
                guard let self,self.inquiryPopover.isShown,let popup=self.inquiryPopover.contentViewController?.view.window,
                      event.window === popup || (event.window === self.window && popup.isKeyWindow) else { return event }
                if self.editorHasFocus { return event }
                if event.keyCode == 53 { self.inquiryLater(); return nil }
                if event.modifierFlags.contains(.command) && event.keyCode == 36 { if self.inquiryActive && !event.isARepeat { self.sendPressed() };return nil }
                if !event.modifierFlags.intersection([.command,.option,.control]).isEmpty { return event }
                let enabled=self.inquiryButtons.filter(\.isEnabled)
                guard !enabled.isEmpty else { return event }
                let selected=enabled.firstIndex(where:{$0.window?.firstResponder === $0}) ?? 0
                if event.keyCode == 125 || event.keyCode == 126 {
                    let next=(selected+(event.keyCode == 125 ? 1 : enabled.count-1)) % enabled.count
                    enabled[next].window?.makeFirstResponder(enabled[next]);enabled[next].scrollToVisible(enabled[next].bounds);return nil
                }
                if event.keyCode == 36 { if !event.isARepeat { enabled[selected].performClick(nil) };return nil }
                if let c=event.charactersIgnoringModifiers,let digit=Int(c),digit>0 {
                    let choices=enabled.filter{$0.inquiryAction == (self.inquiryView["stage"] as? String == "list" && self.inquiryButtons.contains(where:{$0.inquiryAction=="inspect"}) ? "inspect" : "choose")}
                    if choices.indices.contains(digit-1) { if !event.isARepeat { choices[digit-1].performClick(nil) };return nil }
                }
                return event
            }
        }
    }
    private func updateInquiryPreview(_ preview:String) {
        guard let task=inquiryTaskPreview, task.string != preview else { return }
        // Draft typing changes only preview content. Replacing the popover's
        // controller here would make its read-only NSTextView the responder.
        let wasHidden=task.enclosingScrollView?.isHidden == true
        task.string=preview; task.enclosingScrollView?.isHidden=preview.isEmpty
        let height=min(120,max(38,CGFloat(preview.components(separatedBy:"\n").count)*18))
        guard inquiryPreviewHeight?.constant != height || wasHidden != preview.isEmpty else { return }
        inquiryPreviewHeight?.constant=height
        guard let content=inquiryDetailScroll,
              let document=content.documentView as? InquiryDocumentView else { return }
        document.measure(minimumHeight:content.contentSize.height)
    }
    func commitDraft() {
        guard paneID != nil else { return }
        let editor = inquiryActive ? inquiryText : text
        editor.unmarkText()
        if !editor.hasMarkedText() { onDraft?(editor.string) }
    }
    @objc private func toggleExpanded() {
        expanded.toggle(); toggle.title = isIdea ? (expanded ? "▾ Idea" : "▸ Idea") : (expanded ? "▾ Chat" : "▸ Chat")
        resizeEditor(); if expanded { window?.makeFirstResponder(text) }
    }
    func updateGeneric(_ view: [String:Any]) {
        hideInquiry()
        guard let lease = view["lease"] as? [String:Any], let pane = lease["pane"] as? String else { return }
        update(["pane_id":pane,"text":view["text"] as? String ?? "", "draft_revision":lease["draft_revision"] ?? 0,
                "status":view["unavailable"] as? String ?? "Ready", "can_send":view["can_send"] as? Bool ?? false])
        genericLease = lease
        refineProject = view["project"] as? String
        conversationContext = view["conversation_context"] as? String
        let sendLabel=view["send_label"] as? String ?? "Send"
        send.title = sendLabel; setSendMode(sendMode)
        tools.isHidden = false
        quote.isHidden = false
        retry.isHidden = view["can_refresh"] as? Bool != true
        retry.title = "↻"
        retry.toolTip = "Refresh sending status"
        retry.setAccessibilityLabel("Refresh sending status")
        targetButton.title = view["target_status"] as? String ?? view["target_label"] as? String ?? "Terminal input"
        targetButton.toolTip = view["unavailable"] as? String ?? targetButton.title
        targetButton.isEnabled = false
        toggle.isHidden = true
        expanded = true
        editorRow.isHidden = false
        text.setAccessibilityLabel("Pane input")
        renderFooterControls(); resizeEditor()
    }
    func update(_ effect: [String: Any]) {
        genericLease = nil; refineProject = nil; conversationContext = nil; targetButton.isEnabled = true; quote.isHidden = true; retry.title = "Reconnect"
        dictateRevision = (effect["draft_revision"] as? NSNumber)?.uint64Value ?? 0
        setIdeaMode(false)
        send.title = "Send"
        text.toolTip = sendMode == .enter ? "Enter sends; Shift+Enter inserts newline" : "Cmd+Enter sends; Enter inserts newline"
        if paneID != effect["pane_id"] as? String { focusWork?.cancel(); focusWork=nil }
        paneID = effect["pane_id"] as? String
        currentScope = effect["scope"] as? [String: Any] ?? [:]; channelID = effect["channel_id"] as? String ?? ""
        let channelTitle = effect["channel_title"] as? String ?? ""
        targetButton.title = "\(effect["target_title"] as? String ?? "Polar") › \(channelTitle.isEmpty ? "Channel" : channelTitle) › \(effect["thread_title"] as? String ?? "Thread") ▾"
        targetButton.toolTip = targetButton.title
        text.setAccessibilityLabel("Message \(effect["thread_title"] as? String ?? "Thread") · Chat target")
        if inquiryActive, let incoming = effect["text"] as? String { setInquiryText(incoming) }
        if !inquiryActive, let incoming = effect["text"] as? String, incoming != text.string, !text.hasMarkedText() {
            changing = true; text.string = incoming; changing = false
        }
        send.isEnabled = effect["can_send"] as? Bool == true
        older.isEnabled = effect["can_load_older"] as? Bool == true
        retry.isHidden = effect["can_reconnect"] as? Bool != true
        let message = effect["status"] as? String ?? ""
        let connection = effect["connection"] as? String ?? "offline"
        showStatus(message.isEmpty ? (connection == "live" ? "Connected" : connection) : message)
        renderFooterControls(); resizeEditor()
    }
    private func setIdeaMode(_ value: Bool) {
        if isIdea != value { lastIdeaEditor = nil; closeIdeaPopovers(); leaveIdeaInstruction() }
        draftEditor.setIdeaDocument(value)
        isIdea = value; ideaRow.isHidden = !value; targetButton.isHidden = false
        ideaAttach.isHidden = !value; ideaReferenceMenu.isHidden = true
        toggle.isHidden = true; statusButton.isHidden = true
        ideaMenu.isHidden = !value
        for view in [ideaSearchButton, ideaMore, ideaTheme] { view.isHidden = true }
        if value { ideaRow.isHidden = true; statusButton.isHidden = true; send.title = "Send" }
        tools.isHidden = false; mode.isHidden = value || inquiryActive; send.isHidden = inquiryActive

        toggle.title = value ? (expanded ? "▾ Idea" : "▸ Idea") : (expanded ? "▾ Chat" : "▸ Chat")
    }
    func updateIdea(_ effect: [String: Any]) {
        genericLease = nil; refineProject = nil; conversationContext = nil; targetButton.isEnabled = true; quote.isHidden = true; retry.title = "Reconnect"
        dictateRevision = (effect["draft_revision"] as? NSNumber)?.uint64Value ?? 0
        hideInquiry()
        let changed = paneID != effect["pane_id"] as? String
        if changed {
            lastIdeaEditor = nil
            closeIdeaPopovers(); leaveIdeaInstruction()
            for editor in ideaBlockEditors.values { ideaDocument.removeArrangedSubview(editor); editor.removeFromSuperview() }
            ideaBlockEditors.removeAll()
        }
        setIdeaMode(true); paneID = effect["pane_id"] as? String
        ideaViewID = effect["view_id"] as? String ?? ""
        ideaDocumentText = effect["document_text"] as? String ?? ""
        ideaCopyDraft.isEnabled = !ideaDocumentText.isEmpty
        currentScope = [:]; channelID = ""; retry.isHidden = true; send.isEnabled = effect["can_send"] as? Bool == true
        canOpenIdeaSession = effect["can_open_session"] as? Bool == true
        hasIdeaBinding = effect["binding_id"] as? String != nil
        ideaReferences = effect["references"] as? [[String:Any]] ?? []
        ideaSessionTitle = (effect["target_label"] as? String ?? "Bind Codex…") + " ▾"
        targetButton.title = ideaSessionTitle
        targetButton.toolTip = ideaSessionTitle + " · " + (effect["status"] as? String ?? "Codex session for this Idea Pane")
        rebuildIdeaReferences()
        ideaSelectedNode = effect["selected_node"] as? [String: String]
        ideaAttach.isEnabled = !(ideaSelectedNode?["node_uuid"] ?? "").isEmpty
        updateIdeaDocument(effect["node_blocks"] as? [[String: String]] ?? [])
        if changed { ideaSearch.stringValue = ""; picker?.close(); statusPopover.close() }
        if inquiryActive, let incoming = effect["text"] as? String { setInquiryText(incoming) }
        if !inquiryActive, let incoming = effect["text"] as? String, incoming != text.string, !text.hasMarkedText() {
            changing = true; text.string = incoming; changing = false
        }
        text.setAccessibilityLabel(effect["placeholder"] as? String ?? "Idea Tree instruction")
        text.toolTip = effect["placeholder"] as? String
        let controls = effect["controls"] as? [[String: Any]] ?? []
        let keys = Set(controls.compactMap { control -> String? in
            guard let action = control["action"] as? String else { return nil }
            let query = control["query"] as? String ?? ""
            return query.isEmpty ? action : action + "\u{1f}" + query
        })
        for key in Array(ideaItems.keys) where !keys.contains(key) {
            if let item = ideaItems.removeValue(forKey: key) { item.menu?.removeItem(item) }
        }
        for control in controls {
            guard let action = control["action"] as? String, let label = control["label"] as? String else { continue }
            let query = control["query"] as? String ?? ""
            let key = query.isEmpty ? action : action + "\u{1f}" + query
            let item = ideaItems[key] ?? NSMenuItem(title: label, action: #selector(ideaMenuPressed(_:)), keyEquivalent: "")
            if ideaItems[key] == nil {
                item.target = self; item.representedObject = ["action": action, "query": query]
                ideaActionMenu(action, group: control["group"] as? String ?? "").addItem(item)
                ideaItems[key] = item
            }
            item.title = action == "back" ? "Back" : action == "forward" ? "Forward" : label
            let tooltip = control["tooltip"] as? String ?? ""
            item.toolTip = tooltip.isEmpty ? nil : tooltip
            item.isEnabled = control["enabled"] as? Bool == true
            item.state = control["active"] as? Bool == true ? .on : .off
        }

        if ideaStatus.menu == nil {
            ideaMenu.menu?.addItem(.separator())
            ideaCopyDraft.target = self; tools.menu?.addItem(ideaCopyDraft)
            let search = NSMenuItem(title: "Search nodes…", action: #selector(openIdeaSearch), keyEquivalent: "")
            search.target = self; ideaMenu.menu?.addItem(search)
            let edit = NSMenuItem(title: "Edit instruction…", action: #selector(editSelectedInstruction), keyEquivalent: "")
            edit.target = self; tools.menu?.addItem(edit)
            let check = NSMenuItem(title: "Refresh sending status", action: #selector(checkIdeaReceipt), keyEquivalent: "")
            check.target = self; tools.menu?.addItem(check)
            let session = NSMenuItem(title: "Show original session", action: #selector(showIdeaSession), keyEquivalent: "")
            session.target = self; tools.menu?.addItem(session)
            let unbind = NSMenuItem(title: "Unbind session", action: #selector(bindIdeaSession(_:)), keyEquivalent: "")
            unbind.target = self; tools.menu?.addItem(unbind)
            tools.menu?.addItem(ideaStatus)
        }
        tools.menu?.items.first(where: { $0.action == #selector(checkIdeaReceipt) })?.isHidden = effect["can_check_receipt"] as? Bool != true
        ideaStatus.title = effect["status"] as? String ?? "Local preview"; ideaStatus.isEnabled = false
        ideaSearch.isEnabled = effect["can_search"] as? Bool == true
        ideaSearchButton.isEnabled = ideaSearch.isEnabled
        renderFooterControls()
        showStatus(effect["status"] as? String ?? "")
        statusButton.title = "●"
        statusButton.setAccessibilityLabel("Codex instruction status")
        resizeEditor()
    }
    override func hitTest(_ point: NSPoint) -> NSView? {
        let hit = super.hitTest(point)
        return hit === self ? nil : hit
    }
    override func layout() { super.layout(); resizeEditor() }
    private func ideaActionMenu(_ action: String, group explicitGroup: String = "") -> NSMenu {
        if action == "open_files", let menu = ideaMenu.menu { return menu }
        let group: String
        if !explicitGroup.isEmpty { group = explicitGroup }
        else if action == "open_files" || action == "open_source" { group = "Files" }
        else if (action.hasPrefix("theme_") || action == "select_theme") { group = "Theme" }
        else if ["back","forward","locate","read","fit","focus_this_node"].contains(action) { group = "Navigate" }
        else if ["navigation_animation","frame_endpoints","path_feedback","camera_inertia","node_elasticity","reduce_motion","connection_flow"].contains(action) { group = "Motion" }
        else { group = "View" }
        if let menu = ideaGroups[group] { return menu }
        let menu = NSMenu(title: group); menu.autoenablesItems = false
        let item = NSMenuItem(title: group, action: nil, keyEquivalent: ""); item.submenu = menu
        ideaMenu.menu?.addItem(item); ideaGroups[group] = menu
        return menu
    }
    private func renderDensity(_ compact:Bool) {
        ideaCompact = compact
        ideaMenu.title = compact ? "☰" : "Idea"
        ideaAttach.title = compact ? "+" : "Insert node"
        if isIdea { targetButton.title = compact ? "Codex ▾" : ideaSessionTitle }
    }
    private func renderFooterControls() {
        let key = isIdea ? "idea" : genericLease == nil ? "chat" : "generic"
        if footerDensityKey != key {
            footerDensityKey = key
            renderDensity(false)
            footerLayout.configureTargetSlot(normalWidth:isIdea ? 140 : 200)
        }
        renderDensity(footerLayout.compact)
        // Overflow items may change state, never row geometry.
        for item in tools.menu?.items ?? [] {
            if item.action == #selector(retryPressed) { item.isEnabled = !retry.isHidden }
            if item.action == #selector(inquiryEntryPressed) { item.isEnabled = !isIdea && hasInquiry }
            if let action = item.action, [#selector(referencesPressed), #selector(activityPressed), #selector(olderPressed), #selector(searchPressed)].contains(action) { item.isHidden = isIdea || genericLease != nil }
            if item.title == "Send shortcut" { item.isHidden = isIdea || inquiryActive }
        }
    }
    @objc private func footerShortcutPressed(_ item: NSMenuItem) {
        onMode?(item.representedObject as? String == "enter" ? .enter : .modifiedEnter)
    }
    private func closeIdeaPopovers() {
        ideaPopoverPane = nil
        ideaMore.menu?.cancelTracking(); ideaTheme.menu?.cancelTracking()
        ideaSearchPopover.close()
    }
    func focusIdeaSearch() { if isIdea { openIdeaSearch() } }
    @objc private func openIdeaSearch() {
        guard isIdea, ideaSearch.isEnabled else { return }
        ideaPreviousResponder = window?.firstResponder; ideaPopoverPane = paneID
        ideaSearchPopover.show(relativeTo: ideaMenu.bounds, of: ideaMenu, preferredEdge: .maxY)
        ideaSearch.window?.makeFirstResponder(ideaSearch)
    }
    func popoverDidClose(_ notification: Notification) {
        if isIdea, paneID == ideaPopoverPane, window?.isKeyWindow == true {
            window?.makeFirstResponder(ideaPreviousResponder ?? text)
        }
        ideaPopoverPane = nil
    }
    @objc private func ideaPressed(_ sender: NSButton) { if let action = sender.identifier?.rawValue { onIdeaAction?(action, "") } }
    @objc private func ideaMenuPressed(_ sender: NSMenuItem) { if let intent = sender.representedObject as? [String: String], let action = intent["action"] { onIdeaAction?(action, intent["query"] ?? "") } else if let action = sender.representedObject as? String { onIdeaAction?(action, "") } }
    @objc private func ideaSearchChanged() { onIdeaAction?("search", ideaSearch.stringValue) }
    func showStatus(_ message: String) {
        status.stringValue = message; statusButton.toolTip = message
        targetButton.toolTip = message.isEmpty ? targetButton.title : message
        statusButton.title = "●"
        statusButton.setAccessibilityLabel(message.isEmpty ? "Not connected" : message)
        statusButton.lineBreakMode = .byTruncatingTail
        statusButton.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    }
    @objc private func connectionPressed() { statusPopover.show(relativeTo: tools.bounds, of: tools, preferredEdge: .maxY) }
    func setSendMode(_ value: ChatSendModeV1) {
        sendMode = value; mode.item(at: 1)?.state = value == .enter ? .on : .off; mode.item(at: 2)?.state = value == .modifiedEnter ? .on : .off; mode.selectItem(at: 0)
        // Global Chat preferences also reach Idea and Inquiry composers.
        // They must not replace those modes' action or change the input width.

        if !isIdea && !inquiryActive { text.toolTip = value == .enter ? "Enter sends; Shift+Enter inserts newline" : "Cmd+Enter sends; Enter inserts newline" }
    }
    private func resizeEditor() {
        let height = draftEditor.preferredHeight()
        footerLayout.setDraftPreferredHeight(height)
        if abs(lastIdeaComposerHeight - height - 10) > 0.5 {
            lastIdeaComposerHeight = height + 10
            container?.showStationComposer(self, height: height + 10)
        }
    }
    func textDidBeginEditing(_ notification: Notification) {
        if isIdea, notification.object as? NSTextView === text { lastIdeaEditor = text }
    }
    func textViewDidChangeSelection(_ notification: Notification) {
        if isIdea, notification.object as? NSTextView === text { lastIdeaEditor = text }
    }
    func textDidChange(_ notification: Notification) {
        publishInquiryEnvironment()
        let editor = notification.object as? NSTextView ?? text
        if paneID != nil && !changing && !editor.hasMarkedText() { onDraft?(editor.string) }
        if editor === text { resizeEditor() }
    }
    func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {

        if inquiryActive && selector == #selector(NSResponder.cancelOperation(_:)) && !textView.hasMarkedText() { inquiryLater(); return true }
        if selector == #selector(NSResponder.cancelOperation(_:)) && !textView.hasMarkedText() {
            onExit?()
            return true
        }
        guard selector == #selector(NSResponder.insertNewline(_:)), !textView.hasMarkedText() else { return false }
        if inquiryActive {
            let event = NSApp.currentEvent
            guard event?.modifierFlags.intersection([.command,.option,.control,.shift]) == [.command] else { return false }
            if event?.isARepeat != true { sendPressed() }; return true
        }
        let flags = NSApp.currentEvent?.modifierFlags.intersection(.deviceIndependentFlagsMask) ?? []
        if flags.contains(.shift) || flags.contains(.option) || flags.contains(.control) { return false }
        if (isIdea || sendMode == .modifiedEnter) && !flags.contains(.command) { return false }
        sendPressed(); return true
    }
    @objc private func modePressed() { onMode?(mode.indexOfSelectedItem == 1 ? .enter : .modifiedEnter) }
    @objc private func choosePressed() {
        if isIdea { onChooseIdea?() } else { onChoose?() }
    }
    @objc private func bindIdeaSession(_ item:NSMenuItem) {
        onIdeaInput?(["type":"idea_bind","binding_id":item.representedObject ?? NSNull()])
    }
    @objc private func checkIdeaReceipt() { onIdeaInput?(["type":"idea_check_receipt"]) }
    @objc private func showIdeaSession() {onIdeaInput?(["type":"idea_show_session"])}
    @objc private func attachIdeaReference() { insertIdeaNode() }
    func insertIdeaNode() {
        guard let node = ideaSelectedNode, let uuid = node["node_uuid"], !uuid.isEmpty else { return }
        let insertion = "\(node["title"] ?? "Node")（\(uuid)）"
        let focused = (window?.firstResponder as? NSTextView).flatMap { ownsIdeaEditor($0) ? $0 : nil }
        let saved = lastIdeaEditor.flatMap { ownsIdeaEditor($0) ? $0 : nil }
        let target = focused ?? saved ?? text
        guard !target.hasMarkedText() else { return }
        lastIdeaEditor = target
        target.window?.makeFirstResponder(target)
        target.insertText(insertion, replacementRange: target.selectedRange())
    }
    private func ownsIdeaEditor(_ candidate: NSTextView) -> Bool {
        candidate === text || ideaBlockEditors.values.contains(where: { $0.editor === candidate }) || ideaDetailEditor?.editor === candidate
    }
    func dictateEditorTarget(_ editor:NSTextView, preserveSelection:Bool=false)->StationDictateTarget? {
        let pane=paneID, revision=dictateRevision, identity=dictateInputIdentity, lease=genericLease
        let target=StationDictateTarget.editor(editor,preserveSelection:preserveSelection,includeContext:dictateControls.dictate?.collectsContext == true,valid:{[weak self] in
            self?.paneID==pane && self?.dictateRevision==revision && self?.dictateInputIdentity==identity
        })
        if editor === text, let lease {
            target?.sourceApplied = {[weak self,weak editor] source in
                guard let self,let editor else{return}
                self.onDictated?(lease,source,editor.string)
            }
        }
        return target
    }
    func dictateDefaultTarget(node: Bool) -> StationDictateTarget? {
        let identity = paneID, viewID = ideaViewID, revision = dictateRevision, inputIdentity = dictateInputIdentity
        if node, isIdea, let selected = ideaSelectedNode, let uuid = selected["node_uuid"], !uuid.isEmpty {
            let original = ideaBlockEditors[uuid]?.editor.string ?? ""
            return StationDictateTarget(kind: "idea_node", context: dictateControls.dictate?.collectsContext == true ? String((selected["title"] ?? "").prefix(1024)) + "\n" + String(original.prefix(1024)) : "") { [weak self] text in
                guard let self, self.window != nil, self.paneID == identity, self.ideaViewID == viewID, self.dictateRevision == revision,
                      self.ideaSelectedNode?["node_uuid"] == uuid,
                      (self.ideaBlockEditors[uuid]?.editor.string ?? "") == original else { return .unchanged }
                let inserted = original + (original.isEmpty || original.hasSuffix("\n") ? "" : "\n") + text
                self.setDictateNode(uuid, title: selected["title"] ?? "Node", view: viewID, text: inserted)
                return self.ideaBlockEditors[uuid]?.editor.string == inserted ? .applied : .unknown
            }
        }
        guard activeTextEditor.isEditable else { return nil }
        activeTextEditor.window?.makeFirstResponder(activeTextEditor)
        return StationDictateTarget.editor(activeTextEditor, kind: "default_editor", includeContext:dictateControls.dictate?.collectsContext == true, valid: { [weak self] in self?.paneID == identity && self?.dictateRevision == revision && self?.dictateInputIdentity == inputIdentity })
    }
    private func setDictateNode(_ uuid: String, title: String, view: String, text: String) {
        guard ideaViewID == view else { return }
        let old = ideaBlockEditors[uuid]?.editor.string ?? ""
        undoManager?.registerUndo(withTarget: self) { target in target.setDictateNode(uuid, title: title, view: view, text: old) }
        onIdeaInput?(["type":"idea_node_instruction", "view_id":view, "node_uuid":uuid, "title":title, "text":text])
    }
    func setIdeaReaderOpen(_ value: Bool) {
        ideaReaderOpen = value
        if !value { closeIdeaInstruction() }
    }
    private func updateIdeaDocument(_ blocks: [[String: String]]) {
        let ids = Set(blocks.compactMap { $0["node_uuid"] })
        for id in Array(ideaBlockEditors.keys) where !ids.contains(id) {
            if let editor = ideaBlockEditors.removeValue(forKey: id) { ideaDocument.removeArrangedSubview(editor); editor.removeFromSuperview() }
        }
        for block in blocks {
            guard let id = block["node_uuid"] else { continue }
            let editor = ideaBlockEditors[id] ?? makeNodeEditor(block, detail: false)
            if ideaBlockEditors[id] == nil {
                ideaBlockEditors[id] = editor; ideaDocument.addArrangedSubview(editor)
                editor.widthAnchor.constraint(equalTo: ideaDocument.widthAnchor).isActive = true
            }
            editor.update(block)
        }
        if ideaReaderOpen, let detail = ideaDetailEditor, let selected = ideaSelectedNode?["node_uuid"], selected != detail.nodeID, let canvas = ideaDetailCanvas {
            editIdeaInstruction(in: canvas)
        }
        if let detail = ideaDetailEditor {
            detail.update(blocks.first(where: { $0["node_uuid"] == detail.nodeID }) ?? ["node_uuid": detail.nodeID, "title": detail.nodeTitle, "text": ""])
        }
    }
    private func makeNodeEditor(_ block: [String: String], detail: Bool) -> IdeaNodeInstructionEditor {
        let editor = IdeaNodeInstructionEditor(block: block, detail: detail)
        let identity = paneID
        let viewIdentity = ideaViewID
        editor.onFocus = { [weak self, weak editor] in
            guard let self, let editor, self.paneID == identity, self.ideaViewID == viewIdentity else { return }
            self.lastIdeaEditor = editor.editor
        }
        editor.onChange = { [weak self, weak editor] in
            guard let self, let editor, self.paneID == identity else { return }
            self.onIdeaInput?(["type":"idea_node_instruction", "view_id":viewIdentity, "node_uuid":editor.nodeID, "title":editor.nodeTitle, "text":editor.editor.string])
            self.resizeEditor()
        }
        editor.onRemove = { [weak self, weak editor] in
            guard let self, let editor, self.paneID == identity else { return }
            self.onIdeaInput?(["type":"idea_node_instruction", "view_id":viewIdentity, "node_uuid":editor.nodeID, "title":editor.nodeTitle, "text":"", "remove":true])
        }
        editor.onReview = { [weak self, weak editor] in
            guard let self, let editor else { return }
            let id = editor.nodeID; self.closeIdeaInstruction(notifyScene: true)
            if let body = self.ideaBlockEditors[id] {
                self.lastIdeaEditor = body.editor
                body.scrollToVisible(body.bounds); body.window?.makeFirstResponder(body.editor)
            } else { self.window?.makeFirstResponder(self.text) }
        }
        editor.onSend = { [weak self] in self?.sendPressed() }
        editor.onLocate = { [weak self, weak editor] in
            guard let self, let editor else { return }
            self.onNativeIdeaCommand?("locate_node", editor.nodeID)
        }
        editor.onClose = { [weak self] in self?.closeIdeaInstruction(notifyScene: true) }
        return editor
    }
    @objc private func copyIdeaDraft() {
        guard isIdea, !ideaDocumentText.isEmpty else { return }
        // This is the shared projection used by sending, never a native serializer.
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(ideaDocumentText, forType: .string)
    }
    @objc private func editSelectedInstruction() {
        onNativeIdeaCommand?("edit_instruction", "")
    }
    func editIdeaInstruction(in canvas: NSView) {
        guard isIdea, ideaReaderOpen, let node = ideaSelectedNode, let id = node["node_uuid"], !id.isEmpty,
              let parent = canvas.superview, let rect = ideaEditorRect,
              (rect["width"] ?? 0) > 0, (rect["height"] ?? 0) > 0 else { return }
        closeIdeaInstruction()
        var block = node; block["text"] = ideaBlockEditors[id]?.editor.string ?? ""
        let editor = makeNodeEditor(block, detail: true)
        let box = NSView(); box.wantsLayer = true
        box.layer?.cornerRadius = 10; box.layer?.borderWidth = 1
        box.layer?.shadowColor = NSColor.black.cgColor; box.layer?.shadowOpacity = 0.22
        box.layer?.shadowRadius = 8; box.layer?.shadowOffset = NSSize(width: 0, height: -2)
        box.setAccessibilityElement(false)
        box.translatesAutoresizingMaskIntoConstraints = false
        // SurfaceView is an accessibility text-area leaf. A native sibling
        // exposes the annotation controls without exposing terminal contents.
        parent.addSubview(box, positioned: .above, relativeTo: canvas)
        let viewport = NSScrollView()
        viewport.drawsBackground = false; viewport.hasVerticalScroller = true
        viewport.borderType = .noBorder; viewport.translatesAutoresizingMaskIntoConstraints = false
        viewport.documentView = editor; box.addSubview(viewport)
        let fillHeight = editor.heightAnchor.constraint(equalTo: viewport.contentView.heightAnchor)
        fillHeight.priority = .defaultHigh
        ideaAnchorX = box.leadingAnchor.constraint(equalTo: canvas.leadingAnchor)
        ideaAnchorTop = box.topAnchor.constraint(equalTo: canvas.topAnchor)
        ideaAnchorWidth = box.widthAnchor.constraint(equalToConstant: 1)
        ideaAnchorHeight = box.heightAnchor.constraint(equalToConstant: 1)
        NSLayoutConstraint.activate([
            ideaAnchorX!, ideaAnchorTop!, ideaAnchorWidth!, ideaAnchorHeight!,
            viewport.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 10),
            viewport.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -10),
            viewport.topAnchor.constraint(equalTo: box.topAnchor, constant: 10),
            viewport.bottomAnchor.constraint(equalTo: box.bottomAnchor, constant: -10),
            editor.widthAnchor.constraint(equalTo: viewport.contentView.widthAnchor),
            editor.heightAnchor.constraint(greaterThanOrEqualTo: viewport.contentView.heightAnchor), fillHeight,
        ])
        ideaDetailBox = box; ideaDetailEditor = editor; ideaDetailCanvas = canvas
        updateInstructionPresentation(rect: ideaEditorRect, style: ideaInstructionStyle)
        lastIdeaEditor = editor.editor
        editor.window?.makeFirstResponder(editor.editor)
    }
    func updateInstructionPresentation(rect: [String: Double]?, style: [String: String]?) {
        ideaEditorRect = rect; ideaInstructionStyle = style
        guard let rect else { closeIdeaInstruction(); return }
        guard let box = ideaDetailBox else { return }
        // Godot owns the entire adjacent composition, in Pane logical pixels.
        // No native clamping: that could silently cover the detail card again.
        ideaAnchorX?.constant = CGFloat(rect["x"] ?? 0)
        ideaAnchorTop?.constant = CGFloat(rect["y"] ?? 0)
        ideaAnchorWidth?.constant = CGFloat(rect["width"] ?? 0)
        ideaAnchorHeight?.constant = CGFloat(rect["height"] ?? 0)
        box.isHidden = (rect["width"] ?? 0) <= 0 || (rect["height"] ?? 0) <= 0
        if let style {
            box.layer?.backgroundColor = IdeaNodeInstructionEditor.color(style["card"])?.cgColor
            box.layer?.borderColor = IdeaNodeInstructionEditor.color(style["border"])?.cgColor
            ideaDetailEditor?.applyStyle(style)
        }
    }
    private func leaveIdeaInstruction() {
        // The callback routes using paneID: notify before assigning the new Pane.
        let notify = isIdea && ideaEditorRect != nil
        ideaEditorRect = nil; ideaInstructionStyle = nil; ideaReaderOpen = false
        closeIdeaInstruction(notifyScene: notify, restoreCanvasFocus: false)
    }
    private func closeIdeaInstruction(notifyScene: Bool = false, restoreCanvasFocus: Bool = true) {
        let canvas = ideaDetailCanvas
        let retiredResponder = (window?.firstResponder as? NSView).map { responder in
            ideaDetailBox.map { responder === $0 || responder.isDescendant(of: $0) } ?? false
        } ?? false
        ideaDetailBox?.removeFromSuperview(); ideaDetailBox = nil; ideaDetailEditor = nil; ideaDetailCanvas = nil
        if notifyScene { onNativeIdeaCommand?("close_instruction", "") }
        if restoreCanvasFocus && (notifyScene || retiredResponder) { canvas?.window?.makeFirstResponder(canvas) }
    }
    private func rebuildIdeaReferences() {
        ideaReferenceMenu.removeAllItems()
        let title = ideaReferences.isEmpty ? "Tree context" : ideaReferences.count == 1 ? (ideaReferences[0]["title"] as? String ?? "Node") : "\(ideaReferences.count) nodes"
        ideaReferenceMenu.addItem(withTitle:String(title.prefix(22)))
        for (index,ref) in ideaReferences.enumerated() {
            for (operation,label) in [("locate","Locate"),("remove","Remove")] {
                let item=NSMenuItem(title:"\(label): \(ref["title"] as? String ?? "Node")",action:#selector(ideaReferencePressed(_:)),keyEquivalent:"")
                item.target=self;item.representedObject=["type":"idea_reference","operation":operation,"index":index];ideaReferenceMenu.menu?.addItem(item)
            }
        }
        if canOpenIdeaSession { let item=NSMenuItem(title:"Show original session",action:#selector(showIdeaSession),keyEquivalent:"");item.target=self;ideaReferenceMenu.menu?.addItem(item) }
        if hasIdeaBinding { let item=NSMenuItem(title:"Unbind session",action:#selector(bindIdeaSession(_:)),keyEquivalent:"");item.target=self;ideaReferenceMenu.menu?.addItem(item) }
    }
    @objc private func ideaReferencePressed(_ item:NSMenuItem) { if let value=item.representedObject as? [String:Any] {onIdeaInput?(value)} }

    @objc private func sendPressed() {
        guard send.isEnabled, !activeTextEditor.hasMarkedText(),
              !ideaBlockEditors.values.contains(where: { $0.editor.hasMarkedText() }),
              ideaDetailEditor?.editor.hasMarkedText() != true else { return }
        onDraft?(activeTextEditor.string); onSend?()
    }
    @objc private func quotePressed() {
        quoteSelection()
    }
    func quoteSelection() {
        guard genericLease != nil, !text.hasMarkedText() else { return }
        onQuote?()
    }
    func insertQuotedText(_ selection: String, lease: [String:Any]) {
        guard let current = genericLease, NSDictionary(dictionary: current).isEqual(to: lease), !text.hasMarkedText() else { return }
        window?.makeFirstResponder(text)
        // NSTextView publishes this edit once through textDidChange.
        // A second queued edit would reuse the pre-insertion draft lease.
        text.breakUndoCoalescing()
        text.insertText(selection, replacementRange: text.selectedRange())
        text.breakUndoCoalescing()
    }
    @objc private func olderPressed() { onOlder?() }
    @objc private func retryPressed() { onRetry?() }
    @objc private func referencesPressed() { onReferences?() }
    @objc private func activityPressed() { onActivity?() }
    @objc private func searchPressed() {
        let alert = NSAlert(); alert.messageText = "Search loaded messages"
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 320, height: 26)); alert.accessoryView = field
        alert.addButton(withTitle: "Search"); alert.addButton(withTitle: "Cancel")
        if alert.runModal() == .alertFirstButtonReturn { onSearch?(field.stringValue) }
    }
}

@MainActor
final class StationFooterLayout: NSView {
    let leadingControls = NSStackView(), trailingControls = NSStackView()
    let slot = NSView()
    let viewport: NSScrollView
    var viewportHeight: NSLayoutConstraint!
    var targetWidth: NSLayoutConstraint!
    var onDensityChanged: ((Bool)->Void)?
    private(set) var compact = false
    private var normalTargetWidth:CGFloat = 140
    private var normalMinimum:CGFloat = 0
    private var viewportAnchors: [NSLayoutConstraint] = []
    init(leading: [NSView], trailing: [NSView], primary: NSButton, viewport: NSScrollView, target: NSButton) {
        self.viewport = viewport
        super.init(frame: .zero)
        for v in leading { leadingControls.addArrangedSubview(v) }
        for v in trailing { trailingControls.addArrangedSubview(v) }
        let primarySlot = NSView(); primarySlot.translatesAutoresizingMaskIntoConstraints = false
        primary.translatesAutoresizingMaskIntoConstraints = false; primarySlot.addSubview(primary)
        trailingControls.addArrangedSubview(primarySlot)
        let width = ["Send", "Sending…", "Retry"].map { NSButton(title:$0,target:nil,action:nil).fittingSize.width }.max() ?? 80
        NSLayoutConstraint.activate([primary.widthAnchor.constraint(equalToConstant:width),primary.leadingAnchor.constraint(equalTo:primarySlot.leadingAnchor),primary.trailingAnchor.constraint(equalTo:primarySlot.trailingAnchor),primary.bottomAnchor.constraint(equalTo:primarySlot.bottomAnchor),primarySlot.heightAnchor.constraint(equalToConstant:30)])
        for group in [leadingControls,trailingControls] {
            group.orientation = .horizontal; group.alignment = .bottom; group.spacing = 6
            group.distribution = .fill
            group.setHuggingPriority(.required,for:.horizontal)
            group.translatesAutoresizingMaskIntoConstraints = false; addSubview(group)
        }
        slot.translatesAutoresizingMaskIntoConstraints = false;addSubview(slot)
        viewport.translatesAutoresizingMaskIntoConstraints = false
        viewportHeight = viewport.heightAnchor.constraint(equalToConstant:30)
        targetWidth = target.widthAnchor.constraint(equalToConstant:140)
        NSLayoutConstraint.activate([viewportHeight,targetWidth,
            leadingControls.leadingAnchor.constraint(equalTo:leadingAnchor,constant:8),
            trailingControls.trailingAnchor.constraint(equalTo:trailingAnchor,constant:-8),
            leadingControls.bottomAnchor.constraint(equalTo:bottomAnchor,constant:-6),trailingControls.bottomAnchor.constraint(equalTo:bottomAnchor,constant:-6),
            slot.leadingAnchor.constraint(equalTo:leadingControls.trailingAnchor,constant:6),slot.trailingAnchor.constraint(equalTo:trailingControls.leadingAnchor,constant:-6),slot.bottomAnchor.constraint(equalTo:bottomAnchor,constant:-6),slot.heightAnchor.constraint(equalToConstant:30),slot.widthAnchor.constraint(greaterThanOrEqualToConstant:120)])
        mountViewport()
    }
    required init?(coder:NSCoder) { nil }
    func setDraftPreferredHeight(_ height:CGFloat) {
        if abs(viewportHeight.constant-height)>0.5 { viewportHeight.constant=height }
    }
    func configureTargetSlot(normalWidth:CGFloat) {
        normalTargetWidth=normalWidth;targetWidth.constant=normalWidth
        normalMinimum=leadingControls.fittingSize.width+trailingControls.fittingSize.width+148
        resolveDensity()
    }
    private func resolveDensity() {
        let value=bounds.width<normalMinimum
        targetWidth.constant=value ? 76 : normalTargetWidth
        if compact != value {compact=value;onDensityChanged?(value)}
    }
    override func setFrameSize(_ newSize:NSSize) {
        super.setFrameSize(newSize)
        if targetWidth != nil {resolveDensity()}
    }
    func mountViewport() {
        NSLayoutConstraint.deactivate(viewportAnchors)
        addSubview(viewport)
        viewportAnchors=[viewport.leadingAnchor.constraint(equalTo:slot.leadingAnchor),viewport.trailingAnchor.constraint(equalTo:slot.trailingAnchor),viewport.bottomAnchor.constraint(equalTo:slot.bottomAnchor)]
        NSLayoutConstraint.activate(viewportAnchors)
    }
    override func draw(_ dirtyRect:NSRect) {
        NSColor.windowBackgroundColor.setFill()
        NSRect(x:0,y:0,width:bounds.width,height:40).intersection(dirtyRect).fill()
    }
    override func hitTest(_ point:NSPoint)->NSView? {
        let p=convert(point,from:superview)
        if viewport.superview === self && !viewport.isHidden && viewport.frame.contains(p) {return viewport.hitTest(p)}
        guard p.y<=40 else {return nil}
        let hit=super.hitTest(point);return hit === self ? nil : hit
    }
}

@MainActor
final class StationDraftEditor {
    let text = NSTextView(), scroll = NSScrollView(), document = NSStackView()
    var generalHeight: NSLayoutConstraint?
    var blocks: [String: IdeaNodeInstructionEditor] = [:]
    var mounted = false
    init() {
        text.isRichText = false; text.allowsUndo = true
        text.font = .monospacedSystemFont(ofSize: 14, weight: .regular)
        text.isAutomaticQuoteSubstitutionEnabled = false; text.isAutomaticDashSubstitutionEnabled = false
        text.isHorizontallyResizable = false; text.isVerticallyResizable = true
        text.autoresizingMask = [.width]; text.textContainerInset = NSSize(width: 6, height: 6)
        text.textContainer?.widthTracksTextView = true; text.drawsBackground = false
        scroll.hasVerticalScroller = true; scroll.documentView = text
        scroll.drawsBackground = true; scroll.backgroundColor = .textBackgroundColor; scroll.borderType = .bezelBorder
    }
    func setIdeaDocument(_ value: Bool) {
        if value && !mounted {
            scroll.documentView = nil
            text.autoresizingMask = []
            document.orientation = .vertical; document.alignment = .leading; document.spacing = 8
            document.translatesAutoresizingMaskIntoConstraints = false
            text.translatesAutoresizingMaskIntoConstraints = false
            text.isVerticallyResizable = false
            text.textContainer?.heightTracksTextView = false
            text.textContainer?.containerSize.height = .greatestFiniteMagnitude
            document.addArrangedSubview(text)
            generalHeight = text.heightAnchor.constraint(equalToConstant: 30); generalHeight?.isActive = true
            text.widthAnchor.constraint(equalTo: document.widthAnchor).isActive = true
            scroll.documentView = document
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor).isActive = true
            mounted = true
        } else if !value && mounted {
            for editor in blocks.values { document.removeArrangedSubview(editor); editor.removeFromSuperview() }
            blocks.removeAll()
            document.removeArrangedSubview(text); text.removeFromSuperview()
            generalHeight?.isActive = false; generalHeight = nil
            text.translatesAutoresizingMaskIntoConstraints = true
            text.autoresizingMask = [.width]
            text.isVerticallyResizable = true
            scroll.documentView = text; mounted = false
        }

    }
    func preferredHeight() -> CGFloat {
        let line = text.layoutManager?.defaultLineHeight(for: text.font ?? .systemFont(ofSize: 14)) ?? 18
        if mounted {
            if let lm = text.layoutManager, let tc = text.textContainer {
                lm.ensureLayout(for: tc); generalHeight?.constant = max(30, lm.usedRect(for: tc).height + 12)
            }
            return max(30, min(document.fittingSize.height, line * 6 + 12))
        }
        guard let lm = text.layoutManager, let tc = text.textContainer else { return 30 }
        lm.ensureLayout(for: tc)
        return max(30, min(lm.usedRect(for: tc).height + 12, line * 6 + 12))
    }
}

@MainActor
final class ConversationChoiceButton: NSButton {
    var choose: (() -> Void)?
    init(_ title: String, choose: @escaping () -> Void) {
        super.init(frame: .zero); self.title = title; self.choose = choose
        target = self; action = #selector(invoke); bezelStyle = .rounded
        lineBreakMode = .byTruncatingTail; alignment = .left
    }
    required init?(coder: NSCoder) { nil }
    @objc private func invoke() { choose?() }
}

@MainActor
final class ToolActivityPanel: NSObject, NSWindowDelegate {
    var onInput: (([String: Any]) -> Void)?
    var onClose: (() -> Void)?
    private let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 640, height: 500),
        styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    private let rows = NSStackView()
    private var actions: [[String: Any]] = []
    override init() {
        super.init()
        panel.delegate = self
        panel.title = "Tool activity"; panel.isReleasedWhenClosed = false
        rows.orientation = .vertical; rows.alignment = .leading; rows.spacing = 8
        rows.edgeInsets = NSEdgeInsets(top: 12, left: 12, bottom: 12, right: 12)
        let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.documentView = rows
        rows.translatesAutoresizingMaskIntoConstraints = false
        panel.contentView = scroll
        rows.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor).isActive = true
    }
    func show(_ items: [String: Any]) {
        for view in rows.arrangedSubviews { rows.removeArrangedSubview(view); view.removeFromSuperview() }
        actions.removeAll()
        panel.title = items["title"] as? String ?? "Tool activity"
        for row in items["entries"] as? [[String: Any]] ?? [] {
            let title = row["title"] as? String ?? ""
            if let input = row["input"] as? [String: Any] {
                let button = NSButton(title: title, target: self, action: #selector(selected(_:)))
                button.tag = actions.count; actions.append(input); rows.addArrangedSubview(button)
            } else {
                let label = NSTextField(wrappingLabelWithString: title); label.isSelectable = true
                rows.addArrangedSubview(label)
            }
        }
        panel.makeKeyAndOrderFront(nil)
    }
    @objc private func selected(_ sender: NSButton) {
        guard actions.indices.contains(sender.tag) else { return }; onInput?(actions[sender.tag])
    }
    func close() { panel.close() }
    func windowWillClose(_ notification: Notification) { onClose?() }
}

@MainActor
final class ConversationPicker: NSObject, NSSearchFieldDelegate {
    var onInput: (([String: Any]) -> Void)?
    var onOpen: ((StationPaneContentV1, Bool) -> Void)?
    var onManageConnections: (() -> Void)?
    private let windowID: String
    private let popover = NSPopover()
    private let root = NSStackView()
    private let columns = NSStackView()
    private let channelColumn = NSStackView()
    private let threadColumn = NSStackView()
    private let channels = NSStackView()
    private let threads = NSStackView()
    private let targets = NSPopUpButton(frame: .zero, pullsDown: false)
    private let channelSearch = NSSearchField()
    private let threadSearch = NSSearchField()
    private let status = NSTextField(labelWithString: "Choose a channel, then a thread")
    private let newPane = NSButton(checkboxWithTitle: "Open in new Pane", target: nil, action: nil)
    private let back = NSButton(title: "‹ Channels", target: nil, action: nil)
    private var generation: UInt64 = 0
    private var targetID = "", nodeID = "", channelID = ""
    private var scope: [String: Any] = [:]
    private var targetRows: [[String: Any]] = [], channelRows: [[String: Any]] = [], threadRows: [[String: Any]] = []
    private var narrow = false
    private var requestedThreads = false
    init(windowID: String) {
        self.windowID = windowID; super.init()
        let vc = NSViewController(); vc.view = NSView(); popover.contentViewController = vc; popover.behavior = .transient
        root.orientation = .vertical; root.alignment = .leading; root.spacing = 6
        root.translatesAutoresizingMaskIntoConstraints = false; vc.view.addSubview(root)
        NSLayoutConstraint.activate([root.leadingAnchor.constraint(equalTo: vc.view.leadingAnchor, constant: 10), root.trailingAnchor.constraint(equalTo: vc.view.trailingAnchor, constant: -10), root.topAnchor.constraint(equalTo: vc.view.topAnchor, constant: 10), root.bottomAnchor.constraint(equalTo: vc.view.bottomAnchor, constant: -10)])
        targets.target = self; targets.action = #selector(targetChanged)
        back.target = self; back.action = #selector(showChannels)
        channelSearch.placeholderString = "Search channels"; threadSearch.placeholderString = "Search threads in this channel"
        channelSearch.delegate = self; threadSearch.delegate = self
        channelSearch.target = self; channelSearch.action = #selector(confirmChannel)
        threadSearch.target = self; threadSearch.action = #selector(confirmThread)
        columns.orientation = .horizontal; columns.spacing = 8; columns.distribution = .fillEqually
        for (column, search, rows) in [(channelColumn, channelSearch, channels),(threadColumn,threadSearch,threads)] {
            column.orientation = .vertical; column.alignment = .leading; column.spacing = 4
            rows.orientation = .vertical; rows.alignment = .leading; rows.spacing = 2
            let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.documentView = rows
            rows.translatesAutoresizingMaskIntoConstraints = false
            rows.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor).isActive = true
            scroll.heightAnchor.constraint(equalToConstant: 240).isActive = true
            column.addArrangedSubview(search); column.addArrangedSubview(scroll)
            search.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
            scroll.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
            columns.addArrangedSubview(column)
        }
        let manage = NSButton(title: "Add Polar / Manage Connections…", target: self, action: #selector(managePressed))
        for view in [targets,status,back,columns,newPane,manage] as [NSView] { root.addArrangedSubview(view) }
        columns.widthAnchor.constraint(equalTo: root.widthAnchor).isActive = true
        status.lineBreakMode = .byTruncatingTail
    }
    @objc private func managePressed() { close(); onManageConnections?() }
    private var context: [String: Any] { ["window_id":windowID,"generation":generation] }
    func present(from button: NSButton, scope: [String: Any], channel: String) {
        generation += 1; self.scope = scope; requestedThreads = false
        targetID = scope["target_id"] as? String ?? ""; nodeID = scope["node_id"] as? String ?? ""; channelID = channel
        newPane.state = .off; channelSearch.stringValue = ""; threadSearch.stringValue = ""
        narrow = (button.window?.frame.width ?? 900) < 700
        channelColumn.isHidden = false; threadColumn.isHidden = narrow; back.isHidden = true
        popover.contentSize = NSSize(width: narrow ? 320 : 660, height: 380)
        channelRows = []; threadRows = []; renderRows()
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .maxY)
        popover.contentViewController?.view.window?.makeFirstResponder(narrow ? channelSearch : threadSearch)
        onInput?(["type":"list_targets"])
        if !targetID.isEmpty { browse() }
    }
    func close() { popover.performClose(nil) }
    private func browse() {
        status.stringValue = "Loading channels…"
        onInput?(["type":"browse","target_id":targetID,"context":context])
    }
    private func loadThreads() {
        requestedThreads = true; status.stringValue = "Loading threads…"; threadRows = []; renderRows()
        onInput?(["type":"threads","target_id":targetID,"node_id":nodeID,"channel_id":channelID,"context":context])
    }
    func consume(_ effect: [String: Any]) {
        guard popover.isShown, let kind = effect["type"] as? String else { return }
        if kind == "targets" {
            let rows = effect["items"] as? [[String: Any]] ?? []
            guard rows.isEmpty || rows[0]["origin"] != nil else { return }
            targetRows = rows; targets.removeAllItems()
            targets.addItems(withTitles: rows.map { $0["display_name"] as? String ?? $0["target_id"] as? String ?? "Polar" })
            if let index = rows.firstIndex(where: { $0["target_id"] as? String == targetID }) { targets.selectItem(at: index) }
            return
        }
        guard let ctx = effect["context"] as? [String: Any], ctx["window_id"] as? String == windowID,
              (ctx["generation"] as? NSNumber)?.uint64Value == generation else { return }
        if kind == "browse_error" { status.stringValue = effect["message"] as? String ?? "Cannot load conversations"; return }
        guard effect["target_id"] as? String == targetID else { return }
        if kind == "channels" {
            nodeID = effect["node_id"] as? String ?? ""; channelRows = effect["items"] as? [[String: Any]] ?? []
            status.stringValue = channelRows.isEmpty ? "No channels available" : "Choose a channel, then a thread"
            renderRows(); if !channelID.isEmpty && !requestedThreads { loadThreads() }
        } else if kind == "threads", effect["node_id"] as? String == nodeID, effect["channel_id"] as? String == channelID {
            threadRows = effect["items"] as? [[String: Any]] ?? []
            status.stringValue = threadRows.isEmpty ? "No threads in this channel" : "Select a thread to switch this chat Pane"
            renderRows()
        }
    }
    private func filtered(_ rows: [[String: Any]], query: String, key: String) -> [[String: Any]] {
        rows.filter { query.isEmpty || ($0["display_name"] as? String ?? "").localizedCaseInsensitiveContains(query) || ($0[key] as? String ?? "").localizedCaseInsensitiveContains(query) }
    }
    private func renderRows() {
        let positions = [channels, threads].compactMap { rows -> (NSScrollView, NSPoint)? in
            guard let scroll = rows.enclosingScrollView else { return nil }
            return (scroll, scroll.contentView.bounds.origin)
        }
        defer {
            root.layoutSubtreeIfNeeded()
            for (scroll, position) in positions {
                scroll.contentView.scroll(to: position); scroll.reflectScrolledClipView(scroll.contentView)
            }
        }
        for box in [channels,threads] { for view in box.arrangedSubviews { box.removeArrangedSubview(view); view.removeFromSuperview() } }
        for row in filtered(channelRows, query: channelSearch.stringValue, key: "channel_id") {
            guard let id = row["channel_id"] as? String else { continue }
            let button = ConversationChoiceButton((id == channelID ? "✓ " : "") + (row["display_name"] as? String ?? id)) { [weak self] in self?.selectChannel(id) }
            channels.addArrangedSubview(button); button.widthAnchor.constraint(equalTo: channels.widthAnchor).isActive = true
        }
        for row in filtered(threadRows, query: threadSearch.stringValue, key: "thread_id") {
            guard let id = row["thread_id"] as? String else { continue }
            let selected = id == scope["thread"] as? String && targetID == scope["target_id"] as? String
            let button = ConversationChoiceButton((selected ? "✓ " : "") + (row["display_name"] as? String ?? id)) { [weak self] in self?.selectThread(id) }
            threads.addArrangedSubview(button); button.widthAnchor.constraint(equalTo: threads.widthAnchor).isActive = true
        }
    }
    private func selectChannel(_ id: String) {
        generation += 1; channelID = id; threadSearch.stringValue = ""
        if narrow { channelColumn.isHidden = true; threadColumn.isHidden = false; back.isHidden = false }
        loadThreads(); popover.contentViewController?.view.window?.makeFirstResponder(threadSearch)
    }
    private func selectThread(_ id: String) {
        let content = StationPaneContentV1(targetID: targetID, nodeID: nodeID, owner: nodeID, thread: id, actor: nodeID)
        close(); onOpen?(content,newPane.state == .on)
    }
    @objc private func targetChanged() {
        guard targetRows.indices.contains(targets.indexOfSelectedItem), let id = targetRows[targets.indexOfSelectedItem]["target_id"] as? String else { return }
        generation += 1; requestedThreads = false; targetID = id; nodeID = ""; channelID = ""; channelRows = []; threadRows = []; renderRows(); browse()
    }
    @objc private func showChannels() { channelColumn.isHidden = false; threadColumn.isHidden = true; back.isHidden = true }
    @objc private func confirmChannel() { if let id = filtered(channelRows, query: channelSearch.stringValue, key: "channel_id").first?["channel_id"] as? String { selectChannel(id) } }
    @objc private func confirmThread() { if let id = filtered(threadRows, query: threadSearch.stringValue, key: "thread_id").first?["thread_id"] as? String { selectThread(id) } }
    func controlTextDidChange(_ notification: Notification) { renderRows() }
}

@MainActor
final class BrowserPanel: NSPanel {
    var onTarget: ((String) -> Void)?
    var onChannel: ((String, String, String) -> Void)?
    var onOpen: ((StationPaneContentV1) -> Void)?
    var targetID = "embedded"
    private var nodeID = ""
    private var channelRows: [[String: Any]] = []; private var threadRows: [[String: Any]] = []
    private let target = NSTextField(string: "embedded")
    private let knownTargets = NSPopUpButton(frame: .zero, pullsDown: false)
    private var targetRows: [[String: Any]] = []
    private let channels = NSPopUpButton(frame: .zero, pullsDown: false)
    private let threads = NSPopUpButton(frame: .zero, pullsDown: false)
    private let status = NSTextField(wrappingLabelWithString: "Connect to an explicitly configured Polar target.")
    init() {
        super.init(contentRect: NSRect(x: 0, y: 0, width: 540, height: 250), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        title = "Open Polar Conversation"; isReleasedWhenClosed = false
        let connect = NSButton(title: "Connect", target: self, action: #selector(connectPressed))
        channels.target = self; channels.action = #selector(channelPressed)
        let open = NSButton(title: "Open in Pane", target: self, action: #selector(openPressed))
        knownTargets.target = self; knownTargets.action = #selector(targetSelected)
        let stack = NSStackView(views: [knownTargets, NSStackView(views: [target, connect]), channels, threads, status, open])
        stack.orientation = .vertical; stack.alignment = .leading; stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 14, left: 14, bottom: 14, right: 14); contentView = stack
        target.widthAnchor.constraint(equalToConstant: 350).isActive = true
        channels.widthAnchor.constraint(equalToConstant: 490).isActive = true
        threads.widthAnchor.constraint(equalToConstant: 490).isActive = true
    }
    func present(relativeTo window: NSWindow?) {
        if let window { setFrameOrigin(NSPoint(x: window.frame.midX - frame.width/2, y: window.frame.midY - frame.height/2)) }
        else { center() }
        makeKeyAndOrderFront(nil)
    }
    func showStatus(_ value: String) { status.stringValue = value }
    func consume(_ value: [String: Any]) {
        let kind = value["type"] as? String
        if kind == "targets", let rows = value["items"] as? [[String: Any]] {
            if rows.contains(where: { $0["origin"] != nil }) || rows.isEmpty {
                targetRows = rows; knownTargets.removeAllItems()
                knownTargets.addItems(withTitles: rows.map {
                    let name = $0["display_name"] as? String ?? "Polar"
                    let id = $0["target_id"] as? String ?? ""
                    return "\(name) — \(id)"
                })
                if let index = rows.firstIndex(where: { $0["target_id"] as? String == targetID }) {
                    knownTargets.selectItem(at: index)
                } else { knownTargets.select(nil) }
                if rows.isEmpty { status.stringValue = "No Polar targets configured. Terminal sessions remain available." }
            }
            // Only a verified Describe effect contains node_id; a configured pin
            // must not be promoted to an authenticated conversation by this UI.
            if let row = rows.first(where: { $0["target_id"] as? String == targetID }),
               let node = row["node_id"] as? String { nodeID = node }
        }
        if kind == "channels", value["target_id"] as? String == targetID {
            nodeID = value["node_id"] as? String ?? ""
            channelRows = value["items"] as? [[String: Any]] ?? []; channels.removeAllItems()
            channels.addItems(withTitles: channelRows.map { $0["display_name"] as? String ?? $0["channel_id"] as? String ?? "Channel" })
            threadRows = []; threads.removeAllItems(); channelPressed()
        }
        if kind == "threads", value["target_id"] as? String == targetID, value["node_id"] as? String == nodeID {
            threadRows = value["items"] as? [[String: Any]] ?? []; threads.removeAllItems()
            threads.addItems(withTitles: threadRows.map { $0["display_name"] as? String ?? $0["thread_id"] as? String ?? "Thread" })
            status.stringValue = "\(threadRows.count) Threads — \(nodeID.prefix(16))"
        }
    }
    @objc private func targetSelected() {
        let index = knownTargets.indexOfSelectedItem
        guard targetRows.indices.contains(index), let id = targetRows[index]["target_id"] as? String else { return }
        target.stringValue = id
        connectPressed()
    }
    @objc private func connectPressed() {
        let next = target.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !next.isEmpty, next.utf8.count <= 256 else { return }
        targetID = next; nodeID = ""; channels.removeAllItems(); threads.removeAllItems()
        channelRows = []; threadRows = []; onTarget?(next)
    }
    @objc private func channelPressed() {
        let index = channels.indexOfSelectedItem
        guard channelRows.indices.contains(index), !nodeID.isEmpty,
              let id = channelRows[index]["channel_id"] as? String else { return }
        onChannel?(targetID, nodeID, id)
    }
    @objc private func openPressed() {
        let index = threads.indexOfSelectedItem
        guard threadRows.indices.contains(index), !nodeID.isEmpty,
              let id = threadRows[index]["thread_id"] as? String else { return }
        onOpen?(StationPaneContentV1(targetID: targetID, nodeID: nodeID, owner: nodeID, thread: id, actor: nodeID))
    }
}
/// Scroll documents use a top origin; AppKit stacks keep their measured height.
@MainActor
final class InquiryDocumentView: NSView {
    let stack:NSStackView
    override var isFlipped:Bool { true }
    init(stack:NSStackView,width:CGFloat,minimumHeight:CGFloat) {
        self.stack=stack
        super.init(frame:NSRect(x:0,y:0,width:width,height:minimumHeight))
        stack.frame=NSRect(x:0,y:0,width:width,height:stack.fittingSize.height)
        stack.translatesAutoresizingMaskIntoConstraints=true
        addSubview(stack)
        measure(minimumHeight:minimumHeight)
    }
    required init?(coder:NSCoder) { fatalError("init(coder:) has not been implemented") }
    func measure(minimumHeight:CGFloat) {
        stack.layoutSubtreeIfNeeded()
        let height=stack.fittingSize.height
        stack.frame=NSRect(x:0,y:0,width:bounds.width,height:height)
        frame.size.height=max(minimumHeight,height)
    }
}

@MainActor
final class InquiryButton: NSButton {
    var inquiryAction=""
    var inquiryToken=""
    var inquiryValue:String?
}

/// Two native presentations edit the same Rust-owned node block. No canvas copy.
@MainActor
final class IdeaNodeInstructionEditor: NSView, NSTextViewDelegate {
    private var editorHeight: NSLayoutConstraint!
    override var isFlipped: Bool { true }
    let editor = NSTextView()
    let nodeID: String
    let nodeTitle: String
    var onChange: (() -> Void)?
    var onFocus: (() -> Void)?
    var onRemove: (() -> Void)?
    var onLocate: (() -> Void)?
    var onReview: (() -> Void)?
    var onSend: (() -> Void)?
    var onClose: (() -> Void)?
    private var updating = false
    private let detail: Bool
    private let header = NSStackView()
    private var titleLabel: NSTextField?
    private var flatButtons: [NSButton] = []
    private var detailScroll: NSScrollView?
    var maxEditorHeight: CGFloat = 200 { didSet { measure() } }
    var measuredHeight: CGFloat { fittingSize.height }
    init(block: [String: String], detail: Bool) {
        nodeID = block["node_uuid"] ?? ""; nodeTitle = block["title"] ?? "Node"; self.detail = detail
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        let title = NSTextField(labelWithString: detail ? "\(nodeTitle)\n\(nodeID)" : "\(nodeTitle) · \(nodeID)")
        titleLabel = title
        title.isSelectable = true; title.lineBreakMode = .byTruncatingMiddle
        title.maximumNumberOfLines = detail ? 2 : 1
        title.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        title.toolTip = title.stringValue
        let locate = NSButton(title: "Locate", target: self, action: #selector(locatePressed))
        let remove = NSButton(title: "×", target: self, action: #selector(removePressed))
        remove.setAccessibilityLabel("Remove instruction for \(nodeTitle)")
        flatButtons = [locate, remove]
        header.addArrangedSubview(title)
        if detail {
            header.addArrangedSubview(locate);header.addArrangedSubview(remove)
        } else {
            let actions=NSPopUpButton(frame:.zero,pullsDown:true);actions.addItem(withTitle:"⋯")
            actions.setAccessibilityLabel("Node actions")
            for (label,action) in [("Locate",#selector(locatePressed)),("Remove",#selector(removePressed))] {
                let item=NSMenuItem(title:label,action:action,keyEquivalent:"");item.target=self;actions.menu?.addItem(item)
            }
            actions.widthAnchor.constraint(equalToConstant:28).isActive=true;header.addArrangedSubview(actions)
        }
        header.orientation = .horizontal; header.spacing = 6
        editor.isRichText = false; editor.allowsUndo = true; editor.delegate = self
        editor.drawsBackground = false; editor.textContainer?.widthTracksTextView = true
        editor.isVerticallyResizable = detail; editor.isHorizontallyResizable = false
        editor.textContainer?.heightTracksTextView = false
        editor.font = .monospacedSystemFont(ofSize: 14, weight: .regular)
        editor.textContainerInset = NSSize(width: 6, height: 6)
        editor.isAutomaticQuoteSubstitutionEnabled = false; editor.isAutomaticDashSubstitutionEnabled = false
        editor.setAccessibilityLabel("Instruction for \(nodeTitle)")
        let input: NSView
        if detail {
            let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.documentView = editor
            editor.autoresizingMask = [.width]; detailScroll = scroll; input = scroll
        } else { input = editor }
        input.translatesAutoresizingMaskIntoConstraints = false
        editorHeight = input.heightAnchor.constraint(equalToConstant: detail ? 100 : 30)
        if detail {
            editorHeight.priority = .defaultLow
            input.heightAnchor.constraint(greaterThanOrEqualToConstant: 60).isActive = true
        }
        editorHeight.isActive = true
        let column = NSStackView(views: [header, input]); column.orientation = .vertical; column.alignment = .leading; column.spacing = 4
        if detail {
            let review = NSButton(title: "Review in Idea Bar", target: self, action: #selector(reviewPressed))
            let close = NSButton(title: "Close", target: self, action: #selector(closePressed))
            flatButtons += [review, close]
            let footer = NSStackView(views: [review, close]); footer.orientation = .horizontal
            footer.setContentHuggingPriority(.required, for: .vertical)
            column.addArrangedSubview(footer)
        }
        column.translatesAutoresizingMaskIntoConstraints = false; addSubview(column)
        NSLayoutConstraint.activate([
            column.leadingAnchor.constraint(equalTo: leadingAnchor), column.trailingAnchor.constraint(equalTo: trailingAnchor),
            column.topAnchor.constraint(equalTo: topAnchor), column.bottomAnchor.constraint(equalTo: bottomAnchor),
            header.widthAnchor.constraint(equalTo: column.widthAnchor), input.widthAnchor.constraint(equalTo: column.widthAnchor)
        ])
        if detail {
            title.font = .systemFont(ofSize: 11)
            header.setContentHuggingPriority(.required, for: .vertical)
            editor.font = .systemFont(ofSize: 14)
            editor.drawsBackground = false
            detailScroll?.drawsBackground = false
            detailScroll?.borderType = .noBorder
            for button in flatButtons {
                button.isBordered = false; button.font = .systemFont(ofSize: 11, weight: .medium)
            }
        }
        update(block)
    }
    required init?(coder: NSCoder) { nil }
    static func color(_ token: String?) -> NSColor? {
        guard let token else { return nil }
        let hex = token.hasPrefix("#") ? String(token.dropFirst()) : token
        guard hex.count == 8, let value = UInt64(hex, radix: 16) else { return nil }
        return NSColor(srgbRed: CGFloat((value >> 24) & 255) / 255,
                       green: CGFloat((value >> 16) & 255) / 255,
                       blue: CGFloat((value >> 8) & 255) / 255,
                       alpha: CGFloat(value & 255) / 255)
    }
    func applyStyle(_ style: [String: String]) {
        guard detail else { return }
        editor.textColor = Self.color(style["text"])
        editor.insertionPointColor = Self.color(style["text"]) ?? .labelColor
        titleLabel?.textColor = Self.color(style["muted"])
        if let selection = Self.color(style["selection"]) {
            editor.selectedTextAttributes = [.backgroundColor: selection]
        }
        for button in flatButtons { button.contentTintColor = Self.color(style["text"]) }
    }
    func update(_ block: [String: String]) {
        let incoming = block["text"] ?? ""
        if !editor.hasMarkedText(), editor.string != incoming {
            updating = true
            let selection = editor.selectedRange()
            editor.string = incoming
            editor.setSelectedRange(NSRange(location: min(selection.location, (incoming as NSString).length), length: 0))
            updating = false
        }
        measure()
    }
    private func measure() {
        editor.textContainer?.containerSize.height = .greatestFiniteMagnitude
        if let lm = editor.layoutManager, let tc = editor.textContainer {
            lm.ensureLayout(for: tc)
            let contentHeight = max(detail ? 100 : 30, lm.usedRect(for: tc).height + 12)
            let height = detail ? min(maxEditorHeight, contentHeight) : contentHeight
            if detail { editor.frame.size.height = max(contentHeight, detailScroll?.contentSize.height ?? 0) }
            if abs(editorHeight.constant - height) > 0.5 { editorHeight.constant = height }
        }
    }
    override func layout() { super.layout(); measure() }
    func textDidBeginEditing(_ notification: Notification) { onFocus?() }
    func textViewDidChangeSelection(_ notification: Notification) {
        if !updating { onFocus?() }
    }
    func textDidChange(_ notification: Notification) {
        onFocus?()
        measure()
        if !updating && !editor.hasMarkedText() { onChange?() }
    }
    func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        guard !textView.hasMarkedText() else { return false }
        if detail && selector == #selector(NSResponder.cancelOperation(_:)) { onClose?(); return true }
        if selector == #selector(NSResponder.insertNewline(_:)), NSApp.currentEvent?.modifierFlags.intersection([.command,.option,.control,.shift]) == [.command] {
            if detail { onReview?() } else { onSend?() }; return true
        }
        return false
    }
    @objc private func locatePressed() { onLocate?() }
    @objc private func removePressed() { onRemove?() }
    @objc private func reviewPressed() { onReview?() }
    @objc private func closePressed() { onClose?() }
}

#endif
