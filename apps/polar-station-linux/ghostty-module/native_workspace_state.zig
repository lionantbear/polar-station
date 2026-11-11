//! One native logical Workspace owner and pure command planning. No GTK objects,
//! Host Workspace projections, content transport or physical execution live here.
const std = @import("std");
const renderpkg = @import("workspace_rendering.zig");
const native_workspace_store = @import("native_workspace_store.zig");
const controller_model = @import("control_center/model.zig");
const control_intent = @import("control_center/intent.zig");
const view = @import("native_workspace_view.zig");
pub const SelectionPolicy = enum { force, preserve_native };
pub const State = struct {
    allocator: std.mem.Allocator,
    catalog: ?native_workspace_store.OwnedCatalog = null,
    shared_owner: ?*State = null,
    workspace_id: ?[]u8 = null,
    unbound: bool = false,
    mutation_revision: u64 = 0,
    committing: bool = false,
    observed_pane_id: ?[]u8 = null,
    pub fn init(allocator: std.mem.Allocator) State {
        var arena = std.heap.ArenaAllocator.init(allocator);
        defer arena.deinit();
        const empty = newEmptyNativeWorkspace(arena.allocator(), "Workspace", 14, 1) catch return .{ .allocator = allocator };
        const catalog = native_workspace_store.OwnedCatalog.clone(allocator, .{ .catalog_revision = 1, .selected_workspace_id = empty.workspace_id, .workspaces = &.{empty} }) catch null;
        return .{ .allocator = allocator, .catalog = catalog };
    }
    /// Additional windows borrow the process catalog; the root outlives them.
    pub fn initShared(owner: *State, workspace_id: ?[]const u8) !State {
        const catalog = owner.catalogView() orelse return error.NativeWorkspaceUnavailable;
        var selected = catalog;
        selected.selected_workspace_id = workspace_id orelse catalog.selected_workspace_id;
        _ = selected.selected() orelse return error.WorkspaceNotFound;
        const id = if (workspace_id) |value| try owner.allocator.dupe(u8, value) else null;
        errdefer if (id) |value| owner.allocator.free(value);
        if (owner.workspace_id == null and !owner.unbound) {
            owner.workspace_id = try owner.allocator.dupe(u8, catalog.selected_workspace_id);
        }
        return .{ .allocator = owner.allocator, .shared_owner = owner.shared_owner orelse owner, .workspace_id = id, .unbound = workspace_id == null };
    }
    pub fn deinit(self: *State) void {
        if (self.catalog) |*catalog| catalog.deinit();
        if (self.observed_pane_id) |id| self.allocator.free(id);
        if (self.workspace_id) |id| self.allocator.free(id);
        self.* = undefined;
    }
    pub fn snapshot(self: *const State) ?renderpkg.Snapshot {
        if (self.unbound) return null;
        return if (self.catalogView()) |catalog| catalog.selected() else null;
    }
    pub fn catalogView(self: *const State) ?native_workspace_store.Catalog {
        const owner = self.shared_owner orelse self;
        var catalog = if (owner.catalog) |*catalog| catalog.value else return null;
        if (!self.unbound) if (self.workspace_id) |id| { catalog.selected_workspace_id = id; };
        return catalog;
    }
    /// Keyboard selection is transient native state, distinct from Focus MAIN.
    /// GTK identities are resolved once by the adapter; the view never reads GTK.
    pub fn observePane(self: *State, pane_id: ?[]const u8) !bool {
        const next = pane_id orelse "";
        if (std.mem.eql(u8, self.observed_pane_id orelse "", next)) return false;
        const owned = if (pane_id) |id| try self.allocator.dupe(u8, id) else null;
        if (self.observed_pane_id) |id| self.allocator.free(id);
        self.observed_pane_id = owned;
        return true;
    }
    pub fn selectedPane(self: *const State, tab_id: []const u8) ?[]const u8 {
        const current = self.snapshot() orelse return null;
        const tab = current.tab(tab_id) orelse return null;
        const id = self.observed_pane_id orelse return null;
        return if (tab.pane(id) != null) id else null;
    }
    pub fn begin(self: *State) !void {
        const owner = self.shared_owner orelse self;
        if (owner.committing) return error.NativeTransactionInProgress;
        owner.mutation_revision = try std.math.add(u64, owner.mutation_revision, 1);
        self.mutation_revision = owner.mutation_revision;
        owner.committing = true;
    }
    pub fn end(self: *State) void {
        (self.shared_owner orelse self).committing = false;
    }
    /// All allocation precedes persistence admission. Acceptance itself cannot
    /// fail after the worker has accepted the document.
    pub const Acceptance = struct {
        catalog: native_workspace_store.OwnedCatalog,
        selected_id: []u8,
        allocator: std.mem.Allocator,
        pub fn deinit(self: *Acceptance) void {
            self.catalog.deinit();
            self.allocator.free(self.selected_id);
            self.* = undefined;
        }
    };
    pub fn prepareAcceptance(self: *State, candidate: native_workspace_store.OwnedCatalog) !Acceptance {
        return .{ .catalog = candidate, .selected_id = try self.allocator.dupe(u8, candidate.value.selected_workspace_id), .allocator = self.allocator };
    }
    /// Caller retains the previous catalog until physical execution completes.
    pub fn accept(self: *State, prepared: Acceptance) ?native_workspace_store.OwnedCatalog {
        const owner = self.shared_owner orelse self;
        const previous = owner.catalog;
        owner.catalog = prepared.catalog;
        if (self.workspace_id) |id| self.allocator.free(id);
        self.workspace_id = prepared.selected_id;
        self.unbound = false;
        return previous;
    }
    pub const OpenContentPlan = struct {
        snapshot: renderpkg.Snapshot,
        tab_id: []const u8,
        pane_id: []const u8,
    };

    /// Plan a typed-content addition in the same native owner as layout actions.
    /// This does not allocate terminal sessions, surfaces or a second catalog.
    pub fn planOpenContent(self: *const State, allocator: std.mem.Allocator, content: renderpkg.Content) !OpenContentPlan {
        try content.validate();
        const source = self.snapshot() orelse return error.NativeWorkspaceUnavailable;
        const selected = source.tab(source.selected_tab_id) orelse return error.InvalidSelectedTab;
        if (content.kind == .idea_tree) {
            for (selected.panes) |pane| {
                if (pane.content) |existing| {
                    if (existing.kind == .idea_tree) return .{ .snapshot = source, .tab_id = selected.id, .pane_id = pane.id };
                }
            }
        }
        if (selected.panes.len >= 9) return error.ChatTabFull;
        const tabs = try allocator.dupe(renderpkg.Tab, source.tabs);
        const tab_index: usize = blk: {
            for (tabs, 0..) |tab, i| if (std.mem.eql(u8, tab.id, source.selected_tab_id)) break :blk i;
            return error.InvalidSelectedTab;
        };
        const tab = &tabs[tab_index];
        const pane_id = try newNativeIdentifier(allocator);
        const panes = try allocator.alloc(renderpkg.Pane, tab.panes.len + 1);
        @memcpy(panes[0..tab.panes.len], tab.panes);
        panes[tab.panes.len] = try native_workspace_store.nativePane(allocator, .{ .id = pane_id, .content = content });
        const order = try allocator.alloc([]const u8, panes.len);
        @memcpy(order[0..tab.visual_pane_ids.len], tab.visual_pane_ids);
        order[order.len - 1] = pane_id;
        tab.panes = panes;
        tab.visual_pane_ids = order;
        // Explicit user open chooses the new pane; media/terminal focus changes
        // elsewhere do not select a different Composer target.
        if (tab.display_mode == .focus) tab.focused_pane_id = pane_id;
        if (source.workspace_revision == std.math.maxInt(u64)) return error.InvalidWorkspaceRevision;
        var candidate = source;
        candidate.tabs = tabs;
        candidate.selected_tab_id = tab.id;
        candidate.workspace_revision += 1;
        candidate.snapshot_fingerprint = try std.fmt.allocPrint(allocator, "native:{s}:{d}:lgp:{s}", .{ source.workspace_id, candidate.workspace_revision, pane_id });
        return .{ .snapshot = candidate, .tab_id = tab.id, .pane_id = pane_id };
    }

    /// Replace only a semantic Pane in the selected Tab. Keep the user's slot,
    /// Pane ID and every sibling. A new physical Surface fences old content.
    pub fn planReplaceContent(self: *const State, allocator: std.mem.Allocator, pane_id: []const u8, content: renderpkg.Content) !OpenContentPlan {
        try content.validate();
        const source = self.snapshot() orelse return error.NativeWorkspaceUnavailable;
        const tabs = try allocator.dupe(renderpkg.Tab, source.tabs);
        for (tabs) |*tab| {
            if (!std.mem.eql(u8, tab.id, source.selected_tab_id)) continue;
            if (content.kind == .idea_tree) {
                for (tab.panes) |existing| {
                    if (!std.mem.eql(u8, existing.id, pane_id)) {
                        if (existing.content) |value| if (value.kind == .idea_tree) return error.IdeaExplorerAlreadyInTab;
                    }
                }
            }
            const panes = try allocator.dupe(renderpkg.Pane, tab.panes);
            for (panes) |*pane| {
                if (!std.mem.eql(u8, pane.id, pane_id)) continue;
                if (pane.content == null) return error.NotConversationPane;
                pane.* = try native_workspace_store.nativePane(allocator, .{ .id = pane_id, .content = content });
                tab.panes = panes;
                var candidate = source;
                candidate.tabs = tabs;
                candidate.workspace_revision = std.math.add(u64, source.workspace_revision, 1) catch return error.InvalidWorkspaceRevision;
                candidate.snapshot_fingerprint = try std.fmt.allocPrint(allocator, "native:{s}:{d}:chat:{s}", .{ source.workspace_id, candidate.workspace_revision, pane_id });
                return .{ .snapshot = candidate, .tab_id = tab.id, .pane_id = pane_id };
            }
        }
        return error.NoConversationInCurrentTab;
    }

    pub fn planAction(self: *const State, allocator: std.mem.Allocator, immediate_action: ?control_intent.Action, observed_tab: ?[]const u8) !renderpkg.Snapshot {
        const workspace = self.snapshot() orelse return error.NativeWorkspaceUnavailable;
        return planSnapshotAction(allocator, workspace, immediate_action, observed_tab);
    }

    pub fn planAddSessions(allocator: std.mem.Allocator, source: renderpkg.Snapshot, tab_id: []const u8, sessions: []const controller_model.SessionIdentity) !renderpkg.Snapshot {
        const tab = source.tab(tab_id) orelse return error.UnknownTab;
        var additions: std.ArrayListUnmanaged(controller_model.SessionIdentity) = .empty;
        for (sessions) |session| {
            var exists = false;
            for (tab.panes) |pane| { if (std.mem.eql(u8, pane.execution_target_id, session.execution_target_id) and std.mem.eql(u8, pane.tmux_session_id, session.tmux_session_id)) { exists = true; break; } }
            for (additions.items) |added| { if (std.mem.eql(u8, added.execution_target_id, session.execution_target_id) and std.mem.eql(u8, added.tmux_session_id, session.tmux_session_id)) { exists = true; break; } }
            if (!exists) try additions.append(allocator, session);
        }
        if (tab.panes.len + additions.items.len > 9) return error.TabFull;
        var candidate = source;
        for (additions.items, 0..) |session, index| {
            candidate = try planSnapshotAction(allocator, candidate, .{ .toggle_session_assignment = .{
                .tab_id = tab_id, .execution_target_id = session.execution_target_id, .tmux_session_id = session.tmux_session_id,
                .pane_id = try newNativeIdentifier(allocator), .focus_new = tab.panes.len == 0 and index == 0,
                .promote_new = tab.panes.len == 0 and index == 0,
            } }, source.selected_tab_id);
        }
        return candidate;
    }

    pub fn planSnapshotAction(allocator: std.mem.Allocator, workspace: renderpkg.Snapshot, immediate_action: ?control_intent.Action, observed_tab: ?[]const u8) !renderpkg.Snapshot {
        const source = try view.workspace(allocator, workspace);
        if (source.tabs.len == 0) return error.InvalidWorkspace;
        const selected_native_tab_id = observed_tab orelse source.selected_tab_id;
        if (workspace.tab(selected_native_tab_id) == null) return error.UnknownTab;
        if (immediate_action) |action| switch (action) {
            .delete_tab => |deleted| {
                // A stale menu or repeated click must not decrement the
                // allocation unless the exact native Tab will be omitted.
                if (controlWorkspaceTab(&source, deleted.tab_id) == null) return error.UnknownTab;
                if (source.tabs.len == 1 and deleted.replacement_tab_id == null) {
                    return error.MissingNativeIdentity;
                }
                if (source.tabs.len > 1 and deleted.replacement_tab_id != null) {
                    return error.InvalidNativeIdentity;
                }
            },
            .toggle_session_assignment => |assignment| {
                const target_tab_id = assignment.tab_id orelse
                    return error.MissingNativeIdentity;
                var target_exists = false;
                for (source.tabs) |tab| {
                    if (std.mem.eql(u8, tab.id, target_tab_id)) {
                        target_exists = true;
                        break;
                    }
                }
                if (!target_exists) return error.UnknownTab;
            },
            else => {},
        };

        // Membership is a native layout invariant, including drag/drop paths.
        if (immediate_action) |action| switch (action) {
            .move_pane => |move| {
                if (!std.mem.eql(u8, move.from_tab_id, move.to_tab_id)) {
                    const origin = workspace.tab(move.from_tab_id) orelse return error.UnknownTab;
                    const destination = workspace.tab(move.to_tab_id) orelse return error.UnknownTab;
                    for (origin.panes) |pane| {
                        if (!std.mem.eql(u8, pane.id, move.pane_id)) continue;
                        if (pane.content) |content| {
                            if (content.kind == .idea_tree) {
                                for (destination.panes) |existing| {
                                    if (existing.content) |value| if (value.kind == .idea_tree) return error.IdeaExplorerAlreadyInTab;
                                }
                            }
                        }
                    }
                }
            },
            else => {},
        };

        const tab_delta = tabCountDelta(immediate_action);
        const tabs = try allocator.alloc(
            renderpkg.Tab,
            @intCast(@as(isize, @intCast(source.tabs.len)) + tab_delta),
        );
        var tab_index: usize = 0;
        for (source.tabs) |source_tab| {
            if (omitsTab(source_tab, immediate_action)) continue;
            const pane_delta = paneCountDelta(
                selected_native_tab_id,
                source_tab,
                immediate_action,
            );
            const panes = try allocator.alloc(
                renderpkg.Pane,
                @intCast(@as(isize, @intCast(source_tab.panes.len)) + pane_delta),
            );
            var pane_index: usize = 0;
            for (source_tab.panes) |source_pane| {
                if (omitsPane(
                    selected_native_tab_id,
                    source_tab,
                    source_pane,
                    immediate_action,
                )) {
                    continue;
                }
                const replacement = if (immediate_action) |action| blk: {
                    if (action == .use_existing_session) {
                        const selected = action.use_existing_session;
                        if (std.mem.eql(u8, source_pane.execution_target_id, selected.execution_target_id) and std.mem.eql(u8, source_pane.tmux_session_id, selected.tmux_session_id)) break :blk selected.replacement;
                    }
                    break :blk null;
                } else null;
                panes[pane_index] = if (source_pane.content) |content|
                    try native_workspace_store.nativePane(allocator, .{ .id = source_pane.id, .content = content })
                else
                    try controlPane(
                        allocator,
                        source_pane.id,
                        if (replacement) |next| next.execution_target_id else source_pane.execution_target_id,
                        if (replacement) |next| next.tmux_session_id else source_pane.tmux_session_id,
                        if (replacement != null) .recovering else source_pane.transport_status,
                    );
                pane_index += 1;
            }
            if (assignmentAddition(
                selected_native_tab_id,
                source_tab,
                immediate_action,
            )) |add| {
                panes[pane_index] = try controlPane(
                    allocator,
                    add.pane_id,
                    add.execution_target_id,
                    add.tmux_session_id,
                    .recovering,
                );
                pane_index += 1;
            }
            if (movementAddition(source, source_tab, immediate_action)) |add| {
                const moving = workspace.pane(add.pane_id) orelse return error.UnknownPane;
                panes[pane_index] = if (moving.pane.content) |content|
                    try native_workspace_store.nativePane(allocator, .{ .id = add.pane_id, .content = content })
                else
                    try controlPane(
                        allocator,
                        add.pane_id,
                        add.execution_target_id,
                        add.tmux_session_id,
                        add.transport_status,
                    );
                pane_index += 1;
            }
            std.debug.assert(pane_index == panes.len);
            try applyPaneOrdering(panes, source_tab.id, immediate_action);
            const display_mode = desiredDisplayMode(
                source_tab,
                immediate_action,
            );
            const visual_pane_ids = try desiredVisualPaneIds(
                allocator,
                source_tab,
                panes,
                display_mode,
                immediate_action,
            );
            const reset_ratios = if(immediate_action) |action| switch(action){.toggle_display_mode=>|v|std.mem.eql(u8,v.tab_id,source_tab.id),else=>false} else false;
            tabs[tab_index] = .{
                .split_ratios = if(reset_ratios) &.{} else if(workspace.tab(source_tab.id)) |old| old.split_ratios else &.{},
                .id = source_tab.id,
                .title = desiredTabTitle(source_tab, immediate_action),
                .display_mode = display_mode,
                .focused_pane_id = desiredFocusMain(
                    source_tab,
                    panes,
                    display_mode,
                    immediate_action,
                ),
                .visual_pane_ids = visual_pane_ids,
                .layout = .{
                    .pane_order = &.{},
                    .slots = &.{},
                    .fingerprint = "native-owned",
                },
                .panes = panes,
            };
            tab_index += 1;
        }
        if (immediate_action) |action| switch (action) {
            .add_tab => |value| {
                const tab_id = value.tab_id orelse return error.MissingNativeIdentity;
                tabs[tab_index] = .{
                    .id = tab_id,
                    .title = value.title,
                    .display_mode = .monitor,
                    .focused_pane_id = null,
                    .visual_pane_ids = &.{},
                    .layout = .{
                        .pane_order = &.{},
                        .slots = &.{},
                        .fingerprint = "native-owned",
                    },
                    .panes = &.{},
                };
                tab_index += 1;
            },
            .delete_tab => |value| if (value.replacement_tab_id) |replacement_tab_id| {
                tabs[tab_index] = .{
                    .id = replacement_tab_id,
                    .title = "Main",
                    .display_mode = .monitor,
                    .focused_pane_id = null,
                    .visual_pane_ids = &.{},
                    .layout = .{
                        .pane_order = &.{},
                        .slots = &.{},
                        .fingerprint = "native-owned",
                    },
                    .panes = &.{},
                };
                tab_index += 1;
            },
            else => {},
        };
        std.debug.assert(tab_index == tabs.len);
        try applyTabOrdering(tabs, immediate_action);
        const selected_tab_id = desiredSelectedTab(
            source,
            selected_native_tab_id,
            tabs,
            immediate_action,
        );
        const next_revision = if (immediate_action == null)
            @max(source.desired_revision, 1)
        else if (source.desired_revision == std.math.maxInt(u64))
            return error.InvalidWorkspaceRevision
        else
            source.desired_revision + 1;
        const fingerprint = if (immediate_action == null and
            source.structure_fingerprint.len > 0)
            source.structure_fingerprint
        else
            try std.fmt.allocPrint(
                allocator,
                "native:{s}:{d}:mutation={d}:selected={s}",
                .{
                    source.workspace_id,
                    next_revision,
                    workspace.workspace_revision + 1,
                    selected_tab_id,
                },
            );
        const candidate: renderpkg.Snapshot = .{
            .layout_map_visible = workspace.layout_map_visible,
            .inbox_visible = workspace.inbox_visible,
            .workspace_id = source.workspace_id,
            .workspace_name = desiredWorkspaceName(
                source.workspace_name,
                immediate_action,
            ),
            .workspace_revision = next_revision,
            .font_size_points = try desiredManagedFontSize(
                workspace.font_size_points,
                immediate_action,
            ),
            .selected_tab_id = selected_tab_id,
            .tabs = tabs,
            .snapshot_fingerprint = fingerprint,
        };
        return candidate;
    }

    pub fn prepareWorkspace(
        self: *const State,
        desired: renderpkg.Snapshot,
    ) !native_workspace_store.OwnedCatalog {
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const allocator = arena.allocator();
        const candidate = if (self.catalogView()) |catalog| blk: {
            const workspaces = try allocator.alloc(
                renderpkg.Snapshot,
                catalog.workspaces.len,
            );
            var replaced = false;
            for (catalog.workspaces, 0..) |workspace, index| {
                if (std.mem.eql(
                    u8,
                    workspace.workspace_id,
                    catalog.selected_workspace_id,
                )) {
                    workspaces[index] = desired;
                    replaced = true;
                } else {
                    workspaces[index] = workspace;
                }
            }
            if (!replaced) return error.InvalidSelectedWorkspace;
            break :blk native_workspace_store.Catalog{
                .catalog_revision = try nextCatalogRevision(
                    catalog.catalog_revision,
                ),
                .selected_workspace_id = catalog.selected_workspace_id,
                .open_workspace_ids = catalog.open_workspace_ids,
                .archived_workspace_ids = catalog.archived_workspace_ids,
                .workspaces = workspaces,
            };
        } else blk: {
            const workspaces = try allocator.alloc(renderpkg.Snapshot, 1);
            workspaces[0] = desired;
            break :blk native_workspace_store.Catalog{
                .catalog_revision = 1,
                .selected_workspace_id = desired.workspace_id,
                .workspaces = workspaces,
            };
        };
        return native_workspace_store.OwnedCatalog.clone(self.allocator, candidate);
    }

    pub fn prepareTabTransfer(self: *State, tab_id: []const u8, target_id: ?[]const u8, new_name: []const u8) !native_workspace_store.OwnedCatalog {
        const before = self.snapshot() orelse return error.NativeWorkspaceUnavailable;
        var created: ?native_workspace_store.OwnedCatalog = null;
        defer if (created) |*value| value.deinit();
        if (target_id == null) created = try self.prepareCatalogAction(.{ .create_workspace = .{ .name = new_name } });
        var catalog = if (created) |value| value.value else self.catalogView() orelse return error.NativeWorkspaceUnavailable;
        const destination = target_id orelse catalog.selected_workspace_id;
        if (std.mem.eql(u8, before.workspace_id, destination) or catalog.isArchived(destination)) return error.InvalidWorkspace;
        const moved = before.tab(tab_id) orelse return error.UnknownTab;
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const allocator = arena.allocator();
        const workspaces = try allocator.dupe(renderpkg.Snapshot, catalog.workspaces);
        var found = false;
        for (workspaces) |*workspace| {
            if (std.mem.eql(u8, workspace.workspace_id, before.workspace_id)) {
                workspace.* = try State.planSnapshotAction(allocator, before, .{ .delete_tab = .{
                    .tab_id = tab_id, .confirmation_id = "native-move",
                    .replacement_tab_id = if (before.tabs.len == 1) try newNativeIdentifier(allocator) else null,
                } }, null);
            } else if (std.mem.eql(u8, workspace.workspace_id, destination)) {
                if (workspace.tab(tab_id) != null) return error.InvalidNativeIdentity;
                const tabs = try allocator.alloc(renderpkg.Tab, workspace.tabs.len + 1);
                @memcpy(tabs[0..workspace.tabs.len], workspace.tabs);
                tabs[workspace.tabs.len] = moved.*;
                workspace.tabs = tabs;
                workspace.selected_tab_id = tab_id;
                workspace.workspace_revision = try std.math.add(u64, workspace.workspace_revision, 1);
                found = true;
            }
        }
        if (!found) return error.InvalidWorkspace;
        catalog.workspaces = workspaces;
        // Destination is retained for the caller; the source binding is explicit.
        catalog.selected_workspace_id = destination;
        catalog.catalog_revision = try std.math.add(u64, self.catalogView().?.catalog_revision, 1);
        return native_workspace_store.OwnedCatalog.clone(self.allocator, catalog);
    }

    pub fn prepareCatalogAction(
        self: *const State,
        action: control_intent.Action,
    ) !native_workspace_store.OwnedCatalog {
        const current_catalog = self.catalogView() orelse
            return error.NativeWorkspaceUnavailable;
        const previous = current_catalog.selected() orelse
            return error.InvalidSelectedWorkspace;
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const allocator = arena.allocator();

        var candidate: native_workspace_store.Catalog = switch (action) {
            .create_workspace => |value| blk: {
                var name = std.mem.trim(u8, value.name, " \t\r\n");
                if (std.mem.eql(u8, name, "Workspace")) {
                    var suffix: usize = 2;
                    while (true) {
                        var exists = false;
                        for (current_catalog.workspaces) |workspace| {
                            if (std.mem.eql(u8, workspace.workspace_name, name)) { exists = true; break; }
                        }
                        if (!exists) break;
                        name = try std.fmt.allocPrint(allocator, "Workspace {d}", .{suffix});
                        suffix += 1;
                    }
                }
                if (name.len == 0) return error.InvalidWorkspace;
                const workspaces = try allocator.alloc(
                    renderpkg.Snapshot,
                    current_catalog.workspaces.len + 1,
                );
                @memcpy(
                    workspaces[0..current_catalog.workspaces.len],
                    current_catalog.workspaces,
                );
                workspaces[current_catalog.workspaces.len] =
                    try newEmptyNativeWorkspace(
                        allocator,
                        name,
                        previous.font_size_points,
                        self.mutation_revision,
                    );
                break :blk .{
                    .catalog_revision = try nextCatalogRevision(
                        current_catalog.catalog_revision,
                    ),
                    .selected_workspace_id = workspaces[current_catalog.workspaces.len].workspace_id,
                    .workspaces = workspaces,
                };
            },
            .select_workspace => |value| blk: {
                if (current_catalog.isArchived(value.workspace_id)) return error.WorkspaceArchived;
                for (current_catalog.workspaces) |workspace| {
                    if (std.mem.eql(
                        u8,
                        workspace.workspace_id,
                        value.workspace_id,
                    )) break :blk .{
                        .catalog_revision = try nextCatalogRevision(
                            current_catalog.catalog_revision,
                        ),
                        .selected_workspace_id = workspace.workspace_id,
                        .workspaces = current_catalog.workspaces,
                    };
                }
                return error.WorkspaceNotFound;
            },
            .delete_workspace => return error.WorkspaceDeletionForbidden,
            else => return error.NotCatalogAction,
        };
        const open = try allocator.alloc([]const u8, current_catalog.open_workspace_ids.len + 1);
        var count: usize = 0;
        // A freshly allocated second executor initially borrows the root ID
        // before creating its own Workspace; it does not replace the root view.
        const replacing = !self.unbound;
        for (current_catalog.open_workspace_ids) |id| {
            const next_id = if (replacing and std.mem.eql(u8, id, previous.workspace_id)) candidate.selected_workspace_id else id;
            var exists = false;
            for (candidate.workspaces) |workspace| {
                if (std.mem.eql(u8, next_id, workspace.workspace_id)) { exists = true; break; }
            }
            if (!exists) continue;
            var duplicate = false;
            for (open[0..count]) |saved| if (std.mem.eql(u8, saved, next_id)) { duplicate = true; break; };
            if (!duplicate) { open[count] = next_id; count += 1; }
        }
        var selected_open = false;
        for (open[0..count]) |id| if (std.mem.eql(u8, id, candidate.selected_workspace_id)) { selected_open = true; break; };
        if (!selected_open) { open[count] = candidate.selected_workspace_id; count += 1; }
        candidate.open_workspace_ids = open[0..count];
        candidate.archived_workspace_ids = current_catalog.archived_workspace_ids;
        _ = candidate.selected() orelse return error.InvalidSelectedWorkspace;
        return native_workspace_store.OwnedCatalog.clone(self.allocator, candidate);
    }
};

fn desiredVisualPaneIds(
    allocator: std.mem.Allocator,
    source_tab: controller_model.Tab,
    panes: []renderpkg.Pane,
    display_mode: renderpkg.DisplayMode,
    immediate_action: ?control_intent.Action,
) ![]const []const u8 {
    if (panes.len == 0) return &.{};
    const result = try allocator.alloc([]const u8, panes.len);
    if (display_mode == .monitor) {
        for (panes, 0..) |pane, index| result[index] = pane.id;
        return result;
    }
    var count: usize = 0;
    const projected_visual = source_tab.visual_pane_ids;
    for (projected_visual) |pane_id| {
        for (result[0..count]) |present| {
            if (std.mem.eql(u8, present, pane_id)) break;
        } else {
            for (panes) |pane| {
                if (std.mem.eql(u8, pane.id, pane_id)) {
                    result[count] = pane.id;
                    count += 1;
                    break;
                }
            }
        }
    }
    for (panes) |pane| {
        for (result[0..count]) |present| {
            if (std.mem.eql(u8, present, pane.id)) break;
        } else {
            result[count] = pane.id;
            count += 1;
        }
    }
    const main = desiredFocusMain(
        source_tab,
        panes,
        display_mode,
        immediate_action,
    ) orelse return error.MissingMainPane;
    const main_index = for (result, 0..) |pane_id, index| {
        if (std.mem.eql(u8, pane_id, main)) break index;
    } else return error.MissingMainPane;
    std.mem.swap([]const u8, &result[0], &result[main_index]);
    return result;
}

pub fn isNativeWorkspaceAction(action: control_intent.Action) bool {
    return switch (action) {
        .create_workspace,
        .select_workspace,
        .rename_workspace,
        .delete_workspace,
        .set_managed_terminal_font_size,
        .add_tab,
        .select_tab,
        .rename_tab,
        .delete_tab,
        .reorder_tab,
        .reorder_pane,
        .move_pane,
        .toggle_display_mode,
        .toggle_session_assignment,
        .use_existing_session,
        .select_pane,
        .activate_pane,
        .remove_pane,
        => true,
        else => false,
    };
}

pub fn isNativeCatalogAction(action: control_intent.Action) bool {
    return switch (action) {
        .create_workspace, .select_workspace, .delete_workspace => true,
        else => false,
    };
}

fn nextCatalogRevision(current: u64) !u64 {
    if (current == 0 or current == std.math.maxInt(u64)) {
        return error.InvalidCatalogRevision;
    }
    return current + 1;
}

fn newEmptyNativeWorkspace(
    allocator: std.mem.Allocator,
    name: []const u8,
    font_size_points: u8,
    revision: u64,
) !renderpkg.Snapshot {
    const workspace_id = try newNativeIdentifier(allocator);
    const tab_id = try newNativeIdentifier(allocator);
    const tabs = try allocator.alloc(renderpkg.Tab, 1);
    tabs[0] = .{
        .id = tab_id,
        .title = "Main",
        .display_mode = .monitor,
        .focused_pane_id = null,
        .visual_pane_ids = &.{},
        .layout = .{
            .pane_order = &.{},
            .slots = &.{},
            .fingerprint = "native-owned",
        },
        .panes = &.{},
    };
    return .{
        .workspace_id = workspace_id,
        .workspace_name = try allocator.dupe(u8, name),
        .workspace_revision = if (revision == 0) 1 else revision,
        .font_size_points = font_size_points,
        .selected_tab_id = tab_id,
        .tabs = tabs,
        .snapshot_fingerprint = try std.fmt.allocPrint(
            allocator,
            "native:{s}:{d}",
            .{ workspace_id, if (revision == 0) @as(u64, 1) else revision },
        ),
    };
}

pub fn newNativeIdentifier(allocator: std.mem.Allocator) ![]u8 {
    var bytes: [16]u8 = undefined;
    std.crypto.random.bytes(&bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    return std.fmt.allocPrint(
        allocator,
        "{x:0>2}{x:0>2}{x:0>2}{x:0>2}-" ++
            "{x:0>2}{x:0>2}-{x:0>2}{x:0>2}-" ++
            "{x:0>2}{x:0>2}-{x:0>2}{x:0>2}" ++
            "{x:0>2}{x:0>2}{x:0>2}{x:0>2}",
        .{
            bytes[0],  bytes[1],  bytes[2],  bytes[3],
            bytes[4],  bytes[5],  bytes[6],  bytes[7],
            bytes[8],  bytes[9],  bytes[10], bytes[11],
            bytes[12], bytes[13], bytes[14], bytes[15],
        },
    );
}

fn desiredWorkspaceName(
    current: []const u8,
    action: ?control_intent.Action,
) []const u8 {
    if (action) |value| switch (value) {
        .rename_workspace => |rename| return rename.name,
        else => {},
    };
    return current;
}

fn desiredManagedFontSize(
    current: u8,
    action: ?control_intent.Action,
) !u8 {
    if (action) |value| switch (value) {
        .set_managed_terminal_font_size => |font| {
            if (font.points < 10 or font.points > 32) {
                return error.InvalidWorkspace;
            }
            return font.points;
        },
        else => {},
    };
    return current;
}

pub fn selectionPolicyForAction(action: control_intent.Action) SelectionPolicy {
    return switch (action) {
        .select_pane => .preserve_native,
        .move_pane, .activate_pane => .force,
        .add_tab, .select_tab, .delete_tab => .force,
        else => .preserve_native,
    };
}

fn controlPane(
    allocator: std.mem.Allocator,
    pane_id: []const u8,
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
    status: controller_model.TransportStatus,
) !renderpkg.Pane {
    return .{
        .id = pane_id,
        .execution_target_id = execution_target_id,
        .tmux_session_id = tmux_session_id,
        .transport_status = switch (status) {
            .online => .online,
            .offline => .offline,
            .recovering => .recovering,
        },
        .attach_fingerprint = try std.fmt.allocPrint(
            allocator,
            "content:{s}:{s}",
            .{ execution_target_id, tmux_session_id },
        ),
        .attach_plan = .{
            .program = "polar-tmux-pane-host",
            .args = &.{},
        },
    };
}

fn tabCountDelta(action: ?control_intent.Action) isize {
    const value = action orelse return 0;
    return switch (value) {
        .add_tab => 1,
        .delete_tab => |deleted| if (deleted.replacement_tab_id == null) -1 else 0,
        else => 0,
    };
}

fn omitsTab(
    tab: controller_model.Tab,
    action: ?control_intent.Action,
) bool {
    const value = action orelse return false;
    return switch (value) {
        .delete_tab => |deleted| std.mem.eql(u8, tab.id, deleted.tab_id),
        else => false,
    };
}

fn applyTabOrdering(
    tabs: []renderpkg.Tab,
    action: ?control_intent.Action,
) !void {
    const value = action orelse return;
    const reorder = switch (value) {
        .reorder_tab => |reorder| reorder,
        else => return,
    };
    if (reorder.to_index >= tabs.len) return error.InvalidTabIndex;
    const from_index = for (tabs, 0..) |tab, index| {
        if (std.mem.eql(u8, tab.id, reorder.tab_id)) break index;
    } else return error.UnknownTab;
    moveTab(tabs, from_index, reorder.to_index);
}

fn moveTab(tabs: []renderpkg.Tab, from_index: usize, to_index: usize) void {
    if (from_index == to_index) return;
    const moved = tabs[from_index];
    if (from_index < to_index) {
        var index = from_index;
        while (index < to_index) : (index += 1) tabs[index] = tabs[index + 1];
    } else {
        var index = from_index;
        while (index > to_index) : (index -= 1) tabs[index] = tabs[index - 1];
    }
    tabs[to_index] = moved;
}

const PaneOrder = struct {
    pane_id: []const u8,
    to_index: usize,
};

fn applyPaneOrdering(
    panes: []renderpkg.Pane,
    tab_id: []const u8,
    action: ?control_intent.Action,
) !void {
    const value = action orelse return;
    const order = switch (value) {
        .reorder_pane => |reorder| blk: {
            if (!std.mem.eql(u8, tab_id, reorder.tab_id)) return;
            break :blk PaneOrder{ .pane_id = reorder.pane_id, .to_index = reorder.to_index };
        },
        .move_pane => |move| blk: {
            if (!std.mem.eql(u8, tab_id, move.to_tab_id)) return;
            break :blk PaneOrder{ .pane_id = move.pane_id, .to_index = move.to_index };
        },
        else => return,
    };
    if (order.to_index >= panes.len) return error.InvalidPaneIndex;
    const from_index = for (panes, 0..) |pane, index| {
        if (std.mem.eql(u8, pane.id, order.pane_id)) break index;
    } else return error.UnknownPane;
    if (from_index == order.to_index) return;
    const moved = panes[from_index];
    if (from_index < order.to_index) {
        var index = from_index;
        while (index < order.to_index) : (index += 1) panes[index] = panes[index + 1];
    } else {
        var index = from_index;
        while (index > order.to_index) : (index -= 1) panes[index] = panes[index - 1];
    }
    panes[order.to_index] = moved;
}

fn paneCountDelta(
    _: []const u8,
    tab: controller_model.Tab,
    action: ?control_intent.Action,
) isize {
    const value = action orelse return 0;
    switch (value) {
        .remove_pane => |remove| return if (controlTabHasPane(tab, remove.pane_id)) -1 else 0,
        .toggle_session_assignment => |assignment| {
            const target_tab_id = assignment.tab_id orelse return 0;
            if (!std.mem.eql(u8, tab.id, target_tab_id)) return 0;
            if (controlTabHasSession(tab, assignment.execution_target_id, assignment.tmux_session_id)) return -1;
            return 1;
        },
        .move_pane => |move| {
            if (std.mem.eql(u8,tab.id,move.from_tab_id)) return -1;
            if (std.mem.eql(u8,tab.id,move.to_tab_id)) return 1;
            return 0;
        },
        else => return 0,
    }
}

fn omitsPane(
    _: []const u8,
    tab: controller_model.Tab,
    pane: controller_model.Pane,
    action: ?control_intent.Action,
) bool {
    const value = action orelse return false;
    return switch (value) {
        .remove_pane => |remove| std.mem.eql(u8, pane.id, remove.pane_id),
        .toggle_session_assignment => |assignment| blk: {
            const target_tab_id = assignment.tab_id orelse break :blk false;
            break :blk std.mem.eql(u8, tab.id, target_tab_id) and
                controlTabHasSession(
                    tab,
                    assignment.execution_target_id,
                    assignment.tmux_session_id,
                ) and std.mem.eql(
                u8,
                pane.execution_target_id,
                assignment.execution_target_id,
            ) and std.mem.eql(
                u8,
                pane.tmux_session_id,
                assignment.tmux_session_id,
            );
        },
        .move_pane => |move| std.mem.eql(u8, tab.id, move.from_tab_id) and
            std.mem.eql(u8, pane.id, move.pane_id),
        else => false,
    };
}

const AssignmentAddition = struct {
    pane_id: []const u8,
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
};

fn assignmentAddition(
    _: []const u8,
    tab: controller_model.Tab,
    action: ?control_intent.Action,
) ?AssignmentAddition {
    const value = action orelse return null;
    return switch (value) {
        .toggle_session_assignment => |assignment| blk: {
            const target_tab_id = assignment.tab_id orelse break :blk null;
            if (!std.mem.eql(u8, tab.id, target_tab_id) or
                controlTabHasSession(
                    tab,
                    assignment.execution_target_id,
                    assignment.tmux_session_id,
                )) break :blk null;
            break :blk .{
                .pane_id = assignment.pane_id orelse break :blk null,
                .execution_target_id = assignment.execution_target_id,
                .tmux_session_id = assignment.tmux_session_id,
            };
        },
        else => null,
    };
}

const MovementAddition = struct {
    pane_id: []const u8,
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
    transport_status: controller_model.TransportStatus,
};

fn movementAddition(
    workspace: controller_model.WorkspaceMap,
    tab: controller_model.Tab,
    action: ?control_intent.Action,
) ?MovementAddition {
    const value = action orelse return null;
    const move = switch (value) {
        .move_pane => |move| move,
        else => return null,
    };
    if (!std.mem.eql(u8, tab.id, move.to_tab_id)) return null;
    for (workspace.tabs) |source| {
        if (!std.mem.eql(u8, source.id, move.from_tab_id)) continue;
        for (source.panes) |pane| {
            if (!std.mem.eql(u8, pane.id, move.pane_id)) continue;
            return .{
                .pane_id = pane.id,
                .execution_target_id = pane.execution_target_id,
                .tmux_session_id = pane.tmux_session_id,
                .transport_status = pane.transport_status,
            };
        }
    }
    return null;
}

fn desiredDisplayMode(
    tab: controller_model.Tab,
    action: ?control_intent.Action,
) renderpkg.DisplayMode {
    const current: renderpkg.DisplayMode = switch (tab.display_mode) {
        .monitor => .monitor,
        .focus => .focus,
    };
    const value = action orelse return current;
    return switch (value) {
        .activate_pane => |activate| if (controlTabHasPane(
            tab,
            activate.pane_id,
        )) .focus else current,
        .toggle_display_mode => |toggle| if (std.mem.eql(
            u8,
            toggle.tab_id,
            tab.id,
        )) switch (current) {
            .monitor => .focus,
            .focus => .monitor,
        } else current,
        else => current,
    };
}

fn desiredFocusMain(
    source: controller_model.Tab,
    panes: []const renderpkg.Pane,
    mode: renderpkg.DisplayMode,
    action: ?control_intent.Action,
) ?[]const u8 {
    if (mode == .monitor or panes.len == 0) return null;
    if (action) |value| switch (value) {
        .toggle_session_assignment => |assignment| if (assignment.promote_new and std.mem.eql(u8, assignment.tab_id orelse "", source.id)) return assignment.pane_id,
        .activate_pane => |activate| if (controlTabHasPane(
            source,
            activate.pane_id,
        )) return activate.pane_id,
        .move_pane => |move| if (std.mem.eql(
            u8,
            source.id,
            move.to_tab_id,
        )) return move.pane_id,
        .toggle_display_mode => |toggle| if (std.mem.eql(
            u8,
            toggle.tab_id,
            source.id,
        )) {
            for (source.panes) |pane| {
                if (pane.selected and renderedHasPane(panes, pane.id)) {
                    return pane.id;
                }
            }
            return panes[0].id;
        },
        else => {},
    };
    if (source.focused_pane_id) |focused| {
        if (renderedHasPane(panes, focused)) return focused;
    }
    return panes[0].id;
}

fn desiredTabTitle(
    tab: controller_model.Tab,
    action: ?control_intent.Action,
) []const u8 {
    const value = action orelse return tab.title;
    return switch (value) {
        .rename_tab => |rename| if (std.mem.eql(
            u8,
            rename.tab_id,
            tab.id,
        )) rename.title else tab.title,
        else => tab.title,
    };
}

fn desiredSelectedTab(
    workspace: controller_model.WorkspaceMap,
    current: []const u8,
    tabs: []const renderpkg.Tab,
    action: ?control_intent.Action,
) []const u8 {
    const value = action orelse return current;
    return switch (value) {
        .add_tab => |add| add.tab_id orelse current,
        .select_tab => |select| select.tab_id,
        .move_pane => |move| move.to_tab_id,
        .activate_pane => |activate| blk: {
            const owner = controlWorkspaceTabForPane(
                &workspace,
                activate.pane_id,
            ) orelse break :blk current;
            break :blk owner.id;
        },
        .delete_tab => |deleted| blk: {
            if (!std.mem.eql(u8, deleted.tab_id, current)) break :blk current;
            if (deleted.replacement_tab_id) |replacement| break :blk replacement;
            const source_index = for (workspace.tabs, 0..) |tab, index| {
                if (std.mem.eql(u8, tab.id, deleted.tab_id)) break index;
            } else break :blk current;
            break :blk tabs[@min(source_index, tabs.len - 1)].id;
        },
        else => current,
    };
}

fn controlTabHasPane(tab: controller_model.Tab, pane_id: []const u8) bool {
    for (tab.panes) |pane| {
        if (std.mem.eql(u8, pane.id, pane_id)) return true;
    }
    return false;
}

fn controlWorkspaceTab(
    workspace: *const controller_model.WorkspaceMap,
    tab_id: []const u8,
) ?*const controller_model.Tab {
    for (workspace.tabs) |*tab| {
        if (std.mem.eql(u8, tab.id, tab_id)) return tab;
    }
    return null;
}

fn controlWorkspaceTabForPane(
    workspace: *const controller_model.WorkspaceMap,
    pane_id: []const u8,
) ?*const controller_model.Tab {
    for (workspace.tabs) |*tab| {
        if (controlTabHasPane(tab.*, pane_id)) return tab;
    }
    return null;
}

fn controlTabHasSession(
    tab: controller_model.Tab,
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
) bool {
    for (tab.panes) |pane| {
        if (std.mem.eql(u8, pane.execution_target_id, execution_target_id) and
            std.mem.eql(u8, pane.tmux_session_id, tmux_session_id)) return true;
    }
    return false;
}

fn renderedHasPane(panes: []const renderpkg.Pane, pane_id: []const u8) bool {
    for (panes) |pane| {
        if (std.mem.eql(u8, pane.id, pane_id)) return true;
    }
    return false;
}


















// Both windows share stored mappings but retain their own selected Workspace.






















