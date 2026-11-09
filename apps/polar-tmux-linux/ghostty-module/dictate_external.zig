//! Borrowed AT-SPI handles over the existing GIO transport. No text is read
//! until capture/binding; no clipboard, synthetic keys, polling or history.
const std = @import("std");
const gio = @import("gio");
const glib = @import("glib");
const a = std.heap.c_allocator;
const accessible = "org.a11y.atspi.Accessible";
const text_interface = "org.a11y.atspi.Text";
fn same(x: []const u8, y: []const u8) bool {
    return std.mem.eql(u8, x, y);
}
fn invoke(bus: *gio.DBusConnection, name: [*:0]const u8, path: [*:0]const u8, interface: [*:0]const u8, method: [*:0]const u8, args: ?*glib.Variant, signature: []const u8) ?*glib.Variant {
    const reply = bus.callSync(name, path, interface, method, args, null, .{}, 250, null, null) orelse return null;
    if (!same(std.mem.span(reply.getTypeString()), signature)) {
        reply.unref();
        return null;
    }
    return reply;
}

pub const Watcher = struct {
    bus: *gio.DBusConnection,
    subscription: c_uint = 0,
    mouse_subscription: c_uint = 0,
    last_click_us: i64 = 0,
    name: ?[:0]u8 = null,
    path: ?[:0]u8 = null,
    context: *anyopaque,
    changed: *const fn (*anyopaque) void,
    pub fn create(context: *anyopaque, changed: *const fn (*anyopaque) void) ?*Watcher {
        const session = gio.busGetSync(.session, null, null) orelse return null;
        defer session.unref();
        const result = invoke(session, "org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress", null, "(s)") orelse return null;
        defer result.unref();
        const address = result.getChildValue(0);
        defer address.unref();
        const bus = gio.DBusConnection.newForAddressSync(address.getString(null), .{ .authentication_client = true, .message_bus_connection = true }, null, null, null) orelse return null;
        const self = a.create(Watcher) catch {
            bus.unref();
            return null;
        };
        self.* = .{ .bus = bus, .context = context, .changed = changed };
        self.subscription = bus.signalSubscribe(null, "org.a11y.atspi.Event.Object", "StateChanged", null, "focused", .{}, focusChanged, self, null);
        const registered = invoke(bus, "org.a11y.atspi.Registry", "/org/a11y/atspi/registry", "org.a11y.atspi.Registry", "RegisterEvent", glib.Variant.newParsed("('object:state-changed:focused', @as [], '')"), "()") orelse {
            self.destroy();
            return null;
        };
        registered.unref();
        self.mouse_subscription = bus.signalSubscribe(null, "org.a11y.atspi.Event.Mouse", "Button", null, null, .{}, mouseButton, self, null);
        if (invoke(bus, "org.a11y.atspi.Registry", "/org/a11y/atspi/registry", "org.a11y.atspi.Registry", "RegisterEvent", glib.Variant.newParsed("('mouse:', @as [], '')"), "()")) |mouse| mouse.unref();
        return self;
    }
    pub fn destroy(self: *Watcher) void {
        self.bus.signalUnsubscribe(self.subscription);
        if (self.mouse_subscription != 0) self.bus.signalUnsubscribe(self.mouse_subscription);
        // A private connection owns the registration: disconnect removes it.
        _ = self.bus.closeSync(null, null);
        self.bus.unref();
        self.clear();
        a.destroy(self);
    }
    fn clear(self: *Watcher) void {
        if (self.name) |v| a.free(v);
        if (self.path) |v| a.free(v);
        self.name = null;
        self.path = null;
    }
    pub fn userActivated(self: *const Watcher) bool {
        const elapsed = glib.getMonotonicTime() - self.last_click_us;
        return self.last_click_us > 0 and elapsed >= 0 and elapsed <= 1_000_000;
    }
    fn mouseButton(_: *gio.DBusConnection, _: ?[*:0]const u8, _: [*:0]const u8, _: [*:0]const u8, _: [*:0]const u8, parameters: *glib.Variant, raw: ?*anyopaque) callconv(.c) void {
        const self: *Watcher = @ptrCast(@alignCast(raw orelse return));
        if (parameters.nChildren() < 1) return;
        const detail = parameters.getChildValue(0);
        defer detail.unref();
        if (!same(std.mem.span(detail.getTypeString()), "s")) return;
        const button = std.mem.span(detail.getString(null));
        if (!same(button, "1p") and !same(button, "b1p") and !same(button, "1r") and !same(button, "b1r")) return;
        self.last_click_us = glib.getMonotonicTime();
        self.changed(self.context);
    }
    fn focusChanged(_: *gio.DBusConnection, sender: ?[*:0]const u8, path: [*:0]const u8, _: [*:0]const u8, _: [*:0]const u8, parameters: *glib.Variant, raw: ?*anyopaque) callconv(.c) void {
        const self: *Watcher = @ptrCast(@alignCast(raw orelse return));
        const name = sender orelse return;
        if (parameters.nChildren() < 3) return;
        const focused = parameters.getChildValue(1);
        defer focused.unref();
        if (!same(std.mem.span(focused.getTypeString()), "i")) return;
        if (focused.getInt32() == 0) {
            if (self.name) |old| if (same(old, std.mem.span(name)) and same(self.path orelse "", std.mem.span(path))) {
                self.clear();
            };
            return;
        }
        self.clear();
        if (std.mem.len(name) > 128 or std.mem.len(path) > 1024) return;
        self.name = a.dupeZ(u8, std.mem.span(name)) catch return;
        self.path = a.dupeZ(u8, std.mem.span(path)) catch {
            self.clear();
            return;
        };
        self.changed(self.context);
    }
    pub fn capture(self: *Watcher) ?*Snapshot {
        return Snapshot.create(self.bus, self.name orelse return null, self.path orelse return null);
    }
};

pub const Snapshot = struct {
    pub const Delivery = enum { applied, unchanged, unknown };
    bus: *gio.DBusConnection,
    name: [:0]u8,
    path: [:0]u8,
    original: [:0]u8,
    caret: i32,
    offset: usize,
    pub fn destroy(self: *Snapshot) void {
        self.bus.unref();
        a.free(self.name);
        a.free(self.path);
        @memset(self.original, 0);
        a.free(self.original);
        a.destroy(self);
    }
    fn call(self: *const Snapshot, interface: [*:0]const u8, method: [*:0]const u8, args: ?*glib.Variant, signature: []const u8) ?*glib.Variant {
        return invoke(self.bus, self.name, self.path, interface, method, args, signature);
    }
    fn property(self: *const Snapshot, name: [*:0]const u8) ?i32 {
        const reply = self.call("org.freedesktop.DBus.Properties", "Get", glib.Variant.newParsed("(%s, %s)", @as([*:0]const u8, text_interface), name), "(v)") orelse return null;
        defer reply.unref();
        const wrapped = reply.getChildValue(0);
        defer wrapped.unref();
        const value = wrapped.getVariant();
        defer value.unref();
        if (!same(std.mem.span(value.getTypeString()), "i")) return null;
        return value.getInt32();
    }
    fn foreground(self:*const Snapshot) bool {
        var name=a.dupeZ(u8,self.name) catch return false;defer a.free(name);
        var path=a.dupeZ(u8,self.path) catch return false;defer a.free(path);
        for(0..32) |_| {
            const role=invoke(self.bus,name,path,accessible,"GetRoleName",null,"(s)") orelse return false;defer role.unref();
            const child=role.getChildValue(0);defer child.unref();const kind=std.mem.span(child.getString(null));
            if(same(kind,"frame") or same(kind,"dialog") or same(kind,"window")) {
                const state=invoke(self.bus,name,path,accessible,"GetState",null,"(au)") orelse return false;defer state.unref();
                const bits=state.getChildValue(0);defer bits.unref();if(bits.nChildren()==0)return false;
                const first=bits.getChildValue(0);defer first.unref();return first.getUint32() & (1<<1) != 0;
            }
            const parent=invoke(self.bus,name,path,"org.freedesktop.DBus.Properties","Get",glib.Variant.newParsed("(%s, 'Parent')",@as([*:0]const u8,accessible)),"(v)") orelse return false;defer parent.unref();
            const wrapped=parent.getChildValue(0);defer wrapped.unref();const pair=wrapped.getVariant();defer pair.unref();
            if(!same(std.mem.span(pair.getTypeString()),"(so)"))return false;
            const n=pair.getChildValue(0);defer n.unref();const p=pair.getChildValue(1);defer p.unref();
            const next_name=a.dupeZ(u8,std.mem.span(n.getString(null))) catch return false;
            const next_path=a.dupeZ(u8,std.mem.span(p.getString(null))) catch {a.free(next_name);return false;};
            a.free(name);a.free(path);name=next_name;path=next_path;
            if(name.len==0 or same(path,"/org/a11y/atspi/null"))return false;
        }
        return false;
    }
    fn editable(self: *const Snapshot) bool {
        // Read role BEFORE any text: password controls never supply context.
        const role = self.call(accessible, "GetRoleName", null, "(s)") orelse return false;
        defer role.unref();
        const role_name = role.getChildValue(0);
        defer role_name.unref();
        const value = std.mem.span(role_name.getString(null));
        if (!same(value, "text") and !same(value, "entry")) return false;
        const state = self.call(accessible, "GetState", null, "(au)") orelse return false;
        defer state.unref();
        const bits = state.getChildValue(0);
        defer bits.unref();
        if (bits.nChildren() == 0) return false;
        const first = bits.getChildValue(0);
        defer first.unref();
        // GTK reports Sensitive for enabled controls, but omits Enabled.
        // AT-SPI stable enum: editable=7, focused=12, sensitive=24.
        const required: u32 = (1 << 7) | (1 << 12) | (1 << 24) | (1 << 25) | (1 << 30);
        if (first.getUint32() & required != required or !self.foreground()) return false;
        const selections = self.call(text_interface, "GetNSelections", null, "(i)") orelse return false;
        defer selections.unref();
        const count = selections.getChildValue(0);
        defer count.unref();
        // One native insertion gives one undo. Never delete a selection first.
        return count.getInt32() == 0;
    }
    fn read(self: *const Snapshot) ?[:0]u8 {
        const count = self.property("CharacterCount") orelse return null;
        if (count < 0 or count > 16384) return null;
        const reply = self.call(text_interface, "GetText", glib.Variant.newParsed("(0, %i)", count), "(s)") orelse return null;
        defer reply.unref();
        const value = reply.getChildValue(0);
        defer value.unref();
        const text = std.mem.span(value.getString(null));
        if (text.len > 65536 or !std.unicode.utf8ValidateSlice(text)) return null;
        return a.dupeZ(u8, text) catch null;
    }
    fn create(bus: *gio.DBusConnection, name: [:0]const u8, path: [:0]const u8) ?*Snapshot {
        const self = a.create(Snapshot) catch return null;
        const owned_name = a.dupeZ(u8, name) catch {
            a.destroy(self);
            return null;
        };
        _ = bus.ref();
        self.* = .{ .bus = bus, .name = owned_name, .path = undefined, .original = undefined, .caret = 0, .offset = 0 };
        self.path = a.dupeZ(u8, path) catch {
            bus.unref();
            a.free(self.name);
            a.destroy(self);
            return null;
        };
        self.original = a.dupeZ(u8, "") catch {
            bus.unref();
            a.free(self.name);
            a.free(self.path);
            a.destroy(self);
            return null;
        };
        if (!self.editable()) {
            self.destroy();
            return null;
        }
        const original = self.read() orelse {
            self.destroy();
            return null;
        };
        a.free(self.original);
        self.original = original;
        const caret = self.property("CaretOffset") orelse {
            self.destroy();
            return null;
        };
        if (caret < 0) {
            self.destroy();
            return null;
        }
        var it = (std.unicode.Utf8View.init(original) catch {
            self.destroy();
            return null;
        }).iterator();
        var count: i32 = 0;
        while (count < caret) : (count += 1) {
            _ = it.nextCodepoint() orelse {
                self.destroy();
                return null;
            };
        }
        self.caret = caret;
        self.offset = it.i;
        return self;
    }
    pub fn context(self: *const Snapshot) []const u8 {
        var start = self.offset -| 2048;
        var end = @min(self.original.len, self.offset + 2048);
        while (start < self.offset and !std.unicode.utf8ValidateSlice(self.original[start..self.offset])) start += 1;
        while (end > self.offset and !std.unicode.utf8ValidateSlice(self.original[self.offset..end])) end -= 1;
        return self.original[start..end];
    }
    pub fn insert(self: *Snapshot, text: []const u8) Delivery {
        if (!self.editable() or (self.property("CaretOffset") orelse return .unchanged) != self.caret) return .unchanged;
        const before = self.read() orelse return .unchanged;
        defer a.free(before);
        if (!same(before, self.original)) return .unchanged;
        const expected = std.fmt.allocPrint(a, "{s}{s}{s}", .{ self.original[0..self.offset], text, self.original[self.offset..] }) catch return .unchanged;
        defer a.free(expected);
        const content = a.dupeZ(u8, text) catch return .unchanged;
        defer a.free(content);
        const reply = self.call("org.a11y.atspi.EditableText", "InsertText", glib.Variant.newParsed("(%i, %s, %i)", self.caret, content.ptr, @as(i32, @intCast(text.len))), "(b)") orelse return .unknown;
        defer reply.unref();
        const after = self.read() orelse return .unknown;
        defer a.free(after);
        return if (same(after, expected)) .applied else .unknown;
    }
};
