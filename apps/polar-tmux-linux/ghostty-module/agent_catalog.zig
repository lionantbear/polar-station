//! JSON/C ABI only. Graph, search and family reference rules are shared Rust.
const std = @import("std");
const c = @cImport({ @cInclude("polar_station_desktop_client.h"); });
pub const Row = struct { root: usize, parent: ?usize, key: []const u8, group: []const u8, matches: bool, rank: ?usize };
pub fn project(allocator: std.mem.Allocator, input: anytype) !std.json.Parsed([]const Row) {
    const bytes = try std.json.Stringify.valueAlloc(allocator, input, .{ .emit_null_optional_fields = false });
    defer allocator.free(bytes);
    const result = c.polar_station_agent_catalog_project(bytes.ptr, bytes.len) orelse return error.InvalidCatalogInput;
    defer c.polar_station_desktop_string_free(result);
    return try std.json.parseFromSlice([]const Row, allocator, std.mem.span(result), .{ .allocate = .alloc_always });
}
