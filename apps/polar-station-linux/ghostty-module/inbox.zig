//! Inbox v1 codec for the thin GTK adapter. Rust owns message semantics,
//! persistence, AI generation, revisions, and approval authorization.
const std = @import("std");

pub const Origin = struct {
    workspace_id: []const u8,
    tab_id: []const u8,
    pane_id: []const u8,
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
    consumer_generation: u64,
    native_recovery_token: []const u8,
};
pub const AiInvocation = struct {
    provider: []const u8,
    model: ?[]const u8 = null,
    duration_ms: u64,
    @"error": ?[]const u8 = null,
};
pub const Message = struct {
    id: []const u8,
    origin: Origin,
    session_label: []const u8,
    attention: []const u8,
    source: ?[]const u8 = null,
    confidence: ?[]const u8 = null,
    summary: []const u8,
    created_at_ms: u64,
    revision: u64,
    unread: bool,
    reply: ?[]const u8 = null,
    draft: ?[]const u8 = null,
    draft_status: []const u8,
    stale_context: bool,
    ai_invocation: ?AiInvocation = null,
};
pub const Page = struct {
    messages: []const Message,
    next_before_created_at_ms: ?u64 = null,
    unread_count: u32,
    overview: ?Overview = null,
};
pub const Overview = struct {
    activity: []const OverviewRow,
    records: []const OverviewRow,
    errors: []const []const u8,
};
pub const OverviewRow = struct {
    id: []const u8,
    source: []const u8,
    execution_target_id: []const u8,
    title: []const u8,
    task_status: []const u8,
    task_label: []const u8,
    observation_label: []const u8,
    stale: bool,
    summary: []const u8,
    created_at_ms: u64,
    session: ?@import("session_runtime.zig").KeyWire = null,
    terminal_message_id: ?[]const u8 = null,
    codex_owner_id: ?[]const u8 = null,
    codex_binding_id: ?[]const u8 = null,
    deliveries: []const @import("codex.zig").Delivery = &.{},
};
const ErrorBody = struct { code: []const u8, message: []const u8, retryable: bool };
const Result = struct {
    kind: []const u8,
    page: ?Page = null,
    message: ?Message = null,
    message_id: ?[]const u8 = null,
    origin: ?Origin = null,
    text: ?[]const u8 = null,
    delivery_mode: ?[]const u8 = null,
    approval_nonce: ?[]const u8 = null,
    state: ?[]const u8 = null,
    @"error": ?ErrorBody = null,
};
const Response = struct {
    protocol_version: u32,
    api: []const u8,
    request_id: []const u8,
    event_revision: u64,
    result: Result,
};
const Frame = struct { type: []const u8, response: Response };

pub const Delivery = struct {
    message_id: []const u8,
    origin: Origin,
    text: []const u8,
    mode: []const u8,
};

pub const Model = struct {
    allocator: std.mem.Allocator,
    current: ?std.json.Parsed(Frame) = null,
    pub fn init(allocator: std.mem.Allocator) Model {
        return .{ .allocator = allocator };
    }
    pub fn deinit(self: *Model) void {
        if (self.current) |*value| value.deinit();
        self.* = undefined;
    }
    pub fn page(self: *const Model) ?*const Page {
        if (self.current) |*value| if (value.value.response.result.page) |*page_value| return page_value;
        return null;
    }
    pub fn applyResponse(self: *Model, json: []const u8) !?Delivery {
        var parsed = try std.json.parseFromSlice(Frame, self.allocator, json, .{ .ignore_unknown_fields = true, .allocate = .alloc_always });
        errdefer parsed.deinit();
        if (!std.mem.eql(u8, parsed.value.type, "application_response") or
            !std.mem.eql(u8, parsed.value.response.api, "inbox_v1") or
            parsed.value.response.protocol_version != 1)
            return error.InvalidInboxResponse;
        const result = &parsed.value.response.result;
        if (std.mem.eql(u8, result.kind, "page") or std.mem.eql(u8, result.kind, "message") or
            std.mem.eql(u8, result.kind, "delivery_receipt"))
        {
            if (self.current) |*value| value.deinit();
            self.current = parsed;
            return null;
        }
        if (std.mem.eql(u8, result.kind, "delivery_requested")) {
            const delivery = Delivery{
                .message_id = result.message_id orelse return error.InvalidInboxResponse,
                .origin = result.origin orelse return error.InvalidInboxResponse,
                .text = result.text orelse return error.InvalidInboxResponse,
                .mode = result.delivery_mode orelse return error.InvalidInboxResponse,
            };
            // The caller must consume the borrowed delivery before the parsed
            // response is released, so retain it as the current response.
            if (self.current) |*value| value.deinit();
            self.current = parsed;
            return delivery;
        }
        parsed.deinit();
        return null;
    }
};

pub fn encodeObserveAttention(allocator: std.mem.Allocator, request_id: []const u8, origin: Origin) ![]u8 {
    return encode(allocator, request_id, .{ .kind = "observe_attention", .origin = origin });
}

fn encode(allocator: std.mem.Allocator, request_id: []const u8, command: anytype) ![]u8 {
    return std.json.Stringify.valueAlloc(allocator, .{ .type = "application_request", .request = .{
        .protocol_version = 1,
        .api = "inbox_v1",
        .request_id = request_id,
        .command = command,
    } }, .{ .whitespace = .minified });
}
pub fn encodeGetPage(allocator: std.mem.Allocator, request_id: []const u8) ![]u8 {
    return encode(allocator, request_id, .{ .kind = "get_page", .before_created_at_ms = @as(?u64, null), .limit = @as(u16, 100) });
}
pub fn encodeReply(allocator: std.mem.Allocator, request_id: []const u8, message_id: []const u8, revision: u64, reply: []const u8) ![]u8 {
    return encode(allocator, request_id, .{ .kind = "reply", .message_id = message_id, .expected_revision = revision, .reply = reply });
}
pub fn encodeEditDraft(allocator: std.mem.Allocator, request_id: []const u8, message_id: []const u8, revision: u64, draft: []const u8) ![]u8 {
    return encode(allocator, request_id, .{ .kind = "edit_draft", .message_id = message_id, .expected_revision = revision, .draft = draft });
}
pub fn encodeApprove(allocator: std.mem.Allocator, request_id: []const u8, message_id: []const u8, revision: u64, mode: []const u8, nonce: []const u8) ![]u8 {
    return encode(allocator, request_id, .{ .kind = "approve", .message_id = message_id, .expected_revision = revision, .delivery_mode = mode, .approval_nonce = nonce });
}
pub fn encodeReceipt(allocator: std.mem.Allocator, request_id: []const u8, message_id: []const u8, revision: u64, state: []const u8) ![]u8 {
    return encode(allocator, request_id, .{ .kind = "record_delivery", .message_id = message_id, .expected_revision = revision, .state = state });
}


