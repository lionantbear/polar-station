//! Linux-native cache for the Rust-owned tmux Session event feed.
//!
//! The cache deliberately keys only by execution target and tmux Session.
//! Workspace, Tab, Pane, layout, focus, and Ghostty object identifiers never
//! cross this protocol boundary.

const std = @import("std");

pub const Lifecycle = enum {
    connecting,
    ready,
    recovering,
    unavailable,
    ended,
    ending,
};

pub const AttentionState = enum { idle, unknown, working, review_ready, needs_attention, approval_required, decision_required, permission_required, failed };
pub const SummaryFreshness = enum { analyzing, fresh, stale, unavailable };
pub const AttentionSource = enum { unspecified, tmux_activity, tmux_bell, pane_title, agent_hook, ai_classification };
pub const AttentionConfidence = enum { direct, inferred };
pub const MonitorHealth = enum { connected, degraded, disconnected };
pub const Attention = struct {
    summary_revision: u64 = 0,
    summary_sections: []const []const u8 = &.{},
    state: AttentionState = .idle,
    freshness: SummaryFreshness = .unavailable,
    primary_source: AttentionSource = .unspecified,
    confidence: AttentionConfidence = .inferred,
    monitor_health: MonitorHealth = .disconnected,
    attention_since_ms: ?u64 = null,
    unread: bool = false,
    notification_pending: bool = false,
};

pub const KeyWire = struct {
    execution_target: []const u8,
    tmux_session_id: []const u8,
};

const SessionWire = struct {
    key: KeyWire,
    display_name: []const u8,
    lifecycle: Lifecycle,
    revision: u64,
    attention: ?Attention = null,
};

const SnapshotFrame = struct {
    type: []const u8,
    snapshot: struct {
        epoch: []const u8,
        revision: u64,
        sessions: []const SessionWire,
    },
};

const ChangeWire = struct {
    type: []const u8,
    session: ?SessionWire = null,
    key: ?KeyWire = null,
};

const EventFrame = struct {
    type: []const u8,
    event: struct {
        epoch: []const u8,
        revision: u64,
        changes: []const ChangeWire,
    },
};

const Subscription = struct {
    type: []const u8 = "subscribe_session_feed",
    known_epoch: ?[]const u8 = null,
    after_revision: ?u64 = null,
};

const Entry = struct {
    execution_target: []u8,
    tmux_session_id: []u8,
    display_name: []u8,
    lifecycle: Lifecycle,
    revision: u64,
    attention: ?Attention,
};

pub const Cache = struct {
    allocator: std.mem.Allocator,
    epoch: ?[]u8 = null,
    revision: u64 = 0,
    entries: std.StringHashMapUnmanaged(Entry) = .empty,

    pub fn init(allocator: std.mem.Allocator) Cache {
        return .{ .allocator = allocator };
    }

    pub fn deinit(self: *Cache) void {
        self.clear();
        self.entries.deinit(self.allocator);
        self.* = undefined;
    }

    pub fn count(self: *const Cache) usize {
        return self.entries.count();
    }

    pub fn attentionCount(self: *const Cache) usize {
        var count_value: usize = 0;
        var iterator = self.entries.iterator();
        while (iterator.next()) |entry| {
            if (entry.value_ptr.attention != null) count_value += 1;
        }
        return count_value;
    }

    pub fn attention(
        self: *const Cache,
        execution_target: []const u8,
        tmux_session_id: []const u8,
    ) ?Attention {
        var key_buffer: [1024]u8 = undefined;
        const key = cacheKey(&key_buffer, execution_target, tmux_session_id) catch return null;
        return if (self.entries.get(key)) |entry| entry.attention else null;
    }

    pub fn lifecycle(
        self: *const Cache,
        execution_target: []const u8,
        tmux_session_id: []const u8,
    ) ?Lifecycle {
        var key_buffer: [1024]u8 = undefined;
        const key = cacheKey(
            &key_buffer,
            execution_target,
            tmux_session_id,
        ) catch return null;
        return if (self.entries.get(key)) |entry| entry.lifecycle else null;
    }

    pub fn displayName(
        self: *const Cache,
        execution_target: []const u8,
        tmux_session_id: []const u8,
    ) ?[]const u8 {
        var key_buffer: [1024]u8 = undefined;
        const key = cacheKey(
            &key_buffer,
            execution_target,
            tmux_session_id,
        ) catch return null;
        return if (self.entries.get(key)) |entry| entry.display_name else null;
    }

    pub fn applySnapshot(self: *Cache, json: []const u8) !void {
        var parsed = try std.json.parseFromSlice(
            SnapshotFrame,
            self.allocator,
            json,
            .{ .ignore_unknown_fields = true },
        );
        defer parsed.deinit();
        const frame = parsed.value;
        if (!std.mem.eql(u8, frame.type, "session_feed_snapshot")) {
            return error.InvalidFrame;
        }
        try validatePosition(frame.snapshot.epoch, frame.snapshot.revision);

        var replacement = Cache.init(self.allocator);
        errdefer replacement.deinit();
        replacement.epoch = try self.allocator.dupe(u8, frame.snapshot.epoch);
        replacement.revision = frame.snapshot.revision;
        for (frame.snapshot.sessions) |session| {
            if (session.revision == 0 or
                session.revision > frame.snapshot.revision)
            {
                return error.InvalidRevision;
            }
            try replacement.put(session);
        }
        self.swap(&replacement);
        replacement.deinit();
    }

    /// Returns without mutation for a duplicate delta. A gap or Host epoch
    /// change is explicit so the caller can request one replacement snapshot.
    pub fn applyEvent(self: *Cache, json: []const u8) !void {
        var parsed = try std.json.parseFromSlice(
            EventFrame,
            self.allocator,
            json,
            .{ .ignore_unknown_fields = true },
        );
        defer parsed.deinit();
        const frame = parsed.value;
        if (!std.mem.eql(u8, frame.type, "session_feed_event")) {
            return error.InvalidFrame;
        }
        try validatePosition(frame.event.epoch, frame.event.revision);
        if (self.epoch == null or
            !std.mem.eql(u8, self.epoch.?, frame.event.epoch))
        {
            return error.FeedGap;
        }
        if (frame.event.revision <= self.revision) return;
        if (frame.event.revision != self.revision + 1 or
            frame.event.changes.len == 0)
        {
            return error.FeedGap;
        }

        for (frame.event.changes, 0..) |change, index| {
            const key = try changeKey(change);
            for (frame.event.changes[0..index]) |prior| {
                const prior_key = try changeKey(prior);
                if (std.mem.eql(
                    u8,
                    key.execution_target,
                    prior_key.execution_target,
                ) and std.mem.eql(
                    u8,
                    key.tmux_session_id,
                    prior_key.tmux_session_id,
                )) return error.DuplicateSession;
            }
            if (std.mem.eql(u8, change.type, "upsert")) {
                const session = change.session orelse return error.InvalidFrame;
                if (change.key != null or
                    session.revision != frame.event.revision)
                {
                    return error.InvalidFrame;
                }
            } else if (std.mem.eql(u8, change.type, "remove")) {
                if (change.key == null or change.session != null) {
                    return error.InvalidFrame;
                }
            } else return error.InvalidFrame;
        }

        for (frame.event.changes) |change| {
            if (std.mem.eql(u8, change.type, "upsert")) {
                try self.put(change.session.?);
            } else {
                try self.remove(change.key.?);
            }
        }
        self.revision = frame.event.revision;
    }

    pub fn encodeSubscription(
        self: *const Cache,
        allocator: std.mem.Allocator,
    ) ![]u8 {
        return std.json.Stringify.valueAlloc(
            allocator,
            Subscription{
                .known_epoch = self.epoch,
                .after_revision = if (self.epoch == null) null else self.revision,
            },
            .{ .whitespace = .minified },
        );
    }

    fn put(self: *Cache, session: SessionWire) !void {
        try validateKey(session.key);
        if (session.display_name.len == 0 or session.display_name.len > 256) {
            return error.InvalidDisplayName;
        }
        var key_buffer: [1024]u8 = undefined;
        const joined = try cacheKey(
            &key_buffer,
            session.key.execution_target,
            session.key.tmux_session_id,
        );
        if (self.entries.getPtr(joined)) |existing| {
            const display_name = try self.allocator.dupe(
                u8,
                session.display_name,
            );
            self.allocator.free(existing.display_name);
            existing.display_name = display_name;
            existing.lifecycle = session.lifecycle;
            existing.revision = session.revision;
            freeAttention(self.allocator, existing.attention);
            existing.attention = try cloneAttention(self.allocator, session.attention);
            return;
        }
        const owned_key = try self.allocator.dupe(u8, joined);
        errdefer self.allocator.free(owned_key);
        const execution_target = try self.allocator.dupe(
            u8,
            session.key.execution_target,
        );
        errdefer self.allocator.free(execution_target);
        const tmux_session_id = try self.allocator.dupe(
            u8,
            session.key.tmux_session_id,
        );
        errdefer self.allocator.free(tmux_session_id);
        const display_name = try self.allocator.dupe(u8, session.display_name);
        errdefer self.allocator.free(display_name);
        try self.entries.put(self.allocator, owned_key, .{
            .execution_target = execution_target,
            .tmux_session_id = tmux_session_id,
            .display_name = display_name,
            .lifecycle = session.lifecycle,
            .revision = session.revision,
            .attention = try cloneAttention(self.allocator, session.attention),
        });
    }

    fn remove(self: *Cache, key: KeyWire) !void {
        try validateKey(key);
        var key_buffer: [1024]u8 = undefined;
        const joined = try cacheKey(
            &key_buffer,
            key.execution_target,
            key.tmux_session_id,
        );
        if (self.entries.fetchRemove(joined)) |removed| {
            self.freeEntry(removed.key, removed.value);
        }
    }

    fn clear(self: *Cache) void {
        if (self.epoch) |epoch| self.allocator.free(epoch);
        self.epoch = null;
        self.revision = 0;
        var iterator = self.entries.iterator();
        while (iterator.next()) |entry| {
            self.freeEntry(entry.key_ptr.*, entry.value_ptr.*);
        }
        self.entries.clearRetainingCapacity();
    }

    fn freeEntry(self: *Cache, key: []const u8, entry: Entry) void {
        self.allocator.free(key);
        self.allocator.free(entry.execution_target);
        self.allocator.free(entry.tmux_session_id);
        self.allocator.free(entry.display_name);
        freeAttention(self.allocator, entry.attention);
    }

    fn swap(self: *Cache, replacement: *Cache) void {
        std.mem.swap(Cache, self, replacement);
    }
};

fn cloneAttention(allocator: std.mem.Allocator, source: ?Attention) !?Attention {
    const value = source orelse return null;
    const sections = try allocator.alloc([]const u8, value.summary_sections.len);
    errdefer allocator.free(sections);
    var copied: usize = 0;
    errdefer for (sections[0..copied]) |section| allocator.free(section);
    for (value.summary_sections, 0..) |section, index| {
        sections[index] = try allocator.dupe(u8, section);
        copied += 1;
    }
    var result = value;
    result.summary_sections = sections;
    return result;
}

fn freeAttention(allocator: std.mem.Allocator, source: ?Attention) void {
    const value = source orelse return;
    for (value.summary_sections) |section| allocator.free(section);
    allocator.free(value.summary_sections);
}

fn validatePosition(epoch: []const u8, revision: u64) !void {
    if (epoch.len == 0 or epoch.len > 256 or revision == 0) {
        return error.InvalidRevision;
    }
}

fn validateKey(key: KeyWire) !void {
    if (key.execution_target.len == 0 or
        key.tmux_session_id.len == 0 or
        key.execution_target.len > 256 or
        key.tmux_session_id.len > 512)
    {
        return error.InvalidSessionKey;
    }
}

fn changeKey(change: ChangeWire) !KeyWire {
    if (std.mem.eql(u8, change.type, "upsert")) {
        return (change.session orelse return error.InvalidFrame).key;
    }
    if (std.mem.eql(u8, change.type, "remove")) {
        return change.key orelse return error.InvalidFrame;
    }
    return error.InvalidFrame;
}

fn cacheKey(
    buffer: []u8,
    execution_target: []const u8,
    tmux_session_id: []const u8,
) ![]const u8 {
    return std.fmt.bufPrint(
        buffer,
        "{s}\x1f{s}",
        .{ execution_target, tmux_session_id },
    );
}








