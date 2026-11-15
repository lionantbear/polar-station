//! One process-wide native GTK Resources editor.

const auxiliary = @import("auxiliary_window.zig");
const std = @import("std");
const adw = @import("adw");
const gio = @import("gio");
const glib = @import("glib");
const gobject = @import("gobject");
const gtk = @import("gtk");
const file_launcher = @import("native_file_launcher.zig");
const Application = @import("../class/application.zig").Application;
const resource = @import("resources.zig");

pub const Action = union(enum) {
    apply: resource.Catalog,
    restart: []const u8,
    open_mirror: []const u8,
    get_log: []const u8,
    reload,
    cancel_file_open,
};

pub const ActionFn = *const fn (?*anyopaque, Action) void;

const CommonRow = struct {
    id: []u8,
    root: *gtk.Box,
    enabled: *gtk.ToggleButton,
    name: *gtk.Entry,
    target: *gtk.Entry,
    status: *gtk.Label,
    removed: bool = false,
};

const NavigationRow = struct {
    owner: *Panel,
    id: []const u8,
    detail: *gtk.Box,
};

const FileRow = struct {
    owner: *Panel,
    common: CommonRow,
    remote_path: *gtk.Entry,
    local_path: *gtk.Entry,
    applied_local_path: []u8,
    reveal: *gtk.Button,
    restart: *gtk.Button,
    log_button: *gtk.Button,
};

const PortRow = struct {
    owner: *Panel,
    common: CommonRow,
    bind_address: *gtk.Entry,
    allow_non_loopback: *gtk.ToggleButton,
    local_start: *gtk.SpinButton,
    local_end: *gtk.SpinButton,
    remote_host: *gtk.Entry,
    remote_start: *gtk.SpinButton,
    remote_range: *gtk.Label,
    restart: *gtk.Button,
    log_button: *gtk.Button,
};

const AndroidRow = struct {
    owner: *Panel,
    common: CommonRow,
    bind_address: *gtk.Entry,
    allow_non_loopback: *gtk.ToggleButton,
    local_port: *gtk.SpinButton,
    remote_host: *gtk.Entry,
    remote_port: *gtk.SpinButton,
    scrcpy_arguments: *gtk.Entry,
    mirror: *gtk.Button,
    restart: *gtk.Button,
    log_button: *gtk.Button,
};

pub const Panel = struct {
    allocator: std.mem.Allocator,
    app: *Application,
    action_fn: ActionFn,
    action_context: ?*anyopaque,
    split: ?*gtk.Paned = null,
    navigation_scroll: ?*gtk.ScrolledWindow = null,
    detail_scroll: ?*gtk.ScrolledWindow = null,
    content_scroll: ?*gtk.ScrolledWindow = null,
    page_header: ?*gtk.Box = null,
    page_footer: ?*gtk.Box = null,
    editor_root: ?*gtk.Widget = null,
    log_window: ?*adw.Window = null,
    log_body: ?*gtk.Box = null,
    body: ?*gtk.Box = null,
    navigation: ?*gtk.Box = null,
    type_stack: ?*gtk.Stack = null,
    state_label: ?*gtk.Label = null,
    conflict_label: ?*gtk.Label = null,
    reload_button: ?*gtk.Button = null,
    apply_button: ?*gtk.Button = null,
    file_section: ?*gtk.Box = null,
    port_section: ?*gtk.Box = null,
    android_section: ?*gtk.Box = null,
    revision: ?u64 = null,
    editor: @import("resource_editor.zig").Adapter = .{},
    projection_fn: ?*const fn (?*anyopaque) ?*const resource.Projection = null,
    leave_dialog: ?*adw.MessageDialog = null,
    backend_available: bool = false,
    can_apply: bool = false,
    rendering: bool = false,
    files: std.ArrayList(*FileRow) = .empty,
    ports: std.ArrayList(*PortRow) = .empty,
    android: std.ArrayList(*AndroidRow) = .empty,
    navigation_rows: std.ArrayList(*NavigationRow) = .empty,

    pub fn init(
        allocator: std.mem.Allocator,
        app: *Application,
        action_fn: ActionFn,
        action_context: ?*anyopaque,
    ) Panel {
        return .{
            .allocator = allocator,
            .app = app,
            .action_fn = action_fn,
            .action_context = action_context,
        };
    }

    pub fn deinit(self: *Panel) void {
        self.editor.deinit();
        file_launcher.cancel(.resources);

        if (self.log_window) |window| auxiliary.destroy(window);
        self.clearRows();
        if(self.editor_root) |root| root.as(gobject.Object).unref();
        self.files.deinit(self.allocator);
        self.ports.deinit(self.allocator);
        self.android.deinit(self.allocator);
        self.navigation_rows.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn embed(self: *Panel, projection: ?*const resource.Projection, selected: []const u8) ?auxiliary.ContentLease {
        self.ensureContent();
        self.setEmbedded(true);
        self.navigation_scroll.?.as(gtk.Widget).setVisible(@intFromBool(selected.len == 0));
        if (projection) |value| self.render(value, false);
        self.applyBackendAvailability();
        if (selected.len > 0) self.selectDetail(selected);
        return auxiliary.ContentLease.borrow(self.editor_root.?);
    }
    fn setEmbedded(self: *Panel, embedded: bool) void {
        self.page_header.?.as(gtk.Widget).setVisible(@intFromBool(!embedded));
        self.page_footer.?.as(gtk.Orientable).setOrientation(if (embedded) .vertical else .horizontal);
        const destination = if (embedded) self.page_footer.? else self.page_header.?;
        for ([_]*gtk.Widget{ self.reload_button.?.as(gtk.Widget), self.state_label.?.as(gtk.Widget) }) |widget| {
            if (widget.getParent() != destination.as(gtk.Widget)) {
                _ = widget.as(gobject.Object).ref();
                if (widget.getParent()) |parent| if (gobject.ext.cast(gtk.Box, parent.as(gobject.Object))) |box| box.remove(widget);
                if (widget == self.state_label.?.as(gtk.Widget)) destination.prepend(widget) else destination.append(widget);
                widget.as(gobject.Object).unref();
            }
        }
        self.editor_root.?.setSizeRequest(if (embedded) -1 else 760, if (embedded) -1 else 520);
        self.split.?.as(gtk.Orientable).setOrientation(if (embedded) .vertical else .horizontal);
        self.content_scroll.?.setPolicy(.never, if (embedded) .automatic else .never);
        self.navigation_scroll.?.as(gtk.Widget).setVisible(1);
        for ([_]*gtk.ScrolledWindow{ self.navigation_scroll.?, self.detail_scroll.? }) |scroll| {
            scroll.setPolicy(.never, if (embedded) .never else .automatic);
            scroll.setPropagateNaturalHeight(@intFromBool(embedded));
        }
    }
    /// Prepare the original mapping draft; the caller presents its Settings route.
    pub fn prefillFile(self: *Panel, target: []const u8, remote: []const u8, source: []const u8, request_id: []const u8) ?[]u8 {
        self.ensureContent();
        if (self.projection_fn) |projection| if (projection(self.action_context)) |value| self.render(value, false);
        self.syncDraft();
        var result = self.editor.dispatch(self.allocator, .{ .type = "prefill", .prefill = .{ .execution_target_id = target, .remote_directory = remote, .source_path = source, .request_id = request_id } }) orelse return null;
        defer result.deinit();
        self.renderEditor(result.value, true);
        return if (result.value.view.selected) |id| self.allocator.dupe(u8, id) catch null else null;
    }
    pub fn setBackendAvailable(self: *Panel, available: bool) void {
        self.backend_available = available;
        if (self.editor_root == null) return;
        self.applyBackendAvailability();
    }

    pub fn render(self: *Panel, projection: *const resource.Projection, _: bool) void {
        self.ensureContent();
        self.syncDraft();
        var result = self.editor.dispatch(self.allocator, .{ .type = "snapshot", .catalog = projection.catalog }) orelse return;
        defer result.deinit();
        setLabel(self.allocator, self.state_label.?, projection.aggregate_message) catch {};
        self.renderEditor(result.value, false);
        self.updateStatuses(projection);
    }
    fn renderEditor(self: *Panel, output: @import("resource_editor.zig").Output, rebuild: bool) void {
        const view = output.view;
        self.conflict_label.?.as(gtk.Widget).setVisible(@intFromBool(view.message != null));
        if (view.message) |message| setLabel(self.allocator, self.conflict_label.?, message) catch {};
        self.can_apply = view.can_apply;
        self.apply_button.?.as(gtk.Widget).setSensitive(@intFromBool(self.backend_available and self.can_apply));
        if (view.loaded and (rebuild or self.revision == null or self.revision.? != view.catalog.revision)) {
            self.revision = view.catalog.revision;
            self.rendering = true;
            defer self.rendering = false;
            self.clearRows();
            clearBox(self.body.?);
            clearBox(self.navigation.?);
            const current = if (self.projection_fn) |get| get(self.action_context) else null;
            self.buildCatalog(view.catalog, current) catch return;
            if (view.selected) |id| self.selectDetail(id);
        }
        if (view.confirmation != null and self.leave_dialog == null) {
            const title = self.allocator.dupeZ(u8, view.confirmation_title) catch return;
            defer self.allocator.free(title);
            const dialog = adw.MessageDialog.new(auxiliary.contentParent(self.app,self.editor_root.?), title, null);
            inline for (.{ "discard", "keep_editing" }, .{ view.discard_label, view.keep_label }) |id, label_text| {
                const value = self.allocator.dupeZ(u8, label_text) catch return;
                defer self.allocator.free(value);
                dialog.addResponse(id, value);
            }
            dialog.setResponseAppearance("discard", .destructive);
            dialog.setDefaultResponse("keep_editing");
            dialog.setCloseResponse("keep_editing");
            _ = adw.MessageDialog.signals.response.connect(dialog, *Panel, leaveResponse, self, .{});
            self.leave_dialog = dialog;
            dialog.as(gtk.Window).present();
        }
        for (output.effects) |effect| {
            if (std.mem.eql(u8, effect.type, "apply")) {
                if (effect.catalog) |catalog| self.action_fn(self.action_context, .{ .apply = catalog });
            } else if (std.mem.eql(u8, effect.type, "reload")) self.action_fn(self.action_context, .reload) else if (std.mem.eql(u8, effect.type, "close")) {
                self.action_fn(self.action_context, .cancel_file_open);
                auxiliary.closeContent(self.editor_root.?);
            }
        }
    }
    fn leaveResponse(dialog: *adw.MessageDialog, response: [*:0]const u8, self: *Panel) callconv(.c) void {
        self.leave_dialog = null;
        dialog.as(gtk.Window).destroy();
        self.editorAction(.{ .type = std.mem.span(response) }, true);
    }
    fn editorAction(self: *Panel, input: anytype, rebuild: bool) void {
        self.ensureContent();
        self.syncDraft();
        var result = self.editor.dispatch(self.allocator, input) orelse return;
        defer result.deinit();
        self.renderEditor(result.value, rebuild);
    }
    pub fn showError(self: *Panel, message: []const u8) void {
        self.editorAction(.{ .type = "failed", .message = message }, false);
    }

    pub fn showLog(self: *Panel, tail: *const resource.LogTail) void {
        if (self.log_window == null) {
            const window = auxiliary.create(self.app);
            window.as(gtk.Window).setTitle("Resource Log");
            window.as(gtk.Window).setIconName("com.polarealm.PolarStation");
            window.as(gtk.Window).setDefaultSize(640, 380);
            window.as(gtk.Window).setHideOnClose(1);
            const root = gtk.Box.new(.vertical, 8);
            setMargins(root.as(gtk.Widget), 16);
            root.append(label("Resource Log", "title-2").as(gtk.Widget));
            const scroll = gtk.ScrolledWindow.new();
            scroll.as(gtk.Widget).setVexpand(1);
            const body = gtk.Box.new(.vertical, 5);
            scroll.setChild(body.as(gtk.Widget));
            root.append(scroll.as(gtk.Widget));
            const toolbar = adw.ToolbarView.new();
            toolbar.addTopBar(adw.HeaderBar.new().as(gtk.Widget));
            toolbar.setContent(root.as(gtk.Widget));
            window.setContent(toolbar.as(gtk.Widget));
            self.log_window = window;
            self.log_body = body;
        }
        clearBox(self.log_body.?);
        for (tail.events) |event| {
            const line = std.fmt.allocPrint(
                self.allocator,
                "{s}  {s}",
                .{ event.level, event.message },
            ) catch continue;
            defer self.allocator.free(line);
            const owned = self.allocator.dupeZ(u8, line) catch continue;
            defer self.allocator.free(owned);
            self.log_body.?.append(label(owned, "monospace").as(gtk.Widget));
        }
        auxiliary.present(self.app, self.log_window.?);
    }

    fn ensureContent(self: *Panel) void {
        if (self.editor_root != null) return;
        const root = gtk.Box.new(.vertical, 0);
        _=root.as(gobject.Object).refSink();
        self.editor_root = root.as(gtk.Widget);
        root.as(gtk.Widget).setSizeRequest(760, 520);
        const header = gtk.Box.new(.horizontal, 10);
        self.page_header = header;
        setMargins(header.as(gtk.Widget), 16);
        const title = label("Global Resources", "title-2");
        const state = label("Loading Resources…", "caption");
        state.as(gtk.Widget).setHexpand(1);
        const reload = gtk.Button.newWithLabel("Reload");
        const done = gtk.Button.newWithLabel("Done");
        header.append(title.as(gtk.Widget));
        header.append(state.as(gtk.Widget));
        header.append(reload.as(gtk.Widget));
        header.append(done.as(gtk.Widget));
        const conflict = label("", "warning");
        conflict.as(gtk.Widget).setVisible(0);
        conflict.as(gtk.Widget).setMarginStart(16);
        conflict.as(gtk.Widget).setMarginEnd(16);
        const paned = gtk.Paned.new(.horizontal);
        self.split = paned;
        paned.as(gtk.Widget).setVexpand(1);
        paned.setPosition(260);
        paned.setResizeStartChild(0);
        paned.setShrinkStartChild(0);
        const navigation_scroll = gtk.ScrolledWindow.new();
        self.navigation_scroll = navigation_scroll;
        // Category controls define the sidebar minimum; never hide a category
        // behind horizontal scrolling when the splitter starts at 260px.
        navigation_scroll.setPolicy(.never, .automatic);
        navigation_scroll.as(gtk.Widget).setSizeRequest(220, -1);
        const navigation = gtk.Box.new(.vertical, 8);
        setMargins(navigation.as(gtk.Widget), 12);
        navigation_scroll.setChild(navigation.as(gtk.Widget));
        const scroll = gtk.ScrolledWindow.new();
        self.detail_scroll = scroll;
        scroll.as(gtk.Widget).setVexpand(1);
        const body = gtk.Box.new(.vertical, 12);
        setMargins(body.as(gtk.Widget), 20);
        scroll.setChild(body.as(gtk.Widget));
        paned.setStartChild(navigation_scroll.as(gtk.Widget));
        paned.setEndChild(scroll.as(gtk.Widget));
        const footer = gtk.Box.new(.horizontal, 8);
        self.page_footer = footer;
        setMargins(footer.as(gtk.Widget), 16);
        const note = label(
            "Draft changes do nothing until one revision-bound Apply.",
            "caption",
        );
        note.as(gtk.Widget).setHexpand(1);
        const apply = gtk.Button.newWithLabel("Apply");
        apply.as(gtk.Widget).addCssClass("suggested-action");
        footer.append(note.as(gtk.Widget));
        footer.append(apply.as(gtk.Widget));
        root.append(header.as(gtk.Widget));
        root.append(conflict.as(gtk.Widget));
        const content_scroll = gtk.ScrolledWindow.new();
        self.content_scroll = content_scroll;
        content_scroll.setPolicy(.never, .never);
        content_scroll.as(gtk.Widget).setVexpand(1);
        content_scroll.setChild(paned.as(gtk.Widget));
        root.append(content_scroll.as(gtk.Widget));
        root.append(footer.as(gtk.Widget));
        connect(reload, Panel, reloadClicked, self);
        connect(done, Panel, doneClicked, self);
        connect(apply, Panel, applyClicked, self);
        self.body = body;
        self.navigation = navigation;
        self.state_label = state;
        self.conflict_label = conflict;
        self.reload_button = reload;
        self.apply_button = apply;
        self.applyBackendAvailability();
    }

    fn applyBackendAvailability(self: *Panel) void {
        const enabled = @intFromBool(self.backend_available);
        if (self.reload_button) |button| {
            button.as(gtk.Widget).setSensitive(enabled);
        }
        if (self.apply_button) |button| {
            button.as(gtk.Widget).setSensitive(@intFromBool(self.backend_available and self.can_apply));
        }
        if (!self.backend_available) {
            if (self.state_label) |state| {
                setLabel(
                    self.allocator,
                    state,
                    "Backend unavailable — this draft is preserved. Reconnect to apply changes.",
                ) catch {};
            }
        }
        for (self.files.items) |row| {
            if (!self.backend_available) setFileActionAvailability(row, null);
        }
        for (self.ports.items) |row| {
            if (!self.backend_available) setPortActionAvailability(row, null);
        }
        for (self.android.items) |row| {
            if (!self.backend_available) setAndroidActionAvailability(row, null);
        }
    }

    fn buildCatalog(self: *Panel, catalog: resource.Catalog, projection: ?*const resource.Projection) !void {
        const type_switcher = gtk.StackSwitcher.new();
        const type_stack = gtk.Stack.new();
        self.type_stack = type_stack;
        type_switcher.setStack(type_stack);
        type_switcher.as(gtk.Widget).setHexpand(1);
        self.navigation.?.append(type_switcher.as(gtk.Widget));
        self.navigation.?.append(type_stack.as(gtk.Widget));
        const files = gtk.Box.new(.vertical, 8);
        const android = gtk.Box.new(.vertical, 8);
        const ports = gtk.Box.new(.vertical, 8);
        self.file_section = files;
        self.android_section = android;
        self.port_section = ports;
        _ = type_stack.addTitled(ports.as(gtk.Widget), "ports", "Ports");
        _ = type_stack.addTitled(files.as(gtk.Widget), "files", "Files");
        _ = type_stack.addTitled(android.as(gtk.Widget), "android", "Android");
        var selected = false;
        for (catalog.file_mappings) |value| {
            const row = try self.createFileRow(value, if (projection) |current| current.entry(value.id) else null);
            try self.addNavigation(files, &row.common, &selected);
        }
        for (catalog.android_mirrors) |value| {
            const row = try self.createAndroidRow(value, if (projection) |current| current.entry(value.id) else null);
            try self.addNavigation(android, &row.common, &selected);
        }
        for (catalog.port_forwards) |value| {
            const row = try self.createPortRow(value, if (projection) |current| current.entry(value.id) else null);
            try self.addNavigation(ports, &row.common, &selected);
        }
        const add_file = gtk.Button.newWithLabel("+ Add File Mapping");
        const add_android = gtk.Button.newWithLabel("+ Add Android Mirror");
        const add_port = gtk.Button.newWithLabel("+ Add Port Forward");
        files.append(add_file.as(gtk.Widget));
        android.append(add_android.as(gtk.Widget));
        ports.append(add_port.as(gtk.Widget));
        connect(add_file, Panel, addFileClicked, self);
        connect(add_android, Panel, addAndroidClicked, self);
        connect(add_port, Panel, addPortClicked, self);
    }

    fn addNavigation(
        self: *Panel,
        list: *gtk.Box,
        common: *CommonRow,
        selected: *bool,
    ) !void {
        const button = gtk.Button.new();
        const summary = gtk.Box.new(.vertical, 2);
        const name_label = label("", "heading");
        try setLabel(self.allocator, name_label, text(common.name));
        summary.append(name_label.as(gtk.Widget));
        const status_label = label("Select to edit", "caption");
        summary.append(status_label.as(gtk.Widget));
        button.setChild(summary.as(gtk.Widget));
        list.append(button.as(gtk.Widget));
        self.body.?.append(common.root.as(gtk.Widget));
        const navigation_row = try self.allocator.create(NavigationRow);
        navigation_row.* = .{ .owner = self, .id = common.id, .detail = common.root };
        try self.navigation_rows.append(self.allocator, navigation_row);
        connect(button, NavigationRow, navigationClicked, navigation_row);
        common.root.as(gtk.Widget).setVisible(@intFromBool(!selected.*));
        if (!selected.*) selected.* = true;
    }

    fn selectDetail(self: *Panel, selected_id: []const u8) void {
        if (self.type_stack) |stack| {
            for (self.files.items) |row| if (std.mem.eql(u8, row.common.id, selected_id)) {
                stack.setVisibleChildName("files");
                break;
            };
            for (self.ports.items) |row| if (std.mem.eql(u8, row.common.id, selected_id)) {
                stack.setVisibleChildName("ports");
                break;
            };
            for (self.android.items) |row| if (std.mem.eql(u8, row.common.id, selected_id)) {
                stack.setVisibleChildName("android");
                break;
            };
        }
        for (self.navigation_rows.items) |row| {
            row.detail.as(gtk.Widget).setVisible(
                @intFromBool(std.mem.eql(u8, row.id, selected_id)),
            );
        }
    }

    fn createCommon(
        self: *Panel,
        id: []const u8,
        name_value: []const u8,
        enabled_value: bool,
        target_value: []const u8,
        status_value: []const u8,
    ) !CommonRow {
        const root = gtk.Box.new(.vertical, 6);
        root.as(gtk.Widget).addCssClass("card");
        setMargins(root.as(gtk.Widget), 8);
        const top = gtk.Box.new(.horizontal, 6);
        const enabled = toggle("Enabled", enabled_value);
        const name = try entry(self.allocator, name_value);
        name.as(gtk.Widget).setHexpand(1);
        const status = label("", "caption");
        try setLabel(self.allocator, status, status_value);
        top.append(enabled.as(gtk.Widget));
        top.append(name.as(gtk.Widget));
        top.append(status.as(gtk.Widget));
        const target = try entry(self.allocator, target_value);
        target.setPlaceholderText("SSH target");
        root.append(top.as(gtk.Widget));
        root.append(target.as(gtk.Widget));
        self.watchEntry(name);
        self.watchEntry(target);
        self.watchToggle(enabled);
        return .{
            .id = try self.allocator.dupe(u8, id),
            .root = root,
            .enabled = enabled,
            .name = name,
            .target = target,
            .status = status,
        };
    }

    fn createFileRow(
        self: *Panel,
        value: resource.FileMapping,
        projected: ?*const resource.Entry,
    ) !*FileRow {
        const row = try self.allocator.create(FileRow);
        errdefer self.allocator.destroy(row);
        const reveal = gtk.Button.newWithLabel("Show Local");
        const restart = gtk.Button.newWithLabel("Restart");
        const log_button = gtk.Button.newWithLabel("Log");
        row.* = .{
            .owner = self,
            .common = try self.createCommon(
                value.id,
                value.name,
                value.enabled,
                value.execution_target_id,
                if (projected) |entry_value| entry_value.status.message else "Pending Apply",
            ),
            .remote_path = try entry(self.allocator, value.remote_path),
            .local_path = try entry(self.allocator, value.local_path),
            .applied_local_path = try self.allocator.dupe(u8, value.local_path),
            .reveal = reveal,
            .restart = restart,
            .log_button = log_button,
        };
        row.remote_path.setPlaceholderText("Remote path");
        row.local_path.setPlaceholderText("Local path");
        row.common.root.prepend(section("File Mapping").as(gtk.Widget));
        row.common.root.append(section("Remote on target").as(gtk.Widget));
        row.common.root.append(row.remote_path.as(gtk.Widget));
        row.common.root.append(section("On this computer").as(gtk.Widget));
        row.common.root.append(row.local_path.as(gtk.Widget));
        const actions = gtk.Box.new(.horizontal, 6);
        const remove = gtk.Button.newWithLabel("Remove");
        actions.append(reveal.as(gtk.Widget));
        actions.append(restart.as(gtk.Widget));
        actions.append(log_button.as(gtk.Widget));
        actions.append(remove.as(gtk.Widget));
        row.common.root.append(actions.as(gtk.Widget));
        setFileActionAvailability(row, projected);
        connect(reveal, FileRow, fileRevealClicked, row);
        connect(restart, FileRow, fileRestartClicked, row);
        connect(log_button, FileRow, fileLogClicked, row);
        connect(remove, FileRow, fileRemoveClicked, row);
        self.watchEntry(row.remote_path);
        self.watchEntry(row.local_path);
        try self.files.append(self.allocator, row);
        return row;
    }

    fn createPortRow(
        self: *Panel,
        value: resource.PortForward,
        projected: ?*const resource.Entry,
    ) !*PortRow {
        const row = try self.allocator.create(PortRow);
        errdefer self.allocator.destroy(row);
        const restart = gtk.Button.newWithLabel("Restart");
        const log_button = gtk.Button.newWithLabel("Log");
        row.* = .{
            .owner = self,
            .common = try self.createCommon(
                value.id,
                value.name,
                value.enabled,
                value.execution_target_id,
                if (projected) |entry_value| entry_value.status.message else "Pending Apply",
            ),
            .bind_address = try entry(self.allocator, value.bind_address),
            .allow_non_loopback = toggle("Allow non-loopback", value.allow_non_loopback),
            .local_start = configurePortSpinButton(gtk.SpinButton.newWithRange(1, 65535, 1)),
            .local_end = configurePortSpinButton(gtk.SpinButton.newWithRange(1, 65535, 1)),
            .remote_host = try entry(self.allocator, value.remote_host),
            .remote_start = configurePortSpinButton(gtk.SpinButton.newWithRange(1, 65535, 1)),
            .remote_range = label("Remote range:", "caption"),
            .restart = restart,
            .log_button = log_button,
        };
        row.local_start.setValue(@floatFromInt(value.local_port_start));
        row.local_end.setValue(@floatFromInt(value.local_port_end));
        row.remote_start.setValue(@floatFromInt(value.remote_port_start));
        row.common.root.prepend(section("Port Forwarding").as(gtk.Widget));
        row.common.root.append(section("This computer").as(gtk.Widget));
        row.bind_address.setPlaceholderText("Bind address");
        row.common.root.append(row.bind_address.as(gtk.Widget));
        const local_range = gtk.Box.new(.horizontal, 8);
        local_range.append(label("From", "caption").as(gtk.Widget));
        local_range.append(row.local_start.as(gtk.Widget));
        local_range.append(label("To", "caption").as(gtk.Widget));
        local_range.append(row.local_end.as(gtk.Widget));
        row.common.root.append(local_range.as(gtk.Widget));
        row.common.root.append(section("Remote through target").as(gtk.Widget));
        row.remote_host.setPlaceholderText("Remote host");
        row.common.root.append(row.remote_host.as(gtk.Widget));
        row.common.root.append(row.remote_start.as(gtk.Widget));
        row.common.root.append(row.remote_range.as(gtk.Widget));
        row.common.root.append(section("Advanced").as(gtk.Widget));
        row.common.root.append(row.allow_non_loopback.as(gtk.Widget));
        const actions = gtk.Box.new(.horizontal, 6);
        const remove = gtk.Button.newWithLabel("Remove");
        actions.append(restart.as(gtk.Widget));
        actions.append(log_button.as(gtk.Widget));
        actions.append(remove.as(gtk.Widget));
        row.common.root.append(actions.as(gtk.Widget));
        setPortActionAvailability(row, projected);
        connect(restart, PortRow, portRestartClicked, row);
        connect(log_button, PortRow, portLogClicked, row);
        connect(remove, PortRow, portRemoveClicked, row);
        _ = gtk.SpinButton.signals.value_changed.connect(
            row.local_start,
            *PortRow,
            portRangeChanged,
            row,
            .{},
        );
        _ = gtk.SpinButton.signals.value_changed.connect(
            row.local_end,
            *PortRow,
            portRangeChanged,
            row,
            .{},
        );
        _ = gtk.SpinButton.signals.value_changed.connect(
            row.remote_start,
            *PortRow,
            portRangeChanged,
            row,
            .{},
        );
        updatePortRange(row);
        self.watchEntry(row.bind_address);
        self.watchEntry(row.remote_host);
        self.watchToggle(row.allow_non_loopback);
        self.watchSpin(row.local_start);
        self.watchSpin(row.local_end);
        self.watchSpin(row.remote_start);
        try self.ports.append(self.allocator, row);
        return row;
    }

    fn createAndroidRow(
        self: *Panel,
        value: resource.AndroidMirror,
        projected: ?*const resource.Entry,
    ) !*AndroidRow {
        const row = try self.allocator.create(AndroidRow);
        errdefer self.allocator.destroy(row);
        const args = try std.mem.join(self.allocator, "\n", value.scrcpy_arguments);
        defer self.allocator.free(args);
        const mirror = gtk.Button.newWithLabel("Open Mirror");
        const restart = gtk.Button.newWithLabel("Restart");
        const log_button = gtk.Button.newWithLabel("Log");
        row.* = .{
            .owner = self,
            .common = try self.createCommon(
                value.id,
                value.name,
                value.enabled,
                value.execution_target_id,
                if (projected) |entry_value| entry_value.status.message else "Pending Apply",
            ),
            .bind_address = try entry(self.allocator, value.bind_address),
            .allow_non_loopback = toggle("Allow non-loopback", value.allow_non_loopback),
            .local_port = configurePortSpinButton(gtk.SpinButton.newWithRange(1, 65535, 1)),
            .remote_host = try entry(self.allocator, value.remote_adb_host),
            .remote_port = configurePortSpinButton(gtk.SpinButton.newWithRange(1, 65535, 1)),
            .scrcpy_arguments = try entry(self.allocator, args),
            .mirror = mirror,
            .restart = restart,
            .log_button = log_button,
        };
        row.local_port.setValue(@floatFromInt(value.local_adb_port));
        row.remote_port.setValue(@floatFromInt(value.remote_adb_port));
        row.common.root.prepend(section("Android · ADB + scrcpy").as(gtk.Widget));
        row.common.root.append(section("Local ADB").as(gtk.Widget));
        row.bind_address.setPlaceholderText("Local ADB host");
        row.common.root.append(row.bind_address.as(gtk.Widget));
        row.common.root.append(row.local_port.as(gtk.Widget));
        row.common.root.append(section("Remote ADB through target").as(gtk.Widget));
        row.remote_host.setPlaceholderText("Remote ADB host");
        row.common.root.append(row.remote_host.as(gtk.Widget));
        row.common.root.append(row.remote_port.as(gtk.Widget));
        row.common.root.append(row.scrcpy_arguments.as(gtk.Widget));
        row.common.root.append(section("Advanced").as(gtk.Widget));
        row.common.root.append(row.allow_non_loopback.as(gtk.Widget));
        const actions = gtk.Box.new(.horizontal, 6);
        const remove = gtk.Button.newWithLabel("Remove");
        actions.append(mirror.as(gtk.Widget));
        actions.append(restart.as(gtk.Widget));
        actions.append(log_button.as(gtk.Widget));
        actions.append(remove.as(gtk.Widget));
        row.common.root.append(actions.as(gtk.Widget));
        setAndroidActionAvailability(row, projected);
        connect(mirror, AndroidRow, androidMirrorClicked, row);
        connect(restart, AndroidRow, androidRestartClicked, row);
        connect(log_button, AndroidRow, androidLogClicked, row);
        connect(remove, AndroidRow, androidRemoveClicked, row);
        self.watchEntry(row.bind_address);
        self.watchEntry(row.remote_host);
        self.watchEntry(row.scrcpy_arguments);
        self.watchToggle(row.allow_non_loopback);
        self.watchSpin(row.local_port);
        self.watchSpin(row.remote_port);
        try self.android.append(self.allocator, row);
        return row;
    }

    fn updateStatuses(self: *Panel, projection: *const resource.Projection) void {
        for (self.files.items) |row| {
            updateStatus(self.allocator, &row.common, projection);
            setFileActionAvailability(row, projection.entry(row.common.id));
        }
        for (self.ports.items) |row| {
            updateStatus(self.allocator, &row.common, projection);
            setPortActionAvailability(row, projection.entry(row.common.id));
        }
        for (self.android.items) |row| {
            updateStatus(self.allocator, &row.common, projection);
            setAndroidActionAvailability(row, projection.entry(row.common.id));
        }
    }

    fn clearRows(self: *Panel) void {
        for (self.navigation_rows.items) |row| self.allocator.destroy(row);
        self.navigation_rows.clearRetainingCapacity();
        for (self.files.items) |row| {
            self.allocator.free(row.common.id);
            self.allocator.free(row.applied_local_path);
            self.allocator.destroy(row);
        }
        for (self.ports.items) |row| {
            self.allocator.free(row.common.id);
            self.allocator.destroy(row);
        }
        for (self.android.items) |row| {
            self.allocator.free(row.common.id);
            self.allocator.destroy(row);
        }
        self.files.clearRetainingCapacity();
        self.ports.clearRetainingCapacity();
        self.android.clearRetainingCapacity();
    }

    fn watchEntry(self: *Panel, value: *gtk.Entry) void {
        _ = gtk.Editable.signals.changed.connect(value.as(gtk.Editable), *Panel, entryChanged, self, .{});
    }
    fn watchToggle(self: *Panel, value: *gtk.ToggleButton) void {
        _ = gtk.ToggleButton.signals.toggled.connect(value, *Panel, toggleChanged, self, .{});
    }
    fn watchSpin(self: *Panel, value: *gtk.SpinButton) void {
        _ = gtk.SpinButton.signals.value_changed.connect(value, *Panel, spinChanged, self, .{});
    }
    fn entryChanged(_: *gtk.Editable, self: *Panel) callconv(.c) void {
        self.fieldsChanged();
    }
    fn toggleChanged(_: *gtk.ToggleButton, self: *Panel) callconv(.c) void {
        self.fieldsChanged();
    }
    fn spinChanged(_: *gtk.SpinButton, self: *Panel) callconv(.c) void {
        self.fieldsChanged();
    }
    fn fieldsChanged(self: *Panel) void {
        if (!self.rendering) self.editorAction(.{ .type = "keep_editing" }, false);
    }
    fn syncDraft(self: *Panel) void {
        if (self.revision == null) return;
        var files = std.ArrayList(resource.FileMapping).empty;
        defer files.deinit(self.allocator);
        var ports = std.ArrayList(resource.PortForward).empty;
        defer ports.deinit(self.allocator);
        var android = std.ArrayList(resource.AndroidMirror).empty;
        defer android.deinit(self.allocator);
        var scratch = std.heap.ArenaAllocator.init(self.allocator);
        defer scratch.deinit();
        for (self.files.items) |row| if (!row.common.removed) {
            files.append(self.allocator, .{
                .id = row.common.id,
                .name = text(row.common.name),
                .enabled = row.common.enabled.getActive() != 0,
                .execution_target_id = text(row.common.target),
                .remote_path = text(row.remote_path),
                .local_path = text(row.local_path),
            }) catch return;
        };
        for (self.ports.items) |row| if (!row.common.removed) {
            ports.append(self.allocator, .{
                .id = row.common.id,
                .name = text(row.common.name),
                .enabled = row.common.enabled.getActive() != 0,
                .execution_target_id = text(row.common.target),
                .bind_address = text(row.bind_address),
                .allow_non_loopback = row.allow_non_loopback.getActive() != 0,
                .local_port_start = @intCast(row.local_start.getValueAsInt()),
                .local_port_end = @intCast(row.local_end.getValueAsInt()),
                .remote_host = text(row.remote_host),
                .remote_port_start = @intCast(row.remote_start.getValueAsInt()),
            }) catch return;
        };
        for (self.android.items) |row| if (!row.common.removed) {
            var split_args = std.ArrayList([]const u8).empty;
            var iterator = std.mem.tokenizeAny(u8, text(row.scrcpy_arguments), "\r\n");
            while (iterator.next()) |argument| {
                split_args.append(scratch.allocator(), argument) catch return;
            }
            android.append(self.allocator, .{
                .id = row.common.id,
                .name = text(row.common.name),
                .enabled = row.common.enabled.getActive() != 0,
                .execution_target_id = text(row.common.target),
                .bind_address = text(row.bind_address),
                .allow_non_loopback = row.allow_non_loopback.getActive() != 0,
                .local_adb_port = @intCast(row.local_port.getValueAsInt()),
                .remote_adb_host = text(row.remote_host),
                .remote_adb_port = @intCast(row.remote_port.getValueAsInt()),
                .scrcpy_arguments = split_args.items,
            }) catch return;
        };
        var result = self.editor.dispatch(self.allocator, .{ .type = "change", .catalog = .{
            .schema_version = 1,
            .revision = self.revision orelse return,
            .file_mappings = files.items,
            .port_forwards = ports.items,
            .android_mirrors = android.items,
        } }) orelse return;
        result.deinit();
    }

    fn reloadClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.editorAction(.{ .type = "reload" }, false);
    }
    fn doneClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.editorAction(.{ .type = "close" }, false);
    }
    fn applyClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.editorAction(.{ .type = "apply" }, false);
    }
    fn addFileClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.addResource("file_mapping");
    }
    fn addPortClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.addResource("port_forward");
    }
    fn addAndroidClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.addResource("android_mirror");
    }
    fn addResource(self: *Panel, kind: []const u8) void {
        const projection = if (self.projection_fn) |get| get(self.action_context) else null;
        const target = if (projection) |current| if (current.available_target_ids.len > 0) current.available_target_ids[0] else "" else "";
        self.editorAction(.{ .type = "add", .kind = kind, .execution_target_id = target }, true);
    }
};

fn updateStatus(
    allocator: std.mem.Allocator,
    row: *CommonRow,
    projection: *const resource.Projection,
) void {
    const projected = projection.entry(row.id) orelse return;
    setLabel(allocator, row.status, projected.status.message) catch {};
}

fn setFileActionAvailability(row: *FileRow, projected: ?*const resource.Entry) void {
    if (!row.owner.backend_available) {
        row.reveal.as(gtk.Widget).setSensitive(0);
        row.restart.as(gtk.Widget).setSensitive(0);
        row.log_button.as(gtk.Widget).setSensitive(0);
        return;
    }
    const available = projected orelse {
        row.reveal.as(gtk.Widget).setSensitive(0);
        row.restart.as(gtk.Widget).setSensitive(0);
        row.log_button.as(gtk.Widget).setSensitive(0);
        return;
    };
    row.reveal.as(gtk.Widget).setSensitive(
        @intFromBool(available.can_reveal_local_path),
    );
    row.restart.as(gtk.Widget).setSensitive(
        @intFromBool(available.status.can_restart),
    );
    row.log_button.as(gtk.Widget).setSensitive(
        @intFromBool(available.can_open_log),
    );
}

fn setPortActionAvailability(row: *PortRow, projected: ?*const resource.Entry) void {
    if (!row.owner.backend_available) {
        row.restart.as(gtk.Widget).setSensitive(0);
        row.log_button.as(gtk.Widget).setSensitive(0);
        return;
    }
    const available = projected orelse {
        row.restart.as(gtk.Widget).setSensitive(0);
        row.log_button.as(gtk.Widget).setSensitive(0);
        return;
    };
    row.restart.as(gtk.Widget).setSensitive(
        @intFromBool(available.status.can_restart),
    );
    row.log_button.as(gtk.Widget).setSensitive(
        @intFromBool(available.can_open_log),
    );
}

fn setAndroidActionAvailability(
    row: *AndroidRow,
    projected: ?*const resource.Entry,
) void {
    if (!row.owner.backend_available) {
        row.mirror.as(gtk.Widget).setSensitive(0);
        row.restart.as(gtk.Widget).setSensitive(0);
        row.log_button.as(gtk.Widget).setSensitive(0);
        return;
    }
    const available = projected orelse {
        row.mirror.as(gtk.Widget).setSensitive(0);
        row.restart.as(gtk.Widget).setSensitive(0);
        row.log_button.as(gtk.Widget).setSensitive(0);
        return;
    };
    row.mirror.as(gtk.Widget).setSensitive(
        @intFromBool(available.status.can_open_mirror),
    );
    row.restart.as(gtk.Widget).setSensitive(
        @intFromBool(available.status.can_restart),
    );
    row.log_button.as(gtk.Widget).setSensitive(
        @intFromBool(available.can_open_log),
    );
}

pub fn openLocalPath(allocator: std.mem.Allocator, parent: *gtk.Window, path: []const u8, reveal: bool, slot: file_launcher.Slot, id: []const u8, context: ?*anyopaque, callback: file_launcher.Callback) !void {
    const expanded = try expandHome(allocator, path);
    defer allocator.free(expanded);
    try file_launcher.start(slot, parent, expanded, reveal, id, context, callback);
}
fn resourceLaunchFinished(raw: ?*anyopaque, _: []const u8, message: ?[]const u8, cancelled: bool) void {
    const owner: *Panel = @ptrCast(@alignCast(raw orelse return));
    if (!cancelled) if (message) |error_message| owner.showError(error_message);
}
fn fileRevealClicked(_: *gtk.Button, row: *FileRow) callconv(.c) void {
    const parent = auxiliary.contentParent(row.owner.app,row.owner.editor_root orelse return) orelse return;
    openLocalPath(row.owner.allocator, parent, row.applied_local_path, false, .resources, "", row.owner, resourceLaunchFinished) catch row.owner.showError("Could not start the system file launcher.");
}

fn fileRestartClicked(_: *gtk.Button, row: *FileRow) callconv(.c) void {
    row.owner.action_fn(row.owner.action_context, .{ .restart = row.common.id });
}
fn fileLogClicked(_: *gtk.Button, row: *FileRow) callconv(.c) void {
    row.owner.action_fn(row.owner.action_context, .{ .get_log = row.common.id });
}
fn fileRemoveClicked(_: *gtk.Button, row: *FileRow) callconv(.c) void {
    row.common.removed = true;
    row.common.root.as(gtk.Widget).setVisible(0);
    row.owner.fieldsChanged();
}
fn portRestartClicked(_: *gtk.Button, row: *PortRow) callconv(.c) void {
    row.owner.action_fn(row.owner.action_context, .{ .restart = row.common.id });
}
fn portLogClicked(_: *gtk.Button, row: *PortRow) callconv(.c) void {
    row.owner.action_fn(row.owner.action_context, .{ .get_log = row.common.id });
}
fn portRangeChanged(_: *gtk.SpinButton, row: *PortRow) callconv(.c) void {
    updatePortRange(row);
}
fn updatePortRange(row: *PortRow) void {
    const local_start = row.local_start.getValueAsInt();
    const local_end = row.local_end.getValueAsInt();
    const remote_start = row.remote_start.getValueAsInt();
    if (local_end < local_start or remote_start + (local_end - local_start) > 65535) {
        setLabel(row.owner.allocator, row.remote_range, "Remote range: Invalid range") catch {};
        return;
    }
    const remote_end = remote_start + (local_end - local_start);
    const message = if (local_start == local_end)
        std.fmt.allocPrint(row.owner.allocator, "Remote range: {d}", .{remote_start})
    else
        std.fmt.allocPrint(
            row.owner.allocator,
            "Remote range: {d}–{d}",
            .{ remote_start, remote_end },
        );
    const owned = message catch return;
    defer row.owner.allocator.free(owned);
    setLabel(row.owner.allocator, row.remote_range, owned) catch {};
}
fn portRemoveClicked(_: *gtk.Button, row: *PortRow) callconv(.c) void {
    row.common.removed = true;
    row.common.root.as(gtk.Widget).setVisible(0);
    row.owner.fieldsChanged();
}
fn androidMirrorClicked(_: *gtk.Button, row: *AndroidRow) callconv(.c) void {
    row.owner.action_fn(row.owner.action_context, .{ .open_mirror = row.common.id });
}
fn androidRestartClicked(_: *gtk.Button, row: *AndroidRow) callconv(.c) void {
    row.owner.action_fn(row.owner.action_context, .{ .restart = row.common.id });
}
fn androidLogClicked(_: *gtk.Button, row: *AndroidRow) callconv(.c) void {
    row.owner.action_fn(row.owner.action_context, .{ .get_log = row.common.id });
}
fn androidRemoveClicked(_: *gtk.Button, row: *AndroidRow) callconv(.c) void {
    row.common.removed = true;
    row.common.root.as(gtk.Widget).setVisible(0);
    row.owner.fieldsChanged();
}

fn navigationClicked(_: *gtk.Button, row: *NavigationRow) callconv(.c) void {
    row.owner.selectDetail(row.id);
}

fn section(title_text: [*:0]const u8) *gtk.Box {
    const result = gtk.Box.new(.vertical, 8);
    result.append(label(title_text, "heading").as(gtk.Widget));
    return result;
}

fn toggle(title_text: [*:0]const u8, active: bool) *gtk.ToggleButton {
    const result = gtk.ToggleButton.new();
    result.as(gtk.Button).setChild(label(title_text, "caption").as(gtk.Widget));
    result.setActive(@intFromBool(active));
    return result;
}

fn configurePortSpinButton(spin: *gtk.SpinButton) *gtk.SpinButton {
    spin.setNumeric(1);
    spin.setDigits(0);
    return spin;
}

fn entry(allocator: std.mem.Allocator, value: []const u8) !*gtk.Entry {
    const result = gtk.Entry.new();
    const owned = try allocator.dupeZ(u8, value);
    defer allocator.free(owned);
    result.as(gtk.Editable).setText(owned);
    return result;
}

fn text(value: *gtk.Entry) []const u8 {
    return std.mem.span(value.as(gtk.Editable).getText());
}

fn label(value: [*:0]const u8, css_class: [*:0]const u8) *gtk.Label {
    const result = gtk.Label.new(value);
    result.setXalign(0);
    result.setWrap(1);
    result.as(gtk.Widget).addCssClass(css_class);
    return result;
}

fn setLabel(
    allocator: std.mem.Allocator,
    target: *gtk.Label,
    value: []const u8,
) !void {
    const owned = try allocator.dupeZ(u8, value);
    defer allocator.free(owned);
    target.setLabel(owned);
}

fn setMargins(widget: *gtk.Widget, value: c_int) void {
    widget.setMarginTop(value);
    widget.setMarginBottom(value);
    widget.setMarginStart(value);
    widget.setMarginEnd(value);
}

fn clearBox(box: *gtk.Box) void {
    while (box.as(gtk.Widget).getFirstChild()) |child| box.remove(child);
}

fn connect(
    button: *gtk.Button,
    comptime Context: type,
    callback: *const fn (*gtk.Button, *Context) callconv(.c) void,
    context: *Context,
) void {
    _ = gtk.Button.signals.clicked.connect(button, *Context, callback, context, .{});
}

fn expandHome(allocator: std.mem.Allocator, path: []const u8) ![]u8 {
    if (std.mem.eql(u8, path, "~")) {
        const home = std.posix.getenv("HOME") orelse return error.HomeUnavailable;
        return allocator.dupe(u8, home);
    }
    if (std.mem.startsWith(u8, path, "~/")) {
        const home = std.posix.getenv("HOME") orelse return error.HomeUnavailable;
        return std.fs.path.join(allocator, &.{ home, path[2..] });
    }
    return allocator.dupe(u8, path);
}
