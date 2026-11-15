//! Trusted GNOME Shell capture adapter. No recording or gesture policy here.
const std = @import("std");
const gio = @import("gio");
const glib = @import("glib");
const main = @import("main.zig");
const destination = "org.gnome.Shell";
const path = "/com/polarealm/PolarStation/Shortcuts";
const interface = "com.polarealm.PolarStation.Shortcuts1";
var connection: ?*gio.DBusConnection = null;
var subscription: c_uint = 0;
var properties_subscription: c_uint = 0;
var owner_subscription: c_uint = 0;
pub var selected = false;
pub var access_denied = false;
pub var needs_extension = false;
pub fn stop() void {
    if (connection) |bus| {
        bus.signalUnsubscribe(subscription);
        bus.signalUnsubscribe(properties_subscription);
        bus.signalUnsubscribe(owner_subscription);
        bus.unref();
    }
    if (selected) main.dictateShortcutLost();
    connection = null;
    selected = false;
    needs_extension = false;
}
fn version(bus: *gio.DBusConnection) bool {
    const args = glib.Variant.newTuple(&.{ glib.Variant.newString(interface), glib.Variant.newString("Version") }, 2);
    defer args.unref();
    const reply = bus.callSync(destination, path, "org.freedesktop.DBus.Properties", "Get", args, null, .{}, 1000, null, null) orelse return false;
    defer reply.unref();
    const boxed = reply.getChildValue(0); defer boxed.unref();
    const value = boxed.getVariant(); defer value.unref();
    return value.getUint32() == 1;
}
pub fn configure(bus: *gio.DBusConnection) bool {
    if (connection == null) {
        bus.ref(); connection = bus;
        subscription = bus.signalSubscribe(destination, interface, "Edge", path, null, .{}, event, null, null);
        owner_subscription = bus.signalSubscribe("org.freedesktop.DBus", "org.freedesktop.DBus", "NameOwnerChanged", "/org/freedesktop/DBus", destination, .{}, ownerChanged, null, null);
        properties_subscription = bus.signalSubscribe(destination, "org.freedesktop.DBus.Properties", "PropertiesChanged", path, interface, .{}, changed, null, null);
    }
    selected = version(bus);
    // Installed GNOME schemas do not mean this is the active desktop.
    const desktop = std.process.getEnvVarOwned(std.heap.c_allocator, "XDG_CURRENT_DESKTOP") catch return true;
    defer std.heap.c_allocator.free(desktop);
    needs_extension = std.ascii.indexOfIgnoreCase(desktop, "gnome") != null and !selected;
    return true;
}
fn ownerChanged(_: *gio.DBusConnection, _: ?[*:0]const u8, _: [*:0]const u8, _: [*:0]const u8, _: [*:0]const u8, _: *glib.Variant, _: ?*anyopaque) callconv(.c) void {
    if (selected) {
        selected = false; needs_extension = true;
        main.dictateShortcutLost();
        main.dictateShortcutStatus("capture_unavailable");
    }
}
fn changed(_: *gio.DBusConnection, _: ?[*:0]const u8, _: [*:0]const u8, _: [*:0]const u8, _: [*:0]const u8, params: *glib.Variant, _: ?*anyopaque) callconv(.c) void {
    const values = params.getChildValue(1); defer values.unref();
    var value: u32 = 0;
    if (values.lookup("Version", "u", &value) == 0) return;
    const was_selected = selected;
    selected = value == 1;
    needs_extension = !selected;
    if (was_selected and !selected) main.dictateShortcutLost();
    main.dictateShortcutStatus(if (selected) "ready" else "capture_unavailable");
}
fn event(_: *gio.DBusConnection, _: ?[*:0]const u8, _: [*:0]const u8, _: [*:0]const u8, _: [*:0]const u8, params: *glib.Variant, _: ?*anyopaque) callconv(.c) void {
    if (!selected) return;
    const value = params.getChildValue(0); defer value.unref();
    switch (value.getUint32()) {
        1 => main.dictateShortcutEdge(true),
        2 => main.dictateShortcutEdge(false),
        4 => main.dictateShortcutLost(),
        else => {},
    }
}
