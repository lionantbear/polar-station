//! Native Host import/export review. Matching, choices and apply eligibility stay in Rust.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const gobject = @import("gobject");
const glib = @import("glib");
const model = @import("control_center/model.zig");
const Value = std.json.Value;
const Dispatch = *const fn (?*anyopaque, []const u8) void;
const Send = *const fn (?*anyopaque, []const u8) anyerror!void;
const Choice = struct { owner: *Review, index: usize };
const Check = struct { key: []u8, widget: *gtk.CheckButton };

fn field(v: Value, key: []const u8) Value {
    return if (v == .object) v.object.get(key) orelse .null else .null;
}
fn string(v: Value, key: []const u8) []const u8 {
    const f = field(v, key);
    return if (f == .string) f.string else "";
}
fn list(v: Value) []const Value {
    return if (v == .array) v.array.items else &.{};
}
fn yes(v: Value, key: []const u8) bool {
    const f = field(v, key);
    return f == .bool and f.bool;
}
fn sameSource(a: Value, b: Value) bool {
    return eq(string(a, "catalog_id"), string(b, "catalog_id")) and eq(string(a, "target_id"), string(b, "target_id"));
}
fn eq(a: []const u8, b: []const u8) bool {
    return std.mem.eql(u8, a, b);
}

pub const Review = struct {
    allocator: std.mem.Allocator,
    context: ?*anyopaque,
    dispatch: Dispatch,
    send: ?Send = null,
    send_context: ?*anyopaque = null,
    parent: ?*gtk.Window = null,
    on_review_closed: ?*const fn (?*anyopaque) void = null,
    review_context: ?*anyopaque = null,
    window: ?*gtk.Window = null,
    inline_parent: ?*gtk.Box = null,
    content: ?*gtk.Box = null,
    status: ?*gtk.Label = null,
    dialog: ?*gtk.NativeDialog = null,
    export_bytes: ?[]u8 = null,
    frontend_id: []u8,
    pending_id: ?[]u8 = null,
    pending_token: ?u64 = null,
    view: ?std.json.Parsed(Value) = null,
    selected: usize = 0,
    selected_target_id: ?[]u8 = null,
    rendered_state: ?[]u8 = null,
    exporting: bool = false,
    checks: std.ArrayList(Check) = .empty,
    choices: std.ArrayList(*Choice) = .empty,

    pub fn create(a: std.mem.Allocator, context: ?*anyopaque, dispatch: Dispatch) !*Review {
        const self = try a.create(Review);
        errdefer a.destroy(self);
        const frontend_id = try std.fmt.allocPrint(a, "host-transfer-ui-{x}", .{std.crypto.random.int(u64)});
        self.* = .{ .allocator = a, .context = context, .dispatch = dispatch, .frontend_id = frontend_id };
        return self;
    }
    pub fn deinit(self: *Review) void {
        if (self.dialog) |d| {
            d.destroy();
            d.unref();
        }
        if (self.window) |w| w.destroy();
        self.clearCallbacks();
        self.checks.deinit(self.allocator);
        self.choices.deinit(self.allocator);
        if (self.view) |v| v.deinit();
        if (self.selected_target_id) |id| self.allocator.free(id);
        if (self.rendered_state) |bytes| self.allocator.free(bytes);
        if (self.export_bytes) |v| self.allocator.free(v);
        if (self.pending_id) |v| self.allocator.free(v);
        self.allocator.free(self.frontend_id);
        self.allocator.destroy(self);
    }
    fn clearCallbacks(self: *Review) void {
        for (self.checks.items) |c| self.allocator.free(c.key);
        self.checks.clearRetainingCapacity();
        for (self.choices.items) |c| self.allocator.destroy(c);
        self.choices.clearRetainingCapacity();
    }
    fn input(self: *Review, value: anytype) void {
        const bytes = std.json.Stringify.valueAlloc(self.allocator, value, .{}) catch return;
        defer self.allocator.free(bytes);
        self.dispatch(self.context, bytes);
    }
    pub fn problem(self: *Review, message: []const u8) void {
        const z = self.allocator.dupeZ(u8, message) catch return;
        defer self.allocator.free(z);
        if (self.status) |status| status.setText(z);
        if (self.inline_parent != null) return;
        const dialog = adw.MessageDialog.new(self.parent, "Host transfer", z);
        dialog.addResponse("close", "Close");
        dialog.setDefaultResponse("close");
        dialog.setCloseResponse("close");
        dialog.as(gtk.Window).present();
    }
    pub fn request(self: *Review, token: u64, transfer_request: Value) void {
        if (self.pending_id != null) return;
        const id = std.fmt.allocPrint(self.allocator, "host-transfer-{x}", .{std.crypto.random.int(u64)}) catch return;
        self.pending_id = id;
        self.pending_token = token;
        const payload = std.json.Stringify.valueAlloc(self.allocator, .{ .type = "application_request", .request = .{ .protocol_version = @as(u32, 1), .api = "control_center_v1", .request_id = id, .frontend_session_id = self.frontend_id, .expected_control_revision = @as(?u64, null), .command = .{ .type = "host_transfer", .request = transfer_request } } }, .{}) catch {
            self.failed("Cannot encode Host transfer request.");
            return;
        };
        defer self.allocator.free(payload);
        const send = self.send orelse {
            self.failed("Host is unavailable.");
            return;
        };
        send(self.send_context, payload) catch {
            self.failed("Host transfer was not sent. Reconnect before trying again.");
        };
    }
    fn failed(self: *Review, message: []const u8) void {
        const token = self.pending_token orelse return;
        if (self.pending_id) |id| self.allocator.free(id);
        self.pending_id = null;
        self.pending_token = null;
        self.input(.{ .type = "transfer_complete", .token = token, .result = @as(?u8, null), .@"error" = message });
    }
    pub fn requestFailed(self: *Review, id: []const u8, message: []const u8) bool {
        if (self.pending_id) |pending| if (eq(id, pending)) {
            self.failed(message);
            return true;
        };
        return false;
    }
    pub fn receive(self: *Review, payload: []const u8) bool {
        const id = self.pending_id orelse return false;
        const parsed = std.json.parseFromSlice(Value, self.allocator, payload, .{ .allocate = .alloc_always }) catch return false;
        defer parsed.deinit();
        const envelope = field(parsed.value, "response");
        if (!eq(string(envelope, "request_id"), id)) return false;
        const result = field(envelope, "result");
        if (eq(string(result, "type"), "error")) {
            self.failed(string(field(result, "error"), "message"));
            return true;
        }
        if (!eq(string(result, "type"), "host_transfer")) {
            self.failed("Unexpected Host transfer response.");
            return true;
        }
        const token = self.pending_token orelse return false;
        self.allocator.free(id);
        self.pending_id = null;
        self.pending_token = null;
        self.input(.{ .type = "transfer_complete", .token = token, .result = field(result, "result"), .@"error" = @as(?u8, null) });
        return true;
    }
    pub fn update(self: *Review, view: Value) void {
        const render_state = std.json.Stringify.valueAlloc(self.allocator, .{
            .plan = field(view, "host_import_plan"),
            .decisions = field(view, "host_import_decisions"),
            .pending = field(view, "pending"),
            .can_apply = field(view, "host_import_can_apply"),
            .message = field(view, "message"),
        }, .{}) catch return;
        if (self.rendered_state) |old| {
            if (eq(old, render_state)) {
                self.allocator.free(render_state);
                return;
            }
            self.allocator.free(old);
        }
        self.rendered_state = render_state;
        const bytes = std.json.Stringify.valueAlloc(self.allocator, view, .{}) catch return;
        defer self.allocator.free(bytes);
        const next = std.json.parseFromSlice(Value, self.allocator, bytes, .{ .allocate = .alloc_always }) catch return;
        const had_plan = if (self.view) |old| field(old.value, "host_import_plan") != .null else false;
        if (self.view) |old| old.deinit();
        self.view = next;
        if (field(view, "host_import_plan") != .null) {
            self.exporting = false;
            self.render();
        } else if (!self.exporting) {
            if (self.window) |w| w.as(gtk.Widget).setVisible(0);
            if (had_plan) {
                if (self.inline_parent != null) {
                    self.clear();
                    self.inline_parent = null;
                    self.content = if (self.window) |window| @import("gobject").ext.cast(gtk.Box, window.getChild() orelse return) else null;
                }
                if (self.on_review_closed) |callback| {
                    self.on_review_closed = null;
                    callback(self.review_context);
                }
            }
        }
    }
    fn ensureWindow(self: *Review) void {
        if (self.window != null) return;
        const w = gtk.Window.new();
        w.setTitle("Import Hosts");
        w.setDefaultSize(1100, 680);
        w.setTransientFor(self.parent);
        w.setModal(1);
        const root = gtk.Box.new(.vertical, 14);
        root.as(gtk.Widget).setMarginTop(20);
        root.as(gtk.Widget).setMarginBottom(20);
        root.as(gtk.Widget).setMarginStart(20);
        root.as(gtk.Widget).setMarginEnd(20);
        w.setChild(root.as(gtk.Widget));
        self.window = w;
        self.content = root;
        _ = gtk.Window.signals.close_request.connect(w, *Review, closeRequested, self, .{});
    }
    fn closeRequested(_: *gtk.Window, self: *Review) callconv(.c) c_int {
        if (self.pending_id == null) {
            self.exporting = false;
            self.input(.{ .type = "cancel_host_import" });
            if (self.window) |w| w.as(gtk.Widget).setVisible(0);
        }
        return 1;
    }
    fn clear(self: *Review) void {
        // Retire the old focus before GTK's deferred label-selection work can use it.
        if (self.window) |window| window.setFocus(null);
        if (self.content) |root| {
            while (root.as(gtk.Widget).getFirstChild()) |child| root.remove(child);
            // Import and export reuse this container. Pending belongs to the
            // retired view; each new view applies its own current eligibility.
            root.as(gtk.Widget).setSensitive(1);
        }
        self.clearCallbacks();
        self.status = null;
    }
    fn label(self: *Review, box: *gtk.Box, text: []const u8) *gtk.Label {
        const z = self.allocator.dupeZ(u8, text) catch return gtk.Label.new("");
        defer self.allocator.free(z);
        const l = gtk.Label.new(z);
        l.setXalign(0);
        l.setWrap(1);
        l.setWrapMode(.word_char);
        box.append(l.as(gtk.Widget));
        return l;
    }
    fn button(self: *Review, box: *gtk.Box, title: [:0]const u8, callback: *const fn (*gtk.Button, *Review) callconv(.c) void) *gtk.Button {
        const b = gtk.Button.newWithLabel(title);
        box.append(b.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(b, *Review, callback, self, .{});
        return b;
    }
    fn selectedRow(self: *Review) Value {
        const view = if (self.view) |v| v.value else return .null;
        const rows = list(field(field(view, "host_import_plan"), "rows"));
        return if (self.selected < rows.len) rows[self.selected] else .null;
    }
    fn selectedCandidate(self: *Review) Value {
        const candidates = list(field(self.selectedRow(), "candidates"));
        if (self.selected_target_id) |id| {
            for (candidates) |candidate| if (eq(string(candidate, "target_id"), id)) return candidate;
            return .null;
        }
        return if (candidates.len > 0) candidates[0] else .null;
    }
    fn render(self: *Review) void {
        if (self.inline_parent) |inline_parent| {
            self.content = inline_parent;
        } else {
            self.ensureWindow();
            self.window.?.setTitle("Import Hosts");
        }
        self.clear();
        const root = self.content.?;
        const view = self.view.?.value;
        _ = self.label(root, "Import Hosts — review identity and connection differences");
        const body = gtk.Box.new(.horizontal, 18);
        body.as(gtk.Widget).setVexpand(1);
        root.append(body.as(gtk.Widget));
        const sidebar = gtk.Box.new(.vertical, 6);
        sidebar.as(gtk.Widget).setSizeRequest(220, -1);
        const navscroll = gtk.ScrolledWindow.new();
        navscroll.setPolicy(.never, .automatic);
        navscroll.setChild(sidebar.as(gtk.Widget));
        body.append(navscroll.as(gtk.Widget));
        const rows = list(field(field(view, "host_import_plan"), "rows"));
        if (self.selected >= rows.len) self.selected = 0;
        for (rows, 0..) |row, index| {
            const context = self.allocator.create(Choice) catch continue;
            context.* = .{ .owner = self, .index = index };
            self.choices.append(self.allocator, context) catch {
                self.allocator.destroy(context);
                continue;
            };
            const z = self.allocator.dupeZ(u8, string(field(row, "host"), "display_name")) catch continue;
            defer self.allocator.free(z);
            const b = gtk.Button.newWithLabel(z);
            if (index == self.selected) b.as(gtk.Widget).addCssClass("suggested-action");
            sidebar.append(b.as(gtk.Widget));
            _ = gtk.Button.signals.clicked.connect(b, *Choice, selectRow, context, .{});
            var action = string(row, "disposition");
            for (list(field(view, "host_import_decisions"))) |decision| {
                if (sameSource(field(decision, "source"), field(field(row, "host"), "source"))) {
                    action = string(decision, "action");
                    break;
                }
            }
            _ = self.label(sidebar, action);
        }
        const detail = gtk.Box.new(.vertical, 10);
        detail.as(gtk.Widget).setHexpand(1);
        body.append(detail.as(gtk.Widget));
        const row = self.selectedRow();
        const candidate = self.selectedCandidate();
        const targets = list(field(field(view, "host_import_plan"), "targets"));
        if (targets.len > 0) {
            const names = gtk.StringList.new(null);
            // GtkDropDown.new takes full ownership of the model.
            names.append("Choose an existing Host…");
            var selected: u32 = 0;
            for (targets, 0..) |item, index| {
                const z = self.allocator.dupeZ(u8, string(item, "display_name")) catch continue;
                defer self.allocator.free(z);
                names.append(z);
                const target_id = self.selected_target_id orelse string(candidate, "target_id");
                if (eq(target_id, string(item, "target_id"))) selected = @intCast(index + 1);
            }
            const selector = gtk.DropDown.new(names.as(@import("gio").ListModel), null);
            selector.setSelected(selected);
            detail.append(selector.as(gtk.Widget));
            _ = gobject.Object.signals.notify.connect(selector.as(gobject.Object), *Review, candidateChanged, self, .{ .detail = "selected" });
        }
        const scroll = gtk.ScrolledWindow.new();
        scroll.setPolicy(.automatic, .automatic);
        scroll.as(gtk.Widget).setVexpand(1);
        detail.append(scroll.as(gtk.Widget));
        const table = gtk.Grid.new();
        table.setColumnSpacing(16);
        table.setRowSpacing(10);
        scroll.setChild(table.as(gtk.Widget));
        for ([_][]const u8{ "Field", "Local", "Imported", "Change" }, 0..) |title, col| self.cell(table, @intCast(col), 0, title);
        for (list(field(if (candidate == .null) row else candidate, "fields")), 0..) |item, index| {
            self.cell(table, 0, @intCast(index + 1), string(item, "label"));
            self.cell(table, 1, @intCast(index + 1), if (field(item, "local") == .null) "Unknown" else string(item, "local"));
            self.cell(table, 2, @intCast(index + 1), if (field(item, "imported") == .null) "Unknown" else string(item, "imported"));
            self.cell(table, 3, @intCast(index + 1), string(item, "change"));
        }
        _ = self.label(detail, "Associate only: existing settings and authorization stay unchanged.");
        if (eq(string(row, "disposition"), "duplicate_address")) _ = self.label(detail, "Address, port and SSH username match an existing Host. Select a Host to save an association.");
        const actions = gtk.Box.new(.horizontal, 8);
        detail.append(actions.as(gtk.Widget));
        _ = self.button(actions, "Skip", skip);
        const create_button = self.button(actions, "Create separate", createSeparate);
        create_button.as(gtk.Widget).setSensitive(@intFromBool(!eq(string(field(field(row, "host"), "connection"), "kind"), "local") and !eq(string(row, "disposition"), "duplicate_address")));
        const merge_button = self.button(actions, "Associate with this Host", merge);
        merge_button.as(gtk.Widget).setSensitive(@intFromBool(candidate != .null));
        self.status = self.label(root, string(view, "message"));
        const footer = gtk.Box.new(.horizontal, 8);
        root.append(footer.as(gtk.Widget));
        _ = self.button(footer, "Cancel", cancel);
        const apply_button = self.button(footer, "Apply import", apply);
        apply_button.as(gtk.Widget).setSensitive(@intFromBool(yes(view, "host_import_can_apply")));
        root.as(gtk.Widget).setSensitive(@intFromBool(!yes(view, "pending")));
        if (self.inline_parent == null) if (self.window) |window| {
            if (window.as(gtk.Widget).getVisible() == 0) window.present();
        };
    }
    fn cell(self: *Review, grid: *gtk.Grid, col: c_int, row: c_int, text: []const u8) void {
        const z = self.allocator.dupeZ(u8, text) catch return;
        defer self.allocator.free(z);
        const l = gtk.Label.new(z);
        l.setXalign(0);
        l.setYalign(0);
        l.setSelectable(1);
        l.setWrap(1);
        l.setWrapMode(.word_char);
        l.setMaxWidthChars(34);
        grid.attach(l.as(gtk.Widget), col, row, 1, 1);
    }
    // Checkboxes belong only to the export Host selection, never import field overrides.
    fn addCheck(self: *Review, key: []const u8, widget: *gtk.CheckButton) void {
        const copy = self.allocator.dupe(u8, key) catch return;
        self.checks.append(self.allocator, .{ .key = copy, .widget = widget }) catch {
            self.allocator.free(copy);
            return;
        };
    }
    fn selectRow(_: *gtk.Button, choice: *Choice) callconv(.c) void {
        const self = choice.owner;
        self.selected = choice.index;
        if (self.selected_target_id) |id| self.allocator.free(id);
        self.selected_target_id = null;
        if (self.view) |view| for (list(field(view.value, "host_import_decisions"))) |decision| {
            if (sameSource(field(decision, "source"), field(field(self.selectedRow(), "host"), "source")) and eq(string(decision, "action"), "merge")) {
                self.selected_target_id = self.allocator.dupe(u8, string(decision, "target_id")) catch null;
            }
        };
        self.render();
    }
    fn candidateChanged(object: *gobject.Object, _: *gobject.ParamSpec, self: *Review) callconv(.c) void {
        const dropdown = gobject.ext.cast(gtk.DropDown, object) orelse return;
        const index = dropdown.getSelected();
        if (index == 0) return;
        const targets = list(field(field(self.view.?.value, "host_import_plan"), "targets"));
        if (index > targets.len) return;
        const id = self.allocator.dupe(u8, string(targets[index - 1], "target_id")) catch return;
        if (self.selected_target_id) |old| self.allocator.free(old);
        self.selected_target_id = id;
        self.input(.{ .type = "compare_host_import", .source = field(field(self.selectedRow(), "host"), "source"), .target_id = id });
        self.render();
    }
    fn skip(_: *gtk.Button, self: *Review) callconv(.c) void {
        self.choose("skip");
    }
    fn createSeparate(_: *gtk.Button, self: *Review) callconv(.c) void {
        self.choose("create");
    }
    fn choose(self: *Review, action: []const u8) void {
        self.input(.{ .type = "choose_host_import", .decision = .{ .action = action, .source = field(field(self.selectedRow(), "host"), "source") } });
    }
    fn merge(_: *gtk.Button, self: *Review) callconv(.c) void {
        const candidate = self.selectedCandidate();
        if (candidate == .null) return;
        self.input(.{ .type = "choose_host_import", .decision = .{ .action = "merge", .source = field(field(self.selectedRow(), "host"), "source"), .target_id = string(candidate, "target_id"), .fields = [_][]const u8{}, .allow_address_removal = false } });
    }
    fn apply(_: *gtk.Button, self: *Review) callconv(.c) void {
        self.input(.{ .type = "apply_host_import" });
    }
    fn cancel(_: *gtk.Button, self: *Review) callconv(.c) void {
        if (self.inline_parent != null) {
            if (self.pending_id == null) self.input(.{ .type = "cancel_host_import" });
        } else if (self.window) |window| _ = closeRequested(window, self);
    }
    pub fn openImport(self: *Review) void {
        self.openFile(false);
    }
    pub fn openExport(self: *Review, facts: model.Connections) void {
        self.ensureWindow();
        self.clear();
        self.exporting = true;
        self.window.?.setTitle("Export Hosts");
        const root = self.content.?;
        _ = self.label(root, "Export Hosts — connection configuration only; no credentials");
        const scroll = gtk.ScrolledWindow.new();
        scroll.as(gtk.Widget).setVexpand(1);
        const box = gtk.Box.new(.vertical, 8);
        scroll.setChild(box.as(gtk.Widget));
        root.append(scroll.as(gtk.Widget));
        const local = gtk.CheckButton.newWithLabel("This computer");
        box.append(local.as(gtk.Widget));
        self.addCheck("local", local);
        for (facts.machines) |m| {
            const z = self.allocator.dupeZ(u8, m.display_name) catch continue;
            defer self.allocator.free(z);
            const c = gtk.CheckButton.newWithLabel(z);
            box.append(c.as(gtk.Widget));
            self.addCheck(m.id, c);
        }
        self.status = self.label(root, "Select Hosts to export.");
        const actions = gtk.Box.new(.horizontal, 8);
        root.append(actions.as(gtk.Widget));
        _ = self.button(actions, "Cancel", cancel);
        _ = self.button(actions, "Export…", exportSelected);
        self.window.?.present();
    }
    fn exportSelected(_: *gtk.Button, self: *Review) callconv(.c) void {
        var ids: std.ArrayList([]const u8) = .empty;
        defer ids.deinit(self.allocator);
        for (self.checks.items) |check| if (check.widget.getActive() != 0) {
            ids.append(self.allocator, check.key) catch return;
        };
        if (ids.items.len == 0) {
            self.problem("Select at least one Host.");
            return;
        }
        self.exporting = false;
        self.input(.{ .type = "export_hosts", .target_ids = ids.items });
    }
    pub fn writeExport(self: *Review, bundle: Value) void {
        if (self.export_bytes) |old| self.allocator.free(old);
        self.export_bytes = std.json.Stringify.valueAlloc(self.allocator, bundle, .{ .whitespace = .indent_2 }) catch return;
        self.openFile(true);
    }
    fn openFile(self: *Review, saving: bool) void {
        if (self.dialog != null) return;
        const chooser = gtk.FileChooserNative.new(if (saving) "Export Hosts" else "Import Hosts", self.parent, if (saving) .save else .open, if (saving) "Export" else "Import", "Cancel");
        if (saving) chooser.as(gtk.FileChooser).setCurrentName("hosts.polar-hosts.json");
        self.dialog = chooser.as(gtk.NativeDialog);
        _ = gtk.NativeDialog.signals.response.connect(self.dialog.?, *Review, fileResponse, self, .{});
        self.dialog.?.show();
    }
    fn fileResponse(dialog: *gtk.NativeDialog, response: c_int, self: *Review) callconv(.c) void {
        self.dialog = null;
        defer {
            dialog.destroy();
            dialog.unref();
        }
        if (response != @intFromEnum(gtk.ResponseType.accept)) {
            if (self.export_bytes) |bytes| self.allocator.free(bytes);
            self.export_bytes = null;
            return;
        }
        const chooser = gobject.ext.cast(gtk.FileChooser, dialog) orelse return;
        const file = chooser.getFile() orelse return;
        defer file.unref();
        const path = file.getPath() orelse return;
        defer glib.free(path);
        const name = std.mem.span(path);
        if (self.export_bytes) |bytes| {
            defer {
                self.allocator.free(bytes);
                self.export_bytes = null;
            }
            var write_buffer: [4096]u8 = undefined;
            var out = std.fs.cwd().atomicFile(name, .{ .mode = 0o600, .write_buffer = &write_buffer }) catch {
                self.problem("Cannot open export file.");
                return;
            };
            defer out.deinit();
            out.file_writer.interface.writeAll(bytes) catch {
                self.problem("Cannot write export file.");
                return;
            };
            out.finish() catch {
                self.problem("Cannot finish export.");
                return;
            };
        } else {
            const bytes = std.fs.cwd().readFileAlloc(self.allocator, name, 524288) catch {
                self.problem("Cannot read Host file (maximum 512 KiB).");
                return;
            };
            defer self.allocator.free(bytes);
            const parsed = std.json.parseFromSlice(Value, self.allocator, bytes, .{}) catch {
                self.problem("Host file is not valid JSON.");
                return;
            };
            defer parsed.deinit();
            self.input(.{ .type = "import_hosts", .bundle = parsed.value });
        }
    }
};
