#if POLAR_STATION
@preconcurrency import AVFoundation
import Foundation
import AppKit
import UniformTypeIdentifiers
import AudioToolbox
import CoreAudio
import CPolarStationDesktopClient
import PolarTmuxHostClient

/// Native microphone mechanics only. The shared Rust engine owns target,
/// lifecycle and duration policy; callbacks carry the captured operation ID.
@MainActor
final class PolarStationDictateAudio {
    private var operation: String?
    private var engine: AVAudioEngine?
    private var buffer: StationDictatePCMBuffer?
    private var configurationObserver: NSObjectProtocol?

    func start(operation id: String, maximumMS: Int, deviceUID: String? = nil,
               chunk: @escaping @Sendable (Data) -> Bool,
               ready: @escaping @MainActor (String) -> Void,
               limit: @escaping @MainActor (String) -> Void,
               failed: @escaping @MainActor (String, String) -> Void) {
        cancel()
        operation = id
        let permission: @Sendable (Bool) -> Void = { [weak self] allowed in
            DispatchQueue.main.async {
                guard let self, self.operation == id else { return }
                guard allowed else {
                    failed(id, "Allow Polar Station to use the microphone in System Settings.")
                    return
                }
                do {
                    let engine = AVAudioEngine()
                    let input = engine.inputNode
                    if let deviceUID, !deviceUID.isEmpty {
                        guard let unit = input.audioUnit, var device = Self.deviceID(uid: deviceUID),
                              AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0,
                                  &device, UInt32(MemoryLayout<AudioDeviceID>.size)) == noErr else {
                            failed(id, "The selected microphone is unavailable. Choose a device in Settings → Voice."); return
                        }
                    }
                    let format = input.outputFormat(forBus: 0)
                    guard format.sampleRate > 0, format.channelCount > 0,
                          let target = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000,
                              channels: 1, interleaved: false),
                          let converter = AVAudioConverter(from: format, to: target) else {
                        failed(id, "No usable microphone format is available."); return
                    }
                    converter.downmix = true
                    let buffer = StationDictatePCMBuffer(converter: converter, format: target,
                        capacity: min(600_000, max(1, maximumMS)) * 32, sink: chunk)
                    input.installTap(onBus: 0, bufferSize: 1024, format: format) { chunk, _ in
                        let event = buffer.append(chunk)
                        if event != .none {
                            DispatchQueue.main.async { [weak self] in
                                guard self?.operation == id else { return }
                                if event == .limit { limit(id) }
                                else { failed(id, "Audio capture or storage stopped. Saved audio is available in Settings → Voice → Recordings.") }
                            }
                        }
                    }
                    self.engine = engine; self.buffer = buffer
                    try engine.start()
                    self.configurationObserver = NotificationCenter.default.addObserver(
                        forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main
                    ) { [weak self] _ in
                        MainActor.assumeIsolated {
                            guard self?.operation == id else { return }
                            failed(id, "The microphone changed or disconnected. Start a new recording.")
                        }
                    }
                    ready(id)
                } catch {
                    self.cancel()
                    failed(id, "Microphone capture could not start: " + error.localizedDescription)
                }
            }
        }
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: permission(true)
        case .notDetermined: AVCaptureDevice.requestAccess(for: .audio, completionHandler: permission)
        default: permission(false)
        }
    }
    private static func deviceID(uid: String) -> AudioDeviceID? {
        var address = AudioObjectPropertyAddress(mSelector:kAudioHardwarePropertyDevices, mScope:kAudioObjectPropertyScopeGlobal, mElement:kAudioObjectPropertyElementMain)
        var size: UInt32 = 0
        guard AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size) == noErr else { return nil }
        var devices = [AudioDeviceID](repeating:0, count:Int(size)/MemoryLayout<AudioDeviceID>.size)
        guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &devices) == noErr else { return nil }
        for device in devices {
            var property = AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyDeviceUID, mScope:kAudioObjectPropertyScopeGlobal, mElement:kAudioObjectPropertyElementMain)
            var name: CFString = "" as CFString
            var bytes = UInt32(MemoryLayout<CFString>.size)
            if AudioObjectGetPropertyData(device, &property, 0, nil, &bytes, &name) == noErr, name as String == uid { return device }
        }
        return nil
    }
    func finish(operation id: String) -> Data {
        guard operation == id else { return Data() }
        stopEngine()
        let audio = buffer?.take() ?? Data()
        buffer = nil; operation = nil
        return audio
    }
    func cancel() {
        operation = nil
        stopEngine()
        buffer?.discard(); buffer = nil
    }
    private func stopEngine() {
        if let observer = configurationObserver { NotificationCenter.default.removeObserver(observer) }
        configurationObserver = nil
        if let engine { engine.inputNode.removeTap(onBus: 0); engine.stop() }
        engine = nil
    }

}

/// The audio callback and UI finalization share only this bounded buffer.
private final class StationDictatePCMBuffer: @unchecked Sendable {
    enum Event: Sendable, Equatable { case none, limit, failed }
    private let lock = NSLock()
    private let converter: AVAudioConverter
    private let format: AVAudioFormat
    private let capacity: Int
    private var data = Data()
    private var total = 0
    private let sink: @Sendable (Data) -> Bool
    private var stopped = false
    init(converter: AVAudioConverter, format: AVAudioFormat, capacity: Int, sink: @escaping @Sendable (Data) -> Bool) {
        self.converter = converter; self.format = format; self.capacity = capacity
        self.sink = sink
    }
    func append(_ input: AVAudioPCMBuffer) -> Event {
        lock.lock(); defer { lock.unlock() }
        guard !stopped else { return .none }
        let frames = AVAudioFrameCount(ceil(Double(input.frameLength) * 16_000 / input.format.sampleRate) + 32)
        guard let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames) else {
            stopped = true; return .failed
        }
        var consumed = false
        var failure: NSError?
        let status = converter.convert(to: output, error: &failure) { _, state in
            if consumed { state.pointee = .noDataNow; return nil }
            consumed = true; state.pointee = .haveData; return input
        }
        guard status != .error, failure == nil, let samples = output.int16ChannelData?[0] else {
            stopped = true; return .failed
        }
        let count = min(Int(output.frameLength) * 2, capacity - total)
        if count > 0 {
            let packet = Data(bytes: samples, count: count)
            guard sink(packet) else { stopped = true; return .failed }
            total += count
        }
        if total == capacity { stopped = true; return .limit }
        return .none
    }
    func take() -> Data {
        lock.lock(); defer { lock.unlock() }
        stopped = true
        let result = data
        data = Data()
        return result
    }
    func discard() {
        lock.lock(); defer { lock.unlock() }
        stopped = true
        data.resetBytes(in: 0..<data.count); data.removeAll()
    }
    deinit { discard() }
}
/// Native test/playback controls reuse the same microphone adapter. Test results
/// never enter the normal target router or any user's draft.
@MainActor
final class PolarStationVoiceTools {
    private let audio = PolarStationDictateAudio()
    private let client = VoiceClient()
    private let worker = TestWorker()
    private var operation: String?
    private var processing: String?
    private var upload = false
    private var callback: (@MainActor ([String:Any])->Void)?
    private var player: AVAudioPlayer?
    private var playbackGeneration: UInt64 = 0
    private var exportPanel: NSSavePanel?
    private let local = LocalAudio()
    var setShortcut: ((String, @escaping @MainActor ([String:Any])->Void)->Void)?
    var recoverRecording: ((String)->Void)?
    private final class LocalAudio: @unchecked Sendable {
        let lock = NSLock(); var data = Data(); var lastMeter = 0.0
        func meter(_ bytes:Data)->Double? {
            lock.lock();defer {lock.unlock()}
            let now=ProcessInfo.processInfo.systemUptime
            guard now-lastMeter>=0.1 else{return nil};lastMeter=now
            var peak=0
            bytes.withUnsafeBytes { buffer in
                for offset in stride(from:0,to:buffer.count-1,by:2) { peak=max(peak,abs(Int(Int16(littleEndian:buffer.loadUnaligned(fromByteOffset:offset,as:Int16.self))))) }
            }
            return Double(peak)/32768.0
        }
        func append(_ bytes: Data)->Bool { lock.lock(); defer {lock.unlock()}; guard data.count+bytes.count<=320000 else{return false};data.append(bytes);return true }
        func take()->Data {lock.lock();defer{lock.unlock()};let value=data;data=Data();return value}
    }
    private final class TestWorker: @unchecked Sendable {
        let handle = polar_station_dictate_speech_new()
        deinit {polar_station_dictate_speech_free(handle)}
        func call(_ data:Data)->Data? {data.withUnsafeBytes { bytes in
            guard let result=polar_station_dictate_service_request(handle,bytes.bindMemory(to:UInt8.self).baseAddress,data.count)else{return nil}
            defer{polar_station_desktop_string_free(result)};return Data(bytes:result,count:strlen(result))
        }}
    }
    func request(_ input:[String:Any], reply:@escaping @MainActor ([String:Any])->Void) {
        switch input["op"] as? String {
        case "recover_recording":
            guard let id=input["id"] as? String, let recoverRecording else {reply(["error":"Saved recording input is unavailable"]);return}
            cancelTests();recoverRecording(id)
        case "accessibility_permission":
            GlobalEventTap.shared.enableStationDictate();reply(["message":"Accessibility permission requested. Return here after allowing Polar Station."])
        case "set_shortcut":
            let value=input["shortcut"] as? String ?? ""
            if let setShortcut {setShortcut(value,reply)}else{reply(["error":"Shortcut registration is unavailable"])}
        case "microphone_test", "transcription_test":
            guard operation == nil && processing == nil else {reply(["error":"Stop the current test first"]);return}
            let id=UUID().uuidString;operation=id;upload=input["op"] as? String == "transcription_test";callback=reply;_ = local.take()
            if upload {
                guard let data=try? JSONSerialization.data(withJSONObject:["op":"capture_begin","id":id,"is_test":true]) else { operation=nil; reply(["error":"Voice test request could not be created"]); return }
                guard let output=worker.call(data),let value=try? JSONSerialization.jsonObject(with:output) as? [String:Any],value["ok"] as? Bool == true else{
                    operation=nil;reply(["error":"Voice test could not start"]);return
                }
            }
            let send=upload;let key=Array(id.utf8)
            audio.start(operation:id,maximumMS:10000,deviceUID:input["device"] as? String,chunk:{[worker,local,weak self] data in
                if let level=local.meter(data) { Task { @MainActor [weak self] in self?.callback?(["level":level]) } }
                if !send {return local.append(data)}
                return key.withUnsafeBufferPointer {id in data.withUnsafeBytes {bytes in
                    polar_station_dictate_speech_append(worker.handle,id.baseAddress,id.count,bytes.bindMemory(to:UInt8.self).baseAddress,data.count)
                }}
            },ready:{_ in reply(["message":"Recording test… Stop when ready (maximum 10 seconds).","test_status":"Recording"])
            },limit:{[weak self] _ in self?.finishTest()},failed:{[weak self] id,error in
                self?.audio.cancel();key.withUnsafeBufferPointer {polar_station_dictate_speech_cancel(self?.worker.handle,$0.baseAddress,$0.count)}
                self?.operation=nil;reply(["error":error,"test_status":"Microphone/storage failed; saved Test audio retained"])
            })
        case "seek_playback":
            if let time=input["seconds"] as? Double,let player {player.currentTime=max(0,min(time,player.duration))}
        case "pause_playback": player?.pause()
        case "resume_playback": player?.play()
        case "stop_playback": playbackGeneration &+= 1;player?.stop()
        case "stop_test": finishTest()
        case "cancel_test": cancelTests(); reply(["message":"Test stopped; saved audio retained"])
        case "play_recording", "export_recording":
            guard let id=input["id"] as? String else{return}
            playbackGeneration &+= 1
            let generation=playbackGeneration
            exportPanel?.cancel(nil);exportPanel=nil;player?.stop()
            Task {do {
                let pcm=try await readAudio(id:id,start:input["start_sample"] as? Int ?? 0,count:input["sample_count"] as? Int)
                guard generation == playbackGeneration else{return}
                let wave=Self.wav(pcm)
                if input["op"] as? String == "export_recording" {
                    let panel=NSSavePanel();panel.nameFieldStringValue="Dictate-"+id+".wav";panel.allowedContentTypes=[.wav]
                    exportPanel=panel
                    let response=await panel.begin()
                    guard generation == playbackGeneration else{return}
                    exportPanel=nil
                    if response == .OK,let url=panel.url {try wave.write(to:url,options:.atomic);reply(["message":"WAV exported"])}
                }else{player=try AVAudioPlayer(data:wave);player?.currentTime=max(0,min(input["seconds"] as? Double ?? 0,player?.duration ?? 0));player?.play();reply(["message":"Playing saved audio"])}
            }catch{if generation == playbackGeneration {reply(["error":error.localizedDescription])}}}
        default:reply(["error":"Unknown native Voice action"])
        }
    }
    func escape() -> Bool {
        guard operation != nil || processing != nil else{return false};cancelTests();return true
    }
    func cancelTests() {
        playbackGeneration &+= 1;exportPanel?.cancel(nil);exportPanel=nil
        audio.cancel()
        for id in [operation,processing].compactMap({$0}) {
            let key=Array(id.utf8)
            key.withUnsafeBufferPointer { polar_station_dictate_speech_cancel(worker.handle,$0.baseAddress,$0.count) }
        }
        operation=nil;processing=nil;callback=nil;_ = local.take();player?.stop()
    }
    private func finishTest() {
        guard let id=operation else{return};operation=nil;_ = audio.finish(operation:id)
        guard upload else {
            do {player=try AVAudioPlayer(data:Self.wav(local.take()));player?.play();callback?(["message":"Local microphone test complete; playing back. No audio was uploaded.","test_status":"Microphone captured"])}catch{callback?(["error":error.localizedDescription])};return
        }
        let key=Array(id.utf8)
        guard key.withUnsafeBufferPointer({polar_station_dictate_speech_reserve(worker.handle,$0.baseAddress,$0.count)})else{return}
        callback?(["message":"Test audio saved. Transcribing…","test_status":"Transcription pending"])
        processing=id
        Task { [worker] in
            defer { if processing == id { processing=nil } }
            let output=await Task.detached {key.withUnsafeBufferPointer { key->Data? in
                guard let result=polar_station_dictate_speech_transcribe(worker.handle,key.baseAddress,key.count,nil,0)else{return nil}
                defer{polar_station_desktop_string_free(result)};return Data(bytes:result,count:strlen(result))
            }}.value
            guard processing == id else { return }
            guard let output,let response=try? JSONSerialization.jsonObject(with:output) as? [String:Any],let result=response["input"] as? [String:Any]else{callback?(["error":"Test returned no result"]);return}
            if let error=result["error"] as? String {callback?(["error":error,"test_status":"Transcription failed · saved audio recoverable"]);return}
            do {let payload=try await call(["op":"get","id":id]);let record=payload["recording"] as? [String:Any] ?? [:]
                callback?(["message":"Test complete. Review the text here; nothing was pasted into another input.","text":record["cleaned"] as? String ?? result["text"] as? String ?? "",
                    "original":result["text"] as? String ?? "", "cleaned":record["cleaned"] as? String ?? "",
                    "test_status":record["cleanup_enabled"] as? Bool == false ? "Transcription passed · cleanup disabled" : (record["cleanup_error"] == nil || record["cleanup_error"] is NSNull ? "Transcription passed · cleanup passed" : "Transcription passed · cleanup failed, original retained")])
            }catch{callback?(["error":error.localizedDescription])}
        }
    }
    private func call(_ request:[String:Any]) async throws->[String:Any] {
        let data=await client.request(try JSONSerialization.data(withJSONObject:request))
        let value=try JSONSerialization.jsonObject(with:data) as? [String:Any] ?? [:]
        guard value["ok"] as? Bool == true else{throw NSError(domain:"Voice",code:1,userInfo:[NSLocalizedDescriptionKey:value["error"] as? String ?? "Voice request failed"])}
        return value["payload"] as? [String:Any] ?? [:]
    }
    private func readAudio(id:String,start:Int,count:Int?) async throws->Data {
        let result=try await call(["op":"get","id":id]);let record=result["recording"] as? [String:Any] ?? [:]
        let saved=record["saved_samples"] as? Int ?? 0
        guard start>=0 && start<=saved && record["capture"] as? String != "recording" else { throw NSError(domain:"Voice",code:2,userInfo:[NSLocalizedDescriptionKey:"Stop recording before playback or export"]) }
        let end=min(saved,start+(count ?? (saved-start)));var pcm=Data();var cursor=start
        while cursor<end {
            let count=min(16000,end-cursor);let part=try await call(["op":"audio","id":id,"start_sample":cursor,"sample_count":count])
            guard let bytes=part["pcm"] as? [UInt8],bytes.count==count*2 else{throw NSError(domain:"Voice",code:2,userInfo:[NSLocalizedDescriptionKey:"Saved audio range is missing"])}
            pcm.append(contentsOf:bytes);cursor+=count
        };return pcm
    }
    private static func wav(_ pcm:Data)->Data {
        var data=Data("RIFF".utf8)
        func u32(_ n:UInt32){var n=n.littleEndian;withUnsafeBytes(of:&n){data.append(contentsOf:$0)}}
        func u16(_ n:UInt16){var n=n.littleEndian;withUnsafeBytes(of:&n){data.append(contentsOf:$0)}}
        u32(UInt32(pcm.count)+36);data.append(contentsOf:"WAVEfmt ".utf8);u32(16);u16(1);u16(1);u32(16000);u32(32000);u16(2);u16(16)
        data.append(contentsOf:"data".utf8);u32(UInt32(pcm.count));data.append(pcm);return data
    }
}
#endif
