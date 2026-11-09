//! Native theme application. Rust resolves product palettes; Ghostty parses terminal colors.
const std = @import("std");
const gdk = @import("gdk");
const gtk = @import("gtk");
const adw = @import("adw");
const gio = @import("gio");
const glib = @import("glib");
const gobject = @import("gobject");
const settings = @import("settings.zig");
const Application = @import("../class/application.zig").Application;
const internal_os = @import("../../../os/main.zig");

pub fn themesDirectory(allocator: std.mem.Allocator) ![]u8 {
    const exe = try std.fs.selfExePathAlloc(allocator);
    defer allocator.free(exe);
    // The build artifact carries upstream resources; installed Station has its own namespace.
    for ([_][]const u8{ "../share/polar-station/themes", "../share/ghostty/themes" }) |suffix| {
        const path = try std.fs.path.resolve(allocator, &.{ std.fs.path.dirname(exe).?, suffix });
        if (std.fs.openDirAbsolute(path, .{})) |opened| {
            var dir = opened;
            dir.close();
            return path;
        } else |_| allocator.free(path);
    }
    const data = try std.fs.path.join(allocator, &.{ std.mem.span(glib.getUserDataDir()), "polar-station/themes" });
    errdefer allocator.free(data);
    var installed = try std.fs.openDirAbsolute(data, .{});
    installed.close();
    return data;
}
pub fn themeNames(allocator: std.mem.Allocator) !std.ArrayListUnmanaged([:0]u8) {
    const path = try themesDirectory(allocator);
    defer allocator.free(path);
    var dir = try std.fs.openDirAbsolute(path, .{ .iterate = true });
    defer dir.close();
    var result: std.ArrayListUnmanaged([:0]u8) = .empty;
    errdefer {
        for (result.items) |name| allocator.free(name);
        result.deinit(allocator);
    }
    var iter = dir.iterate();
    while (try iter.next()) |entry| if (entry.kind == .file and !std.mem.startsWith(u8, entry.name, ".")) {
        try result.append(allocator, try allocator.dupeZ(u8, entry.name));
    };
    std.mem.sort([:0]u8, result.items, {}, struct {
        fn less(_: void, a: [:0]u8, b: [:0]u8) bool {
            return std.ascii.lessThanIgnoreCase(a, b);
        }
    }.less);
    return result;
}
pub const Controller = struct {
    allocator: std.mem.Allocator,
    app: *Application,
    value: ?std.json.Parsed(settings.Projection) = null,
    css: ?*gtk.CssProvider = null,
    signal: ?c_ulong = null,
    terminal_key: ?[]u8 = null,
    config_path: ?[]u8 = null,
    applying: bool = false,
    pub fn init(allocator: std.mem.Allocator, app: *Application) Controller {
        return .{ .allocator = allocator, .app = app };
    }
    pub fn deinit(self: *Controller) void {
        if (self.signal) |id| gobject.signalHandlerDisconnect(self.app.as(adw.Application).getStyleManager().as(gobject.Object), id);
        if (self.value) |*value| value.deinit();
        if (self.css) |css| {
            gtk.StyleContext.removeProviderForDisplay(gdk.Display.getDefault().?, css.as(gtk.StyleProvider));
            css.unref();
        }
        if (self.terminal_key) |key| self.allocator.free(key);
        if (self.config_path) |path| {
            std.fs.deleteFileAbsolute(path) catch {};
            self.allocator.free(path);
        }
    }
    pub fn update(self: *Controller, projection: *const settings.Projection) !void {
        if (projection.theme_projection == null) return;
        const wire = try std.json.Stringify.valueAlloc(self.allocator, projection.*, .{});
        defer self.allocator.free(wire);
        const parsed = try std.json.parseFromSlice(settings.Projection, self.allocator, wire, .{ .allocate = .alloc_always });
        if (self.value) |*old| old.deinit();
        self.value = parsed;
        if (self.signal == null) self.signal = gobject.Object.signals.notify.connect(self.app.as(adw.Application).getStyleManager(), *Controller, appearanceChanged, self, .{ .detail = "dark" });
        try self.apply();
    }
    fn appearanceChanged(_: *adw.StyleManager, _: *gobject.ParamSpec, self: *Controller) callconv(.c) void {
        self.apply() catch |err| std.log.err("Station theme: {}", .{err});
    }
    fn apply(self: *Controller) !void {
        if (self.applying) return;
        self.applying = true;
        defer self.applying = false;
        const projection = if (self.value) |*v| &v.value else return;
        const themes = projection.theme_projection orelse return;
        const style = self.app.as(adw.Application).getStyleManager();
        const system = std.mem.eql(u8, projection.appearance.theme.mode, "system");
        style.setColorScheme(if (system) .default else if (themes.dark.dark) .force_dark else .force_light);
        const selected = if (style.getDark() != 0) themes.dark else themes.light;
        const name = projection.appearance.theme.terminal_theme orelse selected.name;
        const key = try std.fmt.allocPrint(self.allocator, "{s}:{s}:{s}", .{ name, selected.id, if (system) "system" else "preset" });
        defer self.allocator.free(key);
        if (self.terminal_key == null or !std.mem.eql(u8, self.terminal_key.?, key)) {
            const root = try themesDirectory(self.allocator);
            defer self.allocator.free(root);
            const path = try std.fs.path.join(self.allocator, &.{ root, name });
            defer self.allocator.free(path);
            const file = try std.fs.openFileAbsolute(path, .{});
            defer file.close();
            const data = try file.readToEndAlloc(self.allocator, 128 * 1024);
            defer self.allocator.free(data);
            if (self.config_path == null) self.config_path = try std.fmt.allocPrint(self.allocator, "{s}/polar-station-theme-{d}.conf", .{ std.mem.span(glib.getUserRuntimeDir()), std.os.linux.getpid() });
            const config = try std.fmt.allocPrint(self.allocator, "theme =\n{s}\nminimum-contrast = 4.5\nwindow-theme = {s}\n", .{ data, if (system) "system" else if (selected.dark) "dark" else "light" });
            defer self.allocator.free(config);
            const out = try std.fs.createFileAbsolute(self.config_path.?, .{ .mode = 0o600 });
            defer out.close();
            try out.writeAll(config);
            const path_z = try self.allocator.dupeZ(u8, self.config_path.?);
            defer self.allocator.free(path_z);
            _ = internal_os.setenv("POLAR_STATION_THEME_CONFIG", path_z);
            if (self.terminal_key) |old| self.allocator.free(old);
            self.terminal_key = try self.allocator.dupe(u8, key);
            self.app.as(gio.ActionGroup).activateAction("reload-config", null);
        }
        if (self.css == null) {
            const css = gtk.CssProvider.new();
            self.css = css;
            gtk.StyleContext.addProviderForDisplay(gdk.Display.getDefault().?, css.as(gtk.StyleProvider), gtk.STYLE_PROVIDER_PRIORITY_APPLICATION + 10);
        }
        const css_text = try std.fmt.allocPrintSentinel(self.allocator, "@define-color window_bg_color {s}; @define-color window_fg_color {s}; " ++
            "@define-color view_bg_color {s}; @define-color view_fg_color {s}; " ++
            "@define-color headerbar_bg_color {s}; @define-color headerbar_fg_color {s}; " ++
            "@define-color card_bg_color {s}; @define-color card_fg_color {s}; " ++
            "@define-color dialog_bg_color {s}; @define-color dialog_fg_color {s}; " ++
            "@define-color popover_bg_color {s}; @define-color popover_fg_color {s}; " ++
            "@define-color sidebar_bg_color {s}; @define-color sidebar_fg_color {s}; " ++
            "@define-color accent_color {s}; @define-color accent_bg_color {s}; @define-color accent_fg_color {s}; " ++
            ".polar-station-controller-root flowboxchild:selected {{ color: @window_fg_color; background-color: alpha(@accent_bg_color, 0.15); }} " ++
            "textview text selection, entry selection {{ background-color: alpha(@accent_bg_color, 0.20); color: @window_fg_color; }} " ++
            "row:selected {{ background-color: alpha(@accent_bg_color, 0.15); color: @window_fg_color; }} {s}", .{ selected.background, selected.foreground, selected.background, selected.foreground, selected.surface, selected.foreground, selected.surface, selected.foreground, selected.background, selected.foreground, selected.surface, selected.foreground, selected.surface, selected.foreground, selected.accent, selected.accent, if (selected.dark) selected.background else "#ffffff", if (selected.dark) "" else ".dim-label:not(:disabled) { opacity: 0.85; } entry:not(:disabled), spinbutton:not(:disabled), dropdown > button:not(:disabled) { background-color: @card_bg_color; color: @window_fg_color; box-shadow: inset 0 0 0 1px alpha(@window_fg_color, 0.45); }" }, 0);
        defer self.allocator.free(css_text);
        self.css.?.loadFromString(css_text);
    }
};
