//! A native GTK content surface backed by real processless Ghostty Termio.
//! The owner mounts `surface` in the existing layout and waits for its init
//! signal before bind/submit. No placeholder PTY or hidden shell is created.
const std = @import("std");
const gtk = @import("gtk");
const gobject = @import("gobject");
const Surface = @import("../class/surface.zig").Surface;
const c = @cImport({ @cInclude("station_display.h"); });

pub const DisplaySurface = struct {
    allocator: std.mem.Allocator,
    owner_thread: std.Thread.Id,
    surface: *Surface,
    generation: u64,
    handle: ?*c.ps_display = null,
    graphics_widget: ?*gtk.Widget = null,
    graphics_overlay: ?*gtk.Overlay = null,

    pub fn create(allocator: std.mem.Allocator, generation: u64) !*DisplaySurface {
        if (generation == 0) return error.InvalidGeneration;
        const self = try allocator.create(DisplaySurface);
        errdefer allocator.destroy(self);
        const surface = Surface.new(.{ .station_display_only = true });
        // Own one non-floating reference; the native layout owns its own.
        _ = surface.as(gobject.Object).refSink();
        errdefer surface.unref();
        // A new processless surface is already blank. Keep it renderable:
        // GLArea emits its initial resize during rendering, and Ghostty
        // initializes core() there. Zero opacity would prevent that first
        // render and leave the content owner waiting for init forever.
        self.* = .{
            .allocator = allocator,
            .owner_thread = std.Thread.getCurrentId(),
            .surface = surface,
            .generation = generation,
        };
        const config: c.ps_display_config = .{
            .abi_version = c.PS_DISPLAY_ABI_VERSION,
            .surface_generation = generation,
            .max_bytes = c.PS_DISPLAY_MAX_BYTES,
            .sink = consume,
            .context = self,
        };
        if (c.ps_display_create(&config, &self.handle) != c.PS_DISPLAY_OK)
            return error.DisplayAdmissionInitialization;
        return self;
    }

    pub fn createGraphics(allocator: std.mem.Allocator, generation: u64, view_id: []const u8) !*DisplaySurface {
        const self = try allocator.create(DisplaySurface);
        errdefer allocator.destroy(self);
        const widget = try @import("graphics_surface.zig").create(allocator, view_id);
        const overlay = gtk.Overlay.new();
        overlay.setChild(widget);
        overlay.as(gtk.Widget).setOverflow(.hidden);
        const surface = Surface.new(.{ .title = "Idea Tree" });
        surface.stationSetExternalWidget(overlay.as(gtk.Widget));
        _ = surface.as(gobject.Object).refSink();
        self.* = .{ .allocator = allocator, .owner_thread = std.Thread.getCurrentId(), .surface = surface, .generation = generation, .graphics_widget = widget, .graphics_overlay = overlay };
        return self;
    }

    /// Bind after realization, once per physical Surface. Changing identity
    /// requires a new Surface: parser admission is not a GPU-present fence.
    /// Empty replacement is a real accepted revision, reported to the caller.
    pub fn bind(self: *DisplaySurface, content_epoch: u64) !c.ps_display_snapshot {
        try self.checkOwner();
        if (self.surface.core() == null) return error.SurfaceNotReady;
        const handle = self.handle orelse return error.Retired;
        const before = try self.snapshot();
        if (before.content_epoch != 0) return error.ContentIdentityRequiresNewSurface;
        if (c.ps_display_bind(handle, content_epoch) != c.PS_DISPLAY_OK)
            return error.ContentBindingRejected;
        self.surface.as(gtk.Widget).setOpacity(0);
        const status = self.submit(content_epoch, 0, 1, c.PS_DISPLAY_REPLACE_WINDOW, "");
        if (status != c.PS_DISPLAY_OK) return error.DisplayClearRejected;
        self.surface.as(gtk.Widget).setOpacity(1);
        return self.snapshot();
    }

    pub fn submit(self: *DisplaySurface, epoch: u64, base: u64, next: u64,
                  operation: u32, text: []const u8) c.ps_display_status {
        self.checkOwner() catch return c.PS_DISPLAY_WRONG_THREAD;
        const handle = self.handle orelse return c.PS_DISPLAY_RETIRED;
        const batch: c.ps_display_batch = .{
            .surface_generation = self.generation,
            .content_epoch = epoch,
            .base_revision = base,
            .next_revision = next,
            .operation = operation,
            .utf8 = text.ptr,
            .byte_count = text.len,
        };
        return c.ps_display_submit(handle, &batch);
    }

    pub fn snapshot(self: *DisplaySurface) !c.ps_display_snapshot {
        try self.checkOwner();
        var result: c.ps_display_snapshot = undefined;
        if (c.ps_display_read(self.handle orelse return error.Retired, &result) != c.PS_DISPLAY_OK)
            return error.DisplaySnapshotRejected;
        return result;
    }

    /// Retire when the pane is removed, even if another object temporarily
    /// retains this controller. No callback is registered with GTK by this type.
    pub fn retire(self: *DisplaySurface) !void {
        try self.checkOwner();
        self.surface.as(gtk.Widget).setVisible(0);
        if (self.graphics_widget) |widget| {
            @import("graphics_surface.zig").retire(widget);
            self.graphics_widget = null;
        }
        self.surface.stationRetireExternalWidget();
        self.graphics_overlay = null;
        if (self.handle) |handle| {
            if (c.ps_display_destroy(handle) != c.PS_DISPLAY_OK) return error.DisplayBusy;
            self.handle = null;
        }
    }

    pub fn destroy(self: *DisplaySurface) !void {
        try self.retire();
        self.surface.unref();
        const allocator = self.allocator;
        allocator.destroy(self);
    }

    fn checkOwner(self: *const DisplaySurface) !void {
        if (self.owner_thread != std.Thread.getCurrentId()) return error.WrongThread;
    }

    fn consume(context: ?*anyopaque, operation: u32,
               text: [*c]const u8, length: usize) callconv(.c) c_int {
        const self: *DisplaySurface = @ptrCast(@alignCast(context orelse return 1));
        self.checkOwner() catch return 1;
        // Null with zero bytes is the valid empty window reset.
        const bytes: []const u8 = if (length == 0) "" else text[0..length];
        self.surface.stationDisplayText(operation, bytes) catch return 1;
        return 0;
    }
};
