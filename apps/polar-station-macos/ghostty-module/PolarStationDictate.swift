#if POLAR_STATION
import AppKit
import ApplicationServices
import Foundation
import GhosttyKit
import CPolarStationDesktopClient
import PolarTmuxHostClient
import PolarTmuxMacUI

/// Borrowed native editor and its captured revision. Routing belongs to Rust.
@MainActor
final class StationDictateTarget {
    private(set) var value: [String: Any]
    enum Delivery: String { case applied, unchanged, unknown, no_target }
    var sourceApplied: (([String:Any])->Void)?
    private let deliver:(String,@escaping (Delivery)->Void)->Void
    func insert(_ text:String,completion:@escaping (Delivery)->Void) {deliver(text,completion)}
    init(kind:String,context:String,deliver:@escaping (String,@escaping (Delivery)->Void)->Void) {
        value=["kind":kind,"identity":UUID().uuidString,"revision":UUID().uuidString,"context":context]
        self.deliver=deliver
    }
    init(kind: String, context: String, insert: @escaping (String) -> Delivery) {
        value = ["kind": kind, "identity": UUID().uuidString,
                 "revision": UUID().uuidString, "context": context]
        self.deliver = {text,completion in completion(insert(text))}
    }
    func protectedTarget() -> StationDictateTarget { value["protected"] = true; return self }
    static func editor(_ editor: NSTextView, kind: String = "editor", preserveSelection: Bool = false, includeContext: Bool = true,
                       applied: ((String)->Void)? = nil, valid: @escaping () -> Bool = { true }) -> StationDictateTarget? {
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
            context: includeContext ? source.substring(with: NSRange(location: start, length: end - start)) : "") { [weak editor] text in
            guard let editor, editor.window != nil, editor.isEditable,
                  !editor.hasMarkedText(), valid(), editor.string == original,
                  editor.selectedRange() == range else { return .unchanged }
            // NSTextView performs one undoable replacement and informs its
            // existing delegate. Never synthesize Return or invoke Send.
            let insertion = preserveSelection ? NSRange(location: NSMaxRange(range), length: 0) : range
            let expected = source.replacingCharacters(in: insertion, with: text)
            editor.breakUndoCoalescing()
            editor.insertText(text, replacementRange: insertion)
            editor.breakUndoCoalescing()
            let actual = editor.string
            if actual != expected, ProcessInfo.processInfo.environment["POLAR_STATION_TRACE_DIR"] != nil {
                NSLog("Dictate insertion receipt: expected UTF16=%ld actual UTF16=%ld original UTF16=%ld range=%@",
                      expected.utf16.count, actual.utf16.count, original.utf16.count, NSStringFromRange(range))
            }
            if actual == expected {applied?(text);return .applied}
            return .unknown
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
    var currentTarget: ((NSTextView,Bool)->StationDictateTarget?)?
    var currentProject: (()->String?)?
    var currentConversation: (()->String?)?
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
    private var voiceSettingsLoaded = false
    private var voiceMicrophone: String?
    private var shortcutBinding = "Fn"
    private var settingsObserver: NSObjectProtocol?
    private var deletionObserver: NSObjectProtocol?
    var selectedReference: (() -> String)?
    func captureReference() -> String {
        NSApp.isActive ? (selectedReference?() ?? "") : StationDictateExternalEditors.selectedText()
    }
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
            guard let self, self.hasUnboundDraft else { return false }
            guard let target=StationDictateExternalEditors.capture(activationPoint:point) else{return false}
            self.focus(target);return true
        }
        recordingPanel=StationDictateRecordingPanel(action:{[weak self] action,text in
            if action == "stop" {self?.finish()} else if action == "cancel" {self?.cancel()} else if action == "use_available" {self?.useAvailable()} else {self?.panelAction(action,text:text)}
        })
        polar_station_dictate_speech_set_progress(worker.speech, { context,id,length,saved,preview,previewLength in
            guard let context,let id else{return}
            let owner=Unmanaged<PolarStationDictate>.fromOpaque(context).takeUnretainedValue()
            let operation=String(decoding:UnsafeBufferPointer(start:id,count:length),as:UTF8.self)
            let text = preview.map { String(decoding:UnsafeBufferPointer(start:$0,count:previewLength),as:UTF8.self) } ?? ""
            DispatchQueue.main.async { [weak owner] in
                guard let owner,!owner.stopped else{return}
                if text.isEmpty { owner.input(["type":"audio_saved","operation_id":operation,"saved_samples":saved]) }
                else { owner.input(["type":"transcript_preview","operation_id":operation,"text":text,"saved_samples":saved]) }
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
        refreshHistory() // Processing results are subscribed independently of Settings.
    }
    func storeLearning(_ sample:[String:Any],completion:@escaping (Bool)->Void) {
        guard let data=try? JSONSerialization.data(withJSONObject:["op":"learn_refine","sample":sample]) else {completion(false);return}
        DispatchQueue.global(qos:.utility).async { [worker] in
            let output=worker.service(data)
            let value=output.flatMap{try? JSONSerialization.jsonObject(with:$0) as? [String:Any]}
            let success=value?["ok"] as? Bool == true && (value?["payload"] as? [String:Any])?["stored"] as? Bool == true
            DispatchQueue.main.async{completion(success)}
        }
    }
    func setRefineMode(contextID:String,english:Bool) {input(["type":"set_refine_mode","context_id":contextID,"english":english])}
    func refineEnglish(contextID:String)->Bool {(view["refine_modes"] as? [String:Bool])?[contextID] == true}
    var collectsContext: Bool { (view["reference_policy"] as? [String:Any])?["surrounding"] as? Bool ?? false }
    var presentation: [String: Any] { view["presentation"] as? [String: Any] ?? [:] }
    var enabled: Bool { (presentation["enabled"] as? Bool ?? false) && (submittedID == nil || presentation["action"] as? String == "stop") }
    var status: String { error ?? presentation["status"] as? String ?? "" }
    func refine(target: StationDictateTarget, text: String, contextID:String = "", project:String? = nil) {
        guard enabled, presentation["action"] as? String == "start", !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        let id = UUID().uuidString
        submittedID = id
        captured = [target.value["identity"] as? String ?? "": target]
        input(["type":"refine_context", "context_id":contextID, "project":project as Any? ?? NSNull(), "operation_id":id, "target":target.value, "text":text])
    }
    func toggle(targets: [StationDictateTarget], reference: String = "") {
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
        input(["type":"start_context", "conversation_context":(collectsContext ? currentConversation?() : nil) as Any? ?? NSNull(), "project":currentProject?() as Any? ?? NSNull(), "operation_id":id, "targets":targets.map(\.value), "reference":reference])
    }
    // Physical edges are latched before the asynchronous Rust transition.
    // This is an input handle, not a second recording lifecycle.
    func shortcut(pressed: Bool) {
        guard !stopped else { return }
        let intent = polar_station_shortcut_gesture_event(gesture, pressed ? 1 : 2,
            submittedID != nil && presentation["action"] as? String != "wait",
            (enabled && presentation["action"] as? String == "start") || presentation["action"] as? String == "retry_open", presentation["use_available_id"] is String)
        if intent == 2 && pressed { heldID = submittedID }
        executeShortcut(intent)
    }
    private func executeShortcut(_ intent: UInt32) {
        switch intent {
        case 1:
            let reference = captureReference()
            toggle(targets: NSApp.isActive ? (shortcutTargets?() ?? []) : StationDictateExternalEditors.capture(includeContext:collectsContext).map { [$0] } ?? [], reference: reference)
            heldID = submittedID
        case 2:
            guard let id = heldID, submittedID == id else { return }
            heldID = nil
            if recordingStart == nil { cancelled.insert(id); audio.cancel() }
            let duration = recordingStart.map { min(600_000, Int((ProcessInfo.processInfo.systemUptime - $0) * 1000)) } ?? 0
            input(["type":"stop", "operation_id":id, "duration_ms":duration])
        case 3:
            heldID = nil
            useAvailable()
        default: break
        }
    }
    private let shortcutCapture = StationDictateKeyCapture()
    func handleShortcut(_ event: NSEvent) -> Bool {
        guard !stopped else { return false }
        return shortcutCapture.handle(event, binding: shortcutBinding) { [weak self] event in
            guard let self else { return }
            if event == 4 { cancel() }
            let intent = polar_station_shortcut_gesture_event(gesture, event,
                submittedID != nil && presentation["action"] as? String != "wait",
                (enabled && presentation["action"] as? String == "start") || presentation["action"] as? String == "retry_open", presentation["use_available_id"] is String)
            if intent == 2 && (event == 1 || event == 3) { heldID = submittedID }
            executeShortcut(intent)
        }
    }
    private func reloadVoiceSettings() {
        Task {let client=VoiceClient();let data=await client.request(Data(#"{"op":"voice_settings"}"#.utf8))
            if let reply=try? JSONSerialization.jsonObject(with:data) as? [String:Any],let value=reply["payload"] as? [String:Any] {
                input(["type":"refresh_settings"])
                voiceMicrophone=value["microphone"] as? String
                voiceSettingsLoaded = true
                if let shortcut=value["shortcut_mac"] as? String,StationDictateKeyCapture.binding(shortcut) != nil {
                    shortcutBinding=shortcut
                    GlobalEventTap.shared.enableStationDictate(prompt: false)
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
            reply(["message":presentation["shortcut_status"] as? String ?? "Shortcut applied: "+text])
        }
    }
    func shortcutStatus(_ status: String) {
        let status = status == "ready" && shortcutBinding == "Fn" && !StationDictateKeyCapture.systemGlobeActionDisabled
            ? "fn_setup_required" : status
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
        ExternalTextInsertion.cancel()
        input(["type":"cancel", "operation_id":id])
    }
    func stop() {
        guard !stopped else { return }
        polar_station_shortcut_gesture_free(gesture)
        polar_station_dictate_speech_set_progress(worker.speech,nil,nil)
        polar_station_dictate_speech_set_history(worker.speech,nil,nil)
        externalEditors?.stop(); externalEditors = nil
        ExternalTextInsertion.cancel()
        recordingPanel?.orderOut(nil)
        cancel(); stopped = true; audio.cancel(); changed = nil
        if let settingsObserver {NotificationCenter.default.removeObserver(settingsObserver)}
        if let deletionObserver {NotificationCenter.default.removeObserver(deletionObserver)}
        if let events { NSEvent.removeMonitor(events) }; events = nil
    }
    func useAvailable() {
        guard let id = presentation["use_available_id"] as? String else { return }
        input(["type":"use_available", "operation_id":id])
    }
    func useOriginal() {
        guard let id = presentation["use_original_id"] as? String else { return }
        input(["type":"use_original", "operation_id":id])
    }
    func reviewResult(_ id:String) { input(["type":"show_panel","operation_id":id]) }
    func refreshHistory() {
        polar_station_dictate_speech_set_history(worker.speech, { context, bytes, count in
            guard let context, let bytes,
                  let value = try? JSONSerialization.jsonObject(with: Data(bytes: bytes, count: count)) as? [String:Any] else { return }
            let owner = Unmanaged<PolarStationDictate>.fromOpaque(context).takeUnretainedValue()
            DispatchQueue.main.async { [weak owner] in
                guard let owner, !owner.stopped else { return }
                owner.input(value)
            }
        }, Unmanaged.passUnretained(self).toOpaque())
        input(["type":"history_open"])
    }
    func historyCommand(_ command: String, id: String? = nil, original:Bool=false) {
        if let id, command == "history_insert" || command == "history_retry" { cancelled.remove(id) }
        var value: [String:Any] = ["type":command]
        if command=="history_copy"{value["original"]=original}
        if let id { value["operation_id"] = id }
        input(value)
    }
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
    fileprivate func input(_ value: [String: Any], opening: Bool = false) {
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
           ["delivered", "copied", "unknown", "review", "pending", "cancelled", "failed"].contains(entry["phase"] as? String ?? "") { submittedID = nil }
        if presentation["setup_required"] as? Bool == true,
           let entry=(result["entries"] as? [[String:Any]])?.last,
           let operation=entry["operation_id"] as? String, shownSetupOperation != operation {
            shownSetupOperation=operation;showVoiceSettings?()
        }
        recordingPanel?.update(result["panel"] as? [String:Any])
        changed?()
        for effect in result["effects"] as? [[String: Any]] ?? [] { execute(effect) }
    }
    private func currentInputTarget(preserveSelection: Bool) -> StationDictateTarget? {
        // This is sampled when text is ready, not when recording began.
        if recordingPanel?.isKeyWindow == true { return nil }
        if NSWorkspace.shared.frontmostApplication?.processIdentifier == ProcessInfo.processInfo.processIdentifier {
            guard let window=NSApp.keyWindow, window.isVisible, window.occlusionState.contains(.visible), window !== recordingPanel else {return nil}
            if let editor=window.firstResponder as? NSTextView, !editor.isHiddenOrHasHiddenAncestor, !editor.visibleRect.isEmpty {
                return currentTarget?(editor,preserveSelection) ?? StationDictateTarget.editor(editor, preserveSelection: preserveSelection)
            }
            return shortcutTargets?().first(where: { $0.value["kind"] as? String == "idea_node" })
        }
        return StationDictateExternalEditors.capture(preserveSelection: preserveSelection)
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
        case "copy":
            if effect["stop_capture"] as? Bool == true, captureID == id {
                _ = audio.finish(operation: id); captureID = nil
            }
            cancelled.insert(id)
            cancelSpeech(id)
            let text=effect["text"] as? String ?? ""
            NSPasteboard.general.clearContents()
            let copied=NSPasteboard.general.setString(text,forType:.string) && NSPasteboard.general.string(forType:.string)==text
            input(["type":"clipboard","operation_id":id,"copied":copied])
        case "capture":
            guard voiceSettingsLoaded else { reloadVoiceSettings(); input(["type":"failed", "operation_id":id,"error":"Voice settings are not ready. Open Settings → Voice and retry."]); return }
            guard !cancelled.contains(id) else { return }
            captureID = id; recordingStart = nil
            let begin: [String: Any] = ["op":"capture_begin", "id":id, "context":effect["context"] ?? NSNull(), "project":effect["project"] ?? NSNull(), "reference":effect["reference"] ?? ""]
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
                    MicrophonePermissionPresentation.showCaptureFailureIfNeeded()
                })
        case "cancel":
            cancelled.insert(id)
            if captureID == id { audio.cancel(); captureID = nil }
            cancelSpeech(id)
        case "finish_capture":
            guard captureID == id else {return}
            _ = audio.finish(operation:id);captureID=nil
            let ended=Array(id.utf8).withUnsafeBufferPointer {
                polar_station_dictate_speech_finish(worker.speech,$0.baseAddress,$0.count)
            }
            if !ended {input(["type":"failed","operation_id":id,"error":"Audio input could not finish; saved audio retained."])}
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
            guard let request = try? JSONSerialization.data(withJSONObject: ["op":effect["saved_recording"] as? Bool == false ? "refine_text" : "rewrite_result", "id":id,"context":effect["context"] ?? "", "raw":effect["raw"] ?? "", "english":effect["english"] as? Bool ?? false,"scope":effect["scope"] ?? NSNull(),"project":effect["project"] ?? NSNull()]) else { return }
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
            let target=currentInputTarget(preserveSelection:effect["preserve_selection"] as? Bool ?? false)
            deliver(id:id,target:target,source:effect["source"] as? [String:Any],text:effect["text"] as? String ?? "",missing:.no_target)
        case "insert":
            let target=effect["target"] as? [String:Any] ?? [:]
            let identity=target["identity"] as? String ?? ""
            deliver(id:id,target:captured[identity],source:nil,text:effect["text"] as? String ?? "",missing:.unchanged)
        default: break
        }
    }
    private func deliver(id:String,target:StationDictateTarget?,source:[String:Any]?,text:String,missing:StationDictateTarget.Delivery) {
        guard !cancelled.contains(id),let target else {
            input(["type":"delivery","operation_id":id,"outcome":cancelled.contains(id) ? "unchanged":missing.rawValue]);captured.removeAll();return
        }
        target.insert(text) {[weak self,target] outcome in
            guard let self,!self.stopped else{return}
            if outcome == .applied,let source{target.sourceApplied?(source)}
            self.input(["type":"delivery","operation_id":id,"outcome":outcome.rawValue])
            self.captured.removeAll()
        }
    }

}

@MainActor
private final class StationDictateRecordingPanel: NSPanel, NSTextViewDelegate {
    private let referenceLabel = NSTextField(labelWithString: "")

    private let more=NSButton(title:"⋯",target:nil,action:nil)
    private let actions=NSStackView()
    private var textHeight:NSLayoutConstraint!
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
        super.init(contentRect:NSRect(x:0,y:0,width:360,height:48),styleMask:[.nonactivatingPanel,.borderless],backing:.buffered,defer:false)
        title="Dictate";level = .floating;hidesOnDeactivate=false;isReleasedWhenClosed=false
        isOpaque=false;backgroundColor = .clear;hasShadow=true;isMovableByWindowBackground=true
        collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary]
        status.maximumNumberOfLines=1;status.lineBreakMode = .byTruncatingTail
        status.textColor = .white;status.setAccessibilityLabel("Dictate status")
        status.setContentCompressionResistancePriority(.defaultLow,for:.horizontal)
        status.setContentHuggingPriority(.defaultLow,for:.horizontal)
        editor.isRichText=false;editor.isEditable=false;editor.isSelectable=true;editor.delegate=self
        editor.frame=NSRect(x:0,y:0,width:336,height:20);editor.isVerticallyResizable=true;editor.isHorizontallyResizable=false
        editor.autoresizingMask=[.width];editor.textContainer?.widthTracksTextView=true;editor.allowsUndo=true
        editor.font = .systemFont(ofSize:14);editor.textColor = .white;editor.drawsBackground=false
        editor.textContainerInset = .zero;editor.textContainer?.lineFragmentPadding=0;editor.setAccessibilityLabel("Dictate result")
        scroll.documentView=editor;scroll.hasVerticalScroller=true;scroll.borderType = .noBorder;scroll.drawsBackground=false
        scroll.translatesAutoresizingMaskIntoConstraints=false
        textHeight=scroll.heightAnchor.constraint(equalToConstant:20);textHeight.isActive=true
        let header=NSStackView();header.orientation = .horizontal;header.distribution = .fill;header.spacing=6;header.addArrangedSubview(status)
        actions.orientation = .horizontal;actions.spacing=6
        for (kind,title) in [("stop","Stop"),("cancel","Cancel"),("use_available","Use text now"),("copy","Copy"),("edit","Edit"),("insert_draft","Insert"),("retry_saved","Retry"),("hide_panel","Close")] {
            let button=NSButton(title:title,target:self,action:#selector(clicked(_:)))
            button.identifier=NSUserInterfaceItemIdentifier(kind);button.refusesFirstResponder=true
            button.isBordered=false;button.contentTintColor = .white
            controls[kind]=button
            if kind == "stop" {header.addArrangedSubview(button)}
            else if kind == "copy" || kind == "insert_draft" {actions.addArrangedSubview(button)}
        }
        more.target=self;more.action=#selector(showMore(_:));more.refusesFirstResponder=true
        more.isBordered=false;more.contentTintColor = .white;more.setAccessibilityLabel("More Dictate actions")
        header.addArrangedSubview(more)
        referenceLabel.maximumNumberOfLines=1;referenceLabel.lineBreakMode = .byTruncatingTail;referenceLabel.textColor = .lightGray;referenceLabel.setAccessibilityLabel("Quoted text")
        let column=NSStackView(views:[header,referenceLabel,scroll,actions]);column.orientation = .vertical;column.alignment = .leading;column.spacing=8
        column.edgeInsets=NSEdgeInsets(top:12,left:12,bottom:12,right:12)
        column.wantsLayer=true;column.layer?.backgroundColor=NSColor(calibratedWhite:0.09,alpha:0.8).cgColor
        column.layer?.cornerRadius=12;contentView=column
        for view in [header,referenceLabel,scroll] {view.widthAnchor.constraint(equalTo:column.widthAnchor,constant:-24).isActive=true}
    }
    @objc private func showMore(_ sender:NSButton) {
        let menu=NSMenu()
        for kind in ["edit","use_available","retry_saved","cancel","hide_panel"] {
            guard let control=controls[kind],!control.isHidden else {continue}
            let item=NSMenuItem(title:control.title,action:#selector(menuAction(_:)),keyEquivalent:"")
            item.target=self;item.representedObject=kind;menu.addItem(item)
        }
        menu.popUp(positioning:nil,at:NSPoint(x:0,y:sender.bounds.maxY),in:sender)
    }
    @objc private func menuAction(_ sender:NSMenuItem) {if let kind=sender.representedObject as? String {performAction(kind)}}
    override func close(){ action("hide_panel",nil);orderOut(nil) }
    func textDidChange(_ notification:Notification){if !updating && editing {action("edit_draft",editor.string)}}
    @objc private func clicked(_ sender:NSButton) {
        guard let kind=sender.identifier?.rawValue else{return}
        performAction(kind)
    }
    private func performAction(_ kind:String) {
        switch kind {
        case "copy":action("copy_draft",editor.string)
        case "edit":editing=true;editor.isEditable=true;makeKeyAndOrderFront(nil);makeFirstResponder(editor)
        default:action(kind,nil)
        }
    }
    func update(_ panel:[String:Any]?) {
        guard let panel,let id=panel["operation_id"] as? String else {hideTimer?.invalidate();hideTimer=nil;hidingID=nil;orderOut(nil);return}
        if currentID != id {currentID=id;editing=false;editor.isEditable=false}
        status.stringValue=panel["heading"] as? String ?? "Dictate"
        status.toolTip=panel["status"] as? String
        let text=panel["text"] as? String ?? ""
        if editor.string != text {
            let origin=scroll.contentView.bounds.origin
            updating=true
            if text.hasPrefix(editor.string) {editor.textStorage?.append(NSAttributedString(string:String(text.dropFirst(editor.string.count)),attributes:[.font:NSFont.systemFont(ofSize:14),.foregroundColor:NSColor.white]))}
            else {editor.string=text}
            updating=false;scroll.contentView.scroll(to:origin)
        }
        if let container=editor.textContainer,let layout=editor.layoutManager {
            layout.ensureLayout(for:container)
            let lineHeight=ceil(layout.defaultLineHeight(for:editor.font ?? .systemFont(ofSize:14)))
            textHeight.constant=min(4*lineHeight,max(lineHeight,ceil(layout.usedRect(for:container).height)))
        }
        scroll.isHidden=text.isEmpty && !editing
        for (kind,key,label) in [("stop","can_stop","stop_label"),("cancel","can_cancel","cancel_label"),("use_available","can_available","available_label"),("copy","can_copy","copy_label"),("edit","can_edit","edit_label"),("insert_draft","can_insert","insert_label"),("retry_saved","can_retry","retry_label")] {
            controls[kind]?.isHidden=panel[key] as? Bool != true
            controls[kind]?.title=panel[label] as? String ?? ""
        }
        if panel["can_edit"] as? Bool != true {editing=false;editor.isEditable=false}
        referenceLabel.stringValue=panel["reference"] as? String ?? ""
        referenceLabel.toolTip=referenceLabel.stringValue
        referenceLabel.isHidden=referenceLabel.stringValue.isEmpty
        actions.isHidden=panel["can_copy"] as? Bool != true && panel["can_insert"] as? Bool != true
        let height:CGFloat=48 + (referenceLabel.isHidden ? 0 : 24) + (scroll.isHidden ? 0 : textHeight.constant+8) + (actions.isHidden ? 0 : 28)
        // Grow upward from the bottom anchor so newly revealed actions stay on screen.
        var origin=frame.origin
        if let visible=(screen ?? NSScreen.main)?.visibleFrame {
            if !isVisible {origin=NSPoint(x:visible.midX-180,y:visible.minY+70)}
            origin.x=max(visible.minX,min(origin.x,visible.maxX-360))
            origin.y=max(visible.minY,min(origin.y,visible.maxY-height))
        }
        setFrame(NSRect(origin:origin,size:NSSize(width:360,height:height)),display:true)
        if !isVisible {orderFrontRegardless()}
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
private final class StationDictateHistoryRow: NSButton {
    private let previewLabel = NSTextField(labelWithString: "")
    private let metadataLabel = NSTextField(labelWithString: "")
    required init?(coder: NSCoder) { nil }
    init(target: AnyObject, action: Selector) {
        super.init(frame: .zero)
        title = ""; self.target = target; self.action = action
        setButtonType(.pushOnPushOff); bezelStyle = .regularSquare; refusesFirstResponder = true
        setAccessibilityElement(true)
        setAccessibilityRole(.button)
        previewLabel.font = .systemFont(ofSize: 13)
        metadataLabel.font = .systemFont(ofSize: 11)
        let column = NSStackView(views: [previewLabel, metadataLabel])
        column.orientation = .vertical; column.alignment = .leading; column.spacing = 2
        column.translatesAutoresizingMaskIntoConstraints = false; addSubview(column)
        for label in [previewLabel, metadataLabel] {
            label.maximumNumberOfLines = 1; label.lineBreakMode = .byTruncatingTail
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            label.setAccessibilityElement(false)
            label.widthAnchor.constraint(equalTo: column.widthAnchor).isActive = true
        }
        NSLayoutConstraint.activate([
            column.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            column.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            column.centerYAnchor.constraint(equalTo: centerYAnchor),
            heightAnchor.constraint(equalToConstant: 46),
        ])
    }
    func update(preview: String, metadata: String) {
        if previewLabel.stringValue != preview { previewLabel.stringValue = preview }
        if metadataLabel.stringValue != metadata { metadataLabel.stringValue = metadata }
        let summary = preview + "\n" + metadata
        if toolTip != summary { setAccessibilityLabel(summary); toolTip = summary }
    }
    func select(_ selected: Bool) {
        let next: NSControl.StateValue = selected ? .on : .off
        if state != next { state = next }
        let font = NSFont.systemFont(ofSize: 13, weight: selected ? .semibold : .regular)
        if previewLabel.font != font { previewLabel.font = font }
    }
    override func hitTest(_ point: NSPoint) -> NSView? {
        super.hitTest(point) == nil ? nil : self
    }
}

@MainActor
final class StationDictateControls: NSStackView {
    var targets: (() -> [StationDictateTarget])?
    var activateInput: (() -> Void)?
    weak var dictate: PolarStationDictate?
    var refineContext:(()->String)?
    var refine: (() -> Void)?
    private let refineButton = NSButton()
    private var cleanupRequested=false
    private let microphone = NSButton()
    private let history = NSButton(title: "▾", target: nil, action: nil)
    private let popover = NSPopover()
    private let list = NSStackView()
    private let listScroll = NSScrollView()
    private let detail = NSStackView()
    private let variants = NSSegmentedControl(labels: ["Original", "Refined"], trackingMode: .selectOne, target: nil, action: nil)
    private let content = NSTextView()
    private let errorLabel = NSTextField(wrappingLabelWithString: "")
    private let shortcutLabel = NSTextField(wrappingLabelWithString: "")
    private let emptyLabel = NSTextField(labelWithString: "")
    private var buttons: [String:NSButton] = [:]
    private var rows: [String:StationDictateHistoryRow] = [:]
    private var actions: [NSButton:() -> Void] = [:]
    private var selectedID: String?
    required init?(coder: NSCoder) { nil }
    init(dictate: PolarStationDictate) {
        self.dictate = dictate
        super.init(frame: .zero)
        orientation = .horizontal; spacing = 0; distribution = .fill
        microphone.bezelStyle = .rounded; microphone.refusesFirstResponder = true
        microphone.target = self; microphone.action = #selector(toggle)
        history.refusesFirstResponder = true; history.target = self; history.action = #selector(openHistory)
        refineButton.target = self; refineButton.action = #selector(refinePressed)
        refineButton.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "Refine draft")
        refineButton.imagePosition = .imageOnly; refineButton.toolTip = "Refine draft"; refineButton.setAccessibilityLabel("Refine draft")
        refineButton.refusesFirstResponder = true; refineButton.bezelStyle = .rounded
        addArrangedSubview(refineButton); addArrangedSubview(microphone); addArrangedSubview(history)
        refineButton.widthAnchor.constraint(equalToConstant: 44).isActive = true
        let press=NSPressGestureRecognizer(target:self,action:#selector(refineMenu(_:)));press.minimumPressDuration=0.5;press.delaysPrimaryMouseButtonEvents=true;refineButton.addGestureRecognizer(press)
        let micPress=NSPressGestureRecognizer(target:self,action:#selector(microphoneMenu(_:)));micPress.minimumPressDuration=0.5;micPress.delaysPrimaryMouseButtonEvents=true;microphone.addGestureRecognizer(micPress)
        microphone.widthAnchor.constraint(equalToConstant: 30).isActive = true
        history.widthAnchor.constraint(equalToConstant: 18).isActive = true
        popover.behavior = .transient
        buildHistory()
        refresh()
    }
    private func button(_ key: String, _ action: @escaping () -> Void) -> NSButton {
        let button = NSButton(title: "", target: self, action: #selector(historyAction(_:)))
        button.refusesFirstResponder = true; actions[button] = action; buttons[key] = button
        return button
    }
    private func buildHistory() {
        let column = NSStackView(); column.orientation = .vertical; column.alignment = .leading; column.spacing = 8
        column.edgeInsets = NSEdgeInsets(top: 10, left: 10, bottom: 10, right: 10)
        column.addArrangedSubview(NSTextField(labelWithString: "Dictate history"))
        shortcutLabel.maximumNumberOfLines=0; shortcutLabel.preferredMaxLayoutWidth=340
        shortcutLabel.font = .systemFont(ofSize:NSFont.smallSystemFontSize)
        column.addArrangedSubview(shortcutLabel)
        errorLabel.textColor = .systemOrange; errorLabel.maximumNumberOfLines=0; errorLabel.preferredMaxLayoutWidth=340; errorLabel.isHidden = true; column.addArrangedSubview(errorLabel)
        column.addArrangedSubview(button("retry_open") { [weak self] in self?.dictate?.historyCommand("retry_open") })
        list.orientation = .vertical; list.alignment = .leading; list.spacing = 4
        listScroll.hasVerticalScroller = true; listScroll.documentView = list
        listScroll.translatesAutoresizingMaskIntoConstraints = false
        list.translatesAutoresizingMaskIntoConstraints = false
        list.widthAnchor.constraint(equalTo: listScroll.contentView.widthAnchor).isActive = true
        listScroll.heightAnchor.constraint(equalToConstant: 130).isActive = true
        column.addArrangedSubview(listScroll)
        column.addArrangedSubview(button("more") { [weak self] in self?.dictate?.historyCommand("history_more") })
        detail.orientation = .vertical; detail.alignment = .leading; detail.spacing = 6
        variants.target = self; variants.action = #selector(variantChanged); detail.addArrangedSubview(variants)
        content.isEditable = false; content.isSelectable = true; content.isRichText = false
        content.font = .systemFont(ofSize: NSFont.systemFontSize)
        content.isHorizontallyResizable = false; content.autoresizingMask = [.width]
        content.isVerticallyResizable = true
        content.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude,height: CGFloat.greatestFiniteMagnitude)
        content.textContainer?.containerSize = NSSize(width:340,height:CGFloat.greatestFiniteMagnitude)
        content.textContainer?.widthTracksTextView = true
        let scroll = NSScrollView(); scroll.hasVerticalScroller = true; scroll.documentView = content
        scroll.heightAnchor.constraint(equalToConstant: 140).isActive = true
        detail.addArrangedSubview(scroll)
        let operations = NSStackView(); operations.spacing = 6
        operations.addArrangedSubview(button("copy") { [weak self] in
            guard let self,let id=self.selectedID else { return }
            self.dictate?.historyCommand("history_copy",id:id,original:self.variants.selectedSegment==0)
        })
        for (key, command) in [("insert","history_insert"),("retry","history_retry"),("dismiss","history_dismiss")] {
            operations.addArrangedSubview(button(key) { [weak self] in
                guard let self, let id = self.selectedID else { return }
                // Close before resolving the current visible input; the popover
                // is a reader and must never become the insertion destination.
                self.popover.performClose(nil)
                if command == "history_insert" { self.activateInput?() }
                self.dictate?.historyCommand(command, id: id)
            })
        }
        detail.addArrangedSubview(operations)
        let maintenance=NSStackView();maintenance.spacing=6
        maintenance.addArrangedSubview(button("pin") { [weak self] in
            guard let self,let id=self.selectedID,let value=self.dictate?.view["history_detail"] as? [String:Any] else{return}
            self.dictate?.input(["type":"history_pin","operation_id":id,"pinned":value["pinned"] as? Bool != true])
        })
        maintenance.addArrangedSubview(button("delete") { [weak self] in
            guard let self,let id=self.selectedID,let ui=self.dictate?.view["delete_ui"] as? [String:Any] else{return}
            let alert=NSAlert();alert.messageText=ui["confirmation"] as? String ?? "";alert.informativeText=(ui["summary"] as? String ?? "")+"\n\n"+(ui["detail"] as? String ?? "")
            alert.addButton(withTitle:ui["confirm_label"] as? String ?? "");alert.addButton(withTitle:ui["cancel_label"] as? String ?? "")
            if alert.runModal() == .alertFirstButtonReturn {self.dictate?.input(["type":"history_delete","operation_id":id,"confirmation":true])}
        })
        detail.addArrangedSubview(maintenance);column.addArrangedSubview(detail)
        let separator = NSBox(); separator.boxType = .separator; column.addArrangedSubview(separator)
        column.addArrangedSubview(button("cleanup") { [weak self] in
            self?.cleanupRequested=true;self?.dictate?.input(["type":"cleanup_preview"])
        })
        column.addArrangedSubview(button("settings") { [weak self] in
            guard let self else { return }
            self.popover.performClose(nil); self.dictate?.showVoiceSettings?()
        })
        column.widthAnchor.constraint(equalToConstant: 360).isActive = true
        for view in [shortcutLabel,errorLabel,listScroll,detail,separator] {
            view.widthAnchor.constraint(equalTo: column.widthAnchor, constant: -20).isActive = true
        }
        for view in [scroll,operations] { view.widthAnchor.constraint(equalTo: detail.widthAnchor).isActive = true }
        let controller = NSViewController(); controller.view = column; popover.contentViewController = controller
    }
    func refresh() {
        guard let dictate else { return }
        let presentation = dictate.presentation
        let english=dictate.refineEnglish(contextID:refineContext?() ?? "")
        refineButton.title=english ? "EN" : "";refineButton.imagePosition=english ? .imageLeading : .imageOnly
        refineButton.toolTip=english ? "Refine + English · Hold to choose mode" : "Refine · Hold to choose mode"
        refineButton.isEnabled = dictate.enabled && presentation["action"] as? String == "start"
        microphone.isEnabled = dictate.enabled
        let label = presentation["button_label"] as? String ?? ""
        let outputLabel=(dictate.view["output_modes"] as? [[String:String]])?.first(where:{$0["mode"] == dictate.view["output_mode"] as? String})?["label"] ?? ""
        microphone.toolTip = label + " · " + outputLabel + " · Hold to choose mode · " + dictate.status; microphone.setAccessibilityLabel(microphone.toolTip)
        let state = dictate.view["microphone_state"] as? String ?? "unavailable"
        microphone.image = NSImage(systemSymbolName: state == "recording" ? "stop.fill" : state == "unavailable" ? "mic.slash" : "mic.fill", accessibilityDescription: label)
        microphone.contentTintColor = state == "recording" ? .systemRed : state == "ready" ? .controlAccentColor : .secondaryLabelColor
        history.toolTip = presentation["history_title"] as? String; history.setAccessibilityLabel(history.toolTip)
        shortcutLabel.stringValue = (presentation["shortcut_status"] as? String ?? "") + "\n" + (presentation["shortcut_hint"] as? String ?? "")
        if let id=dictate.view["copy_completed"] as? String,id==selectedID {popover.performClose(nil)}
        if cleanupRequested,let response=dictate.view["cleanup_preview"] as? [String:Any],let ui=response["presentation"] as? [String:Any] {
            cleanupRequested=false
            let alert=NSAlert();alert.messageText=ui["confirmation"] as? String ?? "";alert.informativeText=(ui["summary"] as? String ?? "")+"\n\n"+(ui["detail"] as? String ?? "")
            if ui["can_apply"] as? Bool == true {alert.addButton(withTitle:ui["confirm_label"] as? String ?? "");alert.addButton(withTitle:ui["cancel_label"] as? String ?? "");if alert.runModal() == .alertFirstButtonReturn {dictate.input(["type":"cleanup_apply","confirmation":true])}}
            else {alert.addButton(withTitle:"OK");alert.runModal()}
        }
        if popover.isShown { updateHistory() }
    }
    @objc private func refinePressed() { refine?() }
    @objc private func toggle() {
        guard let dictate else { return }
        let reference = dictate.captureReference()
        dictate.toggle(targets: dictate.presentation["action"] as? String == "start" ? targets?() ?? [] : [], reference: reference)
    }
    @objc private func microphoneMenu(_ gesture:NSPressGestureRecognizer) {
        guard gesture.state == .began,let dictate,let choices=dictate.view["output_modes"] as? [[String:Any]] else{return}
        let menu=NSMenu()
        for choice in choices {
            guard let mode=choice["mode"] as? String,let label=choice["label"] as? String else{continue}
            let item=NSMenuItem(title:label,action:#selector(microphoneModeSelected(_:)),keyEquivalent:"")
            item.target=self;item.representedObject=mode;item.state=(dictate.view["output_mode"] as? String == mode) ? .on:.off;menu.addItem(item)
        }
        menu.popUp(positioning:nil,at:NSPoint(x:0,y:microphone.bounds.maxY),in:microphone)
    }
    @objc private func microphoneModeSelected(_ item:NSMenuItem) {
        guard let mode=item.representedObject as? String else{return}
        dictate?.input(["type":"set_output_mode","mode":mode])
    }
    @objc private func refineMenu(_ gesture:NSPressGestureRecognizer) {
        guard gesture.state == .began,let context=refineContext?(),!context.isEmpty else{return}
        let menu=NSMenu()
        for (index,title) in ["Refine","Refine + English"].enumerated() {
            let item=NSMenuItem(title:title,action:#selector(refineModeSelected(_:)),keyEquivalent:"")
            item.target=self;item.tag=index;item.representedObject=context
            item.state=(dictate?.refineEnglish(contextID:context) == (index==1)) ? .on : .off
            menu.addItem(item)
        }
        menu.popUp(positioning:nil,at:NSPoint(x:0,y:refineButton.bounds.maxY),in:refineButton)
    }
    @objc private func refineModeSelected(_ item:NSMenuItem) {
        guard let context=item.representedObject as? String else{return}
        dictate?.setRefineMode(contextID:context,english:item.tag==1)
    }
    @objc private func openHistory() {
        dictate?.refreshHistory(); updateHistory()
        popover.show(relativeTo: bounds, of: self, preferredEdge: .maxY)
    }
    private func updateHistory() {
        guard let dictate else { return }
        let view = dictate.view
        let ui = view["history_ui"] as? [String:String] ?? [:]
        content.setAccessibilityLabel(ui["detail"])
        for (key, button) in buttons { button.title = ui[key] ?? "" }
        emptyLabel.stringValue = ui["empty"] ?? ""
        let error = view["history_error"] as? String ?? ""
        errorLabel.stringValue = error; errorLabel.isHidden = error.isEmpty
        buttons["retry_open"]?.isHidden = view["history_retry_open"] as? Bool != true
        let items = view["history_rows"] as? [[String:Any]] ?? []
        let ids = Set(items.compactMap { $0["operation_id"] as? String })
        for id in Array(rows.keys) where !ids.contains(id) {
            if let row = rows.removeValue(forKey: id) { list.removeArrangedSubview(row); row.removeFromSuperview(); actions.removeValue(forKey: row) }
        }
        if !items.isEmpty {
            if list.arrangedSubviews.contains(where: { $0 === emptyLabel }) { list.removeArrangedSubview(emptyLabel) }
            emptyLabel.removeFromSuperview()
        }
        else if emptyLabel.superview == nil { list.addArrangedSubview(emptyLabel) }
        let previous = listScroll.contentView.bounds.origin
        for (index,item) in items.enumerated() {
            guard let id = item["operation_id"] as? String else { continue }
            let row: StationDictateHistoryRow
            if let existing = rows[id] { row = existing }
            else {
                row = StationDictateHistoryRow(target: self, action: #selector(historyAction(_:)))
                rows[id] = row; actions[row] = { [weak self] in self?.dictate?.historyCommand("history_select",id:id) }
                list.addArrangedSubview(row)
                row.widthAnchor.constraint(equalTo: list.widthAnchor).isActive = true
            }
            if index >= list.arrangedSubviews.count || list.arrangedSubviews[index] !== row {
                list.removeArrangedSubview(row); list.insertArrangedSubview(row, at: min(index,list.arrangedSubviews.count))
            }
            let date: String
            if let ms = item["created_at_ms"] as? NSNumber {
                date = DateFormatter.localizedString(from: Date(timeIntervalSince1970: ms.doubleValue / 1000),dateStyle:.short,timeStyle:.short)
            } else { date = ui["earlier"] ?? "" }
            let seconds = (item["duration_ms"] as? NSNumber)?.intValue ?? 0
            let kind = item["kind"] as? String == "text" ? ui["text"] ?? "" : String(format:"%d:%02d",seconds/60000,(seconds/1000)%60)
            row.update(preview: item["preview"] as? String ?? "", metadata: "\(date) · \(kind) · \(item["mode_label"] as? String ?? "") · \(item["status"] as? String ?? "")")
        }
        buttons["more"]?.isHidden = view["history_more"] as? Bool != true
        let value = view["history_detail"] as? [String:Any]
        let nextID = value?["operation_id"] as? String
        if selectedID != nextID { variants.selectedSegment = value?["can_refined"] as? Bool == true ? 1 : 0 }
        selectedID = nextID; detail.isHidden = value == nil
        for (id,row) in rows { row.select(id == selectedID) }
        variants.setLabel(ui["original"] ?? "",forSegment:0); variants.setLabel(ui["refined"] ?? "",forSegment:1)
        variants.setEnabled(value?["can_original"] as? Bool == true,forSegment:0)
        variants.setEnabled(value?["can_refined"] as? Bool == true,forSegment:1)
        buttons["copy"]?.isEnabled = value?["can_copy"] as? Bool == true
        buttons["insert"]?.isHidden = value?["can_recover"] as? Bool != true && value?["can_insert"] as? Bool != true
        buttons["retry"]?.title = value?["retry_label"] as? String ?? "Retry"
        buttons["retry"]?.isHidden = value?["can_retry"] as? Bool != true
        buttons["dismiss"]?.isHidden = value?["can_dismiss"] as? Bool != true
        buttons["delete"]?.isEnabled = value?["can_delete"] as? Bool == true
        buttons["pin"]?.isEnabled = value?["can_pin"] as? Bool == true
        buttons["pin"]?.title=value?["pin_label"] as? String ?? ""
        variantChanged()
        list.layoutSubtreeIfNeeded()
        list.setFrameSize(NSSize(width:listScroll.contentSize.width,height:list.fittingSize.height))
        listScroll.contentView.scroll(to: previous); listScroll.reflectScrolledClipView(listScroll.contentView)
        popover.contentSize = popover.contentViewController?.view.fittingSize ?? NSSize(width:360,height:260)
    }
    @objc private func variantChanged() {
        let value = dictate?.view["history_detail"] as? [String:Any]
        buttons["copy"]?.isEnabled = value?[variants.selectedSegment == 0 ? "can_copy_original" : "can_copy"] as? Bool == true
        let text = value?[variants.selectedSegment == 0 ? "raw" : "text"] as? String ?? ""
        if content.string != text { content.string = text }
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
    private var tabDown = false
    private var activationPoint:CGPoint?
    private var tabFocusChanged=false
    private var lastFocusChange = -Double.infinity
    private let focused: (CGPoint?) -> Bool

    private func bindRecentGesture() {
        guard !pointerDown,!tabDown,ProcessInfo.processInfo.systemUptime-lastInteraction<1 else{return}
        if focused(activationPoint){lastInteraction = -Double.infinity;activationPoint=nil}
    }
    init(focused: @escaping (CGPoint?) -> Bool) {
        self.focused = focused
        interaction = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .leftMouseUp, .keyDown, .keyUp]) { [weak self] event in
            guard let self else{return}
            if event.type == .leftMouseDown {
                self.pointerDown=true
                self.lastInteraction=ProcessInfo.processInfo.systemUptime
            } else if event.type == .leftMouseUp {
                self.pointerDown=false
                self.lastInteraction=ProcessInfo.processInfo.systemUptime
                self.activationPoint=event.cgEvent?.location
                // The target may publish its AX focus after mouse-up. Keep
                // this bounded explicit gesture until a real target is admitted.
                self.bindRecentGesture()
            } else if event.keyCode == 48 {
                let plain=event.modifierFlags.intersection([.command,.control,.option]).isEmpty
                if event.type == .keyDown,plain {
                    self.pointerDown=false
                    // AX and global NSEvent callbacks have independent delivery
                    // order. A focus notification may precede this callback
                    // while still following the actual Tab key timestamp.
                    if !self.tabDown {self.tabFocusChanged=self.lastFocusChange>=event.timestamp}
                    self.tabDown=true;self.activationPoint=nil;self.lastInteraction=ProcessInfo.processInfo.systemUptime
                } else if event.type == .keyUp,self.tabDown {
                    self.tabDown=false
                    if plain {
                        self.lastInteraction=ProcessInfo.processInfo.systemUptime
                        if self.tabFocusChanged {self.bindRecentGesture()}
                    } else {self.lastInteraction = -Double.infinity;self.activationPoint=nil}
                }
            }
        }
        activation = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { [weak self] _ in MainActor.assumeIsolated {
            guard let self else{return}
            self.observeFrontmost()
            self.bindRecentGesture()
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
                owner.lastFocusChange=ProcessInfo.processInfo.systemUptime
                if owner.tabDown {owner.tabFocusChanged=true}
                else {owner.bindRecentGesture()}
            }
        }, &next) == .success, let next else { return }
        let root = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(root, 0.25)
        let context = Unmanaged.passUnretained(self).toOpaque()
        AXObserverAddNotification(next, root, kAXFocusedUIElementChangedNotification as CFString, context)
        observer = next; application = root
        CFRunLoopAddSource(CFRunLoopGetMain(), AXObserverGetRunLoopSource(next), .commonModes)
    }
    static func selectedText()->String {ExternalTextInsertion.selectedText()}
    static func capture(activationPoint:CGPoint?=nil,preserveSelection:Bool=false,includeContext:Bool=true)->StationDictateTarget? {
        guard let target=ExternalTextInsertion.capture(activationPoint:activationPoint,preserveSelection:preserveSelection) else{return nil}
        return StationDictateTarget(kind:"editor",context:includeContext ? target.context : "",deliver:{text,completion in
            target.insert(text) {result in
                switch result {
                case .verified:completion(.applied)
                case .notPosted:completion(.unchanged)
                case .postedUnconfirmed:completion(.unknown)
                }
            }
        })
    }
}
#endif
