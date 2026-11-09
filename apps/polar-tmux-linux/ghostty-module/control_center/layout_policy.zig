//! Pure Mac-derived Control Center geometry for the Linux native renderer.
//!
//! Pane identity and order arrive as backend facts. Native code derives the
//! approved Monitor/Focus family and integer GtkGrid units without accepting
//! Rust-authored columns, rows, ratios, roles, slots, or split trees.

const model = @import("model.zig");
const native_layout = @import("../native_workspace_layout.zig");
const std = @import("std");

pub const layout_map_min_width: c_int = 200;
pub const layout_map_max_width: c_int = 390;
/// Compatibility alias for callers that still describe the mature maximum.
/// New composition code must use the selected width passed to
/// `bodyLayoutWithMapWidth`.
pub const layout_map_outer_width: c_int = layout_map_max_width;
pub const sessions_outer_width: c_int = 280;
pub const divider_width: c_int = 1;
pub const controller_section_margin: c_int = 10;
pub const layout_list_inset: c_int = 12;
pub const layout_tab_spacing: c_int = 6;
pub const tab_header_body_spacing: c_int = 4;
pub const layout_map_content_width: c_int =
    layout_map_outer_width - (2 * layout_list_inset);
pub const sessions_content_width: c_int =
    sessions_outer_width - (2 * controller_section_margin);
pub const pane_tile_horizontal_padding: c_int = 8;
pub const pane_tile_vertical_padding: c_int = 4;
pub const pane_tile_min_width: c_int = 96;
pub const pane_grid_gap: c_int = 8;
pub const tab_card_horizontal_padding: c_int = 0;
pub const compact_width: c_int = layout_map_outer_width;
pub const sessions_width: c_int = sessions_outer_width;
pub const default_height: c_int = 653;
pub const session_row_min_height: c_int = 44;
pub const session_row_horizontal_padding: c_int = 10;
pub const session_row_corner_radius: c_int = 9;
pub const session_row_spacing: c_int = 6;
pub const tab_header_height: c_int = 32;
pub const focus_row_height: c_int = 48;
pub const monitor_row_height: c_int = 56;
pub const focus_minimum_body_height: c_int = 200;
pub const monitor_minimum_body_height: c_int = 170;

pub fn layoutMapPaneBodyWidth(map_width: c_int) c_int {
    return @max(
        @as(c_int, 1),
        layoutMapContentWidth(map_width) - (2 * tab_card_horizontal_padding),
    );
}

fn scaledPaneBodyMetric(reference_value: c_int, map_width: c_int) c_int {
    const reference_width = layoutMapPaneBodyWidth(layout_map_max_width);
    const selected_width = layoutMapPaneBodyWidth(map_width);
    return @max(
        @as(c_int, 1),
        @divTrunc(
            reference_value * selected_width + @divTrunc(reference_width, 2),
            reference_width,
        ),
    );
}

pub fn scaledPaneGridGap(map_width: c_int) c_int {
    return scaledPaneBodyMetric(pane_grid_gap, map_width);
}

pub fn clampMapWidth(width: c_int) c_int {
    return @min(
        layout_map_max_width,
        @max(layout_map_min_width, width),
    );
}

/// Apply a native divider translation in either direction and hard-clamp the
/// selected Map width before GTK receives it.
pub fn resizedMapWidth(start: c_int, translation: f64) c_int {
    const start_width = clampMapWidth(start);
    const delta: c_int = @intFromFloat(translation);
    return clampMapWidth(start_width + delta);
}

pub fn adaptiveMapWidth(visible_width: c_int) c_int {
    const positive_width = @max(@as(c_int, 0), visible_width);
    return clampMapWidth(@divTrunc(positive_width * 25, 100));
}

pub fn layoutMapContentWidth(map_width: c_int) c_int {
    return @max(
        @as(c_int, 1),
        clampMapWidth(map_width) - (2 * layout_list_inset),
    );
}

pub const SidebarLayout = struct {
    sidebar_visible: bool,
    divider_visible: bool,
    layout_map_visible: bool,
    sessions_outer_width: c_int,
    layout_map_outer_width: c_int,
    controller_width: c_int,
};

/// Resolve every GTK-visible part of the Sessions layout as one atomic state.
///
/// A closed sidebar is removed from layout together with its divider. Merely
/// hiding the sidebar's contents would preserve an empty 280-pixel column and
/// prevent the Layout Map from returning to the compact origin.
pub fn bodyLayout(
    sessions_visible: bool,
    requested_layout_map_visible: bool,
) SidebarLayout {
    return bodyLayoutWithMapWidth(
        sessions_visible,
        requested_layout_map_visible,
        layout_map_max_width,
    );
}

/// Resolve one native composition using the selected Layout Map width.
/// Sessions defaults to 280 points and the one-point divider is present
/// only while both regions are visible.
pub fn bodyLayoutWithMapWidth(
    sessions_visible: bool,
    requested_layout_map_visible: bool,
    requested_map_width: c_int,
) SidebarLayout {
    const map_width = clampMapWidth(requested_map_width);
    const layout_map_visible = requested_layout_map_visible;
    const both_visible = sessions_visible and layout_map_visible;
    return .{
        .sidebar_visible = sessions_visible,
        .divider_visible = both_visible,
        .layout_map_visible = layout_map_visible,
        .sessions_outer_width = if (sessions_visible) sessions_outer_width else 0,
        .layout_map_outer_width = if (layout_map_visible)
            map_width
        else
            0,
        .controller_width = if (both_visible)
            map_width + sessions_outer_width + divider_width
        else if (layout_map_visible) map_width else if (sessions_visible) sessions_outer_width else 0,
    };
}

pub fn sidebarLayout(sessions_visible: bool) SidebarLayout {
    return bodyLayout(sessions_visible, true);
}

pub fn windowWidth(sessions_visible: bool) c_int {
    return sidebarLayout(sessions_visible).controller_width;
}

pub fn windowWidthWithMapWidth(
    sessions_visible: bool,
    map_width: c_int,
) c_int {
    return bodyLayoutWithMapWidth(sessions_visible, true, map_width).controller_width;
}

pub fn paneRemoveVisible(
    selected: bool,
    focused: bool,
    hovered: bool,
) bool {
    return selected or focused or hovered;
}

/// Preserve each mature-Mac Tab as a bounded card instead of distributing
/// unused Control Center height between cards.
pub fn tabBodyHeight(
    display_mode: model.DisplayMode,
    row_span_value: usize,
) c_int {
    const rows: c_int = @intCast(@max(1, row_span_value));
    return switch (display_mode) {
        .focus => @max(focus_minimum_body_height, rows * focus_row_height),
        .monitor => @max(monitor_minimum_body_height, rows * monitor_row_height),
    };
}

pub fn scaledTabBodyHeight(
    display_mode: model.DisplayMode,
    row_span_value: usize,
    map_width: c_int,
) c_int {
    return scaledPaneBodyMetric(
        tabBodyHeight(display_mode, row_span_value),
        map_width,
    );
}

pub const TabCardLayout = struct {
    header_height: c_int,
    body_height: c_int,
    pane_column_count: usize,
    card_hexpand: bool,
    grid_hexpand: bool,
    pane_tile_hexpand: bool,
    pane_overlay_hexpand: bool,
    card_vexpand: bool,
    grid_vexpand: bool,
    pane_tile_vexpand: bool,
    pane_overlay_vexpand: bool,
    remove_overlay_measured: bool,
};

/// One inseparable native composition contract for a Tab card.
///
/// A child action or overlay must not turn the bounded body height back into
/// a vertical expansion request. Keep all GTK expansion and measurement
/// choices next to the density conversion so renderer changes cannot update
/// one layer without revalidating the whole composition.
pub fn tabCardLayout(
    display_mode: model.DisplayMode,
    row_span_value: usize,
    pane_column_count: usize,
) TabCardLayout {
    return .{
        .header_height = tab_header_height,
        .body_height = tabBodyHeight(display_mode, row_span_value),
        .pane_column_count = @max(1, pane_column_count),
        .card_hexpand = true,
        .grid_hexpand = true,
        .pane_tile_hexpand = true,
        .pane_overlay_hexpand = true,
        .card_vexpand = false,
        .grid_vexpand = false,
        // The card and grid stay bounded, but every Pane cell must fill its
        // allocated row so the whole visible tile is one pointer target.
        .pane_tile_vexpand = true,
        .pane_overlay_vexpand = true,
        .remove_overlay_measured = false,
    };
}

pub fn tabCardLayoutForMapWidth(
    display_mode: model.DisplayMode,
    row_span_value: usize,
    pane_column_count: usize,
    map_width: c_int,
) TabCardLayout {
    var result = tabCardLayout(
        display_mode,
        row_span_value,
        pane_column_count,
    );
    result.body_height = scaledTabBodyHeight(
        display_mode,
        row_span_value,
        map_width,
    );
    return result;
}

pub const GridPlacement = struct {
    column: c_int,
    row: c_int,
    column_span: c_int,
    row_span: c_int,
};

/// Derives the GTK tile position from the native visual order that is already
/// installed in Ghostty. The Rust projection remains the source of tile
/// labels and actions, but it is not allowed to reconstruct a stale MAIN-first
/// order after a native Focus swap.
pub fn gridPlacementFromVisualPaneIds(
    visual_pane_ids: []const []const u8,
    display_mode: model.DisplayMode,
    pane_id: []const u8,
) !GridPlacement {
    if (visual_pane_ids.len == 0 or visual_pane_ids.len > 9) {
        return error.InvalidPaneSet;
    }
    const layout = try native_layout.layout(
        visual_pane_ids,
        switch (display_mode) {
            .monitor => .monitor,
            .focus => .focus,
        },
        if (display_mode == .focus) visual_pane_ids[0] else null,
    );
    const slot = for (layout.slotsSlice()) |candidate| {
        if (@import("std").mem.eql(u8, candidate.surface_id, pane_id)) {
            break candidate;
        }
    } else return error.UnknownPane;
    const row_units = commonRowUnits(layout.columnPopulations());
    const population = layout.columnPopulations()[slot.column];
    const row_span = row_units / population;
    var column: usize = 0;
    for (layout.columnsSlice()[0..slot.column]) |candidate| {
        column += candidate.width.numerator;
    }
    return .{
        .column = @intCast(column),
        .row = @intCast(slot.row * row_span),
        .column_span = @intCast(slot.width.numerator),
        .row_span = @intCast(row_span),
    };
}

/// Map width is presentation scale only. The selected native placement remains
/// invariant at every supported width and therefore never becomes a compact
/// equal-grid reflow.
pub fn gridPlacementForMapWidth(
    visual_pane_ids: []const []const u8,
    display_mode: model.DisplayMode,
    pane_id: []const u8,
    map_width: c_int,
) !GridPlacement {
    _ = map_width;
    return gridPlacementFromVisualPaneIds(
        visual_pane_ids,
        display_mode,
        pane_id,
    );
}

pub fn gridPlacement(
    panes: []const model.Pane,
    display_mode: model.DisplayMode,
    focused_pane_id: ?[]const u8,
    pane: model.Pane,
) !GridPlacement {
    if (panes.len == 0 or panes.len > 9) return error.InvalidPaneSet;
    var pane_ids: [9][]const u8 = undefined;
    for (panes, 0..) |candidate, index| pane_ids[index] = candidate.id;
    if (display_mode == .focus and focused_pane_id != null) {
        var visual_pane_ids: [9][]const u8 = undefined;
        visual_pane_ids[0] = focused_pane_id.?;
        var index: usize = 1;
        for (pane_ids[0..panes.len]) |pane_id| {
            if (!@import("std").mem.eql(u8, pane_id, focused_pane_id.?)) {
                visual_pane_ids[index] = pane_id;
                index += 1;
            }
        }
        return gridPlacementFromVisualPaneIds(
            visual_pane_ids[0..panes.len],
            display_mode,
            pane.id,
        );
    }
    return gridPlacementFromVisualPaneIds(
        pane_ids[0..panes.len],
        display_mode,
        pane.id,
    );
}

fn commonRowUnits(populations: []const usize) usize {
    var result: usize = 1;
    for (populations) |population| {
        result = leastCommonMultiple(result, population);
    }
    return result;
}

fn leastCommonMultiple(left: usize, right: usize) usize {
    return (left / greatestCommonDivisor(left, right)) * right;
}

fn greatestCommonDivisor(left_value: usize, right_value: usize) usize {
    var left = left_value;
    var right = right_value;
    while (right != 0) {
        const remainder = left % right;
        left = right;
        right = remainder;
    }
    return if (left == 0) 1 else left;
}
