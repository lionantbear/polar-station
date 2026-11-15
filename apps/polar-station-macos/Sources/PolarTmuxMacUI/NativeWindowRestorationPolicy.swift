public enum NativeWindowRestorationPolicy {
    private static let processLocalAuxiliaryWindowIdentifiers: Set<String> = [
        "resources",
        "iphone",
    ]

    public static func shouldDiscardPersistedWindow(
        identifier: String
    ) -> Bool {
        processLocalAuxiliaryWindowIdentifiers.contains(identifier)
    }
}
