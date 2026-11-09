//! Read-only diagnostic: decode one Host response received on standard input.
const std = @import("std");
const model = @import("control_center/model.zig");
pub fn main() !void {
    const allocator = std.heap.page_allocator;
    const bytes = try std.fs.File.stdin().readToEndAlloc(allocator, 16 * 1024 * 1024);
    defer allocator.free(bytes);
    var value = model.Model.init(allocator);
    defer value.deinit();
    try value.applyApplicationResponse(bytes);
    if (value.projection() == null) return error.MissingProjection;
    std.debug.print("Control Center projection decoded successfully\n", .{});
}
