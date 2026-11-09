import Foundation
import PolarTmuxHostClient
import SwiftUI

/// Read-only pages within the existing Inbox. No terminal draft or delivery state.
struct CodexInboxView: View {
    let request: CodexRequestHandler
    let revision: UInt64
    let openSession: @MainActor (PolarTmuxSessionKey) -> Void
    @State private var labels: [String: String] = [:]
    @State private var owners: [String] = []
    @State private var pages: [String: Page] = [:]
    @State private var busy = false
    @State private var queued = false
    @State private var error: String?
    private struct Page {
        var records: [CodexCompletionProjectionV1]
        var before: CodexCompletionCursor?
        var error: String?
    }
    private func text(_ key: String) -> String { labels[key] ?? "" }
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(labels["completions"] ?? "Codex").font(.caption.bold())
                Spacer()
                if busy { ProgressView().controlSize(.small) }
                Button(text("refresh")) { Task { await refresh(reset: true) } }.disabled(busy)
            }
            if let error { Text(error).font(.caption).foregroundStyle(.red) }
            ForEach(owners, id: \.self) { owner in
                Text(owner).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                if let page = pages[owner] {
                    if let error = page.error { Text(error).font(.caption).foregroundStyle(.red) }
                    if page.records.isEmpty { Text(text("no_completions")).font(.caption) }
                    ForEach(page.records, id: \.completion.id) { projection in
                        completionCard(projection)
                    }
                    if page.before != nil {
                        Button(text("more")) { Task { await earlier(owner) } }.disabled(busy)
                    }
                }
            }
        }
        .task { await refresh(reset: true) }
        .onChange(of: revision) { _ in
            if busy { queued = true }
            else { Task { await refresh(reset: false) } }
        }
        .accessibilityIdentifier("polar-station-codex-completions")
    }
    private func completionCard(_ projection: CodexCompletionProjectionV1) -> some View {
        let completion = projection.completion
        return VStack(alignment: .leading, spacing: 7) {
            HStack {
                Text(completion.origin.tmux_session_id ?? completion.origin.thread_id).font(.subheadline.bold())
                Spacer()
                Text(completion.status).font(.caption)
            }
            Text(completion.summary).font(.callout).textSelection(.enabled)
            ForEach(Array(projection.deliveries.enumerated()), id: \.offset) { _, delivery in
                VStack(alignment: .leading, spacing: 3) {
                    Text(delivery.label).font(.caption)
                    if let code = delivery.error_code {
                        DisclosureGroup(code) {
                            Text(delivery.status).font(.caption).textSelection(.enabled)
                        }.font(.caption)
                    }
                }
            }
            if let session = completion.origin.tmux_session_id {
                Button(text("open_session")) {
                    openSession(PolarTmuxSessionKey(executionTarget: completion.origin.execution_target_id, tmuxSessionID: session))
                }
            }
        }
        .padding(10).frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.secondary.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
    }
    @MainActor private func finish() {
        busy = false
        if queued {
            queued = false
            Task { await refresh(reset: false) }
        }
    }
    @MainActor private func refresh(reset: Bool) async {
        guard !busy else { queued = true; return }
        busy = true; error = nil
        defer { finish() }
        do {
            let ui = try await request(nil, .getUi)
            guard case .ui(let values) = ui.result else { error = "Codex control unavailable"; return }
            labels = values
            let settings = try await request(nil, .getSettings)
            guard case .settings(let settings) = settings.result else { error = text("operation_failed"); return }
            owners = Array(Set([ui.ownerHostID] + settings.endpoints.filter { $0.control_mode == "remote_host" }.map(\.owner_host_id))).sorted()
            for owner in owners { await load(owner, before: nil, reset: reset) }
        } catch {
            if !Task.isCancelled { self.error = labels.isEmpty ? error.localizedDescription : text("operation_failed") }
        }
    }
    @MainActor private func earlier(_ owner: String) async {
        guard !busy, let cursor = pages[owner]?.before else { return }
        busy = true
        defer { finish() }
        await load(owner, before: cursor, reset: false)
    }
    @MainActor private func load(_ owner: String, before: CodexCompletionCursor?, reset: Bool) async {
        do {
            let response = try await request(owner, .getCompletions(endpoint: nil, before: before, limit: 32))
            guard case .completions(let records, let next) = response.result else { failed(owner); return }
            if Task.isCancelled { return }
            let previous = pages[owner]
            // A front-page event must not remove earlier pages already opened.
            var merged = reset ? [] : (previous?.records ?? [])
            for record in records {
                merged.removeAll { $0.completion.id == record.completion.id }
                merged.append(record)
            }
            merged.sort {
                if $0.completion.created_at_ms != $1.completion.created_at_ms { return $0.completion.created_at_ms > $1.completion.created_at_ms }
                return $0.completion.id > $1.completion.id
            }
            // Keep the cursor from this exact page. A new front page may
            // introduce a gap; walking it again safely deduplicates older cards.
            pages[owner] = Page(records: merged, before: next, error: nil)
        } catch { if !Task.isCancelled { failed(owner) } }
    }
    @MainActor private func failed(_ owner: String) {
        if pages[owner] == nil { pages[owner] = Page(records: [], before: nil, error: nil) }
        pages[owner]?.error = text("operation_failed")
    }
}
