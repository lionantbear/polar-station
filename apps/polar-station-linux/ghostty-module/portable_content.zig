//! Scalar mirror of the shared terminal-source contract; no connection policy.
const std = @import("std");
pub const HostRef = struct { catalog_id: []const u8, target_id: []const u8 };
pub const Session = struct { server_generation: []const u8, session_id: []const u8, created_unix_seconds: i64 };
pub const Source = struct {
    host: HostRef,
    session: ?Session = null,
    original_agent_id: ?[]const u8 = null,

    pub fn clone(self: Source, allocator: std.mem.Allocator) !Source {
        const bytes = try std.json.Stringify.valueAlloc(allocator, self, .{});
        defer allocator.free(bytes);
        return std.json.parseFromSliceLeaky(Source, allocator, bytes, .{ .allocate = .alloc_always });
    }
};
