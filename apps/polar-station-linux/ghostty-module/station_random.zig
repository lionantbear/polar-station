const std = @import("std");
const global = @import("../../../global.zig");

pub fn int(comptime T: type) T {
    var value: T = undefined;
    global.io().random(std.mem.asBytes(&value));
    return value;
}
