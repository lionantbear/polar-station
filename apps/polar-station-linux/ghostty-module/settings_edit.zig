//! Native control observations only; Rust Settings owns edit protection and CAS.
const std = @import("std");
const c = @cImport({
    @cInclude("polar_station_desktop_client.h");
});
pub const Section = enum { appearance, attention, monitoring, session_summary, voice, voice_retention, imessage };
pub const Lease = struct {
    allocator: std.mem.Allocator,
    handle: ?*c.SettingsEditHandle,
    state: ?std.json.Parsed(View) = null,
    const View = struct { revision: ?u64, can_rebase: bool, rebase_label: []const u8, save_revision: ?u64, sections: []const struct { section: Section, replace: bool, revision: ?u64 } };
    pub fn init(allocator: std.mem.Allocator) Lease {
        return .{ .allocator = allocator, .handle = c.polar_station_settings_edit_new() };
    }
    pub fn deinit(self: *Lease) void {
        if (self.state) |*v| v.deinit();
        c.polar_station_settings_edit_free(self.handle);
    }
    pub fn observe(self: *Lease, revision: u64) void {
        self.send(.{ .type = "observe", .revision = revision });
    }
    pub fn edit(self: *Lease, section: Section) void {
        self.send(.{ .type = "edit", .section = section });
    }
    pub fn replaces(self: *Lease, section: Section) bool {
        if (self.state) |state| for (state.value.sections) |row| {
            if (row.section == section) return row.replace;
        };
        return false;
    }
    pub fn begin(self: *Lease, section: Section, request: []const u8) ?u64 {
        self.send(.{ .type = "begin_save", .section = section, .request_id = request });
        return if (self.state) |state| state.value.save_revision else null;
    }
    pub fn complete(self: *Lease, payload: []const u8) void {
        var parsed = std.json.parseFromSlice(std.json.Value, self.allocator, payload, .{}) catch return;
        defer parsed.deinit();
        const response = field(parsed.value, "response");
        const result = field(response, "result");
        const id = field(response, "request_id");
        const kind = field(result, "kind");
        if (id != .string or kind != .string) return;
        const revision = field(field(result, "projection"), "revision");
        self.completed(id.string,std.mem.eql(u8,kind.string,"projection"),if(revision==.integer and revision.integer>=0) @intCast(revision.integer) else null);
    }
    pub fn completed(self:*Lease,id:[]const u8,accepted:bool,revision:?u64) void {
        self.send(.{.type="complete_save",.request_id=id,.accepted=accepted,.revision=revision});
    }
    pub fn failed(self: *Lease, id: []const u8) void {
        self.send(.{ .type = "complete_save", .request_id = id, .accepted = false, .revision = @as(?u64, null) });
    }
    pub fn canRebase(self: *Lease) bool {
        return if (self.state) |state| state.value.can_rebase else false;
    }
    pub fn rebaseLabel(self: *Lease) []const u8 {
        return if (self.state) |state| state.value.rebase_label else "";
    }
    pub fn rebase(self: *Lease) void {
        self.send(.{ .type = "rebase" });
    }
    pub fn disconnected(self: *Lease) void {
        self.send(.{ .type = "disconnected" });
    }
    fn send(self: *Lease, input: anytype) void {
        const bytes = std.json.Stringify.valueAlloc(self.allocator, input, .{}) catch return;
        defer self.allocator.free(bytes);
        const raw = c.polar_station_settings_edit_dispatch(self.handle, bytes.ptr, bytes.len) orelse return;
        defer c.polar_station_desktop_string_free(raw);
        const next = std.json.parseFromSlice(View, self.allocator, std.mem.span(raw), .{ .allocate = .alloc_always }) catch return;
        if (self.state) |*old| old.deinit();
        self.state = next;
    }
};
fn field(value: std.json.Value, key: []const u8) std.json.Value {
    return if (value == .object) value.object.get(key) orelse .null else .null;
}
