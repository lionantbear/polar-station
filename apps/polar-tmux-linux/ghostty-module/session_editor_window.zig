//! Native editor composition and selection. Host operations stay in Controller.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const gdk = @import("gdk");
const gobject = @import("gobject");
const glib = @import("glib");
const auxiliary = @import("auxiliary_window.zig");
const preferences = @import("control_center/width_preferences.zig");

pub const State = struct {
    history_mode: ?bool = null,
    detail_cards: ?*gtk.Box = null,
    detail_path: ?*gtk.Label = null,
    detail_status: ?*gtk.Label = null,
    detail_time: ?*gtk.Label = null,
    detail_identity: ?*gtk.Label = null,
    detail_retry: ?*gtk.Button = null,
    new_split: ?*gtk.Paned = null,
    command_view: ?*gtk.TextView = null,
    name_row: ?*gtk.Box = null,
    command_row: ?*gtk.Box = null,
    manual_button: ?*gtk.MenuButton = null,
    manual_popover: ?*gtk.Popover = null,
    manual_draft: ?*gtk.Entry = null,
    manual_apply: ?*gtk.Button = null,
    history_scroll: ?*gtk.ScrolledWindow = null,
    details_scroll: ?*gtk.ScrolledWindow = null,
    filters: ?*gtk.MenuButton = null,
    options_body: ?*gtk.Box = null,
    options_menu: ?*gtk.MenuButton = null,
    preferences_store: ?preferences.Preferences = null,
};

fn label(text: [*:0]const u8) *gtk.Label {
    const value = gtk.Label.new(text);
    value.setXalign(0);
    return value;
}
fn margins(widget: *gtk.Widget, n: c_int) void {
    widget.setMarginStart(n);
    widget.setMarginEnd(n);
    widget.setMarginTop(n);
    widget.setMarginBottom(n);
}
fn field(caption: *gtk.Label, control: *gtk.Widget) *gtk.Box {
    const row = gtk.Box.new(.horizontal, 12);
    caption.as(gtk.Widget).setSizeRequest(130, -1);
    caption.setXalign(1);
    caption.as(gtk.Widget).addCssClass("dim-label");
    caption.setMnemonicWidget(control);
    control.setHexpand(1);
    row.append(caption.as(gtk.Widget));
    row.append(control);
    return row;
}
fn entry(placeholder: [:0]const u8) *gtk.Entry {
    const value = gtk.Entry.new();
    value.setPlaceholderText(placeholder);
    value.setActivatesDefault(1);
    return value;
}
fn button(text: [:0]const u8) *gtk.Button {
    return gtk.Button.newWithLabel(text);
}
fn clicked(comptime C: type, widget: *gtk.Button, callback: anytype, self: *C) void {
    _ = gtk.Button.signals.clicked.connect(widget, *C, callback, self, .{});
}
fn menu(text: [:0]const u8, child: *gtk.Widget) *gtk.MenuButton {
    const value = gtk.MenuButton.new();
    value.setLabel(text);
    const popover = gtk.Popover.new();
    margins(child, 12);
    popover.setChild(child);
    value.setPopover(popover);
    return value;
}

pub fn create(comptime C: type, self: *C) void {
    const window = auxiliary.create(self.app);
    window.as(gtk.Window).setTitle("Add Session");
    window.as(gtk.Window).setIconName("com.polarealm.PolarStation");
    window.as(gtk.Window).setModal(0);
    window.as(gtk.Window).setHideOnClose(1);
    window.as(gtk.Window).setTransientFor(self.host_window.as(gtk.Window));
    self.editor.preferences_store = preferences.Preferences.init(self.allocator) catch null;
    const root = gtk.Box.new(.vertical, 0);
    const header = gtk.Box.new(.vertical, 10);
    margins(header.as(gtk.Widget), 16);
    const title_row = gtk.Box.new(.horizontal, 8);
    const kinds = gtk.StringList.new(null);
    for ([_][:0]const u8{ "Shell", "Command", "Codex · Agent", "Claude · Agent" }) |kind| kinds.append(kind);
    const kind = gtk.DropDown.new(kinds.as(@import("gio").ListModel), null);
    title_row.append(kind.as(gtk.Widget));
    const spacer = gtk.Box.new(.horizontal, 0);
    spacer.as(gtk.Widget).setHexpand(1);

    const mode_bar = gtk.Box.new(.horizontal, 8);
    const new = button("New");
    const resume_button = button("Resume");
    new.as(gtk.Widget).addCssClass("flat");
    resume_button.as(gtk.Widget).addCssClass("flat");
    mode_bar.append(new.as(gtk.Widget));
    mode_bar.append(resume_button.as(gtk.Widget));

    const modes = gtk.StringList.new(null);
    modes.append("New");
    modes.append("Resume");
    const mode = gtk.DropDown.new(modes.as(@import("gio").ListModel), null);
    mode.as(gtk.Widget).setVisible(0);
    root.append(mode.as(gtk.Widget));
    const scope = gtk.Box.new(.horizontal, 8);

    const machine = gtk.DropDown.new(null, null);
    machine.as(gtk.Widget).setSizeRequest(145, -1);
    scope.append(machine.as(gtk.Widget));
    const appserver = gtk.Box.new(.horizontal, 8);
    appserver.as(gtk.Widget).setHexpand(1);
    const appserver_label = label("App server");
    appserver_label.as(gtk.Widget).setVisible(0);
    appserver.append(appserver_label.as(gtk.Widget));
    const appserver_row = gtk.Box.new(.horizontal, 8);
    appserver_row.as(gtk.Widget).setHexpand(1);
    const endpoint = gtk.DropDown.new(null, null);
    endpoint.as(gtk.Widget).setHexpand(1);
    appserver_row.append(endpoint.as(gtk.Widget));
    const status = label("");
    status.setEllipsize(.end);
    status.setMaxWidthChars(1);
    status.setWrap(1);
    status.as(gtk.Widget).setHexpand(1);
    margins(status.as(gtk.Widget), 8);
    status.as(gtk.Widget).addCssClass("dim-label");
    const manage = gtk.Button.newFromIconName("emblem-system-symbolic");
    manage.as(gtk.Widget).setTooltipText("Manage app servers");
    appserver_row.append(manage.as(gtk.Widget));
    appserver.append(appserver_row.as(gtk.Widget));
    scope.append(appserver.as(gtk.Widget));
    title_row.append(scope.as(gtk.Widget));
    title_row.append(spacer.as(gtk.Widget));
    title_row.append(mode_bar.as(gtk.Widget));
    header.append(title_row.as(gtk.Widget));
    root.append(header.as(gtk.Widget));
    root.append(gtk.Separator.new(.horizontal).as(gtk.Widget));

    const history = gtk.Box.new(.vertical, 8);
    history.as(gtk.Widget).setVexpand(1);
    margins(history.as(gtk.Widget), 16);
    const toolbar = gtk.Box.new(.horizontal, 8);
    const search = entry("Search conversations");
    search.setActivatesDefault(0);
    search.as(gtk.Widget).setHexpand(1);
    toolbar.append(search.as(gtk.Widget));
    const filter_body = gtk.Box.new(.vertical, 8);
    const archived = gtk.CheckButton.newWithLabel("Archived");
    const subagents = gtk.CheckButton.newWithLabel("Include subagents");
    filter_body.append(archived.as(gtk.Widget));
    filter_body.append(subagents.as(gtk.Widget));
    const filters = menu("Filters", filter_body.as(gtk.Widget));
    toolbar.append(filters.as(gtk.Widget));
    history.append(toolbar.as(gtk.Widget));
    const split = gtk.Paned.new(.horizontal);
    split.setPosition(340);
    split.setShrinkStartChild(0);
    split.setShrinkEndChild(0);
    split.setResizeStartChild(0);
    split.as(gtk.Widget).setVexpand(1);
    const table = gtk.Box.new(.vertical, 0);
    table.as(gtk.Widget).setSizeRequest(280, -1);
    const headings = gtk.Box.new(.horizontal, 12);
    margins(headings.as(gtk.Widget), 8);
    const name_header = label("Conversation");
    name_header.as(gtk.Widget).setHexpand(1);
    headings.append(name_header.as(gtk.Widget));
    table.append(headings.as(gtk.Widget));
    const rows = gtk.ListBox.new();
    rows.setPlaceholder(label("No conversations found").as(gtk.Widget));
    rows.setSelectionMode(.single);
    rows.setActivateOnSingleClick(0);
    rows.as(gtk.Widget).addCssClass("data-table");
    const history_scroll = gtk.ScrolledWindow.new();
    history_scroll.setPolicy(.never, .automatic);
    history_scroll.setChild(rows.as(gtk.Widget));
    history_scroll.as(gtk.Widget).setVexpand(1);
    table.append(history_scroll.as(gtk.Widget));
    const more = button("Retry loading");
    more.as(gtk.Widget).setVisible(0);
    more.as(gtk.Widget).setHalign(.center);
    table.append(more.as(gtk.Widget));
    split.setStartChild(table.as(gtk.Widget));
    const details = label("Select a conversation to see its details");
    details.setWrap(1);
    details.setWrapMode(.word_char);
    details.setLines(2);
    details.setEllipsize(.end);
    details.setSelectable(1);
    details.setYalign(0);
    const detail_body = gtk.Box.new(.vertical, 12);
    margins(detail_body.as(gtk.Widget), 16);
    details.as(gtk.Widget).addCssClass("title-3");
    const title_line = gtk.Box.new(.horizontal, 12);
    details.as(gtk.Widget).setHexpand(1);
    details.setMaxWidthChars(1);
    title_line.append(details.as(gtk.Widget));
    const detail_status = label("");
    detail_status.as(gtk.Widget).addCssClass("accent");
    title_line.append(detail_status.as(gtk.Widget));
    detail_body.append(title_line.as(gtk.Widget));
    const detail_path = label("");
    detail_path.setEllipsize(.middle); detail_path.setSelectable(1);
    detail_path.setMaxWidthChars(1); detail_path.as(gtk.Widget).setHexpand(1);
    const path_line = gtk.Box.new(.horizontal, 8);
    path_line.append(gtk.Image.newFromIconName("folder-symbolic").as(gtk.Widget));
    path_line.append(detail_path.as(gtk.Widget));
    const copy_path = gtk.Button.newFromIconName("edit-copy-symbolic");
    copy_path.as(gtk.Widget).setTooltipText("Copy directory");
    _ = gtk.Button.signals.clicked.connect(copy_path, *gtk.Label, copyDirectory, detail_path, .{});
    path_line.append(copy_path.as(gtk.Widget));
    detail_body.append(path_line.as(gtk.Widget));
    const meta_line = gtk.Box.new(.horizontal, 12);
    const detail_time = label("");
    detail_time.as(gtk.Widget).setHexpand(1);
    meta_line.append(detail_time.as(gtk.Widget));
    self.editor.detail_path = detail_path;
    self.editor.detail_status = detail_status;
    self.editor.detail_time = detail_time;
    const copy_id = button("Copy ID");
    copy_id.as(gtk.Widget).setHalign(.start);
    copy_id.as(gtk.Widget).setSensitive(0);
    const detail_identity = label("");
    self.editor.detail_identity = detail_identity;
    meta_line.append(detail_identity.as(gtk.Widget));
    meta_line.append(copy_id.as(gtk.Widget));
    detail_body.append(meta_line.as(gtk.Widget));
    const detail_scroll = gtk.ScrolledWindow.new();
    detail_scroll.setPolicy(.never, .automatic);
    const cards = gtk.Box.new(.vertical, 12);
    margins(cards.as(gtk.Widget), 16);
    detail_scroll.setChild(detail_body.as(gtk.Widget));
    detail_scroll.as(gtk.Widget).setVexpand(1);
    detail_body.append(gtk.Separator.new(.horizontal).as(gtk.Widget));
    detail_body.append(cards.as(gtk.Widget));
    self.editor.detail_cards = cards;
    const detail_retry = button("Retry details");
    detail_retry.as(gtk.Widget).setVisible(0);
    clicked(C, detail_retry, C.historyDetailRetryClicked, self);
    detail_body.append(detail_retry.as(gtk.Widget));
    self.editor.detail_retry = detail_retry;
    detail_scroll.as(gtk.Widget).setSizeRequest(360, -1);
    split.setEndChild(detail_scroll.as(gtk.Widget));
    history.append(split.as(gtk.Widget));
    root.append(history.as(gtk.Widget));

    const body = gtk.Box.new(.vertical, 14);
    margins(body.as(gtk.Widget), 16);
    const name = entry("Session name");
    const name_label = label("Session name");
    const name_row = field(name_label, name.as(gtk.Widget));
    body.append(name_row.as(gtk.Widget));
    const directory = entry("Working directory");
    const directory_label = label("Working directory");
    const directory_controls = gtk.Box.new(.horizontal, 8);
    directory.as(gtk.Widget).setHexpand(1);
    directory_controls.append(directory.as(gtk.Widget));
    const browse = button("Browse…");
    directory_controls.append(browse.as(gtk.Widget));
    const directory_row = field(directory_label, directory_controls.as(gtk.Widget));
    body.append(directory_row.as(gtk.Widget));
    const command = entry("Command");
    const command_label = label("Command");
    const command_view = gtk.TextView.new();
    command_view.setWrapMode(.word_char); command_view.setMonospace(1);
    self.editor.command_view = command_view;
    const command_scroll = gtk.ScrolledWindow.new();
    command_scroll.setChild(command_view.as(gtk.Widget));
    command_scroll.as(gtk.Widget).setSizeRequest(-1, 120);
    const command_row = field(command_label, command_scroll.as(gtk.Widget));
    command.as(gtk.Widget).setVisible(0);
    body.append(command.as(gtk.Widget));
    _ = gtk.TextBuffer.signals.changed.connect(command_view.getBuffer(), *gtk.Entry, commandBufferChanged, command, .{});
    _ = gtk.Editable.signals.changed.connect(command.as(gtk.Editable), *gtk.TextBuffer, commandEntryChanged, command_view.getBuffer(), .{});
    body.append(command_row.as(gtk.Widget));
    const auto_restart = gtk.CheckButton.newWithLabel("Automatically restart command after server loss");
    body.append(auto_restart.as(gtk.Widget));
    const manual = gtk.Expander.new("Session UUID");
    manual.setExpanded(1);
    const resume_id = entry("Original agent session ID");
    const resume_label = label("Claude session UUID");
    const manual_content = gtk.Box.new(.vertical, 4);
    manual_content.append(resume_label.as(gtk.Widget));
    manual_content.append(resume_id.as(gtk.Widget));
    manual.setChild(manual_content.as(gtk.Widget));
    body.append(manual.as(gtk.Widget));
    const advanced = gtk.Expander.new("More options");
    const advanced_body = gtk.Box.new(.vertical, 8);
    margins(advanced_body.as(gtk.Widget), 8);
    const yolo = gtk.CheckButton.newWithLabel("YOLO · Skip approvals and sandbox");
    const local_cli = gtk.CheckButton.newWithLabel("Open CLI on this Station Host");
    advanced_body.append(yolo.as(gtk.Widget));
    advanced_body.append(local_cli.as(gtk.Widget));
    advanced.setChild(advanced_body.as(gtk.Widget));
    body.append(advanced.as(gtk.Widget));
    const body_scroll = gtk.ScrolledWindow.new();
    body_scroll.setPolicy(.never, .automatic);
    body_scroll.setPropagateNaturalHeight(1);
    body_scroll.setChild(body.as(gtk.Widget));
    const new_split = gtk.Paned.new(.horizontal);
    new_split.setPosition(480);
    new_split.as(gtk.Widget).setVexpand(1);
    new_split.setEndChild(body_scroll.as(gtk.Widget));
    self.editor.new_split = new_split;
    root.append(new_split.as(gtk.Widget));
    const error_label = label("");
    error_label.setWrap(1);
    error_label.as(gtk.Widget).addCssClass("error");
    error_label.as(gtk.Widget).setMarginStart(16);
    error_label.as(gtk.Widget).setMarginEnd(16);
    root.append(error_label.as(gtk.Widget));
    root.append(status.as(gtk.Widget));
    root.append(gtk.Separator.new(.horizontal).as(gtk.Widget));
    const footer = gtk.Box.new(.horizontal, 10);
    margins(footer.as(gtk.Widget), 12);
    const manual_body = gtk.Box.new(.vertical, 12);
    const manual_draft = entry("Codex Thread ID");
    const manual_caption = label("Codex Thread ID");
    manual_caption.setMnemonicWidget(manual_draft.as(gtk.Widget));
    manual_body.append(manual_caption.as(gtk.Widget));
    manual_draft.setActivatesDefault(0);
    manual_draft.as(gtk.Widget).setSizeRequest(330, -1);
    manual_body.append(manual_draft.as(gtk.Widget));
    const manual_actions = gtk.Box.new(.horizontal, 8);
    manual_actions.as(gtk.Widget).setHalign(.end);
    const manual_cancel = button("Cancel");
    const manual_apply = button("Use ID");
    manual_actions.append(manual_cancel.as(gtk.Widget));
    manual_actions.append(manual_apply.as(gtk.Widget));
    manual_body.append(manual_actions.as(gtk.Widget));
    const manual_button = menu("Open using ID…", manual_body.as(gtk.Widget));
    footer.append(manual_button.as(gtk.Widget));
    const options_menu = gtk.MenuButton.new();
    options_menu.setLabel("More options");
    options_menu.setPopover(gtk.Popover.new());
    footer.append(options_menu.as(gtk.Widget));
    self.editor.options_menu = options_menu;
    self.editor.options_body = advanced_body;
    const target = label("");
    target.setEllipsize(.end);
    target.as(gtk.Widget).setHexpand(1);
    target.as(gtk.Widget).addCssClass("dim-label");
    footer.append(target.as(gtk.Widget));
    const cancel = button("Cancel");
    const submit = button("New & Open");
    submit.as(gtk.Widget).addCssClass("suggested-action");
    footer.append(cancel.as(gtk.Widget));
    footer.append(submit.as(gtk.Widget));
    root.append(footer.as(gtk.Widget));

    self.editor.name_row = name_row;
    self.editor.command_row = command_row;
    self.editor.manual_button = manual_button;
    self.editor.manual_popover = manual_button.getPopover();
    self.editor.manual_draft = manual_draft;
    self.editor.manual_apply = manual_apply;
    self.editor.history_scroll = history_scroll;
    self.editor.details_scroll = detail_scroll;
    self.editor.filters = filters;
    self.session_create_window = window;
    self.session_create_target = target;
    self.session_create_name = name;
    self.session_create_name_label = name_label;
    self.session_create_directory = directory;
    self.session_create_directory_row = directory_row;
    self.session_create_directory_label = directory_label;
    self.session_create_command = command;
    self.session_create_command_label = command_label;
    self.session_create_auto_restart = auto_restart;
    self.session_create_machine = machine;
    self.session_create_type = kind;
    self.session_create_mode = mode;
    self.session_create_mode_bar = mode_bar;
    self.session_create_new_button = new;
    self.session_create_resume_button = resume_button;
    self.session_create_endpoint = endpoint;
    self.session_appserver_field = appserver;
    self.session_create_appserver_label = appserver_label;
    self.session_create_appserver_row = appserver_row;
    self.session_endpoint_status = status;
    self.session_history_box = history;
    self.session_history_rows = rows;
    self.session_history_details = details;
    self.session_history_copy = copy_id;
    self.session_history_search = search;
    self.session_history_more = more;
    self.session_history_archived = archived;
    self.session_history_subagents = subagents;
    self.session_manual_id = manual;
    self.session_create_resume_id = resume_id;
    self.session_create_resume_label = resume_label;
    self.session_create_advanced = advanced;
    self.session_create_yolo = yolo;
    self.session_create_local_cli = local_cli;
    self.session_create_browse = browse;
    self.session_editor_body = body;
    self.session_editor_scroll = body_scroll;
    self.session_create_error = error_label;
    self.session_create_submit = submit;
    self.session_create_cancel = cancel;
    inline for (.{ .{ new, C.sessionNewModeClicked }, .{ resume_button, C.sessionResumeModeClicked }, .{ manage, C.sessionManageConnectionsClicked }, .{ browse, C.sessionDirectoryBrowseClicked }, .{ more, C.historyMoreClicked }, .{ copy_id, C.historyCopyClicked }, .{ cancel, C.sessionCreateCancelClicked }, .{ submit, C.sessionCreateSubmitClicked }, .{ manual_apply, C.historyManualApply }, .{ manual_cancel, C.historyManualCancel } }) |pair| clicked(C, pair[0], pair[1], self);
    inline for (.{ name, command, resume_id }) |value| _ = gtk.Editable.signals.changed.connect(value.as(gtk.Editable), *C, C.sessionCreateChanged, self, .{});
    _ = gtk.Editable.signals.changed.connect(manual_draft.as(gtk.Editable), *C, C.historyManualChanged, self, .{});
    inline for (.{ machine, kind, mode }) |value| _ = gobject.Object.signals.notify.connect(value.as(gobject.Object), *C, C.sessionCreateChoiceChanged, self, .{ .detail = "selected" });
    _ = gobject.Object.signals.notify.connect(endpoint.as(gobject.Object), *C, C.sessionCreateEndpointChanged, self, .{ .detail = "selected" });
    _ = gtk.Editable.signals.changed.connect(search.as(gtk.Editable), *C, C.historySearchChanged, self, .{});
    _ = gtk.Entry.signals.activate.connect(search, *C, C.historySearchActivated, self, .{});
    inline for (.{ archived, subagents }) |value| _ = gtk.CheckButton.signals.toggled.connect(value, *C, C.historyFilterChanged, self, .{});
    _ = gtk.CheckButton.signals.toggled.connect(local_cli, *C, C.sessionCreateLocalCLIChanged, self, .{});
    _ = gtk.Adjustment.signals.value_changed.connect(history_scroll.getVadjustment(), *C, C.historyViewportChanged, self, .{});
    _ = gtk.Adjustment.signals.changed.connect(history_scroll.getVadjustment(), *C, C.historyViewportChanged, self, .{});
    _ = gtk.ListBox.signals.row_selected.connect(rows, *C, C.historyRowSelected, self, .{});
    rows.as(gtk.Widget).setFocusable(1);
    const history_keys = gtk.EventControllerKey.new();
    history_keys.as(gtk.EventController).setPropagationPhase(.capture);
    _ = gtk.EventControllerKey.signals.key_pressed.connect(history_keys, *C, C.sessionCreateKeyPressed, self, .{});
    rows.as(gtk.Widget).addController(history_keys.as(gtk.EventController));
    _ = gtk.ListBox.signals.row_activated.connect(rows, *C, C.historyRowActivated, self, .{});
    _ = gobject.Object.signals.notify.connect(advanced.as(gobject.Object), *C, C.editorOptionsChanged, self, .{ .detail = "expanded" });
    _ = gtk.Window.signals.close_request.connect(window.as(gtk.Window), *C, C.sessionCreateCloseRequested, self, .{});
    const keys = gtk.EventControllerKey.new();
    _ = gtk.EventControllerKey.signals.key_pressed.connect(keys, *C, C.sessionCreateKeyPressed, self, .{});
    window.as(gtk.Widget).addController(keys.as(gtk.EventController));
    window.as(gtk.Window).setDefaultWidget(submit.as(gtk.Widget));
    window.setContent(root.as(gtk.Widget));
}

pub fn saveSize(self: anytype) void {
    const history = self.editor.history_mode orelse return;
    const window = self.session_create_window orelse return;
    if (window.as(gtk.Widget).getVisible() == 0) return;
    if (self.editor.preferences_store) |*store| {
        const maximized = window.as(gtk.Window).isMaximized() != 0;
        if (!maximized) store.saveEditorSize(history, .{ window.as(gtk.Widget).getWidth(), window.as(gtk.Widget).getHeight() }) catch {};
        store.saveEditorMaximized(maximized) catch {};
    }
    // GTK unmaps the native maximized state on hide; restore saved geometry
    // on the next presentation, while mode switches keep the live allocation.
    self.editor.history_mode = null;
}
pub fn resize(self: anytype, history: bool, _: bool) void {
    const configured = self.editor.history_mode != null;
    const entering_history = history and !(self.editor.history_mode orelse false);
    self.editor.history_mode = history;
    if (entering_history) _ = self.session_history_rows.?.as(gtk.Widget).grabFocus();
    if (configured) return;
    var size: [2]c_int = undefined;
    var available: [2]c_int = .{ 1280, 800 };
    if (self.host_window.as(gtk.Native).getSurface()) |surface| if (self.host_window.as(gtk.Widget).getDisplay().getMonitorAtSurface(surface)) |monitor| {
        var geometry: gdk.Rectangle = undefined; monitor.getGeometry(&geometry);
        available = .{ geometry.f_width, geometry.f_height };
    };
    const golden_width = @min(@max(800, @divTrunc(available[0] * 618, 1000)), @divTrunc((available[1] - 64) * 1618, 1000));
    size = .{ golden_width, @divTrunc(golden_width * 1000, 1618) };
    if (self.editor.preferences_store) |*store| if (store.loadEditorSize(true)) |saved| { size = saved; };
    size[0] = @min(@max(size[0], 800), available[0] - 32);
    size[1] = @min(@max(size[1], 494), available[1] - 64);
    self.session_create_window.?.as(gtk.Widget).setSizeRequest(@min(800, available[0] - 32), @min(494, available[1] - 64));
    self.session_create_window.?.as(gtk.Window).setDefaultSize(size[0], size[1]);
    if (self.editor.preferences_store) |*store| if (store.editorMaximized()) { self.session_create_window.?.as(gtk.Window).maximize(); };
}

pub fn appendThread(self: anytype, thread: anytype) void {
    const allocator = self.allocator;
    var index: c_int = 0;
    while (self.session_history_rows.?.getRowAtIndex(index)) |existing| : (index += 1) {
        if (std.mem.eql(u8, std.mem.span(existing.as(gtk.Widget).getName()), thread.thread_id)) return;
    }
    const row = gtk.ListBoxRow.new();
    const id = allocator.dupeZ(u8, thread.thread_id) catch return;
    defer allocator.free(id);
    row.as(gtk.Widget).setName(id);
    const content = gtk.Box.new(.horizontal, 12);
    margins(content.as(gtk.Widget), 8);
    content.as(gtk.Widget).setMarginTop(4);
    content.as(gtk.Widget).setMarginBottom(4);
    content.as(gtk.Widget).setSizeRequest(-1, 24);
    const selected = gtk.CheckButton.new();
    if (self.historyAlreadyAssigned(thread.thread_id)) {
        selected.as(gtk.Widget).setTooltipText("Already in this Tab");
    }
    content.append(selected.as(gtk.Widget));
    _ = gtk.CheckButton.signals.toggled.connect(selected, @TypeOf(self), @TypeOf(self.*).historySelectionChanged, self, .{});
    const title_z = allocator.dupeZ(u8, thread.display_name) catch return;
    defer allocator.free(title_z);
    const title = label(title_z);
    title.setEllipsize(.end);
    title.setMaxWidthChars(1);
    title.as(gtk.Widget).setHexpand(1);
    title.as(gtk.Widget).setTooltipText(title_z);
    content.append(title.as(gtk.Widget));
    const date = glib.DateTime.newFromUnixLocal(thread.updated_at);
    defer if (date) |v| v.unref();
    const time = if (date) |v| v.format("%m-%d %H:%M") else null;
    defer if (time) |v| glib.free(v);
    const updated = if (time) |v| v else "Unknown";
    const date_label = label(updated);
    date_label.as(gtk.Widget).setVisible(0);
    content.append(date_label.as(gtk.Widget));
    const metadata_json = std.json.Stringify.valueAlloc(allocator, thread, .{}) catch return;
    defer allocator.free(metadata_json);
    const details_z = allocator.dupeZ(u8, metadata_json) catch return;
    defer allocator.free(details_z);
    const metadata = label(details_z);
    metadata.as(gtk.Widget).setVisible(0);
    content.append(metadata.as(gtk.Widget));
    row.setChild(content.as(gtk.Widget));
    self.session_history_rows.?.append(row.as(gtk.Widget));
}

pub fn showHistoryOptions(self: anytype, history: bool) void {
    const body = self.editor.options_body orelse return;
    const popover = self.editor.options_menu.?.getPopover().?;
    const in_menu = popover.getChild() != null;
    if (history != in_menu) {
        _ = body.as(gobject.Object).ref();
        if (history) {
            self.session_create_advanced.?.setChild(null);
            popover.setChild(body.as(gtk.Widget));
        } else {
            popover.setChild(null);
            self.session_create_advanced.?.setChild(body.as(gtk.Widget));
        }
        body.as(gobject.Object).unref();
    }
    self.editor.options_menu.?.as(gtk.Widget).setVisible(@intFromBool(history));
    self.session_editor_scroll.?.as(gtk.Widget).setVisible(@intFromBool(!history));
    self.editor.new_split.?.as(gtk.Widget).setVisible(@intFromBool(!history));
}

pub fn detailCard(self: anytype, title: [:0]const u8, text: []const u8) void {
    const card = gtk.Box.new(.vertical, 8);
    card.as(gtk.Widget).addCssClass("card");
    const heading = label(title);
    heading.as(gtk.Widget).addCssClass("heading");
    margins(heading.as(gtk.Widget), 12);
    card.append(heading.as(gtk.Widget));
    const owned = self.allocator.dupeZ(u8, text) catch return;
    defer self.allocator.free(owned);
    const body = label(owned);
    body.setWrap(1); body.setWrapMode(.word_char); body.setSelectable(1);
    margins(body.as(gtk.Widget), 12);
    card.append(body.as(gtk.Widget));
    if (std.mem.eql(u8, title, "Opening preview")) {
        body.setLines(4); body.setEllipsize(.end);
        const expand = button("Show more");
        expand.as(gtk.Widget).setHalign(.start);
        _ = gtk.Button.signals.clicked.connect(expand, *gtk.Label, expandPreview, body, .{});
        card.append(expand.as(gtk.Widget));
    }
    self.editor.detail_cards.?.append(card.as(gtk.Widget));
}
pub fn clearDetails(self: anytype) void {
    if (self.editor.detail_retry) |retry| retry.as(gtk.Widget).setVisible(0);
    const cards = self.editor.detail_cards orelse return;
    while (cards.as(gtk.Widget).getFirstChild()) |child| cards.remove(child);
}
pub fn showMetadata(self: anytype, json: []const u8) void {
    const Metadata = struct { display_name: []const u8, cwd: []const u8, status: []const u8, thread_id: []const u8, preview: ?[]const u8 = null, updated_at: i64 = 0 };
    const parsed = std.json.parseFromSlice(Metadata, self.allocator, json, .{ .ignore_unknown_fields = true }) catch return;
    defer parsed.deinit();
    const value = parsed.value;
    const title = self.allocator.dupeZ(u8, value.display_name) catch return;
    defer self.allocator.free(title);
    self.session_history_details.?.setLabel(title);
    self.session_history_details.?.as(gtk.Widget).setTooltipText(title);
    const cwd = self.allocator.dupeZ(u8, value.cwd) catch return;
    defer self.allocator.free(cwd);
    self.editor.detail_path.?.setLabel(cwd);
    self.editor.detail_path.?.as(gtk.Widget).setTooltipText(cwd);
    const status = self.allocator.dupeZ(u8, value.status) catch return;
    defer self.allocator.free(status);
    self.editor.detail_status.?.setLabel(status);
    const date = glib.DateTime.newFromUnixLocal(value.updated_at);
    defer if (date) |v| v.unref();
    const time = if (date) |v| v.format("Updated %Y-%m-%d %H:%M") else null;
    defer if (time) |v| glib.free(v);
    self.editor.detail_time.?.setLabel(if (time) |v| v else "Unknown update time");
    const identity = self.allocator.dupeZ(u8, value.thread_id) catch return;
    defer self.allocator.free(identity);
    self.session_history_copy.?.as(gtk.Widget).setTooltipText(identity);
    const short = self.allocator.dupeZ(u8, value.thread_id[0..@min(8, value.thread_id.len)]) catch return;
    defer self.allocator.free(short);
    self.editor.detail_identity.?.setLabel(short);
    self.editor.detail_identity.?.as(gtk.Widget).setTooltipText(identity);
    clearDetails(self);
    if (value.preview) |preview| if (!std.mem.eql(u8, preview, value.display_name) and preview.len > 0) detailCard(self, "Opening preview", preview);
}

fn copyDirectory(_: *gtk.Button, value: *gtk.Label) callconv(.c) void {
    value.as(gtk.Widget).getClipboard().setText(value.getText());
}
fn expandPreview(control: *gtk.Button, body: *gtk.Label) callconv(.c) void {
    const expanded = body.getLines() == -1;
    body.setLines(if (expanded) 4 else -1);
    body.setEllipsize(if (expanded) .end else .none);
    control.setLabel(if (expanded) "Show more" else "Show less");
}

fn commandBufferChanged(buffer: *gtk.TextBuffer, value: *gtk.Entry) callconv(.c) void {
    var start: gtk.TextIter = undefined; var end: gtk.TextIter = undefined;
    buffer.getBounds(&start, &end);
    const text = buffer.getText(&start, &end, 1);
    defer glib.free(text);
    if (!std.mem.eql(u8, std.mem.span(text), std.mem.span(value.as(gtk.Editable).getText()))) value.as(gtk.Editable).setText(text);
}
fn commandEntryChanged(value: *gtk.Editable, buffer: *gtk.TextBuffer) callconv(.c) void {
    var start: gtk.TextIter = undefined; var end: gtk.TextIter = undefined;
    buffer.getBounds(&start, &end);
    const text = buffer.getText(&start, &end, 1);
    defer glib.free(text);
    if (!std.mem.eql(u8, std.mem.span(text), std.mem.span(value.getText()))) buffer.setText(value.getText(), -1);
}

pub fn resetDetails(self: anytype) void {
    clearDetails(self);
    inline for (.{self.editor.detail_path, self.editor.detail_status, self.editor.detail_time, self.editor.detail_identity}) |value| {
        if (value) |field_label| field_label.setLabel("");
    }
    if (self.session_history_copy) |copy| copy.as(gtk.Widget).setTooltipText(null);
}
