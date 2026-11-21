//! Local-only shared Rust policy. No catalog or window state is stored here.
const std = @import("std");
const c = @import("polar_station_c");
pub const Decision = struct {
    effect: []const u8 = "none",
    selected: []const u8 = "",
    @"error": ?[]const u8 = null,
    can_archive: bool = false,
};
pub fn decide(allocator: std.mem.Allocator, input: anytype) !std.json.Parsed(Decision) {
    const bytes = try std.json.Stringify.valueAlloc(allocator, input, .{});
    defer allocator.free(bytes);
    const result = c.polar_station_workspace_lifecycle(bytes.ptr, bytes.len) orelse return error.InvalidLifecycleInput;
    defer c.polar_station_desktop_string_free(result);
    return std.json.parseFromSlice(Decision, allocator, std.mem.span(result), .{ .allocate = .alloc_always });
}

const rendering = @import("workspace_rendering.zig");
const Session = @import("control_center/model.zig").SessionIdentity;

pub const PanePlacement = struct {
    additions: []const Session,
    existing: []const []const u8,
    full: bool,
};

pub fn planPanePlacement(
    allocator: std.mem.Allocator,
    tab: rendering.Tab,
    sessions: []const Session,
    reserved: usize,
    unbound: usize,
) !std.json.Parsed(PanePlacement) {
    const Member = struct { id: []const u8, session: ?Session };
    const panes = try allocator.alloc(Member, tab.panes.len);
    defer allocator.free(panes);
    for (panes, tab.panes) |*member, pane| {
        member.* = .{ .id = pane.id, .session = if (pane.isTerminal()) .{
            .execution_target_id = pane.execution_target_id,
            .tmux_session_id = pane.tmux_session_id,
        } else null };
    }
    const bytes = try std.json.Stringify.valueAlloc(allocator, .{ .panes = panes, .sessions = sessions, .reserved = reserved, .unbound = unbound }, .{});
    defer allocator.free(bytes);
    const result = c.polar_station_plan_pane_placement(bytes.ptr, bytes.len) orelse return error.InvalidPanePlacement;
    defer c.polar_station_desktop_string_free(result);
    return std.json.parseFromSlice(PanePlacement, allocator, std.mem.span(result), .{ .allocate = .alloc_always });
}
