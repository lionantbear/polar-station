//! Shared GTK input shell; specialized content controls reuse the same editor.
const std = @import("std");
const gtk = @import("gtk");
const gdk = @import("gdk");
const gio = @import("gio");
const adw = @import("adw");
const glib = @import("glib");
const gobject = @import("gobject");
const auxiliary = @import("auxiliary_window.zig");
const NativeWindow = @import("../class/window.zig").Window;
const Application = @import("../class/application.zig").Application;
const Surface = @import("../class/surface.zig").Surface;
const DisplaySurface = @import("display_surface.zig").DisplaySurface;
const graphics = @import("graphics_surface.zig");
const dictate_module = @import("dictate.zig");
const Content = @import("station_content.zig").Content;
const Hooks = @import("native_content_hooks.zig").Hooks;
const c = @cImport({
    @cInclude("polar_station_desktop_client.h");
});
const Value = std.json.Value;
const TerminalWait = @import("lgp_workspace.zig").TerminalWait;
const TerminalViewState = @import("lgp_workspace.zig").TerminalViewState;
const Workspace = @import("lgp_workspace.zig").Workspace;
const Entry = @import("lgp_workspace.zig").Entry;
const scope = @import("lgp_workspace.zig").scope;
const field = @import("lgp_workspace.zig").field;
const string = @import("lgp_workspace.zig").string;
const themeColor = @import("lgp_workspace.zig").themeColor;
const coordinate = @import("lgp_workspace.zig").coordinate;
const number = @import("lgp_workspace.zig").number;
const boolean = @import("lgp_workspace.zig").boolean;
const array = @import("lgp_workspace.zig").array;
const setLabel = @import("lgp_workspace.zig").setLabel;
const margins = @import("lgp_workspace.zig").margins;

const FooterLayout = struct {
    root:*gtk.Grid, leading:*gtk.Box, trailing:*gtk.Box,
    fn create() FooterLayout {
        const root=gtk.Grid.new();root.setColumnSpacing(6);
        const leading=gtk.Box.new(.horizontal,6);const trailing=gtk.Box.new(.horizontal,6);
        leading.as(gtk.Widget).setHexpand(0);trailing.as(gtk.Widget).setHexpand(0);
        leading.as(gtk.Widget).setValign(.end);trailing.as(gtk.Widget).setValign(.end);
        trailing.as(gtk.Widget).setHalign(.end);
        root.attach(leading.as(gtk.Widget),0,0,1,1);root.attach(trailing.as(gtk.Widget),2,0,1,1);
        return .{.root=root,.leading=leading,.trailing=trailing};
    }
    fn mountDraft(self:FooterLayout,widget:*gtk.Widget) void {
        widget.setHexpand(1);self.root.attach(widget,1,0,1,1);
    }
};

const DraftEditor = struct {
    text: *gtk.TextView,
    scroll: *gtk.ScrolledWindow,
    document: *gtk.Box,
    overlay: *gtk.Overlay,
    hint: *gtk.Label,
    fn create() DraftEditor {
        const text = gtk.TextView.new();
        text.setWrapMode(.word_char);
        text.getBuffer().setEnableUndo(1);
        text.setTopMargin(6);
        text.setBottomMargin(6);
        text.setLeftMargin(8);
        text.setRightMargin(8);
        const scroll = gtk.ScrolledWindow.new();
        scroll.setPolicy(.never, .automatic);
        scroll.as(gtk.Widget).addCssClass("station-chat-editor");
        scroll.setMinContentHeight(30);
        scroll.setMaxContentHeight(120);
        scroll.setPropagateNaturalHeight(1);
        const document = gtk.Box.new(.vertical, 6);
        document.append(text.as(gtk.Widget));
        scroll.setChild(document.as(gtk.Widget));
        const overlay = gtk.Overlay.new();
        // A measured single-line slot owns row geometry; GTK's unmeasured
        // overlay grows upward and keeps native picking outside the slot.
        const editor_slot = gtk.Box.new(.vertical, 0);
        editor_slot.as(gtk.Widget).setSizeRequest(-1,30);
        overlay.setChild(editor_slot.as(gtk.Widget));
        scroll.as(gtk.Widget).setValign(.end);
        overlay.addOverlay(scroll.as(gtk.Widget));
        overlay.setMeasureOverlay(scroll.as(gtk.Widget),0);
        overlay.setClipOverlay(scroll.as(gtk.Widget),0);
        _ = gtk.Overlay.signals.get_child_position.connect(overlay,*gtk.ScrolledWindow,DraftEditor.position,scroll,.{});
        overlay.as(gtk.Widget).setHexpand(1);
        overlay.as(gtk.Widget).setSizeRequest(120,-1);
        const hint = gtk.Label.new("Select a conversation");
        hint.setXalign(0);
        hint.setEllipsize(.end);
        hint.as(gtk.Widget).setHalign(.fill);
        hint.as(gtk.Widget).setValign(.start);
        hint.as(gtk.Widget).setMarginStart(8);
        hint.as(gtk.Widget).setMarginTop(6);
        hint.as(gtk.Widget).addCssClass("dim-label");
        hint.as(gtk.Widget).setCanTarget(0);
        overlay.addOverlay(hint.as(gtk.Widget));

        return .{.text=text,.scroll=scroll,.document=document,.overlay=overlay,.hint=hint};
    }
    fn position(overlay: *gtk.Overlay, child: *gtk.Widget, allocation: *gdk.Rectangle, scroll: *gtk.ScrolledWindow) callconv(.c) c_int {
        if (child != scroll.as(gtk.Widget)) return 0;
        const width = overlay.as(gtk.Widget).getWidth();
        var natural: c_int = 0;
        child.measure(.vertical,width,null,&natural,null,null);
        allocation.* = .{ .f_x=0, .f_y=overlay.as(gtk.Widget).getHeight()-natural, .f_width=width, .f_height=natural };
        return 1;
    }

};

pub const InputBar = struct {
    dictate_button: *gtk.Button,
    dictate_menu: *gtk.MenuButton,
    dictate_history: *@import("dictate_history.zig").History,
    dictate_click: ?*gtk.GestureClick = null,
    dictate_chord:@import("dictate_local_keys.zig").Chord=.{},
    dictate_keys: ?*gtk.EventControllerKey = null,
    owner: *Workspace,
    id: []u8,
    host: *NativeWindow,
    container: *gtk.Box,
    body: *gtk.Box,
    chat_tools: *gtk.Box,
    idea_row: *gtk.Box,
    idea_menu:*gtk.MenuButton,
    idea_files:*gtk.Box,
    idea_configuration:*gtk.Box,
    idea_configuration_menu:*gtk.MenuButton,
    idea_search_button:*gtk.MenuButton,
    editor_overlay:*gtk.Overlay,
    idea_search: *gtk.Entry,
    idea_more: *gtk.MenuButton,
    idea_overflow: *gtk.Box,
    idea_view_scroll: *gtk.ScrolledWindow,
    idea_theme: *gtk.MenuButton,
    idea_themes: *gtk.Box,
    more: *gtk.MenuButton,
    idea_bindings: std.ArrayListUnmanaged(*IdeaBinding) = .empty,
    is_idea: bool = false,
    inquiry_entry:*gtk.Button,
    dictate_active_signal:c_ulong=0,
    dictate_map_signal:c_ulong=0,
    dictate_panel:?*@import("dictate_panel.zig").Panel=null,
    inquiry_popover:*gtk.Popover,
    inquiry_data:?std.json.Parsed(Value)=null,
    inquiry_active:bool=false,
    inquiry_answer_scroll:?*gtk.ScrolledWindow=null,
    inquiry_buttons:std.ArrayListUnmanaged(*gtk.Button)=.empty,
    inquiry_held:?c_uint=null,
    inquiry_deferred:bool=false,
    inquiry_signature:?[]u8=null,
    inquiry_extra:?*gtk.Label=null,
    terminal_wait: ?TerminalWait = null,
    terminal_timeout: c_uint = 0,

    inquiry_focus_source:c_uint=0,
    inquiry_focus_pending:bool=false,
    inquiry_restore_index:?usize=null,
    toggle: *gtk.Button,
    target: *gtk.Button,
    status: *gtk.Label,
    refine_mode_popup:?*gtk.Popover=null,
    dictate_mode_popup:?*gtk.Popover=null,
    refine_button: *gtk.Button,
    edit_action_depth: usize = 0,
    status_button: *gtk.MenuButton,
    mode_button: *gtk.MenuButton,
    tools: *gtk.Popover,
    hint: *gtk.Label,
    text: *gtk.TextView,
    inquiry_text:*gtk.TextView,
    editor_scroll: *gtk.ScrolledWindow,
    send_button: *gtk.Button,
    older: *gtk.Button,
    retry: *gtk.Button,
    quote: *gtk.Button,
    modified: *gtk.CheckButton,
    pane_id: ?[]u8 = null,
    metadata: ?std.json.Parsed(Value) = null,
    idea_document: *gtk.Box,
    node_editors: std.ArrayListUnmanaged(*NodeEditor) = .empty,
    detail_editor: ?*NodeEditor = null,
    detail_overlay: ?*gtk.Overlay = null,
    detail_scroll: ?*gtk.ScrolledWindow = null,
    detail_css: ?*gtk.CssProvider = null,
    detail_style_hash: u64 = 0,
    last_editor: ?*gtk.TextView = null,
    idea_refs: *gtk.MenuButton,
    idea_attach: *gtk.Button,
    picker: ?*Picker = null,
    updating: bool = false,
    composing: bool = false,
    expanded: bool = true,
    focus_source: c_uint = 0,
    css: *gtk.CssProvider,

    pub fn create(owner: *Workspace, id: []const u8, host: *NativeWindow) !*InputBar {
        const self = try owner.allocator.create(InputBar);
        errdefer owner.allocator.destroy(self);
        const owned_id = try owner.allocator.dupe(u8, id);
        errdefer owner.allocator.free(owned_id);
        const outer = gtk.Box.new(.vertical, 4);
        outer.as(gtk.Widget).addCssClass("station-input-bar");
        outer.as(gtk.Widget).setValign(.end);
        _ = outer.as(gobject.Object).refSink();
        const idea_refs=gtk.MenuButton.new();idea_refs.setLabel("References");idea_refs.setPopover(gtk.Popover.new());idea_refs.as(gtk.Widget).setVisible(0);
        const idea_attach=gtk.Button.newWithLabel("Insert node");idea_attach.as(gtk.Widget).setTooltipText("Insert selected title and UUID · Ctrl+Shift+I");idea_attach.as(gtk.Widget).setVisible(0);
        const idea_row = gtk.Box.new(.horizontal, 6);
        idea_row.as(gtk.Widget).setVisible(0);
        const idea_menu=gtk.MenuButton.new();idea_menu.setLabel("Idea");
        const idea_files=gtk.Box.new(.vertical,4);margins(idea_files.as(gtk.Widget));
        const idea_files_popup=gtk.Popover.new();idea_files_popup.setChild(idea_files.as(gtk.Widget));idea_menu.setPopover(idea_files_popup);
        const idea_configuration=gtk.Box.new(.vertical,4);margins(idea_configuration.as(gtk.Widget));
        const idea_configuration_popup=gtk.Popover.new();idea_configuration_popup.setChild(idea_configuration.as(gtk.Widget));
        const idea_configuration_menu=gtk.MenuButton.new();idea_configuration_menu.setPopover(idea_configuration_popup);
        idea_configuration_menu.as(gtk.Widget).setVisible(0);idea_files.append(idea_configuration_menu.as(gtk.Widget));
        idea_menu.as(gtk.Widget).setVisible(0);
        const idea_search_button=gtk.MenuButton.new();idea_search_button.setIconName("system-search-symbolic");
        idea_search_button.as(gtk.Widget).setTooltipText("Search nodes");
        const idea_search_popup=gtk.Popover.new();idea_search_button.setPopover(idea_search_popup);
        const idea_search = gtk.Entry.new();
        idea_search.setPlaceholderText("Search nodes…");
        idea_search.as(gtk.Widget).setSizeRequest(260,-1);
        idea_search_popup.setChild(idea_search.as(gtk.Widget));
        const idea_more = gtk.MenuButton.new();
        idea_more.setLabel("View");
        const idea_popup = gtk.Popover.new();
        const idea_overflow = gtk.Box.new(.vertical, 4);
        margins(idea_overflow.as(gtk.Widget));
        const idea_view_scroll = gtk.ScrolledWindow.new();
        idea_view_scroll.setPolicy(.never, .automatic);
        idea_view_scroll.setMaxContentHeight(640);
        idea_view_scroll.setPropagateNaturalHeight(1);
        idea_view_scroll.setChild(idea_overflow.as(gtk.Widget));
        idea_popup.setChild(idea_view_scroll.as(gtk.Widget));
        idea_more.setPopover(idea_popup);
        const idea_theme = gtk.MenuButton.new();
        idea_theme.setLabel("Theme");
        idea_theme.as(gtk.Widget).setVisible(0);
        const idea_theme_popup = gtk.Popover.new();
        const idea_themes = gtk.Box.new(.vertical, 4);
        margins(idea_themes.as(gtk.Widget));
        idea_theme_popup.setChild(idea_themes.as(gtk.Widget));
        idea_theme.setPopover(idea_theme_popup);
        const layout = FooterLayout.create();
        outer.append(layout.root.as(gtk.Widget));
        outer.as(gtk.Widget).setMarginStart(8);outer.as(gtk.Widget).setMarginEnd(8);
        outer.as(gtk.Widget).setMarginTop(4);outer.as(gtk.Widget).setMarginBottom(6);
        const header=layout.leading;const trailing=layout.trailing;
        const toggle = gtk.Button.newWithLabel("▾ Chat");
        toggle.as(gtk.Widget).addCssClass("flat");
        header.append(toggle.as(gtk.Widget));
        header.append(idea_menu.as(gtk.Widget));
        header.append(idea_row.as(gtk.Widget));
        const target = gtk.Button.newWithLabel("Choose Channel / Thread…");
        target.setCanShrink(1);
        target.as(gtk.Widget).setHexpand(0);
        target.as(gtk.Widget).setSizeRequest(90, -1);
        if (target.getChild()) |child| if (gobject.ext.cast(gtk.Label, child)) |label| {
            label.setMaxWidthChars(1);
            label.setEllipsize(.middle);
        };
        target.as(gtk.Widget).setTooltipText("Choose Polar, Channel and Thread");
        header.append(target.as(gtk.Widget));
        const status = gtk.Label.new("");
        status.setWrap(1);
        status.setMaxWidthChars(40);
        const status_button = gtk.MenuButton.new();
        status_button.setLabel("Choose a conversation");
        status_button.as(gtk.Widget).setVisible(0);
        status_button.as(gtk.Widget).setTooltipText("Connection status");
        const status_popup = gtk.Popover.new();
        const status_box = gtk.Box.new(.vertical, 6);
        margins(status_box.as(gtk.Widget));
        status_box.append(status.as(gtk.Widget));
        status_popup.setChild(status_box.as(gtk.Widget));
        status_button.setPopover(status_popup);
        const retry = gtk.Button.newWithLabel("Reconnect");
        retry.as(gtk.Widget).setTooltipText("Reconnect this conversation; unsent messages are not sent automatically");
        retry.as(gtk.Widget).setVisible(0);

        const more = gtk.MenuButton.new();
        more.setLabel("⋯");
        more.as(gtk.Widget).setTooltipText("Conversation tools");
        const tools = gtk.Popover.new();
        const tools_box = gtk.Box.new(.vertical, 6);
        const chat_tools = gtk.Box.new(.vertical,6); tools_box.append(chat_tools.as(gtk.Widget));
        margins(tools_box.as(gtk.Widget));
        const older = gtk.Button.newWithLabel("Load older messages");
        chat_tools.append(older.as(gtk.Widget));
        const references = gtk.Button.newWithLabel("Message references…");
        chat_tools.append(references.as(gtk.Widget));
        const activity = gtk.Button.newWithLabel("Tool activity…");
        chat_tools.append(activity.as(gtk.Widget));
        const search = gtk.Entry.new();
        search.setPlaceholderText("Search loaded messages");
        chat_tools.append(search.as(gtk.Widget));
        tools.setChild(tools_box.as(gtk.Widget));
        more.setPopover(tools);

        const body = gtk.Box.new(.horizontal, 6);
        const draft_editor = DraftEditor.create();
        const inquiry_text=gtk.TextView.new();inquiry_text.setWrapMode(.word_char);
        inquiry_text.getBuffer().setEnableUndo(1);_=inquiry_text.as(gobject.Object).refSink();
        const text=draft_editor.text;const scroll=draft_editor.scroll;const document=draft_editor.document;
        const overlay=draft_editor.overlay;const hint=draft_editor.hint;
        body.append(overlay.as(gtk.Widget));
        const send = gtk.Button.newWithLabel("Send");
        if(send.getChild()) |child| if(gobject.ext.cast(gtk.Label,child)) |label| {label.setMaxWidthChars(1);};
        send.as(gtk.Widget).setSensitive(0);
        send.as(gtk.Widget).addCssClass("suggested-action");
        send.as(gtk.Widget).setValign(.end);
        const dictate_button = gtk.Button.newFromIconName("audio-input-microphone-symbolic");
        dictate_button.as(gtk.Widget).setFocusOnClick(0);
        dictate_button.as(gtk.Widget).setSizeRequest(30, -1);
        const dictate_menu = gtk.MenuButton.new(); dictate_menu.setIconName("pan-down-symbolic");
        const dictate_history = try @import("dictate_history.zig").History.create(owner.dictate,text);
        dictate_menu.setPopover(dictate_history.popup);

        const mode = gtk.MenuButton.new();
        mode.setIconName("pan-down-symbolic");
        mode.as(gtk.Widget).setTooltipText("Send keyboard shortcut");
        mode.as(gtk.Widget).setValign(.end);
        const mode_popup = gtk.Popover.new();
        const modes = gtk.Box.new(.vertical, 6);
        margins(modes.as(gtk.Widget));
        const enter = gtk.CheckButton.newWithLabel("Enter sends · Shift+Enter inserts newline");
        const modified = gtk.CheckButton.newWithLabel("Ctrl+Enter sends · Enter inserts newline");
        modified.setGroup(enter);
        modes.append(enter.as(gtk.Widget));
        modes.append(modified.as(gtk.Widget));
        mode_popup.setChild(modes.as(gtk.Widget));
        mode.setPopover(mode_popup);
        body.as(gtk.Widget).setHexpand(1);
        body.as(gtk.Widget).setValign(.end);
        idea_row.as(gtk.Widget).setValign(.end);
        const inquiry_entry=gtk.Button.newWithLabel("Inquiry");
        inquiry_entry.as(gtk.Widget).setVisible(0);
        const inquiry_popover=gtk.Popover.new();
        inquiry_popover.as(gtk.Widget).setParent(more.as(gtk.Widget));
        inquiry_popover.setPosition(.top);inquiry_popover.setAutohide(0);
        tools_box.append(inquiry_entry.as(gtk.Widget));
        layout.mountDraft(body.as(gtk.Widget));
        trailing.append(idea_attach.as(gtk.Widget));


        tools_box.append(status_button.as(gtk.Widget));
        tools_box.append(retry.as(gtk.Widget));
        const quote = gtk.Button.newWithLabel("❞");
        quote.as(gtk.Widget).setTooltipText("Quote selection · Ctrl+Shift+G");
        quote.as(gtk.Widget).setFocusOnClick(0);
        quote.as(gtk.Widget).setVisible(0);
        trailing.append(quote.as(gtk.Widget));
        trailing.append(more.as(gtk.Widget));
        const refine = gtk.Button.newFromIconName("document-edit-symbolic"); refine.as(gtk.Widget).setTooltipText("Refine draft"); refine.as(gtk.Widget).setFocusOnClick(0);
        refine.as(gtk.Widget).setSizeRequest(44,-1); refine.as(gtk.Widget).setValign(.end); trailing.append(refine.as(gtk.Widget));
        trailing.append(dictate_button.as(gtk.Widget)); trailing.append(dictate_menu.as(gtk.Widget));
        const primary_slot = gtk.Box.new(.horizontal,0);
        primary_slot.as(gtk.Widget).setSizeRequest(88,30);
        primary_slot.as(gtk.Widget).setHexpand(0);
        send.as(gtk.Widget).setHexpand(1);
        primary_slot.append(send.as(gtk.Widget));
        trailing.append(primary_slot.as(gtk.Widget));
        tools_box.append(mode.as(gtk.Widget));
        for ([_]*gtk.Widget{ toggle.as(gtk.Widget), idea_menu.as(gtk.Widget), idea_attach.as(gtk.Widget), target.as(gtk.Widget), status_button.as(gtk.Widget), more.as(gtk.Widget), dictate_button.as(gtk.Widget), dictate_menu.as(gtk.Widget), send.as(gtk.Widget), mode.as(gtk.Widget), retry.as(gtk.Widget), quote.as(gtk.Widget) }) |widget| widget.setValign(.end);
        const css = gtk.CssProvider.new();
        // The theme's vertical slider minimum otherwise forces two lines even
        // for an empty editor. Keep the scrollbar and six-line growth limit.
        css.loadFromString(".station-input-bar { background-color: @window_bg_color; } .station-chat-target { outline: 2px solid @accent_color; outline-offset: -2px; } .station-chat-editor { background-color: @view_bg_color; border: 1px solid alpha(@theme_fg_color,0.2); border-radius: 4px; } .station-chat-editor textview { background-color: transparent; } .station-chat-editor scrollbar.vertical slider { min-height: 16px; }");
        gtk.StyleContext.addProviderForDisplay(host.as(gtk.Widget).getDisplay(), css.as(gtk.StyleProvider), gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 5);
        self.* = .{ .inquiry_text=inquiry_text, .chat_tools=chat_tools, .refine_button=refine, .dictate_button=dictate_button, .dictate_menu=dictate_menu, .dictate_history=dictate_history, .idea_document=document, .idea_refs=idea_refs,.idea_attach=idea_attach,.inquiry_entry=inquiry_entry,.inquiry_popover=inquiry_popover,.owner = owner, .id = owned_id, .host = host, .container = outer, .body = body, .idea_row = idea_row,.idea_menu=idea_menu,.idea_files=idea_files, .idea_configuration=idea_configuration, .idea_configuration_menu=idea_configuration_menu,.idea_search_button=idea_search_button,.editor_overlay=overlay, .idea_search = idea_search, .idea_more = idea_more, .idea_overflow = idea_overflow, .idea_view_scroll = idea_view_scroll, .idea_theme = idea_theme, .idea_themes = idea_themes, .more = more, .toggle = toggle, .target = target, .status = status, .status_button = status_button, .mode_button = mode, .tools = tools, .hint = hint, .text = text, .editor_scroll = scroll, .send_button = send, .older = older, .retry = retry, .quote = quote, .modified = modified, .css = css };
        // Keep all tree actions inside the one Idea menu.
        idea_files.append(idea_search_button.as(gtk.Widget));
        idea_files.append(idea_refs.as(gtk.Widget));
        const copy_draft=gtk.Button.newWithLabel("Copy draft");idea_files.append(copy_draft.as(gtk.Widget));
        _=gtk.Button.signals.clicked.connect(copy_draft,*InputBar,copyIdeaDraft,self,.{});
        idea_files.append(idea_more.as(gtk.Widget));
        idea_files.append(idea_theme.as(gtk.Widget));
        const overlay_host: *gtk.Overlay = @ptrCast(@alignCast(host.as(gobject.Object).getData("polar-station-composer-host") orelse return error.MainWindowContentUnavailable));
        overlay_host.addOverlay(outer.as(gtk.Widget));
        overlay_host.setMeasureOverlay(outer.as(gtk.Widget),0);
        _ = gtk.Entry.signals.activate.connect(idea_search, *InputBar, ideaSearchActivated, self, .{});
        _ = gobject.Object.signals.notify.connect(idea_popup.as(gobject.Object),*InputBar,ideaViewVisibility,self,.{ .detail="visible" });
        _ = gtk.Button.signals.clicked.connect(inquiry_entry,*InputBar,inquiryEntry,self,.{});
        const inquiry_keys=gtk.EventControllerKey.new();inquiry_keys.as(gtk.EventController).setPropagationPhase(.bubble);
        _ = gtk.EventControllerKey.signals.key_pressed.connect(inquiry_keys,*InputBar,inquiryKeys,self,.{});
        _ = gtk.EventControllerKey.signals.key_released.connect(inquiry_keys,*InputBar,inquiryKeyReleased,self,.{});
        inquiry_popover.as(gtk.Widget).addController(inquiry_keys.as(gtk.EventController));
        self.setSendMode(owner.modified_send);
        _ = gtk.Button.signals.clicked.connect(dictate_button, *InputBar, dictateClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(refine, *InputBar, refineClicked, self, .{});
        const refine_press=gtk.GestureLongPress.new();refine_press.as(gtk.EventController).setPropagationPhase(.capture);
        _=gtk.GestureLongPress.signals.pressed.connect(refine_press,*InputBar,refineHeld,self,.{});
        refine.as(gtk.Widget).addController(refine_press.as(gtk.EventController));
        const dictate_press=gtk.GestureLongPress.new();dictate_press.as(gtk.EventController).setPropagationPhase(.capture);
        _=gtk.GestureLongPress.signals.pressed.connect(dictate_press,*InputBar,dictateHeld,self,.{});
        dictate_button.as(gtk.Widget).addController(dictate_press.as(gtk.EventController));
        _ = gtk.TextBuffer.signals.begin_user_action.connect(text.getBuffer(), *InputBar, editBegin, self, .{});
        _ = gtk.TextBuffer.signals.end_user_action.connect(text.getBuffer(), *InputBar, editEnd, self, .{});
        self.refreshDictate();
        self.dictate_active_signal=gobject.Object.signals.notify.connect(self.host.as(gobject.Object),*InputBar,dictateWindowActive,self,.{.detail="is-active"});
        self.dictate_map_signal=gtk.Widget.signals.map.connect(dictate_button.as(gtk.Widget),*InputBar,dictateMapped,self,.{});
        self.mountDictateFocus();
        _ = gtk.Button.signals.clicked.connect(toggle, *InputBar, toggleClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(idea_attach,*InputBar,attachReference,self,.{});
        _ = gtk.Button.signals.clicked.connect(target, *InputBar, chooseClicked, self, .{});
        const text_focus=gtk.EventControllerFocus.new();
        _=gtk.EventControllerFocus.signals.enter.connect(text_focus,*InputBar,generalFocused,self,.{});text.as(gtk.Widget).addController(text_focus.as(gtk.EventController));
        _ = gtk.TextBuffer.signals.changed.connect(text.getBuffer(), *InputBar, changed, self, .{});
        _ = gtk.TextBuffer.signals.changed.connect(inquiry_text.getBuffer(),*InputBar,inquiryEdited,self,.{});
        _ = gtk.TextView.signals.preedit_changed.connect(inquiry_text,*InputBar,inquiryPreedit,self,.{});
        _ = gtk.TextView.signals.preedit_changed.connect(text, *InputBar, preeditChanged, self, .{});
        _ = gtk.Button.signals.clicked.connect(send, *InputBar, sendClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(older, *InputBar, olderClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(references, *InputBar, referencesClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(activity, *InputBar, activityClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(quote, *InputBar, quoteClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(retry, *InputBar, retryClicked, self, .{});
        _ = gtk.Entry.signals.activate.connect(search, *InputBar, searchActivated, self, .{});
        _ = gtk.CheckButton.signals.toggled.connect(modified, *InputBar, modeChanged, self, .{});
        for ([_]*gtk.TextView{text,inquiry_text}) |editor| {
            const keys = gtk.EventControllerKey.new();
            keys.as(gtk.EventController).setPropagationPhase(.capture);
            _ = gtk.EventControllerKey.signals.key_pressed.connect(keys, *InputBar, keyPressed, self, .{});
            _ = gtk.EventControllerKey.signals.key_released.connect(keys,*InputBar,inquiryKeyReleased,self,.{});
            editor.as(gtk.Widget).addController(keys.as(gtk.EventController));
        }
        return self;
    }
    pub fn beginTerminalView(self: *InputBar, effect: Value) !void {
        if (self.terminal_wait != null) self.finishTerminalView(false);
        const target=field(field(effect,"target") orelse return error.InvalidTarget,"client_view") orelse return error.InvalidTarget;
        const execution=string(target,"execution_target_id") orelse return error.InvalidTarget;
        const session=string(target,"tmux_session_id") orelse return error.InvalidTarget;
        const source=string(effect,"pane_id") orelse return error.InvalidTarget;
        const ensure=self.owner.ensure_terminal orelse return error.TerminalViewUnsupported;
        const pane=try ensure(self.owner.context,self.host,source,execution,session);
        const allocator=self.owner.allocator;
        const owned_pane=try allocator.dupe(u8,pane);errdefer allocator.free(owned_pane);
        const owned_execution=try allocator.dupe(u8,execution);errdefer allocator.free(owned_execution);
        const owned_session=try allocator.dupe(u8,session);errdefer allocator.free(owned_session);
        const token=try allocator.dupe(u8,string(effect,"token") orelse return error.InvalidToken);
        self.terminal_wait=.{.pane=owned_pane,.execution=owned_execution,.session=owned_session,.token=token};
        self.terminal_timeout=glib.timeoutAdd(30_000,terminalExpired,self);
        if(self.terminal_timeout==0){self.finishTerminalView(false);return;}
        self.checkTerminalView();
    }
    fn terminalExpired(context:?*anyopaque) callconv(.c) c_int {
        const self:*InputBar=@ptrCast(@alignCast(context orelse return 0));
        self.terminal_timeout=0;self.finishTerminalView(false);return 0;
    }
    pub fn checkTerminalView(self:*InputBar) void {
        const wait=self.terminal_wait orelse return;
        const status=self.owner.terminal_state orelse {self.finishTerminalView(false);return;};
        switch (status(self.owner.context,self.host,wait.pane,wait.execution,wait.session)) {
            .pending=>{},.ready=>self.finishTerminalView(true),.unavailable=>self.finishTerminalView(false),
        }
    }
    fn clearTerminalWait(self:*InputBar) void {
        if(self.terminal_timeout!=0){_ = glib.Source.remove(self.terminal_timeout);self.terminal_timeout=0;}
        if(self.terminal_wait) |wait| {for([_] []u8{wait.pane,wait.execution,wait.session,wait.token}) |slice| self.owner.allocator.free(slice);self.terminal_wait=null;}
    }
    fn finishTerminalView(self:*InputBar,ready:bool) void {
        const wait=self.terminal_wait orelse return;
        // input serializes synchronously; clear the native lease first so an
        // event or repeated callback cannot confirm the same view twice.
        self.terminal_wait=null;
        if(self.terminal_timeout!=0){_ = glib.Source.remove(self.terminal_timeout);self.terminal_timeout=0;}
        defer for([_] []u8{wait.pane,wait.execution,wait.session,wait.token}) |slice| self.owner.allocator.free(slice);
        self.owner.input(.{.type="terminal_view_result",.window_id=self.id,.token=wait.token,.ready=ready,.@"error"=if(ready) @as(?[]const u8,null) else "Target terminal unavailable"});
    }
    pub fn destroy(self: *InputBar) void {
        if(self.dictate_active_signal!=0)gobject.signalHandlerDisconnect(self.host.as(gobject.Object),self.dictate_active_signal);
        if(self.dictate_map_signal!=0)gobject.signalHandlerDisconnect(self.dictate_button.as(gobject.Object),self.dictate_map_signal);
        if(self.refine_mode_popup) |popup| popup.as(gtk.Widget).unparent();
        if(self.dictate_mode_popup) |popup| popup.as(gtk.Widget).unparent();
        if(self.dictate_panel) |panel| panel.destroy();
        if(self.dictate_click)|controller|self.host.as(gtk.Widget).removeController(controller.as(gtk.EventController));
        if(self.dictate_keys)|controller|self.host.as(gtk.Widget).removeController(controller.as(gtk.EventController));
        self.clearNodeEditors();
        self.clearTerminalWait();
        if(self.inquiry_focus_source!=0)_ = glib.Source.remove(self.inquiry_focus_source);
        if(self.inquiry_signature) |signature| self.owner.allocator.free(signature);
        self.restoreInquiryEditor();self.inquiry_popover.popdown();self.inquiry_popover.as(gtk.Widget).unparent();
        self.inquiry_buttons.deinit(self.owner.allocator);
        if(self.inquiry_data) |*data| data.deinit();
        for (self.idea_bindings.items) |binding| { self.owner.allocator.free(binding.action); self.owner.allocator.free(binding.query); self.owner.allocator.destroy(binding); }
        self.idea_bindings.deinit(self.owner.allocator);
        self.updating = true;
        if (self.focus_source != 0) _ = glib.Source.remove(self.focus_source);
        if (self.picker) |picker| picker.destroy();
        if (self.metadata) |*value| value.deinit();
        if (self.pane_id) |value| self.owner.allocator.free(value);
        gtk.StyleContext.removeProviderForDisplay(self.host.as(gtk.Widget).getDisplay(), self.css.as(gtk.StyleProvider));
        self.css.unref();
        if (self.container.as(gtk.Widget).getParent()) |parent| if (gobject.ext.cast(gtk.Overlay, parent)) |overlay| overlay.removeOverlay(self.container.as(gtk.Widget));
        self.inquiry_text.as(gobject.Object).unref();
        self.container.unref();
        self.owner.allocator.free(self.id);
        self.owner.allocator.destroy(self);
    }
    pub fn updateGeneric(self: *InputBar, view: Value) void {
        self.restoreInquiryEditor();self.inquiry_popover.popdown();self.inquiry_active=false;self.dictate_history.bindInput(self.text);self.text.setEditable(1);
        const lease = field(view, "lease") orelse return;
        const pane = string(lease, "pane") orelse return;
        const bytes = std.json.Stringify.valueAlloc(self.owner.allocator, .{
            .type="composer", .pane_id=pane, .text=string(view,"text") orelse "",
            .draft_revision=number(lease,"draft_revision"), .generic_lease=lease, .project=field(view,"project"),
            .status=string(view,"unavailable") orelse "Ready", .can_send=boolean(view,"can_send"),
        }, .{}) catch return;
        defer self.owner.allocator.free(bytes);
        var parsed = std.json.parseFromSlice(Value,self.owner.allocator,bytes,.{}) catch return;
        defer parsed.deinit();
        self.update(parsed.value);
        const send_label=string(view,"send_label") orelse "Send";
        {
            const label_=self.owner.allocator.dupeZ(u8,send_label) catch return;
            defer self.owner.allocator.free(label_);self.send_button.setLabel(label_);
        }
        self.setSendMode(self.owner.modified_send);
        const label = self.owner.allocator.dupeZ(u8,string(view,"target_status") orelse string(view,"target_label") orelse "Terminal input") catch return;
        defer self.owner.allocator.free(label);
        self.target.setLabel(label);
        self.more.as(gtk.Widget).setVisible(1);
        const explanation=self.owner.allocator.dupeZ(u8,string(view,"unavailable") orelse string(view,"target_label") orelse "Terminal input") catch return;
        defer self.owner.allocator.free(explanation);self.target.as(gtk.Widget).setTooltipText(explanation);
        setLabel(self.owner.allocator, self.hint, "Input…");
        self.retry.setLabel("↻");
        self.retry.as(gtk.Widget).setTooltipText("Refresh sending status");
        self.retry.as(gtk.Widget).setVisible(@intFromBool(boolean(view,"can_refresh")));
        self.target.as(gtk.Widget).setSensitive(0);
        self.toggle.as(gtk.Widget).setVisible(0);
        self.expanded=true;
        self.body.as(gtk.Widget).setVisible(1);
    }
    pub fn update(self: *InputBar, effect: Value) void {
        self.retry.setLabel("Reconnect");
        self.quote.as(gtk.Widget).setVisible(if (field(effect,"generic_lease") != null) 1 else 0);
        self.target.as(gtk.Widget).setSensitive(1);
        self.toggle.as(gtk.Widget).setVisible(0);
        const identity = std.json.Stringify.valueAlloc(self.owner.allocator, .{
            .pane=field(effect,"pane_id"), .view=field(effect,"view_id"), .revision=field(effect,"draft_revision"), .input_lease=field(effect,"generic_lease")
        }, .{}) catch return;
        defer self.owner.allocator.free(identity);
        self.text.as(gobject.Object).setData("station-dictate-scope", @ptrFromInt(std.hash.Wyhash.hash(0,identity) | 1));
        const answer_token=if(self.inquiry_data) |data| string(data.value,"token") orelse "" else "";
        self.inquiry_text.as(gobject.Object).setData("station-dictate-scope",@ptrFromInt(std.hash.Wyhash.hash(std.hash.Wyhash.hash(0,identity),answer_token)|1));
        const bytes = std.json.Stringify.valueAlloc(self.owner.allocator, effect, .{}) catch return;
        defer self.owner.allocator.free(bytes);
        const metadata = std.json.parseFromSlice(Value, self.owner.allocator, bytes, .{ .allocate = .alloc_always }) catch return;
        if (self.metadata) |*old| old.deinit();
        self.metadata = metadata;
        const id = string(effect, "pane_id") orelse return;
        const changed_target = self.pane_id == null or !std.mem.eql(u8, self.pane_id.?, id);
        if (changed_target) {
            self.clearNodeEditors();
            self.idea_refs.popdown();
            self.idea_more.popdown();
            self.idea_theme.popdown();
            if (self.focus_source!=0) {_ = glib.Source.remove(self.focus_source);self.focus_source=0;}
            const copy = self.owner.allocator.dupe(u8, id) catch return;
            if (self.pane_id) |old| self.owner.allocator.free(old);
            self.pane_id = copy;
        }
        self.updating = true;
        defer self.updating = false;
        const incoming = string(effect, "text") orelse "";
        const buffer = (if(self.inquiry_active) self.inquiry_text else self.text).getBuffer();
        var start: gtk.TextIter = undefined;
        var end: gtk.TextIter = undefined;
        buffer.getBounds(&start, &end);
        const current = buffer.getText(&start, &end, 1);
        defer glib.free(current);
        if (!self.composing and !std.mem.eql(u8, std.mem.span(current), incoming)) {
            const z = self.owner.allocator.dupeZ(u8, incoming) catch return;
            defer self.owner.allocator.free(z);
            buffer.setText(z, @intCast(incoming.len));
        }
        self.hint.as(gtk.Widget).setVisible(@intFromBool(incoming.len == 0 and !self.composing));
        self.setIdeaMode(std.mem.eql(u8, string(effect, "type") orelse "", "idea_composer"));
        if (self.is_idea) {
            if (changed_target) { self.idea_search.as(gtk.Editable).setText(""); if (self.picker) |picker| { picker.destroy(); self.picker = null; } }
            self.updateIdeaControls(effect);
            const editor_hint = self.owner.allocator.dupeZ(u8, string(effect, "placeholder") orelse "") catch return;
            defer self.owner.allocator.free(editor_hint);
            self.text.as(gtk.Widget).setTooltipText(editor_hint);
            setLabel(self.owner.allocator, self.hint, string(effect, "placeholder") orelse "Idea Tree instruction");
            self.showStatus(string(effect, "status") orelse "");
            self.status_button.setLabel("●");
        self.status_button.as(gtk.Widget).setVisible(0);
            self.send_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(effect,"can_send")));
            const target_label=self.owner.allocator.dupeZ(u8,string(effect,"target_label") orelse "Bind Codex…") catch return;
            defer self.owner.allocator.free(target_label);self.target.setLabel(if(self.host.as(gtk.Widget).getWidth()<760) "Codex" else target_label);
            const target_hint=std.fmt.allocPrintSentinel(self.owner.allocator,"{s} · {s}",.{string(effect,"target_label") orelse "Codex",string(effect,"status") orelse ""},0) catch return;
            defer self.owner.allocator.free(target_hint);self.target.as(gtk.Widget).setTooltipText(target_hint);
            self.updateReferences(effect);
            self.updateNodeEditors(effect);
            self.idea_attach.as(gtk.Widget).setSensitive(@intFromBool((string(field(effect,"selected_node") orelse .null,"node_uuid") orelse "").len > 0));

            self.retry.as(gtk.Widget).setVisible(0);
            self.container.as(gtk.Widget).setVisible(1);
            return;
        }
        const title = std.fmt.allocPrintSentinel(self.owner.allocator, "{s}  ›  {s}  ›  {s}  ▾", .{ string(effect, "target_title") orelse "Polar", channelTitle(effect), string(effect, "thread_title") orelse "Thread" }, 0) catch return;
        defer self.owner.allocator.free(title);
        self.target.setLabel(title);
        self.target.as(gtk.Widget).setTooltipText(title);
        const hint = std.fmt.allocPrint(self.owner.allocator, "Message {s} · Chat target", .{string(effect, "thread_title") orelse "Thread"}) catch return;
        defer self.owner.allocator.free(hint);
        setLabel(self.owner.allocator, self.hint, hint);
        const status = string(effect, "status") orelse "";
        const connection = string(effect, "connection") orelse "offline";
        self.showStatus(if (status.len > 0) status else if (std.mem.eql(u8, connection, "live")) "Connected" else connection);
        self.send_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(effect, "can_send")));
        self.older.as(gtk.Widget).setSensitive(@intFromBool(boolean(effect, "can_load_older")));
        self.retry.as(gtk.Widget).setVisible(@intFromBool(boolean(effect, "can_reconnect")));

        self.container.as(gtk.Widget).setVisible(1);
    }
    fn inlineEditorLimit(self: *InputBar) c_int {
        const layout = self.text.as(gtk.Widget).createPangoLayout("Ag");
        defer layout.as(gobject.Object).unref();
        var height: c_int = 0;
        layout.getPixelSize(null,&height);
        return @max(30,height * 6 + self.text.getTopMargin() + self.text.getBottomMargin());
    }
    pub fn activate(self: *InputBar) void {
        self.expanded = true;
        self.editor_scroll.setMaxContentHeight(self.inlineEditorLimit());
        self.body.as(gtk.Widget).setVisible(1);
        self.send_button.as(gtk.Widget).setVisible(@intFromBool(!self.inquiry_active));
        self.mode_button.as(gtk.Widget).setVisible(@intFromBool(!self.is_idea and !self.inquiry_active));
        self.toggle.setLabel(if (self.is_idea) "▾ Idea" else "▾ Chat");
        self.container.as(gtk.Widget).setVisible(1);
        // Surface initialization and popover dismissal may restore native focus
        // after their callbacks. Complete this explicit activation once, after
        // those owners have finished; background projections never schedule it.
        if (self.focus_source == 0) self.focus_source = glib.idleAdd(finishActivation, self);
    }
    fn finishActivation(context: ?*anyopaque) callconv(.c) c_int {
        const self: *InputBar = @ptrCast(@alignCast(context orelse return 0));
        self.focus_source = 0;
        const pane=self.owner.panes.get(self.pane_id orelse return 0) orelse return 0;
        if (self.host.getActiveSurface()!=pane.display.surface) return 0;
        const focused=self.host.as(gtk.Window).getFocus() orelse return 0;
        if (focused!=self.text.as(gtk.Widget) and focused.isAncestor(self.text.as(gtk.Widget))==0 and focused!=pane.display.surface.as(gtk.Widget) and focused.isAncestor(pane.display.surface.as(gtk.Widget))==0) return 0;
        if (self.expanded and self.container.as(gtk.Widget).getVisible() != 0 and self.host.as(gtk.Window).isActive() != 0) {
            _ = self.text.as(gtk.Widget).grabFocus();
        }
        return 0;
    }
    pub fn clearSelection(self: *InputBar) void {
        if (self.focus_source!=0) {_ = glib.Source.remove(self.focus_source);self.focus_source=0;}
        self.inquiry_held=null;self.inquiry_deferred=false;self.inquiry_focus_pending=false;
        self.restoreInquiryEditor();self.inquiry_popover.popdown();self.inquiry_entry.as(gtk.Widget).setVisible(0);self.inquiry_active=false;self.dictate_history.bindInput(self.text);self.text.setEditable(1);
        self.setIdeaMode(false);
        self.updating = true;
        defer self.updating = false;
        if (self.pane_id) |id| self.owner.allocator.free(id);
        self.pane_id = null;
        if (self.metadata) |*old| old.deinit();
        self.metadata = null;
        self.text.getBuffer().setText("", 0);
        self.target.setLabel("Choose Channel / Thread…");
        setLabel(self.owner.allocator, self.hint, "Select a conversation");
        self.hint.as(gtk.Widget).setVisible(1);
        self.send_button.as(gtk.Widget).setSensitive(0);
        self.older.as(gtk.Widget).setSensitive(0);
        self.retry.as(gtk.Widget).setVisible(0);
        self.showStatus("Choose a conversation");
        self.container.as(gtk.Widget).setVisible(1);
    }
    const IdeaInput = struct {composer:*InputBar, allocator:std.mem.Allocator, bytes:[]u8};
    fn freeIdeaInput(data:?*anyopaque,_:*gobject.Object) callconv(.c) void {
        const b:*IdeaInput=@ptrCast(@alignCast(data orelse return));b.allocator.free(b.bytes);b.allocator.destroy(b);
    }
    fn sendIdeaInput(_: *gtk.Button,b:*IdeaInput) callconv(.c) void {
        const self=b.composer;
        // dispatch copies before effects can replace the owning menu buttons.
        self.idea_refs.popdown();
        self.owner.dispatch(b.bytes);self.owner.saveSoon();
    }
    fn ideaInputButton(self:*InputBar,box:*gtk.Box,label:[]const u8,value:anytype) void {
        const a=self.owner.allocator;
        const z=a.dupeZ(u8,label) catch return;defer a.free(z);
        const bytes=std.json.Stringify.valueAlloc(a,value,.{}) catch return;
        const data=a.create(IdeaInput) catch {a.free(bytes);return;};data.*=.{.composer=self,.allocator=a,.bytes=bytes};
        const button=gtk.Button.newWithLabel(z);
        if(button.getChild()) |child| if(gobject.ext.cast(gtk.Label,child)) |l| {l.setMaxWidthChars(60);l.setEllipsize(.middle);};
        button.as(gobject.Object).weakRef(freeIdeaInput,data);
        _=gtk.Button.signals.clicked.connect(button,*IdeaInput,sendIdeaInput,data,.{});box.append(button.as(gtk.Widget));
    }
    const NodeEditor = struct {
        composer: *InputBar,
        uuid: []u8,
        title: []u8,
        view_id: []u8,
        box: *gtk.Box,
        text: *gtk.TextView,
        syncing: bool = false,
        composing: bool = false,
        detail: bool,
        pub fn create(cview: *InputBar, uuid: []const u8, title: []const u8, detail: bool) !*NodeEditor {
            const a=cview.owner.allocator;
            const self=try a.create(NodeEditor);
            const box=gtk.Box.new(.vertical,4);
            _=box.as(gobject.Object).refSink();
            const heading=gtk.Box.new(.horizontal,4);
            const label_text=try std.fmt.allocPrintSentinel(a,"{s} · {s}",.{title,uuid},0);defer a.free(label_text);
            const label=gtk.Label.new(label_text);label.setSelectable(1);label.setXalign(0);label.as(gtk.Widget).setHexpand(1);
            label.setEllipsize(.middle);label.setMaxWidthChars(1);label.as(gtk.Widget).setTooltipText(label_text);
            // A UUID is one unbroken token to Pango's default WORD wrapping.
            // Permit character wrapping so the native heading cannot enlarge
            // the scene-owned editor rectangle to its token's minimum width.
            if(detail) {label.setWrap(1);label.setLines(2);label.setWrapMode(.word_char);label.as(gtk.Widget).addCssClass("instruction-heading");}
            heading.append(label.as(gtk.Widget));box.append(heading.as(gtk.Widget));
            const editor=gtk.TextView.new();editor.setWrapMode(.word_char);editor.getBuffer().setEnableUndo(1);
            editor.setTopMargin(6);editor.setBottomMargin(6);editor.setLeftMargin(8);editor.setRightMargin(8);
            editor.as(gtk.Widget).setTooltipText("Node instruction");box.append(editor.as(gtk.Widget));
            self.*=.{.composer=cview,.uuid=try a.dupe(u8,uuid),.title=try a.dupe(u8,title),.view_id=try a.dupe(u8,string(cview.metadata.?.value,"view_id") orelse ""),.box=box,.text=editor,.detail=detail};
            const node_actions=gtk.Box.new(.vertical,4);
            if(!detail) {
                const menu=gtk.MenuButton.new();menu.setLabel("⋯");menu.as(gtk.Widget).setTooltipText("Node actions");
                const popup=gtk.Popover.new();popup.setChild(node_actions.as(gtk.Widget));menu.setPopover(popup);heading.append(menu.as(gtk.Widget));
                const locate=gtk.Button.newWithLabel("Locate");node_actions.append(locate.as(gtk.Widget));
                _=gtk.Button.signals.clicked.connect(locate,*NodeEditor,locateClicked,self,.{});
            }
            const action=gtk.Button.newWithLabel(if(detail) "Review" else "Remove");
            if(detail) {
                action.as(gtk.Widget).setTooltipText("Review in Idea Bar");
                const actions=gtk.Box.new(.horizontal,4);
                actions.append(action.as(gtk.Widget));
                const close=gtk.Button.newWithLabel("Close");actions.append(close.as(gtk.Widget));
                _=gtk.Button.signals.clicked.connect(close,*NodeEditor,closeClicked,self,.{});
                box.append(actions.as(gtk.Widget));
            } else node_actions.append(action.as(gtk.Widget));
            _=gtk.Button.signals.clicked.connect(action,*NodeEditor,actionClicked,self,.{});
            _=gtk.TextBuffer.signals.changed.connect(editor.getBuffer(),*NodeEditor,edited,self,.{});
            _=gtk.TextView.signals.preedit_changed.connect(editor,*NodeEditor,preedit,self,.{});
            const focus=gtk.EventControllerFocus.new();
            _=gtk.EventControllerFocus.signals.enter.connect(focus,*NodeEditor,focused,self,.{});editor.as(gtk.Widget).addController(focus.as(gtk.EventController));
            const keys=gtk.EventControllerKey.new();keys.as(gtk.EventController).setPropagationPhase(.capture);
            _=gtk.EventControllerKey.signals.key_pressed.connect(keys,*NodeEditor,key,self,.{});
            editor.as(gtk.Widget).addController(keys.as(gtk.EventController));
            return self;
        }
        fn focused(_: *gtk.EventControllerFocus,self:*NodeEditor) callconv(.c) void {self.composer.last_editor=self.text;}
        fn locateClicked(_: *gtk.Button,self:*NodeEditor) callconv(.c) void {
            const cview=self.composer;const entry=cview.owner.panes.get(cview.pane_id orelse return) orelse return;
            const uuid=cview.owner.allocator.dupeZ(u8,self.uuid) catch return;defer cview.owner.allocator.free(uuid);
            if(entry.display.graphics_widget) |widget| graphics.command(widget,"locate_node",uuid);
        }
        pub fn destroy(self:*NodeEditor) void {
            const a=self.composer.owner.allocator;
            self.box.unref();a.free(self.uuid);a.free(self.title);a.free(self.view_id);a.destroy(self);
        }
        pub fn update(self:*NodeEditor,value:[]const u8) void {
            if(self.composing) return;
            const b=self.text.getBuffer();var start:gtk.TextIter=undefined;var end:gtk.TextIter=undefined;b.getBounds(&start,&end);
            const current=b.getText(&start,&end,1);defer glib.free(current);
            if(std.mem.eql(u8,std.mem.span(current),value)) return;
            self.syncing=true;defer self.syncing=false;
            const z=self.composer.owner.allocator.dupeZ(u8,value) catch return;defer self.composer.owner.allocator.free(z);
            b.setText(z,@intCast(value.len));
        }
        fn edited(buffer:*gtk.TextBuffer,self:*NodeEditor) callconv(.c) void {
            if(self.syncing or self.composing or self.composer.updating) return;
            self.composer.last_editor=self.text;
            var start:gtk.TextIter=undefined;var end:gtk.TextIter=undefined;buffer.getBounds(&start,&end);
            const value=buffer.getText(&start,&end,1);defer glib.free(value);
            const cview=self.composer;
            cview.owner.input(.{.type="idea_node_instruction",.window_id=cview.id,.view_id=self.view_id,.node_uuid=self.uuid,.title=self.title,.text=std.mem.span(value),.remove=false});
            cview.owner.saveSoon();
        }
        fn preedit(_: *gtk.TextView,value:[*:0]const u8,self:*NodeEditor) callconv(.c) void {
            self.composing=value[0]!=0;if(!self.composing) edited(self.text.getBuffer(),self);
        }
        fn key(_: *gtk.EventControllerKey,keyval:c_uint,_:c_uint,mods:gdk.ModifierType,self:*NodeEditor) callconv(.c) c_int {
            if(self.composing) return 0;
            if(self.detail and keyval==gdk.KEY_Escape) {self.composer.dismissDetail();return 1;}
            if((keyval==gdk.KEY_Return or keyval==gdk.KEY_KP_Enter) and mods.control_mask and !mods.shift_mask and !mods.alt_mask and !mods.super_mask) {
                if(self.detail) self.review() else sendClicked(self.composer.send_button,self.composer);
                return 1;
            }
            return 0;
        }
        fn review(self:*NodeEditor) void {
            const cview=self.composer;
            for(cview.node_editors.items) |editor| if(std.mem.eql(u8,editor.uuid,self.uuid)) {_=editor.text.as(gtk.Widget).grabFocus();break;};
            cview.dismissDetail();
        }
        fn closeClicked(_: *gtk.Button,self:*NodeEditor) callconv(.c) void {self.composer.dismissDetail();}
        fn actionClicked(_: *gtk.Button,self:*NodeEditor) callconv(.c) void {
            if(self.detail) {self.review();return;}
            const cview=self.composer;
            cview.owner.input(.{.type="idea_node_instruction",.window_id=cview.id,.view_id=self.view_id,.node_uuid=self.uuid,.title=self.title,.text="",.remove=true});
            cview.owner.saveSoon();
        }
    };
    pub fn closeDetail(self:*InputBar) void {
        if(self.detail_scroll) |scroll| {
            scroll.setChild(null);
            if(self.detail_overlay) |overlay| overlay.removeOverlay(scroll.as(gtk.Widget));
            self.detail_scroll=null;self.detail_overlay=null;
        }
        if(self.detail_css) |css| {
            gtk.StyleContext.removeProviderForDisplay(self.host.as(gtk.Widget).getDisplay(),css.as(gtk.StyleProvider));
            css.as(gobject.Object).unref();self.detail_css=null;self.detail_style_hash=0;
        }
        if(self.detail_editor) |editor| {if(self.last_editor==editor.text)self.last_editor=null;editor.destroy();self.detail_editor=null;}
    }
    fn dismissDetail(self:*InputBar) void {
        self.closeDetail();
        const entry=self.owner.panes.get(self.pane_id orelse return) orelse return;
        if(entry.display.graphics_widget) |widget| graphics.command(widget,"close_instruction","");
    }
    fn clearNodeEditors(self:*InputBar) void {
        // Context changes run before pane_id is replaced: release the OLD
        // scene's composition as well as its native editor. Internal rebuilds
        // use closeDetail directly and never send a scene command.
        if(self.detail_editor!=null) self.dismissDetail() else self.closeDetail();
        self.last_editor=null;
        for(self.node_editors.items) |editor| {self.idea_document.remove(editor.box.as(gtk.Widget));editor.destroy();}
        self.node_editors.deinit(self.owner.allocator);self.node_editors=.empty;
    }
    fn updateNodeEditors(self:*InputBar,effect:Value) void {
        const blocks=array(field(effect,"node_blocks"));
        var i:usize=0;
        while(i<self.node_editors.items.len) {
            const editor=self.node_editors.items[i];
            var exists=false;for(blocks) |block| {if(std.mem.eql(u8,string(block,"node_uuid") orelse "",editor.uuid)){exists=true;break;}}
            if(exists){i+=1;continue;}
            _=self.node_editors.orderedRemove(i);if(self.last_editor==editor.text)self.last_editor=null;
            self.idea_document.remove(editor.box.as(gtk.Widget));editor.destroy();
        }
        for(blocks) |block| {
            const uuid=string(block,"node_uuid") orelse continue;
            var existing:?*NodeEditor=null;for(self.node_editors.items) |editor| {if(std.mem.eql(u8,editor.uuid,uuid)){existing=editor;break;}}
            const editor=existing orelse blk:{
                const created=NodeEditor.create(self,uuid,string(block,"title") orelse "",false) catch continue;
                self.node_editors.append(self.owner.allocator,created) catch {created.destroy();continue;};
                self.idea_document.append(created.box.as(gtk.Widget));break :blk created;
            };
            editor.update(string(block,"text") orelse "");
        }
        if(self.detail_editor) |editor| {
            var value:[]const u8="";
            for(blocks) |block| {if(std.mem.eql(u8,string(block,"node_uuid") orelse "",editor.uuid)){value=string(block,"text") orelse "";break;}}
            editor.update(value);
        }
    }
    pub fn insertNode(self:*InputBar) void {
        if(!self.is_idea or self.composing) return;
        const metadata=self.metadata orelse return;
        const selected=field(metadata.value,"selected_node") orelse return;
        const uuid=string(selected,"node_uuid") orelse return;if(uuid.len==0)return;
        const a=self.owner.allocator;
        const value=std.fmt.allocPrintSentinel(a,"{s}（{s}）",.{string(selected,"title") orelse "",uuid},0) catch return;defer a.free(value);
        const editor=if(self.last_editor) |last| (if(last.as(gtk.Widget).getMapped()!=0) last else self.text) else self.text;
        const buffer=editor.getBuffer();buffer.beginUserAction();
        _=buffer.deleteSelection(1,1);buffer.insertAtCursor(value,@intCast(value.len));buffer.endUserAction();
        _=editor.as(gtk.Widget).grabFocus();
    }
    pub fn editInstruction(self:*InputBar,state:Value) void {
        if(!self.is_idea) return;
        const rect=field(state,"instruction_editor_rect") orelse return;
        if(rect!=.object) return;
        const metadata=self.metadata orelse return;
        const selected=field(metadata.value,"selected_node") orelse return;
        const uuid=string(selected,"node_uuid") orelse return;if(uuid.len==0)return;
        if(self.detail_editor) |existing| {
            if(std.mem.eql(u8,existing.uuid,uuid)) {self.updateDetailPresentation(state);_=existing.text.as(gtk.Widget).grabFocus();return;}
        }
        const entry=self.owner.panes.get(self.pane_id orelse return) orelse return;
        const overlay=entry.display.graphics_overlay orelse return;
        self.closeDetail();
        const editor=NodeEditor.create(self,uuid,string(selected,"title") orelse "",true) catch return;
        self.detail_editor=editor;
        const scroll=gtk.ScrolledWindow.new();self.detail_scroll=scroll;self.detail_overlay=overlay;
        scroll.setPolicy(.never,.automatic);scroll.setPropagateNaturalWidth(0);scroll.setPropagateNaturalHeight(0);
        const widget=scroll.as(gtk.Widget);widget.setHalign(.start);widget.setValign(.start);widget.setOverflow(.hidden);
        const name=std.fmt.allocPrintSentinel(self.owner.allocator,"station-instruction-{x}",.{@intFromPtr(self)},0) catch return;
        defer self.owner.allocator.free(name);widget.setName(name);
        scroll.setChild(editor.box.as(gtk.Widget));overlay.addOverlay(widget);overlay.setMeasureOverlay(widget,0);overlay.setClipOverlay(widget,1);
        const css=gtk.CssProvider.new();self.detail_css=css;
        gtk.StyleContext.addProviderForDisplay(self.host.as(gtk.Widget).getDisplay(),css.as(gtk.StyleProvider),gtk.STYLE_PROVIDER_PRIORITY_APPLICATION+10);
        self.updateNodeEditors(metadata.value);self.updateDetailPresentation(state);
        _=editor.text.as(gtk.Widget).grabFocus();self.last_editor=editor.text;
    }
    pub fn updateDetailPresentation(self:*InputBar,state:Value) void {
        const scroll=self.detail_scroll orelse return;
        const rect=field(state,"instruction_editor_rect") orelse {self.closeDetail();return;};
        if(rect!=.object or !boolean(state,"reader_open")) {self.closeDetail();return;}
        if(self.detail_editor) |editor| if(self.metadata) |metadata| if(field(metadata.value,"selected_node")) |selected| {
            if(!std.mem.eql(u8,string(selected,"node_uuid") orelse "",editor.uuid)) {self.editInstruction(state);return;}
        };
        const widget=scroll.as(gtk.Widget);
        // Godot owns the composite layout. Quantize only at GTK's integer
        // allocation boundary, with no independent popup/clamping algorithm.
        widget.setMarginStart(coordinate(rect,"x"));widget.setMarginTop(coordinate(rect,"y"));
        widget.setSizeRequest(coordinate(rect,"width"),coordinate(rect,"height"));
        const style=field(state,"instruction_style") orelse return;
        const css=self.detail_css orelse return;
        const id=@intFromPtr(self);
        const value=std.fmt.allocPrintSentinel(self.owner.allocator,
            "#station-instruction-{x} {{ background: {s}; color: {s}; border: 1px solid {s}; border-radius: 8px; padding: 8px; box-shadow: inset 0 1px rgba(255,255,255,0.10), 0 3px 8px rgba(0,0,0,0.25); }} " ++
            "#station-instruction-{x} .instruction-heading {{ color: {s}; font-size: 12px; }} " ++
            "#station-instruction-{x} textview, #station-instruction-{x} textview text {{ background: transparent; color: {s}; font-size: 14px; caret-color: {s}; }} " ++
            "#station-instruction-{x} textview text selection {{ background: {s}; }} " ++
            "#station-instruction-{x} button {{ background: transparent; background-image: none; color: {s}; border: none; box-shadow: none; min-height: 22px; padding: 2px 8px; font-size: 12px; }} " ++
            "#station-instruction-{x} button:hover {{ background: {s}; }}",
            .{id,themeColor(style,"card","#132731ee"),themeColor(style,"text","#e4eef5ff"),themeColor(style,"border","#6b8796ff"),
              id,themeColor(style,"muted","#a9bfcdff"),id,id,themeColor(style,"text","#e4eef5ff"),themeColor(style,"text","#e4eef5ff"),
              id,themeColor(style,"selection","#3c667fff"),id,themeColor(style,"text","#e4eef5ff"),id,themeColor(style,"selection","#3c667fff")},0) catch return;
        defer self.owner.allocator.free(value);
        const hash=std.hash.Wyhash.hash(0,value);
        if(hash!=self.detail_style_hash) {css.loadFromString(value);self.detail_style_hash=hash;}
    }
    fn copyIdeaDraft(_: *gtk.Button,self:*InputBar) callconv(.c) void {
        const metadata=self.metadata orelse return;
        const value=self.owner.allocator.dupeZ(u8,string(metadata.value,"document_text") orelse "") catch return;
        defer self.owner.allocator.free(value);
        self.container.as(gtk.Widget).getClipboard().setText(value);self.idea_menu.popdown();
    }
    fn attachReference(_: *gtk.Button,self:*InputBar) callconv(.c) void {
        self.insertNode();
    }
    fn updateReferences(self:*InputBar,effect:Value) void {
        const refs=array(field(effect,"references"));
        self.idea_refs.setLabel("Session");
        if(self.idea_refs.getChild()) |child| if(gobject.ext.cast(gtk.Label,child)) |l| {l.setMaxWidthChars(18);l.setEllipsize(.middle);};
        const box=gtk.Box.new(.vertical,4);margins(box.as(gtk.Widget));
        for(refs,0..) |ref,index| {
            const row=gtk.Box.new(.horizontal,4);box.append(row.as(gtk.Widget));
            self.ideaInputButton(row,string(ref,"title") orelse "Locate",.{.type="idea_reference",.window_id=self.id,.operation="locate",.index=index});
            self.ideaInputButton(row,"×",.{.type="idea_reference",.window_id=self.id,.operation="remove",.index=index});
        }
        if(boolean(effect,"can_check_receipt")) self.ideaInputButton(box,"Refresh sending status",.{.type="idea_check_receipt",.window_id=self.id});
        if(boolean(effect,"can_open_session")) self.ideaInputButton(box,"Show original session",.{.type="idea_show_session",.window_id=self.id});
        if(string(effect,"binding_id")!=null) self.ideaInputButton(box,"Unbind session",.{.type="idea_bind",.window_id=self.id,.binding_id=@as(?[]const u8,null)});
        const scroll=gtk.ScrolledWindow.new();scroll.setPolicy(.never,.automatic);scroll.setMaxContentHeight(320);scroll.setPropagateNaturalHeight(1);scroll.setChild(box.as(gtk.Widget));
        if(self.idea_refs.getPopover()) |popup| popup.setChild(scroll.as(gtk.Widget));
    }
    fn showIdeaSessions(self:*InputBar) void {
        const metadata=self.metadata orelse return;
        const pane=string(metadata.value,"pane_id") orelse return;
        const view=string(metadata.value,"view_id") orelse return;
        if(self.owner.choose_idea_session) |choose| choose(self.owner.context,self.host,pane,view);
    }
    const IdeaBinding = struct { composer: *InputBar, action: [:0]u8, query: [:0]u8, button: *gtk.Button, container:*gtk.Box };
    fn ideaPressed(_: *gtk.Button, binding: *IdeaBinding) callconv(.c) void {
        const self = binding.composer;
        self.idea_more.popdown();
        self.idea_theme.popdown();
        self.idea_menu.popdown();
        self.owner.input(.{ .type = "idea_action", .window_id = self.id, .action = binding.action, .query = binding.query });
    }
    fn ideaViewVisibility(_: *gobject.Object, _: *gobject.ParamSpec, self: *InputBar) callconv(.c) void {
        self.idea_view_scroll.setMaxContentHeight(@max(160, @min(640, @divTrunc(self.host.as(gtk.Widget).getHeight() * 6, 10))));
    }
    fn ideaSearchActivated(entry: *gtk.Entry, self: *InputBar) callconv(.c) void {
        self.owner.input(.{ .type = "idea_action", .window_id = self.id, .action = "search", .query = std.mem.span(entry.as(gtk.Editable).getText()) });
    }
    fn setIdeaMode(self: *InputBar, value: bool) void {
        if(self.is_idea and !value) self.clearNodeEditors();
        if(value) {self.restoreInquiryEditor();self.inquiry_popover.popdown();self.inquiry_entry.as(gtk.Widget).setVisible(0);self.inquiry_active=false;self.dictate_history.bindInput(self.text);self.text.setEditable(1);}
        if (value and self.focus_source != 0) { _ = glib.Source.remove(self.focus_source); self.focus_source = 0; }
        self.is_idea = value;
        self.chat_tools.as(gtk.Widget).setVisible(@intFromBool(!value and (if(self.metadata) |m| field(m.value,"generic_lease")==null else true)));
        if(!value) self.send_button.setLabel("Send");
        self.target.as(gtk.Widget).setSizeRequest(if(self.host.as(gtk.Widget).getWidth()<760) 76 else if(value) 140 else 200,-1);
        self.idea_attach.setLabel(if(self.host.as(gtk.Widget).getWidth()<760) "+" else "Insert node");
        if(value) {self.send_button.setLabel("Send");self.send_button.as(gtk.Widget).setTooltipText("Send · Ctrl+Enter");}
        if (!value) self.text.as(gtk.Widget).setTooltipText(if (self.owner.modified_send) "Ctrl+Enter sends; Enter inserts newline" else "Enter sends; Shift+Enter inserts newline");
        if (!value) {self.idea_more.popdown();self.idea_theme.popdown();if (self.idea_search_button.getPopover()) |popup| popup.popdown();}
        self.idea_row.as(gtk.Widget).setVisible(@intFromBool(value));
        for ([_]*gtk.Widget{self.idea_menu.as(gtk.Widget),self.idea_more.as(gtk.Widget),self.idea_theme.as(gtk.Widget)}) |widget| widget.setVisible(@intFromBool(value));
        self.toggle.as(gtk.Widget).setVisible(0);
        self.status_button.as(gtk.Widget).setVisible(0);
        self.idea_row.as(gtk.Widget).setVisible(0);
        self.editor_scroll.setMaxContentHeight(self.inlineEditorLimit());
        self.send_button.as(gtk.Widget).setVisible(@intFromBool(!self.inquiry_active));
        self.target.as(gtk.Widget).setVisible(1);
        self.idea_refs.as(gtk.Widget).setVisible(@intFromBool(value));self.idea_attach.as(gtk.Widget).setVisible(@intFromBool(value));
        self.more.as(gtk.Widget).setVisible(1);
        self.mode_button.as(gtk.Widget).setVisible(@intFromBool(!value and !self.inquiry_active));
        self.toggle.setLabel(if (value) (if (self.expanded) "▾ Idea" else "▸ Idea") else (if (self.expanded) "▾ Chat" else "▸ Chat"));
    }
    fn updateIdeaControls(self: *InputBar, effect: Value) void {
        for (self.idea_bindings.items) |binding| binding.button.as(gtk.Widget).setVisible(0);
        self.idea_configuration_menu.as(gtk.Widget).setVisible(0);
        for (array(field(effect, "controls"))) |control| {
            const action = string(control, "action") orelse continue;
            const group = string(control,"group") orelse "";
            if (group.len>0) {
                if (self.owner.allocator.dupeZ(u8,group)) |label| {
                    defer self.owner.allocator.free(label);self.idea_configuration_menu.setLabel(label);
                } else |_| {}
                self.idea_configuration_menu.as(gtk.Widget).setVisible(1);
            }
            const query = string(control, "query") orelse "";
            var found: ?*IdeaBinding = null;
            for (self.idea_bindings.items) |binding| if (std.mem.eql(u8, binding.action, action) and std.mem.eql(u8, binding.query, query)) { found = binding; break; };
            const binding = found orelse blk: {
                const label = self.owner.allocator.dupeZ(u8, string(control, "label") orelse action) catch continue;
                defer self.owner.allocator.free(label);
                const binding = self.owner.allocator.create(IdeaBinding) catch continue;
                const name = self.owner.allocator.dupeZ(u8, action) catch { self.owner.allocator.destroy(binding); continue; };
                const argument = self.owner.allocator.dupeZ(u8, query) catch { self.owner.allocator.free(name); self.owner.allocator.destroy(binding); continue; };
                const button = gtk.Button.newWithLabel(if(std.mem.eql(u8,action,"back")) "‹" else if(std.mem.eql(u8,action,"forward")) "›" else label);
                button.as(gtk.Widget).setTooltipText(label);
                const container=if(group.len>0) self.idea_configuration else if(std.mem.eql(u8,action,"open_files")) self.idea_files else if (std.mem.startsWith(u8,action,"theme_") or std.mem.eql(u8,action,"select_theme")) self.idea_themes else self.idea_overflow;
                binding.* = .{ .composer = self, .action = name, .query = argument, .button = button, .container=container };
                self.idea_bindings.append(self.owner.allocator, binding) catch { self.owner.allocator.free(name); self.owner.allocator.free(argument); self.owner.allocator.destroy(binding); continue; };
                if(std.mem.eql(u8,action,"open_files")) container.prepend(button.as(gtk.Widget)) else container.append(button.as(gtk.Widget));
                _ = gtk.Button.signals.clicked.connect(button, *IdeaBinding, ideaPressed, binding, .{});
                break :blk binding;
            };
            if (self.owner.allocator.dupeZ(u8, string(control, "label") orelse action)) |label| {
                defer self.owner.allocator.free(label);
                binding.button.setLabel(if (std.mem.eql(u8, action, "back")) "‹" else if (std.mem.eql(u8, action, "forward")) "›" else label);
                const tip=string(control,"tooltip") orelse "";
                if (self.owner.allocator.dupeZ(u8, if(tip.len>0) tip else label)) |tooltip| {
                    defer self.owner.allocator.free(tooltip);binding.button.as(gtk.Widget).setTooltipText(tooltip);
                } else |_| {}
            } else |_| {}
            binding.button.as(gtk.Widget).setVisible(1);
            binding.button.as(gtk.Widget).setSensitive(@intFromBool(boolean(control, "enabled")));
            if (boolean(control, "active")) binding.button.as(gtk.Widget).addCssClass("suggested-action")
            else binding.button.as(gtk.Widget).removeCssClass("suggested-action");
        }

        var stale_index = self.idea_bindings.items.len;
        while (stale_index > 0) {
            stale_index -= 1;
            const binding = self.idea_bindings.items[stale_index];
            if (binding.button.as(gtk.Widget).getVisible() != 0) continue;
            binding.container.remove(binding.button.as(gtk.Widget));
            _ = self.idea_bindings.swapRemove(stale_index);
            self.owner.allocator.free(binding.action);self.owner.allocator.free(binding.query);self.owner.allocator.destroy(binding);
        }
        self.idea_search.as(gtk.Widget).setSensitive(@intFromBool(boolean(effect, "can_search")));
        self.idea_search_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(effect,"can_search")));
    }
    fn channelTitle(effect: Value) []const u8 {
        const name = string(effect, "channel_title") orelse return "Channel";
        return if (name.len == 0) "Channel" else name;
    }
    pub fn showStatus(self: *InputBar, message: []const u8) void {
        setLabel(self.owner.allocator, self.status, message);
        self.status.as(gtk.Widget).setVisible(@intFromBool(message.len > 0));
        const tooltip = self.owner.allocator.dupeZ(u8, message) catch return;
        defer self.owner.allocator.free(tooltip);
        self.status_button.as(gtk.Widget).setTooltipText(tooltip);
        self.status_button.setLabel("●");
        self.status_button.as(gtk.Widget).setVisible(0);
        if (self.status_button.getChild()) |child| if (gobject.ext.cast(gtk.Label, child)) |label| {
            label.setMaxWidthChars(24); label.setEllipsize(.end);
        };
    }
    pub fn setSendMode(self: *InputBar, modified: bool) void {
        self.updating = true;
        defer self.updating = false;
        self.modified.setActive(@intFromBool(modified));
        // Settings fanout updates Chat preference, not Idea/Inquiry actions.
        if (!self.is_idea and !self.inquiry_active) self.text.as(gtk.Widget).setTooltipText(if (modified) "Ctrl+Enter sends; Enter inserts newline" else "Enter sends; Shift+Enter inserts newline");
    }
    fn toggleClicked(_: *gtk.Button, self: *InputBar) callconv(.c) void {
        self.expanded = !self.expanded;
        self.editor_scroll.setMaxContentHeight(if (self.expanded) 144 else self.inlineEditorLimit());
        self.toggle.setLabel(if (self.is_idea) (if (self.expanded) "▾ Idea" else "▸ Idea") else (if (self.expanded) "▾ Chat" else "▸ Chat"));
        if (self.expanded) self.activate();
    }
    pub fn chooseClicked(_: *gtk.Button, self: *InputBar) callconv(.c) void {
        if (self.is_idea) {self.showIdeaSessions();return;}
        if (self.picker == null) self.picker = Picker.create(self) catch return;
        self.picker.?.present();
    }
    fn dictateWindowActive(_: *gobject.Object,_: *gobject.ParamSpec,self:*InputBar) callconv(.c) void {if(self.host.as(gtk.Window).isActive()==0 and self.dictate_chord.reset()==.release)self.owner.dictateLocalShortcut(false);if(self.dictate_panel)|panel|panel.refresh();self.owner.dictate.updateRecordingNotification();}
    fn dictateMapped(_: *gtk.Widget,self:*InputBar) callconv(.c) void {if(self.dictate_panel)|panel|panel.refresh();}
    pub fn refreshDictate(self:*InputBar) void {
        if(self.dictate_panel==null) self.dictate_panel=@import("dictate_panel.zig").Panel.create(self.owner.dictate,self.dictate_button.as(gtk.Widget)) catch null;
        if(self.dictate_panel) |panel| panel.refresh();
        const state = self.owner.dictate;
        const presentation = state.presentation();
        const modes=dictate_module.field(state.view(),"refine_modes");
        const english=if(self.pane_id) |id| dictate_module.boolean(modes,id) else false;
        if(english)self.refine_button.setLabel("EN") else self.refine_button.setIconName("document-edit-symbolic");
        self.refine_button.as(gtk.Widget).setTooltipText(if(english) "Refine + English — hold to choose mode" else "Refine — hold to choose mode");
        self.refine_button.as(gtk.Widget).setSensitive(@intFromBool(state.enabled() and std.mem.eql(u8,dictate_module.string(presentation,"action"),"start")));
        self.dictate_button.as(gtk.Widget).setSensitive(@intFromBool(state.enabled()));
        const microphone_state=dictate_module.string(state.view(),"microphone_state");
        self.dictate_button.setIconName(if (std.mem.eql(u8,microphone_state,"recording")) "media-playback-stop-symbolic" else if(std.mem.eql(u8,microphone_state,"unavailable")) "microphone-sensitivity-muted-symbolic" else "audio-input-microphone-symbolic");
        if(std.mem.eql(u8,microphone_state,"recording"))self.dictate_button.as(gtk.Widget).addCssClass("destructive-action") else self.dictate_button.as(gtk.Widget).removeCssClass("destructive-action");
        const tooltip = std.fmt.allocPrintSentinel(self.owner.allocator,"{s} · {s} · Hold to choose mode · {s}",.{dictate_module.string(presentation,"button_label"),dictate_module.string(state.view(),"output_mode_label"),state.status()},0) catch return;
        defer self.owner.allocator.free(tooltip); self.dictate_button.as(gtk.Widget).setTooltipText(tooltip);
        const history_title = self.owner.allocator.dupeZ(u8,dictate_module.string(presentation,"history_title")) catch return;
        defer self.owner.allocator.free(history_title); self.dictate_menu.as(gtk.Widget).setTooltipText(history_title);
        if (self.dictate_history.popup.as(gtk.Widget).getVisible()!=0) self.dictate_history.refresh();
    }
    pub fn dictateClicked(_: *gtk.Button,self:*InputBar) callconv(.c) void {
        const state=self.owner.dictate;
        if(std.mem.eql(u8,dictate_module.string(state.presentation(),"action"),"stop")){state.toggle(null);return;}
        if(self.composing)return;
        if(self.host.as(gtk.Window).getFocus()) |focus| {
            if(gobject.ext.cast(gtk.TextView,focus))|editor|{state.toggle(editor);return;}
            // Unsupported editable fields (including password entries) must not
            // fall through into an unrelated composer.
            if(gobject.ext.cast(gtk.Editable,focus)!=null){state.toggle(null);return;}
            if(self.is_idea)if(self.pane_id)|pane|if(self.owner.panes.get(pane))|entry|{
                if(focus==entry.display.surface.as(gtk.Widget) or (entry.display.graphics_widget != null and (focus==entry.display.graphics_widget.? or focus.isAncestor(entry.display.graphics_widget.?) != 0))){
                    if(self.captureDictateNode())|target|{state.startNode(target.port,target.context);return;}
                }
            };
        }
        const reference=if(self.pane_id) |pane| (if(self.owner.terminal_selection) |read| read(self.owner.context,self.owner.allocator,self.host,pane) else null) else null;
        defer if(reference) |text| self.owner.allocator.free(text);
        const editor=if(self.inquiry_active) self.inquiry_text else self.text;
        _=editor.as(gtk.Widget).grabFocus();state.toggleQuoted(editor,reference orelse "");
    }
    const DictateNode = struct {
        owner:*Workspace,
        record:std.json.Parsed(Value),
        fn release(context:*anyopaque) void {
            const self:*DictateNode=@ptrCast(@alignCast(context));self.record.deinit();self.owner.allocator.destroy(self);
        }
        fn insert(context:*anyopaque,text:[]const u8) dictate_module.Controller.Delivery {
            const self:*DictateNode=@ptrCast(@alignCast(context));const saved=self.record.value;
            const composer=self.owner.composers.get(dictate_module.string(saved,"window")) orelse return .unchanged;
            const meta=if(composer.metadata)|m|m.value else return .unchanged;
            if(!std.mem.eql(u8,composer.pane_id orelse "",dictate_module.string(saved,"pane")) or
                !std.mem.eql(u8,string(meta,"view_id") orelse "",dictate_module.string(saved,"view")) or
                number(meta,"draft_revision") != number(saved,"revision"))return .unchanged;
            const uuid=dictate_module.string(saved,"node");
            if(!std.mem.eql(u8,string(field(meta,"selected_node") orelse .null,"node_uuid") orelse "",uuid))return .unchanged;
            const before=dictate_module.string(saved,"text");
            const combined=std.fmt.allocPrint(self.owner.allocator,"{s}{s}{s}",.{before,if(before.len==0 or std.mem.endsWith(u8,before,"\n")) "" else "\n",text}) catch return .unchanged;
            defer self.owner.allocator.free(combined);
            self.owner.input(.{.type="idea_node_instruction",.window_id=composer.id,.view_id=dictate_module.string(saved,"view"),.node_uuid=uuid,.title=dictate_module.string(saved,"title"),.text=combined,.remove=false});
            self.owner.saveSoon();
            for(composer.node_editors.items)|node|if(std.mem.eql(u8,node.uuid,uuid)){
                const buffer=node.text.getBuffer();
                var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;buffer.getBounds(&first,&last);
                const current=buffer.getText(&first,&last,1);defer glib.free(current);
                if(!std.mem.eql(u8,std.mem.span(current),combined))return .unknown;
                // Seed the existing editor's undo history without publishing an
                // intermediate draft. Undo later uses its normal onChange path.
                const old=self.owner.allocator.dupeZ(u8,before) catch return .applied;defer self.owner.allocator.free(old);
                const addition=self.owner.allocator.dupeZ(u8,combined[before.len..]) catch return .applied;defer self.owner.allocator.free(addition);
                node.syncing=true;defer node.syncing=false;
                buffer.setText(old,@intCast(before.len));buffer.getEndIter(&last);buffer.placeCursor(&last);
                buffer.beginUserAction();buffer.insertAtCursor(addition,@intCast(addition.len));buffer.endUserAction();
                return .applied;
            };
            return .unknown;
        }
    };
    pub fn captureDictateNode(self:*InputBar) ?struct{port:dictate_module.Controller.NativeTarget,context:[]const u8} {
        const meta=if(self.metadata)|m|m.value else return null;
        const node=field(meta,"selected_node") orelse return null;const uuid=string(node,"node_uuid") orelse return null;
        if(uuid.len==0)return null;
        var original:[]const u8="";for(array(field(meta,"node_blocks")))|block|{
            if(std.mem.eql(u8,string(block,"node_uuid") orelse "",uuid)){original=string(block,"text") orelse "";break;}
        }
        const bytes=std.json.Stringify.valueAlloc(self.owner.allocator,.{.window=self.id,.pane=self.pane_id,.view=string(meta,"view_id") orelse "",.node=uuid,.title=string(node,"title") orelse "",.text=original,.revision=number(meta,"draft_revision")},.{}) catch return null;
        defer self.owner.allocator.free(bytes);
        const record=std.json.parseFromSlice(Value,self.owner.allocator,bytes,.{.allocate=.alloc_always}) catch return null;
        const target=self.owner.allocator.create(DictateNode) catch {var r=record;r.deinit();return null;};target.*=.{.owner=self.owner,.record=record};
        const context=dictate_module.string(record.value,"title");var end=@min(4096,context.len);
        while(end>0 and !std.unicode.utf8ValidateSlice(context[0..end]))end-=1;
        return .{.port=.{.context=target,.insert=DictateNode.insert,.release=DictateNode.release},.context=context[0..end]};
    }
    fn mountDictateFocus(self:*InputBar) void {
        const click=gtk.GestureClick.new();self.dictate_click=click;
        click.as(gtk.EventController).setPropagationPhase(.capture);
        _=gtk.GestureClick.signals.released.connect(click,*InputBar,dictateEditorClick,self,.{});
        self.host.as(gtk.Widget).addController(click.as(gtk.EventController));
        const keys=gtk.EventControllerKey.new();self.dictate_keys=keys;
        keys.as(gtk.EventController).setPropagationPhase(.capture);
        _=gtk.EventControllerKey.signals.key_pressed.connect(keys,*InputBar,dictateKeyPressed,self,.{});
        _=gtk.EventControllerKey.signals.key_released.connect(keys,*InputBar,dictateKeyReleased,self,.{});
        self.host.as(gtk.Widget).addController(keys.as(gtk.EventController));
    }
    fn dictateEditorClick(_: *gtk.GestureClick,_:c_int,x:f64,y:f64,self:*InputBar) callconv(.c) void {
        const focus=self.host.as(gtk.Window).getFocus() orelse return;
        const editor=gobject.ext.cast(gtk.TextView,focus) orelse return;
        const hit=self.host.as(gtk.Widget).pick(x,y,.{}) orelse return;
        if(hit==editor.as(gtk.Widget) or hit.isAncestor(editor.as(gtk.Widget))!=0)self.owner.dictate.focus(editor);
    }
    fn localDictateKey(key:c_uint) @import("dictate_local_keys.zig").Key {
        return switch(key) {
            gdk.KEY_Control_L,gdk.KEY_Control_R=>.control,
            gdk.KEY_Super_L,gdk.KEY_Super_R=>.super,
            gdk.KEY_Shift_L,gdk.KEY_Shift_R,gdk.KEY_Alt_L,gdk.KEY_Alt_R=>.other_modifier,
            else=>.other,
        };
    }
    fn dictateKeyPressed(_: *gtk.EventControllerKey,key:c_uint,_:c_uint,mods:gdk.ModifierType,self:*InputBar) callconv(.c) c_int {
        const edge=self.dictate_chord.press(localDictateKey(key),mods.control_mask,mods.super_mask,mods.shift_mask or mods.alt_mask);
        if(edge==.press){self.owner.dictateLocalShortcut(true);return 1;}
        if(edge==.cancel){
            if(self.owner.dictate.shortcut_owner==2)self.owner.dictate.cancel();
            self.owner.dictateLocalShortcut(false);
        }
        if(key==gdk.KEY_Escape and dictate_module.string(self.owner.dictate.presentation(),"cancel_id").len>0){self.owner.dictate.cancel();return 1;}return 0;
    }
    fn dictateKeyReleased(_: *gtk.EventControllerKey,key:c_uint,_:c_uint,_:gdk.ModifierType,self:*InputBar) callconv(.c) void {
        if(self.dictate_chord.release(localDictateKey(key))==.release)self.owner.dictateLocalShortcut(false);
        if(key!=gdk.KEY_Tab and key!=gdk.KEY_ISO_Left_Tab)return;
        const focus=self.host.as(gtk.Window).getFocus() orelse return;
        if(gobject.ext.cast(gtk.TextView,focus))|editor|self.owner.dictate.focus(editor);
    }
    fn generalFocused(_: *gtk.EventControllerFocus,self:*InputBar) callconv(.c) void {self.last_editor=self.text;}
    fn dictateHeld(gesture:*gtk.GestureLongPress,_:f64,_:f64,self:*InputBar) callconv(.c) void {
        _=gesture.as(gtk.Gesture).setState(.claimed);
        if(self.dictate_mode_popup)|old|{old.as(gtk.Widget).unparent();self.dictate_mode_popup=null;}
        const view=self.owner.dictate.view();const choices=dictate_module.field(view,"output_modes");if(choices!=.array)return;
        const popup=gtk.Popover.new();popup.as(gtk.Widget).setParent(self.dictate_button.as(gtk.Widget));
        const box=gtk.Box.new(.vertical,4);
        for(choices.array.items,0..) |choice,index| {
            const title=std.fmt.allocPrintSentinel(self.owner.allocator,"{s}{s}",.{if(std.mem.eql(u8,dictate_module.string(view,"output_mode"),dictate_module.string(choice,"mode"))) "✓ " else "",dictate_module.string(choice,"label")},0) catch continue;
            defer self.owner.allocator.free(title);
            const button=gtk.Button.newWithLabel(title);button.as(gobject.Object).setData("dictate-output-choice",@ptrFromInt(index+1));
            _=gtk.Button.signals.clicked.connect(button,*InputBar,dictateModeSelected,self,.{});box.append(button.as(gtk.Widget));
        }
        popup.setChild(box.as(gtk.Widget));self.dictate_mode_popup=popup;popup.popup();
    }
    fn dictateModeSelected(button:*gtk.Button,self:*InputBar) callconv(.c) void {
        const ptr=button.as(gobject.Object).getData("dictate-output-choice") orelse return;const index=@intFromPtr(ptr)-1;
        const choices=dictate_module.field(self.owner.dictate.view(),"output_modes");if(choices!=.array or index>=choices.array.items.len)return;
        self.owner.dictate.input(.{.type="set_output_mode",.mode=dictate_module.string(choices.array.items[index],"mode")});
        if(self.dictate_mode_popup)|popup|popup.popdown();
    }
    fn refineHeld(gesture:*gtk.GestureLongPress,_:f64,_:f64,self:*InputBar) callconv(.c) void {
        if(self.pane_id==null)return;
        _=gesture.as(gtk.Gesture).setState(.claimed);
        if(self.refine_mode_popup==null){
            const popup=gtk.Popover.new();popup.as(gtk.Widget).setParent(self.refine_button.as(gtk.Widget));
            const box=gtk.Box.new(.vertical,4);
            for([_][:0]const u8{"Refine","Refine + English"},0..) |title,index| {
                const button=gtk.Button.newWithLabel(title);button.as(gobject.Object).setData("english-mode",if(index==1) @ptrFromInt(1) else null);
                _=gtk.Button.signals.clicked.connect(button,*InputBar,refineModeSelected,self,.{});box.append(button.as(gtk.Widget));
            }
            popup.setChild(box.as(gtk.Widget));self.refine_mode_popup=popup;
        }
        self.refine_mode_popup.?.popup();
    }
    fn refineModeSelected(button:*gtk.Button,self:*InputBar) callconv(.c) void {
        const context=self.pane_id orelse return;
        self.owner.dictate.input(.{.type="set_refine_mode",.context_id=context,.english=button.as(gobject.Object).getData("english-mode")!=null});
        if(self.refine_mode_popup) |popup| popup.popdown();
    }
    fn refineClicked(_: *gtk.Button, self: *InputBar) callconv(.c) void {
        if (self.composing or self.pane_id == null) return;
        const editor = if (self.is_idea) self.last_editor orelse self.text else if(self.inquiry_active) self.inquiry_text else self.text;
        _ = editor.as(gtk.Widget).grabFocus();
        const lease=if(self.metadata) |meta| field(meta.value,"generic_lease") else null;
        const project=if(self.metadata) |meta| field(meta.value,"project") else null;
        self.owner.dictate.refine(editor,self.pane_id orelse "",lease,project);
    }
    fn editBegin(_: *gtk.TextBuffer, self: *InputBar) callconv(.c) void { self.edit_action_depth += 1; }
    fn editEnd(buffer: *gtk.TextBuffer, self: *InputBar) callconv(.c) void {
        self.edit_action_depth -|= 1;
        if (self.edit_action_depth == 0) changed(buffer,self);
    }
    fn changed(_: *gtk.TextBuffer, self: *InputBar) callconv(.c) void {
        if(!self.updating) self.last_editor=self.text;
        if (self.updating or self.composing or self.edit_action_depth > 0) return;
        const buffer = self.text.getBuffer();
        var start: gtk.TextIter = undefined;
        var end: gtk.TextIter = undefined;
        buffer.getBounds(&start, &end);
        const text = buffer.getText(&start, &end, 1);
        defer glib.free(text);
        self.hint.as(gtk.Widget).setVisible(@intFromBool(text[0] == 0));
        if (!self.inquiry_active)
            self.editor_scroll.setMaxContentHeight(self.inlineEditorLimit());
        // Presentation follows the buffer even while sending is unavailable.
        if (self.pane_id == null) return;
        const lease = if (self.metadata) |metadata| field(metadata.value,"generic_lease") else null;
        if (lease) |value| {
            self.owner.input(.{ .type="edit_input", .lease=value, .text=std.mem.span(text) });
        } else {
            self.owner.input(.{ .type = "draft", .window_id = self.id, .text = std.mem.span(text) });
        }
        self.owner.saveSoon();
    }
    pub fn commitDraft(self: *InputBar) void {
        if(self.inquiry_active) {inquiryEdited(self.inquiry_text.getBuffer(),self);return;}
        if (self.pane_id == null) return;
        // End native composition while the old scope still owns the editor.
        if (self.composing) self.text.resetImContext();
        self.composing = false;
        changed(self.text.getBuffer(), self);
    }
    fn preeditChanged(_: *gtk.TextView, preedit: [*:0]const u8, self: *InputBar) callconv(.c) void {
        self.composing = preedit[0] != 0;
        self.inquiryEnvironment();
        self.hint.as(gtk.Widget).setVisible(0);
        // Fcitx/Rime can commit into TextBuffer before clearing preedit. Its
        // changed signal was intentionally ignored while composing; publish
        // the committed buffer now so Send is ready without another edit.
        if (!self.composing) changed(self.text.getBuffer(), self);
    }
    fn keyPressed(keys: *gtk.EventControllerKey, key: c_uint, _: c_uint, mods: gdk.ModifierType, self: *InputBar) callconv(.c) c_int {
        if(self.inquiry_active and key==gdk.KEY_Escape and !self.composing) {self.inquiryCommand("later",null);return 1;}
        if(key==gdk.KEY_Escape and !self.composing) {
            if(self.host.getActiveSurface()) |surface| { _ = surface.as(gtk.Widget).grabFocus(); }
            return 1;
        }
        if (key != gdk.KEY_Return and key != gdk.KEY_KP_Enter) return 0;
        const composing = self.composing;
        const editor=if(self.inquiry_active) self.inquiry_text else self.text;
        if (keys.as(gtk.EventController).getCurrentEvent()) |event| if (editor.imContextFilterKeypress(event) != 0) return 1;
        if (composing) return 1;
        if(self.inquiry_active) {if(!mods.control_mask or mods.shift_mask or mods.alt_mask or mods.super_mask) return 0;if(self.inquiry_held!=key){self.inquiry_held=key;sendClicked(self.send_button,self);}return 1;}
        if (mods.shift_mask or mods.alt_mask or mods.super_mask) return 0;
        if ((self.is_idea or self.owner.modified_send) and !mods.control_mask) return 0;
        if (self.send_button.as(gtk.Widget).getSensitive() != 0) sendClicked(self.send_button, self);
        return 1;
    }
    fn modeChanged(_: *gtk.CheckButton, self: *InputBar) callconv(.c) void {
        if (self.updating) return;
        const modified = self.modified.getActive() != 0;
        self.owner.setSendMode(modified);
        if (self.owner.save_mode) |save| save(self.owner.context, modified);
    }
    fn sendClicked(_: *gtk.Button, self: *InputBar) callconv(.c) void {
        if (self.composing or self.send_button.as(gtk.Widget).getSensitive() == 0) return;
        for(self.node_editors.items) |editor| if(editor.composing)return;
        if(self.detail_editor) |editor| if(editor.composing)return;
        self.commitDraft();
        const lease = if (self.metadata) |metadata| field(metadata.value,"generic_lease") else null;
        if (lease) |value| {
            self.owner.input(.{ .type="submit_input", .lease=value, .press_enter=true });
        } else {
            self.owner.input(.{ .type = "send", .window_id = self.id, .created_at_secs = @as(u64, @intCast(@max(1, std.time.timestamp()))) });
        }
    }
    pub fn inquiryEnvironment(self:*InputBar) void {
        self.owner.input(.{.type="inquiry_environment",.window_id=self.id,.foreground=self.host.as(gtk.Window).isActive()!=0,.composing=self.composing});
    }
    fn inquiryEntry(_: *gtk.Button,self:*InputBar) callconv(.c) void {
        self.tools.popdown();
        self.commitDraft();self.inquiryEnvironment();self.owner.input(.{.type="inquiry_action",.window_id=self.id,.action="inbox"});
    }
    fn inquiryCommand(self:*InputBar,action:[]const u8,value:?[]const u8) void {
        const data=self.inquiry_data orelse return;
        self.owner.input(.{.type="inquiry_action",.window_id=self.id,.action=action,.token=string(data.value,"token") orelse "",.value=value});
    }
    const InquiryBinding=struct {composer:*InputBar,allocator:std.mem.Allocator,token:[]u8,value:?[]u8,action:[]const u8};
    fn inquiryBindingFreed(data:?*anyopaque,_:*gobject.Object) callconv(.c) void {
        const b:*InquiryBinding=@ptrCast(@alignCast(data orelse return));const a=b.allocator;
        a.free(b.token);if(b.value) |v| a.free(v);a.destroy(b);
    }
    fn inquiryChosen(_: *gtk.Button,b:*InquiryBinding) callconv(.c) void {
        b.composer.owner.input(.{.type="inquiry_action",.window_id=b.composer.id,.action=b.action,.token=b.token,.value=b.value});
    }
    fn inquiryButton(self:*InputBar,label:[]const u8,action:[]const u8,value:?[]const u8,enabled:bool) ?*gtk.Button {
        const a=self.owner.allocator;const z=a.dupeZ(u8,label) catch return null;defer a.free(z);
        const data=self.inquiry_data orelse return null;
        const b=a.create(InquiryBinding) catch return null;
        const token=a.dupe(u8,string(data.value,"token") orelse "") catch {a.destroy(b);return null;};
        const owned=if(value) |v| a.dupe(u8,v) catch {a.free(token);a.destroy(b);return null;} else null;
        b.*=.{.composer=self,.allocator=a,.token=token,.value=owned,.action=action};
        const button=gtk.Button.newWithLabel(z);button.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        if(button.getChild()) |child| if(gobject.ext.cast(gtk.Label,child)) |label_widget| {label_widget.setWrap(1);label_widget.setMaxWidthChars(48);};
        button.as(gobject.Object).weakRef(inquiryBindingFreed,b);
        _ = gtk.Button.signals.clicked.connect(button,*InquiryBinding,inquiryChosen,b,.{});
        self.inquiry_buttons.append(a,button) catch {};
        return button;
    }
    pub fn setInquiryText(self:*InputBar,value:[]const u8) void {
        if(self.composing) return;
        const old=self.updating;self.updating=true;defer self.updating=old;
        const buffer=self.inquiry_text.getBuffer();
        var start:gtk.TextIter=undefined;var end:gtk.TextIter=undefined;buffer.getBounds(&start,&end);
        const current=buffer.getText(&start,&end,1);defer glib.free(current);
        if(std.mem.eql(u8,std.mem.span(current),value)) return;
        const z=self.owner.allocator.dupeZ(u8,value) catch return;defer self.owner.allocator.free(z);
        buffer.setText(z,@intCast(value.len));
    }
    fn inquiryPreedit(_: *gtk.TextView, preedit: [*:0]const u8,self:*InputBar) callconv(.c) void {
        self.composing=preedit[0]!=0;
        if(!self.composing) inquiryEdited(self.inquiry_text.getBuffer(),self);
    }
    fn inquiryEdited(buffer:*gtk.TextBuffer,self:*InputBar) callconv(.c) void {
        if(self.updating or self.composing or !self.inquiry_active) return;
        var start:gtk.TextIter=undefined;var end:gtk.TextIter=undefined;buffer.getBounds(&start,&end);
        const text=buffer.getText(&start,&end,1);defer glib.free(text);
        self.owner.input(.{.type="draft",.window_id=self.id,.text=std.mem.span(text)});self.owner.saveSoon();
    }
    fn restoreInquiryEditor(self:*InputBar) void {
        if(self.inquiry_answer_scroll) |scroll| scroll.setChild(null);
        self.inquiry_answer_scroll=null;
    }
    fn inquiryLabel(self:*InputBar,text:[]const u8,dim:bool) *gtk.Label {
        const label=gtk.Label.new(null);setLabel(self.owner.allocator,label,text);
        label.setWrap(1);label.setXalign(0);label.setMaxWidthChars(64);
        label.as(gtk.Widget).setVisible(@intFromBool(text.len>0));
        if(dim) label.as(gtk.Widget).addCssClass("dim-label");
        return label;
    }
    pub fn updateInquiry(self:*InputBar,value:Value,focus:bool) void {
        const encoded=std.json.Stringify.valueAlloc(self.owner.allocator,value,.{}) catch return;defer self.owner.allocator.free(encoded);
        const parsed=std.json.parseFromSlice(Value,self.owner.allocator,encoded,.{.allocate=.alloc_always}) catch return;
        if(self.inquiry_data) |*old| old.deinit();self.inquiry_data=parsed;
        if(self.inquiry_held!=null) {self.inquiry_deferred=true;self.inquiry_focus_pending=self.inquiry_focus_pending or focus;return;}
        self.inquiry_active=boolean(value,"active");
        self.dictate_history.bindInput(if(self.inquiry_active) self.inquiry_text else self.text);
        self.send_button.as(gtk.Widget).setVisible(@intFromBool(!self.inquiry_active));
        self.mode_button.as(gtk.Widget).setVisible(@intFromBool(!self.inquiry_active));
        const count=number(value,"count");self.inquiry_entry.as(gtk.Widget).setVisible(@intFromBool(count>0 or self.inquiry_active or boolean(value,"has_history")));
        const entry=self.owner.allocator.dupeZ(u8,string(value,"entry_title") orelse "Inquiry") catch return;
        defer self.owner.allocator.free(entry);self.inquiry_entry.setLabel(entry);
        self.text.setEditable(@intFromBool(!self.inquiry_active));
        self.inquiry_text.setEditable(@intFromBool(self.inquiry_active and boolean(value,"editable")));
        if(!self.inquiry_active) self.setSendMode(self.owner.modified_send);
        const stage=string(value,"stage") orelse "badge";
        if(std.mem.eql(u8,stage,"badge") or self.host.as(gtk.Window).isActive()==0) {
            self.restoreInquiryEditor();
            if(self.inquiry_focus_source!=0){_ = glib.Source.remove(self.inquiry_focus_source);self.inquiry_focus_source=0;}
            self.inquiry_focus_pending=false;self.inquiry_popover.popdown();return;
        }
        const signature=std.json.Stringify.valueAlloc(self.owner.allocator,.{.stage=stage,.token=string(value,"token"),.title=string(value,"title"),.question=string(value,"question"),.choices=field(value,"choices"),.status=string(value,"status"),.editable=boolean(value,"editable"),.invitations=field(value,"invitations"),.history=boolean(value,"history")},.{}) catch return;
        const same=if(self.inquiry_signature) |old| std.mem.eql(u8,old,signature) else false;
        if(same and !focus and self.inquiry_popover.as(gtk.Widget).getVisible()!=0) {
            self.owner.allocator.free(signature);
            if(self.inquiry_extra) |label| {setLabel(self.owner.allocator,label,string(value,"others_label") orelse "");label.as(gtk.Widget).setVisible(@intFromBool(count>1));}
            return;
        }
        var restore:?usize=null;
        for(self.inquiry_buttons.items,0..) |button,i| if(button.as(gtk.Widget).hasFocus()!=0){restore=i;break;};
        if(self.inquiry_signature) |old| self.owner.allocator.free(old);self.inquiry_signature=signature;
        self.inquiry_focus_pending=self.inquiry_focus_pending or focus;
        self.inquiry_restore_index=restore;
        self.restoreInquiryEditor();
        self.inquiry_buttons.clearRetainingCapacity();
        const panel_width=@max(240,@min(760,@divTrunc(self.host.as(gtk.Widget).getWidth()*618,1000)));
        const listing=std.mem.eql(u8,stage,"list");
        const prompt=std.mem.eql(u8,stage,"prompt");
        const routing=boolean(value,"routing");
        const panel=gtk.Box.new(.vertical,8);margins(panel.as(gtk.Widget));panel.as(gtk.Widget).setSizeRequest(panel_width,-1);
        const header=gtk.Box.new(.horizontal,6);
        if(!listing) {if(self.inquiryButton(string(value,"inbox_label") orelse "All Inquiries","inbox",null,true)) |button| header.append(button.as(gtk.Widget));}
        else {
            for ([_][2][]const u8{.{"Pending","inbox"},.{"History","history"}}) |item| {
                if(self.inquiryButton(item[0],item[1],null,true)) |button| header.append(button.as(gtk.Widget));
            }
        }
        panel.append(header.as(gtk.Widget));
        const title=self.inquiryLabel(string(value,"title") orelse "Inquiry",false);title.as(gtk.Widget).addCssClass("title-3");panel.append(title.as(gtk.Widget));
        panel.append(self.inquiryLabel(string(value,"details") orelse "",true).as(gtk.Widget));
        const root=gtk.Box.new(.vertical,8);
        if(!listing) {
            if(routing) root.append(self.inquiryLabel(string(value,"task_label") orelse "",true).as(gtk.Widget));
            if(!routing and !prompt) root.append(self.inquiryLabel(string(value,"question") orelse "",false).as(gtk.Widget));
            if(boolean(value,"editable")) {
                const answer=gtk.ScrolledWindow.new();answer.setPolicy(.never,.automatic);
                answer.setMinContentHeight(96);answer.setMaxContentHeight(144);answer.setPropagateNaturalHeight(1);
                answer.setChild(self.inquiry_text.as(gtk.Widget));root.append(answer.as(gtk.Widget));
                self.inquiry_answer_scroll=answer;
            } else {
                const task=self.inquiryLabel(string(value,"task_preview") orelse "",false);task.setSelectable(1);root.append(task.as(gtk.Widget));
            }
        }
        if(!prompt) {
            if(routing) root.append(self.inquiryLabel(string(value,"question") orelse "",true).as(gtk.Widget));
            for(array(field(value,"choices"))) |choice| {
                const label=std.fmt.allocPrint(self.owner.allocator,"{s}{d}. {s}",.{if(listing) "" else if(boolean(choice,"selected")) "● " else "○ ",number(choice,"number"),string(choice,"label") orelse ""}) catch continue;
                defer self.owner.allocator.free(label);
                const row=gtk.Box.new(.vertical,3);row.as(gtk.Widget).setMarginBottom(6);
                if(self.inquiryButton(label,"choose",string(choice,"value"),boolean(choice,"enabled"))) |button| {
                    button.as(gtk.Widget).addCssClass("flat");button.as(gtk.Widget).setHalign(.start);row.append(button.as(gtk.Widget));
                }
                if(routing) {
                    row.append(self.inquiryLabel(string(choice,"subtitle") orelse "",true).as(gtk.Widget));
                    row.append(self.inquiryLabel(string(choice,"reason_text") orelse "",false).as(gtk.Widget));
                    if(boolean(choice,"selected")) {
                        const directory=self.inquiryLabel(string(choice,"directory") orelse "",true);directory.setSelectable(1);row.append(directory.as(gtk.Widget));
                        row.append(self.inquiryLabel(string(choice,"context") orelse "",false).as(gtk.Widget));
                    }
                } else if(listing) {
                    row.append(self.inquiryLabel(string(choice,"context") orelse "",false).as(gtk.Widget));
                    row.append(self.inquiryLabel(string(choice,"detail") orelse "",true).as(gtk.Widget));
                }
                root.append(row.as(gtk.Widget));
            }
            if(routing) {
                const alternatives=gtk.Box.new(.horizontal,6);
                if(self.inquiryButton(string(value,"recommend_label") orelse "Other Sessions…","recommend",null,boolean(value,"can_recommend"))) |button| alternatives.append(button.as(gtk.Widget));
                if(boolean(value,"can_retry_view")) {if(self.inquiryButton(string(value,"retry_view_label") orelse "Retry opening","retry_view",null,true)) |button| alternatives.append(button.as(gtk.Widget));}
                root.append(alternatives.as(gtk.Widget));
                root.append(self.inquiryLabel(string(value,"delivery_hint") orelse "",true).as(gtk.Widget));
            }
        }
        const scroll=gtk.ScrolledWindow.new();scroll.setPolicy(.never,.automatic);scroll.setPropagateNaturalHeight(1);
        scroll.setMaxContentHeight(@max(100,@min(480,self.host.as(gtk.Widget).getHeight()-280)));scroll.setChild(root.as(gtk.Widget));panel.append(scroll.as(gtk.Widget));
        panel.append(gtk.Separator.new(.horizontal).as(gtk.Widget));
        if(routing and !listing) panel.append(self.inquiryLabel(string(value,"destination") orelse "",false).as(gtk.Widget));
        panel.append(self.inquiryLabel(string(value,"status") orelse "",false).as(gtk.Widget));
        const actions=gtk.Box.new(.horizontal,6);
        if(listing) {
            if(!boolean(value,"history")) {if(self.inquiryButton("Dismiss pending","dismiss_all",null,count>0)) |button| actions.append(button.as(gtk.Widget));}
            if(self.inquiryButton("Load older","older",null,true)) |button| actions.append(button.as(gtk.Widget));
        } else if(boolean(value,"can_restore")) {
            if(self.inquiryButton("Restore","restore",null,true)) |button| actions.append(button.as(gtk.Widget));
        } else if(!boolean(value,"history")) {
            if(self.inquiryButton("Dismiss","dismiss",null,true)) |button| actions.append(button.as(gtk.Widget));
        }
        if(self.inquiryButton(string(value,"later_label") orelse "Later","later",null,true)) |button| actions.append(button.as(gtk.Widget));
        const spacer=gtk.Box.new(.horizontal,0);spacer.as(gtk.Widget).setHexpand(1);actions.append(spacer.as(gtk.Widget));
        if(!listing and !boolean(value,"history")) {
            if(self.inquiryButton(string(value,if(prompt) "open_label" else "primary_label") orelse "Open",if(prompt) "open" else "submit",null,prompt or boolean(value,"can_next"))) |button| {
                button.as(gtk.Widget).addCssClass("suggested-action");actions.append(button.as(gtk.Widget));
            }
        }
        panel.append(actions.as(gtk.Widget));self.inquiry_extra=null;
        self.inquiry_popover.setChild(panel.as(gtk.Widget));
        if(self.inquiry_focus_source==0)self.inquiry_focus_source=glib.idleAdd(presentInquiry,self);
    }
    fn presentInquiry(context:?*anyopaque) callconv(.c) c_int {
        const self:*InputBar=@ptrCast(@alignCast(context orelse return 0));self.inquiry_focus_source=0;
        const data=self.inquiry_data orelse return 0;
        if(self.composing or self.host.as(gtk.Window).isActive()==0 or std.mem.eql(u8,string(data.value,"stage") orelse "badge","badge")) return 0;
        self.inquiry_popover.popup();
        if(!self.composing) {
            if(self.inquiry_focus_pending) {
                if(boolean(data.value,"editable")){_ = self.inquiry_text.as(gtk.Widget).grabFocus();}
                else if(self.inquiry_buttons.items.len>0){
                    var target=self.inquiry_buttons.items[0];
                    for(self.inquiry_buttons.items) |button| {if(button.getLabel()) |label| {if(std.mem.eql(u8,std.mem.span(label),"Open")){target=button;break;}}}
                    _ = target.as(gtk.Widget).grabFocus();
                }
            }else if(self.inquiry_restore_index) |i| if(i<self.inquiry_buttons.items.len){_ = self.inquiry_buttons.items[i].as(gtk.Widget).grabFocus();};
        }
        self.inquiry_focus_pending=false;self.inquiry_restore_index=null;
        return 0;
    }
    fn inquiryKeyReleased(keys: *gtk.EventControllerKey,key:c_uint,keycode:c_uint,mods:gdk.ModifierType,self:*InputBar) callconv(.c) void {
        self.inquiry_held=null;
        NativeWindow.shortcutKeyReleased(keys,key,keycode,mods,self.host);
        self.refreshDeferredInquiry();
    }
    fn refreshDeferredInquiry(self:*InputBar) void {
        if(!self.inquiry_deferred)return;
        self.inquiry_deferred=false;
        const data=self.inquiry_data orelse return;
        // updateInquiry replaces its owned projection; render from a separate
        // copy so replacing it cannot invalidate the incoming JSON value.
        const encoded=std.json.Stringify.valueAlloc(self.owner.allocator,data.value,.{}) catch return;
        defer self.owner.allocator.free(encoded);
        const copy=std.json.parseFromSlice(Value,self.owner.allocator,encoded,.{.allocate=.alloc_always}) catch return;
        defer copy.deinit();
        self.updateInquiry(copy.value,self.inquiry_focus_pending);
    }
    fn inquiryKeys(_: *gtk.EventControllerKey,key:c_uint,_:c_uint,mods:gdk.ModifierType,self:*InputBar) callconv(.c) c_int {
        if(self.composing) return 0;
        if(key==gdk.KEY_Escape){self.inquiryCommand("later",null);return 1;}
        if(key==gdk.KEY_Return or key==gdk.KEY_KP_Enter) {
            if(self.inquiry_held==key)return 1;self.inquiry_held=key;
            if(mods.control_mask){if(self.inquiry_active and !mods.shift_mask and !mods.alt_mask and !mods.super_mask)sendClicked(self.send_button,self);return 1;}
            for(self.inquiry_buttons.items) |button| if(button.as(gtk.Widget).hasFocus()!=0){_ = button.as(gtk.Widget).activate();return 1;};return 1;
        }
        if(mods.control_mask or mods.alt_mask or mods.super_mask) return 0;
        if(key==gdk.KEY_Up or key==gdk.KEY_Down) {
            const buttons=self.inquiry_buttons.items;if(buttons.len==0)return 1;
            var index:usize=0;for(buttons,0..) |button,i| if(button.as(gtk.Widget).hasFocus()!=0){index=i;break;};
            index=if(key==gdk.KEY_Down)(index+1)%buttons.len else (index+buttons.len-1)%buttons.len;
            _ = buttons[index].as(gtk.Widget).grabFocus();return 1;
        }
        if(key>=gdk.KEY_1 and key<=gdk.KEY_9) {
            if(self.inquiry_held==key)return 1;self.inquiry_held=key;
            const data=self.inquiry_data orelse return 1;const choices=array(field(data.value,"choices"));const i:usize=@intCast(key-gdk.KEY_1);
            if(i<choices.len and boolean(choices[i],"enabled"))self.inquiryCommand("choose",string(choices[i],"value"));return 1;
        }
        return 0;
    }
    fn referencesClicked(_: *gtk.Button, self: *InputBar) callconv(.c) void {
        self.tools.popdown();
        self.owner.input(.{ .type = "references", .pane_id = self.pane_id orelse return });
    }
    fn activityClicked(_: *gtk.Button, self: *InputBar) callconv(.c) void {
        self.tools.popdown();
        self.owner.input(.{ .type = "activity", .pane_id = self.pane_id orelse return });
    }
    fn olderClicked(_: *gtk.Button, self: *InputBar) callconv(.c) void {
        self.tools.popdown();
        self.owner.input(.{ .type = "older", .pane_id = self.pane_id orelse return });
    }
    pub fn quoteClicked(_: *gtk.Button, self: *InputBar) callconv(.c) void {
        if(self.composing) return;
        const meta = if(self.metadata) |m| m.value else return;
        const lease = field(meta,"generic_lease") orelse return;
        const read = self.owner.terminal_selection orelse return;
        const selected = read(self.owner.context,self.owner.allocator,self.host,self.pane_id orelse return) orelse return;
        defer self.owner.allocator.free(selected);
        self.owner.input(.{.type="quote_selection",.lease=lease,.text=selected});
    }
    pub fn insertQuotedText(self: *InputBar, lease: Value, selection: []const u8) void {
        if(self.composing) return;
        const meta = if(self.metadata) |m| m.value else return;
        const current = field(meta,"generic_lease") orelse return;
        for([_] []const u8{"instance_id","window","pane","draft_key"}) |key| {
            if(!std.mem.eql(u8,string(current,key) orelse "",string(lease,key) orelse "")) return;
        }
        if(number(current,"context_revision")!=number(lease,"context_revision") or number(current,"draft_revision")!=number(lease,"draft_revision")) return;
        const insertion = self.owner.allocator.dupeZ(u8,selection) catch return;
        defer self.owner.allocator.free(insertion);
        const buffer = self.text.getBuffer();
        // Replacement emits delete and insert signals. Publish only the completed
        // native undo action, so both signals cannot queue the same old lease.
        self.updating = true;
        buffer.beginUserAction();
        _ = buffer.deleteSelection(1,1);
        buffer.insertAtCursor(insertion,@intCast(insertion.len));
        buffer.endUserAction();
        self.updating = false;
        changed(buffer,self);
        _ = self.text.as(gtk.Widget).grabFocus();
    }
    fn retryClicked(_: *gtk.Button, self: *InputBar) callconv(.c) void {
        if (self.metadata) |meta| if (field(meta.value,"generic_lease")) |lease| {
            self.owner.input(.{.type="refresh_input",.lease=lease});return;
        };
        self.owner.input(.{ .type = "retry", .pane_id = self.pane_id orelse return });
    }
    fn searchActivated(entry: *gtk.Entry, self: *InputBar) callconv(.c) void {
        self.owner.input(.{ .type = "search", .pane_id = self.pane_id orelse return, .query = std.mem.span(entry.as(gtk.Editable).getText()) });
    }
};

/// Native transient navigation. Candidate rows never change the active draft.
const Picker = struct {
    composer: *InputBar,
    popover: *gtk.Popover,
    columns: *gtk.Box,
    channels_box: *gtk.Box,
    threads_box: *gtk.Box,
    target_box: *gtk.Box,
    channel_search: *gtk.Entry,
    thread_search: *gtk.Entry,
    status: *gtk.Label,
    new_pane: *gtk.CheckButton,
    back: *gtk.Button,
    channel_column: *gtk.Box,
    thread_column: *gtk.Box,
    target: []u8,
    node: []u8,
    channel: []u8,
    generation: u64 = 0,
    narrow: bool = false,
    requested_threads: bool = false,
    channels_arena: std.heap.ArenaAllocator,
    threads_arena: std.heap.ArenaAllocator,
    targets_arena: std.heap.ArenaAllocator,
    const Row = struct { picker: *Picker, id: [:0]const u8, label: [:0]const u8, kind: enum { target, channel, thread } };
    pub fn create(composer: *InputBar) !*Picker {
        const a = composer.owner.allocator;
        const self = try a.create(Picker);
        errdefer a.destroy(self);
        const popover = gtk.Popover.new();
        popover.as(gtk.Widget).setParent(composer.target.as(gtk.Widget));
        popover.setPosition(.top);
        const root = gtk.Box.new(.vertical, 6);
        margins(root.as(gtk.Widget));
        const targets = gtk.Box.new(.horizontal, 4);
        root.append(targets.as(gtk.Widget));
        const status = gtk.Label.new("Choose a channel, then a thread");
        status.setXalign(0);
        status.setEllipsize(.end);
        root.append(status.as(gtk.Widget));
        const back = gtk.Button.newWithLabel("‹ Channels");
        back.as(gtk.Widget).setVisible(0);
        root.append(back.as(gtk.Widget));
        const columns = gtk.Box.new(.horizontal, 8);
        const left = gtk.Box.new(.vertical, 4);
        const right = gtk.Box.new(.vertical, 4);
        const cs = gtk.Entry.new();
        cs.setPlaceholderText("Search channels");
        left.append(cs.as(gtk.Widget));
        const ts = gtk.Entry.new();
        ts.setPlaceholderText("Search threads in this channel");
        right.append(ts.as(gtk.Widget));
        const channels = gtk.Box.new(.vertical, 2);
        const threads = gtk.Box.new(.vertical, 2);
        for ([_]*gtk.Box{ left, right }, [_]*gtk.Box{ channels, threads }) |column, rows| {
            const scroll = gtk.ScrolledWindow.new();
            scroll.setPolicy(.never, .automatic);
            scroll.setMinContentHeight(220);
            scroll.setMaxContentHeight(360);
            scroll.setChild(rows.as(gtk.Widget));
            column.append(scroll.as(gtk.Widget));
            column.as(gtk.Widget).setHexpand(1);
            columns.append(column.as(gtk.Widget));
        }
        root.append(columns.as(gtk.Widget));
        const new_pane = gtk.CheckButton.newWithLabel("Open in new Pane");
        root.append(new_pane.as(gtk.Widget));
        const manage = gtk.Button.newWithLabel("Add Polar / Manage Connections…");
        root.append(manage.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(manage, *Picker, manageConnections, self, .{});
        popover.setChild(root.as(gtk.Widget));
        self.* = .{ .composer = composer, .popover = popover, .columns = columns, .channels_box = channels, .threads_box = threads, .target_box = targets, .channel_search = cs, .thread_search = ts, .status = status, .new_pane = new_pane, .back = back, .channel_column = left, .thread_column = right, .target = try a.dupe(u8, ""), .node = try a.dupe(u8, ""), .channel = try a.dupe(u8, ""), .channels_arena = std.heap.ArenaAllocator.init(a), .threads_arena = std.heap.ArenaAllocator.init(a), .targets_arena = std.heap.ArenaAllocator.init(a) };
        _ = gtk.Editable.signals.changed.connect(cs.as(gtk.Editable), *Picker, filterChannels, self, .{});
        _ = gtk.Editable.signals.changed.connect(ts.as(gtk.Editable), *Picker, filterThreads, self, .{});
        _ = gtk.Entry.signals.activate.connect(cs, *Picker, activateChannel, self, .{});
        _ = gtk.Entry.signals.activate.connect(ts, *Picker, activateThread, self, .{});
        _ = gtk.Button.signals.clicked.connect(back, *Picker, goBack, self, .{});
        const keys = gtk.EventControllerKey.new();
        _ = gtk.EventControllerKey.signals.key_pressed.connect(keys, *Picker, keyPressed, self, .{});
        popover.as(gtk.Widget).addController(keys.as(gtk.EventController));
        return self;
    }
    pub fn destroy(self: *Picker) void {
        self.popover.as(gtk.Widget).unparent();
        self.channels_arena.deinit();
        self.threads_arena.deinit();
        self.targets_arena.deinit();
        const a = self.composer.owner.allocator;
        a.free(self.target);
        a.free(self.node);
        a.free(self.channel);
        a.destroy(self);
    }
    fn manageConnections(_: *gtk.Button, self: *Picker) callconv(.c) void {
        self.popover.popdown();
        if (self.composer.owner.manage_connections) |open| open(self.composer.owner.context, self.composer.host);
    }
    fn context(self: *Picker) struct { window_id: []const u8, generation: u64 } {
        return .{ .window_id = self.composer.id, .generation = self.generation };
    }
    fn replace(self: *Picker, value: *[]u8, text: []const u8) void {
        const a = self.composer.owner.allocator;
        const copy = a.dupe(u8, text) catch return;
        a.free(value.*);
        value.* = copy;
    }
    fn present(self: *Picker) void {
        self.generation += 1;
        self.requested_threads = false;
        self.new_pane.setActive(0);
        self.channel_search.as(gtk.Editable).setText("");
        self.thread_search.as(gtk.Editable).setText("");
        self.narrow = self.composer.host.as(gtk.Widget).getWidth() < 700;
        self.columns.as(gtk.Widget).setSizeRequest(if (self.narrow) 280 else 620, -1);
        self.channel_column.as(gtk.Widget).setVisible(1);
        self.thread_column.as(gtk.Widget).setVisible(@intFromBool(!self.narrow));
        self.back.as(gtk.Widget).setVisible(0);
        if (self.composer.metadata) |metadata| if (field(metadata.value, "scope")) |scope_value| {
            self.replace(&self.target, string(scope_value, "target_id") orelse "");
            self.replace(&self.node, string(scope_value, "node_id") orelse "");
            self.replace(&self.channel, string(metadata.value, "channel_id") orelse "");
        };
        clear(self.channels_box, &self.channels_arena);
        clear(self.threads_box, &self.threads_arena);
        self.popover.popup();
        _ = self.thread_search.as(gtk.Widget).grabFocus();
        self.composer.owner.input(.{ .type = "list_targets" });
        if (self.target.len > 0) self.browse();
    }
    fn browse(self: *Picker) void {
        self.requested_threads = false;
        self.status.setText("Loading channels…");
        self.composer.owner.input(.{ .type = "browse", .target_id = self.target, .context = self.context() });
    }
    fn loadThreads(self: *Picker) void {
        self.requested_threads = true;
        self.status.setText("Loading threads…");
        clear(self.threads_box, &self.threads_arena);
        self.composer.owner.input(.{ .type = "threads", .target_id = self.target, .node_id = self.node, .channel_id = self.channel, .context = self.context() });
    }
    pub fn consume(self: *Picker, effect: Value) void {
        if (self.popover.as(gtk.Widget).getVisible() == 0) return;
        const kind = string(effect, "type") orelse return;
        if (std.mem.eql(u8, kind, "targets")) {
            const rows = array(field(effect, "items"));
            if (rows.len > 0 and field(rows[0], "origin") == null) return;
            clear(self.target_box, &self.targets_arena);
            for (rows) |row| self.add(row, .target);
            return;
        }
        const ctx = field(effect, "context") orelse return;
        if (!std.mem.eql(u8, string(ctx, "window_id") orelse "", self.composer.id) or number(ctx, "generation") != self.generation) return;
        if (std.mem.eql(u8, kind, "browse_error")) {
            setLabel(self.composer.owner.allocator, self.status, string(effect, "message") orelse "Cannot load conversations");
            return;
        }
        if (!std.mem.eql(u8, string(effect, "target_id") orelse "", self.target)) return;
        if (std.mem.eql(u8, kind, "channels")) {
            self.replace(&self.node, string(effect, "node_id") orelse "");
            const adjustment = rowAdjustment(self.channels_box);
            const position = if (adjustment) |value| value.getValue() else 0;
            defer if (adjustment) |value| value.setValue(position);
            clear(self.channels_box, &self.channels_arena);
            const rows = array(field(effect, "items"));
            for (rows) |row| self.add(row, .channel);
            self.status.setText(if (rows.len == 0) "No channels available" else "Choose a channel, then a thread");
            filterChannels(self.channel_search.as(gtk.Editable), self);
            if (self.channel.len > 0 and !self.requested_threads) self.loadThreads();
        } else if (std.mem.eql(u8, kind, "threads")) {
            if (!std.mem.eql(u8, string(effect, "node_id") orelse "", self.node) or !std.mem.eql(u8, string(effect, "channel_id") orelse "", self.channel)) return;
            const adjustment = rowAdjustment(self.threads_box);
            const position = if (adjustment) |value| value.getValue() else 0;
            defer if (adjustment) |value| value.setValue(position);
            clear(self.threads_box, &self.threads_arena);
            const rows = array(field(effect, "items"));
            for (rows) |row| self.add(row, .thread);
            self.status.setText(if (rows.len == 0) "No threads in this channel" else "Select a thread to switch this chat Pane");
            filterThreads(self.thread_search.as(gtk.Editable), self);
        }
    }
    fn rowAdjustment(box: *gtk.Box) ?*gtk.Adjustment {
        var parent = box.as(gtk.Widget).getParent();
        while (parent) |widget| : (parent = widget.getParent()) {
            if (gobject.ext.cast(gtk.ScrolledWindow, widget)) |scroll| return scroll.getVadjustment();
        }
        return null;
    }
    fn clear(box: *gtk.Box, arena: *std.heap.ArenaAllocator) void {
        while (box.as(gtk.Widget).getFirstChild()) |child| box.remove(child);
        _ = arena.reset(.retain_capacity);
    }
    fn add(self: *Picker, value: Value, kind: @FieldType(Row, "kind")) void {
        const arena = switch (kind) {
            .target => &self.targets_arena,
            .channel => &self.channels_arena,
            .thread => &self.threads_arena,
        };
        const a = arena.allocator();
        const id = string(value, switch (kind) {
            .target => "target_id",
            .channel => "channel_id",
            .thread => "thread_id",
        }) orelse return;
        const title = string(value, "display_name") orelse id;
        const row = a.create(Row) catch return;
        row.* = .{ .picker = self, .kind = kind, .id = a.dupeZ(u8, id) catch return, .label = a.dupeZ(u8, title) catch return };
        const selected = switch (kind) {
            .target => std.mem.eql(u8, id, self.target),
            .channel => std.mem.eql(u8, id, self.channel),
            .thread => blk: {
                const metadata = self.composer.metadata orelse break :blk false;
                const scope_value = field(metadata.value, "scope") orelse break :blk false;
                break :blk std.mem.eql(u8, id, string(scope_value, "thread") orelse "") and std.mem.eql(u8, self.target, string(scope_value, "target_id") orelse "");
            },
        };
        const label = std.fmt.allocPrintSentinel(a, "{s}{s}", .{ if (selected) "✓ " else "", title }, 0) catch return;
        const button = gtk.Button.newWithLabel(label);
        button.setCanShrink(1);
        button.as(gtk.Widget).setHalign(.fill);
        button.as(gobject.Object).setData("station-picker-row", row);
        const box = switch (kind) {
            .target => self.target_box,
            .channel => self.channels_box,
            .thread => self.threads_box,
        };
        box.append(button.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(button, *Row, clicked, row, .{});
    }
    fn clicked(_: *gtk.Button, row: *Row) callconv(.c) void {
        const self = row.picker;
        const owner = self.composer.owner;
        // Snapshot before dispatch: replacement destroys rows and old Surface.
        const id = owner.allocator.dupe(u8, row.id) catch return;
        defer owner.allocator.free(id);
        switch (row.kind) {
            .target => {
                self.generation += 1;
                self.replace(&self.target, id);
                self.replace(&self.node, "");
                self.replace(&self.channel, "");
                clear(self.channels_box, &self.channels_arena);
                clear(self.threads_box, &self.threads_arena);
                self.browse();
            },
            .channel => {
                self.generation += 1;
                self.replace(&self.channel, id);
                self.thread_search.as(gtk.Editable).setText("");
                if (self.narrow) {
                    self.channel_column.as(gtk.Widget).setVisible(0);
                    self.thread_column.as(gtk.Widget).setVisible(1);
                    self.back.as(gtk.Widget).setVisible(1);
                }
                self.loadThreads();
                _ = self.thread_search.as(gtk.Widget).grabFocus();
            },
            .thread => {
                const content: Content = .{ .target_id = self.target, .node_id = self.node, .owner = self.node, .actor = self.node, .thread = id };
                // Commit any native preedit to the old scope before replacing it.
                _ = self.composer.target.as(gtk.Widget).grabFocus();
                self.composer.commitDraft();
                const replace_pane = if (self.new_pane.getActive() != 0) null else self.composer.pane_id;
                self.popover.popdown();
                owner.open(owner.context, self.composer.host, content, replace_pane) catch |err| {
                    owner.showError(@errorName(err));
                    return;
                };
            },
        }
    }
    fn filter(box: *gtk.Box, query: [:0]const u8) void {
        const folded = glib.utf8Casefold(query, @intCast(query.len));
        defer glib.free(folded);
        var next = box.as(gtk.Widget).getFirstChild();
        while (next) |child| : (next = child.getNextSibling()) {
            const row: *Row = @ptrCast(@alignCast(child.as(gobject.Object).getData("station-picker-row") orelse continue));
            const title = glib.utf8Casefold(row.label, @intCast(row.label.len));
            defer glib.free(title);
            const folded_id = glib.utf8Casefold(row.id.ptr, @intCast(row.id.len));
            defer glib.free(folded_id);
            child.setVisible(@intFromBool(std.mem.indexOf(u8, std.mem.span(title), std.mem.span(folded)) != null or std.mem.indexOf(u8, std.mem.span(folded_id), std.mem.span(folded)) != null));
        }
    }
    fn filterChannels(entry: *gtk.Editable, self: *Picker) callconv(.c) void {
        filter(self.channels_box, std.mem.span(entry.getText()));
    }
    fn filterThreads(entry: *gtk.Editable, self: *Picker) callconv(.c) void {
        filter(self.threads_box, std.mem.span(entry.getText()));
    }
    fn firstVisible(box: *gtk.Box, activate: bool) void {
        var next = box.as(gtk.Widget).getFirstChild();
        while (next) |child| : (next = child.getNextSibling()) if (child.getVisible() != 0) {
            if (activate) {
                const row: *Row = @ptrCast(@alignCast(child.as(gobject.Object).getData("station-picker-row") orelse return));
                clicked(@ptrCast(child), row);
            } else {
                _ = child.grabFocus();
            }
            return;
        };
    }
    fn activateChannel(_: *gtk.Entry, self: *Picker) callconv(.c) void {
        firstVisible(self.channels_box, true);
    }
    fn activateThread(_: *gtk.Entry, self: *Picker) callconv(.c) void {
        firstVisible(self.threads_box, true);
    }
    fn goBack(_: *gtk.Button, self: *Picker) callconv(.c) void {
        self.channel_column.as(gtk.Widget).setVisible(1);
        self.thread_column.as(gtk.Widget).setVisible(0);
        self.back.as(gtk.Widget).setVisible(0);
        _ = self.channel_search.as(gtk.Widget).grabFocus();
    }
    fn keyPressed(_: *gtk.EventControllerKey, key: c_uint, _: c_uint, _: gdk.ModifierType, self: *Picker) callconv(.c) c_int {
        if (key == gdk.KEY_Escape) {
            self.popover.popdown();
            self.composer.activate();
            return 1;
        }
        if (key == gdk.KEY_Down) {
            if (self.channel_search.as(gtk.Widget).hasFocus() != 0) {
                firstVisible(self.channels_box, false);
                return 1;
            }
            if (self.thread_search.as(gtk.Widget).hasFocus() != 0) {
                firstVisible(self.threads_box, false);
                return 1;
            }
        }
        return 0;
    }
};

pub const Browser = struct {
    owner: *Workspace,
    window: *adw.Window,
    rows: *gtk.Box,
    status: *gtk.Label,
    target: []u8,
    node: ?[]u8 = null,
    return_pane: ?[]u8 = null,
    targets_box: *gtk.Box,
    targets_scroll: *gtk.ScrolledWindow,
    target_arena: std.heap.ArenaAllocator,
    arena: std.heap.ArenaAllocator,
    const Click = struct { browser: *Browser, json: []const u8, open: bool = false };
    const TargetClick = struct { browser: *Browser, id: [:0]const u8 };
    pub fn create(owner: *Workspace) !*Browser {
        const self = try owner.allocator.create(Browser);
        errdefer owner.allocator.destroy(self);
        const target = try owner.allocator.dupe(u8, "");
        errdefer owner.allocator.free(target);
        const window = auxiliary.create(owner.app);
        window.as(gtk.Window).setTitle("Open Polar Conversation");
        window.as(gtk.Window).setDefaultSize(640, 500);
        window.as(gtk.Window).setHideOnClose(1);
        window.as(gtk.Window).setIconName("com.polarealm.PolarStation");
        const box = gtk.Box.new(.vertical, 8);
        margins(box.as(gtk.Widget));
        const targets_box = gtk.Box.new(.vertical, 4);
        const targets_scroll = gtk.ScrolledWindow.new();
        targets_scroll.as(gtk.Widget).setSizeRequest(-1, 100);
        targets_scroll.setChild(targets_box.as(gtk.Widget));
        box.append(targets_scroll.as(gtk.Widget));
        const status = gtk.Label.new("Select an explicitly configured Polar target.");
        status.setWrap(1);
        status.setXalign(0);
        box.append(status.as(gtk.Widget));
        const rows = gtk.Box.new(.vertical, 6);
        const scroll = gtk.ScrolledWindow.new();
        scroll.as(gtk.Widget).setVexpand(1);
        scroll.setChild(rows.as(gtk.Widget));
        box.append(scroll.as(gtk.Widget));
        const toolbar = adw.ToolbarView.new();
        toolbar.addTopBar(adw.HeaderBar.new().as(gtk.Widget));
        toolbar.setContent(box.as(gtk.Widget));
        window.setContent(toolbar.as(gtk.Widget));
        self.* = .{ .owner = owner, .window = window, .rows = rows, .status = status, .target = target, .arena = std.heap.ArenaAllocator.init(owner.allocator), .targets_box = targets_box, .targets_scroll = targets_scroll, .target_arena = std.heap.ArenaAllocator.init(owner.allocator) };
        _ = gtk.Window.signals.close_request.connect(window.as(gtk.Window), *Browser, closed, self, .{});
        return self;
    }
    fn closed(_: *gtk.Window, self: *Browser) callconv(.c) c_int {
        const pane = self.return_pane orelse return 0;
        self.window.as(gtk.Widget).setVisible(0);
        var composers = self.owner.composers.valueIterator();
        while (composers.next()) |composer| {
            if (composer.*.pane_id) |selected| if (std.mem.eql(u8, selected, pane)) {
                composer.*.activate();
                // Set the destination before Wayland asynchronously activates
                // the parent; an idle is-active check can run too early.
                composer.*.host.as(gtk.Window).setFocus(composer.*.text.as(gtk.Widget));
                composer.*.host.as(gtk.Window).present();
            };
        }
        return 1;
    }
    pub fn destroy(self: *Browser) void {
        auxiliary.destroy(self.window);
        self.arena.deinit();
        self.target_arena.deinit();
        if (self.node) |n| self.owner.allocator.free(n);
        if (self.return_pane) |pane| self.owner.allocator.free(pane);
        self.owner.allocator.free(self.target);
        self.owner.allocator.destroy(self);
    }
    fn connectTarget(self: *Browser, text: []const u8) void {
        if (text.len == 0 or text.len > 256) return;
        const copy = self.owner.allocator.dupe(u8, text) catch return;
        self.owner.allocator.free(self.target);
        self.target = copy;
        if (self.node) |n| self.owner.allocator.free(n);
        self.node = null;
        self.clear();
        self.owner.input(.{ .type = "browse", .target_id = self.target });
    }
    fn clear(self: *Browser) void {
        while (self.rows.as(gtk.Widget).getFirstChild()) |child| self.rows.remove(child);
        _ = self.arena.reset(.retain_capacity);
    }
    pub fn consume(self: *Browser, effect: Value) void {
        if (field(effect, "context")) |context| {
            if ((string(context, "window_id") orelse "").len != 0) return;
        }
        const kind = string(effect, "type") orelse return;
        if (std.mem.eql(u8, kind, "targets")) {
            const items = array(field(effect, "items"));
            // Describe uses this effect too; do not replace the configured list
            // with its single authenticated descriptor.
            var is_list = items.len == 0;
            for (items) |item| {
                if (field(item, "origin") != null) is_list = true;
            }
            if (!is_list) return;
            while (self.targets_box.as(gtk.Widget).getFirstChild()) |child| self.targets_box.remove(child);
            _ = self.target_arena.reset(.retain_capacity);
            const a = self.target_arena.allocator();
            for (items) |item| {
                const id = string(item, "target_id") orelse continue;
                const text = string(item, "display_name") orelse id;
                const label = a.dupeZ(u8, text) catch continue;
                const click = a.create(TargetClick) catch continue;
                click.* = .{ .browser = self, .id = a.dupeZ(u8, id) catch continue };
                const button = gtk.Button.newWithLabel(label);
                self.targets_box.append(button.as(gtk.Widget));
                _ = gtk.Button.signals.clicked.connect(button, *TargetClick, targetClicked, click, .{});
            }
            if (items.len == 0) setLabel(self.owner.allocator, self.status, "No Polar targets configured. Terminal sessions remain available.");
            return;
        }
        if (!std.mem.eql(u8, string(effect, "target_id") orelse "", self.target)) return;
        const node = string(effect, "node_id") orelse return;
        if (std.mem.eql(u8, kind, "channels")) {
            const copy = self.owner.allocator.dupe(u8, node) catch return;
            if (self.node) |old| self.owner.allocator.free(old);
            self.node = copy;
        } else if (self.node == null or !std.mem.eql(u8, self.node.?, node)) return;
        self.clear();
        setLabel(self.owner.allocator, self.status, node);
        for (array(field(effect, "items"))) |row| {
            const channel = std.mem.eql(u8, kind, "channels");
            const id = string(row, if (channel) "channel_id" else "thread_id") orelse continue;
            const label = string(row, "display_name") orelse id;
            if (channel) self.add(label, .{ .type = "threads", .target_id = self.target, .node_id = node, .channel_id = id }, false) else self.add(label, Content{ .target_id = self.target, .node_id = node, .owner = node, .thread = id, .actor = node }, true);
        }
    }
    fn add(self: *Browser, label: []const u8, object: anytype, open: bool) void {
        const a = self.arena.allocator();
        const click = a.create(Click) catch return;
        click.* = .{ .browser = self, .json = std.json.Stringify.valueAlloc(a, object, .{}) catch return, .open = open };
        const z = a.dupeZ(u8, label) catch return;
        const button = gtk.Button.newWithLabel(z);
        self.rows.append(button.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(button, *Click, clicked, click, .{});
    }
    fn clicked(_: *gtk.Button, click: *Click) callconv(.c) void {
        const browser = click.browser;
        const owner = browser.owner;
        // Copy before dispatch: a synchronous effect can rebuild this browser.
        const copy = owner.allocator.dupe(u8, click.json) catch return;
        defer owner.allocator.free(copy);
        if (click.open) {
            var parsed = std.json.parseFromSlice(Content, owner.allocator, copy, .{ .ignore_unknown_fields = false }) catch return;
            defer parsed.deinit();
            const parent = browser.window.as(gtk.Window).getTransientFor() orelse return;
            const host = gobject.ext.cast(NativeWindow, parent.as(gobject.Object)) orelse return;
            owner.open(owner.context, host, parsed.value, null) catch {
                owner.showError("Cannot open native conversation Pane");
                return;
            };
            browser.window.as(gtk.Widget).setVisible(0);
            // Explicit user navigation may re-open its existing input window;
            // background feed/status updates must never do this.
            var composers = owner.composers.valueIterator();
            while (composers.next()) |composer| {
                const pane = composer.*.pane_id orelse continue;
                const entry = owner.panes.get(pane) orelse continue;
                if (std.mem.eql(u8, entry.content.target_id, parsed.value.target_id) and
                    std.mem.eql(u8, entry.content.thread, parsed.value.thread))
                {
                    composer.*.container.as(gtk.Widget).setVisible(1);
                    composer.*.activate();
                    _ = composer.*.text.as(gtk.Widget).grabFocus();
                }
            }
        } else owner.dispatch(copy);
    }
    fn targetClicked(_: *gtk.Button, click: *TargetClick) callconv(.c) void {
        // Copy before dispatch because native effects may rebuild the list.
        const browser = click.browser;
        const id = browser.owner.allocator.dupeZ(u8, click.id) catch return;
        defer browser.owner.allocator.free(id);
        browser.connectTarget(id);
    }
    pub fn showActivity(self: *Browser, items: Value, pane: []const u8) void {
        const copy = self.owner.allocator.dupe(u8, pane) catch return;
        if (self.return_pane) |old| self.owner.allocator.free(old);
        self.return_pane = copy;
        self.clear();
        const panel_title = self.owner.allocator.dupeZ(u8, string(items, "title") orelse "Tool activity") catch return;
        defer self.owner.allocator.free(panel_title);
        self.window.as(gtk.Window).setTitle(panel_title);
        self.targets_scroll.as(gtk.Widget).setVisible(0);
        setLabel(self.owner.allocator, self.status, string(items, "title") orelse "Tool activity");
        for (array(field(items, "entries"))) |row| {
            const title = string(row, "title") orelse "";
            if (field(row, "input")) |action| {
                self.add(title, action, false);
            } else {
                const z = self.arena.allocator().dupeZ(u8, title) catch continue;
                const label = gtk.Label.new(z);
                label.setWrap(1); label.setSelectable(1); label.setXalign(0);
                self.rows.append(label.as(gtk.Widget));
            }
        }
        auxiliary.present(self.owner.app, self.window);
    }
    pub fn showSearch(self: *Browser, items: Value) void {
        self.clear();
        setLabel(self.owner.allocator, self.status, "Search results in locally loaded messages only");
        for (array(field(items, "items"))) |row| {
            const text = string(row, "text") orelse "";
            const z = self.arena.allocator().dupeZ(u8, text) catch continue;
            const label = gtk.Label.new(z);
            label.setWrap(1);
            label.setSelectable(1);
            label.setXalign(0);
            self.rows.append(label.as(gtk.Widget));
        }
        auxiliary.present(self.owner.app, self.window);
    }
};
