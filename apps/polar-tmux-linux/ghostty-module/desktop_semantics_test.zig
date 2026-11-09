const std = @import("std");
const catalog = @import("agent_catalog.zig");
const attention = @import("attention_interaction.zig");
const editor = @import("session_editor_interaction.zig");
test "native adapters consume the shared desktop fixtures" {
    const alloc = std.testing.allocator;
    const bytes = try std.fs.cwd().readFileAlloc(alloc, "crates/polar-tmux-workspace/tests/fixtures/desktop-semantics.json", 1024 * 1024);
    defer alloc.free(bytes);
    const fixture = try std.json.parseFromSlice(std.json.Value, alloc, bytes, .{});
    defer fixture.deinit();
    const expected = fixture.value.object.get("catalog").?.object;
    const rows = try catalog.project(alloc, expected.get("input").?);
    defer rows.deinit();
    for (rows.value, 0..) |row, i| {
        try std.testing.expectEqual(@as(usize, @intCast(expected.get("roots").?.array.items[i].integer)), row.root);
        const parent = expected.get("parents").?.array.items[i];
        try std.testing.expectEqual(if (parent == .null) null else @as(?usize, @intCast(parent.integer)), row.parent);
    }
    var notifications: attention.Engine = .{};
    defer notifications.deinit();
    for (fixture.value.object.get("attention").?.array.items) |step| {
        const output = try notifications.dispatch(alloc, step.object.get("input").?);
        defer output.deinit();
        try std.testing.expectEqual(@as(usize,@intCast(step.object.get("notify").?.integer)), output.value.notify.len);
    }
    var end: editor.Engine = .{};
    defer end.deinit();
    for (fixture.value.object.get("end").?.array.items) |step| {
        const output = try end.dispatch(alloc, step.object.get("input").?);
        defer output.deinit();
        try std.testing.expectEqual(step.object.get("can_submit").?.bool, output.value.view.can_submit);
        try std.testing.expectEqual(step.object.get("loading").?.bool, output.value.view.end_loading);
        try std.testing.expectEqual(step.object.get("pending").?.bool, output.value.view.pending);
        try std.testing.expectEqual(@as(usize,@intCast(step.object.get("effects").?.integer)), output.value.effects.len);
    }
    var keyboard: editor.Engine = .{};
    defer keyboard.deinit();
    for (fixture.value.object.get("keyboard").?.array.items) |step| {
        const output = try keyboard.dispatch(alloc, step.object.get("input").?);
        defer output.deinit();
        try std.testing.expectEqualStrings(step.object.get("focused").?.string, output.value.view.draft.original_id);
        const selected = step.object.get("selected").?.array.items;
        try std.testing.expectEqual(selected.len, output.value.view.selected.len);
        for (selected, output.value.view.selected) |expected_id, actual| try std.testing.expectEqualStrings(expected_id.string, actual);
        try std.testing.expectEqual(step.object.get("pending").?.bool, output.value.view.pending);
    }
}
test "typed native end requests omit unrelated optional Host fields" {
    var end: editor.Engine = .{};
    defer end.deinit();
    const Request = @import("control_center/intent.zig").EditorRequest;
    const output = try end.dispatch(std.testing.allocator, .{ .type = "open_end", .request = Request{
        .type = "preview_end", .execution_target_id = "local", .tmux_session_id = "$1",
    } });
    defer output.deinit();
    try std.testing.expect(output.value.view.end_loading);
    try std.testing.expectEqual(@as(usize,1), output.value.effects.len);
}
