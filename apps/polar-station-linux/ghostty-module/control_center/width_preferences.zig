//! Native Linux persistence for the Control Center Layout Map width.
//!
//! The file is deliberately a tiny XDG state JSON document. Workspace facts,
//! Pane bindings, and physical Ghostty geometry never enter the Rust IPC
//! payload; only this GTK-native preference owns the user's width choice.

const std = @import("std");
const layout_policy = @import("layout_policy.zig");

pub const Preferences = struct {
    allocator: std.mem.Allocator,
    path: []u8,

    pub fn init(allocator: std.mem.Allocator) !Preferences {
        return .{
            .allocator = allocator,
            .path = try statePath(allocator),
        };
    }

    pub fn deinit(self: *Preferences) void {
        self.allocator.free(self.path);
        self.* = undefined;
    }

    pub fn loadOrDefault(
        self: *const Preferences,
        visible_width: c_int,
    ) c_int {
        const fallback = layout_policy.adaptiveMapWidth(visible_width);
        var file = std.fs.openFileAbsolute(self.path, .{}) catch return fallback;
        defer file.close();
        const content = file.readToEndAlloc(self.allocator, 4096) catch return fallback;
        defer self.allocator.free(content);
        const stored = parseMapWidth(content) orelse return fallback;
        return layout_policy.clampMapWidth(stored);
    }

    /// Persist only after a completed drag. The temporary file keeps a
    /// compositor crash from leaving a partial JSON document behind.
    pub fn save(self: *const Preferences, map_width: c_int) !void {
        const parent = std.fs.path.dirname(self.path) orelse return error.InvalidPath;
        std.fs.cwd().makePath(parent) catch |err| switch (err) {
            error.PathAlreadyExists => {},
            else => return err,
        };

        const temporary_path = try std.fmt.allocPrint(
            self.allocator,
            "{s}.tmp",
            .{self.path},
        );
        defer self.allocator.free(temporary_path);

        var file = try std.fs.createFileAbsolute(temporary_path, .{
            .truncate = true,
            .mode = 0o600,
        });
        errdefer std.fs.deleteFileAbsolute(temporary_path) catch {};
        defer file.close();
        const document = try self.mergedValues(&.{.{ .key = "map_width", .value = layout_policy.clampMapWidth(map_width) }});
        defer self.allocator.free(document);
        try file.writeAll(document);
        try file.sync();
        try std.fs.renameAbsolute(temporary_path, self.path);
    }
    const Value = struct { key: []const u8, value: c_int };
    fn mergedValues(self: *const Preferences, values: []const Value) ![]u8 {
        const old = std.fs.cwd().readFileAlloc(self.allocator, self.path, 4096) catch try self.allocator.dupe(u8, "{}");
        defer self.allocator.free(old);
        var parsed = std.json.parseFromSlice(std.json.Value, self.allocator, old, .{ .allocate = .alloc_always }) catch try std.json.parseFromSlice(std.json.Value, self.allocator, "{}", .{});
        defer parsed.deinit();
        if (parsed.value != .object) return error.InvalidPreferences;
        for (values) |value| try parsed.value.object.put(value.key, .{ .integer = value.value });
        return std.json.Stringify.valueAlloc(self.allocator, parsed.value, .{});
    }
    pub fn loadEditorSize(self: *const Preferences, _: bool) ?[2]c_int {
        const bytes = std.fs.cwd().readFileAlloc(self.allocator, self.path, 4096) catch return null;
        defer self.allocator.free(bytes);
        const parsed = std.json.parseFromSlice(std.json.Value, self.allocator, bytes, .{}) catch return null;
        defer parsed.deinit();
        if (parsed.value != .object) return null;
        const width = parsed.value.object.get("editor_golden_v3_width") orelse return null;
        const height = parsed.value.object.get("editor_golden_v3_height") orelse return null;
        if (width != .integer or height != .integer or width.integer < 100 or height.integer < 100 or width.integer > 16384 or height.integer > 16384) return null;
        return .{ @intCast(width.integer), @intCast(height.integer) };
    }
    pub fn editorMaximized(self: *const Preferences) bool {
        const bytes = std.fs.cwd().readFileAlloc(self.allocator, self.path, 4096) catch return false;
        defer self.allocator.free(bytes);
        const parsed = std.json.parseFromSlice(std.json.Value, self.allocator, bytes, .{}) catch return false;
        defer parsed.deinit();
        if (parsed.value != .object) return false;
        const value = parsed.value.object.get("editor_golden_v3_maximized") orelse return false;
        return value == .integer and value.integer == 1;
    }
    pub fn saveEditorMaximized(self: *const Preferences, maximized: bool) !void {
        const document = try self.mergedValues(&.{.{ .key = "editor_golden_v3_maximized", .value = @intFromBool(maximized) }});
        defer self.allocator.free(document);
        try self.writeEditorPreferences(document);
    }
    pub fn saveEditorSize(self: *const Preferences, _: bool, size: [2]c_int) !void {
        const document = try self.mergedValues(&.{
            .{ .key = "editor_golden_v3_width", .value = size[0] },
            .{ .key = "editor_golden_v3_height", .value = size[1] },
        });
        defer self.allocator.free(document);
        try self.writeEditorPreferences(document);
    }
    fn writeEditorPreferences(self: *const Preferences, document: []const u8) !void {
        const parent = std.fs.path.dirname(self.path) orelse return error.InvalidPath;
        try std.fs.cwd().makePath(parent);
        const temporary = try std.fmt.allocPrint(self.allocator, "{s}.tmp", .{self.path});
        defer self.allocator.free(temporary);
        var file = try std.fs.createFileAbsolute(temporary, .{ .mode = 0o600 });
        defer file.close();
        try file.writeAll(document);
        try file.sync();
        try std.fs.renameAbsolute(temporary, self.path);
    }
};

fn statePath(allocator: std.mem.Allocator) ![]u8 {
    const base = std.process.getEnvVarOwned(allocator, "XDG_STATE_HOME") catch |err| switch (err) {
        error.EnvironmentVariableNotFound => blk: {
            const home = try std.process.getEnvVarOwned(allocator, "HOME");
            defer allocator.free(home);
            break :blk try std.fs.path.join(allocator, &.{ home, ".local", "state" });
        },
        else => return err,
    };
    defer allocator.free(base);
    return std.fs.path.join(
        allocator,
        &.{ base, "polar-station", "preferences.json" },
    );
}

fn parseMapWidth(content: []const u8) ?c_int {
    const marker = "\"map_width\"";
    const marker_start = std.mem.indexOf(u8, content, marker) orelse return null;
    var cursor = marker_start + marker.len;
    while (cursor < content.len and
        (content[cursor] == ':' or std.ascii.isWhitespace(content[cursor])))
    {
        cursor += 1;
    }
    const number_start = cursor;
    while (cursor < content.len and std.ascii.isDigit(content[cursor])) {
        cursor += 1;
    }
    if (cursor == number_start) return null;
    return std.fmt.parseInt(c_int, content[number_start..cursor], 10) catch null;
}








