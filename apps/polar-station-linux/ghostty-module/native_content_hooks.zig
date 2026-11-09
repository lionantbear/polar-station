//! Main-thread notifications; the native catalog owns surfaces, Rust owns LGP.
const Window = @import("../class/window.zig").Window;
const DisplaySurface = @import("display_surface.zig").DisplaySurface;
const Content = @import("station_content.zig").Content;
pub const Hooks = struct {
    context: ?*anyopaque = null,
    on_created: ?*const fn (?*anyopaque, []const u8, []const u8, *Window, Content, *DisplaySurface) void = null,
    on_retired: ?*const fn (?*anyopaque, []const u8, *DisplaySurface) void = null,
    on_activated: ?*const fn (?*anyopaque, []const u8) void = null,
    on_selected: ?*const fn (?*anyopaque, []const u8) void = null,
    on_deselected: ?*const fn (?*anyopaque, *Window, []const u8, []const u8, []const u8) void = null,
    pub fn deselected(self: Hooks, window: *Window, pane: []const u8, target: []const u8, session: []const u8) void {
        if (self.on_deselected) |callback| callback(self.context, window, pane, target, session);
    }
    pub fn created(self: Hooks, pane: []const u8, window: []const u8, host: *Window, content: Content, display: *DisplaySurface) void {
        if (self.on_created) |callback| callback(self.context, pane, window, host, content, display);
    }
    pub fn retired(self: Hooks, pane: []const u8, display: *DisplaySurface) void {
        if (self.on_retired) |callback| callback(self.context, pane, display);
    }
    pub fn activated(self: Hooks, pane: []const u8) void {
        if (self.on_activated) |callback| callback(self.context,pane);
    }
    pub fn selected(self: Hooks, pane: []const u8) void {
        if (self.on_selected) |callback| callback(self.context, pane);
    }
};
