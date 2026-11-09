//! Polar Station's processless text-to-terminal adapter.
//! No PTY, exec, messages, input editor, font or independent renderer lives here.
//! Only trusted operation bytes are generated; message bytes remain plain text.
const std = @import("std");

pub const max_text_bytes: usize = 64 * 1024;
pub const Operation = enum(u32) { replace_window = 1, append_text = 2, replace_runs = 3, append_runs = 4 };
pub const State = struct {
    owner: std.Thread.Id,
    pub fn init() State { return .{ .owner = std.Thread.getCurrentId() }; }
    pub fn checkOwner(self: State) !void {
        if (self.owner != std.Thread.getCurrentId()) return error.WrongThread;
    }
};

// Clear visible cells AND scrollback before displaying another bounded window.
// Disable the terminal input cursor: editing belongs to the native composer.
const reset_prefix = "\x1b[0m\x1b[?25l\x1b[?12l\x1b[2J\x1b[3J\x1b[H";

pub fn encode(alloc: std.mem.Allocator, operation: Operation, text: []const u8) ![]u8 {
    return encodeThemed(alloc, operation, text, .{ 216, 216, 216 }, .{ 32, 32, 32 });
}

/// Theme colors come from this Ghostty Surface, never from message text.
pub fn encodeThemed(alloc: std.mem.Allocator, operation: Operation, text: []const u8, foreground: [3]u8, background: [3]u8) ![]u8 {
    if (text.len > max_text_bytes) return error.TextLimit;
    if (operation == .replace_runs or operation == .append_runs) return encodeRuns(alloc, operation, text, foreground, background);
    return encodePlain(alloc, operation, text);
}

fn encodePlain(alloc: std.mem.Allocator, operation: Operation, text: []const u8) ![]u8 {
    if (text.len > max_text_bytes) return error.TextLimit;
    const view = std.unicode.Utf8View.init(text) catch return error.UnsafeText;
    var it = view.iterator();
    while (it.nextCodepoint()) |cp| {
        if ((cp < 0x20 and cp != '\n' and cp != '\t') or
            (cp >= 0x7f and cp <= 0x9f)) return error.UnsafeText;
    }
    const prefix = if (operation == .replace_window) reset_prefix else "";
    // Input is bounded above, so this addition cannot overflow usize.
    const output = try alloc.alloc(u8, prefix.len + text.len + std.mem.count(u8, text, "\n"));
    @memcpy(output[0..prefix.len], prefix);
    var cursor = prefix.len;
    for (text) |byte| {
        if (byte == '\n') { output[cursor] = '\r'; cursor += 1; }
        output[cursor] = byte;
        cursor += 1;
    }
    std.debug.assert(cursor == output.len);
    return output;
}

test "display encoder emits only its own control sequences" {
    const bytes = try encode(std.testing.allocator, .replace_window, "Polar\n对话\t🙂");
    defer std.testing.allocator.free(bytes);
    try std.testing.expectEqualStrings(reset_prefix ++ "Polar\r\n对话\t🙂", bytes);
}

test "append does not reset the existing grid or scrollback" {
    const bytes = try encode(std.testing.allocator, .append_text, "a\nb");
    defer std.testing.allocator.free(bytes);
    try std.testing.expectEqualStrings("a\r\nb", bytes);
}

test "empty replacement clears previous conversation" {
    const bytes = try encode(std.testing.allocator, .replace_window, "");
    defer std.testing.allocator.free(bytes);
    try std.testing.expectEqualStrings(reset_prefix, bytes);
}

test "untrusted terminal controls and invalid utf8 never reach parser" {
    for ([_][]const u8{ "a\x1b]52;c;secret\x07", "\xc2\x9b31m", "\x00", "\r", "\x7f", "\xff", "\xc0\xaf", "\xed\xa0\x80", "\xf4\x90\x80\x80" }) |text| {
        try std.testing.expectError(error.UnsafeText, encode(std.testing.allocator, .append_text, text));
    }
}

test "capacity and allocation failure have no partial output" {
    const text = try std.testing.allocator.alloc(u8, max_text_bytes + 1);
    defer std.testing.allocator.free(text);
    @memset(text, 'x');
    try std.testing.expectError(error.TextLimit, encode(std.testing.allocator, .append_text, text));
    var buffer: [1]u8 = undefined;
    var fixed = std.heap.FixedBufferAllocator.init(&buffer);
    try std.testing.expectError(error.OutOfMemory, encode(fixed.allocator(), .replace_window, "text"));
}

const BackgroundRole = enum { message_header, message_body, reference };
const Style = struct { source_grapheme: usize = 0, source: ?struct { start: usize, end: usize } = null, background: ?BackgroundRole = null, bold: bool = false, italic: bool = false, dim: bool = false, strike: bool = false, code: bool = false, foreground: ?u8 = null, link: ?[]const u8 = null };
const Run = struct { text: []const u8, style: Style = .{} };
fn encodeRuns(alloc: std.mem.Allocator, operation: Operation, json: []const u8, foreground: [3]u8, background: [3]u8) ![]u8 {
    const parsed = try std.json.parseFromSlice([]Run, alloc, json, .{});
    defer parsed.deinit();
    if (parsed.value.len > 8192) return error.TextLimit;
    var output: std.ArrayList(u8) = .empty;
    errdefer output.deinit(alloc);
    try output.appendSlice(alloc, "\x1b[?2026h");
    if (operation == .replace_runs) try output.appendSlice(alloc, reset_prefix);
    for (parsed.value) |run| {
        // Decode and validate before any byte is delivered to the VT parser.
        const text = try encodePlain(alloc, .append_text, run.text);
        defer alloc.free(text);
        try output.appendSlice(alloc, "\x1b[0m");
        if (run.style.bold) try output.appendSlice(alloc, "\x1b[1m");
        if (run.style.dim) try output.appendSlice(alloc, "\x1b[2m");
        if (run.style.italic) try output.appendSlice(alloc, "\x1b[3m");
        if (run.style.strike) try output.appendSlice(alloc, "\x1b[9m");
        if (run.style.code or run.style.background != null) {
            const strength: u16 = if (run.style.code) 10 else if (run.style.background) |role| switch (role) {
                .message_header => 8,
                .message_body => 4,
                .reference => 7,
            } else 0;
            var shade: [3]u8 = undefined;
            for (&shade, foreground, background) |*component, fg, bg| component.* = @intCast((@as(u16, bg) * (100 - strength) + @as(u16, fg) * strength) / 100);
            const sequence = try std.fmt.allocPrint(alloc, "\x1b[48;2;{d};{d};{d}m", .{ shade[0], shade[1], shade[2] });
            defer alloc.free(sequence);
            try output.appendSlice(alloc, sequence);
        }
        if (run.style.foreground) |color| {
            if (color > 15) return error.UnsafeText;
            const sequence = try std.fmt.allocPrint(alloc, "\x1b[38;5;{d}m", .{color});
            defer alloc.free(sequence);
            try output.appendSlice(alloc, sequence);
        }
        if (run.style.link) |url| {
            if (!(std.ascii.startsWithIgnoreCase(url, "https://") or std.ascii.startsWithIgnoreCase(url, "http://"))) return error.UnsafeText;
            for (url) |c| if (c <= 0x20 or c == 0x7f or c == '\\') return error.UnsafeText;
            const validated = try encodePlain(alloc, .append_text, url);
            defer alloc.free(validated);
            try output.appendSlice(alloc, "\x1b]8;;");
            try output.appendSlice(alloc, url);
            try output.appendSlice(alloc, "\x1b\\");
        }
        try output.appendSlice(alloc, text);
        if (run.style.link != null) try output.appendSlice(alloc, "\x1b]8;;\x1b\\");
        if (output.items.len > 512 * 1024) return error.TextLimit;
    }
    try output.appendSlice(alloc, "\x1b[0m\x1b[?2026l");
    return output.toOwnedSlice(alloc);
}

test "typed styles encode safely and reject escaped controls" {
    const bytes = try encode(std.testing.allocator, .replace_runs,
        "[{\"text\":\"hello\",\"style\":{\"bold\":true}}]");
    defer std.testing.allocator.free(bytes);
    try std.testing.expect(std.mem.indexOf(u8, bytes, "\x1b[1mhello") != null);
    try std.testing.expectError(error.UnsafeText, encode(std.testing.allocator, .replace_runs,
        "[{\"text\":\"\\u001b]52;x\"}]"));
    try std.testing.expectError(error.UnsafeText, encode(std.testing.allocator, .append_runs,
        "[{\"text\":\"click\",\"style\":{\"link\":\"file:///tmp/a\"}}]"));
}

test "code has a subdued actual-theme background without inverse video" {
    const json = "[{\"text\":\"code\",\"style\":{\"code\":true}}]";
    for ([_]struct { fg: u8, bg: u8, shade: []const u8 }{ .{ .fg = 240, .bg = 16, .shade = "38;38;38" }, .{ .fg = 16, .bg = 240, .shade = "217;217;217" } }) |theme| {
        const encoded = try encodeThemed(std.testing.allocator, .replace_runs, json, @splat(theme.fg), @splat(theme.bg));
        defer std.testing.allocator.free(encoded);
        try std.testing.expect(std.mem.indexOf(u8, encoded, theme.shade) != null);
        try std.testing.expect(std.mem.indexOf(u8, encoded, "\x1b[7m") == null);
    }
}


test "message backgrounds use semantic theme roles and reject arbitrary values" {
    const json = "[{\"text\":\"card\",\"style\":{\"background\":\"message_body\"}}]";
    const encoded = try encodeThemed(std.testing.allocator, .replace_runs, json, .{240,240,240}, .{16,16,16});
    defer std.testing.allocator.free(encoded);
    try std.testing.expect(std.mem.indexOf(u8, encoded, "\x1b[48;2;24;24;24m") != null);
    try std.testing.expectError(error.InvalidEnumTag, encode(std.testing.allocator, .replace_runs,
        "[{\"text\":\"card\",\"style\":{\"background\":\"arbitrary\"}}]"));
}
