//! Transient native PTY ownership and real Ghostty events. Rust owns workflow.
const std = @import("std");
const gtk = @import("gtk");
const glib = @import("glib");
const gobject = @import("gobject");
const Surface = @import("../class/surface.zig").Surface;
const c = @import("polar_station_c");
pub const Launch = struct { id: []const u8, execution_target_id: []const u8, title: []const u8, command: []const u8 };
pub const Source = struct { controller_id: u64, token: u64, kind: enum { editor, diagnostics } };
pub const Material = struct { setup: Launch, source: Source };
pub const Copy = struct { restored_title: []const u8, restored_detail: []const u8, close_title: []const u8, close_detail: []const u8, keep: []const u8, stop: []const u8 };
pub fn loadCopy(allocator: std.mem.Allocator) !std.json.Parsed(Copy) {
    const bytes = c.polar_station_tool_setup_copy() orelse return error.SetupCopyUnavailable;
    defer c.polar_station_desktop_string_free(bytes);
    return std.json.parseFromSlice(Copy, allocator, std.mem.span(bytes), .{ .allocate = .alloc_always });
}
const key = "polar-station-setup-surface";
const Delivery = struct {
    source: Source,
    event: union(enum) { started: ?[]const u8, exited: ?u32 },
    fn send(source: Source, event: @FieldType(Delivery, "event")) void {
        const value = std.heap.c_allocator.create(Delivery) catch return;
        value.* = .{ .source = source, .event = event };
        _ = glib.idleAddOnce(deliver, value);
    }
    fn deliver(raw: ?*anyopaque) callconv(.c) void {
        const value: *Delivery = @ptrCast(@alignCast(raw orelse return));
        defer std.heap.c_allocator.destroy(value);
        switch (value.event) {
            .started => |failure| @import("main.zig").toolSetupStarted(value.source, failure),
            .exited => |code| @import("main.zig").toolSetupExited(value.source, code),
        }
    }
};
pub const SetupSurface = struct {
    allocator: std.mem.Allocator,
    surface: *Surface,
    source: Source,
    init_handler: c_ulong,
    initialized: bool = false,
    finished: bool = false,
    pub fn create(allocator: std.mem.Allocator, material: Material) !*SetupSurface {
        const self = try allocator.create(SetupSurface);
        errdefer allocator.destroy(self);
        const command = try allocator.dupeSentinel(u8, material.setup.command, 0);
        defer allocator.free(command);
        const title = try allocator.dupeSentinel(u8, material.setup.title, 0);
        defer allocator.free(title);
        const surface = Surface.new(.{ .command = .{ .shell = command }, .title = title, .station_wait_after_command = true });
        _ = surface.as(gobject.Object).refSink();
        self.* = .{ .allocator = allocator, .surface = surface, .source = material.source, .init_handler = 0 };
        surface.as(gobject.Object).setData(key, self);
        self.init_handler = Surface.signals.init.connect(surface, *SetupSurface, ready, self, .{});
        return self;
    }
    fn ready(_: *Surface, self: *SetupSurface) callconv(.c) void {
        if (self.initialized or self.finished) return;
        self.initialized = true;
        Delivery.send(self.source, .{ .started = null });
    }
    pub fn retire(self: *SetupSurface) void {
        if (self.finished) return;
        self.finished = true;
        if (!self.initialized) Delivery.send(self.source, .{ .started = "The setup terminal closed before it initialized." });
        Delivery.send(self.source, .{ .exited = null });
    }
    pub fn destroy(self: *SetupSurface) void {
        self.retire();
        self.surface.as(gobject.Object).setData(key, null);
        gobject.signalHandlerDisconnect(self.surface.as(gobject.Object), self.init_handler);
        self.surface.unref();
        self.allocator.destroy(self);
    }
};
fn from(surface: *Surface) ?*SetupSurface {
    return @ptrCast(@alignCast(surface.as(gobject.Object).getData(key) orelse return null));
}
pub fn pasteCommitted(surface: *Surface) void {
    const setup = from(surface) orelse return;
    if (setup.finished) return;
    const root = surface.as(gtk.Widget).getRoot() orelse return;
    const window = gobject.ext.cast(@import("../class/window.zig").Window, root) orelse return;
    window.showSetupPasteReceipt();
}
pub fn childExited(surface: *Surface, code: u32) void {
    const self = from(surface) orelse return;
    if (self.finished) return;
    self.finished = true;
    Delivery.send(self.source, .{ .exited = code });
}
pub fn initializationFailed(surface: *Surface) void {
    const self = from(surface) orelse return;
    if (self.finished) return;
    self.finished = true;
    Delivery.send(self.source, .{ .started = "The native setup terminal failed to initialize." });
}

const Decision = struct {
    context: ?*anyopaque,
    callback: *const fn (?*anyopaque, bool) void,
    fn responded(dialog: *@import("adw").MessageDialog, response: [*:0]const u8, self: *Decision) callconv(.c) void {
        const context = self.context;
        const callback = self.callback;
        const accepted = std.mem.eql(u8, std.mem.span(response), "stop");
        dialog.as(gtk.Window).destroy();
        std.heap.c_allocator.destroy(self);
        callback(context, accepted);
    }
};
pub fn confirmClose(parent: ?*gtk.Window, context: ?*anyopaque, callback: *const fn (?*anyopaque, bool) void) void {
    var copy = loadCopy(std.heap.c_allocator) catch {
        callback(context, false);
        return;
    };
    defer copy.deinit();
    const allocator = std.heap.c_allocator;
    const title = allocator.dupeSentinel(u8, copy.value.close_title, 0) catch {
        callback(context, false);
        return;
    };
    defer allocator.free(title);
    const detail = allocator.dupeSentinel(u8, copy.value.close_detail, 0) catch {
        callback(context, false);
        return;
    };
    defer allocator.free(detail);
    const keep = allocator.dupeSentinel(u8, copy.value.keep, 0) catch {
        callback(context, false);
        return;
    };
    defer allocator.free(keep);
    const stop = allocator.dupeSentinel(u8, copy.value.stop, 0) catch {
        callback(context, false);
        return;
    };
    defer allocator.free(stop);
    const decision = allocator.create(Decision) catch {
        callback(context, false);
        return;
    };
    decision.* = .{ .context = context, .callback = callback };
    const dialog = @import("adw").MessageDialog.new(parent, title, detail);
    dialog.addResponse("keep", keep);
    dialog.addResponse("stop", stop);
    dialog.setDefaultResponse("keep");
    dialog.setCloseResponse("keep");
    dialog.setResponseAppearance("stop", .destructive);
    _ = @import("adw").MessageDialog.signals.response.connect(dialog, *Decision, Decision.responded, decision, .{});
    dialog.as(gtk.Window).present();
}
