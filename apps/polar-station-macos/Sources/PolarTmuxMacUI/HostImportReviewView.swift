import SwiftUI
import PolarTmuxHostClient

/// The shared Connections engine owns choices and apply eligibility.
public struct HostImportReviewView: View {
    @ObservedObject var interaction: ConnectionsInteraction
    let consume: ([ConnectionsInteraction.Effect]) -> Void
    @State private var selected = 0
    @State private var candidateID = ""
    private var plan: HostImportPlanV1? { interaction.state?.host_import_plan }
    private var row: HostImportRowV1? { plan?.rows.indices.contains(selected) == true ? plan?.rows[selected] : nil }
    private var candidate: HostImportCandidateV1? { candidateID.isEmpty ? row?.candidates.first : row?.candidates.first(where: { $0.target_id == candidateID }) }
    public init(interaction: ConnectionsInteraction,consume: @escaping ([ConnectionsInteraction.Effect])->Void) {self.interaction=interaction;self.consume=consume}
    public var body: some View {
        VStack(alignment:.leading,spacing:16) {
            Text("Import Hosts").font(.title2)
            Text("Compare all information, then associate the source with an existing Host. Existing settings stay unchanged.").foregroundStyle(.secondary)
            HStack(alignment:.top,spacing:16) {
                List(selection:Binding(get:{selected},set:{selected=$0; restoreChoice()})) {
                    ForEach(Array((plan?.rows ?? []).enumerated()),id:\.offset) { index,item in
                        VStack(alignment:.leading,spacing:4) {
                            Text(item.host.display_name)
                            Text(interaction.state?.host_import_decisions.first(where:{$0.source == item.host.source})?.action ?? "Needs review").font(.caption).foregroundStyle(.secondary)
                        }.tag(index)
                    }
                }.frame(width:220)
                if let row {
                    VStack(alignment:.leading,spacing:12) {
                        Text(row.host.display_name).font(.headline)
                        if let targets=plan?.targets, !targets.isEmpty {
                            Picker("Existing Host",selection:Binding(get:{candidateID.isEmpty ? candidate?.target_id ?? "" : candidateID},set:{candidateID=$0; if !candidateID.isEmpty { consume(interaction.compareHostImport(row.host.source,target:candidateID)) }})) {
                                Text("Choose an existing Host…").tag("")
                                ForEach(targets,id:\.target_id) { item in Text(item.display_name).tag(item.target_id) }
                            }
                        }
                        ScrollView {
                            Group {
                                Grid(alignment:.topLeading,horizontalSpacing:16,verticalSpacing:10) {
                                    GridRow { Text("Field").bold(); Text("Local").bold(); Text("Imported").bold(); Text("Change").bold() }
                                    ForEach(candidate?.fields ?? row.fields,id:\.field) { field in
                                        GridRow {
                                            Text(field.label)
                                            Text(field.local ?? "Unknown").textSelection(.enabled)
                                            Text(field.imported ?? "Unknown").textSelection(.enabled)
                                            Text(field.change.replacingOccurrences(of:"_",with:" ")).foregroundStyle(.secondary)
                                        }.font(.callout)
                                    }
                                }.frame(maxWidth:.infinity,alignment:.leading)
                            }
                        }
                        HStack {
                            Button("Skip") { choose("skip",row) }
                            Button("Create separate") { choose("create",row) }.disabled(row.host.connection.kind == "local" || row.disposition == "duplicate_address")
                            Spacer()
                            if let candidate {
                                Button("Associate with this Host") {
                                    consume(interaction.chooseHostImport(.init(action:"merge",source:row.host.source,target:candidate.target_id,fields:[],allowRemoval:false)))
                                }
                            }
                        }
                        if row.disposition == "duplicate_address" { Text("Address, port and SSH username match an existing Host. Select a Host to save an association.").foregroundStyle(.secondary) }
                    }.frame(maxWidth:.infinity,maxHeight:.infinity,alignment:.topLeading)
                }
            }
            Divider()
            if let message=interaction.state?.message { Text(message).font(.callout).textSelection(.enabled) }
            HStack {
                Button("Cancel") { consume(interaction.action("cancel_host_import")) }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Apply import") { consume(interaction.action("apply_host_import")) }
                    .disabled(interaction.state?.host_import_can_apply != true).keyboardShortcut(.defaultAction)
            }
        }.padding(24).frame(minWidth:1060,minHeight:660)
        .disabled(interaction.state?.pending == true)
        .interactiveDismissDisabled(interaction.state?.pending == true)
    }
    private func restoreChoice() {
        let decision = interaction.state?.host_import_decisions.first(where: { $0.source == row?.host.source })
        candidateID = decision?.target_id ?? ""
    }
    private func choose(_ action:String,_ row:HostImportRowV1) { consume(interaction.chooseHostImport(.init(action:action,source:row.host.source))) }
}
