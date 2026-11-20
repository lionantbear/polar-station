//! Native serialization only; Resource draft and confirmation policy is Rust.
const std = @import("std");
const resource = @import("resources.zig");
const c = @import("polar_station_c");
pub const Output = struct {
    view: struct {
        catalog: resource.Catalog,
        loaded: bool,
        dirty: bool,
        can_apply: bool,
        selected: ?[]const u8,
        confirmation: ?[]const u8,
        confirmation_title: []const u8,
        discard_label: []const u8,
        keep_label: []const u8,
        message: ?[]const u8,
    },
    effects: []const struct { type: []const u8, catalog: ?resource.Catalog = null },
};
pub const Adapter = struct {
    handle: ?*c.ResourceEditorHandle = null,
    pub fn deinit(self: *Adapter) void {
        if (self.handle) |handle| c.polar_station_resource_editor_free(handle);
        self.handle = null;
    }
    pub fn dispatch(self: *Adapter, allocator: std.mem.Allocator, input: anytype) ?std.json.Parsed(Output) {
        if (self.handle == null) self.handle = c.polar_station_resource_editor_new();
        const bytes = std.json.Stringify.valueAlloc(allocator, input, .{}) catch return null;
        defer allocator.free(bytes);
        const result = c.polar_station_resource_editor_dispatch(self.handle, bytes.ptr, bytes.len) orelse return null;
        defer c.polar_station_desktop_string_free(result);
        return std.json.parseFromSlice(Output, allocator, std.mem.span(result), .{ .allocate = .alloc_always }) catch null;
    }
};
