//! Full Workspace snapshot wire contract for the managed Ghostty frontend.
//!
//! Native Station owns Workspace meaning and persists its desired state.
//! Rust supplies only backend facts and the compatible logical wire mirror.
//! This module decodes that state without accepting Host-authored physical
//! Ghostty identifiers or operation lists. The GTK executor owns those local
//! details and returns only a logical render receipt.

const std = @import("std");
const attach_argv = @import("attach_argv.zig");
const protocol = @import("protocol.zig");

pub const schema_version: u32 = 2;

pub const Ratio = struct {
    numerator: u32,
    denominator: u32,
};

pub const LayoutRole = union(enum) {
    monitor,
    main,
    sidebar: struct { slot: u8 },
};

pub const LayoutSlot = struct {
    pane_id: []const u8,
    canonical_index: usize,
    visual_index: usize,
    column: usize,
    row: usize,
    width: Ratio,
    height: Ratio,
    role: LayoutRole,
};

pub const Layout = struct {
    pane_order: []const []const u8,
    slots: []const LayoutSlot,
    fingerprint: []const u8,
};

pub const AttachPlan = struct {
    program: []const u8,
    args: []const []const u8,
};

pub const DisplayMode = enum {
    monitor,
    focus,
};

pub const TransportStatus = enum {
    online,
    offline,
    recovering,
};

pub const Content = @import("station_content.zig").Content;

pub const Pane = struct {
    id: []const u8,
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
    transport_status: TransportStatus,
    attach_fingerprint: []const u8,
    attach_plan: AttachPlan,
    content: ?Content = null,

    pub fn isTerminal(self: Pane) bool { return self.content == null; }
};

pub const Tab = struct {
    id: []const u8,
    title: []const u8,
    display_mode: DisplayMode,
    focused_pane_id: ?[]const u8,
    visual_pane_ids: []const []const u8 = &.{},
    layout: Layout,
    panes: []const Pane,

    pub fn pane(self: Tab, pane_id: []const u8) ?*const Pane {
        for (self.panes) |*candidate| {
            if (std.mem.eql(u8, candidate.id, pane_id)) return candidate;
        }
        return null;
    }
};

pub const Snapshot = struct {
    layout_map_visible: ?bool = null,
    inbox_visible: ?bool = null,
    workspace_id: []const u8,
    workspace_name: []const u8,
    workspace_revision: u64,
    font_size_points: u8,
    selected_tab_id: []const u8,
    tabs: []const Tab,
    snapshot_fingerprint: []const u8,

    pub fn tab(self: Snapshot, tab_id: []const u8) ?*const Tab {
        for (self.tabs) |*candidate| {
            if (std.mem.eql(u8, candidate.id, tab_id)) return candidate;
        }
        return null;
    }

    pub fn pane(self: Snapshot, pane_id: []const u8) ?struct {
        tab: *const Tab,
        pane: *const Pane,
    } {
        for (self.tabs) |*candidate_tab| {
            if (candidate_tab.pane(pane_id)) |candidate_pane| {
                return .{ .tab = candidate_tab, .pane = candidate_pane };
            }
        }
        return null;
    }
};

pub const RenderRequest = struct {
    arena: std.heap.ArenaAllocator,
    transaction_id: []const u8,
    snapshot: Snapshot,

    pub fn parse(
        allocator: std.mem.Allocator,
        json: []const u8,
        identity: protocol.FrontendIdentity,
    ) !RenderRequest {
        try identity.validate();
        var arena = std.heap.ArenaAllocator.init(allocator);
        errdefer arena.deinit();
        const alloc = arena.allocator();
        const root = try std.json.parseFromSliceLeaky(
            std.json.Value,
            alloc,
            json,
            .{ .allocate = .alloc_always },
        );
        const object = try requireObject(&root);
        if (!std.mem.eql(u8, try requireString(object, "type"), "render_workspace")) {
            return error.InvalidFrame;
        }
        try requireFrontend(object, identity);
        const transaction_id = try requireIdentifier(object, "transaction_id");
        const snapshot = try parseSnapshot(
            alloc,
            try requireObject(try requireValue(object, "snapshot")),
        );
        return .{
            .arena = arena,
            .transaction_id = transaction_id,
            .snapshot = snapshot,
        };
    }

    pub fn deinit(self: *RenderRequest) void {
        self.arena.deinit();
        self.* = undefined;
    }
};

const AppliedResult = struct {
    status: []const u8 = "applied",
};

const RejectedResult = struct {
    status: []const u8 = "rejected",
    @"error": struct {
        code: []const u8,
        message: []const u8,
        retryable: bool,
    },
};

fn Receipt(comptime Result: type) type {
    return struct {
        transaction_id: []const u8,
        frontend_instance_id: []const u8,
        frontend_generation: u64,
        workspace_id: []const u8,
        workspace_revision: u64,
        snapshot_fingerprint: []const u8,
        result: Result,
    };
}

fn ReceiptFrame(comptime Result: type) type {
    return struct {
        type: []const u8 = "workspace_rendered",
        receipt: Receipt(Result),
    };
}

pub fn encodeAppliedReceipt(
    allocator: std.mem.Allocator,
    identity: protocol.FrontendIdentity,
    transaction_id: []const u8,
    workspace_id: []const u8,
    workspace_revision: u64,
    snapshot_fingerprint: []const u8,
) ![]u8 {
    try validateReceiptFields(
        identity,
        transaction_id,
        workspace_id,
        workspace_revision,
        snapshot_fingerprint,
    );
    return std.json.Stringify.valueAlloc(
        allocator,
        ReceiptFrame(AppliedResult){
            .receipt = .{
                .transaction_id = transaction_id,
                .frontend_instance_id = identity.frontend_instance_id,
                .frontend_generation = identity.frontend_generation,
                .workspace_id = workspace_id,
                .workspace_revision = workspace_revision,
                .snapshot_fingerprint = snapshot_fingerprint,
                .result = .{},
            },
        },
        .{ .whitespace = .minified },
    );
}

pub fn encodeRejectedReceipt(
    allocator: std.mem.Allocator,
    identity: protocol.FrontendIdentity,
    transaction_id: []const u8,
    workspace_id: []const u8,
    workspace_revision: u64,
    snapshot_fingerprint: []const u8,
    code: []const u8,
    message: []const u8,
    retryable: bool,
) ![]u8 {
    try validateReceiptFields(
        identity,
        transaction_id,
        workspace_id,
        workspace_revision,
        snapshot_fingerprint,
    );
    try validateIdentifier(code);
    try validateDisplayText(message);
    return std.json.Stringify.valueAlloc(
        allocator,
        ReceiptFrame(RejectedResult){
            .receipt = .{
                .transaction_id = transaction_id,
                .frontend_instance_id = identity.frontend_instance_id,
                .frontend_generation = identity.frontend_generation,
                .workspace_id = workspace_id,
                .workspace_revision = workspace_revision,
                .snapshot_fingerprint = snapshot_fingerprint,
                .result = .{
                    .@"error" = .{
                        .code = code,
                        .message = message,
                        .retryable = retryable,
                    },
                },
            },
        },
        .{ .whitespace = .minified },
    );
}

fn parseSnapshot(
    allocator: std.mem.Allocator,
    object: *const std.json.ObjectMap,
) !Snapshot {
    if (try requireU32(object, "schema_version") != schema_version) {
        return error.UnsupportedSchemaVersion;
    }
    const workspace_id = try requireIdentifier(object, "workspace_id");
    const workspace_name = try requireDisplayText(object, "workspace_name");
    const workspace_revision = try requireU64(object, "workspace_revision");
    if (workspace_revision == 0) return error.InvalidRevision;
    const font_size_points = try requireU8(
        object,
        "managed_terminal_font_size_points",
    );
    if (font_size_points < 10 or font_size_points > 32) {
        return error.InvalidFontSize;
    }
    const selected_tab_id = try requireIdentifier(object, "selected_tab_id");
    const tab_values = try requireArray(object, "tabs");
    if (tab_values.len == 0) return error.InvalidTabSet;
    const tabs = try allocator.alloc(Tab, tab_values.len);
    var pane_ids = std.StringHashMapUnmanaged(void).empty;
    defer pane_ids.deinit(allocator);
    for (tab_values, 0..) |*value, index| {
        tabs[index] = try parseTab(allocator, try requireObject(value));
        for (tabs[0..index]) |prior| {
            if (std.mem.eql(u8, prior.id, tabs[index].id)) {
                return error.DuplicateTab;
            }
        }
        for (tabs[index].panes) |pane| {
            const entry = try pane_ids.getOrPut(allocator, pane.id);
            if (entry.found_existing) return error.DuplicatePane;
        }
    }
    var selected_exists = false;
    for (tabs) |tab| {
        if (std.mem.eql(u8, tab.id, selected_tab_id)) selected_exists = true;
    }
    if (!selected_exists) return error.UnknownSelectedTab;

    const fingerprint = try requireDisplayText(object, "snapshot_fingerprint");
    if (object.getPtr("error")) |value| try validateOptionalError(value);
    return .{
        .workspace_id = workspace_id,
        .workspace_name = workspace_name,
        .workspace_revision = workspace_revision,
        .font_size_points = font_size_points,
        .selected_tab_id = selected_tab_id,
        .tabs = tabs,
        .snapshot_fingerprint = fingerprint,
    };
}

fn parseTab(
    allocator: std.mem.Allocator,
    object: *const std.json.ObjectMap,
) !Tab {
    const pane_values = try requireArray(object, "panes");
    const panes = try allocator.alloc(Pane, pane_values.len);
    for (pane_values, 0..) |*value, index| {
        panes[index] = try parsePane(
            allocator,
            try requireObject(value),
        );
        for (panes[0..index]) |prior| {
            if (std.mem.eql(u8, prior.id, panes[index].id)) {
                return error.DuplicatePane;
            }
        }
    }
    const focused_pane_id = try optionalIdentifier(
        object,
        "focused_pane_id",
    );
    const display_mode = try requireEnum(
        DisplayMode,
        object,
        "display_mode",
    );
    switch (display_mode) {
        .monitor => if (focused_pane_id != null) {
            return error.InvalidFocusedPane;
        },
        .focus => if (panes.len == 0) {
            if (focused_pane_id != null) return error.InvalidFocusedPane;
        } else {
            const focused = focused_pane_id orelse
                return error.InvalidFocusedPane;
            var found = false;
            for (panes) |pane| {
                if (std.mem.eql(u8, pane.id, focused)) found = true;
            }
            if (!found) return error.InvalidFocusedPane;
        },
    }
    const layout = try parseLayout(
        allocator,
        try requireObject(try requireValue(object, "layout")),
    );
    if (layout.pane_order.len != panes.len) return error.InvalidLayout;
    for (panes) |pane| {
        var found = false;
        for (layout.pane_order) |pane_id| {
            if (std.mem.eql(u8, pane.id, pane_id)) found = true;
        }
        if (!found) return error.InvalidLayout;
    }
    return .{
        .id = try requireIdentifier(object, "id"),
        .title = try requireDisplayText(object, "title"),
        .display_mode = display_mode,
        .focused_pane_id = focused_pane_id,
        .visual_pane_ids = layout.pane_order,
        .layout = layout,
        .panes = panes,
    };
}

fn parsePane(
    allocator: std.mem.Allocator,
    object: *const std.json.ObjectMap,
) !Pane {
    if (object.get("content")) |value| if (value != .null) {
        // Structural parsing is strict. Neither restored data nor a backend
        // frame can bind a semantic pane to a terminal at the same time.
        if (object.get("execution_target_id")) |v| if (v != .null and !(v == .string and v.string.len == 0)) return error.InvalidPane;
        if (object.get("tmux_session_id")) |v| if (v != .null and !(v == .string and v.string.len == 0)) return error.InvalidPane;
        const content = try std.json.parseFromValueLeaky(Content, allocator, value, .{ .allocate = .alloc_always, .ignore_unknown_fields = false });
        try content.validate();
        return .{ .id = try requireIdentifier(object, "id"), .execution_target_id = "", .tmux_session_id = "",
            .transport_status = .offline, .attach_fingerprint = try content.fingerprint(allocator),
            .attach_plan = .{ .program = "", .args = &.{} }, .content = content };
    };
    const session_id = try requireIdentifier(object, "tmux_session_id");
    const attach_object = try requireObject(
        try requireValue(object, "attach_plan"),
    );
    const attach_plan: AttachPlan = .{
        .program = try requireString(attach_object, "program"),
        .args = try parseStringArray(allocator, attach_object, "args"),
    };
    try attach_argv.validate(.{
        .program = attach_plan.program,
        .args = attach_plan.args,
    }, session_id);
    return .{
        .id = try requireIdentifier(object, "id"),
        .execution_target_id = try requireIdentifier(
            object,
            "execution_target_id",
        ),
        .tmux_session_id = session_id,
        .transport_status = try requireEnum(
            TransportStatus,
            object,
            "transport_status",
        ),
        .attach_fingerprint = try requireDisplayText(
            object,
            "attach_fingerprint",
        ),
        .attach_plan = attach_plan,
    };
}

fn parseLayout(
    allocator: std.mem.Allocator,
    object: *const std.json.ObjectMap,
) !Layout {
    const pane_order = try parseIdentifierArray(
        allocator,
        object,
        "pane_order",
    );
    const slot_values = try requireArray(object, "slots");
    if (pane_order.len != slot_values.len) return error.InvalidLayout;
    const slots = try allocator.alloc(LayoutSlot, slot_values.len);
    for (slot_values, 0..) |*value, index| {
        const slot = try requireObject(value);
        slots[index] = .{
            .pane_id = try requireIdentifier(slot, "pane_id"),
            .canonical_index = try requireUsize(slot, "canonical_index"),
            .visual_index = try requireUsize(slot, "visual_index"),
            .column = try requireUsize(slot, "column"),
            .row = try requireUsize(slot, "row"),
            .width = try parseRatio(
                try requireObject(try requireValue(slot, "width")),
            ),
            .height = try parseRatio(
                try requireObject(try requireValue(slot, "height")),
            ),
            .role = try parseRole(
                try requireObject(try requireValue(slot, "role")),
            ),
        };
        if (slots[index].canonical_index >= pane_order.len or
            !std.mem.eql(
                u8,
                slots[index].pane_id,
                pane_order[slots[index].canonical_index],
            ))
        {
            return error.InvalidLayout;
        }
        for (slots[0..index]) |prior| {
            if (std.mem.eql(u8, prior.pane_id, slots[index].pane_id)) {
                return error.InvalidLayout;
            }
        }
    }
    return .{
        .pane_order = pane_order,
        .slots = slots,
        .fingerprint = try requireDisplayText(object, "fingerprint"),
    };
}

fn parseRatio(object: *const std.json.ObjectMap) !Ratio {
    const numerator = try requireU32(object, "numerator");
    const denominator = try requireU32(object, "denominator");
    if (numerator == 0 or denominator == 0 or numerator > denominator) {
        return error.InvalidLayout;
    }
    return .{ .numerator = numerator, .denominator = denominator };
}

fn parseRole(object: *const std.json.ObjectMap) !LayoutRole {
    const kind = try requireString(object, "type");
    if (std.mem.eql(u8, kind, "monitor")) return .monitor;
    if (std.mem.eql(u8, kind, "main")) return .main;
    if (std.mem.eql(u8, kind, "sidebar")) {
        const slot = try requireU8(object, "slot");
        if (slot == 0 or slot > 8) return error.InvalidLayout;
        return .{ .sidebar = .{ .slot = slot } };
    }
    return error.InvalidLayout;
}

fn validateOptionalError(value: *const std.json.Value) !void {
    switch (value.*) {
        .null => {},
        .object => |*object| {
            _ = try requireIdentifier(object, "code");
            _ = try requireDisplayText(object, "message");
            _ = try requireBool(object, "retryable");
        },
        else => return error.InvalidFrame,
    }
}

fn validateReceiptFields(
    identity: protocol.FrontendIdentity,
    transaction_id: []const u8,
    workspace_id: []const u8,
    workspace_revision: u64,
    snapshot_fingerprint: []const u8,
) !void {
    try identity.validate();
    try validateIdentifier(transaction_id);
    try validateIdentifier(workspace_id);
    if (workspace_revision == 0) return error.InvalidRevision;
    try validateDisplayText(snapshot_fingerprint);
}

fn requireFrontend(
    object: *const std.json.ObjectMap,
    identity: protocol.FrontendIdentity,
) !void {
    const instance = try requireString(object, "frontend_instance_id");
    const generation = try requireU64(object, "frontend_generation");
    if (!std.mem.eql(u8, instance, identity.frontend_instance_id) or
        generation != identity.frontend_generation)
    {
        return error.StaleFrontend;
    }
}

fn requireObject(value: *const std.json.Value) !*const std.json.ObjectMap {
    return switch (value.*) {
        .object => |*object| object,
        else => error.InvalidFrame,
    };
}

fn requireValue(
    object: *const std.json.ObjectMap,
    key: []const u8,
) !*const std.json.Value {
    return object.getPtr(key) orelse error.MissingField;
}

fn requireString(
    object: *const std.json.ObjectMap,
    key: []const u8,
) ![]const u8 {
    return switch ((try requireValue(object, key)).*) {
        .string => |string| string,
        else => error.InvalidFrame,
    };
}

fn requireIdentifier(
    object: *const std.json.ObjectMap,
    key: []const u8,
) ![]const u8 {
    const value = try requireString(object, key);
    try validateIdentifier(value);
    return value;
}

fn optionalIdentifier(
    object: *const std.json.ObjectMap,
    key: []const u8,
) !?[]const u8 {
    return switch ((try requireValue(object, key)).*) {
        .null => null,
        .string => |string| value: {
            try validateIdentifier(string);
            break :value string;
        },
        else => error.InvalidFrame,
    };
}

fn requireDisplayText(
    object: *const std.json.ObjectMap,
    key: []const u8,
) ![]const u8 {
    const value = try requireString(object, key);
    try validateDisplayText(value);
    return value;
}

fn requireArray(
    object: *const std.json.ObjectMap,
    key: []const u8,
) ![]std.json.Value {
    return switch ((try requireValue(object, key)).*) {
        .array => |array| array.items,
        else => error.InvalidFrame,
    };
}

fn parseIdentifierArray(
    allocator: std.mem.Allocator,
    object: *const std.json.ObjectMap,
    key: []const u8,
) ![]const []const u8 {
    const result = try parseStringArray(allocator, object, key);
    for (result, 0..) |value, index| {
        try validateIdentifier(value);
        for (result[0..index]) |prior| {
            if (std.mem.eql(u8, prior, value)) {
                return error.DuplicateIdentifier;
            }
        }
    }
    return result;
}

fn parseStringArray(
    allocator: std.mem.Allocator,
    object: *const std.json.ObjectMap,
    key: []const u8,
) ![]const []const u8 {
    const values = try requireArray(object, key);
    const result = try allocator.alloc([]const u8, values.len);
    for (values, 0..) |value, index| {
        result[index] = switch (value) {
            .string => |string| string,
            else => return error.InvalidFrame,
        };
    }
    return result;
}

fn requireEnum(
    comptime T: type,
    object: *const std.json.ObjectMap,
    key: []const u8,
) !T {
    return std.meta.stringToEnum(T, try requireString(object, key)) orelse
        error.InvalidFrame;
}

fn requireBool(
    object: *const std.json.ObjectMap,
    key: []const u8,
) !bool {
    return switch ((try requireValue(object, key)).*) {
        .bool => |value| value,
        else => error.InvalidFrame,
    };
}

fn requireU64(
    object: *const std.json.ObjectMap,
    key: []const u8,
) !u64 {
    return switch ((try requireValue(object, key)).*) {
        .integer => |value| std.math.cast(u64, value) orelse
            error.InvalidNumber,
        else => error.InvalidNumber,
    };
}

fn requireUsize(
    object: *const std.json.ObjectMap,
    key: []const u8,
) !usize {
    return std.math.cast(usize, try requireU64(object, key)) orelse
        error.InvalidNumber;
}

fn requireU32(
    object: *const std.json.ObjectMap,
    key: []const u8,
) !u32 {
    return std.math.cast(u32, try requireU64(object, key)) orelse
        error.InvalidNumber;
}

fn requireU8(
    object: *const std.json.ObjectMap,
    key: []const u8,
) !u8 {
    return std.math.cast(u8, try requireU64(object, key)) orelse
        error.InvalidNumber;
}

fn validateIdentifier(value: []const u8) !void {
    if (value.len == 0 or value.len > 512 or
        !std.mem.eql(u8, value, std.mem.trim(u8, value, " \t\r\n")))
    {
        return error.InvalidIdentifier;
    }
    for (value) |byte| if (std.ascii.isControl(byte)) {
        return error.InvalidIdentifier;
    };
}

fn validateDisplayText(value: []const u8) !void {
    if (value.len == 0 or value.len > 8192 or
        !std.mem.eql(u8, value, std.mem.trim(u8, value, " \t\r\n")))
    {
        return error.InvalidText;
    }
    for (value) |byte| if (std.ascii.isControl(byte)) {
        return error.InvalidText;
    };
}
