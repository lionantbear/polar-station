//! Atomic XDG persistence for the Linux-owned Workspace V2 mapping.
//!
//! The persisted document contains only native Workspace, Tab, Pane, order,
//! selection, mode, and Pane-to-Session bindings. Backend lifecycle and
//! physical Ghostty identities are deliberately excluded.

const global = @import("../../../global.zig");

const std = @import("std");
const renderpkg = @import("workspace_rendering.zig");

pub const schema_version: u32 = 2;

const PersistedPane = struct {
    source: ?@import("portable_content.zig").Source = null,
    id: []const u8,
    execution_target_id: ?[]const u8 = null,
    tmux_session_id: ?[]const u8 = null,
    content: ?renderpkg.Content = null,
};

const PersistedTab = struct {
    split_ratios: []const f32 = &.{},
    id: []const u8,
    title: []const u8,
    display_mode: renderpkg.DisplayMode,
    main_pane_id: ?[]const u8 = null,
    canonical_pane_ids: []const []const u8,
    visual_pane_ids: []const []const u8,
    panes: []const PersistedPane,
};

const PersistedWorkspace = struct {
    portable_hosts: std.json.Value = .null,
    portable_source_workspace_id: ?[]const u8 = null,
    layout_map_visible: ?bool = null,
    inbox_visible: ?bool = null,
    id: []const u8,
    name: []const u8,
    revision: u64,
    selected_tab_id: []const u8,
    managed_terminal_font_size_points: u8,
    tabs: []const PersistedTab,
};

const PersistedCatalogEnvelope = struct {
    schema_version: u32 = schema_version,
    catalog_revision: u64 = 1,
    archived_workspace_ids: []const []const u8 = &.{},
    open_workspace_ids: []const []const u8 = &.{},
    selected_workspace_id: []const u8,
    workspaces: []const PersistedWorkspace,
};

const VersionEnvelope = struct {
    schema_version: u32,
};

pub const LegacySettingsScope=struct {workspace_id:[]const u8,remote:bool,pending:bool=true};

pub const Catalog = struct {
    legacy_settings:[]LegacySettingsScope=&.{},
    settings_migrated:bool=false,
    catalog_revision: u64,
    archived_workspace_ids: []const []const u8 = &.{},
    open_workspace_ids: []const []const u8 = &.{},
    selected_workspace_id: []const u8,
    workspaces: []const renderpkg.Snapshot,

    pub fn isArchived(self: Catalog, id: []const u8) bool {
        for (self.archived_workspace_ids) |archived| if (std.mem.eql(u8, archived, id)) return true;
        return false;
    }
    pub fn selected(self: Catalog) ?renderpkg.Snapshot {
        for (self.workspaces) |workspace| {
            if (std.mem.eql(
                u8,
                workspace.workspace_id,
                self.selected_workspace_id,
            )) return workspace;
        }
        return null;
    }

    fn isOpen(self: Catalog, id: []const u8) bool {
        if (self.isArchived(id)) return false;
        for (self.open_workspace_ids) |open_id| {
            if (std.mem.eql(u8, open_id, id)) return true;
        }
        return self.open_workspace_ids.len == 0 and std.mem.eql(u8, self.selected_workspace_id, id);
    }

    pub fn hasOpenSession(self: Catalog, target: []const u8, session: []const u8) bool {
        for (self.workspaces) |workspace| {
            if (self.isOpen(workspace.workspace_id) and workspace.sessionPane(target, session) != null) return true;
        }
        return false;
    }

    /// A layout written by an older version may already contain repeated
    /// bindings. Keep that layout editable while rejecting additional clients.
    pub fn rejectNewSessionDuplicates(self: Catalog, previous: Catalog, allocator: std.mem.Allocator) !void {
        var counts = try self.openSessionCounts(allocator);
        defer counts.deinit(allocator);
        var prior = try previous.openSessionCounts(allocator);
        defer prior.deinit(allocator);
        for (counts.keys(), counts.values()) |key, count| {
            if (count > 1 and count > (prior.get(key) orelse 0)) return error.SessionAlreadyOpen;
        }
    }

    const SessionKey = struct {
        target: []const u8,
        session: []const u8,
    };

    const SessionKeyContext = struct {
        pub fn hash(_: @This(), key: SessionKey) u32 {
            var hasher = std.hash.Wyhash.init(0);
            hasher.update(key.target);
            hasher.update(key.session);
            return @truncate(hasher.final());
        }

        pub fn eql(_: @This(), left: SessionKey, right: SessionKey, _: usize) bool {
            return std.mem.eql(u8, left.target, right.target) and std.mem.eql(u8, left.session, right.session);
        }
    };
    const SessionCounts = std.ArrayHashMapUnmanaged(SessionKey, usize, SessionKeyContext, true);

    fn openSessionCounts(self: Catalog, allocator: std.mem.Allocator) !SessionCounts {
        var counts: SessionCounts = .empty;
        errdefer counts.deinit(allocator);
        for (self.workspaces) |workspace| {
            if (!self.isOpen(workspace.workspace_id)) continue;
            for (workspace.tabs) |tab| for (tab.panes) |pane| {
                if (!pane.isTerminal()) continue;
                const entry = try counts.getOrPut(allocator, .{
                    .target = pane.execution_target_id,
                    .session = pane.tmux_session_id,
                });
                if (!entry.found_existing) entry.value_ptr.* = 0;
                entry.value_ptr.* += 1;
            };
        }
        return counts;
    }
};

pub const OwnedCatalog = struct {
    arena: std.heap.ArenaAllocator,
    value: Catalog,

    pub fn clone(
        allocator: std.mem.Allocator,
        source: Catalog,
    ) !OwnedCatalog {
        const encoded = try encodeCatalog(allocator, source);
        defer allocator.free(encoded);
        var result=try decodeCatalog(allocator, encoded);
        errdefer result.deinit();
        const a=result.arena.allocator();
        if(source.legacy_settings.len>0) {
            const scopes=try a.dupe(LegacySettingsScope,source.legacy_settings);
            for(scopes) |*scope| scope.workspace_id=try a.dupe(u8,scope.workspace_id);
            result.value.legacy_settings=scopes;
        }
        result.value.settings_migrated=result.value.settings_migrated or source.settings_migrated;
        return result;
    }

    pub fn deinit(self: *OwnedCatalog) void {
        self.arena.deinit();
        self.* = undefined;
    }
};

pub const OwnedSnapshot = struct {
    arena: std.heap.ArenaAllocator,
    value: renderpkg.Snapshot,

    pub fn clone(
        allocator: std.mem.Allocator,
        source: renderpkg.Snapshot,
    ) !OwnedSnapshot {
        var arena = std.heap.ArenaAllocator.init(allocator);
        errdefer arena.deinit();
        const value = try deepCloneFromDecoded(arena.allocator(), source);
        return .{
            .arena = arena,
            .value = value,
        };
    }

    pub fn deinit(self: *OwnedSnapshot) void {
        self.arena.deinit();
        self.* = undefined;
    }
};

pub const Store = struct {
    allocator: std.mem.Allocator,
    path: []u8,

    pub fn init(allocator: std.mem.Allocator) !Store {
        return .{
            .allocator = allocator,
            .path = try statePath(allocator),
        };
    }

    pub fn initAt(allocator: std.mem.Allocator, path: []const u8) !Store {
        return .{
            .allocator = allocator,
            .path = try allocator.dupe(u8, path),
        };
    }

    pub fn deinit(self: *Store) void {
        self.allocator.free(self.path);
        self.* = undefined;
    }

    pub fn load(self: *const Store) !?OwnedCatalog {
        const content = std.Io.Dir.cwd().readFileAlloc(global.io(), self.path, self.allocator, .limited(1024 * 1024)) catch |err| switch (err) {
            error.FileNotFound => return null,
            else => return err,
        };
        defer self.allocator.free(content);
        return try decodeCatalog(self.allocator, content);
    }

    /// Writes, fsyncs, atomically renames, then reads the mapping back before
    /// reporting success. A failed verification never becomes the in-memory
    /// authority.
    pub fn save(
        self: *const Store,
        catalog: Catalog,
    ) !void {
        const document = try encodeCatalog(self.allocator, catalog);
        defer self.allocator.free(document);
        const parent = std.fs.path.dirname(self.path) orelse
            return error.InvalidPath;
        try std.Io.Dir.cwd().createDirPath(global.io(), parent);
        const temporary_path = try std.fmt.allocPrint(
            self.allocator,
            "{s}.tmp",
            .{self.path},
        );
        defer self.allocator.free(temporary_path);
        const file = try std.Io.Dir.createFileAbsolute(global.io(), temporary_path, .{
            .truncate = true,
            .permissions = .fromMode(0o600),
        });
        errdefer std.Io.Dir.deleteFileAbsolute(global.io(), temporary_path) catch {};
        defer file.close(global.io());
        try file.writeStreamingAll(global.io(), document);
        try file.sync(global.io());
        try std.Io.Dir.renameAbsolute(temporary_path, self.path, global.io());
        var read_back = (try self.load()) orelse return error.ReadBackMismatch;
        defer read_back.deinit();
        if (!catalogEqual(catalog, read_back.value)) {
            return error.ReadBackMismatch;
        }
    }
};

pub fn encodeCatalog(
    allocator: std.mem.Allocator,
    catalog: Catalog,
) ![]u8 {
    try validateCatalog(catalog);
    var arena = std.heap.ArenaAllocator.init(allocator);
    defer arena.deinit();
    const temporary = arena.allocator();
    const workspaces = try temporary.alloc(
        PersistedWorkspace,
        catalog.workspaces.len,
    );
    for (catalog.workspaces, 0..) |snapshot, workspace_index| {
        workspaces[workspace_index] = try persistedWorkspace(
            temporary,
            snapshot,
        );
    }
    return std.json.Stringify.valueAlloc(
        allocator,
        PersistedCatalogEnvelope{
            .catalog_revision = catalog.catalog_revision,
            .archived_workspace_ids = catalog.archived_workspace_ids,
            .open_workspace_ids = catalog.open_workspace_ids,
            .selected_workspace_id = catalog.selected_workspace_id,
            .workspaces = workspaces,
        },
        .{ .whitespace = .minified },
    );
}

fn persistedWorkspace(
    allocator: std.mem.Allocator,
    snapshot: renderpkg.Snapshot,
) !PersistedWorkspace {
    const tabs = try allocator.alloc(PersistedTab, snapshot.tabs.len);
    for (snapshot.tabs, 0..) |tab, tab_index| {
        const panes = try allocator.alloc(PersistedPane, tab.panes.len);
        const canonical_ids = try allocator.alloc([]const u8, tab.panes.len);
        for (tab.panes, 0..) |pane, pane_index| {
            panes[pane_index] = .{
                .id = pane.id,
                .execution_target_id = if (pane.isTerminal()) pane.execution_target_id else null,
                .tmux_session_id = if (pane.isTerminal()) pane.tmux_session_id else null,
                .content = pane.content,
                .source = pane.source,
            };
            canonical_ids[pane_index] = pane.id;
        }
        tabs[tab_index] = .{
            .id = tab.id,
            .split_ratios = tab.split_ratios,
            .title = tab.title,
            .display_mode = tab.display_mode,
            .main_pane_id = tab.focused_pane_id,
            .canonical_pane_ids = canonical_ids,
            .visual_pane_ids = tab.visual_pane_ids,
            .panes = panes,
        };
    }
    return .{
        .portable_hosts = snapshot.portable_hosts,
        .portable_source_workspace_id = snapshot.portable_source_workspace_id,
        .id = snapshot.workspace_id,
        .name = snapshot.workspace_name,
        .revision = snapshot.workspace_revision,
        .selected_tab_id = snapshot.selected_tab_id,
        .managed_terminal_font_size_points = snapshot.font_size_points,
        .layout_map_visible = snapshot.layout_map_visible,
        .inbox_visible = snapshot.inbox_visible,
        .tabs = tabs,
    };
}

pub fn encode(
    allocator: std.mem.Allocator,
    snapshot: renderpkg.Snapshot,
) ![]u8 {
    const workspaces = [_]renderpkg.Snapshot{snapshot};
    return encodeCatalog(allocator, .{
        .catalog_revision = @max(snapshot.workspace_revision, 1),
        .selected_workspace_id = snapshot.workspace_id,
        .workspaces = &workspaces,
    });
}

pub fn decode(
    allocator: std.mem.Allocator,
    json: []const u8,
) !OwnedSnapshot {
    var catalog = try decodeCatalog(allocator, json);
    defer catalog.deinit();
    const selected = catalog.value.selected() orelse
        return error.InvalidSelectedWorkspace;
    return OwnedSnapshot.clone(allocator, selected);
}

pub fn decodeCatalog(
    allocator: std.mem.Allocator,
    json: []const u8,
) !OwnedCatalog {
    var version = try std.json.parseFromSlice(
        VersionEnvelope,
        allocator,
        json,
        .{ .allocate = .alloc_always, .ignore_unknown_fields = true },
    );
    defer version.deinit();
    var arena = std.heap.ArenaAllocator.init(allocator);
    errdefer arena.deinit();
    const alloc = arena.allocator();
    var value = switch (version.value.schema_version) {
        1, schema_version => blk: {
            var parsed = try std.json.parseFromSlice(
                PersistedCatalogEnvelope,
                allocator,
                json,
                .{ .allocate = .alloc_always, .ignore_unknown_fields = false },
            );
            defer parsed.deinit();
            break :blk try nativeCatalog(
                alloc,
                parsed.value.catalog_revision,
                parsed.value.selected_workspace_id,
                parsed.value.open_workspace_ids,
                parsed.value.archived_workspace_ids,
                parsed.value.workspaces,
            );
        },
        else => return error.UnsupportedSchemaVersion,
    };
    if (value.selected() == null or value.isArchived(value.selected_workspace_id)) {
        for (value.workspaces) |workspace| if (!value.isArchived(workspace.workspace_id)) {
            value.selected_workspace_id = workspace.workspace_id;
            break;
        };
    }
    value=try retireLegacySettings(alloc,value);
    try validateCatalog(value);
    return .{ .arena = arena, .value = value };
}

/// Use the same native removal operations as user actions; never create a Session.
fn retireLegacySettings(allocator:std.mem.Allocator,source:Catalog) !Catalog {
    var result=source;
    const workspaces=try allocator.dupe(renderpkg.Snapshot,source.workspaces);
    var scopes:std.ArrayList(LegacySettingsScope)=.empty;
    var changed=false;
    for(workspaces,source.workspaces) |*workspace,original| {
        var scope:?bool=null;
        for(original.tabs) |tab| {
            var legacy:std.ArrayList([]const u8)=.empty;
            for(tab.panes) |pane| if(pane.content)|content| {
                if(content.kind==.local_diagnostics or content.kind==.remote_hosts) {
                    try legacy.append(allocator,pane.id);
                    if(scope==null and std.mem.eql(u8,tab.id,original.selected_tab_id)) scope=content.kind==.remote_hosts;
                }
            };
            if(legacy.items.len==0) continue;
            changed=true;
            if(legacy.items.len==tab.panes.len and workspace.tabs.len>1) {
                workspace.*=try @import("native_workspace_state.zig").State.planSnapshotAction(allocator,workspace.*,.{.delete_tab=.{.tab_id=tab.id,.confirmation_id="native-settings-migration"}},null);
            } else for(legacy.items) |id| {
                workspace.*=try @import("native_workspace_state.zig").State.planSnapshotAction(allocator,workspace.*,.{.remove_pane=.{.pane_id=id}},null);
            }
        }
        if(scope)|remote| try scopes.append(allocator,.{.workspace_id=original.workspace_id,.remote=remote});
    }
    if(!changed) return source;
    result.workspaces=workspaces;
    result.catalog_revision=try std.math.add(u64,source.catalog_revision,1);
    result.legacy_settings=scopes.items;
    result.settings_migrated=true;
    return result;
}

fn nativeCatalog(
    allocator: std.mem.Allocator,
    catalog_revision: u64,
    selected_workspace_id: []const u8,
    open_workspace_ids: []const []const u8,
    archived_workspace_ids: []const []const u8,
    source: []const PersistedWorkspace,
) !Catalog {
    const workspaces = try allocator.alloc(renderpkg.Snapshot, source.len);
    for (source, 0..) |workspace, index| {
        workspaces[index] = try nativeWorkspace(allocator, workspace);
    }
    return .{
        .catalog_revision = catalog_revision,
        .open_workspace_ids = try cloneStrings(allocator, open_workspace_ids),
        .archived_workspace_ids = try cloneStrings(allocator, archived_workspace_ids),
        .selected_workspace_id = try allocator.dupe(
            u8,
            selected_workspace_id,
        ),
        .workspaces = workspaces,
    };
}

fn nativeWorkspace(
    alloc: std.mem.Allocator,
    workspace: PersistedWorkspace,
) !renderpkg.Snapshot {
    const tabs = try alloc.alloc(renderpkg.Tab, workspace.tabs.len);
    for (workspace.tabs, 0..) |tab, tab_index| {
        const panes = try alloc.alloc(renderpkg.Pane, tab.panes.len);
        for (tab.panes, 0..) |pane, pane_index| {
            panes[pane_index] = try nativePane(alloc, pane);
        }
        const visual = try cloneStrings(alloc, tab.visual_pane_ids);
        tabs[tab_index] = .{
            .id = try alloc.dupe(u8, tab.id),
            .split_ratios = try alloc.dupe(f32, tab.split_ratios),
            .title = try alloc.dupe(u8, tab.title),
            .display_mode = tab.display_mode,
            .focused_pane_id = if (tab.main_pane_id) |id|
                try alloc.dupe(u8, id)
            else
                null,
            .visual_pane_ids = visual,
            .layout = .{
                .pane_order = try cloneStrings(alloc, tab.canonical_pane_ids),
                .slots = &.{},
                .fingerprint = "native-owned",
            },
            .panes = panes,
        };
    }
    const revision = workspace.revision;
    return .{
        .portable_hosts = try cloneValue(alloc, workspace.portable_hosts),
        .portable_source_workspace_id = if(workspace.portable_source_workspace_id) |v| try alloc.dupe(u8,v) else null,
        .workspace_id = try alloc.dupe(u8, workspace.id),
        .workspace_name = try alloc.dupe(u8, workspace.name),
        .workspace_revision = revision,
        .font_size_points = workspace.managed_terminal_font_size_points,
        .layout_map_visible = workspace.layout_map_visible,
        .inbox_visible = workspace.inbox_visible,
        .selected_tab_id = try alloc.dupe(u8, workspace.selected_tab_id),
        .tabs = tabs,
        .snapshot_fingerprint = try std.fmt.allocPrint(
            alloc,
            "native:{s}:{d}",
            .{ workspace.id, revision },
        ),
    };
}

pub fn nativePane(
    allocator: std.mem.Allocator,
    pane: PersistedPane,
) !renderpkg.Pane {
    if (pane.content) |content| {
        if (pane.source != null) return error.InvalidPane;
        if (pane.execution_target_id != null or pane.tmux_session_id != null) return error.InvalidPane;
        const copy = try content.clone(allocator);
        return .{ .id = try allocator.dupe(u8, pane.id), .content = copy,
            .execution_target_id = "", .tmux_session_id = "", .transport_status = .offline,
            .attach_fingerprint = try copy.fingerprint(allocator), .attach_plan = .{ .program = "", .args = &.{} } };
    }
    const execution_target_id = try allocator.dupe(
        u8,
        pane.execution_target_id orelse return error.InvalidPane,
    );
    const tmux_session_id = try allocator.dupe(u8, pane.tmux_session_id orelse return error.InvalidPane);
    return .{
        .id = try allocator.dupe(u8, pane.id),
        .source = if (pane.source) |source| try source.clone(allocator) else null,
        .execution_target_id = execution_target_id,
        .tmux_session_id = tmux_session_id,
        .transport_status = .recovering,
        .attach_fingerprint = if (pane.source) |source|
            try std.fmt.allocPrint(allocator, "content:{s}:{s}:{s}", .{ execution_target_id, tmux_session_id, try std.json.Stringify.valueAlloc(allocator, source, .{}) })
        else try std.fmt.allocPrint(allocator, "content:{s}:{s}", .{ execution_target_id, tmux_session_id }),
        .attach_plan = .{
            .program = "polar-tmux-pane-host",
            .args = &.{},
        },
    };
}

fn deepCloneFromDecoded(
    allocator: std.mem.Allocator,
    source: renderpkg.Snapshot,
) !renderpkg.Snapshot {
    const tabs = try allocator.alloc(renderpkg.Tab, source.tabs.len);
    for (source.tabs, 0..) |tab, tab_index| {
        const panes = try allocator.alloc(renderpkg.Pane, tab.panes.len);
        for (tab.panes, 0..) |pane, pane_index| {
            panes[pane_index] = try nativePane(allocator, .{
                .id = pane.id,
                .execution_target_id = if (pane.isTerminal()) pane.execution_target_id else null,
                .tmux_session_id = if (pane.isTerminal()) pane.tmux_session_id else null,
                .content = pane.content,
                .source = pane.source,
            });
            panes[pane_index].transport_status = pane.transport_status;
        }
        tabs[tab_index] = .{
            .id = try allocator.dupe(u8, tab.id),
            .split_ratios = try allocator.dupe(f32, tab.split_ratios),
            .title = try allocator.dupe(u8, tab.title),
            .display_mode = tab.display_mode,
            .focused_pane_id = if (tab.focused_pane_id) |id|
                try allocator.dupe(u8, id)
            else
                null,
            .visual_pane_ids = try cloneStrings(
                allocator,
                tab.visual_pane_ids,
            ),
            .layout = .{
                .pane_order = try cloneStrings(
                    allocator,
                    tab.layout.pane_order,
                ),
                .slots = &.{},
                .fingerprint = "native-owned",
            },
            .panes = panes,
        };
    }
    return .{
        .portable_hosts = try cloneValue(allocator, source.portable_hosts),
        .portable_source_workspace_id = if(source.portable_source_workspace_id) |v| try allocator.dupe(u8,v) else null,
        .workspace_id = try allocator.dupe(u8, source.workspace_id),
        .workspace_name = try allocator.dupe(u8, source.workspace_name),
        .workspace_revision = source.workspace_revision,
        .font_size_points = source.font_size_points,
        .layout_map_visible = source.layout_map_visible,
        .inbox_visible = source.inbox_visible,
        .selected_tab_id = try allocator.dupe(u8, source.selected_tab_id),
        .tabs = tabs,
        .snapshot_fingerprint = try allocator.dupe(
            u8,
            source.snapshot_fingerprint,
        ),
    };
}

fn validateSnapshot(snapshot: renderpkg.Snapshot) !void {
    if (snapshot.workspace_id.len == 0 or
        snapshot.workspace_name.len == 0 or
        snapshot.workspace_revision == 0 or
        snapshot.tabs.len == 0 or
        snapshot.font_size_points < 10 or
        snapshot.font_size_points > 32)
    {
        return error.InvalidWorkspace;
    }
    var selected_found = false;
    for (snapshot.tabs, 0..) |tab, tab_index| {
        if (tab.split_ratios.len > 8) return error.InvalidWorkspace;
        for(tab.split_ratios) |ratio| if(!std.math.isFinite(ratio) or ratio<=0 or ratio>=1) return error.InvalidWorkspace;
        if (tab.id.len == 0 or tab.title.len == 0 or tab.panes.len > 9 or
            tab.visual_pane_ids.len != tab.panes.len)
        {
            return error.InvalidWorkspace;
        }
        if (std.mem.eql(u8, tab.id, snapshot.selected_tab_id)) {
            selected_found = true;
        }
        for (snapshot.tabs[0..tab_index]) |prior| {
            if (std.mem.eql(u8, prior.id, tab.id)) {
                return error.DuplicateTab;
            }
        }
        if (tab.display_mode == .monitor and tab.focused_pane_id != null) {
            return error.InvalidMainPane;
        }
        if (tab.display_mode == .focus and tab.panes.len > 0 and
            tab.focused_pane_id == null)
        {
            return error.InvalidMainPane;
        }
        for (tab.panes, 0..) |pane, pane_index| {
            if (pane.id.len == 0 or (pane.isTerminal() and (pane.execution_target_id.len == 0 or
                pane.tmux_session_id.len == 0)))
            {
                return error.InvalidPane;
            }
            if (pane.content) |content| {
                try content.validate();
                if (pane.execution_target_id.len != 0 or pane.tmux_session_id.len != 0) return error.InvalidPane;
            }
            var visual_count: usize = 0;
            for (tab.visual_pane_ids) |id| {
                if (std.mem.eql(u8, id, pane.id)) visual_count += 1;
            }
            if (visual_count != 1) return error.InvalidPaneOrder;
            for (tab.panes[0..pane_index]) |prior| {
                if (std.mem.eql(u8, prior.id, pane.id)) {
                    return error.DuplicatePane;
                }
            }
            for (snapshot.tabs[0..tab_index]) |prior_tab| {
                for (prior_tab.panes) |prior| {
                    if (std.mem.eql(u8, prior.id, pane.id)) {
                        return error.DuplicatePane;
                    }
                }
            }
        }
        if (tab.focused_pane_id) |main| {
            for (tab.panes) |pane| {
                if (std.mem.eql(u8, pane.id, main)) break;
            } else return error.InvalidMainPane;
        }
    }
    if (!selected_found) return error.InvalidSelectedTab;
}

fn validateCatalog(catalog: Catalog) !void {
    if (catalog.catalog_revision == 0 or
        catalog.selected_workspace_id.len == 0 or
        catalog.workspaces.len == 0)
    {
        return error.InvalidCatalog;
    }
    var selected_found = false;
    for (catalog.workspaces, 0..) |workspace, index| {
        try validateSnapshot(workspace);
        if (std.mem.eql(
            u8,
            workspace.workspace_id,
            catalog.selected_workspace_id,
        )) selected_found = true;
        for (catalog.workspaces[0..index]) |prior| {
            if (std.mem.eql(
                u8,
                prior.workspace_id,
                workspace.workspace_id,
            )) return error.DuplicateWorkspace;
        }
    }
    if (!selected_found) return error.InvalidSelectedWorkspace;
    if (catalog.isArchived(catalog.selected_workspace_id)) return error.InvalidSelectedWorkspace;
    for (catalog.archived_workspace_ids, 0..) |id, index| {
        var found = false;
        for (catalog.workspaces) |workspace| if (std.mem.eql(u8, id, workspace.workspace_id)) { found = true; break; };
        if (!found) return error.WorkspaceNotFound;
        for (catalog.archived_workspace_ids[0..index]) |prior| if (std.mem.eql(u8, prior, id)) return error.DuplicateWorkspace;
    }

    for (catalog.open_workspace_ids, 0..) |id, index| {
        if (catalog.isArchived(id)) return error.InvalidCatalog;
        var found = false;
        for (catalog.workspaces) |workspace| {
            if (std.mem.eql(u8, workspace.workspace_id, id)) { found = true; break; }
        }
        if (!found) return error.WorkspaceNotFound;
        for (catalog.open_workspace_ids[0..index]) |prior| {
            if (std.mem.eql(u8, id, prior)) return error.DuplicateWorkspace;
        }
    }
}

fn mappingEqual(
    left: renderpkg.Snapshot,
    right: renderpkg.Snapshot,
) bool {
    const left_json = encode(std.heap.page_allocator, left) catch return false;
    defer std.heap.page_allocator.free(left_json);
    const right_json = encode(std.heap.page_allocator, right) catch return false;
    defer std.heap.page_allocator.free(right_json);
    return std.mem.eql(u8, left_json, right_json);
}

fn catalogEqual(left: Catalog, right: Catalog) bool {
    if (left.archived_workspace_ids.len != right.archived_workspace_ids.len) return false;
    for (left.archived_workspace_ids, right.archived_workspace_ids) |a, b| if (!std.mem.eql(u8, a, b)) return false;
    const left_json = encodeCatalog(
        std.heap.page_allocator,
        left,
    ) catch return false;
    defer std.heap.page_allocator.free(left_json);
    const right_json = encodeCatalog(
        std.heap.page_allocator,
        right,
    ) catch return false;
    defer std.heap.page_allocator.free(right_json);
    return std.mem.eql(u8, left_json, right_json);
}

fn cloneStrings(
    allocator: std.mem.Allocator,
    source: []const []const u8,
) ![]const []const u8 {
    const result = try allocator.alloc([]const u8, source.len);
    for (source, 0..) |value, index| {
        result[index] = try allocator.dupe(u8, value);
    }
    return result;
}

pub fn statePath(allocator: std.mem.Allocator) ![]u8 {
    const base = global.environ().getAlloc(
        allocator,
        "XDG_STATE_HOME",
    ) catch |err| switch (err) {
        error.EnvironmentVariableMissing => blk: {
            const home = try global.environ().getAlloc(allocator, "HOME");
            defer allocator.free(home);
            break :blk try std.fs.path.join(
                allocator,
                &.{ home, ".local", "state" },
            );
        },
        else => return err,
    };
    defer allocator.free(base);
    return std.fs.path.join(
        allocator,
        &.{ base, "polar-station", "native-layout.json" },
    );
}


















fn cloneValue(a:std.mem.Allocator,value:std.json.Value) !std.json.Value {
    const bytes=try std.json.Stringify.valueAlloc(a,value,.{});defer a.free(bytes);
    return std.json.parseFromSliceLeaky(std.json.Value,a,bytes,.{.allocate=.alloc_always});
}
pub fn encodeWorkspace(a:std.mem.Allocator,snapshot:renderpkg.Snapshot) ![]u8 {
    var arena=std.heap.ArenaAllocator.init(a);defer arena.deinit();
    return std.json.Stringify.valueAlloc(a,try persistedWorkspace(arena.allocator(),snapshot),.{});
}
pub fn decodeWorkspace(a:std.mem.Allocator,bytes:[]const u8) !OwnedSnapshot {
    var arena=std.heap.ArenaAllocator.init(a);errdefer arena.deinit();
    const saved=try std.json.parseFromSliceLeaky(PersistedWorkspace,arena.allocator(),bytes,.{.allocate=.alloc_always,.ignore_unknown_fields=true});
    const value=try nativeWorkspace(arena.allocator(),saved);try validateSnapshot(value);
    return .{.arena=arena,.value=value};
}
