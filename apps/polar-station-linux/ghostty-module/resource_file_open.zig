//! JSON/FFI transport only; Resource Rust owns pending requests and decisions.
const std = @import("std");
const c = @cImport({ @cInclude("polar_station_desktop_client.h"); });
pub const Adapter = struct {
    handle: ?*c.ResourceFileOpenHandle = null,
    pub fn deinit(self: *Adapter) void {
        if (self.handle) |handle| c.polar_station_resource_file_open_free(handle);
        self.handle = null;
    }
    pub fn dispatch(self: *Adapter, allocator: std.mem.Allocator, input: anytype) ?std.json.Parsed(std.json.Value) {
        if (self.handle == null) self.handle = c.polar_station_resource_file_open_new();
        const bytes = std.json.Stringify.valueAlloc(allocator, input, .{}) catch return null;
        defer allocator.free(bytes);
        const result = c.polar_station_resource_file_open_dispatch(self.handle, bytes.ptr, bytes.len) orelse return null;
        defer c.polar_station_desktop_string_free(result);
        return std.json.parseFromSlice(std.json.Value, allocator, std.mem.span(result), .{ .allocate = .alloc_always }) catch null;
    }
};


