//! C ABI and JSON delivery for shared diagnostic state, without native workflow caches.
const std = @import("std");
const intent = @import("control_center/intent.zig");
const c = @import("polar_station_c");
pub const Effect = struct {
    setup: ?@import("tool_setup_surface.zig").Launch = null,
    setup_id: ?[]const u8 = null,
    type: []const u8,
    token: u64 = 0,
    request: ?intent.EditorRequest = null,
    action: ?std.json.Value = null,
    expected_revision: ?[]const u8 = null,
    machine: ?@import("control_center/model.zig").MachineConnection = null,
};
pub const Output = struct { projection: std.json.Value, busy: bool, message: ?[]const u8 = null, effects: []const Effect };
pub const Engine = struct {
    handle: ?*c.DiagnosticsHandle = null,
    pub fn deinit(self: *Engine) void { c.polar_station_diagnostics_free(self.handle); self.handle = null; }
    pub fn dispatch(self: *Engine, allocator: std.mem.Allocator, input: anytype) !std.json.Parsed(Output) {
        if (self.handle == null) self.handle = c.polar_station_diagnostics_new();
        const bytes = try std.json.Stringify.valueAlloc(allocator, input, .{ .emit_null_optional_fields = false });
        defer allocator.free(bytes);
        const result = c.polar_station_diagnostics_dispatch(self.handle, bytes.ptr, bytes.len) orelse return error.InvalidDiagnosticsInput;
        defer c.polar_station_desktop_string_free(result);
        return try std.json.parseFromSlice(Output, allocator, std.mem.span(result), .{ .allocate = .alloc_always, .ignore_unknown_fields = true });
    }
};
