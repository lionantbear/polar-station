public enum NativeWindowRestorationPolicy {
    private static let stationOwnedWindowIdentifiers: Set<String> = [
        "resources",
        "iphone",
        "TerminalWindowRestoration",
    ]

    public static func shouldDiscardPersistedWindow(
        identifier: String
    ) -> Bool {
        stationOwnedWindowIdentifiers.contains(identifier)
    }
}
