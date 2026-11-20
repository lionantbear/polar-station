//! Native directory controls for the shared Session editor projection.
const std = @import("std");
const gtk = @import("gtk");
const gobject = @import("gobject");

pub const Listing = struct {
    path: []const u8,
    home: []const u8,
    parent: ?[]const u8,
    directories: []const []const u8,
};

pub const View = struct {
    listing: ?Listing,
    loading: bool,
    @"error": ?[]const u8,
};

pub const Browser = struct {
    const PathAction = struct {
        allocator: std.mem.Allocator,
        browser: *Browser,
        path: []const u8,
        choose: bool,
    };
    const Folder = struct {
        name: *gtk.Button,
        check: *gtk.Widget,
    };

    allocator: std.mem.Allocator,
    root: *gtk.Box,
    toolbar: *gtk.Box,
    list: *gtk.Box,
    scroll: *gtk.ScrolledWindow,
    status: *gtk.Label,
    retry_button: *gtk.Button,
    folders: std.ArrayList(Folder) = .empty,
    listing_path: ?[]const u8 = null,
    context: *anyopaque,
    navigate: *const fn (*anyopaque, []const u8) void,
    choose: *const fn (*anyopaque, []const u8) void,
    retry: *const fn (*anyopaque) void,

    pub fn create(
        allocator: std.mem.Allocator,
        context: *anyopaque,
        navigate: *const fn (*anyopaque, []const u8) void,
        choose: *const fn (*anyopaque, []const u8) void,
        retry: *const fn (*anyopaque) void,
    ) !*Browser {
        const self = try allocator.create(Browser);
        self.* = .{
            .allocator = allocator,
            .root = gtk.Box.new(.vertical, 6),
            .toolbar = gtk.Box.new(.horizontal, 6),
            .list = gtk.Box.new(.vertical, 2),
            .scroll = gtk.ScrolledWindow.new(),
            .status = gtk.Label.new(""),
            .retry_button = gtk.Button.newWithLabel("Retry"),
            .context = context,
            .navigate = navigate,
            .choose = choose,
            .retry = retry,
        };
        _ = self.root.as(gobject.Object).refSink();
        self.root.as(gtk.Widget).setVisible(0);
        self.status.setWrap(1);
        self.status.setXalign(0);
        self.retry_button.as(gtk.Widget).setHalign(.start);
        self.scroll.setPolicy(.never, .automatic);
        self.scroll.as(gtk.Widget).setSizeRequest(-1, 152);
        self.scroll.setChild(self.list.as(gtk.Widget));
        const toolbar_scroll = gtk.ScrolledWindow.new();
        toolbar_scroll.setPolicy(.automatic, .never);
        toolbar_scroll.setChild(self.toolbar.as(gtk.Widget));
        self.root.append(toolbar_scroll.as(gtk.Widget));
        self.root.append(self.scroll.as(gtk.Widget));
        self.root.append(self.status.as(gtk.Widget));
        self.root.append(self.retry_button.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(self.retry_button, *Browser, retryClicked, self, .{});
        return self;
    }

    pub fn deinit(self: *Browser) void {
        self.clear();
        if (self.root.as(gtk.Widget).getParent()) |parent| {
            if (gobject.ext.cast(gtk.Box, parent)) |box| box.remove(self.root.as(gtk.Widget));
        }
        self.root.as(gobject.Object).unref();
        self.folders.deinit(self.allocator);
        self.allocator.destroy(self);
    }

    fn clear(self: *Browser) void {
        if (self.listing_path) |path| self.allocator.free(path);
        self.listing_path = null;
        self.folders.clearRetainingCapacity();
        while (self.list.as(gtk.Widget).getFirstChild()) |child| self.list.remove(child);
        while (self.toolbar.as(gtk.Widget).getFirstChild()) |child| self.toolbar.remove(child);
    }

    pub fn render(self: *Browser, view: ?View, selected_path: []const u8) !void {
        self.root.as(gtk.Widget).setVisible(@intFromBool(view != null));
        const state = view orelse {
            if (self.listing_path != null) self.clear();
            return;
        };
        const status = if (state.loading) "Reading the selected machine…" else if (state.@"error") |message| message else if (state.listing != null and state.listing.?.directories.len == 0) "No subfolders" else "";
        const status_z = try self.allocator.dupeSentinel(u8, status, 0);
        defer self.allocator.free(status_z);
        self.status.setLabel(status_z);
        self.status.as(gtk.Widget).setVisible(@intFromBool(status.len != 0));
        self.retry_button.as(gtk.Widget).setVisible(@intFromBool(state.@"error" != null));
        self.scroll.as(gtk.Widget).setVisible(@intFromBool(state.listing != null and state.listing.?.directories.len != 0));

        const listing = state.listing orelse {
            self.clear();
            return;
        };
        if (self.listing_path == null or !std.mem.eql(u8, self.listing_path.?, listing.path)) {
            self.clear();
            errdefer self.clear();
            try self.build(listing);
            self.listing_path = try self.allocator.dupe(u8, listing.path);
        }
        for (self.folders.items, listing.directories) |folder, path| {
            const chosen = std.mem.eql(u8, selected_path, path);
            folder.check.setVisible(@intFromBool(chosen));
            if (chosen) {
                folder.name.as(gtk.Widget).removeCssClass("flat");
                folder.name.as(gtk.Widget).addCssClass("suggested-action");
            } else {
                folder.name.as(gtk.Widget).removeCssClass("suggested-action");
                folder.name.as(gtk.Widget).addCssClass("flat");
            }
        }
    }

    fn build(self: *Browser, listing: Listing) !void {
        try self.appendNavigation("Home", listing.home);
        try self.appendNavigation("Up", listing.parent);
        try self.appendNavigation("/", "/");
        var parts = std.mem.splitScalar(u8, listing.path, '/');
        var prefix_length: usize = 0;
        while (parts.next()) |part| {
            prefix_length += part.len;
            if (part.len != 0) try self.appendNavigation(part, listing.path[0..prefix_length]);
            prefix_length += 1;
        }

        try self.folders.ensureTotalCapacity(self.allocator, listing.directories.len);
        for (listing.directories) |path| {
            const name_z = try self.allocator.dupeSentinel(u8, std.fs.path.basename(path), 0);
            defer self.allocator.free(name_z);
            const row = gtk.Box.new(.horizontal, 6);
            self.list.append(row.as(gtk.Widget));
            const name = gtk.Button.new();
            name.as(gtk.Widget).setHexpand(1);
            const label = gtk.Label.new(name_z);
            label.setXalign(0);
            label.as(gtk.Widget).setHexpand(1);
            const check = gtk.Image.newFromIconName("object-select-symbolic").as(gtk.Widget);
            const content = gtk.Box.new(.horizontal, 6);
            content.append(gtk.Image.newFromIconName("folder-symbolic").as(gtk.Widget));
            content.append(label.as(gtk.Widget));
            content.append(check);
            name.setChild(content.as(gtk.Widget));
            row.append(name.as(gtk.Widget));
            try self.bind(name, path, true);

            const enter = gtk.Button.newFromIconName("go-next-symbolic");
            enter.as(gtk.Widget).setTooltipText("Browse subfolders");
            row.append(enter.as(gtk.Widget));
            try self.bind(enter, path, false);
            self.folders.appendAssumeCapacity(.{ .name = name, .check = check });
        }
    }

    fn appendNavigation(self: *Browser, label: []const u8, path: ?[]const u8) !void {
        const title = try self.allocator.dupeSentinel(u8, label, 0);
        defer self.allocator.free(title);
        const button = gtk.Button.newWithLabel(title);
        self.toolbar.append(button.as(gtk.Widget));
        if (path) |value| {
            try self.bind(button, value, false);
        } else {
            button.as(gtk.Widget).setSensitive(0);
        }
    }

    fn bind(self: *Browser, button: *gtk.Button, path: []const u8, choose: bool) !void {
        const action = try self.allocator.create(PathAction);
        errdefer self.allocator.destroy(action);
        action.* = .{
            .allocator = self.allocator,
            .browser = self,
            .path = try self.allocator.dupe(u8, path),
            .choose = choose,
        };
        button.as(gobject.Object).weakRef(freeAction, action);
        _ = gtk.Button.signals.clicked.connect(button, *PathAction, pathClicked, action, .{});
    }

    fn freeAction(data: ?*anyopaque, _: *gobject.Object) callconv(.c) void {
        const action: *PathAction = @ptrCast(@alignCast(data orelse return));
        action.allocator.free(action.path);
        action.allocator.destroy(action);
    }

    fn pathClicked(_: *gtk.Button, action: *PathAction) callconv(.c) void {
        const browser = action.browser;
        if (action.choose) {
            browser.choose(browser.context, action.path);
        } else {
            browser.navigate(browser.context, action.path);
        }
    }

    fn retryClicked(_: *gtk.Button, self: *Browser) callconv(.c) void {
        self.retry(self.context);
    }
};
