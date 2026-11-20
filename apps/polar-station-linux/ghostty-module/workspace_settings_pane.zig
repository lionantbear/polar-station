//! Workspace page in the native Settings Pane. The Rust draft owns edits.
const std = @import("std");
const gtk = @import("gtk");
const gdk = @import("gdk");
const glib = @import("glib");
const gobject = @import("gobject");
const transfer = @import("workspace_transfer_window.zig");
const layout = @import("layout_map_view.zig");
const layout_policy = @import("control_center/layout_policy.zig");
const native_view = @import("native_workspace_view.zig");
const store = @import("native_workspace_store.zig");
const control = @import("control_center/model.zig");
const Action = @import("control_center/intent.zig").Action;
const interaction = @import("pane_interaction.zig");
const Frame = @import("settings_pane.zig").Frame;

pub const Layout = struct {
    name: []const u8,
    tabs: []const Tab,
    const Tab = struct { id: []const u8, title: []const u8, pane_ids: []const []const u8 };
};

const Draft = struct {
    layout: Layout,
    workspace: store.OwnedSnapshot,

    fn read(allocator: std.mem.Allocator, layout_allocator: std.mem.Allocator, document: std.json.Value) !Draft {
        const edited = try std.json.parseFromValueLeaky(Layout, layout_allocator, transfer.field(document, "layout"), .{ .allocate = .alloc_always });
        const workspace = try std.json.Stringify.valueAlloc(allocator, transfer.field(document, "workspace"), .{});
        defer allocator.free(workspace);
        return .{ .layout = edited, .workspace = try store.decodeWorkspace(allocator, workspace) };
    }
};

const CardBinding = struct {
    owner: *Pane,
    tab: Layout.Tab,
    index: usize,
    selected: bool,
    title: *gtk.Entry,
    rename: *gtk.Box,
};
const DragBinding = struct {
    owner: *Pane,
    tab: Layout.Tab,
    tab_index: usize,
    pane_id: ?[]const u8 = null,
    widget: *gtk.Widget,
};

const column_spacing = 12;
const column_minimum = 320;

pub const Pane = struct {
    allocator: std.mem.Allocator,
    model: *const control.Model,
    root: *gtk.ScrolledWindow,
    frame: Frame,
    context: *anyopaque,
    save: *const fn (*anyopaque, *Pane, []const u8, u64, Layout) anyerror!void,
    reload: *const fn (*anyopaque, *Pane, []const u8) anyerror!void,
    base: std.heap.ArenaAllocator,
    preview: std.heap.ArenaAllocator,
    bindings: std.heap.ArenaAllocator,
    workspace_id: ?[]const u8 = null,
    workspace: std.json.Value = .null,
    revision: u64 = 0,
    draft: ?Draft = null,
    pending: bool = false,
    name: ?*gtk.Entry = null,
    grid: ?*gtk.Grid = null,
    probe: ?*gtk.DrawingArea = null,
    probe_handler: c_ulong = 0,
    cards: []layout.Card = &.{},
    width: c_int = 0,
    columns: c_int = 1,
    update_source: ?c_uint = null,
    drag_source: ?*gtk.DragSource = null,
    drag_nonce: u64 = 0,
    drag: ?*const DragBinding = null,
    drop_action: ?Action = null,

    pub fn create(allocator: std.mem.Allocator, model: *const control.Model, context: *anyopaque, save: @FieldType(Pane, "save"), reload: @FieldType(Pane, "reload")) !*Pane {
        const self = try allocator.create(Pane);
        const frame = Frame.create();
        self.* = .{
            .allocator = allocator,
            .model = model,
            .root = frame.root,
            .frame = frame,
            .context = context,
            .save = save,
            .reload = reload,
            .base = .init(allocator),
            .preview = .init(allocator),
            .bindings = .init(allocator),
        };
        const hint = gtk.Label.new("Choose a Workspace in Control Center Settings.");
        hint.setWrap(1);
        frame.page.append(hint.as(gtk.Widget));
        return self;
    }

    pub fn deinit(self: *Pane) void {
        self.clearPage();
        if (self.draft) |*draft| draft.workspace.deinit();
        self.bindings.deinit();
        self.preview.deinit();
        self.base.deinit();
        self.allocator.destroy(self);
    }

    pub fn show(self: *Pane, message: []const u8) void {
        self.frame.show(self.allocator, message);
    }

    pub fn setPending(self: *Pane, pending: bool) void {
        self.pending = pending;
        self.frame.setPending(pending);
    }

    pub fn load(self: *Pane, id: []const u8, document: std.json.Value) !void {
        var next_base = std.heap.ArenaAllocator.init(self.allocator);
        errdefer next_base.deinit();
        const a = next_base.allocator();
        const work = try std.json.Stringify.valueAlloc(a, transfer.field(document, "workspace"), .{});
        const workspace = try std.json.parseFromSliceLeaky(std.json.Value, a, work, .{});
        const owned_id = try a.dupe(u8, id);
        const revision = try std.json.parseFromValueLeaky(u64, a, transfer.field(document, "revision"), .{});

        var next_preview = std.heap.ArenaAllocator.init(self.allocator);
        errdefer next_preview.deinit();
        var draft = try Draft.read(self.allocator, next_preview.allocator(), document);
        errdefer draft.workspace.deinit();
        const name = try self.allocator.dupeSentinel(u8, draft.layout.name, 0);
        defer self.allocator.free(name);

        self.clearPage();
        if (self.draft) |*previous| previous.workspace.deinit();
        self.preview.deinit();
        self.base.deinit();
        self.base = next_base;
        self.preview = next_preview;
        self.workspace_id = owned_id;
        self.workspace = workspace;
        self.revision = revision;
        self.draft = draft;
        self.mount(name);
        self.show("");
        self.setPending(false);
        self.renderCards() catch |err| self.show(@errorName(err));
    }

    fn mount(self: *Pane, name: [:0]const u8) void {
        self.frame.title.setText("Workspace");
        const bar = gtk.Box.new(.horizontal, 6);
        const entry = gtk.Entry.new();
        entry.setPlaceholderText("Workspace name");
        entry.as(gtk.Editable).setText(name);
        entry.as(gtk.Widget).setHexpand(1);
        // The current name should not determine the minimum window width.
        entry.as(gtk.Editable).setWidthChars(1);
        self.name = entry;
        bar.append(entry.as(gtk.Widget));
        const save = gtk.Button.newWithLabel("Save");
        bar.append(save.as(gtk.Widget));
        const reload = gtk.Button.newWithLabel("Reload");
        bar.append(reload.as(gtk.Widget));
        self.frame.page.append(bar.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(save, *Pane, saveClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(reload, *Pane, reloadClicked, self, .{});

        const grid = gtk.Grid.new();
        grid.setColumnHomogeneous(1);
        grid.setColumnSpacing(column_spacing);
        grid.setRowSpacing(column_spacing);
        grid.as(gtk.Widget).setHexpand(1);
        self.grid = grid;

        // GTK4 delivers allocated width through the drawing area's resize
        // signal. The transparent overlay contributes no size or hit region.
        const overlay = gtk.Overlay.new();
        overlay.setChild(grid.as(gtk.Widget));
        const probe = gtk.DrawingArea.new();
        probe.as(gtk.Widget).setCanTarget(0);
        overlay.addOverlay(probe.as(gtk.Widget));
        self.probe = probe;
        self.probe_handler = @intCast(gtk.DrawingArea.signals.resize.connect(probe, *Pane, resized, self, .{}));
        self.frame.page.append(overlay.as(gtk.Widget));
    }

    fn resized(_: *gtk.DrawingArea, width: c_int, _: c_int, self: *Pane) callconv(.c) void {
        if (self.width == width) return;
        self.width = width;
        self.queueUpdate();
    }

    fn queueUpdate(self: *Pane) void {
        if (self.drag != null or self.update_source != null) return;
        self.update_source = glib.idleAdd(updateIdle, self);
    }

    fn updateIdle(raw: ?*anyopaque) callconv(.c) c_int {
        const self: *Pane = @ptrCast(@alignCast(raw orelse return 0));
        self.update_source = null;
        if (self.drag != null) return 0;
        if (self.drop_action) |action| {
            self.drop_action = null;
            self.edit(action) catch |err| self.show(@errorName(err));
        }
        self.reflow();
        return 0;
    }

    fn clearPage(self: *Pane) void {
        if (self.drag_source) |source| source.dragCancel();
        self.drag_source = null;
        self.drag = null;
        self.drop_action = null;
        if (self.update_source) |source| _ = glib.Source.remove(source);
        self.update_source = null;
        if (self.probe) |probe| gobject.signalHandlerDisconnect(probe.as(gobject.Object), self.probe_handler);
        if (self.grid != null) self.clearCards();
        while (self.frame.page.as(gtk.Widget).getFirstChild()) |child| self.frame.page.remove(child);
        self.probe = null;
        self.grid = null;
        self.name = null;
    }

    fn clearCards(self: *Pane) void {
        const grid = self.grid.?;
        while (grid.as(gtk.Widget).getFirstChild()) |child| grid.remove(child);
        self.cards = &.{};
        _ = self.bindings.reset(.retain_capacity);
    }

    fn columnsForWidth(self: *const Pane, count: usize) c_int {
        return @min(@max(1, @as(c_int, @intCast(count))), @max(1, @divTrunc(self.width + column_spacing, column_minimum + column_spacing)));
    }

    fn mapWidth(self: *const Pane, columns: c_int) c_int {
        if (self.width <= 0) return column_minimum;
        return @divTrunc(self.width - (columns - 1) * column_spacing, columns) + 2 * layout_policy.layout_list_inset;
    }

    fn reflow(self: *Pane) void {
        const grid = self.grid orelse return;
        const columns = self.columnsForWidth(self.cards.len);
        for (self.cards, 0..) |*card, index| {
            if (columns != self.columns) {
                const widget = card.root.as(gtk.Widget);
                _ = widget.as(gobject.Object).ref();
                grid.remove(widget);
                attach(grid, widget, index, columns);
                widget.as(gobject.Object).unref();
            }
            card.updateGeometry(self.mapWidth(columns));
        }
        self.columns = columns;
    }

    fn attach(grid: *gtk.Grid, widget: *gtk.Widget, index: usize, columns: c_int) void {
        const i: c_int = @intCast(index);
        grid.attach(widget, @mod(i, columns), @divTrunc(i, columns), 1, 1);
    }

    fn renderCards(self: *Pane) !void {
        const draft = self.draft orelse return;
        self.clearCards();
        const a = self.bindings.allocator();
        const snapshot = draft.workspace.value;
        const map = try native_view.build(a, snapshot, self.model.projection(), self.model.session_cache, null);
        const cards = try a.alloc(layout.Card, map.tabs.len);
        self.columns = self.columnsForWidth(cards.len);
        for (map.tabs, draft.layout.tabs, snapshot.tabs, 0..) |tab, draft_tab, saved_tab, i| {
            var refs: [9][]const u8 = undefined;
            for (saved_tab.panes, 0..) |pane, j| {
                refs[j] = if (pane.content) |content| @tagName(content.kind) else try std.fmt.allocPrint(
                    a,
                    "{s} · {s}",
                    .{ if (pane.source) |source| source.host.target_id else pane.execution_target_id, pane.tmux_session_id },
                );
            }
            const card = try layout.makeTabCard(a, &tab, i, map.tabs.len, self.mapWidth(self.columns), refs[0..tab.panes.len]);
            attach(self.grid.?, card.root.as(gtk.Widget), i, self.columns);
            cards[i] = card;
            self.cards = cards[0 .. i + 1];
            try self.bindCard(a, card, draft_tab, tab.selected, i);
        }
    }

    fn bindCard(self: *Pane, a: std.mem.Allocator, card: layout.Card, tab: Layout.Tab, selected: bool, index: usize) !void {
        const title = gtk.Entry.new();
        title.setPlaceholderText("Tab title");
        title.as(gtk.Widget).setHexpand(1);
        title.as(gtk.Editable).setWidthChars(1);
        const z = try a.dupeSentinel(u8, tab.title, 0);
        title.as(gtk.Editable).setText(z);
        const rename = gtk.Box.new(.horizontal, 4);
        rename.append(title.as(gtk.Widget));
        const apply = gtk.Button.newWithLabel("Rename");
        rename.append(apply.as(gtk.Widget));
        rename.as(gtk.Widget).setVisible(0);
        card.root.insertChildAfter(rename.as(gtk.Widget), card.root.as(gtk.Widget).getFirstChild());

        const binding = try a.create(CardBinding);
        binding.* = .{ .owner = self, .tab = tab, .index = index, .selected = selected, .title = title, .rename = rename };
        card.select.as(gtk.Widget).setTooltipText("Rename or drag Tab");
        _ = gtk.Button.signals.clicked.connect(card.select, *CardBinding, openRename, binding, .{});
        _ = gtk.Entry.signals.activate.connect(title, *CardBinding, submitRename, binding, .{});
        _ = gtk.Button.signals.clicked.connect(apply, *CardBinding, renameClicked, binding, .{});
        _ = gtk.Button.signals.clicked.connect(card.move_earlier, *CardBinding, moveEarlier, binding, .{});
        _ = gtk.Button.signals.clicked.connect(card.move_later, *CardBinding, moveLater, binding, .{});

        try self.bindDrag(a, card.select.as(gtk.Widget), card.root.as(gtk.Widget), tab, index, null);
        for (card.tiles[0..card.count]) |tile| {
            const widget = tile.button.as(gtk.Widget);
            try self.bindDrag(a, widget, widget, tab, index, tile.pane_id);
        }
    }

    fn openRename(button: *gtk.Button, binding: *CardBinding) callconv(.c) void {
        if (gobject.ext.cast(gtk.ToggleButton, button.as(gtk.Widget))) |toggle| {
            toggle.setActive(@intFromBool(binding.selected));
        }
        binding.rename.as(gtk.Widget).setVisible(1);
        _ = binding.title.as(gtk.Widget).grabFocus();
    }

    fn submitRename(_: *gtk.Entry, binding: *CardBinding) callconv(.c) void {
        const title = std.mem.span(binding.title.as(gtk.Editable).getText());
        binding.owner.edit(.{ .rename_tab = .{ .tab_id = binding.tab.id, .title = title } }) catch |err| binding.owner.show(@errorName(err));
    }

    fn renameClicked(_: *gtk.Button, binding: *CardBinding) callconv(.c) void {
        submitRename(binding.title, binding);
    }

    fn moveEarlier(_: *gtk.Button, binding: *CardBinding) callconv(.c) void {
        if (binding.index > 0) moveTab(binding, binding.index - 1);
    }

    fn moveLater(_: *gtk.Button, binding: *CardBinding) callconv(.c) void {
        moveTab(binding, binding.index + 1);
    }

    fn moveTab(binding: *CardBinding, index: usize) void {
        binding.owner.edit(.{ .reorder_tab = .{ .tab_id = binding.tab.id, .to_index = index } }) catch |err| binding.owner.show(@errorName(err));
    }

    fn edit(self: *Pane, action: Action) !void {
        if (self.pending or self.drag != null) return;
        const id = self.workspace_id orelse return;
        var request = std.heap.ArenaAllocator.init(self.allocator);
        defer request.deinit();
        const a = request.allocator();
        const operation = switch (action) {
            inline .rename_tab, .reorder_tab, .reorder_pane, .move_pane => |payload, kind| blk: {
                const bytes = try std.json.Stringify.valueAlloc(a, payload, .{});
                var data = try std.json.parseFromSliceLeaky(std.json.Value, a, bytes, .{});
                try data.object.put(a, "intent", .{ .string = @tagName(kind) });
                break :blk data;
            },
            else => return error.UnsupportedWorkspaceEdit,
        };
        const result = try transfer.local(a, .{
            .action = "edit_workspace_draft",
            .workspace_id = id,
            .revision = self.revision,
            .workspace = self.workspace,
            .layout = self.draft.?.layout,
            .operation = operation,
        });
        defer result.deinit();
        const message = transfer.string(result.value, "error");
        if (message.len != 0) return self.show(message);

        var next = std.heap.ArenaAllocator.init(self.allocator);
        errdefer next.deinit();
        const draft = try Draft.read(self.allocator, next.allocator(), result.value);
        self.clearCards();
        self.draft.?.workspace.deinit();
        self.preview.deinit();
        self.preview = next;
        self.draft = draft;
        self.show("");
        self.renderCards() catch |err| self.show(@errorName(err));
    }

    fn saveClicked(_: *gtk.Button, self: *Pane) callconv(.c) void {
        if (self.pending or self.drag != null) return;
        const id = self.workspace_id orelse return;
        var draft = self.draft.?.layout;
        draft.name = std.mem.span(self.name.?.as(gtk.Editable).getText());
        self.save(self.context, self, id, self.revision, draft) catch |err| self.show(@errorName(err));
    }

    fn reloadClicked(_: *gtk.Button, self: *Pane) callconv(.c) void {
        if (self.pending or self.drag != null) return;
        self.reload(self.context, self, self.workspace_id orelse return) catch |err| self.show(@errorName(err));
    }

    fn bindDrag(self: *Pane, a: std.mem.Allocator, source: *gtk.Widget, target: *gtk.Widget, tab: Layout.Tab, index: usize, pane_id: ?[]const u8) !void {
        const binding = try a.create(DragBinding);
        binding.* = .{ .owner = self, .tab = tab, .tab_index = index, .pane_id = pane_id, .widget = target };
        const drag = gtk.DragSource.new();
        drag.setActions(gdk.DragAction.flags_move);
        drag.as(gtk.GestureSingle).setButton(1);
        drag.as(gtk.EventController).setPropagationPhase(.capture);
        _ = gtk.DragSource.signals.prepare.connect(drag, *DragBinding, dragPrepare, binding, .{});
        _ = gtk.DragSource.signals.drag_cancel.connect(drag, *DragBinding, dragCancel, binding, .{});
        _ = gtk.DragSource.signals.drag_end.connect(drag, *DragBinding, dragEnd, binding, .{});
        source.addController(drag.as(gtk.EventController));

        const drop = gtk.DropTarget.new(gobject.ext.types.string, gdk.DragAction.flags_move);
        drop.setPreload(1);
        _ = gtk.DropTarget.signals.enter.connect(drop, *DragBinding, dropMotion, binding, .{});
        _ = gtk.DropTarget.signals.motion.connect(drop, *DragBinding, dropMotion, binding, .{});
        _ = gtk.DropTarget.signals.leave.connect(drop, *DragBinding, dropLeave, binding, .{});
        _ = gtk.DropTarget.signals.drop.connect(drop, *DragBinding, dropped, binding, .{});
        target.addController(drop.as(gtk.EventController));
    }

    fn dragPrepare(source: *gtk.DragSource, _: f64, _: f64, binding: *DragBinding) callconv(.c) ?*gdk.ContentProvider {
        const self = binding.owner;
        if (self.pending or self.drag != null or self.drop_action != null) return null;
        const nonce = @import("station_random.zig").int(u64);
        const payload = std.fmt.allocPrintSentinel(self.allocator, "station-workspace-settings:{x}", .{nonce}, 0) catch return null;
        defer self.allocator.free(payload);
        var value = gobject.ext.Value.newFrom(payload);
        defer value.unset();
        self.drag_nonce = nonce;
        self.drag = binding;
        self.drag_source = source;
        return gdk.ContentProvider.newForValue(&value);
    }

    fn dragCancel(_: *gtk.DragSource, _: *gdk.Drag, _: gdk.DragCancelReason, binding: *DragBinding) callconv(.c) c_int {
        binding.owner.drop_action = null;
        finishDrag(binding.owner);
        return 0;
    }

    fn dragEnd(_: *gtk.DragSource, _: *gdk.Drag, _: c_int, binding: *DragBinding) callconv(.c) void {
        finishDrag(binding.owner);
    }

    fn finishDrag(self: *Pane) void {
        self.drag = null;
        self.drag_source = null;
        self.queueUpdate();
    }

    const Placement = struct { action: Action, css: [:0]const u8 };

    fn placement(binding: *const DragBinding, value: ?*const gobject.Value, x: f64, y: f64) ?Placement {
        const source = binding.owner.drag orelse return null;
        const actual = gobject.ext.Value.get(value orelse return null, ?[:0]const u8) orelse return null;
        var buf: [64]u8 = undefined;
        const expected = std.fmt.bufPrint(&buf, "station-workspace-settings:{x}", .{binding.owner.drag_nonce}) catch return null;
        if (!std.mem.eql(u8, actual, expected)) return null;
        const same_tab = std.mem.eql(u8, source.tab.id, binding.tab.id);
        const widget = binding.widget;
        if (source.pane_id) |pane| {
            if (!same_tab and binding.tab.pane_ids.len >= 9) return null;
            var destination = binding.tab.pane_ids.len - @intFromBool(same_tab);
            var css: [:0]const u8 = "pane-drop-append";
            if (binding.pane_id) |anchor| {
                if (std.mem.eql(u8, pane, anchor)) return null;
                const edge = interaction.dropEdge(x, y, @floatFromInt(widget.getWidth()), @floatFromInt(widget.getHeight())) orelse return null;
                destination = 0;
                for (binding.tab.pane_ids) |id| {
                    if (std.mem.eql(u8, id, anchor)) break;
                    if (!std.mem.eql(u8, id, pane)) destination += 1;
                }
                destination += @intFromBool(edge.placement() == .after);
                css = switch (edge) {
                    .left => "pane-drop-left",
                    .right => "pane-drop-right",
                    .up => "pane-drop-up",
                    .down => "pane-drop-down",
                };
            } else {
                var picked = widget.pick(x, y, .{});
                while (picked) |child| : (picked = child.getParent()) {
                    if (child == widget) break;
                    if (child.hasCssClass("polar-station-pane-action") != 0) return null;
                }
            }
            return .{ .css = css, .action = if (same_tab)
                .{ .reorder_pane = .{ .tab_id = binding.tab.id, .pane_id = pane, .to_index = destination } }
            else
                .{ .move_pane = .{ .from_tab_id = source.tab.id, .to_tab_id = binding.tab.id, .pane_id = pane, .to_index = destination } } };
        }
        if (binding.pane_id != null or same_tab) return null;
        const header = widget.getFirstChild() orelse return null;
        if (y > @as(f64, @floatFromInt(header.getHeight()))) return null;
        const horizontal = binding.owner.columns > 1;
        const after = if (horizontal)
            x >= @as(f64, @floatFromInt(widget.getWidth())) / 2
        else
            y >= @as(f64, @floatFromInt(header.getHeight())) / 2;
        const insertion = binding.tab_index + @intFromBool(after);
        return .{
            .css = if (horizontal)
                (if (after) "pane-drop-right" else "pane-drop-left")
            else
                (if (after) "pane-drop-down" else "pane-drop-up"),
            .action = .{ .reorder_tab = .{ .tab_id = source.tab.id, .to_index = insertion - @intFromBool(source.tab_index < insertion) } },
        };
    }

    fn clearDrop(widget: *gtk.Widget) void {
        inline for (.{ "pane-drop-left", "pane-drop-right", "pane-drop-up", "pane-drop-down", "pane-drop-append" }) |css| widget.removeCssClass(css);
    }

    fn dropMotion(target: *gtk.DropTarget, x: f64, y: f64, binding: *DragBinding) callconv(.c) gdk.DragAction {
        clearDrop(binding.widget);
        const proposed = placement(binding, target.getValue(), x, y) orelse return .{};
        binding.widget.addCssClass(proposed.css);
        return gdk.DragAction.flags_move;
    }

    fn dropLeave(_: *gtk.DropTarget, binding: *DragBinding) callconv(.c) void {
        clearDrop(binding.widget);
    }

    fn dropped(_: *gtk.DropTarget, value: *gobject.Value, x: f64, y: f64, binding: *DragBinding) callconv(.c) c_int {
        clearDrop(binding.widget);
        const proposed = placement(binding, value, x, y) orelse return 0;
        // Rebuild only after GTK finishes the source's drag callbacks.
        binding.owner.drop_action = proposed.action;
        return 1;
    }
};
