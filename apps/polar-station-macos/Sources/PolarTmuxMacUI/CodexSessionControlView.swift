import Foundation
import PolarTmuxHostClient
import SwiftUI

/// Native form state only. Every command keeps its original binding/turn revision.
public struct CodexSessionControlView: View {
    public let session: PolarTmuxSessionKey?
    private let initialBinding: CodexBindingV1?
    public let stateRevision: UInt64
    public let request: CodexRequestHandler
    public let dismiss: @MainActor () -> Void
    @State private var labels: [String: String] = [:]
    @State private var endpoints: [CodexEndpointSettingsV1] = []
    @State private var endpointID = ""
    @State private var threads: [CodexLoadedThreadV1] = []
    @State private var state: CodexStateV1?
    @State private var destinations: [TelegramDestinationV1] = []
    @State private var telegram = ""
    @State private var imessage = false
    @State private var input = ""
    @State private var notificationsDirty = false
    @State private var operationError: String?
    @State private var receipt: String?
    @State private var error: String?
    @State private var busy = false
    @State private var pending: Pending?
    private struct Pending { let action: ActionProjectionV1; let command: CodexCommandV1; let owner: String }
    public init(session: PolarTmuxSessionKey?, stateRevision: UInt64, request: @escaping CodexRequestHandler, dismiss: @escaping @MainActor () -> Void, binding: CodexBindingV1? = nil) {
        self.initialBinding = binding
        self.session = session; self.stateRevision = stateRevision; self.request = request; self.dismiss = dismiss
    }
    private func text(_ id: String) -> String { labels[id] ?? "" }
    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text(labels["title"] ?? "Codex").font(.headline)
                Spacer()
                Button(labels["close"] ?? "Close", action: dismiss).keyboardShortcut(.cancelAction)
            }
            if let session { Text("\(session.tmuxSessionID) · \(session.executionTarget)").foregroundStyle(.secondary) }
            if busy { ProgressView() }
            if let error { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            if let operationError { Text(operationError).foregroundStyle(.red).textSelection(.enabled) }
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if let state, let presentation = state.presentation {
                        Text(state.thread_name ?? "Unnamed session").font(.headline)
                        Text(state.binding.thread_id).textSelection(.enabled)
                        Text(presentation.connection_label).font(.headline)
                        Text(presentation.task_label)
                        Text(state.binding.owner_host_id).foregroundStyle(.secondary).textSelection(.enabled)
                        if let detail = presentation.detail { Text(detail).foregroundStyle(.secondary) }
                        GroupBox(text("notifications")) {
                            VStack(alignment: .leading) {
                                Picker(text("telegram"), selection: Binding(get: { telegram }, set: { telegram = $0; notificationsDirty = true })) {
                                    Text(text("none")).tag("")
                                    ForEach(destinations, id: \.id) { Text($0.id).tag($0.id) }
                                }
                                Toggle(text("imessage"), isOn: Binding(get: { imessage }, set: { imessage = $0; notificationsDirty = true }))
                                Button(text("save_notifications")) {
                                    submit(.updateNotifications(id: state.binding.binding_id, revision: state.binding.revision, telegram: telegram.isEmpty ? nil : telegram, imessage: imessage), owner: state.binding.owner_host_id)
                                }.disabled(busy)
                            }
                        }.disabled(busy || error != nil)
                        Text(presentation.input_label)
                        TextEditor(text: $input).frame(minHeight: 100).font(.body)
                            .accessibilityLabel(presentation.input_label)
                            .disabled(busy)
                        ForEach(presentation.actions, id: \.id) { action in
                            Button(action.label) { invoke(action, state: state) }
                                .disabled(busy || error != nil || !action.enabled)
                                .help(action.disabledReason ?? action.accessibilityLabel)
                                .accessibilityLabel(action.accessibilityLabel)
                        }
                        if let answer = state.final_answer { Text(answer).textSelection(.enabled) }
                    } else {
                        if endpoints.isEmpty { Text(text("no_endpoint")) }
                        Picker(text("endpoint"), selection: $endpointID) {
                            Text(text("choose_endpoint")).tag("")
                            ForEach(endpoints, id: \.id) { Text($0.id).tag($0.id) }
                        }.disabled(busy)
                        Button(text("find_loaded")) { Task { await findLoaded() } }
                            .disabled(busy || endpointID.isEmpty)
                        ForEach(threads, id: \.thread_id) { thread in
                            Button { bind(thread) } label: { Text("\(text("connect")): \(thread.display_name)") }
                                .disabled(busy)
                        }
                    }
                    if let receipt { Text(receipt).textSelection(.enabled) }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            Button(text("refresh")) { Task { await reload() } }.disabled(busy)
        }
        .padding(20).frame(minWidth: 520, idealWidth: 620, minHeight: 480, idealHeight: 620)
        .task { await reload() }
        .onChange(of: stateRevision) { _ in if !busy { Task { await reload() } } }
        .confirmationDialog(pending?.action.confirmation?.title ?? "", isPresented: Binding(get: { pending != nil }, set: { if !$0 { pending = nil } }), titleVisibility: .visible) {
            if let pending, let confirmation = pending.action.confirmation {
                Button(confirmation.confirmLabel, role: confirmation.destructive ? .destructive : nil) {
                    submit(pending.command, owner: pending.owner)
                    self.pending = nil
                }
                Button(text("cancel"), role: .cancel) { self.pending = nil }
            }
        } message: { Text(pending?.action.confirmation?.message ?? "") }
        .accessibilityIdentifier("polar-station-codex-session")
    }
    @MainActor private func result(_ owner: String?, _ command: CodexCommandV1) async throws -> CodexResultV1 {
        let result = try await request(owner, command).result
        if case .error = result { error = text("operation_failed") }
        return result
    }
    @MainActor private func reload() async {
        guard !busy else { return }
        busy = true; error = nil
        defer { busy = false }
        do {
            if labels.isEmpty, case .ui(let values) = try await result(nil, .getUi) { labels = values }
            if let binding = initialBinding {
                if case .binding(let value) = try await result(binding.owner_host_id, .getBinding(id: binding.binding_id)) { installState(value) }
                if case .settings(let settings) = try await result(binding.owner_host_id, .getSettings) { destinations = settings.telegram.destinations }
                return
            }
            guard let session else { return }
            if case .settings(let settings) = try await result(nil, .getSettings) {
                endpoints = settings.endpoints.filter { $0.execution_target_id == session.executionTarget }
                if endpointID.isEmpty && endpoints.count == 1 { endpointID = endpoints[0].id }
            }
            var observedBindings = false
            for owner in Set(endpoints.map(\.owner_host_id)) {
                guard case .bindings(let bindings) = try await result(owner, .getBindings) else { continue }
                observedBindings = true
                if let binding = bindings.first(where: { $0.binding.session == session }) {
                    if case .binding(let value) = try await result(owner, .getBinding(id: binding.binding.binding_id)) { installState(value) }
                    if case .settings(let settings) = try await result(owner, .getSettings) { destinations = settings.telegram.destinations }
                    return
                }
            }
            if observedBindings && error == nil { installState(nil) }
        } catch { self.error = labels.isEmpty ? error.localizedDescription : text("operation_failed") }
    }
    @MainActor private func findLoaded() async {
        guard !busy, let endpoint = endpoints.first(where: { $0.id == endpointID }) else { return }
        busy = true; error = nil
        defer { busy = false }
        do {
            if case .loaded(let values) = try await result(endpoint.owner_host_id, .listLoaded(endpoint: endpoint.id)) {
                threads = values
                if values.isEmpty { error = text("no_loaded") }
            }
        } catch { self.error = text("operation_failed") }
    }
    private func bind(_ thread: CodexLoadedThreadV1) {
        guard let session, let endpoint = endpoints.first(where: { $0.id == endpointID }) else { return }
        submit(.bind(session: session, endpoint: endpoint.id, thread: thread.thread_id), owner: endpoint.owner_host_id)
    }
    private func invoke(_ action: ActionProjectionV1, state: CodexStateV1) {
        let b = state.binding
        let command: CodexCommandV1
        switch action.id {
        case "queue": command = .queueText(id: b.binding_id, revision: b.revision, intent: UUID().uuidString, text: input)
        case "steer":
            guard let turn = state.turn_id else { return }
            command = .steer(id: b.binding_id, revision: b.revision, intent: UUID().uuidString, turn: turn, text: input)
        case "interrupt":
            guard let turn = state.turn_id else { return }
            command = .interrupt(id: b.binding_id, revision: b.revision, intent: UUID().uuidString, turn: turn)
        case "resume": command = .resume(id: b.binding_id, revision: b.revision)
        case "unbind": command = .unbind(id: b.binding_id, revision: b.revision)
        default: return
        }
        if action.confirmation != nil { pending = Pending(action: action, command: command, owner: b.owner_host_id) }
        else { submit(command, owner: b.owner_host_id) }
    }
    private func installState(_ value: CodexStateV1?) {
        if state?.binding.binding_id != value?.binding.binding_id { notificationsDirty = false }
        state = value
        if !notificationsDirty {
            telegram = value?.binding.telegram_destination_ref ?? ""
            imessage = value?.binding.imessage_enabled ?? false
        }
    }
    private func submit(_ command: CodexCommandV1, owner: String) {
        guard !busy else { return }
        busy = true; operationError = nil
        Task { @MainActor in
            do {
                let value = try await result(owner, command)
                if case .receipt(let value) = value, let value {
                    receipt = value.label
                    if value.clear_input == true { input = "" }
                }
                if case .error = value { operationError = text("operation_failed") }
                if case .binding(let value) = value {
                    if case .updateNotifications = command { notificationsDirty = false }
                    installState(value)
                }
            } catch { operationError = text("operation_failed") }
            busy = false
            await reload()
        }
    }
}
