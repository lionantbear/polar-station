//! The native Settings Pane frame hosts a page and its status.
const std = @import("std");
const gtk = @import("gtk");

pub const Frame = struct {
    root: *gtk.ScrolledWindow,
    page: *gtk.Box,
    title: *gtk.Label,
    status: *gtk.Label,

    pub fn create() Frame {
        const root = gtk.ScrolledWindow.new();
        root.setPolicy(.external, .automatic);
        const body = gtk.Box.new(.vertical, 10);
        const widget = body.as(gtk.Widget);
        widget.setMarginStart(12);
        widget.setMarginEnd(12);
        widget.setMarginTop(12);
        widget.setMarginBottom(12);
        widget.addCssClass("polar-station-controller-root");
        root.setChild(widget);
        const title = gtk.Label.new("Settings");
        title.setXalign(0);
        title.as(gtk.Widget).addCssClass("title-2");
        const page = gtk.Box.new(.vertical, 10);
        const status = gtk.Label.new(null);
        status.setWrap(1);
        status.setXalign(0);
        status.setSelectable(1);
        body.append(title.as(gtk.Widget));
        body.append(page.as(gtk.Widget));
        body.append(status.as(gtk.Widget));
        return .{ .root = root, .page = page, .title = title, .status = status };
    }

    pub fn show(self: Frame, allocator: std.mem.Allocator, message: []const u8) void {
        const z = allocator.dupeSentinel(u8, message, 0) catch return;
        defer allocator.free(z);
        self.status.setText(z);
    }

    pub fn setPending(self: Frame, pending: bool) void {
        self.page.as(gtk.Widget).setSensitive(@intFromBool(!pending));
    }
};
