//! Polar Station Workspace geometry owned by the Linux Ghostty adapter.
//!
//! No Rust projection supplies these columns, rows, ratios, roles, or trees.
//! The adapter derives them immediately from its native ordered Surface list.

const std = @import("std");

pub const Mode = enum { monitor, focus };

pub const FocusCycleDirection = enum { previous, next };

pub const FocusCycle = struct {
    visual_pane_ids: [9][]const u8 = undefined,
    pane_count: usize,
    changed: bool,

    pub fn visualPaneIds(self: *const FocusCycle) []const []const u8 {
        return self.visual_pane_ids[0..self.pane_count];
    }

    pub fn mainPaneId(self: *const FocusCycle) ?[]const u8 {
        if (self.pane_count == 0) return null;
        return self.visual_pane_ids[0];
    }
};

pub const Ratio = struct {
    numerator: usize,
    denominator: usize,

    pub fn init(numerator: usize, denominator: usize) Ratio {
        std.debug.assert(numerator > 0 and denominator > 0);
        return .{ .numerator = numerator, .denominator = denominator };
    }
};

pub const Role = union(enum) {
    monitor,
    main,
    sidebar: usize,
};

pub const Slot = struct {
    surface_id: []const u8,
    visual_index: usize,
    column: usize,
    row: usize,
    width: Ratio,
    height: Ratio,
    role: Role,
};

pub const Column = struct {
    index: usize,
    width: Ratio,
    first_visual_index: usize,
    surface_count: usize,
};

pub const Tree = struct {
    pub const Axis = enum { horizontal, vertical };
    pub const Node = union(enum) {
        surface: []const u8,
        split: struct {
            axis: Axis,
            first: u8,
            second: u8,
            ratio: Ratio,
        },
    };

    nodes: [17]Node = undefined,
    node_count: u8 = 0,
    root: u8 = 0,

    fn append(self: *Tree, node: Node) !u8 {
        if (self.node_count >= self.nodes.len) return error.TreeCapacityExceeded;
        const index = self.node_count;
        self.nodes[index] = node;
        self.node_count += 1;
        return index;
    }

    fn surface(self: *Tree, surface_id: []const u8) !u8 {
        return self.append(.{ .surface = surface_id });
    }

    fn joinWeightedRightAssociated(
        self: *Tree,
        roots: []const u8,
        weights: []const usize,
        axis: Axis,
    ) !u8 {
        std.debug.assert(roots.len > 0 and roots.len == weights.len);
        if (roots.len == 1) return roots[0];
        const second = try self.joinWeightedRightAssociated(
            roots[1..],
            weights[1..],
            axis,
        );
        var total_weight = weights[0];
        for (weights[1..]) |weight| total_weight += weight;
        return self.append(.{ .split = .{
            .axis = axis,
            .first = roots[0],
            .second = second,
            .ratio = Ratio.init(weights[0], total_weight),
        } });
    }

    pub fn describeAlloc(
        self: *const Tree,
        allocator: std.mem.Allocator,
    ) ![]u8 {
        return self.describeNodeAlloc(allocator, self.root);
    }

    fn describeNodeAlloc(
        self: *const Tree,
        allocator: std.mem.Allocator,
        index: u8,
    ) ![]u8 {
        return switch (self.nodes[index]) {
            .surface => |surface_id| allocator.dupe(u8, surface_id),
            .split => |split| blk: {
                const first = try self.describeNodeAlloc(allocator, split.first);
                defer allocator.free(first);
                const second = try self.describeNodeAlloc(allocator, split.second);
                defer allocator.free(second);
                const prefix: u8 = if (split.axis == .horizontal) 'H' else 'V';
                break :blk std.fmt.allocPrint(
                    allocator,
                    "{c}({s},{s})",
                    .{ prefix, first, second },
                );
            },
        };
    }
};

pub const Layout = struct {
    mode: Mode,
    visual_pane_ids: [9][]const u8 = undefined,
    pane_count: usize,
    column_populations: [3]usize = .{ 0, 0, 0 },
    column_count: usize,
    columns: [3]Column = undefined,
    slots: [9]Slot = undefined,
    tree: Tree,

    pub fn visualPaneIds(self: *const Layout) []const []const u8 {
        return self.visual_pane_ids[0..self.pane_count];
    }

    pub fn columnPopulations(self: *const Layout) []const usize {
        return self.column_populations[0..self.column_count];
    }

    pub fn columnsSlice(self: *const Layout) []const Column {
        return self.columns[0..self.column_count];
    }

    pub fn slotsSlice(self: *const Layout) []const Slot {
        return self.slots[0..self.pane_count];
    }
};

const monitor_populations = [9][3]usize{
    .{ 1, 0, 0 },
    .{ 1, 1, 0 },
    .{ 1, 2, 0 },
    .{ 1, 2, 1 },
    .{ 1, 2, 2 },
    .{ 2, 2, 2 },
    .{ 2, 3, 2 },
    .{ 2, 3, 3 },
    .{ 3, 3, 3 },
};

const focus_populations = [9][3]usize{
    .{ 1, 0, 0 },
    .{ 1, 1, 0 },
    .{ 1, 2, 0 },
    .{ 1, 2, 1 },
    .{ 1, 2, 2 },
    .{ 1, 2, 3 },
    .{ 1, 3, 3 },
    .{ 1, 3, 4 },
    .{ 1, 4, 4 },
};

/// Advances through canonical Pane order while changing only the physical
/// MAIN slot and the target Pane's current visual slot. Surface identity and
/// every unaffected visual slot remain stable.
pub fn cycleFocus(
    canonical_pane_ids: []const []const u8,
    current_visual_pane_ids: []const []const u8,
    direction: FocusCycleDirection,
) !FocusCycle {
    if (canonical_pane_ids.len == 0 or canonical_pane_ids.len > 9 or
        canonical_pane_ids.len != current_visual_pane_ids.len)
    {
        return error.UnsupportedPaneCount;
    }
    try validateIdentitySet(canonical_pane_ids);
    try validateIdentitySet(current_visual_pane_ids);
    for (canonical_pane_ids) |canonical_id| {
        if (indexOfIdentity(current_visual_pane_ids, canonical_id) == null) {
            return error.InvalidPaneIdentity;
        }
    }

    var result: FocusCycle = .{
        .pane_count = canonical_pane_ids.len,
        .changed = canonical_pane_ids.len > 1,
    };
    for (current_visual_pane_ids, 0..) |pane_id, index| {
        result.visual_pane_ids[index] = pane_id;
    }
    if (!result.changed) return result;

    const canonical_main_index = indexOfIdentity(
        canonical_pane_ids,
        current_visual_pane_ids[0],
    ) orelse return error.InvalidPaneIdentity;
    const target_canonical_index = switch (direction) {
        .previous => if (canonical_main_index == 0)
            canonical_pane_ids.len - 1
        else
            canonical_main_index - 1,
        .next => (canonical_main_index + 1) % canonical_pane_ids.len,
    };
    const target_visual_index = indexOfIdentity(
        current_visual_pane_ids,
        canonical_pane_ids[target_canonical_index],
    ) orelse return error.InvalidPaneIdentity;
    std.mem.swap(
        []const u8,
        &result.visual_pane_ids[0],
        &result.visual_pane_ids[target_visual_index],
    );
    return result;
}

/// Promotes one existing visual Pane to MAIN without rebuilding the order from
/// the Rust canonical list. The old MAIN and target exchange their current
/// visual slots; every other Surface keeps its slot and identity.
pub fn promoteFocus(
    current_visual_pane_ids: []const []const u8,
    target_pane_id: []const u8,
) !FocusCycle {
    if (current_visual_pane_ids.len == 0 or
        current_visual_pane_ids.len > 9)
    {
        return error.UnsupportedPaneCount;
    }
    try validateIdentitySet(current_visual_pane_ids);
    const target_index = indexOfIdentity(
        current_visual_pane_ids,
        target_pane_id,
    ) orelse return error.InvalidPaneIdentity;

    var result: FocusCycle = .{
        .pane_count = current_visual_pane_ids.len,
        .changed = target_index != 0,
    };
    for (current_visual_pane_ids, 0..) |pane_id, index| {
        result.visual_pane_ids[index] = pane_id;
    }
    if (result.changed) {
        std.mem.swap(
            []const u8,
            &result.visual_pane_ids[0],
            &result.visual_pane_ids[target_index],
        );
    }
    return result;
}

fn validateIdentitySet(pane_ids: []const []const u8) !void {
    for (pane_ids, 0..) |pane_id, index| {
        if (!validIdentifier(pane_id)) return error.InvalidPaneIdentity;
        if (indexOfIdentity(pane_ids[0..index], pane_id) != null) {
            return error.InvalidPaneIdentity;
        }
    }
}

fn indexOfIdentity(pane_ids: []const []const u8, expected: []const u8) ?usize {
    for (pane_ids, 0..) |pane_id, index| {
        if (std.mem.eql(u8, pane_id, expected)) return index;
    }
    return null;
}

pub fn layout(
    ordered_pane_ids: []const []const u8,
    mode: Mode,
    main_pane_id: ?[]const u8,
) !Layout {
    if (ordered_pane_ids.len == 0 or ordered_pane_ids.len > 9) {
        return error.UnsupportedPaneCount;
    }
    for (ordered_pane_ids, 0..) |pane_id, index| {
        if (!validIdentifier(pane_id)) return error.InvalidPaneIdentity;
        for (ordered_pane_ids[0..index]) |previous| {
            if (std.mem.eql(u8, pane_id, previous)) {
                return error.InvalidPaneIdentity;
            }
        }
    }

    var result: Layout = .{
        .mode = mode,
        .pane_count = ordered_pane_ids.len,
        .column_count = 0,
        .tree = .{},
    };
    switch (mode) {
        .monitor => {
            for (ordered_pane_ids, 0..) |pane_id, index| {
                result.visual_pane_ids[index] = pane_id;
            }
            result.column_populations = monitor_populations[ordered_pane_ids.len - 1];
        },
        .focus => {
            const main = main_pane_id orelse return error.MissingMainPane;
            var found = false;
            for (ordered_pane_ids) |pane_id| {
                if (std.mem.eql(u8, pane_id, main)) {
                    found = true;
                    break;
                }
            }
            if (!found) return error.MissingMainPane;
            result.visual_pane_ids[0] = main;
            var target_index: usize = 1;
            for (ordered_pane_ids) |pane_id| {
                if (!std.mem.eql(u8, pane_id, main)) {
                    result.visual_pane_ids[target_index] = pane_id;
                    target_index += 1;
                }
            }
            result.column_populations = focus_populations[ordered_pane_ids.len - 1];
        },
    }
    while (result.column_count < result.column_populations.len and
        result.column_populations[result.column_count] != 0)
    {
        result.column_count += 1;
    }

    var visual_index: usize = 0;
    var column_roots: [3]u8 = undefined;
    for (result.column_populations[0..result.column_count], 0..) |population, column_index| {
        const width = columnWidth(mode, ordered_pane_ids.len, result.column_count, column_index);
        result.columns[column_index] = .{
            .index = column_index,
            .width = width,
            .first_visual_index = visual_index,
            .surface_count = population,
        };
        var row_roots: [9]u8 = undefined;
        var row_weights: [9]usize = undefined;
        for (0..population) |row| {
            const pane_id = result.visual_pane_ids[visual_index];
            const role: Role = switch (mode) {
                .monitor => .monitor,
                .focus => if (visual_index == 0) .main else .{ .sidebar = visual_index },
            };
            result.slots[visual_index] = .{
                .surface_id = pane_id,
                .visual_index = visual_index,
                .column = column_index,
                .row = row,
                .width = width,
                .height = Ratio.init(1, population),
                .role = role,
            };
            row_roots[row] = try result.tree.surface(pane_id);
            row_weights[row] = 1;
            visual_index += 1;
        }
        column_roots[column_index] = try result.tree.joinWeightedRightAssociated(
            row_roots[0..population],
            row_weights[0..population],
            .vertical,
        );
    }
    var column_weights: [3]usize = undefined;
    for (result.columns[0..result.column_count], 0..) |column, index| {
        column_weights[index] = column.width.numerator;
    }
    result.tree.root = try result.tree.joinWeightedRightAssociated(
        column_roots[0..result.column_count],
        column_weights[0..result.column_count],
        .horizontal,
    );
    return result;
}

fn columnWidth(
    mode: Mode,
    pane_count: usize,
    column_count: usize,
    column_index: usize,
) Ratio {
    return switch (mode) {
        .monitor => Ratio.init(1, column_count),
        .focus => if (pane_count == 1)
            Ratio.init(1, 1)
        else if (pane_count <= 3)
            (if (column_index == 0) Ratio.init(4, 5) else Ratio.init(1, 5))
        else if (column_index == 0)
            Ratio.init(3, 5)
        else
            Ratio.init(1, 5),
    };
}

fn validIdentifier(value: []const u8) bool {
    if (value.len == 0 or value.len > 256) return false;
    if (value[0] == ' ' or value[value.len - 1] == ' ') return false;
    for (value) |byte| {
        if (byte < 0x20 or byte == 0x7f) return false;
    }
    return true;
}













