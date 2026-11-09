//! Thin Zig wake-and-drain adapter over the shared Rust desktop client.
//!
//! Socket connection, registered-ack readiness, heartbeat, reconnect,
//! generation fencing, and bounded queues belong to Rust. This module owns no
//! control IPC state machine and never carries Pane Host terminal bytes.

const std = @import("std");
const c = @cImport({
    @cInclude("polar_station_desktop_client.h");
});

pub fn trace(stage: u32, identity: []const u8, generation: u64) void {
    c.polar_station_trace_native(stage, identity.ptr, identity.len, generation);
}

pub fn tracePresentation(key: u64, frame_counter: i64, presentation_us: i64) void {
    c.polar_station_trace_presentation(key, frame_counter, presentation_us);
}

pub const WakeFn = *const fn (?*anyopaque) void;

pub const Event = union(enum) {
    request_failed: struct { generation: u64, request_id: []u8, api: []u8, code: []u8, message: []u8, outcome_unknown: bool, cancelled: bool },
    connecting: struct {
        generation: u64,
        attempt: u32,
    },
    ready: struct {
        generation: u64,
        frame: []u8,
    },
    frame: struct {
        generation: u64,
        payload: []u8,
    },
    session_completed: []u8,
    content_completed: []u8,
    native_state_completed: []u8,
    unavailable: struct {
        generation: u64,
        attempt: u32,
        message: []u8,
        retry_after_ms: u64,
    },

    pub fn deinit(self: *Event, allocator: std.mem.Allocator) void {
        switch (self.*) {
            .request_failed => |value| { allocator.free(value.request_id); allocator.free(value.api); allocator.free(value.code); allocator.free(value.message); },
            .ready => |value| allocator.free(value.frame),
            .frame => |value| allocator.free(value.payload),
            .content_completed, .native_state_completed, .session_completed => |value| allocator.free(value),
            .unavailable => |value| allocator.free(value.message),
            .connecting => {},
        }
        self.* = undefined;
    }
};

pub const Client = struct {
    allocator: std.mem.Allocator,
    socket_path: []u8,
    registration: []u8,
    wake_fn: WakeFn,
    wake_context: ?*anyopaque,
    handle: ?*c.DesktopClientHandle = null,

    pub fn init(
        allocator: std.mem.Allocator,
        socket_path: []const u8,
        registration: []const u8,
        wake_fn: WakeFn,
        wake_context: ?*anyopaque,
    ) !Client {
        return .{
            .allocator = allocator,
            .socket_path = try allocator.dupe(u8, socket_path),
            .registration = try allocator.dupe(u8, registration),
            .wake_fn = wake_fn,
            .wake_context = wake_context,
        };
    }

    pub fn start(self: *Client) !void {
        if (self.handle != null) return error.AlreadyStarted;
        var parsed = try std.json.parseFromSlice(
            std.json.Value,
            self.allocator,
            self.registration,
            .{},
        );
        defer parsed.deinit();
        const config = try std.json.Stringify.valueAlloc(self.allocator, .{
            .socket_path = self.socket_path,
            .registration = parsed.value,
            .event_capacity = 256,
        }, .{});
        defer self.allocator.free(config);
        const config_z = try self.allocator.dupeZ(u8, config);
        defer self.allocator.free(config_z);
        const handle = c.polar_station_desktop_client_new(
            config_z.ptr,
            wakeBridge,
            self,
        ) orelse return error.InvalidClientConfiguration;
        errdefer c.polar_station_desktop_client_free(handle);
        if (!c.polar_station_desktop_client_start(handle)) {
            return error.ClientStartFailed;
        }
        self.handle = handle;
    }

    pub fn deinit(self: *Client) void {
        if (self.handle) |handle| {
            c.polar_station_desktop_client_stop(handle);
            c.polar_station_desktop_client_free(handle);
        }
        self.allocator.free(self.registration);
        self.allocator.free(self.socket_path);
        self.* = undefined;
    }

    pub fn sendJson(self: *Client, payload: []const u8) !void {
        const handle = self.handle orelse return error.NotStarted;
        const payload_z = try self.allocator.dupeZ(u8, payload);
        defer self.allocator.free(payload_z);
        switch (c.polar_station_desktop_client_send_json_status(handle, payload_z.ptr)) {
            0 => {},
            1 => return error.NotConnected,
            2 => return error.OutboundQueueFull,
            3 => return error.ExtensionUnsupported,
            4 => return error.InvalidRequest,
            else => return error.NotStarted,
        }
    }

    pub fn cancelRequest(self: *Client, id: []const u8) void {
        const handle = self.handle orelse return;
        const id_z = self.allocator.dupeZ(u8, id) catch return;
        defer self.allocator.free(id_z);
        c.polar_station_desktop_client_cancel_request(handle, c.polar_station_desktop_client_generation(handle), id_z.ptr);
    }
    pub fn retryNow(self: *Client) !void {
        const handle = self.handle orelse return error.NotStarted;
        if (!c.polar_station_desktop_client_retry_now(handle)) {
            return error.RetryUnavailable;
        }
    }

    pub fn sendSessionJson(self: *Client, request: []const u8) !void {
        const handle = self.handle orelse return error.NotStarted;
        const json = try self.allocator.dupeZ(u8, request);
        defer self.allocator.free(json);
        if (!c.polar_station_desktop_client_session_json(handle,json.ptr)) return error.OutboundQueueFull;
    }

    pub fn nativeStateJson(self: *Client, request: []const u8) !void {
        const handle = self.handle orelse return error.NotStarted;
        const json = try self.allocator.dupeZ(u8, request);
        defer self.allocator.free(json);
        if (!c.polar_station_desktop_client_native_state_json(handle, json.ptr)) return error.OutboundUnavailable;
    }

    pub fn sendContentJson(self: *Client, socket_path: []const u8, request_json: []const u8) !void {
        const handle = self.handle orelse return error.NotStarted;
        var parsed = try std.json.parseFromSlice(std.json.Value, self.allocator, request_json, .{});
        defer parsed.deinit();
        const payload = try std.json.Stringify.valueAlloc(self.allocator, .{
            .socket_path = socket_path,
            .request = parsed.value,
        }, .{});
        defer self.allocator.free(payload);
        const payload_z = try self.allocator.dupeZ(u8, payload);
        defer self.allocator.free(payload_z);
        if (!c.polar_station_desktop_client_send_content_json(handle, payload_z.ptr)) return error.ContentQueueBusy;
    }

    /// Ownership of the returned allocation passes to the caller.
    pub fn pop(self: *Client) ?Event {
        const handle = self.handle orelse return null;
        const raw = c.polar_station_desktop_client_pop_event_json(handle) orelse
            return null;
        defer c.polar_station_desktop_string_free(raw);
        var parsed = std.json.parseFromSlice(
            std.json.Value,
            self.allocator,
            std.mem.span(raw),
            .{},
        ) catch return null;
        defer parsed.deinit();
        const object = parsed.value.object;
        const event_type = (object.get("type") orelse return null).string;
        if (std.mem.eql(u8, event_type, "session_completed"))
            return .{ .session_completed = self.allocator.dupe(u8, std.mem.span(raw)) catch return null };
        if (std.mem.eql(u8, event_type, "native_state_completed"))
            return .{ .native_state_completed = self.allocator.dupe(u8, std.mem.span(raw)) catch return null };
        if (std.mem.eql(u8, event_type, "content_completed")) {
            const response = object.get("response") orelse return null;
            return .{ .content_completed = std.json.Stringify.valueAlloc(self.allocator, response, .{}) catch return null };
        }
        const generation = unsigned(object.get("generation") orelse return null) orelse
            return null;
        if (std.mem.eql(u8, event_type, "request_failed")) {
            const Failure = struct { request_id: []const u8, api: []const u8, code: []const u8, message: []const u8, outcome_unknown: bool };
            const failure = std.json.parseFromValue(Failure, self.allocator, object.get("failure") orelse return null, .{}) catch return null;
            defer failure.deinit();
            const id = self.allocator.dupe(u8, failure.value.request_id) catch return null;
            const message = self.allocator.dupe(u8, failure.value.message) catch { self.allocator.free(id); return null; };
            const api = self.allocator.dupe(u8, failure.value.api) catch { self.allocator.free(id); self.allocator.free(message); return null; };
            const code = self.allocator.dupe(u8, failure.value.code) catch { self.allocator.free(id); self.allocator.free(message); self.allocator.free(api); return null; };
            return .{ .request_failed = .{ .generation = generation, .request_id = id, .api = api, .code = code, .message = message,
                .outcome_unknown = failure.value.outcome_unknown, .cancelled = std.mem.eql(u8, code, "request_cancelled") } };
        }
        if (std.mem.eql(u8, event_type, "connecting")) {
            const attempt = unsigned(object.get("attempt") orelse return null) orelse
                return null;
            return .{ .connecting = .{
                .generation = generation,
                .attempt = std.math.cast(u32, attempt) orelse return null,
            } };
        }
        if (std.mem.eql(u8, event_type, "ready")) {
            const registration_id =
                (object.get("registration_id") orelse return null).string;
            const host_id = (object.get("host_id") orelse return null).string;
            const frame = std.json.Stringify.valueAlloc(self.allocator, .{
                .type = "registered",
                .registration_id = registration_id,
                .protocol_version = 1,
                .host_id = host_id,
                .host_control_versions = object.get("host_control_versions") orelse std.json.Value.null,
                .extensions = object.get("extensions") orelse std.json.Value{ .array = std.json.Array.init(self.allocator) },
            }, .{}) catch return null;
            return .{ .ready = .{ .generation = generation, .frame = frame } };
        }
        if (std.mem.eql(u8, event_type, "frame")) {
            const frame = object.get("frame") orelse return null;
            const payload = std.json.Stringify.valueAlloc(
                self.allocator,
                frame,
                .{},
            ) catch return null;
            return .{ .frame = .{ .generation = generation, .payload = payload } };
        }
        if (std.mem.eql(u8, event_type, "unavailable")) {
            const attempt = unsigned(object.get("attempt") orelse return null) orelse
                return null;
            const retry_after_ms = unsigned(
                object.get("retry_after_ms") orelse return null,
            ) orelse return null;
            const message = (object.get("message") orelse return null).string;
            return .{ .unavailable = .{
                .generation = generation,
                .attempt = std.math.cast(u32, attempt) orelse return null,
                .message = self.allocator.dupe(u8, message) catch return null,
                .retry_after_ms = retry_after_ms,
            } };
        }
        return null;
    }

    pub fn hasPending(self: *Client) bool {
        const handle = self.handle orelse return false;
        return c.polar_station_desktop_client_has_pending(handle);
    }

    fn wakeBridge(context: ?*anyopaque) callconv(.c) void {
        const self: *Client = @ptrCast(@alignCast(context orelse return));
        self.wake_fn(self.wake_context);
    }

    fn unsigned(value: std.json.Value) ?u64 {
        return switch (value) {
            .integer => |number| std.math.cast(u64, number),
            else => null,
        };
    }
};

pub fn resolveSocketPath(allocator: std.mem.Allocator) ![]u8 {
    if (std.process.getEnvVarOwned(allocator, "POLAR_TMUX_IPC_SOCKET")) |path| {
        if (path.len == 0) {
            allocator.free(path);
            return error.InvalidSocketPath;
        }
        return path;
    } else |error_value| switch (error_value) {
        error.EnvironmentVariableNotFound => {},
        else => return error_value,
    }
    const runtime_dir = try std.process.getEnvVarOwned(allocator, "XDG_RUNTIME_DIR");
    defer allocator.free(runtime_dir);
    if (runtime_dir.len == 0) return error.InvalidSocketPath;
    return std.fs.path.join(allocator, &.{ runtime_dir, "polar-tmux", "host.sock" });
}


