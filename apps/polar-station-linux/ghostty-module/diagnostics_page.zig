//! Native presentation only. Readiness, copy and actions come from the Rust projection.
//! All methods are called on the GTK thread. The owner detaches widget() before deinit().
const std = @import("std");
const gtk = @import("gtk");
const gobject = @import("gobject");

pub const Kind = enum { local, remote };
pub const ActionFn = *const fn (?*anyopaque, []const u8, []const u8, []const u8) void;
pub const ConnectFn = *const fn (?*anyopaque, []const u8) void;
pub const RefreshFn = *const fn (?*anyopaque) void;
const RowView = struct {
    id: []const u8,
    title: []const u8,
    state: []const u8,
    detail: []const u8,
    action: ?std.json.Value = null,
    action_label: ?[]const u8 = null,
};
const GroupView = struct { id: []const u8, name: []const u8, rows: []const RowView };
const CapabilitySections = std.json.ArrayHashMap([]const u8);
const Projection = struct {
    local_title: []const u8,
    remote_title: []const u8,
    refresh_label: []const u8,
    empty_remote_label: []const u8,
    local: []const RowView,
    remote_setup: []const RowView,
    remote: []const GroupView,
    capability_sections: CapabilitySections = .{},
};

pub const Page = struct {
    allocator: std.mem.Allocator,
    kind: Kind,
    context: ?*anyopaque,
    on_action: ActionFn,
    on_refresh: RefreshFn,
    on_connect: ?ConnectFn = null,
    ssh_aliases: *gtk.ComboBoxText,
    managed_hosts: *gtk.ComboBoxText,
    ssh_destination: *gtk.Entry,
    ssh_connect: *gtk.Button,
    remote_choices_hash: ?u64 = null,
    on_editor_close: ?RefreshFn = null,
    on_return_to_editor: ?RefreshFn = null,
    root: *gtk.ScrolledWindow,
    body: *gtk.Box,
    title: *gtk.Label,
    refresh: *gtk.Button,
    return_to_editor: *gtk.Button,
    refresh_signal: c_ulong,
    empty: *gtk.Label,
    message: *gtk.Label,
    groups: std.ArrayListUnmanaged(*Group) = .empty,
    editing: bool = false,

    pub fn create(allocator: std.mem.Allocator, kind: Kind, context: ?*anyopaque, on_action: ActionFn, on_refresh: RefreshFn) !*Page {
        const self = try allocator.create(Page);
        const root = gtk.ScrolledWindow.new();
        _ = root.as(gobject.Object).refSink();
        root.setPolicy(.never, .automatic);
        root.as(gtk.Widget).setHexpand(1);
        root.as(gtk.Widget).setVexpand(1);
        const body = gtk.Box.new(.vertical, 16);
        margins(body.as(gtk.Widget), 20);
        const header = gtk.Box.new(.horizontal, 12);
        const title = label("title-2");
        title.as(gtk.Widget).setHexpand(1);
        const refresh = gtk.Button.new();
        refresh.as(gtk.Widget).setValign(.start);
        header.append(title.as(gtk.Widget));
        header.append(refresh.as(gtk.Widget));
        body.append(header.as(gtk.Widget));
        const return_to_editor = gtk.Button.newWithLabel("Back to Add Session");
        return_to_editor.as(gtk.Widget).setHalign(.start);
        return_to_editor.as(gtk.Widget).setVisible(0);
        body.append(return_to_editor.as(gtk.Widget));
        const aliases = gtk.ComboBoxText.new();
        aliases.as(gtk.Widget).setVisible(0);
        body.append(aliases.as(gtk.Widget));
        const manual = gtk.Box.new(.vertical, 6);
        const destination = gtk.Entry.new();
        destination.setPlaceholderText("SSH destination");
        const connect = gtk.Button.newWithLabel("Connect");
        manual.append(destination.as(gtk.Widget));
        manual.append(connect.as(gtk.Widget));
        manual.as(gtk.Widget).setVisible(@intFromBool(kind == .remote));
        body.append(manual.as(gtk.Widget));
        const empty = label("dim-label");
        empty.as(gtk.Widget).setVisible(0);
        body.append(empty.as(gtk.Widget));
        const hosts = gtk.ComboBoxText.new();
        hosts.as(gtk.Widget).setVisible(0);
        body.append(hosts.as(gtk.Widget));
        const message = label("dim-label");
        body.append(message.as(gtk.Widget));
        root.setChild(body.as(gtk.Widget));
        self.* = .{ .allocator = allocator, .kind = kind, .context = context, .on_action = on_action, .on_refresh = on_refresh, .root = root, .body = body, .title = title, .refresh = refresh, .return_to_editor = return_to_editor, .refresh_signal = 0, .empty = empty, .message = message, .ssh_aliases = aliases, .managed_hosts = hosts, .ssh_destination = destination, .ssh_connect = connect };
        _ = gtk.Button.signals.clicked.connect(return_to_editor, *Page, returnToEditor, self, .{});
        self.refresh_signal = gtk.Button.signals.clicked.connect(refresh, *Page, refreshClicked, self, .{});
        _ = gtk.ComboBox.signals.changed.connect(aliases.as(gtk.ComboBox), *Page, remoteChanged, self, .{});
        _ = gtk.ComboBox.signals.changed.connect(hosts.as(gtk.ComboBox), *Page, remoteChanged, self, .{});
        _ = gtk.Button.signals.clicked.connect(connect, *Page, sshConnectClicked, self, .{});
        return self;
    }

    pub fn setMessage(self: *Page, message: ?[]const u8) void {
        setLabel(self.allocator, self.message, message orelse "") catch {};
        self.message.as(gtk.Widget).setVisible(@intFromBool(message != null));
    }

    pub fn widget(self: *Page) *gtk.Widget {
        return self.root.as(gtk.Widget);
    }

    fn returnToEditor(_: *gtk.Button, self: *Page) callconv(.c) void {
        self.return_to_editor.as(gtk.Widget).setVisible(0);
        if (self.on_return_to_editor) |callback| callback(self.context);
    }

    pub fn reveal(self: *Page, group_id: []const u8, section: []const u8) void {
        if (self.kind == .remote and group_id.len != 0) {
            const target = self.allocator.dupeSentinel(u8, group_id, 0) catch return;
            defer self.allocator.free(target);
            _ = self.managed_hosts.as(gtk.ComboBox).setActiveId(target);
        }
        for (self.groups.items) |group| {
            if (!std.mem.eql(u8, group.id, group_id)) continue;
            for (group.rows.items) |row| {
                if (!std.mem.eql(u8, row.id, section)) continue;
                row.title.as(gtk.Widget).setFocusable(1);
                _ = row.title.as(gtk.Widget).grabFocus();
                if (std.mem.eql(u8, section, "voice_input_device") or std.mem.eql(u8, section, "voice_account")) {
                    Row.clicked(row.button, row);
                }
                return;
            }
        }
    }

    pub fn deinit(self: *Page) void {
        gobject.signalHandlerDisconnect(self.refresh.as(gobject.Object), self.refresh_signal);
        for (self.groups.items) |group| group.deinit(self);
        self.groups.deinit(self.allocator);
        self.root.as(gobject.Object).unref();
        self.allocator.destroy(self);
    }

    pub fn update(self: *Page, projection_json: []const u8, busy: bool) !void {
        const parsed = try std.json.parseFromSlice(Projection, self.allocator, projection_json, .{ .ignore_unknown_fields = true });
        defer parsed.deinit();
        const view = parsed.value;
        try setLabel(self.allocator, self.title, if (self.kind == .local) view.local_title else view.remote_title);
        const refresh_text = try self.allocator.dupeSentinel(u8, view.refresh_label, 0);
        defer self.allocator.free(refresh_text);
        self.refresh.setLabel(refresh_text);
        self.refresh.as(gtk.Widget).setSensitive(@intFromBool(!busy));
        self.ssh_connect.as(gtk.Widget).setSensitive(@intFromBool(!busy and !self.editing));
        try setLabel(self.allocator, self.empty, view.empty_remote_label);
        self.empty.as(gtk.Widget).setVisible(@intFromBool(self.kind == .remote and view.remote.len == 0));
        for (self.groups.items) |group| group.seen = false;
        var previous: ?*gtk.Widget = self.empty.as(gtk.Widget);
        if (self.kind == .local) {
            const group = try self.groupFor("");
            try group.update(self, "", view.local, view.capability_sections, busy or self.editing);
            self.body.reorderChildAfter(group.root.as(gtk.Widget), previous);
        } else {
            const setup = try self.groupFor("remote_setup");
            try setup.update(self, "", view.remote_setup, .{}, busy or self.editing);
            self.body.reorderChildAfter(setup.root.as(gtk.Widget), previous);
            self.body.reorderChildAfter(self.managed_hosts.as(gtk.Widget), setup.root.as(gtk.Widget));
            previous = self.managed_hosts.as(gtk.Widget);
            for (view.remote) |host| {
                const group = try self.groupFor(host.id);
                try group.update(self, host.name, host.rows, view.capability_sections, busy or self.editing);
                self.body.reorderChildAfter(group.root.as(gtk.Widget), previous);
                previous = group.root.as(gtk.Widget);
            }
        }
        var index: usize = 0;
        while (index < self.groups.items.len) {
            const group = self.groups.items[index];
            if (group.seen) {
                index += 1;
                continue;
            }
            // A transiently absent target must not destroy its unsaved editor.
            if (group.hasEditor()) {
                for (group.rows.items) |row| row.disableAction(self);
                index += 1;
            } else {
                _ = self.groups.orderedRemove(index);
                group.deinit(self);
            }
        }
        if (self.kind == .remote) try self.updateRemoteChoices(view);
    }

    fn updateRemoteChoices(self: *Page, view: Projection) !void {
        var hash = std.hash.Wyhash.init(0);
        for (view.remote_setup) |row| {
            if (std.mem.startsWith(u8, row.id, "ssh:")) {
                hash.update(row.id);
                hash.update(row.title);
            }
        }
        hash.update(&.{0});
        for (view.remote) |host| {
            hash.update(host.id);
            hash.update(host.name);
        }
        const signature = hash.final();
        if (self.remote_choices_hash == signature) {
            self.applyRemoteSelection();
            return;
        }
        const previous_alias = try self.activeChoice(self.ssh_aliases);
        defer self.allocator.free(previous_alias);
        const previous_host = try self.activeChoice(self.managed_hosts);
        defer self.allocator.free(previous_host);
        self.ssh_aliases.removeAll();
        self.managed_hosts.removeAll();
        var alias_count: usize = 0;
        for (view.remote_setup) |row| {
            if (!std.mem.startsWith(u8, row.id, "ssh:")) continue;
            try self.appendChoice(self.ssh_aliases, row.id, row.title);
            alias_count += 1;
        }
        for (view.remote) |host| try self.appendChoice(self.managed_hosts, host.id, host.name);
        for ([_]*gtk.ComboBoxText{ self.ssh_aliases, self.managed_hosts }, [_][:0]u8{ previous_alias, previous_host }) |combo, previous| {
            if (combo.as(gtk.ComboBox).setActiveId(previous) == 0) combo.as(gtk.ComboBox).setActive(0);
            combo.as(gtk.Widget).setSensitive(@intFromBool(!self.editing));
        }
        self.ssh_aliases.as(gtk.Widget).setVisible(@intFromBool(alias_count > 0));
        self.managed_hosts.as(gtk.Widget).setVisible(@intFromBool(view.remote.len > 0));
        self.remote_choices_hash = signature;
        self.applyRemoteSelection();
    }

    fn activeChoice(self: *Page, combo: *gtk.ComboBoxText) ![:0]u8 {
        const active = combo.as(gtk.ComboBox).getActiveId();
        return self.allocator.dupeSentinel(u8, std.mem.span(active) orelse "", 0);
    }

    fn appendChoice(self: *Page, combo: *gtk.ComboBoxText, id: []const u8, title: []const u8) !void {
        const id_z = try self.allocator.dupeSentinel(u8, id, 0);
        defer self.allocator.free(id_z);
        const title_z = try self.allocator.dupeSentinel(u8, title, 0);
        defer self.allocator.free(title_z);
        combo.append(id_z, title_z);
    }

    fn applyRemoteSelection(self: *Page) void {
        const alias = self.ssh_aliases.as(gtk.ComboBox).getActiveId();
        const host = self.managed_hosts.as(gtk.ComboBox).getActiveId();
        for (self.groups.items) |group| {
            if (std.mem.eql(u8, group.id, "remote_setup")) {
                for (group.rows.items) |row| if (std.mem.startsWith(u8, row.id, "ssh:")) {
                    row.root.as(gtk.Widget).setVisible(@intFromBool(std.mem.eql(u8, std.mem.span(alias) orelse "", row.id)));
                };
            } else {
                const selected = std.mem.eql(u8, std.mem.span(host) orelse "", group.id);
                group.root.as(gtk.Widget).setVisible(@intFromBool(selected or group.hasEditor()));
            }
        }
    }

    fn remoteChanged(_: *gtk.ComboBox, self: *Page) callconv(.c) void {
        self.applyRemoteSelection();
    }

    fn sshConnectClicked(_: *gtk.Button, self: *Page) callconv(.c) void {
        const connect = self.on_connect orelse return;
        const value = std.mem.trim(u8, std.mem.span(self.ssh_destination.as(gtk.Editable).getText()), " \t\n\r");
        const row_id = std.fmt.allocPrintSentinel(self.allocator, "ssh:{s}", .{value}, 0) catch return;
        defer self.allocator.free(row_id);
        connect(self.context, value);
        _ = self.ssh_aliases.as(gtk.ComboBox).setActiveId(row_id);
    }

    /// Local group_id is empty; remote group_id is the shared projection's host id.
    /// The caller owns editor semantics; this slot merely parents an existing widget.
    pub fn setEditor(self: *Page, group_id: []const u8, row_id: []const u8, editor: *gtk.Widget, interactive: bool) !void {
        for (self.groups.items) |group| {
            if (!std.mem.eql(u8, group.id, group_id)) continue;
            for (group.rows.items) |row| {
                if (!std.mem.eql(u8, row.id, row_id)) continue;
                if (row.editor == editor) return;
                if (editor.getParent() != null) return error.EditorAlreadyParented;
                if (row.editor) |old| row.editor_box.remove(old);
                row.editor = editor;
                row.editor_box.append(editor);
                row.editor_box.as(gtk.Widget).setVisible(1);
                row.editor_close.as(gtk.Widget).setVisible(@intFromBool(interactive));
                self.setEditing(interactive);
                return;
            }
        }
        return error.DiagnosticRowNotFound;
    }

    fn setEditing(self: *Page, editing: bool) void {
        self.editing = editing;
        self.ssh_aliases.as(gtk.Widget).setSensitive(@intFromBool(!editing));
        self.managed_hosts.as(gtk.Widget).setSensitive(@intFromBool(!editing));
        self.ssh_connect.as(gtk.Widget).setSensitive(@intFromBool(!editing));
        for (self.groups.items) |group| for (group.rows.items) |row| {
            row.button.as(gtk.Widget).setSensitive(@intFromBool(!editing and row.action != null));
        };
    }
    pub fn detachEditor(self: *Page, child: *gtk.Widget) void {
        self.setEditing(false);
        for (self.groups.items) |group| for (group.rows.items) |row| {
            if (row.editor == child) {
                row.editor_box.remove(child); row.editor = null;
                row.editor_box.as(gtk.Widget).setVisible(0); row.editor_close.as(gtk.Widget).setVisible(0);
            }
        };
    }

    fn groupFor(self: *Page, id: []const u8) !*Group {
        for (self.groups.items) |group| if (std.mem.eql(u8, group.id, id)) return group;
        try self.groups.ensureUnusedCapacity(self.allocator, 1);
        const group = try Group.create(self, id);
        self.groups.appendAssumeCapacity(group);
        self.body.append(group.root.as(gtk.Widget));
        return group;
    }

    fn refreshClicked(_: *gtk.Button, self: *Page) callconv(.c) void {
        self.on_refresh(self.context);
    }
};

const CapabilityCard = struct {
    title: []u8,
    root: *gtk.Box,
    rows: *gtk.Box,
    last_row: ?*gtk.Widget = null,
};

const Group = struct {
    id: []u8,
    root: *gtk.Box,
    title: *gtk.Label,
    rows_box: *gtk.Box,
    rows: std.ArrayListUnmanaged(*Row) = .empty,
    cards: std.ArrayListUnmanaged(CapabilityCard) = .empty,
    seen: bool = false,

    fn create(page: *Page, id: []const u8) !*Group {
        const owned = try page.allocator.dupe(u8, id);
        errdefer page.allocator.free(owned);
        const self = try page.allocator.create(Group);
        const root = gtk.Box.new(.vertical, 8);
        _ = root.as(gobject.Object).refSink();
        const title = label("heading");
        const rows_box = gtk.Box.new(.vertical, 8);
        root.append(title.as(gtk.Widget));
        root.append(rows_box.as(gtk.Widget));
        self.* = .{ .id = owned, .root = root, .title = title, .rows_box = rows_box };
        return self;
    }

    fn deinit(self: *Group, page: *Page) void {
        for (self.rows.items) |row| row.deinit(page);
        self.rows.deinit(page.allocator);
        for (self.cards.items) |card| page.allocator.free(card.title);
        self.cards.deinit(page.allocator);
        page.body.remove(self.root.as(gtk.Widget));
        self.root.as(gobject.Object).unref();
        page.allocator.free(self.id);
        page.allocator.destroy(self);
    }

    fn hasEditor(self: *Group) bool {
        for (self.rows.items) |row| if (row.editor != null) return true;
        return false;
    }

    fn findOrCreateCard(self: *Group, page: *Page, title: []const u8) !*CapabilityCard {
        for (self.cards.items) |*card| if (std.mem.eql(u8, card.title, title)) return card;
        try self.cards.ensureUnusedCapacity(page.allocator, 1);
        const owned_title = try page.allocator.dupe(u8, title);
        errdefer page.allocator.free(owned_title);
        const root = gtk.Box.new(.vertical, 4);
        root.as(gtk.Widget).addCssClass("card");
        const body = gtk.Box.new(.vertical, 4);
        margins(body.as(gtk.Widget), 12);
        const heading = label("heading");
        try setLabel(page.allocator, heading, title);
        body.append(heading.as(gtk.Widget));
        const rows = gtk.Box.new(.vertical, 8);
        body.append(rows.as(gtk.Widget));
        root.append(body.as(gtk.Widget));
        self.rows_box.append(root.as(gtk.Widget));
        self.cards.appendAssumeCapacity(.{ .title = owned_title, .root = root, .rows = rows });
        return &self.cards.items[self.cards.items.len - 1];
    }

    fn update(self: *Group, page: *Page, name: []const u8, views: []const RowView, sections: CapabilitySections, busy: bool) !void {
        self.seen = true;
        try setLabel(page.allocator, self.title, name);
        self.title.as(gtk.Widget).setVisible(@intFromBool(name.len != 0));
        for (self.rows.items) |row| row.seen = false;
        for (self.cards.items) |*card| card.last_row = null;
        var previous: ?*gtk.Widget = null;
        for (views) |view| {
            const optional_card = if (sections.map.get(view.id)) |title| try self.findOrCreateCard(page, title) else null;
            const parent = if (optional_card) |card| card.rows else self.rows_box;
            const existing: ?*Row = for (self.rows.items) |row| {
                if (std.mem.eql(u8, row.id, view.id)) break row;
            } else null;
            const row = existing orelse blk: {
                try self.rows.ensureUnusedCapacity(page.allocator, 1);
                const created = try Row.create(page, self.id, view.id, parent, optional_card == null);
                self.rows.appendAssumeCapacity(created);
                parent.append(created.root.as(gtk.Widget));
                break :blk created;
            };
            try row.update(page, view, busy);
            const widget = row.root.as(gtk.Widget);
            if (optional_card) |card| {
                if (card.last_row == null) {
                    self.rows_box.reorderChildAfter(card.root.as(gtk.Widget), previous);
                    previous = card.root.as(gtk.Widget);
                }
                card.rows.reorderChildAfter(widget, card.last_row);
                card.last_row = widget;
            } else {
                self.rows_box.reorderChildAfter(widget, previous);
                previous = widget;
            }
        }
        var index: usize = 0;
        while (index < self.rows.items.len) {
            const row = self.rows.items[index];
            if (row.seen) {
                index += 1;
                continue;
            }
            if (row.editor != null) {
                row.disableAction(page);
                index += 1;
            } else {
                _ = self.rows.orderedRemove(index);
                row.deinit(page);
            }
        }
        for (self.cards.items) |card| {
            card.root.as(gtk.Widget).setVisible(@intFromBool(card.rows.as(gtk.Widget).getFirstChild() != null));
        }
    }
};

const Row = struct {
    page: *Page,
    group_id: []const u8,
    id: []u8,
    root: *gtk.Box,
    parent: *gtk.Box,
    title: *gtk.Label,
    detail: *gtk.Label,
    icon: *gtk.Image,
    button: *gtk.Button,
    signal: c_ulong,
    action: ?[]u8 = null,
    editor_box: *gtk.Box,
    editor_close: *gtk.Button,
    editor: ?*gtk.Widget = null,
    seen: bool = false,

    fn create(page: *Page, group_id: []const u8, id: []const u8, parent: *gtk.Box, standalone: bool) !*Row {
        const owned = try page.allocator.dupe(u8, id);
        errdefer page.allocator.free(owned);
        const self = try page.allocator.create(Row);
        const root = gtk.Box.new(.vertical, 8);
        _ = root.as(gobject.Object).refSink();
        if (standalone) root.as(gtk.Widget).addCssClass("card");
        const content = gtk.Box.new(.vertical, 8);
        if (standalone) margins(content.as(gtk.Widget), 12);
        root.append(content.as(gtk.Widget));
        const heading = gtk.Box.new(.horizontal, 8);
        const icon = gtk.Image.newFromIconName("dialog-question-symbolic");
        icon.as(gtk.Widget).setValign(.start);
        const title = label("heading");
        title.as(gtk.Widget).setHexpand(1);
        heading.append(icon.as(gtk.Widget));
        heading.append(title.as(gtk.Widget));
        content.append(heading.as(gtk.Widget));
        const detail = label("dim-label");
        detail.setSelectable(1);
        content.append(detail.as(gtk.Widget));
        const button = gtk.Button.new();
        button.as(gtk.Widget).setHalign(.start);
        content.append(button.as(gtk.Widget));
        const editor_box = gtk.Box.new(.vertical, 8);
        editor_box.as(gtk.Widget).setVisible(0);
        content.append(editor_box.as(gtk.Widget));
        const editor_close = gtk.Button.newWithLabel("Close details");
        editor_close.as(gtk.Widget).setVisible(0);
        content.append(editor_close.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(editor_close, *Row, closeEditor, self, .{});
        self.* = .{ .page = page, .group_id = group_id, .id = owned, .root = root, .parent = parent, .title = title, .detail = detail, .icon = icon, .button = button, .signal = 0, .editor_box = editor_box, .editor_close = editor_close };
        self.signal = gtk.Button.signals.clicked.connect(button, *Row, clicked, self, .{});
        return self;
    }

    fn deinit(self: *Row, page: *Page) void {
        gobject.signalHandlerDisconnect(self.button.as(gobject.Object), self.signal);
        self.parent.remove(self.root.as(gtk.Widget));
        self.root.as(gobject.Object).unref();
        if (self.action) |action| page.allocator.free(action);
        page.allocator.free(self.id);
        page.allocator.destroy(self);
    }

    fn disableAction(self: *Row, page: *Page) void {
        self.button.as(gtk.Widget).setSensitive(0);
        if (self.action) |action| page.allocator.free(action);
        self.action = null;
    }

    fn update(self: *Row, page: *Page, view: RowView, busy: bool) !void {
        const action = if (view.action) |value| try std.json.Stringify.valueAlloc(page.allocator, value, .{}) else null;
        errdefer if (action) |bytes| page.allocator.free(bytes);
        try setLabel(page.allocator, self.title, view.title);
        try setLabel(page.allocator, self.detail, view.detail);
        const button_text = try page.allocator.dupeSentinel(u8, view.action_label orelse "", 0);
        defer page.allocator.free(button_text);
        self.button.setLabel(button_text);
        self.icon.setFromIconName(stateIcon(view.state));
        for ([_][*:0]const u8{ "success", "warning", "error", "dim-label" }) |css| self.icon.as(gtk.Widget).removeCssClass(css);
        self.icon.as(gtk.Widget).addCssClass(if (std.mem.eql(u8, view.state, "ready")) "success" else if (std.mem.eql(u8, view.state, "missing") or std.mem.eql(u8, view.state, "needs_attention")) "warning" else "dim-label");
        self.button.as(gtk.Widget).setVisible(@intFromBool(action != null and button_text.len > 0));
        self.button.as(gtk.Widget).setSensitive(@intFromBool(!busy and action != null));
        if (self.action) |old| page.allocator.free(old);
        self.action = action;
        self.seen = true;
    }

    fn closeEditor(_: *gtk.Button, self: *Row) callconv(.c) void {
        if (self.page.on_editor_close) |close| close(self.page.context);
    }
    fn clicked(_: *gtk.Button, self: *Row) callconv(.c) void {
        // The callback can replace this projection or close the entire page.
        const allocator = self.page.allocator;
        const action = allocator.dupe(u8, self.action orelse return) catch return;
        defer allocator.free(action);
        const callback = self.page.on_action;
        const context = self.page.context;
        callback(context, self.group_id, self.id, action);
    }
};

fn stateIcon(state: []const u8) [*:0]const u8 {
    if (std.mem.eql(u8, state, "ready")) return "emblem-ok-symbolic";
    if (std.mem.eql(u8, state, "busy")) return "content-loading-symbolic";
    if (std.mem.eql(u8, state, "offline")) return "network-offline-symbolic";
    if (std.mem.eql(u8, state, "missing") or std.mem.eql(u8, state, "needs_attention")) return "dialog-warning-symbolic";
    if (std.mem.eql(u8, state, "not_enabled")) return "list-add-symbolic";
    return "dialog-question-symbolic";
}
fn label(css: [*:0]const u8) *gtk.Label {
    const result = gtk.Label.new("");
    result.setXalign(0);
    result.setWrap(1);
    result.setWrapMode(.word_char);
    result.as(gtk.Widget).addCssClass(css);
    return result;
}
fn setLabel(allocator: std.mem.Allocator, target: *gtk.Label, value: []const u8) !void {
    const text = try allocator.dupeSentinel(u8, value, 0);
    defer allocator.free(text);
    target.setLabel(text);
}
fn margins(widget: *gtk.Widget, value: c_int) void {
    widget.setMarginTop(value);
    widget.setMarginBottom(value);
    widget.setMarginStart(value);
    widget.setMarginEnd(value);
}
