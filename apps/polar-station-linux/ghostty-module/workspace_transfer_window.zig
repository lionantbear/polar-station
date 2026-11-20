//! Native file chooser and presentation adapter for local Workspace interchange.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const glib = @import("glib");
const gobject = @import("gobject");
const c = @cImport({
    @cInclude("polar_station_desktop_client.h");
});
const Value = std.json.Value;
const Dispatch = *const fn (?*anyopaque, []const u8, []const u8, []const u8) void;
pub fn field(v: Value, key: []const u8) Value {
    return if (v == .object) v.object.get(key) orelse .null else .null;
}
pub fn string(v: Value, key: []const u8) []const u8 {
    const f = field(v, key);
    return if (f == .string) f.string else "";
}
pub fn local(a: std.mem.Allocator, request: anytype) !std.json.Parsed(Value) {
    const bytes = try std.json.Stringify.valueAlloc(a, request, .{});
    defer a.free(bytes);
    const result = c.polar_station_workspace_transfer(bytes.ptr, bytes.len) orelse return error.WorkspaceTransferFailed;
    defer c.polar_station_desktop_string_free(result);
    return std.json.parseFromSlice(Value, a, std.mem.span(result), .{ .allocate = .alloc_always });
}
pub const Transfer = struct {
    allocator: std.mem.Allocator,
    context: ?*anyopaque,
    dispatch: Dispatch,
    parent: ?*gtk.Window = null,
    review_box: ?*gtk.Box = null,
    status: ?*gtk.Label = null,
    send: ?*const fn (?*anyopaque, []const u8) anyerror!void = null,
    send_context: ?*anyopaque = null,
    dialog: ?*gtk.NativeDialog = null,
    review: ?*adw.MessageDialog = null,
    name: ?*gtk.Entry = null,
    opening: bool = false,
    idea_sources: ?std.json.Parsed(Value) = null,
    native: ?std.json.Parsed(Value) = null,
    package: ?std.json.Parsed(Value) = null,
    prepared: ?std.json.Parsed(Value) = null,
    export_bytes: ?[]u8 = null,
    pending_id: ?[]u8 = null,
    polar_instances: ?std.json.Parsed(Value) = null,
    pub fn deinit(self: *Transfer) void {
        if (self.dialog) |d| {
            d.destroy();
            d.unref();
        }
        if (self.review) |d| d.as(gtk.Window).destroy();
        if (self.idea_sources) |v| v.deinit();
        if (self.polar_instances) |v| v.deinit();
        if (self.native) |v| v.deinit();
        if (self.package) |v| v.deinit();
        if (self.prepared) |v| v.deinit();
        if (self.export_bytes) |v| self.allocator.free(v);
        if (self.pending_id) |v| self.allocator.free(v);
    }
    pub fn problem(self: *Transfer, message: []const u8) void {
        const z = self.allocator.dupeZ(u8, message) catch return;
        defer self.allocator.free(z);
        if (self.status) |status| status.setText(z);
    }
    fn clearReview(self: *Transfer) void {
        if (self.review_box) |box| {
            while (box.as(gtk.Widget).getFirstChild()) |child| box.remove(child);
        }
        self.name = null;
    }

    fn reviewLabel(box: *gtk.Box, text: [:0]const u8) void {
        const label = gtk.Label.new(text);
        label.setXalign(0);
        label.setWrap(1);
        label.setSelectable(1);
        box.append(label.as(gtk.Widget));
    }

    fn checked(self: *Transfer, result: std.json.Parsed(Value)) ?std.json.Parsed(Value) {
        if (field(result.value, "error") == .string) {
            self.problem(string(result.value, "error"));
            result.deinit();
            return null;
        }
        return result;
    }
    pub fn exportWorkspace(self: *Transfer, bytes: []const u8) void {
        if (self.pending_id != null or self.dialog != null) return;
        const parsed = std.json.parseFromSlice(Value, self.allocator, bytes, .{ .allocate = .alloc_always }) catch return;
        if (self.native) |v| v.deinit();
        self.native = parsed;
        self.opening = false;
        const prepared = self.checked(local(self.allocator, .{ .action = "prepare_export", .native = parsed.value }) catch return) orelse return;
        defer prepared.deinit();
        self.request(.{ .type = "export_content", .bindings = field(prepared.value, "bindings"), .target_ids = field(prepared.value, "target_ids") });
    }
    pub fn openWorkspace(self: *Transfer, bytes: []const u8) void {
        if (self.pending_id != null or self.dialog != null) return;
        const saved = std.json.parseFromSlice(Value, self.allocator, bytes, .{ .allocate = .alloc_always }) catch return;
        if (self.native) |v| v.deinit();
        self.native = saved;
        const sources = self.checked(local(self.allocator, .{ .action = "sources", .native = saved.value }) catch return) orelse return;
        defer sources.deinit();
        const needed = field(sources.value, "needed");
        if (needed == .bool and !needed.bool) {
            self.dispatch(self.context, "open_resolved", string(saved.value, "id"), "");
            return;
        }
        self.opening = true;
        self.request(.{ .type = "resolve", .sources = field(sources.value, "sources") });
    }
    pub fn openImport(self: *Transfer) void {
        if (self.pending_id == null and self.review == null) self.openFile(false);
    }
    fn request(self: *Transfer, value: anytype) void {
        if (self.pending_id != null) return;
        const id = std.fmt.allocPrint(self.allocator, "workspace-transfer-{x}", .{std.crypto.random.int(u64)}) catch return;
        self.pending_id = id;
        self.problem("Reading Workspace references…");
        const bytes = std.json.Stringify.valueAlloc(self.allocator, .{ .type = "application_request", .request = .{ .protocol_version = @as(u32, 1), .api = "control_center_v1", .request_id = id, .frontend_session_id = "workspace-transfer", .expected_control_revision = @as(?u64, null), .command = .{ .type = "host_transfer", .request = value } } }, .{}) catch {
            self.failed("Cannot encode transfer request.");
            return;
        };
        defer self.allocator.free(bytes);
        const send = self.send orelse {
            self.failed("Host is unavailable.");
            return;
        };
        send(self.send_context, bytes) catch self.failed("Host request failed; no Workspace was changed.");
    }
    fn failed(self: *Transfer, message: []const u8) void {
        if (self.pending_id) |v| self.allocator.free(v);
        self.pending_id = null;
        self.problem(message);
    }
    pub fn requestFailed(self: *Transfer, id: []const u8, message: []const u8) bool {
        if (self.pending_id) |p| if (std.mem.eql(u8, p, id)) {
            self.failed(message);
            return true;
        };
        return false;
    }
    pub fn receive(self: *Transfer, payload: []const u8) bool {
        const id = self.pending_id orelse return false;
        const parsed = std.json.parseFromSlice(Value, self.allocator, payload, .{}) catch return false;
        defer parsed.deinit();
        const envelope = field(parsed.value, "response");
        if (!std.mem.eql(u8, id, string(envelope, "request_id"))) return false;
        self.allocator.free(id);
        self.pending_id = null;
        const result = field(envelope, "result");
        if (!std.mem.eql(u8, string(result, "type"), "host_transfer")) {
            self.problem(string(field(result, "error"), "message"));
            return true;
        }
        const response = field(result, "result");
        self.problem("");
        if (std.mem.eql(u8, string(response, "type"), "content_exported")) {
            const native = self.native orelse return true;
            // A view has its own source identity; imported unresolved views retain it.
            const package = self.checked(local(self.allocator, .{ .action = "export", .native = native.value, .bundle = field(response, "bundle"), .sources = field(response, "sources"), .source_workspace = string(native.value, "id"), .idea_sources = if (self.idea_sources) |v| v.value else Value{ .object = std.json.ObjectMap.init(self.allocator) } }) catch return true) orelse return true;
            defer package.deinit();
            if (self.export_bytes) |v| self.allocator.free(v);
            self.export_bytes = std.json.Stringify.valueAlloc(self.allocator, package.value, .{ .whitespace = .indent_2 }) catch return true;
            self.openFile(true);
        } else if (std.mem.eql(u8, string(response, "type"), "resolved")) {
            if (self.opening) {
                self.opening = false;
                const saved = self.native orelse return true;
                const resolved = self.checked(local(self.allocator, .{ .action = "resolve_native", .native = saved.value, .associations = field(response, "associations"), .polar_instances = if (self.polar_instances) |v| v.value else Value{ .array = std.array_list.Managed(Value).init(self.allocator) } }) catch return true) orelse return true;
                defer resolved.deinit();
                const changed = field(resolved.value, "changed");
                const bytes = std.json.Stringify.valueAlloc(self.allocator, field(resolved.value, "native"), .{}) catch return true;
                defer self.allocator.free(bytes);
                self.dispatch(self.context, if (changed == .bool and changed.bool) "resolve_commit" else "open_resolved", string(saved.value, "id"), bytes);
                return true;
            }
            const package = self.package orelse return true;
            const title = string(field(package.value, "workspace"), "name");
            const copy_name = std.fmt.allocPrint(self.allocator, "{s} copy", .{title}) catch return true;
            defer self.allocator.free(copy_name);
            const next = self.checked(local(self.allocator, .{ .action = "import", .package = package.value, .name = copy_name, .associations = field(response, "associations"), .polar_instances = if (self.polar_instances) |v| v.value else Value{ .array = std.array_list.Managed(Value).init(self.allocator) }, .platform = "linux" }) catch return true) orelse return true;
            if (self.prepared) |v| v.deinit();
            self.prepared = next;
            self.confirmImport();
        } else self.problem("Unexpected Workspace transfer response.");
        return true;
    }
    fn confirmImport(self: *Transfer) void {
        const prepared = self.prepared orelse return;
        const box = self.review_box orelse return;
        self.clearReview();
        const text = self.allocator.dupeZ(u8, string(field(prepared.value, "preview"), "summary")) catch return;
        defer self.allocator.free(text);
        reviewLabel(box, text);
        const entry = gtk.Entry.new();
        const entry_name = self.allocator.dupeZ(u8, string(field(prepared.value, "native"), "name")) catch return;
        defer self.allocator.free(entry_name);
        entry.as(gtk.Editable).setText(entry_name);
        self.name = entry;
        box.append(entry.as(gtk.Widget));
        const actions = gtk.Box.new(.horizontal, 8);
        const ok = gtk.Button.newWithLabel("Import Workspace");
        const cancel = gtk.Button.newWithLabel("Cancel");
        actions.append(ok.as(gtk.Widget));
        actions.append(cancel.as(gtk.Widget));
        box.append(actions.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(ok, *Transfer, confirmResponse, self, .{});
        _ = gtk.Button.signals.clicked.connect(cancel, *Transfer, cancelInline, self, .{});
    }
    fn cancelInline(_: *gtk.Button, self: *Transfer) callconv(.c) void {
        self.clearReview();
    }
    fn confirmResponse(_: *gtk.Button, self: *Transfer) callconv(.c) void {
        const prepared = self.prepared orelse return;
        const name = std.mem.span((self.name orelse return).as(gtk.Editable).getText());
        if (std.mem.trim(u8, name, " \t\r\n").len == 0) {
            self.problem("Workspace name is required.");
            return;
        }
        var native = field(prepared.value, "native");
        native.object.put("name", .{ .string = name }) catch return;
        const bytes = std.json.Stringify.valueAlloc(self.allocator, native, .{}) catch return;
        defer self.allocator.free(bytes);
        self.dispatch(self.context, "import_commit", string(native, "id"), bytes);
        self.clearReview();
    }
    fn resolve(self: *Transfer) void {
        const package = self.package orelse return;
        const hosts = field(field(package.value, "hosts"), "hosts");
        var sources: std.ArrayList(Value) = .empty;
        defer sources.deinit(self.allocator);
        if (hosts == .array) for (hosts.array.items) |host| {
            sources.append(self.allocator, field(host, "source")) catch return;
        };
        self.request(.{ .type = "resolve", .sources = sources.items });
    }
    pub fn reviewHosts(self: *Transfer) void {
        const box = self.review_box orelse return;
        self.clearReview();
        reviewLabel(box, "Review included Hosts or continue to see which references are recognized.");
        const actions = gtk.Box.new(.horizontal, 8);
        const hosts = gtk.Button.newWithLabel("Review Hosts…");
        const next = gtk.Button.newWithLabel("Continue");
        const cancel = gtk.Button.newWithLabel("Cancel");
        actions.append(hosts.as(gtk.Widget));
        actions.append(next.as(gtk.Widget));
        actions.append(cancel.as(gtk.Widget));
        box.append(actions.as(gtk.Widget));
        _ = gtk.Button.signals.clicked.connect(hosts, *Transfer, reviewIncludedHosts, self, .{});
        _ = gtk.Button.signals.clicked.connect(next, *Transfer, continueImport, self, .{});
        _ = gtk.Button.signals.clicked.connect(cancel, *Transfer, cancelInline, self, .{});
    }

    pub fn hostsReviewed(context: ?*anyopaque) void {
        const self: *Transfer = @ptrCast(@alignCast(context orelse return));
        self.reviewHosts();
    }

    fn reviewIncludedHosts(_: *gtk.Button, self: *Transfer) callconv(.c) void {
        const package = self.package orelse return;
        const bytes = std.json.Stringify.valueAlloc(self.allocator, field(package.value, "hosts"), .{}) catch return;
        defer self.allocator.free(bytes);
        self.dispatch(self.context, "import_hosts", "", bytes);
    }

    fn continueImport(_: *gtk.Button, self: *Transfer) callconv(.c) void {
        self.clearReview();
        self.resolve();
    }

    fn openFile(self: *Transfer, saving: bool) void {
        if (self.dialog != null) return;
        const chooser = gtk.FileChooserNative.new(if (saving) "Export Workspace" else "Import Workspace", self.parent, if (saving) .save else .open, if (saving) "Export" else "Choose", "Cancel");
        if (saving) chooser.as(gtk.FileChooser).setCurrentName("workspace.polar-workspace.json");
        self.dialog = chooser.as(gtk.NativeDialog);
        _ = gtk.NativeDialog.signals.response.connect(self.dialog.?, *Transfer, fileResponse, self, .{});
        self.dialog.?.show();
    }
    fn fileResponse(dialog: *gtk.NativeDialog, response: c_int, self: *Transfer) callconv(.c) void {
        self.dialog = null;
        defer {
            dialog.destroy();
            dialog.unref();
        }
        if (response != @intFromEnum(gtk.ResponseType.accept)) {
            if (self.export_bytes) |v| self.allocator.free(v);
            self.export_bytes = null;
            return;
        }
        const chooser = gobject.ext.cast(gtk.FileChooser, dialog) orelse return;
        const file = chooser.getFile() orelse return;
        defer file.unref();
        const path = file.getPath() orelse return;
        defer glib.free(path);
        if (self.export_bytes) |bytes| {
            defer {
                self.allocator.free(bytes);
                self.export_bytes = null;
            }
            var buffer: [4096]u8 = undefined;
            var out = std.fs.cwd().atomicFile(std.mem.span(path), .{ .mode = 0o600, .write_buffer = &buffer }) catch {
                self.problem("Cannot open export file.");
                return;
            };
            defer out.deinit();
            out.file_writer.interface.writeAll(bytes) catch {
                self.problem("Cannot write export file.");
                return;
            };
            out.finish() catch {
                self.problem("Cannot finish export.");
                return;
            };
        } else {
            const bytes = std.fs.cwd().readFileAlloc(self.allocator, std.mem.span(path), 8 * 1024 * 1024) catch {
                self.problem("Cannot read Workspace file (maximum 8 MiB).");
                return;
            };
            defer self.allocator.free(bytes);
            const package = std.json.parseFromSlice(Value, self.allocator, bytes, .{ .allocate = .alloc_always }) catch {
                self.problem("Workspace file is not valid JSON.");
                return;
            };
            const inspected = self.checked(local(self.allocator, .{ .action = "inspect", .package = package.value }) catch {
                package.deinit();
                return;
            }) orelse {
                package.deinit();
                return;
            };
            inspected.deinit();
            if (self.package) |old| old.deinit();
            self.package = package;
            self.reviewHosts();
        }
    }
};
