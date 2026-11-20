//! Native presentation only. Readiness, copy and actions come from the Rust projection.
//! All methods are called on the GTK thread. The owner detaches widget() before deinit().
const std = @import("std");
const gtk = @import("gtk");
const gobject = @import("gobject");

pub const Kind = enum { local, remote };
pub const ActionFn = *const fn (?*anyopaque, []const u8, []const u8, []const u8) void;
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
const Projection = struct {
    local_title: []const u8,
    remote_title: []const u8,
    refresh_label: []const u8,
    empty_remote_label: []const u8,
    local: []const RowView,
    remote_setup: []const RowView,
    remote: []const GroupView,
};

pub const Page = struct {
    allocator: std.mem.Allocator,
    kind: Kind,
    context: ?*anyopaque,
    on_action: ActionFn,
    on_refresh: RefreshFn,
    on_editor_close: ?RefreshFn = null,
    root: *gtk.ScrolledWindow,
    body: *gtk.Box,
    title: *gtk.Label,
    refresh: *gtk.Button,
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
        const empty = label("dim-label");
        empty.as(gtk.Widget).setVisible(0);
        body.append(empty.as(gtk.Widget));
        const message = label("dim-label");
        body.append(message.as(gtk.Widget));
        root.setChild(body.as(gtk.Widget));
        self.* = .{ .allocator = allocator, .kind = kind, .context = context, .on_action = on_action, .on_refresh = on_refresh, .root = root, .body = body, .title = title, .refresh = refresh, .refresh_signal = 0, .empty = empty, .message = message };
        self.refresh_signal = gtk.Button.signals.clicked.connect(refresh, *Page, refreshClicked, self, .{});
        return self;
    }

    pub fn setMessage(self: *Page, message: ?[]const u8) void {
        setLabel(self.allocator, self.message, message orelse "") catch {};
        self.message.as(gtk.Widget).setVisible(@intFromBool(message != null));
    }

    pub fn widget(self: *Page) *gtk.Widget {
        return self.root.as(gtk.Widget);
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
        try setLabel(self.allocator, self.empty, view.empty_remote_label);
        self.empty.as(gtk.Widget).setVisible(@intFromBool(self.kind == .remote and view.remote.len == 0));
        for (self.groups.items) |group| group.seen = false;
        var previous: ?*gtk.Widget = self.empty.as(gtk.Widget);
        if (self.kind == .local) {
            const group = try self.groupFor("");
            try group.update(self, "", view.local, busy or self.editing);
            self.body.reorderChildAfter(group.root.as(gtk.Widget), previous);
        } else {
            const setup = try self.groupFor("remote_setup");
            try setup.update(self, "", view.remote_setup, busy or self.editing);
            self.body.reorderChildAfter(setup.root.as(gtk.Widget), previous);
            previous = setup.root.as(gtk.Widget);
            for (view.remote) |host| {
                const group = try self.groupFor(host.id);
                try group.update(self, host.name, host.rows, busy or self.editing);
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
    }

    /// Local group_id is empty; remote group_id is the shared projection's host id.
    /// The caller owns editor semantics; this slot merely parents an existing widget.
    pub fn setEditor(self: *Page, group_id: []const u8, row_id: []const u8, editor: ?*gtk.Widget) !void {
        for (self.groups.items) |group| {
            if (!std.mem.eql(u8, group.id, group_id)) continue;
            for (group.rows.items) |row| {
                if (!std.mem.eql(u8, row.id, row_id)) continue;
                if (row.editor == editor) return;
                if (editor) |child| if (child.getParent() != null) return error.EditorAlreadyParented;
                if (row.editor) |old| row.editor_box.remove(old);
                row.editor = editor;
                if (editor) |child| row.editor_box.append(child);
                row.editor_box.as(gtk.Widget).setVisible(@intFromBool(editor != null));
                row.editor_close.as(gtk.Widget).setVisible(@intFromBool(editor != null));
                self.setEditing(editor != null);
                return;
            }
        }
        return error.DiagnosticRowNotFound;
    }

    fn setEditing(self: *Page, editing: bool) void {
        self.editing = editing;
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

const Group = struct {
    id: []u8,
    root: *gtk.Box,
    title: *gtk.Label,
    rows_box: *gtk.Box,
    rows: std.ArrayListUnmanaged(*Row) = .empty,
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
        for (self.rows.items) |row| row.deinit(page, self);
        self.rows.deinit(page.allocator);
        page.body.remove(self.root.as(gtk.Widget));
        self.root.as(gobject.Object).unref();
        page.allocator.free(self.id);
        page.allocator.destroy(self);
    }

    fn hasEditor(self: *Group) bool {
        for (self.rows.items) |row| if (row.editor != null) return true;
        return false;
    }

    fn update(self: *Group, page: *Page, name: []const u8, views: []const RowView, busy: bool) !void {
        self.seen = true;
        try setLabel(page.allocator, self.title, name);
        self.title.as(gtk.Widget).setVisible(@intFromBool(name.len != 0));
        for (self.rows.items) |row| row.seen = false;
        var previous: ?*gtk.Widget = null;
        for (views) |view| {
            const existing: ?*Row = for (self.rows.items) |row| {
                if (std.mem.eql(u8, row.id, view.id)) break row;
            } else null;
            const row = existing orelse blk: {
                try self.rows.ensureUnusedCapacity(page.allocator, 1);
                const created = try Row.create(page, self.id, view.id);
                self.rows.appendAssumeCapacity(created);
                self.rows_box.append(created.root.as(gtk.Widget));
                break :blk created;
            };
            try row.update(page, view, busy);
            self.rows_box.reorderChildAfter(row.root.as(gtk.Widget), previous);
            previous = row.root.as(gtk.Widget);
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
                row.deinit(page, self);
            }
        }
    }
};

const Row = struct {
    page: *Page,
    group_id: []const u8,
    id: []u8,
    root: *gtk.Box,
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

    fn create(page: *Page, group_id: []const u8, id: []const u8) !*Row {
        const owned = try page.allocator.dupe(u8, id);
        errdefer page.allocator.free(owned);
        const self = try page.allocator.create(Row);
        const root = gtk.Box.new(.vertical, 8);
        _ = root.as(gobject.Object).refSink();
        root.as(gtk.Widget).addCssClass("card");
        const content = gtk.Box.new(.vertical, 8);
        margins(content.as(gtk.Widget), 12);
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
        self.* = .{ .page = page, .group_id = group_id, .id = owned, .root = root, .title = title, .detail = detail, .icon = icon, .button = button, .signal = 0, .editor_box = editor_box, .editor_close = editor_close };
        self.signal = gtk.Button.signals.clicked.connect(button, *Row, clicked, self, .{});
        return self;
    }

    fn deinit(self: *Row, page: *Page, group: *Group) void {
        gobject.signalHandlerDisconnect(self.button.as(gobject.Object), self.signal);
        group.rows_box.remove(self.root.as(gtk.Widget));
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
