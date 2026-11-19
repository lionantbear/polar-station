//! Native scalar mirror of StationPaneContentV1, not an authorization grant.
//! The Rust LGP engine owns behavior. These values only survive native layout
//! mutations and select the processless factory; they never become tmux IDs.
const std = @import("std");
pub const Kind = enum { lgp_text, idea_tree, local_diagnostics, remote_hosts, settings_text, tool_setup, unavailable };
pub const Content = struct {
    kind: Kind = .lgp_text,
    reference: ?std.json.Value = null,
    target_id: []const u8 = "",
    node_id: []const u8 = "",
    owner: []const u8 = "",
    thread: []const u8 = "",
    actor: []const u8 = "",

    view_id: []const u8 = "",
    field: []const u8 = "",
    setup_id: []const u8 = "",
    execution_target_id: []const u8 = "",
    pub fn jsonStringify(self: Content, writer: anytype) !void {
        if (self.kind == .unavailable) return writer.write(.{.kind=self.kind,.reference=self.reference});
        if (self.kind == .local_diagnostics or self.kind == .remote_hosts) return writer.write(.{ .kind = self.kind });
        if (self.kind == .tool_setup) return writer.write(.{ .kind=self.kind, .setup_id=self.setup_id, .execution_target_id=self.execution_target_id });
        if (self.kind == .settings_text) return writer.write(.{ .kind = self.kind, .field = self.field });
        if (self.kind == .idea_tree) return writer.write(.{ .kind = self.kind, .view_id = self.view_id });
        return writer.write(.{ .kind = self.kind, .target_id = self.target_id, .node_id = self.node_id, .owner = self.owner, .thread = self.thread, .actor = self.actor });
    }
    pub fn validate(self: Content) !void {
        if (self.kind == .unavailable) {
            if (self.reference == null or self.reference.? != .object) return error.InvalidContentIdentity;
            return;
        }
        if (self.reference != null) return error.InvalidContentIdentity;
        if (self.kind == .tool_setup) {
            inline for (.{ "setup_id", "execution_target_id" }) |name| {
                const value=@field(self,name);
                if (value.len==0 or value.len>256 or !std.unicode.utf8ValidateSlice(value)) return error.InvalidContentIdentity;
                for(value) |c| if(c<32 or c==127) return error.InvalidContentIdentity;
            }
            inline for (.{ "field", "view_id", "target_id", "node_id", "owner", "thread", "actor" }) |name| if(@field(self,name).len!=0) return error.InvalidContentIdentity;
            return;
        }
        if(self.setup_id.len!=0 or self.execution_target_id.len!=0) return error.InvalidContentIdentity;
        if (self.kind == .settings_text) {
            if (!std.mem.eql(u8, self.field, "cleanup") and !std.mem.eql(u8, self.field, "english")) return error.InvalidContentIdentity;
            inline for (.{ "view_id", "target_id", "node_id", "owner", "thread", "actor" }) |name| if (@field(self, name).len != 0) return error.InvalidContentIdentity;
            return;
        }
        if (self.field.len != 0) return error.InvalidContentIdentity;
        if (self.kind == .local_diagnostics or self.kind == .remote_hosts) {
            inline for (.{ "view_id", "target_id", "node_id", "owner", "thread", "actor" }) |field| {
                if (@field(self, field).len != 0) return error.InvalidContentIdentity;
            }
            return;
        }
        if (self.kind == .idea_tree) {
            if (self.view_id.len == 0 or self.view_id.len > 256 or !std.unicode.utf8ValidateSlice(self.view_id)) return error.InvalidContentIdentity;
            for (self.view_id) |c| if (c < 32 or c == 127) return error.InvalidContentIdentity;
            inline for (.{ "target_id", "node_id", "owner", "thread", "actor" }) |field| {
                if (@field(self, field).len != 0) return error.InvalidContentIdentity;
            }
            return;
        }
        if (self.view_id.len != 0) return error.InvalidContentIdentity;
        inline for (.{ "target_id", "node_id", "owner", "thread", "actor" }) |field| {
            const v = @field(self, field);
            if (v.len == 0 or v.len > 256 or !std.unicode.utf8ValidateSlice(v)) return error.InvalidContentIdentity;
            for (v) |c| if (c < 32 or c == 127) return error.InvalidContentIdentity;
        }
    }
    pub fn clone(self: Content, allocator: std.mem.Allocator) !Content {
        // Callers use an arena for the native catalog transaction.
        try self.validate();
        var result = self;
        if(self.reference) |v| {
            const bytes=try std.json.Stringify.valueAlloc(allocator,v,.{});defer allocator.free(bytes);
            result.reference=try std.json.parseFromSliceLeaky(std.json.Value,allocator,bytes,.{.allocate=.alloc_always});
        }
        result.view_id = try allocator.dupe(u8, self.view_id);
        result.field = try allocator.dupe(u8, self.field);
        result.setup_id = try allocator.dupe(u8, self.setup_id);
        result.execution_target_id = try allocator.dupe(u8,self.execution_target_id);
        inline for (.{ "target_id", "node_id", "owner", "thread", "actor" }) |field|
            @field(result, field) = try allocator.dupe(u8, @field(self, field));
        return result;
    }
    pub fn eql(self: Content, other: Content) bool {
        if (self.kind == .unavailable or other.kind == .unavailable) {
            if(self.kind != other.kind) return false;
            var arena=std.heap.ArenaAllocator.init(std.heap.page_allocator);defer arena.deinit();
            const a=self.fingerprint(arena.allocator()) catch return false;
            const b=other.fingerprint(arena.allocator()) catch return false;
            return std.mem.eql(u8,a,b);
        }
        if (!std.mem.eql(u8,self.setup_id,other.setup_id) or !std.mem.eql(u8,self.execution_target_id,other.execution_target_id)) return false;
        if (self.kind != other.kind or !std.mem.eql(u8, self.field, other.field) or !std.mem.eql(u8, self.view_id, other.view_id)) return false;
        inline for (.{ "target_id", "node_id", "owner", "thread", "actor" }) |field|
            if (!std.mem.eql(u8, @field(self, field), @field(other, field))) return false;
        return true;
    }
    pub fn fingerprint(self: Content, allocator: std.mem.Allocator) ![]u8 {
        try self.validate();
        // Length-delimited components, like the canonical Rust/Swift hashes:
        // colons inside identifiers cannot make two targets alias.
        var hash = std.crypto.hash.sha2.Sha256.init(.{});
        component(&hash, "polar-station-pane-content-v1");
        component(&hash, @tagName(self.kind));
        if (self.kind == .unavailable) {
            const bytes=try std.json.Stringify.valueAlloc(allocator,self.reference,.{});defer allocator.free(bytes);component(&hash,bytes);
        } else if (self.kind == .tool_setup) {
            component(&hash,self.setup_id);component(&hash,self.execution_target_id);
        } else if (self.kind == .settings_text) component(&hash, self.field) else if (self.kind == .idea_tree) component(&hash, self.view_id) else if (self.kind == .lgp_text) {
            inline for (.{ "target_id", "node_id", "owner", "thread", "actor" }) |field|
                component(&hash, @field(self, field));
        }
        const digest = hash.finalResult();
        const hex = std.fmt.bytesToHex(digest, .lower);
        return std.fmt.allocPrint(allocator, "semantic:{s}", .{hex});
    }
    fn component(hash: *std.crypto.hash.sha2.Sha256, value: []const u8) void {
        var len: [8]u8 = undefined;
        std.mem.writeInt(u64, &len, @intCast(value.len), .little);
        hash.update(&len);
        hash.update(value);
    }
};










