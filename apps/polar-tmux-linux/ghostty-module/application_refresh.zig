//! Event-driven Control Center projection refresh coalescing.
//!
//! The managed Ghostty adapter never polls SSH, tmux, or Resources. The Rust
//! Host publishes a monotonic invalidation event; this gate permits one local
//! IPC projection request at a time and remembers only whether a newer
//! revision arrived while that request was in flight.

pub const Effect = enum {
    none,
    request_projection,
};

pub const Gate = struct {
    last_revision: u64 = 0,
    in_flight: bool = false,
    queued: bool = false,

    pub fn request(self: *Gate) Effect {
        if (self.in_flight) {
            self.queued = true;
            return .none;
        }
        self.in_flight = true;
        return .request_projection;
    }

    pub fn observe(self: *Gate, revision: u64) Effect {
        if (revision == 0 or revision <= self.last_revision) {
            return .none;
        }
        self.last_revision = revision;
        return self.request();
    }

    pub fn complete(self: *Gate) Effect {
        if (!self.in_flight) return .none;
        if (self.queued) {
            self.queued = false;
            return .request_projection;
        }
        self.in_flight = false;
        return .none;
    }

    pub fn failed(self: *Gate) void {
        self.in_flight = false;
        self.queued = false;
    }

    pub fn disconnected(self: *Gate) void {
        self.failed();
    }
};

// Native widget inputs, not a second product model. Exact byte comparison
// retains live GTK objects across semantically identical projection responses.
pub const RenderedInputs = struct {
    bytes: ?[]u8 = null,

    pub fn deinit(self: *RenderedInputs, allocator: @import("std").mem.Allocator) void {
        if (self.bytes) |bytes| allocator.free(bytes);
        self.* = .{};
    }

    pub fn changed(self: *RenderedInputs, allocator: @import("std").mem.Allocator, value: anytype) bool {
        const std = @import("std");
        const next = std.json.Stringify.valueAlloc(allocator, value, .{}) catch return true;
        if (self.bytes) |previous| {
            if (std.mem.eql(u8, previous, next)) {
                allocator.free(next);
                return false;
            }
            allocator.free(previous);
        }
        self.bytes = next;
        return true;
    }
};

test "equal displayed content retains widgets while order status and action changes invalidate" {
    const std = @import("std");
    const Inputs = struct { names: []const []const u8, online: bool, enabled: bool };
    var inputs: RenderedInputs = .{};
    defer inputs.deinit(std.testing.allocator);
    const original = Inputs{ .names = &.{ "one", "two" }, .online = true, .enabled = true };
    try std.testing.expect(inputs.changed(std.testing.allocator, original));
    const copy = try std.testing.allocator.dupe(u8, "one");
    defer std.testing.allocator.free(copy);
    try std.testing.expect(!inputs.changed(std.testing.allocator, Inputs{ .names = &.{ copy, "two" }, .online = true, .enabled = true }));
    try std.testing.expect(inputs.changed(std.testing.allocator, Inputs{ .names = &.{ "two", "one" }, .online = true, .enabled = true }));
    try std.testing.expect(inputs.changed(std.testing.allocator, Inputs{ .names = &.{ "two", "one" }, .online = false, .enabled = true }));
    try std.testing.expect(inputs.changed(std.testing.allocator, Inputs{ .names = &.{ "two", "one" }, .online = false, .enabled = false }));
}
