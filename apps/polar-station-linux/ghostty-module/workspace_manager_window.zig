//! Settings window over the native catalog. Selection only previews saved data.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const glib = @import("glib");
const gobject = @import("gobject");
const c = @cImport({
    @cInclude("polar_station_desktop_client.h");
});
const store = @import("native_workspace_store.zig");
const model = @import("control_center/model.zig");
const layout_view = @import("layout_map_view.zig");
const Value = std.json.Value;
const Dispatch = *const fn (?*anyopaque, []const u8, []const u8, []const u8) void;
fn field(v: Value, key: []const u8) Value {
    return if (v == .object) v.object.get(key) orelse .null else .null;
}
fn string(v: Value, key: []const u8) []const u8 {
    const f = field(v, key);
    return if (f == .string) f.string else "";
}
fn list(v: Value) []const Value {
    return if (v == .array) v.array.items else &.{};
}
fn yes(v: Value, key: []const u8) bool {
    const f = field(v, key);
    return f == .bool and f.bool;
}
fn eq(a: []const u8, b: []const u8) bool {
    return std.mem.eql(u8, a, b);
}

pub const Manager = struct {
    transfer: @import("workspace_transfer_window.zig").Transfer,
    allocator: std.mem.Allocator,
    context: ?*anyopaque,
    dispatch: Dispatch,
    get_projection: ?*const fn (?*anyopaque) ?*const model.Projection = null,
    window: ?*gtk.Window = null,
    root: ?*gtk.Box = null,
    picker: ?*gtk.MenuButton = null,
    live: ?*gtk.FlowBox = null,
    preview_cards: ?*gtk.FlowBox = null,
    layout_scroll: ?*gtk.ScrolledWindow = null,
    layout_width_handler: c_ulong = 0,
    layout_source: ?c_uint = null,
    review_box: ?*gtk.Box = null,
    open_in_new_window: bool = false,
    rows: ?*gtk.ListBox = null,
    details: ?*gtk.Box = null,
    actions: ?*gtk.Box = null,
    page_actions: ?*gtk.Box = null,
    archived_button: ?*gtk.Button = null,
    status: ?*gtk.Label = null,
    catalog: ?store.OwnedCatalog = null,
    machines: ?std.json.Parsed(Value) = null,
    connections_revision: ?[]u8 = null,
    projection: ?std.json.Parsed(Value) = null,
    selected: ?[]u8 = null,
    current: ?[]u8 = null,
    expanded: bool = false,
    pending: bool = false,
    open_after_save:?[]u8=null,
    open_after_revision:u64=0,
    pending_import: ?[]u8 = null,
    import_revision: u64 = 0,
    rendering: bool = false,
    controller_id: u64 = 0,
    dialog: ?*adw.MessageDialog = null,
    dialog_target: ?[]u8 = null,
    dialog_action: ?[]u8 = null,
    name_entry: ?*gtk.Entry = null,

    pub fn create(a: std.mem.Allocator, context: ?*anyopaque, dispatch: Dispatch) !*Manager {
        const self = try a.create(Manager);
        self.* = .{ .allocator = a, .context = context, .dispatch = dispatch, .transfer=.{.allocator=a,.context=context,.dispatch=dispatch} };
        return self;
    }
    pub fn deinit(self: *Manager) void {
        if (self.layout_source) |source| _ = glib.Source.remove(source);
        if (self.layout_scroll) |scroll| {
            gobject.signalHandlerDisconnect(scroll.getHadjustment().as(gobject.Object), self.layout_width_handler);
        }
        if(self.open_after_save)|id|self.allocator.free(id);
        if (self.pending_import) |id| self.allocator.free(id);
        self.transfer.deinit();
        if (self.dialog) |d| d.as(gtk.Window).destroy();
        if (self.root) |root| root.as(@import("gobject").Object).unref();
        self.clearDialog();
        if (self.catalog) |*v| v.deinit();
        if (self.machines) |v| v.deinit();
        if (self.connections_revision) |v| self.allocator.free(v);
        if (self.projection) |v| v.deinit();
        if (self.selected) |s| self.allocator.free(s);
        if (self.current) |s| self.allocator.free(s);
        self.allocator.destroy(self);
    }
    pub fn mount(self: *Manager, parent: *gtk.Window, controller_id: u64, live: *gtk.FlowBox, current: ?[]const u8) !*gtk.Widget {
        self.controller_id = controller_id;
        self.window = parent;
        self.transfer.parent = parent;
        try self.setCurrent(current);
        if (self.root) |root| return root.as(gtk.Widget);
        const root = gtk.Box.new(.vertical, 10);
        const widget = root.as(gtk.Widget);
        _ = widget.as(@import("gobject").Object).refSink();
        widget.setHexpand(1);
        widget.setVexpand(1);
        widget.setHalign(.fill);
        widget.setValign(.fill);
        widget.addCssClass("background");
        inline for (.{ "Top", "Bottom", "Start", "End" }) |edge| @field(gtk.Widget, "setMargin" ++ edge)(widget, 0);
        const content = gtk.Box.new(.vertical, 10);
        inline for (.{ "Top", "Bottom", "Start", "End" }) |edge| @field(gtk.Widget, "setMargin" ++ edge)(content.as(gtk.Widget), 12);
        root.append(content.as(gtk.Widget));
        self.root = root;
        const top = gtk.Box.new(.horizontal, 8);
        const picker = gtk.MenuButton.new();
        self.picker = picker;
        picker.setLabel("Workspace");
        const popup = gtk.Popover.new();
        const rows = gtk.ListBox.new();
        rows.setSelectionMode(.single);
        rows.setActivateOnSingleClick(1);
        self.rows = rows;
        const list_scroll = gtk.ScrolledWindow.new();
        list_scroll.setPolicy(.never, .automatic);
        list_scroll.setMinContentWidth(210);
        list_scroll.setMaxContentHeight(340);
        list_scroll.setPropagateNaturalHeight(1);
        list_scroll.setChild(rows.as(gtk.Widget));
        popup.setChild(list_scroll.as(gtk.Widget));
        picker.setPopover(popup);
        _ = gtk.ListBox.signals.row_activated.connect(rows, *Manager, rowActivated, self, .{});
        _ = gtk.Popover.signals.closed.connect(popup, *Manager, pickerClosed, self, .{});
        top.append(picker.as(gtk.Widget));
        self.page_actions = gtk.Box.new(.horizontal, 6);
        top.append(self.page_actions.?.as(gtk.Widget));
        content.append(top.as(gtk.Widget));
        self.actions = gtk.Box.new(.horizontal, 6);
        content.append(self.actions.?.as(gtk.Widget));
        self.review_box = gtk.Box.new(.vertical, 8);
        content.append(self.review_box.?.as(gtk.Widget));
        const scroll = gtk.ScrolledWindow.new();
        scroll.setPolicy(.external, .automatic);
        scroll.as(gtk.Widget).setVexpand(1);
        const body = gtk.Box.new(.vertical, 8);
        self.details = gtk.Box.new(.vertical, 10);
        body.append(self.details.?.as(gtk.Widget));
        body.append(live.as(gtk.Widget));
        self.live = live;
        const preview_cards = layout_view.newExpandedCards();
        preview_cards.as(gtk.Widget).setVisible(0);
        body.append(preview_cards.as(gtk.Widget));
        self.preview_cards = preview_cards;
        scroll.setChild(body.as(gtk.Widget));
        self.layout_scroll = scroll;
        self.layout_width_handler = gobject.Object.signals.notify.connect(
            scroll.getHadjustment().as(gobject.Object),
            *Manager,
            expandedWidthChanged,
            self,
            .{ .detail = "page-size" },
        );
        content.append(scroll.as(gtk.Widget));
        self.status = self.label(content, "");
        self.transfer.review_box = self.review_box;
        self.transfer.status = self.status;
        return widget;
    }

    pub fn queueExpandedCardLayout(self: *Manager) void {
        if (self.layout_source == null) self.layout_source = glib.idleAdd(updateExpandedCardLayout, self);
    }

    fn expandedWidthChanged(_: *gobject.Object, _: *gobject.ParamSpec, self: *Manager) callconv(.c) void {
        // The viewport reports page size during allocation. Reflow afterward.
        self.queueExpandedCardLayout();
    }

    fn updateExpandedCardLayout(data: ?*anyopaque) callconv(.c) c_int {
        const self: *Manager = @ptrCast(@alignCast(data orelse return 0));
        self.layout_source = null;
        const scroll = self.layout_scroll orelse return 0;
        const cards = (if (self.isPreview()) self.preview_cards else self.live) orelse return 0;
        const width: c_int = @intFromFloat(scroll.getHadjustment().getPageSize());
        layout_view.applyExpandedCardColumns(cards, width);
        return 0;
    }

    pub fn setCurrent(self: *Manager, current: ?[]const u8) !void {
        if (eq(current orelse "", self.current orelse "")) return;
        const next = if (current) |id| try self.allocator.dupe(u8, id) else null;
        if (self.current) |id| self.allocator.free(id);
        self.current = next;
        try self.select(current);
    }

    pub fn select(self: *Manager, workspace: ?[]const u8) !void {
        const next = if (workspace) |id| try self.allocator.dupe(u8, id) else null;
        if (self.selected) |id| self.allocator.free(id);
        self.selected = next;
        if (self.root != null) try self.render(true);
    }

    pub fn isPreview(self: *const Manager) bool {
        return !eq(self.current orelse "", self.selected orelse "");
    }

    pub fn updateConnections(self: *Manager, connections: ?*const model.Connections) void {
        if (self.root == null) return;
        if (connections) |facts| {
            if (self.connections_revision) |revision| if (eq(revision, facts.revision)) return;
            const polar_bytes=std.json.Stringify.valueAlloc(self.allocator,facts.polar_instances,.{}) catch return;defer self.allocator.free(polar_bytes);
            const polar=std.json.parseFromSlice(Value,self.allocator,polar_bytes,.{.allocate=.alloc_always}) catch return;
            if(self.transfer.polar_instances)|old|old.deinit();self.transfer.polar_instances=polar;
            const bytes = std.json.Stringify.valueAlloc(self.allocator, facts.machines, .{}) catch return;
            defer self.allocator.free(bytes);
            const next = std.json.parseFromSlice(Value, self.allocator, bytes, .{ .allocate = .alloc_always }) catch return;
            const revision = self.allocator.dupe(u8, facts.revision) catch {
                next.deinit();
                return;
            };
            if (self.machines) |old| old.deinit();
            if (self.connections_revision) |old| self.allocator.free(old);
            self.machines = next;
            self.connections_revision = revision;
        } else {
            if (self.machines == null) return;
            self.machines.?.deinit();
            self.machines = null;
            if (self.connections_revision) |old| self.allocator.free(old);
            self.connections_revision = null;
        }
        self.render(false) catch {};
    }

    pub fn refresh(self: *Manager, catalog: store.Catalog, pending: bool) void {
        if (self.root == null) return;
        if (self.catalog) |v| if (v.value.catalog_revision == catalog.catalog_revision and self.pending == pending) return;
        const next = store.OwnedCatalog.clone(self.allocator, catalog) catch |err| {
            self.problem(@errorName(err));
            return;
        };
        if (self.catalog) |*v| v.deinit();
        self.catalog = next;
        self.pending = pending;
        if(!pending and catalog.catalog_revision==self.open_after_revision) if(self.open_after_save)|id| {
            self.open_after_save=null;defer self.allocator.free(id);self.dispatch(self.context,"open_resolved",id,"");
        };
        self.render(true) catch |err| self.problem(@errorName(err));
    }
    pub fn importReceipt(self: *Manager, bytes: []const u8) void {
        const imported = self.pending_import orelse return;
        const receipt = std.json.parseFromSlice(struct { revision: u64, @"error": ?[]const u8 = null }, self.allocator, bytes, .{ .ignore_unknown_fields = true }) catch return;
        defer receipt.deinit();
        if (receipt.value.revision != self.import_revision) return;
        self.pending_import = null;
        if (receipt.value.@"error") |message| {
            self.allocator.free(imported);
            self.problem(message);
            return;
        }
        if (self.selected) |old| self.allocator.free(old);
        self.selected = imported;
        self.problem("Workspace imported");
    }

    pub fn problem(self: *Manager, text: []const u8) void {
        const z = self.allocator.dupeZ(u8, text) catch return;
        defer self.allocator.free(z);
        if (self.status) |s| s.setText(z);
    }
    fn label(self: *Manager, box: *gtk.Box, text: []const u8) *gtk.Label {
        const z = self.allocator.dupeZ(u8, text) catch return gtk.Label.new("");
        defer self.allocator.free(z);
        const l = gtk.Label.new(z);
        l.setXalign(0);
        l.setWrap(1);
        l.setWrapMode(.word_char);
        box.append(l.as(gtk.Widget));
        return l;
    }
    fn clear(box: *gtk.Box) void {
        while (box.as(gtk.Widget).getFirstChild()) |child| box.remove(child);
    }
    fn render(self: *Manager, rebuild_rows: bool) !void {
        const catalog = if (self.catalog) |v| v.value else return;
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const a = arena.allocator();
        const Item = struct { id: []const u8, name: []const u8, archived: bool, open: bool, tab_count: usize, pane_count: usize };
        const items = try a.alloc(Item, catalog.workspaces.len);
        for (catalog.workspaces, 0..) |w, i| {
            var pane_count: usize = 0;
            for (w.tabs) |tab| pane_count += tab.panes.len;
            var open = false;
            for (catalog.open_workspace_ids) |id| {
                if (eq(id, w.workspace_id)) {
                    open = true;
                    break;
                }
            }
            items[i] = .{ .id = w.workspace_id, .name = w.workspace_name, .archived = catalog.isArchived(w.workspace_id), .open = open, .tab_count = w.tabs.len, .pane_count = pane_count };
        }
        var required_hosts: std.ArrayListUnmanaged([]const u8) = .empty;
        for (catalog.workspaces) |w| if (eq(w.workspace_id, self.selected orelse self.current orelse catalog.selected_workspace_id)) {
            for (w.tabs) |tab| for (tab.panes) |pane| {
                const host = if (pane.execution_target_id.len > 0) pane.execution_target_id else if (pane.content) |content| content.execution_target_id else "";
                if (host.len > 0) try required_hosts.append(a, host);
            };
        };
        const bytes = try std.json.Stringify.valueAlloc(a, .{ .workspaces = items, .selected = self.selected, .current = self.current, .archived_expanded = self.expanded, .pending = self.pending, .transfer_available = if(self.get_projection) |get| if(get(self.context)) |p| p.connections.workspace_transfer_supported else false else false, .required_hosts = required_hosts.items, .machines = if (self.machines) |m| m.value else Value.null }, .{});
        const result = c.polar_station_workspace_manager_view(bytes.ptr, bytes.len) orelse return error.WorkspaceProjectionFailed;
        defer c.polar_station_desktop_string_free(result);
        const next = try std.json.parseFromSlice(Value, self.allocator, std.mem.span(result), .{ .allocate = .alloc_always });
        if (self.projection) |v| v.deinit();
        self.projection = next;
        const v = next.value;
        const id = string(v, "selected_id");
        // The committed catalog can lag a newly imported selection while saving.
        // Preserve that intent until the writer returns its receipt.
        if (!self.pending) {
            const owned = try self.allocator.dupe(u8, id);
            if (self.selected) |s| self.allocator.free(s);
            self.selected = owned;
        }
        self.rendering = true;
        defer self.rendering = false;
        if (rebuild_rows) {
            self.window.?.setFocus(null);
            while (self.rows.?.as(gtk.Widget).getFirstChild()) |child| self.rows.?.remove(child);
            var archived_heading = false;
            for (list(field(v, "rows"))) |row| {
                if (yes(row, "archived") and !archived_heading) {
                    archived_heading = true;
                    try self.appendArchivedHeading(string(v, "archived_label"));
                }
                const r = gtk.ListBoxRow.new();
                r.as(gtk.Widget).setName(try a.dupeZ(u8, string(row, "id")));
                const box = gtk.Box.new(.vertical, 4);
                box.as(gtk.Widget).setMarginTop(8);
                box.as(gtk.Widget).setMarginBottom(8);
                _ = self.label(box, string(row, "name"));
                _ = self.label(box, try std.fmt.allocPrint(a, "{s} · {s}", .{ string(row, "summary"), string(row, "status") }));
                r.setChild(box.as(gtk.Widget));
                self.rows.?.append(r.as(gtk.Widget));
                if (yes(row, "selected")) self.rows.?.selectRow(r);
            }
            if (!archived_heading) try self.appendArchivedHeading(string(v, "archived_label"));
        }
        self.archived_button.?.setLabel(try std.fmt.allocPrintSentinel(a, "{s}  {s}", .{ if (self.expanded) "−" else "+", string(v, "archived_label") }, 0));
        self.renderActions(self.page_actions.?, list(field(v, "page_actions"))) catch {};
        self.renderActions(self.actions.?, list(field(v, "actions"))) catch {};
        clear(self.details.?);
        const live = self.live.?;
        const preview_cards = self.preview_cards.?;
        const preview = self.isPreview();
        live.as(gtk.Widget).setVisible(@intFromBool(!preview));
        preview_cards.as(gtk.Widget).setVisible(@intFromBool(preview));
        while (preview_cards.as(gtk.Widget).getFirstChild()) |child| preview_cards.remove(child);
        for (catalog.workspaces) |w| {
            if (!eq(w.workspace_id, id)) continue;
            self.picker.?.setLabel(try a.dupeZ(u8, w.workspace_name));
            if (!preview) break;
            _ = self.label(self.details.?, "Workspace preview");
            const facts = if (self.get_projection) |get| get(self.context) else null;
            const map = try @import("native_workspace_view.zig").build(a, w, facts, null, null);
            for (map.tabs, 0..) |tab, index| {
                var bindings: [9][]const u8 = undefined;
                if (tab.panes.len > bindings.len) continue;
                for (tab.panes, 0..) |pane, pane_index| {
                    bindings[pane_index] = try std.fmt.allocPrint(a, "{s} · {s}", .{ pane.target_label, pane.tmux_session_id });
                }
                const card = try layout_view.makeTabCard(a, &tab, index, map.tabs.len, 344, bindings[0..tab.panes.len]);
                card.root.as(gtk.Widget).setSensitive(0);
                preview_cards.insert(card.root.as(gtk.Widget), -1);
            }
            break;
        }
        self.queueExpandedCardLayout();
        if (field(v, "error") == .string) self.problem(string(v, "error"));
    }
    fn appendArchivedHeading(self: *Manager, title: []const u8) !void {
        const heading = gtk.ListBoxRow.new();
        heading.setSelectable(0);
        heading.setActivatable(0);
        const z = try self.allocator.dupeZ(u8, title);
        defer self.allocator.free(z);
        const button = gtk.Button.newWithLabel(z);
        self.archived_button = button;
        button.as(gtk.Widget).setMarginTop(8);
        heading.setChild(button.as(gtk.Widget));
        self.rows.?.append(heading.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(button, *Manager, toggleArchived, self, .{});
    }

    fn renderActions(self: *Manager, box: *gtk.Box, actions: []const Value) !void {
        clear(box);
        for (actions) |action| {
            const action_id = string(action, "id");
            if (std.mem.endsWith(u8, action_id, ".new")) continue;
            if (self.isPreview() and std.mem.endsWith(u8, action_id, ".rename")) continue;
            if (std.mem.endsWith(u8, action_id, ".open")) {
                if (self.isPreview()) {
                    const opened = eq(string(action, "label"), "Focus existing window");
                    self.appendAction(box, if (opened) "Go to Existing Window" else "Switch Here", "workspace.manage.open", !self.pending);
                    if (!opened) self.appendAction(box, "Open in New Window", "workspace.manage.open_new", !self.pending);
                }
                continue;
            }
            const title = try self.allocator.dupeZ(u8, string(action, "label"));
            defer self.allocator.free(title);
            const id = try self.allocator.dupeZ(u8, string(action, "id"));
            defer self.allocator.free(id);
            const b = gtk.Button.newWithLabel(title);
            b.as(gtk.Widget).setName(id);
            b.as(gtk.Widget).setSensitive(@intFromBool(yes(action, "enabled")));
            const reason = try self.allocator.dupeZ(u8, string(action, "disabled_reason"));
            defer self.allocator.free(reason);
            if (reason.len > 0) b.as(gtk.Widget).setTooltipText(reason);
            box.append(b.as(gtk.Widget));
            _ = gtk.Button.signals.clicked.connect(b, *Manager, clicked, self, .{});
        }
    }
    fn appendAction(self: *Manager, box: *gtk.Box, title: [:0]const u8, id: [:0]const u8, enabled: bool) void {
        const button = gtk.Button.newWithLabel(title);
        button.as(gtk.Widget).setName(id);
        button.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        box.append(button.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(button, *Manager, clicked, self, .{});
    }
    fn rowActivated(_: *gtk.ListBox, row: *gtk.ListBoxRow, self: *Manager) callconv(.c) void {
        if (self.rendering or self.pending) return;
        const id = self.allocator.dupe(u8, std.mem.span(row.as(gtk.Widget).getName())) catch return;
        defer self.allocator.free(id);
        self.picker.?.popdown();
        self.select(id) catch |err| self.problem(@errorName(err));
    }
    fn pickerClosed(_: *gtk.Popover, self: *Manager) callconv(.c) void {
        if (self.expanded) {
            self.expanded = false;
            self.render(true) catch {};
        }
    }
    fn toggleArchived(_: *gtk.Button, self: *Manager) callconv(.c) void {
        self.expanded = !self.expanded;
        self.render(true) catch {};
    }
    fn clicked(button: *gtk.Button, self: *Manager) callconv(.c) void {
        const id = std.mem.span(button.as(gtk.Widget).getName());
        if (!std.mem.startsWith(u8, id, "workspace.manage.")) return;
        const action = id["workspace.manage.".len..];
        if (eq(action, "archive") or eq(action, "rename") or eq(action, "new")) {
            self.showDialog(action);
            return;
        }
        self.dispatch(self.context, action, self.selected orelse "", "");
    }
    fn clearDialog(self: *Manager) void {
        if (self.dialog_target) |v| self.allocator.free(v);
        if (self.dialog_action) |v| self.allocator.free(v);
        self.dialog_target = null;
        self.dialog_action = null;
        self.dialog = null;
        self.name_entry = null;
    }
    fn showDialog(self: *Manager, action: []const u8) void {
        if (self.dialog != null) return;
        self.dialog_target = self.allocator.dupe(u8, self.selected orelse "") catch return;
        self.dialog_action = self.allocator.dupe(u8, action) catch {
            self.clearDialog();
            return;
        };
        var title: []const u8 = if (eq(action, "new")) "New Workspace" else "Rename Workspace";
        var body: []const u8 = "";
        if (eq(action, "archive")) if (self.projection) |v| {
            for (list(field(v.value, "actions"))) |a| if (eq(string(a, "id"), "workspace.manage.archive")) {
                title = string(field(a, "confirmation"), "title");
                body = string(field(a, "confirmation"), "message");
                break;
            };
        };
        const tz = self.allocator.dupeZ(u8, title) catch {
            self.clearDialog();
            return;
        };
        defer self.allocator.free(tz);
        const bz = self.allocator.dupeZ(u8, body) catch {
            self.clearDialog();
            return;
        };
        defer self.allocator.free(bz);
        const review_box = self.review_box.?;
        clear(review_box);
        _ = self.label(review_box, tz);
        if (bz.len > 0) _ = self.label(review_box, bz);
        if (eq(action, "rename")) {
            const entry = gtk.Entry.new();
            self.name_entry = entry;
            for (self.catalog.?.value.workspaces) |workspace| {
                if (!eq(workspace.workspace_id, self.selected orelse "")) continue;
                const name = self.allocator.dupeZ(u8, workspace.workspace_name) catch break;
                defer self.allocator.free(name);
                entry.as(gtk.Editable).setText(name);
                break;
            }
            review_box.append(entry.as(gtk.Widget));
        }
        const ok = gtk.Button.newWithLabel(if (eq(action, "archive")) "Archive" else "Save");
        const cancel = gtk.Button.newWithLabel("Cancel");
        review_box.append(ok.as(gtk.Widget));
        review_box.append(cancel.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(ok, *Manager, confirmAction, self, .{});
        _ = gtk.Button.signals.clicked.connect(cancel, *Manager, cancelAction, self, .{});
    }

    fn confirmAction(_: *gtk.Button, self: *Manager) callconv(.c) void {
        const action = self.dialog_action orelse return;
        self.dispatch(self.context, action, self.dialog_target orelse "", if (self.name_entry) |entry| std.mem.span(entry.as(gtk.Editable).getText()) else "");
        cancelAction(undefined, self);
    }
    fn cancelAction(_: *gtk.Button, self: *Manager) callconv(.c) void {
        clear(self.review_box.?);
        self.clearDialog();
    }

};
