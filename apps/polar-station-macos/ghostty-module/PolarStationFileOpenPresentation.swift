import AppKit

/// Native presentation and OS opening only. Rust supplies every operation and
/// its enabled actions; this object does not resolve paths or manage resources.
@MainActor
final class PolarStationFileOpenPresentation: NSObject {
    weak var sourceWindow: NSWindow?
    var sendInput: (([String: Any]) -> Void)?
    private var requestID: String?
    private var observer: NSObjectProtocol?
    private var alert: NSAlert?
    private let progress = NSPopover()
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
    }

    deinit { if let observer { NotificationCenter.default.removeObserver(observer) } }

    func bind(to window: NSWindow) {
        dismiss()
        if let observer { NotificationCenter.default.removeObserver(observer) }
        sourceWindow = window
        observer = NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: window, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.cancelRequest() }
        }
    }

    @objc private func cancelRequest() {
        if let requestID { sendInput?(["type": "cancel_request", "request_id": requestID]) }
        dismiss()
    }

    private func dismiss() {
        progress.close()
        let old = alert
        alert = nil
        if let old, let parent = old.window.sheetParent { parent.endSheet(old.window, returnCode: .abort) }
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
            let accepted: Bool
            if effect["strategy"] as? String == "reveal" {
                NSWorkspace.shared.activateFileViewerSelecting([url]); accepted = true
            } else { accepted = NSWorkspace.shared.open(url) }
            sendInput?(["type": "viewer_finished", "request_id": id, "accepted": accepted])
            return true
        }
        dismiss()
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
        } else {
            sheet.messageText = "Could not open file"
            sheet.informativeText = effect["message"] as? String ?? ""
            sheet.addButton(withTitle: "Dismiss")
            if effect["can_retry"] as? Bool == true { sheet.addButton(withTitle: "Retry") }
            sheet.beginSheetModal(for: window) { [weak self] response in
                guard let self, self.requestID == id, self.alert === sheet else { return }
                self.alert = nil
                self.sendInput?(["type": response == .alertSecondButtonReturn ? "retry" : "cancel_request", "request_id": id])
            }
        }
        return true
    }
}
