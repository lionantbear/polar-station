const std = @import("std");

pub const max_payload_bytes: usize = 1024 * 1024;

pub const Error = error{
    PayloadTooLarge,
    EmptyPayload,
};

pub fn encodeLength(length: usize) Error![4]u8 {
    if (length == 0) return error.EmptyPayload;
    if (length > max_payload_bytes) return error.PayloadTooLarge;
    var result: [4]u8 = undefined;
    std.mem.writeInt(u32, &result, @intCast(length), .big);
    return result;
}

pub fn decodeLength(prefix: [4]u8) Error!usize {
    const length: usize = std.mem.readInt(u32, &prefix, .big);
    if (length == 0) return error.EmptyPayload;
    if (length > max_payload_bytes) return error.PayloadTooLarge;
    return length;
}
