//! Local-only shared Rust policy. No catalog or window state is stored here.
const std = @import("std");
const c = @cImport({ @cInclude("polar_station_desktop_client.h"); });
pub const Decision = struct {
    effect: []const u8 = "none",
    selected: []const u8 = "",
    @"error": ?[]const u8 = null,
    can_archive: bool = false,
};
pub fn decide(allocator: std.mem.Allocator, input: anytype) !std.json.Parsed(Decision) {
    const bytes = try std.json.Stringify.valueAlloc(allocator, input, .{});
    defer allocator.free(bytes);
    const result = c.polar_station_workspace_lifecycle(bytes.ptr, bytes.len) orelse return error.InvalidLifecycleInput;
    defer c.polar_station_desktop_string_free(result);
    return std.json.parseFromSlice(Decision, allocator, std.mem.span(result), .{ .allocate = .alloc_always });
}
