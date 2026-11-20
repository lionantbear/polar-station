//! Native file-open UI only. Shared Rust supplies all decisions and actions.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const gdk = @import("gdk");
const glib = @import("glib");
const gobject = @import("gobject");
const dict = @import("dictate.zig");
const Window = @import("../class/window.zig").Window;
pub const Action = union(enum) {
    cancel: []const u8,
    retry: []const u8,
    edit: struct { id: []const u8, text: []const u8 },
    choose: struct { id: []const u8, index: ?usize },
    mapping: struct { id: []const u8, confirmed: bool },
};
pub const Presentation = struct {
    allocator: std.mem.Allocator = undefined,
    window: ?*Window = null,
    anchor: ?*gtk.Widget = null,
    progress: ?*gtk.Popover = null,
    failure: ?ErrorPopover = null,
    dialog: ?*adw.MessageDialog = null,
    request_id: ?[]u8 = null,
    send: ?*const fn (Action) void = null,
    const Callback = struct { owner: *Presentation, allocator: std.mem.Allocator, id: []u8, kind: enum { cancel, choice, mapping }, picker: ?*gtk.DropDown = null };
    const ErrorPopover = struct {
        popup: *gtk.Popover,
        editor: *gtk.TextView,
        editor_scroll: *gtk.ScrolledWindow,
        submit: *gtk.Button,
        copy_details: [:0]u8,
        selection: ?struct { start: c_int, end: c_int },
    };

    pub fn bind(self: *Presentation, allocator: std.mem.Allocator, window: *Window, anchor: *gtk.Widget, send: *const fn (Action) void) void {
        self.deinit();
        self.allocator = allocator; self.window = window; self.anchor = anchor; self.send = send;
        window.as(gobject.Object).weakRef(windowGone, self);
        anchor.as(gobject.Object).weakRef(anchorGone, self);
    }
    pub fn deinit(self: *Presentation) void {
        self.dismiss();
        if (self.window) |window| window.as(gobject.Object).weakUnref(windowGone, self);
        if (self.anchor) |anchor| anchor.as(gobject.Object).weakUnref(anchorGone, self);
        if (self.request_id) |id| self.allocator.free(id);
        self.* = .{};
    }
    pub fn sourceWindow(self: *Presentation) ?*Window { return self.window; }
    fn dismiss(self: *Presentation) void {
        if (self.progress) |popup| { self.progress = null; popup.popdown(); popup.as(gtk.Widget).unparent(); }
        self.dismissError();
        if (self.dialog) |dialog| { self.dialog = null; dialog.as(gtk.Window).close(); }
    }
    fn cancelled(self: *Presentation) void { if (self.request_id) |id| if (self.send) |send| send(.{ .cancel = id }); }
    fn windowGone(raw: ?*anyopaque, _: *gobject.Object) callconv(.c) void {
        const self: *Presentation = @ptrCast(@alignCast(raw orelse return));
        self.window = null; self.dialog = null; self.dismissError(); self.cancelled();
    }
    fn anchorGone(raw: ?*anyopaque, _: *gobject.Object) callconv(.c) void {
        const self: *Presentation = @ptrCast(@alignCast(raw orelse return));
        self.anchor = null; self.progress = null; self.cancelled();
    }
    fn callback(self: *Presentation, id: []const u8, kind: @FieldType(Callback, "kind")) ?*Callback {
        const value = self.allocator.create(Callback) catch return null;
        value.* = .{ .owner = self, .allocator = self.allocator, .id = self.allocator.dupe(u8, id) catch { self.allocator.destroy(value); return null; }, .kind = kind };
        return value;
    }
    fn freed(raw: ?*anyopaque, _: *gobject.Object) callconv(.c) void {
        const value: *Callback = @ptrCast(@alignCast(raw orelse return));
        value.allocator.free(value.id); value.allocator.destroy(value);
    }
    fn cancelClicked(_: *gtk.Button, value: *Callback) callconv(.c) void {
        if (value.owner.send) |send| send(.{ .cancel = value.id });
    }
    fn response(dialog: *adw.MessageDialog, name: [*:0]const u8, value: *Callback) callconv(.c) void {
        const self = value.owner;
        if (self.dialog != dialog) return;
        self.dialog = null;
        // Libadwaita closes the dialog after emitting response. Destroying it
        // inside that signal invalidates its own close path and child widgets.
        if (self.send) |send| switch (value.kind) {
            .choice => send(.{ .choose = .{ .id = value.id, .index = if (std.mem.eql(u8, std.mem.span(name), "open")) @as(usize, @intCast(value.picker.?.getSelected())) else null } }),
            .mapping => send(.{ .mapping = .{ .id = value.id, .confirmed = std.mem.eql(u8, std.mem.span(name), "configure") } }),
            .cancel => send(.{ .cancel = value.id }),
        };
    }
    fn dismissError(self: *Presentation) void {
        const failure = self.failure orelse return;
        self.failure = null;
        failure.popup.popdown();
        const widget = failure.popup.as(gtk.Widget);
        if (widget.getParent() != null) widget.unparent();
        failure.popup.as(gobject.Object).unref();
        self.allocator.free(failure.copy_details);
    }

    fn errorLabel(self: *Presentation, text: []const u8) *gtk.Label {
        const label = gtk.Label.new(null);
        if (self.allocator.dupeZ(u8, text)) |value| {
            defer self.allocator.free(value);
            label.setText(value);
        } else |_| {}
        label.setWrap(1);
        label.setWrapMode(.word_char);
        label.setXalign(0);
        label.setMaxWidthChars(58);
        label.setSelectable(1);
        return label;
    }

    fn errorDismissed(_: *gtk.Button, self: *Presentation) callconv(.c) void {
        self.dismissError();
        self.cancelled();
    }

    fn errorKey(_: *gtk.EventControllerKey, key: c_uint, _: c_uint, _: gdk.ModifierType, self: *Presentation) callconv(.c) c_int {
        if (key != gdk.KEY_Escape) return 0;
        self.dismissError();
        self.cancelled();
        return 1;
    }

    fn errorRetry(_: *gtk.Button, self: *Presentation) callconv(.c) void {
        if (self.request_id) |id| if (self.send) |send| send(.{ .retry = id });
    }

    fn copyError(button: *gtk.Button, self: *Presentation) callconv(.c) void {
        const failure = self.failure orelse return;
        button.as(gtk.Widget).getClipboard().setText(failure.copy_details);
    }

    fn editError(_: *gtk.Button, self: *Presentation) callconv(.c) void {
        const failure = if (self.failure) |*value| value else return;
        failure.editor_scroll.as(gtk.Widget).setVisible(1);
        failure.submit.as(gtk.Widget).setVisible(1);
        _ = failure.editor.as(gtk.Widget).grabFocus();
        if (failure.selection) |selection| {
            const buffer = failure.editor.getBuffer();
            var start: gtk.TextIter = undefined;
            var end: gtk.TextIter = undefined;
            buffer.getIterAtOffset(&start, selection.start);
            buffer.getIterAtOffset(&end, selection.end);
            buffer.selectRange(&start, &end);
            failure.selection = null;
        }
    }

    fn submitEdit(_: *gtk.Button, self: *Presentation) callconv(.c) void {
        const failure = self.failure orelse return;
        const id = self.request_id orelse return;
        const send = self.send orelse return;
        const buffer = failure.editor.getBuffer();
        var start: gtk.TextIter = undefined;
        var end: gtk.TextIter = undefined;
        buffer.getBounds(&start, &end);
        const text = buffer.getText(&start, &end, 1);
        defer glib.free(text);
        send(.{ .edit = .{ .id = id, .text = std.mem.span(text) } });
    }

    fn errorButton(self: *Presentation, parent: *gtk.Box, title: [:0]const u8, callback_fn: anytype) *gtk.Button {
        const button = gtk.Button.newWithLabel(title);
        _ = gtk.Button.signals.clicked.connect(button, *Presentation, callback_fn, self, .{});
        parent.append(button.as(gtk.Widget));
        return button;
    }

    fn showError(self: *Presentation, window: *Window, effect: std.json.Value) void {
        const details = self.allocator.dupeZ(u8, dict.string(effect, "copy_details")) catch return;
        const source = window.as(gtk.Window);
        const anchor = window.getTabView().as(gtk.Widget);
        const width = @min(@max(anchor.getWidth() - 32, 1), 590);
        const height = @min(@max(anchor.getHeight() - 125, 48), 405);
        const popup = gtk.Popover.new();
        popup.setAutohide(0);
        popup.setHasArrow(0);
        popup.setPosition(.bottom);
        const point: gdk.Rectangle = .{
            .f_x = @divTrunc(anchor.getWidth(), 2),
            .f_y = 8,
            .f_width = 1,
            .f_height = 1,
        };
        popup.setPointingTo(&point);
        const content = gtk.Box.new(.vertical, 9);
        const widget = content.as(gtk.Widget);
        widget.setMarginStart(8);
        widget.setMarginEnd(8);
        widget.setMarginTop(8);
        widget.setMarginBottom(8);
        const body = gtk.Box.new(.vertical, 8);
        const heading = self.errorLabel("Could not open file");
        heading.as(gtk.Widget).addCssClass("title-3");
        body.append(heading.as(gtk.Widget));
        const summary = self.errorLabel(dict.string(effect, "summary"));
        summary.as(gtk.Widget).addCssClass("error");
        body.append(summary.as(gtk.Widget));
        const results = dict.field(effect, "steps");
        if (results == .array) for (results.array.items) |step| {
            const row = gtk.Box.new(.vertical, 2);
            const title = self.errorLabel(dict.string(step, "title"));
            title.as(gtk.Widget).addCssClass("heading");
            row.append(title.as(gtk.Widget));
            const status = self.errorLabel(dict.string(step, "summary"));
            if (dict.field(step, "failed") == .bool and dict.field(step, "failed").bool)
                status.as(gtk.Widget).addCssClass("error");
            row.append(status.as(gtk.Widget));
            const detail = dict.string(step, "details");
            if (detail.len > 0) {
                const expander = gtk.Expander.new("Details");
                expander.setChild(self.errorLabel(detail).as(gtk.Widget));
                row.append(expander.as(gtk.Widget));
            }
            body.append(row.as(gtk.Widget));
        };
        const editor = gtk.TextView.new();
        editor.setWrapMode(.word_char);
        editor.getBuffer().setEnableUndo(1);
        const original = dict.string(effect, "edit_text");
        if (self.allocator.dupeZ(u8, original)) |initial| {
            defer self.allocator.free(initial);
            editor.getBuffer().setText(initial, @intCast(original.len));
        } else |_| {}
        const edit_scroll = gtk.ScrolledWindow.new();
        edit_scroll.setPolicy(.never, .automatic);
        edit_scroll.setMinContentHeight(86);
        edit_scroll.setChild(editor.as(gtk.Widget));
        edit_scroll.as(gtk.Widget).setVisible(0);
        body.append(edit_scroll.as(gtk.Widget));
        const scroll = gtk.ScrolledWindow.new();
        scroll.setPolicy(.never, .automatic);
        scroll.setMaxContentWidth(width);
        scroll.setMinContentWidth(width);
        scroll.setMaxContentHeight(height);
        scroll.setPropagateNaturalHeight(1);
        scroll.setChild(body.as(gtk.Widget));
        content.append(scroll.as(gtk.Widget));

        const footer = gtk.Box.new(if (width < 505) .vertical else .horizontal, 6);
        const left = gtk.Box.new(.horizontal, 6);
        left.as(gtk.Widget).setHexpand(1);
        _ = self.errorButton(left, "Copy details", copyError);
        if (dict.field(effect, "edit_text") == .string)
            _ = self.errorButton(left, "Edit reference", editError);
        const right = gtk.Box.new(.horizontal, 6);
        _ = self.errorButton(right, "Dismiss", errorDismissed);
        if (dict.field(effect, "can_retry") == .bool and dict.field(effect, "can_retry").bool)
            _ = self.errorButton(right, "Retry", errorRetry);
        const submit = self.errorButton(right, "Try opening", submitEdit);
        submit.as(gtk.Widget).addCssClass("suggested-action");
        submit.as(gtk.Widget).setVisible(0);
        footer.append(left.as(gtk.Widget));
        footer.append(right.as(gtk.Widget));
        content.append(footer.as(gtk.Widget));
        popup.setChild(widget);
        const keys = gtk.EventControllerKey.new();
        _ = gtk.EventControllerKey.signals.key_pressed.connect(keys, *Presentation, errorKey, self, .{});
        popup.as(gtk.Widget).addController(keys.as(gtk.EventController));
        self.failure = .{
            .popup = popup,
            .editor = editor,
            .editor_scroll = edit_scroll,
            .submit = submit,
            .copy_details = details,
            .selection = null,
        };
        const bounds = dict.field(effect, "edit_range");
        const from = dict.field(bounds, "start");
        const to = dict.field(bounds, "end");
        if (from == .integer and to == .integer and from.integer >= 0 and
            to.integer >= from.integer and to.integer <= original.len)
        selection: {
            const start = std.unicode.utf8CountCodepoints(original[0..@intCast(from.integer)]) catch break :selection;
            const end = std.unicode.utf8CountCodepoints(original[0..@intCast(to.integer)]) catch break :selection;
            self.failure.?.selection = .{ .start = @intCast(start), .end = @intCast(end) };
        }
        const previous_focus = source.getFocus();
        popup.as(gtk.Widget).setParent(anchor);
        _ = popup.as(gobject.Object).ref();
        popup.popup();
        if (source.getFocus() != previous_focus) source.setFocus(previous_focus);
    }

    pub fn present(self: *Presentation, effect: std.json.Value) bool {
        const kind = dict.string(effect, "type");
        const handled = std.mem.eql(u8, kind, "show_progress") or std.mem.eql(u8, kind, "dismiss_progress") or std.mem.eql(u8, kind, "show_error") or std.mem.eql(u8, kind, "choose_file") or std.mem.eql(u8, kind, "confirm_mapping");
        if (!handled) return false;
        const id = dict.string(effect, "request_id");
        if (std.mem.eql(u8, kind, "dismiss_progress")) {
            if (self.request_id) |current| if (std.mem.eql(u8, current, id)) { self.dismiss(); };
            return true;
        }
        if (self.request_id) |previous| self.allocator.free(previous);
        self.request_id = self.allocator.dupe(u8, id) catch return true;
        const window = self.window orelse { self.cancelled(); return true; };
        self.dismiss();
        if (std.mem.eql(u8, kind, "show_error")) {
            self.showError(window, effect);
            return true;
        }
        if (std.mem.eql(u8, kind, "show_progress")) {
            const anchor = self.anchor orelse { self.cancelled(); return true; };
            const popup = gtk.Popover.new(); popup.setAutohide(0); popup.setHasArrow(0);
            const row = gtk.Box.new(.horizontal, 10);
            const message = self.allocator.dupeZ(u8, dict.string(effect, "message")) catch return true; defer self.allocator.free(message);
            const label = gtk.Label.new(message); label.setWrap(1); label.setMaxWidthChars(36); row.append(label.as(gtk.Widget));
            const cancel = gtk.Button.newWithLabel("Cancel"); row.append(cancel.as(gtk.Widget));
            const value = self.callback(id, .cancel) orelse return true;
            _ = gtk.Button.signals.clicked.connect(cancel, *Callback, cancelClicked, value, .{});
            cancel.as(gobject.Object).weakRef(freed, value);
            popup.setChild(row.as(gtk.Widget)); popup.as(gtk.Widget).setParent(anchor); self.progress = popup; popup.popup();
            return true;
        }
        const is_choice = std.mem.eql(u8, kind, "choose_file");
        const body = if (is_choice)
            std.fmt.allocPrintSentinel(self.allocator, "Several matching files exist on {s}.", .{dict.string(dict.field(dict.field(effect, "source"), "origin"), "execution_target_id")}, 0) catch return true
        else
            std.fmt.allocPrintSentinel(self.allocator, "Source: {s}\nFile: {s}\nFolder: {s}\n\nNo existing file mapping covers this folder.", .{dict.string(effect, "target"), dict.string(effect, "path"), dict.string(effect, "directory")}, 0) catch return true;
        defer self.allocator.free(body);
        const dialog = adw.MessageDialog.new(window.as(gtk.Window), if (is_choice) "Choose a file" else "Access remote files", body);
        const value = self.callback(id, if (is_choice) .choice else .mapping) orelse return true;
        if (is_choice) {
            const names = gtk.StringList.new(null);
            const candidates = dict.field(effect, "candidates");
            if (candidates == .array) for (candidates.array.items) |file| {
                const path = self.allocator.dupeZ(u8, dict.string(file, "path")) catch continue; defer self.allocator.free(path); names.append(path);
            };
            const gio = @import("gio");
            value.picker = gtk.DropDown.new(names.as(gio.ListModel), null); dialog.setExtraChild(value.picker.?.as(gtk.Widget));
            dialog.addResponse("open", "Open"); dialog.addResponse("cancel", "Cancel"); dialog.setCloseResponse("cancel"); dialog.setDefaultResponse("cancel");
        } else {
            dialog.addResponse("configure", "Configure mapping"); dialog.addResponse("cancel", "Cancel"); dialog.setCloseResponse("cancel"); dialog.setDefaultResponse("cancel");
        }
        _ = adw.MessageDialog.signals.response.connect(dialog, *Callback, response, value, .{});
        dialog.as(gobject.Object).weakRef(freed, value); self.dialog = dialog; dialog.as(gtk.Window).present();
        return true;
    }
};
