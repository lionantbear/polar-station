const std = @import("std");
const control_model = @import("control_center/model.zig");

pub const FrontendIdentity = struct {
    frontend_instance_id: []const u8,
    frontend_generation: u64,

    pub fn validate(self: FrontendIdentity) Error!void {
        try validateIdentifier(self.frontend_instance_id);
        if (self.frontend_generation == 0) return error.InvalidGeneration;
    }
};

pub const HostFrameKind = enum {
    registered,
    response,
    application_response,
    application_state_changed,
    session_feed_snapshot,
    session_feed_event,
    workspace_relay,
    frontend_execute,
    ping,
};

pub const Error = error{
    InvalidIdentifier,
    InvalidGeneration,
    InvalidFrame,
    UnsupportedFrame,
    StaleFrontend,
    OutOfMemory,
    SyntaxError,
    UnexpectedToken,
    InvalidNumber,
    Overflow,
    UnknownField,
    MissingField,
    LengthMismatch,
    DuplicateField,
};

const Registration = struct {
    type: []const u8 = "register",
    frontend_id: []const u8 = "linux-ghostty",
    extensions: [1]ExtensionOffer = .{.{}},
    platform: []const u8 = "linux",
    host_control_versions: HostControlVersions = .{ .codex = 2, .notification = 1 },
    supported_actions: []const []const u8 = &.{"activate"},
    capabilities: []const []const u8 = &.{
        "application_state_events_v1",
        "session_feed_v1",
        "workspace_relay_v1",
        "session_lifecycle_v2",
    },
};

const ExtensionOffer = struct {
    name: []const u8 = "polar_access",
    version: u16 = 1,
    max_payload_bytes: u32 = 64 * 1024,
};

const HostFrameHeader = struct {
    type: []const u8,
};

pub const RegisteredFrame = struct {
    type: []const u8,
    registration_id: []const u8,
    protocol_version: u32,
    host_id: []const u8,
    host_control_versions: ?HostControlVersions = null,
    extensions: []const ExtensionOffer = &.{},

    pub fn supportsPolarAccess(self: RegisteredFrame) bool {
        for (self.extensions) |extension| {
            if (std.mem.eql(u8, extension.name, "polar_access") and extension.version == 1) return true;
        }
        return false;
    }
};

pub const HostControlVersions = struct {
    desktop: ?u16 = null,
    codex: ?u16 = null,
    notification: ?u16 = null,
};

const Pong = struct {
    type: []const u8 = "pong",
    nonce: u64,
};

const PingFrame = struct {
    type: []const u8,
    nonce: u64,
};

pub const ApplicationStateApi = enum {
    codex_v1,
    notification_v1,
    control_center_v1,
    resources_v1,
    settings_v1,
    inbox_v1,
};

pub const ApplicationStateChange = struct {
    api: ApplicationStateApi,
    state_revision: u64,
};

const ApplicationStateFrame = struct {
    type: []const u8,
    api: ApplicationStateApi,
    state_revision: u64,
};

const ApplicationResponseHeader = struct {
    type: []const u8,
    response: struct {
        api: []const u8,
        request_id: []const u8,
        result: struct {
            type: []const u8,
            @"error": ?control_model.ErrorProjection = null,
            session: ?control_model.SessionIdentity = null,
            items: []const SessionCreationItem = &.{},
        },
    },
};

pub const ApplicationResponseApi = enum {
    codex_v1,
    notification_v1,
    control_center_v1,
    resources_v1,
    settings_v1,
    inbox_v1,
};

const ApplicationResponseApiFrame = struct {
    type: []const u8,
    response: struct { api: ApplicationResponseApi },
};

pub fn decodeApplicationResponseApi(
    allocator: std.mem.Allocator,
    json: []const u8,
) !ApplicationResponseApi {
    var parsed = try std.json.parseFromSlice(
        ApplicationResponseApiFrame,
        allocator,
        json,
        .{ .ignore_unknown_fields = true },
    );
    defer parsed.deinit();
    if (!std.mem.eql(u8, parsed.value.type, "application_response")) {
        return error.InvalidFrame;
    }
    return parsed.value.response.api;
}

pub const ControlCenterResponseKind = enum {
    projection,
    intent_accepted,
    session_created,
    session_batch_created,
    session_editor,
    structure_accepted,
    structure_conflict,
    @"error",

    pub fn accepted(self: ControlCenterResponseKind) bool {
        return self == .session_editor or self == .intent_accepted or self == .session_created or self == .session_batch_created or self == .structure_accepted;
    }
};

pub const SessionCreationItem = struct {
    thread_id: []const u8,
    session: ?control_model.SessionIdentity = null,
    @"error": ?control_model.ErrorProjection = null,
};
pub const ControlCenterResponseOutcome = struct {
    request_id: []u8,
    kind: ControlCenterResponseKind,
    error_message: ?[]u8 = null,
    session: ?control_model.SessionIdentity = null,
    batch: ?std.json.Parsed([]const SessionCreationItem) = null,

    pub fn deinit(
        self: *ControlCenterResponseOutcome,
        allocator: std.mem.Allocator,
    ) void {
        allocator.free(self.request_id);
        if (self.batch) |*batch| batch.deinit();
        if (self.error_message) |message| allocator.free(message);
        if (self.session) |session| {
            allocator.free(session.execution_target_id);
            allocator.free(session.tmux_session_id);
        }
        self.* = undefined;
    }
};

pub fn encodeRegistration(
    allocator: std.mem.Allocator,
    identity: FrontendIdentity,
) ![]u8 {
    try identity.validate();
    return std.json.Stringify.valueAlloc(
        allocator,
        Registration{},
        .{ .whitespace = .minified, .emit_null_optional_fields = false },
    );
}

pub fn classifyHostFrame(
    allocator: std.mem.Allocator,
    json: []const u8,
    identity: FrontendIdentity,
) !HostFrameKind {
    try identity.validate();
    var parsed = try std.json.parseFromSlice(
        HostFrameHeader,
        allocator,
        json,
        .{ .ignore_unknown_fields = true },
    );
    defer parsed.deinit();

    return std.meta.stringToEnum(HostFrameKind, parsed.value.type) orelse
        error.UnsupportedFrame;
}

pub fn encodePong(allocator: std.mem.Allocator, nonce: u64) ![]u8 {
    return std.json.Stringify.valueAlloc(
        allocator,
        Pong{ .nonce = nonce },
        .{ .whitespace = .minified },
    );
}

pub fn decodePingNonce(allocator: std.mem.Allocator, json: []const u8) !u64 {
    var parsed = try std.json.parseFromSlice(
        PingFrame,
        allocator,
        json,
        .{ .ignore_unknown_fields = false },
    );
    defer parsed.deinit();
    if (!std.mem.eql(u8, parsed.value.type, "ping")) return error.InvalidFrame;
    return parsed.value.nonce;
}

pub fn decodeApplicationStateChange(
    allocator: std.mem.Allocator,
    json: []const u8,
) !ApplicationStateChange {
    var parsed = try std.json.parseFromSlice(
        ApplicationStateFrame,
        allocator,
        json,
        .{ .ignore_unknown_fields = false },
    );
    defer parsed.deinit();
    if (!std.mem.eql(u8, parsed.value.type, "application_state_changed") or
        parsed.value.state_revision == 0)
    {
        return error.InvalidFrame;
    }
    return .{
        .api = parsed.value.api,
        .state_revision = parsed.value.state_revision,
    };
}

pub fn decodeControlCenterResponseRequestId(
    allocator: std.mem.Allocator,
    json: []const u8,
) ![]u8 {
    var parsed = try std.json.parseFromSlice(
        ApplicationResponseHeader,
        allocator,
        json,
        .{ .ignore_unknown_fields = true },
    );
    defer parsed.deinit();
    if (!std.mem.eql(u8, parsed.value.type, "application_response") or
        !std.mem.eql(u8, parsed.value.response.api, "control_center_v1"))
    {
        return error.InvalidFrame;
    }
    try validateIdentifier(parsed.value.response.request_id);
    return allocator.dupe(
        u8,
        parsed.value.response.request_id,
    );
}

pub fn decodeControlCenterResponseOutcome(
    allocator: std.mem.Allocator,
    json: []const u8,
) !ControlCenterResponseOutcome {
    var parsed = try std.json.parseFromSlice(
        ApplicationResponseHeader,
        allocator,
        json,
        .{ .ignore_unknown_fields = true },
    );
    defer parsed.deinit();
    if (!std.mem.eql(u8, parsed.value.type, "application_response") or
        !std.mem.eql(u8, parsed.value.response.api, "control_center_v1"))
    {
        return error.InvalidFrame;
    }
    try validateIdentifier(parsed.value.response.request_id);
    const kind = std.meta.stringToEnum(
        ControlCenterResponseKind,
        parsed.value.response.result.type,
    ) orelse return error.InvalidFrame;
    const request_id = try allocator.dupe(u8, parsed.value.response.request_id);
    errdefer allocator.free(request_id);
    const error_message = if (parsed.value.response.result.@"error") |failure|
        try allocator.dupe(u8, failure.message)
    else
        null;
    var session: ?control_model.SessionIdentity = null;
    if (parsed.value.response.result.session) |identity| {
        try validateIdentifier(identity.execution_target_id);
        try validateIdentifier(identity.tmux_session_id);
        const target = try allocator.dupe(u8, identity.execution_target_id);
        errdefer allocator.free(target);
        session = .{ .execution_target_id = target, .tmux_session_id = try allocator.dupe(u8, identity.tmux_session_id) };
    }
    const items_json = try std.json.Stringify.valueAlloc(allocator, parsed.value.response.result.items, .{});
    defer allocator.free(items_json);
    const batch = if (kind == .session_batch_created) try std.json.parseFromSlice([]const SessionCreationItem, allocator, items_json, .{ .allocate = .alloc_always }) else null;
    return .{
        .batch = batch,
        .request_id = request_id,
        .kind = kind,
        .session = session,
        .error_message = error_message,
    };
}

/// One validated registration feeds both base controls and optional extensions.
pub fn decodeRegisteredFrame(allocator: std.mem.Allocator, json: []const u8) !std.json.Parsed(RegisteredFrame) {
    var parsed = try std.json.parseFromSlice(RegisteredFrame, allocator, json, .{ .ignore_unknown_fields = false });
    errdefer parsed.deinit();
    if (!std.mem.eql(u8, parsed.value.type, "registered") or parsed.value.protocol_version != 1) return error.InvalidFrame;
    try validateIdentifier(parsed.value.registration_id);
    try validateIdentifier(parsed.value.host_id);
    return parsed;
}

pub fn decodeRegisteredHostId(allocator: std.mem.Allocator, json: []const u8) ![]u8 {
    var parsed = try decodeRegisteredFrame(allocator, json);
    defer parsed.deinit();
    return allocator.dupe(u8, parsed.value.host_id);
}

fn validateIdentifier(value: []const u8) Error!void {
    if (value.len == 0 or value.len > 512) return error.InvalidIdentifier;
    if (!std.mem.eql(u8, value, std.mem.trim(u8, value, " \t\r\n"))) {
        return error.InvalidIdentifier;
    }
    for (value) |byte| {
        if (std.ascii.isControl(byte)) return error.InvalidIdentifier;
    }
}

/// Old Hosts can still render Sessions without accepting new controls.
pub fn registeredSupportsCodex(allocator: std.mem.Allocator, payload: []const u8) bool {
    var parsed = decodeRegisteredFrame(allocator, payload) catch return false;
    defer parsed.deinit();
    return if (parsed.value.host_control_versions) |versions| versions.codex == 2 else false;
}
pub fn registeredSupportsNotification(allocator: std.mem.Allocator, payload: []const u8) bool {
    var parsed = decodeRegisteredFrame(allocator, payload) catch return false;
    defer parsed.deinit();
    return if (parsed.value.host_control_versions) |versions| versions.notification == 1 else false;
}


