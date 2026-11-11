//! Control Center v1 request encoding.
//!
//! The Linux renderer sends only typed action/selection intent plus the exact
//! projection revision it rendered.

const std = @import("std");
const model = @import("model.zig");

/// The complete Control Center v1 action vocabulary available to the thin
/// Linux renderer. Every submit variant maps one-to-one to the Rust protocol.
const SessionSummaryAction = struct { execution_target_id: []const u8, tmux_session_id: []const u8, summary_revision: u64 };

pub const SessionReplacementSelection = struct {
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
    replacement: model.SessionReplacement,
    workspace_id: []const u8,
    confirmation_id: []const u8,
};

pub const ConnectionChange = union(enum) {
    save_machine: model.MachineConnection,
    save_polar: model.PolarConnection,
    remove_machine: []const u8,
    remove_polar: []const u8,
};
pub const ConnectionUpdate = struct { expected_revision: []const u8, change: ConnectionChange };
pub const EditorRequest = struct {
    type: []const u8,
    execution_target_id: ?[]const u8 = null,
    yolo: ?bool = null,
    cursor: ?[]const u8 = null,
    search_term: ?[]const u8 = null,
    archived: ?bool = null,
    include_subagents: ?bool = null,
    source_session_id: ?[]const u8 = null,
    tmux_session_id: ?[]const u8 = null,
    path: ?[]const u8 = null,
    endpoint_id: ?[]const u8 = null,
    owner_host_id: ?[]const u8 = null,
    thread_id: ?[]const u8 = null,
};

pub const Action = union(enum) {
    start_local_codex:struct{endpoint_id:[]const u8,owner_host_id:[]const u8,token:u64},
    session_editor: struct { request: EditorRequest, generation: u64, shared: bool = false, ending: bool = false },
    cancel_session_editor: struct { token: u64, ending: bool = false },
    update_connection: ConnectionUpdate,
    check_connection: struct { target_id: []const u8, polar: bool },
    use_existing_session: SessionReplacementSelection,
    retire_lost_session: SessionReplacementSelection,
    open_codex_thread: struct { owner: []const u8, binding: []const u8 },
    open_inbox_session: struct { execution_target_id: []const u8, tmux_session_id: []const u8 },
    open_codex_settings,
    open_codex_sessions: struct { target_id: []const u8 },
    open_codex: struct { execution_target_id: []const u8, tmux_session_id: []const u8 },
    retry_host,
    refresh_sessions,
    restart_session: struct { execution_target_id: []const u8, tmux_session_id: []const u8 },
    acknowledge_session_summary: SessionSummaryAction,
    mark_session_notification_handled: SessionSummaryAction,
    get_projection,
    toggle_sessions,
    set_managed_terminal_font_size: struct { points: u8 },
    create_workspace: struct { name: []const u8 },
    select_workspace: struct { workspace_id: []const u8 },
    rename_workspace: struct { name: []const u8 },
    set_workspace_ssh_target: struct {
        target_id: []const u8,
        target_revision: u64,
        destination: []const u8,
        route_profile: ?model.SshRouteProfile,
    },
    delete_workspace: struct { confirmation_id: []const u8 },
    add_tab: struct {
        title: []const u8,
        tab_id: ?[]const u8 = null,
    },
    select_tab: struct { tab_id: []const u8 },
    rename_tab: struct { tab_id: []const u8, title: []const u8 },
    delete_tab: struct {
        tab_id: []const u8,
        confirmation_id: []const u8,
        replacement_tab_id: ?[]const u8 = null,
    },
    reorder_tab: struct { tab_id: []const u8, to_index: usize },
    reorder_pane: struct { tab_id: []const u8, pane_id: []const u8, to_index: usize },
    move_pane: struct {
        pane_id: []const u8,
        from_tab_id: []const u8,
        to_tab_id: []const u8,
        to_index: usize,
    },
    toggle_display_mode: struct { tab_id: []const u8 },
    select_session: struct {
        execution_target_id: []const u8,
        tmux_session_id: []const u8,
    },
    toggle_session_assignment: struct {
        tab_id: ?[]const u8 = null,
        execution_target_id: []const u8,
        tmux_session_id: []const u8,
        pane_id: ?[]const u8 = null,
        focus_new: bool = false,
        promote_new: bool = false,
    },
    quick_create_session: struct {
        execution_target_id: []const u8,
        source_session_id: ?[]const u8,
        kind: enum { shell, codex },
    },
    create_codex_session: struct {
        execution_target_id: []const u8,
        endpoint_id: []const u8,
        name: []const u8,
        working_directory: ?[]const u8,
        yolo: bool = false,
    },
    create_agent_session: struct {
        execution_target_id: []const u8,
        provider: []const u8,
        mode: []const u8,
        name: []const u8,
        working_directory: ?[]const u8,
        endpoint_id: ?[]const u8,
        session_id: ?[]const u8,
        session_ids: []const []const u8 = &.{},
        yolo: bool = false,
        open_local_cli: bool = false,
    },
    create_session: struct {
        execution_target_id: []const u8,
        name: []const u8,
        working_directory: ?[]const u8,
        command: ?[]const u8,
        auto_restart: bool = false,
    },
    end_agent_session: struct { owner_host_id: []const u8, endpoint_id: []const u8, thread_id: []const u8, confirmation_id: []const u8 },
    rename_agent_session: struct { owner_host_id: []const u8, endpoint_id: []const u8, thread_id: []const u8, name: []const u8 },
    rename_session: struct {
        execution_target_id: []const u8,
        tmux_session_id: []const u8,
        name: []const u8,
    },
    kill_session: struct {
        execution_target_id: []const u8,
        tmux_session_id: []const u8,
        confirmation_id: []const u8,
    },
    select_pane: struct { pane_id: []const u8 },
    activate_pane: struct { pane_id: []const u8 },
    remove_pane: struct { pane_id: []const u8 },
    acknowledge_pane_summary: struct {
        pane_id: []const u8,
        summary_revision: u64,
    },
    mark_attention_notification_handled: struct {
        pane_id: []const u8,
        summary_revision: u64,
    },
    invoke_action: struct {
        action_id: []const u8,
        confirmation_id: ?[]const u8 = null,
    },
    inbox_reply: struct { message_id: []const u8, revision: u64, reply: []const u8 },
    inbox_edit_draft: struct { message_id: []const u8, revision: u64, draft: []const u8 },
    inbox_approve: struct { message_id: []const u8, revision: u64 },
    inbox_refresh,
};

pub fn encodeAction(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    frontend_session_id: []const u8,
    control_revision: u64,
    action: Action,
) ![]u8 {
    return switch (action) {
        .start_local_codex=>error.InvalidNativeAction,
        .cancel_session_editor => error.InvalidAction,
        .session_editor => |v| std.json.Stringify.valueAlloc(allocator, .{
            .type = "application_request",
            .request = .{
                .protocol_version = @as(u32, 1),
                .api = "control_center_v1",
                .request_id = request_id,
                .frontend_session_id = frontend_session_id,
                .expected_control_revision = @as(?u64, null),
                .command = .{ .type = "session_editor", .request = v.request },
            },
        }, .{ .emit_null_optional_fields = false }),
        .check_connection => |v| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "check_connection", .target_id = v.target_id, .polar = v.polar }),
        .update_connection => |v| switch (v.change) {
            .save_machine => |m| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "update_connection", .expected_revision = v.expected_revision, .change = .{ .type = "save_machine", .machine = m } }),
            .save_polar => |p| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "update_connection", .expected_revision = v.expected_revision, .change = .{ .type = "save_polar", .polar = p } }),
            .remove_machine => |id| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "update_connection", .expected_revision = v.expected_revision, .change = .{ .type = "remove_machine", .id = id } }),
            .remove_polar => |id| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "update_connection", .expected_revision = v.expected_revision, .change = .{ .type = "remove_polar", .id = id } }),
        },
        .quick_create_session => |v| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{
            .intent = "quick_create_session",
            .execution_target_id = v.execution_target_id,
            .source_session_id = v.source_session_id,
            .kind = @tagName(v.kind),
        }),
        .use_existing_session, .retire_lost_session => |v| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{
            .intent = "use_existing_session",
            .execution_target_id = v.execution_target_id,
            .tmux_session_id = v.tmux_session_id,
            .replacement = v.replacement,
            .confirmation_id = v.confirmation_id,
            .retire = action == .retire_lost_session,
        }),
        .get_projection => encodeGetProjection(allocator, request_id, frontend_session_id),
        .refresh_sessions => encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "refresh_sessions" }),
        .set_workspace_ssh_target => |v| encodeSetWorkspaceSshTarget(allocator, request_id, frontend_session_id, control_revision, v.target_id, v.target_revision, v.destination, v.route_profile),
        .create_codex_session => |v| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "create_codex_session", .execution_target_id = v.execution_target_id, .endpoint_id = v.endpoint_id, .name = v.name, .working_directory = v.working_directory, .yolo = v.yolo }),
        .create_agent_session => |v| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "create_agent_session", .execution_target_id = v.execution_target_id, .provider = v.provider, .mode = v.mode, .name = v.name, .working_directory = v.working_directory, .endpoint_id = v.endpoint_id, .session_id = v.session_id, .session_ids = v.session_ids, .yolo = v.yolo, .open_local_cli = v.open_local_cli, .open_terminal = true }),
        .create_session => |v| encodeCreateSession(allocator, request_id, frontend_session_id, control_revision, v.execution_target_id, v.name, v.working_directory, v.command, v.auto_restart),
        .end_agent_session => |v| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "end_agent_session", .owner_host_id = v.owner_host_id, .endpoint_id = v.endpoint_id, .thread_id = v.thread_id, .confirmation_id = v.confirmation_id }),
        .rename_agent_session => |v| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "rename_agent_session", .owner_host_id = v.owner_host_id, .endpoint_id = v.endpoint_id, .thread_id = v.thread_id, .name = v.name }),
        .rename_session => |v| encodeRenameSession(allocator, request_id, frontend_session_id, control_revision, v.execution_target_id, v.tmux_session_id, v.name),
        .kill_session => |v| encodeKillSession(allocator, request_id, frontend_session_id, control_revision, v.execution_target_id, v.tmux_session_id, v.confirmation_id),
        .restart_session => |v| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{ .intent = "restart_session", .execution_target_id = v.execution_target_id, .tmux_session_id = v.tmux_session_id }),
        .acknowledge_session_summary, .mark_session_notification_handled => |v| encodeIntent(allocator, request_id, frontend_session_id, control_revision, .{
            .intent = if (action == .acknowledge_session_summary) "acknowledge_session_summary" else "mark_attention_notification_handled",
            .execution_target_id = v.execution_target_id,
            .tmux_session_id = v.tmux_session_id,
            .summary_revision = v.summary_revision,
        }),
        else => error.NativeActionMustNotReachHost,
    };
}

pub fn encodeGetProjection(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    frontend_session_id: []const u8,
) ![]u8 {
    return std.json.Stringify.valueAlloc(allocator, .{
        .type = "application_request",
        .request = .{
            .protocol_version = 1,
            .api = "control_center_v1",
            .request_id = request_id,
            .frontend_session_id = frontend_session_id,
            .expected_control_revision = null,
            .command = .{ .type = "get_projection" },
        },
    }, .{});
}

pub fn encodeSetWorkspaceSshTarget(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    frontend_session_id: []const u8,
    control_revision: u64,
    target_id: []const u8,
    target_revision: u64,
    destination: []const u8,
    route_profile: ?model.SshRouteProfile,
) ![]u8 {
    return encodeIntent(
        allocator,
        request_id,
        frontend_session_id,
        control_revision,
        .{
            .intent = "set_ssh_target",
            .target_id = target_id,
            .target_revision = target_revision,
            .destination = destination,
            .route_profile = route_profile,
        },
    );
}

pub fn encodeCreateSession(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    frontend_session_id: []const u8,
    control_revision: u64,
    execution_target_id: []const u8,
    name: []const u8,
    working_directory: ?[]const u8,
    command: ?[]const u8,
    auto_restart: bool,
) ![]u8 {
    return encodeIntent(
        allocator,
        request_id,
        frontend_session_id,
        control_revision,
        .{
            .intent = "create_session",
            .execution_target_id = execution_target_id,
            .name = name,
            .working_directory = working_directory,
            .command = command,
            .auto_restart = auto_restart,
        },
    );
}

pub fn encodeRenameSession(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    frontend_session_id: []const u8,
    control_revision: u64,
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
    name: []const u8,
) ![]u8 {
    return encodeIntent(
        allocator,
        request_id,
        frontend_session_id,
        control_revision,
        .{
            .intent = "rename_session",
            .execution_target_id = execution_target_id,
            .tmux_session_id = tmux_session_id,
            .name = name,
        },
    );
}

pub fn encodeKillSession(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    frontend_session_id: []const u8,
    control_revision: u64,
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
    confirmation_id: []const u8,
) ![]u8 {
    return encodeIntent(
        allocator,
        request_id,
        frontend_session_id,
        control_revision,
        .{
            .intent = "kill_session",
            .execution_target_id = execution_target_id,
            .tmux_session_id = tmux_session_id,
            .confirmation_id = confirmation_id,
        },
    );
}

fn encodeIntent(
    allocator: std.mem.Allocator,
    request_id: []const u8,
    frontend_session_id: []const u8,
    control_revision: u64,
    intent: anytype,
) ![]u8 {
    return std.json.Stringify.valueAlloc(allocator, .{
        .type = "application_request",
        .request = .{
            .protocol_version = 1,
            .api = "control_center_v1",
            .request_id = request_id,
            .frontend_session_id = frontend_session_id,
            .expected_control_revision = control_revision,
            .command = .{
                .type = "submit_intent",
                .intent = intent,
            },
        },
    }, .{});
}


