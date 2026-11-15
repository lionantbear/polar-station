import PolarTmuxHostClient
import SwiftUI

struct AgentRenameSheet: View {
    let agent: AgentSessionCatalogRowV1
    let close: () -> Void
    let submit: (String, @escaping @MainActor (String?) -> Void) -> Void
    @State private var name = ""
    @State private var pending = false
    @State private var error: String?
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Rename Codex Session").font(.headline)
            TextField("Session name", text: $name).disabled(pending)
            if let error { Text(error).foregroundStyle(.red) }
            HStack { Spacer(); Button("Cancel", action: close).disabled(pending)
                Button("Save") {
                    pending = true
                    submit(name.trimmingCharacters(in: .whitespacesAndNewlines)) { failure in
                        pending = false; error = failure
                        if failure == nil { close() }
                    }
                }.keyboardShortcut(.defaultAction).disabled(pending || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }.padding(24).frame(width: 420).onAppear { name = agent.display_name }
    }
}
