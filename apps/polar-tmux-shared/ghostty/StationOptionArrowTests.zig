    test "Station Option arrows preserve remote Alt identity and user overrides" {
        if (!builtin.target.os.tag.isDarwin())
            return error.SkipZigTest;
        const testing = std.testing;
        var arena = ArenaAllocator.init(testing.allocator);
        defer arena.deinit();
        const alloc = arena.allocator();
        var keybinds: Keybinds = .{};
        try keybinds.init(alloc);
        const left: inputpkg.Binding.Trigger = .{
            .key = .{ .physical = .arrow_left }, .mods = .{ .alt = true },
        };
        const right: inputpkg.Binding.Trigger = .{
            .key = .{ .physical = .arrow_right }, .mods = .{ .alt = true },
        };
        const previous = keybinds.set.get(left).?.value_ptr.*;
        const next = keybinds.set.get(right).?.value_ptr.*;
        try testing.expect(previous.leaf.action == .csi);
        try testing.expect(next.leaf.action == .csi);
        try testing.expectEqualStrings("1;3D", previous.leaf.action.csi);
        try testing.expectEqualStrings("1;3C", next.leaf.action.csi);
        try keybinds.parseCLI(alloc, "alt+left=esc:b");
        const customized = keybinds.set.get(left).?.value_ptr.*;
        try testing.expect(customized.leaf.action == .esc);
        try testing.expectEqualStrings("b", customized.leaf.action.esc);
    }
