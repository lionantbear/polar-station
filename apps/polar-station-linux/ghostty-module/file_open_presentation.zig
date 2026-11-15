//! Native file-open UI only. Shared Rust supplies all decisions and actions.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const gobject = @import("gobject");
const dict = @import("dictate.zig");
const Window = @import("../class/window.zig").Window;
pub const Action = union(enum) {
    cancel: []const u8,
    retry: []const u8,
    choose: struct { id: []const u8, index: ?usize },
    mapping: struct { id: []const u8, confirmed: bool },
};
pub const Presentation = struct {
    allocator: std.mem.Allocator = undefined,
    window: ?*Window = null,
    anchor: ?*gtk.Widget = null,
    progress: ?*gtk.Popover = null,
    dialog: ?*adw.MessageDialog = null,
    request_id: ?[]u8 = null,
    send: ?*const fn (Action) void = null,
    const Callback = struct { owner: *Presentation, allocator: std.mem.Allocator, id: []u8, kind: enum { cancel, error_box, choice, mapping }, picker: ?*gtk.DropDown = null };

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
        if (self.dialog) |dialog| { self.dialog = null; dialog.as(gtk.Window).close(); }
    }
    fn cancelled(self: *Presentation) void { if (self.request_id) |id| if (self.send) |send| send(.{ .cancel = id }); }
    fn windowGone(raw: ?*anyopaque, _: *gobject.Object) callconv(.c) void {
        const self: *Presentation = @ptrCast(@alignCast(raw orelse return));
        self.window = null; self.dialog = null; self.cancelled();
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
            .error_box => if (std.mem.eql(u8, std.mem.span(name), "retry")) { send(.{ .retry = value.id }); } else { send(.{ .cancel = value.id }); },
            .choice => send(.{ .choose = .{ .id = value.id, .index = if (std.mem.eql(u8, std.mem.span(name), "open")) @as(usize, @intCast(value.picker.?.getSelected())) else null } }),
            .mapping => send(.{ .mapping = .{ .id = value.id, .confirmed = std.mem.eql(u8, std.mem.span(name), "configure") } }),
            .cancel => send(.{ .cancel = value.id }),
        };
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
        const is_mapping = std.mem.eql(u8, kind, "confirm_mapping");
        const body = if (is_choice)
            std.fmt.allocPrintSentinel(self.allocator, "Several matching files exist on {s}.", .{dict.string(dict.field(dict.field(effect, "source"), "origin"), "execution_target_id")}, 0) catch return true
        else if (is_mapping)
            std.fmt.allocPrintSentinel(self.allocator, "Source: {s}\nFile: {s}\nFolder: {s}\n\nNo existing file mapping covers this folder.", .{dict.string(effect, "target"), dict.string(effect, "path"), dict.string(effect, "directory")}, 0) catch return true
        else self.allocator.dupeZ(u8, dict.string(effect, "message")) catch return true;
        defer self.allocator.free(body);
        const dialog = adw.MessageDialog.new(window.as(gtk.Window), if (is_choice) "Choose a file" else if (is_mapping) "Access remote files" else "Could not open file", body);
        const value = self.callback(id, if (is_choice) .choice else if (is_mapping) .mapping else .error_box) orelse return true;
        if (is_choice) {
            const names = gtk.StringList.new(null);
            const candidates = dict.field(effect, "candidates");
            if (candidates == .array) for (candidates.array.items) |file| {
                const path = self.allocator.dupeZ(u8, dict.string(file, "path")) catch continue; defer self.allocator.free(path); names.append(path);
            };
            const gio = @import("gio");
            value.picker = gtk.DropDown.new(names.as(gio.ListModel), null); dialog.setExtraChild(value.picker.?.as(gtk.Widget));
            dialog.addResponse("open", "Open"); dialog.addResponse("cancel", "Cancel"); dialog.setCloseResponse("cancel"); dialog.setDefaultResponse("cancel");
        } else if (is_mapping) {
            dialog.addResponse("configure", "Configure mapping"); dialog.addResponse("cancel", "Cancel"); dialog.setCloseResponse("cancel"); dialog.setDefaultResponse("cancel");
        } else {
            dialog.addResponse("dismiss", "Dismiss");
            if (dict.field(effect, "can_retry") == .bool and dict.field(effect, "can_retry").bool) dialog.addResponse("retry", "Retry");
            dialog.setCloseResponse("dismiss"); dialog.setDefaultResponse("dismiss");
        }
        _ = adw.MessageDialog.signals.response.connect(dialog, *Callback, response, value, .{});
        dialog.as(gobject.Object).weakRef(freed, value); self.dialog = dialog; dialog.as(gtk.Window).present();
        return true;
    }
};
