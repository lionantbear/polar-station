//! Pure geometry contract for the Linux in-window Control Center host.
//!
//! GTK owns the actual allocation. This module freezes the requested
//! grow/shrink behavior so it can be tested without GTK or a compositor.

const layout = @import("layout_policy.zig");

pub const Geometry = struct {
    requested_outer_width: c_int,
    terminal_width: c_int,
    pane_width: c_int,
};

/// Only a normal toplevel owns a default-size request. Maximized and
/// fullscreen frames belong to the compositor and must remain unchanged.
pub fn shouldSetDefaultSize(maximized: bool, fullscreen: bool) bool {
    return !maximized and !fullscreen;
}

/// Resolve the terminal-only baseline when a hidden pane is shown again.
///
/// Wayland applies a toplevel resize asynchronously. A rapid hide/show can
/// therefore observe the previous expanded width even though the hidden state
/// already requested the baseline width. Preserve the stored baseline for that
/// one stale allocation; otherwise accept the current hidden-window width so a
/// user resize while the pane is closed becomes the new baseline.
pub fn baselineForShow(
    stored_baseline_width: c_int,
    last_visible_width: c_int,
    current_width: c_int,
) c_int {
    const current = @max(@as(c_int, 1), current_width);
    const stored = @max(@as(c_int, 0), stored_baseline_width);
    if (stored == 0) return current;
    if (last_visible_width > 0 and current == last_visible_width) return stored;
    return current;
}

pub fn geometry(
    baseline_terminal_width: c_int,
    visible: bool,
    sessions_visible: bool,
    work_area_width: ?c_int,
) Geometry {
    return geometryWithMapWidth(
        baseline_terminal_width,
        visible,
        sessions_visible,
        layout.layout_map_max_width,
        work_area_width,
    );
}

pub fn geometryWithMapWidth(
    baseline_terminal_width: c_int,
    visible: bool,
    sessions_visible: bool,
    map_width: c_int,
    work_area_width: ?c_int,
) Geometry {
    return geometryForPaneWidth(
        baseline_terminal_width,
        visible,
        layout.windowWidthWithMapWidth(sessions_visible, map_width),
        work_area_width,
    );
}

/// Resolve child allocations for an already measured controller pane.
/// `available_outer_width` is the compositor-configured width while maximized;
/// a normal window passes null and may request baseline plus controller width.
pub fn geometryForPaneWidth(
    baseline_terminal_width: c_int,
    visible: bool,
    desired_pane_width: c_int,
    available_outer_width: ?c_int,
) Geometry {
    const baseline = @max(@as(c_int, 0), baseline_terminal_width);
    if (!visible) {
        const requested_outer_width = constrained(
            baseline,
            available_outer_width,
        );
        return .{
            .requested_outer_width = requested_outer_width,
            .terminal_width = requested_outer_width,
            .pane_width = 0,
        };
    }

    const desired_pane = @max(@as(c_int, 0), desired_pane_width);
    const desired_outer_width = baseline + desired_pane + layout.divider_width;
    const requested_outer_width = constrained(
        desired_outer_width,
        available_outer_width,
    );
    const pane_width = @min(
        desired_pane,
        @max(@as(c_int, 0), requested_outer_width - layout.divider_width),
    );
    return .{
        .requested_outer_width = requested_outer_width,
        .terminal_width = @max(
            @as(c_int, 0),
            requested_outer_width - pane_width - layout.divider_width,
        ),
        .pane_width = pane_width,
    };
}

fn constrained(
    desired: c_int,
    work_area_width: ?c_int,
) c_int {
    const available = work_area_width orelse return desired;
    return @min(desired, @max(@as(c_int, 0), available));
}
