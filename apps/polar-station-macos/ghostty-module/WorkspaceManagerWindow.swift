#if POLAR_STATION
import AppKit
import SwiftUI
import CPolarStationDesktopClient
import PolarTmuxHostClient
import PolarTmuxMacUI

/// The native catalog remains the only owner. This window borrows saved details.
@MainActor final class WorkspaceManagerWindow: ObservableObject {
    struct Row: Decodable, Identifiable {
        let id: String
        let name: String
        let summary: String
        let status: String
        let archived: Bool
    }
    struct HostReference: Decodable, Identifiable {
        let id: String
        let name: String
        let access: String
        let pane_count: Int
        let status: String
    }
    struct Projection: Decodable {
        let title: String
        let subtitle: String
        let archived_label: String
        let rows: [Row]
        let selected_id: String?
        let actions: [ActionProjectionV1]
        let page_actions: [ActionProjectionV1]
        let error: String?
        let hosts: [HostReference]
        let details_title: String
        let preview_caption: String
        let hosts_title: String
    }
    @Published private(set) var projection: Projection?
    @Published private(set) var catalog: PolarStationNativeWorkspaceCatalog?
    @Published var selected: String? { didSet { if oldValue != selected { project() } } }
    @Published var expanded = false { didSet { project() } }
    @Published private(set) var message = ""
    var ideaSources: (() -> [String:[String]])?
    var requestHostTransfer: HostTransferReader?
    var connections: ConnectionsProjectionV1? {
        didSet {
            if let connections, oldValue?.revision != connections.revision {
                hostInteraction.snapshot(connections)
            }
        }
    }
    private var transferPackage: [String:Any]?
    private let hostInteraction=ConnectionsInteraction()
    private var current: String?
    private var machines: [MachineConnectionV1]?
    private(set) var sessions: [SessionRowProjectionV1] = []
    private var pending = false
    private var applying = false
    private weak var window: NSWindow?
    @Published var review: Review?
    @Published var reviewName = ""
    struct Review {
        let title: String
        let text: String
        let confirm: String
        let apply: () -> Void
        var alternate: String? = nil
        var applyAlternate: (() -> Void)? = nil
    }

    func attach(to window: NSWindow, current: String?, selected: String?, catalog: PolarStationNativeWorkspaceCatalog?, machines: [MachineConnectionV1]?, sessions: [SessionRowProjectionV1]) {
        self.window = window
        self.current = current
        self.catalog = catalog
        self.machines = machines
        self.sessions = sessions
        self.selected = selected ?? current
        project()
    }

    var isPreview: Bool { selected != nil && selected != current }
    var reviewHostsInteraction: ConnectionsInteraction { hostInteraction }
    @Published var reviewingHosts = false

    func applyHostReview(_ effects: [ConnectionsInteraction.Effect]) { consumeHosts(effects) }

    func invokeWindowAction(_ kind: String) {
        if let selected { openWorkspace(selected, as: kind) }
    }

    var dispatch: (@MainActor (String, String, String) async throws -> Void)?

    func refresh(_ catalog: PolarStationNativeWorkspaceCatalog?, current: String?, machines: [MachineConnectionV1]?, sessions: [SessionRowProjectionV1]) {
        if self.current != current {
            self.current = current
            selected = current
        }
        guard window != nil, self.catalog != catalog || self.machines != machines || self.sessions != sessions else { return }
        self.catalog = catalog
        self.machines = machines
        self.sessions = sessions
        project()
    }
    private func project() {
        guard !applying, let catalog else { return }
        applying = true
        defer { applying = false }
        let archived = Set(catalog.archivedWorkspaceIDs ?? [])
        let open = Set(catalog.openWorkspaceIDs ?? [])
        let rows: [[String: Any]] = catalog.workspaces.map { w in
            ["id":w.id,"name":w.name,"archived":archived.contains(w.id),"open":open.contains(w.id),"tab_count":w.tabs.count,"pane_count":w.tabs.reduce(0) { $0 + $1.panes.count }]
        }
        var input: [String: Any] = ["workspaces":rows,"archived_expanded":expanded,"pending":pending,"transfer_available":connections?.workspaceTransferSupported == true]
        if let selected { input["selected"] = selected }
        if let current { input["current"] = current }
        do {
            let workspace = catalog.workspaces.first { $0.id == (selected ?? current ?? catalog.selectedWorkspaceID) }
            input["required_hosts"] = workspace?.tabs.flatMap(\.panes).compactMap { pane in
                let id = pane.executionTargetID ?? pane.content?.executionTargetID ?? ""
                return id.isEmpty ? nil : id
            } ?? []
            if let machines { input["machines"] = try JSONSerialization.jsonObject(with:JSONEncoder().encode(machines)) }
            let data = try JSONSerialization.data(withJSONObject: input)
            try data.withUnsafeBytes { buffer in
                guard let pointer = polar_station_workspace_manager_view(buffer.bindMemory(to: UInt8.self).baseAddress,buffer.count) else {
                    throw PolarStationNativeWorkspaceError.invalidStructure
                }
                defer { polar_station_desktop_string_free(pointer) }
                let view = try JSONDecoder().decode(Projection.self,from:Data(String(cString:pointer).utf8))
                projection = view
                if !pending { selected = view.selected_id }
                message = view.error ?? ""
            }
        } catch { message = error.localizedDescription }
    }
    func invoke(_ action: ActionProjectionV1) {
        guard action.enabled, !pending, window != nil else { return }
        let kind = String(action.id.dropFirst("workspace.manage.".count))
        let id = selected ?? ""
        let name = ""
        if kind == "open" { openWorkspace(id); return }
        if kind == "export" { exportWorkspace(); return }
        if kind == "import" { importWorkspace(); return }
        if let confirmation = action.confirmation {
            review = Review(title: confirmation.title, text: confirmation.message, confirm: confirmation.confirmLabel, apply: { [weak self] in
                self?.run(kind, id: id, name: "")
            })
            return
        }
        if kind == "rename" || kind == "new" {
            reviewName = kind == "new" ? "Workspace" : catalog?.workspaces.first(where: { $0.id == id })?.name ?? ""
            review = Review(title: kind == "new" ? "New Workspace" : "Rename Workspace", text: "", confirm: "Save", apply: { [weak self] in
                guard let self else { return }
                run(kind, id: id, name: reviewName)
            })
            return
        }
        run(kind,id:id,name:name)
    }
    private func run(_ action:String,id:String,name:String) {
        guard let dispatch, !pending else { return }
        pending = true
        project()
        Task { @MainActor in
            var failure: String?
            do { try await dispatch(action,id,name) }
            catch { failure = error.localizedDescription }
            pending = false
            if failure == nil && action == "import_commit" { selected = id }
            project()
            if let failure { message = failure }
            else if action == "import_commit" { message = "Workspace imported" }
        }
    }
}

struct WorkspaceLayoutDetail: View {
    @ObservedObject var manager: WorkspaceManagerWindow
    @ObservedObject var model: ControlCenterStore
    let liveLayout: NativeWorkspaceLayoutView?
    let emit: @MainActor (ControlCenterIntentV1) -> Void

    var body: some View {
        let selected = manager.catalog?.workspaces.first(where: { $0.id == manager.selected })
        let archived = manager.catalog?.archivedWorkspaceIDs?.contains(selected?.id ?? "") == true
        let opened = manager.catalog?.openWorkspaceIDs?.contains(selected?.id ?? "") == true
        let layout = manager.isPreview ? selected.flatMap { PolarStationNativeExecutor.layoutView(for: $0) } : liveLayout
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                WorkspaceLayoutPicker(title: selected?.name ?? "Workspace", choices: choices) { manager.selected = $0 }
                Spacer()
                if let projection = manager.projection {
                    ForEach(projection.page_actions.filter { $0.id.hasSuffix(".import") }, id: \.id) { action in
                        Button(action.label) { manager.invoke(action) }.disabled(!action.enabled)
                    }
                    ForEach(projection.actions.filter {
                        !$0.id.hasSuffix(".open") && (!manager.isPreview || !$0.id.hasSuffix(".rename"))
                    }, id: \.id) { action in
                        Button(action.label) { manager.invoke(action) }.disabled(!action.enabled)
                    }
                }
            }
            if manager.isPreview {
                HStack {
                    Text(archived ? "Archived · Preview" : "Workspace preview").foregroundStyle(.secondary)
                    if !archived {
                        if opened {
                            Button("Go to Existing Window") { manager.invokeWindowAction("open") }
                        } else {
                            Button("Switch Here") { manager.invokeWindowAction("open") }
                            Button("Open in New Window") { manager.invokeWindowAction("open_new") }
                        }
                    }
                }
            }
            if manager.reviewingHosts {
                HostImportReviewView(interaction: manager.reviewHostsInteraction, consume: manager.applyHostReview)
            } else if let review = manager.review {
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(review.title).font(.headline)
                        Text(review.text).textSelection(.enabled)
                        if review.confirm == "Import Workspace" || review.confirm == "Save" {
                            TextField("Workspace name", text: $manager.reviewName)
                        }
                        HStack {
                            Button(review.confirm) { manager.review = nil; review.apply() }
                            if let alternate = review.alternate {
                                Button(alternate) { manager.review = nil; review.applyAlternate?() }
                            }
                            Button("Cancel") { manager.review = nil }
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                WorkspaceMapView(
                    controlRevision: model.projection?.controlRevision ?? 0,
                    nativeView: layout,
                    sessions: model.projection?.sessions.rows ?? [],
                    liveSessions: Array(model.sessionRuntime.values),
                    expanded: true, readOnly: manager.isPreview
                ) { if !manager.isPreview { emit($0) } }
                .id(manager.selected)
            }
            if !manager.message.isEmpty { Text(manager.message).foregroundStyle(.secondary) }
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(StationTheme.shared.surface)
    }

    private var choices: [NativeWorkspaceChoiceView] {
        let archived = Set(manager.catalog?.archivedWorkspaceIDs ?? [])
        return (manager.catalog?.workspaces ?? []).map {
            NativeWorkspaceChoiceView(workspaceID: $0.id, name: $0.name,
                                      selected: $0.id == manager.selected, archived: archived.contains($0.id))
        }
    }
}

extension WorkspaceManagerWindow {
    static func transfer(_ object:[String:Any]) throws -> [String:Any] {
        let data=try JSONSerialization.data(withJSONObject:object)
        return try data.withUnsafeBytes { buffer in
            guard let pointer=polar_station_workspace_transfer(buffer.bindMemory(to:UInt8.self).baseAddress,buffer.count) else {throw PolarStationNativeWorkspaceError.invalidStructure}
            defer {polar_station_desktop_string_free(pointer)}
            guard let result=try JSONSerialization.jsonObject(with:Data(String(cString:pointer).utf8)) as? [String:Any] else {throw PolarStationNativeWorkspaceError.invalidStructure}
            if let message=result["error"] as? String {throw NSError(domain:"Workspace",code:1,userInfo:[NSLocalizedDescriptionKey:message])}
            return result
        }
    }
    private func object<T:Encodable>(_ value:T) throws -> Any {try JSONSerialization.jsonObject(with:JSONEncoder().encode(value))}
    private func request(_ object:[String:Any]) async throws -> HostTransferResponseV1 {
        guard let requestHostTransfer else {throw PolarStationNativeExecutorError.workspaceUnavailable}
        return try await requestHostTransfer(JSONDecoder().decode(HostTransferRequestV1.self,from:JSONSerialization.data(withJSONObject:object)))
    }
    private func openWorkspace(_ id:String, as action: String = "open") {
        guard !pending,let w=catalog?.workspaces.first(where:{$0.id==id}),let dispatch else {return}
        pending=true;project()
        Task { @MainActor in
            var failure:String?
            do {
                let native=try object(w)
                let sources=try Self.transfer(["action":"sources","native":native])
                if sources["needed"] as? Bool == true {
                    let response=try await request(["type":"resolve","sources":sources["sources"] ?? []])
                    guard response.type=="resolved",let associations=response.associations else {throw PolarStationNativeWorkspaceError.invalidStructure}
                    let resolved=try Self.transfer(["action":"resolve_native","native":native,"associations":try object(associations),"polar_instances":try object(connections?.polarInstances ?? [])])
                    if resolved["changed"] as? Bool == true {
                        let bytes=try JSONSerialization.data(withJSONObject:resolved["native"] ?? [:])
                        try await dispatch("resolve_commit",id,String(decoding:bytes,as:UTF8.self))
                    }
                }
                try await dispatch(action,id,"")
            } catch {failure=error.localizedDescription}
            pending=false;project();if let failure {message=failure}
        }
    }
    private func exportWorkspace() {
        guard let window,let w=catalog?.workspaces.first(where:{$0.id==selected}),!pending else {return}
        pending=true;project()
        Task { @MainActor in
            var failure:String?
            do {
                let native=try object(w)
                let prepared=try Self.transfer(["action":"prepare_export","native":native])
                let response=try await request(["type":"export_content","bindings":prepared["bindings"] ?? [],"target_ids":prepared["target_ids"] ?? []])
                guard response.type=="content_exported",let bundle=response.bundle,let sources=response.sources else {throw PolarStationNativeWorkspaceError.invalidStructure}
                let package=try Self.transfer(["action":"export","native":native,"bundle":try object(bundle),"sources":try object(sources),"source_workspace":w.id,"idea_sources":ideaSources?() ?? [:]])
                let data=try JSONSerialization.data(withJSONObject:package,options:[.prettyPrinted,.sortedKeys])
                let panel=NSSavePanel();panel.nameFieldStringValue="workspace.polar-workspace.json"
                panel.beginSheetModal(for:window) { [weak self] result in
                    guard result == .OK,let url=panel.url else {return}
                    do {try data.write(to:url,options:.atomic);try FileManager.default.setAttributes([.posixPermissions:0o600],ofItemAtPath:url.path)}
                    catch {self?.message=error.localizedDescription}
                }
            } catch {failure=error.localizedDescription}
            pending=false;project();if let failure {message=failure}
        }
    }
    private func importWorkspace() {
        guard let window,!pending else {return}
        let panel=NSOpenPanel();panel.canChooseDirectories=false;panel.allowsMultipleSelection=false
        panel.beginSheetModal(for:window) { [weak self] result in
            guard let self,result == .OK,let url=panel.url else {return}
            do {
                let size=try url.resourceValues(forKeys:[.fileSizeKey]).fileSize ?? 0
                guard size<=8*1024*1024 else {throw NSError(domain:"Workspace",code:1,userInfo:[NSLocalizedDescriptionKey:"Workspace file exceeds 8 MiB"])}
                guard let package=try JSONSerialization.jsonObject(with:Data(contentsOf:url)) as? [String:Any] else {throw PolarStationNativeWorkspaceError.invalidStructure}
                _=try Self.transfer(["action":"inspect","package":package]);transferPackage=package;reviewHosts()
            } catch {message=error.localizedDescription}
        }
    }
    private func reviewHosts() {
        guard let package = transferPackage else { return }
        reviewingHosts = false
        review = Review(title: "Workspace Hosts",
                        text: "Review included Hosts or continue to see which references are recognized.",
                        confirm: "Continue", apply: { [weak self] in self?.resolveImport() },
                        alternate: "Review Hosts…", applyAlternate: { [weak self] in
            guard let self else { return }
            do {
                let bundle = try JSONDecoder().decode(HostExportBundleV1.self,
                    from: JSONSerialization.data(withJSONObject: package["hosts"] ?? [:]))
                if let connections { hostInteraction.snapshot(connections) }
                consumeHosts(hostInteraction.importHosts(bundle))
            } catch { message = error.localizedDescription }
        })
    }
    private func consumeHosts(_ effects:[ConnectionsInteraction.Effect]) {
        for effect in effects {
            guard effect.type=="host_transfer",let request=effect.request,let token=effect.token,let requestHostTransfer else {continue}
            Task { @MainActor in
                let next:[ConnectionsInteraction.Effect]
                do {next=hostInteraction.transferComplete(token,result:try await requestHostTransfer(request),error:nil)}
                catch {next=hostInteraction.transferComplete(token,result:nil,error:error.localizedDescription)}
                if !next.isEmpty { consumeHosts(next); return }
                if hostInteraction.state?.host_import_plan != nil { reviewingHosts = true }
                else { reviewHosts() }
            }
        }
        if effects.isEmpty, hostInteraction.state?.host_import_plan == nil { reviewHosts() }
    }

    private func resolveImport() {
        guard let package=transferPackage,!pending else {return}
        pending=true;project()
        Task { @MainActor in
            var failure:String?
            do {
                let hosts=(package["hosts"] as? [String:Any])?["hosts"] as? [[String:Any]] ?? []
                let response=try await request(["type":"resolve","sources":hosts.compactMap{$0["source"]}])
                guard response.type=="resolved",let associations=response.associations else {throw PolarStationNativeWorkspaceError.invalidStructure}
                let name=((package["workspace"] as? [String:Any])?["name"] as? String ?? "Workspace")+" copy"
                let result=try Self.transfer(["action":"import","package":package,"name":name,"associations":try object(associations),"polar_instances":try object(connections?.polarInstances ?? []),"platform":"macos"])
                confirmImport(result)
            } catch {failure=error.localizedDescription}
            pending=false;project();if let failure {message=failure}
        }
    }
    private func confirmImport(_ result: [String: Any]) {
        guard let workspace = result["native"] as? [String: Any] else { return }
        reviewName = workspace["name"] as? String ?? "Workspace copy"
        review = Review(title: "Import Workspace",
                        text: (result["preview"] as? [String: Any])?["summary"] as? String ?? "",
                        confirm: "Import Workspace", apply: { [weak self] in
            guard let self else { return }
            var native = workspace
            native["name"] = reviewName
            do {
                let bytes = try JSONSerialization.data(withJSONObject: native)
                run("import_commit", id: native["id"] as? String ?? "", name: String(decoding: bytes, as: UTF8.self))
            } catch { message = error.localizedDescription }
        })
    }

}

/// Native file/physical-window adapter; shared Rust owns inspection and switching.
@MainActor final class StationBackendWindow: ObservableObject {
    struct Row: Decodable { let label:String; let value:String }
    struct ViewState: Decodable {
        let title:String; let description:String; let can_switch:Bool
        let inspect_label:String; let switch_label:String; let endpoint_label:String
        let fields:[Row]; let impacts:[Row]; let message:String
    }
    @Published var target = "" {
        didSet { if target != oldValue { preview = nil; project() } }
    }
    @Published private(set) var view:ViewState?
    @Published private(set) var busy = false
    private var current = ""
    private var previous:String?
    private var message = ""
    private var catalog:[String:Any]?
    private var prepared:[String:Any] = [:]
    private var preview:[String:Any]?
    private var waiting = false
    private var window:NSWindow?
    var capture:(() throws -> Data)?
    var save:((Data) async throws -> Void)?
    var suspend:(() async throws -> Void)?
    var restore:(() -> Void)?
    var requestHost:HostTransferReader?
    var requestBackend:((Data) async throws -> Data)?

    func present(current:String) {
        self.current = current
        if target.isEmpty { target = current }
        if window == nil {
            let native = NSWindow(contentRect:NSRect(x:0,y:0,width:960,height:640),styleMask:[.titled,.closable,.resizable],backing:.buffered,defer:false)
            native.title = "Station Backend"; native.minSize = NSSize(width:760,height:460)
            native.isReleasedWhenClosed = false
            native.contentView = NSHostingView(rootView:StationBackendView(model:self).stationTheme())
            native.center(); window = native
        }
        project(); window?.makeKeyAndOrderFront(nil)
    }
    private func object<T:Encodable>(_ value:T) throws -> Any { try JSONSerialization.jsonObject(with:JSONEncoder().encode(value)) }
    private func host(_ value:[String:Any]) async throws -> HostTransferResponseV1 {
        guard let requestHost else { throw PolarStationNativeExecutorError.workspaceUnavailable }
        return try await requestHost(JSONDecoder().decode(HostTransferRequestV1.self,from:JSONSerialization.data(withJSONObject:value)))
    }
    private func backend(_ command:[String:Any]) async throws -> [String:Any] {
        guard let requestBackend else { throw PolarStationNativeExecutorError.workspaceUnavailable }
        var command = command; command["request_id"] = UUID().uuidString.lowercased()
        let data = try await requestBackend(JSONSerialization.data(withJSONObject:command))
        guard let result = try JSONSerialization.jsonObject(with:data) as? [String:Any] else { throw PolarTmuxLocalIPCError.invalidJSON }
        if let error = result["error"] as? String { throw NSError(domain:"Backend",code:1,userInfo:[NSLocalizedDescriptionKey:error]) }
        return result
    }
    func inspectPrevious() { if let previous { target = previous; inspect() } }
    func inspect() {
        guard !busy,let capture else { return }
        busy = true; preview = nil; waiting = false; message = "Reading original Session references…"; project()
        Task {
            do {
                guard var catalog = try JSONSerialization.jsonObject(with:capture()) as? [String:Any] else { throw PolarTmuxLocalIPCError.invalidJSON }
                let initial = try WorkspaceManagerWindow.transfer(["action":"prepare_catalog","catalog":catalog])
                let bindings = initial["bindings"] as? [Any] ?? [], targets = initial["target_ids"] as? [String] ?? []
                if !bindings.isEmpty || !targets.isEmpty {
                    let exported = try await host(["type":"export_content","bindings":bindings,"target_ids":targets])
                    guard let bundle = exported.bundle,let sources = exported.sources else { throw PolarTmuxLocalIPCError.invalidJSON }
                    catalog = try WorkspaceManagerWindow.transfer(["action":"protect_catalog","catalog":catalog,"bundle":object(bundle),"sources":object(sources)])
                }
                self.catalog = catalog
                prepared = try WorkspaceManagerWindow.transfer(["action":"prepare_catalog","catalog":catalog])
                let result = try await backend(["action":"inspect","socket_path":target,"sources":prepared["sources"] ?? []])
                preview = result["preview"] as? [String:Any]
                message = "Review the impact below before switching."
            } catch { message = error.localizedDescription }
            busy = false; project()
        }
    }
    func switchBackend() {
        guard view?.can_switch == true,let catalog,let preview,let save,let suspend else { return }
        busy = true; message = "Saving Workspaces before switching…"; project()
        Task {
            do {
                try await save(JSONSerialization.data(withJSONObject:catalog))
                try await suspend()
                waiting = true
                let result = try await backend(["action":"switch","preview_id":preview["preview_id"] ?? ""])
                guard result["switched"] as? Bool == true else { throw PolarTmuxLocalIPCError.invalidJSON }
                previous = current; current = target
                if waiting { message = "Connecting to selected backend…" }; project()
            } catch { failed(error.localizedDescription) }
        }
    }
    /// Called by the new backend's actual projection, never by a preview reply.
    func connected(socket:String,connections:ConnectionsProjectionV1) {
        guard waiting, socket == target,let capture,let save else { return }
        waiting = false
        Task {
            do {
                guard let catalog = try JSONSerialization.jsonObject(with:capture()) as? [String:Any] else { throw PolarTmuxLocalIPCError.invalidJSON }
                let sources = try WorkspaceManagerWindow.transfer(["action":"prepare_catalog","catalog":catalog])
                let resolved = try await host(["type":"resolve","sources":sources["sources"] ?? []])
                let updated = try WorkspaceManagerWindow.transfer(["action":"resolve_catalog","catalog":catalog,"associations":object(resolved.associations ?? []),"polar_instances":object(connections.polarInstances)])
                try await save(JSONSerialization.data(withJSONObject:updated))
                busy = false; message = "Backend connected. Configure missing Hosts to reconnect unavailable original Sessions."
                restore?(); project()
            } catch { failed(error.localizedDescription) }
        }
    }
    func failed(_ value:String) { waiting = false; busy = false; message = value; restore?(); project() }
    private func project() {
        let input:[String:Any] = ["current":current,"target":target,"preview":preview?["descriptor"] ?? NSNull(),"busy":busy,
            "workspace_count":prepared["workspace_count"] ?? 0,"pane_count":prepared["pane_count"] ?? 0,
            "references":prepared["references"] ?? [],"message":message]
        do {
            let data = try JSONSerialization.data(withJSONObject:input)
            try data.withUnsafeBytes { bytes in
                guard let raw = polar_station_backend_view(bytes.bindMemory(to:UInt8.self).baseAddress,bytes.count) else { throw PolarTmuxLocalIPCError.invalidJSON }
                defer { polar_station_desktop_string_free(raw) }
                view = try JSONDecoder().decode(ViewState.self,from:Data(String(cString:raw).utf8))
            }
        } catch { message = error.localizedDescription }
    }
}
private struct StationBackendView:View {
    @ObservedObject var model:StationBackendWindow
    var body:some View {
        VStack(alignment:.leading,spacing:12) {
            if let view = model.view {
                Text(view.title).font(.title2)
                Text(view.description).foregroundStyle(.secondary)
                HStack {
                    Text(view.endpoint_label)
                    TextField("/path/to/host.sock",text:$model.target).disabled(model.busy)
                    Button(view.inspect_label,action:model.inspect).disabled(model.busy)
                    Button("Previous backend",action:model.inspectPrevious).disabled(model.busy)
                }
                ScrollView {
                    Grid(alignment:.topLeading,horizontalSpacing:20,verticalSpacing:12) {
                        ForEach(Array((view.fields + view.impacts).enumerated()),id:\.offset) { _,row in
                            GridRow {
                                Text(row.label).foregroundStyle(.secondary).frame(width:185,alignment:.leading)
                                Text(row.value).textSelection(.enabled).frame(maxWidth:.infinity,alignment:.leading)
                            }
                        }
                    }.frame(maxWidth:.infinity,alignment:.leading)
                }
                Text(view.message).textSelection(.enabled)
                HStack { Spacer(); Button(view.switch_label,action:model.switchBackend).disabled(!view.can_switch).buttonStyle(.borderedProminent) }
            }
        }.padding(16).frame(minWidth:720,minHeight:420)
    }
}
#endif
