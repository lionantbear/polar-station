//! Native form for the Host-owned target registry. Never reads or writes configuration files.
const std = @import("std");
const gtk = @import("gtk");
const c = @cImport({ @cInclude("polar_station_desktop_client.h"); });
const adw = @import("adw");
const auxiliary = @import("auxiliary_window.zig");
const Application = @import("../class/application.zig").Application;
const model = @import("control_center/model.zig");
const intent = @import("control_center/intent.zig");
const codex_wire = @import("codex.zig");
const codex_page_module = @import("codex_connections_page.zig");
const Changed = *const fn (?*anyopaque, intent.ConnectionUpdate) void;
const Row = struct { owner: *Window, polar: bool, index: usize };

pub const Window = struct {
    list_inputs: @import("application_refresh.zig").RenderedInputs = .{},
    engine: ?*c.ConnectionsHandle = null,
    operation_token: ?u64 = null,
    leave_dialog: ?*adw.MessageDialog = null,
    allocator: std.mem.Allocator,
    app: *Application,
    changed: Changed,
    check: ?*const fn (?*anyopaque, []const u8, bool) void = null,
    checking: bool = false,
    check_button: ?*gtk.Button = null,
    codex_send: ?*const fn (?*anyopaque, []const u8) anyerror!void = null,
    codex_context: ?*anyopaque = null,
    codex_owner: ?[]u8 = null,
    codex_request: ?[]u8 = null,
    codex_token: ?u64 = null,
    codex_page: bool = false,
    codex_view: ?*codex_page_module.Page = null,
    normal_content: ?*gtk.Box = null,
    normal_buttons: ?*gtk.Box = null,
    context: ?*anyopaque,
    window: ?*adw.Window = null,
    list: ?*gtk.Box = null,
    search: ?*gtk.Entry = null,
    instances: ?*gtk.Box = null,
    heading: ?*gtk.Label = null,
    connection_tab: ?*gtk.Button = null,
    polar_tab: ?*gtk.Button = null,
    codex_tab: ?*gtk.Button = null,
    more: ?*gtk.MenuButton = null,
    form: ?*gtk.Box = null,
    detail: ?*gtk.Label = null,
    edit_button: ?*gtk.Button = null,
    cancel_button: ?*gtk.Button = null,
    add_polar_button: ?*gtk.Button = null,
    machine_fields: ?*gtk.Box = null,
    polar_fields: ?*gtk.Box = null,
    selected_machine: ?usize = null,
    editing: bool = false,
    polar_page: bool = false,
    body: ?*gtk.Box = null,
    status: ?*gtk.Label = null,
    id: ?*gtk.Entry = null,
    name: ?*gtk.Entry = null,
    destination: ?*gtk.Entry = null,
    route_enabled: ?*gtk.CheckButton = null,
    route_expander: ?*gtk.Expander = null,
    route_fields: [6]?*gtk.Entry = @splat(null),
    route_editor: model.HostDraft = .{},
    machine: ?*gtk.DropDown = null,
    socket: ?*gtk.Entry = null,
    node: ?*gtk.Entry = null,
    save: ?*gtk.Button = null,
    remove: ?*gtk.Button = null,
    latest: ?std.json.Parsed(model.Connections) = null,
    draft: ?std.json.Parsed(model.Connections) = null,
    rows: std.ArrayList(*Row) = .empty,
    polar: bool = false,
    index: ?usize = null,
    pending: bool = false,
    confirming_remove: bool = false,

    pub fn init(allocator: std.mem.Allocator, app: *Application, changed: Changed, context: ?*anyopaque) Window {
        return .{ .engine = c.polar_station_connections_new(), .allocator = allocator, .app = app, .changed = changed, .context = context };
    }
    pub fn deinit(self: *Window) void {
        self.list_inputs.deinit(self.allocator);

        if(self.codex_owner)|v|self.allocator.free(v);
        if(self.codex_request)|v|self.allocator.free(v);
        if (self.leave_dialog) |dialog| dialog.as(gtk.Window).destroy();
        self.route_editor.deinit(self.allocator);
        if (self.window) |w| auxiliary.destroy(w);
        if(self.codex_view)|page|page.deinit();
        c.polar_station_connections_free(self.engine);
        for (self.rows.items) |row| self.allocator.destroy(row);
        self.rows.deinit(self.allocator);
        if (self.latest) |v| v.deinit();
        if (self.draft) |v| v.deinit();
    }
    pub fn update(self: *Window, value: model.Connections) void {
        const next = self.copy(value) catch return;
        if (self.latest) |v| v.deinit();
        self.latest = next;
        self.interact(.{ .type = "snapshot", .facts = value });
        if (self.window != null and self.list_inputs.changed(self.allocator, .{ .machines = value.machines, .statuses = value.machine_statuses, .instances = value.polar_instances })) self.renderList();
    }
    fn copy(self: *Window, value: model.Connections) !std.json.Parsed(model.Connections) {
        const bytes = try std.json.Stringify.valueAlloc(self.allocator, value, .{});
        defer self.allocator.free(bytes);
        return std.json.parseFromSlice(model.Connections, self.allocator, bytes, .{ .allocate = .alloc_always });
    }
    pub fn present(self: *Window, parent: *gtk.Window, value: model.Connections) void {
        self.update(value);
        self.ensureWindow(parent);
        if (!self.pending and !self.editing) { self.renderList(); self.select(false, self.selected_machine); }
        auxiliary.present(self.app, self.window.?);
    }
    pub fn result(self: *Window, error_message: ?[]const u8) void {
        self.pending = false;
        if (self.operation_token) |token| {
            self.operation_token = null;
            self.interact(.{ .type = "complete", .token = token, .@"error" = error_message });
        }
        self.body.?.as(gtk.Widget).setSensitive(1);
        self.save.?.as(gtk.Widget).setSensitive(1);
        self.remove.?.as(gtk.Widget).setSensitive(@intFromBool(self.index != null));
        self.check_button.?.as(gtk.Widget).setSensitive(@intFromBool(self.index != null));
        const checking = self.checking; self.checking = false;
        if (checking) {
            if (error_message) |message| { self.text(self.status.?, message); return; }
            if (self.latest) |latest| { if (latest.value.last_check) |report| { self.text(self.status.?, report.summary); return; } }
            self.text(self.status.?, "Connection check did not return a result. Retry the check."); return;
        }
        if (error_message) |message| { self.text(self.status.?, message); return; }
        self.editing = false;
        if (self.latest) |latest| {
            if (self.draft) |before| {
                if (!self.polar and self.index == null) {
                    for (latest.value.machines, 0..) |machine, i| {
                        var existed = false;
                        for (before.value.machines) |old| { if (std.mem.eql(u8, old.id, machine.id)) existed = true; }
                        if (!existed) self.selected_machine = i;
                    }
                }
            }
            if (self.selected_machine) |i| { if (i >= latest.value.machines.len) self.selected_machine = null; }
        }
        self.renderList(); self.select(false, self.selected_machine);
        self.text(self.status.?, "Saved. Check the connection to verify access.");
    }
    fn ensureWindow(self: *Window, parent: *gtk.Window) void {
        if (self.window != null) return;
        const w = auxiliary.create(self.app); self.window = w;
        w.as(gtk.Window).setTitle("Connections");
        w.as(gtk.Window).setDefaultSize(760, 520);
        w.as(gtk.Window).setHideOnClose(1);
        _ = gtk.Window.signals.close_request.connect(w.as(gtk.Window), *Window, closeRequested, self, .{});
        w.as(gtk.Window).setTransientFor(parent);
        const root = gtk.Box.new(.vertical, 12);
        root.as(gtk.Widget).setMarginTop(18); root.as(gtk.Widget).setMarginBottom(18);
        root.as(gtk.Widget).setMarginStart(18); root.as(gtk.Widget).setMarginEnd(18);
        const body = gtk.Box.new(.horizontal, 18); self.body = body; body.as(gtk.Widget).setVexpand(1); root.append(body.as(gtk.Widget));
        const left = gtk.Box.new(.vertical, 8); body.append(left.as(gtk.Widget));
        self.search = gtk.Entry.new(); self.search.?.setPlaceholderText("Search machines"); left.append(self.search.?.as(gtk.Widget));
        _ = gtk.Editable.signals.changed.connect(self.search.?.as(gtk.Editable), *Window, searchChanged, self, .{});
        const scroll = gtk.ScrolledWindow.new(); scroll.as(gtk.Widget).setSizeRequest(200, 320);
        scroll.as(gtk.Widget).setVexpand(1); self.list = gtk.Box.new(.vertical, 5);
        scroll.setChild(self.list.?.as(gtk.Widget)); left.append(scroll.as(gtk.Widget));
        const add_machine = gtk.Button.newWithLabel("+ Add machine"); left.append(add_machine.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(add_machine, *Window, addMachine, self, .{});
        const right = gtk.Box.new(.vertical, 12); right.as(gtk.Widget).setHexpand(1); body.append(right.as(gtk.Widget));
        self.heading = gtk.Label.new("Local"); self.heading.?.setXalign(0); self.heading.?.as(gtk.Widget).addCssClass("title-2"); right.append(self.heading.?.as(gtk.Widget));
        const tabs = gtk.Box.new(.horizontal, 0); tabs.as(gtk.Widget).addCssClass("linked"); right.append(tabs.as(gtk.Widget));
        const connection_tab = gtk.Button.newWithLabel("Connection"); self.connection_tab = connection_tab; tabs.append(connection_tab.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(connection_tab, *Window, connectionTab, self, .{});
        const polar_tab = gtk.Button.newWithLabel("Polar"); self.polar_tab = polar_tab; tabs.append(polar_tab.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(polar_tab, *Window, polarTab, self, .{});
        const codex = gtk.Button.newWithLabel("Codex"); self.codex_tab=codex; tabs.append(codex.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(codex, *Window, codexClicked, self, .{});
        const content_scroll = gtk.ScrolledWindow.new();
        content_scroll.setPolicy(.never, .automatic);
        content_scroll.as(gtk.Widget).setVexpand(1);
        content_scroll.as(gtk.Widget).setHexpand(1);
        const content = gtk.Box.new(.vertical, 12); self.normal_content = content;
        const pages = gtk.Box.new(.vertical, 0); pages.append(content.as(gtk.Widget));
        self.codex_view = codex_page_module.Page.create(self.allocator,self,codexInput) catch null;
        if(self.codex_view)|page|{ pages.append(page.root.as(gtk.Widget)); page.root.as(gtk.Widget).setVisible(0); }
        content_scroll.setChild(pages.as(gtk.Widget));
        right.append(content_scroll.as(gtk.Widget));
        self.instances = gtk.Box.new(.vertical, 6); content.append(self.instances.?.as(gtk.Widget));
        const add_polar = gtk.Button.newWithLabel("+ Add Polar"); self.add_polar_button = add_polar; content.append(add_polar.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(add_polar, *Window, addPolar, self, .{});
        self.detail = gtk.Label.new("This computer · Local connections do not use SSH.");
        self.detail.?.setXalign(0); self.detail.?.setWrap(1); self.detail.?.setSelectable(1); content.append(self.detail.?.as(gtk.Widget));
        const form = gtk.Box.new(.vertical, 8); self.form = form; content.append(form.as(gtk.Widget));
        self.id = gtk.Entry.new(); self.id.?.as(gtk.Widget).setVisible(0); form.append(self.id.?.as(gtk.Widget));
        self.name = field(form, "Name");
        self.machine_fields = gtk.Box.new(.vertical, 8); form.append(self.machine_fields.?.as(gtk.Widget));
        self.destination = field(self.machine_fields.?, "SSH destination (user@host or SSH alias)");
        const advanced = gtk.Expander.new("Connection paths · LAN / WAN"); self.route_expander = advanced;
        const routes = gtk.Box.new(.vertical, 6); advanced.setChild(routes.as(gtk.Widget)); self.machine_fields.?.append(advanced.as(gtk.Widget));
        self.route_enabled = gtk.CheckButton.newWithLabel("Use explicit LAN/WAN route"); routes.append(self.route_enabled.?.as(gtk.Widget));
        inline for (.{"LAN address", "LAN port", "WAN host", "WAN port", "Primary DNS provider", "Secondary DNS provider"}, 0..) |title, i| self.route_fields[i] = field(routes, title);
        self.polar_fields = gtk.Box.new(.vertical, 8); form.append(self.polar_fields.?.as(gtk.Widget));
        self.machine = gtk.DropDown.new(null, null); self.machine.?.as(gtk.Widget).setVisible(0); self.polar_fields.?.append(self.machine.?.as(gtk.Widget));
        self.socket = field(self.polar_fields.?, "Polar socket path");
        self.node = field(self.polar_fields.?, "Expected Node ID · identity verification");
        self.status = gtk.Label.new("Select a connection or add one.");
        self.status.?.setWrap(1); self.status.?.setXalign(0); right.append(self.status.?.as(gtk.Widget));
        const actions = gtk.Box.new(.horizontal, 8); right.append(actions.as(gtk.Widget));
        const buttons = gtk.Box.new(.horizontal, 8); self.normal_buttons = buttons; buttons.as(gtk.Widget).setHexpand(1); actions.append(buttons.as(gtk.Widget));
        if(self.codex_view)|page|{page.footer.as(gtk.Widget).setHexpand(1);actions.append(page.footer.as(gtk.Widget));page.footer.as(gtk.Widget).setVisible(0);}
        const more = gtk.MenuButton.new(); self.more = more; more.setIconName("view-more-symbolic");
        const popover = gtk.Popover.new(); const menu = gtk.Box.new(.vertical, 6);
        menu.as(gtk.Widget).setMarginTop(8); menu.as(gtk.Widget).setMarginBottom(8);
        menu.as(gtk.Widget).setMarginStart(8); menu.as(gtk.Widget).setMarginEnd(8);
        popover.setChild(menu.as(gtk.Widget)); more.setPopover(popover);
        self.remove = gtk.Button.newWithLabel("Remove…"); self.remove.?.as(gtk.Widget).addCssClass("destructive-action"); menu.append(self.remove.?.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(self.remove.?, *Window, removeClicked, self, .{});
        self.check_button = gtk.Button.newWithLabel("Check"); buttons.append(self.check_button.?.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(self.check_button.?, *Window, checkClicked, self, .{});
        self.edit_button = gtk.Button.newWithLabel("Edit…"); buttons.append(self.edit_button.?.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(self.edit_button.?, *Window, editClicked, self, .{});
        self.cancel_button = gtk.Button.newWithLabel("Cancel");
        _ = gtk.Button.signals.clicked.connect(self.cancel_button.?, *Window, reloadClicked, self, .{});
        buttons.append(more.as(gtk.Widget));
        const spacer = gtk.Box.new(.horizontal, 0); spacer.as(gtk.Widget).setHexpand(1); buttons.append(spacer.as(gtk.Widget));
        buttons.append(self.cancel_button.?.as(gtk.Widget));
        self.save = gtk.Button.newWithLabel("Save"); buttons.append(self.save.?.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(self.save.?, *Window, saveClicked, self, .{});
        const close = gtk.Button.newWithLabel("Close"); actions.append(close.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(close, *Window, closeClicked, self, .{});
        w.setContent(root.as(gtk.Widget));
    }
    pub fn connectCodex(self: *Window, owner: ?[]const u8) void {
        if(self.codex_owner)|old|{ if(owner)|id|{if(std.mem.eql(u8,old,id))return;} self.allocator.free(old); }
        self.codex_owner=if(owner)|id|self.allocator.dupe(u8,id) catch null else null;
    }
    pub fn codexDisconnected(self: *Window) void {
        if(self.codex_request)|id|{self.allocator.free(id);self.codex_request=null;}
        if(self.codex_token)|token|{self.codex_token=null;self.interact(.{.type="complete",.token=token,.@"error"="Host disconnected. Save outcome may be unknown; your draft is retained."});}
        if(self.codex_view)|page|{page.root.as(gtk.Widget).setSensitive(1);page.footer.as(gtk.Widget).setSensitive(1);}
        if(self.codex_page)self.text(self.status.?,"Host disconnected. Reconnect and reload app-server settings.");
    }
    fn codexClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        if(self.editing or self.pending)return;
        self.codex_page=true;self.renderMode();
        if(self.codex_view)|page|page.setMachine(self.selectedMachineId());
        self.requestCodex(.{.kind="get_settings"},null);
    }
    fn requestCodex(self:*Window,command:anytype,token:?u64) void {
        if(self.codex_request!=null)return;
        self.codex_token=token;
        const owner=self.codex_owner orelse {self.codexDisconnected();return;};
        const send=self.codex_send orelse {self.codexDisconnected();return;};
        const id=std.fmt.allocPrint(self.allocator,"connections-{x}",.{std.crypto.random.int(u64)}) catch return;
        self.codex_request=id;
        if(self.codex_view)|page|{page.root.as(gtk.Widget).setSensitive(0);page.footer.as(gtk.Widget).setSensitive(0);}
        const payload=codex_wire.encode(self.allocator,id,owner,command) catch {self.codexDisconnected();return;};defer self.allocator.free(payload);
        self.text(self.status.?,if(token!=null)"Saving…" else "Loading app-server settings…");
        send(self.codex_context,payload) catch {self.codexDisconnected();};
    }
    pub fn requestFailed(self: *Window, id: []const u8, message: []const u8) bool {
        const pending = self.codex_request orelse return false;
        if (!std.mem.eql(u8, pending, id)) return false;
        self.allocator.free(pending);
        self.codex_request = null;
        const token = self.codex_token;
        self.codex_token = null;
        if (token) |value| self.interact(.{ .type = "complete", .token = value, .@"error" = message });
        if (self.codex_view) |page| { page.root.as(gtk.Widget).setSensitive(1); page.footer.as(gtk.Widget).setSensitive(1); }
        if (self.status) |status| self.text(status, message);
        return true;
    }
    pub fn receiveCodex(self:*Window,payload:[]const u8) void {
        const id=self.codex_request orelse return;
        const parsed=codex_wire.decode(self.allocator,payload) catch return;defer parsed.deinit();
        const response=parsed.value.response;
        if(!std.mem.eql(u8,id,response.request_id))return;
        self.allocator.free(id);self.codex_request=null;
        const token=self.codex_token;self.codex_token=null;
        if(self.codex_view)|page|{page.root.as(gtk.Widget).setSensitive(1);page.footer.as(gtk.Widget).setSensitive(1);}
        if(response.result.settings)|settings|{
            self.interact(.{.type="codex_snapshot",.settings=settings,.owner=response.owner_host_id});
            if(token)|t|self.interact(.{.type="complete",.token=t,.@"error"=@as(?[]const u8,null)}) else self.text(self.status.?,"App-server settings loaded.");
        }else{
            const message=response.result.code orelse "App-server settings request failed. Your draft is retained.";
            if(token)|t|self.interact(.{.type="complete",.token=t,.@"error"=message}) else self.text(self.status.?,message);
        }
    }
    fn codexInput(context:?*anyopaque,bytes:[]const u8) void {
        const self:*Window=@ptrCast(@alignCast(context.?));
        const Input=struct{type:[]const u8};
        const parsed=std.json.parseFromSlice(Input,self.allocator,bytes,.{.ignore_unknown_fields=true}) catch return;defer parsed.deinit();
        if(std.mem.eql(u8,parsed.value.type,"confirm_remove_codex")){
            if(self.leave_dialog!=null)return;
            const dialog=adw.MessageDialog.new(self.window.?.as(gtk.Window),"Remove this app-server endpoint?","Active bindings are checked by the Host. Running sessions remain active.");
            dialog.addResponse("cancel","Cancel");dialog.addResponse("remove","Remove");dialog.setResponseAppearance("remove",.destructive);dialog.setDefaultResponse("cancel");dialog.setCloseResponse("cancel");
            _=adw.MessageDialog.signals.response.connect(dialog,*Window,codexRemoveResponse,self,.{});self.leave_dialog=dialog;dialog.as(gtk.Window).present();return;
        }
        self.interactBytes(bytes);
    }
    fn codexRemoveResponse(dialog:*adw.MessageDialog,response:[*:0]const u8,self:*Window) callconv(.c) void {
        self.leave_dialog=null;
        dialog.as(gtk.Window).destroy();
        if(!std.mem.eql(u8,std.mem.span(response),"remove"))return;
        const page=self.codex_view orelse return;
        const id=page.selected orelse return;
        self.interact(.{.type="remove_codex",.machine=page.machine,.id=id});
    }
    fn renderList(self: *Window) void {
        const list = self.list orelse return;
        while (list.as(gtk.Widget).getFirstChild()) |child| list.remove(child);
        for (self.rows.items) |row| self.allocator.destroy(row);
        self.rows.clearRetainingCapacity();
        list.append(gtk.Label.new("Machines").as(gtk.Widget));
        const local = gtk.Button.newWithLabel("Local · This computer"); list.append(local.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(local, *Window, localClicked, self, .{});
        while (self.instances.?.as(gtk.Widget).getFirstChild()) |child| self.instances.?.remove(child);
        const value = (self.latest orelse return).value;
        for (value.machines, 0..) |m, i| {
            const query = get(self.search.?);
            if (query.len == 0 or std.ascii.indexOfIgnoreCase(m.display_name, query) != null or std.ascii.indexOfIgnoreCase(m.destination, query) != null) self.appendRow(m.display_name, false, i);
        }
        const machine_id = if (self.selected_machine) |i| value.machines[i].id else "local";
        for (value.polar_instances, 0..) |p, i| {
            if (std.mem.eql(u8, p.execution_target_id, machine_id)) self.appendRow(p.display_name, true, i);
        }
    }
    fn searchChanged(_: *gtk.Editable, self: *Window) callconv(.c) void { if (!self.editing) self.renderList(); }
    fn appendRow(self: *Window, name: []const u8, polar: bool, index: usize) void {
        const row = self.allocator.create(Row) catch return;
        row.* = .{ .owner = self, .polar = polar, .index = index };
        self.rows.append(self.allocator, row) catch { self.allocator.destroy(row); return; };
        const z = self.allocator.dupeZ(u8, name) catch return; defer self.allocator.free(z);
        const button = gtk.Button.newWithLabel(z);
        if (!polar) {
            const value = (self.latest orelse return).value;
            for (value.machine_statuses) |machine_status| {
                if (!std.mem.eql(u8, machine_status.id, value.machines[index].id)) continue;
                const state = machine_status.connection;
                const box = gtk.Box.new(.vertical, 3);
                const title = gtk.Label.new(z); title.setXalign(0);
                box.append(title.as(gtk.Widget));
                const line = gtk.Box.new(.horizontal, 4);
                const online = std.mem.eql(u8, state.status, "connected");
                const icon = gtk.Image.newFromIconName(if (online) "emblem-ok-symbolic" else "network-offline-symbolic");
                icon.as(gtk.Widget).addCssClass(if (online) "success" else if (std.mem.eql(u8, state.status, "offline")) "error" else "warning");
                line.append(icon.as(gtk.Widget));
                const caption = if (state.route) |route| std.fmt.allocPrintSentinel(self.allocator, "{s} · {s}", .{state.label, route}, 0) catch return else self.allocator.dupeZ(u8, state.label) catch return;
                defer self.allocator.free(caption);
                const label = gtk.Label.new(caption); label.setXalign(0);
                label.as(gtk.Widget).addCssClass("caption");
                line.append(label.as(gtk.Widget)); box.append(line.as(gtk.Widget));
                button.setChild(box.as(gtk.Widget));
                const hint = self.allocator.dupeZ(u8, state.accessibility_label) catch return;
                defer self.allocator.free(hint);
                button.as(gtk.Widget).setTooltipText(hint);
                break;
            }
        }
        if ((!polar and self.selected_machine == index) or (polar and self.polar and self.index == index)) button.as(gtk.Widget).addCssClass("suggested-action");
        (if (polar) self.instances.? else self.list.?).append(button.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(button, *Row, rowClicked, row, .{});
    }
    fn select(self: *Window, polar: bool, index: ?usize) void {
        if (self.pending) return;
        const value = (self.latest orelse return).value;
        const draft = self.copy(value) catch return;
        if (self.draft) |v| v.deinit(); self.draft = draft;
        self.polar = polar; self.index = index; self.confirming_remove = false;
        self.remove.?.setLabel("Remove…");
        self.check_button.?.as(gtk.Widget).setSensitive(@intFromBool(index != null));
        self.remove.?.as(gtk.Widget).setSensitive(@intFromBool(index != null));
        self.id.?.as(gtk.Editable).setEditable(@intFromBool(index == null));
        const choices = gtk.StringList.new(null); defer choices.unref();
        choices.append("Local · This computer");
        for (value.machines) |m| { const z = self.allocator.dupeZ(u8, m.display_name) catch continue; defer self.allocator.free(z); choices.append(z); }
        self.machine.?.setModel(choices.as(@import("gio").ListModel)); self.machine.?.setSelected(if (self.selected_machine) |i| @intCast(i + 1) else 0);
        self.entry(self.id.?, ""); self.entry(self.name.?, ""); self.entry(self.destination.?, ""); self.entry(self.socket.?, ""); self.entry(self.node.?, "");
        self.route_enabled.?.setActive(0); self.route_expander.?.setExpanded(0);
        const defaults = [_][]const u8{"", "22", "", "22888", "google", "cloudflare"};
        for (defaults, 0..) |v, i| self.entry(self.route_fields[i].?, v);
        if (index) |i| {
            if (polar) {
                const p = value.polar_instances[i]; self.entry(self.id.?, p.id); self.entry(self.name.?, p.display_name); self.entry(self.socket.?, p.socket_path); self.entry(self.node.?, p.expected_node_id);
                for (value.machines, 0..) |m, mi| { if (std.mem.eql(u8, m.id, p.execution_target_id)) self.machine.?.setSelected(@intCast(mi + 1)); }
            } else {
                const m = value.machines[i]; self.entry(self.id.?, m.id); self.entry(self.name.?, m.display_name); self.entry(self.destination.?, m.destination);
                if (m.route_profile) |route| {
                    self.route_enabled.?.setActive(1); self.route_expander.?.setExpanded(1);
                    self.entry(self.route_fields[0].?, route.lan_address); self.entry(self.route_fields[2].?, route.wan_host);
                    var buf: [8]u8 = undefined;
                    self.entry(self.route_fields[1].?, std.fmt.bufPrint(&buf, "{d}", .{route.lan_port}) catch "");
                    self.entry(self.route_fields[3].?, std.fmt.bufPrint(&buf, "{d}", .{route.wan_port}) catch "");
                    self.entry(self.route_fields[4].?, if (route.doh_providers.len > 0) route.doh_providers[0] else "");
                    self.entry(self.route_fields[5].?, if (route.doh_providers.len > 1) route.doh_providers[1] else "");
                }
            }
        }
        self.destination.?.as(gtk.Widget).setSensitive(@intFromBool(!polar));
        self.machine.?.as(gtk.Widget).setSensitive(@intFromBool(polar));
        self.socket.?.as(gtk.Widget).setSensitive(@intFromBool(polar)); self.node.?.as(gtk.Widget).setSensitive(@intFromBool(polar));
        self.text(self.status.?, "Not checked");
        const route = if (!polar and index != null) value.machines[index.?].route_profile else null;
        const details = if (route) |r|
            std.fmt.allocPrint(self.allocator, "{s}\nSSH destination: {s}\n\nConnection paths\nLAN: {s}:{d}\nWAN: {s}:{d}", .{get(self.name.?), get(self.destination.?), r.lan_address, r.lan_port, r.wan_host, r.wan_port}) catch null
        else if (index != null)
            std.fmt.allocPrint(self.allocator, "{s}\n{s}", .{ get(self.name.?), if (polar) get(self.socket.?) else get(self.destination.?) }) catch null
        else null;
        defer if (details) |v| self.allocator.free(v);
        self.text(self.detail.?, details orelse if (self.polar_page) "Select a Polar instance, or add one for this machine." else "This computer · Local connections do not use SSH.");
        self.renderMode();
        if (self.editing) self.beginInteraction();
    }
    fn submit(self: *Window, change: intent.ConnectionChange) void {
        if (self.pending) return;
        const draft = self.draft orelse return; self.pending = true;
        self.body.?.as(gtk.Widget).setSensitive(0);
        self.save.?.as(gtk.Widget).setSensitive(0); self.remove.?.as(gtk.Widget).setSensitive(0);
        self.text(self.status.?, "Saving…"); self.changed(self.context, .{ .expected_revision = draft.value.revision, .change = change });
    }
    fn checkClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        const check = self.check orelse return;
        const index = self.index orelse return;
        if (self.pending) return;
        const draft = (self.draft orelse return).value;
        const id = if (self.polar) draft.polar_instances[index].id else draft.machines[index].id;
        self.pending = true; self.checking = true;
        self.body.?.as(gtk.Widget).setSensitive(0);
        self.save.?.as(gtk.Widget).setSensitive(0); self.remove.?.as(gtk.Widget).setSensitive(0);
        self.check_button.?.as(gtk.Widget).setSensitive(0);
        self.text(self.status.?, "Checking saved connection…");
        check(self.context, id, self.polar);
    }
    fn saveClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        const draft = (self.draft orelse return).value;
        if (self.polar) {
            const selected = self.machine.?.getSelected();
            const machine = if (selected == 0) "local" else if (selected <= draft.machines.len) draft.machines[selected - 1].id else return;
            self.saveInteraction(.{ .save_polar = .{ .id = get(self.id.?), .display_name = get(self.name.?), .execution_target_id = machine,
                .socket_path = get(self.socket.?), .expected_node_id = get(self.node.?),
                .expected_peer_uid = if (selected == 0 and self.index != null) draft.polar_instances[self.index.?].expected_peer_uid else null } });
        } else {
            self.route_editor.setRouteEnabled(self.route_enabled.?.getActive() != 0);
            inline for (.{model.RouteField.lan_address, .lan_port, .wan_host, .wan_port, .doh_primary, .doh_secondary}, 0..) |route_field, i| {
                self.route_editor.editRouteField(self.allocator, route_field, get(self.route_fields[i].?)) catch { self.text(self.status.?, "Could not retain route draft."); return; };
            }
            self.saveInteraction(.{ .save_machine = .{ .id = get(self.id.?), .display_name = get(self.name.?), .destination = get(self.destination.?),
            .route_profile = self.route_editor.routeProfile() } });
        }
    }
    fn removeClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        const i = self.index orelse return; const draft = (self.draft orelse return).value;
        if (!self.confirming_remove) { self.confirming_remove = true; self.remove.?.setLabel("Confirm remove"); self.text(self.status.?, "Remove this connection? Active references will be checked; running Sessions are not terminated."); return; }
        self.submit(if (self.polar) .{ .remove_polar = draft.polar_instances[i].id } else .{ .remove_machine = draft.machines[i].id });
    }
    pub fn openCodexFor(self: *Window, target_id: []const u8) void {
        if (self.editing or self.pending) return;
        const latest = self.latest orelse return;
        self.selected_machine = null;
        for (latest.value.machines, 0..) |machine, index| {
            if (std.mem.eql(u8, machine.id, target_id)) { self.selected_machine = index; break; }
        }
        self.renderList();
        codexClicked(self.codex_tab.?, self);
    }
    pub fn selectedMachineId(self: *Window) []const u8 {
        const value = (self.latest orelse return "local").value;
        if (self.selected_machine) |i| { if (i < value.machines.len) return value.machines[i].id; }
        return "local";
    }
    fn renderMode(self: *Window) void {
        self.normal_content.?.as(gtk.Widget).setVisible(@intFromBool(!self.codex_page));
        self.normal_buttons.?.as(gtk.Widget).setVisible(@intFromBool(!self.codex_page));
        if(self.codex_view)|page|{page.root.as(gtk.Widget).setVisible(@intFromBool(self.codex_page));page.footer.as(gtk.Widget).setVisible(@intFromBool(self.codex_page));}
        self.form.?.as(gtk.Widget).setVisible(@intFromBool(self.editing));
        self.detail.?.as(gtk.Widget).setVisible(@intFromBool(!self.editing));
        self.machine_fields.?.as(gtk.Widget).setVisible(@intFromBool(!self.polar));
        self.polar_fields.?.as(gtk.Widget).setVisible(@intFromBool(self.polar));
        self.save.?.as(gtk.Widget).setVisible(@intFromBool(self.editing));
        self.cancel_button.?.as(gtk.Widget).setVisible(@intFromBool(self.editing));
        self.edit_button.?.as(gtk.Widget).setVisible(@intFromBool(!self.editing and self.index != null));
        self.check_button.?.as(gtk.Widget).setVisible(@intFromBool(!self.editing and self.index != null));
        self.more.?.as(gtk.Widget).setVisible(@intFromBool(!self.editing and self.index != null));
        self.instances.?.as(gtk.Widget).setVisible(@intFromBool(self.polar_page and !self.editing));
        self.add_polar_button.?.as(gtk.Widget).setVisible(@intFromBool(self.polar_page and !self.editing));
        self.list.?.as(gtk.Widget).setSensitive(@intFromBool(!self.editing));
        self.search.?.as(gtk.Widget).setSensitive(@intFromBool(!self.editing));
        self.text(self.heading.?, if (self.selected_machine) |i| self.latest.?.value.machines[i].display_name else "Local · This computer");
        self.codex_tab.?.as(gtk.Widget).removeCssClass("suggested-action");
        self.connection_tab.?.as(gtk.Widget).removeCssClass("suggested-action");
        self.polar_tab.?.as(gtk.Widget).removeCssClass("suggested-action");
        (if(self.codex_page)self.codex_tab.? else if (self.polar_page) self.polar_tab.? else self.connection_tab.?).as(gtk.Widget).addCssClass("suggested-action");
    }
    fn rowClicked(_: *gtk.Button, row: *Row) callconv(.c) void {
        const self = row.owner;
        if (self.pending or self.editing) return;
        self.codex_page=false;
        if (!row.polar) { self.selected_machine = row.index; self.polar_page = false; }
        const polar = row.polar; const index = row.index;
        self.select(polar, index);
        self.renderList();
    }
    fn localClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.editing or self.pending) return;
        self.codex_page=false; self.selected_machine = null; self.polar_page = false; self.select(false, null); self.renderList();
    }
    fn connectionTab(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.editing or self.pending) return;
        self.codex_page = false; self.polar_page = false; self.select(false, self.selected_machine);
    }
    fn polarTab(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.editing or self.pending) return;
        self.codex_page = false; self.polar_page = true; self.select(true, null); self.renderList();
    }
    fn editClicked(_: *gtk.Button, self: *Window) callconv(.c) void { self.editing = true; self.beginInteraction(); self.renderMode(); }
    fn addMachine(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.pending or self.editing) return;
        self.codex_page=false; self.editing = true; self.polar_page = false; self.select(false, null);
    }
    fn addPolar(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.pending or self.editing) return;
        self.editing = true; self.select(true, null);
    }
    fn reloadClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.pending) return;
        self.interact(.{ .type = "cancel" });
        if (!self.editing) { self.polar_page = false; self.renderList(); self.select(false, self.selected_machine); }
    }
    fn closeClicked(_: *gtk.Button, self: *Window) callconv(.c) void { self.interact(.{ .type = "close" }); }
    fn closeRequested(_: *gtk.Window, self: *Window) callconv(.c) c_int { self.interact(.{ .type = "close" }); return 1; }
    fn beginInteraction(self: *Window) void {
        const value = (self.draft orelse return).value;
        self.interact(.{ .type = "snapshot", .facts = value });
        if (self.polar) {
            const machine_id = self.selectedMachineId();
            const polar: model.PolarConnection = if (self.index) |index| value.polar_instances[index] else .{ .id = "", .display_name = "", .execution_target_id = machine_id, .socket_path = "", .expected_node_id = "" };
            self.interact(.{ .type = "edit", .change = .{ .type = "save_polar", .polar = polar } });
        } else {
            const machine: model.MachineConnection = if (self.index) |index| value.machines[index] else .{ .id = "", .display_name = "", .destination = "" };
            self.interact(.{ .type = "edit", .change = .{ .type = "save_machine", .machine = machine } });
        }
    }
    fn saveInteraction(self: *Window, change: intent.ConnectionChange) void {
        switch (change) {
            .save_machine => |m| self.interact(.{ .type = "draft", .change = .{ .type = "save_machine", .machine = m } }),
            .save_polar => |p| self.interact(.{ .type = "draft", .change = .{ .type = "save_polar", .polar = p } }),
            else => return,
        }
        self.interact(.{ .type = "save" });
    }
    fn interact(self: *Window, input: anytype) void {
        const bytes = std.json.Stringify.valueAlloc(self.allocator, input, .{}) catch return;
        defer self.allocator.free(bytes);
        self.interactBytes(bytes);
    }
    fn interactBytes(self:*Window,bytes:[]const u8) void {
        const engine=self.engine orelse return;
        const response = c.polar_station_connections_dispatch(engine, bytes.ptr, bytes.len) orelse return;
        defer c.polar_station_desktop_string_free(response);
        const Change = struct { type: []const u8, machine: ?model.MachineConnection = null, polar: ?model.PolarConnection = null };
        const Effect = struct { type: []const u8, token: ?u64 = null, expected_revision: std.json.Value = .null, settings: ?codex_wire.Settings = null, change: ?Change = null };
        const View = struct { codex_settings: ?codex_wire.Settings = null, codex_draft: ?codex_wire.Endpoint = null, codex_enabled: bool = false, pending: bool, editing: bool, confirm_discard: bool, message: ?[]const u8 = null, confirmation: []const u8, save_label: []const u8, discard_label: []const u8, keep_label: []const u8 };
        const Output = struct { view: ?View = null, effects: []const Effect = &.{}, @"error": ?[]const u8 = null };
        const parsed = std.json.parseFromSlice(Output, self.allocator, std.mem.span(response), .{ .ignore_unknown_fields = true }) catch return;
        defer parsed.deinit();
        if (parsed.value.@"error") |message| { if (self.status) |status| self.text(status, message); return; }
        const view = parsed.value.view orelse return;
        // Snapshot arrival never mutates native input widgets or their IME selection.
        self.editing = view.editing;
        if(self.codex_page)self.pending=view.pending;
        if(self.codex_view)|page|page.update(view.codex_settings,view.codex_draft,view.codex_enabled,view.pending or self.codex_request!=null);
        if(self.codex_page)self.renderMode();
        if (view.message) |message| { if (self.status) |status| self.text(status, message); }
        if (view.confirm_discard and self.leave_dialog == null and self.window != null) {
            const title = self.allocator.dupeZ(u8, view.confirmation) catch return; defer self.allocator.free(title);
            const dialog = adw.MessageDialog.new(self.window.?.as(gtk.Window), title, null);
            inline for (.{"save", "discard", "keep_editing"}, .{view.save_label, view.discard_label, view.keep_label}) |id, label| {
                const z = self.allocator.dupeZ(u8, label) catch return; defer self.allocator.free(z); dialog.addResponse(id, z);
            }
            dialog.setResponseAppearance("discard", .destructive);
            dialog.setDefaultResponse("keep_editing"); dialog.setCloseResponse("keep_editing");
            _ = adw.MessageDialog.signals.response.connect(dialog, *Window, leaveResponse, self, .{});
            self.leave_dialog = dialog; dialog.as(gtk.Window).present();
        }
        for (parsed.value.effects) |effect| {
            if (std.mem.eql(u8, effect.type, "close")) { if (self.window) |w| w.as(gtk.Widget).setVisible(0); }
            if(std.mem.eql(u8,effect.type,"save_codex")){
                const settings=effect.settings orelse continue;
                if(effect.expected_revision!=.integer)continue;
                self.requestCodex(.{.kind="update_settings",.expected_revision=@as(u64,@intCast(effect.expected_revision.integer)),.settings=settings},effect.token);
            }
            if (std.mem.eql(u8, effect.type, "save")) {
                const change = effect.change orelse continue;
                self.operation_token = effect.token;
                // submit retains the frozen snapshot revision; shared Rust verifies that baseline.
                if (self.draft) |draft| if (!std.mem.eql(u8, draft.value.revision, if(effect.expected_revision==.string)effect.expected_revision.string else "")) return;
                if (change.machine) |m| self.submit(.{ .save_machine = m });
                if (change.polar) |p| self.submit(.{ .save_polar = p });
            }
        }
    }
    fn leaveResponse(dialog: *adw.MessageDialog, response: [*:0]const u8, self: *Window) callconv(.c) void {
        self.leave_dialog = null;
        dialog.as(gtk.Window).destroy();
        const id = std.mem.span(response);
        if (std.mem.eql(u8, id, "save")) { if(self.codex_page)self.interact(.{.type="save"}) else saveClicked(self.save.?, self); }
        else if (std.mem.eql(u8, id, "discard")) {
            self.interact(.{ .type = "discard" });
            if(!self.codex_page){self.renderList(); self.select(false, self.selected_machine);}
        } else self.interact(.{ .type = "keep_editing" });
    }
    fn field(box: *gtk.Box, title: [:0]const u8) *gtk.Entry { box.append(gtk.Label.new(title).as(gtk.Widget)); const e = gtk.Entry.new(); box.append(e.as(gtk.Widget)); return e; }
    fn get(e: *gtk.Entry) []const u8 { return std.mem.span(e.as(gtk.Editable).getText()); }
    fn entry(self: *Window, e: *gtk.Entry, value: []const u8) void { const z = self.allocator.dupeZ(u8, value) catch return; defer self.allocator.free(z); e.as(gtk.Editable).setText(z); }
    fn text(self: *Window, l: *gtk.Label, value: []const u8) void { const z = self.allocator.dupeZ(u8, value) catch return; defer self.allocator.free(z); l.setText(z); }
};
