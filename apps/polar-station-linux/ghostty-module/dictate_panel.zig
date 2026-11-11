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
    .{.kind="use_available",.key="can_available",.label="available_label"},
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
    reference:*gtk.Label,
    css:*gtk.CssProvider,
    more:*gtk.MenuButton,
    actions:*gtk.Box,
    editor:*gtk.TextView,
    scroll:*gtk.ScrolledWindow,
    buttons:[items.len]*gtk.Button,
    editing:bool=false,
    updating:bool=false,
    pub fn create(owner:*dictate.Controller,anchor:?*gtk.Widget) !*Panel {
        const self=try a.create(Panel);
        const box=gtk.Box.new(.vertical,8);box.as(gtk.Widget).setSizeRequest(360,-1);
        box.as(gtk.Widget).addCssClass("station-dictate-card");
        const header=gtk.Box.new(.horizontal,6);
        if(anchor==null) {const handle=gtk.WindowHandle.new();handle.setChild(header.as(gtk.Widget));box.append(handle.as(gtk.Widget));} else box.append(header.as(gtk.Widget));
        const status=gtk.Label.new("");status.setEllipsize(.end);status.setXalign(0);
        status.as(gtk.Widget).setHexpand(1);header.append(status.as(gtk.Widget));
        const reference=gtk.Label.new("");reference.setEllipsize(.end);reference.setSingleLineMode(1);reference.setXalign(0);reference.as(gtk.Widget).addCssClass("station-dictate-reference");box.append(reference.as(gtk.Widget));
        const editor=gtk.TextView.new();editor.getBuffer().setEnableUndo(1);editor.setWrapMode(.word_char);editor.setEditable(0);editor.as(gtk.Widget).setFocusable(0);
        editor.as(gobject.Object).setData("station-dictate-panel",self);editor.as(gtk.Widget).setTooltipText("Dictate result");
        const scroll=gtk.ScrolledWindow.new();scroll.as(gtk.Widget).setFocusable(0);scroll.setPolicy(.never,.automatic);scroll.setMinContentHeight(20);scroll.setMaxContentHeight(80);scroll.setPropagateNaturalHeight(1);scroll.setChild(editor.as(gtk.Widget));box.append(scroll.as(gtk.Widget));
        const row=gtk.Box.new(.horizontal,6);box.append(row.as(gtk.Widget));
        const more=gtk.MenuButton.new();more.setLabel("⋯");more.as(gtk.Widget).setTooltipText("More Dictate actions");more.as(gtk.Widget).setFocusable(0);
        const menu=gtk.Popover.new();menu.as(gtk.Widget).addCssClass("station-dictate-menu");menu.setHasArrow(0);const extra=gtk.Box.new(.vertical,4);menu.setChild(extra.as(gtk.Widget));more.setPopover(menu);
        var buttons:[items.len]*gtk.Button=undefined;
        for(items,0..) |item,i| {
            const button=gtk.Button.newWithLabel("");button.as(gtk.Widget).setFocusOnClick(0);button.as(gtk.Widget).setFocusable(0);
            button.as(gtk.Widget).addCssClass("flat");
            button.as(gobject.Object).setData("dictate-action",@constCast(item.kind.ptr));
            _=gtk.Button.signals.clicked.connect(button,*Panel,clicked,self,.{});
            if(std.mem.eql(u8,item.kind,"stop")) header.append(button.as(gtk.Widget))
            else if(std.mem.eql(u8,item.kind,"copy") or std.mem.eql(u8,item.kind,"insert_draft")) row.append(button.as(gtk.Widget))
            else extra.append(button.as(gtk.Widget));
            buttons[i]=button;
        }
        header.append(more.as(gtk.Widget));
        const css=gtk.CssProvider.new();
        css.loadFromString(".station-dictate-window, .station-dictate-popup > contents { background: transparent; box-shadow: none; padding: 0; } .station-dictate-card { background: rgba(24,26,30,0.80); color: #f4f4f6; border-radius: 12px; padding: 12px; } .station-dictate-card textview, .station-dictate-card textview text, .station-dictate-card scrolledwindow { background: transparent; color: #f4f4f6; border: none; } .station-dictate-reference { color: #c9cbd1; font-size: 12px; border-left: 2px solid #a2a5ad; padding-left: 6px; } .station-dictate-card textview { font-size: 14px; } .station-dictate-card scrollbar.vertical slider { min-height: 16px; } .station-dictate-menu > contents { background: #24262a; color: #f4f4f6; border: 1px solid rgba(255,255,255,0.12); border-radius: 8px; padding: 4px; } .station-dictate-menu button:hover { background: rgba(255,255,255,0.12); } .station-dictate-card button { color: #f4f4f6; min-height: 20px; padding: 2px 6px; }");
        gtk.StyleContext.addProviderForDisplay(box.as(gtk.Widget).getDisplay(),css.as(gtk.StyleProvider),gtk.STYLE_PROVIDER_PRIORITY_APPLICATION+10);
        self.*=.{.owner=owner,.anchor=anchor,.status=status,.reference=reference,.editor=editor,.scroll=scroll,.buttons=buttons,.css=css,.more=more,.actions=row};
        _=gtk.TextBuffer.signals.changed.connect(editor.getBuffer(),*Panel,edited,self,.{});
        if(anchor) |widget| {
            const popup=gtk.Popover.new();popup.as(gtk.Widget).addCssClass("station-dictate-popup");popup.setAutohide(0);popup.setHasArrow(0);popup.setPosition(.top);
            popup.as(gtk.Widget).setFocusable(0);popup.as(gtk.Widget).setParent(widget);popup.setChild(box.as(gtk.Widget));self.popover=popup;
        } else {
            const window=gtk.Window.new();window.setTitle("Dictate result");window.setDefaultSize(360,-1);window.setDecorated(0);window.as(gtk.Widget).addCssClass("station-dictate-window");window.setChild(box.as(gtk.Widget));
            if(gio.Application.getDefault()) |app| if(gobject.ext.cast(gtk.Application,app)) |native| window.setApplication(native);
            _=gtk.Window.signals.close_request.connect(window,*Panel,closed,self,.{});self.window=window;
        }
        return self;
    }
    pub fn destroy(self:*Panel) void {
        gtk.StyleContext.removeProviderForDisplay(self.editor.as(gtk.Widget).getDisplay(),self.css.as(gtk.StyleProvider));self.css.unref();
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
        self.more.popdown();
        if(std.mem.eql(u8,kind,"edit")) {
            self.editing=true;self.editor.setEditable(1);self.editor.as(gtk.Widget).setFocusable(1);
            if(self.popover) |popup| popup.as(gtk.Widget).setFocusable(1);
            _=self.editor.as(gtk.Widget).grabFocus();return;
        }
        if(std.mem.eql(u8,kind,"copy")) {
            var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;self.editor.getBuffer().getBounds(&first,&last);
            const live=self.editor.getBuffer().getText(&first,&last,1);defer glib.free(live);
            self.owner.input(.{.type="copy_draft",.operation_id=dictate.string(self.owner.panelValue(),"operation_id"),.text=std.mem.span(live)});return;
        }
        self.owner.panelAction(kind);
    }
    pub fn present(self:*Panel) void {self.refresh();if(self.window) |window| window.present();}
    pub fn refresh(self:*Panel) void {
        const panel=self.owner.panelValue();
        const quote=a.dupeZ(u8,dictate.string(panel,"reference")) catch return;defer a.free(quote);
        self.reference.setText(quote);self.reference.as(gtk.Widget).setTooltipText(quote);self.reference.as(gtk.Widget).setVisible(@intFromBool(quote.len>0));
        var visible=panel!=.null;
        if(self.anchor) |anchor| {
            visible=visible and anchor.getMapped()!=0;
            if(anchor.getRoot()) |root| {if(gobject.ext.cast(gtk.Window,root.as(gobject.Object))) |window| {
                // A nested native menu temporarily deactivates its owning
                // window. Hiding this parent here would destroy that menu.
                visible=visible and (window.isActive()!=0 or self.more.getActive()!=0);
            }} else visible=false;
        }
        if(!visible) {
            if(self.popover) |popup| popup.popdown();
            if(self.window) |window| window.as(gtk.Widget).setVisible(0);
            return;
        }
        const detail=a.dupeZ(u8,dictate.string(panel,"status")) catch return;defer a.free(detail);self.status.as(gtk.Widget).setTooltipText(detail);
        const status=a.dupeZ(u8,dictate.string(panel,"heading")) catch return;defer a.free(status);self.status.setText(status);
        const text=a.dupeZ(u8,dictate.string(panel,"text")) catch return;defer a.free(text);
        const buffer=self.editor.getBuffer();var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;buffer.getBounds(&first,&last);
        const existing=buffer.getText(&first,&last,1);defer glib.free(existing);
        if(!std.mem.eql(u8,std.mem.span(existing),text)){
            const adjustment=self.scroll.getVadjustment();const position=adjustment.getValue();
            self.updating=true;
            if(std.mem.startsWith(u8,text,std.mem.span(existing))) buffer.insert(&last,text.ptr+std.mem.len(existing),@intCast(text.len-std.mem.len(existing)))
            else buffer.setText(text,@intCast(text.len));
            self.updating=false;adjustment.setValue(position);
        }
        // Measure the font before realization; GTK supplies the wrapped natural
        // content height once allocated. Pre-layout iterator coordinates can
        // incorrectly reserve four lines for a single short line.
        const line=self.editor.as(gtk.Widget).createPangoLayout("Ag");
        defer line.as(gobject.Object).unref();
        var measured:c_int=0;line.getPixelSize(null,&measured);
        const line_height=@max(1,measured);
        self.scroll.setMinContentHeight(-1);self.scroll.setMaxContentHeight(4*line_height);
        self.scroll.setMinContentHeight(line_height);
        self.actions.as(gtk.Widget).setVisible(@intFromBool(dictate.boolean(panel,"can_copy") or dictate.boolean(panel,"can_insert")));
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
