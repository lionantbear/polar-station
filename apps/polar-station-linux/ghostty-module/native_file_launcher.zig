//! GTK system file opening. The shared Resource flow owns request semantics.
const std = @import("std");
const gtk = @import("gtk");
const c = @import("gtk_c");

pub const Slot = enum { terminal, resources };
pub const Callback = *const fn (?*anyopaque, []const u8, ?[]const u8, bool) void;
const Operation = struct {
    slot: Slot,
    id: []u8,
    context: ?*anyopaque,
    callback: Callback,
    cancellable: *c.GCancellable,
    reveal: bool,
};
var pending: [2]?*Operation = .{ null, null };

pub fn cancel(slot: Slot) void {
    const index = @intFromEnum(slot);
    if (pending[index]) |op| {
        pending[index] = null;
        c.g_cancellable_cancel(op.cancellable);
    }
}

pub fn start(slot: Slot, parent: *gtk.Window, path: []const u8, reveal: bool, id: []const u8, context: ?*anyopaque, callback: Callback) !void {
    cancel(slot);
    const allocator = std.heap.c_allocator;
    const op = try allocator.create(Operation);
    errdefer allocator.destroy(op);
    const copy = try allocator.dupe(u8, id);
    errdefer allocator.free(copy);
    const path_z = try allocator.dupeSentinel(u8, path, 0);
    defer allocator.free(path_z);
    const file = c.g_file_new_for_path(path_z.ptr) orelse return error.FileCreationFailed;
    defer c.g_object_unref(file);
    const launcher = c.gtk_file_launcher_new(file) orelse return error.LauncherCreationFailed;
    defer c.g_object_unref(launcher);
    const cancellable = c.g_cancellable_new() orelse return error.CancellableCreationFailed;
    op.* = .{ .slot = slot, .id = copy, .context = context, .callback = callback, .cancellable = cancellable, .reveal = reveal };
    pending[@intFromEnum(slot)] = op;
    const window: *c.GtkWindow = @ptrCast(parent);
    if (reveal) c.gtk_file_launcher_open_containing_folder(launcher, window, cancellable, finished, op)
    else c.gtk_file_launcher_launch(launcher, window, cancellable, finished, op);
}

fn finished(source: ?*c.GObject, result: ?*c.GAsyncResult, data: ?*anyopaque) callconv(.c) void {
    const op: *Operation = @ptrCast(@alignCast(data orelse return));
    const allocator = std.heap.c_allocator;
    defer allocator.destroy(op);
    defer allocator.free(op.id);
    defer c.g_object_unref(op.cancellable);
    var failure: ?*c.GError = null;
    const launcher: *c.GtkFileLauncher = @ptrCast(@alignCast(source orelse return));
    const ok = if (op.reveal) c.gtk_file_launcher_open_containing_folder_finish(launcher, result, &failure) else c.gtk_file_launcher_launch_finish(launcher, result, &failure);
    defer if (failure) |err| c.g_error_free(err);
    const index = @intFromEnum(op.slot);
    if (pending[index] != op) return;
    pending[index] = null;
    const cancelled = if (failure) |err| c.g_error_matches(err, c.g_io_error_quark(), c.G_IO_ERROR_CANCELLED) != 0 or c.g_error_matches(err, c.gtk_dialog_error_quark(), c.GTK_DIALOG_ERROR_CANCELLED) != 0 or c.g_error_matches(err, c.gtk_dialog_error_quark(), c.GTK_DIALOG_ERROR_DISMISSED) != 0 else false;
    const message: ?[]const u8 = if (failure) |err| std.mem.span(err.message) else if (ok == 0) "The system file launcher failed." else null;
    op.callback(op.context, op.id, message, cancelled);
}
