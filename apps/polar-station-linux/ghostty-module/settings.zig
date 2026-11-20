//! Settings v1 codec for the thin GTK adapter. Rust owns validation and persistence.
const std = @import("std");
const c = @cImport({ @cInclude("polar_station_desktop_client.h"); });

pub const TmuxStatusView = struct {
    fields: []const struct { id: []const u8, label: []const u8, selected: bool },
    preview: []const u8,
};

pub fn tmuxStatusView(allocator: std.mem.Allocator, fields: []const []const u8, facts: ?std.json.Value, purpose: ?[]const u8) !std.json.Parsed(TmuxStatusView) {
    const input = try std.json.Stringify.valueAlloc(allocator, .{ .fields = fields, .facts = facts, .purpose = purpose }, .{});
    defer allocator.free(input);
    const output = c.polar_station_tmux_status_bar_view(input.ptr, input.len);
    if (output == null) return error.InvalidTmuxStatusFields;
    defer c.polar_station_desktop_string_free(output);
    return std.json.parseFromSlice(TmuxStatusView, allocator, std.mem.span(output), .{ .allocate = .alloc_always });
}

pub const WorkspaceSessionBinding = struct {
    execution_target: []const u8,
    tmux_session_id: []const u8,
    source: ?@import("portable_content.zig").Source,
};

pub const TmuxPreviewTarget = struct {
    binding: WorkspaceSessionBinding,
    workspace_name: []const u8,
};

pub const TmuxPurposeEdit = struct {
    pane: WorkspaceSessionBinding,
    text: []const u8,
};

pub const tmux_preview_prefix = "tmux-preview-";
pub const TmuxPreviewReply = struct {
    response: struct {
        request_id: []const u8,
        result: struct { tmux_status_facts: ?std.json.Value = null },
    },
};

pub fn encodeTmuxPreview(allocator: std.mem.Allocator, request_id: []const u8, target: TmuxPreviewTarget) ![]u8 {
    return encodeRead(allocator, request_id, .{ .kind = "get_projection", .tmux_preview = target.binding, .workspace_name = target.workspace_name });
}

pub fn encodeUpdateTmuxStatusBar(allocator: std.mem.Allocator, request_id: []const u8, revision: u64, fields: []const []const u8, sessions: []const WorkspaceSessionBinding, workspace_name: []const u8, purpose: ?TmuxPurposeEdit) ![]u8 {
    return encodeMutation(allocator, request_id, revision, .{
        .kind = "update_tmux_status_bar",
        .value = fields,
        .workspace_sessions = sessions,
        .workspace_name = workspace_name,
        .tmux_purpose = purpose,
    });
}

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
    tmux_status_bar: ?[]const []const u8 = null,
    tmux_status_warning: ?[]const u8 = null,
};
const ErrorBody = struct { code: []const u8, message: []const u8, retryable: bool };
pub const CodexNotifyStatus = enum { missing, configured, conflict, invalid, unsafe, unsupported };
const Result = struct { kind: []const u8, tmux_status_bar: ?[]const []const u8 = null, tmux_status_warning: ?[]const u8 = null, theme_projection: ?ThemeProjection = null, projection: ?Projection = null, summary: ?[]const u8 = null, execution_target_id: ?[]const u8 = null, status: ?CodexNotifyStatus = null, @"error": ?ErrorBody = null };
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
            projected.tmux_status_bar = parsed.value.response.result.tmux_status_bar;
            projected.tmux_status_warning = parsed.value.response.result.tmux_status_warning;
            if(self.projection()) |current| if(projected.revision<current.revision) {parsed.deinit();return;};
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








