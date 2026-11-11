#!/usr/bin/env swift
// Native LaunchServices gate. No Apple Events, Accessibility, screenshots, or
// system security changes. Run in a logged-in macOS graphical session.
import AppKit
import CoreGraphics
import Foundation
import Darwin

func pump(_ seconds: TimeInterval) {
    _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(seconds))
}
func canonical(_ url: URL) -> URL { url.standardizedFileURL.resolvingSymlinksInPath() }

var args = Array(CommandLine.arguments.dropFirst())
var appPath: String?
var reportPath: String?
while !args.isEmpty {
    let flag = args.removeFirst()
    guard !args.isEmpty, flag == "--app" || flag == "--report" else {
        fputs("Usage: swift verify_trial_app.swift --app PATH [--report NEW_FILE]\n", stderr)
        exit(2)
    }
    let value = args.removeFirst()
    if flag == "--app" { appPath = value } else { reportPath = value }
}
guard let path = appPath else { fputs("--app is required\n", stderr); exit(2) }
let appURL = canonical(URL(fileURLWithPath: path))
guard appURL.pathExtension == "app", FileManager.default.fileExists(atPath: appURL.path),
      Bundle(url: appURL) != nil else { fputs("Expected installed app bundle\n", stderr); exit(2) }
if let reportPath = reportPath, FileManager.default.fileExists(atPath: reportPath) {
    fputs("Refusing to overwrite report\n", stderr); exit(2)
}
for key in ["GH_TOKEN", "GITHUB_TOKEN"] {
    if let value = ProcessInfo.processInfo.environment[key], !value.isEmpty {
        fputs("Refusing to launch with GitHub token in verifier environment\n", stderr)
        exit(2)
    }
}
let started = Date()
let existingPIDs = Set(NSWorkspace.shared.runningApplications.map { $0.processIdentifier })
let config = NSWorkspace.OpenConfiguration()
config.createsNewApplicationInstance = true
config.activates = true
config.arguments = [] // Do not pass -e: exercise ordinary .app launch/runtime.
// Do not forward GitHub credentials or dynamic-loader overrides to the app.
var environment: [String: String] = ["PATH": "/usr/bin:/bin:/usr/sbin:/sbin"]
for key in ["HOME", "TMPDIR", "USER", "LOGNAME", "LANG"] {
    if let value = ProcessInfo.processInfo.environment[key] { environment[key] = value }
}
config.environment = environment
var launched: NSRunningApplication?
var launchError: String?
var completed = false
NSWorkspace.shared.openApplication(at: appURL, configuration: config) { app, error in
    launched = app
    launchError = error?.localizedDescription
    completed = true
}
let deadline = Date().addingTimeInterval(60)
while !completed && Date() < deadline { pump(0.1) }
var report: [String: Any] = [
    "passed": false, "requested_app": appURL.path,
    "started_utc": ISO8601DateFormatter().string(from: started),
    "scope": "Native macOS CI LaunchServices .app launch and owning-PID layer-0 window metadata only. Not clean VM, Gatekeeper, rendered content, terminal interaction, TCC or updater acceptance.",
    "launch_callback_received": completed,
    "cleanup_requested": false, "force_terminate_requested": false,
    "natural_exit": false
]
if let launchError = launchError { report["launch_error"] = launchError }
if let app = launched {
    let pid = app.processIdentifier
    let actualURL = app.bundleURL.map(canonical)
    report["pid"] = Int(pid)
    report["bundle_url"] = actualURL?.path ?? ""
    report["bundle_identifier"] = app.bundleIdentifier ?? ""
    report["launch_date"] = app.launchDate.map { ISO8601DateFormatter().string(from: $0) } ?? ""
    let newPID = !existingPIDs.contains(pid)
    report["new_pid"] = newPID
    let exact = actualURL == appURL
    report["exact_bundle_url"] = exact
    var alive = exact && newPID && launchError == nil
    var windows: [[String: Any]] = []
    if alive {
        let observationStart = Date()
        while Date().timeIntervalSince(observationStart) < 15 {
            if app.isTerminated { alive = false; break }
            pump(0.1)
        }
        report["observed_seconds"] = Date().timeIntervalSince(observationStart)
        alive = alive && !app.isTerminated
        if let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] {
            report["window_list_available"] = true
            for row in rows {
                guard (row[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
                      (row[kCGWindowLayer as String] as? NSNumber)?.intValue == 0 else { continue }
                // Avoid window titles or other private content. Metadata only.
                windows.append([
                    "window_id": (row[kCGWindowNumber as String] as? NSNumber)?.intValue ?? -1,
                    "layer": 0,
                    "bounds": row[kCGWindowBounds as String] ?? [:]
                ])
            }
        } else { report["window_list_available"] = false }
    }
    report["alive_after_observation"] = alive
    report["layer0_window_count"] = windows.count
    report["windows"] = windows
    report["functional_check_passed"] = alive && !windows.isEmpty
    report["terminated_before_cleanup"] = app.isTerminated
    // Only act on the exact newly launched NSRunningApplication instance.
    if exact && newPID && !app.isTerminated {
        report["cleanup_requested"] = true
        report["terminate_request_accepted"] = app.terminate()
        let gracefulDeadline = Date().addingTimeInterval(10)
        while !app.isTerminated && Date() < gracefulDeadline { pump(0.1) }
        if !app.isTerminated {
            report["force_terminate_requested"] = true
            report["force_terminate_accepted"] = app.forceTerminate()
            let forceDeadline = Date().addingTimeInterval(5)
            while !app.isTerminated && Date() < forceDeadline { pump(0.1) }
        }
    }
    report["terminated_after_cleanup"] = app.isTerminated
    let graceful = report["cleanup_requested"] as? Bool == true &&
                   report["force_terminate_requested"] as? Bool == false && app.isTerminated
    report["graceful_requested_exit"] = graceful
    // Forced cleanup is evidence of a problem, never a passing/natural exit.
    report["passed"] = alive && !windows.isEmpty && graceful
    if !exact || !newPID { report["error"] = "Unexpected bundle or pre-existing PID; no termination attempted" }
} else {
    report["error"] = completed ? "Launch did not return an application" : "Launch callback timed out; launch outcome uncertain; no process guessed or terminated"
}
do {
    let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
    if let reportPath = reportPath {
        try data.write(to: URL(fileURLWithPath: reportPath), options: .withoutOverwriting)
    }
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
} catch {
    fputs("Cannot serialize/write launch report: \(error)\n", stderr)
    exit(2)
}
exit(report["passed"] as? Bool == true ? 0 : 1)
