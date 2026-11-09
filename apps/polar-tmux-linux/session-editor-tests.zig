const std = @import("std");
const intent = @import("ghostty-module/control_center/intent.zig");
test "Session editor reads have no mutation revision or unrelated fields" {
    const alloc = std.testing.allocator;
    const json = try intent.encodeAction(alloc, "read-directory", "private", 42, .{ .session_editor = .{
        .generation = 7,
        .request = .{ .type = "browse_directory", .execution_target_id = "dev06", .path = "/work/中文 project" },
    } });
    defer alloc.free(json);
    const value = try std.json.parseFromSlice(std.json.Value, alloc, json, .{});
    defer value.deinit();
    const envelope = value.value.object.get("request").?.object;
    try std.testing.expect(envelope.get("expected_control_revision") == null);
    const command = envelope.get("command").?.object;
    try std.testing.expectEqualStrings("session_editor", command.get("type").?.string);
    const request = command.get("request").?.object;
    try std.testing.expectEqualStrings("dev06", request.get("execution_target_id").?.string);
    try std.testing.expectEqualStrings("/work/中文 project", request.get("path").?.string);
    try std.testing.expect(request.get("source_session_id") == null);
    try std.testing.expect(request.get("tmux_session_id") == null);
}
test "Codex creation explicitly forwards the human YOLO choice" {
    const alloc = std.testing.allocator;
    const json = try intent.encodeAction(alloc, "create", "private", 42, .{ .create_codex_session = .{
        .execution_target_id = "dev06",
        .endpoint_id = "original",
        .name = "Work",
        .working_directory = "/work",
        .yolo = true,
    } });
    defer alloc.free(json);
    const value = try std.json.parseFromSlice(std.json.Value, alloc, json, .{});
    defer value.deinit();
    const operation = value.value.object.get("request").?.object.get("command").?.object.get("intent").?.object;
    try std.testing.expect(operation.get("yolo").?.bool);
    try std.testing.expectEqualStrings("original", operation.get("endpoint_id").?.string);
}
