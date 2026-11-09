//! Inline native rendering of the window-local Rust Connections draft.
const std = @import("std");
const gtk = @import("gtk");
const gobject = @import("gobject");
const wire = @import("codex.zig");
pub const Dispatch = *const fn (?*anyopaque, []const u8) void;
pub const Page = struct {
    allocator: std.mem.Allocator,
    context: ?*anyopaque,
    dispatch: Dispatch,
    root: *gtk.Box,
    footer: *gtk.Box,
    selector: *gtk.DropDown,
    form: *gtk.Box,
    detail: *gtk.Label,
    fields: [5]*gtk.Entry,
    mode: *gtk.DropDown,
    host: *gtk.DropDown,
    advanced: *gtk.Expander,
    enabled: *gtk.CheckButton,
    add: *gtk.Button,
    edit: *gtk.Button,
    remove: *gtk.Button,
    cancel: *gtk.Button,
    save: *gtk.Button,
    settings: ?std.json.Parsed(wire.Settings) = null,
    machine: []u8,
    selected: ?[]u8 = null,
    ids: std.ArrayList([]const u8) = .empty,
    building: bool = false,
    editing: bool = false,
    pending: bool = false,
    pub fn create(a: std.mem.Allocator, context: ?*anyopaque, dispatch: Dispatch) !*Page {
        const self = try a.create(Page);
        errdefer a.destroy(self);
        const machine = try a.dupe(u8, "local");
        const root = gtk.Box.new(.vertical, 8);
        const selector = gtk.DropDown.new(null, null);
        root.append(selector.as(gtk.Widget));
        const detail = gtk.Label.new("Loading app-server settings…");
        detail.setWrap(1);
        detail.setXalign(0);
        root.append(detail.as(gtk.Widget));
        const form = gtk.Box.new(.vertical, 8);
        root.append(form.as(gtk.Widget));
        const enabled = gtk.CheckButton.newWithLabel("Enable Codex control and monitoring");
        form.append(enabled.as(gtk.Widget));
        var fields: [5]*gtk.Entry = undefined;
        fields[0] = field(form, "Endpoint ID");
        const modes = gtk.StringList.new(null);
        // DropDown.new consumes the model reference (transfer full).
        inline for (.{ "Local app-server", "App-server over SSH", "Remote Station Host" }) |title| modes.append(title);
        const mode = gtk.DropDown.new(modes.as(@import("gio").ListModel), null);
        form.append(mode.as(gtk.Widget));
        fields[1] = field(form, "App-server socket on machine");
        const host = gtk.DropDown.new(null, null);
        form.append(host.as(gtk.Widget));
        const advanced = gtk.Expander.new("Advanced · Remote Host mapping");
        form.append(advanced.as(gtk.Widget));
        const mapping = gtk.Box.new(.vertical, 8);
        advanced.setChild(mapping.as(gtk.Widget));
        fields[2] = field(mapping, "Endpoint ID on remote Host");
        fields[3] = field(mapping, "Target ID on remote Host");
        fields[4] = gtk.Entry.new();
        fields[4].as(gtk.Widget).setVisible(0);
        form.append(fields[4].as(gtk.Widget));
        const footer = gtk.Box.new(.horizontal, 8);
        const add = gtk.Button.newWithLabel("Add endpoint");
        footer.append(add.as(gtk.Widget));
        const edit = gtk.Button.newWithLabel("Edit…");
        footer.append(edit.as(gtk.Widget));
        const remove = gtk.Button.newWithLabel("Remove…");
        footer.append(remove.as(gtk.Widget));
        const spacer = gtk.Box.new(.horizontal, 0);
        spacer.as(gtk.Widget).setHexpand(1);
        footer.append(spacer.as(gtk.Widget));
        const cancel = gtk.Button.newWithLabel("Cancel");
        footer.append(cancel.as(gtk.Widget));
        const save = gtk.Button.newWithLabel("Save");
        footer.append(save.as(gtk.Widget));
        self.* = .{ .allocator = a, .context = context, .dispatch = dispatch, .root = root, .footer = footer, .selector = selector, .form = form, .detail = detail, .fields = fields, .mode = mode, .host = host, .advanced = advanced, .enabled = enabled, .add = add, .edit = edit, .remove = remove, .cancel = cancel, .save = save, .machine = machine };
        _ = gtk.Button.signals.clicked.connect(add, *Page, addClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(edit, *Page, editClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(remove, *Page, removeClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(cancel, *Page, cancelClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(save, *Page, saveClicked, self, .{});
        _ = gobject.Object.signals.notify.connect(selector.as(gobject.Object), *Page, selectionChanged, self, .{ .detail = "selected" });
        _ = gobject.Object.signals.notify.connect(mode.as(gobject.Object), *Page, modeChanged, self, .{ .detail = "selected" });
        _ = gobject.Object.signals.notify.connect(host.as(gobject.Object), *Page, modeChanged, self, .{ .detail = "selected" });
        _ = gtk.CheckButton.signals.toggled.connect(enabled, *Page, toggled, self, .{});
        for (fields) |input| _ = gtk.Editable.signals.changed.connect(input.as(gtk.Editable), *Page, fieldChanged, self, .{});
        self.visibility();
        return self;
    }
    pub fn deinit(self: *Page) void {
        if (self.settings) |s| s.deinit();
        self.ids.deinit(self.allocator);
        if (self.selected) |s| self.allocator.free(s);
        self.allocator.free(self.machine);
        self.allocator.destroy(self);
    }
    pub fn setMachine(self: *Page, machine: []const u8) void {
        if (std.mem.eql(u8, self.machine, machine)) return;
        const copy = self.allocator.dupe(u8, machine) catch return;
        self.allocator.free(self.machine);
        self.machine = copy;
        if (self.selected) |s| self.allocator.free(s);
        self.selected = null;
        self.rebuild();
    }
    pub fn update(self: *Page, settings: ?wire.Settings, draft: ?wire.Endpoint, enabled: bool, pending: bool) void {
        const was_editing = self.editing;
        self.editing = draft != null;
        self.pending = pending;
        if (!was_editing or !self.editing) {
            if (settings) |value| {
                const bytes = std.json.Stringify.valueAlloc(self.allocator, value, .{}) catch return;
                defer self.allocator.free(bytes);
                const copy = std.json.parseFromSlice(wire.Settings, self.allocator, bytes, .{ .allocate = .alloc_always }) catch return;
                // ids borrow the settings snapshot, so always rebuild before another event.
                if (self.settings) |s| s.deinit();
                self.settings = copy;
            }
        }
        if (was_editing and !self.editing) {
            const id = self.allocator.dupe(u8, get(self.fields[0])) catch null;
            if (self.selected) |old| self.allocator.free(old);
            self.selected = id;
        }
        self.building = true;
        if (!was_editing and self.editing) {
            const e = draft.?;
            for ([_][]const u8{ e.id, e.socket_path, e.remote_endpoint_id orelse "", e.remote_target_id orelse "", e.owner_host_id }, 0..) |value, i| self.entry(self.fields[i], value);
            self.fields[0].as(gtk.Editable).setEditable(@intFromBool(e.id.len == 0));
            self.mode.setSelected(if (std.mem.eql(u8, e.control_mode, "remote_host")) 2 else if (std.mem.eql(u8, e.control_mode, "ssh_direct")) 1 else 0);
            self.enabled.setActive(@intFromBool(enabled));
            const hosts = gtk.StringList.new(null);
            defer hosts.unref();
            hosts.append("Select authorized Station Host");
            var selected: u32 = 0;
            if (self.settings) |s| for (s.value.host_connections, 0..) |h, i| {
                const z = self.allocator.dupeZ(u8, h.host_id) catch continue;
                defer self.allocator.free(z);
                hosts.append(z);
                if (std.mem.eql(u8, h.id, e.peer_connection_ref orelse "")) selected = @intCast(i + 1);
            };
            self.host.setModel(hosts.as(@import("gio").ListModel));
            self.host.setSelected(selected);
        }
        self.building = false;
        self.rebuild();
        self.visibility();
    }
    fn rebuild(self: *Page) void {
        self.building = true;
        defer self.building = false;
        self.ids.clearRetainingCapacity();
        const choices = gtk.StringList.new(null);
        defer choices.unref();
        var index: u32 = 0;
        if (self.settings) |s| for (s.value.endpoints) |e| {
            if (!std.mem.eql(u8, e.execution_target_id, self.machine)) continue;
            if (self.selected) |id| {
                if (std.mem.eql(u8, id, e.id)) index = @intCast(self.ids.items.len);
            }
            self.ids.append(self.allocator, e.id) catch continue;
            const z = self.allocator.dupeZ(u8, e.id) catch continue;
            defer self.allocator.free(z);
            choices.append(z);
        };
        self.selector.setModel(choices.as(@import("gio").ListModel));
        self.selector.setSelected(index);
        self.showSelection();
    }
    fn showSelection(self: *Page) void {
        const index = self.selector.getSelected();
        if (self.selected) |id| self.allocator.free(id);
        self.selected = null;
        if (index < self.ids.items.len) self.selected = self.allocator.dupe(u8, self.ids.items[index]) catch null;
        if (self.selected) |id| {
            if (self.settings) |s| for (s.value.endpoints) |e| {
                if (!std.mem.eql(u8, id, e.id)) continue;
                const text = std.fmt.allocPrint(self.allocator, "{s}\nConnection: {s}\nSocket: {s}\nConnection is checked when a session uses this app-server.", .{ e.id, e.control_mode, e.socket_path }) catch return;
                defer self.allocator.free(text);
                const z = self.allocator.dupeZ(u8, text) catch return;
                defer self.allocator.free(z);
                self.detail.setText(z);
                break;
            };
        } else self.detail.setText(if (self.settings == null) "Loading app-server settings…" else "No app-server on this machine. Add an endpoint to configure monitoring.");
        self.visibility();
    }
    fn visibility(self: *Page) void {
        self.root.as(gtk.Widget).setSensitive(@intFromBool(!self.pending));
        self.footer.as(gtk.Widget).setSensitive(@intFromBool(!self.pending));
        self.form.as(gtk.Widget).setVisible(@intFromBool(self.editing));
        self.detail.as(gtk.Widget).setVisible(@intFromBool(!self.editing));
        self.selector.as(gtk.Widget).setSensitive(@intFromBool(!self.editing));
        self.add.as(gtk.Widget).setVisible(@intFromBool(!self.editing));
        self.add.as(gtk.Widget).setSensitive(@intFromBool(self.settings != null));
        self.edit.as(gtk.Widget).setVisible(@intFromBool(!self.editing and self.selected != null));
        self.remove.as(gtk.Widget).setVisible(@intFromBool(!self.editing and self.selected != null));
        self.cancel.as(gtk.Widget).setVisible(@intFromBool(self.editing));
        self.save.as(gtk.Widget).setVisible(@intFromBool(self.editing));
        const remote = self.mode.getSelected() == 2;
        self.fields[1].as(gtk.Widget).setVisible(@intFromBool(!remote));
        if (self.fields[1].as(gtk.Widget).getPrevSibling()) |label| label.setVisible(@intFromBool(!remote));
        self.host.as(gtk.Widget).setVisible(@intFromBool(remote));
        self.advanced.as(gtk.Widget).setVisible(@intFromBool(remote));
    }
    fn send(self: *Page, input: anytype) void {
        const bytes = std.json.Stringify.valueAlloc(self.allocator, input, .{}) catch return;
        defer self.allocator.free(bytes);
        self.dispatch(self.context, bytes);
    }
    fn sendDraft(self: *Page) void {
        if (self.building or !self.editing or self.pending) return;
        const remote = self.mode.getSelected() == 2;
        var peer: ?[]const u8 = null;
        if (self.settings) |s| {
            const i = self.host.getSelected();
            if (i > 0 and i <= s.value.host_connections.len) peer = s.value.host_connections[i - 1].id;
        }
        const endpoint: wire.Endpoint = .{ .id = get(self.fields[0]), .execution_target_id = self.machine, .control_mode = if (remote) "remote_host" else if (self.mode.getSelected() == 1) "ssh_direct" else "local", .socket_path = get(self.fields[1]), .owner_host_id = get(self.fields[4]), .peer_connection_ref = peer, .remote_endpoint_id = get(self.fields[2]), .remote_target_id = get(self.fields[3]) };
        self.send(.{ .type = "draft_codex", .endpoint = endpoint, .enabled = self.enabled.getActive() != 0 });
    }
    fn addClicked(_: *gtk.Button, self: *Page) callconv(.c) void {
        self.send(.{ .type = "edit_codex", .machine = self.machine, .id = @as(?[]const u8, null) });
    }
    fn editClicked(_: *gtk.Button, self: *Page) callconv(.c) void {
        self.send(.{ .type = "edit_codex", .machine = self.machine, .id = self.selected });
    }
    fn removeClicked(_: *gtk.Button, self: *Page) callconv(.c) void {
        self.send(.{ .type = "confirm_remove_codex", .machine = self.machine, .id = self.selected });
    }
    fn cancelClicked(_: *gtk.Button, self: *Page) callconv(.c) void {
        self.send(.{ .type = "cancel" });
    }
    fn saveClicked(_: *gtk.Button, self: *Page) callconv(.c) void {
        self.sendDraft();
        self.send(.{ .type = "save" });
    }
    fn fieldChanged(_: *gtk.Editable, self: *Page) callconv(.c) void {
        self.sendDraft();
    }
    fn toggled(_: *gtk.CheckButton, self: *Page) callconv(.c) void {
        self.sendDraft();
    }
    fn modeChanged(_: *gobject.Object, _: *gobject.ParamSpec, self: *Page) callconv(.c) void {
        self.visibility();
        self.sendDraft();
    }
    fn selectionChanged(_: *gobject.Object, _: *gobject.ParamSpec, self: *Page) callconv(.c) void {
        if (!self.building) self.showSelection();
    }
    fn field(box: *gtk.Box, title: [:0]const u8) *gtk.Entry {
        box.append(gtk.Label.new(title).as(gtk.Widget));
        const e = gtk.Entry.new();
        box.append(e.as(gtk.Widget));
        return e;
    }
    fn get(e: *gtk.Entry) []const u8 {
        return std.mem.span(e.as(gtk.Editable).getText());
    }
    fn entry(self: *Page, e: *gtk.Entry, value: []const u8) void {
        const z = self.allocator.dupeZ(u8, value) catch return;
        defer self.allocator.free(z);
        e.as(gtk.Editable).setText(z);
    }
};
