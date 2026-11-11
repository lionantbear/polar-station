import AppKit

/// Physical capture only. No recording, provider, focus, or editor calls.
@MainActor
final class StationDictateKeyCapture {
    private var held = false
    private var globeHeld = false
    /// A missing setting uses an OS-dependent default; it is not proof that
    /// the Globe short-tap action has been disabled. Never write this domain.
    static var systemGlobeActionDisabled: Bool {
        _ = CFPreferencesAppSynchronize("com.apple.HIToolbox" as CFString)
        return (CFPreferencesCopyAppValue("AppleFnUsageType" as CFString,
            "com.apple.HIToolbox" as CFString) as? NSNumber)?.intValue == 0
    }
    func handle(_ event: NSEvent, binding text: String, emit: (UInt32) -> Void) -> Bool {
        let binding = Self.binding(text) ?? (63, NSEvent.ModifierFlags.function)
        let modifiers = event.modifierFlags.intersection([.control, .option, .shift, .command, .function])
        // Some keyboards/remappers emit a companion Globe key (179) beside
        // Fn's modifier edges, sometimes after Fn-up. It is not another chord
        // or another gesture. Balance its release even if modifiers change.
        if event.keyCode == 179 {
            if event.type == .keyUp, globeHeld { globeHeld = false; return true }
            if text == "Fn", modifiers.subtracting(.function).isEmpty,
               event.type == .keyDown || event.type == .keyUp {
                globeHeld = event.type == .keyDown
                return true
            }
        }
        if event.keyCode == binding.0 {
            if (event.type == .flagsChanged || event.type == .keyDown), modifiers == binding.1 {
                if !held { held = true; emit(1) }
                return true
            }
            if held, event.type == .flagsChanged || event.type == .keyUp {
                held = false; emit(2); return true
            }
        }
        if held, event.type == .flagsChanged, !modifiers.isSuperset(of: binding.1) {
            held = false; emit(2); return false
        }
        // Fn navigation is passed through after emitting cancellation.
        if held, event.type == .keyDown || event.type == .flagsChanged {
            held = false; emit(4)
        }
        return false
    }
    static func binding(_ text: String) -> (UInt16, NSEvent.ModifierFlags)? {
        switch text {
        case "Fn": return (63, .function)
        case "RightOption": return (61, .option)
        case "Control+Option+Space": return (49, [.control, .option])
        case "Control+Shift+Space": return (49, [.control, .shift])
        default: return nil
        }
    }
}
