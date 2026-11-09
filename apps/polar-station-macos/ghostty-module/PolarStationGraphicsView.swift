#if POLAR_STATION
import AppKit
import OpenGL.GL3

/// AppKit presentation only. Godot owns the scene and all navigation semantics.
/// One engine context survives Pane retirement; each drawable shares its textures.
@MainActor
final class PolarStationGraphicsEngine {
    static let shared = PolarStationGraphicsEngine()
    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }
    typealias Start = @convention(c) (Int32, UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?, Int32, Int32, UInt32) -> Int32
    typealias Create = @convention(c) (UnsafePointer<CChar>?, UnsafePointer<CChar>?) -> UInt64
    typealias HostEvent = @convention(c) (UnsafePointer<CChar>?, UnsafeMutableRawPointer?) -> Void
    typealias Subscribe = @convention(c) (UInt64, HostEvent?, UnsafeMutableRawPointer?) -> Void
    typealias Command = @convention(c) (UInt64, UnsafePointer<CChar>?, UnsafePointer<CChar>?) -> Void
    typealias Configure = @convention(c) (UInt64, Int32, Int32, Float, Int32) -> Void
    typealias ViewFlag = @convention(c) (UInt64) -> Int32
    typealias ViewAction = @convention(c) (UInt64) -> Void
    typealias Step = @convention(c) (UInt32) -> Int32
    typealias Present = @convention(c) (UInt64, Int32, Int32, UInt32, UInt32) -> Int32
    typealias Pointer = @convention(c) (Int32, Double, Double, Double, Double, Int32, Int32, UInt32, UInt32, Int32) -> Void
    typealias Magnify = @convention(c) (Double, Double, Double, UInt32) -> Void
    typealias Pan = @convention(c) (Double, Double, Double, Double, UInt32) -> Void
    typealias Key = @convention(c) (UInt32, UInt32, Int32, UInt32) -> Void
    typealias Readbacks = @convention(c) () -> UInt64
    private(set) var context: NSOpenGLContext!
    private(set) var format: NSOpenGLPixelFormat!
    private var library: UnsafeMutableRawPointer?
    private var bootstrap: GLuint = 0
    private var bootstrapTexture: GLuint = 0
    private var timer: Timer?
    private let views = NSHashTable<PolarStationGraphicsView>.weakObjects()
    private var identities: Set<String> = []
    private var lastFrame = 0.0
    private var loadFailure: Error?
    private var started = false
    private(set) var subscribe: Subscribe!
    private(set) var command: Command!
    private(set) var configure: Configure!
    private(set) var needsFrame: ViewFlag!
    private(set) var select: ViewAction!
    private(set) var destroy: ViewAction!
    private(set) var present: Present!
    private(set) var pointer: Pointer!
    private(set) var magnify: Magnify!
    private(set) var pan: Pan!
    private(set) var key: Key!
    private var create: Create!
    private var step: Step!
    private var readbacks: Readbacks!
    private var stop: (@convention(c) () -> Void)!

    private func symbol<T>(_ name: String, _: T.Type) throws -> T {
        guard let address = dlsym(library, name) else { throw Failure(message: "Missing graphics entry: \(name)") }
        return unsafeBitCast(address, to: T.self)
    }
    func prepare() throws {
        if let loadFailure { throw loadFailure }
        if started { return }
        do {
            let attributes: [NSOpenGLPixelFormatAttribute] = [
                UInt32(NSOpenGLPFAOpenGLProfile), UInt32(NSOpenGLProfileVersion4_1Core),
                UInt32(NSOpenGLPFAAccelerated), UInt32(NSOpenGLPFADoubleBuffer),
                UInt32(NSOpenGLPFAColorSize), 24, UInt32(NSOpenGLPFAAlphaSize), 8, 0
            ]
            guard let pixelFormat = NSOpenGLPixelFormat(attributes: attributes),
                  let shared = NSOpenGLContext(format: pixelFormat, share: nil) else {
                throw Failure(message: "Cannot create the Idea Tree GPU context")
            }
            format = pixelFormat; context = shared
            context.makeCurrentContext()
            glGenFramebuffers(1, &bootstrap); glBindFramebuffer(GLenum(GL_FRAMEBUFFER), bootstrap)
            glGenTextures(1, &bootstrapTexture); glBindTexture(GLenum(GL_TEXTURE_2D), bootstrapTexture)
            glTexImage2D(GLenum(GL_TEXTURE_2D), 0, GL_RGBA8, 64, 64, 0, GLenum(GL_RGBA), GLenum(GL_UNSIGNED_BYTE), nil)
            glFramebufferTexture2D(GLenum(GL_FRAMEBUFFER), GLenum(GL_COLOR_ATTACHMENT0), GLenum(GL_TEXTURE_2D), bootstrapTexture, 0)
            guard glCheckFramebufferStatus(GLenum(GL_FRAMEBUFFER)) == GLenum(GL_FRAMEBUFFER_COMPLETE) else {
                throw Failure(message: "Incomplete Idea Tree GPU framebuffer")
            }
            let root = Bundle.main.resourceURL!.appendingPathComponent("graphics")
            library = dlopen(root.appendingPathComponent("libgodot.dylib").path, RTLD_NOW | RTLD_LOCAL)
            guard library != nil else { throw Failure(message: "Idea Tree runtime unavailable: \(String(cString: dlerror()))") }
            let start = try symbol("polar_godot_start", Start.self)
            stop = try symbol("polar_godot_stop", (@convention(c) () -> Void).self)
            create = try symbol("polar_godot_view_create", Create.self)
            subscribe = try symbol("polar_godot_view_subscribe", Subscribe.self)
            command = try symbol("polar_godot_view_command", Command.self)
            configure = try symbol("polar_godot_view_configure", Configure.self)
            needsFrame = try symbol("polar_godot_view_needs_frame", ViewFlag.self)
            select = try symbol("polar_godot_view_input", ViewAction.self)
            destroy = try symbol("polar_godot_view_destroy", ViewAction.self)
            step = try symbol("polar_godot_step", Step.self)
            present = try symbol("polar_godot_view_present_context", Present.self)
            pointer = try symbol("polar_godot_pointer_event", Pointer.self)
            magnify = try symbol("polar_godot_magnify", Magnify.self)
            pan = try symbol("polar_godot_pan", Pan.self)
            key = try symbol("polar_godot_key", Key.self)
            readbacks = try symbol("polar_godot_readbacks", Readbacks.self)
            let args = [Bundle.main.executablePath!, "--path", root.appendingPathComponent("project").path,
                        "--display-driver", "gtk_surface", "--rendering-method", "gl_compatibility",
                        "--render-thread", "safe", "--audio-driver", "Dummy", "--disable-vsync", "res://polar_graphics_host.tscn"]
            var argv = args.map { strdup($0) }
            defer { argv.forEach { free($0) } }
            guard start(Int32(argv.count), &argv, 64, 64, bootstrap) == 1 else { throw Failure(message: "Idea Tree engine startup failed") }
            started = true
            let timer = Timer(timeInterval: 1.0 / 60.0, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.tick() }
            }
            RunLoop.main.add(timer, forMode: .common)
            self.timer = timer
            NSLog("POLAR_GRAPHICS engine_started=1 renderer=OpenGL")
        } catch { loadFailure = error; throw error }
    }
    func shutdown() {
        guard started else { return }
        timer?.invalidate(); timer = nil
        for view in views.allObjects { view.retire() }
        context.makeCurrentContext()
        NSLog("POLAR_GRAPHICS shutdown readbacks=%llu", readbacks())
        stop(); started = false
        glDeleteFramebuffers(1, &bootstrap)
        glDeleteTextures(1, &bootstrapTexture)
        context.clearDrawable()
        // Do not dlclose: the engine cannot reinitialize in this process.
    }
    func attach(_ view: PolarStationGraphicsView, identity: String) throws -> UInt64 {
        try prepare()
        guard !identities.contains(identity) else { throw Failure(message: "Idea Tree identity is already mounted") }
        context.makeCurrentContext()
        let id = identity.withCString { name in "res://itree_workspace.tscn".withCString { create(name, $0) } }
        guard id != 0 else { throw Failure(message: "Cannot load the Idea Tree scene") }
        identities.insert(identity); views.add(view)
        return id
    }
    func detach(_ view: PolarStationGraphicsView) {
        context.makeCurrentContext()
        destroy(view.viewHandle)
        identities.remove(view.identity); views.remove(view)
        NSLog("POLAR_GRAPHICS retired views=%lu readbacks=%llu", identities.count, readbacks())
    }
    func input(_ view: PolarStationGraphicsView, _ send: () -> Void) {
        guard view.viewHandle != 0 else { return }
        context.makeCurrentContext(); select(view.viewHandle); send()
        view.activeUntil = ProcessInfo.processInfo.systemUptime + 0.2
    }
    private func tick() {
        let all = views.allObjects
        let now = ProcessInfo.processInfo.systemUptime
        context.makeCurrentContext()
        var visible: [PolarStationGraphicsView] = []
        for view in all {
            let shown = view.window?.occlusionState.contains(.visible) == true && !view.isHiddenOrHasHiddenAncestor && view.bounds.width > 1 && view.bounds.height > 1
            if !shown && view.wasVisible { view.releaseInput() }
            view.wasVisible = shown
            let pixels = view.convertToBacking(view.bounds).size
            configure(view.viewHandle, Int32(max(1, pixels.width)), Int32(max(1, pixels.height)), Float(view.window?.backingScaleFactor ?? 1), shown ? 1 : 0)
            if shown { visible.append(view) }
        }
        guard !visible.isEmpty, now - lastFrame >= 0.5 || visible.contains(where: { $0.activeUntil > now || needsFrame($0.viewHandle) != 0 }) else { return }
        context.makeCurrentContext()
        _ = step(bootstrap)
        // Finish the producer command stream on the GPU before shared texture consumers.
        let fence = glFenceSync(GLenum(GL_SYNC_GPU_COMMANDS_COMPLETE), 0)
        glFlush()
        for view in visible {
            view.openGLContext?.makeCurrentContext()
            if let fence { glWaitSync(fence, 0, GLuint64(GL_TIMEOUT_IGNORED)) }
            view.needsDisplay = true
        }
        context.makeCurrentContext()
        if let fence { glDeleteSync(fence) }
        lastFrame = now
    }
}

@MainActor
final class PolarStationGraphicsView: NSOpenGLView {
    let identity: String
    private(set) var viewHandle: UInt64 = 0
    private var hostEventGeneration: UInt64 = 0
    var hostEvent: (([String: Any]) -> Void)? {
        didSet {
            hostEventGeneration &+= 1
            guard viewHandle != 0 else { return }
            engine.context.makeCurrentContext()
            engine.subscribe(viewHandle, hostEvent == nil ? nil : Self.receiveHostEvent,
                hostEvent == nil ? nil : Unmanaged.passUnretained(self).toOpaque())
        }
    }
    private static let receiveHostEvent: PolarStationGraphicsEngine.HostEvent = { bytes, context in
        guard let bytes, let context,
              let data = String(cString: bytes).data(using: .utf8),
              let event = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        MainActor.assumeIsolated {
            let view = Unmanaged<PolarStationGraphicsView>.fromOpaque(context).takeUnretainedValue()
            let generation = view.hostEventGeneration
            DispatchQueue.main.async { [weak view] in
                guard let view, view.viewHandle != 0, view.hostEventGeneration == generation else { return }
                view.hostEvent?(event)
            }
        }
    }
    private var filePanel: NSSavePanel?
    func sendCommand(_ action: String, query: String) {
        if action == "open_files" || action == "install_view_package" {
            guard filePanel == nil, viewHandle != 0, let window else { return }
            let panel = NSOpenPanel()
            panel.title = action == "open_files" ? "Open Idea Trees" : "Install trusted view package"; panel.canChooseDirectories = false
            panel.canChooseFiles = true; panel.allowsMultipleSelection = action == "open_files"
            let origin = viewHandle
            filePanel = panel
            panel.beginSheetModal(for: window) { [weak self] result in
                guard let self else { return }
                self.filePanel = nil
                guard result == .OK, self.viewHandle == origin, origin != 0,
                      let bytes = try? JSONSerialization.data(withJSONObject: panel.urls.map(\.path)),
                      let paths = String(data: bytes, encoding: .utf8) else { return }
                self.sendCommand(action == "open_files" ? "load_files" : "install_package", query: action == "open_files" ? paths : (panel.url?.path ?? ""))
            }
            return
        }
        if action == "export_view_package" {
            guard filePanel == nil, viewHandle != 0, let window else { return }
            let panel = NSSavePanel()
            panel.title = "Export view package"; panel.nameFieldStringValue = query.replacingOccurrences(of: "/", with: "-") + ".polar-view.zip"
            let origin = viewHandle; filePanel = panel
            panel.beginSheetModal(for: window) { [weak self] result in
                guard let self else { return }
                self.filePanel = nil
                guard result == .OK, self.viewHandle == origin, origin != 0, let url = panel.url,
                      let bytes = try? JSONSerialization.data(withJSONObject: ["id":query,"destination":url.path]),
                      let request = String(data:bytes,encoding:.utf8) else { return }
                self.sendCommand("export_package",query:request)
            }
            return
        }
        if action == "open_files" || action == "read" { window?.makeFirstResponder(self) }
        engine.input(self) {
            action.withCString { name in query.withCString { engine.command(viewHandle, name, $0) } }
        }
    }
    var focusOwner: (() -> Void)?
    var blurOwner: (() -> Void)?
    var activeUntil = 0.0
    var wasVisible = false
    private var readFramebuffer: GLuint = 0
    private var heldButtons: UInt32 = 0
    private var heldKeys: [UInt16: (UInt32, UInt32)] = [:]
    private var tracking: NSTrackingArea?
    private var pointerPosition = NSPoint.zero
    private let engine = PolarStationGraphicsEngine.shared
    override var acceptsFirstResponder: Bool { true }

    init?(viewID: String) throws {
        identity = viewID
        try engine.prepare()
        super.init(frame: .zero, pixelFormat: engine.format)
        guard let drawing = NSOpenGLContext(format: engine.format, share: engine.context) else {
            throw PolarStationGraphicsEngine.Failure(message: "Cannot create Idea Tree presentation context")
        }
        openGLContext = drawing
        wantsBestResolutionOpenGLSurface = true
        viewHandle = try engine.attach(self, identity: viewID)
        setAccessibilityLabel("Idea Tree")
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    override func draw(_ dirtyRect: NSRect) {
        guard viewHandle != 0, let context = openGLContext else { return }
        context.makeCurrentContext()
        if readFramebuffer == 0 { glGenFramebuffers(1, &readFramebuffer) }
        var destination: GLint = 0
        glGetIntegerv(GLenum(GL_DRAW_FRAMEBUFFER_BINDING), &destination)
        let pixels = convertToBacking(bounds).size
        _ = engine.present(viewHandle, Int32(pixels.width), Int32(pixels.height), UInt32(destination), readFramebuffer)
        context.flushBuffer()
    }
    func retire() {
        filePanel?.cancel(nil); filePanel = nil
        guard viewHandle != 0 else { return }
        hostEvent = nil
        releaseInput(); engine.detach(self); viewHandle = 0
        openGLContext?.makeCurrentContext()
        if readFramebuffer != 0 { glDeleteFramebuffers(1, &readFramebuffer); readFramebuffer = 0 }
        openGLContext?.clearDrawable()
    }
    override func updateTrackingAreas() {
        if let tracking { removeTrackingArea(tracking) }
        tracking = NSTrackingArea(rect: bounds, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(tracking!)
        super.updateTrackingAreas()
    }
    private func modifiers(_ event: NSEvent) -> UInt32 {
        (event.modifierFlags.contains(.shift) ? 1 : 0) |
        (event.modifierFlags.contains(.control) || event.modifierFlags.contains(.command) ? 4 : 0) |
        (event.modifierFlags.contains(.option) ? 8 : 0)
    }
    private func mouse(_ event: NSEvent, button: Int32 = 0, pressed: Int32 = 0) {
        let local = convert(event.locationInWindow, from: nil)
        let point = NSPoint(x: local.x, y: bounds.height - local.y)
        if button > 0 {
            let bit = UInt32(1) << UInt32(button - 1)
            if pressed != 0 { heldButtons |= bit } else { heldButtons &= ~bit }
        }
        // NSEvent raises for clickCount on motion, scroll and gesture events.
        let doubleClick = button != 0 && pressed != 0 && event.clickCount == 2
        engine.input(self) { engine.pointer(button == 0 ? 0 : 1, point.x, point.y, point.x-pointerPosition.x, point.y-pointerPosition.y, button, pressed, heldButtons, modifiers(event), doubleClick ? 1 : 0) }
        pointerPosition = point
    }
    override func mouseDown(with event: NSEvent) {
        let before = window?.firstResponder
        let accepted = window?.makeFirstResponder(self) ?? false
        if ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil {
            NSLog("Station graphics click view=%@ key=%d before=%@ accepted=%d after=%@", identity, window?.isKeyWindow == true ? 1 : 0, String(describing: before), accepted ? 1 : 0, String(describing: window?.firstResponder))
        }
        mouse(event, button: 1, pressed: 1)
    }
    override func mouseUp(with event: NSEvent) { mouse(event, button: 1) }
    override func rightMouseDown(with event: NSEvent) { window?.makeFirstResponder(self); mouse(event, button: 2, pressed: 1) }
    override func rightMouseUp(with event: NSEvent) { mouse(event, button: 2) }
    override func otherMouseDown(with event: NSEvent) { mouse(event, button: 3, pressed: 1) }
    override func otherMouseUp(with event: NSEvent) { mouse(event, button: 3) }
    override func mouseMoved(with event: NSEvent) { mouse(event) }
    override func mouseDragged(with event: NSEvent) { mouse(event) }
    override func rightMouseDragged(with event: NSEvent) { mouse(event) }
    override func otherMouseDragged(with event: NSEvent) { mouse(event) }
    override func magnify(with event: NSEvent) {
        mouse(event)
        if ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil {
            NSLog("POLAR_GRAPHICS magnify view=%@ point=%@ factor=%g", identity, NSStringFromPoint(pointerPosition), 1 + event.magnification)
        }
        engine.input(self) { engine.magnify(pointerPosition.x, pointerPosition.y, 1 + event.magnification, modifiers(event)) }
    }
    override func scrollWheel(with event: NSEvent) {
        if event.hasPreciseScrollingDeltas {
            mouse(event)
            if ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil {
                NSLog("POLAR_GRAPHICS pan view=%@ point=%@ delta=%g,%g", identity, NSStringFromPoint(pointerPosition), event.scrollingDeltaX, event.scrollingDeltaY)
            }
            // Spatial gestures use ten logical pixels per unit. Preserve the
            // precise-scroll point displacement. Like Godot macOS processPanEvent,
            // convert AppKit scroll direction to PanGesture direction once.
            engine.input(self) { engine.pan(pointerPosition.x, pointerPosition.y, -event.scrollingDeltaX * 0.1, -event.scrollingDeltaY * 0.1, modifiers(event)) }
            return
        }
        guard event.scrollingDeltaY != 0 else { return }
        mouse(event)
        let button: Int32 = event.scrollingDeltaY > 0 ? 4 : 5
        engine.input(self) {
            engine.pointer(1, pointerPosition.x, pointerPosition.y, 0, 0, button, 1, heldButtons, modifiers(event), 0)
            engine.pointer(1, pointerPosition.x, pointerPosition.y, 0, 0, button, 0, heldButtons, modifiers(event), 0)
        }
    }
    private func keyValues(_ event: NSEvent) -> (UInt32, UInt32) {
        let special: [UInt16: UInt32] = [36: 0xff0d, 53: 0xff1b, 51: 0xff08, 48: 0xff09, 123: 0xff51, 126: 0xff52, 124: 0xff53, 125: 0xff54, 117: 0xffff]
        return (event.characters?.unicodeScalars.first?.value ?? 0, special[event.keyCode] ?? event.charactersIgnoringModifiers?.unicodeScalars.first?.value ?? 0)
    }
    override func keyDown(with event: NSEvent) {
        let values = keyValues(event); heldKeys[event.keyCode] = values
        engine.input(self) { engine.key(values.0, values.1, 1, modifiers(event)) }
    }
    override func keyUp(with event: NSEvent) {
        let values = heldKeys.removeValue(forKey: event.keyCode) ?? keyValues(event)
        engine.input(self) { engine.key(values.0, values.1, 0, modifiers(event)) }
    }
    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        if ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil { NSLog("Station graphics focus view=%@ accepted=%d key=%d",identity,accepted ? 1 : 0,window?.isKeyWindow == true ? 1 : 0) }
        if accepted { focusOwner?() }
        return accepted
    }
    override func resignFirstResponder() -> Bool { releaseInput(); blurOwner?(); return super.resignFirstResponder() }
    func releaseInput() {
        engine.input(self) {
            for button in 1...3 where heldButtons & (1 << (button-1)) != 0 {
                heldButtons &= ~(1 << (button-1))
                engine.pointer(1, pointerPosition.x, pointerPosition.y, 0, 0, Int32(button), 0, heldButtons, 0, 0)
            }
            for (_, values) in heldKeys { engine.key(values.0, values.1, 0, 0) }
            heldKeys.removeAll()
        }
    }
}
#endif
