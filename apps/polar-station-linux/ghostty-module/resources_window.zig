//! One process-wide native GTK Resources editor.

const auxiliary = @import("auxiliary_window.zig");
const std = @import("std");
const adw = @import("adw");
const gio = @import("gio");
const glib = @import("glib");
const gobject = @import("gobject");
const gtk = @import("gtk");
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
    owner: *Window,
    id: []const u8,
    detail: *gtk.Box,
};

const FileRow = struct {
    owner: *Window,
    common: CommonRow,
    remote_path: *gtk.Entry,
    local_path: *gtk.Entry,
    applied_local_path: []u8,
    reveal: *gtk.Button,
    restart: *gtk.Button,
    log_button: *gtk.Button,
};

const PortRow = struct {
    owner: *Window,
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
    owner: *Window,
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

pub const Window = struct {
    allocator: std.mem.Allocator,
    app: *Application,
    action_fn: ActionFn,
    action_context: ?*anyopaque,
    window: ?*adw.Window = null,
    log_window: ?*adw.Window = null,
    log_body: ?*gtk.Box = null,
    body: ?*gtk.Box = null,
    navigation: ?*gtk.Box = null,
    state_label: ?*gtk.Label = null,
    conflict_label: ?*gtk.Label = null,
    reload_button: ?*gtk.Button = null,
    apply_button: ?*gtk.Button = null,
    file_section: ?*gtk.Box = null,
    port_section: ?*gtk.Box = null,
    android_section: ?*gtk.Box = null,
    revision: ?u64 = null,
    force_reload: bool = false,
    backend_available: bool = false,
    files: std.ArrayList(*FileRow) = .empty,
    ports: std.ArrayList(*PortRow) = .empty,
    android: std.ArrayList(*AndroidRow) = .empty,
    navigation_rows: std.ArrayList(*NavigationRow) = .empty,

    pub fn init(
        allocator: std.mem.Allocator,
        app: *Application,
        action_fn: ActionFn,
        action_context: ?*anyopaque,
    ) Window {
        return .{
            .allocator = allocator,
            .app = app,
            .action_fn = action_fn,
            .action_context = action_context,
        };
    }

    pub fn deinit(self: *Window) void {
        if (self.window) |window| auxiliary.destroy(window);
        if (self.log_window) |window| auxiliary.destroy(window);
        self.clearRows();
        self.files.deinit(self.allocator);
        self.ports.deinit(self.allocator);
        self.android.deinit(self.allocator);
        self.navigation_rows.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn present(self: *Window, projection: ?*const resource.Projection) void {
        self.ensureWindow();
        if (projection) |value| self.render(value, false);
        self.applyBackendAvailability();
        auxiliary.present(self.app, self.window.?);
    }

    pub fn prefillFile(self:*Window,target:[]const u8,remote:[]const u8) void {
        self.ensureWindow();
        const id=newId(self.allocator) catch return;defer self.allocator.free(id);
        const local=std.fmt.allocPrint(self.allocator,"~/Polar/remote/{s}",.{id}) catch return;defer self.allocator.free(local);
        const row=self.createFileRow(.{.id=id,.name="File Mapping",.enabled=true,.execution_target_id=target,.remote_path=remote,.local_path=local},null) catch return;
        var selected=self.navigation_rows.items.len!=0;self.addNavigation(self.file_section.?,&row.common,&selected) catch return;self.selectDetail(row.common.id);
        auxiliary.present(self.app,self.window.?);
    }
    pub fn setBackendAvailable(self: *Window, available: bool) void {
        self.backend_available = available;
        if (self.window == null) return;
        self.applyBackendAvailability();
    }

    pub fn render(
        self: *Window,
        projection: *const resource.Projection,
        applied: bool,
    ) void {
        self.ensureWindow();
        setLabel(self.allocator, self.state_label.?, projection.aggregate_message) catch {};
        if (!applied and !self.force_reload and self.revision != null and
            projection.revision != self.revision.?)
        {
            setLabel(
                self.allocator,
                self.conflict_label.?,
                "The Host catalog changed. This draft is preserved; Reload discards it.",
            ) catch {};
            self.conflict_label.?.as(gtk.Widget).setVisible(1);
            self.updateStatuses(projection);
            return;
        }
        if (!applied and !self.force_reload and self.revision != null and projection.revision == self.revision.?) {
            self.updateStatuses(projection);
            return;
        }
        self.force_reload = false;
        self.revision = projection.revision;
        self.conflict_label.?.as(gtk.Widget).setVisible(0);
        self.clearRows();
        clearBox(self.body.?);
        clearBox(self.navigation.?);
        self.buildCatalog(projection) catch return;
    }

    pub fn showError(self: *Window, message: []const u8) void {
        self.ensureWindow();
        setLabel(self.allocator, self.conflict_label.?, message) catch return;
        self.conflict_label.?.as(gtk.Widget).setVisible(1);
    }

    pub fn showLog(self: *Window, tail: *const resource.LogTail) void {
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

    fn ensureWindow(self: *Window) void {
        if (self.window != null) return;
        const window = auxiliary.create(self.app);
        window.as(gtk.Window).setTitle("Global Resources");
        _ = gtk.Window.signals.close_request.connect(window.as(gtk.Window), *Window, closeRequested, self, .{});
        window.as(gtk.Window).setIconName("com.polarealm.PolarStation");
        window.as(gtk.Window).setDefaultSize(1000, 700);
        window.as(gtk.Window).setHideOnClose(1);
        const root = gtk.Box.new(.vertical, 0);
        root.as(gtk.Widget).setSizeRequest(760, 520);
        const header = gtk.Box.new(.horizontal, 10);
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
        paned.as(gtk.Widget).setVexpand(1);
        paned.setPosition(260);
        paned.setResizeStartChild(0);
        paned.setShrinkStartChild(0);
        const navigation_scroll = gtk.ScrolledWindow.new();
        // Category controls define the sidebar minimum; never hide a category
        // behind horizontal scrolling when the splitter starts at 260px.
        navigation_scroll.setPolicy(.never, .automatic);
        navigation_scroll.as(gtk.Widget).setSizeRequest(220, -1);
        const navigation = gtk.Box.new(.vertical, 8);
        setMargins(navigation.as(gtk.Widget), 12);
        navigation_scroll.setChild(navigation.as(gtk.Widget));
        const scroll = gtk.ScrolledWindow.new();
        scroll.as(gtk.Widget).setVexpand(1);
        const body = gtk.Box.new(.vertical, 12);
        setMargins(body.as(gtk.Widget), 20);
        scroll.setChild(body.as(gtk.Widget));
        paned.setStartChild(navigation_scroll.as(gtk.Widget));
        paned.setEndChild(scroll.as(gtk.Widget));
        const footer = gtk.Box.new(.horizontal, 8);
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
        root.append(paned.as(gtk.Widget));
        root.append(footer.as(gtk.Widget));
        const toolbar = adw.ToolbarView.new();
        toolbar.addTopBar(adw.HeaderBar.new().as(gtk.Widget));
        toolbar.setContent(root.as(gtk.Widget));
        window.setContent(toolbar.as(gtk.Widget));
        connect(reload, Window, reloadClicked, self);
        connect(done, Window, doneClicked, self);
        connect(apply, Window, applyClicked, self);
        self.window = window;
        self.body = body;
        self.navigation = navigation;
        self.state_label = state;
        self.conflict_label = conflict;
        self.reload_button = reload;
        self.apply_button = apply;
        self.applyBackendAvailability();
    }

    fn applyBackendAvailability(self: *Window) void {
        const enabled = @intFromBool(self.backend_available);
        if (self.reload_button) |button| {
            button.as(gtk.Widget).setSensitive(enabled);
        }
        if (self.apply_button) |button| {
            button.as(gtk.Widget).setSensitive(enabled);
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

    fn buildCatalog(self: *Window, projection: *const resource.Projection) !void {
        const type_switcher = gtk.StackSwitcher.new();
        const type_stack = gtk.Stack.new();
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
        for (projection.catalog.file_mappings) |value| {
            const row = try self.createFileRow(value, projection.entry(value.id));
            try self.addNavigation(files, &row.common, &selected);
        }
        for (projection.catalog.android_mirrors) |value| {
            const row = try self.createAndroidRow(value, projection.entry(value.id));
            try self.addNavigation(android, &row.common, &selected);
        }
        for (projection.catalog.port_forwards) |value| {
            const row = try self.createPortRow(value, projection.entry(value.id));
            try self.addNavigation(ports, &row.common, &selected);
        }
        const add_file = gtk.Button.newWithLabel("+ Add File Mapping");
        const add_android = gtk.Button.newWithLabel("+ Add Android Mirror");
        const add_port = gtk.Button.newWithLabel("+ Add Port Forward");
        files.append(add_file.as(gtk.Widget));
        android.append(add_android.as(gtk.Widget));
        ports.append(add_port.as(gtk.Widget));
        connect(add_file, Window, addFileClicked, self);
        connect(add_android, Window, addAndroidClicked, self);
        connect(add_port, Window, addPortClicked, self);
    }

    fn addNavigation(
        self: *Window,
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

    fn selectDetail(self: *Window, selected_id: []const u8) void {
        for (self.navigation_rows.items) |row| {
            row.detail.as(gtk.Widget).setVisible(
                @intFromBool(std.mem.eql(u8, row.id, selected_id)),
            );
        }
    }

    fn createCommon(
        self: *Window,
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
        self: *Window,
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
        try self.files.append(self.allocator, row);
        return row;
    }

    fn createPortRow(
        self: *Window,
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
        row.common.root.append(section("This Mac").as(gtk.Widget));
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
        try self.ports.append(self.allocator, row);
        return row;
    }

    fn createAndroidRow(
        self: *Window,
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
        try self.android.append(self.allocator, row);
        return row;
    }

    fn updateStatuses(self: *Window, projection: *const resource.Projection) void {
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

    fn clearRows(self: *Window) void {
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

    fn submitApply(self: *Window) void {
        var files = std.ArrayList(resource.FileMapping).empty;
        defer files.deinit(self.allocator);
        var ports = std.ArrayList(resource.PortForward).empty;
        defer ports.deinit(self.allocator);
        var android = std.ArrayList(resource.AndroidMirror).empty;
        defer android.deinit(self.allocator);
        var split_args = std.ArrayList([]const u8).empty;
        defer split_args.deinit(self.allocator);
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
            const first = split_args.items.len;
            var iterator = std.mem.tokenizeAny(u8, text(row.scrcpy_arguments), "\r\n");
            while (iterator.next()) |argument| {
                split_args.append(self.allocator, argument) catch return;
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
                .scrcpy_arguments = split_args.items[first..],
            }) catch return;
        };
        self.action_fn(self.action_context, .{ .apply = .{
            .schema_version = 1,
            .revision = self.revision orelse return,
            .file_mappings = files.items,
            .port_forwards = ports.items,
            .android_mirrors = android.items,
        } });
    }

    fn reloadClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        self.force_reload = true;
        self.action_fn(self.action_context, .reload);
    }
    fn doneClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        self.action_fn(self.action_context,.cancel_file_open);
        self.window.?.as(gtk.Widget).setVisible(0);
    }
    fn closeRequested(_: *gtk.Window, self: *Window) callconv(.c) c_int {
        self.action_fn(self.action_context, .cancel_file_open);
        return 0;
    }
    fn applyClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        self.submitApply();
    }
    fn addFileClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        const value = resource.FileMapping{
            .id = newId(self.allocator) catch return,
            .name = "File Mapping",
            .enabled = false,
            .execution_target_id = "",
            .remote_path = "/home/operator",
            .local_path = "~/Polar/remote",
        };
        defer self.allocator.free(value.id);
        const row = self.createFileRow(value, null) catch return;
        var selected = self.navigation_rows.items.len != 0;
        self.addNavigation(self.file_section.?, &row.common, &selected) catch return;
        self.selectDetail(row.common.id);
    }
    fn addPortClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        const value = resource.PortForward{
            .id = newId(self.allocator) catch return,
            .name = "Port Forward",
            .enabled = false,
            .execution_target_id = "",
            .bind_address = "127.0.0.1",
            .allow_non_loopback = false,
            .local_port_start = 8080,
            .local_port_end = 8080,
            .remote_host = "127.0.0.1",
            .remote_port_start = 8080,
        };
        defer self.allocator.free(value.id);
        const row = self.createPortRow(value, null) catch return;
        var selected = self.navigation_rows.items.len != 0;
        self.addNavigation(self.port_section.?, &row.common, &selected) catch return;
        self.selectDetail(row.common.id);
    }
    fn addAndroidClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        const value = resource.AndroidMirror{
            .id = newId(self.allocator) catch return,
            .name = "Android Mirror",
            .enabled = false,
            .execution_target_id = "",
            .bind_address = "127.0.0.1",
            .allow_non_loopback = false,
            .local_adb_port = 5555,
            .remote_adb_host = "127.0.0.1",
            .remote_adb_port = 5555,
            .scrcpy_arguments = &.{},
        };
        defer self.allocator.free(value.id);
        const row = self.createAndroidRow(value, null) catch return;
        var selected = self.navigation_rows.items.len != 0;
        self.addNavigation(self.android_section.?, &row.common, &selected) catch return;
        self.selectDetail(row.common.id);
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

pub fn revealLocalFile(allocator:std.mem.Allocator,path:[]const u8) bool {
    const expanded=expandHome(allocator,path) catch return false;defer allocator.free(expanded);
    const parent=std.fs.path.dirname(expanded) orelse return false;
    return openLocalPath(allocator,parent);
}
pub fn openLocalPath(allocator:std.mem.Allocator,path:[]const u8) bool {
    const expanded=expandHome(allocator,path) catch return false;defer allocator.free(expanded);
    const owned=allocator.dupeZ(u8,expanded) catch return false;defer allocator.free(owned);
    const file=gio.File.newForPath(owned);defer @as(*gobject.Object,@ptrCast(@alignCast(file))).unref();
    const uri=file.getUri();defer glib.free(uri);
    return gio.AppInfo.launchDefaultForUri(uri,null,null)!=0;
}
fn fileRevealClicked(_: *gtk.Button,row:*FileRow) callconv(.c) void {if(!openLocalPath(row.owner.allocator,row.applied_local_path))row.owner.showError("The system viewer could not open this file.");}

fn fileRestartClicked(_: *gtk.Button, row: *FileRow) callconv(.c) void {
    row.owner.action_fn(row.owner.action_context, .{ .restart = row.common.id });
}
fn fileLogClicked(_: *gtk.Button, row: *FileRow) callconv(.c) void {
    row.owner.action_fn(row.owner.action_context, .{ .get_log = row.common.id });
}
fn fileRemoveClicked(_: *gtk.Button, row: *FileRow) callconv(.c) void {
    row.common.removed = true;
    row.common.root.as(gtk.Widget).setVisible(0);
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

fn newId(allocator: std.mem.Allocator) ![]u8 {
    var bytes: [16]u8 = undefined;
    std.crypto.random.bytes(&bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    return std.fmt.allocPrint(
        allocator,
        "{x:0>2}{x:0>2}{x:0>2}{x:0>2}-{x:0>2}{x:0>2}-{x:0>2}{x:0>2}-" ++
            "{x:0>2}{x:0>2}-{x:0>2}{x:0>2}{x:0>2}{x:0>2}{x:0>2}{x:0>2}",
        .{
            bytes[0],  bytes[1],  bytes[2],  bytes[3],  bytes[4],  bytes[5],
            bytes[6],  bytes[7],  bytes[8],  bytes[9],  bytes[10], bytes[11],
            bytes[12], bytes[13], bytes[14], bytes[15],
        },
    );
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
