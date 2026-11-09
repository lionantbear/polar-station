//! Pure local matching rules for Workspace snapshot reconciliation.
//!
//! The wire carries only logical state. Native bindings may include both the
//! currently applied object and an older object retained while a candidate is
//! being verified. Prefer an exact retained binding over starting a new
//! terminal channel when Rust resends the authoritative snapshot.

const std = @import("std");

pub const BindingDecision = union(enum) {
    reuse_active: usize,
    reactivate: usize,
    create,
};

pub fn paneBindingDecision(
    bindings: anytype,
    pane_id: []const u8,
    tab_id: []const u8,
    attach_fingerprint: []const u8,
) BindingDecision {
    var inactive_match: ?usize = null;
    var index = bindings.len;
    while (index > 0) {
        index -= 1;
        const binding = bindings[index];
        if (!std.mem.eql(u8, binding.pane_id, pane_id) or
            !std.mem.eql(u8, binding.tab_id, tab_id) or
            !std.mem.eql(
                u8,
                binding.attach_fingerprint,
                attach_fingerprint,
            ))
        {
            continue;
        }
        if (binding.active) return .{ .reuse_active = index };
        if (inactive_match == null) inactive_match = index;
    }
    if (inactive_match) |match| return .{ .reactivate = match };
    return .create;
}

pub fn tabBindingDecision(
    bindings: anytype,
    tab_id: []const u8,
) BindingDecision {
    var inactive_match: ?usize = null;
    var index = bindings.len;
    while (index > 0) {
        index -= 1;
        const binding = bindings[index];
        if (!std.mem.eql(u8, binding.tab_id, tab_id)) continue;
        if (binding.active) return .{ .reuse_active = index };
        if (inactive_match == null) inactive_match = index;
    }
    if (inactive_match) |match| return .{ .reactivate = match };
    return .create;
}
