//! A native view of directory facts returned by the shared target owner.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const gobject = @import("gobject");
const Application = @import("../class/application.zig").Application;
const auxiliary = @import("auxiliary_window.zig");
pub const Directory = struct {
    execution_target_id: []const u8,
    home: []const u8,
    path: []const u8,
    parent: ?[]const u8,
    directories: []const []const u8,
};
pub const Window = struct {
    allocator: std.mem.Allocator,
    native: *adw.Window,
    embedded: bool = false,
    embedded_root: ?*gtk.Widget = null,
    cancel: *gtk.Button,
    path: *gtk.Entry,
    crumbs: *gtk.Box,
    list: *gtk.ListBox,
    status: *gtk.Label,
    choose: *gtk.Button,
    directory: ?std.json.Parsed(Directory) = null,
    context: ?*anyopaque,
    navigate: *const fn (?*anyopaque, []const u8) void,
    selected: *const fn (?*anyopaque, []const u8) void,
    const Row = struct { owner: *Window, path: []u8 };

    pub fn create(allocator: std.mem.Allocator, app: *Application, parent: *gtk.Window, context: ?*anyopaque, navigate: *const fn (?*anyopaque, []const u8) void, selected: *const fn (?*anyopaque, []const u8) void) !*Window {
        const self = try allocator.create(Window);
        const native = auxiliary.create(app);
        native.as(gtk.Window).setTitle("Choose Startup Directory");
        native.as(gtk.Window).setTransientFor(parent);
        native.as(gtk.Window).setDefaultSize(540, 420);
        native.as(gtk.Window).setHideOnClose(1);
        const box = gtk.Box.new(.vertical, 10);
        box.as(gtk.Widget).setMarginTop(16);
        box.as(gtk.Widget).setMarginBottom(16);
        box.as(gtk.Widget).setMarginStart(16);
        box.as(gtk.Widget).setMarginEnd(16);
        const top = gtk.Box.new(.horizontal, 6);
        const home = gtk.Button.newWithLabel("Home");
        const up = gtk.Button.newWithLabel("Up");
        const path = gtk.Entry.new();
        path.as(gtk.Widget).setHexpand(1);
        top.append(home.as(gtk.Widget));
        top.append(up.as(gtk.Widget));
        top.append(path.as(gtk.Widget));
        const crumbs = gtk.Box.new(.horizontal, 4);
        const crumb_scroll = gtk.ScrolledWindow.new();
        crumb_scroll.setPolicy(.automatic, .never);
        crumb_scroll.setChild(crumbs.as(gtk.Widget));
        const list = gtk.ListBox.new();
        list.setSelectionMode(.none);
        const scroll = gtk.ScrolledWindow.new();
        scroll.setChild(list.as(gtk.Widget));
        scroll.as(gtk.Widget).setVexpand(1);
        const status = gtk.Label.new("");
        status.setWrap(1);
        const actions = gtk.Box.new(.horizontal, 6);
        actions.as(gtk.Widget).setHalign(.end);
        const cancel = gtk.Button.newWithLabel("Cancel");
        const choose = gtk.Button.newWithLabel("Choose This Directory");
        choose.as(gtk.Widget).addCssClass("suggested-action");
        actions.append(cancel.as(gtk.Widget));
        actions.append(choose.as(gtk.Widget));
        box.append(top.as(gtk.Widget));
        box.append(crumb_scroll.as(gtk.Widget));
        box.append(scroll.as(gtk.Widget));
        box.append(status.as(gtk.Widget));
        box.append(actions.as(gtk.Widget));
        native.setContent(box.as(gtk.Widget));
        self.* = .{ .allocator = allocator, .native = native, .path = path, .crumbs = crumbs, .list = list, .status = status, .choose = choose, .cancel = cancel, .context = context, .navigate = navigate, .selected = selected };
        _ = gtk.Button.signals.clicked.connect(home, *Window, homeClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(up, *Window, upClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(cancel, *Window, cancelClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(choose, *Window, chooseClicked, self, .{});
        _ = gtk.Entry.signals.activate.connect(path, *Window, pathActivated, self, .{});
        return self;
    }
    pub fn embed(self: *Window, split: *gtk.Paned) void {
        if (self.embedded) return;
        const content = self.native.getContent() orelse return;
        _ = content.as(gobject.Object).ref();
        self.native.setContent(null);
        split.setStartChild(content);
        content.as(gobject.Object).unref();
        self.embedded = true;
        self.embedded_root = content;
        content.as(gobject.Object).weakRef(embeddedFreed, self);
        self.cancel.as(gtk.Widget).setVisible(0);
    }
    fn embeddedFreed(data: ?*anyopaque, _: *gobject.Object) callconv(.c) void {
        const self: *Window = @ptrCast(@alignCast(data orelse return));
        self.embedded_root = null;
    }
    pub fn deinit(self: *Window) void {
        if (self.embedded_root) |root| if (root.getParent()) |parent| {
            if (gobject.ext.cast(gtk.Paned, parent)) |split| split.setStartChild(null);
        };
        auxiliary.destroy(self.native);
        if (self.directory) |directory| directory.deinit();
        const allocator = self.allocator;
        allocator.destroy(self);
    }
    pub fn loading(self: *Window) void {
        while (self.list.as(gtk.Widget).getFirstChild()) |child| self.list.remove(child);
        while (self.crumbs.as(gtk.Widget).getFirstChild()) |child| self.crumbs.remove(child);
        if (self.directory) |old| old.deinit();
        self.directory = null;
        self.beginRead();
        if (!self.embedded) self.native.as(gtk.Window).present();
    }
    pub fn beginRead(self: *Window) void {
        self.status.setLabel("Reading the selected machine…");
        self.choose.as(gtk.Widget).setSensitive(0);
    }
    pub fn failed(self: *Window, message: []const u8) void {
        const text = self.allocator.dupeZ(u8, message) catch return;
        defer self.allocator.free(text);
        self.status.setLabel(text);
    }
    pub fn apply(self: *Window, directory: Directory) !void {
        if (!self.embedded and self.native.as(gtk.Widget).getVisible() == 0) return;
        const json = try std.json.Stringify.valueAlloc(self.allocator, directory, .{});
        defer self.allocator.free(json);
        const owned = try std.json.parseFromSlice(Directory, self.allocator, json, .{ .allocate = .alloc_always });
        while (self.list.as(gtk.Widget).getFirstChild()) |child| self.list.remove(child);
        if (self.directory) |old| old.deinit();
        self.directory = owned;
        const path = try self.allocator.dupeZ(u8, directory.path);
        defer self.allocator.free(path);
        self.path.as(gtk.Editable).setText(path);
        for (owned.value.directories) |entry| {
            const title = try self.allocator.dupeZ(u8, std.fs.path.basename(entry));
            defer self.allocator.free(title);
            const button = gtk.Button.newWithLabel(title);
            button.as(gtk.Widget).addCssClass("flat");
            const row = try self.allocator.create(Row);
            row.* = .{ .owner = self, .path = try self.allocator.dupe(u8, entry) };
            button.as(gobject.Object).weakRef(rowFreed, row);
            _ = gtk.Button.signals.clicked.connect(button, *Row, rowClicked, row, .{});
            self.list.append(button.as(gtk.Widget));
        }
        while (self.crumbs.as(gtk.Widget).getFirstChild()) |child| self.crumbs.remove(child);
        try self.appendCrumb("/", "/");
        var parts = std.mem.splitScalar(u8, directory.path, '/');
        var prefix = try self.allocator.dupe(u8, "");
        defer self.allocator.free(prefix);
        while (parts.next()) |part| {
            if (part.len == 0) continue;
            const next = try std.fmt.allocPrint(self.allocator, "{s}/{s}", .{ prefix, part });
            self.allocator.free(prefix);
            prefix = next;
            try self.appendCrumb(prefix, part);
        }
        self.status.setLabel(if (directory.directories.len == 0) "No subfolders" else "");
        self.choose.as(gtk.Widget).setSensitive(1);
    }
    fn appendCrumb(self: *Window, path: []const u8, label: []const u8) !void {
        const text = try self.allocator.dupeZ(u8, label);
        defer self.allocator.free(text);
        const button = gtk.Button.newWithLabel(text);
        button.as(gtk.Widget).addCssClass("flat");
        const row = try self.allocator.create(Row);
        row.* = .{ .owner = self, .path = try self.allocator.dupe(u8, path) };
        button.as(gobject.Object).weakRef(rowFreed, row);
        _ = gtk.Button.signals.clicked.connect(button, *Row, rowClicked, row, .{});
        self.crumbs.append(button.as(gtk.Widget));
    }
    fn rowFreed(data: ?*anyopaque, _: *gobject.Object) callconv(.c) void {
        const row: *Row = @ptrCast(@alignCast(data orelse return));
        const allocator = row.owner.allocator;
        allocator.free(row.path);
        allocator.destroy(row);
    }
    fn rowClicked(_: *gtk.Button, row: *Row) callconv(.c) void {
        row.owner.navigate(row.owner.context, row.path);
    }
    fn homeClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.directory) |data| self.navigate(self.context, data.value.home);
    }
    fn upClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.directory) |data| if (data.value.parent) |path| {
            self.navigate(self.context, path);
        };
    }
    fn pathActivated(entry: *gtk.Entry, self: *Window) callconv(.c) void {
        self.navigate(self.context, std.mem.span(entry.as(gtk.Editable).getText()));
    }
    fn chooseClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        const path = if (self.directory) |data| data.value.path else return;
        // Close first; selecting never recreates a directory or submits creation.
        self.native.as(gtk.Widget).setVisible(0);
        self.selected(self.context, path);
    }
    fn cancelClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        self.native.as(gtk.Widget).setVisible(0);
    }
};
