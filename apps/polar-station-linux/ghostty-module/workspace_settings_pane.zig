//! Native presentation of a single saved Workspace.
const std = @import("std");
const gtk = @import("gtk");
const gobject = @import("gobject");
const transfer = @import("workspace_transfer_window.zig");

pub const Layout = struct {
    name: []const u8,
    tabs: []Tab,
    const Tab = struct { id: []const u8, title: []const u8, pane_ids: [][]const u8 };
};
const Detail = struct {
    layout: Layout,
    revision: u64,
    workspace: struct {
        tabs: []const struct {
            id: []const u8,
            display_mode: []const u8,
            main_pane_id: ?[]const u8 = null,
            panes: []const struct {
                id: []const u8,
                execution_target_id: ?[]const u8 = null,
                tmux_session_id: ?[]const u8 = null,
                content: ?std.json.Value = null,
                source: ?std.json.Value = null,
            },
        },
    },
};

pub const Pane = struct {
    allocator: std.mem.Allocator,
    arena: std.heap.ArenaAllocator,
    actions: std.heap.ArenaAllocator,
    root: *gtk.ScrolledWindow,
    body: *gtk.Box,
    status: *gtk.Label,
    workspace_id: ?[]const u8 = null,
    detail: ?Detail = null,
    name: ?*gtk.Entry = null,
    titles: []const *gtk.Entry = &.{},
    context: *anyopaque,
    save: *const fn (*anyopaque, *Pane, []const u8, u64, Layout) anyerror!void,
    reload: *const fn (*anyopaque, *Pane, []const u8) anyerror!void,
    pending: bool = false,

    const Action = struct {
        pane: *Pane,
        kind: enum { save, reload, tab_order, pane_order, pane_move },
        tab: usize = 0,
        from: usize = 0,
        to: usize = 0,
    };

    pub fn create(allocator: std.mem.Allocator, context: *anyopaque, save: @FieldType(Pane, "save"), reload: @FieldType(Pane, "reload")) !*Pane {
        const self = try allocator.create(Pane);
        const root = gtk.ScrolledWindow.new();
        const body = gtk.Box.new(.vertical, 10);
        const status = gtk.Label.new(null);
        _ = status.as(gobject.Object).refSink();
        root.setPolicy(.never, .automatic);
        root.setChild(body.as(gtk.Widget));
        body.as(gtk.Widget).setMarginStart(16);
        body.as(gtk.Widget).setMarginEnd(16);
        body.as(gtk.Widget).setMarginTop(16);
        body.as(gtk.Widget).setMarginBottom(16);
        self.* = .{
            .allocator = allocator,
            .arena = std.heap.ArenaAllocator.init(allocator),
            .actions = std.heap.ArenaAllocator.init(allocator),
            .root = root,
            .body = body,
            .status = status,
            .context = context,
            .save = save,
            .reload = reload,
        };
        self.render();
        return self;
    }

    pub fn deinit(self: *Pane) void {
        self.clear();
        self.status.as(gobject.Object).unref();
        self.actions.deinit();
        self.arena.deinit();
        self.allocator.destroy(self);
    }

    pub fn load(self: *Pane, id: []const u8, document: std.json.Value) !void {
        // Build replacement before touching the visible draft.
        var next = std.heap.ArenaAllocator.init(self.allocator);
        errdefer next.deinit();
        const a = next.allocator();
        const detail = try std.json.parseFromValueLeaky(Detail, a, document, .{ .ignore_unknown_fields = true, .allocate = .alloc_always });
        const owned_id = try a.dupe(u8, id);
        self.arena.deinit();
        self.arena = next;
        self.workspace_id = owned_id;
        self.detail = detail;
        self.pending = false;
        self.render();
    }

    pub fn show(self: *Pane, message: []const u8) void {
        const text = self.allocator.dupeZ(u8, message) catch return;
        defer self.allocator.free(text);
        self.status.setText(text);
    }

    pub fn setPending(self: *Pane, pending: bool) void {
        self.pending = pending;
        self.body.as(gtk.Widget).setSensitive(@intFromBool(!pending));
    }

    fn clear(self: *Pane) void {
        while (self.body.as(gtk.Widget).getFirstChild()) |child| self.body.remove(child);
        _ = self.actions.reset(.retain_capacity);
        self.name = null;
        self.titles = &.{};
    }

    fn label(self: *Pane, box: *gtk.Box, text: []const u8) void {
        const z = self.allocator.dupeZ(u8, text) catch return;
        defer self.allocator.free(z);
        const widget = gtk.Label.new(z);
        widget.setXalign(0);
        widget.setWrap(1);
        widget.setSelectable(1);
        box.append(widget.as(gtk.Widget));
    }

    fn entry(self: *Pane, box: *gtk.Box, placeholder: [:0]const u8, text: []const u8) !*gtk.Entry {
        const z = try self.allocator.dupeZ(u8, text);
        defer self.allocator.free(z);
        const widget = gtk.Entry.new();
        widget.setPlaceholderText(placeholder);
        widget.as(gtk.Editable).setText(z);
        box.append(widget.as(gtk.Widget));
        return widget;
    }

    fn button(self: *Pane, box: *gtk.Box, title: []const u8, action: Action) void {
        const a = self.actions.allocator();
        const z = a.dupeZ(u8, title) catch return;
        const binding = a.create(Action) catch return;
        binding.* = action;
        const widget = gtk.Button.newWithLabel(z);
        _ = gtk.Button.signals.clicked.connect(widget, *Action, clicked, binding, .{});
        box.append(widget.as(gtk.Widget));
    }

    fn render(self: *Pane) void {
        self.clear();
        self.label(self.body, if (self.workspace_id == null) "Settings" else "Workspace");
        if (self.detail) |detail| {
            self.name = self.entry(self.body, "Workspace name", detail.layout.name) catch return;
            self.label(self.body, "Tabs & Panes");
            const titles = self.actions.allocator().alloc(*gtk.Entry, detail.layout.tabs.len) catch return;
            for (detail.layout.tabs, 0..) |tab, index| {
                for (detail.workspace.tabs) |saved| if (std.mem.eql(u8, saved.id, tab.id)) {
                    self.label(self.body, saved.display_mode);
                    break;
                };
                titles[index] = self.entry(self.body, "Tab title", tab.title) catch return;
                if (index > 0) self.button(self.body, "Move Tab up", .{ .pane = self, .kind = .tab_order, .from = index, .to = index - 1 });
                if (index + 1 < titles.len) self.button(self.body, "Move Tab down", .{ .pane = self, .kind = .tab_order, .from = index, .to = index + 1 });
                for (tab.pane_ids, 0..) |id, position| {
                    self.paneLabel(detail, id);
                    if (position > 0) self.button(self.body, "Move Pane up", .{ .pane = self, .kind = .pane_order, .tab = index, .from = position, .to = position - 1 });
                    if (position + 1 < tab.pane_ids.len) self.button(self.body, "Move Pane down", .{ .pane = self, .kind = .pane_order, .tab = index, .from = position, .to = position + 1 });
                    for (detail.layout.tabs, 0..) |target, destination| {
                        if (destination == index or target.pane_ids.len >= 9) continue;
                        const title = std.fmt.allocPrint(self.actions.allocator(), "Move to {s}", .{target.title}) catch continue;
                        self.button(self.body, title, .{ .pane = self, .kind = .pane_move, .tab = index, .from = position, .to = destination });
                    }
                }
            }
            self.titles = titles;
            self.button(self.body, "Save", .{ .pane = self, .kind = .save });
            self.button(self.body, "Reload", .{ .pane = self, .kind = .reload });
        } else self.label(self.body, "Choose a Workspace in Control Center Settings.");
        // Keep the status across form rebuilds. The owning Pane retains a ref.
        self.status.setWrap(1);
        self.status.setXalign(0);
        self.body.append(self.status.as(gtk.Widget));
        self.body.as(gtk.Widget).setSensitive(@intFromBool(!self.pending));
    }

    fn paneLabel(self: *Pane, detail: Detail, id: []const u8) void {
        for (detail.workspace.tabs) |tab| for (tab.panes) |pane| {
            if (!std.mem.eql(u8, pane.id, id)) continue;
            const mark = if (std.mem.eql(u8, tab.main_pane_id orelse "", id)) "MAIN · " else "";
            if (pane.content) |content| {
                const line = std.fmt.allocPrint(self.actions.allocator(), "{s}{s} · {s}", .{ mark, transfer.string(content, "kind"), id }) catch return;
                self.label(self.body, line);
            } else {
                const host = if (pane.source) |source| transfer.string(transfer.field(source, "host"), "target_id") else pane.execution_target_id orelse "";
                const line = std.fmt.allocPrint(self.actions.allocator(), "{s}{s} · {s} · {s}", .{ mark, host, pane.tmux_session_id orelse "Unavailable Session", id }) catch return;
                self.label(self.body, line);
            }
            return;
        };
    }

    fn recordText(self: *Pane, layout: *Layout) !void {
        const a = self.arena.allocator();
        layout.name = try a.dupe(u8, std.mem.span(self.name.?.as(gtk.Editable).getText()));
        for (layout.tabs, self.titles) |*tab, widget| tab.title = try a.dupe(u8, std.mem.span(widget.as(gtk.Editable).getText()));
    }

    fn clicked(_: *gtk.Button, binding: *Action) callconv(.c) void {
        const action = binding.*;
        action.pane.apply(action) catch |err| action.pane.show(@errorName(err));
    }

    fn apply(self: *Pane, action: Action) !void {
        if (self.pending) return;
        const id = self.workspace_id orelse return;
        if (action.kind == .reload) return self.reload(self.context, self, id);
        const detail = &self.detail.?;
        const layout = &detail.layout;
        try self.recordText(layout);
        switch (action.kind) {
            .reload => unreachable,
            .save => return self.save(self.context, self, id, detail.revision, layout.*),
            .tab_order => std.mem.swap(Layout.Tab, &layout.tabs[action.from], &layout.tabs[action.to]),
            .pane_order => {
                const ids = layout.tabs[action.tab].pane_ids;
                std.mem.swap([]const u8, &ids[action.from], &ids[action.to]);
            },
            .pane_move => {
                const source = &layout.tabs[action.tab];
                const destination = &layout.tabs[action.to];
                const a = self.arena.allocator();
                destination.pane_ids = try std.mem.concat(a, []const u8, &.{ destination.pane_ids, &.{source.pane_ids[action.from]} });
                @memmove(source.pane_ids[action.from .. source.pane_ids.len - 1], source.pane_ids[action.from + 1 ..]);
                source.pane_ids = source.pane_ids[0 .. source.pane_ids.len - 1];
            },
        }
        self.render();
    }
};
