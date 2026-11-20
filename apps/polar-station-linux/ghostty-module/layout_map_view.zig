//! Layout card composition shared by Control Center and Workspace Settings.
const std = @import("std");
const gtk = @import("gtk");
const gobject = @import("gobject");
const control = @import("control_center/model.zig");
const layout_policy = @import("control_center/layout_policy.zig");

pub fn newExpandedCards() *gtk.FlowBox {
    const cards = gtk.FlowBox.new();
    cards.setSelectionMode(.none);
    cards.setHomogeneous(1);
    cards.setColumnSpacing(layout_policy.expanded_layout_gap);
    cards.setRowSpacing(layout_policy.expanded_layout_gap);
    cards.setMinChildrenPerLine(1);
    cards.setMaxChildrenPerLine(1);
    cards.as(gtk.Widget).addCssClass("polar-station-expanded-cards");
    return cards;
}

pub fn applyExpandedCardColumns(cards: *gtk.FlowBox, width: c_int) void {
    var count: usize = 0;
    var child = cards.as(gtk.Widget).getFirstChild();
    while (child) |item| : (child = item.getNextSibling()) count += 1;
    const columns = layout_policy.expandedLayoutColumnCount(width, count);
    // FlowBox normally wraps at the largest card's natural width.
    if (cards.getMinChildrenPerLine() != columns) cards.setMinChildrenPerLine(columns);
    if (cards.getMaxChildrenPerLine() != columns) cards.setMaxChildrenPerLine(columns);
}

pub const Body = struct {
    widget: *gtk.Widget,
    grid: ?*gtk.Grid,
    display_mode: control.DisplayMode,
    row_span: usize,

    pub fn updateGeometry(self: Body, width: c_int) void {
        self.widget.setSizeRequest(-1, layout_policy.scaledTabBodyHeight(self.display_mode, self.row_span, width));
        if (self.grid) |grid| {
            const gap: c_uint = @intCast(layout_policy.scaledPaneGridGap(width));
            grid.setColumnSpacing(gap);
            grid.setRowSpacing(gap);
        }
    }
};

pub const Card = struct {
    root: *gtk.Box,
    select: *gtk.Button,
    actions: *gtk.Box,
    move_earlier: *gtk.Button,
    move_later: *gtk.Button,
    body: Body,
    tiles: [9]PaneButtonRef = undefined,
    count: usize = 0,

    pub fn updateGeometry(self: *const Card, width: c_int) void {
        self.body.updateGeometry(width);
        for (self.tiles[0..self.count]) |pane| pane.updateDensity(width);
    }
};

pub fn makeTabCard(
    allocator: std.mem.Allocator,
    tab: *const control.Tab,
    tab_index: usize,
    tab_count: usize,
    map_width: c_int,
    saved_bindings: ?[]const []const u8,
) !Card {
    const display_mode = tab.display_mode;
    var visual_storage: [9][]const u8 = undefined;
    if (tab.panes.len > visual_storage.len) return error.InvalidPaneCount;
    const visual = if (tab.visual_pane_ids.len == tab.panes.len)
        tab.visual_pane_ids
    else blk: {
        for (tab.panes, 0..) |pane, index| visual_storage[index] = pane.id;
        break :blk visual_storage[0..tab.panes.len];
    };
    var rows: usize = 1;
    var columns: usize = 1;
    var placements: [9]layout_policy.GridPlacement = undefined;
    for (visual, 0..) |id, index| {
        const placement = try layout_policy.gridPlacementForMapWidth(visual, display_mode, id, map_width);
        placements[index] = placement;
        rows = @max(rows, @as(usize, @intCast(placement.row + placement.row_span)));
        columns = @max(columns, @as(usize, @intCast(placement.column + placement.column_span)));
    }
    const card_layout = layout_policy.tabCardLayoutForMapWidth(display_mode, rows, columns, map_width);
    const card = gtk.Box.new(.vertical, layout_policy.tab_header_body_spacing);
    // The list owns outer spacing; card margins must not duplicate it.
    card.as(gtk.Widget).addCssClass("card");
    card.as(gtk.Widget).setHalign(.fill);
    card.as(gtk.Widget).setHexpand(
        @intFromBool(card_layout.card_hexpand),
    );
    card.as(gtk.Widget).setValign(.start);
    card.as(gtk.Widget).setVexpand(
        @intFromBool(card_layout.card_vexpand),
    );
    if (tab.selected) card.as(gtk.Widget).addCssClass("polar-tab-selected");
    const header = gtk.Box.new(.horizontal, 4);
    header.as(gtk.Widget).addCssClass("polar-tab-header");
    const header_actions = gtk.Box.new(.horizontal, 4);
    if (tab.selected) header.as(gtk.Widget).addCssClass("polar-tab-selected-header");
    header.as(gtk.Widget).setSizeRequest(
        -1,
        card_layout.header_height,
    );
    const tab_selection = gtk.ToggleButton.new();
    tab_selection.setActive(@intFromBool(tab.selected));
    const select = tab_selection.as(gtk.Button);
    select.setCanShrink(1);
    select.as(gtk.Widget).setHexpand(1);
    try setButtonLabel(allocator, select, tab.title);
    if (select.getChild()) |child| {
        if (gobject.ext.cast(gtk.Label, child)) |title_label| {
            title_label.setXalign(0);
            title_label.setEllipsize(.end);
            title_label.setWrap(0);
        }
    }
    try setWidgetName(allocator, select.as(gtk.Widget), tab.id);
    select.as(gtk.Widget).addCssClass("flat");
    select.as(gtk.Widget).setTooltipText(if (tab.selected) "Current Tab" else "Select Tab");
    const pane_count_text = try std.fmt.allocPrint(
        allocator,
        "{d}",
        .{
            tab.panes.len,
        },
    );
    defer allocator.free(pane_count_text);
    const pane_count = label("", "caption");
    pane_count.setWrap(0);
    pane_count.setEllipsize(.end);
    pane_count.as(gtk.Widget).addCssClass("dim-label");
    try setLabel(allocator, pane_count, pane_count_text);
    header.append(gtk.Image.newFromIconName(if (display_mode == .monitor) "view-grid-symbolic" else "view-dual-symbolic").as(gtk.Widget));
    header.append(select.as(gtk.Widget));
    header.append(header_actions.as(gtk.Widget));
    header_actions.append(pane_count.as(gtk.Widget));
    const move_earlier = gtk.Button.newFromIconName("go-up-symbolic");
    const move_later = gtk.Button.newFromIconName("go-down-symbolic");
    move_earlier.as(gtk.Widget).setSensitive(@intFromBool(tab_index > 0));
    move_later.as(gtk.Widget).setSensitive(@intFromBool(tab_index + 1 < tab_count));
    move_earlier.as(gtk.Widget).setTooltipText("Move Up");
    move_later.as(gtk.Widget).setTooltipText("Move Down");
    move_earlier.as(gtk.Widget).addCssClass("flat");
    move_later.as(gtk.Widget).addCssClass("flat");
    header_actions.prepend(move_later.as(gtk.Widget));
    header_actions.prepend(move_earlier.as(gtk.Widget));
    card.append(header.as(gtk.Widget));

    var result: Card = .{
        .root = card,
        .select = select,
        .actions = header_actions,
        .move_earlier = move_earlier,
        .move_later = move_later,
        .body = undefined,
    };
    if (tab.panes.len == 0) {
        const empty = gtk.Box.new(.vertical, 5);
        setMargins(empty.as(gtk.Widget), 18);
        empty.as(gtk.Widget).addCssClass("view");
        empty.as(gtk.Widget).setSizeRequest(-1, card_layout.body_height);
        empty.append(label(if (saved_bindings == null) "Add a Session to this Tab" else "Drop a Pane here", "heading").as(gtk.Widget));
        if (saved_bindings == null) {
            empty.append(label("This empty Tab is a quiet Session assignment target.", "dim-label").as(gtk.Widget));
        }
        card.append(empty.as(gtk.Widget));
        result.body = .{ .widget = empty.as(gtk.Widget), .grid = null, .display_mode = display_mode, .row_span = rows };
        return result;
    }

    const grid = gtk.Grid.new();
    const scaled_gap: c_uint = @intCast(
        layout_policy.scaledPaneGridGap(map_width),
    );
    grid.setColumnSpacing(scaled_gap);
    grid.setRowSpacing(scaled_gap);
    grid.setColumnHomogeneous(1);
    grid.setRowHomogeneous(1);
    grid.as(gtk.Widget).setHalign(.fill);
    grid.as(gtk.Widget).setHexpand(
        @intFromBool(card_layout.grid_hexpand),
    );
    grid.as(gtk.Widget).setValign(.start);
    grid.as(gtk.Widget).setVexpand(
        @intFromBool(card_layout.grid_vexpand),
    );
    grid.as(gtk.Widget).setSizeRequest(-1, card_layout.body_height);

    card.append(grid.as(gtk.Widget));
    result.body = .{ .widget = grid.as(gtk.Widget), .grid = grid, .display_mode = display_mode, .row_span = rows };
    for (visual, placements[0..visual.len]) |id, placement| {
        const index = for (tab.panes, 0..) |pane, index| {
            if (std.mem.eql(u8, pane.id, id)) break index;
        } else return error.UnknownPane;
        const pane_ref = try makePaneTile(allocator, tab, &tab.panes[index], card_layout, placement.column_span, @intCast(columns), if (saved_bindings) |bindings| bindings[index] else null);
        pane_ref.updateDensity(map_width);
        grid.attach(pane_ref.tile, placement.column, placement.row, placement.column_span, placement.row_span);
        result.tiles[result.count] = pane_ref;
        result.count += 1;
    }
    return result;
}

fn makePaneTile(
    allocator: std.mem.Allocator,
    tab: *const control.Tab,
    pane: *const control.Pane,
    card_layout: layout_policy.TabCardLayout,
    column_span: c_int,
    total_columns: c_int,
    saved_binding: ?[]const u8,
) !PaneButtonRef {
    const tile = gtk.Box.new(.vertical, 4);
    tile.as(gtk.Widget).setHexpand(
        @intFromBool(card_layout.pane_tile_hexpand),
    );
    tile.as(gtk.Widget).setVexpand(
        @intFromBool(card_layout.pane_tile_vexpand),
    );
    tile.as(gtk.Widget).addCssClass("card");
    if (pane.selected) tile.as(gtk.Widget).addCssClass("accent");
    const button = gtk.Button.new();
    button.setCanShrink(1);
    button.as(gtk.Widget).addCssClass("polar-station-pane-action");
    try setWidgetName(allocator, button.as(gtk.Widget), pane.id);
    button.as(gtk.Widget).setHexpand(1);
    button.as(gtk.Widget).setVexpand(1);
    button.as(gtk.Widget).addCssClass("flat");
    const content = gtk.Box.new(.vertical, 2);
    content.as(gtk.Widget).setMarginTop(
        layout_policy.pane_tile_vertical_padding,
    );
    content.as(gtk.Widget).setMarginBottom(
        layout_policy.pane_tile_vertical_padding,
    );
    content.as(gtk.Widget).setMarginStart(
        layout_policy.pane_tile_horizontal_padding,
    );
    content.as(gtk.Widget).setMarginEnd(
        layout_policy.pane_tile_horizontal_padding,
    );
    const primary = gtk.Box.new(.horizontal, 2);
    const status = label("●", "caption");
    status.as(gtk.Widget).addCssClass(
        if (pane.transport_status == .online) "success" else "error",
    );
    status.as(gtk.Widget).setTooltipText(
        if (pane.session_presence == .ended) "Session ended" else if (pane.transport_status == .online) "Connected" else "Disconnected",
    );
    const session = label("", "heading");
    session.setWrap(0);
    session.setWidthChars(1);
    session.setEllipsize(.end);
    session.as(gtk.Widget).setHexpand(1);
    try setLabel(allocator, session, pane.session_name);
    const source = label("", "caption");
    source.setWrap(0);
    source.setWidthChars(1);
    source.as(gtk.Widget).setHexpand(1);
    source.setEllipsize(.end);
    source.as(gtk.Widget).addCssClass("dim-label");
    try setLabel(allocator, source, saved_binding orelse switch (pane.target_kind) {
        .local => "Local",
        .ssh => "SSH",
        .wsl => "WSL",
    });
    primary.append(status.as(gtk.Widget));
    if (saved_binding != null and tab.display_mode == .focus and std.mem.eql(u8, tab.focused_pane_id orelse "", pane.id)) {
        const main = label("MAIN", "caption");
        main.setWidthChars(1);
        main.setWrap(0);
        main.setEllipsize(.end);
        primary.append(main.as(gtk.Widget));
    }
    primary.append(session.as(gtk.Widget));
    if (pane.session_presence == .ended) {
        const ended = label("Ended", "caption");
        ended.as(gtk.Widget).addCssClass("error");
        primary.append(ended.as(gtk.Widget));
    }
    content.append(primary.as(gtk.Widget));
    content.append(source.as(gtk.Widget));
    const attention = paneAttentionStateLabel(
        allocator,
        pane.attention,
    ) catch null;
    if (attention) |value| content.append(value.as(gtk.Widget));
    button.setChild(content.as(gtk.Widget));
    const pane_ref: PaneButtonRef = .{
        .pane_id = pane.id,
        .tile = tile.as(gtk.Widget),
        .button = button,
        .content = content,
        .status = status,
        .attention = attention,
        .attention_visible = pane.attention.state != .idle and pane.attention.state != .unknown,
        .column_span = column_span,
        .total_columns = total_columns,
    };
    const tooltip = try std.fmt.allocPrintSentinel(allocator, "{s}\n{s}\n{s}\n{s}", .{
        pane.session_name,
        saved_binding orelse pane.target_label,
        if (pane.session_presence == .ended) "Session ended" else if (pane.transport_status == .online) "Connected" else "Disconnected",
        if (attention) |value| std.mem.span(value.getLabel()) else "",
    }, 0);
    defer allocator.free(tooltip);
    button.as(gtk.Widget).setTooltipText(tooltip);
    tile.append(button.as(gtk.Widget));
    return pane_ref;
}

pub const PaneButtonRef = struct {
    pane_id: []const u8,
    tile: *gtk.Widget,
    button: *gtk.Button,
    content: *gtk.Box,
    status: *gtk.Label,
    attention: ?*gtk.Label,
    attention_visible: bool,
    column_span: c_int,
    total_columns: c_int,

    pub fn updateDensity(self: PaneButtonRef, map_width: c_int) void {
        const gap = layout_policy.scaledPaneGridGap(map_width);
        const available = layout_policy.layoutMapPaneBodyWidth(map_width) - gap * (self.total_columns - 1);
        const slot_width = @divTrunc(available * self.column_span, self.total_columns);
        const compact = slot_width < 64;
        const padding: c_int = if (compact) 2 else layout_policy.pane_tile_horizontal_padding;
        self.content.as(gtk.Widget).setMarginStart(padding);
        self.content.as(gtk.Widget).setMarginEnd(padding);
        self.content.as(gtk.Widget).setMarginTop(if (compact) 2 else layout_policy.pane_tile_vertical_padding);
        self.content.as(gtk.Widget).setMarginBottom(if (compact) 2 else layout_policy.pane_tile_vertical_padding);
        self.status.as(gtk.Widget).setVisible(@intFromBool(!compact));
        if (self.attention) |attention| attention.as(gtk.Widget).setVisible(@intFromBool(!compact and self.attention_visible));
    }
};

fn paneAttentionStateLabel(
    allocator: std.mem.Allocator,
    attention: control.SessionAttention,
) !*gtk.Label {
    const text: []const u8 = switch (attention.state) {
        .idle, .unknown => "",
        .working => "Working",
        .review_ready => "Review ready",
        .needs_attention => "Needs attention",
        .approval_required => "Approval required",
        .decision_required => "Decision required",
        .permission_required => "Permission required",
        .failed => "Failed",
    };
    const result = try ownedLabel(allocator, text, "caption");
    result.as(gtk.Widget).addCssClass(switch (attention.state) {
        .approval_required,
        .decision_required,
        .permission_required,
        .failed,
        => "error",
        .review_ready, .needs_attention => "warning",
        .working => "accent",
        .idle, .unknown => "dim-label",
    });
    result.setEllipsize(.end);
    result.as(gtk.Widget).setVisible(
        @intFromBool(attention.state != .idle and attention.state != .unknown),
    );
    return result;
}

fn ownedLabel(
    allocator: std.mem.Allocator,
    text_value: []const u8,
    css_class: [*:0]const u8,
) !*gtk.Label {
    const text = try allocator.dupeZ(u8, text_value);
    defer allocator.free(text);
    return label(text, css_class);
}

fn label(text: [*:0]const u8, css_class: [*:0]const u8) *gtk.Label {
    const result = gtk.Label.new(text);
    result.setXalign(0);
    result.setWrap(1);
    result.as(gtk.Widget).addCssClass(css_class);
    return result;
}

fn setMargins(widget: *gtk.Widget, value: c_int) void {
    widget.setMarginTop(value);
    widget.setMarginBottom(value);
    widget.setMarginStart(value);
    widget.setMarginEnd(value);
}

fn setLabel(
    allocator: std.mem.Allocator,
    target: *gtk.Label,
    value: []const u8,
) !void {
    const text = try allocator.dupeZ(u8, value);
    defer allocator.free(text);
    target.setLabel(text);
}

fn setButtonLabel(
    allocator: std.mem.Allocator,
    target: *gtk.Button,
    value: []const u8,
) !void {
    const text = try allocator.dupeZ(u8, value);
    defer allocator.free(text);
    target.setLabel(text);
}

fn setWidgetName(
    allocator: std.mem.Allocator,
    target: *gtk.Widget,
    value: []const u8,
) !void {
    const text = try allocator.dupeZ(u8, value);
    defer allocator.free(text);
    target.setName(text);
}
