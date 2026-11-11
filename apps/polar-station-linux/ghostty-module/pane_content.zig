//! Content-only wire contract between Ghostty GTK Surfaces and Rust pane-host.
//!
//! This module deliberately has no Workspace, Tab, Pane order, mode, slot,
//! ratio, or tree types. The GTK adapter owns every one of those concepts.

const std = @import("std");

pub const Binding = struct {
    execution_target: []const u8,
    tmux_session_id: []const u8,
};

pub const Identity = struct {
    request_id: []const u8,
    channel_id: []const u8,
    channel_generation: u64,
    content_revision: u64,

    pub fn validate(self: Identity) Error!void {
        try validateIdentifier(self.request_id);
        try validateIdentifier(self.channel_id);
        if (self.channel_generation == 0) return error.InvalidGeneration;
        if (self.content_revision == 0) return error.InvalidRevision;
    }
};

pub const ResultKind = enum { accepted, ready, detached, rejected };

pub const Outcome = struct {
    kind: ResultKind,
    binding: ?OwnedBinding = null,
    error_code: ?[]u8 = null,
    error_message: ?[]u8 = null,
    retryable: bool = false,

    pub const OwnedBinding = struct {
        execution_target: []u8,
        tmux_session_id: []u8,
    };

    pub fn deinit(self: *Outcome, allocator: std.mem.Allocator) void {
        if (self.binding) |binding| {
            allocator.free(binding.execution_target);
            allocator.free(binding.tmux_session_id);
        }
        if (self.error_code) |value| allocator.free(value);
        if (self.error_message) |value| allocator.free(value);
        self.* = undefined;
    }
};

pub const Error = error{
    InvalidIdentifier,
    InvalidGeneration,
    InvalidRevision,
    InvalidBinding,
    InvalidResponse,
    ResponseIdentityMismatch,
    EmptyPayload,
    PayloadTooLarge,
    TruncatedResponse,
};

const max_payload_bytes: usize = 1024 * 1024;

const Api = enum { pane_content_v1 };
const BindOperation = enum { bind };
const DetachOperation = enum { detach };
const RetireOperation = enum { retire };
const ReconnectOperation = enum { reconnect };

const BindCommand = struct {
    operation: BindOperation = .bind,
    binding: Binding,
};
const DetachCommand = struct {
    operation: DetachOperation = .detach,
};
const RetireCommand = struct {
    operation: RetireOperation = .retire,
};
const ReconnectCommand = struct {
    operation: ReconnectOperation = .reconnect,
    binding: Binding,
};

fn Request(comptime Command: type) type {
    return struct {
        protocol_version: u32 = 1,
        api: Api = .pane_content_v1,
        request_id: []const u8,
        channel_id: []const u8,
        channel_generation: u64,
        content_revision: u64,
        command: Command,
    };
}

const ResponseBinding = struct {
    execution_target: []const u8,
    tmux_session_id: []const u8,
};
const ResponseError = struct {
    code: []const u8,
    message: []const u8,
    retryable: bool,
};
const ResponseResult = struct {
    status: ResultKind,
    binding: ?ResponseBinding = null,
    @"error": ?ResponseError = null,
};
const Response = struct {
    protocol_version: u32,
    api: Api,
    request_id: []const u8,
    channel_id: []const u8,
    channel_generation: u64,
    content_revision: u64,
    result: ResponseResult,
};

pub fn encodeBind(
    allocator: std.mem.Allocator,
    identity: Identity,
    binding: Binding,
) ![]u8 {
    try identity.validate();
    try validateBinding(binding);
    return std.json.Stringify.valueAlloc(
        allocator,
        Request(BindCommand){
            .request_id = identity.request_id,
            .channel_id = identity.channel_id,
            .channel_generation = identity.channel_generation,
            .content_revision = identity.content_revision,
            .command = .{ .binding = binding },
        },
        .{ .whitespace = .minified },
    );
}

pub fn encodeBindFrame(
    allocator: std.mem.Allocator,
    identity: Identity,
    binding: Binding,
) ![]u8 {
    const payload = try encodeBind(allocator, identity, binding);
    defer allocator.free(payload);
    if (payload.len == 0) return error.EmptyPayload;
    if (payload.len > max_payload_bytes) return error.PayloadTooLarge;
    const frame = try allocator.alloc(u8, payload.len + 4);
    std.mem.writeInt(u32, frame[0..4], @intCast(payload.len), .big);
    @memcpy(frame[4..], payload);
    return frame;
}

pub fn encodeReconnectFrame(
    allocator: std.mem.Allocator,
    identity: Identity,
    binding: Binding,
) ![]u8 {
    const payload = try encodeReconnect(allocator, identity, binding);
    defer allocator.free(payload);
    if (payload.len == 0) return error.EmptyPayload;
    if (payload.len > max_payload_bytes) return error.PayloadTooLarge;
    const frame = try allocator.alloc(u8, payload.len + 4);
    std.mem.writeInt(u32, frame[0..4], @intCast(payload.len), .big);
    @memcpy(frame[4..], payload);
    return frame;
}

pub fn decodeFrameLength(prefix: [4]u8) Error!usize {
    const length: usize = std.mem.readInt(u32, &prefix, .big);
    if (length == 0) return error.EmptyPayload;
    if (length > max_payload_bytes) return error.PayloadTooLarge;
    return length;
}


pub fn encodeDetach(
    allocator: std.mem.Allocator,
    identity: Identity,
) ![]u8 {
    try identity.validate();
    return std.json.Stringify.valueAlloc(
        allocator,
        Request(DetachCommand){
            .request_id = identity.request_id,
            .channel_id = identity.channel_id,
            .channel_generation = identity.channel_generation,
            .content_revision = identity.content_revision,
            .command = .{},
        },
        .{ .whitespace = .minified },
    );
}

pub fn encodeRetire(
    allocator: std.mem.Allocator,
    identity: Identity,
) ![]u8 {
    try identity.validate();
    return std.json.Stringify.valueAlloc(
        allocator,
        Request(RetireCommand){
            .request_id = identity.request_id,
            .channel_id = identity.channel_id,
            .channel_generation = identity.channel_generation,
            .content_revision = identity.content_revision,
            .command = .{},
        },
        .{ .whitespace = .minified },
    );
}

pub fn encodeRetireFrame(
    allocator: std.mem.Allocator,
    identity: Identity,
) ![]u8 {
    const payload = try encodeRetire(allocator, identity);
    defer allocator.free(payload);
    if (payload.len == 0) return error.EmptyPayload;
    if (payload.len > max_payload_bytes) return error.PayloadTooLarge;
    const frame = try allocator.alloc(u8, payload.len + 4);
    std.mem.writeInt(u32, frame[0..4], @intCast(payload.len), .big);
    @memcpy(frame[4..], payload);
    return frame;
}


pub fn encodeReconnect(
    allocator: std.mem.Allocator,
    identity: Identity,
    binding: Binding,
) ![]u8 {
    try identity.validate();
    try validateBinding(binding);
    return std.json.Stringify.valueAlloc(
        allocator,
        Request(ReconnectCommand){
            .request_id = identity.request_id,
            .channel_id = identity.channel_id,
            .channel_generation = identity.channel_generation,
            .content_revision = identity.content_revision,
            .command = .{ .binding = binding },
        },
        .{ .whitespace = .minified },
    );
}

pub fn decodeResponse(
    allocator: std.mem.Allocator,
    json: []const u8,
    expected: Identity,
) !Outcome {
    try expected.validate();
    var parsed = try std.json.parseFromSlice(
        Response,
        allocator,
        json,
        .{ .ignore_unknown_fields = false },
    );
    defer parsed.deinit();
    const response = parsed.value;
    if (response.protocol_version != 1 or
        response.api != .pane_content_v1 or
        !std.mem.eql(u8, response.request_id, expected.request_id) or
        !std.mem.eql(u8, response.channel_id, expected.channel_id) or
        response.channel_generation != expected.channel_generation or
        response.content_revision != expected.content_revision)
    {
        return error.ResponseIdentityMismatch;
    }

    return switch (response.result.status) {
        .accepted, .ready => ready: {
            const binding = response.result.binding orelse
                return error.InvalidResponse;
            if (response.result.@"error" != null) return error.InvalidResponse;
            try validateBinding(.{
                .execution_target = binding.execution_target,
                .tmux_session_id = binding.tmux_session_id,
            });
            const execution_target = try allocator.dupe(
                u8,
                binding.execution_target,
            );
            errdefer allocator.free(execution_target);
            break :ready .{
                .kind = response.result.status,
                .binding = .{
                    .execution_target = execution_target,
                    .tmux_session_id = try allocator.dupe(
                        u8,
                        binding.tmux_session_id,
                    ),
                },
            };
        },
        .detached => if (response.result.binding == null and
            response.result.@"error" == null)
            .{ .kind = .detached }
        else
            error.InvalidResponse,
        .rejected => rejected: {
            const value = response.result.@"error" orelse
                return error.InvalidResponse;
            if (response.result.binding != null or
                !validIdentifier(value.code) or
                value.message.len == 0 or
                value.message.len > 512 or
                containsControl(value.message))
            {
                return error.InvalidResponse;
            }
            const code = try allocator.dupe(u8, value.code);
            errdefer allocator.free(code);
            break :rejected .{
                .kind = .rejected,
                .error_code = code,
                .error_message = try allocator.dupe(u8, value.message),
                .retryable = value.retryable,
            };
        },
    };
}

fn validateBinding(binding: Binding) Error!void {
    if (!validIdentifier(binding.execution_target) or
        !validIdentifier(binding.tmux_session_id))
    {
        return error.InvalidBinding;
    }
}

fn validateIdentifier(value: []const u8) Error!void {
    if (!validIdentifier(value)) return error.InvalidIdentifier;
}

fn validIdentifier(value: []const u8) bool {
    return value.len > 0 and value.len <= 256 and
        std.mem.eql(u8, value, std.mem.trim(u8, value, " \t\r\n")) and
        !containsControl(value);
}

fn containsControl(value: []const u8) bool {
    for (value) |byte| {
        if (std.ascii.isControl(byte)) return true;
    }
    return false;
}










