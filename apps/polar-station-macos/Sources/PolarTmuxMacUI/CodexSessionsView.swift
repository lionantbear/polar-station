import Foundation
import PolarTmuxHostClient
import SwiftUI

public struct CodexSessionsView: View {
    public let request: CodexRequestHandler
    public let revision: UInt64
    public let dismiss: @MainActor () -> Void
    @State private var sessions: [CodexStateV1] = []
    @State private var selected: CodexBindingV1?
    @State private var error: String?
    @State private var busy = false
    public init(request: @escaping CodexRequestHandler, revision: UInt64, dismiss: @escaping @MainActor () -> Void) {
        self.request = request; self.revision = revision; self.dismiss = dismiss
    }
    public var body: some View {
        VStack(alignment: .leading) {
            HStack { Text("Codex sessions").font(.headline); Spacer(); Button("Close", action: dismiss) }
            if let error { Text(error).foregroundStyle(.red) }
            if busy { ProgressView() }
            List(sessions, id: \.binding.binding_id) { state in
                Button { selected = state.binding } label: {
                    VStack(alignment: .leading) {
                        Text(state.thread_name ?? "Unnamed session").font(.headline)
                        Text(state.binding.thread_id).textSelection(.enabled)
                        Text("\(state.binding.endpoint_id) · \(state.connection) · \(state.activity)").foregroundStyle(.secondary)
                    }
                }.buttonStyle(.plain)
            }
            if sessions.isEmpty && !busy { Text("No observed Codex sessions. Configure an app-server monitoring source in Settings.") }
            Button("Refresh") { Task { await reload() } }.disabled(busy)
        }.padding(16).frame(minWidth: 560, minHeight: 460)
        .task { await reload() }
        .onChange(of: revision) { _ in Task { await reload() } }
        .sheet(isPresented: Binding(get: { selected != nil }, set: { if !$0 { selected = nil } })) {
            if let selected { CodexSessionControlView(session: selected.session, stateRevision: revision, request: request, dismiss: { self.selected = nil }, binding: selected) }
        }
    }
    @MainActor private func reload() async {
        guard !busy else { return }; busy = true; error = nil
        defer { busy = false }
        do {
            guard case .settings(let settings) = try await request(nil, .getSettings).result else { return }
            var collected: [CodexStateV1] = []
            // Source projections already constrain remote endpoint aliases in Rust.
            for owner in Set(settings.endpoints.map(\.owner_host_id)) {
                let result = try await request(owner, .getBindings).result
                if case .bindings(let values) = result { collected.append(contentsOf: values) }
                if case .error(let code) = result { throw NSError(domain: "Codex", code: 1, userInfo: [NSLocalizedDescriptionKey: code]) }
            }
            sessions = collected
        } catch { self.error = error.localizedDescription }
    }
}
