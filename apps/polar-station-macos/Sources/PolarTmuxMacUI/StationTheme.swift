import AppKit
import SwiftUI
import PolarTmuxHostClient

/// Native palette application; product choices and light/dark resolution come from Rust.
@MainActor public final class StationTheme: ObservableObject {
    public static let shared = StationTheme()
    @Published public private(set) var palette: ThemePaletteV1?
    @Published public private(set) var error: String?
    public var reloadTerminal: (() -> Void)?
    private var projection: SettingsProjectionV1?
    private var observer: NSObjectProtocol?
    private var appliedTerminal: String?
    private let configURL = FileManager.default.temporaryDirectory.appendingPathComponent("polar-station-theme-\(ProcessInfo.processInfo.processIdentifier).conf")
    private init() {
        observer = DistributedNotificationCenter.default().addObserver(forName: NSNotification.Name("AppleInterfaceThemeChangedNotification"), object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.apply() }
        }
    }
    public func stop() {
        try? FileManager.default.removeItem(at: configURL)
        unsetenv("POLAR_STATION_THEME_CONFIG")
        appliedTerminal = nil
        reloadTerminal = nil
    }
    public func update(_ projection: SettingsProjectionV1) {
        guard projection.themeProjection != nil else { return }
        self.projection = projection
        apply()
    }
    public static var themesDirectory: URL? { Bundle.main.resourceURL?.appendingPathComponent("ghostty/themes", isDirectory: true) }
    public static let terminalThemes: [String] = {
        guard let root = themesDirectory else { return [] }
        return ((try? FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: [.isRegularFileKey])) ?? [])
            .filter { (try? $0.resourceValues(forKeys: [.isRegularFileKey]).isRegularFile) == true }
            .map(\.lastPathComponent).sorted { $0.localizedStandardCompare($1) == .orderedAscending }
    }()
    private func apply() {
        guard let projection, let choices = projection.themeProjection else { return }
        // Read the OS preference independently of our manually forced app appearance.
        let systemDark = UserDefaults.standard.string(forKey: "AppleInterfaceStyle") == "Dark"
        let selected = systemDark ? choices.dark : choices.light
        let name = projection.appearance.theme.terminal_theme ?? selected.name
        let terminalKey = name + ":" + (selected.dark ? "dark" : "light")
        if terminalKey != appliedTerminal {
            do {
                guard let root = Self.themesDirectory else { throw CocoaError(.fileNoSuchFile) }
                let data = try String(contentsOf: root.appendingPathComponent(name), encoding: .utf8)
                // Ghostty parses the original bundled file. Clear an earlier user theme;
                // preserve all unrelated config (fonts, bindings, shell and working directory).
                let config = "theme =\n" + data + "\nminimum-contrast = 4.5\nwindow-theme = \(selected.dark ? "dark" : "light")\n"
                try config.write(to: configURL, atomically: true, encoding: .utf8)
                try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: configURL.path)
                setenv("POLAR_STATION_THEME_CONFIG", configURL.path, 1)
                appliedTerminal = terminalKey
                reloadTerminal?()
            } catch {
                self.error = "Cannot load terminal theme ‘\(name)’: \(error.localizedDescription)"
                return
            }
        }
        self.error = nil
        self.palette = selected
        NSApp.appearance = NSAppearance(named: selected.dark ? .darkAqua : .aqua)
    }
    public var background: Color { palette.map { Color(stationHex: $0.background) } ?? Color(nsColor: .windowBackgroundColor) }
    public var surface: Color { palette.map { Color(stationHex: $0.surface) } ?? Color(nsColor: .controlBackgroundColor) }
    public var accent: Color { palette.map { Color(stationHex: $0.accent) } ?? .accentColor }
}

extension Color {
    init(stationHex: String) {
        let n = UInt32(stationHex.dropFirst(), radix: 16) ?? 0
        self.init(.sRGB, red: Double((n >> 16) & 255) / 255, green: Double((n >> 8) & 255) / 255, blue: Double(n & 255) / 255, opacity: 1)
    }
}
private struct StationThemeModifier: ViewModifier {
    @ObservedObject private var theme = StationTheme.shared
    func body(content: Content) -> some View {
        content
            .scrollContentBackground(.hidden)
            .tint(theme.accent).accentColor(theme.accent)
            .foregroundStyle(theme.palette.map { Color(stationHex: $0.foreground) } ?? .primary)
            .background(theme.background)
            .preferredColorScheme(theme.palette.map { $0.dark ? .dark : .light })
    }
}
public extension View {
    func stationTheme() -> some View { modifier(StationThemeModifier()) }
}

struct StationThemePicker: View {
    let projection: SettingsProjectionV1?
    let save: (AppearanceSettingsV1) -> Void
    @ObservedObject private var theme = StationTheme.shared
    @State private var filter = ""
    private var selection: ThemeSettingsV1 { projection?.appearance.theme ?? .init() }
    private func change(_ edit: (inout ThemeSettingsV1) -> Void) {
        guard var appearance = projection?.appearance else { return }
        edit(&appearance.theme); save(appearance)
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Toggle("Follow system appearance", isOn: Binding(get: { selection.mode == "system" }, set: { enabled in change { $0.mode = enabled ? "system" : "preset" } }))
                .accessibilityLabel("Follow system appearance")
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 10), count: 3), spacing: 10) {
                ForEach(projection?.themeProjection?.choices ?? []) { choice in
                    Button {
                        change { $0.mode = "preset"; $0.preset = choice.id; $0.terminal_theme = nil }
                    } label: {
                        VStack(alignment: .leading, spacing: 8) {
                            HStack {
                                Text(choice.name).font(.caption).lineLimit(1)
                                Spacer(minLength: 0)
                                if selection.mode == "preset" && selection.preset == choice.id { Image(systemName: "checkmark.circle.fill") }
                            }
                            HStack(spacing: 5) {
                                ForEach([choice.accent, choice.selection_background, choice.foreground], id: \.self) { color in
                                    RoundedRectangle(cornerRadius: 3).fill(Color(stationHex: color)).frame(height: 13)
                                }
                            }
                            Text("$ codex").font(.system(.caption, design: .monospaced))
                        }.padding(10).frame(maxWidth: .infinity, alignment: .leading)
                            .foregroundStyle(Color(stationHex: choice.foreground))
                            .background(Color(stationHex: choice.background), in: RoundedRectangle(cornerRadius: 8))
                            .overlay(RoundedRectangle(cornerRadius: 8).stroke(selection.mode == "preset" && selection.preset == choice.id ? Color(stationHex: choice.accent) : Color.secondary.opacity(0.3), lineWidth: 2))
                    }.buttonStyle(.plain).accessibilityLabel("Theme \(choice.name)")
                }
            }
            DisclosureGroup("Advanced terminal colors") {
                TextField("Search bundled terminal themes", text: $filter)
                    .accessibilityLabel("Search bundled terminal themes")
                Picker("Terminal theme", selection: Binding(get: { selection.terminal_theme ?? "" }, set: { name in change { $0.terminal_theme = name.isEmpty ? nil : name } })) {
                    Text("Same as application").tag("")
                    ForEach(StationTheme.terminalThemes.filter { filter.isEmpty || $0.localizedCaseInsensitiveContains(filter) || $0 == selection.terminal_theme }, id: \.self) { Text($0).tag($0) }
                }
                .accessibilityLabel("Terminal theme")
                Text("Overrides terminal colors only.").font(.caption).foregroundStyle(.secondary)
            }
            if let error = theme.error { Text(error).foregroundStyle(.red) }
        }.disabled(projection?.themeProjection == nil)
    }
}
