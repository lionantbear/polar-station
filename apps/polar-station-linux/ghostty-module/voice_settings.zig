//! Native Voice controls over the shared local Host contract. No provider here.
const std = @import("std");
const gtk = @import("gtk");
const glib = @import("glib");
const gio = @import("gio");
const gdk = @import("gdk");
const gobject = @import("gobject");
const domain = @import("dictate.zig");
const c = @cImport({
    @cInclude("polar_station_desktop_client.h");
});
const a = std.heap.c_allocator;
const Value = std.json.Value;
const field = domain.field;
const string = domain.string;
const boolean = domain.boolean;
fn eq(x: []const u8, y: []const u8) bool {
    return std.mem.eql(u8, x, y);
}
fn number(v: Value, k: []const u8) i64 {
    const f = field(v, k);
    return if (f == .integer) f.integer else 0;
}
fn label(text: [:0]const u8) *gtk.Label {
    const v = gtk.Label.new(text);
    v.setWrap(1);
    v.setXalign(0);
    return v;
}
fn setText(v: *gtk.Label, text: []const u8) void {
    const z = a.dupeZ(u8, text) catch return;
    defer a.free(z);
    v.setText(z);
}

fn setEditorText(editor: *gtk.TextView, text: []const u8) void {
    const value = a.dupeZ(u8, text) catch return;
    defer a.free(value);
    editor.getBuffer().setText(value, @intCast(text.len));
}

pub const Panel = struct {
    root: *gtk.Box,
    status: *gtk.Label,
    account: *gtk.Label,
    details: *gtk.Label,
    transcription_mode: *gtk.DropDown,
    shortcut_mode: *gtk.DropDown,
    cleanup: *gtk.CheckButton,
    context: *gtk.CheckButton,
    devices: *gtk.DropDown,
    terminology_status: *gtk.Label,
    terminology_root: *gtk.Box,
    terminology_button: *gtk.Button,
    device_names: *gtk.StringList,
    records: *gtk.Box,
    test_editor: *gtk.TextView,
    glossary: *gtk.TextView,
    meter: *gtk.LevelBar,
    usage: *gtk.Label,
    playback_controls: *gtk.MediaControls,
    player: ?*gtk.MediaFile = null,
    playback_generation: u64 = 0,
    local_pcm: std.ArrayListUnmanaged(u8) = .empty,
    speech: ?*c.DictateSpeech,
    incoming: *glib.AsyncQueue,
    outgoing: *glib.AsyncQueue,
    thread: ?std.Thread = null,
    config: ?std.json.Parsed(Value) = null,
    selected: ?std.json.Parsed(Value) = null,
    device_ids: std.ArrayListUnmanaged([:0]u8) = .empty,
    actions: std.ArrayListUnmanaged(*RowAction) = .empty,
    detail_row: ?*gtk.Box = null,
    delete_dialog: ?*gtk.Dialog = null,
    export_dialog: ?*gtk.NativeDialog = null,
    detail_action_start: usize = 0,
    audio: ?*c.PolarStationAudio = null,
    listing: ?*c.PolarStationAudio = null,
    test_id: ?[]u8 = null,
    pending_test: ?[]u8 = null,
    test_upload: bool = false,
    audio_events: u8 = 0,
    offset: u32 = 0,
    stopped: bool = false,
    callback: ?*const fn (?*anyopaque, bool) void = null,
    callback_context: ?*anyopaque = null,
    recover_recording: ?*const fn (?*anyopaque, []const u8) void = null,
    recording_deleted: ?*const fn (?*anyopaque, []const u8) void = null,
    const Job = struct { request: []u8, result: [*c]u8 = null, transcribe: bool = false, local_pcm: ?[]u8 = null, playback_generation: u64 = 0 };
    const RowAction = struct { panel: *Panel, id: []u8, op: []const u8, pinned: bool = false, start: i64 = 0 };
    fn setStatus(self: *Panel, text: []const u8) void {
        setText(self.status, text);
        setText(self.terminology_status, text);
    }
    pub fn create() !*Panel {
        const self = try a.create(Panel);
        const root = gtk.Box.new(.vertical, 12);
        const status = label("Voice: checking local Host…");
        root.append(status.as(gtk.Widget));
        const account = label("Account not checked");
        root.append(account.as(gtk.Widget));
        self.* = .{ .shortcut_mode = gtk.DropDown.new(null, null), .transcription_mode = gtk.DropDown.new(null, null), .terminology_status = label("Edit terms, then Save terminology."), .terminology_root = gtk.Box.new(.vertical, 12), .terminology_button = gtk.Button.newWithLabel("Terminology — edit names and spellings…"), .root = root, .status = status, .account = account, .details = label("Select a recording to review saved ranges and text."), .cleanup = gtk.CheckButton.newWithLabel("Clean spelling, punctuation and filler words"), .context = gtk.CheckButton.newWithLabel("Use context for names and terminology"), .device_names = gtk.StringList.new(null), .devices = gtk.DropDown.new(null, null), .records = gtk.Box.new(.vertical, 8), .test_editor = gtk.TextView.new(), .glossary = gtk.TextView.new(), .meter = gtk.LevelBar.newForInterval(0, 1), .usage = label("Saved audio usage: checking…"), .playback_controls = gtk.MediaControls.new(null), .speech = c.polar_station_dictate_speech_new(), .incoming = glib.AsyncQueue.new(), .outgoing = glib.AsyncQueue.new() };
        self.device_names.append("System default");
        const login = gtk.Box.new(.horizontal, 8);
        root.append(login.as(gtk.Widget));
        button(login, "Sign in / Switch account", signIn, self);
        button(login, "Cancel sign-in", cancelLogin, self);
        button(login, "Recheck", refresh, self);
        root.append(self.terminology_button.as(gtk.Widget));
        root.append(label("Choose hold/release or press/press below. The desktop owns the actual key binding. Use the Dictate button for recordings up to 10 minutes.").as(gtk.Widget));
        root.append(label("Shortcut interaction").as(gtk.Widget));
        root.append(self.shortcut_mode.as(gtk.Widget));
        button(root, "Save shortcut interaction", save, self);
        button(root, "Open system shortcut settings…", shortcut, self);
        root.append(label("GNOME: this button opens Settings → Apps → Polar Station. Select Global Shortcuts, then Hold to Dictate. For reliable activation on GNOME use an ordinary combination such as Alt+Z. Polar Station installs its GNOME capture extension automatically. After first installation, sign out and back in once to activate it. Both interaction modes then use complete physical key events. Other desktops use their system shortcut editor.").as(gtk.Widget));
        root.append(label("External inputs must support accessibility editing. On desktops without accessibility click events, unbound drafts stay saved: use Copy in Dictate drafts. Recording into the already focused external input uses the global shortcut.").as(gtk.Widget));
        root.append(label("Microphone · choose System default or a fixed input. A missing fixed input stops capture.").as(gtk.Widget));
        try self.device_ids.append(a, try a.dupeZ(u8, ""));
        self.devices.setModel(self.device_names.as(gio.ListModel));
        root.append(self.devices.as(gtk.Widget));
        root.append(label("Transcription mode · changes apply to the next recording").as(gtk.Widget));
        root.append(self.transcription_mode.as(gtk.Widget));
        root.append(self.cleanup.as(gtk.Widget));
        root.append(self.context.as(gtk.Widget));
        root.append(label("Adam · Luna · low. Original language and meaning are preserved. After 8 seconds the original stays available.").as(gtk.Widget));
        self.terminology_root.append(self.terminology_status.as(gtk.Widget));
        self.terminology_root.append(label("Terminology · one term per line: standard spelling | heard variant | another variant").as(gtk.Widget));
        const glossary_scroll = gtk.ScrolledWindow.new();
        glossary_scroll.setMinContentHeight(100); glossary_scroll.setMaxContentHeight(180);
        glossary_scroll.setPolicy(.never, .automatic); self.glossary.setWrapMode(.word_char);
        glossary_scroll.setChild(self.glossary.as(gtk.Widget)); self.terminology_root.append(glossary_scroll.as(gtk.Widget));
        self.terminology_root.append(label("Example: Polar Station | polarstation | 波拉站. Edit or remove lines, then save. No automatic learning or sync.").as(gtk.Widget));
        button(self.terminology_root, "Save terminology", save, self);
        button(root, "Save Voice settings", save, self);
        const test_row = gtk.Box.new(.horizontal, 8);
        root.append(test_row.as(gtk.Widget));
        button(test_row, "Local mic test (10s, no upload)", localTest, self);
        button(test_row, "Transcription test", transcriptionTest, self);
        button(test_row, "Stop test", stopTest, self);
        root.append(self.meter.as(gtk.Widget));
        root.append(self.playback_controls.as(gtk.Widget));
        button(root, "Stop playback", stopPlayback, self);
        self.test_editor.setWrapMode(.word_char);
        self.test_editor.as(gtk.Widget).setSizeRequest(-1, 100);
        root.append(self.test_editor.as(gtk.Widget));
        button(root, "Copy test text", copyTest, self);
        root.append(label("Test text stays here. Test recordings are marked Test and can be recovered below.").as(gtk.Widget));
        root.append(label("Recordings · completed audio is kept 7 days; failed, interrupted, pinned and unconfirmed recordings are retained.").as(gtk.Widget));
        root.append(self.usage.as(gtk.Widget));
        const pager = gtk.Box.new(.horizontal, 8);
        root.append(pager.as(gtk.Widget));
        button(pager, "Previous", previous, self);
        button(pager, "Next", next, self);
        button(pager, "Refresh recordings", refresh, self);
        root.append(self.records.as(gtk.Widget));
        root.append(self.details.as(gtk.Widget));
        self.details.setSelectable(1);
        root.append(label("Diagnostics: built-in Opus/WebM encoder · 1-second durable saves · up to 30-second transcription segments · no shell PATH dependencies.").as(gtk.Widget));
        button(root, "Copy diagnostics", copyDiagnostics, self);
        self.thread = try std.Thread.spawn(.{}, worker, .{self});
        self.reload();
        self.enumerateDevices();
        return self;
    }
    fn copyDiagnostics(_: *gtk.Button, self: *Panel) callconv(.c) void {
        const config = if (self.config) |value| field(value.value, "payload") else .null;
        const text = std.fmt.allocPrint(a, "Polar Station Voice\nEncoder: built-in Opus/WebM\nMaximum: 600s; durable blocks: 1s\nCleanup: {s}; context: {s}\nAudio device: {s}\nAccount/audio/transcript are excluded.", .{ if (boolean(config, "cleanup")) "enabled" else "disabled", if (boolean(config, "context")) "enabled" else "disabled", if (string(config, "microphone").len == 0) "system default" else "fixed input" }) catch return;
        defer a.free(text);
        copyText(text);
        self.setStatus("Diagnostics copied without account details, audio or text");
    }
    fn button(box: *gtk.Box, title: [:0]const u8, comptime f: anytype, self: *Panel) void {
        const b = gtk.Button.newWithLabel(title);
        _ = gtk.Button.signals.clicked.connect(b, *Panel, f, self, .{});
        box.append(b.as(gtk.Widget));
    }
    pub fn destroy(self: *Panel) void {
        self.stopped = true;
        self.cancelTests();
        if (self.listing) |audio| c.polar_station_dictate_audio_free(audio);
        self.incoming.push(self);
        if (self.thread) |t| t.join();
        while (glib.Source.removeByUserData(self) != 0) {}
        while (self.outgoing.tryPop()) |raw| freeJob(@ptrCast(@alignCast(raw)));
        self.incoming.unref();
        self.outgoing.unref();
        c.polar_station_dictate_speech_free(self.speech);
        if (self.config) |*v| v.deinit();
        if (self.selected) |*v| v.deinit();
        if (self.test_id) |v| a.free(v);
        for (self.device_ids.items) |v| a.free(v);
        self.device_ids.deinit(a);
        if (self.local_pcm.items.len > 0) @memset(self.local_pcm.items, 0);
        self.local_pcm.deinit(a);
        self.clearRows();
        self.actions.deinit(a);
        self.device_names.unref();
        a.destroy(self);
    }
    pub fn recheckAccount(self: *Panel) void {
        self.send(.{ .op = "voice_status" });
    }
    fn send(self: *Panel, value: anytype) void {
        const request = std.json.Stringify.valueAlloc(a, value, .{}) catch return;
        const job = a.create(Job) catch {
            a.free(request);
            return;
        };
        job.* = .{ .request = request, .playback_generation = self.playback_generation };
        self.incoming.push(job);
    }
    fn reload(self: *Panel) void {
        self.send(.{ .op = "transcription_modes" });
        self.send(.{ .op = "shortcut_modes" });
        self.send(.{ .op = "voice_status" });
        self.send(.{ .op = "usage" });
        self.send(.{ .op = "voice_settings" });
        self.send(.{ .op = "list", .offset = self.offset, .limit = 25 });
    }
    fn worker(self: *Panel) void {
        while (true) {
            const raw = self.incoming.pop();
            if (raw == @as(*anyopaque, @ptrCast(self))) return;
            const job: *Job = @ptrCast(@alignCast(raw));
            job.result = if (job.local_pcm) |pcm| c.polar_station_dictate_local_wav(self.speech, pcm.ptr, pcm.len) else if (job.transcribe) c.polar_station_dictate_speech_transcribe(self.speech, job.request.ptr, job.request.len, null, 0) else c.polar_station_dictate_service_request(self.speech, job.request.ptr, job.request.len);
            self.outgoing.push(job);
            _ = glib.idleAdd(receive, self);
        }
    }
    fn freeJob(job: *Job) void {
        a.free(job.request);
        if (job.local_pcm) |pcm| {
            @memset(pcm, 0);
            a.free(pcm);
        }
        if (job.result != null) c.polar_station_desktop_string_free(job.result);
        a.destroy(job);
    }
    fn receive(data: ?*anyopaque) callconv(.c) c_int {
        const self: *Panel = @ptrCast(@alignCast(data orelse return 0));
        while (self.outgoing.tryPop()) |raw| {
            const job: *Job = @ptrCast(@alignCast(raw));
            defer freeJob(job);
            if (job.result == null) {
                self.setStatus("Voice client returned no result");
                continue;
            }
            var response = std.json.parseFromSlice(Value, a, std.mem.span(job.result), .{ .allocate = .alloc_always }) catch continue;
            defer response.deinit();
            if (job.transcribe) {
                const pending = self.pending_test orelse continue;
                if (!eq(pending, job.request)) continue;
                a.free(pending);
                self.pending_test = null;
            }
            if (!boolean(response.value, "ok")) {
                self.setStatus(string(response.value, "error"));
                continue;
            }
            const payload = field(response.value, "payload");
            if (job.transcribe) {
                const input = field(response.value, "input");
                const text = string(input, "text");
                if (text.len > 0) {
                    setEditorText(self.test_editor, text);
                    self.setStatus("Transcription passed. Test text was not inserted elsewhere.");
                    self.send(.{ .op = "get", .id = job.request });
                } else self.setStatus(string(input, "error"));
                self.send(.{ .op = "list", .offset = self.offset, .limit = 25 });
                continue;
            }
            if (job.local_pcm != null) {
                if (job.playback_generation != self.playback_generation) continue;
                self.openPath(string(payload, "path"));
                continue;
            }
            var request = std.json.parseFromSlice(Value, a, job.request, .{}) catch continue;
            defer request.deinit();
            const op = string(request.value, "op");
            if (eq(op, "voice_login")) {
                const url = a.dupeZ(u8, string(payload, "authorization_url")) catch continue;
                defer a.free(url);
                _ = gio.AppInfo.launchDefaultForUri(url, null, null);
                self.setStatus("Complete browser sign-in, then Recheck.");
            } else if (eq(op, "shortcut_modes")) {
                const choices = field(payload, "choices");
                if (choices == .array) {
                    const names = gtk.StringList.new(null); defer names.unref();
                    for (choices.array.items) |choice| {
                        const text = a.dupeZ(u8, string(choice, "label")) catch continue; defer a.free(text);
                        names.append(text);
                    }
                    self.shortcut_mode.setModel(names.as(gio.ListModel));
                    if (self.config) |config| self.shortcut_mode.setSelected(if (eq(string(field(config.value, "payload"), "shortcut_mode"), "toggle")) 1 else 0);
                }
            } else if (eq(op, "transcription_modes")) {
                const choices = field(payload, "choices");
                if (choices == .array) {
                    const names = gtk.StringList.new(null);
                    defer names.unref();
                    for (choices.array.items) |choice| {
                        const text = a.dupeZ(u8, string(choice, "label")) catch continue;
                        defer a.free(text);
                        names.append(text);
                    }
                    self.transcription_mode.setModel(names.as(gio.ListModel));
                    if (self.config) |config| self.transcription_mode.setSelected(if (eq(string(field(config.value, "payload"), "transcription_mode"), "segmented")) 1 else 0);
                }
            } else if (eq(op, "voice_status")) {
                setText(self.account, if (boolean(payload, "signed_in")) string(payload, "account") else "Sign in to enable transcription. No CLI installation is needed.");
                self.setStatus("Account checked. Microphone and transcription tests are separate.");
            } else if (eq(op, "voice_settings") or eq(op, "voice_settings_update")) {
                if (self.config) |*v| v.deinit();
                self.config = std.json.parseFromSlice(Value, a, std.mem.span(job.result), .{ .allocate = .alloc_always }) catch null;
                self.transcription_mode.setSelected(if (eq(string(payload, "transcription_mode"), "segmented")) 1 else 0);
                self.shortcut_mode.setSelected(if (eq(string(payload, "shortcut_mode"), "toggle")) 1 else 0);
                self.cleanup.setActive(@intFromBool(boolean(payload, "cleanup")));
                self.context.setActive(@intFromBool(boolean(payload, "context")));
                self.selectDevice(string(payload, "microphone"));
                setEditorText(self.glossary, string(payload, "glossary"));
                if (eq(op, "voice_settings_update")) {
                    self.setStatus("Voice settings saved");
                    if (self.callback) |cb| cb(self.callback_context, false);
                }
            } else if (eq(op, "usage")) {
                const value = std.fmt.allocPrint(a, "Saved audio on this computer: {d} bytes · {d} recordings", .{ number(field(payload, "usage"), "audio_bytes"), number(field(payload, "usage"), "recordings") }) catch continue;
                defer a.free(value);
                setText(self.usage, value);
            } else if (eq(op, "list")) {
                self.renderRows(field(payload, "recordings"));
            } else if (eq(op, "get")) {
                if (self.selected) |*v| v.deinit();
                self.selected = std.json.parseFromSlice(Value, a, std.mem.span(job.result), .{ .allocate = .alloc_always }) catch null;
                const record = field(payload, "recording");
                self.renderDetail(record);
                if (boolean(record, "is_test")) {
                    if (string(record, "cleaned").len > 0) {
                        const text = string(record, "cleaned");
                        setEditorText(self.test_editor, text);
                        self.setStatus("Transcription passed · cleanup passed. Test text stays here.");
                    } else if (string(record, "cleanup_error").len > 0) self.setStatus("Transcription passed · cleanup failed; original retained.");
                }
            } else if (eq(op, "export_audio")) {
                if (field(request.value, "path") == .string) self.setStatus("WAV exported") else if (job.playback_generation == self.playback_generation) self.openPath(string(payload, "path"));
            } else {
                self.setStatus("Voice operation saved");
                if (eq(op, "delete")) {
                    if (self.recording_deleted) |deleted| deleted(self.callback_context, string(request.value, "id"));
                    if (self.selected) |*selected| selected.deinit();
                    self.selected = null;
                    setText(self.details, "Recording deleted");
                }
                self.send(.{ .op = "list", .offset = self.offset, .limit = 25 });
                self.send(.{ .op = "usage" });
                const id = string(request.value, "id");
                if (id.len > 0 and !eq(op, "delete")) self.send(.{ .op = "get", .id = id });
            }
        }
        return 0;
    }
    fn stopPlayback(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.stopPlayer();
    }
    fn stopPlayer(self: *Panel) void {
        self.playback_generation +%= 1;
        if (self.player) |player| {
            player.as(gtk.MediaStream).pause();
            self.playback_controls.setMediaStream(null);
            player.unref();
            self.player = null;
        }
    }
    fn playbackError(_: *gobject.Object, _: *gobject.ParamSpec, self: *Panel) callconv(.c) void {
        if (self.player) |player| if (player.as(gtk.MediaStream).getError()) |err| {
            self.setStatus(std.mem.span(err.f_message orelse "Audio playback is unavailable"));
        };
    }
    fn openPath(self: *Panel, path: []const u8) void {
        const p = a.dupeZ(u8, path) catch return;
        defer a.free(p);
        self.stopPlayer();
        const player = gtk.MediaFile.newForFilename(p);
        self.player = player;
        _ = gobject.Object.signals.notify.connect(player.as(gobject.Object), *Panel, playbackError, self, .{ .detail = "error" });
        self.playback_controls.setMediaStream(player.as(gtk.MediaStream));
        player.as(gtk.MediaStream).play();
        self.setStatus("Playing saved audio · pause and seek with the playback controls");
    }
    fn clearRows(self: *Panel) void {
        self.detail_row = null;
        self.detail_action_start = 0;
        while (self.records.as(gtk.Widget).getFirstChild()) |child| self.records.remove(child);
        for (self.actions.items) |v| {
            a.free(v.id);
            a.destroy(v);
        }
        self.actions.clearRetainingCapacity();
    }
    fn rowButton(self: *Panel, row: *gtk.Box, title: [:0]const u8, id: []const u8, op: []const u8, pinned: bool, start: i64) void {
        const action = a.create(RowAction) catch return;
        action.* = .{ .panel = self, .id = a.dupe(u8, id) catch {
            a.destroy(action);
            return;
        }, .op = op, .pinned = pinned, .start = start };
        self.actions.append(a, action) catch {
            a.free(action.id);
            a.destroy(action);
            return;
        };
        const b = gtk.Button.newWithLabel(title);
        _ = gtk.Button.signals.clicked.connect(b, *RowAction, rowClicked, action, .{});
        row.append(b.as(gtk.Widget));
    }
    fn renderRows(self: *Panel, items: Value) void {
        self.clearRows();
        if (items != .array) return;
        for (items.array.items) |item| {
            const row = gtk.Box.new(.vertical, 6);
            self.records.append(row.as(gtk.Widget));
            const id = string(item, "id");
            const title = std.fmt.allocPrintSentinel(a, "{s}{d}s · {d}/{d} · {s}", .{ if (boolean(item, "is_test")) "Test · " else "", @divTrunc(number(item, "saved_samples"), 16000), number(item, "segments_complete"), number(item, "segments_total"), string(item, "capture") }, 0) catch continue;
            defer a.free(title);
            const date = glib.DateTime.newFromUnixLocal(@divTrunc(number(item, "created_at_ms"), 1000));
            defer if (date) |v| v.unref();
            const stamp = if (date) |v| v.format("%Y-%m-%d %H:%M") else null;
            defer if (stamp) |v| glib.free(v);
            if (stamp) |value| row.append(label(std.mem.span(value)).as(gtk.Widget));
            self.rowButton(row, title, id, "get", false, 0);
            const actions = gtk.Box.new(.horizontal, 6);
            row.append(actions.as(gtk.Widget));
            self.rowButton(actions, "Retry", id, "retry", false, 0);
            if (string(item, "retry_segmented_label").len > 0) {
                const retry_label = a.dupeZ(u8, string(item, "retry_segmented_label")) catch continue;
                defer a.free(retry_label);
                self.rowButton(actions, retry_label, id, "retry_segmented", false, 0);
            }
            self.rowButton(actions, if (boolean(item, "pinned")) "Unpin" else "Pin", id, "pin", !boolean(item, "pinned"), 0);
            self.rowButton(actions, "Delete…", id, "delete", false, 0);
        }
    }
    fn renderDetail(self: *Panel, record: Value) void {
        if (self.detail_row) |old| {
            self.records.remove(old.as(gtk.Widget));
            while (self.actions.items.len > self.detail_action_start) {
                const action = self.actions.pop().?;
                a.free(action.id);
                a.destroy(action);
            }
        }
        self.detail_action_start = self.actions.items.len;
        var text: std.Io.Writer.Allocating = .init(a);
        defer text.deinit();
        const w = &text.writer;
        w.print("{s}\nSaved: {d}s · delivery: {s}\n{s}\n{s}\n", .{ string(record, "id"), @divTrunc(number(record, "saved_samples"), 16000), string(record, "delivery"), string(record, "error"), string(record, "cleanup_error") }) catch {};
        if (number(record, "completed_at_ms") > 0 and !boolean(record, "pinned")) {
            const date = glib.DateTime.newFromUnixLocal(@divTrunc(number(record, "completed_at_ms"), 1000) + 7 * 86400);
            defer if (date) |v| v.unref();
            const expiry = if (date) |v| v.format("%Y-%m-%d %H:%M") else null;
            defer if (expiry) |v| glib.free(v);
            if (expiry) |stamp| w.print("Audio expires {s}\n", .{stamp}) catch {};
        }
        const segments = field(record, "segments");
        if (segments == .array) for (segments.array.items) |s| {
            w.print("{d}s: {s}\n{s}\n", .{ @divTrunc(number(s, "start_sample"), 16000), string(s, "state"), string(s, "original") }) catch {};
        };
        const cleaned = string(record, "cleaned");
        if (cleaned.len > 0) {
            w.print("\nCleaned text:\n{s}", .{cleaned}) catch {};
        }
        setText(self.details, text.written());
        const group = gtk.Box.new(.vertical, 6);
        self.records.append(group.as(gtk.Widget));
        self.detail_row = group;
        const row = gtk.Box.new(.horizontal, 6);
        group.append(row.as(gtk.Widget));
        const copies = gtk.Box.new(.horizontal, 6);
        group.append(copies.as(gtk.Widget));
        const id = string(record, "id");
        self.rowButton(row, "Play", id, "play", false, 0);
        self.rowButton(row, "Export WAV…", id, "export", false, 0);
        self.rowButton(row, "Use Original", id, "use_original", false, 0);
        self.rowButton(row, "Accept text", id, "accept", false, 0);
        self.rowButton(copies, "Copy original", id, "copy_original", false, 0);
        self.rowButton(copies, "Copy cleaned", id, "copy_cleaned", false, 0);
        const recovery = gtk.Box.new(.horizontal, 6);
        group.append(recovery.as(gtk.Widget));
        self.rowButton(recovery, "Use in next input", id, "recover_recording", false, 0);
        if (segments == .array) for (segments.array.items) |s| {
            if (eq(string(s, "state"), "failed")) {
                const range_row = gtk.Box.new(.horizontal, 6);
                group.append(range_row.as(gtk.Widget));
                self.rowButton(range_row, "Play failed range", id, "play_range", false, number(s, "start_sample"));
            }
        };
    }
    fn rowClicked(_: *gtk.Button, action: *RowAction) callconv(.c) void {
        const self = action.panel;
        if (eq(action.op, "recover_recording")) {
            if (self.recover_recording) |recover| recover(self.callback_context, action.id);
        } else if (eq(action.op, "copy_original") or eq(action.op, "copy_cleaned")) {
            const selected = self.selected orelse return;
            const record = field(field(selected.value, "payload"), "recording");
            if (eq(action.op, "copy_cleaned")) {
                copyText(string(record, "cleaned"));
            } else {
                var text: std.Io.Writer.Allocating = .init(a);
                defer text.deinit();
                const segments = field(record, "segments");
                if (segments == .array) for (segments.array.items) |part| {
                    text.writer.print("{s}\n", .{string(part, "original")}) catch {};
                };
                copyText(text.written());
            }
        } else if (eq(action.op, "delete")) {
            self.closeDialogs();
            const dialog = gtk.MessageDialog.new(self.parentWindow(), .{ .modal = true, .destroy_with_parent = true }, .warning, .none, "Delete this saved recording?");
            _ = dialog.as(gtk.Dialog).addButton("Cancel", @intFromEnum(gtk.ResponseType.cancel));
            _ = dialog.as(gtk.Dialog).addButton("Delete audio and text", @intFromEnum(gtk.ResponseType.accept));
            const id = a.dupeZ(u8, action.id) catch return;
            dialog.as(gobject.Object).setDataFull("voice-recording-id", id.ptr, freeString);
            _ = gtk.Dialog.signals.response.connect(dialog.as(gtk.Dialog), *Panel, deleteResponse, self, .{});
            self.delete_dialog = dialog.as(gtk.Dialog);
            dialog.as(gtk.Window).present();
        } else if (eq(action.op, "export")) {
            self.closeDialogs();
            const chooser = gtk.FileChooserNative.new("Export Dictate WAV", self.parentWindow(), .save, "Export", "Cancel");
            _ = chooser.as(gtk.FileChooser).setCurrentName("Dictate.wav");
            const id = a.dupeZ(u8, action.id) catch return;
            chooser.as(gobject.Object).setDataFull("voice-recording-id", id.ptr, freeString);
            _ = gtk.NativeDialog.signals.response.connect(chooser.as(gtk.NativeDialog), *Panel, exportResponse, self, .{});
            self.export_dialog = chooser.as(gtk.NativeDialog);
            chooser.as(gtk.NativeDialog).show();
        } else if (eq(action.op, "play_range")) {
            const selected = self.selected orelse return;
            const record = field(field(selected.value, "payload"), "recording");
            const segments = field(record, "segments");
            if (segments == .array) for (segments.array.items) |part| {
                if (number(part, "start_sample") == action.start) {
                    self.send(.{ .op = "export_audio", .id = action.id, .start_sample = action.start, .sample_count = number(part, "sample_count") });
                    break;
                }
            };
        } else if (eq(action.op, "play")) self.send(.{ .op = "export_audio", .id = action.id, .start_sample = action.start }) else if (eq(action.op, "pin")) self.send(.{ .op = "pin", .id = action.id, .pinned = action.pinned }) else if (eq(action.op, "accept")) self.send(.{ .op = "accept", .id = action.id, .now_ms = std.time.milliTimestamp() }) else self.send(.{ .op = action.op, .id = action.id });
    }
    fn parentWindow(self: *Panel) ?*gtk.Window {
        return gobject.ext.cast(gtk.Window, self.root.as(gtk.Widget).getRoot() orelse return null);
    }
    fn closeDialogs(self: *Panel) void {
        if (self.delete_dialog) |dialog| {
            self.delete_dialog = null;
            dialog.as(gtk.Window).destroy();
        }
        if (self.export_dialog) |dialog| {
            self.export_dialog = null;
            dialog.destroy();
            dialog.unref();
        }
    }
    fn freeString(data: ?*anyopaque) callconv(.c) void {
        if (data) |p| {
            const s: [*:0]u8 = @ptrCast(p);
            a.free(std.mem.span(s));
        }
    }
    fn deleteResponse(dialog: *gtk.Dialog, response: c_int, self: *Panel) callconv(.c) void {
        self.delete_dialog = null;
        if (response == @intFromEnum(gtk.ResponseType.accept)) if (dialog.as(gobject.Object).getData("voice-recording-id")) |p| {
            const id = std.mem.span(@as([*:0]const u8, @ptrCast(p)));
            self.send(.{ .op = "delete", .id = id, .confirmation = id });
        };
        dialog.as(gtk.Window).destroy();
    }
    fn exportResponse(dialog: *gtk.NativeDialog, response: c_int, self: *Panel) callconv(.c) void {
        self.export_dialog = null;
        defer {
            dialog.destroy();
            dialog.unref();
        }
        const chooser = gobject.ext.cast(gtk.FileChooser, dialog) orelse return;
        if (response == @intFromEnum(gtk.ResponseType.accept)) if (chooser.getFile()) |file| {
            defer file.unref();
            if (file.getPath()) |path| {
                defer glib.free(path);
                if (dialog.as(gobject.Object).getData("voice-recording-id")) |p| self.send(.{ .op = "export_audio", .id = std.mem.span(@as([*:0]const u8, @ptrCast(p))), .path = std.mem.span(path) });
            }
        };
    }
    fn selectDevice(self: *Panel, id: []const u8) void {
        for (self.device_ids.items, 0..) |value, i| {
            if (eq(value, id)) {
                self.devices.setSelected(@intCast(i));
                return;
            }
        }
        if (id.len > 0) {
            const saved = a.dupeZ(u8, id) catch return;
            self.device_ids.append(a, saved) catch {
                a.free(saved);
                return;
            };
            self.device_names.append("Configured input (unavailable)");
            self.devices.setSelected(@intCast(self.device_ids.items.len - 1));
        }
    }
    fn enumerateDevices(self: *Panel) void {
        if (self.listing) |old| c.polar_station_dictate_audio_free(old);
        self.listing = c.polar_station_dictate_audio_new(listingEvent, self, 1);
        c.polar_station_dictate_audio_set_devices_callback(self.listing, deviceFound);
        if (!c.polar_station_dictate_audio_start(self.listing)) self.setStatus("Audio service unavailable. Check the microphone connection and reopen Voice settings.");
    }
    fn listingEvent(data: ?*anyopaque, event: c_int) callconv(.c) void {
        const self: *Panel = @ptrCast(@alignCast(data orelse return));
        if (event == c.POLAR_DICTATE_AUDIO_ERROR) self.setStatus(std.mem.span(c.polar_station_dictate_audio_error(self.listing)));
    }
    fn deviceFound(data: ?*anyopaque, id: [*c]const u8, name: [*c]const u8, done: bool) callconv(.c) void {
        const self: *Panel = @ptrCast(@alignCast(data orelse return));
        if (done) {
            if (self.config) |config| self.selectDevice(string(field(config.value, "payload"), "microphone"));
            return;
        }
        if (id == null) return;
        self.device_ids.append(a, a.dupeZ(u8, std.mem.span(id)) catch return) catch return;
        self.device_names.append(if (name == null) id else name);
    }
    fn refresh(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.reload();
    }
    fn previous(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.offset -|= 25;
        self.reload();
    }
    fn next(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.offset +|= 25;
        self.reload();
    }
    fn signIn(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.send(.{ .op = "voice_login" });
    }
    fn cancelLogin(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.send(.{ .op = "voice_cancel_login" });
    }
    fn shortcut(_: *gtk.Button, self: *Panel) callconv(.c) void {
        if (self.callback) |cb| cb(self.callback_context, true);
    }
    fn save(_: *gtk.Button, self: *Panel) callconv(.c) void {
        const config = if (self.config) |v| field(v.value, "payload") else return;
        const index = self.devices.getSelected();
        const microphone: []const u8 = if (index < self.device_ids.items.len) self.device_ids.items[index] else string(config, "microphone");
        const buffer = self.glossary.getBuffer();
        var start: gtk.TextIter = undefined; var end: gtk.TextIter = undefined;
        buffer.getBounds(&start, &end); const glossary = buffer.getText(&start, &end, 0); defer glib.free(glossary);
        self.send(.{ .op = "voice_settings_update", .settings = .{ .shortcut_mode = if (self.shortcut_mode.getSelected() == 1) @as([]const u8, "toggle") else "hold", .revision = number(config, "revision"), .transcription_mode = if (self.transcription_mode.getSelected() == 1) @as([]const u8, "segmented") else "realtime", .glossary = std.mem.span(glossary), .cleanup = self.cleanup.getActive() != 0, .context = self.context.getActive() != 0, .microphone = microphone, .shortcut_mac = string(config, "shortcut_mac"), .shortcut_linux = string(config, "shortcut_linux") } });
    }
    fn localTest(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.startTest(false);
    }
    fn transcriptionTest(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.startTest(true);
    }
    fn copyText(text: []const u8) void {
        const display = gdk.Display.getDefault() orelse return;
        const value = a.dupeZ(u8, text) catch return;
        defer a.free(value);
        display.getClipboard().setText(value);
    }
    fn copyTest(_: *gtk.Button, self: *Panel) callconv(.c) void {
        const buffer = self.test_editor.getBuffer();
        var start: gtk.TextIter = undefined;
        var end: gtk.TextIter = undefined;
        buffer.getBounds(&start, &end);
        const text = buffer.getText(&start, &end, 0);
        defer glib.free(text);
        copyText(std.mem.span(text));
    }
    fn startTest(self: *Panel, upload: bool) void {
        if (self.test_id != null or self.pending_test != null) {
            self.setStatus("Stop the current test first");
            return;
        }
        const id = std.fmt.allocPrint(a, "voice-test-{x}", .{std.crypto.random.int(u128)}) catch return;
        self.test_id = id;
        self.test_upload = upload;
        if (self.local_pcm.items.len > 0) @memset(self.local_pcm.items, 0);
        self.local_pcm.clearRetainingCapacity();
        if (upload) {
            const input = std.json.Stringify.valueAlloc(a, .{ .op = "capture_begin", .id = id, .is_test = true }, .{}) catch return;
            defer a.free(input);
            const response = c.polar_station_dictate_service_request(self.speech, input.ptr, input.len);
            defer if (response != null) c.polar_station_desktop_string_free(response);
            var value = if (response != null) std.json.parseFromSlice(Value, a, std.mem.span(response), .{}) catch null else null;
            defer if (value) |*v| v.deinit();
            if (value == null or !boolean(value.?.value, "ok")) {
                self.setStatus("Voice test could not start. Recheck the local Host.");
                self.cancelTests();
                return;
            }
        }
        self.audio = c.polar_station_dictate_audio_new(audioEvent, self, 10000);
        c.polar_station_dictate_audio_set_chunk_callback(self.audio, audioChunk);
        const index = self.devices.getSelected();
        if (index < self.device_ids.items.len) _ = c.polar_station_dictate_audio_set_device(self.audio, self.device_ids.items[index]);
        if (!c.polar_station_dictate_audio_start(self.audio)) {
            self.setStatus("Microphone capture could not start");
            self.cancelTests();
            return;
        }
        self.setStatus("Recording test… maximum 10 seconds. Stop when ready.");
    }
    fn audioChunk(data: ?*anyopaque, pcm: [*c]const u8, length: usize) callconv(.c) bool {
        const self: *Panel = @ptrCast(@alignCast(data orelse return false));
        const id = self.test_id orelse return false;
        var peak: f64 = 0;
        var offset: usize = 0;
        while (offset + 1 < length) : (offset += 2) {
            const sample: i32 = std.mem.readInt(i16, pcm[offset..][0..2], .little);
            peak = @max(peak, @as(f64, @floatFromInt(@abs(sample))) / 32768.0);
        }
        self.meter.setValue(peak);
        if (!self.test_upload) {
            if (self.local_pcm.items.len + length > 320000) return false;
            self.local_pcm.appendSlice(a, pcm[0..length]) catch return false;
            return true;
        }
        return c.polar_station_dictate_speech_append(self.speech, id.ptr, id.len, pcm, length);
    }
    fn audioEvent(data: ?*anyopaque, event: c_int) callconv(.c) void {
        const self: *Panel = @ptrCast(@alignCast(data orelse return));
        self.audio_events |= @as(u8, 1) << @intCast(event);
        _ = glib.idleAdd(audioReady, self);
    }
    fn audioReady(data: ?*anyopaque) callconv(.c) c_int {
        const self: *Panel = @ptrCast(@alignCast(data orelse return 0));
        const events = self.audio_events;
        self.audio_events = 0;
        if (events & 4 != 0) {
            self.setStatus(std.mem.span(c.polar_station_dictate_audio_error(self.audio)));
            self.cancelTests();
        } else if (events & 2 != 0) self.finishTest();
        return 0;
    }
    fn stopTest(_: *gtk.Button, self: *Panel) callconv(.c) void {
        self.finishTest();
    }
    pub fn cancelTests(self: *Panel) void {
        self.stopPlayer();
        self.closeDialogs();
        if (self.test_id) |id| {
            c.polar_station_dictate_speech_cancel(self.speech, id.ptr, id.len);
            a.free(id);
            self.test_id = null;
        }
        if (self.pending_test) |id| {
            c.polar_station_dictate_speech_cancel(self.speech, id.ptr, id.len);
            a.free(id);
            self.pending_test = null;
        }
        if (self.audio) |audio| {
            c.polar_station_dictate_audio_free(audio);
            self.audio = null;
        }
        self.audio_events = 0;
    }
    fn finishTest(self: *Panel) void {
        const id = self.test_id orelse return;
        self.test_id = null;
        c.polar_station_dictate_audio_stop(self.audio);
        const job = a.create(Job) catch {
            a.free(id);
            return;
        };
        job.* = .{ .request = id, .transcribe = self.test_upload, .playback_generation = self.playback_generation };
        if (self.test_upload) {
            if (!c.polar_station_dictate_speech_reserve(self.speech, id.ptr, id.len)) {
                a.destroy(job);
                a.free(id);
                return;
            }
            self.pending_test = a.dupe(u8, id) catch null;
            self.setStatus("Test audio saved; transcribing…");
        } else {
            const length = self.local_pcm.items.len;
            const pcm = self.local_pcm.items;
            if (length == 0) {
                a.destroy(job);
                a.free(id);
                self.cancelTests();
                self.setStatus("No microphone audio was captured");
                return;
            }
            job.local_pcm = a.dupe(u8, pcm[0..length]) catch {
                a.destroy(job);
                a.free(id);
                return;
            };
            self.setStatus("Local microphone test complete. No audio was uploaded.");
        }
        c.polar_station_dictate_audio_free(self.audio);
        self.audio = null;
        self.incoming.push(job);
    }
};
