//! GTK mechanics for the shared Rust Dictate policy. GLib owns FIFO worker
//! scheduling and main-loop delivery; no timer polls state or audio.
const std = @import("std");
const gtk = @import("gtk");
const gdk = @import("gdk");
const gio = @import("gio");
const external = @import("dictate_external.zig");
const glib = @import("glib");
const gobject = @import("gobject");
const c = @cImport({
    @cInclude("polar_station_desktop_client.h");
});
const a = std.heap.c_allocator;
const Value = std.json.Value;
pub fn field(value: Value, key: []const u8) Value {
    return if (value == .object) value.object.get(key) orelse .null else .null;
}
pub fn string(value: Value, key: []const u8) []const u8 {
    const v = field(value, key);
    return if (v == .string) v.string else "";
}
pub fn boolean(value: Value, key: []const u8) bool {
    const v = field(value, key);
    return v == .bool and v.bool;
}
fn same(x: []const u8, y: []const u8) bool {
    return std.mem.eql(u8, x, y);
}
fn bufferText(buffer: *gtk.TextBuffer) [*:0]u8 {
    var first: gtk.TextIter = undefined;
    var last: gtk.TextIter = undefined;
    buffer.getBounds(&first, &last);
    return buffer.getText(&first, &last, 1);
}
fn selection(buffer: *gtk.TextBuffer) [2]c_int {
    var first: gtk.TextIter = undefined;
    var last: gtk.TextIter = undefined;
    _ = buffer.getSelectionBounds(&first, &last);
    return .{ first.getOffset(), last.getOffset() };
}

pub fn shortcut() c.StationDictateShortcut {
    return c.polar_station_dictate_shortcut();
}

pub const Controller = struct {
    pub const Delivery = enum { applied, unchanged, unknown, no_target };
    pub const NativeTarget = struct {
        context: *anyopaque,
        insert: *const fn (*anyopaque, []const u8) Delivery,
        release: *const fn (*anyopaque) void,
    };
    // Physical edge ownership only; recording policy remains in DictateGesture.
    shortcut_recording_source:u8=0,
    shortcut_owner:u8=0, // 0 none, 1 global, 2 local
    result_panel: ?*@import("dictate_panel.zig").Panel = null,
    panel_hide_source:c_uint=0,
    panel_hide_id:?[]u8=null,
    refine_lease:?std.json.Parsed(Value)=null,
    refine_english:bool=false,
    learning_stored:?*const fn(?*anyopaque,[]const u8,bool) void=null,
    refined:?*const fn(?*anyopaque,Value,[]const u8,[]const u8,bool) void=null,
    insert_current: ?*const fn (?*anyopaque, Value) Delivery = null,
    current_project:?*const fn(?*anyopaque) ?[]const u8=null,
    current_conversation: ?*const fn (?*anyopaque) ?[]const u8 = null,
    custom: ?NativeTarget = null,
    external_editors: ?*external.Watcher = null,
    editor_scope: ?*anyopaque = null,
    state: *c.DictateHandle,
    speech: ?*c.DictateSpeech,
    incoming: *glib.AsyncQueue,
    speech_incoming: *glib.AsyncQueue,
    outgoing: *glib.AsyncQueue,
    thread: ?std.Thread = null,
    speech_thread: ?std.Thread = null,
    stopped: bool = false,
    opened: bool = false,
    response: ?std.json.Parsed(Value) = null,
    error_text: ?[]u8 = null,
    audio: ?*c.PolarStationAudio = null,
    audio_events: u8 = 0,
    microphone: ?[:0]u8 = null,
    capture_id: ?[]u8 = null,
    submitted_id: ?[]u8 = null,
    gesture: ?*c.StationShortcutGesture = null,
    held_id: ?[]u8 = null,
    capture_clock: ?std.time.Timer = null,
    editor: ?*gtk.TextView = null,
    original: ?[]u8 = null,
    target_id: ?[]u8 = null,
    range: [2]c_int = .{ 0, 0 },
    cancelled: bool = false,
    context: ?*anyopaque = null,
    changed: ?*const fn (?*anyopaque) void = null,
    settings_context: ?*anyopaque = null,
    show_settings: ?*const fn (?*anyopaque) void = null,
    shown_setup: ?[]u8 = null,
    const Job = struct { input: []u8, pcm: ?[]u8 = null, result: [*c]u8 = null, opening: bool = false, service: bool = false, configuration: bool = false, learning:bool=false };

    pub fn create() !*Controller {
        const self = try a.create(Controller);
        errdefer a.destroy(self);
        const state = c.polar_station_dictate_new() orelse return error.DictateUnavailable;
        self.* = .{ .gesture = c.polar_station_shortcut_gesture_new(), .state = state, .speech = c.polar_station_dictate_speech_new(), .incoming = glib.AsyncQueue.new(), .speech_incoming = glib.AsyncQueue.new(), .outgoing = glib.AsyncQueue.new() };
        errdefer {
            c.polar_station_shortcut_gesture_free(self.gesture);
            c.polar_station_dictate_free(state);
            c.polar_station_dictate_speech_free(self.speech);
            self.incoming.unref();
            self.speech_incoming.unref();
            self.outgoing.unref();
        }
        const base = std.process.getEnvVarOwned(a, "XDG_STATE_HOME") catch blk: {
            const home = std.process.getEnvVarOwned(a, "HOME") catch break :blk a.dupe(u8, "") catch unreachable;
            defer a.free(home);
            break :blk std.fs.path.join(a, &.{ home, ".local", "state" }) catch unreachable;
        };
        defer a.free(base);
        const path = try std.fs.path.join(a, &.{ base, "polar-station", "dictate-v1.json" });
        defer a.free(path);
        self.thread = try std.Thread.spawn(.{}, worker, .{ self, self.incoming });
        self.speech_thread = std.Thread.spawn(.{}, worker, .{ self, self.speech_incoming }) catch |err| {
            self.incoming.push(self);
            self.thread.?.join();
            return err;
        };
        c.polar_station_dictate_speech_set_progress(self.speech, savedProgress, self);
        if (gio.Application.getDefault()) |app| {
            const stop = gio.SimpleAction.new("station-dictate-stop", null);
            _ = gio.SimpleAction.signals.activate.connect(stop, *Controller, notificationStop, self, .{});
            app.as(gio.ActionMap).addAction(stop.as(gio.Action));
            stop.unref();
            const cancel_action = gio.SimpleAction.new("station-dictate-cancel", null);
            _ = gio.SimpleAction.signals.activate.connect(cancel_action, *Controller, notificationCancel, self, .{});
            app.as(gio.ActionMap).addAction(cancel_action.as(gio.Action));
            cancel_action.unref();
            const original = gio.SimpleAction.new("station-dictate-original", null);
            _ = gio.SimpleAction.signals.activate.connect(original, *Controller, notificationOriginal, self, .{});
            app.as(gio.ActionMap).addAction(original.as(gio.Action));
            original.unref();
            for([_][:0]const u8{"station-dictate-result","station-dictate-insert","station-dictate-hide","station-dictate-retry"}) |name| {
                const action=gio.SimpleAction.new(name,null);
                _=gio.SimpleAction.signals.activate.connect(action,*Controller,notificationResult,self,.{});
                app.as(gio.ActionMap).addAction(action.as(gio.Action));action.unref();
            }
        }
        self.enqueue(.{ .type = "open", .path = path, .host_managed = true }, true);
        self.reloadVoiceSettings();
        self.refreshHistory();
        self.external_editors = external.Watcher.create(self, externalFocused);
        return self;
    }
    pub fn destroy(self: *Controller) void {
        if (self.external_editors) |watcher| watcher.destroy();
        self.external_editors = null;
        c.polar_station_dictate_speech_set_progress(self.speech, null, null);
        c.polar_station_dictate_speech_set_history(self.speech, null, null);
        if (gio.Application.getDefault()) |app| {
            app.withdrawNotification("station-dictate-recording");
            app.as(gio.ActionMap).removeAction("station-dictate-stop");
            app.as(gio.ActionMap).removeAction("station-dictate-cancel");
            app.as(gio.ActionMap).removeAction("station-dictate-original");
            for([_][:0]const u8{"station-dictate-result","station-dictate-insert","station-dictate-hide","station-dictate-retry"}) |name| app.as(gio.ActionMap).removeAction(name);
        }
        if(self.result_panel) |panel| panel.destroy();
        if(self.panel_hide_source!=0) {_=glib.Source.remove(self.panel_hide_source);}
        if(self.panel_hide_id) |id| a.free(id);
        self.cancel();
        self.stopped = true;
        self.stopAudio();
        self.incoming.push(self); // FIFO shutdown after already admitted saves.
        self.speech_incoming.push(self);
        if (self.thread) |thread| thread.join();
        if (self.speech_thread) |thread| thread.join();
        while (glib.Source.removeByUserData(self) != 0) {}
        while (self.outgoing.tryPop()) |raw| freeJob(@ptrCast(@alignCast(raw)));
        self.incoming.unref();
        self.speech_incoming.unref();
        self.outgoing.unref();
        self.clearTarget();
        if (self.shown_setup) |id| a.free(id);
        if (self.submitted_id) |id| a.free(id);
        c.polar_station_shortcut_gesture_free(self.gesture);
        if (self.held_id) |id| a.free(id);
        if (self.response) |*r| r.deinit();
        if (self.error_text) |v| a.free(v);
        if (self.microphone) |v| a.free(v);
        c.polar_station_dictate_free(self.state);
        c.polar_station_dictate_speech_free(self.speech);
        a.destroy(self);
    }
    pub fn refreshHistory(self: *Controller) void {
        c.polar_station_dictate_speech_set_history(self.speech, historyChanged, self);
        self.input(.{.type="history_open"});
    }
    pub fn historyAction(self:*Controller,kind:[]const u8,id:[]const u8) void {
        if(same(kind,"history_insert") or same(kind,"history_retry"))self.cancelled=false;
        self.input(.{.type=kind,.operation_id=id});
    }
    fn historyChanged(context: ?*anyopaque, bytes: [*c]const u8, length: usize) callconv(.c) void {
        const self:*Controller=@ptrCast(@alignCast(context orelse return));
        if(bytes==null)return;
        var parsed=std.json.parseFromSlice(Value,a,bytes[0..length],.{}) catch return;
        defer parsed.deinit();self.input(parsed.value);
    }
    fn savedProgress(context: ?*anyopaque, id: [*c]const u8, length: usize, saved: u64, preview: [*c]const u8, preview_length: usize) callconv(.c) void {
        const self: *Controller = @ptrCast(@alignCast(context orelse return));
        if (id == null) return;
        if (preview != null and preview_length > 0) {
            self.input(.{ .type = "transcript_preview", .operation_id = id[0..length], .text = preview[0..preview_length], .saved_samples = saved });
        } else self.input(.{ .type = "audio_saved", .operation_id = id[0..length], .saved_samples = saved });
    }
    fn notificationStop(_: *gio.SimpleAction, _: ?*glib.Variant, self: *Controller) callconv(.c) void {
        self.finish();
    }
    fn notificationCancel(_: *gio.SimpleAction, _: ?*glib.Variant, self: *Controller) callconv(.c) void {
        self.cancel();
    }
    fn notificationOriginal(_: *gio.SimpleAction, _: ?*glib.Variant, self: *Controller) callconv(.c) void {
        self.useOriginal();
    }
    pub fn useAvailable(self: *Controller) void {
        const id = string(self.presentation(), "use_available_id");
        if (id.len > 0) self.input(.{ .type = "use_available", .operation_id = id });
    }
    pub fn useOriginal(self: *Controller) void {
        const id = string(self.presentation(), "use_original_id");
        if (id.len > 0) self.input(.{ .type = "use_original", .operation_id = id });
    }
    pub fn panelValue(self:*Controller) Value {return field(self.view(),"panel");}
    pub fn panelAction(self:*Controller,kind:[]const u8) void {
        if(same(kind,"stop")){self.finish();return;}
        if(same(kind,"cancel")){self.cancel();return;}
        if(same(kind,"use_available")){self.useAvailable();return;}
        if(same(kind,"use_original")){self.useOriginal();return;}
        const id=string(self.panelValue(),"operation_id");if(id.len==0)return;
        if(same(kind,"insert_draft") or same(kind,"retry_saved")) self.cancelled=false;
        self.input(.{.type=kind,.operation_id=id});
    }
    fn notificationResult(action:*gio.SimpleAction,_:?*glib.Variant,self:*Controller) callconv(.c) void {
        const name=std.mem.span(action.as(gio.Action).getName());
        if(same(name,"station-dictate-insert")){self.panelAction("insert_draft");return;}
        if(same(name,"station-dictate-hide")){self.panelAction("hide_panel");return;}
        if(same(name,"station-dictate-retry")){self.panelAction("retry_saved");return;}
        if(self.result_panel==null) self.result_panel=@import("dictate_panel.zig").Panel.create(self,null) catch return;
        self.result_panel.?.present();
    }
    fn hidePanel(raw:?*anyopaque) callconv(.c) c_int {
        const self:*Controller=@ptrCast(@alignCast(raw orelse return 0));self.panel_hide_source=0;
        if(same(string(self.panelValue(),"operation_id"),self.panel_hide_id orelse "")) self.panelAction("hide_panel");
        return 0;
    }
    pub fn updateRecordingNotification(self:*Controller) void {
        if(self.result_panel) |panel| panel.refresh();
        const panel=self.panelValue();
        const app=gio.Application.getDefault() orelse return;
        if(panel==.null){app.withdrawNotification("station-dictate-recording");return;}
        const id=string(panel,"operation_id");
        const delay=field(panel,"hide_after_ms");
        if(delay==.integer and delay.integer>0 and !same(id,self.panel_hide_id orelse "")) {
            if(self.panel_hide_source!=0) {_=glib.Source.remove(self.panel_hide_source);}
            if(self.panel_hide_id) |old| a.free(old);
            self.panel_hide_id=a.dupe(u8,id) catch null;
            self.panel_hide_source=glib.timeoutAdd(@intCast(delay.integer),hidePanel,self);
        }
        // Foreground Station uses a non-grabbing popover. Other applications
        // retain focus while the desktop presents recording/results/actions.
        if(gobject.ext.cast(gtk.Application,app)) |native| if(native.getActiveWindow()) |window| if(window.isActive()!=0) {app.withdrawNotification("station-dictate-recording");return;};
        const notice=gio.Notification.new("Dictate");defer notice.unref();
        const panel_status=string(panel,"status");const text=string(panel,"text");
        const body=std.fmt.allocPrintSentinel(a,"{s}\n{s}",.{panel_status,text},0) catch return;defer a.free(body);
        notice.setBody(body);notice.setPriority(.urgent);notice.setDefaultAction("app.station-dictate-result");
        if(boolean(panel,"can_stop")) notice.addButton("Stop & Save","app.station-dictate-stop");
        if(boolean(panel,"can_cancel")) notice.addButton("Cancel","app.station-dictate-cancel");
        if(boolean(panel,"can_insert")) notice.addButton("Insert","app.station-dictate-insert");
        if(boolean(panel,"can_retry")) notice.addButton("Retry","app.station-dictate-retry");
        if(boolean(panel,"can_copy")) notice.addButton("View / Edit","app.station-dictate-result");
        app.sendNotification("station-dictate-recording",notice);
    }
    pub fn reloadVoiceSettings(self: *Controller) void {
        const bytes = a.dupe(u8, "{\"op\":\"voice_settings\"}") catch return;
        const job = a.create(Job) catch {
            a.free(bytes);
            return;
        };
        job.* = .{ .input = bytes, .service = true, .configuration = true };
        if (job.service or job.pcm != null) self.speech_incoming.push(job) else self.incoming.push(job);
    }
    pub fn view(self: *Controller) Value {
        return if (self.response) |r| r.value else .null;
    }
    fn conversationContext(self: *Controller) ?[]const u8 {
        if(!self.collectsContext()) return null;
        return if(self.current_conversation) |read| read(self.context) else null;
    }
    pub fn collectsContext(self: *Controller) bool {
        return boolean(field(self.view(), "reference_policy"), "surrounding");
    }
    pub fn presentation(self: *Controller) Value {
        return field(self.view(), "presentation");
    }
    pub fn enabled(self: *Controller) bool {
        return boolean(self.presentation(), "enabled") and (self.submitted_id == null or same(string(self.presentation(), "action"), "stop"));
    }
    pub fn status(self: *Controller) []const u8 {
        return self.error_text orelse string(self.presentation(), "status");
    }
    pub fn storeLearning(self:*Controller,sample:Value) void {
        const bytes=std.json.Stringify.valueAlloc(a,.{.op="learn_refine",.sample=sample},.{}) catch return;
        const job=a.create(Job) catch {a.free(bytes);return;};job.*=.{.input=bytes,.service=true,.learning=true};self.speech_incoming.push(job);
    }
    pub fn input(self: *Controller, value: anytype) void {
        self.enqueue(value, false);
    }
    fn enqueue(self: *Controller, value: anytype, opening: bool) void {
        if (self.stopped) return;
        const bytes = std.json.Stringify.valueAlloc(a, value, .{}) catch return;
        const job = a.create(Job) catch {
            a.free(bytes);
            return;
        };
        job.* = .{ .input = bytes, .opening = opening };
        if (job.service or job.pcm != null) self.speech_incoming.push(job) else self.incoming.push(job);
    }
    fn freeJob(job: *Job) void {
        a.free(job.input);
        if (job.pcm) |pcm| {
            @memset(pcm, 0);
            a.free(pcm);
        }
        if (job.result != null) c.polar_station_desktop_string_free(job.result);
        a.destroy(job);
    }
    fn worker(self: *Controller, incoming: *glib.AsyncQueue) void {
        while (true) {
            const raw = incoming.pop();
            if (raw == @as(*anyopaque, @ptrCast(self))) return;
            const job: *Job = @ptrCast(@alignCast(raw));
            job.result = if (job.service) c.polar_station_dictate_service_request(self.speech, job.input.ptr, job.input.len) else if (job.pcm) |pcm| c.polar_station_dictate_speech_transcribe(self.speech, job.input.ptr, job.input.len, pcm.ptr, pcm.len) else c.polar_station_dictate_dispatch(self.state, job.input.ptr, job.input.len);
            self.outgoing.push(job);
            _ = glib.idleAdd(receive, self);
        }
    }
    fn receive(data: ?*anyopaque) callconv(.c) c_int {
        const self: *Controller = @ptrCast(@alignCast(data orelse return 0));
        while (self.outgoing.tryPop()) |raw| {
            const job: *Job = @ptrCast(@alignCast(raw));
            defer freeJob(job);
            if (job.result == null) continue;
            var parsed = std.json.parseFromSlice(Value, a, std.mem.span(job.result), .{ .allocate = .alloc_always }) catch continue;
            if(job.learning){
                defer parsed.deinit();
                var request=std.json.parseFromSlice(Value,a,job.input,.{}) catch continue;defer request.deinit();
                if(self.learning_stored) |notify| notify(self.context,string(field(request.value,"sample"),"intent"),boolean(parsed.value,"ok") and boolean(field(parsed.value,"payload"),"stored"));
                continue;
            }
            if (job.configuration) {
                defer parsed.deinit();
                if (boolean(parsed.value, "ok")) {
                    self.input(.{.type="refresh_settings"});
                    const mic = string(field(parsed.value, "payload"), "microphone");
                    if (self.microphone) |old| a.free(old);
                    self.microphone = a.dupeZ(u8, mic) catch null;
                }
                continue;
            }
            if (job.pcm != null or job.service) {
                defer parsed.deinit();
                const result = field(parsed.value, "input");
                if (result != .null) self.input(result) else if (job.service) {
                    var request = std.json.parseFromSlice(Value, a, job.input, .{}) catch continue;
                    defer request.deinit();
                    self.input(.{ .type = "rewritten", .operation_id = string(request.value, "id"), .@"error" = string(parsed.value, "error") });
                }
                continue;
            }
            if (!boolean(parsed.value, "ok") and field(parsed.value, "presentation") == .null) {
                if (self.error_text) |v| a.free(v);
                self.error_text = a.dupe(u8, string(parsed.value, "error")) catch null;
                parsed.deinit();
                if(self.submitted_id) |id| a.free(id);self.submitted_id=null;
                if (self.changed) |f| f(self.context);
                continue;
            }
            if (self.response) |*r| r.deinit();
            self.response = parsed;
            self.opened = boolean(parsed.value, "initialized");
            if (self.error_text) |v| a.free(v);
            self.error_text = if (boolean(parsed.value, "ok")) null else a.dupe(u8, string(parsed.value, "error")) catch null;
            if (!boolean(parsed.value, "ok")) {
                if (self.submitted_id) |id| a.free(id);
                self.submitted_id = null;
            }
            if (self.submitted_id) |id| {
                const entries = field(self.view(), "entries");
                if (entries == .array) for (entries.array.items) |entry| {
                    if (!same(string(entry, "operation_id"), id)) continue;
                    const phase = string(entry, "phase");
                    for ([_][]const u8{ "pending", "delivered", "copied", "unknown", "review", "cancelled", "failed" }) |terminal| {
                        if (same(phase, terminal)) {
                            a.free(id);
                            self.submitted_id = null;
                            break;
                        }
                    }
                    break;
                };
            }
            if (boolean(self.presentation(), "setup_required")) {
                const entries = field(self.view(), "entries");
                if (entries == .array and entries.array.items.len > 0) {
                    const id = string(entries.array.items[entries.array.items.len - 1], "operation_id");
                    if (!same(id, self.shown_setup orelse "")) {
                        if (self.shown_setup) |old| a.free(old);
                        self.shown_setup = a.dupe(u8, id) catch null;
                        if (self.show_settings) |show| show(self.settings_context);
                    }
                }
            }
            self.updateRecordingNotification();
            if (self.changed) |f| f(self.context);
            const effects = field(parsed.value, "effects");
            if (effects == .array) for (effects.array.items) |effect| self.execute(effect);
        }
        return 0;
    }
    fn clearTarget(self: *Controller) void {
        if(self.refine_lease) |*lease| lease.deinit();self.refine_lease=null;
        if (self.custom) |target| target.release(target.context);
        self.custom = null;
        if (self.editor) |v| v.as(gobject.Object).unref();
        self.editor = null;
        if (self.original) |v| a.free(v);
        self.original = null;
        if (self.target_id) |v| a.free(v);
        self.target_id = null;
    }
    fn captureTarget(self: *Controller, editor: *gtk.TextView, id: []const u8) !void {
        self.clearTarget();
        const value = bufferText(editor.getBuffer());
        defer glib.free(value);
        self.original = try a.dupe(u8, std.mem.span(value));
        self.target_id = try a.dupe(u8, id);
        _ = editor.as(gobject.Object).ref();
        self.editor = editor;
        self.editor_scope = editor.as(gobject.Object).getData("station-dictate-scope");
        self.range = selection(editor.getBuffer());
    }
    pub fn startNode(self: *Controller, target: NativeTarget, context: []const u8) void {
        self.startNative(target, context, "idea_node", "");
    }
    fn startNative(self: *Controller, target: NativeTarget, context: []const u8, kind: []const u8, reference:[]const u8) void {
        if (same(string(self.presentation(), "action"), "retry_open")) { target.release(target.context); self.input(.{ .type = "retry_open" }); return; }
        if (!self.enabled()) {
            target.release(target.context);
            return;
        }
        const id = std.fmt.allocPrint(a, "{x}", .{std.crypto.random.int(u128)}) catch {
            target.release(target.context);
            return;
        };
        defer a.free(id);
        self.clearTarget();
        self.custom = target;
        self.cancelled = false;
        self.target_id = a.dupe(u8, id) catch {
            self.clearTarget();
            return;
        };
        self.submitted_id = a.dupe(u8, id) catch return;
        self.input(.{ .type = "start_context", .conversation_context=self.conversationContext(), .project=if(self.current_project) |project| project(self.context) else null, .operation_id = id, .reference=reference, .targets = .{.{ .kind = kind, .identity = id, .revision = id, .context = if(self.collectsContext()) context else "" }} });
    }
    fn releaseExternal(raw: *anyopaque) void {
        const target: *external.Snapshot = @ptrCast(@alignCast(raw));
        target.destroy();
    }
    fn insertExternal(raw: *anyopaque, text: []const u8) Delivery {
        const target: *external.Snapshot = @ptrCast(@alignCast(raw));
        return switch (target.insert(text)) {
            .applied => .applied,
            .unchanged => .unchanged,
            .unknown => .unknown,
        };
    }
    pub fn insertExternalCurrent(self:*Controller,text:[]const u8) Delivery {
        const watcher=self.external_editors orelse return .no_target;
        const target=watcher.capture() orelse return .no_target;
        defer target.destroy();
        return switch(target.insert(text)){.applied=>.applied,.unchanged=>.unchanged,.unknown=>.unknown};
    }
    pub fn insertEditorCurrent(editor:*gtk.TextView,text:[]const u8,preserve_selection:bool) Delivery {
        if(editor.getEditable()==0 or editor.as(gtk.Widget).getMapped()==0 or editor.as(gobject.Object).getData("station-dictate-panel")!=null) return .no_target;
        const buffer=editor.getBuffer();
        const old=bufferText(buffer);defer glib.free(old);
        const selected=selection(buffer);
        var left:gtk.TextIter=undefined;var right:gtk.TextIter=undefined;var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;
        buffer.getBounds(&first,&last);buffer.getIterAtOffset(&left,if(preserve_selection) @max(selected[0],selected[1]) else @min(selected[0],selected[1]));buffer.getIterAtOffset(&right,@max(selected[0],selected[1]));
        const prefix=buffer.getText(&first,&left,1);defer glib.free(prefix);
        const suffix=buffer.getText(&right,&last,1);defer glib.free(suffix);
        const expected=std.fmt.allocPrint(a,"{s}{s}{s}",.{std.mem.span(prefix),text,std.mem.span(suffix)}) catch return .unchanged;defer a.free(expected);
        const z=a.dupeZ(u8,text) catch return .unchanged;defer a.free(z);
        buffer.beginUserAction();buffer.delete(&left,&right);buffer.insertAtCursor(z,@intCast(text.len));buffer.endUserAction();
        const actual=bufferText(buffer);defer glib.free(actual);
        return if(same(std.mem.span(actual),expected)) .applied else .unknown;
    }
    pub fn toggleExternal(self: *Controller) void {
        const reference=if(self.external_editors) |watcher| watcher.selectedText() else null;
        defer if(reference) |text| a.free(text);
        if (self.external_editors) |watcher| if (watcher.capture()) |snapshot| {
            self.startNative(.{ .context = snapshot, .insert = insertExternal, .release = releaseExternal }, if(self.collectsContext()) snapshot.context() else "", "editor", reference orelse "");
            return;
        };
        self.toggleQuoted(null,reference orelse "");
    }
    fn externalFocused(raw: *anyopaque) void {
        const self: *Controller = @ptrCast(@alignCast(raw));
        if (!self.opened or self.submitted_id != null or !same(string(self.presentation(), "action"), "start")) return;
        if (gio.Application.getDefault()) |app| {
            if (gobject.ext.cast(gtk.Application, app)) |native| if (native.getActiveWindow()) |window| if (window.isActive() != 0) return;
        }
        const entries = field(self.view(), "entries");
        if (entries != .array) return;
        var pending = false;
        for (entries.array.items) |entry| if (same(string(entry, "phase"), "pending") and field(entry, "target") == .null) {
            pending = true;
            break;
        };
        if (!pending) return;
        const watcher = self.external_editors orelse return;
        // Focus can change programmatically. Never drain a saved draft unless
        // the desktop supplies the corresponding explicit pointer gesture.
        if (!watcher.userActivated()) return;
        const snapshot = watcher.capture() orelse return;
        const id = std.fmt.allocPrint(a, "{x}", .{std.crypto.random.int(u128)}) catch {
            snapshot.destroy();
            return;
        };
        defer a.free(id);
        self.clearTarget();
        self.custom = .{ .context = snapshot, .insert = insertExternal, .release = releaseExternal };
        self.target_id = a.dupe(u8, id) catch {
            self.clearTarget();
            return;
        };
        self.cancelled = false;
        self.input(.{ .type = "focus", .target = .{ .kind = "editor", .identity = id, .revision = id, .context = snapshot.context() } });
    }
    pub fn refine(self: *Controller, editor: *gtk.TextView, context_id:[]const u8, lease:?Value, project:?Value) void {
        if (!self.enabled() or !same(string(self.presentation(), "action"), "start") or editor.getEditable() == 0) return;
        const content = bufferText(editor.getBuffer()); defer glib.free(content);
        if (std.mem.trim(u8,std.mem.span(content)," \t\r\n").len == 0) return;
        const id = std.fmt.allocPrint(a,"{x}",.{std.crypto.random.int(u128)}) catch return;
        defer a.free(id);
        var start: gtk.TextIter = undefined; var end: gtk.TextIter = undefined;
        editor.getBuffer().getBounds(&start,&end);
        editor.getBuffer().selectRange(&end,&start);
        self.captureTarget(editor,id) catch return;
        if(lease) |value| {
            const bytes=std.json.Stringify.valueAlloc(a,value,.{}) catch return;defer a.free(bytes);
            self.refine_lease=std.json.parseFromSlice(Value,a,bytes,.{.allocate=.alloc_always}) catch null;
        }
        const selected=field(field(self.view(),"refine_modes"),context_id);
        self.refine_english=selected==.bool and selected.bool;
        self.cancelled = false;
        self.submitted_id = a.dupe(u8,id) catch return;
        self.input(.{.type="refine_context",.context_id=context_id,.project=project,.operation_id=id,.target=.{.kind="editor",.identity=id,.revision=id,.context=""},.text=self.original.?});
    }
    pub fn toggle(self: *Controller, editor: ?*gtk.TextView) void {
        const reference=if(editor) |view_| selectedText(view_) else null;
        defer if(reference) |text| glib.free(text);
        self.toggleQuoted(editor,if(reference) |text| std.mem.span(text) else "");
    }
    pub fn selectedText(editor:*gtk.TextView) ?[*:0]u8 {
        var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;
        if(editor.getBuffer().getSelectionBounds(&first,&last)==0)return null;
        return editor.getBuffer().getText(&first,&last,1);
    }
    pub fn toggleQuoted(self: *Controller, editor: ?*gtk.TextView, reference:[]const u8) void {
        if (same(string(self.presentation(), "action"), "retry_open")) { self.input(.{ .type = "retry_open" }); return; }
        if (same(string(self.presentation(), "action"), "stop")) {
            self.finish();
            return;
        }
        if (!self.enabled()) return;
        const id = std.fmt.allocPrint(a, "{x}", .{std.crypto.random.int(u128)}) catch return;
        defer a.free(id);
        self.cancelled = false;
        if (editor) |view_| {
            if (view_.getEditable() != 0) {
                self.captureTarget(view_, id) catch return;
                var end = if(self.collectsContext()) @min(self.original.?.len, 8192) else 0;
                while (end > 0 and !std.unicode.utf8ValidateSlice(self.original.?[0..end])) end -= 1;
                self.submitted_id = a.dupe(u8, id) catch return;
                self.input(.{ .type = "start_context", .conversation_context=self.conversationContext(), .project=if(self.current_project) |project| project(self.context) else null, .operation_id = id, .reference=reference, .targets = .{.{ .kind = "editor", .identity = id, .revision = id, .context = self.original.?[0..end] }} });
                return;
            }
        }
        self.clearTarget();
        self.submitted_id = a.dupe(u8, id) catch return;
        self.input(.{ .type = "start_context", .conversation_context=self.conversationContext(), .project=if(self.current_project) |project| project(self.context) else null, .operation_id = id, .reference=reference, .targets = .{} });
    }
    /// The caller snapshots the actual target using the existing button path.
    pub fn shortcutLost(self: *Controller) void {
        if(self.shortcut_owner==2 or (self.shortcut_recording_source==2 and self.submitted_id!=null))return;
        self.shortcut_owner=0;
        _ = c.polar_station_shortcut_gesture_event(self.gesture, 4, false, false, false);
        self.cancel();
        self.releaseHold();
    }
    pub fn shortcutIntent(self: *Controller, pressed: bool, discrete: bool) u32 {
        // Portal Activated is a discrete activation; GTK capture supplies edges separately.
        return c.polar_station_shortcut_gesture_event(self.gesture, if (pressed) (if (discrete) @as(u32, 3) else 1) else 2,
            self.submitted_id != null and !same(string(self.presentation(), "action"), "wait"), (self.enabled() and same(string(self.presentation(), "action"), "start")) or same(string(self.presentation(), "action"), "retry_open"), string(self.presentation(),"use_available_id").len>0);
    }
    pub fn bindHold(self: *Controller) void {
        if (self.held_id) |id| a.free(id);
        self.held_id = if (self.submitted_id) |id| a.dupe(u8, id) catch null else null;
    }
    pub fn releaseHold(self: *Controller) void {
        const id = self.held_id orelse return;
        self.held_id = null;
        defer a.free(id);
        if (!same(id, self.submitted_id orelse "")) return;
        const duration: u64 = if (self.capture_clock) |*timer| @min(600000, timer.read() / std.time.ns_per_ms) else 0;
        if (self.capture_clock == null) {
            self.cancelled = true;
            self.stopAudio();
        }
        self.input(.{ .type = "stop", .operation_id = id, .duration_ms = duration });
    }
    pub fn shortcutBinding(self: *Controller, trigger: []const u8) void {
        self.input(.{ .type = "shortcut_binding", .trigger = trigger });
    }
    pub fn shortcutStatus(self: *Controller, status_: []const u8) void {
        self.input(.{ .type = "shortcut_status", .status = status_ });
    }
    pub fn focus(self: *Controller, editor: *gtk.TextView) void {
        if(editor.as(gobject.Object).getData("station-dictate-panel")!=null)return;
        if (!self.opened or self.submitted_id != null or !same(string(self.presentation(), "action"), "start") or editor.getEditable() == 0) return;
        const id = std.fmt.allocPrint(a, "{x}", .{std.crypto.random.int(u128)}) catch return;
        defer a.free(id);
        self.captureTarget(editor, id) catch return;
        self.cancelled = false;
        var end = @min(self.original.?.len, 8192);
        while (end > 0 and !std.unicode.utf8ValidateSlice(self.original.?[0..end])) end -= 1;
        self.input(.{ .type = "focus", .target = .{ .kind = "editor", .identity = id, .revision = id, .context = self.original.?[0..end] } });
    }
    pub fn cancel(self: *Controller) void {
        const id = self.submitted_id orelse string(self.presentation(), "cancel_id");
        if (id.len == 0) return;
        self.cancelled = true;
        self.stopAudio();
        c.polar_station_dictate_speech_cancel(self.speech, id.ptr, id.len);
        self.input(.{ .type = "cancel", .operation_id = id });
    }
    fn stopAudio(self: *Controller) void {
        if (self.audio) |audio| c.polar_station_dictate_audio_free(audio);
        self.audio = null;
        if (self.capture_id) |id| a.free(id);
        self.capture_id = null;
        self.capture_clock = null;
    }
    fn finish(self: *Controller) void {
        const id = self.capture_id orelse return;
        const duration: u64 = if (self.capture_clock) |*timer| @min(600000, timer.read() / std.time.ns_per_ms) else 0;
        self.input(.{ .type = "stop", .operation_id = id, .duration_ms = duration });
    }
    fn audioChunk(data: ?*anyopaque, pcm: [*c]const u8, length: usize) callconv(.c) bool {
        const self: *Controller = @ptrCast(@alignCast(data orelse return false));
        const id = self.capture_id orelse return false;
        return c.polar_station_dictate_speech_append(self.speech, id.ptr, id.len, pcm, length);
    }
    fn audioEvent(data: ?*anyopaque, event: c_int) callconv(.c) void {
        const self: *Controller = @ptrCast(@alignCast(data orelse return));
        self.audio_events |= @as(u8, 1) << @intCast(event);
        _ = glib.idleAdd(audioReady, self);
    }
    fn audioReady(data: ?*anyopaque) callconv(.c) c_int {
        const self: *Controller = @ptrCast(@alignCast(data orelse return 0));
        const id = self.capture_id orelse return 0;
        const events = self.audio_events;
        self.audio_events = 0;
        if (events & 4 != 0) {
            self.input(.{ .type = "failed", .operation_id = id, .@"error" = std.mem.span(c.polar_station_dictate_audio_error(self.audio)) });
            c.polar_station_dictate_speech_cancel(self.speech, id.ptr, id.len);
            self.stopAudio();
            return 0;
        }
        if (events & 1 != 0) {
            self.capture_clock = std.time.Timer.start() catch null;
            self.input(.{ .type = "capture_ready", .operation_id = id });
        }
        if (events & 2 != 0) self.finish();
        return 0;
    }
    fn execute(self: *Controller, effect: Value) void {
        const kind = string(effect, "type");
        const id = string(effect, "operation_id");
        if (same(kind, "capture")) {
            if (self.cancelled) return;
            self.stopAudio();
            self.audio_events = 0;
            self.capture_id = a.dupe(u8, id) catch return;
            if (self.microphone == null) {
                self.input(.{ .type = "failed", .operation_id = id, .@"error" = "Voice settings are not ready. Open Settings → Voice and retry." });
                self.stopAudio();
                self.reloadVoiceSettings();
                return;
            }
            const begin = std.json.Stringify.valueAlloc(a, .{ .op = "capture_begin", .id = id, .context = field(effect, "context"), .project=field(effect,"project"), .reference = string(effect, "reference") }, .{}) catch return;
            defer a.free(begin);
            const reply = c.polar_station_dictate_service_request(self.speech, begin.ptr, begin.len);
            if (reply == null) {
                self.input(.{ .type = "failed", .operation_id = id, .@"error" = "Voice capture could not start." });
                self.stopAudio();
                return;
            }
            defer c.polar_station_desktop_string_free(reply);
            var started = std.json.parseFromSlice(Value, a, std.mem.span(reply), .{}) catch return;
            defer started.deinit();
            if (!boolean(started.value, "ok")) {
                self.input(.{ .type = "failed", .operation_id = id, .@"error" = string(started.value, "error") });
                self.stopAudio();
                return;
            }
            self.audio = c.polar_station_dictate_audio_new(audioEvent, self, 600000);
            c.polar_station_dictate_audio_set_chunk_callback(self.audio, audioChunk);
            if (self.microphone) |device| _ = c.polar_station_dictate_audio_set_device(self.audio, device);
            if (self.audio == null or !c.polar_station_dictate_audio_start(self.audio)) {
                self.input(.{ .type = "failed", .operation_id = id, .@"error" = "Microphone capture could not start." });
                c.polar_station_dictate_speech_cancel(self.speech, id.ptr, id.len);
                self.stopAudio();
            }
        } else if (same(kind, "copy")) {
            if (boolean(effect,"stop_capture")) {
                if (self.audio) |audio| c.polar_station_dictate_audio_stop(audio);
                self.stopAudio();
            }
            c.polar_station_dictate_speech_cancel(self.speech,id.ptr,id.len);
            const text=a.dupeZ(u8,string(effect,"text")) catch {
                self.input(.{.type="clipboard",.operation_id=id,.copied=false});return;
            };
            defer a.free(text);
            var copied=false;
            if(gdk.Display.getDefault()) |display| {
                const clipboard=display.getClipboard();clipboard.setText(text);
                copied=clipboard.getContent()!=null;
            }
            self.input(.{.type="clipboard",.operation_id=id,.copied=copied});
        } else if (same(kind, "cancel")) {
            self.cancelled = true;
            self.stopAudio();
            c.polar_station_dictate_speech_cancel(self.speech, id.ptr, id.len);
        } else if (same(kind, "finish_capture")) {
            const audio = self.audio orelse return;
            c.polar_station_dictate_audio_stop(audio);
            // Close PCM input; shared Recording events drive the result.
            const ended=c.polar_station_dictate_speech_finish(self.speech,id.ptr,id.len);
            self.stopAudio();
            if(!ended) self.input(.{.type="failed",.operation_id=id,.@"error"="Audio input could not finish; saved audio retained."});
        } else if (same(kind, "transcribe")) {
            const audio = self.audio orelse return;
            c.polar_station_dictate_audio_stop(audio);
            var length: usize = 0;
            const bytes = c.polar_station_dictate_audio_bytes(audio, &length);
            const pcm = (if (length == 0) a.alloc(u8, 0) else a.dupe(u8, bytes[0..length])) catch return;
            self.stopAudio();
            const job = a.create(Job) catch {
                a.free(pcm);
                return;
            };
            const owned = a.dupe(u8, id) catch {
                a.destroy(job);
                a.free(pcm);
                return;
            };
            job.* = .{ .input = owned, .pcm = pcm };
            if (!c.polar_station_dictate_speech_reserve(self.speech, id.ptr, id.len)) {
                freeJob(job);
                self.input(.{ .type = "failed", .operation_id = id, .@"error" = "Dictate worker is unavailable." });
                return;
            }
            if (job.service or job.pcm != null) self.speech_incoming.push(job) else self.incoming.push(job);
        } else if (same(kind, "rewrite")) {
            const request = std.json.Stringify.valueAlloc(a, .{ .op = if (boolean(effect,"saved_recording")) "rewrite_result" else "refine_text", .id = id, .context = string(effect, "context"), .raw = string(effect,"raw"), .english=boolean(effect,"english"), .project=if(string(effect,"project").len>0) string(effect,"project") else @as(?[]const u8,null), .scope=if(string(effect,"scope").len>0) string(effect,"scope") else @as(?[]const u8,null) }, .{}) catch return;
            const job = a.create(Job) catch {
                a.free(request);
                return;
            };
            job.* = .{ .input = request, .service = true };
            if (job.service or job.pcm != null) self.speech_incoming.push(job) else self.incoming.push(job);
        } else if (same(kind, "retry_transcribe")) {
            const bytes=std.json.Stringify.valueAlloc(a,.{.op="retry_result",.id=id},.{}) catch return;
            const job=a.create(Job) catch {a.free(bytes);return;};
            job.*=.{.input=bytes,.service=true};self.speech_incoming.push(job);
        } else if (same(kind, "insert_current")) {
            const outcome:Delivery=if(self.cancelled) .unchanged else if(self.insert_current) |insert| insert(self.context,effect) else .no_target;
            self.input(.{.type="delivery",.operation_id=id,.outcome=@tagName(outcome)});
            self.clearTarget();
        } else if (same(kind, "insert")) {
            var outcome: Delivery = .unchanged;
            if (!self.cancelled and same(string(field(effect, "target"), "identity"), self.target_id orelse "")) {
                if (self.custom) |target| outcome = target.insert(target.context, string(effect, "text"));
            }
            if (!self.cancelled and same(string(field(effect, "target"), "identity"), self.target_id orelse "")) if (self.editor) |editor| {
                const buffer = editor.getBuffer();
                const current = bufferText(buffer);
                defer glib.free(current);
                if (editor.as(gtk.Widget).getRoot() != null and editor.getEditable() != 0 and editor.as(gobject.Object).getData("station-dictate-scope") == self.editor_scope and same(std.mem.span(current), self.original orelse "") and std.meta.eql(selection(buffer), self.range)) {
                    const text = string(effect, "text");
                    const z = a.dupeZ(u8, text) catch return;
                    defer a.free(z);
                    var begin: gtk.TextIter = undefined;
                    var end: gtk.TextIter = undefined;
                    var left: gtk.TextIter = undefined;
                    var right: gtk.TextIter = undefined;
                    buffer.getBounds(&begin, &end);
                    buffer.getIterAtOffset(&left, @min(self.range[0], self.range[1]));
                    buffer.getIterAtOffset(&right, @max(self.range[0], self.range[1]));
                    const prefix = buffer.getText(&begin, &left, 1);
                    defer glib.free(prefix);
                    const suffix = buffer.getText(&right, &end, 1);
                    defer glib.free(suffix);
                    const expected = std.fmt.allocPrint(a, "{s}{s}{s}", .{ std.mem.span(prefix), text, std.mem.span(suffix) }) catch return;
                    defer a.free(expected);
                    buffer.beginUserAction();
                    _ = buffer.deleteSelection(1, 1);
                    buffer.insertAtCursor(z, @intCast(text.len));
                    buffer.endUserAction();
                    const inserted = bufferText(buffer);
                    defer glib.free(inserted);
                    outcome = if (same(std.mem.span(inserted), expected)) .applied else .unknown;
                }
            };
            if(outcome==.applied) if(self.refine_lease) |lease| if(self.refined) |notify| notify(self.context,lease.value,self.original orelse "",string(effect,"text"),self.refine_english);
            self.input(.{ .type = "delivery", .operation_id = id, .outcome = @tagName(outcome) });
            self.clearTarget();
        }
    }
};
