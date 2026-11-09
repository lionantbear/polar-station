//! Thin GTK adapter over polar-station-lgp's Rust reducer. No LGP business
//! state, network loop, terminal application, or screen-scraping lives here.
//! The executor owns DisplaySurface; this adapter only borrows it until the
//! matching physical retirement callback and owns native inputs/browsers.
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
const Composer = @import("input_bar.zig").InputBar;
const Browser = @import("input_bar.zig").Browser;
pub const SendFn = *const fn (?*anyopaque, []const u8) anyerror!void;
pub const TerminalWait = struct { pane:[]u8,execution:[]u8,session:[]u8,token:[]u8 };
pub const TerminalViewState = enum { pending, ready, unavailable };
pub const EnsureTerminalFn = *const fn (?*anyopaque,*NativeWindow,[]const u8,[]const u8,[]const u8) anyerror![]const u8;
pub const TerminalStateFn = *const fn (?*anyopaque,*NativeWindow,[]const u8,[]const u8,[]const u8) TerminalViewState;
pub const OpenFn = *const fn (?*anyopaque, *NativeWindow, Content, ?[]const u8) anyerror!void;

/// Called by native Window.init before any GLArea or terminal is created.
/// The content root must never be unparented from a Surface render callback.
pub fn prepareWindow(host: *NativeWindow) void {
    const main = host.as(adw.ApplicationWindow);
    const content = main.getContent() orelse return;
    _ = content.as(gobject.Object).ref();
    main.setContent(null);
    const column = gtk.Box.new(.vertical, 0);
    content.setVexpand(1);
    column.append(content);
    content.as(gobject.Object).unref();
    // The editor grows over content, never resizing the terminal and its selection.
    const reservation = gtk.Box.new(.vertical, 0);
    reservation.as(gtk.Widget).setSizeRequest(-1,40);
    column.append(reservation.as(gtk.Widget));
    const overlay = gtk.Overlay.new();
    overlay.setChild(column.as(gtk.Widget));
    main.setContent(overlay.as(gtk.Widget));
    host.as(gobject.Object).setData("polar-station-composer-host", overlay);
}

pub const Workspace = struct {
    dictate: *dictate_module.Controller,
    allocator: std.mem.Allocator,
    app: *Application,
    context: ?*anyopaque,
    send: SendFn,
    send_session: ?SendFn = null,
    terminal_selection: ?*const fn (?*anyopaque, std.mem.Allocator, *NativeWindow, []const u8) ?[]u8 = null,
    open: OpenFn,
    ensure_terminal: ?EnsureTerminalFn = null,
    terminal_state: ?TerminalStateFn = null,
    retry_host: ?*const fn (?*anyopaque) void = null,
    save_mode: ?*const fn (?*anyopaque, bool) void = null,
    choose_idea_session: ?*const fn (?*anyopaque,*NativeWindow,[]const u8,[]const u8) void = null,
    manage_connections: ?*const fn (?*anyopaque, *NativeWindow) void = null,
    modified_send: bool = false,
    engine: *c.LgpClientHandle,
    panes: std.StringHashMapUnmanaged(*Entry) = .empty,
    composers: std.StringHashMapUnmanaged(*Composer) = .empty,
    browser: ?*Browser = null,
    activity_browser: ?*Browser = null,
    generation: u64 = 0,
    queue: std.ArrayListUnmanaged([]u8) = .empty,
    dispatching: bool = false,
    stopped: bool = false,
    timer: c_uint = 0,
    flush_timer: c_uint = 0,
    draft_timer: c_uint = 0,
    clock: std.time.Timer,

    pub fn create(allocator: std.mem.Allocator, app: *Application, send: SendFn, open: OpenFn, context: ?*anyopaque) !*Workspace {
        const self = try allocator.create(Workspace);
        errdefer allocator.destroy(self);
        const clock = try std.time.Timer.start();
        const engine = c.polar_station_input_new() orelse return error.LgpEngineUnavailable;
        errdefer c.polar_station_input_free(engine);
        const dictate = try dictate_module.Controller.create();
        self.* = .{ .dictate = dictate, .allocator = allocator, .app = app, .send = send, .open = open, .context = context, .engine = engine, .clock = clock };
        dictate.context = self; dictate.changed = dictateChanged; dictate.insert_current = insertDictateCurrent;
        // Lease/deadline maintenance only, never message polling or renderer IO.
        self.timer = glib.timeoutAdd(1000, maintenance, self);
        self.restoreDrafts();
        return self;
    }
    fn insertDictateCurrent(raw:?*anyopaque,text:[]const u8) dictate_module.Controller.Delivery {
        const self:*Workspace=@ptrCast(@alignCast(raw orelse return .no_target));
        if(self.app.as(gtk.Application).getActiveWindow()) |window| if(window.isActive()!=0) {
            if(window.getFocus()) |focus| if(gobject.ext.cast(gtk.TextView,focus)) |editor| {
                var active=self.composers.valueIterator();
                while(active.next()) |slot| if(slot.*.host.as(gtk.Window)==window and slot.*.composing) return .no_target;
                return dictate_module.Controller.insertEditorCurrent(editor,text);
            };
            if(window.getFocus()) |focus| if(gobject.ext.cast(gtk.Editable,focus)!=null) return .no_target;
            var composers=self.composers.valueIterator();
            while(composers.next()) |slot| {
                const composer=slot.*;
                if(composer.host.as(gtk.Window)!=window or !composer.is_idea) continue;
                if(composer.captureDictateNode()) |target| {defer target.port.release(target.port.context);return target.port.insert(target.port.context,text);}
            }
            return .no_target;
        };
        return self.dictate.insertExternalCurrent(text);
    }
    pub fn dictateShortcut(self: *Workspace, pressed: bool, discrete: bool) void {
        const intent = self.dictate.shortcutIntent(pressed, discrete);
        if (intent == 2) { if (pressed) self.dictate.bindHold(); self.dictate.releaseHold(); return; }
        if (intent != 1) return;
        if (std.mem.eql(u8, dictate_module.string(self.dictate.presentation(), "action"), "retry_open")) { self.dictate.input(.{ .type = "retry_open" }); return; }
        if (self.app.as(gtk.Application).getActiveWindow()) |window| {
            if (window.isActive() != 0) {
                var composers = self.composers.valueIterator();
                while (composers.next()) |composer| {
                    if (composer.*.host.as(gtk.Window) == window) {
                        Composer.dictateClicked(composer.*.dictate_button, composer.*);
                        self.dictate.bindHold(); return;
                    }
                }
                if (window.getFocus()) |focus| if (gobject.ext.cast(gtk.TextView, focus)) |editor| {
                    self.dictate.toggle(editor); self.dictate.bindHold(); return;
                };
            }
        }
        // Borrow the actual external editor; never focus a Station window.
        self.dictate.toggleExternal(); self.dictate.bindHold();
    }
    /// Called by existing pane-content events, never by a readiness poll.
    pub fn terminalViewsChanged(self: *Workspace) void {
        var composers = self.composers.valueIterator();
        while (composers.next()) |composer| composer.*.checkTerminalView();
    }
    pub fn bindIdeaSession(self:*Workspace,pane:[]const u8,view:[]const u8,owner:[]const u8,endpoint:[]const u8,thread:[]const u8,title:[]const u8) void {
        self.input(.{.type="idea_pick_session",.pane_id=pane,.view_id=view,.owner_host_id=owner,.endpoint_id=endpoint,.thread_id=thread,.title=title});self.saveSoon();
    }
    pub fn destroy(self: *Workspace) void {
        self.dictate.destroy();
        self.input(.{ .type = "export_drafts" });
        self.stopped = true;
        for ([_]c_uint{ self.timer, self.flush_timer, self.draft_timer }) |id| {
            if (id != 0) {
                _ = glib.Source.remove(id);
            }
        }
        var entries = self.panes.valueIterator();
        while (entries.next()) |entry| entry.*.destroy();
        self.panes.deinit(self.allocator);
        var composers = self.composers.valueIterator();
        while (composers.next()) |composer| composer.*.destroy();
        self.composers.deinit(self.allocator);
        if (self.browser) |browser| browser.destroy();
        if (self.activity_browser) |browser| browser.destroy();
        for (self.queue.items) |item| self.allocator.free(item);
        self.queue.deinit(self.allocator);
        c.polar_station_input_free(self.engine);
        self.allocator.destroy(self);
    }
    fn dictateChanged(context: ?*anyopaque) void {
        const self: *Workspace = @ptrCast(@alignCast(context orelse return));
        var composers = self.composers.valueIterator();
        while (composers.next()) |composer| composer.*.refreshDictate();
    }
    pub fn windowRemoved(self: *Workspace, window: *NativeWindow) void {
        // Executor retires Pane observers first; only then unmount its editor.
        while (true) {
            var found: ?*Composer = null;
            var it = self.composers.valueIterator();
            while (it.next()) |composer| {
                if (composer.*.host == window) {
                    found = composer.*;
                    break;
                }
            }
            const composer = found orelse break;
            composer.commitDraft();
            self.input(.{ .type = "window_closed", .window_id = composer.id });
            _ = self.composers.remove(composer.id);
            composer.destroy();
        }
    }

    pub fn setCachePolicy(self: *Workspace, policy: @import("settings.zig").ChatCache) void {
        self.input(.{ .type = "cache_policy", .policy = policy });
    }
    pub fn setSendMode(self: *Workspace, modified: bool) void {
        self.modified_send = modified;
        var it = self.composers.valueIterator();
        while (it.next()) |composer| composer.*.setSendMode(modified);
    }
    pub fn hooks(self: *Workspace) Hooks {
        return .{ .context = self, .on_created = paneCreated, .on_retired = paneRetired, .on_selected = paneSelected, .on_activated = paneActivated, .on_deselected = paneDeselected };
    }
    pub fn disconnected(self: *Workspace, generation: u64) void {
        if (generation < self.generation) return;
        self.input(.{ .type = "disconnected", .generation = generation });
    }
    pub fn connected(self: *Workspace, generation: u64, supported: bool, host_id: []const u8, codex_supported: bool) void {
        if (generation < self.generation) return;
        self.generation = generation;
        const names: []const []const u8 = if (supported) &.{"polar_access"} else &.{};
        self.input(.{ .type = "connected", .generation = generation, .extensions = names });
        self.input(.{ .type = "idea_host", .host_id = host_id, .supported = codex_supported });
    }
    /// Extension delivery follows the base router's validated registration.
    pub fn sessionCompleted(self: *Workspace, bytes: []const u8) void {
        self.dispatch(bytes);
    }
    pub fn hostFrame(self: *Workspace, generation: u64, bytes: []const u8) bool {
        var parsed = std.json.parseFromSlice(Value, self.allocator, bytes, .{}) catch return false;
        defer parsed.deinit();
        const kind = string(parsed.value, "type") orelse return false;
        if ((std.mem.eql(u8,kind,"application_response") and std.mem.eql(u8,string(field(parsed.value,"response") orelse .null,"api") orelse "","codex_v1")) or (std.mem.eql(u8,kind,"application_state_changed") and std.mem.eql(u8,string(parsed.value,"api") orelse "","codex_v1"))) {
            self.input(.{.type="frame",.generation=generation,.frame=parsed.value});
            return false; // Existing Codex surfaces also observe these frames.
        }
        if (!std.mem.eql(u8, kind, "extension_event") and !std.mem.eql(u8, kind, "extension_response")) return false;
        var composers = self.composers.valueIterator();
        while (composers.next()) |composer| composer.*.inquiryEnvironment();
        self.input(.{ .type = "frame", .generation = generation, .frame = parsed.value });
        return true;
    }
    pub fn ensureWindow(self: *Workspace, host: *NativeWindow) !*Composer {
        var it = self.composers.valueIterator();
        while (it.next()) |composer| if (composer.*.host == host) return composer.*;
        return error.WorkspaceWindowNotBound;
    }
    pub fn bindWindow(self: *Workspace, host: *NativeWindow, workspace_id: []const u8) !*Composer {
        if (self.composers.get(workspace_id)) |composer| {
            if (composer.host != host) return error.WorkspaceAlreadyOpen;
            return composer;
        }
        self.windowRemoved(host);
        const composer = try Composer.create(self, workspace_id, host);
        errdefer composer.destroy();
        try self.composers.put(self.allocator, composer.id, composer);
        return composer;
    }
    pub fn ideaInstruction(self:*Workspace,host:*NativeWindow,edit:bool) bool {
        var it=self.composers.valueIterator();
        while(it.next()) |slot| {const composer=slot.*;if(composer.host!=host or !composer.is_idea)continue;
            const focus=host.as(gtk.Window).getFocus() orelse return false;
            const entry=self.panes.get(composer.pane_id orelse return false) orelse return false;
            const canvas=entry.display.graphics_widget orelse return false;
            if(focus!=canvas and focus.isAncestor(canvas)==0 and focus!=composer.text.as(gtk.Widget) and focus.isAncestor(composer.container.as(gtk.Widget))==0) return false;
            if(edit) graphics.command(canvas,"edit_instruction","") else composer.insertNode();return true;
        }return false;
    }
    pub fn openInquiry(self:*Workspace, host:*NativeWindow) bool {
        var it=self.composers.valueIterator();
        while(it.next()) |composer| if(composer.*.host==host) {
            const cview=composer.*;
            if(cview.inquiry_data) |data| if(boolean(data.value,"active") or number(data.value,"count")>0) {
                cview.commitDraft();cview.inquiryEnvironment();
                self.input(.{.type="inquiry_action",.window_id=cview.id,.action="show"});return true;
            };
        };
        return false;
    }
    pub fn showBrowser(self: *Workspace) void {
        const window = self.app.as(gtk.Application).getActiveWindow() orelse return;
        const host = gobject.ext.cast(NativeWindow, window.as(gobject.Object)) orelse return;
        const composer = self.ensureWindow(host) catch return;
        Composer.chooseClicked(composer.target, composer);
    }
    fn paneDeselected(context: ?*anyopaque, host: *NativeWindow, pane: []const u8, target: []const u8, session: []const u8) void {
        const self: *Workspace = @ptrCast(@alignCast(context orelse return));
        const composer = self.ensureWindow(host) catch return;
        composer.commitDraft();
        self.input(.{ .type="select_terminal", .window_id=composer.id, .pane_id=pane, .target_id=target, .session_id=session });
    }
    fn paneCreated(context: ?*anyopaque, pane: []const u8, window: []const u8, host: *NativeWindow, content: Content, display: *DisplaySurface) void {
        const self: *Workspace = @ptrCast(@alignCast(context orelse return));
        if (self.panes.get(pane)) |previous| paneRetired(context, pane, previous.display);
        const entry = Entry.create(self, pane, window, host, content, display) catch {
            self.showError("Cannot bind native conversation");
            return;
        };
        self.panes.put(self.allocator, entry.pane, entry) catch {
            entry.destroy();
            self.showError("Native view capacity unavailable");
            return;
        };
        if (content.kind == .idea_tree) {
            _ = self.bindWindow(host, window) catch { self.showError("Native input unavailable"); return; };
            entry.ready = true;
            self.input(.{ .type = "open_idea", .pane_id = entry.pane, .window_id = entry.window_id, .view_id = entry.content.view_id });
            if (display.graphics_widget) |widget| graphics.subscribe(widget, Entry.graphicsEvent, entry);
            return;
        }
        entry.init_signal = Surface.signals.init.connect(display.surface, *Entry, Entry.surfaceReady, entry, .{});
        if (display.surface.core() != null) Entry.surfaceReady(display.surface, entry);
    }
    fn paneRetired(context: ?*anyopaque, pane: []const u8, display: *DisplaySurface) void {
        const self: *Workspace = @ptrCast(@alignCast(context orelse return));
        const entry = self.panes.get(pane) orelse return;
        if (entry.display != display) return; // Ignore a retired predecessor.
        _ = self.panes.remove(pane);
        if (entry.ready) self.input(.{ .type = "close", .pane_id = entry.pane });
        entry.destroy();
    }
    fn paneSelected(context: ?*anyopaque, pane: []const u8) void {
        const self: *Workspace = @ptrCast(@alignCast(context orelse return));
        const entry = self.panes.get(pane) orelse return;
        if (!entry.ready) {
            return;
        }
        // Repeated focus notifications while the editor owns focus are not activation.
        if (entry.host.as(gtk.Window).getFocus()) |focused| {
            if (self.composers.get(entry.window_id)) |composer| {
                if (composer.pane_id != null and std.mem.eql(u8, composer.pane_id.?, pane) and
                    (focused == composer.text.as(gtk.Widget) or focused.isAncestor(composer.text.as(gtk.Widget)) != 0)) return;
            }
        }
        if (self.composers.get(entry.window_id)) |composer| composer.commitDraft();
        self.input(.{ .type = "selection_changed", .pane_id = pane });
    }
    fn paneActivated(context: ?*anyopaque, pane: []const u8) void {
        const self: *Workspace = @ptrCast(@alignCast(context orelse return));
        const entry = self.panes.get(pane) orelse return;
        if (entry.host.getActiveSurface()!=entry.display.surface) return;
        if (!entry.ready) {entry.activate_when_ready=true;return;}
        self.input(.{.type="activate_composer",.pane_id=pane});
    }
    pub fn input(self: *Workspace, object: anytype) void {
        const bytes = std.json.Stringify.valueAlloc(self.allocator, object, .{}) catch {
            self.showError("Native request encoding failed");
            return;
        };
        defer self.allocator.free(bytes);
        self.dispatch(bytes);
    }
    pub fn dispatch(self: *Workspace, bytes: []const u8) void {
        if (self.stopped) return;
        if (bytes.len > 1024 * 1024 or self.queue.items.len >= 128) {
            self.showError("Native request queue is full");
            return;
        }
        const copy = self.allocator.dupe(u8, bytes) catch return;
        self.queue.append(self.allocator, copy) catch {
            self.allocator.free(copy);
            return;
        };
        if (self.dispatching) return;
        self.dispatching = true;
        defer self.dispatching = false;
        while (self.queue.items.len > 0) {
            const next = self.queue.orderedRemove(0);
            defer self.allocator.free(next);
            const response = c.polar_station_input_dispatch(self.engine, next.ptr, next.len) orelse {
                self.showError("LGP state unavailable");
                continue;
            };
            defer c.polar_station_desktop_string_free(response);
            var parsed = std.json.parseFromSlice(Value, self.allocator, std.mem.span(response), .{}) catch {
                self.showError("Invalid LGP response");
                continue;
            };
            defer parsed.deinit();
            if (!boolean(parsed.value, "ok")) {
                self.showError(string(parsed.value, "error") orelse "LGP operation failed");
                continue;
            }
            for (array(field(parsed.value, "effects"))) |effect| self.consume(effect);
        }
    }
    fn consume(self: *Workspace, effect: Value) void {
        const kind = string(effect, "type") orelse return;
        if (std.mem.eql(u8, kind, "insert_text")) {
            const lease=field(effect,"lease") orelse return;
            const composer=self.composers.get(string(lease,"window") orelse return) orelse return;
            composer.insertQuotedText(lease,string(effect,"text") orelse return);
        } else if (std.mem.eql(u8, kind, "session_request")) {
            const request = field(effect,"request") orelse return;
            const id = string(request,"request_id") orelse return;
            const bytes = std.json.Stringify.valueAlloc(self.allocator,request,.{}) catch return;
            defer self.allocator.free(bytes);
            const send_session = self.send_session orelse {
                self.input(.{.type="session_completed",.request_id=id,.@"error"="Session transport unavailable"});
                return;
            };
            send_session(self.context,bytes) catch {
                self.input(.{.type="session_completed",.request_id=id,.@"error"="Session request not admitted"});
            };
        } else if (std.mem.eql(u8, kind, "request")) {
            const frame = field(effect, "frame") orelse return;
            const bytes = std.json.Stringify.valueAlloc(self.allocator, frame, .{}) catch return;
            defer self.allocator.free(bytes);
            self.send(self.context, bytes) catch |err| {
                const request = field(frame, "request") orelse return;
                self.input(.{ .type = "request_rejected", .generation = self.generation, .request_id = string(request, "request_id") orelse return, .reason = switch (err) {
                    error.NotConnected => "not_ready",
                    error.OutboundQueueFull => "queue_full",
                    error.ExtensionUnsupported => "unsupported",
                    error.InvalidRequest => "invalid_request",
                    else => "unavailable",
                } });
                return;
            };
            if (field(frame, "request")) |request| {
                self.input(.{ .type = "request_admitted", .generation = self.generation,
                    .request_id = string(request, "request_id") orelse return });
            }
        } else if (std.mem.eql(u8, kind, "reconnect_host")) {
            if (self.retry_host) |retry| retry(self.context);
        } else if (std.mem.eql(u8, kind, "needs_render")) {
            if (self.flush_timer == 0) self.flush_timer = glib.timeoutAdd(16, flush, self);
        } else if (std.mem.eql(u8, kind, "render")) {
            const entry = self.panes.get(string(effect, "pane_id") orelse return) orelse return;
            if (!entry.ready) return;
            const status = entry.display.submit(1, number(effect, "base_revision"), number(effect, "next_revision"), std.math.cast(u32, number(effect, "operation")) orelse return, string(effect, "text") orelse return);
            self.input(.{ .type = "render_result", .pane_id = entry.pane,
                .revision = number(effect, "next_revision"), .accepted = status == 0 });
            // Rejected batches commit no bytes. Keep the last valid frame.
            if (status != 0) self.showError("Display update failed; previous content preserved. Retry the display.");
        } else if (std.mem.eql(u8, kind, "reference_controls")) {
            const entry = self.panes.get(string(effect, "pane_id") orelse return) orelse return;
            if (entry.references) |references| references.update(effect);
        } else if (std.mem.eql(u8, kind, "reading_hint")) {
            const entry = self.panes.get(string(effect, "pane_id") orelse return) orelse return;
            entry.hint_retry = boolean(effect, "retry");
            if (entry.reading_hint) |button| {
                const label = self.allocator.dupeZ(u8, string(effect, "label") orelse "Back to latest") catch return;
                defer self.allocator.free(label);
                button.setLabel(label);
                button.as(gtk.Widget).setVisible(@intFromBool(boolean(effect, "visible")));
            }
        } else if (std.mem.eql(u8, kind, "scroll_to_row")) {
            const entry = self.panes.get(string(effect, "pane_id") orelse return) orelse return;
            if (entry.display.surface.core()) |core| { _ = core.performBindingAction(.{ .scroll_to_row = @intCast(number(effect, "row")) }) catch false; }
            entry.refreshAfterInput();
        } else if (std.mem.eql(u8, kind, "scroll_latest")) {
            const entry = self.panes.get(string(effect, "pane_id") orelse return) orelse return;
            if (entry.display.surface.core()) |core| { _ = core.performBindingAction(.scroll_to_bottom) catch false; }
            entry.refreshAfterInput();
        } else if (std.mem.eql(u8, kind, "idea_command") or std.mem.eql(u8,kind,"idea_locate_reference")) {
            const entry = self.panes.get(string(effect, "pane_id") orelse return) orelse return;
            if (entry.content.kind != .idea_tree or !std.mem.eql(u8, entry.content.view_id, string(effect, "view_id") orelse return)) return;
            const action = self.allocator.dupeZ(u8, string(effect, "action") orelse "locate_reference") catch return;
            defer self.allocator.free(action);
            const reference_json = if (field(effect,"reference")) |ref| std.json.Stringify.valueAlloc(self.allocator,ref,.{}) catch return else null;
            defer if (reference_json) |bytes| self.allocator.free(bytes);
            const query = self.allocator.dupeZ(u8, reference_json orelse string(effect, "query") orelse "") catch return;
            defer self.allocator.free(query);
            if (entry.display.graphics_widget) |widget| graphics.command(widget, action, query);
        } else if (std.mem.eql(u8, kind, "input_bar")) {
            const view = field(effect, "view") orelse return;
            const lease = field(view, "lease") orelse return;
            const id = string(lease, "window") orelse return;
            if (self.composers.get(id)) |composer| composer.updateGeneric(view);
        } else if (std.mem.eql(u8, kind, "composer") or std.mem.eql(u8, kind, "idea_composer")) {
            const id = string(effect, "window_id") orelse return;
            if (std.mem.eql(u8, kind, "idea_composer")) {
                self.saveSoon();
                var entries = self.panes.valueIterator();
                while (entries.next()) |entry| if (std.mem.eql(u8, entry.*.window_id, id)) {
                    entry.*.display.surface.as(gtk.Widget).removeCssClass("station-chat-target");
                    entry.*.display.surface.as(gtk.Widget).setTooltipText(null);
                };
            }
            self.markComposerTarget(id,if (std.mem.eql(u8,kind,"composer")) string(effect,"pane_id") else null);
            if (self.composers.get(id)) |composer| composer.update(effect);
        } else if (std.mem.eql(u8,kind,"ensure_terminal_visible")) {
            const id=string(effect,"window_id") orelse return;
            if (self.composers.get(id)) |composer| composer.beginTerminalView(effect) catch {
                self.input(.{.type="terminal_view_result",.window_id=id,.token=string(effect,"token") orelse "",.ready=false,.@"error"="Target view unavailable"});
            };
        } else if (std.mem.eql(u8, kind, "inquiry")) {
            const id=string(effect,"window_id") orelse return;
            if(self.composers.get(id)) |composer| composer.updateInquiry(field(effect,"view") orelse return,boolean(effect,"focus"));
        } else if (std.mem.eql(u8, kind, "focus_composer")) {
            const id = string(effect, "window_id") orelse return;
            if (self.composers.get(id)) |composer| {
                if (composer.pane_id) |pane| if (std.mem.eql(u8,pane,string(effect,"pane_id") orelse "")) composer.activate();
            }
        } else if (std.mem.eql(u8, kind, "composer_closed")) {
            const id=string(effect,"window_id") orelse return;
            self.markComposerTarget(id,null);
            if (self.composers.get(id)) |composer| composer.clearSelection();
        } else if (std.mem.eql(u8, kind, "targets") or std.mem.eql(u8, kind, "channels") or std.mem.eql(u8, kind, "threads")) {
            if (self.browser) |browser| browser.consume(effect);
            var it = self.composers.valueIterator();
            while (it.next()) |composer| if (composer.*.picker) |picker| picker.consume(effect);
        } else if (std.mem.eql(u8, kind, "status")) {
            if (self.panes.get(string(effect, "pane_id") orelse return)) |entry|
                if (self.composers.get(entry.window_id)) |composer| {
                    if (composer.pane_id) |id| if (std.mem.eql(u8, id, entry.pane)) composer.showStatus(string(effect, "message") orelse "");
                };
        } else if (std.mem.eql(u8, kind, "drafts")) {
            if (field(effect, "value")) |value| self.saveDrafts(value);
        } else if (std.mem.eql(u8, kind, "open_reference_thread")) {
            const composer = self.composers.get(string(effect, "window_id") orelse return) orelse return;
            const reference_scope = field(effect, "scope") orelse return;
            const content: Content = .{ .target_id = string(reference_scope, "target_id") orelse return,
                .node_id = string(reference_scope, "node_id") orelse return, .owner = string(reference_scope, "owner") orelse return,
                .thread = string(reference_scope, "thread") orelse return, .actor = string(reference_scope, "actor") orelse return };
            self.open(self.context, composer.host, content, null) catch self.showError("Cannot open reference Thread");
        } else if (std.mem.eql(u8, kind, "copy_source")) {
            const entry = self.panes.get(string(effect, "pane_id") orelse return) orelse return;
            const text = self.allocator.dupeZ(u8, string(effect, "text") orelse return) catch return;
            defer self.allocator.free(text);
            entry.display.surface.as(gtk.Widget).getClipboard().setText(text);
        } else if (std.mem.eql(u8, kind, "activity")) {
            if (self.activity_browser == null) self.activity_browser = Browser.create(self) catch return;
            self.activity_browser.?.showActivity(field(effect, "items") orelse return, string(effect, "pane_id") orelse return);
        } else if (std.mem.eql(u8, kind, "search_results")) {
            if (self.browser == null) self.browser = Browser.create(self) catch return;
            self.browser.?.showSearch(field(effect, "items") orelse return);
        } else if (std.mem.eql(u8, kind, "browse_error")) {
            var it = self.composers.valueIterator();
            while (it.next()) |composer| if (composer.*.picker) |picker| picker.consume(effect);
            if (self.browser) |browser| setLabel(self.allocator, browser.status, string(effect, "message") orelse "Cannot load conversations");
        } else if (std.mem.eql(u8, kind, "error")) {
            self.showError(string(effect, "message") orelse "LGP operation failed");
        }
    }
    fn markComposerTarget(self:*Workspace,window:[]const u8,pane:?[]const u8) void {
        var it=self.panes.valueIterator();
        while(it.next()) |entry| if(std.mem.eql(u8,entry.*.window_id,window)) {
            const selected=if(pane) |id| std.mem.eql(u8,entry.*.pane,id) else false;
            const widget=entry.*.display.surface.as(gtk.Widget);
            if(selected) widget.addCssClass("station-chat-target") else widget.removeCssClass("station-chat-target");
            widget.setTooltipText(if(selected) "Chat target" else null);
        };
    }
    pub fn showError(self: *Workspace, message: []const u8) void {
        // Error text is never sent into a terminal parser.
        var it = self.composers.valueIterator();
        while (it.next()) |composer| composer.*.showStatus(message);
        if (self.browser) |browser| setLabel(self.allocator, browser.status, message);
        std.log.warn("Polar Station LGP native operation failed", .{});
    }
    fn maintenance(context: ?*anyopaque) callconv(.c) c_int {
        const self: *Workspace = @ptrCast(@alignCast(context orelse return 0));
        if (self.stopped) return 0;
        self.input(.{ .type = "tick", .now_ms = self.clock.read() / std.time.ns_per_ms });
        return 1;
    }
    fn flush(context: ?*anyopaque) callconv(.c) c_int {
        const self: *Workspace = @ptrCast(@alignCast(context orelse return 0));
        self.flush_timer = 0;
        var it = self.panes.valueIterator();
        while (it.next()) |entry| {
            const blocked = entry.*.pointer_down or entry.*.action_timer != 0 or (if (entry.*.display.surface.core()) |core| core.hasSelection() else false);
            if (entry.*.display.surface.core()) |core| self.input(.{ .type = "render_metrics", .pane_id = entry.*.pane, .columns = core.size.grid().columns });
            self.input(.{ .type = "render_guard", .pane_id = entry.*.pane, .blocked = blocked });
            entry.*.reportViewport();
        }
        self.input(.{ .type = "flush" });
        return 0;
    }
    pub fn saveSoon(self: *Workspace) void {
        if (self.draft_timer != 0) _ = glib.Source.remove(self.draft_timer);
        self.draft_timer = glib.timeoutAdd(350, exportDrafts, self);
    }
    fn exportDrafts(context: ?*anyopaque) callconv(.c) c_int {
        const self: *Workspace = @ptrCast(@alignCast(context orelse return 0));
        self.draft_timer = 0;
        self.input(.{ .type = "export_drafts" });
        return 0;
    }
    fn draftPath(self: *Workspace) ![]u8 {
        const base = std.process.getEnvVarOwned(self.allocator, "XDG_STATE_HOME") catch |err| switch (err) {
            error.EnvironmentVariableNotFound => blk: {
                const home = try std.process.getEnvVarOwned(self.allocator, "HOME");
                defer self.allocator.free(home);
                break :blk try std.fs.path.join(self.allocator, &.{ home, ".local", "state" });
            },
            else => return err,
        };
        defer self.allocator.free(base);
        if (!std.fs.path.isAbsolute(base)) return error.InvalidStateDirectory;
        return std.fs.path.join(self.allocator, &.{ base, "polar-station", "lgp-drafts.json" });
    }
    fn saveDrafts(self: *Workspace, value: Value) void {
        self.saveDraftsChecked(value) catch {
            self.showError("Draft persistence failed; keep this input open.");
            if (field(value,"snapshot_revision")) |revision| self.input(.{.type="snapshot_stored",.revision=revision,.success=false});
            return;
        };
        if (field(value,"snapshot_revision")) |revision| self.input(.{.type="snapshot_stored",.revision=revision,.success=true});
    }
    fn saveDraftsChecked(self: *Workspace, value: Value) !void {
        const bytes = try std.json.Stringify.valueAlloc(self.allocator, value, .{});
        defer self.allocator.free(bytes);
        if (bytes.len > 1024 * 1024) return error.DraftLimit;
        const path = try self.draftPath();
        defer self.allocator.free(path);
        const parent = std.fs.path.dirname(path) orelse return error.InvalidStateDirectory;
        try std.fs.cwd().makePath(parent);
        // Unique exclusive temporary file, never a predictable shared '.tmp'.
        const tmp = try std.fmt.allocPrint(self.allocator, "{s}.{x}.tmp", .{ path, std.crypto.random.int(u64) });
        defer self.allocator.free(tmp);
        var file = try std.fs.createFileAbsolute(tmp, .{ .exclusive = true, .mode = 0o600 });
        defer file.close();
        errdefer std.fs.deleteFileAbsolute(tmp) catch {};
        try file.writeAll(bytes);
        try file.sync();
        try std.fs.renameAbsolute(tmp, path);
    }
    fn restoreDrafts(self: *Workspace) void {
        const path = self.draftPath() catch return;
        defer self.allocator.free(path);
        var file = std.fs.openFileAbsolute(path, .{}) catch return;
        defer file.close();
        const bytes = file.readToEndAlloc(self.allocator, 1024 * 1024) catch return;
        defer self.allocator.free(bytes);
        var parsed = std.json.parseFromSlice(Value, self.allocator, bytes, .{}) catch return;
        defer parsed.deinit();
        // Rust validates every scoped record; no pending send is restored.
        self.input(.{ .type = "restore_drafts", .drafts = parsed.value });
    }
};

pub const Entry = struct {
    instruction_request_id: u64 = 0,
    allocator: std.mem.Allocator,
    arena: std.heap.ArenaAllocator,
    owner: *Workspace,
    pane: []const u8,
    window_id: []const u8,
    content: Content,
    display: *DisplaySurface,
    host: *NativeWindow,
    init_signal: c_ulong = 0,
    resize_signal: c_ulong = 0,
    map_signal: c_ulong = 0,
    unmap_signal: c_ulong = 0,
    resize_area: ?*gtk.GLArea = null,
    ready: bool = false,
    activate_when_ready: bool = false,
    click: ?*gtk.GestureClick = null,
    keys: ?*gtk.EventControllerKey = null,
    reading_hint: ?*gtk.Button = null,
    references: ?*ReferenceOverlay = null,
    reading_overlay: ?*gtk.Overlay = null,
    adjustment: ?*gtk.Adjustment = null,
    adjustment_signal: c_ulong = 0,
    adjustment_changed: c_ulong = 0,
    hint_retry: bool = false,
    pointer_down: bool = false,
    action_timer: c_uint = 0,
    action_row: usize = 0,
    action_column: usize = 0,
    action_revision: u64 = 0,
    press_x: f64 = 0,
    press_y: f64 = 0,
    fn create(owner: *Workspace, pane: []const u8, window: []const u8, host: *NativeWindow, content: Content, display: *DisplaySurface) !*Entry {
        const entry = try owner.allocator.create(Entry);
        errdefer owner.allocator.destroy(entry);
        var arena = std.heap.ArenaAllocator.init(owner.allocator);
        errdefer arena.deinit();
        const a = arena.allocator();
        const id = try a.dupe(u8, pane);
        const win = try a.dupe(u8, window);
        const saved = try content.clone(a);
        entry.* = .{ .allocator = owner.allocator, .arena = arena, .owner = owner, .pane = id, .window_id = win, .content = saved, .display = display, .host = host };
        if (content.kind == .idea_tree) return entry;
        if (findOverlay(display.surface.as(gtk.Widget))) |overlay| {
            const button = gtk.Button.newWithLabel("Back to latest");
            button.as(gtk.Widget).setHalign(.end);
            button.as(gtk.Widget).setValign(.end);
            button.as(gtk.Widget).setMarginEnd(24);
            button.as(gtk.Widget).setMarginBottom(12);
            button.as(gtk.Widget).setVisible(0);
            overlay.addOverlay(button.as(gtk.Widget));
            entry.reading_overlay = overlay;
            entry.references = try ReferenceOverlay.create(entry, overlay);
            entry.reading_hint = button;
            _ = gtk.Button.signals.clicked.connect(button, *Entry, jumpClicked, entry, .{});
        }
        entry.map_signal = gtk.Widget.signals.map.connect(display.surface.as(gtk.Widget), *Entry, visibilityChanged, entry, .{});
        entry.unmap_signal = gtk.Widget.signals.unmap.connect(display.surface.as(gtk.Widget), *Entry, visibilityChanged, entry, .{});
        const click = gtk.GestureClick.new();
        entry.click = click;
        click.as(gtk.GestureSingle).setButton(1);
        click.as(gtk.EventController).setPropagationPhase(.capture);
        _ = gtk.GestureClick.signals.pressed.connect(click, *Entry, pressed, entry, .{});
        _ = gtk.GestureClick.signals.released.connect(click, *Entry, released, entry, .{});
        _ = gtk.GestureClick.signals.stopped.connect(click, *Entry, stopped, entry, .{});
        display.surface.as(gtk.Widget).addController(click.as(gtk.EventController));
        const keys = gtk.EventControllerKey.new();
        entry.keys = keys;
        keys.as(gtk.EventController).setPropagationPhase(.capture);
        _ = gtk.EventControllerKey.signals.key_released.connect(keys, *Entry, keyReleased, entry, .{});
        display.surface.as(gtk.Widget).addController(keys.as(gtk.EventController));
        if (findArea(display.surface.as(gtk.Widget))) |area| {
            entry.resize_area = area;
            entry.resize_signal = gtk.GLArea.signals.resize.connect(area, *Entry, resized, entry, .{ .after = true });
        }
        return entry;
    }
    fn findOverlay(widget: *gtk.Widget) ?*gtk.Overlay {
        if (gobject.ext.cast(gtk.Overlay, widget)) |overlay| return overlay;
        var child = widget.getFirstChild();
        while (child) |item| : (child = item.getNextSibling()) {
            if (findOverlay(item)) |overlay| return overlay;
        }
        return null;
    }
    fn jumpClicked(_: *gtk.Button, self: *Entry) callconv(.c) void {
        if (self.hint_retry) self.owner.input(.{ .type = "retry_display", .pane_id = self.pane })
        else self.owner.input(.{ .type = "jump_to_latest", .pane_id = self.pane });
    }
    fn visibilityChanged(_: *gtk.Widget, self: *Entry) callconv(.c) void {
        self.refreshAfterInput();
    }
    fn scrollChanged(_: *gtk.Adjustment, self: *Entry) callconv(.c) void {
        self.refreshAfterInput();
    }
    fn reportViewport(self: *Entry) void {
        if (!self.ready) return;
        if (self.references) |references| references.position();
        const snapshot = self.display.snapshot() catch return;
        const adjustment = self.display.surface.getVAdjustment() orelse return;
        if (self.adjustment == null) {
            self.adjustment = adjustment;
            self.adjustment_signal = gtk.Adjustment.signals.value_changed.connect(adjustment, *Entry, scrollChanged, self, .{});
            self.adjustment_changed = gtk.Adjustment.signals.changed.connect(adjustment, *Entry, scrollChanged, self, .{});
        }
        self.owner.input(.{ .type = "viewport", .pane_id = self.pane,
            .at_bottom = adjustment.getValue() + adjustment.getPageSize() >= adjustment.getUpper() - 0.5,
            .top_row = @as(usize, @intFromFloat(@max(0, adjustment.getValue()))),
            .visible_rows = @as(usize, @intFromFloat(@max(1, adjustment.getPageSize()))),
            .revision = snapshot.revision, .visible = self.display.surface.as(gtk.Widget).getMapped() != 0 });
    }
    fn findArea(widget: *gtk.Widget) ?*gtk.GLArea {
        if (gobject.ext.cast(gtk.GLArea, widget)) |area| return area;
        var child = widget.getFirstChild();
        while (child) |item| : (child = item.getNextSibling()) {
            if (findArea(item)) |area| return area;
        }
        return null;
    }
    fn resized(_: *gtk.GLArea, _: c_int, _: c_int, self: *Entry) callconv(.c) void {
        self.refreshAfterInput();
    }
    fn refreshAfterInput(self: *Entry) void {
        if (self.owner.flush_timer == 0) self.owner.flush_timer = glib.idleAdd(Workspace.flush, self.owner);
    }
    fn keyReleased(_: *gtk.EventControllerKey, _: c_uint, _: c_uint, _: gdk.ModifierType, self: *Entry) callconv(.c) void {
        self.refreshAfterInput();
    }
    fn pressed(_: *gtk.GestureClick, _: c_int, x: f64, y: f64, self: *Entry) callconv(.c) void {
        self.cancelAction();
        self.pointer_down = true;
        self.owner.input(.{ .type="render_guard", .pane_id=self.pane, .blocked=true });
        self.press_x = x;
        self.press_y = y;
    }
    fn stopped(_: *gtk.GestureClick, self: *Entry) callconv(.c) void {
        self.pointer_down = false;
        self.refreshAfterInput();
    }
    fn released(_: *gtk.GestureClick, count: c_int, x: f64, y: f64, self: *Entry) callconv(.c) void {
        self.pointer_down = false;
        self.refreshAfterInput();
        if (count != 1 or @abs(x - self.press_x) > 4 or @abs(y - self.press_y) > 4) return;
        Workspace.paneSelected(self.owner,self.pane);
        Workspace.paneActivated(self.owner,self.pane);
        if (self.references) |references| if (references.hit(x,y)) |hit| {
            self.action_row=hit.row; self.action_column=hit.column; self.action_revision=references.revision;
            var interval:c_int=400;
            if (gtk.Settings.getDefault()) |settings| {
                var value = gobject.ext.Value.new(c_int);
                defer value.unset();
                settings.as(gobject.Object).getProperty("gtk-double-click-time",&value);
                interval=value.getInt();
            }
            self.action_timer=glib.timeoutAdd(@intCast(@max(1,interval)),activateRow,self);
            return;
        };
    }
    fn cancelAction(self:*Entry) void {
        if (self.action_timer!=0) { _ = glib.Source.remove(self.action_timer); self.action_timer=0; }
    }
    fn activateRow(context:?*anyopaque) callconv(.c) c_int {
        const self:*Entry=@ptrCast(@alignCast(context orelse return 0));
        self.action_timer=0;
        const blocked=if (self.display.surface.core()) |core| core.hasSelection() else true;
        self.owner.input(.{.type="render_guard",.pane_id=self.pane,.blocked=blocked});
        self.refreshAfterInput();
        // Inline rows defer the click for text-selection arbitration. Native
        // focus may have been observed while pointer_down suppressed Select.
        // Reconcile the originating pane before its reader action, but never
        // reactivate it after the user has moved to another pane or editor.
        if (blocked or !self.ready or self.owner.stopped or self.host.as(gtk.Window).isActive()==0 or
            self.host.getActiveSurface()!=self.display.surface) return 0;
        const focused=self.host.as(gtk.Window).getFocus() orelse return 0;
        const display_widget=self.display.surface.as(gtk.Widget);
        const composer=self.owner.composers.get(self.window_id) orelse return 0;
        const editor_is_current=composer.pane_id!=null and std.mem.eql(u8,composer.pane_id.?,self.pane) and
            (focused==composer.text.as(gtk.Widget) or focused.isAncestor(composer.text.as(gtk.Widget))!=0);
        if (focused!=display_widget and focused.isAncestor(display_widget)==0 and !editor_is_current) return 0;
        self.owner.input(.{.type="activate_row",.pane_id=self.pane,.revision=self.action_revision,.row=self.action_row,.column=self.action_column});
        return 0;
    }
    fn destroy(self: *Entry) void {
        if (self.display.graphics_widget) |widget| graphics.subscribe(widget, null, null);
        self.cancelAction();
        if (self.references) |references| references.destroy();
        if (self.map_signal != 0) gobject.signalHandlerDisconnect(self.display.surface.as(gobject.Object), self.map_signal);
        if (self.unmap_signal != 0) gobject.signalHandlerDisconnect(self.display.surface.as(gobject.Object), self.unmap_signal);
        if (self.adjustment) |adjustment| {
            if (self.adjustment_signal != 0) gobject.signalHandlerDisconnect(adjustment.as(gobject.Object), self.adjustment_signal);
            if (self.adjustment_changed != 0) gobject.signalHandlerDisconnect(adjustment.as(gobject.Object), self.adjustment_changed);
        }
        if (self.reading_overlay) |overlay| if (self.reading_hint) |button| overlay.removeOverlay(button.as(gtk.Widget));
        if (self.resize_area) |area| if (self.resize_signal != 0) gobject.signalHandlerDisconnect(area.as(gobject.Object), self.resize_signal);
        if (self.keys) |keys| self.display.surface.as(gtk.Widget).removeController(keys.as(gtk.EventController));
        if (self.click) |click| self.display.surface.as(gtk.Widget).removeController(click.as(gtk.EventController));
        if (self.init_signal != 0) gobject.signalHandlerDisconnect(self.display.surface.as(gobject.Object), self.init_signal);
        const allocator = self.allocator;
        self.arena.deinit();
        allocator.destroy(self);
    }
    fn graphicsEvent(bytes: [*:0]const u8, context: ?*anyopaque) callconv(.c) void {
        const self: *Entry = @ptrCast(@alignCast(context orelse return));
        if (!self.ready or self.owner.stopped) return;
        const parsed = std.json.parseFromSlice(Value, self.allocator, std.mem.span(bytes), .{}) catch return;
        defer parsed.deinit();
        if (field(parsed.value, "state")) |state| {
            self.owner.input(.{ .type = "idea_state", .pane_id = self.pane, .view_id = self.content.view_id, .state = state });
            if(!boolean(state,"reader_open")) {
                if(self.owner.composers.get(self.window_id)) |composer| if(composer.pane_id!=null and std.mem.eql(u8,composer.pane_id.?,self.pane)) composer.closeDetail();
            }
            const request=number(state,"instruction_request_id");
            if(request>self.instruction_request_id) {
                self.instruction_request_id=request;
                if(self.owner.composers.get(self.window_id)) |composer| if(composer.pane_id!=null and std.mem.eql(u8,composer.pane_id.?,self.pane)) composer.editInstruction(state);
            }
            if(self.owner.composers.get(self.window_id)) |composer| if(composer.pane_id!=null and std.mem.eql(u8,composer.pane_id.?,self.pane)) composer.updateDetailPresentation(state);
        } else if (std.mem.eql(u8, string(parsed.value, "kind") orelse "", "open_source")) {
            const path=string(parsed.value,"path") orelse return;
            if (!std.fs.path.isAbsolute(path)) return;
            const filename=self.allocator.dupeZ(u8,path) catch return;
            defer self.allocator.free(filename);
            const file=gio.File.newForPath(filename);defer file.as(gobject.Object).unref();
            const uri=file.getUri();defer glib.free(uri);
            _ = gio.AppInfo.launchDefaultForUri(uri,null,null);
        } else if (std.mem.eql(u8, string(parsed.value, "kind") orelse "", "focus_search")) {
            const composer = self.owner.composers.get(self.window_id) orelse return;
            if (composer.pane_id != null and std.mem.eql(u8, composer.pane_id.?, self.pane)) { composer.idea_search_button.popup(); _ = composer.idea_search.as(gtk.Widget).grabFocus(); }
        }
    }
    fn surfaceReady(_: *Surface, self: *Entry) callconv(.c) void {
        if (self.ready or self.owner.stopped) return;
        _ = self.display.bind(1) catch {
            self.owner.showError("Display initialization failed");
            return;
        };
        if (!self.owner.composers.contains(self.window_id)) {
            _ = self.owner.bindWindow(self.host, self.window_id) catch {
                self.owner.showError("Native input unavailable");
                return;
            };
        }
        self.ready = true;
        self.owner.input(.{ .type = "open", .pane_id = self.pane, .window_id = self.window_id, .scope = scope(self.content) });
        self.reportViewport();
        if (self.host.getActiveSurface()==self.display.surface) {
            Workspace.paneSelected(self.owner,self.pane);
            if (self.activate_when_ready) Workspace.paneActivated(self.owner,self.pane);
        }
        self.activate_when_ready=false;
    }
};

// Only the peripheral Return control is native; Ghostty owns all message rows.
const ReferenceOverlay = struct {
    entry: *Entry,
    overlay: *gtk.Overlay,
    back: *gtk.Button,
    rows: std.ArrayListUnmanaged(struct { row:usize, column_end:usize }) = .empty,
    revision: u64 = 0,
    can_return: bool = false,
    fn create(entry: *Entry, overlay: *gtk.Overlay) !*ReferenceOverlay {
        const self = try entry.allocator.create(ReferenceOverlay);
        self.* = .{ .entry=entry, .overlay=overlay, .back=gtk.Button.newWithLabel("Return to reading") };
        const back = self.back.as(gtk.Widget);
        back.setHalign(.end); back.setValign(.start); back.setMarginEnd(24); back.setMarginTop(4); back.setVisible(0);
        overlay.addOverlay(back);
        _ = gtk.Button.signals.clicked.connect(self.back, *ReferenceOverlay, returned, self, .{});
        return self;
    }
    fn returned(_: *gtk.Button, self: *ReferenceOverlay) callconv(.c) void {
        self.entry.owner.input(.{ .type="return_to_reading", .pane_id=self.entry.pane });
    }
    fn update(self: *ReferenceOverlay, effect: Value) void {
        self.rows.clearRetainingCapacity(); self.revision=number(effect,"revision"); self.can_return=boolean(effect,"can_return");
        if (field(effect,"items")) |items| {
            if (items == .array) for (items.array.items) |item| {
                self.rows.append(self.entry.allocator,.{.row=@intCast(number(item,"row")),.column_end=@intCast(number(item,"column_end"))}) catch return;
            };
        }
        self.position();
    }
    fn position(self: *ReferenceOverlay) void {
        const snapshot=self.entry.display.snapshot() catch return;
        self.back.as(gtk.Widget).setVisible(@intFromBool(self.can_return and snapshot.revision==self.revision));
    }
    fn hit(self: *ReferenceOverlay, x:f64, y:f64) ?struct { row:usize, column:usize } {
        const snapshot=self.entry.display.snapshot() catch return null;
        if (snapshot.revision!=self.revision) return null;
        const area=self.entry.resize_area orelse return null;
        const core=self.entry.display.surface.core() orelse return null;
        const adjustment=self.entry.display.surface.getVAdjustment() orelse return null;
        const width: f64=@floatFromInt(area.as(gtk.Widget).getWidth());
        const height: f64=@floatFromInt(area.as(gtk.Widget).getHeight());
        if (width<=0 or height<=0 or core.size.screen.width==0 or core.size.screen.height==0) return null;
        const sx=width/@as(f64,@floatFromInt(core.size.screen.width));
        const sy=height/@as(f64,@floatFromInt(core.size.screen.height));
        const cw=@as(f64,@floatFromInt(core.size.cell.width))*sx;
        const ch=@as(f64,@floatFromInt(core.size.cell.height))*sy;
        const left=@as(f64,@floatFromInt(core.size.padding.left))*sx;
        const top=@as(f64,@floatFromInt(core.size.padding.top))*sy;
        if (cw<=0 or ch<=0 or x<left+2*cw or x>=width or y<top or y>=height) return null;
        const row:usize=@intFromFloat(@floor((y-top)/ch)+adjustment.getValue());
        const column:usize=@intFromFloat(@floor((x-left)/cw));
        for (self.rows.items) |candidate| { if (candidate.row==row and column<candidate.column_end) return .{.row=row,.column=column}; }
        return null;
    }
    fn destroy(self: *ReferenceOverlay) void {
        self.rows.deinit(self.entry.allocator);
        self.overlay.removeOverlay(self.back.as(gtk.Widget));
        self.entry.allocator.destroy(self);
    }
};

pub fn scope(content: Content) struct { target_id: []const u8, node_id: []const u8, owner: []const u8, thread: []const u8, actor: []const u8 } {
    return .{ .target_id = content.target_id, .node_id = content.node_id, .owner = content.owner, .thread = content.thread, .actor = content.actor };
}
pub fn field(v: Value, key: []const u8) ?Value {
    return if (v == .object) v.object.get(key) else null;
}
pub fn string(v: Value, key: []const u8) ?[]const u8 {
    const x = field(v, key) orelse return null;
    return if (x == .string) x.string else null;
}
pub fn themeColor(value:Value,key:[]const u8,fallback:[]const u8) []const u8 {
    const color=string(value,key) orelse return fallback;
    if(color.len!=9 or color[0]!='#') return fallback;
    for(color[1..]) |byte| if(!std.ascii.isHex(byte)) return fallback;
    return color;
}
pub fn coordinate(v:Value,key:[]const u8) i32 {
    const x=field(v,key) orelse return 0;
    return switch(x){.integer=>@intCast(std.math.clamp(x.integer,-100000,100000)),.float=>@intFromFloat(std.math.clamp(x.float,-100000,100000)),else=>0};
}
pub fn number(v: Value, key: []const u8) u64 {
    const x = field(v, key) orelse return 0;
    return if (x == .integer and x.integer >= 0) @intCast(x.integer) else 0;
}
pub fn boolean(v: Value, key: []const u8) bool {
    const x = field(v, key) orelse return false;
    return x == .bool and x.bool;
}
pub fn array(v: ?Value) []const Value {
    const x = v orelse return &.{};
    return if (x == .array) x.array.items else &.{};
}
pub fn setLabel(allocator: std.mem.Allocator, label: *gtk.Label, text: []const u8) void {
    const z = allocator.dupeZ(u8, text) catch return;
    defer allocator.free(z);
    label.setLabel(z);
}
pub fn margins(widget: *gtk.Widget) void {
    widget.setMarginTop(12);
    widget.setMarginBottom(12);
    widget.setMarginStart(12);
    widget.setMarginEnd(12);
}
