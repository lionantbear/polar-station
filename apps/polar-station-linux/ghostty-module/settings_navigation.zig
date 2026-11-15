//! Thin native container for the shared Settings routes and directory.
const std = @import("std");
const gtk = @import("gtk");
const gobject = @import("gobject");
const c = @cImport({
    @cInclude("polar_station_desktop_client.h");
});
const auxiliary = @import("auxiliary_window.zig");
pub const Page = struct {
    allocator: std.mem.Allocator,
    root: *gtk.Box,
    title: *gtk.Label,
    back: *gtk.Button,
    body: *gtk.Box,
    content_scroll: *gtk.ScrolledWindow,
    handle: ?*c.SettingsNavigationHandle,
    open_context: ?*anyopaque = null,
    on_open: ?*const fn (?*anyopaque, *Page, std.json.Value) void = null,
    lease: ?auxiliary.ContentLease = null,
    can_back: bool = false,
    embedded_host: auxiliary.EmbeddedHost = .{ .close = editorClosed },
    clicks: std.ArrayListUnmanaged(*Click) = .empty,
    const Click = struct { page: *Page, route: []u8, navigate: bool = false };
    pub fn create(allocator: std.mem.Allocator) !*Page {
        const self = try allocator.create(Page);
        const root = gtk.Box.new(.vertical, 0);
        const header = gtk.Box.new(.horizontal, 8);
        const back = gtk.Button.newWithLabel("‹");
        const title = gtk.Label.new("Settings");
        title.setXalign(0);
        title.setWrap(1);
        title.as(gtk.Widget).setHexpand(1);
        title.as(gtk.Widget).addCssClass("heading");
        header.append(back.as(gtk.Widget));
        header.append(title.as(gtk.Widget));
        header.as(gtk.Widget).setMarginStart(12);
        header.as(gtk.Widget).setMarginEnd(12);
        header.as(gtk.Widget).setMarginTop(12);
        header.as(gtk.Widget).setMarginBottom(12);
        root.append(header.as(gtk.Widget));
        const body = gtk.Box.new(.vertical, 8);
        body.as(gtk.Widget).setVexpand(1);
        const content_scroll = gtk.ScrolledWindow.new();
        content_scroll.setPolicy(.never, .never);
        content_scroll.as(gtk.Widget).setVexpand(1);
        content_scroll.setChild(body.as(gtk.Widget));
        root.append(content_scroll.as(gtk.Widget));
        root.as(gtk.Widget).setVexpand(1);
        self.* = .{ .allocator = allocator, .root = root, .back = back, .title = title, .body = body, .content_scroll = content_scroll, .handle = c.polar_station_settings_navigation_new() };
        self.embedded_host.context = self;
        _ = gtk.Button.signals.clicked.connect(back, *Page, goBack, self, .{});
        self.send(.{ .type = "show" });
        return self;
    }
    pub fn deinit(self: *Page) void {
        self.clear();
        c.polar_station_settings_navigation_free(self.handle);
        self.allocator.destroy(self);
    }
    fn clear(self: *Page) void {
        if (self.lease) |lease| {
            self.body.remove(lease.widget);
            lease.restore();
            self.lease = null;
        }
        while (self.body.as(gtk.Widget).getFirstChild()) |child| self.body.remove(child);
        for (self.clicks.items) |click| {
            self.allocator.free(click.route);
            self.allocator.destroy(click);
        }
        self.clicks.deinit(self.allocator);
        self.clicks = .empty;
    }
    pub fn mount(self: *Page, widget: *gtk.Widget) void {
        self.body.append(widget);
    }
    pub fn attach(self: *Page, lease: auxiliary.ContentLease) void {
        // A process-owned original form can have one physical parent. Relinquish
        // its former presentation before moving that same control tree.
        if(lease.widget.getParent()!=null) {
            if(lease.widget.as(gobject.Object).getData("polar-station-editor-host")) |raw| {
                const prior:*auxiliary.EmbeddedHost=@ptrCast(@alignCast(raw));
                if(prior!=&self.embedded_host) prior.close(prior.context);
            }
        }
        lease.bind(&self.embedded_host);
        self.lease = lease;
        self.body.append(lease.widget);
    }
    pub fn send(self: *Page, input: anytype) void {
        const bytes = std.json.Stringify.valueAlloc(self.allocator, input, .{}) catch return;
        defer self.allocator.free(bytes);
        const result = c.polar_station_settings_navigation_dispatch(self.handle, bytes.ptr, bytes.len) orelse return;
        defer c.polar_station_desktop_string_free(result);
        const parsed = std.json.parseFromSlice(std.json.Value, self.allocator, std.mem.span(result), .{}) catch return;
        defer parsed.deinit();
        const value = parsed.value;
        if (value != .object) return;
        const route = value.object.get("route") orelse return;
        self.clear();
        if (value.object.get("title")) |title| if (title == .string) {
            const label = self.allocator.dupeZ(u8, title.string) catch return;
            defer self.allocator.free(label);
            self.title.setLabel(label);
        };
        const can_back: std.json.Value = value.object.get("can_back") orelse .{ .bool = false };
        self.can_back = can_back == .bool and can_back.bool;
        self.back.as(gtk.Widget).setVisible(@intFromBool(self.can_back));
        if(value.object.get("back_label")) |caption| if(caption==.string) {
            const z=self.allocator.dupeZ(u8,caption.string) catch return;defer self.allocator.free(z);
            self.back.setLabel(z);
            @import("form_controls.zig").wrapCaption(self.back.getChild());
            self.back.as(gtk.Widget).setTooltipText(z);
        };
        if (route != .object) return;
        const page = route.object.get("page") orelse return;
        self.content_scroll.setPolicy(.never, if (if (value.object.get("is_directory")) |directory| directory == .bool and directory.bool else false) .automatic else .never);
        if (page == .string and std.mem.eql(u8, page.string, "setup")) {
            if (value.object.get("scopes")) |scopes| if (scopes == .array) {
                const bar = gtk.Box.new(.horizontal, 4);
                for (scopes.array.items) |scope| {
                    if (scope != .object) continue;
                    const label = scope.object.get("title") orelse continue;
                    const destination = scope.object.get("route") orelse continue;
                    if (label != .string) continue;
                    const z = self.allocator.dupeZ(u8, label.string) catch continue;
                    defer self.allocator.free(z);
                    const button = gtk.Button.newWithLabel(z);
                    const click = self.allocator.create(Click) catch continue;
                    const encoded = std.json.Stringify.valueAlloc(self.allocator, destination, .{}) catch {
                        self.allocator.destroy(click);
                        continue;
                    };
                    click.* = .{ .page = self, .route = encoded };
                    self.clicks.append(self.allocator, click) catch {
                        self.allocator.free(encoded);
                        self.allocator.destroy(click);
                        continue;
                    };
                    _ = gtk.Button.signals.clicked.connect(button, *Click, selected, click, .{});
                    bar.append(button.as(gtk.Widget));
                }
                self.body.append(bar.as(gtk.Widget));
            };
        }
        if (if (value.object.get("is_directory")) |directory| directory == .bool and directory.bool else false) {
            const entries = value.object.get("entries") orelse return;
            if (entries != .array) return;
            for (entries.array.items) |entry| {
                if (entry != .object) continue;
                const label = entry.object.get("title") orelse continue;
                const destination = entry.object.get("route") orelse continue;
                if (label != .string) continue;
                const z = self.allocator.dupeZ(u8, label.string) catch continue;
                defer self.allocator.free(z);
                const button = gtk.Button.newWithLabel(z);
                button.as(gtk.Widget).addCssClass("flat");
                const click = self.allocator.create(Click) catch continue;
                const encoded = std.json.Stringify.valueAlloc(self.allocator, destination, .{}) catch {
                    self.allocator.destroy(click);
                    continue;
                };
                click.* = .{ .page = self, .route = encoded, .navigate = true };
                self.clicks.append(self.allocator, click) catch {
                    self.allocator.free(encoded);
                    self.allocator.destroy(click);
                    continue;
                };
                _ = gtk.Button.signals.clicked.connect(button, *Click, selected, click, .{});
                self.body.append(button.as(gtk.Widget));
            }
        } else if (self.on_open) |open| open(self.open_context, self, route);
    }
    fn editorClosed(context: ?*anyopaque) void {
        const self: *Page = @ptrCast(@alignCast(context orelse return));
        self.send(.{ .type = "back" });
    }
    pub fn requestBack(self: *Page) void {
        if (self.lease) |lease| {
            if(lease.close_policy) |policy| {policy.close(policy.context);return;}
            if(!lease.close_before_navigation or lease.window==null) {
                self.send(.{.type="back"});return;
            }
            // The editor is not a realized Window while embedded. Window.close()
            // does not emit close-request in that state; dispatch its existing
            // close policy directly, preserving the original dirty confirmation.
            var handled: c_int = 0;
            gobject.signalEmitByName(lease.window.?.as(gobject.Object), "close-request", &handled);
        } else self.send(.{ .type = "back" });
    }
    fn goBack(_: *gtk.Button, self: *Page) callconv(.c) void {
        self.requestBack();
    }
    fn selected(_: *gtk.Button, click: *Click) callconv(.c) void {
        const self = click.page;
        const parsed = std.json.parseFromSlice(std.json.Value, self.allocator, click.route, .{}) catch return;
        defer parsed.deinit();
        // send clears the clicked button; keep its route bytes alive for this call.
        const input_type: []const u8 = if (click.navigate) "navigate" else "open";
        self.send(.{ .type = input_type, .route = parsed.value });
    }
};
