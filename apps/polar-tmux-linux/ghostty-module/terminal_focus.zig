//! Focus belongs to the resident Pane binding, independent of recovery UI.
const std = @import("std");
const gtk = @import("gtk");
const gobject = @import("gobject");
const content = @import("content_lifecycle.zig");
const Surface = @import("../class/surface.zig").Surface;
const ipc = @import("ipc_client.zig");

// All resident Panes share one native ordering domain. GTK owns this thread.
var selection_sequence: u64 = 0;

pub const Focus = struct {
    lifecycle: *content.Lifecycle,
    surface: *Surface,
    socket: []const u8,
    channel: []const u8,
    generation: u64,
    sequence: u64 = 0,
    focused: bool = false,
    ready_generation: u64 = 0,
    controller: *gtk.EventControllerFocus,
    click: *gtk.GestureClick,
    map_handler: c_ulong,
    init_handler: c_ulong = 0,
    observing: bool = false,

    pub fn create(allocator: std.mem.Allocator, lifecycle: *content.Lifecycle, surface: *Surface, socket: []const u8, channel: []const u8, generation: u64) !*Focus {
        const self = try allocator.create(Focus);
        const controller = gtk.EventControllerFocus.new();
        const click = gtk.GestureClick.new();
        self.* = .{ .lifecycle = lifecycle, .surface = surface, .socket = socket, .channel = channel, .generation = generation, .controller = controller, .click = click, .map_handler = 0 };
        _ = gtk.EventControllerFocus.signals.enter.connect(controller, *Focus, entered, self, .{});
        _ = gtk.EventControllerFocus.signals.leave.connect(controller, *Focus, left, self, .{});
        surface.as(gtk.Widget).addController(controller.as(gtk.EventController));
        click.as(gtk.GestureSingle).setButton(1);
        click.as(gtk.EventController).setPropagationPhase(.capture);
        _ = gtk.GestureClick.signals.pressed.connect(click, *Focus, clicked, self, .{});
        surface.as(gtk.Widget).addController(click.as(gtk.EventController));
        self.map_handler = gtk.Widget.signals.map.connect(surface.as(gtk.Widget), *Focus, mapped, self, .{});
        self.init_handler = Surface.signals.init.connect(surface, *Focus, initialized, self, .{});
        errdefer { self.detach(); allocator.destroy(self); }
        try self.startObservation();
        self.sync(false);
        return self;
    }

    pub fn detach(self: *Focus) void {
        self.setNativeControl(false);
        gobject.signalHandlerDisconnect(self.surface.as(gobject.Object), self.init_handler);
        gobject.signalHandlerDisconnect(self.surface.as(gobject.Object), self.map_handler);
        self.surface.as(gtk.Widget).removeController(self.controller.as(gtk.EventController));
        self.surface.as(gtk.Widget).removeController(self.click.as(gtk.EventController));
    }

    pub fn ready(self: *Focus, generation: u64) void {
        ipc.trace(71, self.channel, generation);
        if (generation == self.ready_generation) return;
        self.ready_generation = generation;
        // The first native event can precede creation of the Pane Host socket.
        // Existing readiness observation is the retry trigger, never a timer.
        self.sync(true);
        ipc.trace(72, self.channel, @intFromBool(self.focused));
    }

    pub fn sync(self: *Focus, force: bool) void {
        self.startObservation() catch {};
        const active = self.windowActive() orelse return;
        self.publish(active and self.controller.containsFocus() != 0, force);
    }

    fn windowActive(self: *Focus) ?bool {
        const root = self.surface.as(gtk.Widget).getRoot() orelse return null;
        const window = gobject.ext.cast(gtk.Window, root.as(gobject.Object)) orelse return null;
        const widget = root.getFocus();
        const root_match = if (widget) |w| (w == self.surface.as(gtk.Widget) or w.isAncestor(self.surface.as(gtk.Widget)) != 0) else false;
        ipc.trace(59, self.channel, @intFromBool(root_match));
        ipc.trace(60, self.channel, @intFromBool(window.isActive() != 0));
        return window.isActive() != 0;
    }

    fn setNativeControl(self: *Focus, enabled: bool) void {
        const core = self.surface.core() orelse return;
        core.renderer_state.mutex.lock();
        defer core.renderer_state.mutex.unlock();
        core.renderer_state.polar_native_focus_control = enabled;
    }

    fn startObservation(self: *Focus) !void {
        if (self.observing or self.surface.core() == null) return;
        self.setNativeControl(true);
        try self.lifecycle.submitOperation(self.socket, self.channel, self.generation, "observe_recovery");
        self.observing = true;
    }

    fn initialized(_: *Surface, self: *Focus) callconv(.c) void {
        self.sync(false);
    }

    fn publish(self: *Focus, focused: bool, force: bool) void {
        self.startObservation() catch {};
        if (!force and self.focused == focused) return;
        const allocator = self.lifecycle.allocator;
        const request_id = self.lifecycle.allocateRequestId(allocator) catch return;
        defer allocator.free(request_id);
        selection_sequence += 1;
        const sequence = selection_sequence;
        const native_instance = std.fmt.allocPrint(allocator, "native-{d}-{x}", .{ self.lifecycle.process_id, self.lifecycle.run_nonce }) catch return;
        defer allocator.free(native_instance);
        const json = std.json.Stringify.valueAlloc(allocator, .{
            .protocol_version = @as(u32, 1),
            .api = "pane_content_v1",
            .request_id = request_id,
            .channel_id = self.channel,
            .channel_generation = self.generation,
            .content_revision = @max(self.ready_generation, 1),
            .command = .{ .operation = "terminal_control", .native_instance_id = native_instance, .focused = focused, .sequence = sequence, .cause = if (focused) "selection" else "blur" },
        }, .{}) catch return;
        defer allocator.free(json);
        self.lifecycle.submit(self.socket, json) catch return;
        self.focused = focused;
        self.sequence = sequence;
        ipc.trace(if (focused) 52 else 53, self.channel, sequence);
    }
    fn entered(_: *gtk.EventControllerFocus, self: *Focus) callconv(.c) void {
        ipc.trace(57, self.channel, @intCast(self.controller.containsFocus()));
        // Use the event itself while GTK is moving Root.focus.
        self.publish(self.windowActive() orelse false, false);
    }
    fn left(_: *gtk.EventControllerFocus, self: *Focus) callconv(.c) void {
        ipc.trace(58, self.channel, @intCast(self.controller.containsFocus()));
        // GTK emits leave before clearing containsFocus: the explicit event is
        // authoritative here; settled/window synchronization reads the getter.
        self.publish(false, false);
    }
    fn mapped(_: *gtk.Widget, self: *Focus) callconv(.c) void {
        self.sync(false);
    }
    fn clicked(_: *gtk.GestureClick, _: c_int, _: f64, _: f64, self: *Focus) callconv(.c) void {
        if (self.windowActive() orelse false) {
            // This capture gesture owns Pane selection before the terminal's
            // bubble gesture. Keep GTK focus and Host control on the same Pane.
            self.surface.grabFocus();
            ipc.trace(70, self.channel, selection_sequence + 1);
            self.publish(true, true);
        }
    }
};
