//! Settings v1 codec for the thin GTK adapter. Rust owns validation and persistence.
const std = @import("std");

pub const ChatCache = struct { message_mib: u32 = 512, presentation_mib: u32 = 32 };
pub const Theme = struct { mode: []const u8 = "system", preset: []const u8 = "catppuccin_mocha", terminal_theme: ?[]const u8 = null };
pub const Palette = struct { id: []const u8, name: []const u8, dark: bool, background: []const u8, foreground: []const u8, surface: []const u8, accent: []const u8, selection_background: []const u8, selection_foreground: []const u8 };
pub const ThemeProjection = struct { choices: []const Palette, light: Palette, dark: Palette };
pub const Appearance = struct {
    theme: Theme = .{},
    lgp_cache: ChatCache = .{},
    interface_text_scale_percent: u16 = 100,
    layout_map_width: ?u16 = null,
    layout_map_visible: bool = true,
    inbox_visible: bool = false,
    inbox_width: ?u16 = null,
    terminal_delivery_mode: []const u8 = "insert",
    chat_send_mode: []const u8 = "enter",
};
pub const Attention = struct { system_notifications_enabled: bool = false };
pub const Monitoring = struct {
    enabled: bool = true,
    quiet_period_ms: u64 = 5_000,
    codex_title_detection: bool = true,
};
pub const SessionSummary = struct {
    enabled: bool = false,
    command: []const u8 = "codex",
    model: ?[]const u8 = null,
    timeout_ms: u64 = 15_000,
    quiet_period_ms: u64 = 5_000,
};
pub const Projection = struct {
    theme_projection: ?ThemeProjection = null,
    schema_version: u32,
    revision: u64,
    appearance: Appearance,
    attention: Attention,
    monitoring: Monitoring = .{},
    session_summary: SessionSummary,
};
const ErrorBody = struct { code: []const u8, message: []const u8, retryable: bool };
pub const CodexNotifyStatus = enum { missing, configured, conflict, invalid, unsafe, unsupported };
const Result = struct { kind: []const u8, theme_projection: ?ThemeProjection = null, projection: ?Projection = null, summary: ?[]const u8 = null, execution_target_id: ?[]const u8 = null, status: ?CodexNotifyStatus = null, @"error": ?ErrorBody = null };
const Response = struct { protocol_version: u32, api: []const u8, request_id: []const u8, revision: u64, result: Result };
const Frame = struct { type: []const u8, response: Response };

pub const Model = struct {
    allocator: std.mem.Allocator,
    current: ?std.json.Parsed(Frame) = null,
    codex_notify_status: ?CodexNotifyStatus = null,
    pub fn init(allocator: std.mem.Allocator) Model {
        return .{ .allocator = allocator };
    }
    pub fn deinit(self: *Model) void {
        if (self.current) |*value| value.deinit();
        self.* = undefined;
    }
    pub fn projection(self: *const Model) ?*const Projection {
        if (self.current) |*value| if (value.value.response.result.projection) |*projected| return projected;
        return null;
    }
    pub fn codexNotifyStatus(self: *const Model) ?CodexNotifyStatus {
        return self.codex_notify_status;
    }
    pub fn applyResponse(self: *Model, json: []const u8) !void {
        var parsed = try std.json.parseFromSlice(Frame, self.allocator, json, .{ .ignore_unknown_fields = true, .allocate = .alloc_always });
        errdefer parsed.deinit();
        if (!std.mem.eql(u8, parsed.value.type, "application_response") or
            !std.mem.eql(u8, parsed.value.response.api, "settings_v1") or parsed.value.response.protocol_version != 1)
            return error.InvalidSettingsResponse;
        if (parsed.value.response.result.projection) |*projected| {
            projected.theme_projection = parsed.value.response.result.theme_projection;
            if (self.current) |*value| value.deinit();
            self.current = parsed;
        } else {
            if (std.mem.eql(u8, parsed.value.response.result.kind, "codex_notify_status")) {
                self.codex_notify_status = parsed.value.response.result.status orelse return error.InvalidSettingsResponse;
            }
            parsed.deinit();
        }
    }
};

pub fn encodeGetProjection(allocator: std.mem.Allocator, request_id: []const u8) ![]u8 {
    return encodeRead(allocator, request_id, .{ .kind = "get_projection" });
}
pub fn encodeUpdateAppearance(allocator: std.mem.Allocator, request_id: []const u8, revision: u64, value: Appearance) ![]u8 {
    return encodeMutation(allocator, request_id, revision, .{ .kind = "update_appearance", .value = value });
}
pub fn encodeUpdateAttention(allocator: std.mem.Allocator, request_id: []const u8, revision: u64, value: Attention) ![]u8 {
    return encodeMutation(allocator, request_id, revision, .{ .kind = "update_attention", .value = value });
}
pub fn encodeUpdateMonitoring(allocator: std.mem.Allocator, request_id: []const u8, revision: u64, value: Monitoring) ![]u8 {
    return encodeMutation(allocator, request_id, revision, .{ .kind = "update_monitoring", .value = value });
}
pub fn encodeUpdateSessionSummary(allocator: std.mem.Allocator, request_id: []const u8, revision: u64, value: SessionSummary) ![]u8 {
    return encodeMutation(allocator, request_id, revision, .{ .kind = "update_session_summary", .value = value });
}
pub fn encodeTestSessionSummary(allocator: std.mem.Allocator, request_id: []const u8, value: SessionSummary) ![]u8 {
    return encodeRead(allocator, request_id, .{ .kind = "test_session_summary_provider", .value = value });
}
pub fn encodeGetCodexNotifyStatus(allocator: std.mem.Allocator, request_id: []const u8) ![]u8 {
    return encodeRead(allocator, request_id, .{ .kind = "get_codex_notify_status", .execution_target_id = "local" });
}
pub fn encodeConfigureCodexNotify(allocator: std.mem.Allocator, request_id: []const u8) ![]u8 {
    return encodeRead(allocator, request_id, .{ .kind = "configure_codex_notify", .execution_target_id = "local" });
}
fn encodeRead(allocator: std.mem.Allocator, request_id: []const u8, command: anytype) ![]u8 {
    return std.json.Stringify.valueAlloc(allocator, .{ .type = "application_request", .request = .{
        .protocol_version = 1,
        .api = "settings_v1",
        .request_id = request_id,
        .expected_revision = null,
        .command = command,
    } }, .{ .whitespace = .minified });
}
fn encodeMutation(allocator: std.mem.Allocator, request_id: []const u8, revision: u64, command: anytype) ![]u8 {
    return std.json.Stringify.valueAlloc(allocator, .{ .type = "application_request", .request = .{
        .protocol_version = 1,
        .api = "settings_v1",
        .request_id = request_id,
        .expected_revision = revision,
        .command = command,
    } }, .{ .whitespace = .minified });
}








