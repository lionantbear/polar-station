//! Thin GTK rendering of the Rust terminal recovery projection.
const std = @import("std");
const gtk = @import("gtk");
const content = @import("content_lifecycle.zig");

pub const Controls = struct {
    lifecycle: *content.Lifecycle,
    socket: []const u8,
    channel: []const u8,
    generation: u64,
    surface: *gtk.Widget,
    box: *gtk.Box,
    message: *gtk.Label,
    detail: *gtk.Label,
    retry: *gtk.Button,
    details: *gtk.Button,
    spinner: *gtk.Spinner,
    retry_enabled: bool = false,
    expanded: bool = false,

    pub fn create(allocator: std.mem.Allocator, lifecycle: *content.Lifecycle, overlay: *gtk.Overlay, surface: *gtk.Widget, socket: []const u8, channel: []const u8, generation: u64) !*Controls {
        const self = try allocator.create(Controls);
        const box = gtk.Box.new(.vertical, 6);
        const spinner = gtk.Spinner.new();
        const message = gtk.Label.new("");
        message.setWrap(1);
        message.setMaxWidthChars(34);
        message.setXalign(0);
        const detail = gtk.Label.new("");
        detail.setWrap(1);
        detail.setMaxWidthChars(34);
        detail.setXalign(0);
        detail.as(gtk.Widget).setVisible(0);
        const retry = gtk.Button.newWithLabel("");
        const details = gtk.Button.newWithLabel("");
        const row = gtk.Box.new(.horizontal, 6);
        row.append(retry.as(gtk.Widget)); row.append(details.as(gtk.Widget));
        box.append(spinner.as(gtk.Widget));
        box.append(message.as(gtk.Widget));
        box.append(row.as(gtk.Widget)); box.append(detail.as(gtk.Widget));
        const widget = box.as(gtk.Widget);
        widget.addCssClass("card");
        widget.setHalign(.end); widget.setValign(.start);
        widget.setMarginTop(8); widget.setMarginEnd(8); widget.setMarginStart(8);
        overlay.addOverlay(widget);
        overlay.setMeasureOverlay(widget, 0);
        overlay.setClipOverlay(widget, 1);
        self.* = .{ .surface = surface, .lifecycle = lifecycle, .socket = socket, .channel = channel, .generation = generation,
            .box = box, .message = message, .detail = detail, .retry = retry, .details = details, .spinner = spinner };
        _ = gtk.Button.signals.clicked.connect(retry, *Controls, retryClicked, self, .{});
        _ = gtk.Button.signals.clicked.connect(details, *Controls, detailsClicked, self, .{});
        return self;
    }

    pub fn render(self: *Controls, view: std.json.ObjectMap) !void {
        const allocator = self.lifecycle.allocator;
        const message = try allocator.dupeZ(u8, (view.get("message") orelse return error.InvalidRecovery).string);
        defer allocator.free(message);
        self.message.setText(message);
        inline for (.{ .{ "retry_label", self.retry }, .{ "details_label", self.details } }) |field| {
            const label = try allocator.dupeZ(u8, (view.get(field[0]) orelse return error.InvalidRecovery).string);
            defer allocator.free(label);
            field[1].setLabel(label);
        }
        const time_label = (view.get("probe_time_label") orelse return error.InvalidRecovery).string;
        const reason = (view.get("detail") orelse return error.InvalidRecovery).string;
        const stamp = view.get("last_probe_unix_ms");
        const detail = if (stamp != null and stamp.? == .integer)
            try std.fmt.allocPrintSentinel(allocator, "{s}\n{s}: {d:0>2}:{d:0>2}:{d:0>2}", .{ reason, time_label, @mod(@divTrunc(stamp.?.integer, 3600000), 24), @mod(@divTrunc(stamp.?.integer, 60000), 60), @mod(@divTrunc(stamp.?.integer, 1000), 60) }, 0)
        else try allocator.dupeZ(u8, reason);
        defer allocator.free(detail);
        self.detail.setText(detail);
        self.retry_enabled = (view.get("retry_enabled") orelse return error.InvalidRecovery).bool;
        self.retry.as(gtk.Widget).setSensitive(@intFromBool(self.retry_enabled));
        const phase = (view.get("phase") orelse return error.InvalidRecovery).string;
        const busy = std.mem.eql(u8, phase, "probing") or std.mem.eql(u8, phase, "attaching");
        self.spinner.setSpinning(@intFromBool(busy));
        self.spinner.as(gtk.Widget).setVisible(@intFromBool(busy));
        self.retry.as(gtk.Widget).setVisible(@intFromBool(!std.mem.eql(u8, phase, "hidden") and !std.mem.eql(u8, phase, "ended") and !std.mem.eql(u8, phase, "recovered")));
        self.box.as(gtk.Widget).setVisible(@intFromBool((view.get("visible") orelse return error.InvalidRecovery).bool and !std.mem.eql(u8, phase, "hidden")));
    }
    fn probe(self: *Controls) void {
        self.lifecycle.submitOperation(self.socket, self.channel, self.generation, "probe_recovery") catch {};
    }
    fn retryClicked(_: *gtk.Button, self: *Controls) callconv(.c) void { self.probe(); }
    fn detailsClicked(_: *gtk.Button, self: *Controls) callconv(.c) void {
        self.expanded = !self.expanded; self.detail.as(gtk.Widget).setVisible(@intFromBool(self.expanded));
    }

};
