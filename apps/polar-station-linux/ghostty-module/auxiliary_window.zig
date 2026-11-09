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
