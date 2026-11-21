//! Raw terminal transport for Ghostty in the managed Station build.
const std = @import("std");
const global = @import("../global.zig");
const configpkg = @import("../config.zig");
const termio = @import("../termio.zig");
const renderer = @import("../renderer.zig");

const Self = @This();
const Handle = opaque {};
const log = std.log.scoped(.station_terminal);

extern fn polar_station_desktop_terminal_open_selected(
    [*:0]const u8,
    [*:0]const u8,
    ?[*:0]const u8,
    ?*const fn (?*anyopaque) callconv(.c) void,
    ?*anyopaque,
) ?*Handle;
extern fn polar_station_desktop_terminal_read(*Handle, [*]u8, usize) usize;
extern fn polar_station_desktop_terminal_write(*Handle, [*]const u8, usize) bool;
extern fn polar_station_desktop_terminal_viewport(*Handle, u16, u16, u16, u16, bool) bool;
extern fn polar_station_desktop_terminal_close(*Handle) void;

allocator: std.mem.Allocator,
config: [:0]const u8,
terminal_type: [:0]const u8,
locale: ?[:0]const u8,
handle: ?*Handle = null,
handle_lock: std.Io.Mutex = .init,
event: std.Io.Event = .unset,
stopping: std.atomic.Value(bool) = .init(false),
reader: ?std.Thread = null,
grid: renderer.GridSize = .{ .columns = 80, .rows = 24 },
pixels: renderer.ScreenSize = .{ .width = 0, .height = 0 },
focused: bool = false,

pub fn init(
    alloc: std.mem.Allocator,
    json_config: [:0]const u8,
    config: *const configpkg.Config,
    env: *const std.process.Environ.Map,
) !Self {
    const json = try alloc.dupeSentinel(u8, json_config, 0);
    errdefer alloc.free(json);
    const terminal_type: []const u8 = if (config.env.map.get("TERM")) |value|
        value
    else if (global.resourcesDir().host() != null)
        config.term
    else
        "xterm-256color";
    const term = try alloc.dupeSentinel(u8, terminal_type, 0);
    errdefer alloc.free(term);
    const locale: ?[:0]const u8 = locale: {
        for ([_][:0]const u8{ "LC_ALL", "LANG" }) |name| {
            const value: []const u8 = if (config.env.map.get(name)) |override|
                override
            else
                env.get(name) orelse continue;
            if (value.len == 0 or value.len > 128 or !std.unicode.utf8ValidateSlice(value)) continue;
            if (std.mem.trim(u8, value, " \t\r\n").len != value.len) continue;
            for (value) |byte| {
                if (std.ascii.isControl(byte)) break;
            } else break :locale try alloc.dupeSentinel(u8, value, 0);
        }
        break :locale null;
    };
    return .{
        .allocator = alloc,
        .config = json,
        .terminal_type = term,
        .locale = locale,
    };
}

pub fn threadEnter(self: *Self, io: *termio.Termio) !void {
    self.handle_lock.lockUncancelable(global.io());
    defer self.handle_lock.unlock(global.io());
    if (self.stopping.load(.acquire)) return error.StationTerminalClosing;
    const handle = polar_station_desktop_terminal_open_selected(
        self.config.ptr,
        self.terminal_type.ptr,
        if (self.locale) |value| value.ptr else null,
        wake,
        self,
    ) orelse
        return error.StationTerminalUnavailable;
    errdefer polar_station_desktop_terminal_close(handle);
    self.handle = handle;
    errdefer self.handle = null;
    self.reader = try std.Thread.spawn(.{}, readLoop, .{ self, io });
    self.reader.?.setName(global.io(), "station-reader") catch {};
}

pub fn stop(self: *Self) void {
    self.stopping.store(true, .release);
    self.event.set(global.io());
    self.handle_lock.lockUncancelable(global.io());
    const thread = self.reader;
    self.reader = null;
    self.handle_lock.unlock(global.io());
    if (thread) |reader| reader.join();
}

pub fn deinit(self: *Self) void {
    self.stop();
    if (self.handle) |handle| polar_station_desktop_terminal_close(handle);
    if (self.locale) |value| self.allocator.free(value);
    self.allocator.free(self.terminal_type);
    self.allocator.free(self.config);
}

fn wake(context: ?*anyopaque) callconv(.c) void {
    const self: *Self = @ptrCast(@alignCast(context orelse return));
    self.event.set(global.io());
}

fn readLoop(self: *Self, io: *termio.Termio) void {
    var bytes: [16 * 1024]u8 = undefined;
    while (true) {
        self.event.waitUncancelable(global.io());
        self.event.reset();
        while (!self.stopping.load(.acquire)) {
            self.handle_lock.lockUncancelable(global.io());
            const count = polar_station_desktop_terminal_read(self.handle.?, &bytes, bytes.len);
            self.handle_lock.unlock(global.io());
            if (count == 0) break;
            // Parsing can enqueue replies in the Termio mailbox. Its consumer
            // runs independently and remains live while this reader is joined.
            io.processOutput(bytes[0..count]);
            io.renderer_state.yieldToDemand(global.io());
        }
        if (self.stopping.load(.acquire)) return;
    }
}

pub fn resize(self: *Self, grid: renderer.GridSize, pixels: renderer.ScreenSize) void {
    self.grid = grid;
    self.pixels = pixels;
    self.publishViewport();
}

pub fn focus(self: *Self, focused: bool) void {
    self.focused = focused;
    self.publishViewport();
}

fn publishViewport(self: *Self) void {
    const handle = self.handle orelse return;
    self.handle_lock.lockUncancelable(global.io());
    defer self.handle_lock.unlock(global.io());
    _ = polar_station_desktop_terminal_viewport(
        handle,
        @intCast(@min(self.grid.columns, std.math.maxInt(u16))),
        @intCast(@min(self.grid.rows, std.math.maxInt(u16))),
        @intCast(@min(self.pixels.width, std.math.maxInt(u16))),
        @intCast(@min(self.pixels.height, std.math.maxInt(u16))),
        self.focused,
    );
}

pub fn write(self: *Self, bytes: []const u8, linefeed: bool) void {
    const handle = self.handle orelse return;
    self.handle_lock.lockUncancelable(global.io());
    defer self.handle_lock.unlock(global.io());
    if (!linefeed) {
        if (!send(handle, bytes)) log.warn("Station terminal input unavailable", .{});
        return;
    }
    var buffer: [16 * 1024]u8 = undefined;
    var count: usize = 0;
    for (bytes) |byte| {
        if (count >= buffer.len - 1) {
            if (!send(handle, buffer[0..count])) {
                log.warn("Station terminal input unavailable", .{});
                return;
            }
            count = 0;
        }
        buffer[count] = byte;
        count += 1;
        if (byte == '\r') {
            buffer[count] = '\n';
            count += 1;
        }
    }
    if (count > 0) {
        if (!send(handle, buffer[0..count])) {
            log.warn("Station terminal input unavailable", .{});
        }
    }
}

fn send(handle: *Handle, bytes: []const u8) bool {
    var offset: usize = 0;
    while (offset < bytes.len) {
        const chunk = bytes[offset..][0..@min(bytes.len - offset, 16 * 1024)];
        if (!polar_station_desktop_terminal_write(handle, chunk.ptr, chunk.len)) return false;
        offset += chunk.len;
    }
    return true;
}
