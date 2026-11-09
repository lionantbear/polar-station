//! Native GPU host loader. One library/engine lives for the frontend process.
const std = @import("std");
const gtk = @import("gtk");
const glib = @import("glib");
var library: ?std.DynLib = null;
const Create = *const fn ([*:0]const u8, [*:0]const u8, [*:0]const u8, *?*glib.Error) callconv(.c) ?*gtk.Widget;
var create_fn: ?Create = null;

pub fn create(allocator: std.mem.Allocator, view_id: []const u8) !*gtk.Widget {
    const root = std.process.getEnvVarOwned(allocator, "POLAR_STATION_GRAPHICS_RUNTIME") catch |err| switch (err) {
        error.EnvironmentVariableNotFound => blk: {
            const exe = try std.fs.selfExeDirPathAlloc(allocator);
            defer allocator.free(exe);
            break :blk try std.fs.path.join(allocator, &.{ exe, "graphics" });
        },
        else => return err,
    };
    defer allocator.free(root);
    if (library == null) {
        const path = try std.fs.path.join(allocator, &.{ root, "libpolar_graphics_gtk.so" });
        defer allocator.free(path);
        var loaded = try std.DynLib.open(path);
        errdefer loaded.close();
        create_fn = loaded.lookup(Create, "polar_graphics_gtk_create_view") orelse return error.GraphicsEntryMissing;
        library = loaded;
    }
    const project = try std.fs.path.joinZ(allocator, &.{ root, "project" });
    defer allocator.free(project);
    var failure: ?*glib.Error = null;
    const identity = try allocator.dupeZ(u8, view_id);
    defer allocator.free(identity);
    const widget = create_fn.?(project, "res://itree_workspace.tscn", identity, &failure);
    if (failure) |value| {
        std.log.err("Idea Tree graphics: {s}", .{value.f_message orelse "unavailable"});
        value.free();
    }
    return widget orelse error.GraphicsUnavailable;
}


pub fn shutdown() void {
    if (library) |*loaded| {
        const Shutdown = *const fn (*?*glib.Error) callconv(.c) c_int;
        const stop = loaded.lookup(Shutdown, "polar_graphics_gtk_shutdown") orelse return;
        var failure: ?*glib.Error = null;
        if (stop(&failure) == 0) {
            if (failure) |value| {
                std.log.err("Graphics shutdown: {s}", .{value.f_message orelse "unavailable"});
                value.free();
            }
        }
    }
}

pub fn retire(widget: *gtk.Widget) void {
    if (library) |*loaded| {
        const Retire = *const fn (*gtk.Widget) callconv(.c) void;
        const release = loaded.lookup(Retire, "polar_graphics_gtk_retire") orelse return;
        release(widget);
    }
}

pub const HostEvent = *const fn ([*:0]const u8, ?*anyopaque) callconv(.c) void;
pub fn subscribe(widget: *gtk.Widget, callback: ?HostEvent, context: ?*anyopaque) void {
    if (library) |*loaded| {
        const Subscribe = *const fn (*gtk.Widget, ?HostEvent, ?*anyopaque) callconv(.c) void;
        const call = loaded.lookup(Subscribe, "polar_graphics_gtk_subscribe") orelse return;
        call(widget, callback, context);
    }
}
pub fn command(widget: *gtk.Widget, action: [:0]const u8, query: [:0]const u8) void {
    if (library) |*loaded| {
        const Command = *const fn (*gtk.Widget, [*:0]const u8, [*:0]const u8) callconv(.c) void;
        const call = loaded.lookup(Command, "polar_graphics_gtk_command") orelse return;
        call(widget, action, query);
    }
}
