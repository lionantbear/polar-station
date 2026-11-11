// swift-tools-version: 6.0
import Foundation
import PackageDescription

let packageDirectory = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
let repositoryRoot = packageDirectory
    .deletingLastPathComponent()
    .deletingLastPathComponent()
let clientLibraryDirectory = ProcessInfo.processInfo.environment[
    "POLAR_STATION_CLIENT_LIB_DIR"
] ?? repositoryRoot
    .appendingPathComponent("target/polar-station-client/current")
    .path

let package = Package(
    name: "PolarTmuxMac",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "PolarTmuxHostClient", targets: ["PolarTmuxHostClient"]),
        .library(name: "PolarTmuxMacUI", targets: ["PolarTmuxMacUI"]),
        .library(
            name: "PolarStationNativeWorkspace",
            targets: ["PolarStationNativeWorkspace"]
        ),
    ],
    targets: [
        .target(
            name: "PolarTmuxHostClient",
            dependencies: ["CPolarStationDesktopClient"]
        ),
        .target(
            name: "CPolarStationDesktopClient",
            path: "Sources/CPolarStationDesktopClient",
            publicHeadersPath: "include",
            linkerSettings: [
                .unsafeFlags(["-L\(clientLibraryDirectory)", "-lpolar_tmux"]),
                // Native dependencies of the linked Rust archive must also be
                // available to standalone SwiftPM tests and other consumers.
                .linkedLibrary("bz2"),
                .linkedLibrary("z"),
                .linkedLibrary("lzma"),
                .linkedLibrary("c++"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
        .target(
            name: "PolarTmuxMacUI",
            dependencies: ["PolarTmuxHostClient"]
        ),
        .target(
            name: "PolarStationNativeWorkspace",
            dependencies: ["PolarTmuxHostClient"],
            path: "ghostty-module",
            exclude: [
                "ControlCenterStore.swift",
                "PolarStationNativeExecutor.swift",
                "PolarStationNativeTabGroupCoordinator.swift",
                "PolarStationPaneSurfaceCoordinator.swift",
                "PolarStationRuntime.swift",
                "PolarStationDisplaySurface.swift",
                "PolarStationGraphicsView.swift",
                "PolarStationLgpWorkspace.swift",
                "PolarStationInputBar.swift",
                "PolarStationDictateAudio.swift",
                "PolarStationDictate.swift",
                "PolarStationShortcuts.swift",
                "ControlCenterWidthPreferences.swift",
                "GhosttyUITests",
                "merge-static-libs.sh",
            ],
            sources: [
                "ManagedApplicationPresentationState.swift",
                "NativeSurfaceRetirementState.swift",
                "NativeTabGroupPresentationState.swift",
                "NativeWorkspaceLayout.swift",
                "NativeWorkspaceState.swift",
                "StationDictateKeyCapture.swift",
            ]
        ),
    ]
)
