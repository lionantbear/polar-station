// Included in the existing generic renderer by the shared managed overlay.
// The cache test calls the real renderer before its synchronized-output exit,
// so no graphics context or 100000-frame stress run is needed.
test "Station renderer cache age restarts after eviction" {
    const alloc = std.testing.allocator;
    var mutex: std.Thread.Mutex = .{};
    var term = try terminal.Terminal.init(alloc, .{ .cols = 4, .rows = 2 });
    defer term.deinit(alloc);
    term.modes.set(.synchronized_output, true);
    var state: renderer.State = .{ .mutex = &mutex, .terminal = &term };
    var value: renderer.Renderer = undefined;
    value.alloc = alloc;
    value.terminal_state = .empty;
    defer value.terminal_state.deinit(alloc);
    try value.terminal_state.update(alloc, &term);
    value.terminal_state_frame_count = 99_999;
    try value.updateFrame(&state, false);
    try std.testing.expectEqual(@as(usize, 100_000), value.terminal_state_frame_count);
    try std.testing.expectEqual(@as(usize, 2), value.terminal_state.rows);
    try value.updateFrame(&state, false);
    try std.testing.expectEqual(@as(usize, 1), value.terminal_state_frame_count);
    try std.testing.expectEqual(@as(usize, 0), value.terminal_state.rows);
    try value.terminal_state.update(alloc, &term);
    try value.updateFrame(&state, false);
    try std.testing.expectEqual(@as(usize, 2), value.terminal_state_frame_count);
    try std.testing.expectEqual(@as(usize, 2), value.terminal_state.rows);
}

// A source locking invariant, not a claim of a ThreadSanitizer race test.
// Existing provider tests separately verify link policies and Unicode cells.
test "Station renderer links consume the locked mouse snapshot" {
    const source = @embedFile("generic.zig");
    const start = std.mem.indexOf(u8, source, "self.config.links.renderCellMap(").?;
    const call = source[start..];
    const args = call[0..std.mem.indexOf(u8, call, ") catch").?];
    try std.testing.expect(std.mem.indexOf(u8, args, "state.mouse.") == null);
    try std.testing.expect(std.mem.indexOf(u8, args, "critical.mouse.point") != null);
    try std.testing.expect(std.mem.indexOf(u8, args, "critical.mouse.mods") != null);
}

test "Station renderer hidden visibility dominates display link focus" {
    if (comptime builtin.os.tag == .macos) {
        const Callback = struct {
            fn frame(_: *macos.video.DisplayLink, _: ?*void) void {}
        };
        const native_display_link = try macos.video.DisplayLink.createWithActiveCGDisplays();
        defer native_display_link.release();
        defer native_display_link.stop() catch {};
        try native_display_link.setOutputCallback(void, &Callback.frame, null);
        var value: renderer.Renderer = undefined;
        value.display_link = native_display_link;
        value.focused = false;
        value.custom_shader_focused_changed = false;

        value.setVisible(false);
        try value.setFocus(true);
        try std.testing.expect(!native_display_link.isRunning());
        value.setVisible(true);
        try std.testing.expect(native_display_link.isRunning());
        try value.setFocus(false);
        try std.testing.expect(!native_display_link.isRunning());
        value.setVisible(false);
        try value.setFocus(true);
        try std.testing.expect(!native_display_link.isRunning());
        value.setVisible(true);
        try std.testing.expect(native_display_link.isRunning());
        value.setVisible(false);
        try std.testing.expect(!native_display_link.isRunning());

        // The existing disabled-vsync path must remain valid.
        value.display_link = null;
        try value.setFocus(false);
        value.setVisible(true);
        try value.setFocus(true);
        try std.testing.expect(!value.hasVsync());
    } else return error.SkipZigTest;
}
