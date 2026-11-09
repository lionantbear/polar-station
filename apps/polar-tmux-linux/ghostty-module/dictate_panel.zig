//! Native rendering of the shared Dictate panel. No recording or routing policy.
const std=@import("std");
const gtk=@import("gtk");
const gio=@import("gio");
const glib=@import("glib");
const gobject=@import("gobject");
const dictate=@import("dictate.zig");
const a=std.heap.c_allocator;
const Item=struct{kind:[:0]const u8,key:[:0]const u8,label:[:0]const u8};
const items=[_]Item{
    .{.kind="stop",.key="can_stop",.label="stop_label"},
    .{.kind="cancel",.key="can_cancel",.label="cancel_label"},
    .{.kind="use_original",.key="can_original",.label="original_label"},
    .{.kind="copy",.key="can_copy",.label="copy_label"},
    .{.kind="edit",.key="can_edit",.label="edit_label"},
    .{.kind="insert_draft",.key="can_insert",.label="insert_label"},
    .{.kind="retry_saved",.key="can_retry",.label="retry_label"},
    .{.kind="hide_panel",.key="",.label="close_label"},
};
pub const Panel=struct{
    owner:*dictate.Controller,
    anchor:?*gtk.Widget,
    popover:?*gtk.Popover=null,
    window:?*gtk.Window=null,
    status:*gtk.Label,
    editor:*gtk.TextView,
    scroll:*gtk.ScrolledWindow,
    buttons:[items.len]*gtk.Button,
    editing:bool=false,
    updating:bool=false,
    pub fn create(owner:*dictate.Controller,anchor:?*gtk.Widget) !*Panel {
        const self=try a.create(Panel);
        const box=gtk.Box.new(.vertical,10);box.as(gtk.Widget).setSizeRequest(440,-1);
        box.as(gtk.Widget).setMarginTop(12);box.as(gtk.Widget).setMarginBottom(12);box.as(gtk.Widget).setMarginStart(12);box.as(gtk.Widget).setMarginEnd(12);
        const status=gtk.Label.new("");status.setWrap(1);status.setMaxWidthChars(60);status.setXalign(0);box.append(status.as(gtk.Widget));
        const editor=gtk.TextView.new();editor.getBuffer().setEnableUndo(1);editor.setWrapMode(.word_char);editor.setEditable(0);editor.as(gtk.Widget).setFocusable(0);
        editor.as(gobject.Object).setData("station-dictate-panel",self);editor.as(gtk.Widget).setTooltipText("Dictate result");
        const scroll=gtk.ScrolledWindow.new();scroll.as(gtk.Widget).setFocusable(0);scroll.setPolicy(.never,.automatic);scroll.setMinContentHeight(100);scroll.setMaxContentHeight(220);scroll.setPropagateNaturalHeight(1);scroll.setChild(editor.as(gtk.Widget));box.append(scroll.as(gtk.Widget));
        const row=gtk.Box.new(.horizontal,6);box.append(row.as(gtk.Widget));
        var buttons:[items.len]*gtk.Button=undefined;
        for(items,0..) |item,i| {
            const button=gtk.Button.newWithLabel("");button.as(gtk.Widget).setFocusOnClick(0);button.as(gtk.Widget).setFocusable(0);
            button.as(gobject.Object).setData("dictate-action",@constCast(item.kind.ptr));
            _=gtk.Button.signals.clicked.connect(button,*Panel,clicked,self,.{});
            row.append(button.as(gtk.Widget));buttons[i]=button;
        }
        self.*=.{.owner=owner,.anchor=anchor,.status=status,.editor=editor,.scroll=scroll,.buttons=buttons};
        _=gtk.TextBuffer.signals.changed.connect(editor.getBuffer(),*Panel,edited,self,.{});
        if(anchor) |widget| {
            const popup=gtk.Popover.new();popup.setAutohide(0);popup.setHasArrow(0);popup.setPosition(.top);
            popup.as(gtk.Widget).setFocusable(0);popup.as(gtk.Widget).setParent(widget);popup.setChild(box.as(gtk.Widget));self.popover=popup;
        } else {
            const window=gtk.Window.new();window.setTitle("Dictate result");window.setDefaultSize(480,240);window.setChild(box.as(gtk.Widget));
            if(gio.Application.getDefault()) |app| if(gobject.ext.cast(gtk.Application,app)) |native| window.setApplication(native);
            _=gtk.Window.signals.close_request.connect(window,*Panel,closed,self,.{});self.window=window;
        }
        return self;
    }
    pub fn destroy(self:*Panel) void {
        if(self.popover) |popup| {popup.popdown();popup.as(gtk.Widget).unparent();}
        if(self.window) |window| window.destroy();
        a.destroy(self);
    }
    fn closed(_: *gtk.Window,self:*Panel) callconv(.c) c_int {self.owner.panelAction("hide_panel");return 1;}
    fn edited(buffer:*gtk.TextBuffer,self:*Panel) callconv(.c) void {
        if(self.updating or !self.editing)return;
        var begin:gtk.TextIter=undefined;var end:gtk.TextIter=undefined;buffer.getBounds(&begin,&end);
        const text=buffer.getText(&begin,&end,1);defer glib.free(text);
        self.owner.input(.{.type="edit_draft",.operation_id=dictate.string(self.owner.panelValue(),"operation_id"),.text=std.mem.span(text)});
    }
    fn clicked(button:*gtk.Button,self:*Panel) callconv(.c) void {
        const ptr=button.as(gobject.Object).getData("dictate-action") orelse return;
        const kind=std.mem.span(@as([*:0]const u8,@ptrCast(ptr)));
        if(std.mem.eql(u8,kind,"edit")) {
            self.editing=true;self.editor.setEditable(1);self.editor.as(gtk.Widget).setFocusable(1);
            if(self.popover) |popup| popup.as(gtk.Widget).setFocusable(1);
            _=self.editor.as(gtk.Widget).grabFocus();return;
        }
        if(std.mem.eql(u8,kind,"copy")) {
            var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;const buffer=self.editor.getBuffer();buffer.getBounds(&first,&last);
            const text=buffer.getText(&first,&last,1);defer glib.free(text);self.editor.as(gtk.Widget).getClipboard().setText(text);return;
        }
        self.owner.panelAction(kind);
    }
    pub fn present(self:*Panel) void {self.refresh();if(self.window) |window| window.present();}
    pub fn refresh(self:*Panel) void {
        const panel=self.owner.panelValue();
        var visible=panel!=.null;
        if(self.anchor) |anchor| {
            visible=visible and anchor.getMapped()!=0;
            if(anchor.getRoot()) |root| {if(gobject.ext.cast(gtk.Window,root.as(gobject.Object))) |window| visible=visible and window.isActive()!=0;} else visible=false;
        }
        if(!visible) {
            if(self.popover) |popup| popup.popdown();
            if(self.window) |window| window.as(gtk.Widget).setVisible(0);
            return;
        }
        const status=a.dupeZ(u8,dictate.string(panel,"status")) catch return;defer a.free(status);self.status.setText(status);
        const text=a.dupeZ(u8,dictate.string(panel,"text")) catch return;defer a.free(text);
        const buffer=self.editor.getBuffer();var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;buffer.getBounds(&first,&last);
        const existing=buffer.getText(&first,&last,1);defer glib.free(existing);
        if(!std.mem.eql(u8,std.mem.span(existing),text)){self.updating=true;buffer.setText(text,@intCast(text.len));self.updating=false;}
        self.scroll.as(gtk.Widget).setVisible(@intFromBool(text.len>0 or self.editing));
        for(items,0..) |item,i| {
            const label=a.dupeZ(u8,dictate.string(panel,item.label)) catch continue;defer a.free(label);
            self.buttons[i].setLabel(label);self.buttons[i].as(gtk.Widget).setVisible(@intFromBool(item.key.len==0 or dictate.boolean(panel,item.key)));
        }
        if(!dictate.boolean(panel,"can_edit")){self.editing=false;self.editor.setEditable(0);self.editor.as(gtk.Widget).setFocusable(0);}
        if(self.popover) |popup| if(popup.as(gtk.Widget).getMapped()==0) {
            var previous:?*gtk.Widget=null;
            if(self.anchor) |anchor| if(anchor.getRoot()) |root| if(gobject.ext.cast(gtk.Window,root.as(gobject.Object))) |window| {previous=window.getFocus();};
            popup.popup();
            if(!self.editing) if(previous) |focused| {_=focused.grabFocus();};
        };
    }
};
