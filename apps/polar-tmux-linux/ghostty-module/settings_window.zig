//! One process-wide GTK Settings window. Rust owns persistence and validation;
//! this adapter owns only native controls and typed user intent.
const auxiliary = @import("auxiliary_window.zig");
const std = @import("std");
const adw = @import("adw");
const gobject = @import("gobject");
const theme = @import("theme.zig");
const gtk = @import("gtk");
const gdk = @import("gdk");
const Application = @import("../class/application.zig").Application;
const settings = @import("settings.zig");
const voice_settings = @import("voice_settings.zig");

pub const Action = union(enum) {
    reload,
    appearance: settings.Appearance,
    attention: settings.Attention,
    monitoring: settings.Monitoring,
    session_summary: settings.SessionSummary,
    test_summary: settings.SessionSummary,
    inspect_codex_notify,
    configure_codex_notify,
    open_workspace_font,
    open_connections,
    open_resources,
    open_codex,
    open_connectors,
    voice_changed,
    voice_shortcuts,
    dictate_recover: []const u8,
    dictate_deleted: []const u8,
};
pub const ActionFn = *const fn (?*anyopaque, Action) void;

const ThemeChoice = struct { owner: *Window, index: usize };
pub const Window = struct {
    voice: ?*voice_settings.Panel = null,
    pages: ?*gtk.Stack = null,
    theme_projection: ?settings.ThemeProjection = null,
    appearance_value: settings.Appearance = .{},
    rendering: bool = false,
    theme_follow: ?*gtk.CheckButton = null,
    theme_preview_css: [6]?*gtk.CssProvider = .{null} ** 6,
    theme_buttons: [6]?*gtk.Button = .{null} ** 6,
    theme_contexts: [6]ThemeChoice = undefined,
    theme_list: std.ArrayListUnmanaged([:0]u8) = .empty,
    theme_dropdown: ?*gtk.DropDown = null,
    theme_error: ?*gtk.Label = null,
    chat_modified: bool = false,
    allocator: std.mem.Allocator,
    app: *Application,
    action_fn: ActionFn,
    action_context: ?*anyopaque,
    window: ?*adw.Window = null,
    state: ?*gtk.Label = null,
    reload_button: ?*gtk.Button = null,
    adaptive_width: ?*gtk.ToggleButton = null,
    interface_text_scale: ?*gtk.SpinButton = null,
    chat_message_cache: ?*gtk.SpinButton = null,
    chat_presentation_cache: ?*gtk.SpinButton = null,
    width: ?*gtk.SpinButton = null,
    layout_visible: ?*gtk.ToggleButton = null,
    inbox_visible: ?*gtk.ToggleButton = null,
    inbox_width: ?*gtk.SpinButton = null,
    send_on_approve: ?*gtk.ToggleButton = null,
    save_width: ?*gtk.Button = null,
    monitoring_enabled: ?*gtk.ToggleButton = null,
    monitoring_codex_titles: ?*gtk.ToggleButton = null,
    monitoring_quiet_period: ?*gtk.SpinButton = null,
    save_monitoring: ?*gtk.Button = null,
    summary_enabled: ?*gtk.ToggleButton = null,
    summary_command: ?*gtk.Entry = null,
    summary_model: ?*gtk.Entry = null,
    summary_timeout: ?*gtk.SpinButton = null,
    summary_quiet_period: ?*gtk.SpinButton = null,
    save_summary: ?*gtk.Button = null,
    test_summary: ?*gtk.Button = null,
    codex_notify_status: ?*gtk.Label = null,
    inspect_codex_notify: ?*gtk.Button = null,
    configure_codex_notify: ?*gtk.Button = null,
    notifications_enabled: ?*gtk.ToggleButton = null,
    save_notifications: ?*gtk.Button = null,
    backend_available: bool = false,

    fn voiceAction(context: ?*anyopaque, shortcut: bool) void {
        const self: *Window = @ptrCast(@alignCast(context orelse return));
        self.action_fn(self.action_context, if (shortcut) .voice_shortcuts else .voice_changed);
    }
    fn terminologyClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        if (self.pages) |pages| pages.setVisibleChildName("terminology");
    }
    fn recordingDeleted(context: ?*anyopaque, id: []const u8) void {
        const self: *Window = @ptrCast(@alignCast(context orelse return));
        self.action_fn(self.action_context, .{ .dictate_deleted = id });
    }
    fn recoverRecording(context: ?*anyopaque, id: []const u8) void {
        const self: *Window = @ptrCast(@alignCast(context orelse return));
        if (self.voice) |panel| panel.cancelTests();
        if (self.window) |window| window.as(gtk.Widget).setVisible(0);
        self.action_fn(self.action_context, .{ .dictate_recover = id });
    }
    pub fn init(allocator: std.mem.Allocator, app: *Application, action_fn: ActionFn, context: ?*anyopaque) Window {
        return .{ .allocator = allocator, .app = app, .action_fn = action_fn, .action_context = context };
    }
    pub fn deinit(self: *Window) void {
        if (self.window) |window| auxiliary.destroy(window);
        if (self.voice) |panel| panel.destroy();
        for (self.theme_preview_css) |provider| if (provider) |css| css.unref();
        for (self.theme_list.items) |name| self.allocator.free(name);
        self.theme_list.deinit(self.allocator);
        self.* = undefined;
    }
    pub fn present(self: *Window, projection: ?*const settings.Projection) void {
        self.ensureWindow();
        if (projection) |value| self.render(value);
        self.applyBackendAvailability();
        auxiliary.present(self.app, self.window.?);
    }
    pub fn presentVoice(self: *Window, projection: ?*const settings.Projection) void {
        self.present(projection);
        if (self.pages) |pages| pages.setVisibleChildName("voice");
    }
    pub fn setBackendAvailable(self: *Window, available: bool) void {
        self.backend_available = available;
        if (self.window != null) self.applyBackendAvailability();
    }
    pub fn render(self: *Window, projection: *const settings.Projection) void {
        self.rendering = true;
        defer self.rendering = false;
        self.appearance_value = projection.appearance;
        self.theme_projection = projection.theme_projection;
        self.chat_modified = std.mem.eql(u8, projection.appearance.chat_send_mode, "modified_enter");
        self.ensureWindow();
        self.theme_follow.?.setActive(@intFromBool(std.mem.eql(u8, projection.appearance.theme.mode, "system")));
        if (projection.theme_projection) |themes| {
            for (themes.choices, 0..) |choice, i| {
                if (i >= 6) break;
                const markup = std.fmt.allocPrintSentinel(self.allocator, "<b>{s}</b>\n<span foreground=\"{s}\" background=\"{s}\">  $ codex             </span>\n<span foreground=\"{s}\">━━</span> <span foreground=\"{s}\">━━</span> <span foreground=\"{s}\">━━</span>", .{ choice.name, choice.foreground, choice.background, choice.accent, choice.selection_background, choice.foreground }, 0) catch continue;
                defer self.allocator.free(markup);
                const label_widget: *gtk.Label = @ptrCast(self.theme_buttons[i].?.getChild().?);
                label_widget.setMarkup(markup);
                const selected_theme = std.mem.eql(u8, projection.appearance.theme.mode, "preset") and std.mem.eql(u8, projection.appearance.theme.preset, choice.id);
                const preview_css = std.fmt.allocPrintSentinel(self.allocator, "button {{ background-image: none; background-color: {s}; color: {s}; border: 2px solid {s}; border-radius: 8px; }} button:hover {{ border-color: {s}; }}", .{ choice.background, choice.foreground, if (selected_theme) choice.accent else choice.surface, choice.accent }, 0) catch continue;
                defer self.allocator.free(preview_css);
                self.theme_preview_css[i].?.loadFromString(preview_css);
            }
        }
        var selected: c_uint = 0;
        if (projection.appearance.theme.terminal_theme) |name| for (self.theme_list.items, 0..) |candidate, i| {
            if (std.mem.eql(u8, name, candidate)) {
                selected = @intCast(i + 1);
                break;
            }
        };
        self.theme_dropdown.?.setSelected(selected);
        const adaptive = projection.appearance.layout_map_width == null;
        self.adaptive_width.?.setActive(@intFromBool(adaptive));
        self.chat_message_cache.?.setValue(@floatFromInt(projection.appearance.lgp_cache.message_mib));
        self.chat_presentation_cache.?.setValue(@floatFromInt(projection.appearance.lgp_cache.presentation_mib));
        self.interface_text_scale.?.setValue(@floatFromInt(projection.appearance.interface_text_scale_percent));
        self.width.?.setValue(@floatFromInt(projection.appearance.layout_map_width orelse 300));
        self.layout_visible.?.setActive(@intFromBool(projection.appearance.layout_map_visible));
        self.inbox_visible.?.setActive(@intFromBool(projection.appearance.inbox_visible));
        self.inbox_width.?.setValue(@floatFromInt(projection.appearance.inbox_width orelse 340));
        self.send_on_approve.?.setActive(@intFromBool(std.mem.eql(u8, projection.appearance.terminal_delivery_mode, "send")));
        self.monitoring_enabled.?.setActive(@intFromBool(projection.monitoring.enabled));
        self.monitoring_codex_titles.?.setActive(@intFromBool(projection.monitoring.codex_title_detection));
        self.monitoring_quiet_period.?.setValue(@floatFromInt(projection.monitoring.quiet_period_ms / 1000));
        self.summary_enabled.?.setActive(@intFromBool(projection.session_summary.enabled));
        setEntry(self.allocator, self.summary_command.?, projection.session_summary.command) catch {};
        setEntry(self.allocator, self.summary_model.?, projection.session_summary.model orelse "") catch {};
        self.summary_timeout.?.setValue(@floatFromInt(projection.session_summary.timeout_ms / 1000));
        self.summary_quiet_period.?.setValue(@floatFromInt(projection.session_summary.quiet_period_ms / 1000));
        self.notifications_enabled.?.setActive(@intFromBool(projection.attention.system_notifications_enabled));
        self.applyBackendAvailability();
        setLabel(self.allocator, self.theme_error.?, "") catch {};
        setLabel(self.allocator, self.state.?, "Settings are synchronized with the local Rust owner.") catch {};
    }
    pub fn showError(self: *Window, message: []const u8) void {
        if (self.state) |label_widget| setLabel(self.allocator, label_widget, message) catch {};
    }

    pub fn renderCodexNotifyStatus(self: *Window, status: settings.CodexNotifyStatus) void {
        self.ensureWindow();
        const status_text = switch (status) {
            .missing => "Codex notify: Missing",
            .configured => "Codex notify: Configured",
            .conflict => "Codex notify: Conflict (existing command preserved)",
            .invalid => "Codex notify: Invalid TOML",
            .unsafe => "Codex notify: Unsafe path",
            .unsupported => "Codex notify: Unsupported target",
        };
        setLabel(self.allocator, self.codex_notify_status.?, status_text) catch {};
        self.configure_codex_notify.?.as(gtk.Widget).setSensitive(@intFromBool(self.backend_available and status == .missing));
    }

    fn ensureWindow(self: *Window) void {
        if (self.window != null) return;
        const window = auxiliary.create(self.app);
        window.as(gtk.Window).setTitle("Polar Station Settings");
        window.as(gtk.Window).setIconName("com.polarealm.PolarStation");
        window.as(gtk.Window).setDefaultSize(780, 620);
        window.as(gtk.Window).setHideOnClose(1);
        const keys = gtk.EventControllerKey.new();
        _ = gtk.EventControllerKey.signals.key_pressed.connect(keys, *Window, keyPressed, self, .{});
        window.as(gtk.Widget).addController(keys.as(gtk.EventController));
        _ = gobject.Object.signals.notify.connect(window.as(gobject.Object), *Window, activated, self, .{ .detail = "is-active" });
        _ = gtk.Widget.signals.unmap.connect(window.as(gtk.Widget), *Window, hidden, self, .{});
        _ = gtk.Window.signals.close_request.connect(window.as(gtk.Window), *Window, closeRequested, self, .{});

        const shell = gtk.Box.new(.vertical, 0);
        const header = gtk.Box.new(.horizontal, 10);
        setMargins(header.as(gtk.Widget), 16);
        header.append(label("Polar Station Settings", "title-2").as(gtk.Widget));
        const state_label = label("Loading Settings…", "caption");
        state_label.as(gtk.Widget).setHexpand(1);
        header.append(state_label.as(gtk.Widget));
        const reload = gtk.Button.newWithLabel("Reload");
        header.append(reload.as(gtk.Widget));
        const close = gtk.Button.newWithLabel("Close");
        header.append(close.as(gtk.Widget));
        shell.append(header.as(gtk.Widget));

        const scroll = gtk.ScrolledWindow.new();
        scroll.as(gtk.Widget).setVexpand(1);
        const body = gtk.Stack.new();
        self.pages = body;
        body.setVhomogeneous(0);
        const appearance_page = gtk.Box.new(.vertical, 14);
        setMargins(body.as(gtk.Widget), 18);

        const themes = section("Theme");
        const follow = gtk.CheckButton.newWithLabel("Follow system appearance");
        self.theme_follow = follow;
        _ = gtk.CheckButton.signals.toggled.connect(follow, *Window, themeFollowChanged, self, .{});
        themes.append(follow.as(gtk.Widget));
        for (0..2) |row| {
            const grid_row = gtk.Box.new(.horizontal, 10);
            grid_row.setHomogeneous(1);
            for (0..3) |col| {
                const index = row * 3 + col;
                const button = gtk.Button.new();
                const preview_css = gtk.CssProvider.new();
                self.theme_preview_css[index] = preview_css;
                button.as(gtk.Widget).getStyleContext().addProvider(preview_css.as(gtk.StyleProvider), gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 20);
                const swatch = gtk.Label.new("Loading theme…");
                swatch.setLines(3);
                button.setChild(swatch.as(gtk.Widget));
                button.as(gtk.Widget).setHexpand(1);
                button.as(gtk.Widget).setSizeRequest(-1, 88);
                self.theme_buttons[index] = button;
                self.theme_contexts[index] = .{ .owner = self, .index = index };
                _ = gtk.Button.signals.clicked.connect(button, *ThemeChoice, themeChosen, &self.theme_contexts[index], .{});
                grid_row.append(button.as(gtk.Widget));
            }
            themes.append(grid_row.as(gtk.Widget));
        }
        const advanced = gtk.Expander.new("Advanced terminal colors");
        self.theme_list = theme.themeNames(self.allocator) catch .empty;
        var names: std.ArrayListUnmanaged(?[*:0]const u8) = .empty;
        defer names.deinit(self.allocator);
        names.append(self.allocator, "Same as application") catch {};
        for (self.theme_list.items) |name| names.append(self.allocator, name.ptr) catch {};
        names.append(self.allocator, null) catch {};
        const dropdown = gtk.DropDown.newFromStrings(@ptrCast(names.items.ptr));
        dropdown.setEnableSearch(1);
        self.theme_dropdown = dropdown;
        _ = gobject.Object.signals.notify.connect(dropdown, *Window, terminalThemeChanged, self, .{ .detail = "selected" });
        advanced.setChild(dropdown.as(gtk.Widget));
        themes.append(advanced.as(gtk.Widget));
        self.theme_error = gtk.Label.new("");
        self.theme_error.?.setWrap(1);
        themes.append(self.theme_error.?.as(gtk.Widget));
        appearance_page.append(themes.as(gtk.Widget));
        const appearance = section("Appearance");
        const adaptive = toggle("Adaptive layout-map width", true);
        const interface_text_scale = gtk.SpinButton.newWithRange(80, 200, 10);
        const width = gtk.SpinButton.newWithRange(200, 390, 1);
        const save_width = gtk.Button.newWithLabel("Save Appearance");
        appendRow(appearance, adaptive.as(gtk.Widget), width.as(gtk.Widget), save_width.as(gtk.Widget));
        appendRow(appearance, label("Interface text size (%)", "caption").as(gtk.Widget), interface_text_scale.as(gtk.Widget), null);
        const layout_visible = toggle("Show Layout", true);
        const inbox_visible = toggle("Show Inbox", false);
        appearance.append(gtk.Label.new("Use Layout and Inbox buttons in each Workspace to change panel visibility.").as(gtk.Widget));
        layout_visible.as(gtk.Widget).setVisible(0);
        inbox_visible.as(gtk.Widget).setVisible(0);
        const inbox_width = gtk.SpinButton.newWithRange(300, 520, 1);
        const send_on_approve = toggle("Approve inserts and presses Return", false);
        appendRow(appearance, label("Inbox width", "caption").as(gtk.Widget), inbox_width.as(gtk.Widget), null);
        appearance.append(send_on_approve.as(gtk.Widget));
        const chat_message_cache = gtk.SpinButton.newWithRange(16, 2048, 16);
        const chat_presentation_cache = gtk.SpinButton.newWithRange(1, 256, 1);
        appearance.append(label("Chat memory — in memory only", "caption").as(gtk.Widget));
        appendRow(appearance, label("Messages and index (MiB)", "caption").as(gtk.Widget), chat_message_cache.as(gtk.Widget), null);
        appendRow(appearance, label("Rendered content (MiB)", "caption").as(gtk.Widget), chat_presentation_cache.as(gtk.Widget), null);
        appearance_page.append(appearance.as(gtk.Widget));
        _ = body.addTitled(appearance_page.as(gtk.Widget), "appearance", "Appearance");

        const intelligence = section("Intelligence");
        const monitoring_enabled = toggle("Monitor tmux Sessions", true);
        const monitoring_codex_titles = toggle("Use Codex pane-title signals", true);
        const monitoring_quiet_period = gtk.SpinButton.newWithRange(1, 30, 1);
        const save_monitoring = gtk.Button.newWithLabel("Save Monitoring");
        intelligence.append(monitoring_enabled.as(gtk.Widget));
        intelligence.append(monitoring_codex_titles.as(gtk.Widget));
        appendRow(intelligence, label("Analyze after quiet (seconds)", "caption").as(gtk.Widget), monitoring_quiet_period.as(gtk.Widget), save_monitoring.as(gtk.Widget));
        const summary_enabled = toggle("Use Codex app-server summaries", false);
        const command = gtk.Entry.new();
        command.setPlaceholderText("Codex executable");
        const model = gtk.Entry.new();
        model.setPlaceholderText("Model (empty means Auto)");
        const timeout = gtk.SpinButton.newWithRange(1, 60, 1);
        const quiet_period = gtk.SpinButton.newWithRange(1, 30, 1);
        const save_summary = gtk.Button.newWithLabel("Save");
        const test_summary = gtk.Button.newWithLabel("Test Provider");
        intelligence.append(summary_enabled.as(gtk.Widget));
        intelligence.append(command.as(gtk.Widget));
        intelligence.append(model.as(gtk.Widget));
        appendRow(intelligence, label("Timeout (seconds)", "caption").as(gtk.Widget), timeout.as(gtk.Widget), save_summary.as(gtk.Widget));
        intelligence.append(test_summary.as(gtk.Widget));
        const codex_notify_status = label("Codex notify: Not checked", "caption");
        const inspect_codex_notify = gtk.Button.newWithLabel("Check Codex Notify");
        const configure_codex_notify = gtk.Button.newWithLabel("Add Polar Station Notify");
        appendRow(intelligence, codex_notify_status.as(gtk.Widget), inspect_codex_notify.as(gtk.Widget), configure_codex_notify.as(gtk.Widget));
        _ = body.addTitled(intelligence.as(gtk.Widget), "intelligence", "Intelligence");

        const notifications = section("Notifications");
        const notifications_enabled = toggle("Show generic system notifications for urgent attention", false);
        const save_notifications = gtk.Button.newWithLabel("Save Notifications");
        appendRow(notifications, notifications_enabled.as(gtk.Widget), save_notifications.as(gtk.Widget), null);
        notifications.append(label("Terminal output is never copied into a notification.", "caption").as(gtk.Widget));
        const codex_sessions = gtk.Button.newWithLabel("All Codex sessions…");
        connect(codex_sessions, Window, codexClicked, self);
        notifications.append(codex_sessions.as(gtk.Widget));
        const connectors = gtk.Button.newWithLabel("Monitoring sources and connectors…");
        connect(connectors, Window, connectorsClicked, self);
        notifications.append(connectors.as(gtk.Widget));
        _ = body.addTitled(notifications.as(gtk.Widget), "notifications", "Notifications");

        const workspace = section("Workspace");
        const workspace_font = gtk.Button.newWithLabel("Current Workspace Font…");
        workspace.append(workspace_font.as(gtk.Widget));
        _ = body.addTitled(workspace.as(gtk.Widget), "workspace", "Workspace");
        const connections = section("Connections");
        const open_connections = gtk.Button.newWithLabel("Open Connections…");
        connections.append(open_connections.as(gtk.Widget));
        _ = body.addTitled(connections.as(gtk.Widget), "connections", "Connections");
        const resources = section("Resources");
        const open_resources = gtk.Button.newWithLabel("Open Resources…");
        resources.append(open_resources.as(gtk.Widget));
        _ = body.addTitled(resources.as(gtk.Widget), "resources", "Resources");
        self.voice = voice_settings.Panel.create() catch null;
        if (self.voice) |panel| {
            panel.callback = voiceAction;
            panel.recover_recording = recoverRecording;
            panel.recording_deleted = recordingDeleted;
            panel.callback_context = self;
            _ = body.addTitled(panel.root.as(gtk.Widget), "voice", "Voice");
            _ = body.addTitled(panel.terminology_root.as(gtk.Widget), "terminology", "Terminology");
            connect(panel.terminology_button, Window, terminologyClicked, self);
        }
        scroll.setChild(body.as(gtk.Widget));
        const navigation = gtk.StackSidebar.new();
        navigation.setStack(body);
        navigation.as(gtk.Widget).setSizeRequest(176, -1);
        const pages = gtk.Box.new(.horizontal, 0);
        pages.append(navigation.as(gtk.Widget));
        scroll.as(gtk.Widget).setHexpand(1);
        pages.append(scroll.as(gtk.Widget));
        shell.append(pages.as(gtk.Widget));
        const toolbar = adw.ToolbarView.new();
        toolbar.addTopBar(adw.HeaderBar.new().as(gtk.Widget));
        toolbar.setContent(shell.as(gtk.Widget));
        window.setContent(toolbar.as(gtk.Widget));

        connect(reload, Window, reloadClicked, self);
        connect(close, Window, closeClicked, self);
        connect(save_width, Window, saveWidthClicked, self);
        connect(save_monitoring, Window, saveMonitoringClicked, self);
        connect(save_summary, Window, saveSummaryClicked, self);
        connect(test_summary, Window, testSummaryClicked, self);
        connect(inspect_codex_notify, Window, inspectCodexNotifyClicked, self);
        connect(configure_codex_notify, Window, configureCodexNotifyClicked, self);
        connect(save_notifications, Window, saveNotificationsClicked, self);
        connect(workspace_font, Window, workspaceFontClicked, self);
        connect(open_connections, Window, connectionsClicked, self);
        connect(open_resources, Window, resourcesClicked, self);
        self.window = window;
        self.state = state_label;
        self.reload_button = reload;
        self.adaptive_width = adaptive;
        self.interface_text_scale = interface_text_scale;
        self.chat_message_cache = chat_message_cache;
        self.chat_presentation_cache = chat_presentation_cache;
        self.width = width;
        self.save_width = save_width;
        self.layout_visible = layout_visible;
        self.inbox_visible = inbox_visible;
        self.inbox_width = inbox_width;
        self.send_on_approve = send_on_approve;
        self.monitoring_enabled = monitoring_enabled;
        self.monitoring_codex_titles = monitoring_codex_titles;
        self.monitoring_quiet_period = monitoring_quiet_period;
        self.save_monitoring = save_monitoring;
        self.summary_enabled = summary_enabled;
        self.summary_command = command;
        self.summary_model = model;
        self.summary_timeout = timeout;
        self.summary_quiet_period = quiet_period;
        self.save_summary = save_summary;
        self.test_summary = test_summary;
        self.codex_notify_status = codex_notify_status;
        self.inspect_codex_notify = inspect_codex_notify;
        self.configure_codex_notify = configure_codex_notify;
        self.notifications_enabled = notifications_enabled;
        self.save_notifications = save_notifications;
    }

    pub fn themeError(self: *Window, message: []const u8) void {
        self.ensureWindow();
        const value = std.fmt.allocPrint(self.allocator, "Cannot load theme: {s}. Previous colors retained.", .{message}) catch return;
        defer self.allocator.free(value);
        setLabel(self.allocator, self.theme_error.?, value) catch {};
    }
    fn applyBackendAvailability(self: *Window) void {
        for (self.theme_buttons) |candidate| if (candidate) |button| button.as(gtk.Widget).setSensitive(@intFromBool(self.backend_available and self.theme_projection != null));
        if (self.theme_follow) |follow| follow.as(gtk.Widget).setSensitive(@intFromBool(self.backend_available and self.theme_projection != null));
        if (self.theme_dropdown) |dropdown| dropdown.as(gtk.Widget).setSensitive(@intFromBool(self.backend_available and self.theme_projection != null));
        const enabled = @intFromBool(self.backend_available);
        inline for (.{ self.reload_button, self.save_width, self.save_monitoring, self.save_summary, self.test_summary, self.inspect_codex_notify, self.configure_codex_notify, self.save_notifications }) |candidate| {
            if (candidate) |button| button.as(gtk.Widget).setSensitive(enabled);
        }
        if (!self.backend_available) setLabel(self.allocator, self.state.?, "Background unavailable — Settings remain open. Reconnect to save.") catch {};
    }

    fn summaryValue(self: *Window) settings.SessionSummary {
        const model_text = text(self.summary_model.?);
        return .{
            .enabled = self.summary_enabled.?.getActive() != 0,
            .command = text(self.summary_command.?),
            .model = if (model_text.len == 0) null else model_text,
            .timeout_ms = @as(u64, @intCast(self.summary_timeout.?.getValueAsInt())) * 1000,
            .quiet_period_ms = @as(u64, @intCast(self.summary_quiet_period.?.getValueAsInt())) * 1000,
        };
    }
};

fn reloadClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .reload);
}
fn activated(_: *gobject.Object, _: *gobject.ParamSpec, self: *Window) callconv(.c) void {
    if (self.window) |window| if (window.as(gtk.Window).isActive() != 0) {
        if (self.voice) |panel| panel.recheckAccount();
    };
}
fn hidden(_: *gtk.Widget, self: *Window) callconv(.c) void {
    if (self.voice) |panel| panel.cancelTests();
}
fn keyPressed(_: *gtk.EventControllerKey, key: c_uint, _: c_uint, _: gdk.ModifierType, self: *Window) callconv(.c) c_int {
    if (key != gdk.KEY_Escape) return 0;
    if (self.voice) |panel| {
        if (panel.test_id != null or panel.pending_test != null) {
            panel.cancelTests();
            return 1;
        }
    }
    if (self.window) |window| window.as(gtk.Window).close();
    return 1;
}
fn closeRequested(_: *gtk.Window, self: *Window) callconv(.c) c_int {
    if (self.voice) |panel| panel.cancelTests();
    return 0;
}
fn closeClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    if (self.voice) |panel| panel.cancelTests();
    if (self.window) |window| window.as(gtk.Widget).setVisible(0);
}
fn saveWidthClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    const layout_visible = self.layout_visible.?.getActive() != 0;
    const inbox_visible = self.inbox_visible.?.getActive() != 0;
    self.action_fn(self.action_context, .{ .appearance = .{
        .theme = self.appearance_value.theme,
        .layout_map_width = if (self.adaptive_width.?.getActive() != 0) null else @intCast(self.width.?.getValueAsInt()),
        .interface_text_scale_percent = @intCast(self.interface_text_scale.?.getValueAsInt()),
        .layout_map_visible = layout_visible,
        .inbox_visible = inbox_visible,
        .inbox_width = @intCast(self.inbox_width.?.getValueAsInt()),
        .terminal_delivery_mode = if (self.send_on_approve.?.getActive() != 0) "send" else "insert",
        .lgp_cache = .{ .message_mib = @intCast(self.chat_message_cache.?.getValueAsInt()), .presentation_mib = @intCast(self.chat_presentation_cache.?.getValueAsInt()) },
        .chat_send_mode = if (self.chat_modified) "modified_enter" else "enter",
    } });
}
fn saveSummaryClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .{ .session_summary = self.summaryValue() });
}
fn saveMonitoringClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .{ .monitoring = .{
        .enabled = self.monitoring_enabled.?.getActive() != 0,
        .codex_title_detection = self.monitoring_codex_titles.?.getActive() != 0,
        .quiet_period_ms = @as(u64, @intCast(self.monitoring_quiet_period.?.getValueAsInt())) * 1000,
    } });
}
fn testSummaryClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .{ .test_summary = self.summaryValue() });
}
fn inspectCodexNotifyClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .inspect_codex_notify);
}
fn configureCodexNotifyClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .configure_codex_notify);
}
fn saveNotificationsClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .{ .attention = .{ .system_notifications_enabled = self.notifications_enabled.?.getActive() != 0 } });
}
fn workspaceFontClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .open_workspace_font);
}
fn connectionsClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .open_connections);
}
fn resourcesClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .open_resources);
}

fn section(title: [*:0]const u8) *gtk.Box {
    const box = gtk.Box.new(.vertical, 8);
    box.as(gtk.Widget).addCssClass("card");
    setMargins(box.as(gtk.Widget), 12);
    box.append(label(title, "heading").as(gtk.Widget));
    return box;
}
fn appendRow(box: *gtk.Box, first: *gtk.Widget, second: *gtk.Widget, third: ?*gtk.Widget) void {
    const row = gtk.Box.new(.horizontal, 8);
    first.setHexpand(1);
    row.append(first);
    row.append(second);
    if (third) |value| row.append(value);
    box.append(row.as(gtk.Widget));
}
fn toggle(title: [*:0]const u8, active: bool) *gtk.ToggleButton {
    const value = gtk.ToggleButton.new();
    value.as(gtk.Button).setChild(label(title, "caption").as(gtk.Widget));
    value.setActive(@intFromBool(active));
    return value;
}
fn text(value: *gtk.Entry) []const u8 {
    return std.mem.span(value.as(gtk.Editable).getText());
}
fn setEntry(allocator: std.mem.Allocator, target: *gtk.Entry, value: []const u8) !void {
    const owned = try allocator.dupeZ(u8, value);
    defer allocator.free(owned);
    target.as(gtk.Editable).setText(owned);
}
fn label(text_value: [*:0]const u8, class: [*:0]const u8) *gtk.Label {
    const value = gtk.Label.new(text_value);
    value.setXalign(0);
    value.setWrap(1);
    value.as(gtk.Widget).addCssClass(class);
    return value;
}
fn setLabel(allocator: std.mem.Allocator, target: *gtk.Label, value: []const u8) !void {
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
fn connect(button: *gtk.Button, comptime Context: type, callback: *const fn (*gtk.Button, *Context) callconv(.c) void, context: *Context) void {
    _ = gtk.Button.signals.clicked.connect(button, *Context, callback, context, .{});
}

fn codexClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .open_codex);
}
fn connectorsClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
    self.action_fn(self.action_context, .open_connectors);
}

fn themeChosen(_: *gtk.Button, context: *ThemeChoice) callconv(.c) void {
    const self = context.owner;
    if (self.rendering or !self.backend_available) return;
    const projected = self.theme_projection orelse return;
    if (context.index >= projected.choices.len) return;
    var appearance = self.appearance_value;
    appearance.theme.mode = "preset";
    appearance.theme.preset = projected.choices[context.index].id;
    appearance.theme.terminal_theme = null;
    self.action_fn(self.action_context, .{ .appearance = appearance });
}
fn themeFollowChanged(button: *gtk.CheckButton, self: *Window) callconv(.c) void {
    if (self.rendering or !self.backend_available) return;
    var appearance = self.appearance_value;
    appearance.theme.mode = if (button.getActive() != 0) "system" else "preset";
    self.action_fn(self.action_context, .{ .appearance = appearance });
}
fn terminalThemeChanged(dropdown: *gtk.DropDown, _: *gobject.ParamSpec, self: *Window) callconv(.c) void {
    if (self.rendering or !self.backend_available) return;
    const selected = dropdown.getSelected();
    if (selected > self.theme_list.items.len) return;
    var appearance = self.appearance_value;
    appearance.theme.terminal_theme = if (selected == 0) null else self.theme_list.items[selected - 1];
    self.action_fn(self.action_context, .{ .appearance = appearance });
}
