import SwiftUI
import PolarTmuxHostClient

/// Raw native field buffers only. Rust Settings owns edit leases and persistence.
@MainActor public final class SettingsFormBuffer:ObservableObject {
    public let messagesEdits=SettingsEditInteraction()
    @Published var messages = ImessageSenderSettingsV1()
    @Published public var messagesRecipient = ""
    @Published var allowedSources = ""
    @Published var widthText = ""
    @Published var appearance = AppearanceSettingsV1()
    @Published var summary = SessionSummarySettingsV1()
    @Published var monitoring = MonitoringSettingsV1()
    @Published var interfaceTextScale: Double = 100
    @Published public var tmuxStatusFieldsDraft: Set<String>?
    public init() {}
}

@MainActor public final class VoiceFormBuffer:ObservableObject {
    let edits=SettingsEditInteraction()
    @Published var cleanup = true
    @Published var context = true
    @Published var glossary = ""
    @Published var loadedGlossary = ""
    @Published var microphone = ""
    @Published var retentionEnabled=true
    @Published var retentionDays=7
    @Published var testText = ""
    public init() {}
}


/// Original Connector form buffers survive its native Settings host unmounting.
@MainActor public final class ConnectorFormBuffer:ObservableObject {
    @Published var draft = MessageConnectorSettingsV1()
    @Published var baseline = MessageConnectorSettingsV1()
    @Published var sources = CodexControlSettingsV1()
    @Published var localHost = ""
    @Published var loaded = false
    @Published var dirty = false
    @Published var busy = false
    @Published var error:String?
    @Published var streams:[ConnectorStreamV1] = []
    @Published var selectedStream:ConnectorStreamV1?
    @Published var returnHost = ""
    @Published var returnEndpoint = ""
    public init() {}
}

/// Original native Connections controls only. Rust retains the authoritative
/// typed draft, dirty/leave decisions and revision-bound save lifecycle.
@MainActor public final class ConnectionsFormBuffer:ObservableObject {
    var initialized = false
    var presentedRoute:[String] = []
    @Published var machine = MachineConnectionV1()
    @Published var polar = PolarConnectionV1()
    @Published var editingPolar = false
    @Published var existing = false
    @Published var draftRevision = ""
    @Published var submitting = false
    @Published var message: String?
    @Published var confirmingRemove = false
    @Published var newMachineBaseline: Set<String>?
    @Published var newPolarBaseline: Set<String>?
    @Published var savedRevision: String?
    @Published var checkedTarget: String?
    @Published var routeEnabled = false
    @Published var routeExpanded = false
    @Published var lanAddress = ""
    @Published var lanPort = "22"
    @Published var wanHost = ""
    @Published var wanPort = "22888"
    @Published var doh = "google, cloudflare"
    @Published var selectedMachine = "local"
    @Published var page = "Connection"
    @Published var search = ""
    @Published var editing = false
    @Published var codexEditing = false
    public init() {}
}
