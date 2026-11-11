//! Thin C ABI adapter for the shared Rust Session editor interaction.
const std = @import("std");
const intent = @import("control_center/intent.zig");
const control = @import("control_center/model.zig");
const c = @cImport({ @cInclude("polar_station_desktop_client.h"); });
pub const Draft = struct {
    bind_only: bool = false,
    target: []const u8,
    endpoint: []const u8 = "",
    session_type: []const u8 = "shell",
    mode: []const u8 = "new",
    name: []const u8 = "",
    directory: []const u8 = "",
    command: []const u8 = "",
    auto_restart: bool = false,
    yolo: bool = true,
    open_local_cli: bool = false,
    original_id: []const u8 = "",
    search: []const u8 = "",
    archived: bool = false,
    subagents: bool = false,
};
pub const Thread = struct { thread_id: []const u8, display_name: []const u8, cwd: []const u8, status: []const u8, updated_at: i64, preview: ?[]const u8 = null };
pub const Detail = struct { thread: Thread, user_text: ?[]const u8 = null, assistant_text: ?[]const u8 = null };
pub const View = struct {
    can_start_local_server:bool=false,server_starting:bool=false,server_start_label:[]const u8="Start local Codex app server",
    end_preview: ?struct { confirmation: control.Confirmation } = null, end_loading: bool = false, end_completed: bool = false,
    draft: Draft, history: []const Thread, selected: []const []const u8, manual: bool,
    detail: ?Detail, history_loading: bool, detail_loading: bool, directory_loading: bool,
    history_error: ?[]const u8, detail_error: ?[]const u8, message: ?[]const u8,
    can_more: bool, can_submit: bool, pending: bool, submit_label: []const u8, local_cli_supported: bool,
};
pub const Effect = struct { type: []const u8, token: u64, endpoint_id: ?[]const u8 = null, thread_id: ?[]const u8 = null, owner_host_id: ?[]const u8 = null, title: ?[]const u8 = null, request: ?intent.EditorRequest = null, operation: ?std.json.Value = null };
pub const Output = struct { view: View, effects: []const Effect };
pub const Engine = struct {
    handle: ?*c.SessionEditorHandle = null,
    pub fn deinit(self: *Engine) void { c.polar_station_session_editor_free(self.handle); self.handle = null; }
    pub fn dispatch(self: *Engine, allocator: std.mem.Allocator, input: anytype) !std.json.Parsed(Output) {
        if (self.handle == null) self.handle = c.polar_station_session_editor_new();
        const bytes = try std.json.Stringify.valueAlloc(allocator, input, .{ .emit_null_optional_fields = false });
        defer allocator.free(bytes);
        const result = c.polar_station_session_editor_dispatch(self.handle, bytes.ptr, bytes.len) orelse return error.InvalidEditorInput;
        defer c.polar_station_desktop_string_free(result);
        return try std.json.parseFromSlice(Output, allocator, std.mem.span(result), .{ .allocate = .alloc_always, .ignore_unknown_fields = true });
    }
};
