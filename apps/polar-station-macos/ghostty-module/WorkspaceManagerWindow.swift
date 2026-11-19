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
    private var hostReviewWindow: NSWindow?
    private let hostInteraction=ConnectionsInteraction()
    private var current: String?
    private var machines: [MachineConnectionV1]?
    private(set) var sessions: [SessionRowProjectionV1] = []
    private var pending = false
    private var applying = false
    private var window: NSWindow?
    var dispatch: (@MainActor (String, String, String) async throws -> Void)?

    func present(catalog: PolarStationNativeWorkspaceCatalog?, current: String?, machines: [MachineConnectionV1]?, sessions: [SessionRowProjectionV1]) {
        self.machines = machines
        self.sessions = sessions
        self.current = current
        self.catalog = catalog
        if window == nil {
            let native = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1000, height: 640), styleMask: [.titled,.closable,.miniaturizable,.resizable], backing: .buffered, defer: false)
            native.title = "Workspaces"
            native.minSize = NSSize(width: 760, height: 460)
            native.isReleasedWhenClosed = false
            native.contentView = NSHostingView(rootView: WorkspaceManagerView(manager: self).stationTheme())
            native.center()
            window = native
        }
        project()
        window?.makeKeyAndOrderFront(nil)
    }
    func refresh(_ catalog: PolarStationNativeWorkspaceCatalog?, machines: [MachineConnectionV1]?, sessions: [SessionRowProjectionV1]) {
        guard window?.isVisible == true, self.catalog != catalog || self.machines != machines || self.sessions != sessions else { return }
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
        guard action.enabled, !pending, let window else { return }
        let kind = String(action.id.dropFirst("workspace.manage.".count))
        let id = selected ?? ""
        var name = ""
        if kind == "open" { openWorkspace(id); return }
        if kind == "export" { exportWorkspace(); return }
        if kind == "import" { importWorkspace(); return }
        if let confirmation = action.confirmation {
            let alert = NSAlert()
            alert.messageText = confirmation.title
            alert.informativeText = confirmation.message
            alert.addButton(withTitle: confirmation.confirmLabel)
            alert.addButton(withTitle: "Cancel")
            alert.beginSheetModal(for: window) { [weak self] result in
                guard result == .alertFirstButtonReturn else { return }
                self?.expanded = true
                self?.run(kind,id:id,name:"")
            }
            return
        }
        if kind == "rename" || kind == "new" {
            name = kind == "new" ? "Workspace" : catalog?.workspaces.first(where: { $0.id == id })?.name ?? ""
            let alert = NSAlert()
            alert.messageText = kind == "new" ? "New Workspace" : "Rename Workspace"
            let input = NSTextField(string: name)
            input.frame = NSRect(x:0,y:0,width:320,height:24)
            alert.accessoryView = input
            alert.addButton(withTitle: kind == "new" ? "Create" : "Save")
            alert.addButton(withTitle:"Cancel")
            alert.beginSheetModal(for:window) { [weak self] result in
                if result == .alertFirstButtonReturn { self?.run(kind,id:id,name:input.stringValue) }
            }
            return
        }
        run(kind,id:id,name:name)
    }
    private func run(_ action:String,id:String,name:String) {
        guard let dispatch, !pending else { return }
        pending = true
        if action == "import_commit" { selected = id }
        project()
        Task { @MainActor in
            var failure: String?
            do { try await dispatch(action,id,name) }
            catch { failure = error.localizedDescription }
            pending = false; project()
            if let failure { message = failure }
        }
    }
}

private struct WorkspaceManagerView: View {
    @ObservedObject var manager: WorkspaceManagerWindow
    var body: some View {
        VStack(alignment:.leading,spacing:12) {
            if let view = manager.projection {
                HStack {
                    Text(view.title).font(.title2)
                    Spacer()
                    actions(view.page_actions)
                }
                Text(view.subtitle).foregroundStyle(.secondary)
                HSplitView {
                    VStack(alignment:.leading,spacing:8) {
                        List(selection:$manager.selected) {
                            ForEach(view.rows.filter { !$0.archived }) { row in item(row) }
                            DisclosureGroup(isExpanded:$manager.expanded) {
                                ForEach(view.rows.filter(\.archived)) { row in item(row) }
                            } label: { Text(view.archived_label) }
                        }
                    }.frame(minWidth:220,idealWidth:280,maxWidth:340)
                    VStack(alignment:.leading,spacing:12) {
                        actions(view.actions)
                        ScrollView {
                            if let w = manager.catalog?.workspaces.first(where: { $0.id == manager.selected }) {
                                VStack(alignment:.leading,spacing:12) {
                                    Text(w.name).font(.title2)
                                    HStack {
                                        Text(view.details_title).font(.headline)
                                        Spacer()
                                        Text(view.preview_caption).font(.caption).foregroundStyle(.secondary)
                                    }
                                    LazyVGrid(columns:[GridItem(.adaptive(minimum:260),spacing:12,alignment:.top)],alignment:.leading,spacing:12) {
                                    ForEach(w.tabs,id:\.id) { tab in
                                        VStack(alignment:.leading,spacing:8) {
                                            HStack {
                                                Text(tab.title).font(.headline)
                                                Spacer()
                                                Text("\(tab.mode.rawValue) · \(tab.panes.count) panes").foregroundStyle(.secondary)
                                            }
                                            WorkspaceLayoutPreview(tab:tab)
                                            ForEach(tab.panes,id:\.id) { pane in
                                                HStack {
                                                    Image(systemName:pane.content == nil ? "terminal" : "doc")
                                                    Text(sessionName(pane)).lineLimit(2)
                                                    Spacer()
                                                    Text(view.hosts.first { $0.id == pane.executionTargetID }?.name ?? pane.executionTargetID ?? "").foregroundStyle(.secondary)
                                                }
                                            }
                                            if tab.panes.isEmpty { Text("Empty Tab").foregroundStyle(.secondary) }
                                        }.padding(12).background(.quaternary,in:RoundedRectangle(cornerRadius:8))
                                    }
                                    }
                                    if !view.hosts.isEmpty { Text(view.hosts_title).font(.headline) }
                                    ForEach(view.hosts) { host in
                                        VStack(alignment:.leading,spacing:4) {
                                            Text("\(host.name)   \(host.access)")
                                            Text(host.status).font(.caption).foregroundStyle(.secondary)
                                        }
                                    }
                                }.frame(maxWidth:.infinity,alignment:.leading)
                            }
                        }
                    }.padding(.leading,12).frame(minWidth:440)
                }
            }
            Text(manager.message).foregroundStyle(.secondary).textSelection(.enabled)
        }.padding(16).frame(minWidth:760,minHeight:460)
    }
    private func sessionName(_ pane: PolarStationNativePane) -> String {
        manager.sessions.first { $0.identity.executionTargetID == pane.executionTargetID && $0.identity.tmuxSessionID == pane.tmuxSessionID }?.displayName
            ?? pane.tmuxSessionID ?? pane.content?.kind.rawValue ?? pane.id
    }
    private func item(_ row: WorkspaceManagerWindow.Row) -> some View {
        VStack(alignment:.leading,spacing:4) {
            Text(row.name)
            Text("\(row.summary) · \(row.status)").font(.caption).foregroundStyle(.secondary)
        }.padding(.vertical,4).tag(row.id)
    }
    private func actions(_ values:[ActionProjectionV1]) -> some View {
        HStack(spacing:6) {
            ForEach(values) { action in
                Button(action.label) { manager.invoke(action) }
                    .disabled(!action.enabled).help(action.disabledReason ?? action.label)
            }
        }
    }
}
/// Draw only the existing native topology; ratios use the same preorder as persistence.
private struct WorkspaceLayoutPreview: View {
    let tab: PolarStationNativeTab
    private struct Cell: Identifiable { let id: String; let number: Int; let rect: CGRect }
    private func cells() -> [Cell] {
        let ids = tab.visualPaneIDs.isEmpty ? tab.panes.map(\.id) : tab.visualPaneIDs
        guard let layout = try? PolarStationNativeLayoutFamily.layout(orderedPaneIDs:ids,mode:tab.mode,mainPaneID:tab.mainPaneID) else { return [] }
        func leaves(_ tree:PolarStationNativeLayoutTree) -> [String] {
            switch tree { case .surface(let id): return [id]
            case .split(_,let left,let right): return leaves(left) + leaves(right) }
        }
        func weight(_ tree:PolarStationNativeLayoutTree,axis:PolarStationNativeLayoutTree.Axis) -> Double {
            let ids = Set(leaves(tree))
            if axis == .vertical { return Double(ids.count) }
            let columns = Set(layout.slots.filter { ids.contains($0.surfaceID) }.map(\.column))
            return layout.columns.filter { columns.contains($0.index) }.reduce(0) { $0 + Double($1.width.numerator)/Double($1.width.denominator) }
        }
        var index = 0
        var result: [Cell] = []
        func visit(_ tree:PolarStationNativeLayoutTree,_ rect:CGRect) {
            switch tree {
            case .surface(let id): result.append(Cell(id:id,number:(tab.panes.firstIndex { $0.id == id } ?? 0)+1,rect:rect))
            case .split(let axis,let left,let right):
                let saved = tab.splitRatios ?? []
                let ratio = saved.indices.contains(index) ? saved[index] : weight(left,axis:axis)/weight(tree,axis:axis)
                index += 1
                let horizontal = axis == .horizontal
                let extent = horizontal ? rect.width : rect.height
                let first = extent * ratio
                visit(left,CGRect(x:rect.minX,y:rect.minY,width:horizontal ? first : rect.width,height:horizontal ? rect.height : first))
                visit(right,CGRect(x:horizontal ? rect.minX+first : rect.minX,y:horizontal ? rect.minY : rect.minY+first,width:horizontal ? extent-first : rect.width,height:horizontal ? rect.height : extent-first))
            }
        }
        visit(layout.tree,CGRect(x:0,y:0,width:1,height:1))
        return result
    }
    var body: some View {
        if !tab.panes.isEmpty {
            GeometryReader { geometry in
                ZStack(alignment:.topLeading) {
                    ForEach(cells()) { cell in
                        Text(String(cell.number)).font(.caption).frame(width:max(0,geometry.size.width*cell.rect.width-2),height:max(0,geometry.size.height*cell.rect.height-2))
                            .background(.quaternary,in:RoundedRectangle(cornerRadius:4)).clipped()
                            .offset(x:geometry.size.width*cell.rect.minX,y:geometry.size.height*cell.rect.minY)
                    }
                }
            }.frame(height:120).accessibilityLabel("Saved layout for \(tab.title)")
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
    private func openWorkspace(_ id:String) {
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
                try await dispatch("open",id,"")
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
        guard let window,let package=transferPackage else {return}
        let alert=NSAlert();alert.messageText="Workspace Hosts"
        alert.informativeText="Review and import the included Hosts first. Host changes are saved separately. Continue to review the whole Workspace; missing Hosts will be listed before import."
        alert.addButton(withTitle:"Review Hosts…");alert.addButton(withTitle:"Continue…");alert.addButton(withTitle:"Cancel")
        alert.beginSheetModal(for:window) { [weak self] result in
            guard let self else {return}
            if result == .alertFirstButtonReturn {
                do {
                    let bundle=try JSONDecoder().decode(HostExportBundleV1.self,from:JSONSerialization.data(withJSONObject:package["hosts"] ?? [:]))
                    if let connections {hostInteraction.snapshot(connections)}
                    consumeHosts(hostInteraction.importHosts(bundle))
                } catch {message=error.localizedDescription}
            } else if result == .alertSecondButtonReturn {resolveImport()}
        }
    }
    private func consumeHosts(_ effects:[ConnectionsInteraction.Effect]) {
        for effect in effects {
            guard effect.type=="host_transfer",let request=effect.request,let token=effect.token,let requestHostTransfer else {continue}
            Task { @MainActor in
                let next:[ConnectionsInteraction.Effect]
                do {next=hostInteraction.transferComplete(token,result:try await requestHostTransfer(request),error:nil)}
                catch {next=hostInteraction.transferComplete(token,result:nil,error:error.localizedDescription)}
                if !next.isEmpty { consumeHosts(next); return }
                if hostInteraction.state?.host_import_plan != nil {
                    if hostReviewWindow==nil {
                        let review=NSWindow(contentRect:NSRect(x:0,y:0,width:980,height:620),styleMask:[.titled,.closable,.resizable],backing:.buffered,defer:false)
                        review.title="Import Hosts";review.isReleasedWhenClosed=false
                        review.contentView=NSHostingView(rootView:HostImportReviewView(interaction:hostInteraction,consume:{[weak self] in self?.consumeHosts($0)}).stationTheme());review.center();hostReviewWindow=review
                    }
                    hostReviewWindow?.makeKeyAndOrderFront(nil)
                } else {hostReviewWindow?.orderOut(nil);reviewHosts()}
            }
        }
        if effects.isEmpty,hostInteraction.state?.host_import_plan==nil {hostReviewWindow?.orderOut(nil);reviewHosts()}
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
    private func confirmImport(_ result:[String:Any]) {
        guard let window,var native=result["native"] as? [String:Any] else {return}
        let alert=NSAlert();alert.messageText="Import Workspace"
        alert.informativeText=(result["preview"] as? [String:Any])?["summary"] as? String ?? ""
        let name=NSTextField(string:native["name"] as? String ?? "Workspace copy");name.frame=NSRect(x:0,y:0,width:340,height:24);alert.accessoryView=name
        alert.addButton(withTitle:"Import Workspace");alert.addButton(withTitle:"Cancel")
        alert.beginSheetModal(for:window) { [weak self] response in
            guard let self,response == .alertFirstButtonReturn else {return}
            native["name"]=name.stringValue
            do {
                let bytes=try JSONSerialization.data(withJSONObject:native)
                run("import_commit",id:native["id"] as? String ?? "",name:String(decoding:bytes,as:UTF8.self))
            } catch {message=error.localizedDescription}
        }
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
