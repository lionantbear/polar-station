//! Companion Workspace relay owned by the Linux desktop adapter.
//!
//! Rust transports the versioned envelope but does not interpret or persist
//! Workspace, Tab, Pane, layout, or selection state. This module decodes only
//! the Companion allowlist and builds privacy-safe responses from the native
//! Workspace snapshot.

const std = @import("std");
const renderpkg = @import("workspace_rendering.zig");
const session_runtime = @import("session_runtime.zig");
const native_workspace_store = @import("native_workspace_store.zig");

pub const Command = union(enum) {
    get_catalog,
    get_workspace,
    select_workspace: []const u8,
    select_tab: []const u8,
    set_display_mode: struct {
        tab_id: []const u8,
        display_mode: renderpkg.DisplayMode,
    },
    select_focus_pane: struct {
        tab_id: []const u8,
        pane_id: []const u8,
    },
    open,
    repair,
    focus_previous,
    focus_next,
    focus_sidebar_slot: u8,

    pub fn isRead(self: Command) bool {
        return self == .get_catalog or self == .get_workspace;
    }

    pub fn isLifecycle(self: Command) bool {
        return self == .open or self == .repair;
    }
};

pub const Request = struct {
    parsed: std.json.Parsed(std.json.Value),
    dispatch_id: []const u8,
    request_id: []const u8,
    scope: std.json.Value,
    expected_catalog_revision: ?u64,
    expected_workspace_revision: ?u64,
    command: Command,

    pub fn deinit(self: *Request) void {
        self.parsed.deinit();
        self.* = undefined;
    }

    pub fn acceptsRevisions(
        self: Request,
        catalog_revision: u64,
        workspace_revision: u64,
    ) bool {
        if (self.expected_catalog_revision) |expected| {
            if (expected != catalog_revision) return false;
        }
        if (self.expected_workspace_revision) |expected| {
            if (expected != workspace_revision) return false;
        }
        return true;
    }
};

pub fn decode(
    allocator: std.mem.Allocator,
    payload: []const u8,
) !Request {
    var parsed = try std.json.parseFromSlice(
        std.json.Value,
        allocator,
        payload,
        .{ .allocate = .alloc_always },
    );
    errdefer parsed.deinit();
    const frame = try requireObject(&parsed.value);
    if (!std.mem.eql(u8, try requireString(frame, "type"), "workspace_relay")) {
        return error.InvalidFrame;
    }
    const dispatch_id = try requireIdentifier(frame, "dispatch_id");
    const request = try requireObject(try requireValue(frame, "request"));
    if (try requireU64(request, "protocol_version") != 1 or
        !std.mem.eql(u8, try requireString(request, "api"), "workspace_v1"))
    {
        return error.UnsupportedProtocol;
    }
    const request_id = try requireIdentifier(request, "request_id");
    const scope = (try requireValue(request, "scope")).*;
    _ = try requireObject(&scope);
    const command_object = try requireObject(try requireValue(request, "command"));
    const command_type = try requireString(command_object, "type");
    const command: Command = if (std.mem.eql(u8, command_type, "get_catalog"))
        .get_catalog
    else if (std.mem.eql(u8, command_type, "get_workspace"))
        .get_workspace
    else if (std.mem.eql(u8, command_type, "select_workspace"))
        .{ .select_workspace = try requireIdentifier(command_object, "workspace_id") }
    else if (std.mem.eql(u8, command_type, "select_tab"))
        .{ .select_tab = try requireIdentifier(command_object, "tab_id") }
    else if (std.mem.eql(u8, command_type, "set_display_mode"))
        .{ .set_display_mode = .{
            .tab_id = try requireIdentifier(command_object, "tab_id"),
            .display_mode = std.meta.stringToEnum(
                renderpkg.DisplayMode,
                try requireString(command_object, "display_mode"),
            ) orelse return error.InvalidFrame,
        } }
    else if (std.mem.eql(u8, command_type, "select_focus_pane"))
        .{ .select_focus_pane = .{
            .tab_id = try requireIdentifier(command_object, "tab_id"),
            .pane_id = try requireIdentifier(command_object, "pane_id"),
        } }
    else if (std.mem.eql(u8, command_type, "open"))
        .open
    else if (std.mem.eql(u8, command_type, "repair"))
        .repair
    else if (std.mem.eql(u8, command_type, "focus_previous"))
        .focus_previous
    else if (std.mem.eql(u8, command_type, "focus_next"))
        .focus_next
    else if (std.mem.eql(u8, command_type, "focus_sidebar_slot"))
        .{ .focus_sidebar_slot = try requireSidebarSlot(command_object) }
    else
        return error.CommandNotAllowed;

    return .{
        .parsed = parsed,
        .dispatch_id = dispatch_id,
        .request_id = request_id,
        .scope = scope,
        .expected_catalog_revision = try optionalU64(
            request,
            "expected_catalog_revision",
        ),
        .expected_workspace_revision = try optionalU64(
            request,
            "expected_workspace_revision",
        ),
        .command = command,
    };
}

pub fn scopeWorkspaceId(request: *const Request) !?[]const u8 {
    const scope = try requireObject(&request.scope);
    const kind = try requireString(scope, "type");
    if (std.mem.eql(u8, kind, "catalog")) return null;
    if (!std.mem.eql(u8, kind, "workspace")) return error.InvalidFrame;
    return try requireIdentifier(scope, "workspace_id");
}

const CompanionPane = struct {
    id: []const u8,
    target_label: []const u8,
    session_name: []const u8,
    transport_status: renderpkg.TransportStatus,
};

const CompanionLayout = struct {
    pane_order: []const []const u8,
    slots: []const std.json.Value = &.{},
    fingerprint: []const u8,
};

const CompanionTab = struct {
    id: []const u8,
    title: []const u8,
    display_mode: renderpkg.DisplayMode,
    focused_pane_id: ?[]const u8,
    pane_order: []const []const u8,
    layout: CompanionLayout,
    panes: []const CompanionPane,
};

const CompanionWorkspace = struct {
    id: []const u8,
    name: []const u8,
    desired_revision: u64,
    applied_revision: ?u64,
    runtime_health: []const u8 = "healthy",
    pending_impact: []const u8 = "none",
    selected_tab_id: []const u8,
    tabs: []const CompanionTab,
};

const CompanionProjection = struct {
    schema_version: u32 = 1,
    revision: u64,
    selected_workspace_id: []const u8,
    workspaces: []const CompanionWorkspace,
    pending_operation: bool = false,
};

pub fn encodeSuccess(
    allocator: std.mem.Allocator,
    request: *const Request,
    catalog: native_workspace_store.Catalog,
    sessions: *const session_runtime.Cache,
) ![]u8 {
    var arena = std.heap.ArenaAllocator.init(allocator);
    defer arena.deinit();
    const temporary = arena.allocator();
    const scope_workspace_id = try scopeWorkspaceId(request);
    const source_workspaces = if (scope_workspace_id) |workspace_id| blk: {
        for (catalog.workspaces) |workspace| {
            if (std.mem.eql(u8, workspace.workspace_id, workspace_id)) {
                const only = try temporary.alloc(renderpkg.Snapshot, 1);
                only[0] = workspace;
                break :blk only;
            }
        }
        return error.WorkspaceNotFound;
    } else catalog.workspaces;
    const workspaces = try temporary.alloc(
        CompanionWorkspace,
        source_workspaces.len,
    );
    for (source_workspaces, 0..) |snapshot, workspace_index| {
        const tabs = try temporary.alloc(CompanionTab, snapshot.tabs.len);
        for (snapshot.tabs, 0..) |tab, tab_index| {
            const panes = try temporary.alloc(CompanionPane, tab.panes.len);
            const pane_order = try temporary.alloc([]const u8, tab.panes.len);
            for (tab.panes, 0..) |pane, pane_index| {
                pane_order[pane_index] = pane.id;
                panes[pane_index] = .{
                    .id = pane.id,
                    .target_label = if (std.mem.eql(
                        u8,
                        pane.execution_target_id,
                        "local",
                    )) "This computer" else pane.execution_target_id,
                    .session_name = sessions.displayName(
                        pane.execution_target_id,
                        pane.tmux_session_id,
                    ) orelse pane.tmux_session_id,
                    .transport_status = transportStatus(
                        sessions.lifecycle(
                            pane.execution_target_id,
                            pane.tmux_session_id,
                        ),
                        pane.transport_status,
                    ),
                };
            }
            tabs[tab_index] = .{
                .id = tab.id,
                .title = tab.title,
                .display_mode = tab.display_mode,
                .focused_pane_id = tab.focused_pane_id,
                .pane_order = pane_order,
                .layout = .{
                    .pane_order = pane_order,
                    .fingerprint = snapshot.snapshot_fingerprint,
                },
                .panes = panes,
            };
        }
        workspaces[workspace_index] = .{
            .id = snapshot.workspace_id,
            .name = snapshot.workspace_name,
            .desired_revision = snapshot.workspace_revision,
            .applied_revision = snapshot.workspace_revision,
            .selected_tab_id = snapshot.selected_tab_id,
            .tabs = tabs,
        };
    }
    const projection: CompanionProjection = .{
        .revision = catalog.catalog_revision,
        .selected_workspace_id = catalog.selected_workspace_id,
        .workspaces = workspaces,
    };
    const response_revision = if (scope_workspace_id) |workspace_id| blk: {
        for (catalog.workspaces) |workspace| {
            if (std.mem.eql(u8, workspace.workspace_id, workspace_id)) {
                break :blk workspace.workspace_revision;
            }
        }
        return error.WorkspaceNotFound;
    } else catalog.catalog_revision;

    if (request.command.isRead()) {
        return encodeCompletion(allocator, request, response_revision, .{
            .type = "companion_catalog",
            .projection = projection,
        });
    }
    if (request.command.isLifecycle()) {
        return encodeCompletion(allocator, request, response_revision, .{
            .type = "companion_operation_completed",
            .operation_id = request.request_id,
            .projection = projection,
            .warnings = @as([]const std.json.Value, &.{}),
        });
    }
    return encodeCompletion(allocator, request, response_revision, .{
        .type = "companion_transition",
        .projection = projection,
        .impact = "none",
        .automatic_apply_candidate = false,
    });
}

pub fn encodeError(
    allocator: std.mem.Allocator,
    request: *const Request,
    revision: u64,
    code: []const u8,
    message: []const u8,
    retryable: bool,
) ![]u8 {
    return encodeCompletion(allocator, request, revision, .{
        .type = "error",
        .@"error" = .{
            .code = code,
            .message = message,
            .retryable = retryable,
        },
    });
}

fn encodeCompletion(
    allocator: std.mem.Allocator,
    request: *const Request,
    revision: u64,
    result: anytype,
) ![]u8 {
    return std.json.Stringify.valueAlloc(
        allocator,
        .{
            .type = "workspace_relay_completed",
            .dispatch_id = request.dispatch_id,
            .response = .{
                .protocol_version = @as(u32, 1),
                .api = "workspace_v1",
                .request_id = request.request_id,
                .scope = request.scope,
                .revision = revision,
                .result = result,
            },
        },
        .{ .whitespace = .minified },
    );
}

fn transportStatus(
    lifecycle: ?session_runtime.Lifecycle,
    fallback: renderpkg.TransportStatus,
) renderpkg.TransportStatus {
    return switch (lifecycle orelse return fallback) {
        .ready => .online,
        .connecting, .recovering, .ending => .recovering,
        .unavailable, .ended => .offline,
    };
}

fn requireSidebarSlot(object: *const std.json.ObjectMap) !u8 {
    const slot = std.math.cast(u8, try requireU64(object, "slot")) orelse
        return error.InvalidNumber;
    if (slot < 1 or slot > 8) return error.InvalidNumber;
    return slot;
}

fn requireObject(value: *const std.json.Value) !*const std.json.ObjectMap {
    return switch (value.*) {
        .object => |*object| object,
        else => error.InvalidFrame,
    };
}

fn requireValue(
    object: *const std.json.ObjectMap,
    key: []const u8,
) !*const std.json.Value {
    return object.getPtr(key) orelse error.MissingField;
}

fn requireString(
    object: *const std.json.ObjectMap,
    key: []const u8,
) ![]const u8 {
    return switch ((try requireValue(object, key)).*) {
        .string => |value| value,
        else => error.InvalidFrame,
    };
}

fn requireIdentifier(
    object: *const std.json.ObjectMap,
    key: []const u8,
) ![]const u8 {
    const value = try requireString(object, key);
    if (value.len == 0 or value.len > 256 or
        !std.mem.eql(u8, value, std.mem.trim(u8, value, " \t\r\n")))
    {
        return error.InvalidIdentifier;
    }
    for (value) |byte| if (std.ascii.isControl(byte)) {
        return error.InvalidIdentifier;
    };
    return value;
}

fn requireU64(
    object: *const std.json.ObjectMap,
    key: []const u8,
) !u64 {
    return switch ((try requireValue(object, key)).*) {
        .integer => |value| std.math.cast(u64, value) orelse
            error.InvalidNumber,
        else => error.InvalidNumber,
    };
}

fn optionalU64(
    object: *const std.json.ObjectMap,
    key: []const u8,
) !?u64 {
    return switch ((try requireValue(object, key)).*) {
        .null => null,
        .integer => |value| std.math.cast(u64, value) orelse
            error.InvalidNumber,
        else => error.InvalidNumber,
    };
}
