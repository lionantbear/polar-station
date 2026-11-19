//! GTK Settings adapter. Rust owns probe/admission/generation and source guards;
//! the native owner saves its catalog and retires physical channels explicitly.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const c = @cImport({
    @cInclude("polar_station_desktop_client.h");
});
const transfer = @import("workspace_transfer_window.zig");
const Value = std.json.Value;
const field = transfer.field;
const string = transfer.string;
const Dispatch = *const fn (?*anyopaque, []const u8, []const u8) void;
pub const Page = struct {
    allocator: std.mem.Allocator,
    context: ?*anyopaque,
    dispatch: Dispatch,
    window: ?*gtk.Window = null,
    entry: ?*gtk.Entry = null,
    body: ?*gtk.Box = null,
    status: ?*gtk.Label = null,
    switch_button: ?*gtk.Button = null,
    inspect_button: ?*gtk.Button = null,
    current: ?[]u8 = null,
    previous: ?[]u8 = null,
    target: ?[]u8 = null,
    message: ?[]u8 = null,
    catalog: ?std.json.Parsed(Value) = null,
    prepared: ?std.json.Parsed(Value) = null,
    preview: ?std.json.Parsed(Value) = null,
    pending_host: ?[]u8 = null,
    pending_backend: ?[]u8 = null,
    busy: bool = false,
    waiting_registration: bool = false,
    save_revision: ?u64 = null,
    resolving: bool = false,
    polar_instances: ?std.json.Parsed(Value) = null,
    pub fn create(a: std.mem.Allocator, context: ?*anyopaque, dispatch: Dispatch) !*Page {
        const self = try a.create(Page);
        self.* = .{ .allocator = a, .context = context, .dispatch = dispatch };
        return self;
    }
    pub fn deinit(self: *Page) void {
        if (self.window) |w| w.destroy();
        inline for (.{ "current", "previous", "target", "message", "pending_host", "pending_backend" }) |key| if (@field(self, key)) |v| self.allocator.free(v);
        inline for (.{ "catalog", "prepared", "preview", "polar_instances" }) |key| if (@field(self, key)) |v| v.deinit();
        self.allocator.destroy(self);
    }
    fn text(self: *Page, box: *gtk.Box, value: []const u8) void {
        const z = self.allocator.dupeZ(u8, value) catch return;
        defer self.allocator.free(z);
        const l = gtk.Label.new(z);
        l.setXalign(0);
        l.setWrap(1);
        box.append(l.as(gtk.Widget));
    }
    pub fn present(self: *Page, parent: *gtk.Window, path: []const u8) void {
        if (self.current) |v| self.allocator.free(v);
        self.current = self.allocator.dupe(u8, path) catch return;
        if (self.window == null) {
            const w = gtk.Window.new();
            w.setTitle("Station Backend");
            w.setDefaultSize(960, 640);
            w.setHideOnClose(1);
            const root = gtk.Box.new(.vertical, 12);
            inline for (.{ "Top", "Bottom", "Start", "End" }) |edge| @field(gtk.Widget, "setMargin" ++ edge)(root.as(gtk.Widget), 16);
            self.text(root, "Choose one local backend for every Workspace in this client.");
            const bar = gtk.Box.new(.horizontal, 8);
            self.text(bar, "Local socket");
            const entry = gtk.Entry.new();
            entry.as(gtk.Widget).setHexpand(1);
            bar.append(entry.as(gtk.Widget));
            self.entry = entry;
            _ = gtk.Editable.signals.changed.connect(entry.as(gtk.Editable), *Page, endpointChanged, self, .{});
            const inspect_button = gtk.Button.newWithLabel("Inspect");
            bar.append(inspect_button.as(gtk.Widget));
            self.inspect_button = inspect_button;
            _ = gtk.Button.signals.clicked.connect(inspect_button, *Page, inspectClicked, self, .{});
            const previous = gtk.Button.newWithLabel("Previous backend");
            bar.append(previous.as(gtk.Widget));
            _ = gtk.Button.signals.clicked.connect(previous, *Page, previousClicked, self, .{});
            root.append(bar.as(gtk.Widget));
            const body = gtk.Box.new(.vertical, 12);
            self.body = body;
            const scroll = gtk.ScrolledWindow.new();
            scroll.setPolicy(.never, .automatic);
            scroll.setChild(body.as(gtk.Widget));
            scroll.as(gtk.Widget).setVexpand(1);
            root.append(scroll.as(gtk.Widget));
            const status = gtk.Label.new("");
            status.setXalign(0);
            status.setWrap(1);
            root.append(status.as(gtk.Widget));
            self.status = status;
            const button = gtk.Button.newWithLabel("Switch backend");
            button.as(gtk.Widget).setHalign(.end);
            button.as(gtk.Widget).addCssClass("suggested-action");
            root.append(button.as(gtk.Widget));
            self.switch_button = button;
            _ = gtk.Button.signals.clicked.connect(button, *Page, switchClicked, self, .{});
            w.setChild(root.as(gtk.Widget));
            self.window = w;
        }
        if (self.target == null) {
            const z = self.allocator.dupeZ(u8, path) catch return;
            defer self.allocator.free(z);
            self.entry.?.as(gtk.Editable).setText(z);
        }
        self.window.?.setTransientFor(parent);
        self.window.?.present();
        self.render();
    }
    fn endpointChanged(_: *gtk.Editable, self: *Page) callconv(.c) void {
        if (self.busy or self.status == null) return;
        if (self.preview) |v| v.deinit();
        self.preview = null;
        self.setMessage("Inspect the endpoint before switching.");
    }
    fn previousClicked(_: *gtk.Button, self: *Page) callconv(.c) void {
        const path = self.previous orelse {
            self.problem("No previous backend in this application session.");
            return;
        };
        const z = self.allocator.dupeZ(u8, path) catch return;
        defer self.allocator.free(z);
        self.entry.?.as(gtk.Editable).setText(z);
        inspectClicked(undefined, self);
    }
    fn inspectClicked(_: *gtk.Button, self: *Page) callconv(.c) void {
        if (self.busy) return;
        self.waiting_registration = false;
        if (self.target) |v| self.allocator.free(v);
        self.target = self.allocator.dupe(u8, std.mem.span(self.entry.?.as(gtk.Editable).getText())) catch return;
        if (self.preview) |v| v.deinit();
        self.preview = null;
        self.busy = true;
        self.setMessage("Reading original Session references…");
        self.dispatch(self.context, "capture", "");
    }
    fn switchClicked(_: *gtk.Button, self: *Page) callconv(.c) void {
        if (self.busy or self.preview == null) return;
        const catalog = self.catalog orelse return;
        const bytes = std.json.Stringify.valueAlloc(self.allocator, catalog.value, .{}) catch return;
        defer self.allocator.free(bytes);
        self.busy = true;
        self.setMessage("Saving Workspaces before switching…");
        self.dispatch(self.context, "save_protected", bytes);
    }
    fn local(self: *Page, request: anytype) ?std.json.Parsed(Value) {
        const result = transfer.local(self.allocator, request) catch {
            self.problem("Cannot prepare Workspace references.");
            return null;
        };
        if (field(result.value, "error") == .string) {
            self.problem(string(result.value, "error"));
            result.deinit();
            return null;
        }
        return result;
    }
    pub fn capture(self: *Page, bytes: []const u8) void {
        const parsed = std.json.parseFromSlice(Value, self.allocator, bytes, .{ .allocate = .alloc_always }) catch {
            self.problem("Cannot read Workspace catalog.");
            return;
        };
        if (self.catalog) |v| v.deinit();
        self.catalog = parsed;
        const prepared = self.local(.{ .action = "prepare_catalog", .catalog = parsed.value }) orelse return;
        if (self.prepared) |v| v.deinit();
        self.prepared = prepared;
        const bindings = field(prepared.value, "bindings");
        const targets = field(prepared.value, "target_ids");
        if (bindings == .array and bindings.array.items.len == 0 and targets == .array and targets.array.items.len == 0) {
            self.inspect();
            return;
        }
        self.host(.{ .type = "export_content", .bindings = bindings, .target_ids = targets });
    }
    fn host(self: *Page, request: anytype) void {
        if (self.pending_host != null) return;
        const id = std.fmt.allocPrint(self.allocator, "backend-host-{x}", .{std.crypto.random.int(u64)}) catch return;
        self.pending_host = id;
        const bytes = std.json.Stringify.valueAlloc(self.allocator, .{ .type = "application_request", .request = .{ .protocol_version = @as(u32, 1), .api = "control_center_v1", .request_id = id, .frontend_session_id = "backend-selection", .expected_control_revision = @as(?u64, null), .command = .{ .type = "host_transfer", .request = request } } }, .{}) catch return;
        defer self.allocator.free(bytes);
        self.dispatch(self.context, "host", bytes);
    }
    fn inspect(self: *Page) void {
        const catalog = self.catalog orelse return;
        const prepared = self.local(.{ .action = "prepare_catalog", .catalog = catalog.value }) orelse return;
        if (self.prepared) |v| v.deinit();
        self.prepared = prepared;
        self.sendBackend(.{ .action = "inspect", .socket_path = self.target orelse "", .sources = field(prepared.value, "sources") });
    }
    fn sendBackend(self: *Page, command: anytype) void {
        if (self.pending_backend != null) return;
        const id = std.fmt.allocPrint(self.allocator, "backend-{x}", .{std.crypto.random.int(u64)}) catch return;
        self.pending_backend = id;
        const raw = std.json.Stringify.valueAlloc(self.allocator, command, .{}) catch return;
        defer self.allocator.free(raw);
        var parsed = std.json.parseFromSlice(Value, self.allocator, raw, .{}) catch return;
        defer parsed.deinit();
        parsed.value.object.put("request_id", .{ .string = id }) catch return;
        const bytes = std.json.Stringify.valueAlloc(self.allocator, parsed.value, .{}) catch return;
        defer self.allocator.free(bytes);
        self.dispatch(self.context, "backend", bytes);
    }
    pub fn receiveBackend(self: *Page, bytes: []const u8) void {
        const parsed = std.json.parseFromSlice(Value, self.allocator, bytes, .{ .allocate = .alloc_always }) catch return;
        defer parsed.deinit();
        if (self.pending_backend) |id| {
            if (!std.mem.eql(u8, id, string(parsed.value, "request_id"))) return;
            self.allocator.free(id);
            self.pending_backend = null;
        } else return;
        if (field(parsed.value, "error") == .string) {
            self.problem(string(parsed.value, "error"));
            self.dispatch(self.context, "recover", "");
            return;
        }
        const switched = field(parsed.value, "switched");
        if (switched == .bool and switched.bool) {
            if (self.previous) |v| self.allocator.free(v);
            self.previous = self.current;
            self.current = self.allocator.dupe(u8, self.target orelse "") catch null;
            self.waiting_registration = true;
            self.setMessage("Connecting to selected backend…");
            self.dispatch(self.context, "switched", "");
            return;
        }
        const encoded = std.json.Stringify.valueAlloc(self.allocator, field(parsed.value, "preview"), .{}) catch return;
        defer self.allocator.free(encoded);
        if (self.preview) |v| v.deinit();
        self.preview = std.json.parseFromSlice(Value, self.allocator, encoded, .{ .allocate = .alloc_always }) catch null;
        self.busy = false;
        self.setMessage("Review the impact below before switching.");
    }
    pub fn receiveHost(self: *Page, bytes: []const u8) bool {
        const id = self.pending_host orelse return false;
        const parsed = std.json.parseFromSlice(Value, self.allocator, bytes, .{}) catch return false;
        defer parsed.deinit();
        const response = field(parsed.value, "response");
        if (!std.mem.eql(u8, id, string(response, "request_id"))) return false;
        self.allocator.free(id);
        self.pending_host = null;
        const result = field(response, "result");
        if (!std.mem.eql(u8, string(result, "type"), "host_transfer")) {
            self.problem(string(field(result, "error"), "message"));
            return true;
        }
        const value = field(result, "result");
        const catalog = self.catalog orelse return true;
        if (std.mem.eql(u8, string(value, "type"), "content_exported")) {
            const protected = self.local(.{ .action = "protect_catalog", .catalog = catalog.value, .bundle = field(value, "bundle"), .sources = field(value, "sources") }) orelse return true;
            self.catalog.?.deinit();
            self.catalog = protected;
            self.inspect();
        } else if (std.mem.eql(u8, string(value, "type"), "resolved")) {
            const resolved = self.local(.{ .action = "resolve_catalog", .catalog = catalog.value, .associations = field(value, "associations"), .polar_instances = if (self.polar_instances) |v| v.value else Value{ .array = std.array_list.Managed(Value).init(self.allocator) } }) orelse return true;
            defer resolved.deinit();
            const encoded = std.json.Stringify.valueAlloc(self.allocator, resolved.value, .{}) catch return true;
            defer self.allocator.free(encoded);
            self.resolving = true;
            self.dispatch(self.context, "save_resolved", encoded);
        } else self.problem("Unexpected backend preparation result.");
        return true;
    }
    pub fn registered(self: *Page, bytes: []const u8, polar: anytype) void {
        if (!self.waiting_registration) return;
        self.waiting_registration = false;
        const encoded = std.json.Stringify.valueAlloc(self.allocator, polar, .{}) catch return;
        defer self.allocator.free(encoded);
        if (self.polar_instances) |v| v.deinit();
        self.polar_instances = std.json.parseFromSlice(Value, self.allocator, encoded, .{ .allocate = .alloc_always }) catch null;
        const catalog = std.json.parseFromSlice(Value, self.allocator, bytes, .{ .allocate = .alloc_always }) catch return;
        if (self.catalog) |v| v.deinit();
        self.catalog = catalog;
        const prepared = self.local(.{ .action = "prepare_catalog", .catalog = catalog.value }) orelse return;
        defer prepared.deinit();
        self.host(.{ .type = "resolve", .sources = field(prepared.value, "sources") });
    }
    pub fn saved(self: *Page, revision: u64, failed: bool) void {
        if (self.save_revision == null or self.save_revision.? != revision) return;
        self.save_revision = null;
        if (failed) {
            self.problem("Workspace save failed; backend selection is unchanged.");
            return;
        }
        if (self.resolving) {
            self.resolving = false;
            self.busy = false;
            self.setMessage("Backend connected. Unavailable original Sessions remain in their Panes; configure missing Hosts in Settings.");
            self.dispatch(self.context, "restore", "");
        } else {
            const preview = self.preview orelse return;
            self.dispatch(self.context, "suspend", "");
            self.sendBackend(.{ .action = "switch", .preview_id = string(preview.value, "preview_id") });
        }
    }
    pub fn requestFailed(self: *Page, id: []const u8, message: []const u8) bool {
        if (self.pending_host) |p| if (std.mem.eql(u8, p, id)) {
            self.allocator.free(p);
            self.pending_host = null;
            self.problem(message);
            return true;
        };
        return false;
    }
    pub fn problem(self: *Page, message: []const u8) void {
        if (self.pending_host) |v| self.allocator.free(v);
        self.pending_host = null;
        if (self.pending_backend) |v| self.allocator.free(v);
        self.pending_backend = null;
        self.save_revision = null;
        self.busy = false;
        self.setMessage(message);
        self.dispatch(self.context, "recover", "");
    }
    fn setMessage(self: *Page, message: []const u8) void {
        const copy = self.allocator.dupe(u8, message) catch return;
        if (self.message) |v| self.allocator.free(v);
        self.message = copy;
        self.render();
    }
    fn render(self: *Page) void {
        const body = self.body orelse return;
        const prepared = if (self.prepared) |v| v.value else Value.null;
        const bytes = std.json.Stringify.valueAlloc(self.allocator, .{ .current = self.current orelse "", .target = self.target orelse "", .preview = if (self.preview) |v| field(v.value, "descriptor") else Value.null, .busy = self.busy, .workspace_count = field(prepared, "workspace_count"), .pane_count = field(prepared, "pane_count"), .references = field(prepared, "references"), .message = self.message }, .{}) catch return;
        defer self.allocator.free(bytes);
        // Absent counts are represented as zero in the Rust input.
        var input = std.json.parseFromSlice(Value, self.allocator, bytes, .{}) catch return;
        defer input.deinit();
        inline for (.{ "workspace_count", "pane_count" }) |key| if (field(input.value, key) == .null) {
            input.value.object.put(key, .{ .integer = 0 }) catch return;
        };
        if (field(input.value, "references") == .null) input.value.object.put("references", .{ .array = std.array_list.Managed(Value).init(self.allocator) }) catch return;
        const request = std.json.Stringify.valueAlloc(self.allocator, input.value, .{}) catch return;
        defer self.allocator.free(request);
        const raw = c.polar_station_backend_view(request.ptr, request.len) orelse return;
        defer c.polar_station_desktop_string_free(raw);
        const view = std.json.parseFromSlice(Value, self.allocator, std.mem.span(raw), .{}) catch return;
        defer view.deinit();
        while (body.as(gtk.Widget).getFirstChild()) |child| body.remove(child);
        inline for (.{ "fields", "impacts" }) |key| {
            const rows = field(view.value, key);
            if (rows == .array) for (rows.array.items) |row| {
                const line = gtk.Box.new(.horizontal, 16);
                const label_box = gtk.Box.new(.vertical, 0);
                label_box.as(gtk.Widget).setSizeRequest(190, -1);
                self.text(label_box, string(row, "label"));
                line.append(label_box.as(gtk.Widget));
                const value_box = gtk.Box.new(.vertical, 0);
                value_box.as(gtk.Widget).setHexpand(1);
                self.text(value_box, string(row, "value"));
                line.append(value_box.as(gtk.Widget));
                body.append(line.as(gtk.Widget));
            };
        }
        const message = self.allocator.dupeZ(u8, string(view.value, "message")) catch return;
        defer self.allocator.free(message);
        self.status.?.setText(message);
        const enabled = field(view.value, "can_switch");
        self.switch_button.?.as(gtk.Widget).setSensitive(@intFromBool(enabled == .bool and enabled.bool));
        self.inspect_button.?.as(gtk.Widget).setSensitive(@intFromBool(!self.busy));
        self.entry.?.as(gtk.Widget).setSensitive(@intFromBool(!self.busy));
    }
};
