//! Wake visible renderers sharing a terminal's state from any producer thread.

const global = @import("../global.zig");
const RenderWakeup = @This();
const std = @import("std");
const xev = @import("xev").Dynamic;
const RendererMailbox = @import("../renderer.zig").Thread.Mailbox;
const log = std.log.scoped(.station_render_wakeup);

const Target = struct {
    wake: *xev.Async,
    mailbox: ?*RendererMailbox,
};

// This lock covers the borrowed handles, including writes to them. A view
// can release its renderer immediately after detach returns.
mutex: std.Io.Mutex = .init,
visible: std.ArrayList(Target) = .empty,
has_visible: std.atomic.Value(bool) = .init(false),

pub fn deinit(self: *RenderWakeup, alloc: std.mem.Allocator) void {
    std.debug.assert(self.visible.items.len == 0);
    self.visible.deinit(alloc);
}

/// The caller owns the handle until it has detached it. Request a frame now
/// so the view can catch up even if the terminal stays idle after showing it.
pub fn attach(self: *RenderWakeup, alloc: std.mem.Allocator, wakeup: *xev.Async, mailbox: ?*RendererMailbox) !void {
    self.mutex.lockUncancelable(global.io());
    defer self.mutex.unlock(global.io());
    for (self.visible.items) |view| if (view.wake == wakeup) return;
    try self.visible.append(alloc, .{ .wake = wakeup, .mailbox = mailbox });
    self.has_visible.store(true, .release);
    errdefer {
        _ = self.visible.pop();
        if (self.visible.items.len == 0) self.has_visible.store(false, .release);
    }
    try wakeup.notify();
}

pub fn detach(self: *RenderWakeup, wakeup: *xev.Async) void {
    self.mutex.lockUncancelable(global.io());
    defer self.mutex.unlock(global.io());
    for (self.visible.items, 0..) |view, idx| {
        if (view.wake != wakeup) continue;
        _ = self.visible.swapRemove(idx);
        if (self.visible.items.len == 0) self.has_visible.store(false, .release);
        return;
    }
}

pub fn notify(self: *RenderWakeup) void {
    if (!self.has_visible.load(.acquire)) return;
    self.mutex.lockUncancelable(global.io());
    defer self.mutex.unlock(global.io());
    for (self.visible.items) |view| {
        view.wake.notify() catch |err|
            log.warn("shared renderer wakeup failed: {}", .{err});
    }
}

pub fn resetCursor(self: *RenderWakeup) void {
    if (!self.hasVisible()) return;
    self.mutex.lockUncancelable(global.io());
    defer self.mutex.unlock(global.io());
    for (self.visible.items) |view| {
        const mailbox = view.mailbox orelse continue;
        _ = mailbox.push(global.io(), .{ .reset_cursor_blink = {} }, .{ .instant = {} });
    }
}

pub fn hasVisible(self: *const RenderWakeup) bool {
    return self.has_visible.load(.acquire);
}


