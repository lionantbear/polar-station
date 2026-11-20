import PolarTmuxHostClient
import SwiftUI

struct SessionDirectoryPicker: View {
    let state: SessionEditorInteraction.DirectoryBrowser
    let selectedPath: String
    let navigate: @MainActor (String) -> Void
    let choose: @MainActor (String) -> Void
    let retry: @MainActor () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let listing = state.listing {
                HStack {
                    Button("Home") { navigate(listing.home) }
                    Button("Up") { if let parent = listing.parent { navigate(parent) } }
                        .disabled(listing.parent == nil)
                    ScrollView(.horizontal) {
                        HStack(spacing: 4) {
                            Button("/") { navigate("/") }
                            ForEach(Array(listing.path.split(separator: "/").enumerated()), id: \.offset) { index, part in
                                Image(systemName: "chevron.right")
                                    .accessibilityHidden(true)
                                Button(String(part)) {
                                    navigate("/" + listing.path.split(separator: "/").prefix(index + 1).joined(separator: "/"))
                                }
                            }
                        }.buttonStyle(.plain)
                    }
                }
                if listing.directories.isEmpty {
                    Text("No subfolders").foregroundStyle(.secondary)
                } else {
                    ScrollView {
                        LazyVStack(spacing: 2) {
                            ForEach(listing.directories, id: \.self) { path in
                                folderRow(path)
                            }
                        }
                    }.frame(height: 152)
                }
            } else if state.loading {
                ProgressView("Reading the selected machine…").controlSize(.small)
            } else if let error = state.error {
                HStack {
                    Text(error).foregroundStyle(.secondary)
                    Button("Retry", action: retry)
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(StationTheme.shared.surface, in: RoundedRectangle(cornerRadius: 8))
    }

    private func folderRow(_ path: String) -> some View {
        let name = (path as NSString).lastPathComponent
        let selected = path == selectedPath
        return HStack(spacing: 8) {
            Button { choose(path) } label: {
                HStack {
                    Label(name, systemImage: "folder")
                    Spacer()
                    if selected { Image(systemName: "checkmark").accessibilityHidden(true) }
                }.contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityAddTraits(selected ? .isSelected : [])
            Button { navigate(path) } label: { Image(systemName: "chevron.right") }
                .accessibilityLabel("Browse \(name)")
        }
        .padding(6)
        .background(selected ? StationTheme.shared.accent.opacity(0.14) : Color.clear,
                    in: RoundedRectangle(cornerRadius: 5))
    }
}
