#if POLAR_STATION
import AppKit
import ApplicationServices
import Foundation
import GhosttyKit
import CPolarStationDesktopClient
import PolarTmuxHostClient

/// Borrowed native editor and its captured revision. Routing belongs to Rust.
@MainActor
final class StationDictateTarget {
    private(set) var value: [String: Any]
    enum Delivery: String { case applied, unchanged, unknown, no_target }
    let insert: (String) -> Delivery
    init(kind: String, context: String, insert: @escaping (String) -> Delivery) {
        value = ["kind": kind, "identity": UUID().uuidString,
                 "revision": UUID().uuidString, "context": context]
        self.insert = insert
    }
    func protectedTarget() -> StationDictateTarget { value["protected"] = true; return self }
    static func editor(_ editor: NSTextView, kind: String = "editor",
                       valid: @escaping () -> Bool = { true }) -> StationDictateTarget? {
        guard editor.isEditable, !editor.hasMarkedText(),
              !(editor.delegate is NSSecureTextField) else { return nil }
        let original = editor.string
        let range = editor.selectedRange()
        let source = original as NSString
        guard range.location != NSNotFound, range.location <= source.length,
              range.length <= source.length - range.location else { return nil }
        let start = max(0, range.location - 1024)
        let end = min(source.length, range.location + range.length + 1024)
        return StationDictateTarget(kind: kind,
            context: source.substring(with: NSRange(location: start, length: end - start))) { [weak editor] text in
            guard let editor, editor.window != nil, editor.isEditable,
                  !editor.hasMarkedText(), valid(), editor.string == original,
                  editor.selectedRange() == range else { return .unchanged }
            // NSTextView performs one undoable replacement and informs its
            // existing delegate. Never synthesize Return or invoke Send.
            let expected = source.replacingCharacters(in: range, with: text)
            editor.insertText(text, replacementRange: range)
            let actual = editor.string
            if actual != expected, ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil {
                NSLog("Dictate insertion receipt: expected UTF16=%ld actual UTF16=%ld original UTF16=%ld range=%@",
                      expected.utf16.count, actual.utf16.count, original.utf16.count, NSStringFromRange(range))
            }
            return actual == expected ? .applied : .unknown
        }
    }
}

/// One instance per application. Rust owns status, target choice and recovery;
/// this adapter owns main-thread capture/editor handles and background calls.
@MainActor
final class PolarStationDictate {
    var changed: (() -> Void)?
    var showVoiceSettings: (() -> Void)?
    private var shownSetupOperation: String?
    var focusedTarget: ((NSTextView) -> StationDictateTarget?)?
    private var events: Any?
    private var externalEditors: StationDictateExternalEditors?
    private(set) var view: [String: Any] = [:]
    private(set) var error: String?
    private var ready = false
    private var stopped = false
    private var captureID: String?
    private var submittedID: String?
    private var heldID: String?
    private let gesture = polar_station_shortcut_gesture_new()
    private var shortcutToggle = false
    private var voiceSettingsLoaded = false
    private var voiceMicrophone: String?
    private var shortcutBinding = "Fn"
    private var settingsObserver: NSObjectProtocol?
    private var deletionObserver: NSObjectProtocol?
    var shortcutTargets: (() -> [StationDictateTarget])?
    private var captured: [String: StationDictateTarget] = [:]
    private var cancelled = Set<String>()
    private var recordingStart: TimeInterval?
    private let audio = PolarStationDictateAudio()
    private var recordingPanel: StationDictateRecordingPanel?
    private let worker = Worker()
    private let queue = DispatchQueue(label: "PolarStation.Dictate.state")
    private let speechQueue = DispatchQueue(label: "PolarStation.Dictate.speech")

    private final class Worker: @unchecked Sendable {
        let state = polar_station_dictate_new()
        let speech = polar_station_dictate_speech_new()
        func dispatch(_ data: Data) -> Data? {
            data.withUnsafeBytes { raw in
                guard let bytes = raw.bindMemory(to: UInt8.self).baseAddress,
                      let result = polar_station_dictate_dispatch(state, bytes, data.count) else { return nil }
                defer { polar_station_desktop_string_free(result) }
                return Data(bytes: result, count: strlen(result))
            }
        }
        func service(_ data: Data) -> Data? {
            data.withUnsafeBytes { raw in
                guard let result = polar_station_dictate_service_request(speech, raw.bindMemory(to: UInt8.self).baseAddress, data.count) else { return nil }
                defer { polar_station_desktop_string_free(result) }
                return Data(bytes: result, count: strlen(result))
            }
        }
        deinit {
            polar_station_dictate_free(state)
            polar_station_dictate_speech_free(speech)
        }
    }
    init() {
        externalEditors = StationDictateExternalEditors { [weak self] point in
            guard let self, self.hasUnboundDraft else { return }
            if let target = StationDictateExternalEditors.capture(activationPoint: point) { self.focus(target) }
        }
        recordingPanel=StationDictateRecordingPanel(action:{[weak self] action,text in
            if action == "stop" {self?.finish()} else if action == "cancel" {self?.cancel()} else if action == "use_original" {self?.useOriginal()} else {self?.panelAction(action,text:text)}
        })
        polar_station_dictate_speech_set_progress(worker.speech, { context,id,length,saved,preview,previewLength in
            guard let context,let id else{return}
            let owner=Unmanaged<PolarStationDictate>.fromOpaque(context).takeUnretainedValue()
            let operation=String(decoding:UnsafeBufferPointer(start:id,count:length),as:UTF8.self)
            let text = preview.map { String(decoding:UnsafeBufferPointer(start:$0,count:previewLength),as:UTF8.self) } ?? ""
            DispatchQueue.main.async { [weak owner] in
                guard let owner,!owner.stopped else{return}
                if text.isEmpty { owner.input(["type":"audio_saved","operation_id":operation,"saved_samples":saved]) }
                else { owner.input(["type":"transcript_preview","operation_id":operation,"text":text]) }
            }
        },Unmanaged.passUnretained(self).toOpaque())
        settingsObserver = NotificationCenter.default.addObserver(forName: Notification.Name("PolarStationVoiceSettingsChanged"), object:nil, queue:.main) { [weak self] _ in
            MainActor.assumeIsolated { self?.reloadVoiceSettings() }
        }
        deletionObserver = NotificationCenter.default.addObserver(forName:Notification.Name("PolarStationRecordingDeleted"), object:nil, queue:.main) { [weak self] notification in
            guard let id=notification.object as? String else{return}
            MainActor.assumeIsolated { self?.input(["type":"recording_deleted", "operation_id":id]) }
        }
        reloadVoiceSettings()
        events = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp, .leftMouseDown]) { [weak self] event in
            guard let self else { return event }
            if event.type == .keyDown, event.keyCode == 53, self.presentation["cancel_id"] is String {
                self.cancel(); return nil
            }
            let clicked = event.type == .leftMouseDown
            let tabbed = event.type == .keyUp && event.keyCode == 48
            if clicked || tabbed {
                let window = event.window
                let point = event.locationInWindow
                // NSTextView can consume mouse-up inside its selection tracking
                // loop. Snapshot only after that loop returns to default mode,
                // so a click/drag has finished establishing its caret/selection.
                RunLoop.main.perform(inModes: [.default]) { [weak self, weak window] in
                    MainActor.assumeIsolated {
                        guard let self, let window, window.isKeyWindow, window !== self.recordingPanel,
                              let editor = window.firstResponder as? NSTextView else { return }
                        guard tabbed || editor.bounds.contains(editor.convert(point, from: nil)) else { return }
                        if let target = self.focusedTarget?(editor) { self.focus(target) }
                    }
                }
            }
            return event
        }
        guard let directory = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else { return }
        input(["type":"open", "host_managed":true, "path":directory.appendingPathComponent("com.polarealm.PolarStation/dictate-v1.json").path], opening: true)
    }
    var presentation: [String: Any] { view["presentation"] as? [String: Any] ?? [:] }
    var enabled: Bool { (presentation["enabled"] as? Bool ?? false) && (submittedID == nil || presentation["action"] as? String == "stop") }
    var status: String { error ?? presentation["status"] as? String ?? "" }
    func refine(target: StationDictateTarget, text: String) {
        guard enabled, presentation["action"] as? String == "start", !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        let id = UUID().uuidString
        submittedID = id
        captured = [target.value["identity"] as? String ?? "": target]
        input(["type":"refine_text", "operation_id":id, "target":target.value, "text":text])
    }
    func toggle(targets: [StationDictateTarget]) {
        if presentation["action"] as? String == "retry_open" { input(["type":"retry_open"]); return }
        guard ready else { return }
        if presentation["action"] as? String == "stop" { finish(); return }
        guard enabled else { return }
        let id = UUID().uuidString
        submittedID = id
        recordingStart = nil // A new press cannot inherit the previous capture clock.
        captured = Dictionary(uniqueKeysWithValues: targets.compactMap { target in
            (target.value["identity"] as? String).map { ($0, target) }
        })
        cancelled.removeAll()
        input(["type":"start", "operation_id":id, "targets":targets.map(\.value)])
    }
    // Physical edges are latched before the asynchronous Rust transition.
    // This is an input handle, not a second recording lifecycle.
    func shortcut(pressed: Bool) {
        guard !stopped else { return }
        let intent = polar_station_shortcut_gesture_event(gesture, pressed ? 1 : 2,
            shortcutToggle, submittedID != nil && presentation["action"] as? String != "wait",
            (enabled && presentation["action"] as? String == "start") || presentation["action"] as? String == "retry_open")
        if intent == 2 && pressed { heldID = submittedID }
        executeShortcut(intent)
    }
    private func executeShortcut(_ intent: UInt32) {
        switch intent {
        case 1:
            toggle(targets: NSApp.isActive ? (shortcutTargets?() ?? []) : StationDictateExternalEditors.capture().map { [$0] } ?? [])
            heldID = submittedID
        case 2:
            guard let id = heldID, submittedID == id else { return }
            heldID = nil
            if recordingStart == nil { cancelled.insert(id); audio.cancel() }
            let duration = recordingStart.map { min(600_000, Int((ProcessInfo.processInfo.systemUptime - $0) * 1000)) } ?? 0
            input(["type":"stop", "operation_id":id, "duration_ms":duration])
        default: break
        }
    }
    private let shortcutCapture = StationDictateKeyCapture()
    func handleShortcut(_ event: NSEvent) -> Bool {
        guard !stopped else { return false }
        return shortcutCapture.handle(event, binding: shortcutBinding) { [weak self] event in
            guard let self else { return }
            if event == 4 { cancel() }
            let intent = polar_station_shortcut_gesture_event(gesture, event, shortcutToggle,
                submittedID != nil && presentation["action"] as? String != "wait",
                (enabled && presentation["action"] as? String == "start") || presentation["action"] as? String == "retry_open")
            if intent == 2 && (event == 1 || event == 3) { heldID = submittedID }
            executeShortcut(intent)
        }
    }
    private func reloadVoiceSettings() {
        Task {let client=VoiceClient();let data=await client.request(Data(#"{"op":"voice_settings"}"#.utf8))
            if let reply=try? JSONSerialization.jsonObject(with:data) as? [String:Any],let value=reply["payload"] as? [String:Any] {
                shortcutToggle = value["shortcut_mode"] as? String == "toggle"
                input(["type":"shortcut_mode", "toggle":shortcutToggle])
                voiceMicrophone=value["microphone"] as? String
                voiceSettingsLoaded = true
                if let shortcut=value["shortcut_mac"] as? String,StationDictateKeyCapture.binding(shortcut) != nil {
                    shortcutBinding=shortcut
                    input(["type":"shortcut_binding", "trigger":shortcut])
                }
            }
        }
    }
    func configureShortcut(_ text:String, reply:@escaping @MainActor ([String:Any])->Void) {
        guard let binding=StationDictateKeyCapture.binding(text)else{reply(["error":"Choose Fn, RightOption, Control+Option+Space or Control+Shift+Space."]);return}
        guard AXIsProcessTrusted()else{GlobalEventTap.shared.enableStationDictate();reply(["error":"Allow Accessibility first. The previous shortcut remains active."]);return}
        if let keys=UserDefaults(suiteName:"com.apple.symbolichotkeys")?.dictionary(forKey:"AppleSymbolicHotKeys") {
            for item in keys.values {
                guard let item=item as? [String:Any],item["enabled"] as? Bool == true,
                      let value=item["value"] as? [String:Any],let parameters=value["parameters"] as? [Int],parameters.count>=3 else{continue}
                if parameters[1]==Int(binding.0), UInt(parameters[2])==binding.1.rawValue {reply(["error":"This shortcut is already used by macOS. The previous shortcut remains active."]);return}
            }
        }
        if let event=CGEvent(keyboardEventSource:nil,virtualKey:binding.0,keyDown:true) {
            event.flags=CGEventFlags(rawValue:UInt64(binding.1.rawValue))
            if let native=NSEvent(cgEvent:event),let delegate=NSApp.delegate as? AppDelegate,let app=delegate.ghostty.app,
               ghostty_app_key_is_binding(app,native.ghosttyKeyEvent(GHOSTTY_ACTION_PRESS)) {reply(["error":"This shortcut is already used by a terminal binding. The previous shortcut remains active."]);return}
        }
        Task {let client=VoiceClient();let data=await client.request(Data(#"{"op":"voice_settings"}"#.utf8))
            guard let response=try? JSONSerialization.jsonObject(with:data) as? [String:Any],var settings=response["payload"] as? [String:Any]else{reply(["error":"Voice settings could not be loaded; shortcut unchanged."]);return}
            settings["shortcut_mac"]=text
            guard let request=try? JSONSerialization.data(withJSONObject:["op":"voice_settings_update","settings":settings])else{return}
            let result=await client.request(request)
            guard let value=try? JSONSerialization.jsonObject(with:result) as? [String:Any],value["ok"] as? Bool == true else{reply(["error":"Voice settings could not be saved; shortcut unchanged."]);return}
            shortcutBinding=text;shortcutStatus("ready");NotificationCenter.default.post(name:Notification.Name("PolarStationVoiceSettingsChanged"),object:nil)
            reply(["message":"Shortcut applied: "+text])
        }
    }
    func shortcutStatus(_ status: String) {
        input(["type":"shortcut_binding", "trigger":status == "ready" ? shortcutBinding : ""])
        input(["type":"shortcut_status", "status":status])
    }
    private var hasUnboundDraft: Bool {
        (view["entries"] as? [[String: Any]] ?? []).contains {
            $0["phase"] as? String == "pending" && ($0["target"] == nil || $0["target"] is NSNull)
        }
    }
    func focus(_ target: StationDictateTarget) {
        guard ready, submittedID == nil, presentation["action"] as? String == "start",
              let id = target.value["identity"] as? String else { return }
        captured = [id: target]
        input(["type":"focus", "target":target.value])
    }
    func cancel() {
        guard let id = submittedID ?? presentation["cancel_id"] as? String else { return }
        cancelled.insert(id); audio.cancel(); captureID = nil
        cancelSpeech(id)
        input(["type":"cancel", "operation_id":id])
    }
    func stop() {
        guard !stopped else { return }
        polar_station_shortcut_gesture_free(gesture)
        polar_station_dictate_speech_set_progress(worker.speech,nil,nil)
        externalEditors?.stop(); externalEditors = nil
        recordingPanel?.orderOut(nil)
        cancel(); stopped = true; audio.cancel(); changed = nil
        if let settingsObserver {NotificationCenter.default.removeObserver(settingsObserver)}
        if let deletionObserver {NotificationCenter.default.removeObserver(deletionObserver)}
        if let events { NSEvent.removeMonitor(events) }; events = nil
    }
    func useOriginal() {
        guard let id = presentation["use_original_id"] as? String else { return }
        input(["type":"use_original", "operation_id":id])
    }
    func reviewResult(_ id:String) { input(["type":"show_panel","operation_id":id]) }
    func dismiss(_ id: String) { input(["type":"dismiss", "operation_id":id]) }
    func recoverRecording(_ id: String) { input(["type":"recover_recording", "operation_id":id]) }
    private func cancelSpeech(_ id: String) {
        Array(id.utf8).withUnsafeBufferPointer {
            polar_station_dictate_speech_cancel(worker.speech, $0.baseAddress, $0.count)
        }
    }
    private func finish() {
        guard let id = captureID else { return }
        let duration = recordingStart.map { min(600_000, Int((ProcessInfo.processInfo.systemUptime - $0) * 1000)) } ?? 0
        input(["type":"stop", "operation_id":id, "duration_ms":duration])
    }
    private func input(_ value: [String: Any], opening: Bool = false) {
        guard let bytes = try? JSONSerialization.data(withJSONObject: value) else { return }
        queue.async { [worker, weak self] in
            let result = worker.dispatch(bytes)
            DispatchQueue.main.async { [weak self] in self?.apply(result, opening: opening) }
        }
    }
    private func apply(_ data: Data?, opening: Bool) {
        guard !stopped else { return }
        guard let data, let result = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              result["presentation"] is [String: Any] else {
            error = data.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }?["error"] as? String
                ?? "Dictate worker could not start."
            submittedID = nil
            changed?(); return
        }
        ready = result["initialized"] as? Bool ?? ready
        error = result["ok"] as? Bool == true ? nil : result["error"] as? String
        if error != nil { submittedID = nil }
        view = result
        if let id = submittedID, let entry = (result["entries"] as? [[String: Any]])?.first(where: { $0["operation_id"] as? String == id }),
           ["delivered", "unknown", "review", "pending", "cancelled", "failed"].contains(entry["phase"] as? String ?? "") { submittedID = nil }
        if presentation["setup_required"] as? Bool == true,
           let entry=(result["entries"] as? [[String:Any]])?.last,
           let operation=entry["operation_id"] as? String, shownSetupOperation != operation {
            shownSetupOperation=operation;showVoiceSettings?()
        }
        recordingPanel?.update(result["panel"] as? [String:Any])
        changed?()
        for effect in result["effects"] as? [[String: Any]] ?? [] { execute(effect) }
    }
    private func currentInputTarget() -> StationDictateTarget? {
        // This is sampled when text is ready, not when recording began.
        if recordingPanel?.isKeyWindow == true { return nil }
        if NSWorkspace.shared.frontmostApplication?.processIdentifier == ProcessInfo.processInfo.processIdentifier {
            guard let window=NSApp.keyWindow, window.isVisible, window.occlusionState.contains(.visible), window !== recordingPanel else {return nil}
            if let editor=window.firstResponder as? NSTextView, !editor.isHiddenOrHasHiddenAncestor, !editor.visibleRect.isEmpty {
                return StationDictateTarget.editor(editor)
            }
            return shortcutTargets?().first(where: { $0.value["kind"] as? String == "idea_node" })
        }
        return StationDictateExternalEditors.capture()
    }
    private func panelAction(_ action:String, text:String? = nil) {
        guard let panel=view["panel"] as? [String:Any],let id=panel["operation_id"] as? String else{return}
        if action == "insert_draft" || action == "retry_saved" {cancelled.remove(id)}
        var value:[String:Any] = ["type":action,"operation_id":id]
        if let text {value["text"]=text}
        input(value)
    }
    private func execute(_ effect: [String: Any]) {
        guard let kind = effect["type"] as? String, let id = effect["operation_id"] as? String else { return }
        switch kind {
        case "capture":
            guard voiceSettingsLoaded else { reloadVoiceSettings(); input(["type":"failed", "operation_id":id,"error":"Voice settings are not ready. Open Settings → Voice and retry."]); return }
            guard !cancelled.contains(id) else { return }
            captureID = id; recordingStart = nil
            let begin: [String: Any] = ["op":"capture_begin", "id":id, "context":effect["context"] ?? NSNull()]
            guard let beginData = try? JSONSerialization.data(withJSONObject: begin),
                  let beginResult = worker.service(beginData),
                  let started = try? JSONSerialization.jsonObject(with: beginResult) as? [String: Any],
                  started["ok"] as? Bool == true else {
                input(["type":"failed", "operation_id":id, "error":"Voice capture could not start. Open Settings → Voice."]); return
            }
            let operation = Array(id.utf8)
            audio.start(operation: id, maximumMS: effect["maximum_ms"] as? Int ?? 600_000, deviceUID: voiceMicrophone,
                chunk: { [worker] pcm in
                    operation.withUnsafeBufferPointer { operation in
                        pcm.withUnsafeBytes { bytes in
                            polar_station_dictate_speech_append(worker.speech, operation.baseAddress, operation.count,
                                bytes.bindMemory(to: UInt8.self).baseAddress, pcm.count)
                        }
                    }
                },
                ready: { [weak self] id in
                    guard let self, self.captureID == id, !self.cancelled.contains(id) else { return }
                    self.recordingStart = ProcessInfo.processInfo.systemUptime
                    self.input(["type":"capture_ready", "operation_id":id])
                }, limit: { [weak self] _ in self?.finish() },
                failed: { [weak self] id, message in
                    self?.audio.cancel(); self?.cancelSpeech(id); self?.captureID = nil
                    self?.input(["type":"failed", "operation_id":id, "error":message])
                })
        case "cancel":
            cancelled.insert(id)
            if captureID == id { audio.cancel(); captureID = nil }
            cancelSpeech(id)
        case "transcribe":
            guard captureID == id else { return }
            let pcm = audio.finish(operation: id); captureID = nil
            let operation = Array(id.utf8)
            let reserved = operation.withUnsafeBufferPointer {
                polar_station_dictate_speech_reserve(worker.speech, $0.baseAddress, $0.count)
            }
            guard reserved else {
                input(["type":"failed", "operation_id":id, "error":"Dictate worker is unavailable."]); return
            }
            speechQueue.async { [worker, weak self] in
                let output: Data? = operation.withUnsafeBufferPointer { operation in
                    pcm.withUnsafeBytes { bytes in
                        guard let result = polar_station_dictate_speech_transcribe(worker.speech,
                            operation.baseAddress, operation.count, bytes.bindMemory(to: UInt8.self).baseAddress, pcm.count) else { return nil }
                        defer { polar_station_desktop_string_free(result) }
                        return Data(bytes: result, count: strlen(result))
                    }
                }
                DispatchQueue.main.async { [weak self] in
                    guard let self, !self.stopped else { return }
                    if let output, let result = try? JSONSerialization.jsonObject(with: output) as? [String: Any],
                       let input = result["input"] as? [String: Any] { self.input(input) }
                    else { self.input(["type":"failed", "operation_id":id, "error":"Dictate transcription failed."]) }
                }
            }
        case "rewrite":
            guard let request = try? JSONSerialization.data(withJSONObject: ["op":effect["saved_recording"] as? Bool == false ? "refine_text" : "rewrite_result", "id":id,"context":effect["context"] ?? "", "raw":effect["raw"] ?? ""]) else { return }
            speechQueue.async { [worker, weak self] in
                let output = worker.service(request)
                DispatchQueue.main.async { [weak self] in
                    guard let self, !self.stopped else { return }
                    if let output, let result = try? JSONSerialization.jsonObject(with: output) as? [String: Any],
                       let input = result["input"] as? [String: Any] { self.input(input) }
                    else { self.input(["type":"rewritten", "operation_id":id, "error":"Saved original retained. Open Settings → Voice to review."]) }
                }
            }
        case "retry_transcribe":
            guard let request = try? JSONSerialization.data(withJSONObject:["op":"retry_result","id":id]) else { return }
            speechQueue.async { [worker, weak self] in
                let output=worker.service(request)
                DispatchQueue.main.async { [weak self] in
                    if let output, let result=try? JSONSerialization.jsonObject(with:output) as? [String:Any], let input=result["input"] as? [String:Any] {self?.input(input)}
                    else {self?.input(["type":"failed","operation_id":id,"error":"Saved recording retry failed."])}
                }
            }
        case "insert_current":
            let target = currentInputTarget()
            let outcome: StationDictateTarget.Delivery = cancelled.contains(id) ? .unchanged : target?.insert(effect["text"] as? String ?? "") ?? .no_target
            input(["type":"delivery","operation_id":id,"outcome":outcome.rawValue])
            captured.removeAll()
        case "insert":
            let target = effect["target"] as? [String: Any] ?? [:]
            let identity = target["identity"] as? String ?? ""
            let outcome: StationDictateTarget.Delivery = cancelled.contains(id) ? .unchanged : captured[identity]?.insert(effect["text"] as? String ?? "") ?? .unchanged
            input(["type":"delivery", "operation_id":id, "outcome":outcome.rawValue])
            captured.removeAll()
        default: break
        }
    }
}

@MainActor
private final class StationDictateRecordingPanel: NSPanel, NSTextViewDelegate {
    private let status=NSTextField(labelWithString:"")
    private let editor=NSTextView()
    private let scroll=NSScrollView()
    private let action:(String,String?)->Void
    private var controls:[String:NSButton]=[:]
    private var currentID:String?
    private var updating=false
    private var editing=false
    private var hideTimer:Timer?
    private var hidingID:String?
    override var canBecomeKey:Bool {editing}
    override var canBecomeMain:Bool {false}
    init(action:@escaping (String,String?)->Void) {
        self.action=action
        super.init(contentRect:NSRect(x:0,y:0,width:480,height:180),styleMask:[.nonactivatingPanel,.titled,.closable],backing:.buffered,defer:false)
        title="Dictate";level = .floating;hidesOnDeactivate=false;isReleasedWhenClosed=false
        collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary]
        status.maximumNumberOfLines=3;status.setAccessibilityLabel("Dictate status")
        editor.isRichText=false;editor.isEditable=false;editor.isSelectable=true;editor.delegate=self
        editor.frame=NSRect(x:0,y:0,width:452,height:130);editor.isVerticallyResizable=true;editor.isHorizontallyResizable=false
        editor.autoresizingMask=[.width];editor.textContainer?.widthTracksTextView=true;editor.allowsUndo=true
        editor.textContainerInset=NSSize(width:8,height:8);editor.setAccessibilityLabel("Dictate result")
        scroll.documentView=editor;scroll.hasVerticalScroller=true;scroll.borderType = .bezelBorder
        scroll.translatesAutoresizingMaskIntoConstraints=false
        scroll.heightAnchor.constraint(equalToConstant:130).isActive=true
        let row=NSStackView();row.orientation = .horizontal;row.spacing=8
        for (kind,title) in [("stop","Stop & Save"),("cancel","Cancel"),("use_original","Use Original"),("copy","Copy"),("edit","Edit"),("insert_draft","Insert"),("retry_saved","Retry"),("hide_panel","Close")] {
            let button=NSButton(title:title,target:self,action:#selector(clicked(_:)))
            button.identifier=NSUserInterfaceItemIdentifier(kind);button.refusesFirstResponder=true
            controls[kind]=button;row.addArrangedSubview(button)
        }
        let column=NSStackView(views:[status,scroll,row]);column.orientation = .vertical;column.alignment = .leading;column.spacing=10
        column.edgeInsets=NSEdgeInsets(top:12,left:14,bottom:12,right:14);contentView=column
        scroll.widthAnchor.constraint(equalToConstant:452).isActive=true
    }
    override func close(){ action("hide_panel",nil);orderOut(nil) }
    func textDidChange(_ notification:Notification){if !updating && editing {action("edit_draft",editor.string)}}
    @objc private func clicked(_ sender:NSButton) {
        guard let kind=sender.identifier?.rawValue else{return}
        switch kind {
        case "copy":NSPasteboard.general.clearContents();NSPasteboard.general.setString(editor.string,forType:.string)
        case "edit":editing=true;editor.isEditable=true;makeKeyAndOrderFront(nil);makeFirstResponder(editor)
        default:action(kind,nil)
        }
    }
    func update(_ panel:[String:Any]?) {
        guard let panel,let id=panel["operation_id"] as? String else {hideTimer?.invalidate();hideTimer=nil;hidingID=nil;orderOut(nil);return}
        if currentID != id {currentID=id;editing=false;editor.isEditable=false}
        status.stringValue=panel["status"] as? String ?? ""
        let text=panel["text"] as? String ?? ""
        if editor.string != text {updating=true;editor.string=text;updating=false}
        scroll.isHidden=text.isEmpty && !editing
        for (kind,key,label) in [("stop","can_stop","stop_label"),("cancel","can_cancel","cancel_label"),("use_original","can_original","original_label"),("copy","can_copy","copy_label"),("edit","can_edit","edit_label"),("insert_draft","can_insert","insert_label"),("retry_saved","can_retry","retry_label")] {
            controls[kind]?.isHidden=panel[key] as? Bool != true
            controls[kind]?.title=panel[label] as? String ?? ""
        }
        if panel["can_edit"] as? Bool != true {editing=false;editor.isEditable=false}
        if !isVisible {if let frame=NSScreen.main?.visibleFrame {setFrameOrigin(NSPoint(x:frame.midX-240,y:frame.minY+70))};orderFrontRegardless()}
        let delay=panel["hide_after_ms"] as? Int ?? 0
        if delay>0 && hidingID != id {
            hideTimer?.invalidate();hidingID=id
            hideTimer=Timer.scheduledTimer(withTimeInterval:Double(delay)/1000,repeats:false){[weak self] _ in
                MainActor.assumeIsolated {guard let self,self.currentID==id else{return};self.action("hide_panel",nil)}
            }
        }
    }
}

@MainActor
final class StationDictateControls: NSStackView {
    var targets: (() -> [StationDictateTarget])?
    weak var dictate: PolarStationDictate?
    var refine: (() -> Void)?
    private let refineButton = NSButton(title: "Refine", target: nil, action: nil)
    private let microphone = NSButton()
    private let history = NSButton(title: "▾", target: nil, action: nil)
    private let popover = NSPopover()
    private var actions: [NSButton: () -> Void] = [:]
    required init?(coder: NSCoder) { return nil }
    init(dictate: PolarStationDictate) {
        self.dictate = dictate
        super.init(frame: .zero)
        orientation = .horizontal; spacing = 0
        microphone.image = NSImage(systemSymbolName: "mic", accessibilityDescription: nil)
        microphone.bezelStyle = .rounded; microphone.refusesFirstResponder = true
        microphone.target = self; microphone.action = #selector(toggle)
        history.refusesFirstResponder = true; history.target = self; history.action = #selector(openHistory)
        refineButton.target = self; refineButton.action = #selector(refinePressed)
        refineButton.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "Refine draft")
        refineButton.imagePosition = .imageOnly; refineButton.toolTip = "Refine draft"; refineButton.setAccessibilityLabel("Refine draft")
        refineButton.widthAnchor.constraint(equalToConstant: 30).isActive = true
        refineButton.refusesFirstResponder = true; refineButton.bezelStyle = .rounded
        addArrangedSubview(refineButton); addArrangedSubview(microphone); addArrangedSubview(history)
        microphone.widthAnchor.constraint(equalToConstant: 30).isActive = true
        history.widthAnchor.constraint(equalToConstant: 18).isActive = true
        popover.behavior = .transient
        refresh()
    }
    func refresh() {
        guard let dictate else { return }
        let presentation = dictate.presentation
        refineButton.isEnabled = dictate.enabled && presentation["action"] as? String == "start"
        microphone.isEnabled = dictate.enabled
        let label = presentation["button_label"] as? String ?? ""
        microphone.toolTip = label + " · " + dictate.status
        microphone.setAccessibilityLabel(label)
        microphone.image = NSImage(systemSymbolName: presentation["action"] as? String == "stop" ? "stop.fill" : "mic", accessibilityDescription: label)
        history.toolTip = presentation["history_title"] as? String
        history.setAccessibilityLabel(presentation["history_title"] as? String ?? "")
        if popover.isShown { rebuildHistory() }
    }
    @objc private func refinePressed() { refine?() }
    @objc private func toggle() {
        guard let dictate else { return }
        // Stopping must not activate or recapture an editor.
        dictate.toggle(targets: dictate.presentation["action"] as? String == "start" ? targets?() ?? [] : [])
    }
    @objc private func openHistory() {
        rebuildHistory(); popover.show(relativeTo: bounds, of: self, preferredEdge: .maxY)
    }
    private func rebuildHistory() {
        guard let dictate else { return }
        actions.removeAll()
        let column = NSStackView(); column.orientation = .vertical; column.alignment = .leading
        column.spacing = 8; column.edgeInsets = NSEdgeInsets(top: 10, left: 10, bottom: 10, right: 10)
        column.addArrangedSubview(NSTextField(wrappingLabelWithString: dictate.status))
        let presentation = dictate.presentation
        column.addArrangedSubview(NSTextField(wrappingLabelWithString: (presentation["shortcut_hint"] as? String ?? "") + " · " + (presentation["shortcut_status"] as? String ?? "")))
        func button(_ label: String, _ action: @escaping () -> Void) -> NSButton {
            let button = NSButton(title: label, target: self, action: #selector(historyAction(_:)))
            actions[button] = action; return button
        }
        if presentation["cancel_id"] is String {
            column.addArrangedSubview(button(presentation["cancel_label"] as? String ?? "") { [weak dictate] in dictate?.cancel() })
        }
        if presentation["use_original_id"] is String {
            column.addArrangedSubview(button("Use Original") { [weak dictate] in dictate?.useOriginal() })
        }
        column.addArrangedSubview(button("Voice Settings…") { [weak dictate] in dictate?.showVoiceSettings?() })
        for entry in dictate.view["history"] as? [[String: Any]] ?? [] {
            let text = entry["text"] as? String ?? ""
            let raw = entry["raw"] as? String ?? ""
            let label = NSTextField(wrappingLabelWithString: text); label.isSelectable = true
            column.addArrangedSubview(label)
            column.addArrangedSubview(NSTextField(wrappingLabelWithString: entry["status"] as? String ?? ""))
            let row = NSStackView()
            if let id=entry["operation_id"] as? String {
                row.addArrangedSubview(button(presentation["review_label"] as? String ?? "") { [weak self, weak dictate] in self?.popover.performClose(nil);dictate?.reviewResult(id) })
            }
            for (name, content, enabled) in [("copy_label", text, "can_copy"), ("original_label", raw, "can_copy_original")] {
                guard entry[enabled] as? Bool == true else { continue }
                let copy = button(presentation[name] as? String ?? "") {
                    NSPasteboard.general.clearContents(); NSPasteboard.general.setString(content, forType: .string)
                }
                copy.isEnabled = entry[enabled] as? Bool ?? false
                row.addArrangedSubview(copy)
            }
            let dismiss = button(presentation["dismiss_label"] as? String ?? "") { [weak dictate] in
                if let id = entry["operation_id"] as? String { dictate?.dismiss(id) }
            }
            dismiss.isEnabled = entry["can_dismiss"] as? Bool ?? false
            row.addArrangedSubview(dismiss); column.addArrangedSubview(row)
        }
        let width = max(200, min(360, (window?.contentView?.bounds.width ?? 380) - 20))
        column.widthAnchor.constraint(equalToConstant: width).isActive = true
        let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.documentView = column
        column.frame = NSRect(origin: .zero, size: column.fittingSize)
        scroll.frame = NSRect(x: 0, y: 0, width: width, height: min(320, column.fittingSize.height))
        let controller = NSViewController(); controller.view = scroll; popover.contentViewController = controller
    }
    @objc private func historyAction(_ sender: NSButton) { actions[sender]?() }
}
/// AX is a borrowed native editor handle. The shared reducer still decides
/// whether this target may receive a result; this adapter never queues a retry.
@MainActor
final class StationDictateExternalEditors {
    private var observer: AXObserver?
    private var application: AXUIElement?
    private var activation: NSObjectProtocol?
    private var interaction: Any?
    private var lastInteraction = -Double.infinity
    private var pointerDown = false
    private let focused: (CGPoint?) -> Void

    init(focused: @escaping (CGPoint?) -> Void) {
        self.focused = focused
        interaction = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .leftMouseUp, .keyDown]) { [weak self] event in
            guard let self else{return}
            if event.type == .leftMouseDown {
                self.pointerDown=true
                self.lastInteraction=ProcessInfo.processInfo.systemUptime
            } else if event.type == .leftMouseUp {
                self.pointerDown=false
                self.lastInteraction = -Double.infinity
                // The target application has already handled the mouse-down
                // selection. Clicking an already-focused editor emits no focus
                // change, so the completed gesture must also bind directly.
                if let point = event.cgEvent?.location { self.focused(point) }
            } else if event.keyCode == 48 {
                self.pointerDown=false
                self.lastInteraction=ProcessInfo.processInfo.systemUptime
            }
        }
        activation = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] _ in MainActor.assumeIsolated {
            guard let self else{return}
            self.observeFrontmost()
            if !self.pointerDown && ProcessInfo.processInfo.systemUptime-self.lastInteraction<1 {
                self.lastInteraction = -Double.infinity;self.focused(nil)
            }
        } }
        observeFrontmost()
    }
    func stop() {
        if let activation { NSWorkspace.shared.notificationCenter.removeObserver(activation) }
        if let interaction { NSEvent.removeMonitor(interaction) }
        activation = nil; interaction = nil
        clearObserver()
    }
    private func clearObserver() {
        if let observer { CFRunLoopRemoveSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(observer), .commonModes) }
        observer = nil; application = nil
    }
    private func observeFrontmost() {
        clearObserver()
        guard AXIsProcessTrusted(), let app = NSWorkspace.shared.frontmostApplication,
              app.processIdentifier != ProcessInfo.processInfo.processIdentifier else { return }
        var next: AXObserver?
        guard AXObserverCreate(app.processIdentifier, { _, _, _, context in
            guard let context else { return }
            MainActor.assumeIsolated {
                let owner = Unmanaged<StationDictateExternalEditors>.fromOpaque(context).takeUnretainedValue()
                if !owner.pointerDown && ProcessInfo.processInfo.systemUptime - owner.lastInteraction < 1 {
                    owner.lastInteraction = -Double.infinity;owner.focused(nil)
                }
            }
        }, &next) == .success, let next else { return }
        let root = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(root, 0.25)
        let context = Unmanaged.passUnretained(self).toOpaque()
        AXObserverAddNotification(next, root, kAXFocusedUIElementChangedNotification as CFString, context)
        observer = next; application = root
        CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(next), .commonModes)
    }
    private static func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
        return value
    }
    private static func focusedElement(_ pid: pid_t) -> AXUIElement? {
        let root = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(root, 0.25)
        guard let value = attribute(root, kAXFocusedUIElementAttribute), CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        return unsafeBitCast(value, to: AXUIElement.self)
    }
    private static func range(_ element: AXUIElement) -> NSRange? {
        guard let value = attribute(element, kAXSelectedTextRangeAttribute), CFGetTypeID(value) == AXValueGetTypeID() else { return nil }
        let ax = unsafeBitCast(value, to: AXValue.self)
        var range = CFRange()
        guard AXValueGetType(ax) == .cfRange, AXValueGetValue(ax, .cfRange, &range), range.location >= 0, range.length >= 0 else { return nil }
        return NSRange(location: range.location, length: range.length)
    }
    private static func hitEditor(_ element: AXUIElement, at point: CGPoint) -> Bool {
        let system = AXUIElementCreateSystemWide()
        AXUIElementSetMessagingTimeout(system, 0.25)
        var hit: AXUIElement?
        guard AXUIElementCopyElementAtPosition(system, Float(point.x), Float(point.y), &hit) == .success else { return false }
        // Text descendants may represent the hit, but unrelated window chrome
        // and controls must not activate an editor that merely retained focus.
        for _ in 0..<16 {
            guard let current = hit else { return false }
            if CFEqual(current, element) { return true }
            AXUIElementSetMessagingTimeout(current, 0.25)
            guard let parent = attribute(current, kAXParentAttribute), CFGetTypeID(parent) == AXUIElementGetTypeID() else { return false }
            hit = unsafeBitCast(parent, to: AXUIElement.self)
        }
        return false
    }
    static func capture(activationPoint: CGPoint? = nil) -> StationDictateTarget? {
        guard AXIsProcessTrusted(), let app = NSWorkspace.shared.frontmostApplication,
              app.processIdentifier != ProcessInfo.processInfo.processIdentifier,
              let element = focusedElement(app.processIdentifier) else { return nil }
        AXUIElementSetMessagingTimeout(element, 0.25)
        if let activationPoint, !hitEditor(element, at: activationPoint) { return nil }
        let role = attribute(element, kAXRoleAttribute) as? String
        let subrole = attribute(element, kAXSubroleAttribute) as? String
        // Reject secure controls before asking for any text or selection.
        guard subrole != kAXSecureTextFieldSubrole, role == kAXTextFieldRole || role == kAXTextAreaRole,
              attribute(element, kAXEnabledAttribute) as? Bool != false else { return nil }
        var writable = DarwinBoolean(false)
        let selectedTextWritable = AXUIElementIsAttributeSettable(element, kAXSelectedTextAttribute as CFString, &writable) == .success && writable.boolValue
        let emptyValueWritable = role == kAXTextFieldRole
            && (attribute(element, kAXNumberOfCharactersAttribute) as? NSNumber)?.intValue == 0
            && AXUIElementIsAttributeSettable(element, kAXValueAttribute as CFString, &writable) == .success && writable.boolValue
        guard selectedTextWritable || emptyValueWritable,
              let original = (attribute(element, kAXValueAttribute) as? String) ?? (emptyValueWritable ? "" : nil), original.utf16.count <= 65536,
              let selection = range(element), selection.location <= original.utf16.count,
              selection.length <= original.utf16.count - selection.location else { return nil }
        let useEmptyValue = !selectedTextWritable
        if useEmptyValue && (!original.isEmpty || selection != NSRange(location: 0, length: 0)) { return nil }
        let source = original as NSString
        let start = max(0, selection.location - 1024)
        let end = min(source.length, selection.location + selection.length + 1024)
        let pid = app.processIdentifier
        return StationDictateTarget(kind: "editor", context: source.substring(with: NSRange(location: start, length: end - start))) { text in
            guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid,
                  let current = focusedElement(pid), CFEqual(current, element),
                  ((attribute(element, kAXValueAttribute) as? String) ?? (useEmptyValue && (attribute(element, kAXNumberOfCharactersAttribute) as? NSNumber)?.intValue == 0 ? "" : nil)) == original,
                  range(element) == selection else { return .unchanged }
            let expected = source.replacingCharacters(in: selection, with: text)
            // Prefer selected-text replacement; only a verified empty field uses
            // AXValue. Never mutate the clipboard, press Return or send.
            guard AXUIElementSetAttributeValue(element, (useEmptyValue ? kAXValueAttribute : kAXSelectedTextAttribute) as CFString, text as CFString) == .success else { return .unknown }
            return attribute(element, kAXValueAttribute) as? String == expected ? .applied : .unknown
        }
    }
}
#endif
