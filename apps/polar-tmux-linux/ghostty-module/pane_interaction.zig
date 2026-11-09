//! Pure Pane pointer interaction rules for the GTK Control Center.
//!
//! The Layout Map is rebuilt after an accepted single-click selection. GTK's
//! per-widget GestureClick count therefore cannot be the only double-click
//! source: the second physical click may arrive on a newly created Button.

const std = @import("std");
const rendering = @import("workspace_rendering.zig");

pub const double_click_interval_us: i64 = 400_000;

/// Returns true when the current click is the second click on the same Pane
/// within the native double-click interval. The caller owns the remembered
/// Pane id and updates it after every click, including across widget redraws.
pub fn isDoubleClick(
    previous_pane_id: ?[]const u8,
    previous_at_us: i64,
    current_pane_id: []const u8,
    current_at_us: i64,
) bool {
    const previous = previous_pane_id orelse return false;
    if (!std.mem.eql(u8, previous, current_pane_id)) return false;
    if (current_at_us < previous_at_us) return false;
    return current_at_us - previous_at_us <= double_click_interval_us;
}

/// Native-only drop coordinates: never encoded into the Host protocol.
pub const Placement = enum { before, after, append };
pub const Edge = enum {
    left,
    right,
    up,
    down,

    pub fn placement(self: Edge) Placement {
        return switch (self) {
            .left, .up => .before,
            .right, .down => .after,
        };
    }
};

pub fn dropEdge(x: f64, y: f64, width: f64, height: f64) ?Edge {
    if (!std.math.isFinite(x) or !std.math.isFinite(y) or
        width <= 0 or height <= 0 or x < 0 or y < 0 or x > width or y > height)
        return null;
    const distances = [_]f64{ x, width - x, y, height - y };
    var closest: usize = 0;
    for (distances, 0..) |distance, index| {
        if (distance < distances[closest]) closest = index;
    }
    return @enumFromInt(closest);
}

pub const PaneDrop = struct {
    workspace_id: []const u8,
    from_tab_id: []const u8,
    pane_id: []const u8,
    to_tab_id: []const u8,
    anchor_pane_id: ?[]const u8 = null,
    placement: Placement = .append,

    pub fn clone(self: PaneDrop, allocator: std.mem.Allocator) !PaneDrop {
        const workspace = try allocator.dupe(u8, self.workspace_id);
        errdefer allocator.free(workspace);
        const from = try allocator.dupe(u8, self.from_tab_id);
        errdefer allocator.free(from);
        const pane = try allocator.dupe(u8, self.pane_id);
        errdefer allocator.free(pane);
        const to = try allocator.dupe(u8, self.to_tab_id);
        errdefer allocator.free(to);
        return .{
            .workspace_id = workspace,
            .from_tab_id = from,
            .pane_id = pane,
            .to_tab_id = to,
            .anchor_pane_id = if (self.anchor_pane_id) |id| try allocator.dupe(u8, id) else null,
            .placement = self.placement,
        };
    }

    pub fn deinit(self: PaneDrop, allocator: std.mem.Allocator) void {
        allocator.free(self.workspace_id);
        allocator.free(self.from_tab_id);
        allocator.free(self.pane_id);
        allocator.free(self.to_tab_id);
        if (self.anchor_pane_id) |id| allocator.free(id);
    }
};

/// Uses the latest native snapshot, not a captured index or Host projection.
/// The caller supplies an arena and commits through the existing executor.
/// Unchanged strings/content facts are borrowed until that commit completes.
pub fn planDrop(
    allocator: std.mem.Allocator,
    current: rendering.Snapshot,
    drop: PaneDrop,
) !?rendering.Snapshot {
    if (!std.mem.eql(u8, current.workspace_id, drop.workspace_id)) return error.WorkspaceUnavailable;
    const from_index = tabIndex(current.tabs, drop.from_tab_id) orelse return error.UnknownTab;
    const to_index = tabIndex(current.tabs, drop.to_tab_id) orelse return error.UnknownTab;
    const source = current.tabs[from_index];
    const target = current.tabs[to_index];
    const pane = source.pane(drop.pane_id) orelse return error.UnknownPane;
    if (drop.anchor_pane_id) |anchor| {
        _ = target.pane(anchor) orelse return error.UnknownPane;
        if (std.mem.eql(u8, anchor, drop.pane_id)) return null;
    } else if (drop.placement != .append) return error.UnknownPane;
    if (from_index != to_index and target.panes.len >= 9) return error.TabFull;

    const tabs = try allocator.dupe(rendering.Tab, current.tabs);
    if (from_index != to_index) {
        const remaining = try withoutPane(allocator, source.panes, drop.pane_id);
        const visual = try visualWithoutPane(allocator, source, drop.pane_id);
        tabs[from_index] = try withPaneOrder(allocator, source, remaining, visual);
    }
    const remaining = try withoutPane(allocator, target.panes, drop.pane_id);
    const canonical_ids = try paneIDs(allocator, remaining);
    const insert_at = try insertionIndex(canonical_ids, drop);
    const panes = try allocator.alloc(rendering.Pane, remaining.len + 1);
    @memcpy(panes[0..insert_at], remaining[0..insert_at]);
    panes[insert_at] = pane.*;
    @memcpy(panes[insert_at + 1 ..], remaining[insert_at..]);

    const visual_remaining = try visualWithoutPane(allocator, target, drop.pane_id);
    const visual_at = try insertionIndex(visual_remaining, drop);
    const visual = try allocator.alloc([]const u8, visual_remaining.len + 1);
    @memcpy(visual[0..visual_at], visual_remaining[0..visual_at]);
    visual[visual_at] = drop.pane_id;
    @memcpy(visual[visual_at + 1 ..], visual_remaining[visual_at..]);
    if (from_index == to_index and
        sameIDs(try paneIDs(allocator, target.panes), try paneIDs(allocator, panes)) and
        sameIDs(try visualIDs(allocator, target), visual)) return null;
    tabs[to_index] = try withPaneOrder(allocator, target, panes, visual);
    var result = current;
    result.tabs = tabs;
    result.selected_tab_id = drop.to_tab_id;
    result.workspace_revision = std.math.add(u64, current.workspace_revision, 1) catch
        return error.InvalidWorkspaceRevision;
    result.snapshot_fingerprint = try std.fmt.allocPrint(
        allocator,
        "native:{s}:{d}:pane-drop",
        .{ current.workspace_id, result.workspace_revision },
    );
    return result;
}

fn tabIndex(tabs: []const rendering.Tab, id: []const u8) ?usize {
    for (tabs, 0..) |tab, index| {
        if (std.mem.eql(u8, tab.id, id)) return index;
    }
    return null;
}

fn paneIDs(allocator: std.mem.Allocator, panes: []const rendering.Pane) ![]const []const u8 {
    const ids = try allocator.alloc([]const u8, panes.len);
    for (panes, 0..) |pane, index| ids[index] = pane.id;
    return ids;
}

fn visualIDs(allocator: std.mem.Allocator, tab: rendering.Tab) ![]const []const u8 {
    return if (tab.visual_pane_ids.len == tab.panes.len)
        tab.visual_pane_ids
    else
        try paneIDs(allocator, tab.panes);
}

fn visualWithoutPane(allocator: std.mem.Allocator, tab: rendering.Tab, id: []const u8) ![]const []const u8 {
    const ids = try visualIDs(allocator, tab);
    const result = try allocator.alloc([]const u8, ids.len);
    var count: usize = 0;
    for (ids) |candidate| {
        if (std.mem.eql(u8, candidate, id)) continue;
        result[count] = candidate;
        count += 1;
    }
    return result[0..count];
}

fn withoutPane(allocator: std.mem.Allocator, panes: []const rendering.Pane, id: []const u8) ![]const rendering.Pane {
    const result = try allocator.alloc(rendering.Pane, panes.len);
    var count: usize = 0;
    for (panes) |pane| {
        if (std.mem.eql(u8, pane.id, id)) continue;
        result[count] = pane;
        count += 1;
    }
    return result[0..count];
}

fn insertionIndex(ids: []const []const u8, drop: PaneDrop) !usize {
    if (drop.placement == .append) return ids.len;
    const anchor = drop.anchor_pane_id orelse return error.UnknownPane;
    for (ids, 0..) |id, index| {
        if (std.mem.eql(u8, id, anchor)) return index + @intFromBool(drop.placement == .after);
    }
    return error.UnknownPane;
}

fn sameIDs(a: []const []const u8, b: []const []const u8) bool {
    if (a.len != b.len) return false;
    for (a, b) |left, right| if (!std.mem.eql(u8, left, right)) return false;
    return true;
}

fn withPaneOrder(
    allocator: std.mem.Allocator,
    source: rendering.Tab,
    panes: []const rendering.Pane,
    visual: []const []const u8,
) !rendering.Tab {
    var result = source;
    result.panes = panes;
    result.visual_pane_ids = visual;
    result.focused_pane_id = if (source.display_mode == .focus and visual.len > 0) visual[0] else null;
    result.layout = .{
        .pane_order = try paneIDs(allocator, panes),
        .slots = &.{},
        .fingerprint = "native-owned",
    };
    return result;
}
