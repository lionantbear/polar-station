//! Thin Control Center v1 projection storage for the Linux native renderer.
//!
//! Every product decision in these values comes from the Rust Workspace
//! domain. This module only owns decoded display-safe wire data.

const std = @import("std");
const session_runtime = @import("../session_runtime.zig");

pub const ActionRole = enum {
    primary,
    secondary,
    toggle,
    recovery,
    destructive,
};

pub const DisplayMode = enum {
    monitor,
    focus,
};

pub const TransportStatus = enum {
    online,
    offline,
    recovering,
};

pub const SessionSection = enum {
    local,
    remote,
};

pub const Action = struct {
    id: []const u8,
    label: []const u8,
    role: ActionRole,
    enabled: bool,
    selected: bool,
    disabled_reason: ?[]const u8,
    confirmation: ?Confirmation,
    accessibility_label: []const u8,
};

pub const Confirmation = struct {
    id: []const u8,
    title: []const u8,
    message: []const u8,
    confirm_label: []const u8,
    destructive: bool,
};

pub const WorkspaceChoice = struct {
    id: []const u8,
    name: []const u8,
    selected: bool,
};

pub const Connection = struct {
    label: []const u8,
    route: ?[]const u8 = null,
    status: []const u8,
    accessibility_label: []const u8,
};

pub const MachineConnection = struct { id: []const u8, display_name: []const u8, destination: []const u8, route_profile: ?SshRouteProfile = null };
pub const PolarConnection = struct { id: []const u8, display_name: []const u8, execution_target_id: []const u8, socket_path: []const u8, expected_node_id: []const u8, expected_peer_uid: ?u32 = null };
pub const ConnectionCheck = struct { target_id: []const u8, polar: bool, ready: bool, summary: []const u8 };
pub const MachineConnectionStatus = struct { id: []const u8, connection: Connection };
pub const Connections = struct { machine_statuses: []const MachineConnectionStatus = &.{}, last_check: ?ConnectionCheck = null, revision: []const u8 = "", machines: []const MachineConnection = &.{}, polar_instances: []const PolarConnection = &.{} };

pub const SshRouteProfile = struct {
    lan_address: []const u8,
    lan_port: u16,
    wan_host: []const u8,
    wan_port: u16,
    doh_providers: []const []const u8,
};

pub const WorkspaceSshTargetEditor = struct {
    title: []const u8,
    target_id: []const u8,
    target_revision: u64,
    destination: []const u8,
    route_profile: ?SshRouteProfile = null,
    destination_label: []const u8,
    destination_placeholder: []const u8,
    cancel_label: []const u8,
    save_label: []const u8,
};

pub const Header = struct {
    connection: Connection,
    ssh_target_editor: WorkspaceSshTargetEditor,
    rename_action: Action,
    add_tab: Action,
    refresh: Action,
    workspace_menu_detach_action: ?Action = null,
    lifecycle_action: ?Action,
};

pub const SessionIdentity = struct {
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
};

pub const SessionReplacement = struct {
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
    server_generation: []const u8,
    created_unix_seconds: i64,
};

pub const SessionRow = struct {
    identity: SessionIdentity,
    session_kind: []const u8 = "terminal",
    original_agent_id: ?[]const u8 = null,
    display_name: []const u8,
    detail_label: ?[]const u8 = null,
    target_label: []const u8,
    section: SessionSection,
    attached_clients: u32,
    transport_status: TransportStatus,
    selected: bool,
    assigned_to_selected_tab: bool,
    assignment_action: Action,
    assignment_actions: ?SessionAssignmentActions = null,
    rename_action: Action,
    kill_action: Action,
    restart_action: ?Action = null,
    replacement: ?SessionReplacement = null,
    replacement_action: ?Action = null,
};

pub const SessionAssignmentActions = struct {
    add: Action,
    remove: Action,
    tab_full: Action,
    assigned_elsewhere: Action,
    no_managed_tab: Action,
};

pub const SessionSectionState = enum {
    loading,
    populated,
    empty,
    target_missing,
    @"error",
};

pub const SessionSectionProjection = struct {
    section: SessionSection,
    title: []const u8,
    state: SessionSectionState,
    message: ?[]const u8,
    row_count: usize,
};

pub const SessionTargetProjection = struct {
    id: []const u8,
    label: []const u8,
};

pub const Sessions = struct {
    visible: bool,
    title: []const u8,
    interaction_hint: []const u8,
    refresh_action: Action,
    close_action: Action,
    sections: []const SessionSectionProjection,
    remote_targets: []const SessionTargetProjection,
    rows: []SessionRow,
    selected: ?SessionIdentity,
    local_execution_target_id: []const u8,
    remote_execution_target_id: ?[]const u8,
    local_new_shell: Action,
    local_new_command: Action,
    remote_new_session: Action,
};

pub const TargetKind = enum {
    local,
    ssh,
    wsl,
};

pub const AttentionState = enum {
    idle,
    unknown,
    working,
    review_ready,
    needs_attention,
    approval_required,
    decision_required,
    permission_required,
    failed,

    pub fn isRed(self: AttentionState) bool {
        return switch (self) {
            .approval_required,
            .decision_required,
            .permission_required,
            .failed,
            => true,
            else => false,
        };
    }

    pub fn isOrange(self: AttentionState) bool {
        return self == .review_ready or self == .needs_attention;
    }
};

pub const SummaryFreshness = enum {
    analyzing,
    fresh,
    stale,
    unavailable,
};

pub const SessionAttention = struct {
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
    summary_revision: u64 = 0,
    summary_sections: []const []const u8 = &.{},
    state: AttentionState = .idle,
    freshness: SummaryFreshness = .unavailable,
    attention_since_ms: ?u64 = null,
    unread: bool = false,
    notification_pending: bool = false,
};

pub const AttentionBadge = struct {
    unresolved_red: u32 = 0,
    unread_orange: u32 = 0,
    highest_state: AttentionState = .idle,
};

pub const Pane = struct {
    content: ?@import("../station_content.zig").Content = null,
    id: []const u8,
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
    target_label: []const u8,
    target_kind: TargetKind = .local,
    session_name: []const u8,
    transport_status: TransportStatus,
    session_presence: enum { present, ended, unknown } = .unknown,
    selected: bool,
    attention: SessionAttention = .{
        .execution_target_id = "local",
        .tmux_session_id = "unknown",
    },
};

pub const Tab = struct {
    id: []const u8,
    title: []const u8,
    selected: bool,
    display_mode: DisplayMode,
    focused_pane_id: ?[]const u8,
    visual_pane_ids: []const []const u8 = &.{},
    rename_action: Action,
    delete_action: Action,
    panes: []Pane,
};

pub const WorkspaceMap = struct {
    workspace_id: []const u8,
    workspace_name: []const u8,
    desired_revision: u64,
    structure_revision: u64 = 0,
    structure_fingerprint: []const u8 = "",
    selected_tab_id: []const u8,
    selected_pane_id: ?[]const u8 = null,
    tabs: []Tab,
};

pub const BottomBar = struct {
    actions: []const Action,
    more_actions: []const Action,
};

pub const AuxiliaryState = enum {
    available,
    unavailable,
    unpaired,
    paired,
};

pub const AuxiliarySurface = struct {
    id: []const u8,
    title: []const u8,
    state: AuxiliaryState,
    message: []const u8,
};

pub const AuxiliarySurfaces = struct {
    resources: AuxiliarySurface,
    iphone: AuxiliarySurface,
};

pub const BannerLevel = enum {
    information,
    warning,
    @"error",
};

pub const Banner = struct {
    level: BannerLevel,
    message: []const u8,
};

pub const CodexEndpoint = struct {
    socket_path: []const u8 = "",
    peer_connection_ref: ?[]const u8 = null,
    remote_endpoint_id: ?[]const u8 = null,
    remote_target_id: ?[]const u8 = null,
    id: []const u8,
    execution_target_id: []const u8,
    owner_host_id: []const u8,
    control_mode: []const u8 = "local",
};
pub const SessionTargetGroup = struct { id: []const u8, title: []const u8, state: []const u8, message: ?[]const u8, stale: bool };

pub const Projection = struct {
    agent_sessions: []const AgentSession = &.{},
    session_groups: []const SessionTargetGroup = &.{},
    codex_endpoints: []const CodexEndpoint = &.{},
    codex_endpoint_states: std.json.ArrayHashMap([]const u8) = .{},
    connections: Connections = .{},
    schema_version: u16,
    control_revision: u64,
    header: Header,
    sessions: Sessions,
    bottom_bar: BottomBar,
    projected_actions: []const Action = &.{},
    auxiliary_surfaces: AuxiliarySurfaces,
    banner: ?Banner,
    attention_badge: AttentionBadge = .{},

    pub fn action(self: *const Projection, id: []const u8) ?*const Action {
        for (self.projected_actions) |*candidate| {
            if (std.mem.eql(u8, candidate.id, id)) return candidate;
        }
        for (self.bottom_bar.actions) |*candidate| {
            if (std.mem.eql(u8, candidate.id, id)) return candidate;
        }
        if (std.mem.eql(u8, self.header.add_tab.id, id)) return &self.header.add_tab;
        if (std.mem.eql(u8, self.header.refresh.id, id)) return &self.header.refresh;
        if (self.header.workspace_menu_detach_action) |*candidate| {
            if (std.mem.eql(u8, candidate.id, id)) return candidate;
        }
        if (self.header.lifecycle_action) |*candidate| {
            if (std.mem.eql(u8, candidate.id, id)) return candidate;
        }
        return null;
    }
};

pub const ErrorProjection = struct {
    code: []const u8,
    message: []const u8,
};

const Result = struct {
    type: []const u8,
    projection: ?Projection = null,
    @"error": ?ErrorProjection = null,
};

const Response = struct {
    protocol_version: u32,
    api: []const u8,
    request_id: []const u8,
    control_revision: u64,
    result: Result,
};

const ApplicationFrame = struct {
    type: []const u8,
    response: Response,
};

pub const ConnectionState = enum {
    connecting,
    connected,
    unavailable,
};

/// Presentation-owned editor bytes and the exact identity shown when opened.
/// They never constitute an applied Host configuration or a synthetic projection.
pub const HostDraft = struct {
    destination: ?[]u8 = null,
    target_id: ?[]u8 = null,
    target_revision: u64 = 0,
    dirty: bool = false,
    pending: bool = false,
    submitted: ?[]u8 = null,
    route: RouteDraft = .{},
    submitted_route: RouteDraft = .{},

    pub fn deinit(self: *HostDraft, allocator: std.mem.Allocator) void {
        if (self.destination) |value| allocator.free(value);
        if (self.target_id) |value| allocator.free(value);
        if (self.submitted) |value| allocator.free(value);
        self.route.deinit(allocator);
        self.submitted_route.deinit(allocator);
        self.* = .{};
    }

    pub fn begin(self: *HostDraft, allocator: std.mem.Allocator, editor: ?*const WorkspaceSshTargetEditor) !void {
        const value = editor orelse return;
        if (self.target_id != null) return;
        const id = try allocator.dupe(u8, value.target_id);
        errdefer allocator.free(id);
        if (self.destination == null) self.destination = try allocator.dupe(u8, value.destination);
        self.target_id = id;
        self.target_revision = value.target_revision;
        try self.route.begin(allocator, value.route_profile);
    }

    pub fn edit(self: *HostDraft, allocator: std.mem.Allocator, text: []const u8) !void {
        try replaceOwned(allocator, &self.destination, text);
        self.dirty = true;
    }

    pub fn setRouteEnabled(self: *HostDraft, enabled: bool) void {
        self.route.enabled = enabled;
        self.dirty = true;
    }

    pub fn editRouteField(
        self: *HostDraft,
        allocator: std.mem.Allocator,
        field: RouteField,
        text: []const u8,
    ) !void {
        switch (field) {
            .lan_address => try replaceOwned(allocator, &self.route.lan_address, text),
            .lan_port => try replaceOwned(allocator, &self.route.lan_port, text),
            .wan_host => try replaceOwned(allocator, &self.route.wan_host, text),
            .wan_port => try replaceOwned(allocator, &self.route.wan_port, text),
            .doh_primary => try replaceOwned(allocator, &self.route.doh_primary, text),
            .doh_secondary => try replaceOwned(allocator, &self.route.doh_secondary, text),
        }
        self.dirty = true;
    }

    pub fn routeProfile(self: *HostDraft) ?SshRouteProfile {
        return self.route.profile();
    }

    pub fn submit(self: *HostDraft, allocator: std.mem.Allocator) !void {
        const value = try allocator.dupe(u8, self.destination orelse "");
        if (self.submitted) |old| allocator.free(old);
        self.submitted = value;
        self.submitted_route.deinit(allocator);
        self.submitted_route = try self.route.clone(allocator);
        self.pending = true;
    }

    pub fn accept(self: *HostDraft, allocator: std.mem.Allocator) void {
        const unchanged = if (self.submitted) |sent|
            std.mem.eql(u8, sent, self.destination orelse "") and
                self.route.eql(&self.submitted_route)
        else
            false;
        if (unchanged) {
            self.deinit(allocator);
        } else {
            self.pending = false;
            if (self.submitted) |value| allocator.free(value);
            self.submitted = null;
        }
    }
};

pub const RouteField = enum {
    lan_address,
    lan_port,
    wan_host,
    wan_port,
    doh_primary,
    doh_secondary,
};

const RouteDraft = struct {
    enabled: bool = false,
    lan_address: ?[]u8 = null,
    lan_port: ?[]u8 = null,
    wan_host: ?[]u8 = null,
    wan_port: ?[]u8 = null,
    doh_primary: ?[]u8 = null,
    doh_secondary: ?[]u8 = null,
    provider_views: [2][]const u8 = .{ "", "" },

    fn deinit(self: *RouteDraft, allocator: std.mem.Allocator) void {
        freeOwned(allocator, &self.lan_address);
        freeOwned(allocator, &self.lan_port);
        freeOwned(allocator, &self.wan_host);
        freeOwned(allocator, &self.wan_port);
        freeOwned(allocator, &self.doh_primary);
        freeOwned(allocator, &self.doh_secondary);
        self.* = .{};
    }

    fn begin(
        self: *RouteDraft,
        allocator: std.mem.Allocator,
        route_profile: ?SshRouteProfile,
    ) !void {
        self.deinit(allocator);
        self.enabled = route_profile != null;
        const value = route_profile orelse return;
        self.lan_address = try allocator.dupe(u8, value.lan_address);
        errdefer self.deinit(allocator);
        self.lan_port = try std.fmt.allocPrint(allocator, "{d}", .{value.lan_port});
        self.wan_host = try allocator.dupe(u8, value.wan_host);
        self.wan_port = try std.fmt.allocPrint(allocator, "{d}", .{value.wan_port});
        if (value.doh_providers.len > 0) {
            self.doh_primary = try allocator.dupe(u8, value.doh_providers[0]);
        }
        if (value.doh_providers.len > 1) {
            self.doh_secondary = try allocator.dupe(u8, value.doh_providers[1]);
        }
    }

    fn clone(self: *const RouteDraft, allocator: std.mem.Allocator) !RouteDraft {
        var copy = RouteDraft{ .enabled = self.enabled };
        errdefer copy.deinit(allocator);
        if (self.lan_address) |value| copy.lan_address = try allocator.dupe(u8, value);
        if (self.lan_port) |value| copy.lan_port = try allocator.dupe(u8, value);
        if (self.wan_host) |value| copy.wan_host = try allocator.dupe(u8, value);
        if (self.wan_port) |value| copy.wan_port = try allocator.dupe(u8, value);
        if (self.doh_primary) |value| copy.doh_primary = try allocator.dupe(u8, value);
        if (self.doh_secondary) |value| copy.doh_secondary = try allocator.dupe(u8, value);
        return copy;
    }

    fn eql(self: *const RouteDraft, other: *const RouteDraft) bool {
        return self.enabled == other.enabled and
            optionalBytesEqual(self.lan_address, other.lan_address) and
            optionalBytesEqual(self.lan_port, other.lan_port) and
            optionalBytesEqual(self.wan_host, other.wan_host) and
            optionalBytesEqual(self.wan_port, other.wan_port) and
            optionalBytesEqual(self.doh_primary, other.doh_primary) and
            optionalBytesEqual(self.doh_secondary, other.doh_secondary);
    }

    fn profile(self: *RouteDraft) ?SshRouteProfile {
        if (!self.enabled) return null;
        self.provider_views[0] = self.doh_primary orelse "";
        self.provider_views[1] = self.doh_secondary orelse "";
        var provider_count: usize = 0;
        for (self.provider_views) |provider| {
            if (provider.len == 0) break;
            provider_count += 1;
        }
        return .{
            .lan_address = self.lan_address orelse "",
            .lan_port = parsePort(self.lan_port),
            .wan_host = self.wan_host orelse "",
            .wan_port = parsePort(self.wan_port),
            .doh_providers = self.provider_views[0..provider_count],
        };
    }
};

fn replaceOwned(
    allocator: std.mem.Allocator,
    destination: *?[]u8,
    text: []const u8,
) !void {
    const value = try allocator.dupe(u8, text);
    if (destination.*) |old| allocator.free(old);
    destination.* = value;
}

fn freeOwned(allocator: std.mem.Allocator, value: *?[]u8) void {
    if (value.*) |owned| allocator.free(owned);
    value.* = null;
}

fn optionalBytesEqual(left: ?[]const u8, right: ?[]const u8) bool {
    if (left) |value| {
        if (right) |other| return std.mem.eql(u8, value, other);
        return false;
    }
    return right == null;
}

fn parsePort(value: ?[]const u8) u16 {
    return std.fmt.parseInt(u16, value orelse "", 10) catch 0;
}

// Shared Rust policy is independent of Host IPC and is usable before registration.
pub const RecoveryView = extern struct {
    can_submit: bool,
    local_enabled: bool,
    status: [*:0]const u8,
    save_label: [*:0]const u8,
    host_label: [*:0]const u8,
    local_label: [*:0]const u8,
    destination_label: [*:0]const u8,
};

pub extern "c" fn polar_station_recovery_view(connected: bool, target_known: bool, pending: bool) RecoveryView;

pub const Model = struct {
    allocator: std.mem.Allocator,
    connection: ConnectionState = .connecting,
    host_id: ?[]u8 = null,
    parsed: ?std.json.Parsed(ApplicationFrame) = null,
    last_error: ?[]u8 = null,
    session_cache: ?*const session_runtime.Cache = null,

    pub fn init(allocator: std.mem.Allocator) Model {
        return .{ .allocator = allocator };
    }

    pub fn deinit(self: *Model) void {
        self.clearProjection();
        if (self.host_id) |value| self.allocator.free(value);
        if (self.last_error) |value| self.allocator.free(value);
        self.* = undefined;
    }

    pub fn setConnected(self: *Model, host_id: []const u8) !void {
        const next_id = try self.allocator.dupe(u8, host_id);
        if (self.host_id) |value| self.allocator.free(value);
        self.host_id = next_id;
        self.connection = .connected;
    }

    pub fn setUnavailable(self: *Model) void {
        self.connection = .unavailable;
        self.setError(
            "Polar Station backend is unavailable. Cached UI remains visible and Host-dependent actions will recover after reconnect.",
        ) catch {};
    }

    pub fn setLocalError(self: *Model, message: []const u8) void {
        self.setError(message) catch {};
    }

    pub fn projection(self: *const Model) ?*const Projection {
        const parsed = if (self.parsed) |*value| value else return null;
        return if (parsed.value.response.result.projection) |*value| value else null;
    }

    /// Joins Rust-owned Session lifecycle onto the cached display projection
    /// without accepting any Workspace structure from that feed.
    pub fn applySessionRuntime(
        self: *Model,
        cache: *const session_runtime.Cache,
    ) void {
        self.session_cache = cache;
        const parsed = if (self.parsed) |*value| value else return;
        const projection_value = if (parsed.value.response.result.projection) |*value|
            value
        else
            return;
        for (projection_value.sessions.rows) |*row| {
            if (cache.lifecycle(
                row.identity.execution_target_id,
                row.identity.tmux_session_id,
            )) |lifecycle| row.transport_status = transportStatus(lifecycle);
        }
    }

    pub fn applyApplicationResponse(self: *Model, json: []const u8) !void {
        var parsed = try std.json.parseFromSlice(
            ApplicationFrame,
            self.allocator,
            json,
            .{
                // IPC payload ownership ends immediately after this call.
                // Keep every decoded string in the Parsed arena so GTK never
                // observes slices into the released receive buffer.
                .allocate = .alloc_always,
                .ignore_unknown_fields = true,
            },
        );
        errdefer parsed.deinit();
        if (!std.mem.eql(u8, parsed.value.type, "application_response") or
            !std.mem.eql(u8, parsed.value.response.api, "control_center_v1"))
        {
            return error.NotControlCenterResponse;
        }
        if (parsed.value.response.result.@"error") |wire_error| {
            self.setError(wire_error.message) catch {};
            if (parsed.value.response.result.projection != null) {
                self.clearProjection();
                self.parsed = parsed;
            } else {
                // Keep the last good projection so a transient Host error
                // cannot replace the Control Center with a dead spinner.
                parsed.deinit();
            }
            return;
        }
        if (parsed.value.response.result.projection) |next_projection| {
            std.log.debug(
                "Control Center projection revision={d} sessions_visible={} session_rows={d}",
                .{
                    next_projection.control_revision,
                    next_projection.sessions.visible,
                    next_projection.sessions.rows.len,
                },
            );
            self.clearProjection();
            self.parsed = parsed;
            self.clearError();
        } else {
            // An acknowledgement proves neither usable projection recovery
            // nor permission to discard the last usable UI or failure cause.
            parsed.deinit();
        }
    }

    fn clearProjection(self: *Model) void {
        if (self.parsed) |*parsed| parsed.deinit();
        self.parsed = null;
    }

    fn setError(self: *Model, message: []const u8) !void {
        const next_error = try self.allocator.dupe(u8, message);
        self.clearError();
        self.last_error = next_error;
    }

    pub fn dismissError(self: *Model) void {
        if (self.connection == .unavailable) return;
        self.clearError();
    }

    fn clearError(self: *Model) void {
        if (self.last_error) |value| self.allocator.free(value);
        self.last_error = null;
    }
};

fn transportStatus(
    lifecycle: session_runtime.Lifecycle,
) TransportStatus {
    return switch (lifecycle) {
        .ready => .online,
        .connecting, .recovering, .ending => .recovering,
        .unavailable, .ended => .offline,
    };
}

pub const CodexThreadMetadata = struct {
    preview: ?[]const u8 = null,
    cwd: ?[]const u8 = null,
    created_at: ?i64 = null,
    updated_at: ?i64 = null,
    parent_thread_id: ?[]const u8 = null,
    agent_role: ?[]const u8 = null,
    agent_nickname: ?[]const u8 = null,
};
pub const AgentSession = struct {
    terminals: []const SessionIdentity = &.{},
    metadata: ?CodexThreadMetadata = null,
    execution_target_id: []const u8,
    owner_host_id: []const u8,
    endpoint_id: []const u8,
    thread_id: []const u8,
    display_name: []const u8,
    status: []const u8,
    loaded: bool,
};
