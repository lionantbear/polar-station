#if POLAR_STATION
@preconcurrency import AVFoundation
import Foundation
import AppKit
import UniformTypeIdentifiers
import AudioToolbox
import CoreAudio
import CPolarStationDesktopClient
import PolarTmuxHostClient
import PolarTmuxMacUI

typealias PolarStationDictateAudio = StationMicrophoneCapture

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
                MicrophonePermissionPresentation.showCaptureFailureIfNeeded()
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
