const std = @import("std");
const drag = @import("pane_interaction.zig");
const rendering = @import("workspace_rendering.zig");
const layout = @import("native_workspace_layout.zig");
const store = @import("native_workspace_store.zig");

fn pane(id: []const u8) rendering.Pane {
    return .{ .id = id, .execution_target_id = "local", .tmux_session_id = id, .transport_status = .online, .attach_fingerprint = id, .attach_plan = .{ .program = "polar-tmux-pane-host", .args = &.{} } };
}

fn tab(id: []const u8, panes: []const rendering.Pane, visual: []const []const u8, mode: rendering.DisplayMode) rendering.Tab {
    return .{ .id = id, .title = id, .display_mode = mode, .focused_pane_id = if (mode == .focus and visual.len > 0) visual[0] else null, .visual_pane_ids = visual, .panes = panes, .layout = .{ .pane_order = visual, .slots = &.{}, .fingerprint = "native-owned" } };
}

fn workspace(tabs: []const rendering.Tab) rendering.Snapshot {
    return .{ .workspace_id = "workspace-1", .workspace_name = "Workspace", .workspace_revision = 4, .selected_tab_id = "tab-a", .font_size_points = 15, .tabs = tabs, .snapshot_fingerprint = "native" };
}

fn expectIDs(actual: []const []const u8, expected: []const []const u8) !void {
    try std.testing.expectEqual(expected.len, actual.len);
    for (actual, expected) |a, b| try std.testing.expectEqualStrings(b, a);
}

test "Pane drop edges preserve mature before and after targeting" {
    try std.testing.expectEqual(drag.Edge.left, drag.dropEdge(1, 50, 100, 100).?);
    try std.testing.expectEqual(drag.Edge.right, drag.dropEdge(99, 50, 100, 100).?);
    try std.testing.expectEqual(drag.Edge.up, drag.dropEdge(50, 1, 100, 100).?);
    try std.testing.expectEqual(drag.Edge.down, drag.dropEdge(50, 99, 100, 100).?);
    try std.testing.expectEqual(drag.Placement.before, drag.Edge.up.placement());
    try std.testing.expectEqual(drag.Placement.after, drag.Edge.right.placement());
    try std.testing.expect(drag.dropEdge(-1, 0, 100, 100) == null);
    try std.testing.expect(drag.dropEdge(0, 0, 0, 100) == null);
}

test "same Tab insertion removes the source before resolving the target index" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const panes = [_]rendering.Pane{ pane("a"), pane("b"), pane("c") };
    const tabs = [_]rendering.Tab{tab("tab-a", &panes, &.{ "a", "b", "c" }, .monitor)};
    const input = workspace(&tabs);
    const result = (try drag.planDrop(arena.allocator(), input, .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-a", .to_tab_id = "tab-a", .pane_id = "a", .anchor_pane_id = "c", .placement = .after })).?;
    try expectIDs(result.tabs[0].visual_pane_ids, &.{ "b", "c", "a" });
    try std.testing.expectEqualStrings("a", result.tabs[0].panes[2].tmux_session_id);
    try expectIDs(input.tabs[0].visual_pane_ids, &.{ "a", "b", "c" });
    try std.testing.expectEqual(@as(u64, 5), result.workspace_revision);
    const back = (try drag.planDrop(arena.allocator(), result, .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-a", .to_tab_id = "tab-a", .pane_id = "a", .anchor_pane_id = "b", .placement = .before })).?;
    try expectIDs(back.tabs[0].visual_pane_ids, &.{ "a", "b", "c" });
}

test "Focus drop changes visible slots and MAIN without changing its template" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const panes = [_]rendering.Pane{ pane("a"), pane("b"), pane("c"), pane("d") };
    const tabs = [_]rendering.Tab{tab("tab-a", &panes, &.{ "c", "b", "a", "d" }, .focus)};
    const input = workspace(&tabs);
    const side = (try drag.planDrop(arena.allocator(), input, .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-a", .to_tab_id = "tab-a", .pane_id = "d", .anchor_pane_id = "b", .placement = .before })).?;
    try expectIDs(side.tabs[0].visual_pane_ids, &.{ "c", "d", "b", "a" });
    try std.testing.expectEqualStrings("c", side.tabs[0].focused_pane_id.?);
    const main = (try drag.planDrop(arena.allocator(), side, .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-a", .to_tab_id = "tab-a", .pane_id = "d", .anchor_pane_id = "c", .placement = .before })).?;
    try expectIDs(main.tabs[0].visual_pane_ids, &.{ "d", "c", "b", "a" });
    try std.testing.expectEqualStrings("d", main.tabs[0].focused_pane_id.?);
    const before = try layout.layout(tabs[0].visual_pane_ids, .focus, "c");
    const after = try layout.layout(main.tabs[0].visual_pane_ids, .focus, "d");
    try std.testing.expectEqualSlices(usize, before.columnPopulations(), after.columnPopulations());
}

test "cross Tab drop uses current native identities and keeps the last source Tab empty" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = [_]rendering.Pane{pane("a")};
    const b = [_]rendering.Pane{ pane("b"), pane("c") };
    const tabs = [_]rendering.Tab{ tab("tab-a", &a, &.{"a"}, .focus), tab("tab-b", &b, &.{ "c", "b" }, .focus) };
    const input = workspace(&tabs);
    const result = (try drag.planDrop(arena.allocator(), input, .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-a", .to_tab_id = "tab-b", .pane_id = "a" })).?;
    try std.testing.expectEqual(@as(usize, 0), result.tabs[0].panes.len);
    try std.testing.expect(result.tabs[0].focused_pane_id == null);
    try expectIDs(result.tabs[1].visual_pane_ids, &.{ "c", "b", "a" });
    try std.testing.expectEqualStrings("c", result.tabs[1].focused_pane_id.?);
    try std.testing.expectEqualStrings("tab-b", result.selected_tab_id);
    const back = (try drag.planDrop(arena.allocator(), result, .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-b", .to_tab_id = "tab-a", .pane_id = "a" })).?;
    try expectIDs(back.tabs[0].visual_pane_ids, &.{"a"});
    try std.testing.expectEqualStrings("a", back.tabs[0].focused_pane_id.?);
}

test "invalid or no-op drops never mutate native state" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const p = [_]rendering.Pane{ pane("a"), pane("b") };
    const tabs = [_]rendering.Tab{tab("tab-a", &p, &.{ "a", "b" }, .monitor)};
    const input = workspace(&tabs);
    var drop: drag.PaneDrop = .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-a", .to_tab_id = "tab-a", .pane_id = "a", .anchor_pane_id = "a", .placement = .before };
    try std.testing.expect(try drag.planDrop(arena.allocator(), input, drop) == null);
    drop.anchor_pane_id = "b";
    try std.testing.expect(try drag.planDrop(arena.allocator(), input, drop) == null);
    drop.anchor_pane_id = "missing";
    try std.testing.expectError(error.UnknownPane, drag.planDrop(arena.allocator(), input, drop));
    drop.to_tab_id = "missing";
    try std.testing.expectError(error.UnknownTab, drag.planDrop(arena.allocator(), input, drop));
    drop.workspace_id = "other-workspace";
    try std.testing.expectError(error.WorkspaceUnavailable, drag.planDrop(arena.allocator(), input, drop));
    try expectIDs(input.tabs[0].visual_pane_ids, &.{ "a", "b" });
    try std.testing.expectEqual(@as(u64, 4), input.workspace_revision);
}

test "nine Pane Tab still reorders but rejects an incoming tenth Pane" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const ids = [_][]const u8{ "a", "b", "c", "d", "e", "f", "g", "h", "i" };
    var full: [9]rendering.Pane = undefined;
    for (ids, 0..) |id, index| full[index] = pane(id);
    const extra = [_]rendering.Pane{pane("x")};
    const tabs = [_]rendering.Tab{ tab("tab-a", &full, &ids, .monitor), tab("tab-b", &extra, &.{"x"}, .monitor) };
    const input = workspace(&tabs);
    try std.testing.expectError(error.TabFull, drag.planDrop(arena.allocator(), input, .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-b", .to_tab_id = "tab-a", .pane_id = "x" }));
    const result = (try drag.planDrop(arena.allocator(), input, .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-a", .to_tab_id = "tab-a", .pane_id = "i", .anchor_pane_id = "a", .placement = .before })).?;
    try expectIDs(result.tabs[0].visual_pane_ids, &.{ "i", "a", "b", "c", "d", "e", "f", "g", "h" });
}

test "drop-owned IDs survive callback lifetime without allocation leaks" {
    const drop: drag.PaneDrop = .{ .workspace_id = "w", .from_tab_id = "a", .pane_id = "p", .to_tab_id = "b", .anchor_pane_id = "q", .placement = .after };
    const owned = try drop.clone(std.testing.allocator);
    defer owned.deinit(std.testing.allocator);
    try std.testing.expectEqualStrings("q", owned.anchor_pane_id.?);
}

test "native catalog restart restores the exact drop order and Focus MAIN" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const p = [_]rendering.Pane{ pane("a"), pane("b"), pane("c") };
    const tabs = [_]rendering.Tab{tab("tab-a", &p, &.{ "b", "c", "a" }, .focus)};
    const input = workspace(&tabs);
    const result = (try drag.planDrop(arena.allocator(), input, .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-a", .to_tab_id = "tab-a", .pane_id = "a", .anchor_pane_id = "b", .placement = .before })).?;
    const json = try store.encode(arena.allocator(), result);
    var decoded = try store.decode(std.testing.allocator, json);
    defer decoded.deinit();
    try std.testing.expectEqual(result.workspace_revision, decoded.value.workspace_revision);
    try std.testing.expectEqualStrings(result.workspace_id, decoded.value.workspace_id);
    try expectIDs(decoded.value.tabs[0].layout.pane_order, result.tabs[0].layout.pane_order);
    for (result.tabs[0].panes, decoded.value.tabs[0].panes) |before, after| {
        try std.testing.expectEqualStrings(before.id, after.id);
        try std.testing.expectEqualStrings(before.execution_target_id, after.execution_target_id);
        try std.testing.expectEqualStrings(before.tmux_session_id, after.tmux_session_id);
    }
    try expectIDs(decoded.value.tabs[0].visual_pane_ids, &.{ "a", "b", "c" });
    try std.testing.expectEqualStrings("a", decoded.value.tabs[0].focused_pane_id.?);
}

test "all one through nine Pane insertion targets preserve identity and fixed topology" {
    const ids = [_][]const u8{ "a", "b", "c", "d", "e", "f", "g", "h", "i" };
    var panes: [9]rendering.Pane = undefined;
    for (ids, 0..) |id, index| panes[index] = pane(id);
    inline for (.{ rendering.DisplayMode.monitor, rendering.DisplayMode.focus }) |mode| {
        for (1..10) |count| {
            for (0..count) |from| {
                for (0..count) |anchor| {
                    for ([_]drag.Placement{ .before, .after }) |placement| {
                        var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
                        defer arena.deinit();
                        const tabs = [_]rendering.Tab{tab("tab-a", panes[0..count], ids[0..count], mode)};
                        const input = workspace(&tabs);
                        const result = try drag.planDrop(arena.allocator(), input, .{ .workspace_id = input.workspace_id, .from_tab_id = "tab-a", .to_tab_id = "tab-a", .pane_id = ids[from], .anchor_pane_id = ids[anchor], .placement = placement });
                        if (from == anchor) {
                            try std.testing.expect(result == null);
                            continue;
                        }
                        const changed = result orelse input;
                        const visual = changed.tabs[0].visual_pane_ids;
                        try std.testing.expectEqual(count, visual.len);
                        var moved_index: usize = 0;
                        var anchor_index: usize = 0;
                        for (ids[0..count]) |id| {
                            var occurrences: usize = 0;
                            for (visual, 0..) |visible, index| {
                                if (std.mem.eql(u8, id, visible)) occurrences += 1;
                                if (std.mem.eql(u8, ids[from], visible)) moved_index = index;
                                if (std.mem.eql(u8, ids[anchor], visible)) anchor_index = index;
                            }
                            try std.testing.expectEqual(@as(usize, 1), occurrences);
                        }
                        if (placement == .before) {
                            try std.testing.expectEqual(moved_index + 1, anchor_index);
                        } else {
                            try std.testing.expectEqual(anchor_index + 1, moved_index);
                        }
                        const native_mode: layout.Mode = if (mode == .focus) .focus else .monitor;
                        const before = try layout.layout(ids[0..count], native_mode, tabs[0].focused_pane_id);
                        const after = try layout.layout(visual, native_mode, changed.tabs[0].focused_pane_id);
                        try std.testing.expectEqualSlices(usize, before.columnPopulations(), after.columnPopulations());
                    }
                }
            }
        }
    }
}
