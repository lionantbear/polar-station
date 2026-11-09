import AppKit
import OSLog
import SwiftUI

private let paneMovePasteboardType = NSPasteboard.PasteboardType(
    "com.polarealm.polar-station.pane-move"
)
private let paneDragLogger = Logger(
    subsystem: "com.polarealm.polar-station",
    category: "PaneDrag"
)

private enum WorkspaceNativeDragPayload: Equatable {
    case pane(PaneTabMoveDragPayload)
    case tab(WorkspaceTabDragPayload)

    var pasteboardType: NSPasteboard.PasteboardType {
        switch self {
        case .pane: paneMovePasteboardType
        case .tab: .init("com.polarealm.polar-station.tab-move")
        }
    }

    func encoded() throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        switch self {
        case .pane(let payload): return try encoder.encode(payload)
        case .tab(let payload): return try encoder.encode(payload)
        }
    }
}

@MainActor
private final class WorkspaceNativeDragCoordinator {
    static let shared = WorkspaceNativeDragCoordinator()

    private(set) var activePayload: WorkspaceNativeDragPayload?
    private weak var hoveredTarget: PaneDropObservationView?
    private weak var lastAcceptedTarget: PaneDropObservationView?
    private let targets = NSHashTable<PaneDropObservationView>.weakObjects()

    func register(_ target: PaneDropObservationView) {
        targets.add(target)
        if let activePayload { target.registerForDraggedTypes([activePayload.pasteboardType]) }
    }

    private func end() {
        activePayload = nil
        hoveredTarget = nil
        lastAcceptedTarget = nil
        for target in targets.allObjects {
            target.unregisterDraggedTypes()
            target.clearDropIndicator()
        }
    }

    func begin(_ payload: WorkspaceNativeDragPayload) {
        activePayload = payload
        hoveredTarget = nil
        lastAcceptedTarget = nil
        for target in targets.allObjects { target.registerForDraggedTypes([payload.pasteboardType]) }
    }

    func hover(
        _ target: PaneDropObservationView,
        payload: WorkspaceNativeDragPayload
    ) {
        guard activePayload == payload else { return }
        hoveredTarget = target
        lastAcceptedTarget = target
    }

    func exit(_ target: PaneDropObservationView) {
        guard hoveredTarget === target else { return }
        hoveredTarget = nil
    }

    func finish(
        _ payload: WorkspaceNativeDragPayload,
        at screenPoint: NSPoint,
        operation: NSDragOperation
    ) {
        defer { end() }
        guard operation.contains(.move), activePayload == payload,
              let target = hoveredTarget ?? lastAcceptedTarget
        else {
            paneDragLogger.notice("finish rejected missing payload or target")
            return
        }
        guard let window = target.window else {
            paneDragLogger.notice("finish rejected missing target window")
            return
        }
        let containsEndpoint = target.contains(screenPoint, in: window)
        let acceptsPayload = target.accepts(payload)
        paneDragLogger.notice(
            "finish endpointInside=\(containsEndpoint, privacy: .public) accepted=\(acceptsPayload, privacy: .public) visualFrame=\(target.visualFrame.minX, privacy: .public),\(target.visualFrame.minY, privacy: .public),\(target.visualFrame.maxX, privacy: .public),\(target.visualFrame.maxY, privacy: .public)"
        )
        guard containsEndpoint, acceptsPayload else { return }
        paneDragLogger.notice("finish accepted")
        target.perform(payload)
    }

    func consume(_ payload: WorkspaceNativeDragPayload) -> Bool {
        guard activePayload == payload else { return false }
        end()
        return true
    }

    func cancel(_ payload: WorkspaceNativeDragPayload) {
        guard activePayload == payload else { return }
        end()
    }
}

public enum PaneClickArbitrationEffect: Equatable, Sendable {
    case scheduleSelection
    case cancelSelection
    case activate
}

public struct PaneClickArbitrator: Equatable, Sendable {
    private var pressedInside = false
    private var dragged = false

    public init() {}

    public mutating func mouseDown(
        clickCount: Int,
        inside: Bool
    ) -> [PaneClickArbitrationEffect] {
        pressedInside = inside
        dragged = false
        return clickCount >= 2 && inside ? [.cancelSelection] : []
    }

    public mutating func mouseDragged() -> [PaneClickArbitrationEffect] {
        guard pressedInside else { return [] }
        dragged = true
        return [.cancelSelection]
    }

    public mutating func mouseUp(
        clickCount: Int,
        inside: Bool
    ) -> [PaneClickArbitrationEffect] {
        defer {
            pressedInside = false
            dragged = false
        }
        guard pressedInside, inside, !dragged else { return [] }
        if clickCount >= 2 {
            return [.cancelSelection, .activate]
        }
        return [.scheduleSelection]
    }
}

public struct PaneClickArbitrationView: NSViewRepresentable {
    private let visualFrame: CGRect
    private let dragPayload: PaneTabMoveDragPayload
    private let onSelect: @MainActor () -> Void
    private let onActivate: @MainActor () -> Void

    public init(
        visualFrame: CGRect,
        dragPayload: PaneTabMoveDragPayload,
        onSelect: @escaping @MainActor () -> Void,
        onActivate: @escaping @MainActor () -> Void
    ) {
        self.visualFrame = visualFrame
        self.dragPayload = dragPayload
        self.onSelect = onSelect
        self.onActivate = onActivate
    }

    public func makeNSView(context: Context) -> PaneClickObservationView {
        PaneClickObservationView(
            visualFrame: visualFrame,
            dragPayload: .pane(dragPayload),
            onSelect: onSelect,
            onActivate: onActivate
        )
    }

    public func updateNSView(
        _ view: PaneClickObservationView,
        context: Context
    ) {
        view.visualFrame = visualFrame
        view.dragPayload = .pane(dragPayload)
        view.onSelect = onSelect
        view.onActivate = onActivate
    }
}

@MainActor
public final class PaneClickObservationView: NSView, NSDraggingSource {
    fileprivate var visualFrame: CGRect
    fileprivate var dragPayload: WorkspaceNativeDragPayload
    fileprivate var onSelect: @MainActor () -> Void
    fileprivate var onActivate: @MainActor () -> Void

    private var arbitrator = PaneClickArbitrator()
    private var eventMonitor: Any?
    private var pendingSelection: DispatchWorkItem?
    private var pressPoint: NSPoint?
    private var dragStarted = false

    fileprivate init(
        visualFrame: CGRect,
        dragPayload: WorkspaceNativeDragPayload,
        onSelect: @escaping @MainActor () -> Void,
        onActivate: @escaping @MainActor () -> Void
    ) {
        self.visualFrame = visualFrame
        self.dragPayload = dragPayload
        self.onSelect = onSelect
        self.onActivate = onActivate
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    public override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window == nil {
            stopMonitoring()
        } else {
            startMonitoring()
        }
    }

    public override func hitTest(_ point: NSPoint) -> NSView? {
        nil
    }

    public func draggingSession(
        _ session: NSDraggingSession,
        sourceOperationMaskFor context: NSDraggingContext
    ) -> NSDragOperation {
        .move
    }

    public func draggingSession(
        _ session: NSDraggingSession,
        endedAt screenPoint: NSPoint,
        operation: NSDragOperation
    ) {
        paneDragLogger.notice(
            "source ended operation=\(operation.rawValue, privacy: .public) point=\(screenPoint.x, privacy: .public),\(screenPoint.y, privacy: .public)"
        )
        WorkspaceNativeDragCoordinator.shared.finish(
            dragPayload,
            at: screenPoint,
            operation: operation
        )
    }

    private func startMonitoring() {
        guard eventMonitor == nil else { return }
        eventMonitor = NSEvent.addLocalMonitorForEvents(
            matching: [.leftMouseDown, .leftMouseDragged, .leftMouseUp, .keyDown]
        ) { [weak self] event in
            self?.observe(event)
            return event
        }
    }

    private func stopMonitoring() {
        pendingSelection?.cancel()
        pendingSelection = nil
        if let eventMonitor {
            NSEvent.removeMonitor(eventMonitor)
            self.eventMonitor = nil
        }
    }

    private func observe(_ event: NSEvent) {
        if event.type == .keyDown, event.keyCode == 53 {
            WorkspaceNativeDragCoordinator.shared.cancel(dragPayload)
            pendingSelection?.cancel()
            pressPoint = nil
            return
        }
        guard let window, event.window === window else { return }
        guard let contentView = window.contentView else { return }
        let pointInContent = contentView.convert(
            event.locationInWindow,
            from: nil
        )
        let visualPoint = contentView.isFlipped
            ? pointInContent
            : CGPoint(
                x: pointInContent.x,
                y: contentView.bounds.height - pointInContent.y
            )
        let localPoint = convert(event.locationInWindow, from: nil)
        let inside = visualFrame.contains(visualPoint)
            && !isRemoveButtonArea(visualPoint)
        if event.type == .leftMouseDown {
            paneDragLogger.notice(
                "source mouseDown point=\(visualPoint.x, privacy: .public),\(visualPoint.y, privacy: .public) inside=\(inside, privacy: .public) frame=\(self.visualFrame.minX, privacy: .public),\(self.visualFrame.minY, privacy: .public),\(self.visualFrame.maxX, privacy: .public),\(self.visualFrame.maxY, privacy: .public)"
            )
        }
        let effects: [PaneClickArbitrationEffect]
        switch event.type {
        case .leftMouseDown:
            pressPoint = inside ? visualPoint : nil
            dragStarted = false
            effects = arbitrator.mouseDown(
                clickCount: event.clickCount,
                inside: inside
            )
        case .leftMouseDragged:
            // Trackpad jitter below the existing drag threshold remains a click.
            guard dragStarted || shouldStartDrag(at: visualPoint) else { return }
            effects = arbitrator.mouseDragged()
            if shouldStartDrag(at: visualPoint) {
                startDrag(with: event, at: localPoint)
            }
        case .leftMouseUp:
            effects = arbitrator.mouseUp(
                clickCount: event.clickCount,
                inside: inside
            )
            pressPoint = nil
            dragStarted = false
        default:
            return
        }
        effects.forEach(apply)
    }

    private func shouldStartDrag(at point: NSPoint) -> Bool {
        guard !dragStarted, let pressPoint else { return false }
        return hypot(point.x - pressPoint.x, point.y - pressPoint.y) >= 4
    }

    private func startDrag(with event: NSEvent, at point: NSPoint) {
        dragStarted = true
        paneDragLogger.notice("source began")
        WorkspaceNativeDragCoordinator.shared.begin(dragPayload)
        guard let data = try? dragPayload.encoded() else {
            WorkspaceNativeDragCoordinator.shared.cancel(dragPayload)
            return
        }

        let pasteboardItem = NSPasteboardItem()
        pasteboardItem.setData(
            data,
            forType: dragPayload.pasteboardType
        )
        let imageSize = NSSize(width: 72, height: 32)
        let dragImage = NSImage(
            size: imageSize,
            flipped: false
        ) { rect in
            NSColor.controlAccentColor.withAlphaComponent(0.28).setFill()
            NSBezierPath(
                roundedRect: rect.insetBy(dx: 1, dy: 1),
                xRadius: 7,
                yRadius: 7
            ).fill()
            return true
        }
        let item = NSDraggingItem(pasteboardWriter: pasteboardItem)
        item.setDraggingFrame(
            NSRect(
                x: point.x - imageSize.width / 2,
                y: point.y - imageSize.height / 2,
                width: imageSize.width,
                height: imageSize.height
            ),
            contents: dragImage
        )
        let session = beginDraggingSession(
            with: [item],
            event: event,
            source: self
        )
        session.animatesToStartingPositionsOnCancelOrFail = false
    }

    private func isRemoveButtonArea(_ point: NSPoint) -> Bool {
        guard case .pane = dragPayload else { return false }
        let actionSide: CGFloat = 36
        return point.x >= visualFrame.maxX - actionSide
            && point.y <= visualFrame.minY + actionSide
    }

    private func apply(_ effect: PaneClickArbitrationEffect) {
        switch effect {
        case .scheduleSelection:
            if case .tab = dragPayload { onSelect(); return }
            pendingSelection?.cancel()
            let item = DispatchWorkItem { [weak self] in
                self?.pendingSelection = nil
                self?.onSelect()
            }
            pendingSelection = item
            DispatchQueue.main.asyncAfter(
                deadline: .now() + NSEvent.doubleClickInterval,
                execute: item
            )
        case .cancelSelection:
            pendingSelection?.cancel()
            pendingSelection = nil
        case .activate:
            onActivate()
        }
    }
}

public struct TabDragSourceView: NSViewRepresentable {
    let visualFrame: CGRect
    let payload: WorkspaceTabDragPayload
    let onSelect: @MainActor () -> Void

    public func makeNSView(context: Context) -> PaneClickObservationView {
        PaneClickObservationView(visualFrame: visualFrame, dragPayload: .tab(payload), onSelect: onSelect, onActivate: onSelect)
    }

    public func updateNSView(_ view: PaneClickObservationView, context: Context) {
        view.visualFrame = visualFrame
        view.dragPayload = .tab(payload)
        view.onSelect = onSelect
        view.onActivate = onSelect
    }
}

public struct PaneDropDestinationView: NSViewRepresentable {
    private let visualFrame: CGRect
    private let canAccept: @MainActor (PaneTabMoveDragPayload) -> Bool
    private let onDrop: @MainActor (PaneTabMoveDragPayload, Bool) -> Void
    private let canAcceptTab: (@MainActor (WorkspaceTabDragPayload) -> Bool)?
    private let onTabDrop: (@MainActor (WorkspaceTabDragPayload, Bool) -> Void)?

    public init(
        visualFrame: CGRect,
        canAccept: @escaping @MainActor (PaneTabMoveDragPayload) -> Bool,
        onDrop: @escaping @MainActor (PaneTabMoveDragPayload, Bool) -> Void,
        canAcceptTab: (@MainActor (WorkspaceTabDragPayload) -> Bool)? = nil,
        onTabDrop: (@MainActor (WorkspaceTabDragPayload, Bool) -> Void)? = nil
    ) {
        self.visualFrame = visualFrame
        self.canAccept = canAccept
        self.onDrop = onDrop
        self.canAcceptTab = canAcceptTab
        self.onTabDrop = onTabDrop
    }

    public func makeNSView(context: Context) -> PaneDropObservationView {
        PaneDropObservationView(
            visualFrame: visualFrame,
            canAccept: canAccept,
            onDrop: onDrop,
            canAcceptTab: canAcceptTab,
            onTabDrop: onTabDrop
        )
    }

    public func updateNSView(
        _ view: PaneDropObservationView,
        context: Context
    ) {
        view.visualFrame = visualFrame
        view.canAccept = canAccept
        view.onDrop = onDrop
        view.canAcceptTab = canAcceptTab
        view.onTabDrop = onTabDrop
    }
}

@MainActor
public final class PaneDropObservationView: NSView {
    fileprivate var visualFrame: CGRect
    fileprivate var canAccept: @MainActor (PaneTabMoveDragPayload) -> Bool
    fileprivate var onDrop: @MainActor (PaneTabMoveDragPayload, Bool) -> Void
    fileprivate var canAcceptTab: (@MainActor (WorkspaceTabDragPayload) -> Bool)?
    fileprivate var onTabDrop: (@MainActor (WorkspaceTabDragPayload, Bool) -> Void)?

    fileprivate init(
        visualFrame: CGRect,
        canAccept: @escaping @MainActor (PaneTabMoveDragPayload) -> Bool,
        onDrop: @escaping @MainActor (PaneTabMoveDragPayload, Bool) -> Void,
        canAcceptTab: (@MainActor (WorkspaceTabDragPayload) -> Bool)?,
        onTabDrop: (@MainActor (WorkspaceTabDragPayload, Bool) -> Void)?
    ) {
        self.visualFrame = visualFrame
        self.canAccept = canAccept
        self.onDrop = onDrop
        self.canAcceptTab = canAcceptTab
        self.onTabDrop = onTabDrop
        super.init(frame: .zero)
        WorkspaceNativeDragCoordinator.shared.register(self)
        wantsLayer = true
        paneDragLogger.notice("destination registered")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    public override func hitTest(_ point: NSPoint) -> NSView? {
        guard let payload = WorkspaceNativeDragCoordinator.shared.activePayload,
              accepts(payload) else { return nil }
        return self
    }

    public override func draggingEntered(
        _ sender: any NSDraggingInfo
    ) -> NSDragOperation {
        let payload = acceptedPayload(from: sender)
        let accepted = payload != nil
        if let payload {
            WorkspaceNativeDragCoordinator.shared.hover(self, payload: payload)
            updateDropIndicator(payload)
        }
        paneDragLogger.notice(
            "destination entered accepted=\(accepted, privacy: .public)"
        )
        return accepted ? .move : []
    }

    public override func draggingUpdated(
        _ sender: any NSDraggingInfo
    ) -> NSDragOperation {
        guard let payload = acceptedPayload(from: sender) else { return [] }
        WorkspaceNativeDragCoordinator.shared.hover(self, payload: payload)
        updateDropIndicator(payload)
        return .move
    }

    public override func draggingExited(_ sender: (any NSDraggingInfo)?) {
        clearDropIndicator()
        WorkspaceNativeDragCoordinator.shared.exit(self)
    }

    public override func prepareForDragOperation(
        _ sender: any NSDraggingInfo
    ) -> Bool {
        let accepted = acceptedPayload(from: sender) != nil
        paneDragLogger.notice(
            "destination prepare accepted=\(accepted, privacy: .public)"
        )
        return accepted
    }

    public override func performDragOperation(
        _ sender: any NSDraggingInfo
    ) -> Bool {
        guard let payload = acceptedPayload(from: sender),
              WorkspaceNativeDragCoordinator.shared.consume(payload)
        else {
            paneDragLogger.notice("destination perform rejected")
            return false
        }
        paneDragLogger.notice("destination perform accepted")
        perform(payload)
        return true
    }

    fileprivate func accepts(_ payload: WorkspaceNativeDragPayload) -> Bool {
        switch payload {
        case .pane(let pane): canAccept(pane)
        case .tab(let tab): onTabDrop != nil && canAcceptTab?(tab) == true
        }
    }

    fileprivate func contains(
        _ screenPoint: NSPoint,
        in window: NSWindow
    ) -> Bool {
        guard let contentView = window.contentView else { return false }
        let pointInWindow = window.convertPoint(fromScreen: screenPoint)
        let pointInContent = contentView.convert(pointInWindow, from: nil)
        let visualPoint = contentView.isFlipped
            ? pointInContent
            : CGPoint(
                x: pointInContent.x,
                y: contentView.bounds.height - pointInContent.y
            )
        return visualFrame.contains(visualPoint)
    }

    private var insertionLine: CALayer?

    fileprivate func clearDropIndicator() {
        layer?.borderWidth = 0
        insertionLine?.removeFromSuperlayer()
        insertionLine = nil
    }

    private func updateDropIndicator(_ payload: WorkspaceNativeDragPayload) {
        clearDropIndicator()
        guard case .tab = payload else {
            layer?.borderColor = NSColor.controlAccentColor.cgColor
            layer?.borderWidth = 2
            return
        }
        let point = window.map { convert($0.convertPoint(fromScreen: NSEvent.mouseLocation), from: nil) } ?? .zero
        let line = CALayer()
        line.backgroundColor = NSColor.controlAccentColor.cgColor
        line.frame = CGRect(x: 0, y: point.y >= bounds.height / 2 ? bounds.height - 3 : 0, width: bounds.width, height: 3)
        layer?.addSublayer(line)
        insertionLine = line
    }

    fileprivate func perform(_ payload: WorkspaceNativeDragPayload) {
        clearDropIndicator()
        if case .tab(let tab) = payload {
            let point = window.map { convert($0.convertPoint(fromScreen: NSEvent.mouseLocation), from: nil) } ?? .zero
            let y = isFlipped ? point.y : bounds.height - point.y
            onTabDrop?(tab, y >= bounds.height / 2); return
        }
        guard case .pane(let pane) = payload else { return }
        let point = window.map { convert($0.convertPoint(fromScreen: NSEvent.mouseLocation), from: nil) } ?? .zero
        let x = point.x / max(bounds.width, 1)
        let y = (isFlipped ? point.y : bounds.height - point.y) / max(bounds.height, 1)
        let after = min(1 - x, 1 - y) < min(x, y)
        onDrop(pane, after)
    }

    private func acceptedPayload(from sender: any NSDraggingInfo) -> WorkspaceNativeDragPayload? {
        guard let active = WorkspaceNativeDragCoordinator.shared.activePayload,
              let data = sender.draggingPasteboard.data(forType: active.pasteboardType),
              let expected = try? active.encoded(), data == expected,
              accepts(active) else { return nil }
        return active
    }
}
