//! Native connector drafts. Rust owns configuration revisions and routing policy.
const auxiliary = @import("auxiliary_window.zig");
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const Application = @import("../class/application.zig").Application;
const codex = @import("codex.zig");
const Connector = struct { id: []const u8 = "", host_id: []const u8 = "", provider: []const u8 = "telegram", connection_ref: ?[]const u8 = null, enabled: bool = true, allowed_source_hosts: []const []const u8 = &.{} };
const Route = struct { endpoint_id: []const u8 = "", thread_id: ?[]const u8 = null, connector_id: []const u8 = "", destination_ref: []const u8 = "current", enabled: bool = true };
const Settings = struct { revision: u64 = 0, connectors: []const Connector = &.{}, routes: []const Route = &.{}, disabled_monitors: []const []const u8 = &.{} };
const Stream = struct { id: []const u8, connector_id: []const u8, thread_id: []const u8, title: []const u8, return_host_id: []const u8, return_endpoint_id: []const u8, status: []const u8, error_code: ?[]const u8 = null };
const Frame = struct { type: []const u8, response: struct { api: []const u8, request_id: []const u8, result: struct { kind: []const u8, code: ?[]const u8 = null, result: ?struct { kind: []const u8, host_id: ?[]const u8 = null, settings: ?Settings = null, streams: ?[]const Stream = null, code: ?[]const u8 = null } = null } } };
const ConnectorFields = struct { id: *gtk.Entry, host: *gtk.ComboBoxText, provider: *gtk.ComboBoxText, enabled: *gtk.CheckButton, sources: *gtk.Entry, remove: *gtk.CheckButton };
const RouteFields = struct { endpoint: *gtk.Entry, thread: *gtk.Entry, connector: *gtk.Entry, destination: *gtk.Entry, enabled: *gtk.CheckButton, remove: *gtk.CheckButton };
const Monitor = struct { id: []const u8, enabled: *gtk.CheckButton };
const Phase = enum { settings, sources, save, streams, return_route };
pub const Send = *const fn (?*anyopaque, []const u8) anyerror!void;
pub const Window = struct {
    allocator: std.mem.Allocator,
    app: *Application,
    send: Send,
    context: ?*anyopaque,
    window: ?*adw.Window = null,
    body: ?*gtk.Box = null,
    connector_box: ?*gtk.Box = null,
    route_box: ?*gtk.Box = null,
    status: ?*gtk.Label = null,
    save: ?*gtk.Button = null,
    frame: ?std.json.Parsed(Frame) = null,
    stream_frame: ?std.json.Parsed(Frame) = null,
    stream_connector: ?*gtk.Entry = null,
    stream_selector: ?*gtk.ComboBoxText = null,
    stream_status: ?*gtk.Label = null,
    return_host: ?*gtk.Entry = null,
    return_endpoint: ?*gtk.Entry = null,
    source_frame: ?std.json.Parsed(codex.Frame) = null,
    connectors: std.ArrayList(ConnectorFields) = .empty,
    routes: std.ArrayList(RouteFields) = .empty,
    monitors: std.ArrayList(Monitor) = .empty,
    strings: std.heap.ArenaAllocator,
    pending: ?[]u8 = null,
    phase: Phase = .settings,
    available: bool = false,
    codex_available: bool = false,
    host: ?[]u8 = null,
    sequence: u64 = 0,
    nonce: u64,
    pub fn init(a: std.mem.Allocator, app: *Application, send: Send, context: ?*anyopaque) Window {
        return .{ .allocator = a, .app = app, .send = send, .context = context, .strings = std.heap.ArenaAllocator.init(a), .nonce = std.crypto.random.int(u64) };
    }
    pub fn deinit(self: *Window) void {
        self.clearPending();
        if (self.window) |w| auxiliary.destroy(w);
        if (self.frame) |*f| f.deinit();
        if (self.stream_frame) |*f| f.deinit();
        if (self.source_frame) |*f| f.deinit();
        if (self.host) |h| self.allocator.free(h);
        self.connectors.deinit(self.allocator);
        self.routes.deinit(self.allocator);
        self.monitors.deinit(self.allocator);
        self.strings.deinit();
    }
    pub fn connected(self: *Window, host: []const u8, supported: bool, has_codex: bool) void {
        if (self.host) |h| self.allocator.free(h);
        self.host = self.allocator.dupe(u8, host) catch null;
        self.available = supported;
        self.codex_available = has_codex;
        self.sensitivity();
    }
    pub fn disconnected(self: *Window) void {
        self.available = false;
        self.clearPending();
        self.message("Host disconnected; your draft is preserved.");
        self.sensitivity();
    }
    pub fn changed(self: *Window) void {
        if (self.frame != null and self.pending == null) self.message("Host state changed. Save checks your configuration revision; Reload discards this draft.");
    }
    pub fn present(self: *Window) void {
        self.ensure();
        auxiliary.present(self.app, self.window.?);
        if (self.frame == null) self.request(.settings, .{ .kind = "get_settings" });
    }
    fn z(self: *Window, s: []const u8) [:0]const u8 {
        return self.strings.allocator().dupeZ(u8, s) catch "";
    }
    fn message(self: *Window, s: []const u8) void {
        if (self.status) |l| l.setLabel(self.z(s));
    }
    fn settings(self: *Window) Settings {
        return if (self.frame) |f| if (f.value.response.result.result) |r| r.settings orelse .{} else .{} else .{};
    }
    fn sources(self: *Window) codex.Settings {
        return if (self.source_frame) |f| f.value.response.result.settings orelse .{} else .{};
    }
    fn clearPending(self: *Window) void {
        if (self.pending) |id| self.allocator.free(id);
        self.pending = null;
    }
    fn sensitivity(self: *Window) void {
        if (self.body) |b| b.as(gtk.Widget).setSensitive(@intFromBool(self.available and self.pending == null and self.frame != null));
        if (self.save) |b| b.as(gtk.Widget).setSensitive(@intFromBool(self.available and self.pending == null and self.frame != null));
    }
    fn request(self: *Window, phase: Phase, command: anytype) void {
        if (!self.available or self.pending != null) return;
        self.sequence +%= 1;
        const id = std.fmt.allocPrint(self.allocator, "connector-{x}-{d}", .{ self.nonce, self.sequence }) catch return;
        const bytes = if (phase == .sources) codex.encode(self.allocator, id, self.host orelse "", command) catch {
            self.allocator.free(id);
            return;
        } else std.json.Stringify.valueAlloc(self.allocator, .{ .type = "application_request", .request = .{ .protocol_version = 1, .api = "notification_v1", .request_id = id, .command = .{ .kind = "connector", .command = command } } }, .{}) catch {
            self.allocator.free(id);
            return;
        };
        defer self.allocator.free(bytes);
        self.pending = id;
        self.phase = phase;
        self.sensitivity();
        self.send(self.context, bytes) catch {
            self.clearPending();
            self.message("Request failed; your draft is preserved.");
            self.sensitivity();
        };
    }
    pub fn requestFailed(self: *Window, id: []const u8, failure_message: []const u8) bool {
        const pending = self.pending orelse return false;
        if (!std.mem.eql(u8, pending, id)) return false;
        self.clearPending();
        self.message(failure_message);
        self.sensitivity();
        return true;
    }
    pub fn receive(self: *Window, payload: []const u8) void {
        const id = self.pending orelse return;
        if (self.phase == .sources) {
            var parsed = codex.decode(self.allocator, payload) catch return;
            if (!std.mem.eql(u8, parsed.value.response.request_id, id)) {
                parsed.deinit();
                return;
            }
            self.clearPending();
            if (self.source_frame) |*f| f.deinit();
            self.source_frame = parsed;
            self.render();
            return;
        }
        var parsed = std.json.parseFromSlice(Frame, self.allocator, payload, .{ .ignore_unknown_fields = true, .allocate = .alloc_always }) catch return;
        if (!std.mem.eql(u8, parsed.value.response.request_id, id) or !std.mem.eql(u8, parsed.value.response.api, "notification_v1")) {
            parsed.deinit();
            return;
        }
        const phase = self.phase;
        self.clearPending();
        const result = parsed.value.response.result.result orelse {
            self.message(parsed.value.response.result.code orelse "Connector unavailable");
            parsed.deinit();
            self.sensitivity();
            return;
        };
        if (phase == .streams) {
            if (result.streams) |streams| {
                if (self.stream_selector) |selector| {
                    selector.removeAll();
                    for (streams) |stream| selector.append(self.z(stream.id), self.z(stream.title));
                }
                if (self.stream_frame) |*f| f.deinit();
                self.stream_frame = parsed;
                if (streams.len > 0) _ = self.stream_selector.?.as(gtk.ComboBox).setActiveId(self.z(streams[0].id));
                self.message(if (streams.len == 0) "No message streams." else "Message streams loaded.");
            } else {
                self.message(result.code orelse "Streams unavailable");
                parsed.deinit();
            }
            self.sensitivity();
            return;
        }
        if (phase == .return_route) {
            self.message(if (std.mem.eql(u8, result.kind, "acknowledged")) "Reply destination updated. Reload streams before another change." else result.code orelse "Reply destination update failed");
            parsed.deinit();
            self.sensitivity();
            return;
        }
        if (result.settings == null) {
            self.message(result.code orelse "Unexpected connector response");
            parsed.deinit();
            self.sensitivity();
            return;
        }
        if (self.frame) |*f| f.deinit();
        self.frame = parsed;
        if (phase == .settings and self.codex_available) {
            self.request(.sources, .{ .kind = "get_settings" });
        } else {
            self.render();
        }
    }
    fn ensure(self: *Window) void {
        if (self.window != null) return;
        const w = auxiliary.create(self.app);
        w.as(gtk.Window).setTitle("Codex monitoring and messages");
        w.as(gtk.Window).setDefaultSize(680, 700);
        w.as(gtk.Window).setHideOnClose(1);
        w.as(gtk.Window).setIconName("com.polarealm.PolarStation");
        const root = gtk.Box.new(.vertical, 10);
        inline for (.{ "setMarginTop", "setMarginBottom", "setMarginStart", "setMarginEnd" }) |method| {
            @field(gtk.Widget, method)(root.as(gtk.Widget), 16);
        }
        self.status = gtk.Label.new("Select monitoring sources and message senders independently.");
        self.status.?.setWrap(1);
        self.status.?.setSelectable(1);
        root.append(self.status.?.as(gtk.Widget));
        const scroll = gtk.ScrolledWindow.new();
        scroll.as(gtk.Widget).setVexpand(1);
        self.body = gtk.Box.new(.vertical, 10);
        scroll.setChild(self.body.?.as(gtk.Widget));
        root.append(scroll.as(gtk.Widget));
        const footer = gtk.Box.new(.horizontal, 8);
        const reload = gtk.Button.newWithLabel("Reload and discard draft");
        connect(reload, reloadClicked, self);
        footer.append(reload.as(gtk.Widget));
        self.save = gtk.Button.newWithLabel("Save");
        connect(self.save.?, saveClicked, self);
        footer.append(self.save.?.as(gtk.Widget));
        root.append(footer.as(gtk.Widget));
        const toolbar = adw.ToolbarView.new();
        toolbar.addTopBar(adw.HeaderBar.new().as(gtk.Widget));
        toolbar.setContent(root.as(gtk.Widget));
        w.setContent(toolbar.as(gtk.Widget));
        self.window = w;
        self.sensitivity();
    }
    fn label(self: *Window, box: *gtk.Box, value: []const u8) void {
        const l = gtk.Label.new(self.z(value));
        l.setWrap(1);
        l.setXalign(0);
        box.append(l.as(gtk.Widget));
    }
    fn entry(self: *Window, box: *gtk.Box, title: []const u8, value: []const u8) *gtk.Entry {
        self.label(box, title);
        const e = gtk.Entry.new();
        e.as(gtk.Editable).setText(self.z(value));
        box.append(e.as(gtk.Widget));
        return e;
    }
    fn check(self: *Window, box: *gtk.Box, title: []const u8, enabled: bool) *gtk.CheckButton {
        const c = gtk.CheckButton.newWithLabel(self.z(title));
        c.setActive(@intFromBool(enabled));
        box.append(c.as(gtk.Widget));
        return c;
    }
    fn render(self: *Window) void {
        const body = self.body orelse return;
        while (body.as(gtk.Widget).getFirstChild()) |child| body.remove(child);
        self.connectors.clearRetainingCapacity();
        self.routes.clearRetainingCapacity();
        self.monitors.clearRetainingCapacity();
        _ = self.strings.reset(.retain_capacity);
        self.message("Edit this Host’s routing. Save validates the configuration; a conflict preserves your draft.");
        const cfg = self.settings();
        self.label(body, "App-server monitoring");
        for (self.sources().endpoints) |e| {
            var enabled = true;
            for (cfg.disabled_monitors) |id| {
                if (std.mem.eql(u8, id, e.id)) {
                    enabled = false;
                    break;
                }
            }
            self.monitors.append(self.allocator, .{ .id = e.id, .enabled = self.check(body, e.id, enabled) }) catch return;
        }
        self.label(body, "Message connectors");
        self.connector_box = gtk.Box.new(.vertical, 12);
        body.append(self.connector_box.?.as(gtk.Widget));
        for (cfg.connectors) |c| self.addConnector(c);
        const add = gtk.Button.newWithLabel("Add connector");
        connect(add, addConnectorClicked, self);
        body.append(add.as(gtk.Widget));
        self.label(body, "Routes — leave session ID empty for all sessions");
        self.route_box = gtk.Box.new(.vertical, 12);
        body.append(self.route_box.?.as(gtk.Widget));
        for (cfg.routes) |r| self.addRoute(r);
        const add_route = gtk.Button.newWithLabel("Add route");
        connect(add_route, addRouteClicked, self);
        body.append(add_route.as(gtk.Widget));
        self.label(body, "Message streams on this Host");
        self.stream_connector = self.entry(body, "Saved local connector name", "");
        const streams_button = gtk.Button.newWithLabel("Load message streams");
        connect(streams_button, streamsClicked, self);
        body.append(streams_button.as(gtk.Widget));
        self.stream_selector = gtk.ComboBoxText.new();
        _ = gtk.ComboBox.signals.changed.connect(self.stream_selector.?.as(gtk.ComboBox), *Window, streamChanged, self, .{});
        body.append(self.stream_selector.?.as(gtk.Widget));
        self.stream_status = gtk.Label.new("");
        self.stream_status.?.setWrap(1);
        self.stream_status.?.setSelectable(1);
        body.append(self.stream_status.?.as(gtk.Widget));
        self.return_host = self.entry(body, "Reply Host ID", "");
        self.return_endpoint = self.entry(body, "App-server source ID on the reply Host", "");
        self.label(body, "Pending or uncertain replies must be resolved before changing destination.");
        const return_button = gtk.Button.newWithLabel("Apply reply destination");
        connect(return_button, returnClicked, self);
        body.append(return_button.as(gtk.Widget));
        self.sensitivity();
    }
    fn selectedStream(self: *Window) ?Stream {
        const selected = combo(self.stream_selector orelse return null);
        const frame = self.stream_frame orelse return null;
        const result = frame.value.response.result.result orelse return null;
        for (result.streams orelse return null) |stream| {
            if (std.mem.eql(u8, selected, stream.id)) return stream;
        }
        return null;
    }
    fn addConnector(self: *Window, c: Connector) void {
        const box = gtk.Box.new(.vertical, 5);
        self.connector_box.?.append(box.as(gtk.Widget));
        const id = self.entry(box, "Connector name", c.id);
        const provider = gtk.ComboBoxText.new();
        provider.append("telegram", "Telegram");
        provider.append("imessage", "iMessage");
        _ = provider.as(gtk.ComboBox).setActiveId(self.z(c.provider));
        box.append(provider.as(gtk.Widget));
        self.label(box, "Host");
        const host = gtk.ComboBoxText.new();
        host.append("", "This Host");
        for (self.sources().host_connections) |h| host.append(self.z(h.id), self.z(h.id));
        if (c.connection_ref) |ref| {
            var found = false;
            for (self.sources().host_connections) |h| {
                if (std.mem.eql(u8, h.id, ref)) {
                    found = true;
                    break;
                }
            }
            if (!found) host.append(self.z(ref), self.z(ref));
        }
        _ = host.as(gtk.ComboBox).setActiveId(self.z(c.connection_ref orelse ""));
        box.append(host.as(gtk.Widget));
        const joined = std.mem.join(self.strings.allocator(), ",", c.allowed_source_hosts) catch "";
        const sources_field = self.entry(box, "Allowed source Host IDs (comma separated)", joined);
        self.label(box, "Telegram needs private Topics enabled. Native iMessage threads remain unavailable pending Messages integration verification.");
        self.connectors.append(self.allocator, .{ .id = id, .host = host, .provider = provider, .sources = sources_field, .enabled = self.check(box, "Enabled", c.enabled), .remove = self.check(box, "Remove on save", false) }) catch {};
    }
    fn addRoute(self: *Window, r: Route) void {
        const box = gtk.Box.new(.vertical, 5);
        self.route_box.?.append(box.as(gtk.Widget));
        self.routes.append(self.allocator, .{ .endpoint = self.entry(box, "Monitoring source", r.endpoint_id), .thread = self.entry(box, "Session ID (optional)", r.thread_id orelse ""), .connector = self.entry(box, "Connector name", r.connector_id), .destination = self.entry(box, "Destination name", r.destination_ref), .enabled = self.check(box, "Send completions", r.enabled), .remove = self.check(box, "Remove on save", false) }) catch {};
    }
    fn saveDraft(self: *Window) void {
        var arena = std.heap.ArenaAllocator.init(self.allocator);
        defer arena.deinit();
        const a = arena.allocator();
        var connectors: std.ArrayList(Connector) = .empty;
        var routes: std.ArrayList(Route) = .empty;
        var disabled: std.ArrayList([]const u8) = .empty;
        for (self.connectors.items, 0..) |c, i| {
            if (c.remove.getActive() != 0) continue;
            const ref = combo(c.host);
            var host = self.host orelse "";
            if (ref.len > 0) {
                var found = false;
                for (self.sources().host_connections) |h| {
                    if (std.mem.eql(u8, ref, h.id)) {
                        host = h.host_id;
                        found = true;
                        break;
                    }
                }
                if (!found and i < self.settings().connectors.len) host = self.settings().connectors[i].host_id;
            }
            var allowed: std.ArrayList([]const u8) = .empty;
            var parts = std.mem.splitScalar(u8, text(c.sources), ',');
            while (parts.next()) |part| {
                const item = std.mem.trim(u8, part, " \t\r\n");
                if (item.len > 0) allowed.append(a, item) catch return;
            }
            connectors.append(a, .{ .id = text(c.id), .host_id = host, .provider = combo(c.provider), .connection_ref = if (ref.len == 0) null else ref, .enabled = c.enabled.getActive() != 0, .allowed_source_hosts = allowed.items }) catch return;
        }
        for (self.routes.items) |r| {
            if (r.remove.getActive() != 0) continue;
            const thread = text(r.thread);
            routes.append(a, .{ .endpoint_id = text(r.endpoint), .thread_id = if (thread.len == 0) null else thread, .connector_id = text(r.connector), .destination_ref = text(r.destination), .enabled = r.enabled.getActive() != 0 }) catch return;
        }
        // Preserve disabled IDs for sources not visible in this snapshot.
        for (self.settings().disabled_monitors) |id| {
            var visible = false;
            for (self.monitors.items) |m| {
                if (std.mem.eql(u8, id, m.id)) {
                    visible = true;
                    break;
                }
            }
            if (!visible) disabled.append(a, id) catch return;
        }
        for (self.monitors.items) |m| {
            if (m.enabled.getActive() == 0) disabled.append(a, m.id) catch return;
        }
        const cfg = Settings{ .revision = self.settings().revision, .connectors = connectors.items, .routes = routes.items, .disabled_monitors = disabled.items };
        self.request(.save, .{ .kind = "update_settings", .expected_revision = cfg.revision, .settings = cfg });
    }
};
fn text(e: *gtk.Entry) []const u8 {
    return std.mem.span(e.as(gtk.Editable).getText());
}
fn combo(c: *gtk.ComboBoxText) []const u8 {
    return if (c.as(gtk.ComboBox).getActiveId()) |id| std.mem.span(id) else "";
}
fn connect(b: *gtk.Button, cb: *const fn (*gtk.Button, *Window) callconv(.c) void, w: *Window) void {
    _ = gtk.Button.signals.clicked.connect(b, *Window, cb, w, .{});
}
fn reloadClicked(_: *gtk.Button, w: *Window) callconv(.c) void {
    w.request(.settings, .{ .kind = "get_settings" });
}
fn saveClicked(_: *gtk.Button, w: *Window) callconv(.c) void {
    w.saveDraft();
}
fn addConnectorClicked(_: *gtk.Button, w: *Window) callconv(.c) void {
    w.addConnector(.{});
}
fn addRouteClicked(_: *gtk.Button, w: *Window) callconv(.c) void {
    w.addRoute(.{});
}

fn streamsClicked(_: *gtk.Button, w: *Window) callconv(.c) void {
    w.request(.streams, .{ .kind = "get_streams", .connector_id = text(w.stream_connector orelse return) });
}
fn streamChanged(_: *gtk.ComboBox, w: *Window) callconv(.c) void {
    const stream = w.selectedStream() orelse return;
    w.return_host.?.as(gtk.Editable).setText(w.z(stream.return_host_id));
    w.return_endpoint.?.as(gtk.Editable).setText(w.z(stream.return_endpoint_id));
    const status = std.fmt.allocPrint(w.strings.allocator(), "{s} · {s}\nID: {s}\n{s}", .{ stream.title, stream.status, stream.thread_id, stream.error_code orelse "" }) catch return;
    w.stream_status.?.setLabel(w.z(status));
}
fn returnClicked(_: *gtk.Button, w: *Window) callconv(.c) void {
    const stream = w.selectedStream() orelse return;
    w.request(.return_route, .{ .kind = "set_return_route", .stream_id = stream.id, .expected_host_id = stream.return_host_id, .host_id = text(w.return_host.?), .endpoint_id = text(w.return_endpoint.?) });
}
