//! Native presentation only. Its lifetime is independent of Session list rows.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const interaction = @import("session_editor_interaction.zig");
const auxiliary = @import("auxiliary_window.zig");
const Application = @import("../class/application.zig").Application;

pub const Request = struct {
    execution_target_id: []const u8,
    tmux_session_id: []const u8 = "",
    owner_host_id: ?[]const u8 = null,
    endpoint_id: ?[]const u8 = null,
    thread_id: ?[]const u8 = null,
    confirmation_id: []const u8,
};

pub const Window = struct {
    allocator: std.mem.Allocator,
    native: *adw.Window,
    title: *gtk.Label,
    message: *gtk.Label,
    status: *gtk.Label,
    submit: *gtk.Button,
    refresh: *gtk.Button,
    request: ?std.json.Parsed(Request) = null,
    pending: bool = false,
    context: ?*anyopaque,
    send: *const fn (?*anyopaque, Request) void,
    review: *const fn (?*anyopaque, Request) void,
    closed: *const fn (?*anyopaque) void,

    pub fn create(allocator: std.mem.Allocator, app: *Application, parent: *gtk.Window, context: ?*anyopaque, send: *const fn (?*anyopaque, Request) void, review: *const fn (?*anyopaque, Request) void, closed: *const fn (?*anyopaque) void) !*Window {
        const self = try allocator.create(Window);
        const window = auxiliary.create(app);
        window.as(gtk.Window).setTitle("End Session");
        window.as(gtk.Window).setTransientFor(parent);
        window.as(gtk.Window).setDefaultSize(440, 220);
        window.as(gtk.Window).setHideOnClose(1);
        const box = gtk.Box.new(.vertical, 12);
        for ([_]*gtk.Widget{box.as(gtk.Widget)}) |widget| {
            widget.setMarginTop(20);
            widget.setMarginBottom(20);
            widget.setMarginStart(20);
            widget.setMarginEnd(20);
        }
        const title = gtk.Label.new("End Session?");
        title.as(gtk.Widget).addCssClass("title-3");
        const message = gtk.Label.new("");
        message.setWrap(1);
        message.setXalign(0);
        const status = gtk.Label.new("");
        status.setWrap(1);
        status.setXalign(0);
        const actions = gtk.Box.new(.horizontal, 8);
        actions.as(gtk.Widget).setHalign(.end);
        const refresh = gtk.Button.newWithLabel("Refresh");
        const cancel = gtk.Button.newWithLabel("Close");
        const submit = gtk.Button.newWithLabel("End Session");
        submit.as(gtk.Widget).addCssClass("destructive-action");
        actions.append(refresh.as(gtk.Widget));
        actions.append(cancel.as(gtk.Widget));
        actions.append(submit.as(gtk.Widget));
        box.append(title.as(gtk.Widget));
        box.append(message.as(gtk.Widget));
        box.append(status.as(gtk.Widget));
        box.append(actions.as(gtk.Widget));
        window.setContent(box.as(gtk.Widget));
        self.* = .{ .allocator = allocator, .native = window, .title = title, .message = message, .status = status, .submit = submit, .refresh = refresh, .context = context, .send = send, .review = review, .closed = closed };
        _ = gtk.Button.signals.clicked.connect(refresh, *Window, refreshClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(cancel, *Window, closeClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(submit, *Window, submitClicked, self, .{});
        _ = gtk.Window.signals.close_request.connect(window.as(gtk.Window), *Window, closeRequest, self, .{});
        return self;
    }

    pub fn deinit(self: *Window) void {
        auxiliary.destroy(self.native);
        if (self.request) |value| value.deinit();
        const allocator = self.allocator;
        allocator.destroy(self);
    }

    pub fn present(self: *Window, request: Request, message: []const u8) !void {
        if (self.pending) {
            self.native.as(gtk.Window).present();
            return;
        }
        const encoded = try std.json.Stringify.valueAlloc(self.allocator, request, .{});
        defer self.allocator.free(encoded);
        const owned = try std.json.parseFromSlice(Request, self.allocator, encoded, .{ .allocate = .alloc_always });
        if (self.request) |old| old.deinit();
        self.request = owned;
        const text = try self.allocator.dupeZ(u8, message);
        defer self.allocator.free(text);
        self.message.setLabel(text);
        self.status.setLabel("");
        self.refresh.as(gtk.Widget).setSensitive(1);
        self.submit.as(gtk.Widget).setSensitive(1);
        self.native.as(gtk.Window).present();
    }

    pub fn apply(self: *Window, view: interaction.View) void {
        self.pending = view.pending;
        self.refresh.as(gtk.Widget).setSensitive(@intFromBool(!view.pending));
        self.submit.as(gtk.Widget).setSensitive(@intFromBool(view.can_submit));
        self.label(self.title, if (view.end_preview) |p| p.confirmation.title else "End Session?");
        self.label(self.message, if (view.end_preview) |p| p.confirmation.message else "");
        self.label(self.status, view.message orelse "");
        const submit = self.allocator.dupeZ(u8, view.submit_label) catch return;
        defer self.allocator.free(submit);
        self.submit.setLabel(submit);
        if (view.end_completed) self.native.as(gtk.Window).close();
    }
    fn label(self: *Window, widget: *gtk.Label, text: []const u8) void {
        const value = self.allocator.dupeZ(u8, text) catch return;
        defer self.allocator.free(value);
        widget.setLabel(value);
    }
    fn closeRequest(_: *gtk.Window, self: *Window) callconv(.c) c_int {
        self.closed(self.context);
        return 0;
    }
    fn refreshClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.pending) return;
        const request = (self.request orelse return).value;
        self.review(self.context, request);
    }
    fn closeClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        // Closing presentation never cancels or replays an admitted operation.
        self.native.as(gtk.Window).close();
    }
    fn submitClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.pending) return;
        const request = (self.request orelse return).value;
        // The callback may synchronously rebuild rows. No row pointer is held.
        self.send(self.context, request);
    }
};
