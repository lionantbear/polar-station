//! Native facts and effects only; all edge and notification policy is shared Rust.
const std = @import("std");
const inbox = @import("inbox.zig");
const session = @import("session_runtime.zig");
const c = @cImport({ @cInclude("polar_station_desktop_client.h"); });
pub const Observation = struct { origin: inbox.Origin, state: session.AttentionState, freshness: session.SummaryFreshness, attention_since_ms: ?u64 };
pub const Notice = struct { workspace_id: []const u8, message_id: []const u8, count: usize };
pub const Output = struct { observe: []const inbox.Origin, notify: []const Notice };
pub const Engine = struct {
    handle: ?*c.AttentionHandle = null,
    pub fn deinit(self: *Engine) void { c.polar_station_attention_free(self.handle); self.handle = null; }
    pub fn dispatch(self: *Engine, allocator: std.mem.Allocator, input: anytype) !std.json.Parsed(Output) {
        if (self.handle == null) self.handle = c.polar_station_attention_new();
        const bytes = try std.json.Stringify.valueAlloc(allocator, input, .{ .emit_null_optional_fields = false });
        defer allocator.free(bytes);
        const result = c.polar_station_attention_dispatch(self.handle, bytes.ptr, bytes.len) orelse return error.InvalidAttentionInput;
        defer c.polar_station_desktop_string_free(result);
        return try std.json.parseFromSlice(Output, allocator, std.mem.span(result), .{ .allocate = .alloc_always });
    }
};
