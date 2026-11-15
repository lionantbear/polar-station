import SwiftUI
import PolarTmuxHostClient

public struct WorkspaceChooserView: View {
    public let choices: [NativeWorkspaceChoiceView]
    public let send: (ControlCenterIntentOperationV1) -> Void
    @State private var creating = false
    @State private var name = "Workspace"
    public init(choices: [NativeWorkspaceChoiceView], send: @escaping (ControlCenterIntentOperationV1) -> Void) {
        self.choices = choices; self.send = send
    }
    public var body: some View {
        VStack(spacing: 16) {
            Text("Open Workspace").font(.title2)
            ForEach(choices.filter { !$0.archived }, id: \.workspaceID) { choice in
                Button(choice.name) { send(.selectWorkspace(workspaceID: choice.workspaceID)) }
            }
            Button("New Workspace…") { creating = true }
            if choices.contains(where: \.archived) {
                Menu("Archived Workspaces") {
                    ForEach(choices.filter(\.archived), id: \.workspaceID) { choice in
                        Button("Restore \(choice.name)") {
                            send(.invokeConfirmedAction(actionID: "workspace.unarchive", confirmationID: choice.workspaceID))
                        }
                    }
                }
            }
        }
        .padding(32).frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
        .alert("New Workspace", isPresented: $creating) {
            TextField("Name", text: $name)
            Button("Cancel", role: .cancel) {}
            Button("Create") { send(.createWorkspace(name: name)) }
                .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
    }
}
