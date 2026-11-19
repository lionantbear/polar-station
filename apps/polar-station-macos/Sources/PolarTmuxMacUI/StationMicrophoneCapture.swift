@preconcurrency import AVFoundation
import Foundation
import AudioToolbox
import CoreAudio

/// Native microphone mechanics only. The shared Rust engine owns target,
/// lifecycle and duration policy; callbacks carry the captured operation ID.
@MainActor
public final class StationMicrophoneCapture {
    public init() {}
    private var operation: String?
    private var engine: AVAudioEngine?
    private var buffer: StationDictatePCMBuffer?
    private var configurationObserver: NSObjectProtocol?

    public func start(operation id: String, maximumMS: Int, deviceUID: String? = nil,
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
                    failed(id, MicrophonePermissionPresentation(status: AVCaptureDevice.authorizationStatus(for: .audio)).explanation)
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
                    self.engine = engine; self.buffer = buffer
                    self.installTap(engine: engine, buffer: buffer, id: id, limit: limit, failed: failed)
                    self.configurationObserver = NotificationCenter.default.addObserver(
                        forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main
                    ) { [weak self, weak engine] _ in
                        MainActor.assumeIsolated {
                            guard let self, let engine, self.operation == id, self.engine === engine else { return }
                            // A Bluetooth route transition changes the hardware format and
                            // stops AVAudioEngine. Retain the recording/sink and restart only
                            // the native tap; this is not a new Dictate operation.
                            engine.stop()
                            engine.inputNode.removeTap(onBus: 0)
                            let format = engine.inputNode.outputFormat(forBus: 0)
                            guard format.sampleRate > 0, format.channelCount > 0 else {
                                failed(id, "The microphone is unavailable after an audio device change. Saved audio is retained.")
                                return
                            }
                            self.installTap(engine: engine, buffer: buffer, id: id, limit: limit, failed: failed)
                            do { try engine.start() }
                            catch {
                                failed(id, "Microphone capture could not resume after an audio device change: " + error.localizedDescription)
                            }
                        }
                    }
                    try engine.start()
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
    private func installTap(engine: AVAudioEngine, buffer: StationDictatePCMBuffer, id: String,
                            limit: @escaping @MainActor (String) -> Void,
                            failed: @escaping @MainActor (String, String) -> Void) {
        // Let the input node supply its current format; the converter below
        // handles a format transition without losing PCM or resetting the cap.
        let tap = makeInputTap(buffer: buffer, id: id, limit: limit, failed: failed)
        engine.inputNode.installTap(onBus: 0, bufferSize: 1024, format: nil, block: tap)
    }
    // AVFoundation calls this block on its audio queue, never MainActor.
    // Keep that contract explicit even when compiled in the app's Swift target.
    nonisolated func makeInputTap(buffer: StationDictatePCMBuffer, id: String,
                      limit: @escaping @MainActor (String) -> Void,
                      failed: @escaping @MainActor (String, String) -> Void) -> @Sendable (AVAudioPCMBuffer, AVAudioTime) -> Void {
        { [weak self] chunk, _ in
            let event = buffer.append(chunk)
            if event != .none {
                DispatchQueue.main.async { [weak self] in
                    guard self?.operation == id else { return }
                    if event == .limit { limit(id) }
                    else { failed(id, "Audio capture or storage stopped. Saved audio is available in Settings → Voice → Recordings.") }
                }
            }
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
    public func finish(operation id: String) -> Data {
        guard operation == id else { return Data() }
        stopEngine()
        let audio = buffer?.take() ?? Data()
        buffer = nil; operation = nil
        return audio
    }
    public func cancel() {
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
final class StationDictatePCMBuffer: @unchecked Sendable {
    enum Event: Sendable, Equatable { case none, limit, failed }
    private let lock = NSLock()
    private var converter: AVAudioConverter
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
        guard input.format.sampleRate > 0, input.format.channelCount > 0 else {
            stopped = true; return .failed
        }
        if input.format != converter.inputFormat {
            guard let replacement = AVAudioConverter(from: input.format, to: format) else {
                stopped = true; return .failed
            }
            replacement.downmix = true
            converter = replacement
        }
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
