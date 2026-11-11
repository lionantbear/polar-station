#if POLAR_STATION
import AppKit
import CPolarStationDesktopClient
import GhosttyKit

/// Owns a real processless Ghostty NSView and the shared C revision guard.
/// No terminal Session/command is synthesized. The existing native layout
/// executor may place `view` in a content slot; it remains that executor's job.
@MainActor
final class PolarStationDisplaySurface: NSObject {
    enum Failure: Error {
        case surfaceCreation
        case admission(ps_display_status)
        case closed
        case contentIdentityRequiresNewSurface
    }

    let view: Ghostty.SurfaceView
    private var graphicsView: PolarStationGraphicsView?
    let generation: UInt64
    var geometryChanged: (() -> Void)?
    var readingAction: ((Bool) -> Void)?
    var referenceAction: ((UInt64, Int?, Int) -> Void)?
    private var actionRows: [Int: Int] = [:]
    private var actionRevision: UInt64 = 0
    private var pendingClick: DispatchWorkItem?
    private(set) var pointerDown = false
    var pointerPending: Bool { pointerDown || pendingClick != nil }
    private let returnButton = NSButton(title: "Return to reading", target: nil, action: nil)
    private var readingRetry = false
    private weak var readingContainer: NSView?
    private let readingButton = NSButton(title: "Back to latest", target: nil, action: nil)
    private let storage: Storage

    init(app: ghostty_app_t, fontSize: Float32, generation: UInt64) throws {
        precondition(Thread.isMainThread)
        guard generation != 0 else { throw Failure.admission(PS_DISPLAY_INVALID) }
        var configuration = Ghostty.SurfaceConfiguration()
        configuration.fontSize = fontSize
        // Constructor selects display mode BEFORE Termio chooses its backend.
        // In particular, do not create a pane-host and later replace its text.
        let view = Ghostty.SurfaceView(
            app, baseConfig: configuration, stationDisplayOnly: true
        )
        guard view.surface != nil else { throw Failure.surfaceCreation }
        var config = ps_display_config(
            abi_version: PS_DISPLAY_ABI_VERSION,
            surface_generation: generation,
            max_bytes: Int(PS_DISPLAY_MAX_BYTES),
            sink: { context, operation, text, count in
                PolarStationDisplaySurface.consume(
                    context: context, operation: operation, text: text, count: count
                )
            },
            context: Unmanaged.passUnretained(view).toOpaque()
        )
        var pointer: OpaquePointer?
        let result = ps_display_create(&config, &pointer)
        guard result == PS_DISPLAY_OK, let pointer else {
            throw Failure.admission(result)
        }
        view.isHidden = true
        self.view = view
        self.generation = generation
        self.storage = Storage(pointer)
        super.init()
        returnButton.target = self
        returnButton.action = #selector(returnClicked)
        returnButton.bezelStyle = .rounded
        returnButton.isHidden = true
        returnButton.translatesAutoresizingMaskIntoConstraints = false
        readingButton.target = self
        readingButton.action = #selector(readingClicked)
        readingButton.bezelStyle = .rounded
        readingButton.isHidden = true
        readingButton.translatesAutoresizingMaskIntoConstraints = false
        NotificationCenter.default.addObserver(self, selector: #selector(frameChanged), name: .ghosttyDidUpdateScrollbar, object: view)
        view.postsFrameChangedNotifications = true
        NotificationCenter.default.addObserver(self, selector: #selector(frameChanged), name: NSView.frameDidChangeNotification, object: view)
    }

    func setIdeaPresentation(scope: String, visible: Bool) { graphicsView?.setPresentation(scope: scope, visible: visible) }
    func subscribeIdea(_ callback: (([String: Any]) -> Void)?) { graphicsView?.hostEvent = callback }
    func ideaCommand(_ action: String, query: String) { graphicsView?.sendCommand(action, query: query) }

    func mountIdeaTree(viewID: String) throws {
        guard let graphics = try PolarStationGraphicsView(viewID: viewID) else { throw Failure.surfaceCreation }
        graphics.focusOwner = { [weak view] in
            guard let view else { return }
            view.focusDidChange(true)
            NotificationCenter.default.post(name: .init("PolarStationTerminalFocused"), object: view)
        }
        graphics.blurOwner = { [weak view] in view?.focusDidChange(false) }
        graphics.frame = view.bounds
        graphics.autoresizingMask = [.width, .height]
        view.addSubview(graphics)
        graphicsView = graphics
        NotificationCenter.default.addObserver(self, selector: #selector(focusGraphics(_:)), name: .init("PolarStationTerminalFocused"), object: view)
    }

    @objc private func focusGraphics(_ note: Notification) {
        guard let graphics = graphicsView, view.window?.firstResponder === view else { return }
        view.window?.makeFirstResponder(graphics)
    }

    private func attachReadingButton() {
        guard storage.pointer != nil else { return }
        var ancestor = view.superview
        while let container = ancestor {
            if container is SurfaceScrollView {
                guard readingContainer !== container else { return }
                readingButton.removeFromSuperview()
                returnButton.removeFromSuperview()
                container.addSubview(readingButton)
                container.addSubview(returnButton)
                readingContainer = container
                NSLayoutConstraint.activate([
                    returnButton.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -24),
                    returnButton.topAnchor.constraint(equalTo: container.topAnchor, constant: 4),
                    readingButton.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -24),
                    readingButton.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -12),
                    readingButton.leadingAnchor.constraint(greaterThanOrEqualTo: container.leadingAnchor, constant: 8)
                ])
                return
            }
            ancestor = container.superview
        }
    }
    func updateReadingHint(_ label: String, visible: Bool, retry: Bool) {
        attachReadingButton()
        readingRetry = retry
        readingButton.title = label
        readingButton.setAccessibilityLabel(label)
        readingButton.isHidden = !visible
    }
    @objc private func readingClicked() { readingAction?(readingRetry) }
    func scrollToLatest() {
        guard let surface = view.surface else { return }
        let action = "scroll_to_bottom"
        _ = ghostty_surface_binding_action(surface, action, UInt(action.utf8.count))
        frameChanged()
    }
    func scrollToRow(_ row: UInt64) {
        guard let surface = view.surface else { return }
        let action = "scroll_to_row:\(row)"
        _ = ghostty_surface_binding_action(surface, action, UInt(action.utf8.count))
        frameChanged()
    }
    @objc private func returnClicked() { referenceAction?(actionRevision, nil, 2) }
    func updateReferences(_ items: [[String: Any]], revision: UInt64, canReturn: Bool) {
        actionRevision = revision
        actionRows = Dictionary(items.compactMap { item -> (Int, Int)? in
            guard let row = item["row"] as? Int, let end = item["column_end"] as? Int else { return nil }; return (row,end)
        }, uniquingKeysWith: { first, _ in first })
        attachReadingButton()
        returnButton.isHidden = !canReturn
    }
    func beginPointer() {
        pendingClick?.cancel(); pendingClick = nil; pointerDown = true
        geometryChanged?()
    }
    func cancelPointer() {
        pendingClick?.cancel(); pendingClick = nil; pointerDown = false
        geometryChanged?()
    }
    @discardableResult
    func endPointer(at location: NSPoint, single: Bool) -> Bool {
        pointerDown = false
        guard single, let surface = view.surface, let snapshot = try? snapshot(),
              snapshot.revision == actionRevision, let scrollbar = view.scrollbar else { geometryChanged?(); return false }
        var left = 0.0, top = 0.0, cw = 0.0, ch = 0.0
        guard ghostty_surface_display_geometry(surface, &left, &top, &cw, &ch), cw > 0, ch > 0 else { return false }
        let point = view.convert(location, from: nil)
        let y = view.isFlipped ? point.y : view.bounds.height - point.y
        guard point.x >= left + cw * 2, point.x < view.bounds.width, y >= top, y < view.bounds.height else { return false }
        let row = Int(floor((y - top) / ch)) + Int(scrollbar.offset)
        let column = Int(floor((point.x - left) / cw))
        guard let end = actionRows[row], column < end else { return false }
        let revision = actionRevision
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.pendingClick = nil
            defer { self.geometryChanged?() }
            guard self.view.window?.isKeyWindow == true,
                  let surface = self.view.surface, !ghostty_surface_has_selection(surface) else { return }
            self.referenceAction?(revision, row, column)
        }
        pendingClick = work
        geometryChanged?()
        DispatchQueue.main.asyncAfter(deadline: .now() + NSEvent.doubleClickInterval, execute: work)
        return true
    }
    @objc private func frameChanged() {
        DispatchQueue.main.async { [weak self] in
            self?.attachReadingButton()
            self?.geometryChanged?()
        }
    }
    deinit { NotificationCenter.default.removeObserver(self) }

    /// One content identity per physical Surface. A parser ACK is not a
    /// GPU-present ACK: do not unhide a recycled texture under another identity.
    /// Layout creates a new Surface on identity change, preserving unchanged ones.
    /// The first clear returns revision 1, not a fictitious 0.
    @discardableResult
    func bind(contentEpoch: UInt64) throws -> ps_display_snapshot {
        let handle = try handle()
        guard try snapshot().content_epoch == 0 else {
            throw Failure.contentIdentityRequiresNewSurface
        }
        try requireOK(ps_display_bind(handle, contentEpoch))
        view.isHidden = true
        try submit(contentEpoch: contentEpoch, baseRevision: 0, nextRevision: 1,
                   operation: PS_DISPLAY_REPLACE_WINDOW, text: "")
        view.isHidden = false
        return try snapshot()
    }

    func submit(contentEpoch: UInt64, baseRevision: UInt64, nextRevision: UInt64,
                operation: ps_display_operation, text: String) throws {
        let handle = try handle()
        // withUTF8 lends a stable byte span only for this synchronous call.
        var text = text
        try text.withUTF8 { bytes in
            var batch = ps_display_batch(
                surface_generation: generation,
                content_epoch: contentEpoch,
                base_revision: baseRevision,
                next_revision: nextRevision,
                operation: operation.rawValue,
                utf8: bytes.baseAddress,
                byte_count: bytes.count
            )
            try requireOK(ps_display_submit(handle, &batch))
        }
    }

    func snapshot() throws -> ps_display_snapshot {
        var result = ps_display_snapshot()
        try requireOK(ps_display_read(try handle(), &result))
        return result
    }

    /// Retire before detaching a physical Surface. Late async view updates can
    /// still hold this controller but are refused after retirement.
    func close() throws {
        graphicsView?.retire()
        graphicsView?.removeFromSuperview()
        graphicsView = nil
        geometryChanged = nil
        readingAction = nil
        referenceAction = nil
        pendingClick?.cancel(); pendingClick = nil
        returnButton.removeFromSuperview()
        readingButton.removeFromSuperview()
        guard let pointer = storage.pointer else { return }
        view.isHidden = true
        try requireOK(ps_display_destroy(pointer))
        storage.pointer = nil
    }

    private func handle() throws -> OpaquePointer {
        guard let pointer = storage.pointer else { throw Failure.closed }
        return pointer
    }

    private func requireOK(_ status: ps_display_status) throws {
        if status != PS_DISPLAY_OK { throw Failure.admission(status) }
    }

    private nonisolated static func consume(
        context: UnsafeMutableRawPointer?, operation: UInt32,
        text: UnsafePointer<UInt8>?, count: Int
    ) -> Int32 {
        guard Thread.isMainThread, let context else { return 1 }
        return MainActor.assumeIsolated {
            let view = Unmanaged<Ghostty.SurfaceView>.fromOpaque(context)
                .takeUnretainedValue()
            guard let surface = view.surface else { return 1 }
            return ghostty_surface_display_text(surface, operation, text, UInt(count)) ? 0 : 1
        }
    }

    // Only the main-actor controller accesses `pointer` while live. Destruction
    // may be initiated by ARC elsewhere; schedule the final C free on its
    // creator thread. The sink is never called by ps_display_destroy, so it
    // cannot dereference an NSView that has already been released.
    private final class Storage: @unchecked Sendable {
        var pointer: OpaquePointer?
        init(_ pointer: OpaquePointer) { self.pointer = pointer }
        deinit {
            guard let pointer else { return }
            let address = UInt(bitPattern: pointer)
            if Thread.isMainThread {
                _ = ps_display_destroy(pointer)
            } else {
                DispatchQueue.main.async {
                    if let value = OpaquePointer(bitPattern: address) {
                        _ = ps_display_destroy(value)
                    }
                }
            }
        }
    }
}

#endif
