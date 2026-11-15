const std = @import("std");

pub const AttachPlan = struct {
    program: []const u8,
    args: []const []const u8,
};

pub const Error = error{
    InvalidAttachPlan,
    SessionMismatch,
};

pub fn validate(plan: AttachPlan, expected_session_id: []const u8) Error!void {
    if (!isStableSessionId(expected_session_id)) return error.InvalidAttachPlan;
    const attached_session_id = if (std.mem.eql(u8, plan.program, "tmux"))
        try localTmuxSession(plan.args)
    else if (std.mem.eql(u8, plan.program, "ssh"))
        try remoteTmuxSession(plan.args)
    else
        return error.InvalidAttachPlan;

    if (!std.mem.eql(u8, attached_session_id, expected_session_id)) {
        return error.SessionMismatch;
    }
}

fn localTmuxSession(args: []const []const u8) Error![]const u8 {
    const session = switch (args.len) {
        3 => blk: {
            if (!std.mem.eql(u8, args[0], "attach-session") or
                !std.mem.eql(u8, args[1], "-t"))
            {
                return error.InvalidAttachPlan;
            }
            break :blk args[2];
        },
        5 => blk: {
            if (!std.mem.eql(u8, args[0], "-L") or
                !isSocketName(args[1]) or
                !std.mem.eql(u8, args[2], "attach-session") or
                !std.mem.eql(u8, args[3], "-t"))
            {
                return error.InvalidAttachPlan;
            }
            break :blk args[4];
        },
        else => return error.InvalidAttachPlan,
    };
    if (!isStableSessionId(session)) return error.InvalidAttachPlan;
    return session;
}

fn remoteTmuxSession(args: []const []const u8) Error![]const u8 {
    if (args.len != 7 or
        !std.mem.eql(u8, args[0], "-S") or
        !isControlSocket(args[1]) or
        !std.mem.eql(u8, args[2], "-o") or
        !std.mem.eql(u8, args[3], "ProxyCommand=/usr/bin/false") or
        !std.mem.eql(u8, args[4], "-tt") or
        !isSshDestination(args[5]))
    {
        return error.InvalidAttachPlan;
    }

    var words = std.mem.splitScalar(u8, args[6], ' ');
    if (!std.mem.eql(u8, words.next() orelse return error.InvalidAttachPlan, "tmux")) {
        return error.InvalidAttachPlan;
    }
    const next = words.next() orelse return error.InvalidAttachPlan;
    if (std.mem.eql(u8, next, "-L")) {
        if (!isSocketName(words.next() orelse return error.InvalidAttachPlan)) {
            return error.InvalidAttachPlan;
        }
        if (!std.mem.eql(
            u8,
            words.next() orelse return error.InvalidAttachPlan,
            "attach-session",
        )) return error.InvalidAttachPlan;
    } else if (!std.mem.eql(u8, next, "attach-session")) {
        return error.InvalidAttachPlan;
    }
    if (!std.mem.eql(u8, words.next() orelse return error.InvalidAttachPlan, "-t")) {
        return error.InvalidAttachPlan;
    }
    const quoted_session = words.next() orelse return error.InvalidAttachPlan;
    if (words.next() != null or quoted_session.len < 3 or
        quoted_session[0] != '\'' or
        quoted_session[quoted_session.len - 1] != '\'')
    {
        return error.InvalidAttachPlan;
    }
    const session = quoted_session[1 .. quoted_session.len - 1];
    if (!isStableSessionId(session)) return error.InvalidAttachPlan;
    return session;
}

fn isControlSocket(value: []const u8) bool {
    if (value.len < 2 or value.len > 103 or value[0] != '/' or
        value[value.len - 1] == '/' or std.mem.indexOf(u8, value, "//") != null)
    {
        return false;
    }
    var components = std.mem.splitScalar(u8, value, '/');
    while (components.next()) |component| {
        if (std.mem.eql(u8, component, ".") or std.mem.eql(u8, component, "..")) {
            return false;
        }
    }
    for (value) |byte| {
        if (!std.ascii.isAlphanumeric(byte) and
            byte != '/' and byte != '.' and byte != '_' and byte != '-')
        {
            return false;
        }
    }
    return true;
}

fn isStableSessionId(value: []const u8) bool {
    if (value.len < 2 or value[0] != '$') return false;
    for (value[1..]) |byte| {
        if (!std.ascii.isDigit(byte)) return false;
    }
    return true;
}

fn isSocketName(value: []const u8) bool {
    if (value.len == 0 or value.len > 96 or !std.ascii.isAlphanumeric(value[0])) {
        return false;
    }
    for (value) |byte| {
        if (!std.ascii.isAlphanumeric(byte) and
            byte != '.' and byte != '_' and byte != '-')
        {
            return false;
        }
    }
    return true;
}

fn isSshDestination(value: []const u8) bool {
    if (value.len == 0 or value.len > 255 or value[0] == '-') return false;
    var has_alphanumeric = false;
    for (value) |byte| {
        if (std.ascii.isAlphanumeric(byte)) {
            has_alphanumeric = true;
            continue;
        }
        if (std.mem.indexOfScalar(u8, "._-@:%+[]", byte) == null) return false;
    }
    return has_alphanumeric;
}
