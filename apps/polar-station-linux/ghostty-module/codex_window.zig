//! Native Session form. Rust owns binding, actions, labels and receipt meaning.
const auxiliary = @import("auxiliary_window.zig");
const std = @import("std");
const adw = @import("adw");
const gtk = @import("gtk");
const glib = @import("glib");
const Application = @import("../class/application.zig").Application;
const wire = @import("codex.zig");
const Key = @import("session_runtime.zig").KeyWire;
const endpoint_form = @import("codex_endpoint_window.zig");
const Phase = enum { configure, ui, settings, bindings, binding, loaded, destinations, mutation, server_start };
const Pending = struct { id: []u8, owner: []u8, phase: Phase };
const Click = struct { window: *Window, kind: enum { endpoint, thread, binding, action }, index: usize };
pub const Send = *const fn (?*anyopaque, []const u8) anyerror!void;

pub const Window = struct {
    editor: ?*endpoint_form.Window = null,
    configure_requested: bool = false,
    configuration_machine: ?[]u8 = null,
    allocator: std.mem.Allocator,
    app: *Application,
    send: Send,
    context: ?*anyopaque,
    window: ?*adw.Window = null,
    body: ?*gtk.Box = null,
    actions: ?*gtk.Box = null,
    notification_box: ?*gtk.Box = null,
    input: ?*gtk.TextView = null,
    status: ?*gtk.Label = null,
    outcome: ?*gtk.Label = null,
    telegram: ?*gtk.ComboBoxText = null,
    imessage: ?*gtk.CheckButton = null,
    refresh_button: ?*gtk.Button = null,
    notifications_dirty: bool = false,
    rendering: bool = false,
    host: ?[]u8 = null,
    session: ?Key = null,
    all_sessions: bool = false,
    filtered_target: ?[]u8 = null,
    original_owner: ?[]u8 = null,
    original_binding: ?[]u8 = null,
    endpoint: ?[]u8 = null,
    supported: bool = false,
    available: bool = false,
    sequence: u64 = 0,
    nonce: u64,
    pending: ?Pending = null,
    start_origin:?struct{controller:u64,token:u64}=null,
    server_started:?*const fn(?*anyopaque,u64,u64,?[]const u8) void=null,
    dirty: bool = false,
    observation_error: bool = false,
    frames: [6]?std.json.Parsed(wire.Frame) = .{null} ** 6,
    click_arena: std.heap.ArenaAllocator,
    confirmation: ?*adw.MessageDialog = null,
    confirmation_payload: ?[]u8 = null,

    pub fn init(allocator: std.mem.Allocator, app: *Application, send: Send, context: ?*anyopaque) Window {
        return .{ .allocator = allocator, .app = app, .send = send, .context = context, .nonce = std.crypto.random.int(u64), .click_arena = std.heap.ArenaAllocator.init(allocator) };
    }
    pub fn deinit(self: *Window) void {
        if (self.filtered_target) |target| self.allocator.free(target);
        if (self.configuration_machine) |id| self.allocator.free(id);
        self.clearPending();
        self.cancelConfirmation();
        if (self.editor) |editor| { editor.deinit(); self.allocator.destroy(editor); }
        if (self.window) |w| auxiliary.destroy(w);
        for (&self.frames) |*frame| if (frame.*) |*value| value.deinit();
        self.click_arena.deinit();
        self.freeSession();
        self.clearOriginal();
        if (self.host) |value| self.allocator.free(value);
        if (self.endpoint) |value| self.allocator.free(value);
    }
    pub fn startLocalServer(self:*Window,owner:[]const u8,endpoint:[]const u8,controller:u64,token:u64) void {
        if(self.pending!=null or !self.available or !self.supported){if(self.server_started) |notify| notify(self.context,controller,token,"Codex control is busy or unavailable");return;}
        self.start_origin=.{.controller=controller,.token=token};
        self.request(owner,.server_start,.{.kind="start_local_server",.endpoint_id=endpoint});
        if(self.pending==null)self.completeServerStart("Codex start request was not admitted");
    }
    fn completeServerStart(self:*Window,message:?[]const u8) void {
        const origin=self.start_origin orelse return;self.start_origin=null;
        if(self.server_started) |notify| notify(self.context,origin.controller,origin.token,message);
    }
    pub fn connected(self: *Window, host: []const u8, supported: bool) void {
        if (self.host) |value| self.allocator.free(value);
        self.host = self.allocator.dupe(u8, host) catch null;
        self.supported = supported;
        self.available = true;
        if (self.window != null) self.refresh();
    }
    pub fn disconnected(self: *Window) void {
        self.completeServerStart("The Host disconnected while starting Codex");
        self.available = false;
        self.clearPending();
        self.cancelConfirmation();
        if (self.window != null) {
            self.showError();
            self.sensitivity();
        }
    }
    fn freeSession(self: *Window) void {
        if (self.session) |key| {
            self.allocator.free(key.execution_target);
            self.allocator.free(key.tmux_session_id);
        }
        self.session = null;
    }
    pub fn present(self: *Window, key: Key) void {
        self.clearOriginal();
        self.all_sessions = false;
        self.ensureWindow();
        // Closing and reopening this Session preserves its unsent draft.
        if (self.session == null or !sameKey(self.session.?, key)) {
            self.clearPending();
            self.cancelConfirmation();
            self.freeSession();
            const target = self.allocator.dupe(u8, key.execution_target) catch return;
            const session = self.allocator.dupe(u8, key.tmux_session_id) catch {
                self.allocator.free(target);
                return;
            };
            self.session = .{ .execution_target = target, .tmux_session_id = session };
            if (self.endpoint) |value| self.allocator.free(value);
            self.endpoint = null;
            self.input.?.getBuffer().setText("", 0);
            self.notifications_dirty = false;
            self.dropFrame(2);
            self.dropFrame(3);
            self.dropFrame(4);
            self.rebuild();
        }
        auxiliary.present(self.app, self.window.?);
        self.refresh();
    }
    pub fn presentSettings(self: *Window) void { self.presentSettingsForMachine(null); }
    pub fn presentSettingsForMachine(self: *Window, machine: ?[]const u8) void {
        if (self.configuration_machine) |old| self.allocator.free(old);
        self.configuration_machine = if (machine) |id| self.allocator.dupe(u8, id) catch null else null;
        self.configure_requested = true;
        self.ensureWindow();
        self.refresh();
    }
    fn configure(self: *Window, settings: wire.Settings) void {
        if (self.editor == null) {
            const editor = self.allocator.create(endpoint_form.Window) catch return;
            editor.* = endpoint_form.Window.init(self.allocator, self.app, saveEndpoints, self);
            self.editor = editor;
        }
        self.editor.?.presentForMachine(settings, self.host orelse return, self.configuration_machine);
    }
    fn saveEndpoints(context: ?*anyopaque, settings: wire.Settings) void {
        const self: *Window = @ptrCast(@alignCast(context.?));
        self.request(self.host orelse return, .configure, .{ .kind = "update_settings", .expected_revision = settings.revision, .settings = settings });
    }
    fn clearOriginal(self: *Window) void {
        if (self.original_owner) |value| self.allocator.free(value);
        if (self.original_binding) |value| self.allocator.free(value);
        self.original_owner = null; self.original_binding = null;
    }
    pub fn presentOriginal(self: *Window, owner: []const u8, binding: []const u8) void {
        self.presentAll();
        self.original_owner = self.allocator.dupe(u8, owner) catch return;
        self.original_binding = self.allocator.dupe(u8, binding) catch return;
    }
    pub fn presentAll(self: *Window) void {
        if (self.filtered_target) |target| self.allocator.free(target);
        self.filtered_target = null;
        self.clearOriginal();
        self.ensureWindow();
        self.clearPending();
        self.cancelConfirmation();
        self.freeSession();
        self.all_sessions = true;
        if (self.endpoint) |value| self.allocator.free(value);
        self.endpoint = null;
        self.dropFrame(2);
        self.dropFrame(3);
        self.dropFrame(4);
        self.dropFrame(5);
        self.notifications_dirty = false;
        self.input.?.getBuffer().setText("", 0);
        auxiliary.present(self.app, self.window.?);
        self.refresh();
    }
    pub fn presentAllForTarget(self: *Window, target_id: []const u8) void {
        self.presentAll();
        self.filtered_target = self.allocator.dupe(u8, target_id) catch null;
        // Refresh the original Host's projected inventory with this selection.
        self.rebuild();
    }
    pub fn changed(self: *Window) void {
        if (self.window == null or self.window.?.as(gtk.Widget).getVisible() == 0) return;
        if (self.pending != null) {
            self.dirty = true;
            return;
        }
        self.refreshBinding();
    }
    fn result(self: *Window, slot: usize) ?wire.Result {
        return if (self.frames[slot]) |frame| frame.value.response.result else null;
    }
    fn currentState(self: *Window) ?wire.State {
        return if (self.result(2)) |r| r.binding else null;
    }
    fn selectedEndpoint(self: *Window) ?wire.Endpoint {
        const id = self.endpoint orelse return null;
        const settings = (self.result(1) orelse return null).settings orelse return null;
        for (settings.endpoints) |endpoint| if (std.mem.eql(u8, endpoint.id, id)) return endpoint;
        return null;
    }
    fn text(self: *Window, id: []const u8) []const u8 {
        if (self.result(0)) |r| if (r.labels) |labels| return labels.map.get(id) orelse "";
        if (std.mem.eql(u8, id, "title")) return "Codex";
        if (std.mem.eql(u8, id, "close")) return "Close";
        // Transport fallback is diagnostic; supported Hosts provide product copy.
        if (std.mem.eql(u8, id, "operation_failed")) return "Codex control unavailable";
        return "";
    }
    fn nextRequestID(self: *Window) ?[]u8 {
        self.sequence +%= 1;
        return std.fmt.allocPrint(self.allocator, "codex-{x}-{d}", .{ self.nonce, self.sequence }) catch null;
    }
    fn request(self: *Window, owner: []const u8, phase: Phase, command: anytype) void {
        if (self.pending != null or !self.available or !self.supported) return;
        const request_id = self.nextRequestID() orelse return;
        defer self.allocator.free(request_id);
        const payload = wire.encode(self.allocator, request_id, owner, command) catch return;
        defer self.allocator.free(payload);
        self.sendPayload(payload, phase);
    }
    fn sendPayload(self: *Window, payload: []const u8, phase: Phase) void {
        if (self.pending != null or !self.available or !self.supported) return;
        const Request = struct { request: struct { request_id: []const u8, owner_host_id: []const u8 } };
        var value = std.json.parseFromSlice(Request, self.allocator, payload, .{ .ignore_unknown_fields = true }) catch return;
        defer value.deinit();
        const request_id = self.allocator.dupe(u8, value.value.request.request_id) catch return;
        const owner = self.allocator.dupe(u8, value.value.request.owner_host_id) catch {
            self.allocator.free(request_id);
            return;
        };
        self.pending = .{ .id = request_id, .owner = owner, .phase = phase };
        self.sensitivity();
        self.send(self.context, payload) catch {
            self.clearPending();
            self.showError();
            self.sensitivity();
        };
    }
    fn clearPending(self: *Window) void {
        if (self.pending) |p| {
            self.allocator.free(p.id);
            self.allocator.free(p.owner);
        }
        self.pending = null;
    }
    pub fn requestFailed(self: *Window, id: []const u8, message: []const u8) bool {
        const pending = self.pending orelse return false;
        if (!std.mem.eql(u8, pending.id, id)) return false;
        if(pending.phase==.server_start)self.completeServerStart(message);
        const configuring = pending.phase == .configure;
        self.clearPending();
        self.observation_error = true;
        if (configuring) { if (self.editor) |editor| editor.result(message); }
        if (self.outcome) |label| self.setLabel(label, message);
        self.sensitivity();
        return true;
    }
    fn refresh(self: *Window) void {
        if (self.pending != null) {
            self.dirty = true;
            return;
        }
        if (!self.available or !self.supported) {
            self.showError();
            self.sensitivity();
            return;
        }
        const owner = self.host orelse return;
        self.request(owner, .ui, .{ .kind = "get_ui" });
    }
    fn refreshBinding(self: *Window) void {
        if (self.original_binding) |binding| {
            self.request(self.original_owner orelse return, .binding, .{ .kind = "get_binding", .binding_id = binding });
            return;
        }
        if (self.all_sessions) {
            if (self.currentState()) |selected| {
                self.request(selected.binding.owner_host_id, .binding, .{ .kind = "get_binding", .binding_id = selected.binding.binding_id });
            } else {
                const owner = if (self.selectedEndpoint()) |e| e.owner_host_id else self.host orelse return;
                self.request(owner, .bindings, .{ .kind = "get_bindings" });
            }
            return;
        }
        const endpoint = self.selectedEndpoint() orelse return;
        self.request(endpoint.owner_host_id, .bindings, .{ .kind = "get_bindings" });
    }
    pub fn receive(self: *Window, payload: []const u8) void {
        const pending = self.pending orelse return;
        var parsed = wire.decode(self.allocator, payload) catch return;
        var retained = false;
        defer if (!retained) parsed.deinit();
        const response = parsed.value.response;
        if (!std.mem.eql(u8, response.request_id, pending.id) or !std.mem.eql(u8, response.owner_host_id, pending.owner)) return;
        const phase = pending.phase;
        self.clearPending();
        defer self.sensitivity();
        const r = response.result;
        if(phase==.server_start){self.completeServerStart(if(std.mem.eql(u8,r.kind,"error")) r.code orelse "Codex start failed" else null);return;}
        if (std.mem.eql(u8, r.kind, "error")) {
            if (phase == .configure) { if (self.editor) |editor| editor.result(r.code orelse "Settings were not saved."); }
            self.showError();
            self.sensitivity();
            return;
        }
        switch (phase) {
            .server_start=>unreachable,
            .configure => {
                if (self.editor) |editor| editor.result(if (r.settings != null) null else "Save outcome is unknown. Reopen before retrying.");
                self.refresh();
            },
            .ui => {
                self.store(0, parsed);
                retained = true;
                self.request(self.host orelse return, .settings, .{ .kind = "get_settings" });
            },
            .settings => {
                if (r.settings == null) return;
                self.store(1, parsed);
                retained = true;
                self.observation_error = false;
                if (self.configure_requested) {
                    self.configure_requested = false;
                    self.configure(r.settings.?);
                    return;
                }
                if (self.endpoint == null) {
                    var only: ?wire.Endpoint = null;
                    var count: usize = 0;
                    for (r.settings.?.endpoints) |candidate| {
                        if (self.session) |key| {
                            if (!std.mem.eql(u8, candidate.execution_target_id, key.execution_target)) continue;
                        }
                        only = candidate;
                        count += 1;
                    }
                    if (count == 1) self.endpoint = self.allocator.dupe(u8, only.?.id) catch null;
                }
                self.rebuild();
                self.refreshBinding();
            },
            .bindings => {
                if (self.all_sessions) {
                    self.store(5, parsed);
                    retained = true;
                    self.observation_error = false;
                    self.rebuild();
                    return;
                }
                const endpoint = self.selectedEndpoint() orelse return;
                for (r.bindings orelse return) |state| {
                    if (sameKey(state.binding.session orelse continue, self.session orelse return) and std.mem.eql(u8, state.binding.endpoint_id, endpoint.id)) {
                        self.request(endpoint.owner_host_id, .binding, .{ .kind = "get_binding", .binding_id = state.binding.binding_id });
                        return;
                    }
                }
                self.dropFrame(2);
                self.observation_error = false;
                self.rebuild();
                self.request(endpoint.owner_host_id, .loaded, .{ .kind = "list_loaded", .endpoint_id = endpoint.id });
            },
            .binding => {
                if (r.binding == null) return;
                self.store(2, parsed);
                retained = true;
                self.observation_error = false;
                self.rebuild();
                self.request(response.owner_host_id, .destinations, .{ .kind = "get_settings" });
            },
            .destinations => {
                self.store(4, parsed);
                retained = true;
                self.renderNotifications();
            },
            .loaded => {
                self.store(3, parsed);
                retained = true;
                self.rebuild();
            },
            .mutation => {
                if (r.receipt) |receipt| {
                    self.setLabel(self.outcome.?, receipt.label orelse "");
                    if (receipt.clear_input) self.input.?.getBuffer().setText("", 0);
                }
                if (std.mem.eql(u8, r.kind, "binding")) {
                    self.notifications_dirty = false;
                    self.store(2, parsed);
                    retained = true;
                    self.rebuild();
                }
                self.refreshBinding();
            },
        }
        self.sensitivity();
        if (self.pending == null and self.dirty) {
            self.dirty = false;
            self.refreshBinding();
        }
    }
    fn dropFrame(self: *Window, slot: usize) void {
        if (self.frames[slot]) |*frame| frame.deinit();
        self.frames[slot] = null;
    }
    fn store(self: *Window, slot: usize, frame: std.json.Parsed(wire.Frame)) void {
        self.dropFrame(slot);
        self.frames[slot] = frame;
    }
    fn showError(self: *Window) void {
        self.observation_error = true;
        if (self.editor) |editor| if (editor.pending) {
            editor.result("Save outcome is unknown. Reopen settings before retrying.");
        };
        if (self.outcome) |label| self.setLabel(label, self.text("operation_failed"));
    }
    fn sensitivity(self: *Window) void {
        if (self.window == null) return;
        const usable = self.available and self.supported and self.pending == null and self.confirmation == null;
        self.body.?.as(gtk.Widget).setSensitive(@intFromBool(usable and !self.observation_error));
        self.actions.?.as(gtk.Widget).setSensitive(@intFromBool(usable and !self.observation_error));
        self.notification_box.?.as(gtk.Widget).setSensitive(@intFromBool(usable and !self.observation_error));
        self.input.?.as(gtk.Widget).setSensitive(@intFromBool(self.pending == null));
        self.refresh_button.?.as(gtk.Widget).setSensitive(@intFromBool(usable));
    }
    fn ensureWindow(self: *Window) void {
        if (self.window != null) return;
        const window = auxiliary.create(self.app);
        window.as(gtk.Window).setTitle("Codex");
        window.as(gtk.Window).setDefaultSize(620, 640);
        window.as(gtk.Window).setIconName("com.polarealm.PolarStation");
        window.as(gtk.Window).setHideOnClose(1);
        const root = gtk.Box.new(.vertical, 10);
        root.as(gtk.Widget).setMarginTop(16);
        root.as(gtk.Widget).setMarginBottom(16);
        root.as(gtk.Widget).setMarginStart(16);
        root.as(gtk.Widget).setMarginEnd(16);
        const header = gtk.Box.new(.horizontal, 8);
        self.status = gtk.Label.new("Codex");
        self.status.?.setWrap(1);
        self.status.?.setXalign(0);
        self.status.?.as(gtk.Widget).setHexpand(1);
        header.append(self.status.?.as(gtk.Widget));
        const close = gtk.Button.newWithLabel("Close");
        connect(close, Window, closeClicked, self);
        header.append(close.as(gtk.Widget));
        const all = gtk.Button.newWithLabel("All Codex sessions");
        connect(all, Window, allClicked, self);
        header.append(all.as(gtk.Widget));
        root.append(header.as(gtk.Widget));
        const scroll = gtk.ScrolledWindow.new();
        scroll.as(gtk.Widget).setVexpand(1);
        self.body = gtk.Box.new(.vertical, 8);
        scroll.setChild(self.body.?.as(gtk.Widget));
        root.append(scroll.as(gtk.Widget));
        self.notification_box = gtk.Box.new(.horizontal, 8);
        self.telegram = gtk.ComboBoxText.new();
        self.telegram.?.as(gtk.Widget).setHexpand(1);
        self.imessage = gtk.CheckButton.newWithLabel("iMessage");
        _ = gtk.ComboBox.signals.changed.connect(self.telegram.?.as(gtk.ComboBox), *Window, notificationChanged, self, .{});
        _ = gtk.CheckButton.signals.toggled.connect(self.imessage.?, *Window, imessageChanged, self, .{});
        self.notification_box.?.append(self.telegram.?.as(gtk.Widget));
        self.notification_box.?.append(self.imessage.?.as(gtk.Widget));
        const save = gtk.Button.newWithLabel("Save");
        connect(save, Window, saveNotifications, self);
        self.notification_box.?.append(save.as(gtk.Widget));
        root.append(self.notification_box.?.as(gtk.Widget));
        self.input = gtk.TextView.new();
        self.input.?.setWrapMode(.word_char);
        const input_scroll = gtk.ScrolledWindow.new();
        input_scroll.as(gtk.Widget).setSizeRequest(-1, 100);
        input_scroll.setChild(self.input.?.as(gtk.Widget));
        root.append(input_scroll.as(gtk.Widget));
        self.actions = gtk.Box.new(.horizontal, 6);
        root.append(self.actions.?.as(gtk.Widget));
        self.outcome = gtk.Label.new("");
        self.outcome.?.setWrap(1);
        self.outcome.?.setSelectable(1);
        root.append(self.outcome.?.as(gtk.Widget));
        self.refresh_button = gtk.Button.newWithLabel("Refresh");
        connect(self.refresh_button.?, Window, refreshClicked, self);
        root.append(self.refresh_button.?.as(gtk.Widget));
        const toolbar = adw.ToolbarView.new();
        toolbar.addTopBar(adw.HeaderBar.new().as(gtk.Widget));
        toolbar.setContent(root.as(gtk.Widget));
        window.setContent(toolbar.as(gtk.Widget));
        self.window = window;
    }
    fn rebuild(self: *Window) void {
        clearBox(self.body.?);
        clearBox(self.actions.?);
        _ = self.click_arena.reset(.retain_capacity);
        self.setLabel(self.status.?, self.text("title"));
        if (self.currentState()) |state| {
            self.appendLabel(self.body.?, state.thread_name orelse "Unnamed session");
            self.appendLabel(self.body.?, state.binding.thread_id);
            if (state.presentation) |presentation| {
                self.appendLabel(self.body.?, presentation.connection_label);
                self.appendLabel(self.body.?, presentation.task_label);
                self.appendLabel(self.body.?, state.binding.owner_host_id);
                if (presentation.detail) |detail| self.appendLabel(self.body.?, detail);
                self.appendLabel(self.body.?, presentation.input_label);
                for (presentation.actions, 0..) |action, i| self.addButton(self.actions.?, action.label, .action, i, action.enabled);
            }
            if (state.final_answer) |answer| self.appendLabel(self.body.?, answer);
        } else if (self.result(1)) |r| {
            if (r.settings) |settings| {
                var found = false;
                for (settings.endpoints, 0..) |endpoint, i| {
                    if (self.filtered_target) |target| {
                        if (!std.mem.eql(u8, endpoint.execution_target_id, target)) continue;
                    }
                    if (self.session) |key| {
                        if (!std.mem.eql(u8, endpoint.execution_target_id, key.execution_target)) continue;
                    }
                    found = true;
                    self.addButton(self.body.?, endpoint.id, .endpoint, i, true);
                }
                if (!found) self.appendLabel(self.body.?, self.text("no_endpoint"));
            }
            if (self.all_sessions) {
                if (self.result(5)) |catalog| {
                    for (catalog.bindings orelse &.{}, 0..) |entry, i| {
                        if (self.filtered_target) |target| {
                            var belongs = false;
                            if (self.result(1)) |settings_result| {
                                if (settings_result.settings) |settings| {
                                    for (settings.endpoints) |candidate| {
                                        if (std.mem.eql(u8, candidate.id, entry.binding.endpoint_id) and std.mem.eql(u8, candidate.execution_target_id, target)) belongs = true;
                                    }
                                }
                            }
                            if (!belongs) continue;
                        }
                        if (self.endpoint) |id| {
                            if (!std.mem.eql(u8, entry.binding.endpoint_id, id)) continue;
                        }
                        const title = std.fmt.allocPrint(self.click_arena.allocator(), "{s}\n{s} · {s}", .{ entry.thread_name orelse "Unnamed session", entry.binding.thread_id, entry.connection }) catch continue;
                        self.addButton(self.body.?, title, .binding, i, true);
                    }
                }
            }
            if (self.result(3)) |threads| if (threads.threads) |values| {
                if (values.len == 0) self.appendLabel(self.body.?, self.text("no_loaded"));
                for (values, 0..) |thread, i| self.addButton(self.body.?, thread.display_name, .thread, i, true);
            };
        }
        const has_binding = self.currentState() != null;
        self.notification_box.?.as(gtk.Widget).setVisible(@intFromBool(has_binding));
        self.input.?.as(gtk.Widget).setVisible(@intFromBool(has_binding));
        self.renderNotifications();
        self.sensitivity();
    }
    fn renderNotifications(self: *Window) void {
        if (self.notifications_dirty) return;
        self.rendering = true;
        defer self.rendering = false;
        self.telegram.?.removeAll();
        self.telegram.?.append("", self.z(self.text("none")) orelse return);
        if (self.result(4)) |r| if (r.settings) |settings| for (settings.telegram.destinations) |destination| {
            const value = self.z(destination.id) orelse continue;
            self.telegram.?.append(value, value);
        };
        if (self.currentState()) |state| {
            _ = self.telegram.?.as(gtk.ComboBox).setActiveId(self.z(state.binding.telegram_destination_ref orelse "") orelse return);
            self.imessage.?.setActive(@intFromBool(state.binding.imessage_enabled));
        }
    }
    fn addButton(self: *Window, box: *gtk.Box, label: []const u8, kind: @FieldType(Click, "kind"), index: usize, enabled: bool) void {
        const context = self.click_arena.allocator().create(Click) catch return;
        context.* = .{ .window = self, .kind = kind, .index = index };
        const button = gtk.Button.newWithLabel(self.z(label) orelse return);
        button.as(gtk.Widget).setSensitive(@intFromBool(enabled));
        connect(button, Click, clicked, context);
        box.append(button.as(gtk.Widget));
    }
    fn z(self: *Window, value: []const u8) ?[:0]u8 {
        return self.click_arena.allocator().dupeZ(u8, value) catch null;
    }
    fn appendLabel(self: *Window, box: *gtk.Box, value: []const u8) void {
        const label = gtk.Label.new(self.z(value) orelse return);
        label.setWrap(1);
        label.setSelectable(1);
        label.setXalign(0);
        box.append(label.as(gtk.Widget));
    }
    fn setLabel(self: *Window, label: *gtk.Label, value: []const u8) void {
        const owned = self.allocator.dupeZ(u8, value) catch return;
        defer self.allocator.free(owned);
        label.setLabel(owned);
    }
    fn allClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        self.presentAll();
    }
    fn clicked(_: *gtk.Button, context: *Click) callconv(.c) void {
        const self = context.window;
        if (self.pending != null) return;
        switch (context.kind) {
            .endpoint => {
                const settings = (self.result(1) orelse return).settings orelse return;
                const endpoint = settings.endpoints[context.index];
                const value = self.allocator.dupe(u8, endpoint.id) catch return;
                if (self.endpoint) |old| self.allocator.free(old);
                self.endpoint = value;
                self.dropFrame(3);
                self.refreshBinding();
            },
            .binding => {
                const catalog = (self.result(5) orelse return).bindings orelse return;
                const selected = catalog[context.index].binding;
                self.request(selected.owner_host_id, .binding, .{ .kind = "get_binding", .binding_id = selected.binding_id });
            },
            .thread => {
                const endpoint = self.selectedEndpoint() orelse return;
                const threads = (self.result(3) orelse return).threads orelse return;
                self.request(endpoint.owner_host_id, .mutation, .{ .kind = "bind", .endpoint_id = endpoint.id, .thread_id = threads[context.index].thread_id, .session = self.session orelse return });
            },
            .action => {
                const state = self.currentState() orelse return;
                const action = (state.presentation orelse return).actions[context.index];
                const request_id = self.nextRequestID() orelse return;
                defer self.allocator.free(request_id);
                var start: gtk.TextIter = undefined;
                var end: gtk.TextIter = undefined;
                const buffer = self.input.?.getBuffer();
                buffer.getBounds(&start, &end);
                const input = buffer.getText(&start, &end, 1);
                defer glib.free(input);
                const payload = wire.encodeAction(self.allocator, request_id, state, action, std.mem.span(input)) catch return;
                if (action.confirmation) |confirmation| {
                    self.cancelConfirmation();
                    self.confirmation_payload = payload;
                    const dialog = adw.MessageDialog.new(self.window.?.as(gtk.Window), self.z(confirmation.title) orelse return, self.z(confirmation.message) orelse return);
                    dialog.addResponse("cancel", self.z(self.text("cancel")) orelse return);
                    dialog.addResponse("confirm", self.z(confirmation.confirm_label) orelse return);
                    if (confirmation.destructive) dialog.setResponseAppearance("confirm", .destructive);
                    dialog.setCloseResponse("cancel");
                    dialog.setDefaultResponse("cancel");
                    _ = adw.MessageDialog.signals.response.connect(dialog, *Window, confirmed, self, .{});
                    self.confirmation = dialog;
                    dialog.as(gtk.Window).present();
                    self.sensitivity();
                } else {
                    defer self.allocator.free(payload);
                    self.sendPayload(payload, .mutation);
                }
            },
        }
    }
    fn cancelConfirmation(self: *Window) void {
        if (self.confirmation) |dialog| {
            self.confirmation = null;
            dialog.as(gtk.Window).destroy();
        }
        if (self.confirmation_payload) |payload| self.allocator.free(payload);
        self.confirmation_payload = null;
    }
    fn confirmed(_: *adw.MessageDialog, response: [*:0]const u8, self: *Window) callconv(.c) void {
        if (std.mem.eql(u8, std.mem.span(response), "confirm")) if (self.confirmation_payload) |payload| self.sendPayload(payload, .mutation);
        self.cancelConfirmation();
        self.sensitivity();
    }
    fn closeClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        self.window.?.as(gtk.Window).close();
    }
    fn refreshClicked(_: *gtk.Button, self: *Window) callconv(.c) void {
        self.refresh();
    }
    fn notificationChanged(_: *gtk.ComboBox, self: *Window) callconv(.c) void {
        if (!self.rendering) self.notifications_dirty = true;
    }
    fn imessageChanged(_: *gtk.CheckButton, self: *Window) callconv(.c) void {
        if (!self.rendering) self.notifications_dirty = true;
    }
    fn saveNotifications(_: *gtk.Button, self: *Window) callconv(.c) void {
        const state = self.currentState() orelse return;
        const active = self.telegram.?.as(gtk.ComboBox).getActiveId();
        const destination = if (active) |value| std.mem.span(value) else "";
        self.request(state.binding.owner_host_id, .mutation, .{ .kind = "update_notifications", .binding_id = state.binding.binding_id, .expected_revision = state.binding.revision, .telegram_destination_ref = if (destination.len == 0) @as(?[]const u8, null) else destination, .imessage_enabled = self.imessage.?.getActive() != 0 });
    }
};
fn sameKey(a: Key, b: Key) bool {
    return std.mem.eql(u8, a.execution_target, b.execution_target) and std.mem.eql(u8, a.tmux_session_id, b.tmux_session_id);
}
fn clearBox(box: *gtk.Box) void {
    while (box.as(gtk.Widget).getFirstChild()) |child| box.remove(child);
}
fn connect(button: *gtk.Button, comptime T: type, callback: *const fn (*gtk.Button, *T) callconv(.c) void, context: *T) void {
    _ = gtk.Button.signals.clicked.connect(button, *T, callback, context, .{});
}
