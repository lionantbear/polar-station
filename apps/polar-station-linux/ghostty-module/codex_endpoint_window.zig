//! Endpoint form over the existing revisioned Codex settings command.
const std = @import("std");
const gtk = @import("gtk");
const adw = @import("adw");
const gobject = @import("gobject");
const auxiliary = @import("auxiliary_window.zig");
const Application = @import("../class/application.zig").Application;
const wire = @import("codex.zig");
const Saved = *const fn (?*anyopaque, wire.Settings) void;
pub const Window = struct {
    allocator: std.mem.Allocator, app: *Application, context: ?*anyopaque, save: Saved,
    window: ?*adw.Window = null, form: ?*gtk.Box = null, selector: ?*gtk.DropDown = null,
    fields: [7]?*gtk.Entry = @splat(null), enabled: ?*gtk.CheckButton = null,
    mode: ?*gtk.DropDown = null, message: ?*gtk.Label = null,
    snapshot: ?std.json.Parsed(wire.Settings) = null, owner: ?[]u8 = null,
    pending: bool = false, confirming: bool = false,
    machine_id: ?[]u8 = null,
    endpoint_indices: std.ArrayList(usize) = .empty,
    pub fn init(a: std.mem.Allocator, app: *Application, save: Saved, context: ?*anyopaque) Window { return .{ .allocator=a, .app=app, .save=save, .context=context }; }
    pub fn deinit(self: *Window) void {
        if (self.machine_id) |id| self.allocator.free(id);
        self.endpoint_indices.deinit(self.allocator);
        if(self.window)|w|auxiliary.destroy(w);
        if(self.snapshot)|v|v.deinit(); if(self.owner)|v|self.allocator.free(v);
    }
    pub fn present(self: *Window, settings: wire.Settings, owner: []const u8) void { self.presentForMachine(settings, owner, null); }
    pub fn presentForMachine(self: *Window, settings: wire.Settings, owner: []const u8, machine: ?[]const u8) void {
        if(self.pending)return;
        const bytes=std.json.Stringify.valueAlloc(self.allocator,settings,.{}) catch return; defer self.allocator.free(bytes);
        const copy=std.json.parseFromSlice(wire.Settings,self.allocator,bytes,.{ .allocate=.alloc_always }) catch return;
        if(self.snapshot)|v|v.deinit();self.snapshot=copy;
        if(self.owner)|v|self.allocator.free(v);self.owner=self.allocator.dupe(u8,owner) catch null;
        if (self.machine_id) |id| self.allocator.free(id);
        self.machine_id = if (machine) |id| self.allocator.dupe(u8, id) catch null else null;
        self.endpoint_indices.clearRetainingCapacity();
        self.ensure();
        const choices=gtk.StringList.new(null);defer choices.unref();choices.append("+ Add endpoint");
        for(settings.endpoints, 0..)|e, i|{
            if (self.machine_id) |id| { if (!std.mem.eql(u8, id, e.execution_target_id)) continue; }
            self.endpoint_indices.append(self.allocator, i) catch continue;
            const z=self.allocator.dupeZ(u8,e.id) catch continue;defer self.allocator.free(z);choices.append(z);}
        self.selector.?.setModel(choices.as(@import("gio").ListModel));self.selector.?.setSelected(0);
        self.enabled.?.setActive(@intFromBool(settings.enabled));self.select();
        self.message.?.setLabel("Select an endpoint or add one. Other notification settings are preserved.");
        auxiliary.present(self.app,self.window.?);
    }
    pub fn result(self: *Window, message: ?[]const u8) void {
        self.pending=false;if(self.form)|form|form.as(gtk.Widget).setSensitive(1);
        if(message)|text|{const z=self.allocator.dupeZ(u8,text) catch return;defer self.allocator.free(z);self.message.?.setLabel(z);}else if(self.window)|w|w.as(gtk.Widget).setVisible(0);
    }
    fn ensure(self: *Window) void {
        if(self.window!=null)return;
        const w=auxiliary.create(self.app);self.window=w;w.as(gtk.Window).setTitle("Codex app-servers");w.as(gtk.Window).setDefaultSize(540,620);w.as(gtk.Window).setHideOnClose(1);
        const root=gtk.Box.new(.vertical,10);root.as(gtk.Widget).setMarginStart(20);root.as(gtk.Widget).setMarginEnd(20);root.as(gtk.Widget).setMarginTop(20);root.as(gtk.Widget).setMarginBottom(20);
        const form=gtk.Box.new(.vertical,8);self.form=form;root.append(form.as(gtk.Widget));
        const selector=gtk.DropDown.new(null,null);self.selector=selector;form.append(selector.as(gtk.Widget));
        const enabled=gtk.CheckButton.newWithLabel("Enable Codex control and monitoring");self.enabled=enabled;form.append(enabled.as(gtk.Widget));
        inline for(.{"Endpoint ID","Machine ID (from Connections; local for this computer)","App-server socket on machine","Remote owner Host ID","Authorized Host connection ID","Endpoint ID on remote Host","Target ID on remote Host"},0..)|title,i|{
            const entry=gtk.Entry.new();entry.setPlaceholderText(title);self.fields[i]=entry;form.append(entry.as(gtk.Widget));
        }
        // DropDown.new consumes the model reference (transfer full).
        const modes=gtk.StringList.new(null);modes.append("Local app-server");modes.append("App-server over SSH");modes.append("Remote Station Host");
        self.mode=gtk.DropDown.new(modes.as(@import("gio").ListModel),null);form.append(self.mode.?.as(gtk.Widget));
        self.message=gtk.Label.new("");self.message.?.setWrap(1);root.append(self.message.?.as(gtk.Widget));
        const save=gtk.Button.newWithLabel("Save");form.append(save.as(gtk.Widget));
        const remove=gtk.Button.newWithLabel("Remove endpoint…");form.append(remove.as(gtk.Widget));
        const close=gtk.Button.newWithLabel("Close");root.append(close.as(gtk.Widget));
        _=gtk.Button.signals.clicked.connect(save,*Window,saveClicked,self,.{});
        _=gtk.Button.signals.clicked.connect(remove,*Window,removeClicked,self,.{});
        _=gtk.Button.signals.clicked.connect(close,*Window,closeClicked,self,.{});
        _=gobject.Object.signals.notify.connect(selector.as(gobject.Object),*Window,selectionChanged,self,.{.detail="selected"});
        _=gobject.Object.signals.notify.connect(self.mode.?.as(gobject.Object),*Window,modeChanged,self,.{.detail="selected"});
        w.setContent(root.as(gtk.Widget));
    }
    fn select(self: *Window) void {
        self.confirming=false;
        const snapshot=self.snapshot orelse return;
        const index=self.selector.?.getSelected();
        var e: wire.Endpoint=.{.id="",.execution_target_id=self.machine_id orelse "local",.owner_host_id=self.owner orelse "",.control_mode="local",.socket_path=""};
        if(index>0 and index<=self.endpoint_indices.items.len)e=snapshot.value.endpoints[self.endpoint_indices.items[index-1]];
        const values=[_] []const u8{e.id,e.execution_target_id,e.socket_path,e.owner_host_id,e.peer_connection_ref orelse "",e.remote_endpoint_id orelse "",e.remote_target_id orelse ""};
        for(values,0..)|value,i|{const z=self.allocator.dupeZ(u8,value) catch continue;defer self.allocator.free(z);self.fields[i].?.as(gtk.Editable).setText(z);}
        self.fields[0].?.as(gtk.Editable).setEditable(@intFromBool(index==0));
        self.fields[1].?.as(gtk.Widget).setSensitive(@intFromBool(self.machine_id == null));
        self.mode.?.setSelected(if(std.mem.eql(u8,e.control_mode,"remote_host"))2 else if(std.mem.eql(u8,e.control_mode,"ssh_direct"))1 else 0);
        modeChanged(self.mode.?.as(gobject.Object),undefined,self);
    }
    fn field(self: *Window, index: usize) []const u8 {return std.mem.trim(u8,std.mem.span(self.fields[index].?.as(gtk.Editable).getText())," \r\n\t");}
    fn submit(self: *Window, remove: bool) void {
        if(self.pending)return;const saved=self.snapshot orelse return;
        const id=self.field(0);if(id.len==0)return;
        if(remove and !self.confirming){self.confirming=true;self.message.?.setLabel("Click Remove endpoint again to confirm. Active bindings are checked by the Host.");return;}
        var entries: std.ArrayList(wire.Endpoint)=.empty;defer entries.deinit(self.allocator);
        for(saved.value.endpoints)|e|{if(!std.mem.eql(u8,e.id,id))entries.append(self.allocator,e) catch return;}
        const remote=self.mode.?.getSelected()==2;
        if(!remove)entries.append(self.allocator,.{.id=id,.execution_target_id=self.field(1),.socket_path=self.field(2),.owner_host_id=if(remote)self.field(3) else self.owner orelse "",.control_mode=if(remote)"remote_host" else if(self.mode.?.getSelected()==1)"ssh_direct" else "local",.peer_connection_ref=if(remote)self.field(4) else null,.remote_endpoint_id=if(remote)self.field(5) else null,.remote_target_id=if(remote)self.field(6) else null}) catch return;
        var settings=saved.value;settings.endpoints=entries.items;settings.enabled=self.enabled.?.getActive()!=0;
        self.pending=true;self.form.?.as(gtk.Widget).setSensitive(0);self.message.?.setLabel("Saving…");self.save(self.context,settings);
    }
    fn saveClicked(_: *gtk.Button,self: *Window) callconv(.c) void {self.submit(false);}
    fn removeClicked(_: *gtk.Button,self: *Window) callconv(.c) void {self.submit(true);}
    fn closeClicked(_: *gtk.Button,self: *Window) callconv(.c) void {self.window.?.as(gtk.Widget).setVisible(0);}
    fn selectionChanged(_: *gobject.Object,_: *gobject.ParamSpec,self: *Window) callconv(.c) void {self.select();}
    fn modeChanged(_: *gobject.Object,_: *gobject.ParamSpec,self: *Window) callconv(.c) void {for(3..7)|i|self.fields[i].?.as(gtk.Widget).setVisible(@intFromBool(self.mode.?.getSelected()==2));}
};
