//! Read-only join of one accepted native Workspace and Host Session facts.
const std = @import("std");
const controller_model = @import("control_center/model.zig");
const renderpkg = @import("workspace_rendering.zig");
const session_runtime = @import("session_runtime.zig");

pub fn workspace(
    allocator: std.mem.Allocator,
    snapshot: renderpkg.Snapshot,
) !controller_model.WorkspaceMap {
    return build(allocator, snapshot, null, null, null);
}

pub fn build(
    allocator: std.mem.Allocator,
    snapshot: renderpkg.Snapshot,
    projected: ?*const controller_model.Projection,
    cache: ?*const session_runtime.Cache,
    observed_pane_id: ?[]const u8,
) !controller_model.WorkspaceMap {
    const tabs = try allocator.alloc(controller_model.Tab, snapshot.tabs.len);
    var selected_pane_id: ?[]const u8 = null;
    for (snapshot.tabs, 0..) |source_tab, tab_index| {
        const panes = try allocator.alloc(
            controller_model.Pane,
            source_tab.panes.len,
        );
        for (source_tab.panes, 0..) |source_pane, pane_index| {
            const selected_id = if (source_tab.display_mode == .focus) source_tab.focused_pane_id else observed_pane_id;
            const selected = if (selected_id) |id| std.mem.eql(u8, id, source_pane.id) else false;
            if (selected and std.mem.eql(
                u8,
                source_tab.id,
                snapshot.selected_tab_id,
            )) selected_pane_id = source_pane.id;
            panes[pane_index] = .{
                .id = source_pane.id,
                .content = source_pane.content,
                .execution_target_id = source_pane.execution_target_id,
                .tmux_session_id = source_pane.tmux_session_id,
                .target_label = if (source_pane.content) |content| (if (content.kind == .idea_tree) "Local" else content.target_id) else source_pane.execution_target_id,
                .session_name = if (source_pane.content) |content| (if (content.kind == .idea_tree) "Idea Tree" else content.thread) else source_pane.tmux_session_id,
                .transport_status = switch (source_pane.transport_status) {
                    .online => .online,
                    .offline => .offline,
                    .recovering => .recovering,
                },
                .selected = selected,
            };
        }
        if (projected) |facts| for (panes) |*pane| {
            if (pane.content != null) continue;
            for (facts.sessions.rows) |row| {
                if (std.mem.eql(u8, row.identity.execution_target_id, pane.execution_target_id) and
                    std.mem.eql(u8, row.identity.tmux_session_id, pane.tmux_session_id))
                {
                    pane.session_name = row.display_name;
                    pane.target_label = row.target_label;
                    pane.target_kind = if (row.section == .local) .local else .ssh;
                    pane.transport_status = row.transport_status;
                    break;
                }
            }
        };
        if (cache) |runtime| for (panes) |*pane| {
            if (pane.content != null) continue;
            if (runtime.lifecycle(pane.execution_target_id, pane.tmux_session_id)) |lifecycle| {
                pane.session_presence = switch (lifecycle) {
                    .ready => .present,
                    .ended => .ended,
                    else => .unknown,
                };
            }
            if (runtime.attention(pane.execution_target_id, pane.tmux_session_id)) |attention| pane.attention = .{
                .execution_target_id = pane.execution_target_id,
                .tmux_session_id = pane.tmux_session_id,
                .summary_revision = attention.summary_revision,
                .summary_sections = attention.summary_sections,
                .state = std.meta.stringToEnum(controller_model.AttentionState, @tagName(attention.state)).?,
                .freshness = std.meta.stringToEnum(controller_model.SummaryFreshness, @tagName(attention.freshness)).?,
                .attention_since_ms = attention.attention_since_ms,
                .unread = attention.unread,
                .notification_pending = attention.notification_pending,
            };
        };
        tabs[tab_index] = .{
            .id = source_tab.id,
            .title = source_tab.title,
            .selected = std.mem.eql(
                u8,
                source_tab.id,
                snapshot.selected_tab_id,
            ),
            .display_mode = switch (source_tab.display_mode) {
                .monitor => .monitor,
                .focus => .focus,
            },
            .focused_pane_id = source_tab.focused_pane_id,
            .visual_pane_ids = source_tab.visual_pane_ids,
            .rename_action = nativeRenameTabAction(),
            .delete_action = nativeDeleteTabAction(),
            .panes = panes,
        };
    }
    return .{
        .workspace_id = snapshot.workspace_id,
        .workspace_name = snapshot.workspace_name,
        .desired_revision = snapshot.workspace_revision,
        .structure_revision = snapshot.workspace_revision,
        .structure_fingerprint = snapshot.snapshot_fingerprint,
        .selected_tab_id = snapshot.selected_tab_id,
        .selected_pane_id = selected_pane_id,
        .tabs = tabs,
    };
}

fn nativeRenameTabAction() controller_model.Action {
    return .{
        .id = "workspace.tab.rename",
        .label = "Rename Tab",
        .role = .secondary,
        .enabled = true,
        .selected = false,
        .disabled_reason = null,
        .confirmation = null,
        .accessibility_label = "Rename Tab",
    };
}

fn nativeDeleteTabAction() controller_model.Action {
    return .{
        .id = "workspace.tab.delete",
        .label = "Delete Tab",
        .role = .destructive,
        .enabled = true,
        .selected = false,
        .disabled_reason = null,
        .confirmation = .{
            .id = "native-delete-tab",
            .title = "Delete Tab?",
            .message = "The tmux Sessions remain available.",
            .confirm_label = "Delete Tab",
            .destructive = true,
        },
        .accessibility_label = "Delete Tab",
    };
}

fn inertAction() controller_model.Action {
    return .{
        .id = "native-owned",
        .label = "Native",
        .role = .secondary,
        .enabled = false,
        .selected = false,
        .disabled_reason = null,
        .confirmation = null,
        .accessibility_label = "Native Workspace action",
    };
}
