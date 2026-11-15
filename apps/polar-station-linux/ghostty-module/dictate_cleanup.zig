//! Native confirmation only. Eligibility, copy and results come from Rust.
const std=@import("std");
const gtk=@import("gtk");
const gobject=@import("gobject");
const domain=@import("dictate.zig");
const a=std.heap.c_allocator;
pub const Confirmation=struct {
    dialog:?*gtk.Dialog=null,
    context:?*anyopaque=null,
    accepted:?*const fn(?*anyopaque) void=null,
    pub fn close(self:*Confirmation) void {
        if(self.dialog)|dialog|{self.dialog=null;dialog.as(gtk.Window).destroy();}
    }
    pub fn show(self:*Confirmation,parent:?*gtk.Window,ui:std.json.Value,context:?*anyopaque,accepted:*const fn(?*anyopaque) void) void {
        self.close();self.context=context;self.accepted=accepted;
        const title=a.dupeZ(u8,domain.string(ui,"confirmation")) catch return;defer a.free(title);
        const dialog=gtk.MessageDialog.new(parent,.{.modal=true,.destroy_with_parent=true},.question,.none,title);
        const area=@as(*gtk.Box,@ptrCast(dialog.getMessageArea()));
        for([_] []const u8{"summary","detail"}) |key| {
            const text=a.dupeZ(u8,domain.string(ui,key)) catch continue;defer a.free(text);
            const label=gtk.Label.new(text);label.setWrap(1);label.setMaxWidthChars(56);label.setXalign(0);area.append(label.as(gtk.Widget));
        }
        if(domain.boolean(ui,"can_apply")) {
            const accept=a.dupeZ(u8,domain.string(ui,"confirm_label")) catch return;defer a.free(accept);
            _=dialog.as(gtk.Dialog).addButton(accept,@intFromEnum(gtk.ResponseType.accept));
        }
        const cancel=a.dupeZ(u8,domain.string(ui,"cancel_label")) catch return;defer a.free(cancel);
        _=dialog.as(gtk.Dialog).addButton(cancel,@intFromEnum(gtk.ResponseType.cancel));
        self.dialog=dialog.as(gtk.Dialog);
        _=gtk.Dialog.signals.response.connect(self.dialog.?,*Confirmation,response,self,.{});
        dialog.as(gtk.Window).present();
    }
    fn response(dialog:*gtk.Dialog,value:c_int,self:*Confirmation) callconv(.c) void {
        self.dialog=null;const accepted=self.accepted;const context=self.context;dialog.as(gtk.Window).destroy();
        if(value==@intFromEnum(gtk.ResponseType.accept))if(accepted)|callback|callback(context);
    }
};
