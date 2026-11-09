import Foundation
import PolarTmuxHostClient
import SwiftUI

/// Native drafts only; the Host validates and persists the complete revision.
public struct ConnectorSettingsView: View {
    public let request: ConnectorRequestHandler
    public let requestCodex: CodexRequestHandler
    public let revision: UInt64
    public let dismiss: @MainActor () -> Void
    @State private var draft = MessageConnectorSettingsV1()
    @State private var baseline = MessageConnectorSettingsV1()
    @State private var sources = CodexControlSettingsV1()
    @State private var localHost = ""
    @State private var loaded = false
    @State private var dirty = false
    @State private var busy = false
    @State private var error: String?
    @State private var streams: [ConnectorStreamV1] = []
    @State private var selectedStream: ConnectorStreamV1?
    @State private var returnHost = ""
    @State private var returnEndpoint = ""
    public init(request: @escaping ConnectorRequestHandler, requestCodex: @escaping CodexRequestHandler, revision: UInt64, dismiss: @escaping @MainActor () -> Void) {
        self.request = request; self.requestCodex = requestCodex; self.revision = revision; self.dismiss = dismiss
    }
    public var body: some View {
        VStack(alignment: .leading) {
            HStack { Text("Codex monitoring and messages").font(.headline); Spacer(); Button("Close", action: dismiss) }
            if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            if busy { ProgressView() }
            Form {
                Section("App-server monitoring") {
                    ForEach(sources.endpoints, id: \.id) { endpoint in
                        Toggle("\(endpoint.id) · \(endpoint.control_mode)", isOn: Binding(get: { !draft.disabled_monitors.contains(endpoint.id) }, set: { enabled in
                            draft.disabled_monitors.removeAll { $0 == endpoint.id }
                            if !enabled { draft.disabled_monitors.append(endpoint.id) }
                            dirty = true
                        }))
                    }
                    if sources.endpoints.isEmpty { Text("No app-server sources configured on this Host.") }
                    Text("Monitoring sources and message senders are selected independently.").foregroundStyle(.secondary)
                }
                Section("Message connectors") {
                    ForEach(draft.connectors.indices, id: \.self) { i in
                        VStack(alignment: .leading) {
                            TextField("Connector name", text: $draft.connectors[i].id)
                            Picker("Provider", selection: $draft.connectors[i].provider) { Text("Telegram").tag("telegram"); Text("iMessage").tag("imessage") }
                            Picker("Host", selection: Binding(get: { draft.connectors[i].connection_ref ?? "" }, set: { ref in
                                draft.connectors[i].connection_ref = ref.isEmpty ? nil : ref
                                draft.connectors[i].host_id = sources.host_connections.first(where: { $0.id == ref })?.host_id ?? localHost
                                dirty = true
                            })) {
                                Text("This Host").tag("")
                                ForEach(sources.host_connections, id: \.id) { Text($0.id).tag($0.id) }
                            }
                            Toggle("Enabled", isOn: $draft.connectors[i].enabled)
                            TextField("Allowed source Host IDs, one per line", text: Binding(get: { draft.connectors[i].allowed_source_hosts.joined(separator: "\n") }, set: { draft.connectors[i].allowed_source_hosts = $0.split(separator: "\n").map(String.init); dirty = true }), axis: .vertical)
                            if draft.connectors[i].provider == "telegram" { Text("Requires private Topics enabled for this bot. Replies inside each topic return to its Codex session.").font(.caption) }
                            else { Text("Native iMessage threads are unavailable until the SIP-enabled Messages integration is verified.").font(.caption).foregroundStyle(.secondary) }
                            HStack {
                                Button("Message streams") { loadStreams(draft.connectors[i].id) }.disabled(dirty || draft.connectors[i].connection_ref != nil)
                                Button("Remove", role: .destructive) { draft.connectors.remove(at: i); dirty = true }
                            }
                        }.padding(.vertical, 6)
                    }
                    Button("Add connector") { var c = MessageConnectorV1(); c.host_id = localHost; draft.connectors.append(c); dirty = true }
                }
                Section("Routes") {
                    ForEach(draft.routes.indices, id: \.self) { i in
                        VStack(alignment: .leading) {
                            Picker("Source", selection: $draft.routes[i].endpoint_id) {
                                Text("Choose source").tag("")
                                ForEach(sources.endpoints, id: \.id) { Text($0.id).tag($0.id) }
                            }
                            TextField("Session ID (empty means all)", text: Binding(get: { draft.routes[i].thread_id ?? "" }, set: { draft.routes[i].thread_id = $0.isEmpty ? nil : $0; dirty = true }))
                            Picker("Connector", selection: $draft.routes[i].connector_id) {
                                Text("Choose connector").tag("")
                                ForEach(draft.connectors, id: \.id) { Text($0.id).tag($0.id) }
                            }
                            TextField("Destination name", text: $draft.routes[i].destination_ref)
                            Toggle("Send completions", isOn: $draft.routes[i].enabled)
                            Button("Remove route", role: .destructive) { draft.routes.remove(at: i); dirty = true }
                        }.padding(.vertical, 6)
                    }
                    Button("Add route") { draft.routes.append(.init()); dirty = true }
                }
                if !streams.isEmpty {
                    Section("Message streams") {
                        ForEach(streams, id: \.id) { stream in
                            VStack(alignment: .leading) {
                                Text(stream.title).font(.headline)
                                Text(stream.thread_id).textSelection(.enabled)
                                Text("\(stream.status) · reply Host: \(stream.return_host_id)")
                                if let code = stream.error_code { Text(code).foregroundStyle(.red) }
                                Button("Change reply destination") {
                                    selectedStream = stream
                                    returnHost = stream.return_host_id
                                    returnEndpoint = stream.return_endpoint_id
                                }
                            }
                        }
                    }
                }
                if let stream = selectedStream {
                    Section("Reply destination: \(stream.title)") {
                        TextField("Host ID", text: $returnHost)
                        TextField("App-server source ID on that Host", text: $returnEndpoint)
                        Text("Pending or uncertain replies must be resolved before changing the destination.").font(.caption)
                        HStack {
                            Button("Apply reply destination") { changeReturnRoute(stream) }
                            Button("Cancel") { selectedStream = nil }
                        }
                    }
                }
            }.formStyle(.grouped).disabled(busy || !loaded)
            HStack {
                Button("Reload and discard draft") { Task { await reload(force: true) } }.disabled(busy)
                Spacer()
                Button("Save") { Task { await save() } }.disabled(!loaded || busy || !dirty)
            }
        }.padding(16).frame(minWidth: 580, idealWidth: 680, minHeight: 540, idealHeight: 700)
        .task { await reload(force: true) }
        .onChange(of: draft) { value in dirty = value != baseline }
        .onChange(of: revision) { _ in if !dirty { Task { await reload(force: false) } } }
    }
    @MainActor private func reload(force: Bool) async {
        guard !busy, force || !dirty else { return }; busy = true; error = nil
        defer { busy = false }
        do {
            let result = try await request(.getSettings)
            if let settings = result.settings { baseline = settings; draft = settings; localHost = result.host_id ?? ""; loaded = true; dirty = false }
            if case .settings(let value) = try await requestCodex(nil, .getSettings).result { sources = value }
        } catch { self.error = error.localizedDescription }
    }
    @MainActor private func save() async {
        guard !busy else { return }; busy = true; error = nil
        defer { busy = false }
        do { if let settings = try await request(.updateSettings(draft)).settings { baseline = settings; draft = settings; dirty = false } }
        catch { self.error = "Save failed; your draft is preserved. \(error.localizedDescription)" }
    }
    private func loadStreams(_ id: String) {
        Task { @MainActor in
            guard !busy else { return }; busy = true; error = nil
            defer { busy = false }
            do { streams = try await request(.getStreams(id)).streams ?? [] }
            catch { self.error = error.localizedDescription }
        }
    }
    private func changeReturnRoute(_ stream: ConnectorStreamV1) {
        Task { @MainActor in
            guard !busy else { return }; busy = true; error = nil
            defer { busy = false }
            do {
                _ = try await request(.setReturnRoute(stream: stream.id, expectedHost: stream.return_host_id, host: returnHost, endpoint: returnEndpoint))
                streams = try await request(.getStreams(stream.connector_id)).streams ?? []
                selectedStream = nil
            } catch { self.error = error.localizedDescription }
        }
    }
}
