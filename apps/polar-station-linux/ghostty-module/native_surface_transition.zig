//! Minimal Ghostty Surface-pointer changes owned by the Linux adapter.
//!
//! Stable logical Panes retain their existing `*Surface`. Pure geometry and
//! Focus-main changes replace only the native `Surface.Tree`; Rust is not
//! called because terminal content did not change.

const std = @import("std");
const native_layout = @import("native_workspace_layout.zig");

/// Control Center selection is not terminal focus while a Tab is in Focus.
/// Monitor may retain the previous MAIN or an explicitly selected terminal.
pub fn terminalFocusPaneId(
    mode: native_layout.Mode,
    main_pane_id: ?[]const u8,
    selected_pane_id: ?[]const u8,
) ?[]const u8 {
    return if (mode == .focus) main_pane_id else selected_pane_id;
}



pub const Plan = struct {
    reused_surface_ids: [9][]const u8 = undefined,
    reused_count: usize = 0,
    created_surface_ids: [9][]const u8 = undefined,
    created_count: usize = 0,
    retired_surface_ids: [9][]const u8 = undefined,
    retired_count: usize = 0,
    rust_content_request_count: usize = 0,
    replace_native_tree: bool = true,

    pub fn reused(self: *const Plan) []const []const u8 {
        return self.reused_surface_ids[0..self.reused_count];
    }

    pub fn created(self: *const Plan) []const []const u8 {
        return self.created_surface_ids[0..self.created_count];
    }

    pub fn retired(self: *const Plan) []const []const u8 {
        return self.retired_surface_ids[0..self.retired_count];
    }
};

pub fn plan(
    current_pane_ids: []const []const u8,
    target_pane_ids: []const []const u8,
) !Plan {
    try validateSet(current_pane_ids);
    try validateSet(target_pane_ids);
    var result: Plan = .{};
    for (target_pane_ids) |pane_id| {
        if (contains(current_pane_ids, pane_id)) {
            result.reused_surface_ids[result.reused_count] = pane_id;
            result.reused_count += 1;
        } else {
            result.created_surface_ids[result.created_count] = pane_id;
            result.created_count += 1;
        }
    }
    for (current_pane_ids) |pane_id| {
        if (!contains(target_pane_ids, pane_id)) {
            result.retired_surface_ids[result.retired_count] = pane_id;
            result.retired_count += 1;
        }
    }
    result.rust_content_request_count = result.created_count + result.retired_count;
    return result;
}

pub const CrossTabMovePlan = struct {
    pane_id: []const u8,
    detach_pointer_from_source_tree: bool = true,
    attach_same_pointer_to_target_tree: bool = true,
    created_surface_count: usize = 0,
    retired_surface_count: usize = 0,
    rust_content_request_count: usize = 0,
};

pub const ContentMutation = enum {
    reuse_surface,
    rebind_content,
    move_surface,
    move_and_rebind,
};

pub fn planContentMutation(
    current_tab_id: []const u8,
    current_attach_fingerprint: []const u8,
    target_tab_id: []const u8,
    target_attach_fingerprint: []const u8,
) ContentMutation {
    const same_tab = std.mem.eql(u8, current_tab_id, target_tab_id);
    const same_content = std.mem.eql(
        u8,
        current_attach_fingerprint,
        target_attach_fingerprint,
    );
    if (same_tab and same_content) return .reuse_surface;
    if (same_tab) return .rebind_content;
    if (same_content) return .move_surface;
    return .move_and_rebind;
}

pub fn planCrossTabMove(
    pane_id: []const u8,
    source_pane_ids: []const []const u8,
    target_pane_ids: []const []const u8,
) !CrossTabMovePlan {
    try validateSet(source_pane_ids);
    try validateSet(target_pane_ids);
    if (!contains(source_pane_ids, pane_id) or contains(target_pane_ids, pane_id)) {
        return error.InvalidCrossTabMove;
    }
    return .{ .pane_id = pane_id };
}

fn validateSet(pane_ids: []const []const u8) !void {
    if (pane_ids.len > 9) return error.UnsupportedPaneCount;
    for (pane_ids, 0..) |pane_id, index| {
        if (pane_id.len == 0) return error.InvalidPaneIdentity;
        for (pane_ids[0..index]) |previous| {
            if (std.mem.eql(u8, pane_id, previous)) {
                return error.InvalidPaneIdentity;
            }
        }
    }
}

fn contains(pane_ids: []const []const u8, wanted: []const u8) bool {
    for (pane_ids) |pane_id| {
        if (std.mem.eql(u8, pane_id, wanted)) return true;
    }
    return false;
}










