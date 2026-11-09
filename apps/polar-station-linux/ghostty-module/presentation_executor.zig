//! GTK transaction adapter and physical Ghostty executor. Logical Workspace
//! rules and ownership live in native_workspace_state; read-only joins live in
//! native_workspace_view. This file owns native object identity and minimal diffs.

const std = @import("std");
const adw = @import("adw");
const gio = @import("gio");
const glib = @import("glib");
const gobject = @import("gobject");
const gtk = @import("gtk");

const Application = @import("../class/application.zig").Application;
const Surface = @import("../class/surface.zig").Surface;
const Tab = @import("../class/tab.zig").Tab;
const Window = @import("../class/window.zig").Window;
const configpkg = @import("../../../config.zig");
const controller_model = @import("control_center/model.zig");
const control_intent = @import("control_center/intent.zig");
const protocol = @import("protocol.zig");
const native_layout = @import("native_workspace_layout.zig");
const native_surface_transition = @import("native_surface_transition.zig");
const content_lifecycle = @import("content_lifecycle.zig");
const terminal_recovery = @import("terminal_recovery.zig");
const terminal_focus = @import("terminal_focus.zig");
const pane_content = @import("pane_content.zig");
const workspace_reconciliation = @import("workspace_reconciliation.zig");
const renderpkg = @import("workspace_rendering.zig");
const native_workspace_store = @import("native_workspace_store.zig");
const pane_interaction = @import("pane_interaction.zig");
const session_runtime = @import("session_runtime.zig");
const workspace_state = @import("native_workspace_state.zig");
const workspace_view = @import("native_workspace_view.zig");
const DisplaySurface = @import("display_surface.zig").DisplaySurface;
const ContentHooks = @import("native_content_hooks.zig").Hooks;
var next_display_generation: u64 = 1;
fn newDisplay(allocator: std.mem.Allocator, content: @import("station_content.zig").Content) !*DisplaySurface {
    if (next_display_generation == std.math.maxInt(u64)) return error.DisplayGenerationExhausted;
    const generation = next_display_generation;
    next_display_generation += 1;
    return if (content.kind == .idea_tree) DisplaySurface.createGraphics(allocator, generation, content.view_id) else DisplaySurface.create(allocator, generation);
}

const SelectionPolicy = workspace_state.SelectionPolicy;

pub const NativeSessionAssignment = union(enum) {
    no_managed_tab,
    available: struct { tab_id: []const u8 },
    assigned_here: struct { tab_id: []const u8, pane_id: []const u8 },
    assigned_elsewhere: struct { tab_id: []const u8 },
    tab_full: struct { tab_id: []const u8 },
};

/// Stable generation token for one borrowed GtkWindow. GTK may allocate a new
/// Window at the exact address of a finalized predecessor, so raw pointer
/// equality is not a lifetime identity.
const WindowLifetime = struct {
    alive: bool = true,
};

fn appliedWindowWeakNotify(
    context: ?*anyopaque,
    _: *gobject.Object,
) callconv(.c) void {
    const window_lifetime: *WindowLifetime = @ptrCast(@alignCast(
        context orelse return,
    ));
    window_lifetime.alive = false;
}

pub const NativePaneView = struct {
    pane_id: []const u8,
    tab_id: []const u8,
    execution_target_id: []const u8,
    tmux_session_id: []const u8,
};

pub const CloseFocusedPaneTarget = union(enum) {
    managed: []const u8,
    native_split: *Surface,
};

pub const Executor = struct {
    const PendingLifecycle = struct { candidate: native_workspace_store.OwnedCatalog, owner: ?*Executor, archive: bool, revision: u64, transfer: bool = false, apply_layout: bool = false };

    const PaneReservation = struct { id: []u8, tab_id: []u8, count: usize };
    allocator: std.mem.Allocator,
    app: *Application,
    identity: protocol.FrontendIdentity,
    content_lifecycle: content_lifecycle.Lifecycle,
    content_hooks: ContentHooks = .{},
    managed_window: ?*Window = null,
    current: ?AppliedRenderState = null,
    applying_physical_snapshot: bool = false,
    workspace: workspace_state.State,
    workspace_store: ?native_workspace_store.Store = null,
    persistence_revision: u64 = 0,
    catalog_owner: ?*Executor = null,
    pending_lifecycle: ?PendingLifecycle = null,
    reservations: [9]?PaneReservation = .{null} ** 9,

    pub fn reserveSessionPanes(self: *Executor, id: []const u8, tab_id: []const u8, count: usize) !void {
        const snapshot = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
        const tab = snapshot.tab(tab_id) orelse return error.TabNotFound;
        var reserved: usize = 0;
        for (self.reservations) |item| if (item) |entry| {
            if (std.mem.eql(u8, entry.tab_id, tab_id) and !std.mem.eql(u8, entry.id, id)) reserved += entry.count;
        };
        if (tab.panes.len + reserved + count > 9) return error.TabFull;
        self.releaseSessionPanes(id);
        for (&self.reservations) |*slot| if (slot.* == null) {
            const owned_id = try self.allocator.dupe(u8, id);
            errdefer self.allocator.free(owned_id);
            slot.* = .{ .id = owned_id, .tab_id = try self.allocator.dupe(u8, tab_id), .count = count };
            return;
        };
        return error.TabFull;
    }

    pub fn releaseSessionPanes(self: *Executor, id: []const u8) void {
        for (&self.reservations) |*slot| if (slot.*) |entry| {
            if (!std.mem.eql(u8, entry.id, id)) continue;
            self.allocator.free(entry.id); self.allocator.free(entry.tab_id); slot.* = null;
        };
    }

    fn validateSessionReservations(self: *Executor, candidate: renderpkg.Snapshot) !void {
        for (candidate.tabs) |tab| {
            var count = tab.panes.len;
            for (self.reservations) |item| if (item) |entry| {
                if (std.mem.eql(u8, entry.tab_id, tab.id)) count += entry.count;
            };
            if (count > 9) return error.TabFull;
        }
    }

    pub fn init(
        allocator: std.mem.Allocator,
        app: *Application,
        identity: protocol.FrontendIdentity,
    ) Executor {
        return .{ .allocator = allocator, .app = app, .identity = identity, .content_lifecycle = content_lifecycle.Lifecycle.init(allocator, @intCast(std.os.linux.getpid()), identity.frontend_instance_id), .workspace = workspace_state.State.init(allocator), .workspace_store = native_workspace_store.Store.init(allocator) catch null };
    }

    /// Physical trees and selection are window-local. Storage and content
    /// channel generations remain owned by the original process executor.
    pub fn initWindow(owner: *Executor, workspace_id: ?[]const u8) !Executor {
        const root = owner.catalog_owner orelse owner;
        return .{
            .allocator = root.allocator, .app = root.app, .identity = root.identity,
            .content_lifecycle = content_lifecycle.Lifecycle.init(root.allocator, @intCast(std.os.linux.getpid()), root.identity.frontend_instance_id),
            .content_hooks = root.content_hooks,
            .workspace = try workspace_state.State.initShared(&root.workspace, workspace_id),
            .catalog_owner = root,
        };
    }
    fn lifecycle(self: *Executor) *content_lifecycle.Lifecycle {
        return &(self.catalog_owner orelse self).content_lifecycle;
    }
    pub fn createWindowWorkspace(self: *Executor, name: []const u8) !void {
        var previous = try self.admitNativeCatalog(try self.workspace.prepareCatalogAction(.{ .create_workspace = .{ .name = name } }));
        defer if (previous) |*old| old.deinit();
    }

    pub fn rememberOpenWindows(self: *Executor, ids: []const []const u8) !void {
        if ((self.catalog_owner orelse self).pending_lifecycle != null) return;
        var catalog = self.nativeWorkspaceCatalog() orelse return;
        var equal = catalog.open_workspace_ids.len == ids.len;
        if (equal) for (ids, catalog.open_workspace_ids) |a, b| {
            if (!std.mem.eql(u8, a, b)) { equal = false; break; }
        };
        if (equal) return;
        catalog.open_workspace_ids = ids;
        catalog.catalog_revision = try std.math.add(u64, catalog.catalog_revision, 1);
        if (self.managed_window == null and ids.len > 0) catalog.selected_workspace_id = ids[0];
        const root = self.catalog_owner orelse self;
        var owned = try native_workspace_store.OwnedCatalog.clone(self.allocator, catalog);
        errdefer owned.deinit();
        try self.enqueueNativeState(owned.value);
        var previous = root.workspace.catalog;
        root.workspace.catalog = owned;
        if (previous) |*old| old.deinit();
    }

    pub fn lifecycleDecision(self: *const Executor, action: []const u8, target: ?[]const u8) !std.json.Parsed(@import("workspace_lifecycle.zig").Decision) {
        const catalog = self.nativeWorkspaceCatalog() orelse return error.NativeWorkspaceUnavailable;
        const ids = try self.allocator.alloc([]const u8, catalog.workspaces.len);
        defer self.allocator.free(ids);
        for (catalog.workspaces, 0..) |workspace, i| ids[i] = workspace.workspace_id;
        const bound = if (self.nativeWorkspaceSnapshot()) |snapshot| snapshot.workspace_id else null;
        return @import("workspace_lifecycle.zig").decide(self.allocator, .{ .action = action, .ids = ids,
            .archived = catalog.archived_workspace_ids, .open = catalog.open_workspace_ids,
            .selected = catalog.selected_workspace_id, .bound = bound, .target = target });
    }

    pub fn archiveOrRestore(self: *Executor, archive: bool, target: []const u8) !void {
        const root = self.catalog_owner orelse self;
        if (root.pending_lifecycle != null) return error.NativeTransactionInProgress;
        var decision = try self.lifecycleDecision(if (archive) "archive" else "restore", target);
        defer decision.deinit();
        if (decision.value.@"error" != null) return error.WorkspaceActionRejected;
        var catalog = self.nativeWorkspaceCatalog() orelse return error.NativeWorkspaceUnavailable;
        // Freeze the root window binding before changing the process selection.
        if (!root.workspace.unbound and root.workspace.workspace_id == null) {
            root.workspace.workspace_id = try root.allocator.dupe(u8, root.workspace.snapshot().?.workspace_id);
        }
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const archived = try arena.allocator().alloc([]const u8, catalog.archived_workspace_ids.len + 1);
        var count: usize = 0;
        for (catalog.archived_workspace_ids) |id| if (!std.mem.eql(u8, id, target)) { archived[count] = id; count += 1; };
        if (archive) { archived[count] = target; count += 1; }
        catalog.archived_workspace_ids = archived[0..count];
        const open = try arena.allocator().alloc([]const u8, catalog.open_workspace_ids.len);
        count = 0;
        for (catalog.open_workspace_ids) |id| if (!archive or !std.mem.eql(u8, id, target)) { open[count] = id; count += 1; };
        catalog.open_workspace_ids = open[0..count];
        catalog.selected_workspace_id = decision.value.selected;
        catalog.catalog_revision = try std.math.add(u64, catalog.catalog_revision, 1);
        var candidate = try native_workspace_store.OwnedCatalog.clone(self.allocator, catalog);
        errdefer candidate.deinit();
        try self.workspace.begin();
        errdefer self.workspace.end();
        try root.enqueueNativeState(candidate.value);
        root.pending_lifecycle = .{ .candidate = candidate, .owner = self, .archive = archive, .revision = root.persistence_revision };
    }

    pub fn unbindWorkspace(self: *Executor) !void {
        if (self.nativeWorkspaceSnapshot()) |previous| {
            var empty = previous;
            var tab = (previous.tab(previous.selected_tab_id) orelse return error.InvalidSelectedTab).*;
            tab.panes = &.{}; tab.visual_pane_ids = &.{}; tab.focused_pane_id = null;
            tab.display_mode = .monitor; tab.layout.pane_order = &.{}; tab.layout.slots = &.{};
            empty.tabs = &.{tab};
            // Presentation retirement only; never a new catalog definition.
            try self.applySnapshot(empty, .force);
        }
        if (self.current) |*current| {
            current.retireAllPaneContent(self.lifecycle());
            current.deinit();
            self.current = null;
        }
        self.workspace.unbound = true;
    }

    pub fn traceSurfaceStage(self: *Executor, surface: *Surface, stage: u32) void {
        if (self.current) |*current| {
            for (current.panes.items) |pane| {
                if (pane.active and pane.isTerminal() and pane.surface == surface) {
                    @import("ipc_client.zig").trace(stage, pane.content_state.terminal.content_channel_id, pane.content_state.terminal.content_channel_generation);
                    return;
                }
            }
        }
        // Initial placeholder exists before the managed Workspace is bound.
        @import("ipc_client.zig").trace(stage, "", 0);
    }

    pub fn traceSurface(self: *Executor, surface: *Surface, key: u64) void {
        if (key == 0) return;
        const current = if (self.current) |*value| value else return;
        for (current.panes.items) |*pane| {
            if (pane.active and pane.isTerminal() and pane.surface == surface and !pane.trace_identity_emitted) {
                @import("ipc_client.zig").trace(10, pane.content_state.terminal.content_channel_id, key);
                pane.trace_identity_emitted = true;
                @import("frame_trace.zig").observe(surface.as(gtk.Widget), key);
                return;
            }
        }
    }

    pub fn deinit(self: *Executor) void {
        const root = self.catalog_owner orelse self;
        if (root.pending_lifecycle) |*pending| { if (pending.owner == self) pending.owner = null; }
        if (self.pending_lifecycle) |*pending| pending.candidate.deinit();
        for (self.reservations) |item| if (item) |entry| {
            self.allocator.free(entry.id); self.allocator.free(entry.tab_id);
        };
        if (self.current) |*current| {
            current.retireAllPaneContent(self.lifecycle());
            current.destroyWindow();
            current.deinit();
        }
        self.retryContentRetirements();
        self.content_lifecycle.deinit();
        self.workspace.deinit();
        if (self.workspace_store) |*store| store.deinit();
        self.* = undefined;
    }

    pub fn windowRemoved(self: *Executor, window: *Window) void {
        const root = self.catalog_owner orelse self;
        if (root.pending_lifecycle) |*pending| { if (pending.owner == self) pending.owner = null; }
        self.workspace.unbound = true;
        if (self.current) |*current| {
            if (current.window != window) return;
            current.retireAllPaneContent(self.lifecycle());
            current.deinit();
            self.current = null;
        }
        if (self.managed_window == window) self.managed_window = null;
    }

    /// Binds rendering to the exact managed Ghostty window that hosts the
    /// Polar Station Control Center. This pointer is borrowed; failed initial
    /// rendering and executor teardown must never destroy the window.
    pub fn setManagedWindow(self: *Executor, window: *Window) void {
        _ = self.setManagedWindowIfReady(window);
    }

    /// Binds only a ready terminal-first Window. While the current managed
    /// Window is alive, ordinary focus changes cannot move the Workspace to a
    /// different Window. After a warm Window replacement, dead native state is
    /// released and the retained Host projection is replayed once.
    pub fn setManagedWindowIfReady(self: *Executor, window: *Window) bool {
        if (window.getTabView().getNPages() == 0 or
            window.getActiveSurface() == null)
        {
            return false;
        }
        if (self.current) |*current| {
            if (current.windowExists()) {
                if (current.window == window) return false;
                // A different active Window is only a focus change while the
                // managed Workspace Window is still alive.
                return false;
            }
            current.retireAllPaneContent(self.lifecycle());
            current.deinit();
            self.current = null;
            self.managed_window = null;
        }
        if (self.managed_window) |managed| {
            if (managed == window) return true;
        }
        self.managed_window = window;
        if (self.nativeWorkspaceSnapshot()) |workspace| {
            self.applySnapshot(workspace, .force) catch |restore_error| {
                std.log.err(
                    "Polar Station native Workspace restore failed: {s}",
                    .{@errorName(restore_error)},
                );
                return false;
            };
        }
        return true;
    }

    pub fn nativePaneView(
        self: *const Executor,
        pane_id: []const u8,
    ) ?NativePaneView {
        const snapshot = self.nativeWorkspaceSnapshot() orelse return null;
        for (snapshot.tabs) |tab| for (tab.panes) |pane| {
            if (std.mem.eql(u8, pane.id, pane_id)) return .{
                .pane_id = pane.id,
                .tab_id = tab.id,
                .execution_target_id = pane.execution_target_id,
                .tmux_session_id = pane.tmux_session_id,
            };
        };
        return null;
    }

    pub fn nativeSelectedTabId(self: *const Executor) ?[]const u8 {
        return if (self.workspace.snapshot()) |workspace| workspace.selected_tab_id else null;
    }

    pub fn nativeTabIdForPage(self: *const Executor, page: *adw.TabPage) ?[]const u8 {
        const current = self.current orelse return null;
        for (current.tabs.items) |tab| {
            if (tab.active and page.getChild() == tab.tab.as(gtk.Widget)) return tab.tab_id;
        }
        return null;
    }

    /// Refresh native Tab-label controls even when Control Center is closed.
    pub fn syncTabFocusIndicators(self: *const Executor) void {
        const current = self.current orelse return;
        if (!current.windowExists()) return;
        const snapshot = self.nativeWorkspaceSnapshot() orelse return;
        for (current.tabs.items) |tab| {
            if (!tab.active) continue;
            const native = snapshot.tab(tab.tab_id) orelse continue;
            const page = current.window.getTabView().getPage(tab.tab.as(gtk.Widget));
            const focused = native.display_mode == .focus;
            const icon = gio.ThemedIcon.new(if (focused) "view-fullscreen-symbolic" else "view-grid-symbolic");
            defer icon.unref();
            page.setIndicatorIcon(icon.as(gio.Icon));
            page.setIndicatorActivatable(1);
            page.setIndicatorTooltip(if (focused) "Focus on — switch to Monitor" else "Focus off — switch to Focus");
        }
    }

    /// Borrows the one native Workspace mapping. Callers must not retain the
    /// returned slices across a native mutation.
    pub fn nativeWorkspaceSnapshot(
        self: *const Executor,
    ) ?renderpkg.Snapshot {
        return self.workspace.snapshot();
    }

    pub fn savePanelVisibility(self: *Executor, layout: bool, inbox_visible: bool) !void {
        var candidate = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
        candidate.layout_map_visible = layout;
        candidate.inbox_visible = inbox_visible;
        var previous = try self.rememberNativeWorkspace(candidate);
        defer if (previous) |*old| old.deinit();
    }

    pub fn nativeWorkspaceCatalog(
        self: *const Executor,
    ) ?native_workspace_store.Catalog {
        return self.workspace.catalogView();
    }

    /// Builds the Control Center's read-only Workspace view from the native
    /// single-writer table. The Host projection contributes only display facts
    /// and action copy; it never supplies Tab order, title, mode, selection, or
    /// Pane binding truth once native persistence exists.
    pub fn nativeWorkspaceMap(
        self: *const Executor,
        allocator: std.mem.Allocator,
        projection: *const controller_model.Projection,
        cache: ?*const session_runtime.Cache,
    ) !?controller_model.WorkspaceMap {
        const snapshot = self.nativeWorkspaceSnapshot() orelse return .{
            .workspace_id = "", .workspace_name = "Open Workspace", .desired_revision = 0,
            .selected_tab_id = "", .tabs = &.{},
        };
        return try workspace_view.build(
            allocator,
            snapshot,
            projection,
            cache,
            self.workspace.observed_pane_id,
        );
    }

    pub fn nativeWorkspaceName(self: *const Executor) ?[]const u8 {
        const snapshot = self.nativeWorkspaceSnapshot() orelse return null;
        return snapshot.workspace_name;
    }

    pub fn nativeManagedTerminalFontSize(self: *const Executor) ?u8 {
        const snapshot = self.nativeWorkspaceSnapshot() orelse return null;
        return snapshot.font_size_points;
    }

    pub fn nativeSelectedPaneId(self: *const Executor, tab_id: []const u8) ?[]const u8 {
        return self.workspace.selectedPane(tab_id);
    }

    fn observedNativePaneId(self: *const Executor, tab_id: []const u8) ?[]const u8 {
        const current = self.current orelse return null;
        for (current.tabs.items) |tab| {
            if (!tab.active or !std.mem.eql(u8, tab.tab_id, tab_id)) continue;
            const surface = tab.tab.getActiveSurface() orelse return null;
            for (current.panes.items) |pane| {
                if (pane.active and pane.tab == tab.tab and pane.surface == surface) return pane.pane_id;
            }
        }
        return null;
    }

    /// Resolve the selected Ghostty Surface against the native Layout before
    /// closing anything. A managed Pane must use the same remove_pane intent
    /// as its Control Center control; only an unregistered extra Split may be
    /// closed directly, and never when it is the Tab's lone Surface.
    pub fn closeFocusedPaneTarget(
        self: *const Executor,
        window: *Window,
    ) ?CloseFocusedPaneTarget {
        const page = window.getTabView().getSelectedPage() orelse return null;
        const tab = gobject.ext.cast(Tab, page.getChild()) orelse return null;
        const surface = tab.getActiveSurface() orelse return null;
        if (window.getActiveSurface() != surface) return null;

        if (self.current) |*current| {
            if (current.window == window) {
                if (!current.windowExists()) return null;
                for (current.panes.items) |pane| {
                    if (pane.surface == surface) {
                        if (!pane.active or pane.tab != tab) return null;
                        return .{ .managed = pane.pane_id };
                    }
                }
                for (current.tabs.items) |binding| {
                    if (binding.tab == tab) {
                        if (!binding.active or binding.placeholder_surface == surface)
                            return null;
                        break;
                    }
                }
            }
        } else if (self.managed_window == window) {
            // Workspace restoration has not established authoritative bindings.
            return null;
        }

        if (!tab.getSplitTree().getIsSplit()) return null;
        return .{ .native_split = surface };
    }

    /// Delivers one Host-authorized Inbox draft to the exact managed Surface.
    /// No clipboard, shell command, or tmux send-keys path is involved.
    pub fn deliverInboxText(
        self: *Executor,
        workspace_id: []const u8,
        tab_id: []const u8,
        pane_id: []const u8,
        execution_target_id: []const u8,
        tmux_session_id: []const u8,
        text: []const u8,
        send: bool,
    ) !void {
        const snapshot = self.nativeWorkspaceSnapshot() orelse
            return error.WorkspaceUnavailable;
        if (!std.mem.eql(u8, snapshot.workspace_id, workspace_id))
            return error.WorkspaceUnavailable;
        const tab = snapshot.tab(tab_id) orelse return error.TabNotFound;
        const desired = tab.pane(pane_id) orelse return error.PaneNotFound;
        if (!std.mem.eql(u8, desired.execution_target_id, execution_target_id) or
            !std.mem.eql(u8, desired.tmux_session_id, tmux_session_id))
            return error.StaleInboxSessionBinding;
        _ = try self.applyNativeWorkspaceAction(.{
            .activate_pane = .{ .pane_id = pane_id },
        });
        const current = if (self.current) |*value| value else return error.WorkspaceUnavailable;
        const pane = current.findPane(pane_id) orelse return error.PaneNotFound;
        if (!pane.isTerminal() or !std.mem.eql(u8, pane.content_state.terminal.execution_target_id, execution_target_id) or
            !std.mem.eql(u8, pane.content_state.terminal.tmux_session_id, tmux_session_id) or
            !pane.active) return error.StaleInboxSessionBinding;
        const core = pane.surface.core() orelse return error.SurfaceUnavailable;
        if (send) {
            const payload = try std.fmt.allocPrint(self.allocator, "{s}\r", .{text});
            defer self.allocator.free(payload);
            try core.textCallback(payload);
        } else {
            try core.textCallback(text);
        }
    }

    /// Host Session facts never mutate native membership or physical identity.
    pub fn applyControlProjection(self: *Executor, projection: *const controller_model.Projection) !void {
        _ = projection;
        if (self.current == null) if (self.nativeWorkspaceSnapshot()) |snapshot| {
            try self.applySnapshot(snapshot, .preserve_native);
        };
    }

    /// Applies a relayed Companion command to the native single-writer table
    /// without consulting or committing a Host Workspace projection.
    pub fn applyNativeWorkspaceAction(
        self: *Executor,
        action: control_intent.Action,
    ) !bool {
        if (!workspace_state.isNativeWorkspaceAction(action)) return false;
        if (self.workspace.catalogView() == null) {
            return error.NativeWorkspaceUnavailable;
        }
        if (action == .delete_tab) { try self.performNativeTabAction("close_tab", action.delete_tab.tab_id, null); return true; }
        if (action == .rename_tab) { try self.performNativeTabAction("rename_tab", action.rename_tab.tab_id, action.rename_tab.title); return true; }
        if (action == .use_existing_session) {
            const selection = action.use_existing_session;
            const workspace = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
            if (!std.mem.eql(u8, workspace.workspace_id, selection.workspace_id)) return error.WorkspaceChanged;
            var old = false;
            var existing = false;
            for (workspace.tabs) |tab| for (tab.panes) |pane| {
                if (std.mem.eql(u8, pane.execution_target_id, selection.execution_target_id) and std.mem.eql(u8, pane.tmux_session_id, selection.tmux_session_id)) old = true;
                if (std.mem.eql(u8, pane.execution_target_id, selection.replacement.execution_target_id) and std.mem.eql(u8, pane.tmux_session_id, selection.replacement.tmux_session_id)) existing = true;
            };
            if (old and existing) return error.AlreadyAssignedInWorkspace;
            if (!old) return true;
        }
        try self.workspace.begin();
        defer self.workspace.end();
        if (workspace_state.isNativeCatalogAction(action)) {
            return try self.applyNativeCatalogAction(action);
        }
        return self.applyWorkspaceAction(action);
    }

    pub fn ensureTerminalVisible(self: *Executor, source_pane: []const u8, execution: []const u8, session: []const u8) ![]const u8 {
        const before = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
        const tab_id = blk: {
            for (before.tabs) |tab| for (tab.panes) |pane| {
                if (std.mem.eql(u8,pane.id,source_pane)) break :blk tab.id;
            };
            return error.SourcePaneUnavailable;
        };
        if (!std.mem.eql(u8,tab_id,self.nativeSelectedTabId() orelse return error.SourceTabUnavailable)) return error.SourceTabUnavailable;
        var arena = std.heap.ArenaAllocator.init(self.allocator); defer arena.deinit();
        var candidate = try workspace_state.State.planAddSessions(arena.allocator(),before,tab_id,&.{.{.execution_target_id=execution,.tmux_session_id=session}});
        if ((candidate.tab(tab_id) orelse return error.SourceTabUnavailable).display_mode == .focus) {
            candidate = try workspace_state.State.planSnapshotAction(arena.allocator(),candidate,.{.toggle_display_mode=.{.tab_id=tab_id}},tab_id);
        }
        // tab_id still borrows the previous Catalog until the final lookup.
        var previous: ?native_workspace_store.OwnedCatalog = null;
        defer if (previous) |*old| old.deinit();
        if (candidate.workspace_revision != before.workspace_revision) {
            try self.workspace.begin(); defer self.workspace.end();
            previous = try self.commitNativeWorkspace(candidate,.preserve_native);
        }
        // Preserving the editor widget alone does not preserve Ghostty's
        // active Surface; native selection adoption would clear the Chat scope.
        try self.focusTerminal(self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable,source_pane);
        return self.nativePaneIdForSessionInTab(tab_id,execution,session) orelse error.TargetPaneUnavailable;
    }
    pub fn terminalViewState(self: *const Executor, pane_id: []const u8, execution: []const u8, session: []const u8) enum { pending, ready, unavailable } {
        const current = if (self.current) |*value| value else return .unavailable;
        for (current.panes.items) |*pane| {
            if (!std.mem.eql(u8,pane.pane_id,pane_id)) continue;
            if (!pane.active or !pane.isTerminal() or !std.mem.eql(u8,pane.content_state.terminal.execution_target_id,execution) or !std.mem.eql(u8,pane.content_state.terminal.tmux_session_id,session)) return .unavailable;
            if (pane.content_state.terminal.content_error != null) return .unavailable;
            return if (pane.content_state.terminal.view_ready) .ready else .pending;
        }
        return .unavailable;
    }

    pub fn appendSessions(self: *Executor, workspace_id: []const u8, tab_id: []const u8, sessions: []const controller_model.SessionIdentity) !void {
        const before = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
        if (!std.mem.eql(u8, before.workspace_id, workspace_id)) return error.WorkspaceChanged;
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const candidate = try workspace_state.State.planAddSessions(arena.allocator(), before, tab_id, sessions);
        if (candidate.workspace_revision == before.workspace_revision) return;
        try self.workspace.begin();
        defer self.workspace.end();
        var previous = try self.commitNativeWorkspace(candidate, .preserve_native);
        defer if (previous) |*value| value.deinit();
    }

    /// Local pointer placement uses the same native commit and Surface reuse
    /// path as other Workspace edits, without adding a Host intent.
    pub fn applyPaneDrop(self: *Executor, drop: pane_interaction.PaneDrop) !bool {
        const current = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const candidate = try pane_interaction.planDrop(arena.allocator(), current, drop) orelse return false;
        try self.workspace.begin();
        defer self.workspace.end();
        var previous_catalog = try self.commitNativeWorkspace(candidate, .force);
        defer if (previous_catalog) |*previous| previous.deinit();
        try self.focusTerminal(candidate, drop.pane_id);
        return true;
    }

    pub fn repairNativeWorkspace(self: *Executor) !void {
        const snapshot = self.nativeWorkspaceSnapshot() orelse
            return error.NativeWorkspaceUnavailable;
        if (self.current) |*current| {
            try current.reconnectSavedPaneContent(self.lifecycle());
        }
        try self.applySnapshot(snapshot, .force);
        if (self.current) |*current| try current.focusTab(snapshot.selected_tab_id, snapshot.tab(snapshot.selected_tab_id).?.focused_pane_id);
    }

    pub fn isStructuralAction(action: control_intent.Action) bool {
        return workspace_state.isNativeWorkspaceAction(action);
    }

    pub const QuickCreateTarget = struct { workspace_id: []const u8, tab_id: []const u8, source: ?controller_model.SessionIdentity };

    pub fn quickCreateTarget(self: *const Executor, window: *Window) !QuickCreateTarget {
        const current = self.current orelse return error.NativeWorkspaceUnavailable;
        if (current.window != window) return error.UnknownWindow;
        const snapshot = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
        const tab_id = current.activeNativeTabId() orelse return error.UnknownTab;
        const tab = snapshot.tab(tab_id) orelse return error.UnknownTab;
        if (tab.panes.len >= 9) return error.TabFull;
        const pane_id = if (tab.display_mode == .focus) tab.focused_pane_id else self.nativeSelectedPaneId(tab.id);
        const pane = if (pane_id) |id| tab.pane(id) else null;
        return .{ .workspace_id = snapshot.workspace_id, .tab_id = tab.id, .source = if (pane) |p| if (p.content == null and p.execution_target_id.len > 0 and p.tmux_session_id.len > 0) .{ .execution_target_id = p.execution_target_id, .tmux_session_id = p.tmux_session_id } else null else null };
    }

    /// Focus order is accepted logical state; physical trees only execute it.
    pub fn canCycleNativeFocus(self: *Executor) bool {
        return (self.nativeFocusCycleTarget(.next) catch null) != null;
    }
    pub fn nativeFocusCycleTarget(self: *Executor, direction: native_layout.FocusCycleDirection) !?[]const u8 {
        const workspace = self.nativeWorkspaceSnapshot() orelse return null;
        const tab = workspace.tab(workspace.selected_tab_id) orelse return null;
        if (tab.display_mode != .focus or tab.panes.len <= 1 or tab.panes.len > 9) return null;
        var canonical: [9][]const u8 = undefined;
        for (tab.panes, 0..) |pane, index| canonical[index] = pane.id;
        const cycle = try native_layout.cycleFocus(canonical[0..tab.panes.len], tab.visual_pane_ids, direction);
        return if (cycle.changed) cycle.mainPaneId() else null;
    }
    pub fn nativeFocusSidebarTarget(self: *Executor, slot: u8) !?[]const u8 {
        const workspace = self.nativeWorkspaceSnapshot() orelse return null;
        const tab = workspace.tab(workspace.selected_tab_id) orelse return null;
        if (tab.display_mode != .focus or slot < 1 or slot >= tab.visual_pane_ids.len) return null;
        return tab.visual_pane_ids[slot];
    }

    /// Numbers follow canonical managed Pane identity, not the mutable Focus
    /// visual slots. An unregistered Ghostty Split has no number.
    pub fn nativeNumberedPaneTarget(self: *const Executor, number: u8) ?[]const u8 {
        if (number < 1 or number > 9) return null;
        const selected_tab_id = self.nativeSelectedTabId() orelse return null;
        const workspace = self.nativeWorkspaceSnapshot() orelse return null;
        const selected = workspace.tab(selected_tab_id) orelse return null;
        const index: usize = @as(usize, number) - 1;
        if (index >= selected.panes.len) return null;
        return selected.panes[index].id;
    }

    const pane_number_sizes = [_]struct { pixels: c_int, css_class: [:0]const u8 }{
        .{ .pixels = 32, .css_class = "polar-pane-number-size-32" },
        .{ .pixels = 48, .css_class = "polar-pane-number-size-48" },
        .{ .pixels = 64, .css_class = "polar-pane-number-size-64" },
        .{ .pixels = 96, .css_class = "polar-pane-number-size-96" },
        .{ .pixels = 128, .css_class = "polar-pane-number-size-128" },
        .{ .pixels = 160, .css_class = "polar-pane-number-size-160" },
        .{ .pixels = 192, .css_class = "polar-pane-number-size-192" },
        .{ .pixels = 256, .css_class = "polar-pane-number-size-256" },
        .{ .pixels = 320, .css_class = "polar-pane-number-size-320" },
        .{ .pixels = 384, .css_class = "polar-pane-number-size-384" },
        .{ .pixels = 448, .css_class = "polar-pane-number-size-448" },
        .{ .pixels = 512, .css_class = "polar-pane-number-size-512" },
        .{ .pixels = 640, .css_class = "polar-pane-number-size-640" },
        .{ .pixels = 768, .css_class = "polar-pane-number-size-768" },
        .{ .pixels = 896, .css_class = "polar-pane-number-size-896" },
    };

    fn resizePaneNumber(overlay: *gtk.Overlay, label: *gtk.Label) void {
        const widget = label.as(gtk.Widget);
        const short_side = @min(
            overlay.as(gtk.Widget).getWidth(),
            overlay.as(gtk.Widget).getHeight(),
        );
        const target = @divTrunc(@max(short_side, 0) * 7, 10);
        var selected = pane_number_sizes[0].css_class;
        for (pane_number_sizes) |size| {
            widget.removeCssClass(size.css_class);
            if (size.pixels <= target) selected = size.css_class;
        }
        widget.addCssClass(selected);
    }

    pub fn setNumberBadgesVisible(self: *Executor, window: *Window, visible: bool) void {
        const current = if (self.current) |*value| value else return;
        if (current.window != window or !current.windowExists()) return;
        const selected_tab_id = current.activeNativeTabId();
        const workspace = self.nativeWorkspaceSnapshot() orelse return;
        const selected = if (selected_tab_id) |tab_id| workspace.tab(tab_id) else null;
        for (current.panes.items) |pane| {
            if (!pane.active) continue;
            const terminal_child = pane.surface.as(adw.Bin).getChild() orelse continue;
            const overlay = gobject.ext.cast(gtk.Overlay, terminal_child) orelse continue;
            var badge: ?*gtk.Label = null;
            var child = overlay.as(gtk.Widget).getFirstChild();
            while (child) |widget| : (child = widget.getNextSibling()) {
                if (widget.hasCssClass("polar-pane-number") != 0) {
                    badge = gobject.ext.cast(gtk.Label, widget);
                    break;
                }
            }
            var number: ?usize = null;
            if (visible) if (selected) |tab| {
                if (std.mem.eql(u8, pane.tab_id, tab.id)) {
                    for (tab.panes, 0..) |candidate, index| {
                        if (std.mem.eql(u8, candidate.id, pane.pane_id)) {
                            number = index + 1;
                            break;
                        }
                    }
                }
            };
            if (number) |value| {
                const label = badge orelse blk: {
                    const created = gtk.Label.new("");
                    const widget = created.as(gtk.Widget);
                    widget.addCssClass("polar-pane-number");
                    widget.setHalign(.center);
                    widget.setValign(.center);
                    widget.setCanTarget(0);
                    overlay.addOverlay(widget);
                    overlay.setMeasureOverlay(widget, 0);
                    break :blk created;
                };
                var text_buffer: [3]u8 = undefined;
                const text_value = std.fmt.bufPrintZ(&text_buffer, "{d}", .{value}) catch continue;
                label.setText(text_value);
                resizePaneNumber(overlay, label);
                label.as(gtk.Widget).setVisible(1);
            } else if (badge) |label| {
                label.as(gtk.Widget).setVisible(0);
            }
        }
    }

    pub fn moveNativeTab(self: *Executor, tab_id: []const u8, target_id: ?[]const u8, new_name: []const u8) !void {
        const root = self.catalog_owner orelse self;
        if (root.pending_lifecycle != null) return error.NativeTransactionInProgress;
        if (!root.workspace.unbound and root.workspace.workspace_id == null) {
            root.workspace.workspace_id = try root.allocator.dupe(u8, root.workspace.snapshot().?.workspace_id);
        }
        var candidate = try self.workspace.prepareTabTransfer(tab_id, target_id, new_name);
        errdefer candidate.deinit();
        try self.workspace.begin();
        errdefer self.workspace.end();
        try root.enqueueNativeState(candidate.value);
        root.pending_lifecycle = .{ .candidate = candidate, .owner = self, .archive = false, .revision = root.persistence_revision, .transfer = true };
    }
    pub fn refreshTransferredWorkspace(self: *Executor) !void {
        if (self.nativeWorkspaceSnapshot()) |snapshot| try self.applySnapshot(snapshot, .force);
    }

    pub fn nativePaneIdForSurface(self: *const Executor, surface: *Surface) ?[]const u8 {
        if (self.current) |current| for (current.panes.items) |pane| {
            if (pane.active and pane.surface == surface) return pane.pane_id;
        };
        return null;
    }

    pub fn nativeActionDecision(self: *const Executor, action: []const u8, tab_id: ?[]const u8) !std.json.Parsed(@import("workspace_lifecycle.zig").Decision) {
        const catalog = self.nativeWorkspaceCatalog() orelse return error.NativeWorkspaceUnavailable;
        const snapshot = self.nativeWorkspaceSnapshot();
        const ids = try self.allocator.alloc([]const u8, catalog.workspaces.len);
        defer self.allocator.free(ids);
        for (catalog.workspaces, 0..) |workspace, i| ids[i] = workspace.workspace_id;
        var index: ?usize = null;
        if (snapshot) |workspace| for (workspace.tabs, 0..) |tab, i| {
            if (tab_id != null and std.mem.eql(u8, tab.id, tab_id.?)) { index = i; break; }
        };
        const name = try std.fmt.allocPrint(self.allocator, "native.{s}", .{action});
        defer self.allocator.free(name);
        return @import("workspace_lifecycle.zig").decide(self.allocator, .{ .action = name, .ids = ids,
            .archived = catalog.archived_workspace_ids, .open = catalog.open_workspace_ids,
            .selected = catalog.selected_workspace_id, .bound = if (snapshot) |w| w.workspace_id else null,
            .target = @as(?[]const u8, null), .tab_count = if (snapshot) |w| w.tabs.len else @as(usize, 0), .tab_index = index });
    }

    pub fn performNativeTabAction(self: *Executor, action: []const u8, tab_id: []const u8, title: ?[]const u8) !void {
        var decision = try self.nativeActionDecision(action, tab_id);
        defer decision.deinit();
        if (decision.value.@"error" != null) return error.NativeActionDisabled;
        if (std.mem.eql(u8, decision.value.effect, "close")) {
            if (self.managed_window) |window| { _ = @import("main.zig").closeManagedWindow(window); }
            return;
        }
        try self.workspace.begin();
        defer self.workspace.end();
        const before = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        var candidate = before;
        if (std.mem.eql(u8, action, "rename_tab")) {
            const trimmed = std.mem.trim(u8, title orelse "", " \t\r\n");
            if (trimmed.len == 0) return error.InvalidTabTitle;
            candidate = try workspace_state.State.planSnapshotAction(arena.allocator(), candidate,
                .{ .rename_tab = .{ .tab_id = tab_id, .title = trimmed } }, null);
        } else {
            var right = false;
            for (before.tabs) |tab| {
                const target = std.mem.eql(u8, tab.id, tab_id);
                const remove = if (std.mem.eql(u8, action, "close_tab")) target
                    else if (std.mem.eql(u8, action, "close_other_tabs")) !target else right;
                if (target) right = true;
                if (remove) candidate = try workspace_state.State.planSnapshotAction(arena.allocator(), candidate,
                    .{ .delete_tab = .{ .tab_id = tab.id, .confirmation_id = "native-close" } }, null);
            }
        }
        const root = self.catalog_owner orelse self;
        if (root.pending_lifecycle != null) return error.NativeTransactionInProgress;
        var saved = try self.workspace.prepareWorkspace(candidate);
        errdefer saved.deinit();
        try root.enqueueNativeState(saved.value);
        root.pending_lifecycle = .{ .candidate = saved, .owner = self, .archive = false,
            .revision = root.persistence_revision, .apply_layout = true };
    }

    /// Native tab-bar drag changes the same persisted order as Layout arrows.
    /// Do not interpret programmatic reconciliation or foreign pages as input.
    pub fn adoptNativeTabOrder(self: *Executor, view: *adw.TabView, page: *adw.TabPage, position: c_int) !bool {
        if (self.workspace.committing or self.applying_physical_snapshot or position < 0) return false;
        const current = if (self.current) |*value| value else return false;
        if (!current.windowExists() or current.window.getTabView() != view) return false;
        const before = self.nativeWorkspaceSnapshot() orelse return false;
        if (view.getNPages() != before.tabs.len) return false;
        const native_tab = gobject.ext.cast(Tab, page.getChild()) orelse return false;
        for (current.tabs.items) |binding| {
            if (!binding.active or binding.tab != native_tab) continue;
            const index: usize = @intCast(position);
            if (index >= before.tabs.len or std.mem.eql(u8, before.tabs[index].id, binding.tab_id)) return false;
            try self.workspace.begin();
            defer self.workspace.end();
            var arena = std.heap.ArenaAllocator.init(self.allocator);
            defer arena.deinit();
            const candidate = try self.workspace.planAction(arena.allocator(), .{ .reorder_tab = .{
                .tab_id = binding.tab_id, .to_index = index,
            } }, null);
            var previous = try self.rememberNativeWorkspace(candidate);
            defer if (previous) |*old| old.deinit();
            return true;
        }
        return false;
    }

    /// Adopts a user-selected native Ghostty Tab into the single-writer
    /// Workspace document. No Host intent or projection round trip occurs.
    pub fn adoptNativeTabSelection(self: *Executor) !bool {
        if (self.workspace.committing) return false;
        const current = if (self.current) |*value| value else return false;
        const selected = current.activeNativeTabId() orelse return false;
        const before = self.nativeWorkspaceSnapshot() orelse return false;
        const pane_changed = try self.workspace.observePane(self.observedNativePaneId(selected));
        if (std.mem.eql(u8, selected, before.selected_tab_id)) return pane_changed and before.tab(selected).?.display_mode == .monitor;
        @import("ipc_client.zig").trace(50, selected, 0);
        try self.workspace.begin();
        defer self.workspace.end();
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const candidate = try self.workspace.planAction(arena.allocator(), .{ .select_tab = .{ .tab_id = selected } }, selected);
        var previous = try self.rememberNativeWorkspace(candidate);
        defer if (previous) |*old| old.deinit();
        @import("ipc_client.zig").trace(51, self.workspace.snapshot().?.selected_tab_id, self.workspace.mutation_revision);
        return true;
    }

    fn applyWorkspaceAction(self: *Executor, action: control_intent.Action) !bool {
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const before = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
        const observed = if (self.current) |*current| current.activeNativeTabId() else null;
        const candidate = try self.workspace.planAction(arena.allocator(), action, observed);
        var previous = try self.commitNativeWorkspace(candidate, workspace_state.selectionPolicyForAction(action));
        defer if (previous) |*old| old.deinit();
        try self.focusAfterAction(before, candidate, action);
        return true;
    }

    pub fn actionMovesTerminalFocus(action: control_intent.Action) bool {
        return switch (action) {
            .select_pane, .activate_pane, .move_pane, .toggle_display_mode, .select_tab, .add_tab, .delete_tab, .remove_pane, .toggle_session_assignment, .select_workspace, .create_workspace, .delete_workspace => true,
            else => false,
        };
    }

    fn focusAfterAction(
        self: *Executor,
        before: renderpkg.Snapshot,
        after: renderpkg.Snapshot,
        action: control_intent.Action,
    ) !void {
        if (!actionMovesTerminalFocus(action)) return;
        const tab = after.tab(after.selected_tab_id) orelse return;
        const requested: ?[]const u8 = switch (action) {
            .select_pane => |value| blk: {
                _ = tab.pane(value.pane_id) orelse return;
                break :blk value.pane_id;
            },
            .activate_pane => |value| value.pane_id,
            .move_pane => |value| value.pane_id,
            .toggle_session_assignment => |value| if (value.focus_new and std.mem.eql(u8, value.tab_id orelse "", tab.id)) value.pane_id else null,
            .toggle_display_mode => |value| blk: {
                if (!std.mem.eql(u8, value.tab_id, tab.id)) return;
                break :blk if (before.tab(tab.id)) |old| old.focused_pane_id else null;
            },
            else => null,
        };
        try self.focusTerminal(after, requested);
    }

    fn focusTerminal(self: *Executor, snapshot: renderpkg.Snapshot, requested: ?[]const u8) !void {
        const current = if (self.current) |*value| value else return;
        const tab = snapshot.tab(snapshot.selected_tab_id) orelse return;
        const pane_id = native_surface_transition.terminalFocusPaneId(
            if (tab.display_mode == .focus) .focus else .monitor,
            tab.focused_pane_id,
            requested,
        );
        try current.focusTab(tab.id, pane_id);
        self.activateSelectedConversation();
        if (current.window.getActiveSurface()) |surface| for (current.panes.items) |pane| {
            if (pane.active and pane.surface==surface and pane.display()!=null) {self.content_hooks.activated(pane.pane_id);break;}
        };
    }

    /// User-opened structured content shares the existing native workspace owner.
    pub fn openContent(self: *Executor, content: renderpkg.Content, replace_pane: ?[]const u8) !void {
        // Creating a Tab emits selection/focus signals synchronously. Keep the
        // accepted catalog alive until its physical tree and focus converge.
        try self.workspace.begin();
        defer self.workspace.end();
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const plan = if (replace_pane) |pane_id|
            self.workspace.planReplaceContent(arena.allocator(), pane_id, content) catch |err| switch (err) {
                error.NoConversationInCurrentTab => try self.workspace.planOpenContent(arena.allocator(), content),
                else => return err,
            }
        else
            try self.workspace.planOpenContent(arena.allocator(), content);
        var previous = try self.commitNativeWorkspace(plan.snapshot, .force);
        defer if (previous) |*value| value.deinit();
        if (self.current) |*current| try current.focusTab(plan.tab_id, plan.pane_id);
        self.content_hooks.selected(plan.pane_id);
        self.content_hooks.activated(plan.pane_id);
    }

    fn commitNativeWorkspace(self: *Executor, candidate: renderpkg.Snapshot, policy: SelectionPolicy) !?native_workspace_store.OwnedCatalog {
        try self.validateSessionReservations(candidate);
        // The saved user intent remains authoritative even if a Pane Host or
        // Ghostty Surface disappears while the physical tree is converging.
        var previous_catalog = try self.rememberNativeWorkspace(candidate);
        errdefer if (previous_catalog) |*previous| previous.deinit();
        // Candidate/action strings can borrow the previous Catalog. Keep that
        // owner until the whole action finishes, and render the new owned copy.
        const saved = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
        self.applySnapshot(saved, policy) catch |physical_error| {
            std.log.err("Polar Station saved Layout but physical repair is pending: {s}", .{@errorName(physical_error)});
            return error.PhysicalRepairPending;
        };
        return previous_catalog;
    }

    fn rememberNativeWorkspace(self: *Executor, snapshot: renderpkg.Snapshot) !?native_workspace_store.OwnedCatalog {
        return self.admitNativeCatalog(try self.workspace.prepareWorkspace(snapshot));
    }

    fn admitNativeCatalog(self: *Executor, candidate: native_workspace_store.OwnedCatalog) !?native_workspace_store.OwnedCatalog {
        var owned = candidate;
        errdefer owned.deinit();
        if ((self.catalog_owner orelse self).pending_lifecycle != null) return error.NativeTransactionInProgress;
        const acceptance = try self.workspace.prepareAcceptance(owned);
        errdefer self.allocator.free(acceptance.selected_id);
        try self.enqueueNativeState(owned.value);
        return self.workspace.accept(acceptance);
    }

    fn applyNativeCatalogAction(self: *Executor, action: control_intent.Action) !bool {
        var previous = try self.admitNativeCatalog(try self.workspace.prepareCatalogAction(action));
        defer if (previous) |*old| old.deinit();
        const saved = self.nativeWorkspaceSnapshot() orelse return error.NativeWorkspaceUnavailable;
        self.applySnapshot(saved, .force) catch return error.PhysicalRepairPending;
        try self.focusTerminal(saved, null);
        return true;
    }

    pub fn flushNativeState(self: *Executor) !u64 {
        const root = self.catalog_owner orelse self;
        if (root.pending_lifecycle != null) return error.NativeTransactionInProgress;
        try root.enqueueNativeState(root.nativeWorkspaceCatalog() orelse return error.NativeWorkspaceUnavailable);
        return root.persistence_revision;
    }

    pub fn enqueueInitialStateRead(self: *Executor) !void {
        const store = self.workspace_store orelse return;
        const callback = self.lifecycle().submit_native_state orelse return error.ContentWorkerUnavailable;
        const request = try std.json.Stringify.valueAlloc(self.allocator, .{ .path = store.path, .revision = @as(u64, 0), .document = @as(?[]const u8, null) }, .{});
        defer self.allocator.free(request);
        try callback(self.lifecycle().submit_context, request);
    }

    fn enqueueNativeState(self: *Executor, catalog: native_workspace_store.Catalog) !void {
        if (self.catalog_owner) |owner| return owner.enqueueNativeState(catalog);
        const store = self.workspace_store orelse return;
        const callback = self.lifecycle().submit_native_state orelse return error.ContentWorkerUnavailable;
        const document = try native_workspace_store.encodeCatalog(self.allocator, catalog);
        defer self.allocator.free(document);
        // Zero identifies the initial read, not an invalid Catalog revision.
        const next = try std.math.add(u64, self.persistence_revision, 1);
        const request = try std.json.Stringify.valueAlloc(self.allocator, .{ .path = store.path, .revision = next, .document = document }, .{});
        defer self.allocator.free(request);
        try callback(self.lifecycle().submit_context, request);
        self.persistence_revision = next;
    }

    pub fn applyNativeStateResponse(self: *Executor, payload: []const u8) !bool {
        const Response = struct { revision: u64, document: ?[]const u8, @"error": ?[]const u8 };
        var parsed = try std.json.parseFromSlice(Response, self.allocator, payload, .{ .ignore_unknown_fields = true });
        defer parsed.deinit();
        const value = parsed.value;
        if (self.pending_lifecycle) |pending| {
            if (value.revision == pending.revision) {
                self.pending_lifecycle = null;
                self.workspace.committing = false;
                var candidate = pending.candidate;
                if (value.@"error" != null) { candidate.deinit(); return error.NativeStatePersistenceFailed; }
                var previous = self.workspace.catalog;
                self.workspace.catalog = candidate;
                defer if (previous) |*old| old.deinit();
                if (pending.apply_layout) if (pending.owner) |owner| {
                    // Bind selection only after the single catalog write succeeds.
                    if (owner.workspace.workspace_id) |id| owner.allocator.free(id);
                    owner.workspace.workspace_id = try owner.allocator.dupe(u8, candidate.value.selected_workspace_id);
                    try owner.refreshTransferredWorkspace();
                };
                if (pending.archive) if (pending.owner) |owner| { try owner.unbindWorkspace(); };
                if (pending.transfer) if (pending.owner) |owner| { try @import("main.zig").finishNativeTabTransfer(owner, candidate.value.selected_workspace_id); };
                return true;
            }
        }
        if (value.revision != self.persistence_revision) return false;
        if (value.@"error" != null) return error.NativeStatePersistenceFailed;
        if (value.revision != 0) return false;
        const document = value.document orelse return false;
        @import("ipc_client.zig").trace(19, "", 0);
        const acceptance = blk: {
            var saved = try native_workspace_store.decodeCatalog(self.allocator, document);
            errdefer saved.deinit();
            break :blk try self.workspace.prepareAcceptance(saved);
        };
        var previous = self.workspace.accept(acceptance);
        defer if (previous) |*old| old.deinit();
        if (self.managed_window != null) {
            const snapshot = self.nativeWorkspaceSnapshot().?;
            try self.applySnapshot(snapshot, .preserve_native);
            try self.focusTerminal(snapshot, null);
        }
        return true;
    }

    pub fn nativePaneIdForSession(
        self: *const Executor,
        execution_target_id: []const u8,
        tmux_session_id: []const u8,
    ) ?[]const u8 {
        const workspace = self.nativeWorkspaceSnapshot() orelse return null;
        for (workspace.tabs) |tab| {
            for (tab.panes) |pane| {
                if (std.mem.eql(
                    u8,
                    pane.execution_target_id,
                    execution_target_id,
                ) and std.mem.eql(
                    u8,
                    pane.tmux_session_id,
                    tmux_session_id,
                )) return pane.id;
            }
        }
        return null;
    }

    pub fn nativePaneIdForSessionInTab(
        self: *const Executor,
        tab_id: []const u8,
        execution_target_id: []const u8,
        tmux_session_id: []const u8,
    ) ?[]const u8 {
        const workspace = self.nativeWorkspaceSnapshot() orelse return null;
        const tab = workspace.tab(tab_id) orelse return null;
        for (tab.panes) |pane| {
            if (std.mem.eql(u8, pane.execution_target_id, execution_target_id) and
                std.mem.eql(u8, pane.tmux_session_id, tmux_session_id))
            {
                return pane.id;
            }
        }
        return null;
    }

    /// Joins Host Session identity to the click-time native Workspace table.
    /// Returned slices are borrowed and must not survive a native mutation.
    pub fn nativeSessionAssignment(
        self: *const Executor,
        execution_target_id: []const u8,
        tmux_session_id: []const u8,
    ) NativeSessionAssignment {
        const workspace = self.nativeWorkspaceSnapshot() orelse
            return .no_managed_tab;
        const selected_tab_id = self.nativeSelectedTabId() orelse
            workspace.selected_tab_id;
        const selected_tab = workspace.tab(selected_tab_id) orelse
            return .no_managed_tab;
        for (selected_tab.panes) |pane| {
            if (std.mem.eql(u8, pane.execution_target_id, execution_target_id) and
                std.mem.eql(u8, pane.tmux_session_id, tmux_session_id))
            {
                return .{ .assigned_here = .{
                    .tab_id = selected_tab.id,
                    .pane_id = pane.id,
                } };
            }
        }
        if (selected_tab.panes.len >= 9) {
            return .{ .tab_full = .{ .tab_id = selected_tab.id } };
        }
        return .{ .available = .{ .tab_id = selected_tab.id } };
    }

    pub fn nativeTabCount(self: *const Executor) usize {
        return if (self.nativeWorkspaceSnapshot()) |workspace|
            workspace.tabs.len
        else
            0;
    }

    pub fn render(self: *Executor, json: []const u8) ![]u8 {
        var request = try renderpkg.RenderRequest.parse(
            self.allocator,
            json,
            self.identity,
        );
        defer request.deinit();

        self.applySnapshot(request.snapshot, .preserve_native) catch |render_error| {
            std.log.err(
                "Polar Station Workspace render failed: {s}",
                .{@errorName(render_error)},
            );
            return renderpkg.encodeRejectedReceipt(
                self.allocator,
                self.identity,
                request.transaction_id,
                request.snapshot.workspace_id,
                request.snapshot.workspace_revision,
                request.snapshot.snapshot_fingerprint,
                "frontend_render_failed",
                "managed Ghostty could not render the Workspace snapshot",
                true,
            );
        };
        return renderpkg.encodeAppliedReceipt(
            self.allocator,
            self.identity,
            request.transaction_id,
            request.snapshot.workspace_id,
            request.snapshot.workspace_revision,
            request.snapshot.snapshot_fingerprint,
        );
    }

    fn applySnapshot(
        self: *Executor,
        snapshot: renderpkg.Snapshot,
        selection_policy: SelectionPolicy,
    ) !void {
        self.applying_physical_snapshot = true;
        defer self.applying_physical_snapshot = false;
        @import("ipc_client.zig").trace(15, "", snapshot.workspace_revision);
        defer @import("ipc_client.zig").trace(16, "", snapshot.workspace_revision);
        if (self.current) |*current| {
            if (!current.windowExists()) {
                current.retireAllPaneContent(self.lifecycle());
                current.deinit();
                self.current = null;
            }
        }
        if (self.current) |*current| {
            try current.reconcile(
                snapshot,
                selection_policy,
                self.lifecycle(),
            );
            self.retryContentRetirements();
            return;
        }

        const window = self.managed_window orelse
            return error.ManagedWindowUnavailable;
        var created = try AppliedRenderState.initBorrowed(
            self.allocator,
            self.app,
            window,
            snapshot,
        );
        errdefer {
            created.cleanupFailedInitialRender(self.lifecycle());
            created.deinit();
            self.retryContentRetirements();
        }
        created.hooks = self.content_hooks;
        try created.buildInitial(snapshot, self.lifecycle());
        try created.verify();
        created.present();
        self.current = created;
        self.retryContentRetirements();
    }

    /// Called for explicit native Pane/Tab navigation, never for arbitrary
    /// focus changes into an editor, menu, search field or popover.
    pub fn terminalSelection(self: *Executor, allocator: std.mem.Allocator, pane_id: []const u8) ?[]u8 {
        const current = if (self.current) |*value| value else return null;
        const surface = current.window.getActiveSurface() orelse return null;
        for (current.panes.items) |pane| {
            if (!pane.active or pane.surface != surface or pane.display() != null or !std.mem.eql(u8,pane.pane_id,pane_id)) continue;
            const core = pane.surface.core() orelse return null;
            core.renderer_state.mutex.lock();
            defer core.renderer_state.mutex.unlock();
            const selection = core.io.terminal.screens.active.selection orelse return null;
            var selected = core.dumpTextLocked(allocator,selection) catch return null;
            defer selected.deinit(allocator);
            if (selected.text.len == 0 or selected.text.len > 1024 * 1024) return null;
            return allocator.dupe(u8,selected.text) catch null;
        }
        return null;
    }

    pub fn activateSelectedConversation(self: *Executor) void {
        const current = if (self.current) |*value| value else return;
        if (!current.windowExists()) return;
        const surface = current.window.getActiveSurface() orelse return;
        for (current.panes.items) |pane| {
            if (pane.active and pane.surface == surface) {
                if (pane.display() != null) self.content_hooks.selected(pane.pane_id) else self.content_hooks.deselected(current.window, pane.pane_id, pane.content_state.terminal.execution_target_id, pane.content_state.terminal.tmux_session_id);
                return;
            }
        }
    }

    pub fn syncTerminalFocus(self: *Executor) void {
        if (self.current) |*current| {
            for (current.panes.items) |*pane| {
                if (pane.active and pane.isTerminal()) pane.content_state.terminal.focus_listener.sync(false);
            }
        }
    }

    pub fn applyContentResponse(self: *Executor, payload: []const u8) !void {
        var parsed = try std.json.parseFromSlice(std.json.Value, self.allocator, payload, .{});
        defer parsed.deinit();
        const response = parsed.value.object;
        const request_id = (response.get("request_id") orelse return error.InvalidContentResponse).string;
        const channel_id = (response.get("channel_id") orelse return error.InvalidContentResponse).string;
        const generation = std.math.cast(u64, (response.get("channel_generation") orelse return error.InvalidContentResponse).integer) orelse return error.InvalidContentResponse;
        const revision = std.math.cast(u64, (response.get("content_revision") orelse return error.InvalidContentResponse).integer) orelse return error.InvalidContentResponse;
        const result = (response.get("result") orelse return error.InvalidContentResponse).object;
        // A control acknowledgement never changes layout, binding or recovery UI.
        if (std.mem.eql(u8, (result.get("status") orelse return error.InvalidContentResponse).string, "terminal_control")) return;
        if (std.mem.eql(u8, (result.get("status") orelse return error.InvalidContentResponse).string, "recovery")) {
            if (self.current) |*current| {
                for (current.panes.items) |*pane| {
                    if (!pane.active or !pane.isTerminal() or !std.mem.eql(u8, pane.content_state.terminal.content_channel_id, channel_id) or generation != pane.content_state.terminal.content_channel_generation or revision < pane.content_state.terminal.content_revision) continue;
                    const view = (result.get("view") orelse return error.InvalidContentResponse).object;
                    const phase = (view.get("phase") orelse return error.InvalidContentResponse).string;
                    pane.content_state.terminal.view_ready = std.mem.eql(u8,phase,"hidden");
                    if (std.mem.eql(u8, phase, "hidden") or std.mem.eql(u8, phase, "recovered")) pane.content_state.terminal.focus_listener.ready(revision);
                    const child = pane.surface.as(adw.Bin).getChild() orelse continue;
                    const overlay = gobject.ext.cast(gtk.Overlay, child) orelse continue;
                    if (pane.content_state.terminal.recovery_controls == null) pane.content_state.terminal.recovery_controls = try terminal_recovery.Controls.create(current.arena.allocator(), self.lifecycle(), overlay, pane.surface.as(gtk.Widget), pane.content_state.terminal.content_control_socket, pane.content_state.terminal.content_channel_id, pane.content_state.terminal.content_channel_generation);
                    try pane.content_state.terminal.recovery_controls.?.render((result.get("view") orelse return error.InvalidContentResponse).object);
                }
            }
            return;
        }
        if (self.current) |*current| {
            for (current.panes.items) |*pane| {
                if (!pane.active or !pane.isTerminal() or !std.mem.eql(u8, pane.content_state.terminal.content_channel_id, channel_id) or
                    generation != pane.content_state.terminal.content_channel_generation or revision != pane.content_state.terminal.content_revision or
                    !std.mem.eql(u8, pane.content_state.terminal.content_pending_request_id orelse continue, request_id)) continue;
                var outcome = try pane_content.decodeResponse(self.allocator, payload, .{
                    .request_id = request_id,
                    .channel_id = channel_id,
                    .channel_generation = pane.content_state.terminal.content_channel_generation,
                    .content_revision = pane.content_state.terminal.content_revision,
                });
                defer outcome.deinit(self.allocator);
                pane.content_state.terminal.content_pending_request_id = null;
                if (outcome.kind == .accepted or outcome.kind == .ready) {
                    const binding = outcome.binding orelse return error.InvalidContentBinding;
                    if (!std.mem.eql(u8, binding.execution_target, pane.content_state.terminal.execution_target_id) or
                        !std.mem.eql(u8, binding.tmux_session_id, pane.content_state.terminal.tmux_session_id)) return error.ContentBindingMismatch;
                    pane.content_state.terminal.content_applied_revision = pane.content_state.terminal.content_revision;
                    pane.content_state.terminal.content_error = null;
                } else {
                    pane.content_state.terminal.content_error = try current.arena.allocator().dupe(u8, outcome.error_message orelse "The terminal connection is unavailable.");
                    std.log.warn("Polar Station terminal connection rejected channel={s}", .{channel_id});
                }
                return;
            }
        }
        var index: usize = 0;
        while (self.lifecycle().retirementAt(index)) |lease| : (index += 1) {
            if (!std.mem.eql(u8, lease.channel_id, channel_id) or generation != lease.channel_generation or
                revision != lease.content_revision or !std.mem.eql(u8, lease.request_id, request_id)) continue;
            const lease_id = lease.lease_id;
            var outcome = try pane_content.decodeResponse(self.allocator, payload, .{
                .request_id = lease.request_id,
                .channel_id = lease.channel_id,
                .channel_generation = lease.channel_generation,
                .content_revision = lease.content_revision,
            });
            defer outcome.deinit(self.allocator);
            if (outcome.kind == .detached or
                (outcome.error_code != null and std.mem.eql(u8, outcome.error_code.?, "content_endpoint_gone")))
            {
                _ = self.lifecycle().completeRetirement(lease_id);
            } else {
                self.lifecycle().markRetirementInFlight(lease_id, false);
            }
            return;
        }
        // Removed/replaced channels deliberately ignore late completions.
    }

    fn retryContentRetirements(self: *Executor) void {
        var index: usize = 0;
        while (self.lifecycle().retirementAt(index)) |lease| {
            const lease_id = lease.lease_id;
            if (lease.in_flight) {
                index += 1;
                continue;
            }
            const request = pane_content.encodeRetire(
                self.allocator,
                .{
                    .request_id = lease.request_id,
                    .channel_id = lease.channel_id,
                    .channel_generation = lease.channel_generation,
                    .content_revision = lease.content_revision,
                },
            ) catch {
                index += 1;
                continue;
            };
            defer self.allocator.free(request);
            self.lifecycle().submit(lease.control_socket, request) catch |retire_error| {
                self.lifecycle().markRetirementAttempt(lease_id);
                std.log.warn(
                    "Polar Station retained Pane retirement lease={d} attempt_error={s}",
                    .{ lease_id, @errorName(retire_error) },
                );
                index += 1;
                continue;
            };
            self.lifecycle().markRetirementAttempt(lease_id);
            self.lifecycle().markRetirementInFlight(lease_id, true);
            index += 1;
        }
    }
};

fn captureSurfaceSet(
    tab: *Tab,
    storage: *[10]*Surface,
) ![]const *Surface {
    const tree = tab.getSurfaceTree() orelse return storage[0..0];
    var count: usize = 0;
    var iterator = tree.iterator();
    while (iterator.next()) |entry| {
        if (count == storage.len) return error.TooManySurfaces;
        storage[count] = entry.view;
        count += 1;
    }
    return storage[0..count];
}

fn findAddedSurface(
    tab: *Tab,
    prior_surfaces: []const *Surface,
) ?*Surface {
    const tree = tab.getSurfaceTree() orelse return null;
    var iterator = tree.iterator();
    while (iterator.next()) |entry| {
        for (prior_surfaces) |prior| {
            if (prior == entry.view) break;
        } else return entry.view;
    }
    return null;
}

const AppliedRenderState = struct {
    hooks: ContentHooks = .{},
    arena: std.heap.ArenaAllocator,
    app: *Application,
    workspace_id: []const u8,
    workspace_revision: u64,
    snapshot_fingerprint: []const u8,
    window: *Window,
    window_lifetime: *WindowLifetime,
    owns_window: bool,
    baseline_tab_count: usize,
    window_id: []const u8,
    tabs: std.ArrayList(TabBinding) = .empty,
    panes: std.ArrayList(PaneBinding) = .empty,

    const TabBinding = struct {
        tab_id: []const u8,
        title: []const u8,
        physical_tab_id: []const u8,
        tab: *Tab,
        owns_tab: bool,
        placeholder_surface: ?*Surface = null,
        layout: ?native_layout.Layout = null,
        active: bool = true,
    };

    const TerminalBinding = struct {
        execution_target_id: []const u8,
        tmux_session_id: []const u8,
        content_channel_id: []const u8,
        content_channel_generation: u64,
        content_revision: u64,
        content_applied_revision: u64 = 1,
        content_pending_request_id: ?[]const u8 = null,
        content_error: ?[]const u8 = null,
        view_ready: bool = false,
        focus_listener: *terminal_focus.Focus,
        recovery_controls: ?*terminal_recovery.Controls = null,
        content_control_socket: []const u8,
    };

    const PaneBinding = struct {
        pane_id: []const u8,
        physical_surface_id: []const u8,
        tab_id: []const u8,
        tab: *Tab,
        attach_fingerprint: []const u8,
        surface: *Surface,
        trace_identity_emitted: bool = false,
        active: bool = true,
        content_state: union(enum) {
            terminal: TerminalBinding,
            display: struct { controller: *DisplaySurface, identity: renderpkg.Content },
        },

        fn isTerminal(self: PaneBinding) bool {
            return self.content_state == .terminal;
        }

        fn display(self: PaneBinding) ?*DisplaySurface {
            return switch (self.content_state) {
                .terminal => null,
                .display => |value| value.controller,
            };
        }

        // Called only after queuePaneRetirement has disconnected subscribers.
        fn release(self: PaneBinding) void {
            switch (self.content_state) {
                .terminal => |terminal| terminal.focus_listener.detach(),
                .display => |value| value.controller.destroy() catch {},
            }
            self.surface.unref();
        }
    };

    fn init(
        allocator: std.mem.Allocator,
        app: *Application,
        snapshot: renderpkg.Snapshot,
    ) !AppliedRenderState {
        var arena = std.heap.ArenaAllocator.init(allocator);
        errdefer arena.deinit();
        const alloc = arena.allocator();
        const window = Window.new(app, .none);
        _ = app.as(gobject.Object).bindProperty(
            "config",
            window.as(gobject.Object),
            "config",
            .{},
        );
        const state: AppliedRenderState = .{
            .arena = arena,
            .app = app,
            .workspace_id = try alloc.dupe(u8, snapshot.workspace_id),
            .workspace_revision = snapshot.workspace_revision,
            .snapshot_fingerprint = try alloc.dupe(
                u8,
                snapshot.snapshot_fingerprint,
            ),
            .window = window,
            .window_lifetime = try alloc.create(WindowLifetime),
            .owns_window = true,
            .baseline_tab_count = 0,
            .window_id = try physicalId(alloc, "window", window),
        };
        state.window_lifetime.* = .{};
        window.as(gobject.Object).weakRef(
            appliedWindowWeakNotify,
            state.window_lifetime,
        );
        return state;
    }

    fn initBorrowed(
        allocator: std.mem.Allocator,
        app: *Application,
        window: *Window,
        snapshot: renderpkg.Snapshot,
    ) !AppliedRenderState {
        var arena = std.heap.ArenaAllocator.init(allocator);
        errdefer arena.deinit();
        const alloc = arena.allocator();
        const state: AppliedRenderState = .{
            .arena = arena,
            .app = app,
            .workspace_id = try alloc.dupe(u8, snapshot.workspace_id),
            .workspace_revision = snapshot.workspace_revision,
            .snapshot_fingerprint = try alloc.dupe(
                u8,
                snapshot.snapshot_fingerprint,
            ),
            .window = window,
            .window_lifetime = try alloc.create(WindowLifetime),
            .owns_window = false,
            .baseline_tab_count = @intCast(window.getTabView().getNPages()),
            .window_id = try physicalId(alloc, "window", window),
        };
        state.window_lifetime.* = .{};
        window.as(gobject.Object).weakRef(
            appliedWindowWeakNotify,
            state.window_lifetime,
        );
        return state;
    }

    fn deinit(self: *AppliedRenderState) void {
        if (self.window_lifetime.alive) {
            self.window.as(gobject.Object).weakUnref(
                appliedWindowWeakNotify,
                self.window_lifetime,
            );
            self.window_lifetime.alive = false;
        }
        for (self.panes.items) |pane| {
            pane.release();
        }
        for (self.tabs.items) |tab| {
            if (tab.placeholder_surface) |surface| surface.unref();
            tab.tab.as(gobject.Object).unref();
        }
        self.arena.deinit();
        self.* = undefined;
    }

    fn destroyWindow(self: *AppliedRenderState) void {
        if (self.owns_window and self.windowExists()) {
            self.window.as(gtk.Window).destroy();
        }
    }

    fn cleanupFailedInitialRender(
        self: *AppliedRenderState,
        lifecycle: *content_lifecycle.Lifecycle,
    ) void {
        if (self.owns_window) {
            self.retireAllPaneContent(lifecycle);
            self.destroyWindow();
        } else {
            self.cleanupFailedBorrowedRender(lifecycle) catch {};
        }
    }

    fn present(self: *AppliedRenderState) void {
        if (self.window.as(gtk.Window).isMaximized() == 0 and
            self.window.as(gtk.Window).isFullscreen() == 0)
        {
            if (self.window.getActiveSurface()) |surface| {
                surface.estimateInitialSize();
                if (surface.getDefaultSize()) |size| {
                    self.window.as(gtk.Window).setDefaultSize(
                        @intCast(size.width),
                        @intCast(size.height),
                    );
                }
            }
        }
        gtk.Window.present(self.window.as(gtk.Window));
    }

    fn buildInitial(
        self: *AppliedRenderState,
        snapshot: renderpkg.Snapshot,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        for (snapshot.tabs, 0..) |tab, index| {
            if (index == 0 and !self.owns_window and self.baseline_tab_count > 0) {
                try self.bindManagedTab(tab);
            } else {
                try self.createTab(tab, lifecycle);
            }
            // Finish this Tab while Ghostty still has its page selected.
            // Creating a later Tab changes the selected page, and splitting
            // the borrowed startup Tab only afterwards can leave that Tab
            // partially materialized.
            for (tab.panes) |pane| {
                _ = try self.ensurePane(tab.id, pane, lifecycle);
            }
            try self.applyLayout(tab.id, tab);
        }
        try self.finishSnapshot(snapshot, .force);
        try self.focusTab(snapshot.selected_tab_id, snapshot.tab(snapshot.selected_tab_id).?.focused_pane_id);
    }

    fn reconcile(
        self: *AppliedRenderState,
        snapshot: renderpkg.Snapshot,
        selection_policy: SelectionPolicy,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        if (std.mem.eql(
            u8,
            self.snapshot_fingerprint,
            snapshot.snapshot_fingerprint,
        ) and self.workspace_revision == snapshot.workspace_revision) {
            const physically_current = blk: {
                self.verifyDesired(snapshot) catch break :blk false;
                self.verify() catch break :blk false;
                break :blk true;
            };
            if (physically_current) {
                try self.finishSnapshot(snapshot, selection_policy);
                return;
            }
            // A native Tab/Surface can disappear without a Layout change.
            // Reconcile against saved intent rather than its fingerprint.
        }

        for (snapshot.tabs) |tab| try self.ensureTab(tab, lifecycle);
        try self.moveRetainedPaneSurface(snapshot, lifecycle);
        for (snapshot.tabs) |tab| {
            var bindings_changed = false;
            for (tab.panes) |pane| {
                if (try self.ensurePane(tab.id, pane, lifecycle)) {
                    bindings_changed = true;
                }
            }
            const existing = self.findTab(tab.id) orelse return error.UnknownTab;
            const desired_layout = try desiredNativeLayout(tab);
            const layout_changed = !nativeLayoutsEqual(
                existing.layout,
                desired_layout,
            );
            if (bindings_changed or layout_changed or
                !self.physicalTreeMatches(tab))
            {
                try self.applyLayout(tab.id, tab);
            }
        }
        self.markTabsOmitted(snapshot);
        self.markPanesOmitted(snapshot);
        try self.finishSnapshot(snapshot, selection_policy);
        self.retireStaleTabs(snapshot, lifecycle);
        self.retireInactivePanes(lifecycle);
        try self.verifyDesired(snapshot);
        try self.verify();
    }

    /// A runtime intent moves at most one Pane between Tabs. Linux can detach
    /// that Pane's existing Surface from the source tree and install the same
    /// pointer in the destination tree; terminal content is rebound only when
    /// the target or tmux Session also changed.
    fn moveRetainedPaneSurface(
        self: *AppliedRenderState,
        snapshot: renderpkg.Snapshot,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        var moved_index: ?usize = null;
        var target_tab_id: []const u8 = undefined;
        var desired_pane: renderpkg.Pane = undefined;
        for (snapshot.tabs) |tab| {
            _ = try desiredNativeLayout(tab);
            for (tab.panes) |pane| {
                const index = self.findActivePaneIndex(pane.id) orelse continue;
                if (std.mem.eql(u8, self.panes.items[index].tab_id, tab.id)) {
                    continue;
                }
                if (moved_index != null) return error.BatchRuntimeMutation;
                moved_index = index;
                target_tab_id = tab.id;
                desired_pane = pane;
            }
        }
        const index = moved_index orelse return;
        const source_tab_id = self.panes.items[index].tab_id;
        const source_desired = snapshot.tab(source_tab_id);
        const target_desired = snapshot.tab(target_tab_id) orelse
            return error.UnknownTab;
        try self.preflightMoveLayout(
            source_tab_id,
            source_desired,
            self.panes.items[index].pane_id,
        );
        try self.preflightMoveLayout(
            target_tab_id,
            target_desired,
            self.panes.items[index].pane_id,
        );
        _ = self.findTab(target_tab_id) orelse return error.UnknownTab;
        const mutation = native_surface_transition.planContentMutation(
            source_tab_id,
            self.panes.items[index].attach_fingerprint,
            target_tab_id,
            desired_pane.attach_fingerprint,
        );
        if (mutation == .move_and_rebind) {
            // A semantic identity switch must retire the old physical surface;
            // ordinary same-identity cross-tab moves retain it below.
            if (self.panes.items[index].display() != null or desired_pane.content != null)
                return error.ContentIdentityRequiresNewSurface;
            try self.rebindPaneContent(
                &self.panes.items[index],
                desired_pane,
                lifecycle,
            );
        }

        if (source_desired) |desired_source| {
            try self.applyLayout(source_tab_id, desired_source.*);
        } else {
            const source = self.findTab(source_tab_id) orelse
                return error.UnknownTab;
            source.tab.getSplitTree().setTree(null);
            source.layout = null;
            for (self.panes.items) |*pane| {
                if (pane.tab == source.tab) pane.active = false;
            }
        }

        const target = self.findTab(target_tab_id) orelse return error.UnknownTab;
        self.panes.items[index].tab_id = try self.arena.allocator().dupe(
            u8,
            target_tab_id,
        );
        self.panes.items[index].tab = target.tab;
        self.panes.items[index].active = true;
        // Complete the physical move before ensurePane checks destination
        // membership. The retained Surface is temporarily in neither tree.
        try self.applyLayout(target_tab_id, target_desired.*);
    }

    fn preflightMoveLayout(
        self: *AppliedRenderState,
        tab_id: []const u8,
        desired: ?*const renderpkg.Tab,
        moved_pane_id: []const u8,
    ) !void {
        const tab = desired orelse return;
        const layout = try desiredNativeLayout(tab.*) orelse return;
        for (layout.visualPaneIds()) |pane_id| {
            const pane = self.findPaneIncludingInactive(pane_id) orelse
                return error.UnknownPane;
            if (!std.mem.eql(u8, pane.tab_id, tab_id) and
                !std.mem.eql(u8, pane_id, moved_pane_id))
            {
                return error.InvalidLayout;
            }
        }
    }

    fn ensureTab(
        self: *AppliedRenderState,
        desired: renderpkg.Tab,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        switch (workspace_reconciliation.tabBindingDecision(
            self.tabs.items,
            desired.id,
        )) {
            .reuse_active => |index| {
                if (!self.tabExists(self.tabs.items[index].tab)) {
                    try self.restoreClosedTab(index, desired, lifecycle);
                    return;
                }
                if (!std.mem.eql(
                    u8,
                    self.tabs.items[index].title,
                    desired.title,
                )) {
                    try self.renameTab(desired.id, desired.title);
                }
            },
            .reactivate => |index| {
                if (!self.tabExists(self.tabs.items[index].tab)) {
                    try self.restoreClosedTab(index, desired, lifecycle);
                    return;
                }
                self.deactivateTabID(desired.id);
                self.tabs.items[index].active = true;
                if (!std.mem.eql(
                    u8,
                    self.tabs.items[index].title,
                    desired.title,
                )) {
                    try self.renameTab(desired.id, desired.title);
                }
            },
            .create => try self.createTab(desired, lifecycle),
        }
    }

    /// Ghostty closes a Tab when its final Surface exits. The logical Tab ID
    /// stays in the Workspace, but neither its Tab nor its Pane channels may
    /// be reused after the native page has disappeared.
    fn restoreClosedTab(
        self: *AppliedRenderState,
        index: usize,
        desired: renderpkg.Tab,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        const closed = self.tabs.items[index];
        self.removePaneBindingsForTab(closed.tab, false, false, lifecycle);
        if (closed.placeholder_surface) |surface| surface.unref();
        closed.tab.as(gobject.Object).unref();
        _ = self.tabs.orderedRemove(index);
        try self.createTab(desired, lifecycle);
    }

    fn finishSnapshot(
        self: *AppliedRenderState,
        snapshot: renderpkg.Snapshot,
        selection_policy: SelectionPolicy,
    ) !void {
        const order = try self.arena.allocator().alloc(
            []const u8,
            snapshot.tabs.len,
        );
        for (snapshot.tabs, 0..) |tab, index| order[index] = tab.id;
        try self.reorderTabs(order);
        var selected_tab_id = snapshot.selected_tab_id;
        var should_select = selection_policy == .force;
        if (!should_select) {
            if (self.activeNativeTabId()) |native_tab_id| {
                if (snapshot.tab(native_tab_id) != null) {
                    selected_tab_id = native_tab_id;
                } else {
                    should_select = true;
                }
            } else {
                should_select = true;
            }
        }
        if (should_select) try self.selectTab(selected_tab_id);
        const selected = snapshot.tab(selected_tab_id) orelse
            return error.UnknownTab;
        _ = selected;
        try self.synchronizeManagedFontSize(snapshot.font_size_points);
        self.workspace_id = try self.arena.allocator().dupe(
            u8,
            snapshot.workspace_id,
        );
        self.workspace_revision = snapshot.workspace_revision;
        self.snapshot_fingerprint = try self.arena.allocator().dupe(
            u8,
            snapshot.snapshot_fingerprint,
        );
    }

    fn createTab(
        self: *AppliedRenderState,
        desired: renderpkg.Tab,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        if (self.findTab(desired.id)) |existing| existing.active = false;
        const first_pane: ?*const renderpkg.Pane =
            if (desired.panes.len == 0) null else &desired.panes[0];
        var display: ?*DisplaySurface = if (first_pane) |pane|
            if (pane.content != null) try newDisplay(self.arena.child_allocator, pane.content.?) else null
        else
            null;
        errdefer if (display) |value| value.destroy() catch {};
        var content_launch: ?ContentHostLaunch = if (first_pane) |pane|
            if (pane.isTerminal()) try self.contentHostLaunch(pane.*, lifecycle) else null
        else
            null;
        if (content_launch) |launch| @import("ipc_client.zig").trace(17, launch.channel_id, launch.channel_generation);
        const title = try self.arena.allocator().dupeZ(u8, desired.title);
        self.window.newTabForWindow(null, .{
            .station_surface = if (display) |value| value.surface else null,
            .command = if (content_launch) |*launch|
                launch.command.command()
            else
                null,
            .title = title,
        });
        const tab_view = self.window.getTabView();
        const page = tab_view.getSelectedPage() orelse return error.TabCreationFailed;
        const tab = gobject.ext.cast(Tab, page.getChild()) orelse
            return error.TabCreationFailed;
        tab.setTitleOverride(title);
        _ = tab.as(gobject.Object).ref();
        var references_transferred = false;
        errdefer if (!references_transferred) tab.as(gobject.Object).unref();
        const placeholder_surface = if (first_pane == null)
            tab.getActiveSurface() orelse return error.SurfaceCreationFailed
        else
            null;
        if (placeholder_surface) |surface| _ = surface.ref();
        errdefer if (!references_transferred) {
            if (placeholder_surface) |surface| surface.unref();
        };
        try self.tabs.append(self.arena.allocator(), .{
            .tab_id = try self.arena.allocator().dupe(u8, desired.id),
            .title = try self.arena.allocator().dupe(u8, desired.title),
            .physical_tab_id = try physicalId(self.arena.allocator(), "tab", tab),
            .tab = tab,
            .owns_tab = true,
            .placeholder_surface = placeholder_surface,
        });
        references_transferred = true;

        if (first_pane) |pane| {
            if (display) |value| {
                try self.addDisplayPane(desired.id, pane.*, value);
                display = null; // PaneBinding now owns the controller.
            } else {
                const surface = tab.getActiveSurface() orelse return error.SurfaceCreationFailed;
                try self.addPane(desired.id, pane.*, surface, content_launch.?, lifecycle);
                @import("ipc_client.zig").trace(18, content_launch.?.channel_id, content_launch.?.channel_generation);
            }
            for (desired.panes[1..]) |extra| {
                try self.createSurface(desired.id, extra, lifecycle);
            }
            // Semantic Surfaces are created detached. Install the complete
            // tree before buildInitial/reconcile checks physical membership.
            try self.applyLayout(desired.id, desired);
        }
    }

    /// Binds the already open startup Tab instead of manufacturing a second
    /// Tab beside it. The existing shell Surface remains as an adapter-owned
    /// placeholder until the first logical Pane tree is installed.
    fn bindManagedTab(
        self: *AppliedRenderState,
        desired: renderpkg.Tab,
    ) !void {
        if (self.owns_window or self.findTab(desired.id) != null) {
            return error.InvalidManagedTabBinding;
        }
        const tab_view = self.window.getTabView();
        const page = tab_view.getSelectedPage() orelse
            return error.ManagedTabUnavailable;
        const tab = gobject.ext.cast(Tab, page.getChild()) orelse
            return error.ManagedTabUnavailable;
        if (self.findTabByPointer(tab) != null) return error.ManagedTabAlreadyBound;
        const placeholder = tab.getActiveSurface() orelse
            return error.ManagedTabHasNoSurface;
        if (self.baseline_tab_count == 0) return error.ManagedTabUnavailable;

        _ = tab.as(gobject.Object).ref();
        var references_transferred = false;
        errdefer if (!references_transferred) tab.as(gobject.Object).unref();
        _ = placeholder.ref();
        errdefer if (!references_transferred) placeholder.unref();
        const title = try self.arena.allocator().dupeZ(u8, desired.title);
        tab.setTitleOverride(title);
        try self.tabs.append(self.arena.allocator(), .{
            .tab_id = try self.arena.allocator().dupe(u8, desired.id),
            .title = title,
            .physical_tab_id = try physicalId(self.arena.allocator(), "tab", tab),
            .tab = tab,
            .owns_tab = false,
            .placeholder_surface = placeholder,
        });
        references_transferred = true;
        self.baseline_tab_count -= 1;
    }

    fn addPane(
        self: *AppliedRenderState,
        tab_id: []const u8,
        desired: renderpkg.Pane,
        surface: *Surface,
        content_launch: ContentHostLaunch,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        if (self.findPane(desired.id)) |existing| existing.active = false;
        const tab = self.findTab(tab_id) orelse return error.UnknownTab;
        _ = surface.ref();
        errdefer surface.unref();
        const focus_listener = try terminal_focus.Focus.create(self.arena.allocator(), lifecycle, surface, content_launch.control_socket, content_launch.channel_id, content_launch.channel_generation);
        errdefer focus_listener.detach();
        try self.panes.append(self.arena.allocator(), .{
            .pane_id = try self.arena.allocator().dupe(u8, desired.id),
            .physical_surface_id = try physicalId(
                self.arena.allocator(),
                "surface",
                surface,
            ),
            .tab_id = try self.arena.allocator().dupe(u8, tab_id),
            .tab = tab.tab,
            .attach_fingerprint = try self.arena.allocator().dupe(
                u8,
                desired.attach_fingerprint,
            ),
            .surface = surface,
            .content_state = .{ .terminal = .{
                .execution_target_id = try self.arena.allocator().dupe(
                    u8,
                    desired.execution_target_id,
                ),
                .tmux_session_id = try self.arena.allocator().dupe(
                    u8,
                    desired.tmux_session_id,
                ),

                .content_channel_id = content_launch.channel_id,
                .content_channel_generation = content_launch.channel_generation,
                .content_revision = 1,
                .content_control_socket = content_launch.control_socket,

                .focus_listener = focus_listener,
            } },
        });
    }

    fn ensurePane(
        self: *AppliedRenderState,
        tab_id: []const u8,
        desired: renderpkg.Pane,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !bool {
        if (self.findPane(desired.id)) |existing| {
            if (!self.tabExists(existing.tab) or
                !self.surfaceExists(existing.tab, existing.surface))
            {
                self.deactivatePaneID(desired.id);
                try self.createSurface(tab_id, desired, lifecycle);
                return true;
            }
            switch (native_surface_transition.planContentMutation(
                existing.tab_id,
                existing.attach_fingerprint,
                tab_id,
                desired.attach_fingerprint,
            )) {
                .reuse_surface => return false,
                .rebind_content => {
                    if (existing.display() != null or desired.content != null) {
                        self.deactivatePaneID(desired.id);
                        try self.createSurface(tab_id, desired, lifecycle);
                        return true;
                    }
                    self.rebindPaneContent(existing, desired, lifecycle) catch |bind_error| {
                        if (bind_error != error.FileNotFound and
                            bind_error != error.ConnectionRefused)
                            return bind_error;
                        self.deactivatePaneID(desired.id);
                        try self.createSurface(tab_id, desired, lifecycle);
                        return true;
                    };
                    return false;
                },
                .move_surface, .move_and_rebind => {},
            }
        }
        switch (workspace_reconciliation.paneBindingDecision(
            self.panes.items,
            desired.id,
            tab_id,
            desired.attach_fingerprint,
        )) {
            .reuse_active => {
                return false;
            },
            .reactivate => |index| {
                const resident = self.panes.items[index];
                if (!self.tabExists(resident.tab) or !self.surfaceExists(resident.tab, resident.surface)) {
                    self.deactivatePaneID(desired.id);
                    try self.createSurface(tab_id, desired, lifecycle);
                    return true;
                }
                self.deactivatePaneID(desired.id);
                self.panes.items[index].active = true;
                return true;
            },
            .create => {
                self.deactivatePaneID(desired.id);
                try self.createSurface(tab_id, desired, lifecycle);
                return true;
            },
        }
    }

    fn rebindPaneContent(
        self: *AppliedRenderState,
        existing: *PaneBinding,
        desired: renderpkg.Pane,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        if (existing.display() != null or desired.content != null) return error.NotTerminalPane;
        const allocator = self.arena.child_allocator;
        const revision = existing.content_state.terminal.content_revision + 1;
        const request_id = try lifecycle.allocateRequestId(allocator);
        defer allocator.free(request_id);
        const request = try pane_content.encodeBind(
            allocator,
            .{
                .request_id = request_id,
                .channel_id = existing.content_state.terminal.content_channel_id,
                .channel_generation = existing.content_state.terminal.content_channel_generation,
                .content_revision = revision,
            },
            .{
                .execution_target = desired.execution_target_id,
                .tmux_session_id = desired.tmux_session_id,
            },
        );
        defer allocator.free(request);
        const target = try self.arena.allocator().dupe(u8, desired.execution_target_id);
        const session = try self.arena.allocator().dupe(u8, desired.tmux_session_id);
        const fingerprint = try self.arena.allocator().dupe(u8, desired.attach_fingerprint);
        const pending = try self.arena.allocator().dupe(u8, request_id);
        try lifecycle.submit(existing.content_state.terminal.content_control_socket, request);
        existing.content_state.terminal.execution_target_id = target;
        existing.content_state.terminal.tmux_session_id = session;
        existing.attach_fingerprint = fingerprint;
        existing.content_state.terminal.content_revision = revision;
        existing.content_state.terminal.content_pending_request_id = pending;
        existing.content_state.terminal.content_error = null;
    }

    fn reconnectSavedPaneContent(
        self: *AppliedRenderState,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        const allocator = self.arena.child_allocator;
        for (self.panes.items) |*pane| {
            if (!pane.active or pane.display() != null) continue;
            const revision = pane.content_state.terminal.content_revision + 1;
            const request_id = try lifecycle.allocateRequestId(allocator);
            defer allocator.free(request_id);
            const request = try pane_content.encodeReconnect(
                allocator,
                .{
                    .request_id = request_id,
                    .channel_id = pane.content_state.terminal.content_channel_id,
                    .channel_generation = pane.content_state.terminal.content_channel_generation,
                    .content_revision = revision,
                },
                .{
                    .execution_target = pane.content_state.terminal.execution_target_id,
                    .tmux_session_id = pane.content_state.terminal.tmux_session_id,
                },
            );
            defer allocator.free(request);
            try lifecycle.submit(pane.content_state.terminal.content_control_socket, request);
            pane.content_state.terminal.content_revision = revision;
            pane.content_state.terminal.content_pending_request_id = try self.arena.allocator().dupe(u8, request_id);
            pane.content_state.terminal.content_error = null;
        }
    }

    fn createSurface(
        self: *AppliedRenderState,
        tab_id: []const u8,
        desired: renderpkg.Pane,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        if (desired.content != null) {
            try self.createDisplayPane(tab_id, desired);
            return;
        }
        const tab = self.findTab(tab_id) orelse return error.UnknownTab;
        var content_launch = try self.contentHostLaunch(desired, lifecycle);
        @import("ipc_client.zig").trace(17, content_launch.channel_id, content_launch.channel_generation);
        defer @import("ipc_client.zig").trace(18, content_launch.channel_id, content_launch.channel_generation);
        const anchor = tab.tab.getActiveSurface();
        var prior_storage: [10]*Surface = undefined;
        const prior_surfaces = try captureSurfaceSet(
            tab.tab,
            &prior_storage,
        );
        try tab.tab.getSplitTree().newSplit(
            .right,
            anchor,
            .{ .command = content_launch.command.command() },
        );
        const surface = findAddedSurface(tab.tab, prior_surfaces) orelse
            return error.SurfaceCreationFailed;
        self.addPane(tab_id, desired, surface, content_launch, lifecycle) catch |err| {
            surface.close();
            return err;
        };
    }

    fn createDisplayPane(self: *AppliedRenderState, tab_id: []const u8, desired: renderpkg.Pane) !void {
        const display = try newDisplay(self.arena.child_allocator, desired.content.?);
        errdefer display.destroy() catch {};
        // The already-existing native layout will place this Surface into its
        // tree. Do not create a terminal split merely to obtain a GTK slot.
        try self.addDisplayPane(tab_id, desired, display);
    }

    fn addDisplayPane(self: *AppliedRenderState, tab_id: []const u8, desired: renderpkg.Pane, display: *DisplaySurface) !void {
        const content = desired.content orelse return error.InvalidContentIdentity;
        const tab = self.findTab(tab_id) orelse return error.UnknownTab;
        const allocator = self.arena.allocator();
        const id = try allocator.dupe(u8, desired.id);
        const saved_content = try content.clone(allocator);
        _ = display.surface.ref();
        errdefer display.surface.unref();
        try self.panes.append(allocator, .{
            .pane_id = id,
            .physical_surface_id = try physicalId(allocator, "display", display.surface),
            .tab_id = try allocator.dupe(u8, tab_id),
            .tab = tab.tab,
            .attach_fingerprint = try allocator.dupe(u8, desired.attach_fingerprint),
            .surface = display.surface,
            .content_state = .{ .display = .{ .controller = display, .identity = saved_content } },
        });
        self.hooks.created(id, self.workspace_id, self.window, saved_content, display);
    }

    fn contentHostLaunch(
        self: *AppliedRenderState,
        desired: renderpkg.Pane,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !ContentHostLaunch {
        if (desired.content != null) return error.NotTerminalPane;
        const allocator = self.arena.allocator();
        const runtime_dir = try std.process.getEnvVarOwned(
            allocator,
            "XDG_RUNTIME_DIR",
        );
        const channel = try lifecycle.allocateChannel(allocator, runtime_dir);
        const host_socket = if (std.process.getEnvVarOwned(
            allocator,
            "POLAR_TMUX_IPC_SOCKET",
        )) |path|
            path
        else |error_value| switch (error_value) {
            error.EnvironmentVariableNotFound => try std.fs.path.join(
                allocator,
                &.{ runtime_dir, "polar-tmux", "host.sock" },
            ),
            else => return error_value,
        };
        const executable = if (std.process.getEnvVarOwned(
            allocator,
            "POLAR_TMUX_EXECUTABLE",
        )) |path|
            path
        else |error_value| switch (error_value) {
            error.EnvironmentVariableNotFound => "polar-tmux",
            else => return error_value,
        };
        const args = [_][]const u8{
            "pane-host",
            "--channel-id",
            channel.channel_id,
            "--channel-generation",
            "1",
            "--control-socket",
            channel.control_socket,
            "--host-socket",
            host_socket,
            "--initial-execution-target",
            desired.execution_target_id,
            "--initial-session",
            desired.tmux_session_id,
        };
        return .{
            .channel_id = channel.channel_id,
            .channel_generation = channel.channel_generation,
            .control_socket = channel.control_socket,
            .command = try DirectCommand.init(
                allocator,
                executable,
                &args,
            ),
        };
    }

    fn cleanupFailedBorrowedRender(
        self: *AppliedRenderState,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        while (self.activeTabCount() > 0) {
            const tab = for (self.tabs.items) |*candidate| {
                if (candidate.active) break candidate;
            } else return error.UnknownTab;
            if (tab.owns_tab) {
                try self.closeLogicalTab(tab.tab_id, lifecycle);
            } else {
                try self.releaseBoundManagedTab(tab.tab_id, lifecycle);
            }
        }
    }

    fn releaseBoundManagedTab(
        self: *AppliedRenderState,
        tab_id: []const u8,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        const tab = self.findTab(tab_id) orelse return error.UnknownTab;
        if (tab.owns_tab) return error.ManagedTabOwnershipMismatch;
        const placeholder = try self.ensurePlaceholder(tab_id);
        var tree = try Surface.Tree.init(self.arena.child_allocator, placeholder);
        defer tree.deinit();
        tab.tab.getSplitTree().setTree(&tree);
        tab.tab.setTitleOverride(null);
        tab.layout = null;
        tab.active = false;
        self.removePaneBindingsForTab(tab.tab, true, false, lifecycle);
        self.baseline_tab_count += 1;
    }

    fn applyLayout(
        self: *AppliedRenderState,
        tab_id: []const u8,
        desired: renderpkg.Tab,
    ) !void {
        const tab = self.findTab(tab_id) orelse return error.UnknownTab;
        const source_layout = try desiredNativeLayout(desired);
        if (source_layout == null) {
            const placeholder = try self.ensurePlaceholder(tab_id);
            for (self.panes.items) |*pane| {
                if (pane.tab == tab.tab) pane.active = false;
            }
            var tree = try Surface.Tree.init(self.arena.child_allocator, placeholder);
            defer tree.deinit();
            tab.tab.getSplitTree().setTree(&tree);
            tab.layout = null;
            return;
        }
        const layout = try cloneNativeLayout(
            self.arena.allocator(),
            source_layout.?,
        );
        for (self.panes.items) |*pane| {
            if (pane.tab == tab.tab) pane.active = false;
        }
        for (layout.visualPaneIds()) |pane_id| {
            const pane = self.findPaneIncludingInactive(pane_id) orelse
                return error.UnknownPane;
            if (!std.mem.eql(u8, pane.tab_id, tab_id)) return error.InvalidLayout;
            pane.active = true;
        }
        var tree = try self.buildTree(layout);
        defer tree.deinit();
        tab.tab.getSplitTree().setTree(&tree);
        if (tab.placeholder_surface) |placeholder| {
            placeholder.unref();
            tab.placeholder_surface = null;
        }
        tab.layout = layout;
    }

    fn ensurePlaceholder(
        self: *AppliedRenderState,
        tab_id: []const u8,
    ) !*Surface {
        const tab = self.findTab(tab_id) orelse return error.UnknownTab;
        if (tab.placeholder_surface) |placeholder| {
            if (self.surfaceExists(tab.tab, placeholder)) return placeholder;
            placeholder.unref();
            tab.placeholder_surface = null;
        }

        const anchor = tab.tab.getActiveSurface();
        var prior_storage: [10]*Surface = undefined;
        const prior_surfaces = try captureSurfaceSet(
            tab.tab,
            &prior_storage,
        );
        try tab.tab.getSplitTree().newSplit(.right, anchor, .{});
        const placeholder = findAddedSurface(tab.tab, prior_surfaces) orelse
            return error.SurfaceCreationFailed;
        _ = placeholder.ref();
        tab.placeholder_surface = placeholder;
        return placeholder;
    }

    fn buildTree(
        self: *AppliedRenderState,
        layout: native_layout.Layout,
    ) !Surface.Tree {
        // Ghostty's setTree clones with the source tree's child allocator.
        // That allocator must outlive this AppliedRenderState because GTK
        // object finalization can run after Window.destroy returns.
        const tree_alloc = self.arena.child_allocator;
        return self.buildTreeNode(layout.tree, layout.tree.root, tree_alloc);
    }

    fn buildTreeNode(
        self: *AppliedRenderState,
        tree: native_layout.Tree,
        node_index: u8,
        allocator: std.mem.Allocator,
    ) !Surface.Tree {
        return switch (tree.nodes[node_index]) {
            .surface => |pane_id| blk: {
                const pane = self.findPane(pane_id) orelse return error.UnknownPane;
                break :blk Surface.Tree.init(allocator, pane.surface);
            },
            .split => |split| blk: {
                var first = try self.buildTreeNode(tree, split.first, allocator);
                defer first.deinit();
                var second = try self.buildTreeNode(tree, split.second, allocator);
                defer second.deinit();
                break :blk first.split(
                    allocator,
                    .root,
                    if (split.axis == .horizontal) .right else .down,
                    @floatCast(ratioValue(split.ratio)),
                    &second,
                );
            },
        };
    }

    fn selectTab(self: *AppliedRenderState, tab_id: ?[]const u8) !void {
        const id = tab_id orelse return;
        const tab = self.findTab(id) orelse return error.UnknownTab;
        const page = self.window.getTabView().getPage(tab.tab.as(gtk.Widget));
        self.window.getTabView().setSelectedPage(page);
    }

    fn focusTab(self: *AppliedRenderState, tab_id: []const u8, pane_id: ?[]const u8) !void {
        const tab = self.findTab(tab_id) orelse return error.UnknownTab;
        const surface = if (pane_id) |id| blk: {
            const pane = self.findPane(id) orelse return error.UnknownPane;
            if (pane.tab != tab.tab) return error.InvalidLayout;
            break :blk pane.surface;
        } else tab.tab.getActiveSurface() orelse return;
        @import("ipc_client.zig").trace(3, pane_id orelse "", 0);
        _ = tab.tab.getSplitTree().focusSurface(surface);
        if (surface.getFocused()) @import("ipc_client.zig").trace(4, pane_id orelse "", 0);
    }

    fn reorderTabs(self: *AppliedRenderState, order: []const []const u8) !void {
        if (order.len != self.activeTabCount()) return error.InvalidTabOrder;
        for (order, 0..) |tab_id, index| {
            const tab = self.findTab(tab_id) orelse return error.UnknownTab;
            const tab_view = self.window.getTabView();
            const page = tab_view.getPage(tab.tab.as(gtk.Widget));
            const target_position: c_int = @intCast(
                self.baseline_tab_count + index,
            );
            if (tab_view.getPagePosition(page) == target_position) continue;
            if (tab_view.reorderPage(page, target_position) == 0) {
                return error.TabReorderFailed;
            }
        }
    }

    fn renameTab(self: *AppliedRenderState, tab_id: []const u8, title: []const u8) !void {
        const tab = self.findTab(tab_id) orelse return error.UnknownTab;
        const title_z = try self.arena.allocator().dupeZ(u8, title);
        tab.tab.setTitleOverride(title_z);
        tab.title = title_z;
    }

    fn synchronizeManagedFontSize(
        self: *AppliedRenderState,
        points: u8,
    ) !void {
        var surface_count: usize = 0;
        var pre_realization_count: usize = 0;
        for (self.tabs.items) |tab| {
            if (!tab.active) continue;
            const tree = tab.tab.getSurfaceTree() orelse continue;
            var iterator = tree.iterator();
            while (iterator.next()) |entry| {
                surface_count += 1;
                if (try entry.view.synchronizeManagedFontSize(
                    @floatFromInt(points),
                ) == .pending_realization) {
                    pre_realization_count += 1;
                }
            }
        }
        if (surface_count == 0) return error.ManagedSurfaceUnavailable;
        std.log.info(
            "Polar Station managed font synchronized: points={d} surfaces={d} pre_realization={d}",
            .{ points, surface_count, pre_realization_count },
        );
    }

    fn retireAllPaneContent(
        self: *AppliedRenderState,
        lifecycle: *content_lifecycle.Lifecycle,
    ) void {
        for (self.panes.items) |pane| {
            self.queuePaneRetirement(lifecycle, pane);
        }
    }

    /// Stage the exact identity before the Surface reference is closed or
    /// released. Network I/O runs later from Executor ownership, after native
    /// tree mutation has completed.
    fn queuePaneRetirement(
        self: *AppliedRenderState,
        lifecycle: *content_lifecycle.Lifecycle,
        pane: PaneBinding,
    ) void {
        if (pane.display()) |display| {
            // Physical identity is part of the notification. A delayed old
            // retirement cannot close the replacement with the same Pane ID.
            if (display.handle != null or display.graphics_widget != null) {
                self.hooks.retired(pane.pane_id, display);
                display.retire() catch {};
            }
            return;
        }
        lifecycle.submitOperation(pane.content_state.terminal.content_control_socket, pane.content_state.terminal.content_channel_id, pane.content_state.terminal.content_channel_generation, "unobserve_recovery") catch {};
        if (pane.content_state.terminal.content_revision == std.math.maxInt(u64)) {
            std.log.err(
                "Polar Station cannot retire exhausted content revision channel={s}",
                .{pane.content_state.terminal.content_channel_id},
            );
            return;
        }
        _ = lifecycle.beginRetirement(.{
            .channel_id = pane.content_state.terminal.content_channel_id,
            .channel_generation = pane.content_state.terminal.content_channel_generation,
            .content_revision = pane.content_state.terminal.content_revision + 1,
            .control_socket = pane.content_state.terminal.content_control_socket,
        }) catch |retire_error| {
            std.log.err(
                "Polar Station could not stage Pane retirement channel={s} error={s}",
                .{ pane.content_state.terminal.content_channel_id, @errorName(retire_error) },
            );
        };
    }

    fn retireStaleTabs(
        self: *AppliedRenderState,
        snapshot: renderpkg.Snapshot,
        lifecycle: *content_lifecycle.Lifecycle,
    ) void {
        var index = self.tabs.items.len;
        while (index > 0) {
            index -= 1;
            const tab = self.tabs.items[index];
            if (tab.active and snapshot.tab(tab.tab_id) != null) continue;
            if (self.tabExists(tab.tab)) {
                tab.tab.getSplitTree().setTree(null);
                // Empty-tree notification may synchronously remove the page.
                if (self.tabExists(tab.tab)) {
                    const page = self.window.getTabView().getPage(tab.tab.as(gtk.Widget));
                    self.window.getTabView().closePage(page);
                }
            }
            self.removePaneBindingsForTab(tab.tab, false, false, lifecycle);
            if (tab.placeholder_surface) |surface| surface.unref();
            tab.tab.as(gobject.Object).unref();
            _ = self.tabs.orderedRemove(index);
        }
    }

    fn markTabsOmitted(
        self: *AppliedRenderState,
        snapshot: renderpkg.Snapshot,
    ) void {
        for (self.tabs.items) |*tab| {
            if (snapshot.tab(tab.tab_id) == null) tab.active = false;
        }
    }

    fn markPanesOmitted(
        self: *AppliedRenderState,
        snapshot: renderpkg.Snapshot,
    ) void {
        for (self.panes.items) |*pane| {
            const desired = snapshot.pane(pane.pane_id) orelse {
                pane.active = false;
                continue;
            };
            if (!std.mem.eql(u8, desired.tab.id, pane.tab_id) or
                !std.mem.eql(
                    u8,
                    desired.pane.attach_fingerprint,
                    pane.attach_fingerprint,
                ))
            {
                pane.active = false;
            }
        }
    }

    fn retireInactivePanes(
        self: *AppliedRenderState,
        lifecycle: *content_lifecycle.Lifecycle,
    ) void {
        var index = self.panes.items.len;
        while (index > 0) {
            index -= 1;
            const pane = self.panes.items[index];
            if (pane.active) continue;
            self.queuePaneRetirement(lifecycle, pane);
            if (self.surfaceExists(pane.tab, pane.surface)) {
                pane.surface.close();
            }
            pane.release();
            _ = self.panes.orderedRemove(index);
        }
    }

    fn verify(self: *AppliedRenderState) !void {
        if (!self.windowExists()) return error.WindowMissing;
        if (self.baseline_tab_count + self.activeTabCount() !=
            @as(usize, @intCast(self.window.getTabView().getNPages())))
        {
            return error.TabSetMismatch;
        }
        for (self.tabs.items) |tab| {
            if (!self.tabExists(tab.tab)) return error.TabMissing;
            if (!tab.active) continue;
            const layout = tab.layout;
            var tree_count: usize = 0;
            if (tab.tab.getSurfaceTree()) |tree| {
                var iterator = tree.iterator();
                while (iterator.next()) |_| tree_count += 1;
            }
            var binding_count: usize = 0;
            for (self.panes.items) |pane| {
                if (pane.active and pane.tab == tab.tab) {
                    binding_count += 1;
                }
            }
            const expected_binding_count = if (layout) |native|
                native.pane_count
            else
                0;
            const placeholder_count: usize =
                if (layout == null and tab.placeholder_surface != null) 1 else 0;
            if (tree_count != binding_count + placeholder_count or
                binding_count != expected_binding_count)
            {
                return error.PaneSetMismatch;
            }
        }
    }

    fn verifyDesired(
        self: *AppliedRenderState,
        snapshot: renderpkg.Snapshot,
    ) !void {
        if (!self.windowExists()) return error.WindowMissing;
        if (self.activeTabCount() != snapshot.tabs.len) {
            return error.TabSetMismatch;
        }
        for (snapshot.tabs) |desired| {
            const tab = self.findTab(desired.id) orelse return error.TabMissing;
            if (!self.tabExists(tab.tab)) return error.TabMissing;
            const desired_layout = try desiredNativeLayout(desired);
            if (!nativeLayoutsEqual(tab.layout, desired_layout)) {
                return error.LayoutMismatch;
            }
            var tree_count: usize = 0;
            if (tab.tab.getSurfaceTree()) |tree| {
                var iterator = tree.iterator();
                while (iterator.next()) |_| tree_count += 1;
            }
            const expected_tree_count: usize =
                if (desired.panes.len == 0) 1 else desired.panes.len;
            if (tree_count != expected_tree_count) {
                return error.PaneSetMismatch;
            }
            for (desired.panes) |pane| {
                const binding = self.findPane(pane.id) orelse
                    return error.PaneMissing;
                if (binding.tab != tab.tab or
                    !std.mem.eql(
                        u8,
                        binding.attach_fingerprint,
                        pane.attach_fingerprint,
                    ))
                {
                    return error.PaneBindingMismatch;
                }
            }
        }
    }

    fn physicalTreeMatches(self: *AppliedRenderState, desired: renderpkg.Tab) bool {
        const tab = self.findTab(desired.id) orelse return false;
        if (!self.tabExists(tab.tab)) return false;
        const tree = tab.tab.getSurfaceTree() orelse return false;
        var count: usize = 0;
        var iterator = tree.iterator();
        while (iterator.next()) |_| count += 1;
        if (count != @max(desired.panes.len, 1)) return false;
        if (desired.panes.len == 0) {
            const placeholder = tab.placeholder_surface orelse return false;
            return self.surfaceExists(tab.tab, placeholder);
        }
        for (desired.panes) |pane| {
            const binding = self.findPane(pane.id) orelse return false;
            if (binding.tab != tab.tab or !self.surfaceExists(tab.tab, binding.surface)) return false;
        }
        return true;
    }

    fn closeLogicalTab(
        self: *AppliedRenderState,
        tab_id: []const u8,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        for (self.tabs.items, 0..) |tab, index| {
            if (!tab.active or !std.mem.eql(u8, tab.tab_id, tab_id)) continue;
            tab.tab.getSplitTree().setTree(null);
            if (self.tabExists(tab.tab)) {
                const page = self.window.getTabView().getPage(tab.tab.as(gtk.Widget));
                self.window.getTabView().closePage(page);
            }
            self.removePaneBindingsForTab(tab.tab, true, true, lifecycle);
            if (tab.placeholder_surface) |surface| surface.unref();
            tab.tab.as(gobject.Object).unref();
            _ = self.tabs.orderedRemove(index);
            self.reactivateTab(tab_id);
            return;
        }
        return error.UnknownTab;
    }

    fn closeLogicalPane(
        self: *AppliedRenderState,
        pane_id: []const u8,
        lifecycle: *content_lifecycle.Lifecycle,
    ) !void {
        for (self.panes.items, 0..) |pane, index| {
            if (!pane.active or !std.mem.eql(u8, pane.pane_id, pane_id)) continue;
            self.queuePaneRetirement(lifecycle, pane);
            pane.surface.close();
            pane.release();
            _ = self.panes.orderedRemove(index);
            self.reactivatePane(pane_id);
            return;
        }
        return error.UnknownPane;
    }

    fn removePaneBindingsForTab(
        self: *AppliedRenderState,
        tab: *Tab,
        active_only: bool,
        reactivate: bool,
        lifecycle: *content_lifecycle.Lifecycle,
    ) void {
        var index = self.panes.items.len;
        while (index > 0) {
            index -= 1;
            const pane = self.panes.items[index];
            if (pane.tab != tab) continue;
            if (active_only and !pane.active) continue;
            const pane_id = pane.pane_id;
            self.queuePaneRetirement(lifecycle, pane);
            pane.release();
            _ = self.panes.orderedRemove(index);
            if (reactivate) self.reactivatePane(pane_id);
        }
    }

    fn reactivateTab(self: *AppliedRenderState, tab_id: []const u8) void {
        var index = self.tabs.items.len;
        while (index > 0) {
            index -= 1;
            if (std.mem.eql(u8, self.tabs.items[index].tab_id, tab_id)) {
                self.tabs.items[index].active = true;
                return;
            }
        }
    }

    fn reactivatePane(self: *AppliedRenderState, pane_id: []const u8) void {
        var index = self.panes.items.len;
        while (index > 0) {
            index -= 1;
            if (std.mem.eql(u8, self.panes.items[index].pane_id, pane_id)) {
                self.panes.items[index].active = true;
                return;
            }
        }
    }

    fn activeTabCount(self: *const AppliedRenderState) usize {
        var count: usize = 0;
        for (self.tabs.items) |tab| if (tab.active) {
            count += 1;
        };
        return count;
    }

    fn findTab(self: *AppliedRenderState, tab_id: []const u8) ?*TabBinding {
        for (self.tabs.items) |*tab| {
            if (tab.active and std.mem.eql(u8, tab.tab_id, tab_id)) return tab;
        }
        return null;
    }

    /// Returns the logical Tab represented by Ghostty's current selected page.
    /// Ctrl+Shift+Left/Right is a native Ghostty action and therefore does not
    /// update the Rust projection's selected_tab_id. Focus arrows must follow
    /// this native page first, falling back to the projection only while the
    /// TabView has no selected managed page yet during startup.
    fn activeNativeTabId(self: *const AppliedRenderState) ?[]const u8 {
        if (!self.windowExists()) return null;
        const page = self.window.getTabView().getSelectedPage() orelse return null;
        const selected_tab = gobject.ext.cast(Tab, page.getChild()) orelse return null;
        for (self.tabs.items) |binding| {
            if (binding.active and binding.tab == selected_tab) {
                return binding.tab_id;
            }
        }
        return null;
    }

    fn findPane(self: *AppliedRenderState, pane_id: []const u8) ?*PaneBinding {
        for (self.panes.items) |*pane| {
            if (pane.active and std.mem.eql(u8, pane.pane_id, pane_id)) return pane;
        }
        return null;
    }

    fn findActivePaneIndex(
        self: *AppliedRenderState,
        pane_id: []const u8,
    ) ?usize {
        var index = self.panes.items.len;
        while (index > 0) {
            index -= 1;
            const pane = self.panes.items[index];
            if (pane.active and std.mem.eql(u8, pane.pane_id, pane_id)) {
                return index;
            }
        }
        return null;
    }

    fn findPaneIncludingInactive(
        self: *AppliedRenderState,
        pane_id: []const u8,
    ) ?*PaneBinding {
        var index = self.panes.items.len;
        while (index > 0) {
            index -= 1;
            if (std.mem.eql(u8, self.panes.items[index].pane_id, pane_id)) {
                return &self.panes.items[index];
            }
        }
        return null;
    }

    fn deactivateTabID(
        self: *AppliedRenderState,
        tab_id: []const u8,
    ) void {
        for (self.tabs.items) |*tab| {
            if (std.mem.eql(u8, tab.tab_id, tab_id)) tab.active = false;
        }
    }

    fn deactivatePaneID(
        self: *AppliedRenderState,
        pane_id: []const u8,
    ) void {
        for (self.panes.items) |*pane| {
            if (std.mem.eql(u8, pane.pane_id, pane_id)) pane.active = false;
        }
    }

    fn windowExists(self: *const AppliedRenderState) bool {
        if (!self.window_lifetime.alive) return false;
        const list = @as(?*glib.List, self.app.as(gtk.Application).getWindows());
        var current: ?*glib.List = list;
        while (current) |node| : (current = node.f_next) {
            const data = node.f_data orelse continue;
            if (@intFromPtr(data) != @intFromPtr(self.window)) continue;
            const gtk_window = self.window.as(gtk.Window);
            return gtk_window.as(gtk.Widget).getVisible() != 0 and
                gtk_window.as(gtk.Widget).getMapped() != 0 and
                gtk_window.as(gtk.Native).getSurface() != null;
        }
        return false;
    }

    fn tabExists(self: *AppliedRenderState, expected: *Tab) bool {
        const tab_view = self.window.getTabView();
        for (0..@as(usize, @intCast(tab_view.getNPages()))) |index| {
            const page = tab_view.getNthPage(@intCast(index));
            const tab = gobject.ext.cast(Tab, page.getChild()) orelse continue;
            if (tab == expected) return true;
        }
        return false;
    }

    fn findTabByPointer(self: *AppliedRenderState, expected: *Tab) ?*TabBinding {
        for (self.tabs.items) |*tab| if (tab.tab == expected) return tab;
        return null;
    }

    fn surfaceExists(
        self: *AppliedRenderState,
        tab: *Tab,
        expected: *Surface,
    ) bool {
        _ = self;
        const tree = tab.getSurfaceTree() orelse return false;
        var iterator = tree.iterator();
        while (iterator.next()) |entry| {
            if (entry.view == expected) return true;
        }
        return false;
    }
};

const ContentHostLaunch = struct {
    channel_id: []const u8,
    channel_generation: u64,
    control_socket: []const u8,
    command: DirectCommand,
};

const DirectCommand = struct {
    argv: []const [:0]const u8,

    fn init(
        allocator: std.mem.Allocator,
        program: []const u8,
        args: []const []const u8,
    ) !DirectCommand {
        const argv = try allocator.alloc([:0]const u8, args.len + 1);
        argv[0] = try allocator.dupeZ(u8, program);
        for (args, 0..) |arg, index| argv[index + 1] = try allocator.dupeZ(u8, arg);
        return .{ .argv = argv };
    }

    fn command(self: *const DirectCommand) configpkg.Command {
        return .{ .direct = self.argv };
    }
};

fn physicalId(allocator: std.mem.Allocator, prefix: []const u8, ptr: anytype) ![]u8 {
    return std.fmt.allocPrint(allocator, "{s}-{x}", .{ prefix, @intFromPtr(ptr) });
}

fn ratioValue(ratio: native_layout.Ratio) f32 {
    return @as(f32, @floatFromInt(ratio.numerator)) /
        @as(f32, @floatFromInt(ratio.denominator));
}

fn desiredNativeLayout(tab: renderpkg.Tab) !?native_layout.Layout {
    if (tab.panes.len == 0) return null;
    var pane_ids: [9][]const u8 = undefined;
    if (tab.visual_pane_ids.len == tab.panes.len) {
        for (tab.visual_pane_ids, 0..) |pane_id, index| {
            pane_ids[index] = pane_id;
        }
    } else {
        for (tab.panes, 0..) |pane, index| pane_ids[index] = pane.id;
    }
    const mode: native_layout.Mode = switch (tab.display_mode) {
        .monitor => .monitor,
        .focus => .focus,
    };
    return @as(
        ?native_layout.Layout,
        try native_layout.layout(
            pane_ids[0..tab.panes.len],
            mode,
            if (mode == .focus) tab.focused_pane_id else null,
        ),
    );
}

fn cloneNativeLayout(
    allocator: std.mem.Allocator,
    source: native_layout.Layout,
) !native_layout.Layout {
    var pane_ids: [9][]const u8 = undefined;
    for (source.visualPaneIds(), 0..) |pane_id, index| {
        pane_ids[index] = try allocator.dupe(u8, pane_id);
    }
    return native_layout.layout(
        pane_ids[0..source.pane_count],
        source.mode,
        if (source.mode == .focus) pane_ids[0] else null,
    );
}

fn nativeLayoutsEqual(
    left: ?native_layout.Layout,
    right: ?native_layout.Layout,
) bool {
    if (left == null or right == null) return left == null and right == null;
    if (left.?.mode != right.?.mode or left.?.pane_count != right.?.pane_count) {
        return false;
    }
    for (left.?.visualPaneIds(), right.?.visualPaneIds()) |left_id, right_id| {
        if (!std.mem.eql(u8, left_id, right_id)) return false;
    }
    return std.mem.eql(
        usize,
        left.?.columnPopulations(),
        right.?.columnPopulations(),
    );
}
