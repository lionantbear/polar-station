//! Auxiliary tools are owned widgets, not independent application lifetimes.
const gtk = @import("gtk");
const adw = @import("adw");
const glib = @import("glib");
const gobject = @import("gobject");
const Application = @import("../class/application.zig").Application;
const NativeWindow = @import("../class/window.zig").Window;

pub fn create(_: *Application) *adw.Window {
    const window = adw.Window.new();
    _ = window.as(gobject.Object).refSink();
    window.as(gobject.Object).setData("polar-station-auxiliary", window);
    window.as(gtk.Window).setHideOnClose(1);
    return window;
}

pub fn present(app: *Application, window: *adw.Window) void {
    var windows = @as(?*glib.List, app.as(gtk.Application).getWindows());
    while (windows) |node| : (windows = node.f_next) {
        const parent: *gtk.Window = @ptrCast(@alignCast(node.f_data orelse continue));
        if (gobject.ext.cast(NativeWindow, parent.as(gobject.Object)) == null) continue;
        window.as(gtk.Window).setTransientFor(parent);
        break;
    }
    window.as(gtk.Window).present();
}

pub fn parentRemoved(parent: *gtk.Window) void {
    const windows = gtk.Window.listToplevels();
    defer windows.free();
    var current: ?*glib.List = windows;
    while (current) |node| : (current = node.f_next) {
        const window: *gtk.Window = @ptrCast(@alignCast(node.f_data orelse continue));
        if (window.as(gobject.Object).getData("polar-station-auxiliary") == null) continue;
        if (window.getTransientFor() != parent) continue;
        window.as(gtk.Widget).setVisible(0);
        window.setTransientFor(null);
    }
}

pub fn destroy(window: *adw.Window) void {
    window.as(gtk.Window).destroy();
    window.as(gobject.Object).unref();
}

/// Move the existing editor widget into a diagnostic row, preserving its draft.
/// The caller removes it from the row before restoring this lease.
pub const ContentLease = struct {
    window: ?*adw.Window = null,
    widget: *gtk.Widget,
    /// A process-owned shared draft survives removal from this native container.
    close_before_navigation: bool = true,
    close_policy: ?EmbeddedHost = null,
    pub fn take(window: *adw.Window) ?ContentLease {
        const widget = window.getContent() orelse return null;
        _ = widget.as(gobject.Object).ref();
        window.as(gtk.Widget).setVisible(0);
        window.setContent(null);
        return .{ .window = window, .widget = widget };
    }
    /// The content owner keeps its original reference; this lease owns only its attachment.
    pub fn borrow(widget:*gtk.Widget) ContentLease {
        _=widget.as(gobject.Object).ref();
        return .{.widget=widget,.close_before_navigation=false};
    }
    pub fn bind(self: ContentLease, host: *EmbeddedHost) void {
        if(self.window) |window| {
            window.as(gobject.Object).setData("polar-station-editor-widget", self.widget);
            window.as(gobject.Object).setData("polar-station-editor-host", host);
        }
        self.widget.as(gobject.Object).setData("polar-station-editor-host",host);
    }
    pub fn restore(self: ContentLease) void {
        self.widget.as(gobject.Object).setData("polar-station-editor-host",null);
        std.debug.assert(self.widget.getParent() == null);
        if(self.window) |window| {
            window.as(gobject.Object).setData("polar-station-editor-widget", null);
            window.as(gobject.Object).setData("polar-station-editor-host", null);
            window.setContent(self.widget);
        }
        self.widget.as(gobject.Object).unref();
    }
};
const std = @import("std");

/// Only native parenting and closing; the original editor still decides whether
/// closing is allowed and performs its existing unsaved-draft confirmation.
pub const EmbeddedHost = struct {
    context: ?*anyopaque = null,
    close: *const fn (?*anyopaque) void,
};
pub fn editorParent(window: *adw.Window) *gtk.Window {
    if (window.as(gobject.Object).getData("polar-station-editor-widget")) |raw| {
        const child: *gtk.Widget = @ptrCast(@alignCast(raw));
        if (child.getRoot()) |root| if (gobject.ext.cast(gtk.Window, root.as(gobject.Object))) |parent| return parent;
    }
    return window.as(gtk.Window);
}
pub fn closeEditor(window: *adw.Window) void {
    if (window.as(gobject.Object).getData("polar-station-editor-host")) |raw| {
        const host: *EmbeddedHost = @ptrCast(@alignCast(raw));
        host.close(host.context);
    } else window.as(gtk.Widget).setVisible(0);
}

/// The live native parent of borrowed configuration content; no hidden shell Window.
pub fn contentParent(app:*Application,widget:*gtk.Widget) ?*gtk.Window {
    if(widget.getRoot()) |root| if(gobject.ext.cast(gtk.Window,root.as(gobject.Object))) |window| return window;
    return app.as(gtk.Application).getActiveWindow();
}
pub fn closeContent(widget:*gtk.Widget) void {
    if(widget.as(gobject.Object).getData("polar-station-editor-host")) |raw| {
        const host:*EmbeddedHost=@ptrCast(@alignCast(raw));host.close(host.context);
    }
}
