//! Resources v1 projection and request codec for the thin GTK adapter.
//!
//! Rust remains the only owner of catalog validation, action availability,
//! runtime state, reconciliation, and result meaning.

const std = @import("std");

pub const RuntimeState = enum {
    disabled,
    stopped,
    waiting_for_ssh,
    starting,
    running,
    recovering,
    needs_attention,
    stopping,
};

pub const RuntimeStatus = struct {
    state: RuntimeState,
    generation: u64,
    message: []const u8,
    can_restart: bool,
    can_open_mirror: bool,
};

pub const FileMapping = struct {
    id: []const u8,
    name: []const u8,
    enabled: bool,
    execution_target_id: []const u8,
    remote_path: []const u8,
    local_path: []const u8,
};

pub const PortForward = struct {
    id: []const u8,
    name: []const u8,
    enabled: bool,
    execution_target_id: []const u8,
    bind_address: []const u8,
    allow_non_loopback: bool,
    local_port_start: u16,
    local_port_end: u16,
    remote_host: []const u8,
    remote_port_start: u16,
};

pub const AndroidMirror = struct {
    id: []const u8,
    name: []const u8,
    enabled: bool,
    execution_target_id: []const u8,
    bind_address: []const u8,
    allow_non_loopback: bool,
    local_adb_port: u16,
    remote_adb_host: []const u8,
    remote_adb_port: u16,
    scrcpy_arguments: []const []const u8,
};

pub const Catalog = struct {
    schema_version: u32,
    revision: u64,
    file_mappings: []const FileMapping,
    port_forwards: []const PortForward,
    android_mirrors: []const AndroidMirror,
};

pub const Entry = struct {
    id: []const u8,
    kind: []const u8,
    name: []const u8,
    enabled: bool,
    execution_target_id: []const u8,
    status: RuntimeStatus,
    can_reveal_local_path: bool = false,
    can_open_log: bool = false,
    action_disabled_reason: ?[]const u8 = null,
};

pub const Projection = struct {
    schema_version: u32,
    revision: u64,
    catalog: Catalog,
    entries: []const Entry,
    available_target_ids: []const []const u8,
    has_pending_draft: bool,
    apply_label: []const u8,
    aggregate_state: RuntimeState = .disabled,
    aggregate_message: []const u8 = "Resources are unavailable.",

    pub fn entry(self: *const Projection, id: []const u8) ?*const Entry {
        for (self.entries) |*candidate| {
            if (std.mem.eql(u8, candidate.id, id)) return candidate;
        }
        return null;
    }
};

const ErrorBody = struct {
    code: []const u8,
    message: []const u8,
    retryable: bool,
};

pub const LogEvent = struct {
    timestamp_ms: u64,
    level: []const u8,
    message: []const u8,
};

pub const LogTail = struct {
    resource_id: []const u8,
    events: []const LogEvent,
    truncated: bool,
};

pub const FileAccess=struct {state:[]const u8,reference:?std.json.Value=null,access:?std.json.Value=null,origin:?std.json.Value=null,candidates:?std.json.Value=null,resource_id:?[]const u8=null,local_path:?[]const u8=null,execution_target_id:?[]const u8=null,remote_directory:?[]const u8=null,source_path:?[]const u8=null};
const Result = struct {
    resolution:?std.json.Value=null,
    access:?FileAccess=null,
    type: []const u8,
    projection: ?Projection = null,
    tail: ?LogTail = null,
    @"error": ?ErrorBody = null,
};

const Response = struct {
    protocol_version: u32,
    api: []const u8,
    request_id: []const u8,
    revision: u64,
    result: Result,
};

const Frame = struct {
    type: []const u8,
    response: Response,
};

pub const ResponseKind = enum { projection, command_accepted, log_tail, file_access, file_open, @"error" };

pub const Outcome = struct {
    request_id: []u8,
    kind: ResponseKind,
    revision: u64,
    error_message: ?[]u8 = null,

    pub fn deinit(self: *Outcome, allocator: std.mem.Allocator) void {
        allocator.free(self.request_id);
        if (self.error_message) |message| allocator.free(message);
        self.* = undefined;
    }
};

pub const Model = struct {
    allocator: std.mem.Allocator,
    current: ?std.json.Parsed(Frame) = null,
    last_log: ?std.json.Parsed(Frame) = null,
    last_file:?std.json.Parsed(Frame)=null,

    pub fn init(allocator: std.mem.Allocator) Model {
        return .{ .allocator = allocator };
    }

    pub fn deinit(self: *Model) void {
        if (self.current) |*current| current.deinit();
        if (self.last_log) |*current| current.deinit();
        if(self.last_file) |*current| current.deinit();
        self.* = undefined;
    }

    pub fn projection(self: *const Model) ?*const Projection {
        if (self.current) |*current| {
            if (current.value.response.result.projection) |*projection_value| {
                return projection_value;
            }
        }
        return null;
    }

    pub fn fileOpenResolution(self:*const Model) ?std.json.Value {return if(self.last_file) |value| value.value.response.result.resolution else null;}
    pub fn fileAccess(self:*const Model) ?FileAccess {return if(self.last_file) |value| value.value.response.result.access else null;}
    pub fn logTail(self: *const Model) ?*const LogTail {
        if (self.last_log) |*current| {
            if (current.value.response.result.tail) |*tail| return tail;
        }
        return null;
    }

    pub fn applyResponse(self: *Model, json: []const u8) !Outcome {
        var parsed = try std.json.parseFromSlice(
            Frame,
            self.allocator,
            json,
            .{ .ignore_unknown_fields = true, .allocate = .alloc_always },
        );
        errdefer parsed.deinit();
        if (!std.mem.eql(u8, parsed.value.type, "application_response") or
            !std.mem.eql(u8, parsed.value.response.api, "resources_v1") or
            parsed.value.response.protocol_version != 1)
        {
            return error.InvalidResourcesResponse;
        }
        const kind = std.meta.stringToEnum(
            ResponseKind,
            parsed.value.response.result.type,
        ) orelse return error.InvalidResourcesResponse;
        var outcome = Outcome{
            .request_id = try self.allocator.dupe(
                u8,
                parsed.value.response.request_id,
            ),
            .kind = kind,
            .revision = parsed.value.response.revision,
        };
        errdefer outcome.deinit(self.allocator);
        if (kind == .@"error") {
            const rejection = parsed.value.response.result.@"error" orelse return error.InvalidResourcesResponse;
            outcome.error_message = try self.allocator.dupe(u8, rejection.message);
        }
        if (parsed.value.response.result.projection != null) {
            if (self.current) |*current| current.deinit();
            self.current = parsed;
        } else if(parsed.value.response.result.access!=null or parsed.value.response.result.resolution!=null) {
            if(self.last_file) |*value| value.deinit();self.last_file=parsed;
        } else if (parsed.value.response.result.tail != null) {
            if (self.last_log) |*current| current.deinit();
            self.last_log = parsed;
        } else {
            parsed.deinit();
        }
        return outcome;
    }
};

pub fn encodeResolveFileOpen(allocator:std.mem.Allocator,request_id:[]const u8,origin:std.json.Value,input:std.json.Value) ![]u8 {
    return encodeRead(allocator,request_id,.{.type="resolve_file_open",.origin=origin,.input=input});
}
pub fn encodePlanFileAccess(allocator:std.mem.Allocator,request_id:[]const u8,file:std.json.Value) ![]u8 {
    return encodeRead(allocator,request_id,.{.type="plan_file_access",.file=file});
}

pub fn encodeGetProjection(
    allocator: std.mem.Allocator,
    request_id: []const u8,
) ![]u8 {
    return encodeRead(allocator, request_id, .{ .type = "get_projection" });
}

pub fn encodeGetLogTail(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    resource_id: []const u8,
) ![]u8 {
    return encodeRead(allocator, request_id, .{
        .type = "get_log_tail",
        .resource_id = resource_id,
    });
}

pub fn encodeRestart(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    revision: u64,
    resource_id: []const u8,
) ![]u8 {
    return encodeMutation(allocator, request_id, revision, .{
        .type = "restart",
        .resource_id = resource_id,
    });
}

pub fn encodeOpenMirror(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    revision: u64,
    resource_id: []const u8,
) ![]u8 {
    return encodeMutation(allocator, request_id, revision, .{
        .type = "open_mirror",
        .resource_id = resource_id,
    });
}

pub fn encodeApply(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    revision: u64,
    catalog: Catalog,
) ![]u8 {
    return encodeMutation(allocator, request_id, revision, .{
        .type = "apply",
        .catalog = catalog,
    });
}

fn encodeRead(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    command: anytype,
) ![]u8 {
    return std.json.Stringify.valueAlloc(allocator, .{
        .type = "application_request",
        .request = .{
            .protocol_version = 1,
            .api = "resources_v1",
            .request_id = request_id,
            .expected_revision = null,
            .command = command,
        },
    }, .{ .whitespace = .minified });
}

fn encodeMutation(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    revision: u64,
    command: anytype,
) ![]u8 {
    return std.json.Stringify.valueAlloc(allocator, .{
        .type = "application_request",
        .request = .{
            .protocol_version = 1,
            .api = "resources_v1",
            .request_id = request_id,
            .expected_revision = revision,
            .command = command,
        },
    }, .{ .whitespace = .minified });
}



pub fn encodeResolveFileCandidates(allocator:std.mem.Allocator,id:[]const u8,origin:std.json.Value,candidates:std.json.Value) ![]u8 {
    return encodeRead(allocator,id,.{.type="resolve_file_candidates",.origin=origin,.candidates=candidates});
}

pub fn encodeResolveFile(allocator:std.mem.Allocator,id:[]const u8,target:[]const u8,path:[]const u8,cwd:?[]const u8,session:?[]const u8) ![]u8 {
    if(session)|value|return encodeRead(allocator,id,.{ .type="resolve_file",.client_supports_local_file=true,.execution_target_id="",.source_session=.{.execution_target=target,.tmux_session_id=value},.path=path,.cwd=cwd});
    return encodeRead(allocator,id,.{.type="resolve_file",.client_supports_local_file=true,.execution_target_id=target,.path=path,.cwd=cwd});
}






