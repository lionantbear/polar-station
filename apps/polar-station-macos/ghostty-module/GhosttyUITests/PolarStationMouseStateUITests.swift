import AppKit
import XCTest

final class PolarStationMouseStateUITests: XCTestCase {
    private var isolatedRoot: URL!
    private var app: XCUIApplication!
    private var preexistingPolarStationPIDs: Set<pid_t> = []

    override func setUpWithError() throws {
        continueAfterFailure = false
        let testID = String(UUID().uuidString.prefix(8))
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("psm-\(testID)")
        isolatedRoot = root
        for path in [
            root,
            root.appendingPathComponent("home"),
            root.appendingPathComponent("runtime"),
            root.appendingPathComponent("tmp"),
        ] {
            try FileManager.default.createDirectory(
                at: path,
                withIntermediateDirectories: true
            )
        }

        let config = root.appendingPathComponent("config.ghostty")
        try "initial-window = true\ncommand = /bin/zsh\n".write(
            to: config,
            atomically: true,
            encoding: .utf8
        )

        preexistingPolarStationPIDs = Set(Self.polarStationPIDs())
        app = XCUIApplication()
        app.launchArguments.append(contentsOf: [
            "-ApplePersistenceIgnoreState", "YES",
        ])
        app.launchEnvironment["GHOSTTY_CONFIG_PATH"] = config.path
        app.launchEnvironment["GHOSTTY_USER_DEFAULTS_SUITE"] =
            "POLAR_STATION_MOUSE_UI_TEST"
        app.launchEnvironment["HOME"] =
            root.appendingPathComponent("home").path
        app.launchEnvironment["XDG_DATA_HOME"] =
            root.appendingPathComponent("home/data").path
        app.launchEnvironment["XDG_RUNTIME_DIR"] =
            root.appendingPathComponent("runtime").path
        app.launchEnvironment["TMPDIR"] =
            root.appendingPathComponent("tmp").path
        app.launchEnvironment["POLAR_TMUX_IPC_SOCKET"] =
            "/tmp/psm-\(testID).sock"
        app.launch()
    }

    override func tearDownWithError() throws {
        app?.terminate()
        if let isolatedRoot {
            try? FileManager.default.removeItem(at: isolatedRoot)
        }
    }

    func testPassiveHoverAfterFocusTransitionDoesNotSelect() throws {
        let window = app.windows.firstMatch
        XCTAssertTrue(window.waitForExistence(timeout: 10))
        let terminal = window.groups["Terminal pane"].firstMatch
        XCTAssertTrue(terminal.waitForExistence(timeout: 10))

        app.typeKey("p", modifierFlags: [.command, .option])
        let controlCenter = app.descendants(matching: .any)[
            "polar-station-control-center"
        ].firstMatch
        XCTAssertTrue(controlCenter.waitForExistence(timeout: 5))

        app.typeKey("p", modifierFlags: [.command, .option])
        XCTAssertFalse(controlCenter.waitForExistence(timeout: 2))

        let freshClick = terminal.coordinate(
            withNormalizedOffset: CGVector(dx: 0.20, dy: 0.82)
        )
        freshClick.click()
        terminal.typeText(
            "printf 'POLAR_PASSIVE_POINTER_0123456789_ABCDEFGHIJKLMNOPQRSTUVWXYZ\\n'\n"
        )
        Thread.sleep(forTimeInterval: 0.5)

        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        XCTAssertTrue(pasteboard.setString(
            "POLAR_CLIPBOARD_SENTINEL",
            forType: .string
        ))

        terminal.coordinate(
            withNormalizedOffset: CGVector(dx: 0.78, dy: 0.82)
        ).hover()
        terminal.typeKey("c", modifierFlags: .command)
        XCTAssertEqual(
            pasteboard.string(forType: .string),
            "POLAR_CLIPBOARD_SENTINEL",
            "passive hover must not create a terminal selection"
        )

        freshClick.press(
            forDuration: 0.15,
            thenDragTo: terminal.coordinate(
                withNormalizedOffset: CGVector(dx: 0.78, dy: 0.82)
            )
        )
        terminal.typeKey("c", modifierFlags: .command)
        XCTAssertNotEqual(
            pasteboard.string(forType: .string),
            "POLAR_CLIPBOARD_SENTINEL",
            "an intentional left-button drag must continue to select"
        )
    }

    func testControlCenterSettlesAndPassiveRefreshDoesNotReclaimFocus() throws {
        let window = app.windows.firstMatch
        XCTAssertTrue(window.waitForExistence(timeout: 10))
        let terminal = window.groups["Terminal pane"].firstMatch
        XCTAssertTrue(terminal.waitForExistence(timeout: 10))

        app.typeKey("p", modifierFlags: [.command, .option])
        let controlCenter = app.descendants(matching: .any)[
            "polar-station-control-center"
        ].firstMatch
        XCTAssertTrue(controlCenter.waitForExistence(timeout: 5))

        Thread.sleep(forTimeInterval: 0.75)
        let settledTerminalFrame = terminal.frame
        Thread.sleep(forTimeInterval: 3)
        XCTAssertEqual(
            terminal.frame,
            settledTerminalFrame,
            "passive Host projections must not repeatedly reallocate panes"
        )

        let finder = XCUIApplication(bundleIdentifier: "com.apple.finder")
        finder.activate()
        let finderBecameFrontmost = XCTNSPredicateExpectation(
            predicate: NSPredicate { _, _ in
                NSWorkspace.shared.frontmostApplication?.bundleIdentifier
                    == "com.apple.finder"
            },
            object: nil
        )
        XCTAssertEqual(
            XCTWaiter.wait(for: [finderBecameFrontmost], timeout: 5),
            .completed
        )
        Thread.sleep(forTimeInterval: 4)
        XCTAssertEqual(
            NSWorkspace.shared.frontmostApplication?.bundleIdentifier,
            "com.apple.finder",
            "passive attach retries and projection refresh must not reactivate Polar Station"
        )
    }

    func testResourcesAuxiliaryWindowOpensIndependently() throws {
        app.terminate()
        app.launchEnvironment["POLAR_STATION_UI_TEST_AUXILIARY"] = "resources"
        app.launch()

        let resourcesWindow = app.windows["Resources"].firstMatch
        XCTAssertTrue(resourcesWindow.waitForExistence(timeout: 5))
        XCTAssertTrue(
            app.staticTexts["Global Resources"].firstMatch
                .waitForExistence(timeout: 5)
        )
        XCTAssertFalse(
            app.descendants(matching: .any)[
                "polar-station-control-center"
            ].firstMatch.exists,
            "Resources must not open the in-terminal Control Center"
        )
        XCTAssertEqual(
            app.windows.matching(identifier: "Resources").count,
            1,
            "Resources must have exactly one native window"
        )
    }

    func testResourcesAuxiliaryWindowIsNotRestoredOnSecondLaunch() throws {
        app.terminate()
        app.launchArguments.removeAll()
        app.launchEnvironment["POLAR_STATION_UI_TEST_AUXILIARY"] = "resources"
        app.launch()

        XCTAssertTrue(
            app.windows["Resources"].firstMatch.waitForExistence(timeout: 5)
        )
        app.terminate()

        app.launchEnvironment.removeValue(
            forKey: "POLAR_STATION_UI_TEST_AUXILIARY"
        )
        app.launch()

        XCTAssertTrue(app.windows.firstMatch.waitForExistence(timeout: 10))
        XCTAssertFalse(
            app.windows["Resources"].firstMatch.waitForExistence(timeout: 2),
            "Resources must reopen only after an explicit user action"
        )
    }

    func testNativeTabStripAndShortcutSelectionDoNotBounce() throws {
        let window = app.windows.firstMatch
        XCTAssertTrue(window.waitForExistence(timeout: 10))
        let applicationPID = try XCTUnwrap(
            Self.polarStationPID(excluding: preexistingPolarStationPIDs)
        )

        app.typeKey("p", modifierFlags: [.command, .option])
        let controlCenter = app.descendants(matching: .any)[
            "polar-station-control-center"
        ].firstMatch
        XCTAssertTrue(controlCenter.waitForExistence(timeout: 5))
        try addTab(named: "Native A")
        try addTab(named: "Native B")
        app.typeKey("p", modifierFlags: [.command, .option])
        XCTAssertTrue(waitForNonexistence(controlCenter, timeout: 5))
        let stableFrame = window.frame

        app.typeKey("2", modifierFlags: .command)
        XCTAssertTrue(
            waitForSelectedNativeTab("Native A", pid: applicationPID),
            "Command-2 did not select the second native Tab"
        )
        Thread.sleep(forTimeInterval: 1)
        XCTAssertEqual(Self.onScreenWindowName(pid: applicationPID), "Native A")

        let nativeB = app.buttons["Native B"].firstMatch
        XCTAssertTrue(nativeB.waitForExistence(timeout: 5))
        nativeB.click()
        XCTAssertTrue(
            waitForSelectedNativeTab("Native B", pid: applicationPID),
            "native Tab-strip pointer selection did not persist"
        )
        Thread.sleep(forTimeInterval: 1)
        XCTAssertEqual(Self.onScreenWindowName(pid: applicationPID), "Native B")

        app.typeKey("2", modifierFlags: .command)
        app.typeKey("3", modifierFlags: .command)
        XCTAssertTrue(waitForSelectedNativeTab("Native B", pid: applicationPID))
        Thread.sleep(forTimeInterval: 1)
        XCTAssertEqual(
            Self.onScreenWindowName(pid: applicationPID),
            "Native B",
            "a stale native-selection completion must not restore Native A"
        )
        XCTAssertEqual(
            Self.polarStationPID(excluding: preexistingPolarStationPIDs),
            applicationPID
        )
        XCTAssertEqual(window.frame, stableFrame)
        assertBackingWindowFramesAreCoherent()
    }

    func testNativeTabGeometryAndSurfaceRetirementStayStable() throws {
        let sessionName = "polar-lifecycle-\(UUID().uuidString.prefix(8))"
            .lowercased()
        try Self.runTmux([
            "new-session", "-d", "-s", sessionName,
            "/bin/zsh", "-lc", "while :; do sleep 60; done",
        ])
        addTeardownBlock {
            _ = try? Self.runTmux([
                "kill-session", "-t", "=\(sessionName)",
            ])
        }

        let window = app.windows.firstMatch
        XCTAssertTrue(window.waitForExistence(timeout: 10))
        let applicationPID = try XCTUnwrap(
            Self.polarStationPID(excluding: preexistingPolarStationPIDs)
        )

        app.typeKey("p", modifierFlags: [.command, .option])
        let controlCenter = app.descendants(matching: .any)[
            "polar-station-control-center"
        ].firstMatch
        XCTAssertTrue(controlCenter.waitForExistence(timeout: 5))

        try addTab(named: "Lifecycle A")
        try addTab(named: "Lifecycle B")
        for title in ["Main", "Lifecycle A", "Lifecycle B"] {
            let tab = app.buttons[title].firstMatch
            XCTAssertTrue(tab.waitForExistence(timeout: 5))
            tab.click()
            XCTAssertEqual(
                Self.polarStationPID(excluding: preexistingPolarStationPIDs),
                applicationPID
            )
            assertBackingWindowFramesAreCoherent()
        }
        let stableFrame = window.frame

        let sessions = app.buttons["Sessions"].firstMatch
        XCTAssertTrue(sessions.waitForExistence(timeout: 5))
        sessions.click()
        let refresh = app.buttons["Refresh sessions"].firstMatch
        XCTAssertTrue(refresh.waitForExistence(timeout: 5))
        refresh.click()

        let row = app.descendants(matching: .any)[sessionName].firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        let assigned = row.descendants(matching: .any)[
            "Assigned to selected tab"
        ].firstMatch

        for cycle in 1...20 {
            row.click()
            XCTAssertTrue(
                assigned.waitForExistence(timeout: 5),
                "cycle \(cycle) did not attach the disposable Local Session"
            )
            XCTAssertEqual(
                Self.polarStationPID(excluding: preexistingPolarStationPIDs),
                applicationPID
            )
            XCTAssertEqual(window.frame, stableFrame)
            assertBackingWindowFramesAreCoherent()

            row.click()
            XCTAssertTrue(
                waitForNonexistence(assigned, timeout: 5),
                "cycle \(cycle) did not remove the Local assignment"
            )
            XCTAssertEqual(
                Self.polarStationPID(excluding: preexistingPolarStationPIDs),
                applicationPID
            )
            XCTAssertTrue(window.exists)
            XCTAssertEqual(window.frame, stableFrame)
            XCTAssertTrue(Self.tmuxSessionExists(sessionName))
            XCTAssertTrue(
                waitUntil(timeout: 5) {
                    !Self.paneHostProcessExists(sessionName: sessionName)
                },
                "cycle \(cycle) retained a Pane host after Surface retirement"
            )
        }
    }

    private func addTab(named title: String) throws {
        let addTab = app.buttons["Add Tab"].firstMatch
        XCTAssertTrue(addTab.waitForExistence(timeout: 5))
        addTab.click()
        let field = app.textFields["Tab title"].firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.click()
        field.typeKey("a", modifierFlags: .command)
        field.typeText(title)
        app.buttons["Add"].firstMatch.click()
        XCTAssertTrue(app.buttons[title].firstMatch.waitForExistence(timeout: 5))
    }

    private func assertBackingWindowFramesAreCoherent(
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        let windows = app.windows.allElementsBoundByIndex.filter(\.exists)
        XCTAssertFalse(windows.isEmpty, file: file, line: line)
        guard let frame = windows.first?.frame else { return }
        for candidate in windows.dropFirst() {
            XCTAssertEqual(
                candidate.frame,
                frame,
                "every managed native Tab backing window must share one frame",
                file: file,
                line: line
            )
        }
    }

    private func waitForNonexistence(
        _ element: XCUIElement,
        timeout: TimeInterval
    ) -> Bool {
        waitUntil(timeout: timeout) { !element.exists }
    }

    private func waitForSelectedNativeTab(
        _ title: String,
        pid: pid_t,
        timeout: TimeInterval = 5
    ) -> Bool {
        waitUntil(timeout: timeout) {
            Self.onScreenWindowName(pid: pid) == title
        }
    }

    private func waitUntil(
        timeout: TimeInterval,
        predicate: @escaping () -> Bool
    ) -> Bool {
        let expectation = XCTNSPredicateExpectation(
            predicate: NSPredicate { _, _ in predicate() },
            object: nil
        )
        return XCTWaiter.wait(for: [expectation], timeout: timeout) == .completed
    }

    private static func tmuxSessionExists(_ sessionName: String) -> Bool {
        (try? runTmux([
            "has-session", "-t", "=\(sessionName)",
        ], acceptsFailure: true)) == 0
    }

    private static func polarStationPIDs() -> [pid_t] {
        NSRunningApplication.runningApplications(
            withBundleIdentifier: "com.polarealm.polar-station"
        ).filter { !$0.isTerminated }.map(\.processIdentifier)
    }

    private static func polarStationPID(
        excluding excluded: Set<pid_t>
    ) -> pid_t? {
        let candidates = polarStationPIDs().filter { !excluded.contains($0) }
        guard candidates.count == 1 else { return nil }
        return candidates[0]
    }

    private static func onScreenWindowName(pid: pid_t) -> String? {
        guard let windows = CGWindowListCopyWindowInfo(
            [.optionOnScreenOnly, .excludeDesktopElements],
            kCGNullWindowID
        ) as? [[String: Any]] else { return nil }
        return windows.first { window in
            (window[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value
                == pid
                && (window[kCGWindowLayer as String] as? NSNumber)?.intValue
                    == 0
        }?[kCGWindowName as String] as? String
    }

    private static func paneHostProcessExists(sessionName: String) -> Bool {
        let process = Process()
        let output = Pipe()
        process.executableURL = URL(fileURLWithPath: "/bin/ps")
        process.arguments = ["-axo", "command="]
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return true
        }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        let listing = String(data: data, encoding: .utf8) ?? ""
        return listing.split(separator: "\n").contains { command in
            command.contains("pane-host")
                && command.contains("--initial-session \(sessionName)")
        }
    }

    @discardableResult
    private static func runTmux(
        _ arguments: [String],
        acceptsFailure: Bool = false
    ) throws -> Int32 {
        let process = Process()
        let error = Pipe()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        process.arguments = ["tmux"] + arguments
        process.standardOutput = FileHandle.nullDevice
        process.standardError = error
        try process.run()
        process.waitUntilExit()
        if !acceptsFailure, process.terminationStatus != 0 {
            let data = error.fileHandleForReading.readDataToEndOfFile()
            let message = String(data: data, encoding: .utf8) ?? ""
            throw NSError(
                domain: "PolarStationMouseStateUITests.tmux",
                code: Int(process.terminationStatus),
                userInfo: [NSLocalizedDescriptionKey: message]
            )
        }
        return process.terminationStatus
    }
}
