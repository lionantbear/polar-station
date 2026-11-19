//! Settings window over the native catalog. Selection only previews saved data.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const c = @cImport({
    @cInclude("polar_station_desktop_client.h");
});
const store = @import("native_workspace_store.zig");
const model = @import("control_center/model.zig");
const geometry = @import("native_workspace_layout.zig");
const rendering = @import("workspace_rendering.zig");
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
        if(self.open_after_save)|id|self.allocator.free(id);
        self.transfer.deinit();
        if (self.dialog) |d| d.as(gtk.Window).destroy();
        if (self.window) |w| w.destroy();
        self.clearDialog();
        if (self.catalog) |*v| v.deinit();
        if (self.machines) |v| v.deinit();
        if (self.connections_revision) |v| self.allocator.free(v);
        if (self.projection) |v| v.deinit();
        if (self.selected) |s| self.allocator.free(s);
        if (self.current) |s| self.allocator.free(s);
        self.allocator.destroy(self);
    }
    pub fn present(self: *Manager, parent: *gtk.Window, controller_id: u64, current: ?[]const u8) void {
        self.controller_id = controller_id;
        const next = if (current) |id| self.allocator.dupe(u8, id) catch return else null;
        if (self.current) |id| self.allocator.free(id);
        self.current = next;
        if (self.window == null) {
            const w = gtk.Window.new();
            w.setTitle("Workspaces");
            w.setDefaultSize(1000, 640);
            w.as(gtk.Widget).setSizeRequest(760, 460);
            const root = gtk.Box.new(.vertical, 12);
            inline for (.{ "Top", "Bottom", "Start", "End" }) |edge| @field(gtk.Widget, "setMargin" ++ edge)(root.as(gtk.Widget), 16);
            const top = gtk.Box.new(.horizontal, 8);
            const heading = self.label(top, "Workspaces");
            heading.as(gtk.Widget).setHexpand(1);
            heading.as(gtk.Widget).addCssClass("title-2");
            self.page_actions = gtk.Box.new(.horizontal, 6);
            top.append(self.page_actions.?.as(gtk.Widget));
            root.append(top.as(gtk.Widget));
            _ = self.label(root, "Browse saved Tabs and Panes without opening them.");
            const body = gtk.Paned.new(.horizontal);
            body.setPosition(280);
            body.as(gtk.Widget).setVexpand(1);
            const left = gtk.Box.new(.vertical, 8);
            const rows = gtk.ListBox.new();
            rows.setSelectionMode(.single);
            rows.setActivateOnSingleClick(0);
            rows.as(gtk.Widget).addCssClass("navigation-sidebar");
            self.rows = rows;
            const scroll = gtk.ScrolledWindow.new();
            scroll.setPolicy(.never, .automatic);
            scroll.setChild(rows.as(gtk.Widget));
            scroll.as(gtk.Widget).setVexpand(1);
            left.append(scroll.as(gtk.Widget));
            _ = gtk.ListBox.signals.row_selected.connect(rows, *Manager, rowSelected, self, .{});
            body.setStartChild(left.as(gtk.Widget));
            body.setResizeStartChild(0);
            body.setShrinkStartChild(0);
            const right = gtk.Box.new(.vertical, 12);
            right.as(gtk.Widget).setMarginStart(16);
            self.actions = gtk.Box.new(.horizontal, 6);
            right.append(self.actions.?.as(gtk.Widget));
            self.details = gtk.Box.new(.vertical, 12);
            const detail_scroll = gtk.ScrolledWindow.new();
            detail_scroll.setPolicy(.never, .automatic);
            detail_scroll.setChild(self.details.?.as(gtk.Widget));
            detail_scroll.as(gtk.Widget).setVexpand(1);
            right.append(detail_scroll.as(gtk.Widget));
            body.setEndChild(right.as(gtk.Widget));
            body.setShrinkEndChild(0);
            root.append(body.as(gtk.Widget));
            self.status = self.label(root, "");
            w.setChild(root.as(gtk.Widget));
            self.window = w;
            _ = gtk.Window.signals.close_request.connect(w, *Manager, closed, self, .{});
        }
        self.transfer.parent=self.window;
        self.window.?.setTransientFor(parent);
        self.window.?.present();
        self.render(true) catch |err| self.problem(@errorName(err));
    }
    pub fn updateConnections(self: *Manager, connections: ?*const model.Connections) void {
        const w = self.window orelse return;
        if (w.as(gtk.Widget).getVisible() == 0) return;
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
        const w = self.window orelse return;
        if (w.as(gtk.Widget).getVisible() == 0) return;
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
        self.archived_button.?.setLabel(try std.fmt.allocPrintSentinel(a, "{s}  {s}", .{ if (self.expanded) "▾" else "▸", string(v, "archived_label") }, 0));
        self.renderActions(self.page_actions.?, list(field(v, "page_actions"))) catch {};
        self.renderActions(self.actions.?, list(field(v, "actions"))) catch {};
        clear(self.details.?);
        for (catalog.workspaces) |w| {
            if (!eq(w.workspace_id, id)) continue;
            self.label(self.details.?, w.workspace_name).as(gtk.Widget).addCssClass("title-2");
            const detail_heading = gtk.Box.new(.horizontal, 8);
            self.label(detail_heading, string(v, "details_title")).as(gtk.Widget).setHexpand(1);
            _ = self.label(detail_heading, string(v, "preview_caption"));
            self.details.?.append(detail_heading.as(gtk.Widget));
            const facts = if (self.get_projection) |get| get(self.context) else null;
            const described = try @import("native_workspace_view.zig").build(a, w, facts, null, null);
            const cards = gtk.FlowBox.new();
            cards.setSelectionMode(.none);
            cards.setMinChildrenPerLine(1);
            cards.setMaxChildrenPerLine(2);
            cards.setHomogeneous(1);
            cards.setColumnSpacing(10);
            cards.setRowSpacing(10);
            for (w.tabs, 0..) |tab, tab_index| {
                const frame = gtk.Box.new(.vertical, 0);
                frame.as(gtk.Widget).addCssClass("card");
                frame.as(gtk.Widget).setSizeRequest(260, -1);
                const card = gtk.Box.new(.vertical, 6);
                frame.append(card.as(gtk.Widget));
                inline for (.{ "Top", "Bottom", "Start", "End" }) |edge| @field(gtk.Widget, "setMargin" ++ edge)(card.as(gtk.Widget), 10);
                _ = self.label(card, try std.fmt.allocPrint(a, "{s}   ·   {s}   ·   {d} panes", .{ tab.title, @tagName(tab.display_mode), tab.panes.len }));
                try self.appendPreview(card, tab, a);
                for (tab.panes, 0..) |pane, pane_index| {
                    _ = pane;
                    const fact = described.tabs[tab_index].panes[pane_index];
                    const text = try std.fmt.allocPrint(a, "{s}   ·   {s}", .{ fact.session_name, fact.target_label });
                    _ = self.label(card, try std.fmt.allocPrint(a, "{d}  {s}", .{ pane_index + 1, text }));
                }
                if (tab.panes.len == 0) _ = self.label(card, "Empty Tab");
                cards.insert(frame.as(gtk.Widget), -1);
            }
            self.details.?.append(cards.as(gtk.Widget));
            if (list(field(v, "hosts")).len > 0) _ = self.label(self.details.?, string(v, "hosts_title"));
            for (list(field(v, "hosts"))) |host| {
                const box = gtk.Box.new(.vertical, 4);
                _ = self.label(box, try std.fmt.allocPrint(a, "{s}   {s}", .{ string(host, "name"), string(host, "access") }));
                _ = self.label(box, string(host, "status"));
                self.details.?.append(box.as(gtk.Widget));
            }
            break;
        }
        self.problem(if (field(v, "error") == .string) string(v, "error") else if (self.pending) "Saving Workspace…" else "");
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

    fn appendPreview(self: *Manager, box: *gtk.Box, tab: rendering.Tab, a: std.mem.Allocator) !void {
        if (tab.panes.len == 0) return;
        const ids = try a.alloc([]const u8, tab.panes.len);
        for (tab.panes, 0..) |pane, i| ids[i] = pane.id;
        const layout = try geometry.layout(if (tab.visual_pane_ids.len == ids.len) tab.visual_pane_ids else ids, if (tab.display_mode == .focus) .focus else .monitor, tab.focused_pane_id);
        const grid = gtk.Grid.new();
        grid.setColumnHomogeneous(1);
        grid.setRowHomogeneous(1);
        grid.setColumnSpacing(1);
        grid.setRowSpacing(1);
        grid.as(gtk.Widget).setSizeRequest(-1, 120);
        grid.as(gtk.Widget).setHexpand(1);
        var ratio_index: usize = 0;
        try self.previewNode(grid, tab, &layout.tree, layout.tree.root, &ratio_index, 0, 0, 100, 100, a);
        box.append(grid.as(gtk.Widget));
    }
    fn previewNode(self: *Manager, grid: *gtk.Grid, tab: rendering.Tab, tree: *const geometry.Tree, index: u8, ratio_index: *usize, x: c_int, y: c_int, width: c_int, height: c_int, a: std.mem.Allocator) anyerror!void {
        switch (tree.nodes[index]) {
            .surface => |id| {
                var number: usize = 1;
                for (tab.panes, 0..) |pane, i| {
                    if (eq(pane.id, id)) {
                        number = i + 1;
                        break;
                    }
                }
                const overlay = gtk.Overlay.new();
                const tile = gtk.Box.new(.vertical, 0);
                tile.as(gtk.Widget).addCssClass("view");
                overlay.setChild(tile.as(gtk.Widget));
                const pane_label = gtk.Label.new(try std.fmt.allocPrintSentinel(a, "{d}", .{number}, 0));
                overlay.addOverlay(pane_label.as(gtk.Widget));
                overlay.as(gtk.Widget).setOverflow(.hidden);
                grid.attach(overlay.as(gtk.Widget), x, y, @max(width, 1), @max(height, 1));
            },
            .split => |split| {
                const current = ratio_index.*;
                ratio_index.* += 1;
                const ratio: f64 = if (current < tab.split_ratios.len) tab.split_ratios[current] else @as(f64, @floatFromInt(split.ratio.numerator)) / @as(f64, @floatFromInt(split.ratio.denominator));
                const extent = if (split.axis == .horizontal) width else height;
                const first = @max(1, @min(extent - 1, @as(c_int, @intFromFloat(@round(@as(f64, @floatFromInt(extent)) * ratio)))));
                try self.previewNode(grid, tab, tree, split.first, ratio_index, x, y, if (split.axis == .horizontal) first else width, if (split.axis == .vertical) first else height, a);
                try self.previewNode(grid, tab, tree, split.second, ratio_index, if (split.axis == .horizontal) x + first else x, if (split.axis == .vertical) y + first else y, if (split.axis == .horizontal) extent - first else width, if (split.axis == .vertical) extent - first else height, a);
            },
        }
    }

    fn renderActions(self: *Manager, box: *gtk.Box, actions: []const Value) !void {
        clear(box);
        for (actions) |action| {
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
    fn rowSelected(_: *gtk.ListBox, row: ?*gtk.ListBoxRow, self: *Manager) callconv(.c) void {
        if (self.rendering) return;
        const r = row orelse return;
        const id = self.allocator.dupe(u8, std.mem.span(r.as(gtk.Widget).getName())) catch return;
        if (self.selected) |s| self.allocator.free(s);
        self.selected = id;
        self.render(false) catch |err| self.problem(@errorName(err));
    }
    fn toggleArchived(_: *gtk.Button, self: *Manager) callconv(.c) void {
        self.expanded = !self.expanded;
        self.render(true) catch {};
    }
    fn closed(w: *gtk.Window, _: *Manager) callconv(.c) c_int {
        w.as(gtk.Widget).setVisible(0);
        return 1;
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
        const d = adw.MessageDialog.new(self.window, tz, bz);
        self.dialog = d;
        d.addResponse("cancel", "Cancel");
        d.addResponse("apply", if (eq(action, "archive")) "Archive" else if (eq(action, "new")) "Create" else "Save");
        d.setCloseResponse("cancel");
        d.setDefaultResponse("cancel");
        if (!eq(action, "archive")) {
            const entry = gtk.Entry.new();
            self.name_entry = entry;
            var name: []const u8 = "Workspace";
            if (eq(action, "rename")) if (self.catalog) |catalog| {
                for (catalog.value.workspaces) |w| {
                    if (eq(w.workspace_id, self.selected orelse "")) {
                        name = w.workspace_name;
                        break;
                    }
                }
            };
            const nz = self.allocator.dupeZ(u8, name) catch return;
            defer self.allocator.free(nz);
            entry.as(gtk.Editable).setText(nz);
            d.setExtraChild(entry.as(gtk.Widget));
        }
        _ = adw.MessageDialog.signals.response.connect(d, *Manager, responded, self, .{});
        d.as(gtk.Window).present();
    }
    fn responded(dialog: *adw.MessageDialog, response: [*:0]const u8, self: *Manager) callconv(.c) void {
        defer {
            dialog.as(gtk.Window).destroy();
            self.clearDialog();
        }
        if (!eq(std.mem.span(response), "apply")) return;
        const action = self.dialog_action orelse return;
        if (eq(action, "archive")) self.expanded = true;
        self.dispatch(self.context, action, self.dialog_target orelse "", if (self.name_entry) |entry| std.mem.span(entry.as(gtk.Editable).getText()) else "");
    }
};
