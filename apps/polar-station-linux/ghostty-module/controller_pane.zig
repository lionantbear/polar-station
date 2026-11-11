//! Native GTK/libadwaita pane renderer for the shared Control Center projection.
//!
//! Native composition and measurement live here; Host facts, command meaning,
//! confirmation, tmux and SSH execution remain in shared Rust.

const connections_window = @import("connections_window.zig");
const auxiliary = @import("auxiliary_window.zig");
const editor_interaction = @import("session_editor_interaction.zig");
const session_editor_window = @import("session_editor_window.zig");
const session_end_window = @import("session_end_window.zig");
const session_directory_window = @import("session_directory_window.zig");
const std = @import("std");
const adw = @import("adw");
const gdk = @import("gdk");
const glib = @import("glib");
const gobject = @import("gobject");
const gtk = @import("gtk");

const Application = @import("../class/application.zig").Application;
const Window = @import("../class/window.zig").Window;
const control = @import("control_center/model.zig");
const control_center_intent = @import("control_center/intent.zig");
const layout_policy = @import("control_center/layout_policy.zig");
const inbox = @import("inbox.zig");
const width_preferences = @import("control_center/width_preferences.zig");
const pane_interaction = @import("pane_interaction.zig");
const session_interaction = @import("control_center/session_interaction.zig");
const shell_lifecycle = @import("shell_lifecycle.zig");
const presentation_executor = @import("presentation_executor.zig");
const agent_catalog = @import("agent_catalog.zig");
const shortcuts = @import("shortcuts.zig");

pub const Action = control_center_intent.Action;
pub const AuxiliaryKind = enum { iphone };
const SessionCreateKind = enum { local_shell, local_command, remote, codex, codex_agent, claude_agent };
const log = std.log.scoped(.polar_station_controller);

pub const ActionFn = *const fn (?*anyopaque, u64, Action) void;
pub const PaneDropFn = *const fn (?*anyopaque, u64, pane_interaction.PaneDrop) void;

const MapBodyRef = struct {
    widget: *gtk.Widget,
    grid: ?*gtk.Grid,
    display_mode: control.DisplayMode,
    row_span: usize,
};

const controller_css =
    \\.polar-station-controller-root button.flat,
    \\.polar-station-controller-root menubutton > button,
    \\.polar-station-pane-action {
    \\  min-width: 0;
    \\  padding-left: 0;
    \\  padding-right: 0;
    \\}
    \\.polar-station-bottom-strip label {
    \\  font-size: 10px;
    \\}
    \\.polar-station-controller-root flowboxchild {
    \\  padding: 0;
    \\}
    \\.polar-station-pane-action {
    \\  padding-top: 0;
    \\  padding-bottom: 0;
    \\}
    \\.polar-tab-header button { min-height: 24px; padding-top: 2px; padding-bottom: 2px; }
    \\.polar-tab-selected { box-shadow: inset 0 0 0 2px @accent_color; }
    \\.polar-tab-selected-header { background: alpha(@accent_bg_color, 0.18); border-radius: 8px; }
    \\.polar-pane-located { outline: 2px dashed @accent_color; outline-offset: -3px; }
    \\.pane-drop-left { box-shadow: inset 3px 0 #3584e4; }
    \\.pane-drop-right { box-shadow: inset -3px 0 #3584e4; }
    \\.pane-drop-up { box-shadow: inset 0 3px #3584e4; }
    \\.pane-drop-down { box-shadow: inset 0 -3px #3584e4; }
    \\.pane-drop-append { box-shadow: inset 0 0 0 2px #3584e4; }
;

pub const Controller = struct {
    workspace_stack: *gtk.Stack,
    workspace_chooser: *gtk.Box,
    chooser_inputs: @import("application_refresh.zig").RenderedInputs = .{},
    header_inputs: @import("application_refresh.zig").RenderedInputs = .{},
    workspace_menu_inputs: @import("application_refresh.zig").RenderedInputs = .{},
    bottom_inputs: @import("application_refresh.zig").RenderedInputs = .{},
    more_inputs: @import("application_refresh.zig").RenderedInputs = .{},

    sessions_search: *gtk.Entry,
    sessions_type: *gtk.DropDown,
    collapsed_session_groups: std.StringHashMapUnmanaged(void) = .{},
    idea_binding_pane: ?[]u8 = null,
    idea_binding_view: ?[]u8 = null,
    idea_binding_context: ?*anyopaque = null,
    idea_binding_callback: ?*const fn (?*anyopaque,[]const u8,[]const u8,[]const u8,[]const u8,[]const u8,[]const u8) void = null,
    session_history_archived: ?*gtk.CheckButton = null,
    session_history_subagents: ?*gtk.CheckButton = null,
    session_manual_id: ?*gtk.Expander = null,
    sessions_inputs: @import("application_refresh.zig").RenderedInputs = .{},
    map_inputs: @import("application_refresh.zig").RenderedInputs = .{},
    inbox_inputs: @import("application_refresh.zig").RenderedInputs = .{},
    allocator: std.mem.Allocator,
    app: *Application,
    model: *control.Model,
    inbox_model: *inbox.Model,
    native_executor: ?*presentation_executor.Executor,
    action_fn: ActionFn,
    pane_drop_fn: PaneDropFn,
    action_context: ?*anyopaque,
    controller_id: u64,

    host_window: *Window,
    host_alive: bool = true,
    css_display: *gdk.Display,
    css_provider: *gtk.CssProvider,
    pane_content: *gtk.Widget,
    window_title: *adw.WindowTitle,
    connection_status_image: *gtk.Image,
    connection_status_label: *gtk.Label,
    connection_badge: *gtk.Button,
    workspace_menu: *gtk.MenuButton,
    workspace_menu_label: *gtk.Label,
    workspace_menu_box: *gtk.Box,
    rename_workspace_button: *gtk.Button,
    add_tab_button: *gtk.Button,
    refresh_button: *gtk.Button,
    restore_popover: *gtk.Popover,
    lifecycle_button: *gtk.Button,
    lifecycle_image: *gtk.Image,
    sessions_revealer: *gtk.Revealer,
    sessions_panel: *gtk.Box,
    sessions_separator: *gtk.Separator,
    layout_region: *gtk.Box,
    sessions_title: *gtk.Label,
    sessions_hint: *gtk.Label,
    sessions_refresh_button: *gtk.Button,
    sessions_close_button: *gtk.Button,
    sessions_scroller: *gtk.ScrolledWindow,
    sessions_list: *gtk.ListBox,
    layout_scroller: *gtk.ScrolledWindow,
    layout_overlay: *gtk.Overlay,
    width_resize_handle: *gtk.Box,
    layout_box: *gtk.Box,
    inbox_scroller: *gtk.ScrolledWindow,
    inbox_box: *gtk.Box,
    sessions_button: *gtk.ToggleButton,
    sessions_button_label: *gtk.Label,
    mode_button: *gtk.ToggleButton,
    mode_button_label: *gtk.Label,
    resources_button: *gtk.Button,
    resources_image: *gtk.Image,
    resources_button_label: *gtk.Label,
    iphone_button: *gtk.ToggleButton,
    iphone_button_label: *gtk.Label,
    more_menu: *gtk.MenuButton,
    more_menu_label: *gtk.Label,
    more_menu_box: *gtk.Box,
    sessions_visible: bool = false,
    layout_map_visible: bool = true,
    inbox_visible: bool = false,
    inbox_filters: [3]?*gtk.ComboBoxText = @splat(null),
    inbox_filter_values: [3]?[]u8 = @splat(null),
    terminal_delivery_mode: enum { insert, send } = .insert,
    inbox_width: c_int = 340,
    map_width: c_int = layout_policy.layout_map_max_width,
    width_drag_start: c_int = layout_policy.layout_map_max_width,
    width_drag_pointer_start: ?f64 = null,
    width_preferences_store: ?width_preferences.Preferences = null,
    iphone_window: ?*adw.Window = null,
    font_size_window: ?*adw.Window = null,
    font_size_spin: ?*gtk.SpinButton = null,
    session_end: ?*session_end_window.Window = null,
    rename_window: ?*adw.Window = null,
    rename_heading: ?*gtk.Label = null,
    rename_entry: ?*gtk.Entry = null,
    rename_submit: ?*gtk.Button = null,
    rename_cancel: ?*gtk.Button = null,
    rename_error: ?*gtk.Label = null,
    rename_target: ?RenameTarget = null,
    rename_pending: bool = false,
    connections: ?*connections_window.Window = null,
    codex_send: ?*const fn (?*anyopaque, []const u8) anyerror!void = null,
    codex_context: ?*anyopaque = null,
    ssh_target_window: ?*adw.Window = null,
    ssh_target_title: ?*gtk.Label = null,
    ssh_target_workspace: ?*gtk.Label = null,
    ssh_target_destination_label: ?*gtk.Label = null,
    ssh_target_entry: ?*gtk.Entry = null,
    ssh_target_route_toggle: ?*gtk.ToggleButton = null,
    ssh_target_lan_address: ?*gtk.Entry = null,
    ssh_target_lan_port: ?*gtk.Entry = null,
    ssh_target_wan_host: ?*gtk.Entry = null,
    ssh_target_wan_port: ?*gtk.Entry = null,
    ssh_target_doh_primary: ?*gtk.Entry = null,
    ssh_target_doh_secondary: ?*gtk.Entry = null,
    ssh_target_error: ?*gtk.Label = null,
    ssh_target_status: ?*gtk.Label = null,
    ssh_target_cancel: ?*gtk.Button = null,
    ssh_target_save: ?*gtk.Button = null,
    ssh_target_draft: control.HostDraft = .{},
    ssh_target_syncing: bool = false,
    new_workspace_window: ?*adw.Window = null,
    new_workspace_entry: ?*gtk.Entry = null,
    session_directory: ?*session_directory_window.Window = null,
    session_create_yolo: ?*gtk.CheckButton = null,
    session_create_local_cli: ?*gtk.CheckButton = null,
    session_create_browse: ?*gtk.Button = null,
    session_context_pending: bool = false,
    active_tab_drag_index: usize = 0,
    sidebar_selecting: bool = false,
    sidebar_selection: std.ArrayListUnmanaged(control.SessionIdentity) = .empty,
    sidebar_tab_id: ?[]u8 = null,
    sidebar_add: ?*gtk.Button = null,
    session_endpoint_status: ?*gtk.Label = null,
    session_history_box: ?*gtk.Box = null,
    editor: session_editor_window.State = .{},
    editor_engine: editor_interaction.Engine = .{},
    editor_open: bool = false,
    editor_applying: bool = false,
    editor_submission: u64 = 0,
    editor_history_json: ?[]u8 = null,
    editor_detail_json: ?[]u8 = null,
    session_history_rows: ?*gtk.ListBox = null,
    session_history_details: ?*gtk.Label = null,
    session_history_copy: ?*gtk.Button = null,
    session_editor_scroll: ?*gtk.ScrolledWindow = null,
    session_editor_body: ?*gtk.Box = null,
    session_appserver_field: ?*gtk.Box = null,
    session_history_search: ?*gtk.Entry = null,
    session_history_more: ?*gtk.Button = null,
    creation_generation: u64 = 0,
    end_engine: editor_interaction.Engine = .{},
    end_submission: u64 = 0,
    session_create_configuring: bool = false,
    session_create_window: ?*adw.Window = null,
    session_create_target: ?*gtk.Label = null,
    session_create_name: ?*gtk.Entry = null,
    session_create_directory: ?*gtk.Entry = null,
    session_create_command: ?*gtk.Entry = null,
    session_create_auto_restart: ?*gtk.CheckButton = null,
    session_create_command_label: ?*gtk.Label = null,
    session_create_submit: ?*gtk.Button = null,
    session_create_cancel: ?*gtk.Button = null,
    session_create_error: ?*gtk.Label = null,
    session_create_pending: bool = false,
    creation_request_id: ?[]u8 = null,
    creation_request_intent: ?[]u8 = null,
    session_create_kind: ?SessionCreateKind = null,
    session_create_machine: ?*gtk.DropDown = null,
    session_create_type: ?*gtk.DropDown = null,
    session_create_mode: ?*gtk.DropDown = null,
    session_create_mode_bar: ?*gtk.Box = null,
    session_create_new_button: ?*gtk.Button = null,
    session_create_resume_button: ?*gtk.Button = null,
    session_create_advanced: ?*gtk.Expander = null,
    session_create_appserver_label: ?*gtk.Label = null,
    session_create_appserver_row: ?*gtk.Box = null,
    session_create_name_label: ?*gtk.Label = null,
    session_create_directory_label: ?*gtk.Label = null,
    session_create_directory_row: ?*gtk.Box = null,
    session_create_resume_label: ?*gtk.Label = null,
    session_create_resume_id: ?*gtk.Entry = null,
    session_create_endpoint: ?*gtk.DropDown = null,
    session_create_endpoints: ?std.json.Parsed([]control.CodexEndpoint) = null,
    session_create_groups: ?std.json.Parsed([]control.SessionTargetGroup) = null,
    session_create_workspace_id: ?[]u8 = null,
    session_create_tab_id: ?[]u8 = null,
    session_create_target_id: ?[]u8 = null,
    lifecycle: shell_lifecycle.State = .{},
    rendering: bool = false,
    last_pane_click_id: ?[]u8 = null,
    last_pane_click_at_us: i64 = 0,
    pending_pane_action: ?PendingPaneAction = null,
    pending_pane_action_source: ?c_uint = null,
    pending_pane_summary_id: ?[]u8 = null,
    pending_pane_summary_source: ?c_uint = null,
    pending_pane_move: ?PendingPaneMove = null,
    pending_pane_move_source: ?c_uint = null,
    active_pane_drag: ?pane_interaction.PaneDrop = null,
    active_tab_drag: ?[]u8 = null,
    pending_tab_move_index: ?usize = null,
    active_drag_source: ?*gtk.DragSource = null,
    pane_pointer_down: bool = false,
    deferred_pointer_render: bool = false,
    suppress_pane_click: bool = false,
    pending_tab_selection_id: ?[]u8 = null,
    pending_tab_selection_source: ?c_uint = null,
    pane_summary_popover: ?*gtk.Popover = null,
    pane_buttons: std.ArrayList(PaneButtonRef) = .empty,
    map_popovers: std.ArrayList(*gtk.Popover) = .empty,
    // Borrowed from the card MenuButtons; never detach these manually.
    map_action_popovers: std.ArrayList(*gtk.Popover) = .empty,
    pending_map_render_source: ?c_uint = null,
    session_popovers: std.ArrayList(*gtk.Popover) = .empty,
    map_bodies: std.ArrayList(MapBodyRef) = .empty,
    located_pane_id: ?[]u8 = null,
    located_workspace_id: ?[]u8 = null,
    locate_pulse_pending: bool = false,
    locate_value: f64 = 1,
    locate_animation: ?*adw.Animation = null,
    locate_clock: ?*gdk.FrameClock = null,
    locate_paint_handler: c_ulong = 0,

    pub fn create(
        allocator: std.mem.Allocator,
        app: *Application,
        host_window: *Window,
        model: *control.Model,
        inbox_model: *inbox.Model,
        native_executor: ?*presentation_executor.Executor,
        action_fn: ActionFn,
        pane_drop_fn: PaneDropFn,
        action_context: ?*anyopaque,
        controller_id: u64,
    ) !*Controller {
        const self = try allocator.create(Controller);
        errdefer allocator.destroy(self);

        const controller_root = gtk.Box.new(.vertical, 0);
        controller_root.as(gtk.Widget).addCssClass(
            "polar-station-controller-root",
        );
        controller_root.as(gtk.Widget).setSizeRequest(
            layout_policy.layout_map_min_width,
            -1,
        );
        controller_root.as(gtk.Widget).setVisible(1);
        _ = controller_root.as(gobject.Object).refSink();
        const header = gtk.Box.new(.horizontal, 4);
        header.as(gtk.Widget).addCssClass("polar-tab-header");
        const header_actions = menuBox();
        const header_more = gtk.MenuButton.new();
        header_more.setIconName("view-more-symbolic");
        header_more.as(gtk.Widget).setTooltipText("Workspace actions");
        const header_popover = gtk.Popover.new();
        header_popover.setChild(header_actions.as(gtk.Widget));
        header_more.setPopover(header_popover);
        header.as(gtk.Widget).setMarginTop(5);
        header.as(gtk.Widget).setMarginBottom(5);
        header.as(gtk.Widget).setMarginStart(6);
        header.as(gtk.Widget).setMarginEnd(6);
        const title = adw.WindowTitle.new("Polar Station", "Connecting");
        title.as(gtk.Widget).setVisible(0);
        controller_root.append(title.as(gtk.Widget));

        const workspace_menu = gtk.MenuButton.new();
        workspace_menu.setCanShrink(1);
        workspace_menu.as(gtk.Widget).setTooltipText("Choose Workspace");
        const workspace_menu_content = gtk.Box.new(.horizontal, 5);
        const workspace_menu_label = label("Workspace", "heading");
        workspace_menu_label.setEllipsize(.end);
        workspace_menu_label.setMaxWidthChars(10);
        workspace_menu.as(gtk.Widget).setHexpand(1);
        const workspace_indicator = gtk.Image.newFromIconName(
            "view-sort-ascending-symbolic",
        );
        workspace_menu_content.append(workspace_menu_label.as(gtk.Widget));
        workspace_menu_content.append(workspace_indicator.as(gtk.Widget));
        workspace_menu.setChild(workspace_menu_content.as(gtk.Widget));
        const workspace_popover = gtk.Popover.new();
        const workspace_menu_box = menuBox();
        workspace_popover.setChild(workspace_menu_box.as(gtk.Widget));
        workspace_menu.setPopover(workspace_popover);
        header.append(workspace_menu.as(gtk.Widget));

        const rename_workspace = gtk.Button.newWithLabel("Rename Workspace");
        rename_workspace.setCanShrink(1);
        rename_workspace.as(gtk.Widget).setTooltipText("Rename Workspace");
        header_actions.append(rename_workspace.as(gtk.Widget));

        const connection_badge = gtk.Button.new();
        const connection_badge_content = gtk.Box.new(.vertical, 0);
        connection_badge.as(gtk.Widget).setSizeRequest(34, 34);
        connection_badge.as(gtk.Widget).setHalign(.center);
        connection_badge.as(gtk.Widget).setValign(.center);
        connection_badge.as(gtk.Widget).addCssClass("flat");
        const connection_status_image = gtk.Image.newFromIconName(
            "network-transmit-receive-symbolic",
        );
        const connection_label = label("SSH", "caption");
        connection_label.setXalign(0.5);
        connection_badge_content.append(connection_status_image.as(gtk.Widget));
        connection_badge_content.append(connection_label.as(gtk.Widget));
        connection_badge.setChild(connection_badge_content.as(gtk.Widget));
        header.append(connection_badge.as(gtk.Widget));
        header.append(header_more.as(gtk.Widget));
        connection_label.as(gtk.Widget).setVisible(0);

        const add_tab = gtk.Button.newWithLabel("Add Tab");
        add_tab.setCanShrink(1);
        add_tab.as(gtk.Widget).setTooltipText("Add Tab");
        header_actions.append(add_tab.as(gtk.Widget));

        const refresh = gtk.Button.newWithLabel("Restore Saved Layout…");
        refresh.setCanShrink(1);
        refresh.as(gtk.Widget).setTooltipText("Restore Saved Layout…");
        const restore_popover = gtk.Popover.new();
        const restore_box = gtk.Box.new(.vertical, 8);
        restore_box.as(gtk.Widget).setMarginTop(12);
        restore_box.as(gtk.Widget).setMarginBottom(12);
        restore_box.as(gtk.Widget).setMarginStart(12);
        restore_box.as(gtk.Widget).setMarginEnd(12);
        const restore_title = label("Restore Saved Layout?", "heading");
        const restore_message = label(
            "Restore replaces tabs and panes that differ from the saved Layout. Save non-tmux work first. tmux Sessions keep running.",
            "caption",
        );
        restore_message.setWrap(1);
        restore_message.setMaxWidthChars(42);
        const restore_actions = gtk.Box.new(.horizontal, 6);
        const restore_cancel = gtk.Button.newWithLabel("Cancel");
        const restore_confirm = gtk.Button.newWithLabel("Restore Layout");
        restore_confirm.as(gtk.Widget).addCssClass("suggested-action");
        restore_actions.append(restore_cancel.as(gtk.Widget));
        restore_actions.append(restore_confirm.as(gtk.Widget));
        restore_box.append(restore_title.as(gtk.Widget));
        restore_box.append(restore_message.as(gtk.Widget));
        restore_box.append(restore_actions.as(gtk.Widget));
        restore_popover.setChild(restore_box.as(gtk.Widget));
        restore_popover.as(gtk.Widget).setParent(refresh.as(gtk.Widget));
        header_actions.append(refresh.as(gtk.Widget));

        const lifecycle = gtk.Button.new();
        const lifecycle_image = gtk.Image.newFromIconName(
            "media-playback-start-symbolic",
        );
        const lifecycle_content = gtk.Box.new(.horizontal, 6);
        lifecycle_content.append(lifecycle_image.as(gtk.Widget));
        lifecycle_content.append(label("Open or repair Workspace", "caption").as(gtk.Widget));
        lifecycle.setChild(lifecycle_content.as(gtk.Widget));
        lifecycle.setCanShrink(1);
        lifecycle.as(gtk.Widget).setTooltipText("Open or repair Workspace");
        lifecycle.as(gtk.Widget).addCssClass("suggested-action");
        header_actions.append(lifecycle.as(gtk.Widget));
        controller_root.append(header.as(gtk.Widget));

        const sessions_list = gtk.ListBox.new();
        sessions_list.setSelectionMode(.none);
        const sessions_scroller = gtk.ScrolledWindow.new();
        sessions_scroller.setPolicy(.external, .automatic);
        sessions_scroller.setMinContentWidth(
            layout_policy.sessions_content_width,
        );
        sessions_scroller.setChild(sessions_list.as(gtk.Widget));
        sessions_scroller.as(gtk.Widget).setVexpand(1);

        const sessions_title = label("", "title-3");
        sessions_title.setEllipsize(.end);
        sessions_title.setMaxWidthChars(1);
        sessions_title.as(gtk.Widget).setHexpand(1);
        const sessions_refresh = gtk.Button.newFromIconName("view-refresh-symbolic");
        sessions_refresh.as(gtk.Widget).addCssClass("flat");
        const sessions_close = gtk.Button.newFromIconName("window-close-symbolic");
        sessions_close.as(gtk.Widget).addCssClass("flat");
        const sessions_header_spacer = gtk.Box.new(.horizontal, 0);
        sessions_header_spacer.as(gtk.Widget).setHexpand(1);
        const sessions_header = gtk.Box.new(.horizontal, 4);
        sessions_header.append(sessions_title.as(gtk.Widget));
        sessions_header.append(sessions_header_spacer.as(gtk.Widget));
        const new_session = gtk.Button.newFromIconName("list-add-symbolic");
        new_session.as(gtk.Widget).setTooltipText("New Session");
        connect(new_session, Controller, newLocalShellClicked, self);
        const select_sessions = gtk.ToggleButton.new();
        select_sessions.as(gtk.Button).setIconName("object-select-symbolic");
        select_sessions.as(gtk.Widget).setTooltipText("Select sessions");
        _ = gtk.ToggleButton.signals.toggled.connect(select_sessions, *Controller, sidebarSelectChanged, self, .{});
        sessions_header.append(select_sessions.as(gtk.Widget));
        const add_sessions = gtk.Button.newFromIconName("go-next-symbolic");
        add_sessions.as(gtk.Widget).setTooltipText("Add selected");
        add_sessions.as(gtk.Widget).setVisible(0);
        connect(add_sessions, Controller, sidebarAddClicked, self);
        sessions_header.append(add_sessions.as(gtk.Widget));
        sessions_header.append(new_session.as(gtk.Widget));
        sessions_header.append(sessions_refresh.as(gtk.Widget));
        sessions_header.append(sessions_close.as(gtk.Widget));
        const sessions_subtitle = label("", "caption");
        sessions_subtitle.setEllipsize(.end);
        sessions_subtitle.as(gtk.Widget).addCssClass("dim-label");

        const sessions_panel = gtk.Box.new(.vertical, 6);
        sessions_panel.as(gtk.Widget).setSizeRequest(
            layout_policy.sessions_content_width,
            -1,
        );
        sessions_panel.as(gtk.Widget).setVisible(1);
        sessions_panel.as(gtk.Widget).setVexpand(1);
        setMargins(
            sessions_panel.as(gtk.Widget),
            layout_policy.controller_section_margin,
        );
        sessions_panel.append(sessions_header.as(gtk.Widget));
        sessions_panel.append(sessions_subtitle.as(gtk.Widget));
        const sessions_search = gtk.Entry.new();
        sessions_search.setPlaceholderText("Search sessions");
        const session_types = gtk.StringList.new(null);
        for ([_][:0]const u8{ "All types", "Codex", "Claude", "Terminal" }) |kind_name| session_types.append(kind_name);
        const sessions_type = gtk.DropDown.new(session_types.as(@import("gio").ListModel), null);
        sessions_panel.append(sessions_search.as(gtk.Widget));
        sessions_panel.append(sessions_type.as(gtk.Widget));
        sessions_panel.append(sessions_scroller.as(gtk.Widget));
        sessions_panel.as(gtk.Widget).addCssClass("sidebar");

        const sessions_revealer = gtk.Revealer.new();
        // Wayland compositors do not guarantee a client-requested resize while
        // a mapped toplevel is animating. Reveal atomically so the sidebar and
        // its accessible children cannot remain stuck at a zero allocation.
        sessions_revealer.setTransitionType(.none);
        sessions_revealer.setTransitionDuration(0);
        sessions_revealer.setChild(sessions_panel.as(gtk.Widget));
        sessions_revealer.as(gtk.Widget).setSizeRequest(
            layout_policy.sessions_outer_width,
            -1,
        );
        sessions_revealer.as(gtk.Widget).setVexpand(1);

        const layout_box = gtk.Box.new(.vertical, layout_policy.layout_tab_spacing);
        layout_box.as(gtk.Widget).setHexpand(1);
        setMargins(
            layout_box.as(gtk.Widget),
            layout_policy.layout_list_inset,
        );
        const layout_scroller = gtk.ScrolledWindow.new();
        layout_scroller.setPolicy(.external, .automatic);
        layout_scroller.setMaxContentWidth(
            layout_policy.layout_map_content_width,
        );
        layout_scroller.setPropagateNaturalWidth(0);
        layout_scroller.setChild(layout_box.as(gtk.Widget));
        layout_scroller.as(gtk.Widget).setSizeRequest(
            layout_policy.layout_map_outer_width,
            -1,
        );
        layout_scroller.as(gtk.Widget).setHexpand(1);
        layout_scroller.as(gtk.Widget).setVexpand(1);

        // The outer terminal boundary is represented by this native overlay
        // handle. It is deliberately separate from the Sessions/Map toggle
        // divider: dragging it in either direction resizes the Map within the
        // native 200-390 logical-pixel bounds.
        const layout_overlay = gtk.Overlay.new();
        layout_overlay.setChild(layout_scroller.as(gtk.Widget));
        const width_resize_handle = gtk.Box.new(.vertical, 0);
        width_resize_handle.as(gtk.Widget).setSizeRequest(8, -1);
        width_resize_handle.as(gtk.Widget).setHalign(.end);
        width_resize_handle.as(gtk.Widget).setValign(.fill);
        width_resize_handle.as(gtk.Widget).setVexpand(1);
        width_resize_handle.as(gtk.Widget).setCursorFromName("col-resize");
        layout_overlay.addOverlay(width_resize_handle.as(gtk.Widget));
        layout_overlay.setMeasureOverlay(width_resize_handle.as(gtk.Widget), 0);

        const body = gtk.Box.new(.horizontal, 0);
        body.as(gtk.Widget).setVexpand(1);
        const sessions_separator = gtk.Separator.new(.vertical);
        const layout_body = gtk.Box.new(.horizontal, 0);
        layout_body.as(gtk.Widget).setVexpand(1);
        body.append(sessions_revealer.as(gtk.Widget));
        body.append(sessions_separator.as(gtk.Widget));
        layout_body.append(layout_overlay.as(gtk.Widget));
        const layout_header = gtk.Box.new(.horizontal, 4);
        setMargins(layout_header.as(gtk.Widget), 10);
        const layout_title = label("Layout", "heading");
        layout_title.setWrap(0);
        layout_title.as(gtk.Widget).setValign(.center);
        layout_title.setEllipsize(.end);
        layout_title.as(gtk.Widget).setHexpand(1);
        layout_header.append(layout_title.as(gtk.Widget));
        const layout_region = gtk.Box.new(.vertical, 0);
        layout_region.append(layout_header.as(gtk.Widget));
        layout_region.append(layout_body.as(gtk.Widget));
        body.append(layout_region.as(gtk.Widget));

        const inbox_box = gtk.Box.new(.vertical, 10);
        setMargins(inbox_box.as(gtk.Widget), 12);
        const inbox_scroller = gtk.ScrolledWindow.new();
        inbox_scroller.setPolicy(.external, .automatic);
        inbox_scroller.setChild(inbox_box.as(gtk.Widget));
        inbox_scroller.as(gtk.Widget).setSizeRequest(340, -1);
        inbox_scroller.as(gtk.Widget).setVexpand(1);
        body.append(inbox_scroller.as(gtk.Widget));

        const content = gtk.Box.new(.vertical, 0);
        content.as(gtk.Widget).setVexpand(1);
        content.append(body.as(gtk.Widget));
        controller_root.append(content.as(gtk.Widget));

        const sessions_button = gtk.ToggleButton.new();
        sessions_button.as(gtk.Widget).setSizeRequest(32, 32);
        sessions_button.as(gtk.Widget).setValign(.center);
        const sessions_control = iconLabelControl(
            "utilities-terminal-symbolic",
            "",
        );
        sessions_button.as(gtk.Button).setChild(
            sessions_control.content.as(gtk.Widget),
        );
        sessions_button.as(gtk.Button).setCanShrink(1);
        sessions_button.as(gtk.Widget).addCssClass("flat");
        sessions_button.as(gtk.Widget).setTooltipText("Show or hide Sessions");
        const mode_button = gtk.ToggleButton.new();
        const mode_control = iconLabelControl(
            "view-dual-symbolic",
            "",
        );
        mode_button.as(gtk.Button).setChild(
            mode_control.content.as(gtk.Widget),
        );
        mode_button.as(gtk.Button).setCanShrink(1);
        mode_button.as(gtk.Widget).addCssClass("flat");
        mode_button.as(gtk.Widget).setTooltipText("Show or hide Layout");
        const resources = gtk.Button.new();
        const resources_control = iconLabelControl(
            "drive-harddisk-symbolic",
            "",
        );
        resources.setChild(resources_control.content.as(gtk.Widget));
        resources.setCanShrink(1);
        resources.as(gtk.Widget).addCssClass("flat");
        resources.as(gtk.Widget).setTooltipText("Open global Resources");
        const iphone = gtk.ToggleButton.new();
        const iphone_control = iconLabelControl(
            "mail-unread-symbolic",
            "",
        );
        iphone.as(gtk.Button).setChild(iphone_control.content.as(gtk.Widget));
        iphone.as(gtk.Button).setCanShrink(1);
        iphone.as(gtk.Widget).addCssClass("flat");
        iphone.as(gtk.Widget).setTooltipText("Show or hide Inbox");

        const more = gtk.MenuButton.new();
        const more_control = iconLabelControl(
            "view-more-symbolic",
            "",
        );
        more.setChild(more_control.content.as(gtk.Widget));
        more.setCanShrink(1);
        more.as(gtk.Widget).addCssClass("flat");
        const more_popover = gtk.Popover.new();
        const more_menu_box = menuBox();
        more_popover.setChild(more_menu_box.as(gtk.Widget));
        more.setPopover(more_popover);

        const footer = gtk.Box.new(.horizontal, 2);
        footer.as(gtk.Widget).addCssClass("polar-station-bottom-strip");
        footer.setHomogeneous(1);
        footer.as(gtk.Widget).setMarginTop(4);
        footer.as(gtk.Widget).setMarginBottom(4);
        footer.as(gtk.Widget).setMarginStart(3);
        footer.as(gtk.Widget).setMarginEnd(3);
        layout_header.append(sessions_button.as(gtk.Widget));
        mode_button.as(gtk.Widget).setHexpand(1);
        resources.as(gtk.Widget).setHexpand(1);
        iphone.as(gtk.Widget).setHexpand(1);
        more.as(gtk.Widget).setHexpand(1);
        footer.append(mode_button.as(gtk.Widget));
        footer.append(iphone.as(gtk.Widget));
        footer.append(resources.as(gtk.Widget));
        footer.append(more.as(gtk.Widget));
        controller_root.append(footer.as(gtk.Widget));
        const controller_viewport = gtk.ScrolledWindow.new();
        controller_viewport.setPolicy(.external, .external);
        controller_viewport.setPropagateNaturalWidth(0);
        controller_viewport.setPropagateNaturalHeight(0);
        const workspace_stack = gtk.Stack.new();
        const workspace_chooser = menuBox();
        _ = workspace_stack.addNamed(controller_root.as(gtk.Widget), "bound");
        _ = workspace_stack.addNamed(workspace_chooser.as(gtk.Widget), "unbound");
        controller_viewport.setChild(workspace_stack.as(gtk.Widget));
        controller_viewport.as(gtk.Widget).setSizeRequest(
            layout_policy.layout_map_min_width,
            -1,
        );
        controller_viewport.as(gtk.Widget).setVisible(0);
        _ = controller_viewport.as(gobject.Object).refSink();
        controller_root.as(gobject.Object).unref();
        const css_display = host_window.as(gtk.Widget).getDisplay();
        const css_provider = gtk.CssProvider.new();
        css_provider.loadFromString(controller_css);
        gtk.StyleContext.addProviderForDisplay(
            css_display,
            css_provider.as(gtk.StyleProvider),
            gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 4,
        );
        errdefer {
            gtk.StyleContext.removeProviderForDisplay(
                css_display,
                css_provider.as(gtk.StyleProvider),
            );
            css_provider.unref();
        }

        self.* = .{
            .allocator = allocator,
            .app = app,
            .host_window = host_window,
            .model = model,
            .inbox_model = inbox_model,
            .native_executor = native_executor,
            .action_fn = action_fn,
            .pane_drop_fn = pane_drop_fn,
            .action_context = action_context,
            .controller_id = controller_id,
            .pane_content = controller_viewport.as(gtk.Widget),
            .workspace_stack = workspace_stack, .workspace_chooser = workspace_chooser,
            .css_display = css_display,
            .css_provider = css_provider,
            .window_title = title,
            .connection_status_image = connection_status_image,
            .connection_status_label = connection_label,
            .connection_badge = connection_badge,
            .workspace_menu = workspace_menu,
            .workspace_menu_label = workspace_menu_label,
            .workspace_menu_box = workspace_menu_box,
            .rename_workspace_button = rename_workspace,
            .add_tab_button = add_tab,
            .refresh_button = refresh,
            .restore_popover = restore_popover,
            .lifecycle_button = lifecycle,
            .lifecycle_image = lifecycle_image,
            .sessions_revealer = sessions_revealer,
            .sessions_panel = sessions_panel,
            .sessions_separator = sessions_separator,
            .layout_region = layout_region,
            .sessions_title = sessions_title,
            .sessions_hint = sessions_subtitle,
            .sessions_search = sessions_search,
            .sessions_type = sessions_type,
            .sessions_refresh_button = sessions_refresh,
            .sessions_close_button = sessions_close,
            .sidebar_add = add_sessions,
            .sessions_scroller = sessions_scroller,
            .sessions_list = sessions_list,
            .layout_scroller = layout_scroller,
            .layout_overlay = layout_overlay,
            .width_resize_handle = width_resize_handle,
            .layout_box = layout_box,
            .inbox_scroller = inbox_scroller,
            .inbox_box = inbox_box,
            .sessions_button = sessions_button,
            .sessions_button_label = sessions_control.text,
            .mode_button = mode_button,
            .mode_button_label = mode_control.text,
            .resources_button = resources,
            .resources_image = resources_control.image,
            .resources_button_label = resources_control.text,
            .iphone_button = iphone,
            .iphone_button_label = iphone_control.text,
            .more_menu = more,
            .more_menu_label = more_control.text,
            .more_menu_box = more_menu_box,
        };
        host_window.as(gobject.Object).weakRef(hostWindowWeakNotify, self);

        self.width_preferences_store =
            width_preferences.Preferences.init(allocator) catch null;
        self.map_width = if (self.width_preferences_store) |*preferences|
            preferences.loadOrDefault(host_window.as(gtk.Widget).getWidth())
        else
            layout_policy.layout_map_max_width;
        self.width_drag_start = self.map_width;
        self.layout_scroller.as(gtk.Widget).setSizeRequest(
            self.map_width,
            -1,
        );
        self.layout_scroller.setMaxContentWidth(
            layout_policy.layoutMapContentWidth(self.map_width),
        );

        connect(rename_workspace, Controller, renameWorkspaceClicked, self);
        connect(connection_badge, Controller, sshTargetClicked, self);
        connect(add_tab, Controller, addTabClicked, self);
        connect(refresh, Controller, refreshClicked, self);
        connect(restore_cancel, Controller, restoreCancelClicked, self);
        connect(restore_confirm, Controller, restoreConfirmClicked, self);
        connect(sessions_refresh, Controller, sessionsRefreshClicked, self);
        connect(sessions_close, Controller, closeSessionsClicked, self);
        connect(lifecycle, Controller, lifecycleClicked, self);
        connect(resources, Controller, resourcesClicked, self);
        _ = gtk.ToggleButton.signals.toggled.connect(
            iphone,
            *Controller,
            inboxToggled,
            self,
            .{},
        );
        _ = gtk.ToggleButton.signals.toggled.connect(
            sessions_button,
            *Controller,
            sessionsToggled,
            self,
            .{},
        );
        _ = gtk.ToggleButton.signals.toggled.connect(
            mode_button,
            *Controller,
            layoutToggled,
            self,
            .{},
        );
        const width_drag = gtk.GestureDrag.new();
        _ = gtk.GestureDrag.signals.drag_begin.connect(
            width_drag,
            *Controller,
            widthResizeDragBegin,
            self,
            .{},
        );
        _ = gtk.GestureDrag.signals.drag_update.connect(
            width_drag,
            *Controller,
            widthResizeDragUpdate,
            self,
            .{},
        );
        _ = gtk.GestureDrag.signals.drag_end.connect(
            width_drag,
            *Controller,
            widthResizeDragEnd,
            self,
            .{},
        );
        self.width_resize_handle.as(gtk.Widget).addController(
            width_drag.as(gtk.EventController),
        );
        const key_controller = gtk.EventControllerKey.new();
        _ = gtk.EventControllerKey.signals.key_pressed.connect(
            key_controller,
            *Controller,
            keyPressed,
            self,
            .{},
        );
        controller_root.as(gtk.Widget).addController(
            key_controller.as(gtk.EventController),
        );

        self.applySidebarLayout(self.sessions_visible);
        self.render();
        _ = gtk.Editable.signals.changed.connect(sessions_search.as(gtk.Editable), *Controller, sessionsFilterChanged, self, .{});
        _ = gobject.Object.signals.notify.connect(sessions_type.as(gobject.Object), *Controller, sessionsTypeChanged, self, .{ .detail = "selected" });
        return self;
    }

    pub fn ownsHostWindow(self: *const Controller, window: *Window) bool {
        return self.host_alive and self.host_window == window;
    }

    pub fn performApplicationShortcut(
        self: *Controller,
        action: shortcuts.Action,
    ) bool {
        const projection = self.model.projection() orelse return false;
        switch (action) {
            .open_inquiry, .insert_idea_node, .edit_idea_instruction, .quote_selection, .open_file_reference => return false,
            .resume_codex => {
                if (self.session_create_window) |window| {
                    if (window.as(gtk.Widget).getVisible() != 0) {
                        window.as(gtk.Window).present();
                        return true;
                    }
                }
                const target = if (self.native_executor) |executor| executor.quickCreateTarget(self.host_window) catch {
                    self.model.setLocalError("The current Tab is unavailable or full.");
                    self.render();
                    return true;
                } else return false;
                self.presentSessionCreateEditor(.codex_agent, if (target.source) |source| source.execution_target_id else projection.sessions.local_execution_target_id);
                if (self.session_create_mode) |mode| mode.setSelected(1);
                if (self.session_history_rows) |rows| _ = rows.as(gtk.Widget).grabFocus();
            },
            .new_shell, .new_codex => {
                const target = if (self.native_executor) |executor| executor.quickCreateTarget(self.host_window) catch {
                    self.model.setLocalError("The current Tab is unavailable or full.");
                    return true;
                } else return false;
                self.presentSessionCreateEditor(if (action == .new_codex) .codex_agent else .local_shell, if (target.source) |source| source.execution_target_id else projection.sessions.local_execution_target_id);
            },
            .workspace_mode_toggle => {
                const projected = projection.action("workspace.mode.toggle") orelse
                    return false;
                if (!projected.enabled) return false;
                const selected_tab_id = self.nativeSelectedTabID() orelse return false;
                self.dispatch(.{ .toggle_display_mode = .{
                    .tab_id = selected_tab_id,
                } });
            },
            .new_workspace => {
                const projected = projection.action("workspace.create") orelse
                    return false;
                if (!projected.enabled) return false;
                self.presentNewWorkspaceEditor();
            },
            .rename_workspace => {
                const projected = projection.action("workspace.rename") orelse
                    return false;
                if (!projected.enabled) return false;
                self.presentRenameWorkspaceEditor();
            },
            .refresh_sessions => {
                if (!projection.sessions.refresh_action.enabled) return false;
                self.dispatch(.{ .invoke_action = .{
                    .action_id = projection.sessions.refresh_action.id,
                } });
            },
            else => return false,
        }
        return true;
    }

    pub fn deinit(self: *Controller) void {
        if(self.idea_binding_pane) |v| self.allocator.free(v);
        if(self.idea_binding_view) |v| self.allocator.free(v);
        self.chooser_inputs.deinit(self.allocator);
        self.editor_engine.deinit();
        self.end_engine.deinit();
        if (self.editor_history_json) |v| self.allocator.free(v);
        if (self.editor_detail_json) |v| self.allocator.free(v);
        self.clearSidebarSelection();
        self.sidebar_selection.deinit(self.allocator);
        if (self.sidebar_tab_id) |id| self.allocator.free(id);
        self.header_inputs.deinit(self.allocator);
        self.workspace_menu_inputs.deinit(self.allocator);
        self.bottom_inputs.deinit(self.allocator);
        self.more_inputs.deinit(self.allocator);
        var collapsed_keys = self.collapsed_session_groups.keyIterator();
        while (collapsed_keys.next()) |key| self.allocator.free(key.*);
        self.collapsed_session_groups.deinit(self.allocator);
        self.sessions_inputs.deinit(self.allocator);
        self.map_inputs.deinit(self.allocator);
        self.inbox_inputs.deinit(self.allocator);
        for (self.inbox_filter_values) |value| if (value) |v| self.allocator.free(v);
        self.rendering = true; // Popover close signals must not schedule a new render.
        detachPopover(self.restore_popover);
        self.clearSessionLocate();
        self.cancelPaneDrag();
        self.cancelPendingPaneSummary();
        self.clearPendingPaneAction();
        self.clearPendingPaneMove();
        self.clearPendingTabSelection();
        self.clearSessionRows();
        self.clearWorkspaceMap();
        if (self.pending_map_render_source) |source| _ = glib.Source.remove(source);
        self.map_action_popovers.deinit(self.allocator);
        if (self.iphone_window) |window| auxiliary.destroy(window);
        if (self.font_size_window) |window| auxiliary.destroy(window);
        self.clearRenameTarget();
        if (self.session_directory) |window| window.deinit();
        if (self.session_end) |window| window.deinit();
        if (self.rename_window) |window| {
            auxiliary.destroy(window);
        }
        if (self.connections) |window| {
            window.deinit();
            self.allocator.destroy(window);
        }
        if (self.ssh_target_window) |window| auxiliary.destroy(window);
        self.ssh_target_draft.deinit(self.allocator);
        if (self.new_workspace_window) |window| auxiliary.destroy(window);
        if (self.session_create_window) |window| auxiliary.destroy(window);
        if (self.creation_request_id) |value| self.allocator.free(value);
        if (self.creation_request_intent) |value| self.allocator.free(value);
        if (self.session_create_workspace_id) |value| self.allocator.free(value);
        if (self.session_create_tab_id) |value| self.allocator.free(value);
        if (self.session_create_target_id) |value| self.allocator.free(value);
        if (self.session_create_groups) |value| value.deinit();
        if (self.session_create_endpoints) |value| value.deinit();
        if (self.last_pane_click_id) |value| self.allocator.free(value);
        self.pane_buttons.deinit(self.allocator);
        self.map_popovers.deinit(self.allocator);
        self.session_popovers.deinit(self.allocator);
        self.map_bodies.deinit(self.allocator);
        gtk.StyleContext.removeProviderForDisplay(
            self.css_display,
            self.css_provider.as(gtk.StyleProvider),
        );
        self.css_provider.unref();
        if (self.editor.preferences_store) |*store| store.deinit();
        if (self.width_preferences_store) |*preferences| {
            preferences.deinit();
        }
        if (self.host_alive) {
            self.host_window.as(gobject.Object).weakUnref(
                hostWindowWeakNotify,
                self,
            );
            self.host_window.detachPolarStationPane(self.pane_content);
        }
        self.pane_content.as(gobject.Object).unref();
        const allocator = self.allocator;
        self.* = undefined;
        allocator.destroy(self);
    }

    pub fn toggle(self: *Controller) void {
        switch (self.lifecycle.toggleControlCenter()) {
            .create_and_show, .show_existing => self.show(),
            .hide_existing => self.hide(),
        }
    }

    pub fn show(self: *Controller) void {
        if (!self.host_alive) return;
        if (self.lifecycle.control_center != .visible) {
            self.lifecycle.control_center = .visible;
        }
        self.host_window.showPolarStationPane(
            self.pane_content,
            self.controllerWidth(),
        );
        self.host_window.as(gtk.Window).present();
    }

    pub fn presentInboxSummary(self: *Controller) void {
        self.inbox_visible = true;
        self.savePanelVisibility();
        self.applySidebarLayout(self.sessions_visible);
        self.render();
        self.show();
    }

    pub fn hide(self: *Controller) void {
        self.cancelPaneDrag();
        if (self.lifecycle.control_center == .absent) return;
        self.lifecycle.controlCenterHidden();
        if (!self.host_alive) return;
        self.host_window.hidePolarStationPane(self.pane_content);
    }

    pub fn applyPresentationSettings(
        self: *Controller,
        layout_visible: bool,
        inbox_visible: bool,
        map_width: ?u16,
        inbox_width: ?u16,
        terminal_delivery_mode: []const u8,
        interface_text_scale_percent: u16,
    ) void {
        _ = layout_visible;
        _ = inbox_visible;
        self.restorePanelVisibility();
        self.terminal_delivery_mode = if (std.mem.eql(u8, terminal_delivery_mode, "send")) .send else .insert;
        if (map_width) |value| {
            const width = layout_policy.clampMapWidth(@intCast(value));
            if (width != self.map_width) {
                self.map_width = width;
                self.updateMapBodyGeometry();
            }
        }
        self.inbox_width = @intCast(inbox_width orelse 340);
        const css = std.fmt.allocPrintSentinel(
            self.allocator,
            "{s}\n.polar-station-controller-root {{ font-size: {d}%; }}",
            .{ controller_css, interface_text_scale_percent },
            0,
        ) catch null;
        if (css) |value| {
            defer self.allocator.free(value);
            self.css_provider.loadFromString(value);
        }
        self.inbox_scroller.as(gtk.Widget).setSizeRequest(
            self.inbox_width,
            -1,
        );
        self.applySidebarLayout(self.sessions_visible);
        self.render();
    }

    pub fn isVisible(self: *const Controller) bool {
        return self.lifecycle.control_center == .visible;
    }

    pub fn isHostedBy(self: *const Controller, window: *Window) bool {
        return self.host_alive and
            @intFromPtr(self.host_window) == @intFromPtr(window);
    }

    pub fn isOrphaned(self: *const Controller) bool {
        return !self.host_alive;
    }

    pub fn render(self: *Controller) void {
        const trace = @import("../../../polar_station_trace.zig");
        const call = trace.beginCall(61, self.controller_id);
        defer trace.endCall(62, self.controller_id, call);
        if (self.rendering) {
            if (self.pending_map_render_source == null) {
                self.pending_map_render_source = glib.idleAdd(mapActionRender, self);
            }
            return;
        }
        if (self.pane_pointer_down or self.active_pane_drag != null or self.active_tab_drag != null) {
            self.deferred_pointer_render = true;
            return;
        }
        self.deferred_pointer_render = false;
        self.rendering = true;
        defer self.rendering = false;

        self.refreshSshTargetRecovery();
        if (self.connections) |window| {
            if (self.model.projection()) |p| window.update(p.connections);
        }

        if (self.native_executor) |executor| {
            if (executor.nativeWorkspaceSnapshot() == null) {
                self.renderWorkspaceChooser(executor);
                return;
            }
        }
        self.workspace_stack.setVisibleChildName("bound");
        const projection = self.model.projection() orelse {
            self.window_title.setSubtitle(
                if (self.model.connection == .unavailable)
                    "Host unavailable"
                else
                    "Loading Workspace",
            );
            self.renderLoading();
            return;
        };
        self.restorePanelVisibility();
        var native_arena = std.heap.ArenaAllocator.init(self.allocator);
        defer native_arena.deinit();
        const workspace_map = if (self.native_executor) |executor|
            executor.nativeWorkspaceMap(
                native_arena.allocator(),
                projection,
                self.model.session_cache,
            ) catch null orelse return
        else
            return;

        self.window_title.setSubtitle("Control Center");
        self.renderWorkspaceMenu(projection, &workspace_map);
        // GTK owns composition; Host data cannot omit or reorder native regions.
        self.renderHeader(projection);
        self.renderSessions(projection);
        self.refreshEditorEndpoints(projection);
        if (self.located_pane_id) |id| {
            const executor = self.native_executor;
            if (executor == null or executor.?.nativePaneView(id) == null or !std.mem.eql(u8, self.located_workspace_id orelse "", workspace_map.workspace_id)) self.clearSessionLocate();
        }
        self.renderWorkspaceMap(projection, &workspace_map);
        self.renderBottomBar(projection);
        self.renderInbox();
        self.renderMoreMenu(projection);
        self.resetLayoutHorizontalPosition();
    }

    fn resetLayoutHorizontalPosition(self: *Controller) void {
        self.layout_scroller.getHadjustment().setValue(0);
    }

    fn renderLoading(self: *Controller) void {
        self.connection_status_image.setFromIconName(
            "network-offline-symbolic",
        );
        self.connection_status_image.as(gtk.Widget).setTooltipText(
            "SSH target status is loading",
        );

        self.header_inputs.deinit(self.allocator);
        self.workspace_menu_inputs.deinit(self.allocator);
        self.bottom_inputs.deinit(self.allocator);
        self.more_inputs.deinit(self.allocator);
        self.sessions_inputs.deinit(self.allocator);
        self.map_inputs.deinit(self.allocator);
        self.inbox_inputs.deinit(self.allocator);
        clearBox(self.layout_box);
        clearBox(self.inbox_box);
        self.clearSessionRows();
        clearBox(self.workspace_menu_box);
        clearBox(self.more_menu_box);
        self.applySidebarLayout(false);
        self.sessions_button.setActive(0);
        self.add_tab_button.as(gtk.Widget).setSensitive(0);
        self.refresh_button.as(gtk.Widget).setSensitive(0);
        self.lifecycle_button.as(gtk.Widget).setVisible(0);
        self.mode_button.as(gtk.Widget).setSensitive(0);
        self.resources_button.as(gtk.Widget).setSensitive(0);
        self.iphone_button.as(gtk.Widget).setSensitive(0);
        self.connection_badge.as(gtk.Widget).setSensitive(1);
        self.connection_status_label.setLabel("SSH");
        self.connection_status_image.setFromIconName(
            if (self.model.connection == .unavailable or self.model.last_error != null)
                "network-error-symbolic"
            else
                "view-refresh-symbolic",
        );
        self.connection_status_image.as(gtk.Widget).removeCssClass("success");
        self.connection_status_image.as(gtk.Widget).removeCssClass("warning");
        self.connection_status_image.as(gtk.Widget).removeCssClass("error");
        self.connection_status_image.as(gtk.Widget).addCssClass(
            if (self.model.connection == .unavailable or self.model.last_error != null)
                "error"
            else
                "warning",
        );
        const loading_tooltip = self.allocator.dupeZ(
            u8,
            self.model.last_error orelse "SSH target status is loading",
        ) catch return;
        defer self.allocator.free(loading_tooltip);
        self.connection_badge.as(gtk.Widget).setTooltipText(
            loading_tooltip,
        );
    }

    fn appendRestoreWorkspace(self: *Controller, box: *gtk.Box, id: []const u8, name: []const u8) void {
        const button = gtk.Button.new();
        const restore_title = std.fmt.allocPrint(self.allocator, "Restore {s}", .{name}) catch return;
        defer self.allocator.free(restore_title);
        setButtonLabel(self.allocator, button, restore_title) catch return;
        setWidgetName(self.allocator, button.as(gtk.Widget), id) catch return;
        connect(button, Controller, restoreWorkspaceClicked, self);
        box.append(button.as(gtk.Widget));
    }
    fn restoreWorkspaceClicked(button: *gtk.Button, self: *Controller) callconv(.c) void {
        const id = std.mem.span(button.as(gtk.Widget).getName());
        self.dispatch(.{ .invoke_action = .{ .action_id = "workspace.unarchive", .confirmation_id = id } });
    }
    fn renderWorkspaceChooser(self: *Controller, executor: *presentation_executor.Executor) void {
        const catalog = executor.nativeWorkspaceCatalog() orelse return;
        self.workspace_stack.setVisibleChildName("unbound");
        if (!self.isVisible()) self.show();
        if (!self.chooser_inputs.changed(self.allocator, .{ .catalog = catalog, .error_message = self.model.last_error })) return;
        clearBox(self.workspace_chooser);
        const title = gtk.Label.new("Open Workspace");
        self.workspace_chooser.append(title.as(gtk.Widget));
        for (catalog.workspaces) |workspace| {
            if (catalog.isArchived(workspace.workspace_id)) continue;
            const button = gtk.Button.new();
            setButtonLabel(self.allocator, button, workspace.workspace_name) catch return;
            setWidgetName(self.allocator, button.as(gtk.Widget), workspace.workspace_id) catch return;
            connect(button, Controller, workspaceClicked, self);
            self.workspace_chooser.append(button.as(gtk.Widget));
        }
        const create_button = gtk.Button.newWithLabel("New Workspace…");
        connect(create_button, Controller, newWorkspaceClicked, self);
        self.workspace_chooser.append(create_button.as(gtk.Widget));
        if (catalog.archived_workspace_ids.len > 0) {
            self.workspace_chooser.append(gtk.Label.new("Archived Workspaces").as(gtk.Widget));
            for (catalog.workspaces) |workspace| if (catalog.isArchived(workspace.workspace_id)) {
                self.appendRestoreWorkspace(self.workspace_chooser, workspace.workspace_id, workspace.workspace_name);
            };
        }
        if (self.model.last_error) |message| {
            const restore_title = ownedLabel(self.allocator, message, "error") catch return;
            self.workspace_chooser.append(restore_title.as(gtk.Widget));
        }
    }

    fn renderWorkspaceMenu(
        self: *Controller,
        projection: *const control.Projection,
        workspace_map: *const control.WorkspaceMap,
    ) void {
        if (!self.workspace_menu_inputs.changed(self.allocator, .{ .name = workspace_map.workspace_name, .catalog = if (self.native_executor) |executor| executor.nativeWorkspaceCatalog() else null, .detach = projection.header.workspace_menu_detach_action, .delete = projection.action("workspace.archive"), .new_window = projection.action("workspace.window.new") })) return;
        clearBox(self.workspace_menu_box);
        setLabel(
            self.allocator,
            self.workspace_menu_label,
            workspace_map.workspace_name,
        ) catch {};
        if (self.native_executor) |executor| {
            if (executor.nativeWorkspaceCatalog()) |catalog| {
                for (catalog.workspaces) |workspace| {
                    if (catalog.isArchived(workspace.workspace_id)) {
                        self.appendRestoreWorkspace(self.workspace_menu_box, workspace.workspace_id, workspace.workspace_name);
                        continue;
                    }
                    self.appendWorkspaceChoice(
                        workspace.workspace_id,
                        workspace.workspace_name,
                        std.mem.eql(
                            u8,
                            workspace.workspace_id,
                            catalog.selected_workspace_id,
                        ),
                    );
                }
            } else {}
        } else {
            // Workspace choices are native-only; no Host fallback.
        }
        if (projection.action("workspace.window.new")) |action| {
            const button = gtk.Button.new();
            const title = ownedLabel(self.allocator, action.label, "caption") catch return;
            button.setChild(title.as(gtk.Widget));
            renderActionState(self.allocator, button, action);
            connect(button, Controller, newWorkspaceWindowClicked, self);
            self.workspace_menu_box.append(button.as(gtk.Widget));
        }
        if (projection.action("workspace.archive")) |action| self.appendWorkspaceConfirmedAction(action, deleteWorkspaceConfirmed);
        if (projection.header.workspace_menu_detach_action) |*action| {
            const close = gtk.Button.new();
            setButtonLabel(self.allocator, close, action.label) catch return;
            connect(close, Controller, detachConfirmClicked, self);
            self.workspace_menu_box.append(close.as(gtk.Widget));
        }
    }

    fn appendWorkspaceConfirmedAction(self: *Controller, action: *const control.Action, comptime onConfirm: fn (*gtk.Button, *Controller) callconv(.c) void) void {
            const separator = gtk.Separator.new(.horizontal);
            self.workspace_menu_box.append(separator.as(gtk.Widget));

            const detach = gtk.MenuButton.new();
            const content = gtk.Box.new(.horizontal, 6);
            const icon = gtk.Image.newFromIconName("window-close-symbolic");
            const title = ownedLabel(self.allocator, action.label, "caption") catch return;
            content.append(icon.as(gtk.Widget));
            content.append(title.as(gtk.Widget));
            detach.setChild(content.as(gtk.Widget));
            renderMenuButtonState(self.allocator, detach, action);
            if (std.mem.eql(u8, action.id, "workspace.archive")) {
                const enabled = blk: {
                    const executor = self.native_executor orelse break :blk false;
                    var decision = executor.lifecycleDecision("inspect", null) catch break :blk false;
                    defer decision.deinit();
                    break :blk decision.value.can_archive;
                };
                detach.as(gtk.Widget).setSensitive(@intFromBool(enabled));
                if (!enabled) detach.as(gtk.Widget).setTooltipText("Keep at least one unarchived Workspace.");
            }

            if (action.confirmation) |confirmation| {
                const popover = gtk.Popover.new();
                const box = menuBox();
                const confirm_title = ownedLabel(
                    self.allocator,
                    confirmation.title,
                    "heading",
                ) catch return;
                const message = ownedLabel(
                    self.allocator,
                    confirmation.message,
                    "caption",
                ) catch return;
                const actions = gtk.Box.new(.horizontal, 6);
                const cancel = gtk.Button.newWithLabel("Cancel");
                const confirm_label = self.allocator.dupeZ(
                    u8,
                    confirmation.confirm_label,
                ) catch return;
                defer self.allocator.free(confirm_label);
                const confirm = gtk.Button.newWithLabel(confirm_label);
                if (self.native_executor) |executor| if (executor.nativeWorkspaceSnapshot()) |snapshot| {
                    setWidgetName(self.allocator, confirm.as(gtk.Widget), snapshot.workspace_id) catch return;
                };
                actions.append(cancel.as(gtk.Widget));
                actions.append(confirm.as(gtk.Widget));
                box.append(confirm_title.as(gtk.Widget));
                if (self.native_executor) |executor| {
                    if (executor.nativeWorkspaceSnapshot()) |snapshot| {
                        const name = ownedLabel(self.allocator, snapshot.workspace_name, "heading") catch return;
                        box.append(name.as(gtk.Widget));
                    }
                }
                box.append(message.as(gtk.Widget));
                box.append(actions.as(gtk.Widget));
                popover.setChild(box.as(gtk.Widget));
                detach.setPopover(popover);
                connect(cancel, Controller, detachCancelClicked, self);
                connect(confirm, Controller, onConfirm, self);
            }
            self.workspace_menu_box.append(detach.as(gtk.Widget));
    }

    fn appendWorkspaceChoice(
        self: *Controller,
        workspace_id: []const u8,
        name: []const u8,
        selected: bool,
    ) void {
        const button = gtk.Button.newWithLabel("");
        setButtonLabel(self.allocator, button, name) catch {};
        setWidgetName(
            self.allocator,
            button.as(gtk.Widget),
            workspace_id,
        ) catch {};
        button.as(gtk.Widget).setTooltipText("Select Workspace");
        if (selected) button.as(gtk.Widget).addCssClass("suggested-action");
        connect(button, Controller, workspaceClicked, self);
        self.workspace_menu_box.append(button.as(gtk.Widget));
        if (selected) {
            const selected_name = self.allocator.dupeZ(u8, name) catch return;
            defer self.allocator.free(selected_name);
            self.workspace_menu_label.setLabel(selected_name);
        }
    }

    fn renderHeader(
        self: *Controller,
        projection: *const control.Projection,
    ) void {
        if (!self.header_inputs.changed(self.allocator, .{ .header = projection.header, .attention = projection.attention_badge })) return;
        setLabel(
            self.allocator,
            self.connection_status_label,
            projection.header.connection.label,
        ) catch {};
        self.connection_status_image.setFromIconName(
            if (std.mem.eql(
                u8,
                projection.header.connection.status,
                "offline",
            ))
                "network-error-symbolic"
            else if ((std.mem.eql(
                u8,
                projection.header.connection.status,
                "connected",
            ) or std.mem.eql(
                u8,
                projection.header.connection.status,
                "online",
            )) and projection.header.connection.route != null and std.mem.eql(
                u8,
                projection.header.connection.route.?,
                "lan",
            ))
                "network-workgroup-symbolic"
            else if ((std.mem.eql(
                u8,
                projection.header.connection.status,
                "connected",
            ) or std.mem.eql(
                u8,
                projection.header.connection.status,
                "online",
            )) and projection.header.connection.route != null and std.mem.eql(
                u8,
                projection.header.connection.route.?,
                "wan",
            ))
                "web-browser-symbolic"
            else if (std.mem.eql(
                u8,
                projection.header.connection.status,
                "connected",
            ) or std.mem.eql(
                u8,
                projection.header.connection.status,
                "online",
            ))
                "network-transmit-receive-symbolic"
            else if (std.mem.eql(
                u8,
                projection.header.connection.status,
                "connecting",
            ) or std.mem.eql(
                u8,
                projection.header.connection.status,
                "reconnecting",
            ) or std.mem.eql(
                u8,
                projection.header.connection.status,
                "recovering",
            ) or std.mem.eql(u8, projection.header.connection.status, "partial"))
                "view-refresh-symbolic"
            else if (std.mem.eql(
                u8,
                projection.header.connection.status,
                "inactive",
            ))
                "network-offline-symbolic"
            else
                "network-error-symbolic",
        );
        self.connection_status_image.as(gtk.Widget).removeCssClass("success");
        self.connection_status_image.as(gtk.Widget).removeCssClass("warning");
        self.connection_status_image.as(gtk.Widget).removeCssClass("error");
        if (std.mem.eql(
            u8,
            projection.header.connection.status,
            "connected",
        ) or std.mem.eql(
            u8,
            projection.header.connection.status,
            "online",
        )) {
            self.connection_status_image.as(gtk.Widget).addCssClass("success");
        } else if (std.mem.eql(
            u8,
            projection.header.connection.status,
            "connecting",
        ) or std.mem.eql(
            u8,
            projection.header.connection.status,
            "recovering",
        ) or std.mem.eql(u8, projection.header.connection.status, "partial")) {
            self.connection_status_image.as(gtk.Widget).addCssClass("warning");
        } else if (std.mem.eql(
            u8,
            projection.header.connection.status,
            "offline",
        )) {
            self.connection_status_image.as(gtk.Widget).addCssClass("error");
        }
        const connection_tooltip = self.allocator.dupeZ(
            u8,
            projection.header.connection.accessibility_label,
        ) catch null;
        defer if (connection_tooltip) |text| self.allocator.free(text);
        if (connection_tooltip) |text| {
            self.connection_badge.as(gtk.Widget).setTooltipText(text);
        }
        self.connection_badge.as(gtk.Widget).removeCssClass("error");
        self.connection_badge.as(gtk.Widget).removeCssClass("warning");
        if (projection.attention_badge.unresolved_red > 0) {
            self.connection_badge.as(gtk.Widget).addCssClass("error");
        } else if (projection.attention_badge.unread_orange > 0) {
            self.connection_badge.as(gtk.Widget).addCssClass("warning");
        }
        renderActionState(
            self.allocator,
            self.rename_workspace_button,
            &projection.header.rename_action,
        );
        renderActionState(self.allocator, self.add_tab_button, &projection.header.add_tab);
        renderActionState(self.allocator, self.refresh_button, &projection.header.refresh);
        if (projection.header.lifecycle_action) |*action| {
            renderActionState(self.allocator, self.lifecycle_button, action);
            self.lifecycle_image.setFromIconName(
                if (action.enabled)
                    "media-playback-start-symbolic"
                else
                    "content-loading-symbolic",
            );
            self.lifecycle_button.as(gtk.Widget).setVisible(1);
            self.lifecycle_button.as(gtk.Widget).removeCssClass("suggested-action");
            if (action.role == .primary) {
                self.lifecycle_button.as(gtk.Widget).addCssClass("suggested-action");
            }
        } else {
            self.lifecycle_button.as(gtk.Widget).setVisible(0);
        }
    }

    fn sessionInteractionActive(widget: *gtk.Widget) bool {
        if (widget.getStateFlags().active or widget.hasCssClass("keyboard-activating") != 0) return true;
        var child = widget.getFirstChild();
        while (child) |current| : (child = current.getNextSibling()) {
            if (sessionInteractionActive(current)) return true;
        }
        return false;
    }

    fn renderSessions(
        self: *Controller,
        projection: *const control.Projection,
    ) void {
        // Retain the row that owns an active context menu, including its close
        // transition. Its closed signal reconciles the newest cached projection.
        for (self.session_popovers.items) |popover| {
            if (popover.as(gtk.Widget).getVisible() != 0 or
                popover.as(gtk.Widget).getMapped() != 0) return;
        }
        if (!self.sessions_visible) return;
        // GtkButton activation may finish on a later frame (including AT-SPI).
        // Rebuilding its row before clicked is emitted silently drops the action.
        if (sessionInteractionActive(self.sessions_list.as(gtk.Widget))) return;
        if (!self.sessions_inputs.changed(self.allocator, .{
            .query = std.mem.span(self.sessions_search.as(gtk.Editable).getText()),
            .type_filter = self.sessions_type.getSelected(),
            .sessions = projection.sessions,
            .groups = projection.session_groups,
            .agents = projection.agent_sessions,
            .endpoints = projection.codex_endpoints,
            .workspace = if (self.native_executor) |executor| executor.nativeWorkspaceSnapshot() else null,
        })) return;
        setLabel(
            self.allocator,
            self.sessions_title,
            projection.sessions.title,
        ) catch {};
        setLabel(
            self.allocator,
            self.sessions_hint,
            projection.sessions.interaction_hint,
        ) catch {};
        renderActionState(
            self.allocator,
            self.sessions_refresh_button,
            &projection.sessions.refresh_action,
        );
        renderActionState(
            self.allocator,
            self.sessions_close_button,
            &projection.sessions.close_action,
        );
        self.sessions_button.setActive(@intFromBool(self.sessions_visible));
        const previous_scroll = self.sessions_scroller
            .getVadjustment()
            .getValue();
        self.clearSessionRows();
        for (projection.session_groups) |group| {
            const key = std.fmt.allocPrint(self.allocator, "host:{s}", .{group.id}) catch continue;
            defer self.allocator.free(key);
            const expanded = std.mem.span(self.sessions_search.as(gtk.Editable).getText()).len != 0 or !self.collapsed_session_groups.contains(key);
            self.catalogHeading(key, group.title, expanded, null);
            if (!expanded) continue;
            self.appendAgentCatalog(projection, group.id);
            self.appendTargetSessionRows(projection, group.id);
            if (group.message) |message| {
                var has_agents = false;
                for (projection.agent_sessions) |agent| if (std.mem.eql(u8, agent.execution_target_id, group.id)) { has_agents = true; break; };
                if (!std.mem.eql(u8, group.state, "empty") or !has_agents) {
                    const notice = ownedLabel(self.allocator, message, "caption") catch continue;
                    notice.setWrap(1);
                    self.sessions_list.append(notice.as(gtk.Widget));
                }
            }
        }
        const adjustment = self.sessions_scroller.getVadjustment();
        adjustment.setValue(session_interaction.restoredScrollOffset(
            previous_scroll,
            adjustment.getUpper(),
            adjustment.getPageSize(),
        ));
        std.log.debug(
            "Control Center sessions rendered rows={d} reveal_child={} child_revealed={} has_first_row={}",
            .{
                projection.sessions.rows.len,
                self.sessions_revealer.getRevealChild() != 0,
                self.sessions_revealer.getChildRevealed() != 0,
                self.sessions_list.as(gtk.Widget).getFirstChild() != null,
            },
        );
    }

    fn agentWorkspaceRank(self: *Controller, agent: control.AgentSession) usize {
        const executor = self.native_executor orelse return std.math.maxInt(usize);
        const workspace = executor.nativeWorkspaceSnapshot() orelse return std.math.maxInt(usize);
        var rank: usize = 0;
        for ([_]bool{ true, false }) |selected| {
            for (workspace.tabs) |tab| {
                if (std.mem.eql(u8, tab.id, workspace.selected_tab_id) != selected) continue;
                for (tab.panes) |pane| {
                    for (agent.terminals) |identity| {
                        if (std.mem.eql(u8, identity.execution_target_id, pane.execution_target_id) and std.mem.eql(u8, identity.tmux_session_id, pane.tmux_session_id)) return rank;
                    }
                    rank += 1;
                }
            }
        }
        return std.math.maxInt(usize);
    }

    fn catalogHeading(self: *Controller, key: []const u8, title: []const u8, expanded: bool, count: ?usize) void {
        const button = gtk.Button.new();
        setWidgetName(self.allocator, button.as(gtk.Widget), key) catch return;
        const detail = self.allocator.dupeZ(u8, key) catch return;
        defer self.allocator.free(detail);
        button.as(gtk.Widget).setTooltipText(detail);
        const text = if (count) |value|
            std.fmt.allocPrint(self.allocator, "{s} {s} ({d})", .{ if (expanded) "▾" else "▸", title, value }) catch return
        else std.fmt.allocPrint(self.allocator, "{s} {s}", .{ if (expanded) "▾" else "▸", title }) catch return;
        defer self.allocator.free(text);
        const row_label = ownedLabel(self.allocator, text, "caption") catch return;
        row_label.setEllipsize(.end);
        button.setChild(row_label.as(gtk.Widget));
        connect(button, Controller, sessionGroupClicked, self);
        const row = gtk.ListBoxRow.new();
        row.setChild(button.as(gtk.Widget));
        self.sessions_list.append(row.as(gtk.Widget));
    }

    fn appendAgentCatalog(self: *Controller, projection: *const control.Projection, target_id: []const u8) void {
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const alloc = arena.allocator();
        const agents = projection.agent_sessions;
        const View = AgentCatalogView;
        const views = alloc.alloc(View, agents.len) catch return;
        const ranks = alloc.alloc(?usize, agents.len) catch return;
        for (agents, 0..) |agent, i| {
            const rank = self.agentWorkspaceRank(agent);
            ranks[i] = if (rank == std.math.maxInt(usize)) null else rank;
        }
        const parsed = agent_catalog.project(alloc, .{
            .agents = agents, .groups = projection.session_groups, .ranks = ranks,
            .query = std.mem.span(self.sessions_search.as(gtk.Editable).getText()),
            .filter = @as([]const u8, switch (self.sessions_type.getSelected()) { 1 => "codex", 2 => "claude", 3 => "terminal", else => "all" }),
        }) catch return;
        defer parsed.deinit();
        if (parsed.value.len != agents.len) return;
        for (parsed.value, 0..) |row, i| views[i] = .{
            .root = row.root, .parent = row.parent, .key = std.fmt.allocPrint(alloc, "agent:{s}", .{row.key}) catch return,
            .group = row.group, .matches = row.matches, .rank = row.rank orelse std.math.maxInt(usize),
        };
        const order = alloc.alloc(usize, agents.len) catch return;
        for (order, 0..) |*value, i| value.* = i;
        var groups = std.StringHashMap(void).init(alloc);
        for (views) |*view| {
            view.group = views[view.root].group;
            if (view.matches and std.mem.eql(u8, agents[view.root].execution_target_id, target_id)) groups.put(view.group, {}) catch return;
        }
        const names = alloc.alloc([]const u8, groups.count()) catch return;
        var it = groups.keyIterator();
        var n: usize = 0;
        while (it.next()) |key| : (n += 1) names[n] = key.*;
        std.mem.sort([]const u8, names, {}, struct {
            fn less(_: void, lhs: []const u8, rhs: []const u8) bool {
                if (std.mem.eql(u8, lhs, "Current Workspace")) return !std.mem.eql(u8, rhs, lhs);
                if (std.mem.eql(u8, rhs, "Current Workspace")) return false;
                return std.mem.lessThan(u8, lhs, rhs);
            }
        }.less);
        const searching = std.mem.span(self.sessions_search.as(gtk.Editable).getText()).len != 0;
        for (names) |group| {
            const key = std.fmt.allocPrint(alloc, "catalog:{s}", .{group}) catch return;
            var referenced = false;
            for (views) |view| {
                if (std.mem.eql(u8, view.group, group) and view.rank != std.math.maxInt(usize)) referenced = true;
            }
            const expanded = searching or (referenced != self.collapsed_session_groups.contains(key));
            var count: usize = 0;
            for (views) |view| if (std.mem.eql(u8, view.group, group)) {
                count += 1;
            };
            const title = if (std.mem.indexOf(u8, group, " / ")) |separator|
                std.fs.path.basename(group[separator + 3 ..])
            else
                group;
            self.catalogHeading(key, title, expanded, count);
            if (!expanded) continue;
            for (order) |i| {
                if (views[i].parent != null or !std.mem.eql(u8, views[i].group, group) or !views[i].matches) continue;
                self.appendAgentTree(agents, views, order, i, searching, 0);
            }
        }
    }

    fn appendAgentTree(self: *Controller, agents: []const control.AgentSession, views: []const AgentCatalogView, order: []const usize, i: usize, searching: bool, depth: usize) void {
        if (depth >= agents.len) return;
        var children: usize = 0;
        var referenced_child = false;
        for (views) |view| {
            if (view.parent == i and view.matches) {
                children += 1;
                referenced_child = referenced_child or view.rank != std.math.maxInt(usize);
            }
        }
        const opened = searching or (referenced_child != self.collapsed_session_groups.contains(views[i].key));
        self.appendAgentRow(agents[i], depth, views[i].key, children, opened);
        if (opened) for (order) |j| {
            if (views[j].parent == i and views[j].matches) self.appendAgentTree(agents, views, order, j, searching, depth + 1);
        };
    }

    fn appendAgentRow(self: *Controller, agent: control.AgentSession, depth: usize, key: []const u8, children: usize, opened: bool) void {
        const metadata = agent.metadata orelse control.CodexThreadMetadata{};
        const button = gtk.Button.new();
        const data = std.json.Stringify.valueAlloc(self.allocator, agent, .{}) catch return;
        defer self.allocator.free(data);
        setWidgetName(self.allocator, button.as(gtk.Widget), data) catch return;
        const age: ?i64 = if (metadata.updated_at) |updated| @max(0, std.time.timestamp() - updated) else null;
        const stamp = if (age) |seconds| (if (seconds < 60) self.allocator.dupe(u8, "now") else if (seconds < 3600) std.fmt.allocPrint(self.allocator, "{d}m", .{@divTrunc(seconds, 60)}) else if (seconds < 86400) std.fmt.allocPrint(self.allocator, "{d}h", .{@divTrunc(seconds, 3600)}) else std.fmt.allocPrint(self.allocator, "{d}d", .{@divTrunc(seconds, 86400)})) catch return else self.allocator.dupe(u8, "") catch return;
        defer self.allocator.free(stamp);
        var assigned = false;
        for (agent.terminals) |identity| {
            if (self.nativeSessionAssignment(identity.execution_target_id, identity.tmux_session_id)) |assignment| {
                assigned = assigned or assignment == .assigned_here;
            }
        }
        const text = std.fmt.allocPrint(self.allocator, "{s}{s}\n{s} · {s}  {s}", .{ if (assigned) "✓ " else "", agent.display_name, if (metadata.cwd) |cwd| std.fs.path.basename(cwd) else "Unknown project", agent.status, stamp }) catch return;
        defer self.allocator.free(text);
        const row_label = ownedLabel(self.allocator, text, "body") catch return;
        row_label.setEllipsize(.end);
        row_label.setWrap(0);
        button.setChild(row_label.as(gtk.Widget));
        const tooltip = std.fmt.allocPrintSentinel(self.allocator, "{s}\n{s}\n{s}\n{s}\n{s}", .{ agent.display_name, metadata.cwd orelse "", metadata.preview orelse "", metadata.agent_role orelse "", agent.thread_id }, 0) catch return;
        defer self.allocator.free(tooltip);
        button.as(gtk.Widget).setTooltipText(tooltip);
        button.as(gtk.Widget).setSizeRequest(-1, layout_policy.session_row_min_height);
        button.as(gtk.Widget).setHexpand(1);
        button.as(gtk.Widget).addCssClass("flat");
        if (agent.terminals.len > 0 and self.sidebarSelected(agent.terminals[0])) button.as(gtk.Widget).addCssClass("suggested-action");
        connect(button, Controller, agentSessionClicked, self);
        const row = gtk.ListBoxRow.new();
        const content = gtk.Box.new(.horizontal, 4);
        content.as(gtk.Widget).setMarginStart(@intCast(@as(usize, @min(depth, 4)) * 12));
        if (children > 0) {
            const disclosure = gtk.Button.newFromIconName(if (opened) "pan-down-symbolic" else "pan-end-symbolic");
            setWidgetName(self.allocator, disclosure.as(gtk.Widget), key) catch return;
            disclosure.as(gtk.Widget).addCssClass("flat");
            disclosure.as(gtk.Widget).setTooltipText("Show or hide subagents");
            connect(disclosure, Controller, sessionGroupClicked, self);
            content.append(disclosure.as(gtk.Widget));
        }
        content.append(button.as(gtk.Widget));
        const menu = gtk.MenuButton.new();
        menu.as(gtk.Widget).setTooltipText("Session actions");
        menu.setIconName("view-more-symbolic");
        menu.as(gtk.Widget).addCssClass("flat");
        menu.as(gtk.Widget).setValign(.center);
        const popover = gtk.Popover.new();
        const box = menuBox();
        const rename = gtk.Button.newWithLabel("Rename…");
        setWidgetName(self.allocator, rename.as(gtk.Widget), data) catch return;
        connect(rename, Controller, agentRenameClicked, self);
        box.append(rename.as(gtk.Widget));
        const end = gtk.Button.newWithLabel("End Session…");
        setWidgetName(self.allocator, end.as(gtk.Widget), data) catch return;
        connect(end, Controller, agentEndClicked, self);
        box.append(end.as(gtk.Widget));
        const copy = gtk.Button.newWithLabel("Copy ID");
        setWidgetName(self.allocator, copy.as(gtk.Widget), agent.thread_id) catch return;
        connect(copy, Controller, agentCopyClicked, self);
        box.append(copy.as(gtk.Widget));
        const details = gtk.Expander.new("Details");
        const details_label = gtk.Label.new(tooltip);
        details_label.setSelectable(1);
        details_label.setWrap(1);
        details_label.setMaxWidthChars(48);
        details.setChild(details_label.as(gtk.Widget));
        box.append(details.as(gtk.Widget));
        if (agent.terminals.len > 0) {
            var identity = agent.terminals[0];
            for (agent.terminals) |candidate| {
                if (self.nativeSessionAssignment(candidate.execution_target_id, candidate.tmux_session_id)) |assignment| {
                    if (assignment == .assigned_here) {
                        identity = candidate;
                        break;
                    }
                }
            }
            const context = self.makeSessionButtonContext(&identity) catch return;
            row.as(gobject.Object).weakRef(sessionButtonContextWeakNotify, context);
            context.context_menu = popover;
            const codex = gtk.Button.newWithLabel("Codex…");
            connect(codex, SessionButtonContext, sessionCodexClicked, context);
            box.append(codex.as(gtk.Widget));
            if (self.model.projection()) |projection| {
                for (projection.sessions.rows) |session| {
                    if (!std.mem.eql(u8, session.identity.execution_target_id, identity.execution_target_id) or !std.mem.eql(u8, session.identity.tmux_session_id, identity.tmux_session_id)) continue;
                    if (session.restart_action) |action| {
                        const restart = gtk.Button.new();
                        renderActionButton(self.allocator, restart, &action);
                        connect(restart, SessionButtonContext, sessionRestartClicked, context);
                        box.append(restart.as(gtk.Widget));
                    }
                    if (sessionAssignedHere(session.assigned_to_selected_tab, self.nativeSessionAssignment(session.identity.execution_target_id, session.identity.tmux_session_id))) {
                        const remove = gtk.Button.newWithLabel("Remove from current Tab");
                        connect(remove, SessionButtonContext, sessionRemoveViewClicked, context);
                        box.append(remove.as(gtk.Widget));
                    }
                    break;
                }
            }
        }
        popover.setChild(box.as(gtk.Widget));
        menu.setPopover(popover);
        const context_click = gtk.GestureClick.new();
        context_click.as(gtk.GestureSingle).setButton(3);
        _ = gtk.GestureClick.signals.pressed.connect(context_click, *gtk.Popover, agentContextClicked, popover, .{});
        button.as(gtk.Widget).addController(context_click.as(gtk.EventController));
        self.session_popovers.append(self.allocator, popover) catch {};
        _ = gtk.Popover.signals.closed.connect(popover, *Controller, mapActionClosed, self, .{});
        content.append(menu.as(gtk.Widget));
        row.setChild(content.as(gtk.Widget));
        self.sessions_list.append(row.as(gtk.Widget));
    }

    fn agentContextClicked(_: *gtk.GestureClick, _: c_int, _: f64, _: f64, popover: *gtk.Popover) callconv(.c) void {
        popover.popup();
    }

    fn agentCopyClicked(button: *gtk.Button, _: *Controller) callconv(.c) void {
        button.as(gtk.Widget).getClipboard().setText(button.as(gtk.Widget).getName());
    }

    fn agentEndClicked(button: *gtk.Button, self: *Controller) callconv(.c) void {
        const parsed = std.json.parseFromSlice(control.AgentSession, self.allocator, std.mem.span(button.as(gtk.Widget).getName()), .{}) catch return;
        defer parsed.deinit();
        if (self.session_end) |window| if (window.pending) { window.native.as(gtk.Window).present(); return; };
        if (self.session_end == null) self.session_end = session_end_window.Window.create(self.allocator, self.app, self.host_window.as(gtk.Window), self, sessionEndSubmit, sessionEndReview, sessionEndClose) catch return;
        const agent = parsed.value;
        self.session_end.?.present(.{ .execution_target_id = agent.execution_target_id, .owner_host_id = agent.owner_host_id,
            .endpoint_id = agent.endpoint_id, .thread_id = agent.thread_id, .confirmation_id = "pending" }, "Checking the original conversation…") catch return;
        for (self.session_popovers.items) |popover| { if (popover.as(gtk.Widget).getVisible() != 0) popover.popdown(); }
        sessionEndReview(self, self.session_end.?.request.?.value);
    }

    fn agentRenameClicked(button: *gtk.Button, self: *Controller) callconv(.c) void {
        const parsed = std.json.parseFromSlice(control.AgentSession, self.allocator, std.mem.span(button.as(gtk.Widget).getName()), .{}) catch return;
        defer parsed.deinit();
        const agent = parsed.value;
        self.presentRenameEditor(.{ .agent = .{ .owner_host_id = agent.owner_host_id, .endpoint_id = agent.endpoint_id, .thread_id = agent.thread_id } }, "Rename Codex Session", "Session name", agent.display_name) catch {};
        for (self.session_popovers.items) |popover| {
            if (popover.as(gtk.Widget).getVisible() != 0) popover.popdown();
        }
    }

    fn clearSidebarSelection(self: *Controller) void {
        for (self.sidebar_selection.items) |item| { self.allocator.free(item.execution_target_id); self.allocator.free(item.tmux_session_id); }
        self.sidebar_selection.clearRetainingCapacity();
    }
    fn sidebarSelectChanged(button: *gtk.ToggleButton, self: *Controller) callconv(.c) void {
        self.sidebar_selecting = button.getActive() != 0;
        self.clearSidebarSelection();
        if (self.sidebar_tab_id) |id| self.allocator.free(id);
        self.sidebar_tab_id = null;
        if (self.native_executor) |executor| if (executor.nativeWorkspaceSnapshot()) |workspace| {
            self.sidebar_tab_id = self.allocator.dupe(u8, workspace.selected_tab_id) catch null;
        };
        self.sidebar_add.?.as(gtk.Widget).setVisible(@intFromBool(self.sidebar_selecting));
        self.sidebar_add.?.as(gtk.Widget).setTooltipText("Add selected");
        self.refreshSessionFilters();
    }
    fn sidebarPick(self: *Controller, identity: control.SessionIdentity) void {
        const tab_id = self.sidebar_tab_id orelse return;
        const executor = self.native_executor orelse return;
        if (executor.nativePaneIdForSessionInTab(tab_id, identity.execution_target_id, identity.tmux_session_id) != null) return;
        for (self.sidebar_selection.items, 0..) |item, index| {
            if (std.mem.eql(u8, item.execution_target_id, identity.execution_target_id) and std.mem.eql(u8, item.tmux_session_id, identity.tmux_session_id)) {
                const removed = self.sidebar_selection.orderedRemove(index);
                self.allocator.free(removed.execution_target_id); self.allocator.free(removed.tmux_session_id);
                self.sidebarSelectionUpdated(); return;
            }
        }
        const target = self.allocator.dupe(u8, identity.execution_target_id) catch return;
        const session = self.allocator.dupe(u8, identity.tmux_session_id) catch { self.allocator.free(target); return; };
        self.sidebar_selection.append(self.allocator, .{ .execution_target_id = target, .tmux_session_id = session }) catch { self.allocator.free(target); self.allocator.free(session); return; };
        self.sidebarSelectionUpdated();
    }
    fn sidebarSelected(self: *Controller, identity: control.SessionIdentity) bool {
        for (self.sidebar_selection.items) |item| if (std.mem.eql(u8, item.execution_target_id, identity.execution_target_id) and std.mem.eql(u8, item.tmux_session_id, identity.tmux_session_id)) return true;
        return false;
    }
    fn sidebarSelectionUpdated(self: *Controller) void {
        const text = std.fmt.allocPrintSentinel(self.allocator, "Add {d}", .{self.sidebar_selection.items.len}, 0) catch return;
        defer self.allocator.free(text); self.sidebar_add.?.as(gtk.Widget).setTooltipText(text);
        self.sidebar_add.?.as(gtk.Widget).setSensitive(@intFromBool(self.sidebar_selection.items.len > 0));
        self.refreshSessionFilters();
    }
    fn sidebarAddClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        const executor = self.native_executor orelse return;
        const snapshot = executor.nativeWorkspaceSnapshot() orelse return;
        executor.appendSessions(snapshot.workspace_id, self.sidebar_tab_id orelse return, self.sidebar_selection.items) catch {
            self.model.setLocalError("The selected Tab is unavailable or has insufficient room (maximum nine Panes)."); return;
        };
        self.clearSidebarSelection(); self.sidebarSelectionUpdated();
    }

    fn sessionsFilterChanged(_: *gtk.Editable, self: *Controller) callconv(.c) void {
        self.refreshSessionFilters();
    }
    fn sessionsTypeChanged(_: *gobject.Object, _: *gobject.ParamSpec, self: *Controller) callconv(.c) void {
        self.refreshSessionFilters();
    }
    fn refreshSessionFilters(self: *Controller) void {
        self.sessions_inputs.deinit(self.allocator);
        if (self.model.projection()) |projection| self.renderSessions(projection);
    }
    fn sessionGroupClicked(button: *gtk.Button, self: *Controller) callconv(.c) void {
        const id = std.mem.span(button.as(gtk.Widget).getName());
        if (self.collapsed_session_groups.fetchRemove(id)) |old| self.allocator.free(old.key) else {
            const copy = self.allocator.dupe(u8, id) catch return;
            self.collapsed_session_groups.put(self.allocator, copy, {}) catch {
                self.allocator.free(copy);
                return;
            };
        }
        self.refreshSessionFilters();
    }
    fn sessionMatchesFilter(self: *Controller, name: []const u8, kind: []const u8) bool {
        const selected = self.sessions_type.getSelected();
        const wanted: ?[]const u8 = switch (selected) {
            1 => "codex",
            2 => "claude",
            3 => "terminal",
            else => null,
        };
        if (wanted) |value| if (!std.mem.eql(u8, kind, value) and !(std.mem.startsWith(u8, kind, value) and std.mem.endsWith(u8, kind, "_plane"))) return false;
        const query = std.mem.trim(u8, std.mem.span(self.sessions_search.as(gtk.Editable).getText()), " ");
        return query.len == 0 or std.ascii.indexOfIgnoreCase(name, query) != null;
    }

    fn agentSessionClicked(button: *gtk.Button, self: *Controller) callconv(.c) void {
        const parsed = std.json.parseFromSlice(control.AgentSession, self.allocator, std.mem.span(button.as(gtk.Widget).getName()), .{}) catch return;
        defer parsed.deinit();
        const agent = parsed.value;
        if (self.sidebar_selecting) { if (agent.terminals.len > 0) self.sidebarPick(agent.terminals[0]); return; }
        if (self.native_executor) |executor| if (executor.nativeWorkspaceSnapshot()) |workspace| {
            for (workspace.tabs) |tab| {
                if (!std.mem.eql(u8, tab.id, workspace.selected_tab_id)) continue;
                for (tab.panes) |pane| for (agent.terminals) |identity| {
                    if (std.mem.eql(u8, identity.execution_target_id, pane.execution_target_id) and std.mem.eql(u8, identity.tmux_session_id, pane.tmux_session_id)) {
                        const pane_id = self.allocator.dupe(u8, pane.id) catch return;
                        defer self.allocator.free(pane_id);
                        self.dispatch(.{ .remove_pane = .{ .pane_id = pane_id } });
                        return;
                    }
                };
            }
        };
        for (agent.terminals) |identity| {
            self.dispatch(.{ .toggle_session_assignment = .{
                .execution_target_id = identity.execution_target_id,
                .tmux_session_id = identity.tmux_session_id,
            } });
            return;
        }
        self.presentSessionCreateEditor(.codex_agent, agent.execution_target_id);
        self.session_create_mode.?.setSelected(1);
        if (self.session_create_endpoints) |configured| {
            var index: u32 = 0;
            for (configured.value) |endpoint| {
                if (!std.mem.eql(u8, endpoint.execution_target_id, agent.execution_target_id)) continue;
                if (std.mem.eql(u8, endpoint.id, agent.endpoint_id)) {
                    self.session_create_endpoint.?.setSelected(index);
                    break;
                }
                index += 1;
            }
        }
        setEntryText(self.allocator, self.session_create_resume_id.?, agent.thread_id) catch return;
        if (agent.loaded) sessionCreateSubmitClicked(self.session_create_submit.?, self);
    }

    fn appendTargetSessionRows(self: *Controller, projection: *const control.Projection, target_id: []const u8) void {
        sessions: for (projection.sessions.rows) |session| {
            for (projection.agent_sessions) |agent| for (agent.terminals) |identity| {
                if (std.mem.eql(u8, identity.execution_target_id, session.identity.execution_target_id) and std.mem.eql(u8, identity.tmux_session_id, session.identity.tmux_session_id)) continue :sessions;
            };
            if (!std.mem.eql(u8, session.identity.execution_target_id, target_id)) continue;
            const searchable = std.fmt.allocPrint(self.allocator, "{s} {s} {s}", .{ session.display_name, session.detail_label orelse "", session.target_label }) catch continue;
            defer self.allocator.free(searchable);
            if (!self.sessionMatchesFilter(searchable, session.session_kind)) continue;
            const row = gtk.ListBoxRow.new();
            const row_card = gtk.Box.new(.horizontal, 0);
            row_card.as(gtk.Widget).setSizeRequest(
                -1,
                layout_policy.session_row_min_height,
            );
            row_card.as(gtk.Widget).setMarginTop(4);
            row_card.as(gtk.Widget).setMarginBottom(4);
            row_card.as(gtk.Widget).addCssClass("card");
            const row_content = gtk.Box.new(
                .horizontal,
                layout_policy.session_row_spacing,
            );
            row_content.as(gtk.Widget).setHexpand(1);
            row_content.as(gtk.Widget).setValign(.center);
            row_content.as(gtk.Widget).setMarginStart(
                layout_policy.session_row_horizontal_padding,
            );
            row_content.as(gtk.Widget).setMarginEnd(
                layout_policy.session_row_horizontal_padding,
            );
            const button = gtk.Button.new();
            const native_assignment = self.nativeSessionAssignment(
                session.identity.execution_target_id,
                session.identity.tmux_session_id,
            );
            const assignment_action = sessionAssignmentAction(
                &session,
                native_assignment,
            );
            button.setCanShrink(1);
            setWidgetName(
                self.allocator,
                button.as(gtk.Widget),
                session.identity.tmux_session_id,
            ) catch {};
            const content = gtk.Box.new(
                .horizontal,
                layout_policy.session_row_spacing,
            );
            content.as(gtk.Widget).setHexpand(1);
            content.as(gtk.Widget).setValign(.center);
            const terminal_icon = gtk.Image.newFromIconName(
                "utilities-terminal-symbolic",
            );
            const session_copy = gtk.Box.new(.vertical, 1);
            session_copy.as(gtk.Widget).setHexpand(1);
            session_copy.as(gtk.Widget).setValign(.center);
            const session_name = ownedLabel(
                self.allocator,
                session.display_name,
                "body",
            ) catch continue;
            session_name.setWrap(0);
            session_name.setEllipsize(.end);
            session_copy.append(session_name.as(gtk.Widget));
            if (session.detail_label) |detail_label| {
                const detail = ownedLabel(
                    self.allocator,
                    detail_label,
                    "caption",
                ) catch continue;
                detail.setWrap(0);
                detail.setEllipsize(.end);
                detail.as(gtk.Widget).addCssClass("dim-label");
                session_copy.append(detail.as(gtk.Widget));
            }
            content.append(terminal_icon.as(gtk.Widget));
            content.append(session_copy.as(gtk.Widget));
            button.setChild(content.as(gtk.Widget));
            button.as(gtk.Widget).addCssClass("flat");
            button.as(gtk.Widget).setHexpand(1);
            renderActionState(self.allocator, button, assignment_action);
            if (session.selected or self.sidebarSelected(session.identity)) button.as(gtk.Widget).addCssClass("suggested-action");
            const context = self.makeSessionButtonContext(
                &session.identity,
            ) catch continue;
            row.as(gobject.Object).weakRef(
                sessionButtonContextWeakNotify,
                context,
            );
            connect(
                button,
                SessionButtonContext,
                sessionAssignmentClicked,
                context,
            );
            const context_click = gtk.GestureClick.new();
            context_click.as(gtk.GestureSingle).setButton(3);
            _ = gtk.GestureClick.signals.pressed.connect(
                context_click,
                *SessionButtonContext,
                sessionContextMenuClicked,
                context,
                .{},
            );
            button.as(gtk.Widget).addController(
                context_click.as(gtk.EventController),
            );
            const assignment_key = gtk.EventControllerKey.new();
            assignment_key.as(gtk.EventController).setPropagationPhase(
                .capture,
            );
            _ = gtk.EventControllerKey.signals.key_pressed.connect(
                assignment_key,
                *SessionButtonContext,
                sessionAssignmentKeyPressed,
                context,
                .{},
            );
            button.as(gtk.Widget).addController(
                assignment_key.as(gtk.EventController),
            );

            const assigned = gtk.Image.newFromIconName(
                "emblem-ok-symbolic",
            );
            assigned.as(gtk.Widget).setSizeRequest(18, 18);
            assigned.as(gtk.Widget).setValign(.center);
            assigned.as(gtk.Widget).setVisible(
                @intFromBool(sessionAssignedHere(
                    session.assigned_to_selected_tab,
                    native_assignment,
                )),
            );
            assigned.as(gtk.Widget).setTooltipText(
                "Assigned to selected Tab",
            );

            const rename = gtk.Button.newFromIconName("document-edit-symbolic");
            rename.setCanShrink(1);
            rename.as(gtk.Widget).setSizeRequest(28, 28);
            rename.as(gtk.Widget).setValign(.center);
            rename.as(gtk.Widget).addCssClass("flat");
            renderActionState(self.allocator, rename, &session.rename_action);
            connect(rename, SessionButtonContext, sessionRenameClicked, context);

            const kill = gtk.Button.newWithLabel("Kill Session");
            kill.as(gtk.Widget).addCssClass("destructive-action");
            renderActionState(self.allocator, kill, &session.kill_action);
            connect(kill, SessionButtonContext, sessionKillClicked, context);

            const context_menu = gtk.Popover.new();
            const context_box = menuBox();
            const codex_button = gtk.Button.newWithLabel("Codex…");
            connect(codex_button, SessionButtonContext, sessionCodexClicked, context);
            context_box.append(codex_button.as(gtk.Widget));
            if (session.replacement_action) |action| {
                if (session.replacement) |replacement| {
                    if (self.native_executor) |executor| {
                        if (executor.nativeWorkspaceSnapshot()) |workspace| {
                            const selection = control_center_intent.SessionReplacementSelection{
                                .execution_target_id = session.identity.execution_target_id,
                                .tmux_session_id = session.identity.tmux_session_id,
                                .replacement = replacement,
                                .workspace_id = workspace.workspace_id,
                                .confirmation_id = if (action.confirmation) |confirmation| confirmation.id else "",
                            };
                            context.replacement_json = std.json.Stringify.valueAlloc(self.allocator, selection, .{}) catch null;
                            var old = false;
                            var existing = false;
                            for (workspace.tabs) |tab| for (tab.panes) |pane| {
                                if (std.mem.eql(u8, pane.execution_target_id, selection.execution_target_id) and std.mem.eql(u8, pane.tmux_session_id, selection.tmux_session_id)) old = true;
                                if (std.mem.eql(u8, pane.execution_target_id, replacement.execution_target_id) and std.mem.eql(u8, pane.tmux_session_id, replacement.tmux_session_id)) existing = true;
                            };
                            const replace = gtk.MenuButton.new();
                            const replacement_label = self.allocator.dupeZ(u8, action.label) catch continue;
                            defer self.allocator.free(replacement_label);
                            replace.setLabel(replacement_label);
                            replace.as(gtk.Widget).setSensitive(@intFromBool(action.enabled and !(old and existing) and context.replacement_json != null));
                            if (old and existing) replace.as(gtk.Widget).setTooltipText("Already assigned in this Workspace");
                            if (action.confirmation) |confirmation| {
                                const popover = gtk.Popover.new();
                                const box = menuBox();
                                const title = ownedLabel(self.allocator, confirmation.title, "heading") catch continue;
                                const message = ownedLabel(self.allocator, confirmation.message, "body") catch continue;
                                message.setWrap(1);
                                const cancel = gtk.Button.newWithLabel("Cancel");
                                const submit_label = self.allocator.dupeZ(u8, confirmation.confirm_label) catch continue;
                                defer self.allocator.free(submit_label);
                                const submit = gtk.Button.newWithLabel(submit_label);
                                box.append(title.as(gtk.Widget));
                                box.append(message.as(gtk.Widget));
                                box.append(cancel.as(gtk.Widget));
                                box.append(submit.as(gtk.Widget));
                                popover.setChild(box.as(gtk.Widget));
                                replace.setPopover(popover);
                                context.replacement_popover = popover;
                                connect(cancel, SessionButtonContext, sessionReplacementCancelClicked, context);
                                connect(submit, SessionButtonContext, sessionReplacementSubmitClicked, context);
                            }
                            context_box.append(replace.as(gtk.Widget));
                            if (old and existing) {
                                const show_existing = gtk.Button.newWithLabel("Show existing Session");
                                connect(show_existing, SessionButtonContext, sessionReplacementShowClicked, context);
                                context_box.append(show_existing.as(gtk.Widget));
                            }
                        }
                    }
                }
            }
            if (session.restart_action) |action| {
                const restart = gtk.Button.new();
                renderActionButton(self.allocator, restart, &action);
                connect(restart, SessionButtonContext, sessionRestartClicked, context);
                context_box.append(restart.as(gtk.Widget));
            }
            if (sessionAssignedHere(session.assigned_to_selected_tab, native_assignment)) {
                const remove = gtk.Button.newWithLabel("Remove from current Tab");
                connect(remove, SessionButtonContext, sessionRemoveViewClicked, context);
                context_box.append(remove.as(gtk.Widget));
            }
            if (session.original_agent_id) |agent_id| {
                const identity_label = ownedLabel(self.allocator, agent_id, "caption") catch continue;
                identity_label.setSelectable(1);
                context_box.append(identity_label.as(gtk.Widget));
            }
            context_box.append(rename.as(gtk.Widget));
            context_box.append(kill.as(gtk.Widget));
            context_menu.setChild(context_box.as(gtk.Widget));
            context_menu.as(gtk.Widget).setParent(button.as(gtk.Widget));
            self.session_popovers.append(self.allocator, context_menu) catch {};
            _ = gtk.Popover.signals.closed.connect(context_menu, *Controller, mapActionClosed, self, .{});
            context.context_menu = context_menu;

            row_content.append(button.as(gtk.Widget));
            row_content.append(assigned.as(gtk.Widget));

            row_card.append(row_content.as(gtk.Widget));
            row.setChild(row_card.as(gtk.Widget));
            self.sessions_list.append(row.as(gtk.Widget));
        }
    }

    fn appendSessionSection(
        self: *Controller,
        projection: *const control.Projection,
        section: *const control.SessionSectionProjection,
    ) void {
        if (section.section == .remote) {
            const divider_row = gtk.ListBoxRow.new();
            divider_row.setChild(
                gtk.Separator.new(.horizontal).as(gtk.Widget),
            );
            divider_row.as(gtk.Widget).setSensitive(0);
            self.sessions_list.append(divider_row.as(gtk.Widget));
        }
        const section_row = gtk.ListBoxRow.new();
        const section_label = ownedLabel(
            self.allocator,
            section.title,
            "caption",
        ) catch return;
        setMargins(section_label.as(gtk.Widget), 8);
        section_label.as(gtk.Widget).addCssClass("dim-label");
        section_row.setChild(section_label.as(gtk.Widget));
        section_row.as(gtk.Widget).setSensitive(0);
        self.sessions_list.append(section_row.as(gtk.Widget));
        self.appendSessionCreationControls(projection, section.section);

        if (section.message) |message| {
            const message_row = gtk.ListBoxRow.new();
            const message_label = ownedLabel(
                self.allocator,
                message,
                "caption",
            ) catch return;
            setMargins(message_label.as(gtk.Widget), 8);
            message_label.setWrap(1);
            message_label.as(gtk.Widget).addCssClass(
                if (section.state == .@"error")
                    "error"
                else
                    "dim-label",
            );
            message_row.setChild(message_label.as(gtk.Widget));
            message_row.as(gtk.Widget).setSensitive(0);
            self.sessions_list.append(message_row.as(gtk.Widget));
        }
    }

    fn appendSessionCreationControls(
        self: *Controller,
        projection: *const control.Projection,
        section: control.SessionSection,
    ) void {
        const action_row = gtk.ListBoxRow.new();
        const actions = gtk.Box.new(.vertical, 6);
        setMargins(actions.as(gtk.Widget), 6);
        if (section == .local) {
            const local_actions = gtk.Box.new(.horizontal, 6);
            const shell = gtk.Button.new();
            renderActionButton(
                self.allocator,
                shell,
                &projection.sessions.local_new_shell,
            );
            shell.setCanShrink(1);
            connect(shell, Controller, newLocalShellClicked, self);
            const command = gtk.Button.new();
            renderActionButton(
                self.allocator,
                command,
                &projection.sessions.local_new_command,
            );
            command.setCanShrink(1);
            connect(command, Controller, newLocalCommandClicked, self);
            local_actions.append(shell.as(gtk.Widget));
            local_actions.append(command.as(gtk.Widget));
            actions.append(local_actions.as(gtk.Widget));
        } else if (projection.sessions.remote_targets.len == 0) {
            const remote = gtk.Button.new();
            renderActionButton(
                self.allocator,
                remote,
                &projection.sessions.remote_new_session,
            );
            remote.setCanShrink(1);
            actions.append(remote.as(gtk.Widget));
        } else {
            for (projection.sessions.remote_targets) |target| {
                const target_row = gtk.Box.new(.horizontal, 6);
                const target_label = ownedLabel(
                    self.allocator,
                    target.label,
                    "caption",
                ) catch continue;
                target_label.as(gtk.Widget).setHexpand(1);
                target_label.setEllipsize(.end);
                target_label.as(gtk.Widget).addCssClass("dim-label");
                const remote = gtk.Button.new();
                renderActionButton(
                    self.allocator,
                    remote,
                    &projection.sessions.remote_new_session,
                );
                remote.setCanShrink(1);
                const context = self.makeSessionCreateTargetContext(
                    target.id,
                ) catch continue;
                remote.as(gobject.Object).weakRef(
                    sessionCreateTargetContextWeakNotify,
                    context,
                );
                connect(
                    remote,
                    SessionCreateTargetContext,
                    newRemoteSessionClicked,
                    context,
                );
                target_row.append(target_label.as(gtk.Widget));
                target_row.append(remote.as(gtk.Widget));
                actions.append(target_row.as(gtk.Widget));
            }
        }
        action_row.setChild(actions.as(gtk.Widget));
        action_row.setActivatable(0);
        action_row.setSelectable(0);
        self.sessions_list.append(action_row.as(gtk.Widget));
    }

    fn applySidebarLayout(
        self: *Controller,
        sessions_visible: bool,
    ) void {
        const layout = layout_policy.bodyLayoutWithMapWidth(sessions_visible, self.layout_map_visible, self.map_width);
        const drawer_visible = layout.sidebar_visible;
        self.sessions_revealer.as(gtk.Widget).setVisible(
            @intFromBool(drawer_visible),
        );
        self.sessions_separator.as(gtk.Widget).setVisible(
            @intFromBool(drawer_visible and (self.layout_map_visible or self.inbox_visible)),
        );
        self.layout_region.as(gtk.Widget).setVisible(@intFromBool(self.layout_map_visible));
        self.layout_scroller.as(gtk.Widget).setVisible(
            @intFromBool(self.layout_map_visible),
        );
        self.inbox_scroller.as(gtk.Widget).setVisible(@intFromBool(self.inbox_visible));
        self.sessions_revealer.setRevealChild(
            @intFromBool(drawer_visible),
        );
        self.sessions_revealer.as(gtk.Widget).setSizeRequest(
            if (drawer_visible) layout_policy.sessions_outer_width else 0,
            -1,
        );
        self.sessions_panel.as(gtk.Widget).setSizeRequest(
            if (drawer_visible)
                layout_policy.sessions_outer_width -
                    (2 * layout_policy.controller_section_margin)
            else
                0,
            -1,
        );
        self.layout_scroller.as(gtk.Widget).setSizeRequest(
            if (self.layout_map_visible) self.map_width else 0,
            -1,
        );
        self.layout_scroller.setMaxContentWidth(
            layout_policy.layoutMapContentWidth(self.map_width),
        );
        self.width_resize_handle.as(gtk.Widget).setVisible(
            @intFromBool(self.layout_map_visible),
        );
        self.pane_content.setSizeRequest(
            self.controllerWidth(),
            -1,
        );
        if (self.host_alive and self.isVisible()) {
            self.host_window.resizePolarStationPane(
                self.controllerWidth(),
            );
        }
    }

    fn availablePanelWidth(self: *const Controller) c_int {
        var available = self.host_window.as(gtk.Widget).getWidth();
        if (self.host_window.as(gtk.Window).isMaximized() == 0 and self.host_window.as(gtk.Window).isFullscreen() == 0) {
            if (self.host_window.as(gtk.Native).getSurface()) |surface| if (self.host_window.as(gtk.Widget).getDisplay().getMonitorAtSurface(surface)) |monitor| {
                var geometry: gdk.Rectangle = undefined;
                monitor.getGeometry(&geometry);
                available = geometry.f_width;
            };
        }
        return @max(0, available - 321);
    }

    fn admitsPanels(self: *Controller, sessions: bool, layout: bool, inbox_visible: bool) bool {
        const width = layout_policy.bodyLayoutWithMapWidth(sessions, layout, self.map_width).controller_width +
            (if (inbox_visible) self.inbox_width else @as(c_int, 0));
        if (width <= self.availablePanelWidth()) return true;
        self.window_title.setSubtitle("Not enough room: close a panel or enlarge the window");
        return false;
    }

    fn controllerWidth(self: *const Controller) c_int {
        var width = layout_policy.bodyLayoutWithMapWidth(self.sessions_visible, self.layout_map_visible, self.map_width).controller_width;
        if (self.inbox_visible) width += self.inbox_width;
        return @max(width, layout_policy.layout_map_min_width);
    }

    fn setMapWidth(
        self: *Controller,
        requested_width: c_int,
        commit: bool,
    ) void {
        const extra = self.controllerWidth() - self.map_width;
        const selected = layout_policy.clampMapWidth(@min(requested_width, self.availablePanelWidth() - extra));
        if (commit) {
            if (self.width_preferences_store) |*preferences| {
                preferences.save(selected) catch {};
            }
        }
        if (selected == self.map_width) return;
        self.map_width = selected;
        self.applySidebarLayout(self.sessions_visible);
        self.updateMapBodyGeometry();
    }

    fn updateMapBodyGeometry(self: *Controller) void {
        for (self.pane_buttons.items) |pane| pane.updateDensity(self.map_width);
        const gap: c_uint = @intCast(
            layout_policy.scaledPaneGridGap(self.map_width),
        );
        for (self.map_bodies.items) |body| {
            body.widget.setSizeRequest(
                -1,
                layout_policy.scaledTabBodyHeight(
                    body.display_mode,
                    body.row_span,
                    self.map_width,
                ),
            );
            if (body.grid) |grid| {
                grid.setColumnSpacing(gap);
                grid.setRowSpacing(gap);
            }
        }
        self.layout_box.as(gtk.Widget).queueResize();
    }

    // Gesture offsets are relative to the handle, which moves with the Map.
    // Surface coordinates stay fixed while that handle is reallocated.
    fn widthDragSurfaceX(gesture: *gtk.GestureDrag) ?f64 {
        const event = gesture.as(gtk.EventController).getCurrentEvent() orelse return null;
        var x: f64 = 0;
        var y: f64 = 0;
        return if (event.getPosition(&x, &y) != 0) x else null;
    }

    fn widthResizeDragBegin(gesture: *gtk.GestureDrag, _: f64, _: f64, self: *Controller) callconv(.c) void {
        self.width_drag_start = self.map_width;
        self.width_drag_pointer_start = widthDragSurfaceX(gesture);
    }

    fn widthResizeDragUpdate(gesture: *gtk.GestureDrag, _: f64, _: f64, self: *Controller) callconv(.c) void {
        const start = self.width_drag_pointer_start orelse return;
        const current = widthDragSurfaceX(gesture) orelse return;
        self.setMapWidth(layout_policy.resizedMapWidth(self.width_drag_start, current - start), false);
    }

    fn widthResizeDragEnd(gesture: *gtk.GestureDrag, _: f64, _: f64, self: *Controller) callconv(.c) void {
        if (self.width_drag_pointer_start) |start| {
            const current = widthDragSurfaceX(gesture) orelse start + @as(f64, @floatFromInt(self.map_width - self.width_drag_start));
            self.setMapWidth(layout_policy.resizedMapWidth(self.width_drag_start, current - start), true);
        }
        self.width_drag_pointer_start = null;
    }

    fn renderWorkspaceMap(
        self: *Controller,
        projection: *const control.Projection,
        workspace_map: *const control.WorkspaceMap,
    ) void {
        for (self.map_action_popovers.items) |popover| {
            if (popover.as(gtk.Widget).getVisible() != 0) return;
            if (popover.as(gtk.Widget).getMapped() != 0) return;
            if (popover.as(gtk.Widget).getParent()) |parent| {
                if (gobject.ext.cast(gtk.MenuButton, parent)) |menu| {
                    if (menu.getActive() != 0) return;
                }
            }
        }
        if (!self.layout_map_visible) return;
        if (!self.map_inputs.changed(self.allocator, .{
            .workspace_id = workspace_map.workspace_id,
            .tabs = workspace_map.tabs,
            .located_pane = self.located_pane_id,
        })) return;
        self.clearWorkspaceMap();

        for (workspace_map.tabs, 0..) |tab, tab_index| {
            const card = self.makeTabCard(
                projection,
                &tab,
                tab_index,
                workspace_map.tabs,
            ) catch continue;
            self.layout_box.append(card.as(gtk.Widget));
        }
    }

    fn clearSessionLocate(self: *Controller) void {
        if (self.locate_clock) |clock| {
            gobject.signalHandlerDisconnect(clock.as(gobject.Object), self.locate_paint_handler);
            clock.as(gobject.Object).unref();
            self.locate_clock = null;
        }
        if (self.locate_animation) |animation| {
            animation.pause();
            animation.as(gobject.Object).unref();
            self.locate_animation = null;
        }
        for (self.pane_buttons.items) |pane| {
            pane.tile.removeCssClass("polar-pane-located");
            pane.tile.setOpacity(1);
        }
        if (self.located_pane_id) |id| self.allocator.free(id);
        if (self.located_workspace_id) |id| self.allocator.free(id);
        self.located_pane_id = null;
        self.located_workspace_id = null;
        self.locate_pulse_pending = false;
        self.locate_value = 1;
    }

    fn locateSession(self: *Controller, target: []const u8, session: []const u8) void {
        const executor = self.native_executor orelse return;
        const pane = executor.nativePaneIdForSession(target, session) orelse return;
        const workspace = executor.nativeWorkspaceSnapshot() orelse return;
        const pane_id = self.allocator.dupe(u8, pane) catch return;
        const workspace_id = self.allocator.dupe(u8, workspace.workspace_id) catch {
            self.allocator.free(pane_id);
            return;
        };
        self.clearSessionLocate();
        self.located_pane_id = pane_id;
        self.located_workspace_id = workspace_id;
        self.locate_pulse_pending = true;
        self.locate_value = 0;
        self.layout_map_visible = true;
        @import("ipc_client.zig").trace(55, pane_id, 0);
        self.render();
        const clock = self.layout_box.as(gtk.Widget).getFrameClock() orelse return;
        _ = clock.as(gobject.Object).ref();
        self.locate_clock = clock;
        self.locate_paint_handler = gdk.FrameClock.signals.after_paint.connect(clock, *Controller, locatePainted, self, .{ .after = true });
        clock.requestPhase(gdk.FrameClockPhase.flags_after_paint);
    }

    fn locatePainted(clock: *gdk.FrameClock, self: *Controller) callconv(.c) void {
        gobject.signalHandlerDisconnect(clock.as(gobject.Object), self.locate_paint_handler);
        self.locate_clock = null;
        defer clock.as(gobject.Object).unref();
        const id = self.located_pane_id orelse return;
        for (self.pane_buttons.items) |pane| {
            if (!std.mem.eql(u8, pane.pane_id, id)) continue;
            var x: f64 = 0;
            var y: f64 = 0;
            if (pane.tile.translateCoordinates(self.layout_box.as(gtk.Widget), 0, 0, &x, &y) != 0) {
                self.layout_scroller.getVadjustment().clampPage(y, y + @as(f64, @floatFromInt(pane.tile.getHeight())));
            }
            @import("ipc_client.zig").trace(56, id, 0);
            break;
        }
        if (!self.locate_pulse_pending) return;
        self.locate_pulse_pending = false;
        const settings = gtk.Settings.getDefault() orelse return;
        var value = gobject.ext.Value.new(bool);
        defer value.unset();
        settings.as(gobject.Object).getProperty("gtk-enable-animations", &value);
        if (value.getBoolean() == 0) {
            self.locateFrameValue(1);
            return;
        }
        // One native animation: three pulses, 1200ms total, no polling source.
        const target = adw.CallbackAnimationTarget.new(locateFrame, self, null);
        const animation = adw.TimedAnimation.new(self.layout_box.as(gtk.Widget), 0, 1, 1200, target.as(adw.AnimationTarget));
        animation.setEasing(.linear);
        self.locate_animation = animation.as(adw.Animation);
        animation.as(adw.Animation).play();
    }

    fn locateOpacity(self: *const Controller) f64 {
        return 0.55 + 0.45 * @cos(self.locate_value * 3 * 2 * std.math.pi);
    }
    fn locateFrame(value: f64, data: ?*anyopaque) callconv(.c) void {
        const self: *Controller = @ptrCast(@alignCast(data orelse return));
        self.locateFrameValue(value);
    }
    fn locateFrameValue(self: *Controller, value: f64) void {
        self.locate_value = value;
        const id = self.located_pane_id orelse return;
        for (self.pane_buttons.items) |pane| {
            if (std.mem.eql(u8, pane.pane_id, id)) pane.tile.setOpacity(self.locateOpacity());
        }
    }

    fn nativeSelectedTabID(self: *const Controller) ?[]const u8 {
        const executor = self.native_executor orelse return null;
        return executor.nativeSelectedTabId();
    }

    fn paneViewsForTab(
        _: *const Controller,
        _: *const control.Projection,
        tab: *const control.Tab,
        storage: *[9]control.Pane,
    ) []const control.Pane {
        if (tab.visual_pane_ids.len == tab.panes.len) {
            var count: usize = 0;
            for (tab.visual_pane_ids) |id| for (tab.panes) |pane| {
                if (std.mem.eql(u8, id, pane.id)) {
                    storage[count] = pane;
                    count += 1;
                    break;
                }
            };
            return storage[0..count];
        }
        for (tab.panes, 0..) |pane, index| storage[index] = pane;
        return storage[0..tab.panes.len];
    }

    fn nativeSessionAssignment(
        self: *const Controller,
        execution_target_id: []const u8,
        tmux_session_id: []const u8,
    ) ?presentation_executor.NativeSessionAssignment {
        const executor = self.native_executor orelse return null;
        return executor.nativeSessionAssignment(
            execution_target_id,
            tmux_session_id,
        );
    }

    fn displayModeForTab(_: *const Controller, tab: *const control.Tab) control.DisplayMode {
        return tab.display_mode;
    }
    fn placementForPane(self: *const Controller, tab: *const control.Tab, pane: control.Pane) !layout_policy.GridPlacement {
        var fallback: [9][]const u8 = undefined;
        for (tab.panes, 0..) |candidate, index| fallback[index] = candidate.id;
        const visual = if (tab.visual_pane_ids.len == tab.panes.len) tab.visual_pane_ids else fallback[0..tab.panes.len];
        return layout_policy.gridPlacementForMapWidth(visual, tab.display_mode, pane.id, self.map_width);
    }

    fn makeTabCard(
        self: *Controller,
        projection: *const control.Projection,
        tab: *const control.Tab,
        tab_index: usize,
        all_tabs: []const control.Tab,
    ) !*gtk.Box {
        const display_mode = self.displayModeForTab(tab);
        var pane_storage: [9]control.Pane = undefined;
        const panes = self.paneViewsForTab(
            projection,
            tab,
            &pane_storage,
        );
        const selected_tab = tab.selected;
        var total_rows: usize = 1;
        var total_columns: usize = 1;
        for (panes) |pane| {
            const placement = self.placementForPane(tab, pane) catch continue;
            total_rows = @max(
                total_rows,
                @as(usize, @intCast(
                    placement.row + placement.row_span,
                )),
            );
            total_columns = @max(
                total_columns,
                @as(usize, @intCast(
                    placement.column + placement.column_span,
                )),
            );
        }
        const card_layout = layout_policy.tabCardLayoutForMapWidth(
            display_mode,
            total_rows,
            total_columns,
            self.map_width,
        );
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
        if (selected_tab) card.as(gtk.Widget).addCssClass("polar-tab-selected");
        const context = try self.makeTabButtonContext(
            tab,
            tab_index,
            all_tabs.len,
        );
        context.card = card.as(gtk.Widget);
        card.as(gobject.Object).weakRef(
            tabButtonContextWeakNotify,
            context,
        );
        const drop_target = gtk.DropTarget.new(
            gobject.ext.types.string,
            gdk.DragAction.flags_move,
        );
        drop_target.setPreload(1);
        _ = gtk.DropTarget.signals.enter.connect(drop_target, *TabButtonContext, tabDropMotion, context, .{});
        _ = gtk.DropTarget.signals.motion.connect(drop_target, *TabButtonContext, tabDropMotion, context, .{});
        _ = gtk.DropTarget.signals.leave.connect(drop_target, *TabButtonContext, tabDropLeave, context, .{});
        _ = gtk.DropTarget.signals.drop.connect(
            drop_target,
            *TabButtonContext,
            paneDroppedOnTab,
            context,
            .{},
        );
        card.as(gtk.Widget).addController(
            drop_target.as(gtk.EventController),
        );

        const header = gtk.Box.new(.horizontal, 4);
        header.as(gtk.Widget).addCssClass("polar-tab-header");
        const header_actions = gtk.Box.new(.horizontal, 4);
        if (selected_tab) header.as(gtk.Widget).addCssClass("polar-tab-selected-header");
        header.as(gtk.Widget).setSizeRequest(
            -1,
            card_layout.header_height,
        );
        const tab_selection = gtk.ToggleButton.new();
        tab_selection.setActive(@intFromBool(selected_tab));
        const select = tab_selection.as(gtk.Button);
        select.setCanShrink(1);
        select.as(gtk.Widget).setHexpand(1);
        try setButtonLabel(self.allocator, select, tab.title);
        if (select.getChild()) |child| {
            if (gobject.ext.cast(gtk.Label, child)) |title_label| {
                title_label.setXalign(0);
                title_label.setEllipsize(.end);
                title_label.setWrap(0);
            }
        }
        try setWidgetName(self.allocator, select.as(gtk.Widget), tab.id);
        select.as(gtk.Widget).addCssClass("flat");
        select.as(gtk.Widget).setTooltipText(if (selected_tab) "Current Tab" else "Select Tab");
        // Selection already has a tinted header and outline; retain ordinary text.
        connect(select, Controller, tabClicked, self);

        const pane_count_text = try std.fmt.allocPrint(
            self.allocator,
            "{d}",
            .{
                panes.len,
            },
        );
        defer self.allocator.free(pane_count_text);
        const pane_count = label("", "caption");
        pane_count.setWrap(0);
        pane_count.setEllipsize(.end);
        pane_count.as(gtk.Widget).addCssClass("dim-label");
        try setLabel(self.allocator, pane_count, pane_count_text);
        const tab_drag = gtk.DragSource.new();
        tab_drag.setActions(gdk.DragAction.flags_move);
        _ = gtk.DragSource.signals.prepare.connect(tab_drag, *TabButtonContext, tabDragPrepare, context, .{});
        _ = gtk.DragSource.signals.drag_end.connect(tab_drag, *TabButtonContext, tabDragEnd, context, .{});
        _ = gtk.DragSource.signals.drag_cancel.connect(tab_drag, *TabButtonContext, tabDragCancel, context, .{});
        select.as(gtk.Widget).addController(tab_drag.as(gtk.EventController));
        header.append(gtk.Image.newFromIconName(if (display_mode == .monitor) "view-grid-symbolic" else "view-dual-symbolic").as(gtk.Widget));
        header.append(select.as(gtk.Widget));
        header.append(header_actions.as(gtk.Widget));
        header_actions.append(pane_count.as(gtk.Widget));
        const card_menu = gtk.MenuButton.new();
        card_menu.as(gtk.Widget).setSizeRequest(32, 32);
        card_menu.setIconName("view-more-symbolic");
        card_menu.setCanShrink(1);
        card_menu.as(gtk.Widget).addCssClass("flat");
        card_menu.as(gtk.Widget).setTooltipText("Tab actions");
        const card_popover = gtk.Popover.new();
        const card_actions = menuBox();
        card_popover.setChild(card_actions.as(gtk.Widget));
        card_menu.setPopover(card_popover);
        try self.trackMapActionPopover(card_popover);
        context.actions_popover = card_popover;
        header_actions.append(card_menu.as(gtk.Widget));

        const rename = gtk.Button.newWithLabel("Rename Tab…");
        rename.as(gtk.Widget).addCssClass("flat");
        renderActionState(self.allocator, rename, &tab.rename_action);
        connect(rename, TabButtonContext, tabRenameClicked, context);
        card_actions.append(rename.as(gtk.Widget));

        const move_earlier = gtk.Button.newWithLabel("Move Up");
        const move_later = gtk.Button.newWithLabel("Move Down");
        move_earlier.as(gtk.Widget).setSensitive(@intFromBool(tab_index > 0));
        move_later.as(gtk.Widget).setSensitive(@intFromBool(tab_index + 1 < all_tabs.len));
        connect(move_earlier, TabButtonContext, tabMoveEarlierClicked, context);
        connect(move_later, TabButtonContext, tabMoveLaterClicked, context);
        card_actions.append(move_earlier.as(gtk.Widget));
        card_actions.append(move_later.as(gtk.Widget));

        const delete = gtk.MenuButton.new();
        delete.setIconName("user-trash-symbolic");
        delete.setCanShrink(1);
        delete.as(gtk.Widget).addCssClass("flat");
        delete.as(gtk.Widget).addCssClass("destructive-action");
        renderMenuButtonState(self.allocator, delete, &tab.delete_action);
        if (tab.delete_action.confirmation) |confirmation| {
            const delete_popover = gtk.Popover.new();
            const delete_box = menuBox();
            const delete_title = try ownedLabel(
                self.allocator,
                confirmation.title,
                "heading",
            );
            const delete_message = try ownedLabel(
                self.allocator,
                confirmation.message,
                "caption",
            );
            const delete_actions = gtk.Box.new(.horizontal, 6);
            const delete_cancel = gtk.Button.newWithLabel("Cancel");
            const confirm_label = try self.allocator.dupeZ(
                u8,
                confirmation.confirm_label,
            );
            defer self.allocator.free(confirm_label);
            const delete_submit = gtk.Button.newWithLabel(confirm_label);
            delete_submit.as(gtk.Widget).addCssClass("destructive-action");
            delete_actions.append(delete_cancel.as(gtk.Widget));
            delete_actions.append(delete_submit.as(gtk.Widget));
            delete_box.append(delete_title.as(gtk.Widget));
            delete_box.append(delete_message.as(gtk.Widget));
            delete_box.append(delete_actions.as(gtk.Widget));
            delete_popover.setChild(delete_box.as(gtk.Widget));
            delete.setPopover(delete_popover);
            try self.trackMapActionPopover(delete_popover);
            context.delete_popover = delete_popover;
            connect(
                delete_cancel,
                TabButtonContext,
                tabDeleteCancelClicked,
                context,
            );
            connect(
                delete_submit,
                TabButtonContext,
                tabDeleteSubmitClicked,
                context,
            );
        } else {
            delete.as(gtk.Widget).setSensitive(0);
        }
        delete.setLabel("Delete Tab…");
        card_actions.append(delete.as(gtk.Widget));
        card.append(header.as(gtk.Widget));

        if (panes.len == 0) {
            const empty = gtk.Box.new(.vertical, 5);
            setMargins(empty.as(gtk.Widget), 18);
            empty.as(gtk.Widget).addCssClass("view");
            empty.as(gtk.Widget).setSizeRequest(
                -1,
                card_layout.body_height,
            );
            try self.map_bodies.append(self.allocator, .{
                .widget = empty.as(gtk.Widget),
                .grid = null,
                .display_mode = display_mode,
                .row_span = total_rows,
            });
            const empty_title = label("Add a Session to this Tab", "heading");
            const empty_detail = label(
                "This empty Tab is a quiet Session assignment target.",
                "dim-label",
            );
            empty.append(empty_title.as(gtk.Widget));
            empty.append(empty_detail.as(gtk.Widget));
            card.append(empty.as(gtk.Widget));
            return card;
        }

        const grid = gtk.Grid.new();
        const scaled_gap: c_uint = @intCast(
            layout_policy.scaledPaneGridGap(self.map_width),
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
        try self.map_bodies.append(self.allocator, .{
            .widget = grid.as(gtk.Widget),
            .grid = grid,
            .display_mode = display_mode,
            .row_span = total_rows,
        });
        for (panes) |pane| {
            const placement = self.placementForPane(tab, pane) catch continue;
            const tile = try self.makePaneTile(
                tab,
                &pane,
                card_layout,
                @intCast(placement.column_span),
                @intCast(total_columns),
            );
            grid.attach(
                tile.as(gtk.Widget),
                placement.column,
                placement.row,
                placement.column_span,
                placement.row_span,
            );
        }
        card.append(grid.as(gtk.Widget));
        return card;
    }

    fn makePaneTile(
        self: *Controller,
        tab: *const control.Tab,
        pane: *const control.Pane,
        card_layout: layout_policy.TabCardLayout,
        column_span: c_int,
        total_columns: c_int,
    ) !*gtk.Box {
        const tile = gtk.Box.new(.vertical, 4);
        tile.as(gtk.Widget).setHexpand(
            @intFromBool(card_layout.pane_tile_hexpand),
        );
        tile.as(gtk.Widget).setVexpand(
            @intFromBool(card_layout.pane_tile_vexpand),
        );
        tile.as(gtk.Widget).addCssClass("card");
        if (pane.selected) tile.as(gtk.Widget).addCssClass("accent");
        const context = try self.makePaneButtonContext(
            tab.id,
            pane.id,
        );
        tile.as(gobject.Object).weakRef(
            paneButtonContextWeakNotify,
            context,
        );

        const button = gtk.Button.new();
        button.setCanShrink(1);
        context.button = button;
        button.as(gtk.Widget).addCssClass("polar-station-pane-action");
        try setWidgetName(self.allocator, button.as(gtk.Widget), pane.id);
        button.as(gtk.Widget).setHexpand(1);
        button.as(gtk.Widget).setVexpand(1);
        button.as(gtk.Widget).addCssClass("flat");
        // Selection belongs to the containing card; this is not a primary action.
        connect(button, PaneButtonContext, paneClicked, context);
        // Observe the press without introducing a competing gesture recognizer.
        // GTK still owns drag thresholds, pointer grabs and button arbitration.
        const pointer = gtk.EventControllerLegacy.new();
        pointer.as(gtk.EventController).setPropagationPhase(.capture);
        _ = gtk.EventControllerLegacy.signals.event.connect(pointer, *Controller, panePointerEvent, self, .{});
        button.as(gtk.Widget).addController(pointer.as(gtk.EventController));
        const drag_source = gtk.DragSource.new();
        drag_source.setActions(gdk.DragAction.flags_move);
        drag_source.as(gtk.GestureSingle).setButton(1);
        drag_source.as(gtk.EventController).setPropagationPhase(.capture);
        _ = gtk.DragSource.signals.drag_begin.connect(drag_source, *PaneButtonContext, paneDragBegin, context, .{});
        _ = gtk.DragSource.signals.drag_cancel.connect(drag_source, *PaneButtonContext, paneDragCancel, context, .{});
        _ = gtk.DragSource.signals.drag_end.connect(drag_source, *PaneButtonContext, paneDragEnd, context, .{});
        _ = gtk.DragSource.signals.prepare.connect(
            drag_source,
            *PaneButtonContext,
            paneDragPrepare,
            context,
            .{},
        );
        button.as(gtk.Widget).addController(
            drag_source.as(gtk.EventController),
        );
        const pane_drop = gtk.DropTarget.new(gobject.ext.types.string, gdk.DragAction.flags_move);
        pane_drop.setPreload(1);
        _ = gtk.DropTarget.signals.enter.connect(pane_drop, *PaneButtonContext, paneDropMotion, context, .{});
        _ = gtk.DropTarget.signals.motion.connect(pane_drop, *PaneButtonContext, paneDropMotion, context, .{});
        _ = gtk.DropTarget.signals.leave.connect(pane_drop, *PaneButtonContext, paneDropLeave, context, .{});
        _ = gtk.DropTarget.signals.drop.connect(pane_drop, *PaneButtonContext, paneDroppedOnPane, context, .{});
        button.as(gtk.Widget).addController(pane_drop.as(gtk.EventController));

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
        try setLabel(self.allocator, session, pane.session_name);
        const source = label("", "caption");
        source.setWrap(0);
        source.setWidthChars(1);
        source.as(gtk.Widget).setHexpand(1);
        source.setEllipsize(.end);
        source.as(gtk.Widget).addCssClass("dim-label");
        try setLabel(self.allocator, source, switch (pane.target_kind) {
            .local => "Local",
            .ssh => "SSH",
            .wsl => "WSL",
        });
        primary.append(status.as(gtk.Widget));
        primary.append(session.as(gtk.Widget));
        if (pane.session_presence == .ended) {
            const ended = label("Ended", "caption");
            ended.as(gtk.Widget).addCssClass("error");
            primary.append(ended.as(gtk.Widget));
        }
        content.append(primary.as(gtk.Widget));
        content.append(source.as(gtk.Widget));
        const attention = paneAttentionStateLabel(
            self.allocator,
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
        if (self.located_pane_id) |id| {
            if (std.mem.eql(u8, id, pane.id)) {
                tile.as(gtk.Widget).addCssClass("polar-pane-located");
                tile.as(gtk.Widget).setOpacity(self.locateOpacity());
            }
        }
        pane_ref.updateDensity(self.map_width);
        self.pane_buttons.append(self.allocator, pane_ref) catch {};
        const tooltip = try std.fmt.allocPrintSentinel(self.allocator, "{s}\n{s}\n{s}\n{s}", .{
            pane.session_name,                                                                                                               pane.target_label,
            if (pane.session_presence == .ended) "Session ended" else if (pane.transport_status == .online) "Connected" else "Disconnected", if (attention) |value| std.mem.span(value.getLabel()) else "",
        }, 0);
        defer self.allocator.free(tooltip);
        button.as(gtk.Widget).setTooltipText(tooltip);

        const context_menu = gtk.Popover.new();
        const context_box = menuBox();
        const remove = gtk.Button.newWithLabel(
            if (tab.panes.len == 1)
                "Remove Pane (Keep Empty Tab)"
            else
                "Remove from Tab",
        );
        remove.as(gtk.Widget).addCssClass("destructive-action");
        context_box.append(remove.as(gtk.Widget));
        context_menu.setChild(context_box.as(gtk.Widget));
        context_menu.as(gtk.Widget).setParent(button.as(gtk.Widget));
        self.map_popovers.append(self.allocator, context_menu) catch {};
        self.trackMapActionPopover(context_menu) catch {};
        context.context_menu = context_menu;
        context.remove_button = remove;
        connect(remove, PaneButtonContext, paneRemoveClicked, context);
        const context_click = gtk.GestureClick.new();
        context_click.as(gtk.GestureSingle).setButton(3);
        _ = gtk.GestureClick.signals.released.connect(
            context_click,
            *PaneButtonContext,
            paneContextMenuClicked,
            context,
            .{},
        );
        button.as(gtk.Widget).addController(
            context_click.as(gtk.EventController),
        );
        tile.append(button.as(gtk.Widget));
        return tile;
    }

    fn renderBottomBar(
        self: *Controller,
        projection: *const control.Projection,
    ) void {
        if (!self.bottom_inputs.changed(self.allocator, .{ .actions = projection.projected_actions, .bottom = projection.bottom_bar, .header = projection.header, .sessions = self.sessions_visible, .layout = self.layout_map_visible, .inbox = self.inbox_visible })) return;
        const sessions = projection.action("sessions.toggle");
        const layout = projection.action("presentation.layout.toggle");
        const inbox_action = projection.action("presentation.inbox.toggle");
        const resources = projection.action("resources.open");
        const more = projection.action("more.open");
        if (sessions) |action| {
            var sessions_action = action.*;
            sessions_action.selected = self.sessions_visible;
            renderToggleState(
                self.allocator,
                self.sessions_button,
                self.sessions_button_label,
                &sessions_action,
            );
        }
        if (layout) |action| {
            var mode_action = action.*;
            mode_action.selected = self.layout_map_visible;
            renderToggleState(
                self.allocator,
                self.mode_button,
                self.mode_button_label,
                &mode_action,
            );
        }
        if (inbox_action) |action| {
            var projected = action.*;
            projected.selected = self.inbox_visible;
            renderToggleState(
                self.allocator,
                self.iphone_button,
                self.iphone_button_label,
                &projected,
            );
        }
        if (resources) |action| {
            renderActionState(
                self.allocator,
                self.resources_button,
                action,
            );
            setLabel(
                self.allocator,
                self.resources_button_label,
                action.label,
            ) catch {};
            self.resources_image.setFromIconName(
                if (!action.enabled)
                    "drive-harddisk-symbolic"
                else if (action.selected)
                    "emblem-ok-symbolic"
                else
                    "drive-harddisk-symbolic",
            );
        }
        if (more) |action| {
            renderMenuButtonState(
                self.allocator,
                self.more_menu,
                action,
            );
            setLabel(
                self.allocator,
                self.more_menu_label,
                action.label,
            ) catch {};
        }
    }

    fn renderMoreMenu(
        self: *Controller,
        projection: *const control.Projection,
    ) void {
        if (!self.more_inputs.changed(self.allocator, .{ projection.action("managed.terminal.font.size"), projection.action("iphone.open"), projection.action("workspace.create") })) return;
        clearBox(self.more_menu_box);

        const font_size = menuRowButton(
            "font-select-symbolic",
            "Ghostty Font Size…",
        );
        font_size.button.as(gtk.Widget).addCssClass("flat");
        if (projection.action("managed.terminal.font.size")) |action| {
            setLabel(self.allocator, font_size.text, action.label) catch {};
            renderActionState(self.allocator, font_size.button, action);
            connect(font_size.button, Controller, fontSizeClicked, self);
        } else {
            font_size.button.as(gtk.Widget).setSensitive(0);
        }

        const first_divider = gtk.Separator.new(.horizontal);

        const new_workspace = menuRowButton(
            "tab-new-symbolic",
            "New Workspace",
        );

        const iphone = menuRowButton("phone-symbolic", "iPhone Companion…");
        iphone.button.as(gtk.Widget).addCssClass("flat");
        if (projection.action("iphone.open")) |action| {
            renderActionState(self.allocator, iphone.button, action);
            connect(iphone.button, Controller, iphoneClicked, self);
        } else {
            iphone.button.as(gtk.Widget).setSensitive(0);
        }
        new_workspace.button.as(gtk.Widget).addCssClass("flat");
        if (projection.action("workspace.create")) |action| {
            renderActionState(self.allocator, new_workspace.button, action);
            connect(new_workspace.button, Controller, newWorkspaceClicked, self);
        } else {
            new_workspace.button.as(gtk.Widget).setSensitive(0);
        }

        self.more_menu_box.append(font_size.button.as(gtk.Widget));
        self.more_menu_box.append(first_divider.as(gtk.Widget));
        self.more_menu_box.append(new_workspace.button.as(gtk.Widget));
        self.more_menu_box.append(iphone.button.as(gtk.Widget));
    }

    fn renderInbox(self: *Controller) void {
        if (!self.inbox_visible) return;
        if (!self.inbox_inputs.changed(self.allocator, .{
            .page = self.inbox_model.page(),
            .filters = self.inbox_filter_values,
        })) return;
        clearBox(self.inbox_box);
        const header = gtk.Box.new(.horizontal, 8);
        header.append(label("Inbox", "heading").as(gtk.Widget));
        const spacer = gtk.Box.new(.horizontal, 0);
        spacer.as(gtk.Widget).setHexpand(1);
        header.append(spacer.as(gtk.Widget));
        const refresh = gtk.Button.newWithLabel("Restore Saved Layout…");
        refresh.as(gtk.Widget).addCssClass("flat");
        connect(refresh, Controller, inboxRefreshClicked, self);
        header.append(refresh.as(gtk.Widget));
        self.inbox_box.append(header.as(gtk.Widget));
        const page = self.inbox_model.page() orelse {
            self.inbox_box.append(label("Loading Inbox…", "caption").as(gtk.Widget));
            return;
        };
        if (page.overview) |overview| {
            self.renderInboxFilters(overview);
            for (overview.errors) |message| self.inbox_box.append((ownedLabel(self.allocator, message, "error") catch continue).as(gtk.Widget));
            self.inbox_box.append(label("Current activity / needs attention", "heading").as(gtk.Widget));
            for (overview.activity) |row| self.renderInboxOverviewRow(row);
            if (overview.activity.len == 0) self.inbox_box.append(label("No current activity", "caption").as(gtk.Widget));
            self.inbox_box.append(label("Recent records", "heading").as(gtk.Widget));
            for (overview.records) |row| {
                var rendered = false;
                if (row.terminal_message_id) |id| for (page.messages) |message| {
                    if (std.mem.eql(u8, message.id, id)) {
                        self.renderInboxMessage(message, row);
                        rendered = true;
                        break;
                    }
                };
                if (!rendered) self.renderInboxOverviewRow(row);
            }
            if (overview.records.len == 0) self.inbox_box.append(label("No recent records", "caption").as(gtk.Widget));
            return;
        }
        if (page.messages.len == 0) {
            self.inbox_box.append(label("No monitor messages", "title-3").as(gtk.Widget));
            self.inbox_box.append(label("Working, review, approval, decision, permission, and failure updates appear here.", "caption").as(gtk.Widget));
            return;
        }
        for (page.messages) |message| self.renderInboxMessage(message, null);
    }

    fn renderInboxMessage(self: *Controller, message: inbox.Message, row: ?inbox.OverviewRow) void {
        const card = gtk.Box.new(.vertical, 7);
        card.as(gtk.Widget).addCssClass("card");
        setMargins(card.as(gtk.Widget), 10);
        const title_text = std.fmt.allocPrint(self.allocator, "{s}{s}", .{ message.session_label, if (message.unread) " · NEW" else "" }) catch return;
        defer self.allocator.free(title_text);
        card.append((ownedLabel(self.allocator, title_text, "heading") catch return).as(gtk.Widget));
        card.append((ownedLabel(self.allocator, message.summary, "caption") catch return).as(gtk.Widget));
        if (message.ai_invocation) |ai| {
            const ai_text = if (ai.@"error") |failure|
                std.fmt.allocPrint(self.allocator, "AI failed: {s}", .{failure}) catch null
            else
                std.fmt.allocPrint(self.allocator, "AI · {s} · {d} ms", .{ ai.model orelse "Auto", ai.duration_ms }) catch null;
            if (ai_text) |value| {
                defer self.allocator.free(value);
                card.append((ownedLabel(self.allocator, value, "caption") catch return).as(gtk.Widget));
            }
        }
        if (message.draft) |draft| {
            const draft_entry = gtk.Entry.new();
            setEntryText(self.allocator, draft_entry, draft) catch return;
            const save = gtk.Button.newWithLabel("Save Draft");
            const edit_context = self.makeInboxContext(message.id, message.revision, draft_entry) catch return;
            save.as(gobject.Object).weakRef(inboxContextWeakNotify, edit_context);
            connect(save, InboxContext, inboxEditClicked, edit_context);
            appendRow(card, draft_entry.as(gtk.Widget), save.as(gtk.Widget));
        }
        const reply = gtk.Entry.new();
        reply.setPlaceholderText("Reply to this message");
        const generate = gtk.Button.newWithLabel("Generate Draft");
        const reply_context = self.makeInboxContext(message.id, message.revision, reply) catch return;
        generate.as(gobject.Object).weakRef(inboxContextWeakNotify, reply_context);
        connect(generate, InboxContext, inboxReplyClicked, reply_context);
        appendRow(card, reply.as(gtk.Widget), generate.as(gtk.Widget));
        const actions = gtk.Box.new(.horizontal, 7);
        const open = gtk.Button.newWithLabel("Open Pane");
        const open_context = self.makeInboxContext(message.id, message.revision, null) catch return;
        open.as(gobject.Object).weakRef(inboxContextWeakNotify, open_context);
        connect(open, InboxContext, inboxOpenClicked, open_context);
        actions.append(open.as(gtk.Widget));
        if (std.mem.eql(u8, message.draft_status, "ready")) {
            const approve = gtk.Button.newWithLabel(if (message.stale_context)
                if (self.terminal_delivery_mode == .send) "Restore & Send" else "Restore & Insert"
            else if (self.terminal_delivery_mode == .send)
                "Approve & Send"
            else
                "Approve & Insert");
            approve.as(gtk.Widget).addCssClass("suggested-action");
            const approve_context = self.makeInboxContext(message.id, message.revision, null) catch return;
            approve.as(gobject.Object).weakRef(inboxContextWeakNotify, approve_context);
            connect(approve, InboxContext, inboxApproveClicked, approve_context);
            actions.append(approve.as(gtk.Widget));
        }
        card.append(actions.as(gtk.Widget));
        self.inbox_box.append(card.as(gtk.Widget));
        if (row) |value| self.applyInboxRowVisibility(card.as(gtk.Widget), value);
    }

    fn renderInboxFilters(self: *Controller, overview: inbox.Overview) void {
        const row = gtk.Box.new(.horizontal, 6);
        self.inbox_box.append(row.as(gtk.Widget));
        for (0..3) |i| {
            const combo = gtk.ComboBoxText.new();
            self.inbox_filters[i] = combo;
            combo.append("", switch (i) {
                0 => "All sources",
                1 => "All machines",
                else => "All states",
            });
            var seen = std.StringHashMap(void).init(self.allocator);
            defer seen.deinit();
            for ([_][]const inbox.OverviewRow{ overview.activity, overview.records }) |items| for (items) |item| {
                const value = switch (i) {
                    0 => item.source,
                    1 => item.execution_target_id,
                    else => item.task_status,
                };
                const entry = seen.getOrPut(value) catch continue;
                if (entry.found_existing) continue;
                const id = self.allocator.dupeZ(u8, value) catch continue;
                defer self.allocator.free(id);
                const title = self.allocator.dupeZ(u8, if (i == 2) item.task_label else value) catch continue;
                defer self.allocator.free(title);
                combo.append(id, title);
            };
            if (i == 2) combo.append("disconnected", "Observation disconnected");
            const selected = self.allocator.dupeZ(u8, self.inbox_filter_values[i] orelse "") catch continue;
            defer self.allocator.free(selected);
            if (combo.as(gtk.ComboBox).setActiveId(selected) == 0) {
                combo.append(selected, selected);
                _ = combo.as(gtk.ComboBox).setActiveId(selected);
            }
            _ = gtk.ComboBox.signals.changed.connect(combo.as(gtk.ComboBox), *Controller, inboxFilterChanged, self, .{});
            combo.as(gtk.Widget).setHexpand(1);
            if (i < 2) row.append(combo.as(gtk.Widget)) else self.inbox_box.append(combo.as(gtk.Widget));
        }
    }
    fn inboxRowVisible(self: *Controller, row: inbox.OverviewRow) bool {
        const values = [_][]const u8{ row.source, row.execution_target_id, row.task_status };
        for (values, 0..) |value, i| if (self.inbox_filter_values[i]) |filter| {
            if (filter.len == 0) continue;
            if (i == 2 and std.mem.eql(u8, filter, "disconnected")) {
                if (!row.stale) return false;
            } else if (!std.mem.eql(u8, value, filter)) return false;
        };
        return true;
    }
    fn applyInboxRowVisibility(self: *Controller, widget: *gtk.Widget, row: inbox.OverviewRow) void {
        setWidgetName(self.allocator, widget, row.id) catch {};
        widget.setVisible(@intFromBool(self.inboxRowVisible(row)));
    }
    fn inboxFilterChanged(combo: *gtk.ComboBox, self: *Controller) callconv(.c) void {
        for (self.inbox_filters, 0..) |candidate, i| {
            if (candidate == null or candidate.?.as(gtk.ComboBox) != combo) continue;
            const active = combo.getActiveId() orelse return;
            const copy = self.allocator.dupe(u8, std.mem.span(active)) catch return;
            if (self.inbox_filter_values[i]) |old| self.allocator.free(old);
            self.inbox_filter_values[i] = copy;
        }
        const overview = (self.inbox_model.page() orelse return).overview orelse return;
        // Filter existing cards in place; don't destroy a focused GTK control
        // while its changed signal is being dispatched.
        var child = self.inbox_box.as(gtk.Widget).getFirstChild();
        while (child) |widget| : (child = widget.getNextSibling()) {
            const name = std.mem.span(widget.getName());
            for ([_][]const inbox.OverviewRow{ overview.activity, overview.records }) |items| for (items) |row| {
                if (std.mem.eql(u8, name, row.id)) widget.setVisible(@intFromBool(self.inboxRowVisible(row)));
            };
        }
    }
    fn renderInboxOverviewRow(self: *Controller, row: inbox.OverviewRow) void {
        const card = gtk.Box.new(.vertical, 6);
        card.as(gtk.Widget).addCssClass("card");
        setMargins(card.as(gtk.Widget), 10);
        card.append((ownedLabel(self.allocator, row.title, "heading") catch return).as(gtk.Widget));
        const origin = std.fmt.allocPrint(self.allocator, "{s} · {s}", .{ row.source, row.execution_target_id }) catch return;
        defer self.allocator.free(origin);
        card.append((ownedLabel(self.allocator, origin, "caption") catch return).as(gtk.Widget));
        card.append((ownedLabel(self.allocator, row.task_label, "heading") catch return).as(gtk.Widget));
        card.append((ownedLabel(self.allocator, row.observation_label, if (row.stale) "warning" else "caption") catch return).as(gtk.Widget));
        if (row.summary.len > 0) card.append((ownedLabel(self.allocator, row.summary, "caption") catch return).as(gtk.Widget));
        for (row.deliveries) |delivery| {
            card.append((ownedLabel(self.allocator, delivery.label, "caption") catch continue).as(gtk.Widget));
            if (delivery.error_code) |code| card.append((ownedLabel(self.allocator, code, "error") catch continue).as(gtk.Widget));
        }
        if (row.codex_binding_id != null or row.session != null) {
            const open = gtk.Button.newWithLabel(if (row.codex_binding_id != null) "Open original Thread" else "Open Session");
            const context = self.makeInboxContext(row.id, 0, null) catch return;
            open.as(gobject.Object).weakRef(inboxContextWeakNotify, context);
            connect(open, InboxContext, inboxOverviewOpenClicked, context);
            card.append(open.as(gtk.Widget));
        }
        self.applyInboxRowVisibility(card.as(gtk.Widget), row);
        self.inbox_box.append(card.as(gtk.Widget));
    }
    fn inboxOverviewOpenClicked(_: *gtk.Button, context: *InboxContext) callconv(.c) void {
        const self = context.controller;
        const overview = (self.inbox_model.page() orelse return).overview orelse return;
        for ([_][]const inbox.OverviewRow{ overview.activity, overview.records }) |items| for (items) |row| {
            if (!std.mem.eql(u8, context.message_id, row.id)) continue;
            if (row.codex_binding_id) |binding| {
                self.dispatch(.{ .open_codex_thread = .{ .owner = row.codex_owner_id orelse return, .binding = binding } });
            } else if (row.session) |session| self.dispatch(.{ .open_inbox_session = .{ .execution_target_id = session.execution_target, .tmux_session_id = session.tmux_session_id } });
            return;
        };
    }

    fn makeInboxContext(self: *Controller, id: []const u8, revision: u64, entry: ?*gtk.Entry) !*InboxContext {
        const context = try self.allocator.create(InboxContext);
        errdefer self.allocator.destroy(context);
        context.* = .{ .controller = self, .message_id = try self.allocator.dupe(u8, id), .revision = revision, .entry = entry };
        return context;
    }

    pub fn presentIdeaSessionPicker(self:*Controller,pane:[]const u8,view:[]const u8,context:?*anyopaque,callback:*const fn (?*anyopaque,[]const u8,[]const u8,[]const u8,[]const u8,[]const u8,[]const u8) void) void {
        const projection=self.model.projection() orelse return;
        self.idea_binding_context=context;self.idea_binding_callback=callback;
        self.presentSessionEditor(.codex_agent,projection.sessions.local_execution_target_id,pane,view);
    }
    fn presentSessionCreateEditor(self:*Controller,kind:SessionCreateKind,target_id:[]const u8) void {
        self.presentSessionEditor(kind,target_id,null,null);
    }
    fn presentSessionEditor(self:*Controller,kind:SessionCreateKind,target_id:[]const u8,bind_pane:?[]const u8,bind_view:?[]const u8) void {
        const projection = self.model.projection() orelse return;
        if (self.session_create_window) |window| {
            if (window.as(gtk.Widget).getVisible() != 0) {
                window.as(gtk.Window).present();
                return;
            }
        }
        const initiating = if(self.native_executor) |executor| blk: {
            if(bind_pane!=null) {
                const snapshot=executor.nativeWorkspaceSnapshot() orelse return;
                break :blk @TypeOf(executor.quickCreateTarget(self.host_window) catch unreachable){.workspace_id=snapshot.workspace_id,.tab_id=snapshot.selected_tab_id,.source=null};
            }
            break :blk executor.quickCreateTarget(self.host_window) catch return;
        } else return;
        if(self.idea_binding_pane) |v| self.allocator.free(v);
        if(self.idea_binding_view) |v| self.allocator.free(v);
        self.idea_binding_pane=if(bind_pane) |v| self.allocator.dupe(u8,v) catch null else null;
        self.idea_binding_view=if(bind_view) |v| self.allocator.dupe(u8,v) catch null else null;
        if (self.creation_request_id) |old| self.allocator.free(old);
        if (self.creation_request_intent) |old| self.allocator.free(old);
        self.creation_request_id = null;
        self.creation_request_intent = null;
        const workspace_copy = self.allocator.dupe(u8, initiating.workspace_id) catch return;
        const tab_copy = self.allocator.dupe(u8, initiating.tab_id) catch {
            self.allocator.free(workspace_copy);
            return;
        };
        if (self.session_create_workspace_id) |old| self.allocator.free(old);
        if (self.session_create_tab_id) |old| self.allocator.free(old);
        self.session_create_workspace_id = workspace_copy;
        self.session_create_tab_id = tab_copy;
        if (self.session_create_window == null) session_editor_window.create(Controller, self);

        if (self.session_create_pending) {
            self.session_create_window.?.as(gtk.Window).present();
            return;
        }
        self.session_create_configuring = true;
        if (self.session_create_target_id) |value| {
            self.allocator.free(value);
        }
        self.session_create_target_id = self.allocator.dupe(
            u8,
            target_id,
        ) catch return;
        self.session_create_error.?.setLabel("");
        self.session_create_kind = kind;
        const group_json = std.json.Stringify.valueAlloc(self.allocator, projection.session_groups, .{}) catch return;
        defer self.allocator.free(group_json);
        const groups = std.json.parseFromSlice([]control.SessionTargetGroup, self.allocator, group_json, .{ .allocate = .alloc_always }) catch return;
        const endpoint_json = std.json.Stringify.valueAlloc(self.allocator, projection.codex_endpoints, .{}) catch return;
        defer self.allocator.free(endpoint_json);
        const endpoints = std.json.parseFromSlice([]control.CodexEndpoint, self.allocator, endpoint_json, .{ .allocate = .alloc_always, .ignore_unknown_fields = true }) catch return;
        if (self.session_create_endpoints) |old| old.deinit();
        self.session_create_endpoints = endpoints;
        if (self.session_create_groups) |old| old.deinit();
        self.session_create_groups = groups;
        const choices = gtk.StringList.new(null);
        defer choices.unref();
        var selected: u32 = 0;
        for (groups.value, 0..) |group, i| {
            const z = self.allocator.dupeZ(u8, group.title) catch continue;
            defer self.allocator.free(z);
            choices.append(z);
            if (std.mem.eql(u8, group.id, target_id)) selected = @intCast(i);
        }
        self.session_create_machine.?.setModel(choices.as(@import("gio").ListModel));
        self.session_create_machine.?.setSelected(selected);
        self.session_create_type.?.setSelected(if (kind == .local_command) 1 else if (kind == .codex_agent) 2 else if (kind == .claude_agent) 3 else 0);
        self.session_create_mode.?.setSelected(if(bind_pane!=null) 1 else 0);
        self.session_create_resume_id.?.as(gtk.Editable).setText("");
        self.session_create_local_cli.?.setActive(0);
        self.setSessionCreateEditing(true);
        self.closeSharedEditor();
        self.session_create_name.?.as(gtk.Editable).setText("");
        self.session_create_directory.?.as(gtk.Editable).setText("");
        self.session_create_command.?.as(gtk.Editable).setText("");
        const command_visible = kind == .local_command;
        self.session_create_auto_restart.?.setActive(0);
        self.session_create_auto_restart.?.as(gtk.Widget).setVisible(@intFromBool(command_visible));
        self.session_create_command.?.as(gtk.Widget).setVisible(
            @intFromBool(command_visible),
        );
        self.session_create_command_label.?.as(gtk.Widget).setVisible(
            @intFromBool(command_visible),
        );
        self.session_create_yolo.?.setActive(1);
        self.session_create_advanced.?.setExpanded(1);
        self.session_create_configuring = false;
        self.updateSessionCreateChoices();
        self.session_create_window.?.as(gtk.Window).present();
        self.refreshEditorEndpoints(projection);
        self.requestCreationDirectory();
        _ = self.session_create_name.?.as(gtk.Widget).grabFocus();
    }

    fn presentFontSizeEditor(self: *Controller) void {
        if (self.font_size_window == null) {
            const window = auxiliary.create(self.app);
            window.as(gtk.Window).setTitle("Ghostty Font Size");
            window.as(gtk.Window).setIconName(
                "com.polarealm.PolarStation",
            );
            window.as(gtk.Window).setDefaultSize(320, 170);
            window.as(gtk.Window).setHideOnClose(1);
            window.as(gtk.Window).setTransientFor(
                self.host_window.as(gtk.Window),
            );

            const content = gtk.Box.new(.vertical, 16);
            setMargins(content.as(gtk.Widget), 22);
            content.append(
                label("Ghostty Font Size", "title-2")
                    .as(gtk.Widget),
            );
            const spin = gtk.SpinButton.newWithRange(10, 32, 1);
            spin.as(gtk.Widget).setHalign(.fill);
            content.append(spin.as(gtk.Widget));
            const buttons = gtk.Box.new(.horizontal, 8);
            buttons.as(gtk.Widget).setHalign(.end);
            const cancel = gtk.Button.newWithLabel("Cancel");
            const apply = gtk.Button.newWithLabel("Apply");
            apply.as(gtk.Widget).addCssClass("suggested-action");
            connect(cancel, Controller, fontSizeCancelClicked, self);
            connect(apply, Controller, fontSizeApplyClicked, self);
            buttons.append(cancel.as(gtk.Widget));
            buttons.append(apply.as(gtk.Widget));
            content.append(buttons.as(gtk.Widget));
            window.setContent(content.as(gtk.Widget));
            self.font_size_window = window;
            self.font_size_spin = spin;
        }
        self.font_size_spin.?.setValue(
            @floatFromInt(
                if (self.native_executor) |executor|
                    executor.nativeManagedTerminalFontSize() orelse 14
                else
                    14,
            ),
        );
        self.font_size_window.?.as(gtk.Window).present();
    }

    pub fn presentWorkspaceFontEditor(self: *Controller) void {
        self.presentFontSizeEditor();
    }

    fn presentNewWorkspaceEditor(self: *Controller) void {
        if (self.new_workspace_window == null) {
            const window = auxiliary.create(self.app);
            window.as(gtk.Window).setTitle("New Workspace");
            window.as(gtk.Window).setIconName(
                "com.polarealm.PolarStation",
            );
            window.as(gtk.Window).setDefaultSize(340, 170);
            window.as(gtk.Window).setHideOnClose(1);
            window.as(gtk.Window).setTransientFor(
                self.host_window.as(gtk.Window),
            );
            const content = gtk.Box.new(.vertical, 16);
            setMargins(content.as(gtk.Widget), 22);
            content.append(
                label("New Workspace", "title-2")
                    .as(gtk.Widget),
            );
            const entry = gtk.Entry.new();
            entry.setPlaceholderText("Workspace name");
            content.append(entry.as(gtk.Widget));
            const buttons = gtk.Box.new(.horizontal, 8);
            buttons.as(gtk.Widget).setHalign(.end);
            const cancel = gtk.Button.newWithLabel("Cancel");
            const create_button = gtk.Button.newWithLabel("Create");
            create_button.as(gtk.Widget).addCssClass("suggested-action");
            connect(cancel, Controller, newWorkspaceCancelClicked, self);
            connect(create_button, Controller, newWorkspaceApplyClicked, self);
            buttons.append(cancel.as(gtk.Widget));
            buttons.append(create_button.as(gtk.Widget));
            content.append(buttons.as(gtk.Widget));
            window.setContent(content.as(gtk.Widget));
            self.new_workspace_window = window;
            self.new_workspace_entry = entry;
        }
        self.new_workspace_entry.?.as(gtk.Editable).setText(
            "Workspace",
        );
        self.new_workspace_window.?.as(gtk.Window).present();
    }

    fn presentRenameWorkspaceEditor(self: *Controller) void {
        const executor = self.native_executor orelse return;
        self.presentRenameEditor(
            .workspace,
            "Rename Workspace",
            "Workspace name",
            executor.nativeWorkspaceName() orelse "Workspace",
        ) catch {};
    }

    // Only widget mechanics and owned native draft identity live here. The
    // existing native catalog or Rust Host still validates and applies intents.
    fn presentRenameEditor(
        self: *Controller,
        target: RenameTarget,
        title: [:0]const u8,
        placeholder: [:0]const u8,
        current: []const u8,
    ) !void {
        if (self.rename_target != null) {
            self.rename_window.?.as(gtk.Window).present();
            return;
        }
        const owned_target = try target.clone(self.allocator);
        errdefer owned_target.deinit(self.allocator);
        const current_name = try self.allocator.dupeZ(u8, current);
        defer self.allocator.free(current_name);
        if (self.rename_window == null) {
            const window = auxiliary.create(self.app);
            window.as(gtk.Window).setIconName("com.polarealm.PolarStation");
            window.as(gtk.Window).setDefaultSize(360, 200);
            window.as(gtk.Window).setHideOnClose(1);
            window.as(gtk.Window).setTransientFor(self.host_window.as(gtk.Window));
            window.as(gtk.Window).setModal(1);
            const toolbar = adw.ToolbarView.new();
            toolbar.addTopBar(adw.HeaderBar.new().as(gtk.Widget));
            const content = gtk.Box.new(.vertical, 12);
            setMargins(content.as(gtk.Widget), 22);
            const heading = label("", "title-2");
            content.append(heading.as(gtk.Widget));
            const entry = gtk.Entry.new();
            entry.setActivatesDefault(1);
            _ = gtk.Editable.signals.changed.connect(entry.as(gtk.Editable), *Controller, renameChanged, self, .{});
            content.append(entry.as(gtk.Widget));
            const message = label("", "error");
            message.setWrap(1);
            content.append(message.as(gtk.Widget));
            const buttons = gtk.Box.new(.horizontal, 8);
            buttons.as(gtk.Widget).setHalign(.end);
            const cancel = gtk.Button.newWithLabel("Cancel");
            const submit = gtk.Button.newWithLabel("Rename");
            submit.as(gtk.Widget).addCssClass("suggested-action");
            connect(cancel, Controller, renameCancelClicked, self);
            connect(submit, Controller, renameSubmitClicked, self);
            buttons.append(cancel.as(gtk.Widget));
            buttons.append(submit.as(gtk.Widget));
            content.append(buttons.as(gtk.Widget));
            toolbar.setContent(content.as(gtk.Widget));
            window.setContent(toolbar.as(gtk.Widget));
            window.as(gtk.Window).setDefaultWidget(submit.as(gtk.Widget));
            _ = gtk.Window.signals.close_request.connect(window.as(gtk.Window), *Controller, renameCloseRequested, self, .{});
            const keys = gtk.EventControllerKey.new();
            _ = gtk.EventControllerKey.signals.key_pressed.connect(keys, *Controller, renameKeyPressed, self, .{});
            window.as(gtk.Widget).addController(keys.as(gtk.EventController));
            self.rename_window = window;
            self.rename_heading = heading;
            self.rename_entry = entry;
            self.rename_submit = submit;
            self.rename_cancel = cancel;
            self.rename_error = message;
        }
        self.rename_target = owned_target;
        self.rename_pending = false;
        self.rename_window.?.as(gtk.Window).setTitle(title);
        self.rename_heading.?.setText(title);
        self.rename_entry.?.setPlaceholderText(placeholder);
        self.rename_entry.?.as(gtk.Editable).setText(current_name);
        self.rename_entry.?.as(gtk.Widget).setSensitive(1);
        self.rename_cancel.?.as(gtk.Widget).setSensitive(1);
        self.rename_error.?.as(gtk.Widget).setVisible(0);
        renameChanged(self.rename_entry.?.as(gtk.Editable), self);
        self.rename_window.?.as(gtk.Window).present();
        _ = self.rename_entry.?.as(gtk.Widget).grabFocus();
        self.rename_entry.?.as(gtk.Editable).selectRegion(0, -1);
    }

    pub fn presentConnections(self: *Controller) void {
        const projection = self.model.projection() orelse return;
        if (self.connections == null) {
            const window = self.allocator.create(connections_window.Window) catch return;
            window.* = connections_window.Window.init(self.allocator, self.app, connectionChanged, self);
            window.codex_send = self.codex_send;
            window.codex_context = self.codex_context;
            window.check = connectionCheck;
            self.connections = window;
        }
        self.connections.?.connectCodex(self.model.host_id);
        self.connections.?.present(self.host_window.as(gtk.Window), projection.connections);
    }
    fn connectionCheck(context: ?*anyopaque, id: []const u8, polar: bool) void {
        const self: *Controller = @ptrCast(@alignCast(context.?));
        self.dispatch(.{ .check_connection = .{ .target_id = id, .polar = polar } });
    }
    fn connectionChanged(context: ?*anyopaque, update: control_center_intent.ConnectionUpdate) void {
        const self: *Controller = @ptrCast(@alignCast(context.?));
        self.dispatch(.{ .update_connection = update });
    }
    pub fn connectionResult(self: *Controller, message: ?[]const u8) void {
        if (self.connections) |window| {
            if (self.model.projection()) |p| window.update(p.connections);
            window.result(message);
        }
    }

    pub fn presentSshTargetEditor(self: *Controller) void {
        const editor = if (self.model.projection()) |projection|
            &projection.header.ssh_target_editor
        else
            null;
        self.ssh_target_draft.begin(self.allocator, editor) catch return;
        if (self.ssh_target_window == null) {
            const window = auxiliary.create(self.app);
            window.as(gtk.Window).setIconName(
                "com.polarealm.PolarStation",
            );
            window.as(gtk.Window).setDefaultSize(470, 560);
            window.as(gtk.Window).setHideOnClose(1);
            window.as(gtk.Window).setTransientFor(
                self.host_window.as(gtk.Window),
            );
            const content = gtk.Box.new(.vertical, 12);
            setMargins(content.as(gtk.Widget), 22);
            const title = label("", "title-2");
            content.append(title.as(gtk.Widget));
            const workspace_name = label("", "caption");
            workspace_name.as(gtk.Widget).addCssClass("dim-label");
            content.append(workspace_name.as(gtk.Widget));
            const destination_label = label("", "heading");
            content.append(destination_label.as(gtk.Widget));
            const entry = gtk.Entry.new();
            content.append(entry.as(gtk.Widget));
            const route_toggle = gtk.ToggleButton.new();
            route_toggle.as(gtk.Button).setLabel("Use explicit LAN/WAN route");
            content.append(route_toggle.as(gtk.Widget));
            const lan_address_label = label("LAN address", "caption");
            const lan_address = gtk.Entry.new();
            lan_address.setPlaceholderText("10.0.0.49");
            content.append(lan_address_label.as(gtk.Widget));
            content.append(lan_address.as(gtk.Widget));
            const lan_port_label = label("LAN SSH port", "caption");
            const lan_port = gtk.Entry.new();
            lan_port.setPlaceholderText("22");
            content.append(lan_port_label.as(gtk.Widget));
            content.append(lan_port.as(gtk.Widget));
            const wan_host_label = label("WAN DNS host", "caption");
            const wan_host = gtk.Entry.new();
            wan_host.setPlaceholderText("remote.example");
            content.append(wan_host_label.as(gtk.Widget));
            content.append(wan_host.as(gtk.Widget));
            const wan_port_label = label("WAN SSH port", "caption");
            const wan_port = gtk.Entry.new();
            wan_port.setPlaceholderText("22888");
            content.append(wan_port_label.as(gtk.Widget));
            content.append(wan_port.as(gtk.Widget));
            const doh_primary_label = label("DoH provider 1", "caption");
            const doh_primary = gtk.Entry.new();
            doh_primary.setPlaceholderText("google");
            content.append(doh_primary_label.as(gtk.Widget));
            content.append(doh_primary.as(gtk.Widget));
            const doh_secondary_label = label("DoH provider 2", "caption");
            const doh_secondary = gtk.Entry.new();
            doh_secondary.setPlaceholderText("cloudflare");
            content.append(doh_secondary_label.as(gtk.Widget));
            content.append(doh_secondary.as(gtk.Widget));
            const status_label = label("", "caption");
            status_label.setWrap(1);
            status_label.setSelectable(1);
            content.append(status_label.as(gtk.Widget));
            const error_label = label("", "caption");
            error_label.setSelectable(1);
            error_label.as(gtk.Widget).addCssClass("error");
            error_label.as(gtk.Widget).setVisible(0);
            content.append(error_label.as(gtk.Widget));
            const buttons = gtk.Box.new(.horizontal, 8);
            buttons.as(gtk.Widget).setHalign(.end);
            const cancel = gtk.Button.new();
            const save = gtk.Button.new();
            save.as(gtk.Widget).addCssClass("suggested-action");
            connect(cancel, Controller, sshTargetCancelClicked, self);
            connect(save, Controller, sshTargetSaveClicked, self);
            buttons.append(cancel.as(gtk.Widget));
            buttons.append(save.as(gtk.Widget));
            content.append(buttons.as(gtk.Widget));
            window.setContent(content.as(gtk.Widget));
            self.ssh_target_window = window;
            self.ssh_target_title = title;
            self.ssh_target_workspace = workspace_name;
            self.ssh_target_destination_label = destination_label;
            self.ssh_target_entry = entry;
            self.ssh_target_route_toggle = route_toggle;
            self.ssh_target_lan_address = lan_address;
            self.ssh_target_lan_port = lan_port;
            self.ssh_target_wan_host = wan_host;
            self.ssh_target_wan_port = wan_port;
            self.ssh_target_doh_primary = doh_primary;
            self.ssh_target_doh_secondary = doh_secondary;
            self.ssh_target_error = error_label;
            self.ssh_target_status = status_label;
            self.ssh_target_cancel = cancel;
            self.ssh_target_save = save;
            _ = gtk.Editable.signals.changed.connect(
                entry.as(gtk.Editable),
                *Controller,
                sshTargetChanged,
                self,
                .{},
            );
            _ = gtk.ToggleButton.signals.toggled.connect(
                route_toggle,
                *Controller,
                sshTargetRouteToggled,
                self,
                .{},
            );
            _ = gtk.Editable.signals.changed.connect(
                lan_address.as(gtk.Editable),
                *Controller,
                sshTargetLanAddressChanged,
                self,
                .{},
            );
            _ = gtk.Editable.signals.changed.connect(
                lan_port.as(gtk.Editable),
                *Controller,
                sshTargetLanPortChanged,
                self,
                .{},
            );
            _ = gtk.Editable.signals.changed.connect(
                wan_host.as(gtk.Editable),
                *Controller,
                sshTargetWanHostChanged,
                self,
                .{},
            );
            _ = gtk.Editable.signals.changed.connect(
                wan_port.as(gtk.Editable),
                *Controller,
                sshTargetWanPortChanged,
                self,
                .{},
            );
            _ = gtk.Editable.signals.changed.connect(
                doh_primary.as(gtk.Editable),
                *Controller,
                sshTargetDohPrimaryChanged,
                self,
                .{},
            );
            _ = gtk.Editable.signals.changed.connect(
                doh_secondary.as(gtk.Editable),
                *Controller,
                sshTargetDohSecondaryChanged,
                self,
                .{},
            );
        }
        const recovery = self.sshTargetRecovery();
        setWindowTitle(self.allocator, self.ssh_target_window.?, std.mem.span(recovery.host_label)) catch return;
        setLabel(self.allocator, self.ssh_target_title.?, std.mem.span(recovery.host_label)) catch return;
        setLabel(
            self.allocator,
            self.ssh_target_workspace.?,
            self.ssh_target_draft.target_id orelse "Host configuration unavailable",
        ) catch return;
        setLabel(
            self.allocator,
            self.ssh_target_destination_label.?,
            std.mem.span(recovery.destination_label),
        ) catch return;
        setEntryPlaceholder(
            self.allocator,
            self.ssh_target_entry.?,
            if (editor) |value| value.destination_placeholder else "user@host",
        ) catch return;
        setButtonLabel(
            self.allocator,
            self.ssh_target_cancel.?,
            if (editor) |value| value.cancel_label else "Close",
        ) catch return;
        setButtonLabel(
            self.allocator,
            self.ssh_target_save.?,
            std.mem.span(recovery.save_label),
        ) catch return;
        const destination = self.allocator.dupeZ(
            u8,
            self.ssh_target_draft.destination orelse "",
        ) catch return;
        defer self.allocator.free(destination);
        self.ssh_target_syncing = true;
        self.ssh_target_entry.?.as(gtk.Editable).setText(destination);
        self.ssh_target_route_toggle.?.setActive(@intFromBool(self.ssh_target_draft.route.enabled));
        setEntryText(self.allocator, self.ssh_target_lan_address.?, self.ssh_target_draft.route.lan_address orelse "") catch return;
        setEntryText(self.allocator, self.ssh_target_lan_port.?, self.ssh_target_draft.route.lan_port orelse "22") catch return;
        setEntryText(self.allocator, self.ssh_target_wan_host.?, self.ssh_target_draft.route.wan_host orelse "") catch return;
        setEntryText(self.allocator, self.ssh_target_wan_port.?, self.ssh_target_draft.route.wan_port orelse "22888") catch return;
        setEntryText(self.allocator, self.ssh_target_doh_primary.?, self.ssh_target_draft.route.doh_primary orelse "google") catch return;
        setEntryText(self.allocator, self.ssh_target_doh_secondary.?, self.ssh_target_draft.route.doh_secondary orelse "cloudflare") catch return;
        self.ssh_target_syncing = false;
        self.refreshSshTargetRouteFields();
        self.refreshSshTargetRecovery();
        self.ssh_target_window.?.as(gtk.Window).present();
    }

    fn sshTargetRecovery(self: *Controller) control.RecoveryView {
        return control.polar_station_recovery_view(
            self.model.connection == .connected,
            self.ssh_target_draft.target_id != null,
            self.ssh_target_draft.pending,
        );
    }

    fn refreshSshTargetRouteFields(self: *Controller) void {
        const enabled = if (self.ssh_target_route_toggle) |route_button|
            route_button.getActive() != 0
        else
            false;
        const sensitive: c_int = @intFromBool(enabled);
        if (self.ssh_target_lan_address) |entry| entry.as(gtk.Widget).setSensitive(sensitive);
        if (self.ssh_target_lan_port) |entry| entry.as(gtk.Widget).setSensitive(sensitive);
        if (self.ssh_target_wan_host) |entry| entry.as(gtk.Widget).setSensitive(sensitive);
        if (self.ssh_target_wan_port) |entry| entry.as(gtk.Widget).setSensitive(sensitive);
        if (self.ssh_target_doh_primary) |entry| entry.as(gtk.Widget).setSensitive(sensitive);
        if (self.ssh_target_doh_secondary) |entry| entry.as(gtk.Widget).setSensitive(sensitive);
    }

    fn refreshSshTargetRecovery(self: *Controller) void {
        const recovery = self.sshTargetRecovery();
        if (self.ssh_target_status) |status| status.setText(recovery.status);
        if (self.ssh_target_save) |save| {
            save.setLabel(recovery.save_label);
            save.as(gtk.Widget).setSensitive(@intFromBool(!self.ssh_target_draft.pending));
        }
        if (self.ssh_target_error) |message| {
            setLabel(self.allocator, message, self.model.last_error orelse "") catch {};
            message.as(gtk.Widget).setVisible(@intFromBool(self.model.last_error != null));
        }
    }

    pub fn sshTargetAccepted(self: *Controller) void {
        self.ssh_target_draft.accept(self.allocator);
        self.refreshSshTargetRecovery();
        if (self.ssh_target_window) |window| {
            if (!self.ssh_target_draft.dirty) window.as(gtk.Widget).setVisible(0);
        }
        if (self.ssh_target_save) |save| {
            save.as(gtk.Widget).setSensitive(1);
        }
    }

    pub fn sshTargetRejected(self: *Controller) void {
        self.ssh_target_draft.pending = false;
        self.refreshSshTargetRecovery();
        if (self.ssh_target_error) |error_label| {
            const message = self.model.last_error orelse
                "Polar Station could not save the SSH target.";
            setLabel(self.allocator, error_label, message) catch {};
            error_label.as(gtk.Widget).setVisible(1);
        }
        if (self.ssh_target_save) |save| {
            save.as(gtk.Widget).setSensitive(1);
        }
        if (self.ssh_target_window) |window| {
            window.as(gtk.Window).present();
        }
    }

    fn presentAuxiliary(
        self: *Controller,
        surface: *const control.AuxiliarySurface,
        destination: *?*adw.Window,
    ) void {
        if (destination.* == null) {
            const window = auxiliary.create(self.app);
            const title_text = self.allocator.dupeZ(u8, surface.title) catch return;
            defer self.allocator.free(title_text);
            window.as(gtk.Window).setTitle(title_text);
            window.as(gtk.Window).setIconName("com.polarealm.PolarStation");
            window.as(gtk.Window).setDefaultSize(
                420,
                360,
            );
            window.as(gtk.Window).setHideOnClose(1);
            if (self.host_alive) {
                window.as(gtk.Window).setTransientFor(
                    self.host_window.as(gtk.Window),
                );
            }

            const toolbar = adw.ToolbarView.new();
            const header = adw.HeaderBar.new();
            const title = adw.WindowTitle.new(title_text, "Polar Station");
            header.setTitleWidget(title.as(gtk.Widget));
            toolbar.addTopBar(header.as(gtk.Widget));
            const content = gtk.Box.new(.vertical, 12);
            setMargins(content.as(gtk.Widget), 28);
            const heading = label("", "title-1");
            setLabel(self.allocator, heading, surface.title) catch {};
            const state = label("", "pill");
            setLabel(self.allocator, state, @tagName(surface.state)) catch {};
            const message = label("", "body");
            setLabel(self.allocator, message, surface.message) catch {};
            content.append(heading.as(gtk.Widget));
            content.append(state.as(gtk.Widget));
            content.append(message.as(gtk.Widget));
            toolbar.setContent(content.as(gtk.Widget));
            window.setContent(toolbar.as(gtk.Widget));
            destination.* = window;
        }
        destination.*.?.as(gtk.Window).present();
    }

    fn dispatch(self: *Controller, action: Action) void {
        self.action_fn(self.action_context, self.controller_id, action);
    }

    pub fn presentAcceptedAuxiliary(
        self: *Controller,
        kind: AuxiliaryKind,
    ) void {
        const projection = self.model.projection() orelse return;
        switch (kind) {
            .iphone => self.presentAuxiliary(
                &projection.auxiliary_surfaces.iphone,
                &self.iphone_window,
            ),
        }
    }

    fn makeSessionButtonContext(
        self: *Controller,
        identity: *const control.SessionIdentity,
    ) !*SessionButtonContext {
        const context = try self.allocator.create(SessionButtonContext);
        errdefer self.allocator.destroy(context);
        const execution_target_id = try self.allocator.dupe(
            u8,
            identity.execution_target_id,
        );
        errdefer self.allocator.free(execution_target_id);
        const tmux_session_id = try self.allocator.dupe(
            u8,
            identity.tmux_session_id,
        );
        errdefer self.allocator.free(tmux_session_id);
        context.* = .{
            .allocator = self.allocator,
            .controller = self,
            .execution_target_id = execution_target_id,
            .tmux_session_id = tmux_session_id,
        };
        return context;
    }

    fn makeSessionCreateTargetContext(
        self: *Controller,
        target_id: []const u8,
    ) !*SessionCreateTargetContext {
        const context = try self.allocator.create(SessionCreateTargetContext);
        errdefer self.allocator.destroy(context);
        const owned_target_id = try self.allocator.dupe(u8, target_id);
        errdefer self.allocator.free(owned_target_id);
        context.* = .{
            .allocator = self.allocator,
            .controller = self,
            .target_id = owned_target_id,
        };
        return context;
    }

    fn clearSessionRows(self: *Controller) void {
        for (self.session_popovers.items) |popover| detachPopover(popover);
        self.session_popovers.clearRetainingCapacity();
        clearList(self.sessions_list);
    }

    fn makeTabButtonContext(
        self: *Controller,
        tab: *const control.Tab,
        tab_index: usize,
        tab_count: usize,
    ) !*TabButtonContext {
        const context = try self.allocator.create(TabButtonContext);
        errdefer self.allocator.destroy(context);
        const tab_id = try self.allocator.dupe(u8, tab.id);
        errdefer self.allocator.free(tab_id);
        const delete_confirmation_id = if (tab.delete_action.confirmation) |confirmation|
            try self.allocator.dupe(u8, confirmation.id)
        else
            null;
        errdefer if (delete_confirmation_id) |value|
            self.allocator.free(value);
        context.* = .{
            .allocator = self.allocator,
            .controller = self,
            .tab_id = tab_id,
            .tab_index = tab_index,
            .tab_count = tab_count,
            .pane_count = tab.panes.len,
            .delete_confirmation_id = delete_confirmation_id,
        };
        return context;
    }

    fn makePaneButtonContext(
        self: *Controller,
        tab_id_value: []const u8,
        pane_id_value: []const u8,
    ) !*PaneButtonContext {
        const context = try self.allocator.create(PaneButtonContext);
        errdefer self.allocator.destroy(context);
        const tab_id = try self.allocator.dupe(u8, tab_id_value);
        errdefer self.allocator.free(tab_id);
        const pane_id = try self.allocator.dupe(u8, pane_id_value);
        errdefer self.allocator.free(pane_id);
        context.* = .{
            .allocator = self.allocator,
            .controller = self,
            .tab_id = tab_id,
            .pane_id = pane_id,
        };
        return context;
    }

    fn clearWorkspaceMap(self: *Controller) void {
        self.closePaneSummaryPopover();
        for (self.map_popovers.items) |popover| detachPopover(popover);
        self.map_popovers.clearRetainingCapacity();
        self.pane_buttons.clearRetainingCapacity();
        self.map_bodies.clearRetainingCapacity();
        self.map_action_popovers.clearRetainingCapacity();
        clearBox(self.layout_box);
    }

    fn trackMapActionPopover(self: *Controller, popover: *gtk.Popover) !void {
        try self.map_action_popovers.append(self.allocator, popover);
        _ = gtk.Popover.signals.closed.connect(popover, *Controller, mapActionClosed, self, .{});
    }

    fn mapActionClosed(_: *gtk.Popover, self: *Controller) callconv(.c) void {
        // Destroying an old card may emit closed while it is being replaced.
        if (self.rendering) return;
        if (self.pending_map_render_source != null) return;
        self.pending_map_render_source = glib.idleAdd(mapActionRender, self);
    }

    fn mapActionRender(ud: ?*anyopaque) callconv(.c) c_int {
        const self: *Controller = @ptrCast(@alignCast(ud orelse return 0));
        self.pending_map_render_source = null;
        if (self.active_tab_drag) |tab_id| {
            if (self.active_drag_source != null) return 0;
            self.active_tab_drag = null;
            defer self.allocator.free(tab_id);
            const index = self.pending_tab_move_index;
            self.pending_tab_move_index = null;
            if (index) |to_index| {
                if (self.host_alive) self.dispatch(.{ .reorder_tab = .{ .tab_id = tab_id, .to_index = to_index } });
            }
        }
        if (self.host_alive) self.render();
        return 0;
    }

    fn workspaceClicked(button: *gtk.Button, self: *Controller) callconv(.c) void {
        self.workspace_menu.popdown();
        self.dispatch(.{ .select_workspace = .{
            .workspace_id = std.mem.span(button.as(gtk.Widget).getName()),
        } });
    }

    fn tabClicked(button: *gtk.Button, self: *Controller) callconv(.c) void {
        self.queueTabSelection(std.mem.span(button.as(gtk.Widget).getName()));
    }

    fn tabDragPrepare(source: *gtk.DragSource, _: f64, _: f64, context: *TabButtonContext) callconv(.c) ?*gdk.ContentProvider {
        const self = context.controller;
        if (self.active_pane_drag != null or self.active_tab_drag != null) return null;
        self.active_tab_drag = self.allocator.dupe(u8, context.tab_id) catch return null;
        self.pending_tab_move_index = null;
        self.active_tab_drag_index = context.tab_index;
        self.active_drag_source = source;
        const text = self.allocator.dupeZ(u8, context.tab_id) catch {
            self.allocator.free(self.active_tab_drag.?);
            self.active_tab_drag = null;
            self.active_drag_source = null;
            return null;
        };
        defer self.allocator.free(text);
        var value = gobject.ext.Value.newFrom(text);
        defer value.unset();
        return gdk.ContentProvider.newForValue(&value);
    }

    fn tabDragEnd(_: *gtk.DragSource, _: *gdk.Drag, _: c_int, context: *TabButtonContext) callconv(.c) void {
        context.controller.finishTabDrag(false);
    }

    fn tabDragCancel(_: *gtk.DragSource, _: *gdk.Drag, _: gdk.DragCancelReason, context: *TabButtonContext) callconv(.c) c_int {
        context.controller.finishTabDrag(true);
        return 0;
    }

    fn finishTabDrag(self: *Controller, cancelled: bool) void {
        if (cancelled) self.pending_tab_move_index = null;
        self.active_drag_source = null;
        if (self.pending_map_render_source == null) self.pending_map_render_source = glib.idleAdd(mapActionRender, self);
    }

    fn acceptsTabDrop(context: *TabButtonContext, value: ?*const gobject.Value) bool {
        const id = context.controller.active_tab_drag orelse return false;
        const data = value orelse return false;
        const text = data.getString() orelse return false;
        return !std.mem.eql(u8, id, context.tab_id) and std.mem.eql(u8, id, std.mem.span(text));
    }

    fn paneDragPrepare(
        source: *gtk.DragSource,
        _: f64,
        _: f64,
        context: *PaneButtonContext,
    ) callconv(.c) ?*gdk.ContentProvider {
        const self = context.controller;
        const executor = self.native_executor orelse return null;
        const workspace = executor.nativeWorkspaceSnapshot() orelse return null;
        const tab = workspace.tab(context.tab_id) orelse return null;
        _ = tab.pane(context.pane_id) orelse return null;
        if (self.active_pane_drag != null) return null;
        const payload = std.fmt.allocPrint(
            context.allocator,
            "{s}\n{s}",
            .{ context.tab_id, context.pane_id },
        ) catch return null;
        defer context.allocator.free(payload);
        const payload_z = context.allocator.dupeZ(u8, payload) catch return null;
        defer context.allocator.free(payload_z);
        var value = gobject.ext.Value.newFrom(payload_z);
        defer value.unset();
        self.active_pane_drag = (pane_interaction.PaneDrop{
            .workspace_id = workspace.workspace_id,
            .from_tab_id = context.tab_id,
            .pane_id = context.pane_id,
            .to_tab_id = context.tab_id,
        }).clone(self.allocator) catch return null;
        self.active_drag_source = source;
        self.suppress_pane_click = true;
        self.cancelPendingPaneSummary();
        self.closePaneSummaryPopover();
        self.clearPendingPaneAction();
        log.info("Pane drag prepared", .{});
        return gdk.ContentProvider.newForValue(&value);
    }

    fn panePointerEvent(_: *gtk.EventControllerLegacy, event: *gdk.Event, self: *Controller) callconv(.c) c_int {
        const kind = event.getEventType();
        if (kind == .button_press or kind == .button_release) {
            const button: *gdk.ButtonEvent = @ptrCast(event);
            if (button.getButton() != 1) return 0;
        }
        switch (kind) {
            .button_press, .touch_begin => {
                self.pane_pointer_down = true;
                self.suppress_pane_click = false;
                self.cancelPendingPaneSummary();
            },
            .button_release, .touch_end, .touch_cancel => {
                self.pane_pointer_down = false;
                self.resumePointerRender();
            },
            else => {},
        }
        return 0;
    }

    fn paneDragBegin(_: *gtk.DragSource, _: *gdk.Drag, context: *PaneButtonContext) callconv(.c) void {
        context.controller.suppress_pane_click = true;
        log.info("Pane drag began", .{});
    }

    fn paneDragCancel(_: *gtk.DragSource, _: *gdk.Drag, _: gdk.DragCancelReason, context: *PaneButtonContext) callconv(.c) c_int {
        context.controller.finishPaneDrag(true);
        return 0;
    }

    fn paneDragEnd(_: *gtk.DragSource, _: *gdk.Drag, _: c_int, context: *PaneButtonContext) callconv(.c) void {
        // A move is committed by the native executor once, never deleted again
        // from the DragSource's delete_data notification.
        context.controller.finishPaneDrag(false);
    }

    fn finishPaneDrag(self: *Controller, cancelled: bool) void {
        if (cancelled) self.clearPendingPaneMove();
        if (self.active_pane_drag) |drag| drag.deinit(self.allocator);
        self.active_pane_drag = null;
        self.active_drag_source = null;
        self.pane_pointer_down = false;
        if (self.pending_pane_move != null and self.pending_pane_move_source == null) {
            self.pending_pane_move_source = glib.idleAdd(paneMoveIdle, self);
        }
        // Clear all previews after GTK has left the callback, even if there
        // was no backend refresh during the gesture.
        self.deferred_pointer_render = true;
        self.resumePointerRender();
        log.info("Pane drag ended cancelled={}", .{cancelled});
    }

    fn cancelPaneDrag(self: *Controller) void {
        if (self.active_drag_source) |source| source.dragCancel();
        if (self.active_pane_drag != null) self.finishPaneDrag(true);
        if (self.active_tab_drag) |id| self.allocator.free(id);
        self.active_tab_drag = null;
        self.pending_tab_move_index = null;
        self.pane_pointer_down = false;
        self.clearPendingPaneMove();
    }

    fn resumePointerRender(self: *Controller) void {
        if (self.pane_pointer_down or self.active_pane_drag != null or self.active_tab_drag != null or !self.deferred_pointer_render) return;
        if (self.pending_map_render_source == null) self.pending_map_render_source = glib.idleAdd(mapActionRender, self);
    }

    fn acceptedPaneDrop(
        self: *Controller,
        value: ?*const gobject.Value,
        to_tab_id: []const u8,
        anchor: ?[]const u8,
        placement: pane_interaction.Placement,
    ) ?pane_interaction.PaneDrop {
        var drop = self.active_pane_drag orelse return null;
        const payload = gobject.ext.Value.get(value orelse return null, ?[:0]const u8) orelse return null;
        const separator = std.mem.indexOfScalar(u8, payload, '\n') orelse return null;
        if (!std.mem.eql(u8, payload[0..separator], drop.from_tab_id) or
            !std.mem.eql(u8, payload[separator + 1 ..], drop.pane_id)) return null;
        drop.to_tab_id = to_tab_id;
        drop.anchor_pane_id = anchor;
        drop.placement = placement;
        const executor = self.native_executor orelse return null;
        const current = executor.nativeWorkspaceSnapshot() orelse return null;
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        _ = (pane_interaction.planDrop(arena.allocator(), current, drop) catch return null) orelse return null;
        return drop;
    }

    fn clearDropHighlight(widget: *gtk.Widget) void {
        inline for (.{ "pane-drop-left", "pane-drop-right", "pane-drop-up", "pane-drop-down", "pane-drop-append" }) |name| widget.removeCssClass(name);
    }

    fn paneDropMotion(target: *gtk.DropTarget, x: f64, y: f64, context: *PaneButtonContext) callconv(.c) gdk.DragAction {
        const widget = (context.button orelse return .{}).as(gtk.Widget);
        clearDropHighlight(widget);
        const edge = pane_interaction.dropEdge(x, y, @floatFromInt(widget.getWidth()), @floatFromInt(widget.getHeight())) orelse return .{};
        _ = context.controller.acceptedPaneDrop(target.getValue(), context.tab_id, context.pane_id, edge.placement()) orelse return .{};
        widget.addCssClass(switch (edge) {
            .left => "pane-drop-left",
            .right => "pane-drop-right",
            .up => "pane-drop-up",
            .down => "pane-drop-down",
        });
        return gdk.DragAction.flags_move;
    }

    fn paneDropLeave(_: *gtk.DropTarget, context: *PaneButtonContext) callconv(.c) void {
        if (context.button) |button| clearDropHighlight(button.as(gtk.Widget));
    }

    fn paneDroppedOnPane(_: *gtk.DropTarget, value: *gobject.Value, x: f64, y: f64, context: *PaneButtonContext) callconv(.c) c_int {
        const widget = (context.button orelse return 0).as(gtk.Widget);
        clearDropHighlight(widget);
        const edge = pane_interaction.dropEdge(x, y, @floatFromInt(widget.getWidth()), @floatFromInt(widget.getHeight())) orelse return 0;
        const drop = context.controller.acceptedPaneDrop(value, context.tab_id, context.pane_id, edge.placement()) orelse return 0;
        return @intFromBool(context.controller.queuePaneMove(drop));
    }

    fn tabDropPointIsPane(context: *TabButtonContext, x: f64, y: f64) bool {
        const card = context.card orelse return true;
        var picked = card.pick(x, y, .{});
        while (picked) |widget| {
            if (widget.hasCssClass("polar-station-pane-action") != 0) return true;
            if (widget == card) break;
            picked = widget.getParent();
        }
        return false;
    }

    fn tabDropMotion(target: *gtk.DropTarget, x: f64, y: f64, context: *TabButtonContext) callconv(.c) gdk.DragAction {
        const card = context.card orelse return .{};
        clearDropHighlight(card);
        if (acceptsTabDrop(context, target.getValue())) {
            card.addCssClass(if (y < @as(f64, @floatFromInt(if (card.getFirstChild()) |header| header.getHeight() else layout_policy.tab_header_height)) / 2) "pane-drop-up" else "pane-drop-down");
            return gdk.DragAction.flags_move;
        }
        if (tabDropPointIsPane(context, x, y)) return .{};
        _ = context.controller.acceptedPaneDrop(target.getValue(), context.tab_id, null, .append) orelse return .{};
        card.addCssClass("pane-drop-append");
        return gdk.DragAction.flags_move;
    }

    fn tabDropLeave(_: *gtk.DropTarget, context: *TabButtonContext) callconv(.c) void {
        if (context.card) |card| clearDropHighlight(card);
    }

    fn paneDroppedOnTab(
        _: *gtk.DropTarget,
        value: *gobject.Value,
        x: f64,
        y: f64,
        context: *TabButtonContext,
    ) callconv(.c) c_int {
        if (context.card) |card| clearDropHighlight(card);
        if (acceptsTabDrop(context, value)) {
            const after = if (context.card) |card| y >= @as(f64, @floatFromInt(if (card.getFirstChild()) |header| header.getHeight() else layout_policy.tab_header_height)) / 2 else false;
            const insertion = context.tab_index + @intFromBool(after);
            context.controller.pending_tab_move_index = insertion - @intFromBool(context.controller.active_tab_drag_index < insertion);
            return 1;
        }
        if (tabDropPointIsPane(context, x, y)) return 0;
        const drop = context.controller.acceptedPaneDrop(value, context.tab_id, null, .append) orelse return 0;
        return @intFromBool(context.controller.queuePaneMove(drop));
    }

    fn tabMoveEarlierClicked(
        _: *gtk.Button,
        context: *TabButtonContext,
    ) callconv(.c) void {
        if (context.tab_index == 0) return;
        dispatchTabMove(context, context.tab_index - 1);
    }

    fn tabMoveLaterClicked(
        _: *gtk.Button,
        context: *TabButtonContext,
    ) callconv(.c) void {
        if (context.tab_index + 1 >= context.tab_count) return;
        dispatchTabMove(context, context.tab_index + 1);
    }

    fn dispatchTabMove(context: *TabButtonContext, to_index: usize) void {
        if (context.move_submitted) return;
        const controller = context.controller;
        const allocator = context.allocator;
        const tab_id = allocator.dupe(u8, context.tab_id) catch return;
        defer allocator.free(tab_id);
        const move_popover = context.move_popover;
        const actions_popover = context.actions_popover;
        if (move_popover) |popover| _ = popover.ref();
        defer if (move_popover) |popover| popover.unref();
        if (actions_popover) |popover| _ = popover.ref();
        defer if (actions_popover) |popover| popover.unref();
        context.move_submitted = true;
        // Dismissal and native dispatch can retire the card context.
        if (move_popover) |popover| popover.popdown();
        if (actions_popover) |popover| popover.popdown();
        controller.dispatch(.{ .reorder_tab = .{
            .tab_id = tab_id,
            .to_index = to_index,
        } });
    }

    fn queuePaneAction(
        self: *Controller,
        kind: PendingPaneActionKind,
        pane_id: []const u8,
    ) void {
        if (self.pending_pane_action) |pending| {
            switch (pending) {
                .select => |pending_id| {
                    if (kind == .activate and
                        std.mem.eql(u8, pending_id, pane_id))
                    {
                        self.allocator.free(pending_id);
                        const owned = self.allocator.dupe(u8, pane_id) catch {
                            self.pending_pane_action = null;
                            return;
                        };
                        self.pending_pane_action = .{
                            .select_then_activate = owned,
                        };
                        return;
                    }
                    self.allocator.free(pending_id);
                },
                .activate, .select_then_activate, .remove => |pending_id| {
                    self.allocator.free(pending_id);
                },
            }
            self.pending_pane_action = null;
        }
        const owned = self.allocator.dupe(u8, pane_id) catch return;
        self.pending_pane_action = switch (kind) {
            .select => .{ .select = owned },
            .activate => .{ .activate = owned },
            .remove => .{ .remove = owned },
        };
        if (self.pending_pane_action_source == null) {
            self.pending_pane_action_source = glib.idleAdd(
                paneActionIdle,
                self,
            );
        }
    }

    fn paneActionIdle(ud: ?*anyopaque) callconv(.c) c_int {
        const self: *Controller = @ptrCast(@alignCast(ud orelse return 0));
        self.pending_pane_action_source = null;
        const pending = self.pending_pane_action orelse return 0;
        self.pending_pane_action = null;
        switch (pending) {
            .select => |pane_id| {
                defer self.allocator.free(pane_id);
                self.dispatch(.{ .select_pane = .{ .pane_id = pane_id } });
            },
            .activate => |pane_id| {
                defer self.allocator.free(pane_id);
                self.dispatch(.{ .activate_pane = .{ .pane_id = pane_id } });
            },
            .select_then_activate => |pane_id| {
                defer self.allocator.free(pane_id);
                self.dispatch(.{ .select_pane = .{ .pane_id = pane_id } });
                self.dispatch(.{ .activate_pane = .{ .pane_id = pane_id } });
            },
            .remove => |pane_id| {
                defer self.allocator.free(pane_id);
                if (self.host_alive) self.dispatch(.{ .remove_pane = .{ .pane_id = pane_id } });
            },
        }
        return 0;
    }

    fn clearPendingPaneAction(self: *Controller) void {
        if (self.pending_pane_action_source) |source| {
            _ = glib.Source.remove(source);
            self.pending_pane_action_source = null;
        }
        if (self.pending_pane_action) |pending| {
            switch (pending) {
                .select, .activate, .select_then_activate, .remove => |pane_id| {
                    self.allocator.free(pane_id);
                },
            }
            self.pending_pane_action = null;
        }
    }

    fn schedulePaneSummary(self: *Controller, pane_id: []const u8) void {
        self.cancelPendingPaneSummary();
        self.pending_pane_summary_id = self.allocator.dupe(u8, pane_id) catch null;
        if (self.pending_pane_summary_id == null) return;
        self.pending_pane_summary_source = glib.timeoutAdd(
            pane_interaction.double_click_interval_us / 1000,
            paneSummaryTimer,
            self,
        );
    }

    fn paneSummaryTimer(ud: ?*anyopaque) callconv(.c) c_int {
        const self: *Controller = @ptrCast(@alignCast(ud orelse return 0));
        self.pending_pane_summary_source = null;
        const pane_id = self.pending_pane_summary_id orelse return 0;
        self.pending_pane_summary_id = null;
        defer self.allocator.free(pane_id);
        self.presentPaneSummaryById(pane_id);
        return 0;
    }

    fn cancelPendingPaneSummary(self: *Controller) void {
        if (self.pending_pane_summary_source) |source| {
            _ = glib.Source.remove(source);
            self.pending_pane_summary_source = null;
        }
        if (self.pending_pane_summary_id) |pane_id| {
            self.allocator.free(pane_id);
            self.pending_pane_summary_id = null;
        }
    }

    fn queuePaneMove(
        self: *Controller,
        drop: pane_interaction.PaneDrop,
    ) bool {
        const owned = drop.clone(self.allocator) catch return false;
        self.clearPendingPaneMove();
        self.pending_pane_move = owned;
        if (self.active_pane_drag == null and self.pending_pane_move_source == null) {
            self.pending_pane_move_source = glib.idleAdd(
                paneMoveIdle,
                self,
            );
        }
        log.info("Pane drop queued placement={s}", .{@tagName(drop.placement)});
        return true;
    }

    fn paneMoveIdle(ud: ?*anyopaque) callconv(.c) c_int {
        const self: *Controller = @ptrCast(@alignCast(ud orelse return 0));
        self.pending_pane_move_source = null;
        if (self.active_pane_drag != null) return 0;
        const pending = self.pending_pane_move orelse return 0;
        self.pending_pane_move = null;
        defer pending.deinit(self.allocator);
        self.clearSessionLocate();
        if (self.host_alive) self.pane_drop_fn(self.action_context, self.controller_id, pending);
        return 0;
    }

    fn clearPendingPaneMove(self: *Controller) void {
        if (self.pending_pane_move_source) |source| {
            _ = glib.Source.remove(source);
            self.pending_pane_move_source = null;
        }
        if (self.pending_pane_move) |pending| {
            pending.deinit(self.allocator);
            self.pending_pane_move = null;
        }
    }

    pub fn reportPaneDropError(self: *Controller, err: anyerror) void {
        self.window_title.setSubtitle(switch (err) {
            error.TabFull => "Cannot move Pane: destination Tab is full",
            error.UnknownPane, error.UnknownTab, error.WorkspaceUnavailable => "Cannot move Pane: drop target is no longer available",
            else => "Could not save the Pane move; see application log",
        });
    }

    fn queueTabSelection(self: *Controller, tab_id: []const u8) void {
        if (self.pending_tab_selection_id) |pending| {
            self.allocator.free(pending);
        }
        self.pending_tab_selection_id = self.allocator.dupe(u8, tab_id) catch null;
        if (self.pending_tab_selection_id == null or
            self.pending_tab_selection_source != null) return;
        self.pending_tab_selection_source = glib.idleAdd(
            tabSelectionIdle,
            self,
        );
    }

    fn tabSelectionIdle(ud: ?*anyopaque) callconv(.c) c_int {
        const self: *Controller = @ptrCast(@alignCast(ud orelse return 0));
        self.pending_tab_selection_source = null;
        const tab_id = self.pending_tab_selection_id orelse return 0;
        self.pending_tab_selection_id = null;
        defer self.allocator.free(tab_id);
        self.dispatch(.{ .select_tab = .{ .tab_id = tab_id } });
        return 0;
    }

    fn clearPendingTabSelection(self: *Controller) void {
        if (self.pending_tab_selection_source) |source| {
            _ = glib.Source.remove(source);
            self.pending_tab_selection_source = null;
        }
        if (self.pending_tab_selection_id) |tab_id| {
            self.allocator.free(tab_id);
            self.pending_tab_selection_id = null;
        }
    }

    fn findPaneButton(self: *const Controller, pane_id: []const u8) ?*gtk.Button {
        for (self.pane_buttons.items) |entry| {
            if (std.mem.eql(u8, entry.pane_id, pane_id)) return entry.button;
        }
        return null;
    }

    pub fn prepareTerminalFocus(self: *Controller, preserve_pending_summary: bool) void {
        if (!preserve_pending_summary) self.cancelPendingPaneSummary();
        self.closePaneSummaryPopover();
    }

    fn closePaneSummaryPopover(self: *Controller) void {
        const popover = self.pane_summary_popover orelse return;
        self.pane_summary_popover = null;
        popover.popdown();
        popover.setChild(null);
        popover.as(gtk.Widget).unparent();
    }

    fn paneClicked(
        _: *gtk.Button,
        context: *PaneButtonContext,
    ) callconv(.c) void {
        const controller = context.controller;
        if (controller.suppress_pane_click or controller.active_pane_drag != null) return;
        const now_us = std.time.microTimestamp();
        const is_double = pane_interaction.isDoubleClick(
            if (controller.last_pane_click_id) |value| value else null,
            controller.last_pane_click_at_us,
            context.pane_id,
            now_us,
        );
        if (controller.last_pane_click_id) |value| {
            controller.allocator.free(value);
            controller.last_pane_click_id = null;
        }
        if (is_double) {
            controller.last_pane_click_at_us = 0;
            controller.cancelPendingPaneSummary();
            controller.queuePaneAction(.activate, context.pane_id);
            return;
        }
        controller.last_pane_click_id = controller.allocator.dupe(
            u8,
            context.pane_id,
        ) catch null;
        controller.last_pane_click_at_us = now_us;
        controller.queuePaneAction(.select, context.pane_id);
        controller.schedulePaneSummary(context.pane_id);
    }

    fn presentPaneSummaryById(
        self: *Controller,
        pane_id: []const u8,
    ) void {
        const button = self.findPaneButton(pane_id) orelse return;
        const projection = self.model.projection() orelse return;
        const executor = self.native_executor orelse return;
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const map = (executor.nativeWorkspaceMap(arena.allocator(), projection, self.model.session_cache) catch return) orelse return;
        var pane: ?control.Pane = null;
        for (map.tabs) |tab| for (tab.panes) |candidate| {
            if (std.mem.eql(u8, candidate.id, pane_id)) {
                pane = candidate;
                break;
            }
        };
        const value = pane orelse return;
        self.closePaneSummaryPopover();
        const popover = gtk.Popover.new();
        const box = menuBox();
        const title = ownedLabel(self.allocator, value.session_name, "heading") catch return;
        box.append(title.as(gtk.Widget));
        const source = ownedLabel(
            self.allocator,
            switch (value.target_kind) {
                .local => "Local",
                .ssh => "SSH",
                .wsl => "WSL",
            },
            "caption",
        ) catch return;
        source.as(gtk.Widget).addCssClass("dim-label");
        box.append(source.as(gtk.Widget));
        if (value.attention.summary_sections.len == 0) {
            const empty = label("No summary is available yet.", "caption");
            empty.as(gtk.Widget).addCssClass("dim-label");
            box.append(empty.as(gtk.Widget));
        } else {
            for (value.attention.summary_sections) |section| {
                const line = ownedLabel(self.allocator, section, "body") catch continue;
                line.setWrap(1);
                line.setXalign(0);
                box.append(line.as(gtk.Widget));
            }
        }
        popover.setChild(box.as(gtk.Widget));
        popover.as(gtk.Widget).setParent(button.as(gtk.Widget));
        self.pane_summary_popover = popover;
        popover.popup();
        if (value.attention.summary_revision > 0) {
            self.dispatch(.{ .mark_attention_notification_handled = .{
                .pane_id = value.id,
                .summary_revision = value.attention.summary_revision,
            } });
        }
    }

    fn paneRemoveClicked(
        _: *gtk.Button,
        context: *PaneButtonContext,
    ) callconv(.c) void {
        const controller = context.controller;
        const allocator = context.allocator;
        const pane_id = allocator.dupe(u8, context.pane_id) catch return;
        defer allocator.free(pane_id);
        const popover = context.context_menu orelse return;
        _ = popover.ref();
        defer popover.unref();
        // Dismissal can retire the card context. Structural removal must also
        // wait until GTK has returned from this menu's activation callback.
        controller.cancelPendingPaneSummary();
        popover.popdown();
        controller.queuePaneAction(.remove, pane_id);
    }

    fn paneContextMenuClicked(
        gesture: *gtk.GestureClick,
        _: c_int,
        x: f64,
        y: f64,
        context: *PaneButtonContext,
    ) callconv(.c) void {
        if (gesture.as(gtk.GestureSingle).getCurrentButton() != 3) return;
        const controller = context.controller;
        const popover = context.context_menu orelse return;
        _ = popover.ref();
        defer popover.unref();
        // The click summary and this menu share the same parent. Retire the
        // summary before mapping another grabbing popup on that parent.
        controller.prepareTerminalFocus(false);
        const rect: gdk.Rectangle = .{
            .f_x = @intFromFloat(x),
            .f_y = @intFromFloat(y),
            .f_width = 1,
            .f_height = 1,
        };
        popover.setPointingTo(&rect);
        popover.popup();
    }

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

    fn panePointerEntered(
        _: *gtk.EventControllerMotion,
        _: f64,
        _: f64,
        context: *PaneButtonContext,
    ) callconv(.c) void {
        context.hovered = true;
        context.updateRemoveVisibility();
    }

    fn panePointerLeft(
        _: *gtk.EventControllerMotion,
        context: *PaneButtonContext,
    ) callconv(.c) void {
        context.hovered = false;
        context.updateRemoveVisibility();
    }

    fn paneFocusEntered(
        _: *gtk.EventControllerFocus,
        context: *PaneButtonContext,
    ) callconv(.c) void {
        context.focused = true;
        context.updateRemoveVisibility();
        context.controller.resetLayoutHorizontalPosition();
    }

    fn paneFocusLeft(
        _: *gtk.EventControllerFocus,
        context: *PaneButtonContext,
    ) callconv(.c) void {
        context.focused = false;
        context.updateRemoveVisibility();
    }

    fn tabRenameClicked(
        _: *gtk.Button,
        context: *TabButtonContext,
    ) callconv(.c) void {
        const controller = context.controller;
        const allocator = context.allocator;
        const tab_id = allocator.dupe(u8, context.tab_id) catch return;
        defer allocator.free(tab_id);
        const executor = controller.native_executor orelse return;
        const snapshot = executor.nativeWorkspaceSnapshot() orelse return;
        const tab = snapshot.tab(tab_id) orelse return;
        const title = allocator.dupe(u8, tab.title) catch return;
        defer allocator.free(title);
        const actions_popover = context.actions_popover orelse return;
        _ = actions_popover.ref();
        defer actions_popover.unref();
        // Closing can synchronously retire this card. Nothing below borrows it.
        actions_popover.popdown();
        controller.presentRenameEditor(.{ .tab = tab_id }, "Rename Tab", "Tab title", title) catch {};
    }

    fn tabDeleteCancelClicked(
        _: *gtk.Button,
        context: *TabButtonContext,
    ) callconv(.c) void {
        context.delete_popover.?.popdown();
    }

    fn tabDeleteSubmitClicked(
        button: *gtk.Button,
        context: *TabButtonContext,
    ) callconv(.c) void {
        if (context.delete_submitted) return;
        const confirmation_id =
            context.delete_confirmation_id orelse return;
        const controller = context.controller;
        const allocator = context.allocator;
        const tab_id = allocator.dupe(u8, context.tab_id) catch return;
        defer allocator.free(tab_id);
        const owned_confirmation_id = allocator.dupe(u8, confirmation_id) catch return;
        defer allocator.free(owned_confirmation_id);
        const delete_popover = context.delete_popover orelse return;
        const actions_popover = context.actions_popover orelse return;
        // Closing a popover can emit signals; native dispatch can synchronously
        // rebuild the card and free its callback context. Retain the widgets
        // and own the payload before either operation, then never use context.
        _ = delete_popover.ref();
        defer delete_popover.unref();
        _ = actions_popover.ref();
        defer actions_popover.unref();
        context.delete_submitted = true;
        button.as(gtk.Widget).setSensitive(0);
        delete_popover.popdown();
        actions_popover.popdown();
        controller.dispatch(.{ .delete_tab = .{
            .tab_id = tab_id,
            .confirmation_id = owned_confirmation_id,
        } });
    }

    fn sessionAssignmentKeyPressed(
        _: *gtk.EventControllerKey,
        keyval: c_uint,
        _: c_uint,
        mods: gdk.ModifierType,
        context: *SessionButtonContext,
    ) callconv(.c) c_int {
        if (keyval == gdk.KEY_Menu or (keyval == gdk.KEY_F10 and mods.shift_mask)) {
            if (context.context_menu) |popover| popover.popup();
            return 1;
        }
        if (keyval != gdk.KEY_Return and
            keyval != gdk.KEY_KP_Enter and
            keyval != gdk.KEY_space) return 0;
        return @intFromBool(dispatchNativeSessionAssignment(context));
    }

    fn dispatchNativeSessionAssignment(
        context: *SessionButtonContext,
    ) bool {
        const assignment = context.controller.nativeSessionAssignment(
            context.execution_target_id,
            context.tmux_session_id,
        ) orelse {
            context.controller.dispatch(.{ .toggle_session_assignment = .{
                .execution_target_id = context.execution_target_id,
                .tmux_session_id = context.tmux_session_id,
            } });
            return true;
        };
        switch (assignment) {
            .available => |target| {
                context.controller.clearSessionLocate();
                context.controller.dispatch(.{ .toggle_session_assignment = .{
                    .tab_id = target.tab_id,
                    .execution_target_id = context.execution_target_id,
                    .tmux_session_id = context.tmux_session_id,
                } });
                return true;
            },
            .assigned_here => |target| {
                const pane_id = context.controller.allocator.dupe(u8, target.pane_id) catch return false;
                defer context.controller.allocator.free(pane_id);
                context.controller.dispatch(.{ .remove_pane = .{ .pane_id = pane_id } });
                return true;
            },
            .assigned_elsewhere => {
                const tab_id = context.controller.nativeSelectedTabID() orelse return false;
                context.controller.dispatch(.{ .toggle_session_assignment = .{
                    .tab_id = tab_id,
                    .execution_target_id = context.execution_target_id,
                    .tmux_session_id = context.tmux_session_id,
                } });
                return true;
            },
            .tab_full, .no_managed_tab => return false,
        }
    }

    fn sessionRemoveViewClicked(_: *gtk.Button, context: *SessionButtonContext) callconv(.c) void {
        const controller = context.controller;
        const assignment = controller.nativeSessionAssignment(context.execution_target_id, context.tmux_session_id) orelse return;
        if (assignment != .assigned_here) return;
        // Closing the popover and reconciling native selection may retire borrowed rows.
        const pane_id = controller.allocator.dupe(u8, assignment.assigned_here.pane_id) catch return;
        defer controller.allocator.free(pane_id);
        if (context.context_menu) |menu| menu.popdown();
        controller.dispatch(.{ .remove_pane = .{ .pane_id = pane_id } });
    }

    fn sessionAssignmentClicked(
        _: *gtk.Button,
        context: *SessionButtonContext,
    ) callconv(.c) void {
        if (context.controller.sidebar_selecting) {
            context.controller.sidebarPick(.{ .execution_target_id = context.execution_target_id, .tmux_session_id = context.tmux_session_id }); return;
        }
        _ = dispatchNativeSessionAssignment(context);
    }

    fn sessionContextMenuClicked(
        gesture: *gtk.GestureClick,
        _: c_int,
        _: f64,
        _: f64,
        context: *SessionButtonContext,
    ) callconv(.c) void {
        if (gesture.as(gtk.GestureSingle).getCurrentButton() != 3) return;
        const popover = context.context_menu orelse return;
        popover.popup();
    }

    fn sessionCodexClicked(_: *gtk.Button, context: *SessionButtonContext) callconv(.c) void {
        if (context.context_menu) |popover| popover.popdown();
        context.controller.dispatch(.{ .open_codex = .{
            .execution_target_id = context.execution_target_id,
            .tmux_session_id = context.tmux_session_id,
        } });
    }

    fn sessionReplacementCancelClicked(_: *gtk.Button, context: *SessionButtonContext) callconv(.c) void {
        if (context.replacement_popover) |popover| popover.popdown();
    }

    fn sessionReplacementSubmitClicked(_: *gtk.Button, context: *SessionButtonContext) callconv(.c) void {
        const json = context.replacement_json orelse return;
        const parsed = std.json.parseFromSlice(control_center_intent.SessionReplacementSelection, context.allocator, json, .{}) catch return;
        defer parsed.deinit();
        // Dispatch may rebuild and free the row: close before using the context.
        if (context.replacement_popover) |popover| popover.popdown();
        if (context.context_menu) |popover| popover.popdown();
        context.controller.dispatch(.{ .use_existing_session = parsed.value });
    }

    fn sessionReplacementShowClicked(_: *gtk.Button, context: *SessionButtonContext) callconv(.c) void {
        const json = context.replacement_json orelse return;
        const parsed = std.json.parseFromSlice(control_center_intent.SessionReplacementSelection, context.allocator, json, .{}) catch return;
        defer parsed.deinit();
        const executor = context.controller.native_executor orelse return;
        const workspace = executor.nativeWorkspaceSnapshot() orelse return;
        for (workspace.tabs) |tab| for (tab.panes) |pane| {
            if (std.mem.eql(u8, pane.execution_target_id, parsed.value.replacement.execution_target_id) and std.mem.eql(u8, pane.tmux_session_id, parsed.value.replacement.tmux_session_id)) {
                const allocator = context.allocator;
                const pane_id = allocator.dupe(u8, pane.id) catch return;
                defer allocator.free(pane_id);
                if (context.context_menu) |popover| popover.popdown();
                context.controller.dispatch(.{ .activate_pane = .{ .pane_id = pane_id } });
                return;
            }
        };
    }

    fn sessionRestartClicked(_: *gtk.Button, context: *SessionButtonContext) callconv(.c) void {
        const projection = context.controller.model.projection() orelse return;
        for (projection.sessions.rows) |row| {
            if (!std.mem.eql(u8, row.identity.execution_target_id, context.execution_target_id) or
                !std.mem.eql(u8, row.identity.tmux_session_id, context.tmux_session_id)) continue;
            const action = row.restart_action orelse return;
            if (!action.enabled) return;
            if (context.context_menu) |popover| popover.popdown();
            context.controller.dispatch(.{ .invoke_action = .{ .action_id = action.id } });
            return;
        }
    }

    fn sessionRenameClicked(
        _: *gtk.Button,
        context: *SessionButtonContext,
    ) callconv(.c) void {
        const controller = context.controller;
        if (controller.session_end) |window| if (window.pending) {
            window.native.as(gtk.Window).present();
            return;
        };
        const projection = controller.model.projection() orelse return;
        for (projection.sessions.rows) |row| {
            if (!std.mem.eql(u8, row.identity.execution_target_id, context.execution_target_id) or
                !std.mem.eql(u8, row.identity.tmux_session_id, context.tmux_session_id)) continue;
            const allocator = controller.allocator;
            const target = (RenameTarget{ .session = row.identity }).clone(allocator) catch return;
            defer target.deinit(allocator);
            const name = allocator.dupe(u8, row.display_name) catch return;
            defer allocator.free(name);
            const context_menu = context.context_menu orelse return;
            _ = context_menu.ref();
            defer context_menu.unref();
            // The Session row can be replaced as soon as its menu closes.
            context_menu.popdown();
            controller.presentRenameEditor(target, "Rename Session", "Session name", name) catch {};
            return;
        }
    }

    fn sessionKillClicked(_: *gtk.Button, context: *SessionButtonContext) callconv(.c) void {
        const controller = context.controller;
        if (controller.session_end) |window| if (window.pending) {
            window.native.as(gtk.Window).present();
            return;
        };
        const projection = controller.model.projection() orelse return;
        for (projection.sessions.rows) |row| {
            if (!std.mem.eql(u8, row.identity.execution_target_id, context.execution_target_id) or
                !std.mem.eql(u8, row.identity.tmux_session_id, context.tmux_session_id)) continue;
            const confirmation = row.kill_action.confirmation orelse return;
            if (!row.kill_action.enabled) return;
            if (controller.session_end == null) {
                controller.session_end = session_end_window.Window.create(controller.allocator, controller.app, controller.host_window.as(gtk.Window), controller, sessionEndSubmit, sessionEndReview, sessionEndClose) catch return;
            }
            // Copy the complete draft before closing a menu can retire its row.
            controller.session_end.?.present(.{
                .execution_target_id = row.identity.execution_target_id,
                .tmux_session_id = row.identity.tmux_session_id,
                .confirmation_id = confirmation.id,
            }, confirmation.message) catch return;
            const window = controller.session_end.?;
            window.submit.as(gtk.Widget).setSensitive(0);
            window.status.setLabel("Checking the original Session…");
            if (context.context_menu) |popover| popover.popdown();
            sessionEndReview(controller, window.request.?.value);
            return;
        }
    }

    fn sessionEndReview(data: ?*anyopaque, request: session_end_window.Request) void {
        const self: *Controller = @ptrCast(@alignCast(data orelse return));
        self.endInput(.{ .type = "open_end", .request = .{
            .type = if (request.thread_id != null) "preview_end_agent" else "preview_end",
            .execution_target_id = request.execution_target_id,
            .tmux_session_id = if (request.thread_id == null) request.tmux_session_id else null,
            .owner_host_id = request.owner_host_id, .endpoint_id = request.endpoint_id, .thread_id = request.thread_id,
        } });
    }
    fn sessionEndClose(data: ?*anyopaque) void {
        const self: *Controller = @ptrCast(@alignCast(data orelse return));
        self.endInput(.{ .type = "close" });
    }
    fn sessionEndSubmit(data: ?*anyopaque, _: session_end_window.Request) void {
        const self: *Controller = @ptrCast(@alignCast(data orelse return));
        self.endInput(.{ .type = "submit" });
    }
    pub fn sessionEndResult(self: *Controller, token: u64, failure: ?[]const u8) void {
        self.endInput(.{ .type = "submitted", .token = token, .@"error" = failure, .completed = @as([]const []const u8, &.{}) });
    }
    fn endInput(self: *Controller, input: anytype) void {
        var output = self.end_engine.dispatch(self.allocator, input) catch return;
        defer output.deinit();
        if (self.session_end) |window| window.apply(output.value.view);
        for (output.value.effects) |effect| {
            if ((std.mem.eql(u8, effect.type, "read") or std.mem.eql(u8, effect.type, "save_preference"))) {
                self.dispatch(.{ .session_editor = .{ .generation = effect.token, .request = effect.request orelse continue, .ending = true } });
            } else if (std.mem.eql(u8, effect.type, "cancel")) {
                self.dispatch(.{ .cancel_session_editor = .{ .token = effect.token, .ending = true } });
            } else if (std.mem.eql(u8, effect.type, "submit")) {
                self.end_submission = effect.token;
                const operation = effect.operation orelse continue;
                const kind = (operation.object.get("intent") orelse continue).string;
                inline for (.{ .kill_session, .end_agent_session }) |tag| {
                    if (std.mem.eql(u8, kind, @tagName(tag))) {
                        const value = std.json.parseFromValue(std.meta.TagPayload(Action, tag), self.allocator, operation, .{ .ignore_unknown_fields = true }) catch return;
                        defer value.deinit();
                        self.dispatch(@unionInit(Action, @tagName(tag), value.value));
                    }
                }
            }
        }
    }

    fn addTabClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        const projection = self.model.projection() orelse return;
        if (!projection.header.add_tab.enabled) return;
        const title = std.fmt.allocPrint(
            self.allocator,
            "Tab {d}",
            .{
                if (self.native_executor) |executor|
                    executor.nativeTabCount() + 1
                else
                    1,
            },
        ) catch return;
        defer self.allocator.free(title);
        self.dispatch(.{ .add_tab = .{ .title = title } });
    }

    fn renameWorkspaceClicked(
        _: *gtk.Button,
        self: *Controller,
    ) callconv(.c) void {
        self.presentRenameWorkspaceEditor();
    }

    fn sshTargetClicked(
        _: *gtk.Button,
        self: *Controller,
    ) callconv(.c) void {
        self.presentConnections();
    }

    fn sshTargetChanged(entry: *gtk.Editable, self: *Controller) callconv(.c) void {
        if (self.ssh_target_syncing) return;
        self.ssh_target_draft.edit(self.allocator, std.mem.span(entry.getText())) catch return;
    }

    fn sshTargetRouteToggled(button: *gtk.ToggleButton, self: *Controller) callconv(.c) void {
        if (self.ssh_target_syncing) return;
        self.ssh_target_draft.setRouteEnabled(button.getActive() != 0);
        self.refreshSshTargetRouteFields();
    }

    fn sshTargetLanAddressChanged(entry: *gtk.Editable, self: *Controller) callconv(.c) void {
        if (self.ssh_target_syncing) return;
        self.ssh_target_draft.editRouteField(self.allocator, .lan_address, std.mem.span(entry.getText())) catch return;
    }

    fn sshTargetLanPortChanged(entry: *gtk.Editable, self: *Controller) callconv(.c) void {
        if (self.ssh_target_syncing) return;
        self.ssh_target_draft.editRouteField(self.allocator, .lan_port, std.mem.span(entry.getText())) catch return;
    }

    fn sshTargetWanHostChanged(entry: *gtk.Editable, self: *Controller) callconv(.c) void {
        if (self.ssh_target_syncing) return;
        self.ssh_target_draft.editRouteField(self.allocator, .wan_host, std.mem.span(entry.getText())) catch return;
    }

    fn sshTargetWanPortChanged(entry: *gtk.Editable, self: *Controller) callconv(.c) void {
        if (self.ssh_target_syncing) return;
        self.ssh_target_draft.editRouteField(self.allocator, .wan_port, std.mem.span(entry.getText())) catch return;
    }

    fn sshTargetDohPrimaryChanged(entry: *gtk.Editable, self: *Controller) callconv(.c) void {
        if (self.ssh_target_syncing) return;
        self.ssh_target_draft.editRouteField(self.allocator, .doh_primary, std.mem.span(entry.getText())) catch return;
    }

    fn sshTargetDohSecondaryChanged(entry: *gtk.Editable, self: *Controller) callconv(.c) void {
        if (self.ssh_target_syncing) return;
        self.ssh_target_draft.editRouteField(self.allocator, .doh_secondary, std.mem.span(entry.getText())) catch return;
    }

    fn sshTargetCancelClicked(
        _: *gtk.Button,
        self: *Controller,
    ) callconv(.c) void {
        self.ssh_target_window.?.as(gtk.Widget).setVisible(0);
    }

    fn sshTargetSaveClicked(
        _: *gtk.Button,
        self: *Controller,
    ) callconv(.c) void {
        const entry = self.ssh_target_entry orelse return;
        if (!self.sshTargetRecovery().can_submit) {
            self.refreshSshTargetRecovery();
            return;
        }
        const destination = std.mem.trim(
            u8,
            std.mem.span(entry.as(gtk.Editable).getText()),
            " \t\r\n",
        );
        if (destination.len == 0) return;
        self.ssh_target_draft.edit(self.allocator, destination) catch return;
        self.ssh_target_draft.submit(self.allocator) catch return;
        const route_profile = self.ssh_target_draft.routeProfile();
        self.ssh_target_error.?.as(gtk.Widget).setVisible(0);
        self.ssh_target_save.?.as(gtk.Widget).setSensitive(0);
        self.dispatch(.{ .set_workspace_ssh_target = .{
            .target_id = self.ssh_target_draft.target_id.?,
            .target_revision = self.ssh_target_draft.target_revision,
            .destination = destination,
            .route_profile = route_profile,
        } });
    }

    fn clearRenameTarget(self: *Controller) void {
        if (self.rename_target) |target| target.deinit(self.allocator);
        self.rename_target = null;
    }

    fn cancelRenameEditor(self: *Controller) void {
        if (self.rename_pending) return;
        if (self.rename_window) |window| window.as(gtk.Widget).setVisible(0);
        self.clearRenameTarget();
    }

    fn renameCancelClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.cancelRenameEditor();
    }

    fn renameKeyPressed(
        _: *gtk.EventControllerKey,
        keyval: c_uint,
        _: c_uint,
        _: gdk.ModifierType,
        self: *Controller,
    ) callconv(.c) c_int {
        if (keyval != gdk.KEY_Escape) return 0;
        self.cancelRenameEditor();
        return 1;
    }

    fn renameCloseRequested(_: *gtk.Window, self: *Controller) callconv(.c) c_int {
        self.cancelRenameEditor();
        return 1;
    }

    fn renameChanged(entry: *gtk.Editable, self: *Controller) callconv(.c) void {
        const name = std.mem.trim(u8, std.mem.span(entry.getText()), " \t\r\n");
        if (self.rename_submit) |submit| submit.as(gtk.Widget).setSensitive(
            @intFromBool(self.rename_target != null and !self.rename_pending and name.len > 0),
        );
    }

    fn renameSubmitClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        if (self.rename_pending) return;
        const target = self.rename_target orelse return;
        const entry = self.rename_entry orelse return;
        const name = std.mem.trim(u8, std.mem.span(entry.as(gtk.Editable).getText()), " \t\r\n");
        if (name.len == 0) return;
        const owned_name = self.allocator.dupe(u8, name) catch return;
        defer self.allocator.free(owned_name);
        // A synchronous accepted callback can clear the editor's target.
        const owned_target = target.clone(self.allocator) catch return;
        defer owned_target.deinit(self.allocator);
        self.rename_pending = true;
        self.rename_submit.?.as(gtk.Widget).setSensitive(0);
        self.rename_entry.?.as(gtk.Widget).setSensitive(0);
        self.rename_cancel.?.as(gtk.Widget).setSensitive(0);
        self.rename_error.?.as(gtk.Widget).setVisible(0);
        self.dispatch(switch (owned_target) {
            .agent => |agent| .{ .rename_agent_session = .{ .owner_host_id = agent.owner_host_id, .endpoint_id = agent.endpoint_id, .thread_id = agent.thread_id, .name = owned_name } },
            .tab => |tab_id| .{ .rename_tab = .{ .tab_id = tab_id, .title = owned_name } },
            .workspace => .{ .rename_workspace = .{ .name = owned_name } },
            .session => |identity| .{ .rename_session = .{
                .execution_target_id = identity.execution_target_id,
                .tmux_session_id = identity.tmux_session_id,
                .name = owned_name,
            } },
        });
    }

    pub fn renameAccepted(self: *Controller) void {
        if (!self.rename_pending) return;
        self.rename_pending = false;
        if (self.rename_window) |window| window.as(gtk.Widget).setVisible(0);
        self.clearRenameTarget();
    }

    pub fn renameRejected(self: *Controller, message: ?[]const u8) void {
        if (!self.rename_pending) return;
        self.rename_pending = false;
        setLabel(self.allocator, self.rename_error.?, message orelse self.model.last_error orelse "Rename could not be saved") catch {};
        self.rename_error.?.as(gtk.Widget).setVisible(1);
        self.rename_entry.?.as(gtk.Widget).setSensitive(1);
        self.rename_cancel.?.as(gtk.Widget).setSensitive(1);
        renameChanged(self.rename_entry.?.as(gtk.Editable), self);
        _ = self.rename_entry.?.as(gtk.Widget).grabFocus();
    }

    fn newLocalShellClicked(
        _: *gtk.Button,
        self: *Controller,
    ) callconv(.c) void {
        const projection = self.model.projection() orelse return;
        const source = if (self.native_executor) |executor| source: {
            const target = executor.quickCreateTarget(self.host_window) catch break :source null;
            break :source target.source;
        } else null;
        self.presentSessionCreateEditor(.local_shell, if (source) |value| value.execution_target_id else projection.sessions.local_execution_target_id);
    }

    fn newLocalCommandClicked(
        _: *gtk.Button,
        self: *Controller,
    ) callconv(.c) void {
        const projection = self.model.projection() orelse return;
        self.presentSessionCreateEditor(
            .local_command,
            projection.sessions.local_execution_target_id,
        );
    }

    fn newRemoteSessionClicked(
        _: *gtk.Button,
        context: *SessionCreateTargetContext,
    ) callconv(.c) void {
        context.controller.presentSessionCreateEditor(
            .remote,
            context.target_id,
        );
    }

    pub fn sessionCreateCancelClicked(
        _: *gtk.Button,
        self: *Controller,
    ) callconv(.c) void {
        if (self.session_create_pending) return;
        self.closeSharedEditor();
        self.creation_generation += 1;
        session_editor_window.saveSize(self);
        self.session_create_window.?.as(gtk.Widget).setVisible(0);
        if (self.session_directory) |window| window.native.as(gtk.Widget).setVisible(0);
    }

    pub fn sessionCreateSubmitClicked(
        _: *gtk.Button,
        self: *Controller,
    ) callconv(.c) void {
        self.submitSessionCreate();
    }

    fn submitSessionCreate(self: *Controller) void {
        self.syncSharedEditor();
        self.editorInput(.{ .type = "submit" });
    }

    pub fn sessionCreateAccepted(self: *Controller) void {
        self.editorInput(.{ .type = "submitted", .token = self.editor_submission, .@"error" = @as(?[]const u8, null), .completed = @as([]const []const u8, &.{}) });
        self.closeSharedEditor();
        session_editor_window.saveSize(self);
        if (self.session_create_window) |window| window.as(gtk.Widget).setVisible(0);
    }

    pub fn sessionCreationRecovery(self: *Controller, payload: []const u8) void {
        const parsed = std.json.parseFromSlice(std.json.Value, self.allocator, payload, .{}) catch return;
        defer parsed.deinit();
        const response = parsed.value.object.get("response") orelse return;
        const result = response.object.get("result") orelse return;
        const failure = result.object.get("error") orelse return;
        const raw = failure.object.get("resume") orelse return;
        if (raw != .object) return;
        const thread = raw.object.get("thread_id") orelse return;
        if (thread != .string) return;
        if (self.session_create_window == null) return;
        self.session_create_mode.?.setSelected(1);
        setEntryText(self.allocator, self.session_create_resume_id.?, thread.string) catch {};
    }

    pub fn sessionBatchCompleted(self: *Controller, items: anytype) void {
        for (items) |item| {
            if (item.session == null) continue;
            var row_index: c_int = 0;
            while (self.session_history_rows.?.getRowAtIndex(row_index)) |row| : (row_index += 1) {
                if (std.mem.eql(u8, std.mem.span(row.as(gtk.Widget).getName()), item.thread_id)) {
                    if (row.getChild()) |content| if (content.getFirstChild()) |child| {
                        if (gobject.ext.cast(gtk.CheckButton, child)) |checkbox| checkbox.setActive(0);
                    };
                    break;
                }
            }
        }

    }

    pub fn sessionCreateRejected(self: *Controller, message: ?[]const u8) void {
        self.editorInput(.{ .type = "submitted", .token = self.editor_submission, .@"error" = message orelse self.model.last_error orelse "Creation result is unknown. Refresh Sessions before retrying.", .completed = @as([]const []const u8, &.{}) });
    }

    fn setSessionCreateEditing(self: *Controller, enabled: bool) void {
        if (self.session_create_browse) |button| button.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.session_create_yolo) |yolo_toggle| yolo_toggle.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.session_create_endpoint) |picker| picker.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.session_create_machine) |picker| picker.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.session_create_type) |picker| picker.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.session_create_mode_bar) |bar| bar.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.session_history_box) |box| box.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.session_create_mode) |picker| picker.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.session_create_local_cli) |button| button.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        inline for (.{ "session_create_name", "session_create_directory", "session_create_command", "session_create_resume_id" }) |field| {
            if (@field(self, field)) |entry| entry.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        }
        if (self.editor.new_split) |split| split.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.editor.command_view) |view| view.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.session_create_cancel) |button| button.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if (self.session_create_auto_restart) |restart_toggle| restart_toggle.as(gtk.Widget).setSensitive(@intFromBool(enabled));
    }

    const EditorMeta = struct { kind: []const u8, generation: u64 };
    const EditorReply = struct {
        type: []const u8,
        response: struct { result: struct { result: struct {
            type: []const u8,
            directory: ?session_directory_window.Directory = null,
            notice: ?[]const u8 = null,
            preview: ?struct { session: ?control.SessionIdentity = null, confirmation: control.Confirmation } = null,
        } } },
    };

    pub fn sessionEditorResult(self: *Controller, meta_json: []const u8, payload: []const u8) void {
        const meta = std.json.parseFromSlice(EditorMeta, self.allocator, meta_json, .{}) catch return;
        defer meta.deinit();
        if (std.mem.eql(u8, meta.value.kind, "shared_editor") or std.mem.eql(u8, meta.value.kind, "shared_end")) {
            const raw = std.json.parseFromSlice(std.json.Value, self.allocator, payload, .{}) catch return;
            defer raw.deinit();
            const response = raw.value.object.get("response") orelse return;
            const result = response.object.get("result") orelse return;
            const input = .{ .type = "complete", .token = meta.value.generation, .result = result.object.get("result") orelse std.json.Value.null, .@"error" = @as(?[]const u8, null) };
            if (std.mem.eql(u8, meta.value.kind, "shared_end")) self.endInput(input) else self.editorInput(input);
            return;
        }
        if (meta.value.generation != self.creation_generation) return;
        const parsed = std.json.parseFromSlice(EditorReply, self.allocator, payload, .{ .ignore_unknown_fields = true }) catch {
            self.sessionEditorFailure(meta_json, "The Host returned an invalid editor response.");
            return;
        };
        defer parsed.deinit();
        const result = parsed.value.response.result.result;
        if (result.directory) |directory| {
            if (self.session_directory) |window| window.apply(directory) catch {};
        }
    }

    pub fn sessionEditorFailure(self: *Controller, meta_json: []const u8, message: []const u8) void {
        const meta = std.json.parseFromSlice(EditorMeta, self.allocator, meta_json, .{}) catch return;
        defer meta.deinit();
        if (std.mem.eql(u8, meta.value.kind, "shared_editor") or std.mem.eql(u8, meta.value.kind, "shared_end")) {
            const input = .{ .type = "complete", .token = meta.value.generation, .result = @as(?u8, null), .@"error" = message };
            if (std.mem.eql(u8, meta.value.kind, "shared_end")) self.endInput(input) else self.editorInput(input);
            return;
        }
        if (meta.value.generation != self.creation_generation) return;
        if (std.mem.eql(u8, meta.value.kind, "browse_directory")) {
            if (self.session_directory) |window| window.failed(message);
        } else {
            self.session_context_pending = false;
            sessionCreateChanged(self.session_create_name.?.as(gtk.Editable), self);
            if (self.session_create_error) |label_widget| setLabel(self.allocator, label_widget, message) catch {};
        }
    }

    pub fn sessionNewModeClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.session_create_mode.?.setSelected(0);
    }
    pub fn sessionResumeModeClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.session_create_mode.?.setSelected(1);
    }
    pub fn sessionManageConnectionsClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.presentConnections();
        if (self.connections) |window| {
            window.openCodexFor(self.session_create_target_id orelse "local");
            if (window.window) |native| native.as(gtk.Window).setTransientFor(self.session_create_window.?.as(gtk.Window));
        }
    }
    pub fn historyFilterChanged(_: *gtk.CheckButton, self: *Controller) callconv(.c) void {
        if (self.editor_applying) return;
        const count: u8 = @as(u8, @intFromBool(self.session_history_archived.?.getActive() != 0)) + @as(u8, @intFromBool(self.session_history_subagents.?.getActive() != 0));
        self.editor.filters.?.setLabel(if (count == 0) "Filters" else if (count == 1) "Filters (1)" else "Filters (2)");
        self.requestSessionHistory(true);
    }
    pub fn historySearchChanged(_: *gtk.Editable, self: *Controller) callconv(.c) void { self.syncSharedEditor(); }
    pub fn startLocalCodexClicked(_: *gtk.Button,self:*Controller) callconv(.c) void {self.editorInput(.{.type="start_local_server"});}
    pub fn historyMoreClicked(_: *gtk.Button, self: *Controller) callconv(.c) void { self.editorInput(.{ .type = "retry_history" }); }
    pub fn historyDetailRetryClicked(_: *gtk.Button, self: *Controller) callconv(.c) void { self.editorInput(.{ .type = "retry_detail" }); }
    pub fn historyCopyClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        const entry = self.session_create_resume_id orelse return;
        entry.as(gtk.Widget).getClipboard().setText(entry.as(gtk.Editable).getText());
    }
    pub fn historyRowSelected(_: *gtk.ListBox, row: ?*gtk.ListBoxRow, self: *Controller) callconv(.c) void {
        if (self.editor_applying) return;
        const selected = row orelse return;
        self.editorInput(.{ .type = "select", .id = std.mem.span(selected.as(gtk.Widget).getName()), .checked = @as(?bool, null) });
    }

    pub fn historyRowActivated(_: *gtk.ListBox, row: *gtk.ListBoxRow, self: *Controller) callconv(.c) void {
        self.editorInput(.{.type="activate",.id=std.mem.span(row.as(gtk.Widget).getName())});
    }
    pub fn historyViewportChanged(adjustment: *gtk.Adjustment, self: *Controller) callconv(.c) void {
        if (self.editor_applying) return;
        if (adjustment.getUpper() - adjustment.getValue() - adjustment.getPageSize() <= 160) self.editorInput(.{ .type = "more" });
    }

    pub fn historyAlreadyAssigned(self: *Controller, thread: []const u8) bool {
        const projection = self.model.projection() orelse return false;
        const executor = self.native_executor orelse return false;
        const snapshot = executor.nativeWorkspaceSnapshot() orelse return false;
        const tab = snapshot.tab(self.session_create_tab_id orelse return false) orelse return false;
        const endpoint = self.selectedCreateEndpoint() orelse return false;
        for (projection.agent_sessions) |agent| {
            if (!std.mem.eql(u8, agent.thread_id, thread) or !std.mem.eql(u8, agent.endpoint_id, endpoint)) continue;
            for (agent.terminals) |identity| for (tab.panes) |pane| {
                if (std.mem.eql(u8, identity.execution_target_id, pane.execution_target_id) and std.mem.eql(u8, identity.tmux_session_id, pane.tmux_session_id)) return true;
            };
        }
        return false;
    }
    pub fn historySelectionChanged(selection_toggle: *gtk.CheckButton, self: *Controller) callconv(.c) void {
        if (self.editor_applying) return;
        const row_widget = selection_toggle.as(gtk.Widget).getParent().?.getParent().?;
        self.editorInput(.{ .type = "select", .id = std.mem.span(row_widget.getName()), .checked = selection_toggle.getActive() != 0 });
    }
    pub fn historyManualChanged(_: *gtk.Editable, self: *Controller) callconv(.c) void {
        self.editor.manual_apply.?.as(gtk.Widget).setSensitive(@intFromBool(std.mem.trim(u8, std.mem.span(self.editor.manual_draft.?.as(gtk.Editable).getText()), " \t\r\n").len > 0));
    }
    pub fn historyManualApply(_: *gtk.Button, self: *Controller) callconv(.c) void {
        if (self.session_create_pending) return;
        const id = std.mem.trim(u8, std.mem.span(self.editor.manual_draft.?.as(gtk.Editable).getText()), " \t\r\n");
        if (id.len == 0) return;
        self.editorInput(.{ .type = "manual", .id = id });
        self.editor.manual_popover.?.popdown();
    }
    pub fn historyManualCancel(_: *gtk.Button, self: *Controller) callconv(.c) void { self.editor.manual_popover.?.popdown(); }
    pub fn historySearchActivated(_: *gtk.Entry, self: *Controller) callconv(.c) void { self.syncSharedEditor(); }
    pub fn editorOptionsChanged(_: *gobject.Object, _: *gobject.ParamSpec, self: *Controller) callconv(.c) void {
        if (self.session_create_configuring) return;
        session_editor_window.resize(self, self.editor.history_mode orelse false, self.session_create_advanced.?.getExpanded() != 0);
    }
    fn closeSharedEditor(self: *Controller) void {
        if (!self.editor_open) return;
        self.editor_open = false;
        self.editorInput(.{ .type = "close" });
    }
    fn syncSharedEditor(self: *Controller) void {
        if (self.editor_applying or self.session_create_configuring) return;
        const window = self.session_create_window orelse return;
        if (window.as(gtk.Widget).getVisible() == 0) return;
        const kind = self.session_create_kind orelse return;
        const draft: editor_interaction.Draft = .{
            .bind_only = self.idea_binding_pane!=null,
            .target = self.session_create_target_id orelse return,
            .endpoint = self.selectedCreateEndpoint() orelse "",
            .session_type = switch (kind) { .local_shell, .remote => "shell", .local_command => "command", .codex => "codex_terminal", .codex_agent => "codex", .claude_agent => "claude" },
            .mode = if (self.session_create_mode.?.getSelected() == 1) "resume" else "new",
            .name = std.mem.span(self.session_create_name.?.as(gtk.Editable).getText()),
            .directory = std.mem.span(self.session_create_directory.?.as(gtk.Editable).getText()),
            .command = std.mem.span(self.session_create_command.?.as(gtk.Editable).getText()),
            .original_id = std.mem.span(self.session_create_resume_id.?.as(gtk.Editable).getText()),
            .auto_restart = self.session_create_auto_restart.?.getActive() != 0,
            .yolo = self.session_create_yolo.?.getActive() != 0,
            .open_local_cli = self.session_create_local_cli.?.getActive() != 0,
            .search = std.mem.span(self.session_history_search.?.as(gtk.Editable).getText()),
            .archived = self.session_history_archived.?.getActive() != 0,
            .subagents = self.session_history_subagents.?.getActive() != 0,
        };
        const endpoints = self.session_create_endpoints orelse return;
        if (!self.editor_open) {
            self.editor_open = true;
            const source: ?control.SessionIdentity = if (self.native_executor) |executor| source: {
                const initiating = executor.quickCreateTarget(self.host_window) catch break :source null;
                break :source initiating.source;
            } else null;
            self.editorInput(.{ .type = "open", .draft = draft, .endpoints = endpoints.value, .source = source });
        } else {
            self.editorInput(.{ .type = "edit", .draft = draft });
        }
    }
    pub fn editorInput(self: *Controller, input: anytype) void {
        var output = self.editor_engine.dispatch(self.allocator, input) catch return;
        defer output.deinit();
        if (self.session_create_window != null) self.renderEditorView(output.value.view);
        for (output.value.effects) |effect| {
            if(std.mem.eql(u8,effect.type,"start_local_server")) {
                self.dispatch(.{.start_local_codex=.{.endpoint_id=effect.endpoint_id orelse continue,.owner_host_id=effect.owner_host_id orelse continue,.token=effect.token}});
            } else if ((std.mem.eql(u8, effect.type, "read") or std.mem.eql(u8, effect.type, "save_preference"))) {
                self.dispatch(.{ .session_editor = .{ .generation = effect.token, .request = effect.request orelse continue, .shared = true } });
            } else if (std.mem.eql(u8, effect.type, "cancel")) {
                self.dispatch(.{ .cancel_session_editor = .{ .token = effect.token } });
            } else if (std.mem.eql(u8,effect.type,"picked")) {
                if(self.idea_binding_callback) |callback| callback(self.idea_binding_context,self.idea_binding_pane orelse continue,self.idea_binding_view orelse continue,effect.owner_host_id orelse continue,effect.endpoint_id orelse continue,effect.thread_id orelse continue,effect.title orelse "");
                self.editor_submission=effect.token;self.sessionCreateAccepted();
            } else if (std.mem.eql(u8, effect.type, "submit")) {
                self.editor_submission = effect.token;
                const operation = effect.operation orelse continue;
                const operation_type = (operation.object.get("intent") orelse continue).string;
                inline for (.{ .create_session, .create_codex_session, .create_agent_session }) |tag| {
                    if (std.mem.eql(u8, operation_type, @tagName(tag))) {
                        const value = std.json.parseFromValue(std.meta.TagPayload(Action, tag), self.allocator, operation, .{ .ignore_unknown_fields = true }) catch return;
                        defer value.deinit();
                        self.dispatch(@unionInit(Action, @tagName(tag), value.value));
                    }
                }
            }
        }
    }
    fn renderEditorView(self: *Controller, view: editor_interaction.View) void {
        const directory_ready = self.session_context_pending and !view.directory_loading and view.draft.directory.len > 0;
        self.editor_applying = true;
        defer {
            self.editor_applying = false;
            if (directory_ready and self.editor_open) sessionDirectoryBrowseClicked(self.session_create_browse.?, self);
        }
        const d = view.draft;
        if(self.editor.start_server) |button| {
            button.as(gtk.Widget).setVisible(@intFromBool(view.can_start_local_server or view.server_starting));
            button.as(gtk.Widget).setSensitive(@intFromBool(view.can_start_local_server));
            const title=self.allocator.dupeZ(u8,view.server_start_label) catch return;defer self.allocator.free(title);button.as(gtk.Widget).setTooltipText(title);
        }
        // Only projected field values are copied into native widgets.
        inline for (.{ .{ "session_create_name", "name" }, .{ "session_create_directory", "directory" }, .{ "session_create_resume_id", "original_id" } }) |pair| {
            const entry = @field(self, pair[0]).?;
            const text = @field(d, pair[1]);
            if (!std.mem.eql(u8, std.mem.span(entry.as(gtk.Editable).getText()), text)) setEntryText(self.allocator, entry, text) catch {};
        }
        if (self.session_create_endpoints) |endpoints| {
            var index: u32 = 0;
            for (endpoints.value) |endpoint| {
                if (!std.mem.eql(u8, endpoint.execution_target_id, d.target)) continue;
                if (std.mem.eql(u8, endpoint.id, d.endpoint)) { self.session_create_endpoint.?.setSelected(index); break; }
                index += 1;
            }
        }
        self.session_create_yolo.?.setActive(@intFromBool(d.yolo));
        self.session_create_local_cli.?.setActive(@intFromBool(d.open_local_cli));
        self.session_context_pending = view.directory_loading;
        self.session_create_pending = view.pending;
        self.setSessionCreateEditing(!view.pending);
        self.session_create_local_cli.?.as(gtk.Widget).setSensitive(@intFromBool(view.local_cli_supported and !view.pending and !d.bind_only));
        self.session_create_type.?.as(gtk.Widget).setSensitive(@intFromBool(!view.pending and !d.bind_only));
        self.session_create_machine.?.as(gtk.Widget).setSensitive(@intFromBool(!view.pending));
        self.session_create_mode_bar.?.as(gtk.Widget).setVisible(@intFromBool(!d.bind_only));
        if(self.editor.options_menu) |menu| menu.as(gtk.Widget).setVisible(@intFromBool(!d.bind_only and std.mem.eql(u8,d.session_type,"codex") and std.mem.eql(u8,d.mode,"resume")));
        self.session_create_window.?.as(gtk.Window).setTitle(if(d.bind_only) "Bind Codex session" else "Add Session");
        self.session_create_submit.?.as(gtk.Widget).setSensitive(@intFromBool(view.can_submit));
        const caption = self.allocator.dupeZ(u8, view.submit_label) catch return;
        defer self.allocator.free(caption);
        self.session_create_submit.?.setLabel(caption);
        setLabel(self.allocator, self.session_create_error.?, view.message orelse view.history_error orelse "") catch {};
        self.session_history_more.?.as(gtk.Widget).setVisible(@intFromBool(view.history_error != null));
        self.session_history_more.?.as(gtk.Widget).setSensitive(@intFromBool(!view.history_loading));
        const rows_json = std.json.Stringify.valueAlloc(self.allocator, view.history, .{}) catch return;
        if (self.editor_history_json == null or !std.mem.eql(u8, self.editor_history_json.?, rows_json)) {
            if (self.editor_history_json) |old| self.allocator.free(old);
            self.editor_history_json = rows_json;
            // Preserve native rows and scroll position when the Rust projection
            // extends the existing list; replace only a changed physical prefix.
            var index: usize = 0;
            var replace = false;
            while (self.session_history_rows.?.getRowAtIndex(@intCast(index))) |row| : (index += 1) {
                if (index >= view.history.len or !std.mem.eql(u8, std.mem.span(row.as(gtk.Widget).getName()), view.history[index].thread_id)) { replace = true; break; }
            }
            if (replace) clearList(self.session_history_rows.?);
            for (view.history) |thread| session_editor_window.appendThread(self, thread);
        } else self.allocator.free(rows_json);
        var i: c_int = 0;
        while (self.session_history_rows.?.getRowAtIndex(i)) |row| : (i += 1) {
            const id = std.mem.span(row.as(gtk.Widget).getName());
            var selected = false;
            for (view.selected) |value| if (std.mem.eql(u8, value, id)) { selected = true; break; };
            if (row.getChild()) |content| if (content.getFirstChild()) |child| {
                if (gobject.ext.cast(gtk.CheckButton, child)) |checkbox| checkbox.setActive(@intFromBool(selected));
            };
            if (std.mem.eql(u8, d.original_id, id)) {
                self.session_history_rows.?.selectRow(row);
                if (self.session_create_window.?.as(gtk.Window).getFocus()) |focus| {
                    const list = self.session_history_rows.?.as(gtk.Widget);
                    if (focus == list or focus.isAncestor(list) != 0) _ = row.as(gtk.Widget).grabFocus();
                }
            }
        }
        const detail_json = std.json.Stringify.valueAlloc(self.allocator, .{ .detail = view.detail, .loading = view.detail_loading, .err = view.detail_error, .id = d.original_id }, .{}) catch return;
        if (self.editor_detail_json == null or !std.mem.eql(u8, self.editor_detail_json.?, detail_json)) {
            if (self.editor_detail_json) |old| self.allocator.free(old);
            self.editor_detail_json = detail_json;
            session_editor_window.resetDetails(self);
            if (view.detail) |detail| {
                const metadata = std.json.Stringify.valueAlloc(self.allocator, detail.thread, .{}) catch return;
                defer self.allocator.free(metadata);
                session_editor_window.showMetadata(self, metadata);
                if (detail.user_text) |text| session_editor_window.detailCard(self, "◉ You", text);
                if (detail.assistant_text) |text| session_editor_window.detailCard(self, "✦ Codex", text);
                if (detail.user_text == null and detail.assistant_text == null) session_editor_window.detailCard(self, "Latest exchange", "No messages yet");
            } else if (view.detail_loading) {
                self.session_history_details.?.setLabel("Loading latest exchange…");
            } else self.session_history_details.?.setLabel("Select a conversation to see its details");
            if (view.detail_error) |err| session_editor_window.detailCard(self, "Unable to read latest exchange", err);
            self.editor.detail_retry.?.as(gtk.Widget).setVisible(@intFromBool(view.detail_error != null));
        } else self.allocator.free(detail_json);
    }

    fn requestSessionHistory(self: *Controller, reset: bool) void {
        self.syncSharedEditor();
        if (!reset) self.editorInput(.{ .type = "more" });
    }
    fn resetSessionHistory(self: *Controller) void { self.closeSharedEditor(); }
    fn requestCreationDirectory(self: *Controller) void { self.syncSharedEditor(); }

    pub fn sessionDirectoryBrowseClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        if (self.session_create_pending) return;
        if (self.session_directory == null) self.session_directory = session_directory_window.Window.create(self.allocator, self.app, self.session_create_window.?.as(gtk.Window), self, sessionDirectoryNavigate, sessionDirectorySelected) catch return;
        const path = std.mem.span(self.session_create_directory.?.as(gtk.Editable).getText());
        if (self.editor.new_split) |split| self.session_directory.?.embed(split);
        self.session_directory.?.loading();
        sessionDirectoryNavigate(self, if (path.len == 0) "~" else path);
    }
    fn sessionDirectoryNavigate(data: ?*anyopaque, path: []const u8) void {
        const self: *Controller = @ptrCast(@alignCast(data orelse return));
        const target = self.session_create_target_id orelse return;
        self.session_context_pending = false;
        if (self.session_directory) |window| window.beginRead();
        self.creation_generation += 1;
        self.dispatch(.{ .session_editor = .{ .generation = self.creation_generation, .request = .{
            .type = "browse_directory",
            .execution_target_id = target,
            .path = path,
            .endpoint_id = if (self.session_create_kind == .codex) self.selectedCreateEndpoint() else null,
        } } });
    }
    fn sessionDirectorySelected(data: ?*anyopaque, path: []const u8) void {
        const self: *Controller = @ptrCast(@alignCast(data orelse return));
        if (!self.session_create_pending) setEntryText(self.allocator, self.session_create_directory.?, path) catch {};
    }
    pub fn sessionCreateEndpointChanged(_: *gobject.Object, _: *gobject.ParamSpec, self: *Controller) callconv(.c) void {
        if (self.editor_applying or self.session_create_configuring) return;
        self.requestSessionHistory(true);
        if (!self.session_context_pending) self.creation_generation += 1;
        sessionCreateChanged(self.session_create_name.?.as(gtk.Editable), self);
        if (self.model.projection()) |projection| self.refreshEditorEndpoints(projection);
        if (self.session_directory) |window| window.native.as(gtk.Widget).setVisible(0);
    }

    pub fn sessionCreateLocalCLIChanged(_: *gtk.CheckButton, self: *Controller) callconv(.c) void {
        sessionCreateChanged(self.session_create_name.?.as(gtk.Editable), self);
    }

    pub fn sessionCreateChoiceChanged(source: *gobject.Object, _: *gobject.ParamSpec, self: *Controller) callconv(.c) void {
        if (self.editor_applying or self.session_create_configuring) return;
        self.updateSessionCreateChoices();
        if (source != self.session_create_mode.?.as(gobject.Object) or self.session_history_rows.?.getRowAtIndex(0) == null) self.requestSessionHistory(true);
    }

    fn updateSessionCreateChoices(self: *Controller) void {
        if (self.editor_applying) return;
        const groups = self.session_create_groups orelse return;
        const index = self.session_create_machine.?.getSelected();
        if (index >= groups.value.len) return;
        const group = groups.value[index];
        const changed = if (self.session_create_target_id) |old| !std.mem.eql(u8, old, group.id) else true;
        const next = self.allocator.dupe(u8, group.id) catch return;
        if (self.session_create_target_id) |old| self.allocator.free(old);
        self.session_create_target_id = next;
        const is_command = self.session_create_type.?.getSelected() == 1;
        const selected_type = self.session_create_type.?.getSelected();
        const is_codex = selected_type == 2 or selected_type == 4;
        const is_agent = selected_type == 2 or selected_type == 3;
        self.session_create_kind = switch (selected_type) {
            1 => .local_command,
            2 => .codex_agent,
            3 => .claude_agent,
            4 => .codex,
            else => .local_shell,
        };
        self.session_create_mode_bar.?.as(gtk.Widget).setVisible(@intFromBool(is_agent));
        self.session_appserver_field.?.as(gtk.Widget).setVisible(@intFromBool(is_codex));
        self.session_create_appserver_label.?.as(gtk.Widget).setVisible(0);
        self.session_create_appserver_row.?.as(gtk.Widget).setVisible(@intFromBool(is_codex));
        self.session_create_advanced.?.as(gtk.Widget).setVisible(@intFromBool(is_codex));
        self.session_endpoint_status.?.as(gtk.Widget).setVisible(@intFromBool(is_codex));
        const is_resume = is_agent and self.session_create_mode.?.getSelected() == 1;
        self.session_editor_body.?.as(gtk.Widget).setMarginTop(if (is_resume and is_codex) 0 else 16);
        self.session_editor_body.?.as(gtk.Widget).setMarginBottom(if (is_resume and is_codex) 4 else 16);
        self.editor.name_row.?.as(gtk.Widget).setVisible(@intFromBool(!(is_resume and is_codex)));
        self.editor.command_row.?.as(gtk.Widget).setVisible(@intFromBool(is_command));
        self.session_create_name.?.as(gtk.Widget).setVisible(@intFromBool(!(is_resume and is_codex)));
        self.session_create_name_label.?.as(gtk.Widget).setVisible(@intFromBool(!(is_resume and is_codex)));
        self.session_create_directory_row.?.as(gtk.Widget).setVisible(@intFromBool(!(is_resume and is_codex)));
        self.session_create_directory_label.?.as(gtk.Widget).setVisible(@intFromBool(!(is_resume and is_codex)));
        self.session_create_new_button.?.as(gtk.Widget).removeCssClass("accent");
        self.session_create_resume_button.?.as(gtk.Widget).removeCssClass("accent");
        (if (is_resume) self.session_create_resume_button.? else self.session_create_new_button.?).as(gtk.Widget).addCssClass("accent");
        self.session_create_submit.?.setLabel(if (is_resume) "Add 1 to Tab" else "New & Open");
        self.session_history_box.?.as(gtk.Widget).setVisible(@intFromBool(is_resume and is_codex));
        self.session_editor_scroll.?.as(gtk.Widget).setVexpand(@intFromBool(!(is_resume and is_codex)));
        self.session_manual_id.?.as(gtk.Widget).setVisible(@intFromBool(is_resume and !is_codex));
        self.editor.manual_button.?.as(gtk.Widget).setVisible(@intFromBool(is_resume and is_codex));
        session_editor_window.showHistoryOptions(self, is_resume and is_codex);
        session_editor_window.resize(self, is_resume and is_codex, self.session_create_advanced.?.getExpanded() != 0);
        self.session_create_resume_label.?.as(gtk.Widget).setVisible(@intFromBool(is_resume));
        self.session_create_resume_id.?.as(gtk.Widget).setVisible(@intFromBool(is_resume));
        const endpoints = gtk.StringList.new(null);
        defer endpoints.unref();
        if (self.session_create_endpoints) |configured| for (configured.value) |e| {
            if (!std.mem.eql(u8, e.execution_target_id, group.id)) continue;
            const name = self.allocator.dupeZ(u8, e.id) catch continue;
            defer self.allocator.free(name);
            endpoints.append(name);
        };
        const old_endpoint_index = self.session_create_endpoint.?.getSelected();
        const was_configuring = self.session_create_configuring;
        self.session_create_configuring = true;
        self.session_create_endpoint.?.setModel(endpoints.as(@import("gio").ListModel));
        self.session_create_endpoint.?.setSelected(if (!changed and old_endpoint_index < endpoints.as(@import("gio").ListModel).getNItems()) old_endpoint_index else 0);
        self.session_create_configuring = was_configuring;
        self.session_create_endpoint.?.as(gtk.Widget).setVisible(@intFromBool(is_codex));
        self.session_create_yolo.?.as(gtk.Widget).setVisible(@intFromBool(is_codex and !is_resume));
        self.session_create_local_cli.?.as(gtk.Widget).setVisible(@intFromBool(selected_type == 2));
        if (changed and !self.session_create_configuring) {
            if (self.session_directory) |window| window.native.as(gtk.Widget).setVisible(0);
            self.requestCreationDirectory();
        }
        self.session_create_command.?.as(gtk.Widget).setVisible(@intFromBool(is_command));
        self.session_create_command_label.?.as(gtk.Widget).setVisible(@intFromBool(is_command));
        self.session_create_auto_restart.?.as(gtk.Widget).setVisible(@intFromBool(is_command));
        if (self.native_executor) |executor| if (executor.nativeWorkspaceSnapshot()) |snapshot| {
            for (snapshot.tabs) |tab| {
                if (!std.mem.eql(u8, tab.id, self.session_create_tab_id orelse "")) continue;
                const destination = std.fmt.allocPrint(self.allocator, "{s} {s} / {s}", .{ if (self.idea_binding_pane != null) "Bind to Idea Tree in" else "Opens in", snapshot.workspace_name, tab.title }) catch return;
                defer self.allocator.free(destination);
                setLabel(self.allocator, self.session_create_target.?, destination) catch {};
                break;
            }
        };
        sessionCreateChanged(self.session_create_name.?.as(gtk.Editable), self);
    }

    fn refreshEditorEndpoints(self: *Controller, projection: *const control.Projection) void {
        const window = self.session_create_window orelse return;
        if (window.as(gtk.Widget).getVisible() == 0 or self.session_create_pending or self.session_create_configuring) return;
        const configured = self.session_create_endpoints orelse return;
        const previous = std.json.Stringify.valueAlloc(self.allocator, configured.value, .{}) catch return;
        defer self.allocator.free(previous);
        const next = std.json.Stringify.valueAlloc(self.allocator, projection.codex_endpoints, .{}) catch return;
        defer self.allocator.free(next);
        if (!std.mem.eql(u8, previous, next)) {
            const replacement = std.json.parseFromSlice([]control.CodexEndpoint, self.allocator, next, .{ .allocate = .alloc_always, .ignore_unknown_fields = true }) catch return;
            configured.deinit();
            self.session_create_endpoints = replacement;
            if (self.editor_open) self.editorInput(.{ .type = "endpoints", .endpoints = replacement.value });
            self.updateSessionCreateChoices();
            self.requestSessionHistory(true);
        }
        const text = if (self.selectedCreateEndpoint()) |id| projection.codex_endpoint_states.map.get(id) orelse "Connection status unavailable" else "No app server configured on this machine. Open Manage to add one.";
        setLabel(self.allocator, self.session_endpoint_status.?, text) catch {};
    }

    fn selectedCreateEndpoint(self: *Controller) ?[]const u8 {
        const configured = self.session_create_endpoints orelse return null;
        const target = self.session_create_target_id orelse return null;
        const selected = self.session_create_endpoint.?.getSelected();
        var index: usize = 0;
        for (configured.value) |e| {
            if (!std.mem.eql(u8, e.execution_target_id, target)) continue;
            if (index == selected) return e.id;
            index += 1;
        }
        return null;
    }
    pub fn sessionCreateChanged(_: *gtk.Editable, self: *Controller) callconv(.c) void { self.syncSharedEditor(); }

    pub fn sessionCreateCloseRequested(_: *gtk.Window, self: *Controller) callconv(.c) c_int {
        if (!self.session_create_pending) sessionCreateCancelClicked(self.session_create_cancel.?, self);
        return 1;
    }

    pub fn sessionCreateKeyPressed(_: *gtk.EventControllerKey, keyval: c_uint, _: c_uint, modifiers: gdk.ModifierType, self: *Controller) callconv(.c) c_int {
        if (modifiers.shift_mask or modifiers.control_mask or modifiers.alt_mask or modifiers.super_mask) return 0;
        if (keyval == gdk.KEY_Escape) {
            _ = sessionCreateCloseRequested(self.session_create_window.?.as(gtk.Window), self);
            return 1;
        }
        const focus = self.session_create_window.?.as(gtk.Window).getFocus() orelse return 0;
        const list = self.session_history_rows.?.as(gtk.Widget);
        if (!(self.editor.history_mode orelse false) or (focus != list and focus.isAncestor(list) == 0)) return 0;
        switch (keyval) {
            gdk.KEY_Up, gdk.KEY_KP_Up => self.editorInput(.{ .type = "previous" }),
            gdk.KEY_Down, gdk.KEY_KP_Down => self.editorInput(.{ .type = "next" }),
            gdk.KEY_space => self.editorInput(.{ .type = "toggle_selected" }),
            gdk.KEY_Return, gdk.KEY_KP_Enter => self.editorInput(.{ .type = "submit" }),
            else => return 0,
        }
        return 1;
    }

    fn refreshClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        const projection = self.model.projection() orelse return;
        if (!projection.header.refresh.enabled or
            projection.header.refresh.confirmation == null) return;
        self.restore_popover.popup();
    }

    fn restoreCancelClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.restore_popover.popdown();
    }

    fn restoreConfirmClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        const projection = self.model.projection() orelse return;
        const confirmation = projection.header.refresh.confirmation orelse return;
        self.restore_popover.popdown();
        self.dispatch(.{ .invoke_action = .{
            .action_id = projection.header.refresh.id,
            .confirmation_id = confirmation.id,
        } });
    }

    fn sessionsRefreshClicked(
        _: *gtk.Button,
        self: *Controller,
    ) callconv(.c) void {
        const projection = self.model.projection() orelse return;
        self.dispatch(.{ .invoke_action = .{
            .action_id = projection.sessions.refresh_action.id,
        } });
    }

    fn lifecycleClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        const projection = self.model.projection() orelse return;
        const action = projection.header.lifecycle_action orelse return;
        self.dispatch(.{ .invoke_action = .{ .action_id = action.id } });
    }

    fn newWorkspaceWindowClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.workspace_menu.popdown();
        self.dispatch(.{ .invoke_action = .{ .action_id = "workspace.window.new" } });
    }

    fn deleteWorkspaceConfirmed(button: *gtk.Button, self: *Controller) callconv(.c) void {
        const target = std.mem.span(button.as(gtk.Widget).getName());
        self.workspace_menu.popdown();
        self.dispatch(.{ .invoke_action = .{ .action_id = "workspace.archive", .confirmation_id = target } });
    }

    fn detachCancelClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.workspace_menu.popdown();
    }

    fn detachConfirmClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.workspace_menu.popdown();
        self.dispatch(.{ .invoke_action = .{ .action_id = "window.close" } });
    }

    fn sessionsToggled(
        button: *gtk.ToggleButton,
        self: *Controller,
    ) callconv(.c) void {
        if (self.rendering) return;
        const visible = button.getActive() != 0;
        if (visible and !self.admitsPanels(true, self.layout_map_visible, self.inbox_visible)) {
            self.rendering = true; button.setActive(0); self.rendering = false; return;
        }
        self.sessions_visible = visible;
        self.applySidebarLayout(self.sessions_visible);
        if (self.sessions_visible) self.render();
    }

    fn closeSessionsClicked(
        _: *gtk.Button,
        self: *Controller,
    ) callconv(.c) void {
        if (!self.sessions_visible) return;
        self.sessions_visible = false;
        self.rendering = true;
        self.sessions_button.setActive(0);
        self.rendering = false;
        self.applySidebarLayout(false);
    }

    fn restorePanelVisibility(self: *Controller) void {
        const workspace = if (self.native_executor) |executor| executor.nativeWorkspaceSnapshot() else null;
        self.layout_map_visible = if (workspace) |value| value.layout_map_visible orelse true else true;
        self.inbox_visible = if (workspace) |value| value.inbox_visible orelse false else false;
    }

    fn savePanelVisibility(self: *Controller) void {
        if (self.native_executor) |executor| executor.savePanelVisibility(self.layout_map_visible, self.inbox_visible) catch |err| {
            std.log.err("Could not save panel visibility: {s}", .{@errorName(err)});
        };
    }

    fn layoutToggled(
        button: *gtk.ToggleButton,
        self: *Controller,
    ) callconv(.c) void {
        if (self.rendering) return;
        const visible = button.getActive() != 0;
        if (visible and !self.admitsPanels(self.sessions_visible, true, self.inbox_visible)) {
            self.rendering = true; button.setActive(0); self.rendering = false; return;
        }
        self.layout_map_visible = visible;
        self.savePanelVisibility();
        self.applySidebarLayout(self.sessions_visible);
        if (visible) self.render();
    }

    fn inboxToggled(button: *gtk.ToggleButton, self: *Controller) callconv(.c) void {
        if (self.rendering) return;
        const visible = button.getActive() != 0;
        if (visible and !self.admitsPanels(self.sessions_visible, self.layout_map_visible, true)) {
            self.rendering = true; button.setActive(0); self.rendering = false; return;
        }
        self.inbox_visible = visible;
        self.savePanelVisibility();
        self.applySidebarLayout(self.sessions_visible);
        if (visible) {
            self.render();
            self.dispatch(.inbox_refresh);
        }
    }

    fn inboxRefreshClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.dispatch(.inbox_refresh);
    }

    fn inboxReplyClicked(_: *gtk.Button, context: *InboxContext) callconv(.c) void {
        const entry = context.entry orelse return;
        const value = std.mem.trim(u8, std.mem.span(entry.as(gtk.Editable).getText()), " \t\r\n");
        if (value.len == 0) return;
        context.controller.dispatch(.{ .inbox_reply = .{ .message_id = context.message_id, .revision = context.revision, .reply = value } });
    }

    fn inboxEditClicked(_: *gtk.Button, context: *InboxContext) callconv(.c) void {
        const entry = context.entry orelse return;
        const value = std.mem.trim(u8, std.mem.span(entry.as(gtk.Editable).getText()), " \t\r\n");
        if (value.len == 0) return;
        context.controller.dispatch(.{ .inbox_edit_draft = .{ .message_id = context.message_id, .revision = context.revision, .draft = value } });
    }

    fn inboxApproveClicked(_: *gtk.Button, context: *InboxContext) callconv(.c) void {
        context.controller.dispatch(.{ .inbox_approve = .{ .message_id = context.message_id, .revision = context.revision } });
    }

    fn inboxOpenClicked(_: *gtk.Button, context: *InboxContext) callconv(.c) void {
        const page = context.controller.inbox_model.page() orelse return;
        for (page.messages) |message| {
            if (std.mem.eql(u8, message.id, context.message_id)) {
                context.controller.dispatch(.{ .activate_pane = .{ .pane_id = message.origin.pane_id } });
                return;
            }
        }
    }

    fn resourcesClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        if (self.model.projection() == null) return;
        self.dispatch(.{ .invoke_action = .{ .action_id = "resources.open" } });
    }

    fn iphoneClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        if (self.model.projection() == null) return;
        self.dispatch(.{ .invoke_action = .{ .action_id = "iphone.open" } });
    }

    fn fontSizeClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.more_menu.popdown();
        self.presentFontSizeEditor();
    }

    fn fontSizeCancelClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.font_size_window.?.as(gtk.Widget).setVisible(0);
    }

    fn fontSizeApplyClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        const raw = self.font_size_spin.?.getValueAsInt();
        if (raw < 10 or raw > 32) return;
        self.dispatch(.{ .set_managed_terminal_font_size = .{
            .points = @intCast(raw),
        } });
        self.font_size_window.?.as(gtk.Widget).setVisible(0);
    }

    fn newWorkspaceClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.more_menu.popdown();
        self.presentNewWorkspaceEditor();
    }

    fn newWorkspaceCancelClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        self.new_workspace_window.?.as(gtk.Widget).setVisible(0);
    }

    fn newWorkspaceApplyClicked(_: *gtk.Button, self: *Controller) callconv(.c) void {
        const entry = self.new_workspace_entry orelse return;
        const name = std.mem.trim(
            u8,
            std.mem.span(entry.as(gtk.Editable).getText()),
            " \t\r\n",
        );
        if (name.len == 0) return;
        self.dispatch(.{ .create_workspace = .{ .name = name } });
        self.new_workspace_window.?.as(gtk.Widget).setVisible(0);
    }

    fn keyPressed(
        _: *gtk.EventControllerKey,
        keyval: c_uint,
        _: c_uint,
        _: gdk.ModifierType,
        self: *Controller,
    ) callconv(.c) c_int {
        if (keyval != gdk.KEY_Escape) return 0;
        self.hide();
        return 1;
    }

    fn sessionButtonContextWeakNotify(
        data: ?*anyopaque,
        _: *gobject.Object,
    ) callconv(.c) void {
        const context: *SessionButtonContext = @ptrCast(@alignCast(
            data orelse return,
        ));
        const allocator = context.allocator;
        allocator.free(context.execution_target_id);
        allocator.free(context.tmux_session_id);
        if (context.replacement_json) |json| allocator.free(json);
        context.* = undefined;
        allocator.destroy(context);
    }

    fn sessionCreateTargetContextWeakNotify(
        data: ?*anyopaque,
        _: *gobject.Object,
    ) callconv(.c) void {
        const context: *SessionCreateTargetContext = @ptrCast(@alignCast(
            data orelse return,
        ));
        const allocator = context.allocator;
        allocator.free(context.target_id);
        context.* = undefined;
        allocator.destroy(context);
    }

    fn tabButtonContextWeakNotify(
        data: ?*anyopaque,
        _: *gobject.Object,
    ) callconv(.c) void {
        const context: *TabButtonContext = @ptrCast(@alignCast(
            data orelse return,
        ));
        const allocator = context.allocator;
        allocator.free(context.tab_id);
        if (context.delete_confirmation_id) |value| allocator.free(value);
        context.* = undefined;
        allocator.destroy(context);
    }

    fn paneButtonContextWeakNotify(
        data: ?*anyopaque,
        _: *gobject.Object,
    ) callconv(.c) void {
        const context: *PaneButtonContext = @ptrCast(@alignCast(
            data orelse return,
        ));
        const allocator = context.allocator;
        allocator.free(context.tab_id);
        allocator.free(context.pane_id);
        context.* = undefined;
        allocator.destroy(context);
    }

    fn inboxContextWeakNotify(
        context: ?*anyopaque,
        _: *gobject.Object,
    ) callconv(.c) void {
        const value: *InboxContext = @ptrCast(@alignCast(context orelse return));
        const allocator = value.controller.allocator;
        allocator.free(value.message_id);
        allocator.destroy(value);
    }

    fn hostWindowWeakNotify(
        context: ?*anyopaque,
        object: *gobject.Object,
    ) callconv(.c) void {
        const self: *Controller = @ptrCast(@alignCast(context orelse return));
        if (@intFromPtr(self.host_window) != @intFromPtr(object)) return;
        self.host_alive = false;
        self.lifecycle.controlCenterHidden();
    }
};

fn sessionAssignmentAction(
    session: *const control.SessionRow,
    native: ?presentation_executor.NativeSessionAssignment,
) *const control.Action {
    const assignment = native orelse return &session.assignment_action;
    const actions = if (session.assignment_actions) |*value|
        value
    else
        return &session.assignment_action;
    return switch (assignment) {
        .available => &actions.add,
        .assigned_here => &actions.assigned_elsewhere,
        .assigned_elsewhere => &actions.assigned_elsewhere,
        .tab_full => &actions.tab_full,
        .no_managed_tab => &actions.no_managed_tab,
    };
}

fn sessionAssignedHere(
    fallback: bool,
    native: ?presentation_executor.NativeSessionAssignment,
) bool {
    const assignment = native orelse return fallback;
    return switch (assignment) {
        .assigned_here => true,
        else => false,
    };
}

const AgentCatalogView = struct { root: usize, parent: ?usize, key: []const u8, group: []const u8, matches: bool, rank: usize };

const RenameTarget = union(enum) {
    agent: struct { owner_host_id: []const u8, endpoint_id: []const u8, thread_id: []const u8 },
    workspace,
    tab: []const u8,
    session: control.SessionIdentity,

    fn clone(self: RenameTarget, allocator: std.mem.Allocator) !RenameTarget {
        return switch (self) {
            .agent => |agent| blk: {
                const owner = try allocator.dupe(u8, agent.owner_host_id);
                errdefer allocator.free(owner);
                const endpoint = try allocator.dupe(u8, agent.endpoint_id);
                errdefer allocator.free(endpoint);
                break :blk .{ .agent = .{ .owner_host_id = owner, .endpoint_id = endpoint, .thread_id = try allocator.dupe(u8, agent.thread_id) } };
            },
            .workspace => .workspace,
            .tab => |id| .{ .tab = try allocator.dupe(u8, id) },
            .session => |identity| blk: {
                const target = try allocator.dupe(u8, identity.execution_target_id);
                errdefer allocator.free(target);
                break :blk .{ .session = .{
                    .execution_target_id = target,
                    .tmux_session_id = try allocator.dupe(u8, identity.tmux_session_id),
                } };
            },
        };
    }

    fn deinit(self: RenameTarget, allocator: std.mem.Allocator) void {
        switch (self) {
            .agent => |agent| {
                allocator.free(agent.owner_host_id);
                allocator.free(agent.endpoint_id);
                allocator.free(agent.thread_id);
            },
            .workspace => {},
            .tab => |id| allocator.free(id),
            .session => |identity| {
                allocator.free(identity.execution_target_id);
                allocator.free(identity.tmux_session_id);
            },
        }
    }
};

const SessionButtonContext = struct {
    allocator: std.mem.Allocator,
    controller: *Controller,
    execution_target_id: []u8,
    tmux_session_id: []u8,
    replacement_popover: ?*gtk.Popover = null,
    replacement_json: ?[]u8 = null,
    context_menu: ?*gtk.Popover = null,
};

const SessionCreateTargetContext = struct {
    allocator: std.mem.Allocator,
    controller: *Controller,
    target_id: []u8,
};

const TabButtonContext = struct {
    allocator: std.mem.Allocator,
    controller: *Controller,
    tab_id: []u8,
    tab_index: usize,
    tab_count: usize,
    pane_count: usize,
    card: ?*gtk.Widget = null,
    move_popover: ?*gtk.Popover = null,
    delete_popover: ?*gtk.Popover = null,
    actions_popover: ?*gtk.Popover = null,
    move_submitted: bool = false,
    delete_submitted: bool = false,
    delete_confirmation_id: ?[]u8 = null,
};

const PendingPaneActionKind = enum { select, activate, remove };

const PendingPaneAction = union(enum) {
    select: []u8,
    activate: []u8,
    select_then_activate: []u8,
    remove: []u8,
};

const PendingPaneMove = pane_interaction.PaneDrop;

const PaneButtonRef = struct {
    pane_id: []const u8,
    tile: *gtk.Widget,
    button: *gtk.Button,
    content: *gtk.Box,
    status: *gtk.Label,
    attention: ?*gtk.Label,
    attention_visible: bool,
    column_span: c_int,
    total_columns: c_int,

    fn updateDensity(self: PaneButtonRef, map_width: c_int) void {
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

const PaneButtonContext = struct {
    allocator: std.mem.Allocator,
    controller: *Controller,
    tab_id: []u8,
    pane_id: []u8,
    button: ?*gtk.Button = null,
    context_menu: ?*gtk.Popover = null,
    remove_button: ?*gtk.Button = null,
    selected: bool = false,
    focused: bool = false,
    hovered: bool = false,

    fn updateRemoveVisibility(self: *PaneButtonContext) void {
        const remove = self.remove_button orelse return;
        remove.as(gtk.Widget).setVisible(@intFromBool(
            layout_policy.paneRemoveVisible(
                self.selected,
                self.focused,
                self.hovered,
            ),
        ));
    }
};

const InboxContext = struct {
    controller: *Controller,
    message_id: []u8,
    revision: u64,
    entry: ?*gtk.Entry,
};

fn appendRow(box: *gtk.Box, first: *gtk.Widget, second: *gtk.Widget) void {
    const row = gtk.Box.new(.horizontal, 7);
    first.setHexpand(1);
    row.append(first);
    row.append(second);
    box.append(row.as(gtk.Widget));
}

fn setEntryText(allocator: std.mem.Allocator, entry: *gtk.Entry, value: []const u8) !void {
    const owned = try allocator.dupeZ(u8, value);
    defer allocator.free(owned);
    entry.as(gtk.Editable).setText(owned);
}

const IconLabelControl = struct {
    content: *gtk.Box,
    image: *gtk.Image,
    text: *gtk.Label,
};

fn iconLabelControl(
    icon_name: [*:0]const u8,
    text_value: [*:0]const u8,
) IconLabelControl {
    const content = gtk.Box.new(.vertical, 1);
    content.as(gtk.Widget).setHalign(.fill);
    content.as(gtk.Widget).setValign(.center);
    content.as(gtk.Widget).setSizeRequest(-1, 44);
    const image = gtk.Image.newFromIconName(icon_name);
    image.as(gtk.Widget).setHalign(.center);
    const text = label(text_value, "caption");
    // Icon controls keep their Rust-owned tooltip/accessibility label; visible
    // text must not impose a four-label minimum on a 200-point controller.
    text.as(gtk.Widget).setVisible(0);
    text.setXalign(0.5);
    content.append(image.as(gtk.Widget));
    content.append(text.as(gtk.Widget));
    return .{
        .content = content,
        .image = image,
        .text = text,
    };
}

fn menuBox() *gtk.Box {
    const box = gtk.Box.new(.vertical, 4);
    setMargins(box.as(gtk.Widget), 8);
    return box;
}

fn menuRowButton(
    icon_name: [*:0]const u8,
    text_value: [*:0]const u8,
) MenuRowButton {
    const button = gtk.Button.new();
    const row = gtk.Box.new(.horizontal, 8);
    const text = label(text_value, "body");
    row.append(
        gtk.Image.newFromIconName(icon_name).as(gtk.Widget),
    );
    row.append(text.as(gtk.Widget));
    button.setChild(row.as(gtk.Widget));
    return .{
        .button = button,
        .text = text,
    };
}

const MenuRowButton = struct {
    button: *gtk.Button,
    text: *gtk.Label,
};

fn listContent(
    allocator: std.mem.Allocator,
    title_text: []const u8,
    subtitle_text: []const u8,
) !*gtk.Box {
    const content = gtk.Box.new(.vertical, 3);
    setMargins(content.as(gtk.Widget), 10);
    const title = label("", "heading");
    const subtitle = label("", "caption");
    try setLabel(allocator, title, title_text);
    try setLabel(allocator, subtitle, subtitle_text);
    subtitle.as(gtk.Widget).addCssClass("dim-label");
    content.append(title.as(gtk.Widget));
    content.append(subtitle.as(gtk.Widget));
    return content;
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

fn clearBox(box: *gtk.Box) void {
    while (box.as(gtk.Widget).getFirstChild()) |child| box.remove(child);
}

fn detachPopover(popover: ?*gtk.Popover) void {
    const value = popover orelse return;
    value.popdown();
    value.setChild(null);
    value.as(gtk.Widget).unparent();
}

fn clearList(list: *gtk.ListBox) void {
    while (list.as(gtk.Widget).getFirstChild()) |child| list.remove(child);
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

fn setWindowTitle(
    allocator: std.mem.Allocator,
    target: *adw.Window,
    value: []const u8,
) !void {
    const text = try allocator.dupeZ(u8, value);
    defer allocator.free(text);
    target.as(gtk.Window).setTitle(text);
}

fn setEntryPlaceholder(
    allocator: std.mem.Allocator,
    target: *gtk.Entry,
    value: []const u8,
) !void {
    const text = try allocator.dupeZ(u8, value);
    defer allocator.free(text);
    target.setPlaceholderText(text);
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

fn renderActionButton(
    allocator: std.mem.Allocator,
    button: *gtk.Button,
    action: *const control.Action,
) void {
    setButtonLabel(allocator, button, action.label) catch {};
    button.as(gtk.Widget).setSensitive(@intFromBool(action.enabled));
    const tooltip = action.disabled_reason orelse action.accessibility_label;
    const text = allocator.dupeZ(u8, tooltip) catch return;
    defer allocator.free(text);
    button.as(gtk.Widget).setTooltipText(text);
}

fn renderActionState(
    allocator: std.mem.Allocator,
    button: *gtk.Button,
    action: *const control.Action,
) void {
    button.as(gtk.Widget).setSensitive(
        @intFromBool(action.enabled),
    );
    const tooltip = action.disabled_reason orelse
        action.accessibility_label;
    const text = allocator.dupeZ(u8, tooltip) catch return;
    defer allocator.free(text);
    button.as(gtk.Widget).setTooltipText(text);
}

fn renderMenuButtonState(
    allocator: std.mem.Allocator,
    button: *gtk.MenuButton,
    action: *const control.Action,
) void {
    button.as(gtk.Widget).setSensitive(
        @intFromBool(action.enabled),
    );
    const tooltip = action.disabled_reason orelse
        action.accessibility_label;
    const text = allocator.dupeZ(u8, tooltip) catch return;
    defer allocator.free(text);
    button.as(gtk.Widget).setTooltipText(text);
}

fn renderToggleState(
    allocator: std.mem.Allocator,
    button: *gtk.ToggleButton,
    text_label: *gtk.Label,
    action: *const control.Action,
) void {
    setLabel(allocator, text_label, action.label) catch {};
    button.setActive(@intFromBool(action.selected));
    button.as(gtk.Widget).setSensitive(
        @intFromBool(action.enabled),
    );
    const tooltip = action.disabled_reason orelse
        action.accessibility_label;
    const tooltip_text = allocator.dupeZ(
        u8,
        tooltip,
    ) catch return;
    defer allocator.free(tooltip_text);
    button.as(gtk.Widget).setTooltipText(tooltip_text);
}

fn renderToggle(
    allocator: std.mem.Allocator,
    button: *gtk.ToggleButton,
    action: *const control.Action,
) void {
    const text = allocator.dupeZ(u8, action.label) catch return;
    defer allocator.free(text);
    button.as(gtk.Button).setLabel(text);
    button.setActive(@intFromBool(action.selected));
    button.as(gtk.Widget).setSensitive(@intFromBool(action.enabled));
    const tooltip = action.disabled_reason orelse action.accessibility_label;
    const tooltip_text = allocator.dupeZ(u8, tooltip) catch return;
    defer allocator.free(tooltip_text);
    button.as(gtk.Widget).setTooltipText(tooltip_text);
}

fn connect(
    button: *gtk.Button,
    comptime Context: type,
    callback: *const fn (*gtk.Button, *Context) callconv(.c) void,
    context: *Context,
) void {
    _ = gtk.Button.signals.clicked.connect(button, *Context, callback, context, .{});
}
