// Appended to the pinned renderer/link.zig by the paired managed overlay.
test "Station ineligible links preserve OSC8 cells without allocating a viewport" {
    const alloc = std.testing.allocator;
    var t: terminal.Terminal = try .init(alloc, .{ .cols = 8, .rows = 2 });
    defer t.deinit(alloc);
    var stream = t.vtStream();
    defer stream.deinit();
    try stream.nextSlice("ABCD");
    var state: terminal.RenderState = .empty;
    defer state.deinit(alloc);
    try state.update(alloc, &t);

    const cases = [_]struct { highlight: inputpkg.Link.Highlight, mouse: ?point.Coordinate }{
        .{ .highlight = .hover, .mouse = null },
        .{ .highlight = .{ .hover_mods = .{ .ctrl = true } }, .mouse = null },
        .{ .highlight = .{ .hover_mods = .{ .ctrl = true } }, .mouse = .{ .x = 0, .y = 0 } },
        .{ .highlight = .{ .always_mods = .{ .ctrl = true } }, .mouse = null },
    };
    for (cases) |case| {
        var set = try Set.fromConfig(alloc, &.{.{
            .regex = "AB", .action = .{ .open = {} }, .highlight = case.highlight,
        }});
        defer set.deinit(alloc);
        var result: terminal.RenderState.CellSet = .empty;
        defer result.deinit(alloc);
        const existing: point.Coordinate = .{ .x = 7, .y = 1 };
        try result.put(alloc, existing, {});
        var failing = std.testing.FailingAllocator.init(alloc, .{ .fail_index = 0 });
        try set.renderCellMap(failing.allocator(), &result, &state, case.mouse, .{});
        try std.testing.expect(!failing.has_induced_failure);
        try std.testing.expectEqual(@as(usize, 1), result.count());
        try std.testing.expect(result.contains(existing));
    }
}

test "Station eligible mixed links retain Unicode grapheme and policy semantics" {
    const alloc = std.testing.allocator;
    var t: terminal.Terminal = try .init(alloc, .{ .cols = 8, .rows = 2 });
    defer t.deinit(alloc);
    var stream = t.vtStream();
    defer stream.deinit();
    try stream.nextSlice("a e\u{301} z");
    var state: terminal.RenderState = .empty;
    defer state.deinit(alloc);
    try state.update(alloc, &t);
    const cases = [_]struct { highlight: inputpkg.Link.Highlight, mouse: ?point.Coordinate, mods: inputpkg.Mods }{
        .{ .highlight = .always, .mouse = null, .mods = .{} },
        .{ .highlight = .{ .always_mods = .{ .ctrl = true } }, .mouse = null, .mods = .{ .ctrl = true } },
        .{ .highlight = .hover, .mouse = .{ .x = 2, .y = 0 }, .mods = .{} },
        .{ .highlight = .{ .hover_mods = .{ .ctrl = true } }, .mouse = .{ .x = 2, .y = 0 }, .mods = .{ .ctrl = true } },
    };
    for (cases) |case| {
        var set = try Set.fromConfig(alloc, &.{
            .{ .regex = "z", .action = .{ .open = {} }, .highlight = .{ .always_mods = .{ .alt = true } } },
            .{ .regex = "e\u{301}", .action = .{ .open = {} }, .highlight = case.highlight },
            .{ .regex = "a", .action = .{ .open = {} }, .highlight = .always },
        });
        defer set.deinit(alloc);
        var result: terminal.RenderState.CellSet = .empty;
        defer result.deinit(alloc);
        try set.renderCellMap(alloc, &result, &state, case.mouse, case.mods);
        try std.testing.expectEqual(@as(usize, 2), result.count());
        try std.testing.expect(result.contains(.{ .x = 0, .y = 0 }));
        try std.testing.expect(result.contains(.{ .x = 2, .y = 0 }));
        try std.testing.expect(!result.contains(.{ .x = 4, .y = 0 }));
    }
}
