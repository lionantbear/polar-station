import Foundation
import PolarTmuxHostClient
import SwiftUI

struct SessionDirectoryPicker: View {
    let embedded: Bool
    let target: String
    let endpoint: String?
    let read: SessionEditorReader
    let choose: @MainActor (String) -> Void
    let cancel: @MainActor () -> Void
    @State private var path: String
    @State private var typedPath: String
    @State private var directory: SessionDirectoryV1?
    @State private var failure: String?
    @State private var loading = false
    @State private var retry = 0
    init(target: String, endpoint: String?, initialPath: String, embedded: Bool = false, read: @escaping SessionEditorReader,
         choose: @escaping @MainActor (String) -> Void, cancel: @escaping @MainActor () -> Void) {
        self.embedded = embedded
        self.target = target; self.endpoint = endpoint; self.read = read
        self.choose = choose; self.cancel = cancel
        _path = State(initialValue: initialPath.isEmpty ? "~" : initialPath)
        _typedPath = State(initialValue: initialPath)
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Choose Startup Directory").font(.title3.bold())
            HStack {
                Button("Home") { path = directory?.home ?? "~" }
                Button("Up") { if let parent = directory?.parent { path = parent } }.disabled(directory?.parent == nil)
                TextField("Path", text: $typedPath).onSubmit { path = typedPath; retry += 1 }
            }
            if let directory {
                ScrollView(.horizontal) {
                    HStack(spacing: 3) {
                        Button("/") { path = "/" }
                        ForEach(Array(directory.path.split(separator: "/").enumerated()), id: \.offset) { index, part in
                            Text("/")
                            Button(String(part)) { path = "/" + directory.path.split(separator: "/").prefix(index + 1).joined(separator: "/") }
                        }
                    }.buttonStyle(.plain)
                }
                List(directory.directories, id: \.self) { folder in
                    Button { path = folder } label: { Label((folder as NSString).lastPathComponent, systemImage: "folder") }
                        .buttonStyle(.plain)
                }
            }
            if loading { ProgressView("Reading the selected machine…") }
            if let failure { Text(failure).foregroundStyle(.red); Button("Retry") { retry += 1 } }
            HStack {
                Spacer()
                if !embedded { Button("Cancel", role: .cancel, action: cancel) }
                Button("Choose This Directory") { if let directory { choose(directory.path) } }
                    .disabled(loading || failure != nil || directory == nil)
            }
        }.padding(18).frame(width: embedded ? nil : 520, height: embedded ? nil : 420)
        .task(id: "\(path)|\(retry)") {
            loading = true; failure = nil
            do {
                guard case .directory(let value) = try await read(.browseDirectory(target: target, path: path, endpoint: endpoint)) else {
                    throw PolarTmuxLocalIPCError.unexpectedFrame("No target directory listing")
                }
                guard !Task.isCancelled else { return }
                directory = value; typedPath = value.path
            } catch { if !Task.isCancelled { failure = error.localizedDescription } }
            if !Task.isCancelled { loading = false }
        }
    }
}
