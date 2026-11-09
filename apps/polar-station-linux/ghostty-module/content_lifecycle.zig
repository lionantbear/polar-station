//! Process-lifetime identity and retirement ownership for Linux Pane content.
//!
//! A managed Ghostty application can outlive every one of its windows. Channel
//! and request sequences therefore live beside the application Executor, not
//! inside a window-scoped AppliedRenderState. Failed retirement leases retain
//! their exact socket and protocol identity so a later render can retry them.

const std = @import("std");

pub const Channel = struct {
    channel_id: []u8,
    channel_generation: u64,
    control_socket: []u8,

    pub fn deinit(self: *Channel, allocator: std.mem.Allocator) void {
        allocator.free(self.channel_id);
        allocator.free(self.control_socket);
        self.* = undefined;
    }
};

pub const RetirementTarget = struct {
    channel_id: []const u8,
    channel_generation: u64,
    content_revision: u64,
    control_socket: []const u8,
};

pub const RetirementLease = struct {
    lease_id: u64,
    request_id: []u8,
    channel_id: []u8,
    channel_generation: u64,
    content_revision: u64,
    control_socket: []u8,
    attempt_count: u32 = 0,
    in_flight: bool = false,
};

pub const RetirementFailureDisposition = enum {
    already_retired,
    retry,
};

/// A missing exact socket cannot have an active owner and is therefore the
/// idempotent terminal state of a retirement whose native Surface has already
/// stopped its Pane host. Other transport failures may still be serviceable.
pub fn classifyRetirementTransportFailure(
    failure: anyerror,
) RetirementFailureDisposition {
    return switch (failure) {
        error.FileNotFound => .already_retired,
        else => .retry,
    };
}

pub const Lifecycle = struct {
    allocator: std.mem.Allocator,
    process_id: u32,
    run_nonce: u64,
    next_channel_sequence: u64 = 1,
    next_request_sequence: u64 = 1,
    next_lease_sequence: u64 = 1,
    retirements: std.ArrayListUnmanaged(RetirementLease) = .empty,
    submit_context: ?*anyopaque = null,
    submit_native_state: ?*const fn (?*anyopaque, []const u8) anyerror!void = null,
    submit_content: ?*const fn (?*anyopaque, []const u8, []const u8) anyerror!void = null,

    pub fn submit(self: *Lifecycle, socket_path: []const u8, request: []const u8) !void {
        const callback = self.submit_content orelse return error.ContentWorkerUnavailable;
        try callback(self.submit_context, socket_path, request);
    }

    pub fn markRetirementInFlight(self: *Lifecycle, lease_id: u64, in_flight: bool) void {
        for (self.retirements.items) |*lease| {
            if (lease.lease_id == lease_id) {
                lease.in_flight = in_flight;
                return;
            }
        }
    }

    pub fn init(
        allocator: std.mem.Allocator,
        process_id: u32,
        process_identity: []const u8,
    ) Lifecycle {
        const digest = std.hash.Wyhash.hash(
            0,
            process_identity,
        );
        return .{
            .allocator = allocator,
            .process_id = process_id,
            .run_nonce = if (digest == 0) 1 else digest,
        };
    }

    pub fn deinit(self: *Lifecycle) void {
        while (self.retirements.items.len > 0) {
            self.freeRetirement(self.retirements.orderedRemove(
                self.retirements.items.len - 1,
            ));
        }
        self.retirements.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn submitOperation(self: *Lifecycle, socket: []const u8, channel: []const u8, generation: u64, operation: []const u8) !void {
        const allocator = self.allocator;
        const request_id = try self.allocateRequestId(allocator);
        defer allocator.free(request_id);
        const json = try std.json.Stringify.valueAlloc(allocator, .{
            .protocol_version = @as(u32, 1), .api = "pane_content_v1", .request_id = request_id,
            .channel_id = channel, .channel_generation = generation, .content_revision = @as(u64, 1),
            .command = .{ .operation = operation },
        }, .{});
        defer allocator.free(json);
        try self.submit(socket, json);
    }


    /// Allocate an identity that cannot repeat during this application run,
    /// even when every managed window has been closed and reconstructed.
    pub fn allocateChannel(
        self: *Lifecycle,
        allocator: std.mem.Allocator,
        runtime_dir: []const u8,
    ) !Channel {
        const sequence = takeSequence(&self.next_channel_sequence);
        const channel_id = try std.fmt.allocPrint(
            allocator,
            "linux-surface-{d}-{x}-{d}",
            .{ self.process_id, self.run_nonce, sequence },
        );
        errdefer allocator.free(channel_id);
        const socket_name = try std.fmt.allocPrint(
            allocator,
            "pane-{d}-{x}-{d}.sock",
            .{ self.process_id, self.run_nonce, sequence },
        );
        defer allocator.free(socket_name);
        return .{
            .channel_id = channel_id,
            .channel_generation = 1,
            .control_socket = try std.fs.path.join(
                allocator,
                &.{ runtime_dir, "polar-tmux", socket_name },
            ),
        };
    }

    pub fn allocateRequestId(
        self: *Lifecycle,
        allocator: std.mem.Allocator,
    ) ![]u8 {
        const sequence = takeSequence(&self.next_request_sequence);
        return std.fmt.allocPrint(
            allocator,
            "linux-content-{d}-{x}-{d}",
            .{ self.process_id, self.run_nonce, sequence },
        );
    }

    /// Retain the exact protocol and socket identity before native ownership is
    /// released. Re-registering the same channel returns the existing lease.
    pub fn beginRetirement(
        self: *Lifecycle,
        target: RetirementTarget,
    ) !u64 {
        if (target.channel_generation == 0 or target.content_revision == 0) {
            return error.InvalidRetirementIdentity;
        }
        for (self.retirements.items) |lease| {
            if (!std.mem.eql(u8, lease.channel_id, target.channel_id) or
                lease.channel_generation != target.channel_generation)
            {
                continue;
            }
            if (!std.mem.eql(u8, lease.control_socket, target.control_socket) or
                lease.content_revision != target.content_revision)
            {
                return error.ConflictingRetirementIdentity;
            }
            return lease.lease_id;
        }

        const lease_id = takeSequence(&self.next_lease_sequence);
        const request_id = try self.allocateRequestId(self.allocator);
        errdefer self.allocator.free(request_id);
        const channel_id = try self.allocator.dupe(u8, target.channel_id);
        errdefer self.allocator.free(channel_id);
        const control_socket = try self.allocator.dupe(
            u8,
            target.control_socket,
        );
        errdefer self.allocator.free(control_socket);
        try self.retirements.append(self.allocator, .{
            .lease_id = lease_id,
            .request_id = request_id,
            .channel_id = channel_id,
            .channel_generation = target.channel_generation,
            .content_revision = target.content_revision,
            .control_socket = control_socket,
        });
        return lease_id;
    }

    pub fn retirement(self: *const Lifecycle, lease_id: u64) ?*const RetirementLease {
        for (self.retirements.items) |*lease| {
            if (lease.lease_id == lease_id) return lease;
        }
        return null;
    }

    pub fn retirementAt(
        self: *const Lifecycle,
        index: usize,
    ) ?*const RetirementLease {
        if (index >= self.retirements.items.len) return null;
        return &self.retirements.items[index];
    }

    pub fn markRetirementAttempt(self: *Lifecycle, lease_id: u64) void {
        for (self.retirements.items) |*lease| {
            if (lease.lease_id != lease_id) continue;
            lease.attempt_count +|= 1;
            return;
        }
    }

    /// Release a lease only after the caller has validated the exact Detached
    /// response and completed the matching native Surface close boundary.
    pub fn completeRetirement(self: *Lifecycle, lease_id: u64) bool {
        for (self.retirements.items, 0..) |lease, index| {
            if (lease.lease_id != lease_id) continue;
            self.freeRetirement(self.retirements.orderedRemove(index));
            return true;
        }
        return false;
    }

    pub fn pendingRetirementCount(self: *const Lifecycle) usize {
        return self.retirements.items.len;
    }

    fn freeRetirement(self: *Lifecycle, lease: RetirementLease) void {
        self.allocator.free(lease.request_id);
        self.allocator.free(lease.channel_id);
        self.allocator.free(lease.control_socket);
    }
};

fn takeSequence(next: *u64) u64 {
    const value = next.*;
    next.* +%= 1;
    if (next.* == 0) next.* = 1;
    return value;
}






