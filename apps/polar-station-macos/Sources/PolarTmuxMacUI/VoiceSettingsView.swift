import AppKit
import AVFoundation
import PolarTmuxHostClient
import SwiftUI

public typealias VoicePlatformRequest = @MainActor ([String: Any], @escaping @MainActor ([String: Any]) -> Void) -> Void

@MainActor
public struct VoiceSettingsView: View {
    @State private var client = VoiceClient()
    private let platform: VoicePlatformRequest?
    @State private var section = "Overview"
    @State private var status: [String: Any] = [:]
    @State private var configuration: [String: Any] = [:]
    @State private var offset = 0
    @State private var recordings: [[String: Any]] = []
    @State private var selected: [String: Any]?
    @State private var message = "Checking Voice…"
    @State private var busy = false
    @State private var modeSaving = false
    @State private var transcriptionMode = "realtime"
    @State private var transcriptionChoices: [[String: String]] = []
    @State private var cleanup = true
    @State private var outputMode="refine"
    @State private var outputChoices:[[String:String]]=[]
    @State private var retentionEnabled=true
    @State private var retentionDays=7
    @State private var cleanupPreview:[String:Any]=[:]
    @State private var cleanupConfirmation=false
    @State private var context = true
    @State private var learning:[String:Any]=[:]
    @State private var glossary = ""
    @State private var loadedGlossary = ""
    @State private var microphone = ""
    @State private var microphoneAuthorization = AVCaptureDevice.authorizationStatus(for: .audio)
    @State private var shortcutChoices: [[String:String]] = []
    @State private var shortcut = "Fn"
    @State private var testText = ""
    @State private var testOriginal = ""
    @State private var testCleaned = ""
    @State private var testDisplay = "Cleaned"
    @State private var level = 0.0
    @State private var audioBytes = 0
    @State private var historyCount=0
    @State private var importIssues:[[String:Any]]=[]
    @State private var importOffset=0
    @State private var nextCleanupMS:Double?
    @State private var testStatus = "Not tested"
    @State private var playbackStart = 0.0
    @State private var deleteID: String?
    private let pages = ["Overview", "Account", "Microphone", "Shortcut", "Text cleanup", "Test", "History and storage", "Diagnostics"]
    public init(platform: VoicePlatformRequest? = nil) { self.platform = platform }
    public var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Picker("Voice section", selection: $section) { ForEach(pages, id: \.self) { Text($0).tag($0) } }
            Text(message).font(.callout).textSelection(.enabled)
            if busy { ProgressView() }
            switch section {
            case "Account": account
            case "Microphone": microphoneControls
            case "Shortcut": shortcutControls
            case "Text cleanup": cleanupControls
            case "Test": testControls
            case "History and storage": recordingControls
            case "Diagnostics": diagnostics
            default: overview
            }
        }
        .task { await reload() }
        .onDisappear { platformAction(["op":"cancel_test"]) }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            microphoneAuthorization = AVCaptureDevice.authorizationStatus(for: .audio)
            Task { await reload() }
        }
        .alert("Delete saved recording?", isPresented: Binding(get: { deleteID != nil }, set: { if !$0 { deleteID = nil } })) {
            Button("Cancel", role: .cancel) { deleteID = nil }
            Button("Delete audio and text", role: .destructive) {
                if let id = deleteID { Task { _ = await request(["op":"delete", "id":id, "confirmation":id]); await reload() } }
                deleteID = nil
            }
        } message: { Text("This permanently removes this recording. Cancelling a recording never deletes its saved audio.") }
        .alert((cleanupPreview["presentation"] as? [String:Any])?["confirmation"] as? String ?? "",isPresented:$cleanupConfirmation) {
            let ui=cleanupPreview["presentation"] as? [String:Any] ?? [:]
            if ui["can_apply"] as? Bool == true {
                Button(ui["confirm_label"] as? String ?? "",role:.destructive){Task{if let result=await request(["op":"cleanup_apply","confirmation":true,"plan":cleanupPreview["plan"] ?? NSNull()]){message=(result["presentation"] as? [String:Any])?["result"] as? String ?? "";await reload()}}}
                Button(ui["cancel_label"] as? String ?? "",role:.cancel){}
            } else {Button("OK",role:.cancel){}}
        } message: {let ui=cleanupPreview["presentation"] as? [String:Any] ?? [:];Text((ui["summary"] as? String ?? "")+"\n\n"+(ui["detail"] as? String ?? ""))}
    }
    private var overview: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Dictate anywhere").font(.title2)
            Picker("Transcription mode", selection: Binding(get:{transcriptionMode},set:{mode in Task {await saveTranscriptionMode(mode)}})) {
                ForEach(transcriptionChoices.indices, id: \.self) { index in
                    Text(transcriptionChoices[index]["label"] ?? "").tag(transcriptionChoices[index]["mode"] ?? "")
                }
            }
            .disabled(modeSaving || configuration.isEmpty)
            Text("Changes apply to the next recording.").font(.caption)
            Text("Tap to keep recording, or hold and release to stop. Press again to stop. Use the Dictate button for recordings up to 10 minutes.")
            Text("Audio is saved every second on this computer. Failed or interrupted recordings stay available for recovery.")
            Text(status["signed_in"] as? Bool == true ? "Account connected · run the voice test to verify transcription" : "Sign in to enable transcription")
            HStack { Button("Set up account") { section = "Account" }; Button("Test Voice") { section = "Test" }; Button("Saved recordings") { section = "History and storage" } }
        }
    }
    private var account: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(status["account"] as? String ?? "No connected account").textSelection(.enabled)
            Text("Uses your existing Codex account and its configured secure credential store. No command-line installation is required.")
            HStack {
                Button("Sign in / Switch account") { Task {
                    if let payload = await request(["op":"voice_login"]), let value = payload["authorization_url"] as? String, let url = URL(string:value) { NSWorkspace.shared.open(url) }
                } }
                Button("Cancel sign-in") { Task { _ = await request(["op":"voice_cancel_login"]) } }
                Button("Recheck") { Task { await reload() } }
            }
        }
    }
    private var microphoneControls: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Microphone permission: " + permissionLabel)
            Text(microphonePermission.explanation).font(.callout).foregroundStyle(.secondary)
            if microphonePermission.action == .requestAccess {
                Button(microphonePermission.actionLabel) {
                    AVCaptureDevice.requestAccess(for: .audio) { _ in Task { @MainActor in
                        microphoneAuthorization = AVCaptureDevice.authorizationStatus(for: .audio)
                        message = microphonePermission.explanation
                    } }
                }
            } else if microphonePermission.action == .openSettings {
                Button(microphonePermission.actionLabel) { MicrophonePermissionPresentation.openSettings() }
            }
            Picker("Input device", selection: $microphone) {
                Text("System default").tag("")
                if !microphone.isEmpty && !AVCaptureDevice.devices(for: .audio).contains(where: { $0.uniqueID == microphone }) {
                    Text("Configured input (unavailable)").tag(microphone)
                }
                ForEach(AVCaptureDevice.devices(for: .audio), id: \.uniqueID) { device in Text(device.localizedName).tag(device.uniqueID) }
            }
            Button("Save microphone") { Task { await saveConfiguration() } }
            Button("Local microphone test · 10 seconds, no upload") { platformAction(["op":"microphone_test", "device":microphone]) }
            Button("Stop test") { platformAction(["op":"stop_test"]) }
            ProgressView(value:level).accessibilityLabel("Microphone level")
        }
    }
    private var shortcutControls: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(shortcutChoices.first?["label"] ?? "")
            Picker("Shortcut",selection:$shortcut) { ForEach(["Fn","RightOption","Control+Option+Space","Control+Shift+Space"],id:\.self) { Text($0).tag($0) } }
            Text("Default: Fn / Globe. A new binding is saved only after registration succeeds; conflicts leave the current binding active.")
            if shortcut == "Fn" {
                Text("To prevent the emoji picker or input-source switcher opening during Dictate, set Keyboard → Press Globe key to → Do Nothing. Returning here rechecks the setting.")
                Button("Open Keyboard Settings") {
                    NSWorkspace.shared.open(URL(string:"x-apple.systempreferences:com.apple.Keyboard-Settings.extension")!)
                }
            }
            HStack {
                Button("Apply shortcut") { platformAction(["op":"set_shortcut", "shortcut":shortcut]) }
                Button("Restore default") { shortcut="Fn";platformAction(["op":"set_shortcut","shortcut":"Fn"]) }
                Button("Cancel change") { shortcut=configuration["shortcut_mac"] as? String ?? "Fn" }
            }
            Button("Allow Accessibility") { platformAction(["op":"accessibility_permission"]) }
        }
    }
    private var cleanupControls: some View {
        VStack(alignment: .leading, spacing: 10) {
            Picker("Recording output",selection:$outputMode) {
                ForEach(outputChoices.indices,id:\.self){index in Text(outputChoices[index]["label"] ?? "").tag(outputChoices[index]["mode"] ?? "")}
            }.onChange(of:outputMode){mode in Task{await saveOutputMode(mode)}}
            Toggle(transcriptionChoices.first(where:{$0["mode"] == transcriptionMode})?["context_label"] ?? "", isOn: $context)
            Text(transcriptionChoices.first(where:{$0["mode"] == transcriptionMode})?["reference_hint"] ?? "").font(.caption).foregroundStyle(.secondary)
            Text("Adam · Luna · low reasoning. The selected output mode controls language. It never answers questions or sends messages.")
            Text("After 8 seconds, the original stays available. You can choose Use Original while cleanup runs.")
            Text("Terminology · one term per line; use | between the standard spelling and heard variants.")
            TextEditor(text: $glossary).frame(minHeight: 100, maxHeight: 180).accessibilityLabel("Terminology")
            Text("Example: Polar Station | polarstation | 波拉站. Edit or remove a line, then save. Manual terminology is saved independently; learned rules stay on this Host.").font(.caption)
            Button("Save cleanup settings") { Task { await saveConfiguration() } }
            DisclosureGroup("Automatic learning") {
                let preferences=learning["preferences"] as? [String:Any] ?? [:]
                Toggle("Learn corrections after successful Send",isOn:Binding(get:{preferences["disabled"] as? Bool != true},set:{ enabled in Task{await updateLearning(enabled:enabled)}})).disabled(learning.isEmpty)
                Text("Only corrections to PS drafts are learned. Project rules become global after matching evidence in 2 projects.").font(.caption)
                let rules=preferences["rules"] as? [[String:Any]] ?? []
                ScrollView {
                    VStack(alignment:.leading,spacing:8){
                        ForEach(Array(rules.enumerated()),id:\.offset){index,rule in
                            let replacement=rule["replacement"] as? [String:String] ?? [:]
                            HStack{
                                VStack(alignment:.leading){
                                    Text((replacement["from"] ?? "")+" → "+(replacement["to"] ?? ""))
                                    Text((rule["reason"] as? String ?? "")+" · "+((rule["projects"] as? [String] ?? []).joined(separator:", "))).font(.caption).foregroundStyle(.secondary)
                                }
                                Spacer()
                                if rule["retired"] as? Bool == true {Text("Retired").font(.caption)}
                                else {Button("Disable"){Task{await updateLearning(enabled:preferences["disabled"] as? Bool != true,retire:index)}}}
                            }
                        }
                        if rules.isEmpty {Text("No learned rules yet").foregroundStyle(.secondary)}
                    }
                }.frame(maxHeight:160)
                HStack{Button("Refresh learned rules"){Task{if let value=await request(["op":"refine_learning"]){learning=value}}};Button("Clear learned rules"){Task{await updateLearning(enabled:preferences["disabled"] as? Bool != true,clear:true)}}}
            }

        }
    }
    private var testControls: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(testStatus)
            ProgressView(value:level).accessibilityLabel("Microphone level")
            Picker("Test result",selection:$testDisplay) { Text("Cleaned").tag("Cleaned");Text("Original").tag("Original");Text("Compare").tag("Compare") }
                .onChange(of:testDisplay) { value in testText = value == "Original" ? testOriginal : (testCleaned.isEmpty ? testOriginal : testCleaned) }
            if testDisplay == "Compare" { HStack(alignment:.top) { VStack(alignment:.leading) {Text("Original").font(.caption);Text(testOriginal).textSelection(.enabled)};Divider();VStack(alignment:.leading) {Text("Cleaned").font(.caption);Text(testCleaned).textSelection(.enabled)} } }
            Button("Copy test text") { copy(testText) }
            HStack { Button("Record transcription test") { platformAction(["op":"transcription_test", "device":microphone]) }; Button("Stop and transcribe") { platformAction(["op":"stop_test"]) } }
            TextEditor(text: $testText).frame(minHeight: 140).accessibilityLabel("Voice test text")
            Text("Test text stays here. Test audio is marked Test and remains recoverable in Recordings.")
        }
    }
    private var recordingControls: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("History and storage").font(.headline)
            Toggle("Automatically clean up completed history and audio",isOn:$retentionEnabled)
            HStack {TextField("Keep completed records for days",value:$retentionDays,formatter:NumberFormatter()).frame(maxWidth:180);Button("Save retention rule"){Task{await saveRetention()}}}
            Text("Pending, unresolved, uncertain and pinned records are kept. The retention period starts when a result is inserted, copied or explicitly accepted.").font(.caption)
            Text("\(historyCount) history records").font(.caption)
            if retentionEnabled {
                if let time=nextCleanupMS {Text("Next cleanup: "+Date(timeIntervalSince1970:time/1000).formatted(date:.abbreviated,time:.shortened)).font(.caption)}else{Text("No completed records scheduled for cleanup").font(.caption)}
            }
            Button("Clean up by rules…"){Task{await previewCleanup()}}
            if !importIssues.isEmpty {
                DisclosureGroup("Previous drafts needing review") {
                    ForEach(importIssues.indices,id:\.self){index in
                        let item=importIssues[index]
                        if let ms=item["created_at_ms"] as? NSNumber {Text(Date(timeIntervalSince1970:ms.doubleValue/1000).formatted(date:.abbreviated,time:.shortened)).font(.caption)}else{Text("Earlier draft").font(.caption)}
                        Text(item["text"] as? String ?? "").textSelection(.enabled)
                        Button("Copy preserved text"){copy(item["text"] as? String ?? "")}
                    }
                    HStack{Button("Previous preserved drafts"){importOffset=max(0,importOffset-25);Task{await reload()}}.disabled(importOffset==0);Button("More preserved drafts"){importOffset+=25;Task{await reload()}}.disabled(importIssues.count<25)}
                }
            }
            HStack { Spacer(); Button("Refresh") { Task { await reload() } } }
            Text("Saved audio on this computer: \(ByteCountFormatter.string(fromByteCount:Int64(audioBytes),countStyle:.file))").font(.caption)
            Text("Failed, interrupted, pinned and unconfirmed deliveries are kept. Expiry removes audio only.").font(.caption)
            HStack {
                Button("Previous") { offset = max(0, offset - 25); Task { await reload() } }.disabled(offset == 0)
                Text("Page \(offset / 25 + 1)")
                Button("Next") { offset += 25; Task { await reload() } }.disabled(recordings.count < 25)
            }
            ForEach(Array(recordings.enumerated()), id: \.offset) { _, item in
                VStack(alignment:.leading) {
                    Button(recordingLabel(item)) { Task {
                        if let id = item["id"] as? String, let payload = await request(["op":"get","id":id]) { selected = payload["recording"] as? [String:Any]; playbackStart=0 }
                    } }.buttonStyle(.link)
                    if let id = item["id"] as? String { HStack {
                        Button("Retry") { Task { _ = await request(["op":"retry","id":id]); await reload() } }
                        if let label = item["retry_segmented_label"] as? String {
                            Button(label) { Task { _ = await request(["op":"retry_segmented","id":id]); await reload() } }
                        }
                        Button(item["pinned"] as? Bool == true ? "Unpin" : "Pin") { Task { _ = await request(["op":"pin","id":id,"pinned":!(item["pinned"] as? Bool ?? false)]); await reload() } }
                        Button("Delete", role:.destructive) { deleteID = id }.accessibilityIdentifier("delete-recording-" + id)
                    } }
                }
            }
            if let selected, let id = selected["id"] as? String {
                Divider()
                Text(id).font(.caption).textSelection(.enabled)
                if let completed=selected["completed_at_ms"] as? Double, selected["pinned"] as? Bool != true {
                    Text("Audio expires " + Date(timeIntervalSince1970:completed/1000+7*86400).formatted()).font(.caption)
                }
                Text(selected["error"] as? String ?? selected["cleanup_error"] as? String ?? "Saved recording").foregroundStyle(.secondary)
                let segments = selected["segments"] as? [[String:Any]] ?? []
                ForEach(Array(segments.enumerated()), id: \.offset) { _, segment in
                    HStack { Text("\((segment["start_sample"] as? Int ?? 0)/16000)s · \(segment["state"] as? String ?? "pending")");
                        if segment["state"] as? String == "failed" { Button("Play this range") { platformAction(["op":"play_recording","id":id,"start_sample":segment["start_sample"] ?? 0,"sample_count":segment["sample_count"] ?? 0]) } }
                    }
                }
                Text(selected["cleaned"] as? String ?? segments.compactMap { $0["original"] as? String }.joined(separator:"\n")).textSelection(.enabled)
                HStack {
                    Button("Copy original") { copy(segments.compactMap {$0["original"] as? String}.joined(separator:"\n")) }
                    Button("Copy cleaned") { copy(selected["cleaned"] as? String ?? "") }.disabled((selected["cleaned"] as? String ?? "").isEmpty)
                }
                let duration = Double(selected["saved_samples"] as? Int ?? 0)/16000
                if duration > 0 {
                    Text("Playback position: \(Int(playbackStart)) / \(Int(duration)) seconds").font(.caption)
                    Slider(value:$playbackStart,in:0...duration,onEditingChanged:{ editing in
                        if !editing {platformAction(["op":"seek_playback","seconds":playbackStart])}
                    }).accessibilityLabel("Saved audio playback position")
                    HStack {Button("Pause") {platformAction(["op":"pause_playback"])};Button("Resume") {platformAction(["op":"resume_playback"])};Button("Stop playback") {platformAction(["op":"stop_playback"])}}
                }
                HStack {
                    Button("Play") { platformAction(["op":"play_recording","id":id,"seconds":playbackStart]) }
                    Button("Export WAV") { platformAction(["op":"export_recording","id":id]) }
                    Button("Use Original") { Task { _ = await request(["op":"use_original","id":id]); await reload() } }
                    Button("Accept text") { Task { _ = await request(["op":"accept","id":id,"now_ms":Int(Date().timeIntervalSince1970*1000)]); await reload() } }
                }
                HStack {
                    Button("Use in next input") { platformAction(["op":"recover_recording","id":id]) }
                    Text("Waits for your next explicit input selection.").font(.caption)
                }
            }
        }
    }
    private var diagnostics: some View {
        VStack(alignment:.leading,spacing:10) {
            Text("Audio encoding: built-in Opus / WebM\nMaximum recording: 10 minutes\nLocal save interval: 1 second\nOrdinary Dictate: whole recordings up to 40 seconds\nCleanup deadline: 8 seconds").textSelection(.enabled)
            Button("Recheck Voice") { Task { await reload() } }
            Button("Copy diagnostics") { copy("Polar Station Voice\nEncoder: built-in Opus/WebM\nAccount connected: \(status["signed_in"] as? Bool ?? false)\nMicrophone: \(permissionLabel)\nLimit: 10 minutes\nSave interval: 1 second\nCleanup: Adam Luna low, 8 seconds") }
            Text("Diagnostics exclude audio, transcript content and credentials.").font(.caption)
        }
    }
    private var permissionLabel: String {
        microphonePermission.label
    }
    private var microphonePermission: MicrophonePermissionPresentation { .init(status: microphoneAuthorization) }
    private func recordingLabel(_ item:[String:Any])->String {
        "\(Date(timeIntervalSince1970:(item["created_at_ms"] as? Double ?? 0)/1000).formatted(date:.abbreviated,time:.shortened)) · \(item["is_test"] as? Bool == true ? "Test · " : "")\((item["saved_samples"] as? Int ?? 0)/16000)s · \(item["segments_complete"] as? Int ?? 0)/\(item["segments_total"] as? Int ?? 0) segments · \(item["capture"] as? String ?? "saved")"
    }
    private func request(_ value:[String:Any]) async -> [String:Any]? {
        guard let input = try? JSONSerialization.data(withJSONObject:value) else { return nil }
        busy = true; defer { busy = false }
        let data = await client.request(input)
        guard let result = try? JSONSerialization.jsonObject(with:data) as? [String:Any] else { message = "Voice returned an invalid response"; return nil }
        guard result["ok"] as? Bool == true else { message = result["error"] as? String ?? "Voice request failed"; return nil }
        if value["op"] as? String == "delete", let id=value["id"] as? String {
            selected=nil
            NotificationCenter.default.post(name:Notification.Name("PolarStationRecordingDeleted"),object:id)
        }
        return result["payload"] as? [String:Any]
    }
    private func reload() async {
        guard !modeSaving else {return}
        if let result = await request(["op":"shortcut_modes"]) { shortcutChoices = result["choices"] as? [[String:String]] ?? [] }
        if let result = await request(["op":"transcription_modes"]) { transcriptionChoices = result["choices"] as? [[String:String]] ?? [] }
        if let result = await request(["op":"voice_status"]) { status = result; message = "Voice account checked. Microphone and transcription tests are separate." }
        if let result=await request(["op":"refine_learning"]){learning=result}
        if let result = await request(["op":"voice_settings"]), !modeSaving,
           (result["revision"] as? UInt64 ?? 0) >= (configuration["revision"] as? UInt64 ?? 0) {
            if glossary == loadedGlossary {
                glossary = result["glossary"] as? String ?? ""
                loadedGlossary = glossary
            }
            transcriptionMode = result["transcription_mode"] as? String ?? "realtime"
            configuration = result;outputMode=result["output_mode"] as? String ?? "refine"
            let retention=result["retention"] as? [String:Any] ?? [:];retentionEnabled=retention["enabled"] as? Bool ?? true;retentionDays=retention["days"] as? Int ?? 7
            cleanup = result["cleanup"] as? Bool ?? true; context = result["context"] as? Bool ?? true
            microphone = result["microphone"] as? String ?? ""; shortcut = result["shortcut_mac"] as? String ?? "Fn"
        }
        if let result=await request(["op":"import_issues","offset":importOffset,"limit":25]){importIssues=result["items"] as? [[String:Any]] ?? []}
        if let result=await request(["op":"voice_output_modes"]){outputChoices=result["choices"] as? [[String:String]] ?? []}
        if let result = await request(["op":"list","offset":offset,"limit":25]) { recordings = result["recordings"] as? [[String:Any]] ?? [] }
        if let result = await request(["op":"usage"]), let usage=result["usage"] as? [String:Any] {audioBytes=usage["audio_bytes"] as? Int ?? 0;historyCount=usage["history_items"] as? Int ?? 0}
        if let result=await request(["op":"cleanup_preview"]){nextCleanupMS=(result["report"] as? [String:Any])?["next_at_ms"] as? Double}
        if let id=selected?["id"] as? String,let result=await request(["op":"get","id":id]) {selected=result["recording"] as? [String:Any]}
    }
    private func updateLearning(enabled:Bool,retire:Int?=nil,clear:Bool=false) async {
        guard let preferences=learning["preferences"] as? [String:Any],let revision=preferences["revision"] else{return}
        if let result=await request(["op":"refine_learning_update","revision":revision,"enabled":enabled,"retire":retire as Any? ?? NSNull(),"clear":clear]){learning=result}
    }
    private func saveTranscriptionMode(_ mode:String) async {
        guard !modeSaving,mode != transcriptionMode,let revision=configuration["revision"] else {return}
        modeSaving=true;message="Saving transcription mode…";defer {modeSaving=false}
        if let saved=await request(["op":"voice_transcription_mode_update","revision":revision,"mode":mode]) {
            configuration=saved;transcriptionMode=saved["transcription_mode"] as? String ?? transcriptionMode
            message="Transcription mode saved · applies to the next recording"
            NotificationCenter.default.post(name:Notification.Name("PolarStationVoiceSettingsChanged"),object:nil)
        }
    }
    private func saveOutputMode(_ mode:String) async {
        guard configuration["output_mode"] as? String != mode else{return}
        if let saved=await request(["op":"voice_output_mode_update","mode":mode]){configuration=saved;cleanup=saved["cleanup"] as? Bool ?? true;NotificationCenter.default.post(name:Notification.Name("PolarStationVoiceSettingsChanged"),object:nil)}
    }
    private func saveRetention() async {
        if let saved=await request(["op":"voice_retention_update","retention":["enabled":retentionEnabled,"days":retentionDays]]){configuration=saved;message="Retention rule saved"}
    }
    private func previewCleanup() async {if let result=await request(["op":"cleanup_preview"]){cleanupPreview=result;cleanupConfirmation=true}}
    private func saveConfiguration() async {
        guard !modeSaving else {return}
        configuration["glossary"] = glossary
        configuration["shortcut_mode"] = "auto"
        configuration["transcription_mode"] = transcriptionMode
        configuration["output_mode"]=outputMode
        configuration["cleanup"] = cleanup; configuration["context"] = context; configuration["microphone"] = microphone
        if let saved = await request(["op":"voice_settings_update","settings":configuration]) { configuration = saved; loadedGlossary = saved["glossary"] as? String ?? ""; message = "Voice settings saved"; NotificationCenter.default.post(name:Notification.Name("PolarStationVoiceSettingsChanged"),object:nil) }
    }
    private func copy(_ text:String) { NSPasteboard.general.clearContents();NSPasteboard.general.setString(text,forType:.string) }
    private func platformAction(_ input:[String:Any]) {
        guard let platform else { message = "This action requires the native Polar Station window"; return }
        platform(input) { result in
            if let meter=result["level"] as? Double {level=meter;return}
            if let value=result["original"] as? String {testOriginal=value}
            if let value=result["cleaned"] as? String {testCleaned=value}
            message = result["message"] as? String ?? result["error"] as? String ?? "Done"
            if let text = result["text"] as? String { testText = text }
            if let state = result["test_status"] as? String { testStatus = state }
        }
    }
}
