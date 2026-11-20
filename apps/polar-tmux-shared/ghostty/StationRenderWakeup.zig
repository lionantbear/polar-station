//! Wake visible renderers sharing a terminal's state from any producer thread.

const global = @import("../global.zig");
const RenderWakeup = @This();
const std = @import("std");
const xev = @import("xev").Dynamic;
const log = std.log.scoped(.station_render_wakeup);

// This lock covers the borrowed handles, including writes to them. A view
// can release its renderer immediately after detach returns.
mutex: std.Io.Mutex = .init,
visible: std.ArrayList(*xev.Async) = .empty,

pub fn deinit(self: *RenderWakeup, alloc: std.mem.Allocator) void {
    std.debug.assert(self.visible.items.len == 0);
    self.visible.deinit(alloc);
}

/// The caller owns the handle until it has detached it. Request a frame now
/// so the view can catch up even if the terminal stays idle after showing it.
pub fn attach(self: *RenderWakeup, alloc: std.mem.Allocator, wakeup: *xev.Async) !void {
    self.mutex.lockUncancelable(global.io());
    defer self.mutex.unlock(global.io());
    if (std.mem.indexOfScalar(*xev.Async, self.visible.items, wakeup) != null) return;
    try self.visible.append(alloc, wakeup);
    errdefer _ = self.visible.pop();
    try wakeup.notify();
}

pub fn detach(self: *RenderWakeup, wakeup: *xev.Async) void {
    self.mutex.lockUncancelable(global.io());
    defer self.mutex.unlock(global.io());
    if (std.mem.indexOfScalar(*xev.Async, self.visible.items, wakeup)) |idx| {
        _ = self.visible.swapRemove(idx);
    }
}

pub fn notify(self: *RenderWakeup) void {
    self.mutex.lockUncancelable(global.io());
    defer self.mutex.unlock(global.io());
    for (self.visible.items) |wakeup| {
        wakeup.notify() catch |err|
            log.warn("shared renderer wakeup failed: {}", .{err});
    }
}


