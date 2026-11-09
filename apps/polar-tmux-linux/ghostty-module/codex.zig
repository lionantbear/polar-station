//! Thin Codex codec. Product decisions and original-thread authorization stay in Rust.
const std = @import("std");
const controls = @import("control_center/model.zig");
const sessions = @import("session_runtime.zig");
pub const Endpoint = struct {
    id: []const u8,
    execution_target_id: []const u8,
    control_mode: []const u8,
    owner_host_id: []const u8,
    socket_path: []const u8,
    peer_connection_ref: ?[]const u8 = null,
    remote_endpoint_id: ?[]const u8 = null,
    remote_target_id: ?[]const u8 = null,
};
pub const HostConnection = struct {
    id: []const u8,
    host_id: []const u8,
    ssh_target_id: []const u8,
    port: u16,
    execution_credential_ref: ?[]const u8 = null,
    notification_credential_ref: ?[]const u8 = null,
};
pub const AllowedSource = struct { bot_id: i64, chat_id: i64, sender_id: i64 };
pub const Destination = struct { id: []const u8, bot_id: i64, chat_id: i64 };
pub const TelegramSettings = struct {
    enabled: bool = false,
    bot_credential_ref: ?[]const u8 = null,
    allowed_sources: []const AllowedSource = &.{},
    destinations: []const Destination = &.{},
};
pub const ImessageDestination = struct {
    host_id: []const u8,
    connection_ref: ?[]const u8 = null,
    sender_generation: []const u8,
    destination_ref: []const u8,
};
pub const Settings = struct {
    revision: u64 = 0,
    enabled: bool = false,
    endpoints: []const Endpoint = &.{},
    host_connections: []const HostConnection = &.{},
    telegram: TelegramSettings = .{},
    imessage_enabled: bool = false,
    imessage_sender: ?ImessageDestination = null,
};
pub const Binding = struct {
    owner_host_id: []const u8,
    binding_id: []const u8,
    revision: u64,
    session: ?sessions.KeyWire = null,
    endpoint_id: []const u8,
    thread_id: []const u8,
    telegram_destination_ref: ?[]const u8 = null,
    imessage_enabled: bool,
};
pub const Presentation = struct {
    connection_label: []const u8,
    task_label: []const u8,
    input_label: []const u8,
    detail: ?[]const u8 = null,
    actions: []const controls.Action,
};
pub const State = struct {
    thread_name: ?[]const u8 = null,
    binding: Binding,
    connection: []const u8,
    activity: []const u8,
    turn_id: ?[]const u8 = null,
    final_answer: ?[]const u8 = null,
    error_code: ?[]const u8 = null,
    can_queue: bool,
    can_steer: bool,
    can_interrupt: bool,
    presentation: ?Presentation = null,
};
pub const Thread = struct { thread_id: []const u8, display_name: []const u8 };
pub const Receipt = struct {
    label: ?[]const u8 = null,
    clear_input: bool = false,
    intent_id: []const u8,
    status: []const u8,
    queue_id: ?[]const u8 = null,
    turn_id: ?[]const u8 = null,
    error_code: ?[]const u8 = null,
};
pub const Cursor = struct { created_at_ms: u64, id: []const u8 };
pub const CompletionOrigin = struct {
    owner_host_id: []const u8,
    endpoint_id: []const u8,
    thread_id: []const u8,
    turn_id: []const u8,
    binding_id: []const u8,
    binding_revision: u64,
    execution_target_id: []const u8,
    tmux_session_id: ?[]const u8 = null,
};
pub const Completion = struct { id: []const u8, origin: CompletionOrigin, status: []const u8, summary: []const u8, created_at_ms: u64 };
pub const Delivery = struct { channel: []const u8, status: []const u8, label: []const u8, created_at_ms: u64, error_code: ?[]const u8 = null };
pub const CompletionProjection = struct { completion: Completion, deliveries: []const Delivery };
pub const ChannelStatus = struct { state: []const u8, error_code: ?[]const u8 = null };
pub const Result = struct {
    labels: ?std.json.ArrayHashMap([]const u8) = null,
    kind: []const u8,
    settings: ?Settings = null,
    status: ?ChannelStatus = null,
    threads: ?[]const Thread = null,
    bindings: ?[]const State = null,
    binding: ?State = null,
    receipt: ?Receipt = null,
    completions: ?[]const CompletionProjection = null,
    next_before: ?Cursor = null,
    code: ?[]const u8 = null,
};
pub const Response = struct { protocol_version: u32, api: []const u8, request_id: []const u8, owner_host_id: []const u8, result: Result };
pub const Frame = struct { type: []const u8, response: Response };
pub fn decode(allocator: std.mem.Allocator, payload: []const u8) !std.json.Parsed(Frame) {
    var parsed = try std.json.parseFromSlice(Frame, allocator, payload, .{ .ignore_unknown_fields = true, .allocate = .alloc_always });
    errdefer parsed.deinit();
    if (!std.mem.eql(u8, parsed.value.type, "application_response") or
        !std.mem.eql(u8, parsed.value.response.api, "codex_v1") or parsed.value.response.protocol_version != 1)
        return error.InvalidCodexResponse;
    return parsed;
}
pub fn encode(allocator: std.mem.Allocator, request_id: []const u8, owner: []const u8, command: anytype) ![]u8 {
    return std.json.Stringify.valueAlloc(allocator, .{ .type = "application_request", .request = .{
        .protocol_version = 1,
        .api = "codex_v1",
        .request_id = request_id,
        .owner_host_id = owner,
        .command = command,
    } }, .{ .whitespace = .minified });
}

/// Snapshot the rendered identity before a confirmation is opened. The caller
/// retains these bytes; a later state event cannot retarget this operation.
pub fn encodeAction(allocator: std.mem.Allocator, id: []const u8, state: State, action: controls.Action, input: []const u8) ![]u8 {
    if (!action.enabled) return error.ActionDisabled;
    const b = state.binding;
    if (std.mem.eql(u8, action.id, "queue")) return encode(allocator, id, b.owner_host_id, .{
        .kind = "queue_text",
        .binding_id = b.binding_id,
        .expected_revision = b.revision,
        .intent_id = id,
        .text = input,
    });
    if (std.mem.eql(u8, action.id, "steer")) return encode(allocator, id, b.owner_host_id, .{
        .kind = "steer",
        .binding_id = b.binding_id,
        .expected_revision = b.revision,
        .intent_id = id,
        .expected_turn_id = state.turn_id orelse return error.TurnMissing,
        .text = input,
    });
    if (std.mem.eql(u8, action.id, "interrupt")) return encode(allocator, id, b.owner_host_id, .{
        .kind = "interrupt",
        .binding_id = b.binding_id,
        .expected_revision = b.revision,
        .intent_id = id,
        .turn_id = state.turn_id orelse return error.TurnMissing,
    });
    if (std.mem.eql(u8, action.id, "resume") or std.mem.eql(u8, action.id, "unbind")) return encode(allocator, id, b.owner_host_id, .{
        .kind = action.id,
        .binding_id = b.binding_id,
        .expected_revision = b.revision,
    });
    return error.UnsupportedAction;
}

test "native confirmation keeps original owner binding revision and turn" {
    const allocator = std.testing.allocator;
    var state: State = .{ .binding = .{ .owner_host_id = "owner-a", .binding_id = "binding", .revision = 7, .session = .{ .execution_target = "local", .tmux_session_id = "$1" }, .endpoint_id = "endpoint", .thread_id = "thread", .imessage_enabled = false }, .connection = "connected", .activity = "active", .turn_id = "turn-a", .can_queue = true, .can_steer = true, .can_interrupt = true };
    var action: controls.Action = .{ .id = "interrupt", .label = "Interrupt", .role = .destructive, .enabled = true, .selected = false, .disabled_reason = null, .confirmation = null, .accessibility_label = "Interrupt" };
    const payload = try encodeAction(allocator, "unique-intent", state, action, "");
    defer allocator.free(payload);
    state.turn_id = "turn-b";
    state.binding.revision = 8;
    var parsed = try std.json.parseFromSlice(std.json.Value, allocator, payload, .{});
    defer parsed.deinit();
    const request = parsed.value.object.get("request").?.object;
    const command = request.get("command").?.object;
    try std.testing.expectEqualStrings("owner-a", request.get("owner_host_id").?.string);
    try std.testing.expectEqualStrings("turn-a", command.get("turn_id").?.string);
    try std.testing.expectEqual(@as(i64, 7), command.get("expected_revision").?.integer);
    action.enabled = false;
    try std.testing.expectError(error.ActionDisabled, encodeAction(allocator, "later", state, action, ""));
}
test "Codex original input and typed receipt do not pass through terminal commands" {
    const allocator = std.testing.allocator;
    const wire = try encode(allocator, "rpc", "owner", .{ .kind = "steer", .binding_id = "binding", .expected_revision = 7, .intent_id = "intent", .expected_turn_id = "turn", .text = "/literal\nexact text" });
    defer allocator.free(wire);
    var value = try std.json.parseFromSlice(std.json.Value, allocator, wire, .{});
    defer value.deinit();
    const request = value.value.object.get("request").?.object;
    try std.testing.expectEqualStrings("owner", request.get("owner_host_id").?.string);
    const command = request.get("command").?.object;
    try std.testing.expectEqualStrings("/literal\nexact text", command.get("text").?.string);
    try std.testing.expectEqualStrings("turn", command.get("expected_turn_id").?.string);
    var response = try decode(allocator, "{\"type\":\"application_response\",\"response\":{\"protocol_version\":1,\"api\":\"codex_v1\",\"owner_host_id\":\"owner\",\"request_id\":\"rpc\",\"result\":{\"kind\":\"receipt\",\"receipt\":{\"intent_id\":\"intent\",\"status\":\"unknown\"}}}}");
    defer response.deinit();
    try std.testing.expectEqualStrings("unknown", response.value.response.result.receipt.?.status);
}
