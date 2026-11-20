//! One native text widget/undo history; all draft and save policy is shared Rust.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const glib = @import("glib");
const gobject = @import("gobject");
const c = @import("polar_station_c");
const Value = std.json.Value;
const Field = struct { field: []const u8, title: []const u8 };
const Actions = struct { loading: []const u8, save: []const u8, restore: []const u8, expand: []const u8, collapse: []const u8, focus: []const u8, cancel: []const u8, discard: []const u8, keep: []const u8, unsaved: []const u8, refresh: []const u8, rebase: []const u8 };
const View = struct {
    fields: []const Field,
    actions: Actions,
    field: ?[]const u8,
    title: []const u8,
    text: []const u8,
    dirty: bool,
    can_edit: bool,
    can_save: bool,
    can_rebase: bool,
    pending: bool,
    loading: bool,
    expanded: bool,
    confirm_close: bool,
    confirmation: []const u8,
    @"error": ?[]const u8,
};
const Output = struct { view: View, effects: []const struct { type: []const u8, settings: Value = .null, field: ?[]const u8 = null } };

pub const Panel = struct {
    allocator: std.mem.Allocator,
    root: *gtk.Box,
    editor_root: *gtk.Box,
    editor: *gtk.TextView,
    scroll: *gtk.ScrolledWindow,
    title: *gtk.Label,
    status: *gtk.Label,
    unsaved: *gtk.Label,
    focus_button: *gtk.Button,
    restore: *gtk.Button,
    refresh: *gtk.Button,
    rebase: *gtk.Button,
    position: *gtk.Button,
    cancel: *gtk.Button,
    save: *gtk.Button,
    field_buttons: [3]*gtk.Button,
    field_clicks: [3]FieldClick,
    document_loaded: bool = false,
    field: ?[]u8 = null,
    transfer_selection: ?struct { insertion: c_int, anchor: c_int } = null,
    expanded: bool = false,
    requires_close: bool = false,
    close_completion: ?struct { context: ?*anyopaque, callback: *const fn (?*anyopaque, bool) void } = null,
    updating: bool = false,
    dialog: ?*adw.MessageDialog = null,
    handle: ?*c.SettingsTextHandle,
    transport_context: ?*anyopaque = null,
    transport: ?*const fn (?*anyopaque, ?Value) void = null,
    presentation_context: ?*anyopaque = null,
    dialog_parent: ?*const fn (?*anyopaque) ?*gtk.Window = null,
    expand: ?*const fn (?*anyopaque, []const u8) void = null,
    collapse: ?*const fn (?*anyopaque) void = null,
    close: ?*const fn (?*anyopaque) void = null,
    const FieldClick = struct { panel: *Panel, field: ?[]u8 = null };

    pub fn create(allocator: std.mem.Allocator) !*Panel {
        const self = try allocator.create(Panel);
        const root = gtk.Box.new(.vertical, 8);
        _ = root.as(gobject.Object).refSink();
        const editor_root = gtk.Box.new(.vertical, 8);
        _ = editor_root.as(gobject.Object).refSink();
        const title = label();
        title.as(gtk.Widget).addCssClass("heading");
        const unsaved = label();
        const status = label();
        const editor = gtk.TextView.new();
        editor.setWrapMode(.word_char);
        editor.getBuffer().setEnableUndo(1);
        const scroll = gtk.ScrolledWindow.new();
        scroll.setPolicy(.never, .automatic);
        scroll.setChild(editor.as(gtk.Widget));
        editor_root.append(title.as(gtk.Widget));
        editor_root.append(unsaved.as(gtk.Widget));
        editor_root.append(scroll.as(gtk.Widget));
        editor_root.append(status.as(gtk.Widget));
        const refresh_control = gtk.Button.newWithLabel("");
        const rebase_control = gtk.Button.newWithLabel("");
        editor_root.append(refresh_control.as(gtk.Widget));
        editor_root.append(rebase_control.as(gtk.Widget));
        const restore = gtk.Button.newWithLabel("");
        const position = gtk.Button.newWithLabel("");
        const cancel = gtk.Button.newWithLabel("");
        const save = gtk.Button.newWithLabel("");
        editor_root.append(restore.as(gtk.Widget));
        editor_root.append(position.as(gtk.Widget));
        const footer = gtk.Box.new(.horizontal, 8);
        footer.append(cancel.as(gtk.Widget));
        footer.append(save.as(gtk.Widget));
        editor_root.append(footer.as(gtk.Widget));
        const fields = [_]*gtk.Button{ gtk.Button.newWithLabel(""), gtk.Button.newWithLabel(""), gtk.Button.newWithLabel("") };
        for (fields) |button| root.append(button.as(gtk.Widget));
        const focus_control = gtk.Button.newWithLabel("");
        root.append(focus_control.as(gtk.Widget));
        root.append(editor_root.as(gtk.Widget));
        self.* = .{ .allocator = allocator, .root = root, .editor_root = editor_root, .title = title, .unsaved = unsaved, .status = status, .editor = editor, .scroll = scroll, .restore = restore, .refresh = refresh_control, .rebase = rebase_control, .position = position, .cancel = cancel, .save = save, .focus_button = focus_control, .field_buttons = fields, .field_clicks = .{ .{ .panel = self }, .{ .panel = self }, .{ .panel = self } }, .handle = c.polar_station_settings_text_new() };
        for (fields, 0..) |button, index| _ = gtk.Button.signals.clicked.connect(button, *FieldClick, editClicked, &self.field_clicks[index], .{});
        _ = gtk.TextBuffer.signals.changed.connect(editor.getBuffer(), *Panel, textChanged, self, .{});
        _ = gtk.Button.signals.clicked.connect(restore, *Panel, restoreClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(position, *Panel, positionClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(focus_control, *Panel, focusClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(cancel, *Panel, cancelClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(save, *Panel, saveClicked, self, .{});
        _ = gtk.Widget.signals.map.connect(root.as(gtk.Widget), *Panel, mapped, self, .{});
        _ = gtk.Widget.signals.map.connect(editor_root.as(gtk.Widget), *Panel, mapped, self, .{});
        _ = gtk.Button.signals.clicked.connect(refresh_control, *Panel, refreshClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(rebase_control, *Panel, rebaseClicked, self, .{});
        self.input(.{ .type = "inspect" });
        return self;
    }
    pub fn destroy(self: *Panel) void {
        self.updating = true;
        if (self.dialog) |dialog| dialog.as(gtk.Window).destroy();
        if (self.field) |field| self.allocator.free(field);
        for (self.field_clicks) |click| if (click.field) |field| self.allocator.free(field);
        self.root.as(gobject.Object).unref();
        self.editor_root.as(gobject.Object).unref();
        c.polar_station_settings_text_free(self.handle);
        self.allocator.destroy(self);
    }
    pub fn input(self: *Panel, value: anytype) void {
        const bytes = std.json.Stringify.valueAlloc(self.allocator, value, .{}) catch return;
        defer self.allocator.free(bytes);
        const result = c.polar_station_settings_text_dispatch(self.handle, bytes.ptr, bytes.len) orelse return;
        defer c.polar_station_desktop_string_free(result);
        var output = std.json.parseFromSlice(Output, self.allocator, std.mem.span(result), .{ .allocate = .alloc_always }) catch return;
        defer output.deinit();
        self.render(output.value.view);
        for (output.value.effects) |effect| {
            if (std.mem.eql(u8, effect.type, "read_settings")) {
                if (self.transport) |send| send(self.transport_context, null);
            } else if (std.mem.eql(u8, effect.type, "save")) {
                if (self.transport) |send| send(self.transport_context, effect.settings);
            } else if (std.mem.eql(u8, effect.type, "expand")) {
                if (self.expand) |expand| if (effect.field) |field| expand(self.presentation_context, field);
            } else if (std.mem.eql(u8, effect.type, "collapse")) {
                if (self.collapse) |collapse| collapse(self.presentation_context);
            } else if (std.mem.eql(u8, effect.type, "close")) {
                if (self.close_completion) |completion| {
                    self.close_completion = null;
                    completion.callback(completion.context, true);
                } else if (self.close) |close| close(self.presentation_context);
            }
        }
        if (self.close_completion != null and std.mem.eql(u8, @field(value, "type"), "keep_editing")) {
            const completion = self.close_completion.?;
            self.close_completion = null;
            completion.callback(completion.context, false);
        }
        self.mountSidebar();
    }
    pub fn confirmClose(self: *Panel, context: ?*anyopaque, callback: *const fn (?*anyopaque, bool) void) bool {
        if (self.close_completion != null) return false;
        self.close_completion = .{ .context = context, .callback = callback };
        self.input(.{ .type = "close" });
        return true;
    }
    pub fn receive(self: *Panel, saving: bool, response: Value) void {
        const field = @import("dictate.zig").field;
        const string = @import("dictate.zig").string;
        const boolean = @import("dictate.zig").boolean;
        if (!boolean(response, "ok")) {
            self.failed(saving, string(response, "error"));
            return;
        }
        const settings = field(response, "payload");
        if (field(settings, "prompts") == .null) {
            self.failed(saving, "This Host does not support editable prompts. Update the Host first.");
            return;
        }
        if (saving) self.input(.{ .type = "complete", .settings = settings, .@"error" = @as(?u8, null) }) else self.input(.{ .type = "snapshot", .settings = settings });
    }
    pub fn failed(self: *Panel, saving: bool, message: []const u8) void {
        if (saving) self.input(.{ .type = "complete", .settings = @as(?u8, null), .@"error" = message }) else self.input(.{ .type = "load_failed", .@"error" = message });
    }
    fn render(self: *Panel, view: View) void {
        self.updating = true;
        defer self.updating = false;
        self.expanded = view.expanded;
        self.requires_close = view.field != null and (view.dirty or view.pending);
        for (self.field_buttons, 0..) |button, index| {
            button.as(gtk.Widget).setVisible(@intFromBool(index < view.fields.len));
            if (index >= view.fields.len) continue;
            self.buttonText(button, view.fields[index].title);
            button.as(gtk.Widget).setSensitive(@intFromBool(!view.pending and !view.loading));
            if (self.field_clicks[index].field) |old| self.allocator.free(old);
            self.field_clicks[index].field = self.allocator.dupe(u8, view.fields[index].field) catch null;
        }
        self.labelText(self.title, view.title);
        const editor_label = self.allocator.dupeSentinel(u8, view.title, 0) catch return;
        defer self.allocator.free(editor_label);
        var properties = [_]gtk.AccessibleProperty{.label};
        var values = [_]gobject.Value{std.mem.zeroes(gobject.Value)};
        gtk.AccessibleProperty.initValue(.label, &values[0]);
        defer values[0].unset();
        values[0].setString(editor_label);
        self.editor.as(gtk.Accessible).updatePropertyValue(1, &properties, &values);
        self.labelText(self.unsaved, if (view.dirty) view.actions.unsaved else "");
        self.labelText(self.status, view.@"error" orelse if (view.loading) view.actions.loading else "");
        self.buttonText(self.focus_button, view.actions.focus);
        self.buttonText(self.refresh, view.actions.refresh);
        self.buttonText(self.rebase, view.actions.rebase);
        self.refresh.as(gtk.Widget).setVisible(@intFromBool(view.@"error" != null or view.can_rebase));
        self.refresh.as(gtk.Widget).setSensitive(@intFromBool(!view.pending and !view.loading));
        self.rebase.as(gtk.Widget).setVisible(@intFromBool(view.can_rebase));
        self.buttonText(self.restore, view.actions.restore);
        self.buttonText(self.position, if (view.expanded) view.actions.collapse else view.actions.expand);
        self.buttonText(self.cancel, view.actions.cancel);
        self.buttonText(self.save, view.actions.save);
        self.focus_button.as(gtk.Widget).setVisible(@intFromBool(view.expanded));
        self.restore.as(gtk.Widget).setSensitive(@intFromBool(view.can_edit));
        self.position.as(gtk.Widget).setSensitive(@intFromBool(view.can_edit or view.expanded));
        self.save.as(gtk.Widget).setSensitive(@intFromBool(view.can_save));
        self.cancel.as(gtk.Widget).setSensitive(@intFromBool(!view.pending));
        self.editor.setEditable(@intFromBool(view.can_edit));
        self.editor_root.as(gtk.Widget).setVisible(@intFromBool(view.field != null));
        self.editor_root.as(gtk.Widget).setVexpand(@intFromBool(view.expanded));
        self.scroll.as(gtk.Widget).setVexpand(@intFromBool(view.expanded));
        self.scroll.setMinContentHeight(if (view.expanded) -1 else 120);
        self.scroll.setMaxContentHeight(if (view.expanded) -1 else 180);
        self.scroll.setPropagateNaturalHeight(@intFromBool(!view.expanded));
        const same_field = if (view.field) |next| if (self.field) |previous| std.mem.eql(u8, next, previous) else false else self.field == null;
        const buffer = self.editor.getBuffer();
        var first: gtk.TextIter = undefined;
        var last: gtk.TextIter = undefined;
        buffer.getBounds(&first, &last);
        const old = buffer.getText(&first, &last, 1);
        defer glib.free(old);
        if (!same_field or (!self.document_loaded and view.can_edit)) {
            self.document_loaded = view.can_edit;
            self.transfer_selection = null;
            buffer.beginIrreversibleAction();
            defer buffer.endIrreversibleAction();
            const text = self.allocator.dupeSentinel(u8, view.text, 0) catch return;
            defer self.allocator.free(text);
            buffer.setText(text, @intCast(view.text.len));
            if (self.field) |field| self.allocator.free(field);
            self.field = if (view.field) |field| self.allocator.dupe(u8, field) catch null else null;
        } else if (!std.mem.eql(u8, std.mem.span(old), view.text)) {
            const text = self.allocator.dupeSentinel(u8, view.text, 0) catch return;
            defer self.allocator.free(text);
            buffer.beginUserAction();
            defer buffer.endUserAction();
            buffer.delete(&first, &last);
            buffer.insertAtCursor(text, @intCast(view.text.len));
        }
        if (view.confirm_close and !view.pending and self.dialog == null) if (self.parentWindow()) |parent| {
            const title = self.allocator.dupeSentinel(u8, view.confirmation, 0) catch return;
            defer self.allocator.free(title);
            const dialog = adw.MessageDialog.new(parent, title, null);
            inline for (.{ "save", "discard", "keep_editing" }, .{ view.actions.save, view.actions.discard, view.actions.keep }) |id, text| {
                const z = self.allocator.dupeSentinel(u8, text, 0) catch return;
                defer self.allocator.free(z);
                dialog.addResponse(id, z);
            }
            dialog.setResponseAppearance("discard", .destructive);
            dialog.setDefaultResponse("keep_editing");
            dialog.setCloseResponse("keep_editing");
            dialog.setResponseEnabled("save", @intFromBool(view.can_save));
            _ = adw.MessageDialog.signals.response.connect(dialog, *Panel, decision, self, .{});
            self.dialog = dialog;
            dialog.as(gtk.Window).present();
        };
    }
    fn mountSidebar(self: *Panel) void {
        if (self.expanded) return;
        if (self.editor_root.as(gtk.Widget).getParent() == null) {
            self.root.append(self.editor_root.as(gtk.Widget));
            self.focus();
            self.restoreSelection();
        }
    }
    pub fn borrowEditor(self: *Panel) ?*gtk.Widget {
        const widget = self.editor_root.as(gtk.Widget);
        self.captureSelection();
        if (widget.getParent() == self.root.as(gtk.Widget)) self.root.remove(widget) else if (widget.getParent() != null) return null;
        return widget;
    }
    pub fn returned(self: *Panel) void {
        if (self.expanded) self.input(.{ .type = "collapse" }) else self.mountSidebar();
    }
    pub fn captureSelection(self: *Panel) void {
        const buffer = self.editor.getBuffer();
        var insertion: gtk.TextIter = undefined;
        var anchor: gtk.TextIter = undefined;
        buffer.getIterAtMark(&insertion, buffer.getInsert());
        buffer.getIterAtMark(&anchor, buffer.getSelectionBound());
        self.transfer_selection = .{ .insertion = insertion.getOffset(), .anchor = anchor.getOffset() };
    }
    fn restoreSelection(self: *Panel) void {
        const selection = self.transfer_selection orelse return;
        self.transfer_selection = null;
        const buffer = self.editor.getBuffer();
        var insertion: gtk.TextIter = undefined;
        var anchor: gtk.TextIter = undefined;
        _ = buffer.getIterAtOffset(&insertion, selection.insertion);
        _ = buffer.getIterAtOffset(&anchor, selection.anchor);
        buffer.selectRange(&insertion, &anchor);
    }
    pub fn focus(self: *Panel) void {
        if (self.editor.as(gtk.Widget).getMapped() != 0) _ = self.editor.as(gtk.Widget).grabFocus();
    }
    fn parentWindow(self: *Panel) ?*gtk.Window {
        const widget = if (self.expanded) self.editor_root.as(gtk.Widget) else self.root.as(gtk.Widget);
        if (widget.getMapped() != 0) if (widget.getRoot()) |root| if (gobject.ext.cast(gtk.Window, root.as(gobject.Object))) |window| return window;
        return if (self.dialog_parent) |parent| parent(self.presentation_context) else null;
    }
    fn label() *gtk.Label {
        const result = gtk.Label.new("");
        result.setWrap(1);
        result.setXalign(0);
        return result;
    }
    fn labelText(self: *Panel, value: *gtk.Label, text: []const u8) void {
        const z = self.allocator.dupeSentinel(u8, text, 0) catch return;
        defer self.allocator.free(z);
        value.setText(z);
    }
    fn buttonText(self: *Panel, value: *gtk.Button, text: []const u8) void {
        const z = self.allocator.dupeSentinel(u8, text, 0) catch return;
        defer self.allocator.free(z);
        value.setLabel(z);
        if (value.getChild()) |child| if (gobject.ext.cast(gtk.Label, child.as(gobject.Object))) |caption| caption.setWrap(1);
    }
    fn editClicked(_: *gtk.Button, click: *FieldClick) callconv(.c) void {
        if (click.field) |field| click.panel.input(.{ .type = "edit", .field = field });
    }
    fn mapped(_: *gtk.Widget, self: *Panel) callconv(.c) void {
        self.input(.{ .type = "inspect" });
        if (self.field != null) self.focus();
        self.restoreSelection();
    }
    fn textChanged(_: *gtk.TextBuffer, self: *Panel) callconv(.c) void {
        if (self.updating) return;
        const buffer = self.editor.getBuffer();
        var first: gtk.TextIter = undefined;
        var last: gtk.TextIter = undefined;
        buffer.getBounds(&first, &last);
        const text = buffer.getText(&first, &last, 1);
        defer glib.free(text);
        self.input(.{ .type = "change", .text = std.mem.span(text) });
    }
    fn refreshClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.input(.{ .type = "refresh" });
    }
    fn rebaseClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.input(.{ .type = "rebase" });
    }
    fn restoreClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.input(.{ .type = "restore_default" });
    }
    fn positionClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.input(.{ .type = if (self.expanded) @as([]const u8, "collapse") else "expand" });
    }
    fn focusClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.input(.{ .type = "expand" });
    }
    fn cancelClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.input(.{ .type = "close" });
    }
    fn saveClicked(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.input(.{ .type = "save" });
    }
    fn decision(dialog: *adw.MessageDialog, response: [*:0]const u8, self: *Panel) callconv(.c) void {
        self.dialog = null;
        dialog.as(gtk.Window).destroy();
        self.input(.{ .type = std.mem.span(response) });
    }
};
