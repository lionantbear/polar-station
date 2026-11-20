import AppKit

/// Native presentation and OS opening only. Rust supplies every operation and
/// its enabled actions; this object does not resolve paths or manage resources.
@MainActor
final class PolarStationFileOpenPresentation: NSObject {
    weak var sourceWindow: NSWindow?
    var sendInput: (([String: Any]) -> Void)?
    private var requestID: String?
    private var observer: NSObjectProtocol?
    private var resizeObserver: NSObjectProtocol?
    private var alert: NSAlert?
    private let progress = NSPopover()
    private let error = NSPopover()
    private let progressLabel = NSTextField(wrappingLabelWithString: "")

    override init() {
        super.init()
        let cancel = NSButton(title: "Cancel", target: self, action: #selector(cancelRequest))
        let spinner = NSProgressIndicator()
        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.startAnimation(nil)
        let row = NSStackView(views: [spinner, progressLabel, cancel])
        row.spacing = 10
        row.edgeInsets = NSEdgeInsets(top: 10, left: 12, bottom: 10, right: 12)
        let controller = NSViewController()
        controller.view = row
        controller.preferredContentSize = NSSize(width: 340, height: 56)
        progress.contentViewController = controller
        progress.behavior = .applicationDefined
        progress.animates = false
        error.behavior = .applicationDefined
        error.animates = false
    }

    deinit {
        if let observer { NotificationCenter.default.removeObserver(observer) }
        if let resizeObserver { NotificationCenter.default.removeObserver(resizeObserver) }
    }

    func bind(to window: NSWindow) {
        dismiss()
        if let observer { NotificationCenter.default.removeObserver(observer) }
        if let resizeObserver { NotificationCenter.default.removeObserver(resizeObserver) }
        sourceWindow = window
        observer = NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: window, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.cancelRequest() }
        }
        resizeObserver = NotificationCenter.default.addObserver(forName: NSWindow.didResizeNotification, object: window, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.resizeError() }
        }
    }

    @objc private func cancelRequest() {
        if let requestID { sendInput?(["type": "cancel_request", "request_id": requestID]) }
        dismiss()
    }

    private func dismiss() {
        progress.close()
        error.close()
        error.contentViewController = nil
        let old = alert
        alert = nil
        if let old, let parent = old.window.sheetParent { parent.endSheet(old.window, returnCode: .abort) }
    }

    private func resizeError() {
        guard let controller = error.contentViewController as? FileOpenErrorController,
              let frame = sourceWindow?.contentView?.bounds else { return }
        let size = NSSize(width: min(630, max(1, frame.width - 24)),
                          height: min(480, max(1, frame.height - 36)))
        controller.resize(to: size)
        error.contentSize = size
    }

    private func showError(_ effect: [String: Any], id: String, in window: NSWindow) {
        guard let view = window.contentView else { return }
        let controller = FileOpenErrorController(effect: effect) { [weak self] kind, text in
            guard let self, self.requestID == id else { return }
            if kind == "cancel_request" {
                self.cancelRequest()
                return
            }
            self.dismiss()
            var input: [String: Any] = ["type": kind, "request_id": id]
            if let text { input["text"] = text }
            self.sendInput?(input)
        }
        error.contentViewController = controller
        resizeError()
        let responder = window.firstResponder
        let wasKey = window.isKeyWindow
        error.show(relativeTo: NSRect(x: view.bounds.midX, y: view.bounds.maxY - 12, width: 1, height: 1),
                   of: view, preferredEdge: .minY)
        if wasKey, !window.isKeyWindow { window.makeKey() }
        if let responder, window.firstResponder !== responder { window.makeFirstResponder(responder) }
    }

    @discardableResult
    func present(_ effect: [String: Any]) -> Bool {
        let kind = effect["type"] as? String ?? ""
        guard ["show_progress", "dismiss_progress", "show_error", "choose_file", "confirm_mapping", "open_local_file"].contains(kind) else { return false }
        let id = effect["request_id"] as? String ?? ""
        if kind == "dismiss_progress" {
            if id == requestID { dismiss() }
            return true
        }
        requestID = id
        guard let window = sourceWindow else {
            sendInput?(["type": "cancel_request", "request_id": id])
            return true
        }
        if kind == "show_progress" {
            progressLabel.stringValue = effect["message"] as? String ?? ""
            if !progress.isShown, let view = window.contentView {
                progress.show(relativeTo: NSRect(x: view.bounds.midX, y: view.bounds.maxY - 12, width: 1, height: 1), of: view, preferredEdge: .minY)
            }
            return true
        }
        if kind == "open_local_file" {
            guard let path = effect["path"] as? String else { return true }
            let url = URL(fileURLWithPath: path)
            if effect["strategy"] as? String == "reveal" {
                NSWorkspace.shared.activateFileViewerSelecting([url])
                sendInput?(["type":"viewer_finished","request_id":id,"accepted":true])
            } else {
                NSWorkspace.shared.open(url,configuration:NSWorkspace.OpenConfiguration()) { [weak self] _, error in
                    let failure = error as NSError?
                    let underlying = failure?.userInfo[NSUnderlyingErrorKey] as? NSError
                    let message = [failure?.localizedDescription,underlying?.localizedDescription]
                        .compactMap{$0}.joined(separator:"\n")
                    Task { @MainActor [weak self] in
                        self?.sendInput?(["type":"viewer_finished","request_id":id,
                            "accepted":failure == nil,"message":message.isEmpty ? NSNull() : message as Any])
                    }
                }
            }
            return true
        }
        dismiss()
        if kind == "show_error" {
            showError(effect, id: id, in: window)
            return true
        }
        let sheet = NSAlert()
        alert = sheet
        if kind == "choose_file" {
            let source = effect["source"] as? [String: Any] ?? [:]
            let origin = source["origin"] as? [String: Any] ?? [:]
            sheet.messageText = "Choose a file"
            sheet.informativeText = "Several matching files exist on \(origin["execution_target_id"] as? String ?? "the source machine")."
            let picker = NSPopUpButton(frame: NSRect(x: 0, y: 0, width: 520, height: 28))
            for file in effect["candidates"] as? [[String: Any]] ?? [] { picker.addItem(withTitle: file["path"] as? String ?? "") }
            sheet.accessoryView = picker
            sheet.addButton(withTitle: "Open"); sheet.addButton(withTitle: "Cancel")
            sheet.beginSheetModal(for: window) { [weak self] response in
                guard let self, self.requestID == id, self.alert === sheet else { return }
                self.alert = nil
                self.sendInput?(["type": "choose", "request_id": id, "index": response == .alertFirstButtonReturn ? picker.indexOfSelectedItem as Any : NSNull()])
            }
        } else if kind == "confirm_mapping" {
            sheet.messageText = "Access files on \(effect["target"] as? String ?? "the source machine")"
            sheet.informativeText = "File: \(effect["path"] as? String ?? "")\nFolder: \(effect["directory"] as? String ?? "")\n\nNo existing file mapping covers this folder."
            sheet.addButton(withTitle: "Configure mapping"); sheet.addButton(withTitle: "Cancel")
            sheet.beginSheetModal(for: window) { [weak self] response in
                guard let self, self.requestID == id, self.alert === sheet else { return }
                self.alert = nil
                self.sendInput?(["type": "mapping_decision", "request_id": id, "confirmed": response == .alertFirstButtonReturn])
            }
        }
        return true
    }
}

@MainActor
private final class FileOpenErrorController: NSViewController {
    private let effect: [String: Any]
    private let action: (String, String?) -> Void
    private let editor = NSTextView.scrollableTextView()
    private let footer = NSStackView()
    private var tryButton: NSButton?

    init(effect: [String: Any], action: @escaping (String, String?) -> Void) {
        self.effect = effect
        self.action = action
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { return nil }

    func resize(to size: NSSize) {
        _ = view
        footer.orientation = size.width < 490 ? .vertical : .horizontal
        footer.alignment = size.width < 490 ? .leading : .centerY
        preferredContentSize = size
    }

    private func label(_ text: String, color: NSColor = .labelColor) -> NSTextField {
        let field = NSTextField(wrappingLabelWithString: text)
        field.textColor = color
        field.isSelectable = true
        field.lineBreakMode = .byCharWrapping
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return field
    }

    private func column(_ views: [NSView]) -> NSStackView {
        let stack = NSStackView(views: views)
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        return stack
    }

    override func loadView() {
        let root = NSView()
        let viewport = NSScrollView()
        viewport.hasVerticalScroller = true
        viewport.drawsBackground = false
        let heading = label("Could not open file")
        heading.font = .boldSystemFont(ofSize: 17)
        let summary = label(effect["summary"] as? String ?? "", color: .systemRed)
        let rows = (effect["steps"] as? [[String: Any]] ?? []).map { result in
            let title = result["title"] as? String ?? ""
            let status = result["summary"] as? String ?? ""
            let field = label(status, color: result["failed"] as? Bool == true ? .systemRed : .labelColor)
            let row = column([label(title), field])
            if let detail = result["details"] as? String, !detail.isEmpty {
                let content = label(detail, color: .secondaryLabelColor)
                content.isHidden = true
                let disclosure = NSButton(checkboxWithTitle: "Show \(title.lowercased()) details",
                                          target: self, action: #selector(toggleDetails(_:)))
                row.addArrangedSubview(disclosure)
                row.addArrangedSubview(content)
                content.widthAnchor.constraint(lessThanOrEqualTo: row.widthAnchor).isActive = true
            }
            field.widthAnchor.constraint(lessThanOrEqualTo: row.widthAnchor).isActive = true
            return row
        }
        editor.isHidden = true
        if let text = effect["edit_text"] as? String, let textView = editor.documentView as? NSTextView {
            textView.string = text
            textView.isRichText = false
            textView.font = .monospacedSystemFont(ofSize: 12, weight: .regular)
            textView.setAccessibilityLabel("Captured text")
        }
        let body = column([heading, summary] + rows + [editor])
        body.spacing = 10
        let document = NSView()
        document.addSubview(body)
        viewport.documentView = document
        for subview in [viewport, document, body, footer] { subview.translatesAutoresizingMaskIntoConstraints = false }
        for row in rows { row.widthAnchor.constraint(equalTo: body.widthAnchor).isActive = true }
        summary.widthAnchor.constraint(lessThanOrEqualTo: body.widthAnchor).isActive = true
        editor.widthAnchor.constraint(equalTo: body.widthAnchor).isActive = true
        editor.heightAnchor.constraint(equalToConstant: 96).isActive = true

        let copy = NSButton(title: "Copy details", target: self, action: #selector(copyDetails))
        let dismiss = NSButton(title: "Dismiss", target: self, action: #selector(dismissError))
        dismiss.keyEquivalent = "\u{1b}"
        let right = NSStackView(views: [dismiss])
        if effect["can_retry"] as? Bool == true {
            right.addArrangedSubview(NSButton(title: "Retry", target: self, action: #selector(retry)))
        }
        let left = NSStackView(views: [copy])
        if effect["edit_text"] is String {
            left.addArrangedSubview(NSButton(title: "Edit reference", target: self, action: #selector(edit)))
            let button = NSButton(title: "Try opening", target: self, action: #selector(openEditedReference))
            button.isHidden = true
            right.addArrangedSubview(button)
            tryButton = button
        }
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        footer.setViews([left, spacer, right], in: .leading)
        footer.spacing = 6
        root.addSubview(viewport)
        root.addSubview(footer)
        NSLayoutConstraint.activate([
            viewport.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            viewport.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            viewport.topAnchor.constraint(equalTo: root.topAnchor),
            viewport.bottomAnchor.constraint(equalTo: footer.topAnchor, constant: -8),
            document.leadingAnchor.constraint(equalTo: viewport.contentView.leadingAnchor),
            document.topAnchor.constraint(equalTo: viewport.contentView.topAnchor),
            document.widthAnchor.constraint(equalTo: viewport.contentView.widthAnchor),
            body.leadingAnchor.constraint(equalTo: document.leadingAnchor, constant: 12),
            body.trailingAnchor.constraint(equalTo: document.trailingAnchor, constant: -12),
            body.topAnchor.constraint(equalTo: document.topAnchor, constant: 12),
            body.bottomAnchor.constraint(equalTo: document.bottomAnchor, constant: -12),
            footer.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12),
            footer.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            footer.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -10),
        ])
        view = root
    }

    @objc private func toggleDetails(_ sender: NSButton) {
        guard let row = sender.superview as? NSStackView else { return }
        row.arrangedSubviews.last?.isHidden = sender.state != .on
    }

    @objc private func edit() {
        editor.isHidden = false
        tryButton?.isHidden = false
        guard let text = editor.documentView as? NSTextView else { return }
        if let range = effect["edit_range"] as? [String: Int],
           let start = range["start"], let end = range["end"], start >= 0, end >= start,
           let lower = text.string.utf8.index(text.string.utf8.startIndex, offsetBy: start, limitedBy: text.string.utf8.endIndex),
           let upper = text.string.utf8.index(text.string.utf8.startIndex, offsetBy: end, limitedBy: text.string.utf8.endIndex),
           let from = String.Index(lower, within: text.string), let to = String.Index(upper, within: text.string),
           text.string == effect["edit_text"] as? String {
            text.setSelectedRange(NSRange(from..<to, in: text.string))
        }
        text.window?.makeFirstResponder(text)
        view.layoutSubtreeIfNeeded()
        editor.superview?.scrollToVisible(editor.frame)
    }

    @objc private func copyDetails() {
        guard let details = effect["copy_details"] as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(details, forType: .string)
    }

    @objc private func dismissError() { action("cancel_request", nil) }
    @objc private func retry() { action("retry", nil) }
    @objc private func openEditedReference() {
        if let text = editor.documentView as? NSTextView { action("edit_reference", text.string) }
    }
}
