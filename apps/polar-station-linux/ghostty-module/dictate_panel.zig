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
    .{.kind="view_inputs",.key="can_view_inputs",.label="inputs_label"},
    .{.kind="cancel",.key="can_cancel",.label="cancel_label"},
    .{.kind="use_available",.key="can_available",.label="available_label"},
    .{.kind="copy",.key="can_copy",.label="copy_label"},
    .{.kind="edit",.key="can_edit",.label="edit_label"},
    .{.kind="insert_original",.key="",.label=""},
    .{.kind="insert_refined",.key="",.label=""},
    .{.kind="send_original",.key="",.label=""},
    .{.kind="send_refined",.key="",.label=""},
    .{.kind="retry_saved",.key="can_retry",.label="retry_label"},
    .{.kind="view_history",.key="can_history",.label="history_label"},
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
    failure_actions:*gtk.Box,
    editor:*gtk.TextView,
    scroll:*gtk.ScrolledWindow,
    buttons:[items.len]*gtk.Button,
    latest:*gtk.Button,
    current_id:[]const u8="",
    following:bool=true,
    editing:bool=false,
    updating:bool=false,
    hovering:bool=false,
    reported_interaction:bool=false,
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
        const failure_row=gtk.Box.new(.horizontal,12);box.append(failure_row.as(gtk.Widget));
        const more=gtk.MenuButton.new();more.setLabel("⋯");more.as(gtk.Widget).setTooltipText("More Dictate actions");more.as(gtk.Widget).setFocusable(0);
        const menu=gtk.Popover.new();menu.as(gtk.Widget).addCssClass("station-dictate-menu");menu.setHasArrow(0);const extra=gtk.Box.new(.vertical,4);menu.setChild(extra.as(gtk.Widget));more.setPopover(menu);
        var buttons:[items.len]*gtk.Button=undefined;
        for(items,0..) |item,i| {
            const button=gtk.Button.new();button.as(gtk.Widget).setFocusOnClick(0);button.as(gtk.Widget).setFocusable(0);
            button.as(gtk.Widget).addCssClass("flat");
            button.as(gobject.Object).setData("dictate-action",@constCast(item.kind.ptr));
            _=gtk.Button.signals.clicked.connect(button,*Panel,clicked,self,.{});
            if(std.mem.eql(u8,item.kind,"stop")) header.append(button.as(gtk.Widget))
            else if(std.mem.eql(u8,item.kind,"retry_saved") or std.mem.eql(u8,item.kind,"view_history"))failure_row.append(button.as(gtk.Widget))
            else if(direct(item.kind)) {
                const icon=gtk.Image.newFromIconName(if(std.mem.eql(u8,item.kind,"copy")) "edit-copy-symbolic" else if(std.mem.startsWith(u8,item.kind,"send")) "mail-send-symbolic" else "go-bottom-symbolic");icon.setPixelSize(18);
                const overlay=gtk.Overlay.new();overlay.setChild(icon.as(gtk.Widget));
                if(std.mem.endsWith(u8,item.kind,"refined")) {
                    const star=gtk.Image.newFromIconName("starred-symbolic");star.setPixelSize(9);star.as(gtk.Widget).setHalign(.end);star.as(gtk.Widget).setValign(.start);overlay.addOverlay(star.as(gtk.Widget));
                }
                button.setChild(overlay.as(gtk.Widget));button.as(gtk.Widget).setSizeRequest(32,28);row.append(button.as(gtk.Widget));
            }
            else extra.append(button.as(gtk.Widget));
            buttons[i]=button;
        }
        const latest=gtk.Button.newWithLabel("↓ Latest");latest.as(gtk.Widget).setFocusOnClick(0);latest.as(gtk.Widget).setFocusable(0);latest.as(gtk.Widget).addCssClass("flat");latest.as(gtk.Widget).setVisible(0);
        _=gtk.Button.signals.clicked.connect(latest,*Panel,followLatest,self,.{});header.append(latest.as(gtk.Widget));
        header.append(more.as(gtk.Widget));
        const css=gtk.CssProvider.new();
        css.loadFromString(".station-dictate-window, .station-dictate-popup > contents { background: transparent; box-shadow: none; padding: 0; } .station-dictate-card { background: rgba(24,26,30,0.80); color: #f4f4f6; border-radius: 12px; padding: 12px; } .station-dictate-card textview, .station-dictate-card textview text, .station-dictate-card scrolledwindow { background: transparent; color: #f4f4f6; border: none; } .station-dictate-reference { color: #c9cbd1; font-size: 12px; border-left: 2px solid #a2a5ad; padding-left: 6px; } .station-dictate-card textview { font-size: 14px; } .station-dictate-card scrollbar.vertical slider { min-height: 16px; } .station-dictate-menu > contents { background: #24262a; color: #f4f4f6; border: 1px solid rgba(255,255,255,0.12); border-radius: 8px; padding: 4px; } .station-dictate-menu button:hover { background: rgba(255,255,255,0.12); } .station-dictate-card button { color: #f4f4f6; min-height: 20px; padding: 2px 6px; }");
        gtk.StyleContext.addProviderForDisplay(box.as(gtk.Widget).getDisplay(),css.as(gtk.StyleProvider),gtk.STYLE_PROVIDER_PRIORITY_APPLICATION+10);
        self.*=.{.owner=owner,.anchor=anchor,.status=status,.reference=reference,.editor=editor,.scroll=scroll,.buttons=buttons,.css=css,.more=more,.actions=row,.failure_actions=failure_row,.latest=latest};
        const motion=gtk.EventControllerMotion.new();
        _=gtk.EventControllerMotion.signals.enter.connect(motion,*Panel,pointerEntered,self,.{});
        _=gtk.EventControllerMotion.signals.leave.connect(motion,*Panel,pointerLeft,self,.{});
        box.as(gtk.Widget).addController(motion.as(gtk.EventController));
        _=gobject.Object.signals.notify.connect(more.as(gobject.Object),*Panel,menuChanged,self,.{.detail="active"});
        const wheel=gtk.EventControllerScroll.new(.{.vertical=true});wheel.as(gtk.EventController).setPropagationPhase(.capture);
        _=gtk.EventControllerScroll.signals.scroll.connect(wheel,*Panel,userWheel,self,.{});scroll.as(gtk.Widget).addController(wheel.as(gtk.EventController));
        const drag=gtk.GestureClick.new();drag.as(gtk.EventController).setPropagationPhase(.capture);
        _=gtk.GestureClick.signals.pressed.connect(drag,*Panel,scrollPressed,self,.{});
        _=gtk.GestureClick.signals.released.connect(drag,*Panel,scrollReleased,self,.{});
        scroll.getVscrollbar().addController(drag.as(gtk.EventController));
        _=gtk.Adjustment.signals.changed.connect(scroll.getVadjustment(),*Panel,geometryChanged,self,.{});
        _=gtk.Adjustment.signals.value_changed.connect(scroll.getVadjustment(),*Panel,positionChanged,self,.{});
        _=gobject.Object.signals.notify.connect(editor.getBuffer().as(gobject.Object),*Panel,selectionChanged,self,.{.detail="has-selection"});
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
        a.free(self.current_id);
        a.destroy(self);
    }
    fn direct(kind:[]const u8) bool {return std.mem.eql(u8,kind,"copy") or std.mem.startsWith(u8,kind,"insert_") or std.mem.startsWith(u8,kind,"send_");}
    fn pointerEntered(_: *gtk.EventControllerMotion,_:f64,_:f64,self:*Panel) callconv(.c) void {self.hovering=true;self.reportInteraction();}
    fn pointerLeft(_: *gtk.EventControllerMotion,self:*Panel) callconv(.c) void {self.hovering=false;self.reportInteraction();}
    fn menuChanged(_: *gobject.Object,_:*gobject.ParamSpec,self:*Panel) callconv(.c) void {self.reportInteraction();}
    fn reportInteraction(self:*Panel) void {
        const active=self.hovering or self.editing or self.more.getActive()!=0 or self.hasSelection();
        if(active==self.reported_interaction)return;self.reported_interaction=active;
        self.owner.input(.{.type="panel_interaction",.operation_id=dictate.string(self.owner.panelValue(),"operation_id"),.interacting=active});
    }
    fn hasSelection(self:*Panel) bool {var start:gtk.TextIter=undefined;var end:gtk.TextIter=undefined;return self.editor.getBuffer().getSelectionBounds(&start,&end)!=0;}
    fn geometryChanged(_: *gtk.Adjustment,self:*Panel) callconv(.c) void {self.applyFollowing();}
    fn atBottom(self:*Panel) bool {const adjustment=self.scroll.getVadjustment();return adjustment.getValue()+adjustment.getPageSize()>=adjustment.getUpper()-2;}
    fn userWheel(_: *gtk.EventControllerScroll,_:f64,dy:f64,self:*Panel) callconv(.c) c_int {
        if(dy!=0){self.following=dy>0 and self.atBottom() and !self.editing and !self.hasSelection();self.latest.as(gtk.Widget).setVisible(@intFromBool(!self.following));}
        return 0;
    }
    fn scrollPressed(_: *gtk.GestureClick,_:c_int,_:f64,_:f64,self:*Panel) callconv(.c) void {self.following=false;self.latest.as(gtk.Widget).setVisible(1);}
    fn scrollReleased(_: *gtk.GestureClick,_:c_int,_:f64,_:f64,self:*Panel) callconv(.c) void {positionChanged(self.scroll.getVadjustment(),self);}
    fn positionChanged(_: *gtk.Adjustment,self:*Panel) callconv(.c) void {
        if(self.updating)return;
        // GTK can move the insertion mark during deferred layout. Only native
        // user gestures pause following; a programmatic adjustment must not.
        if(self.following){self.applyFollowing();return;}
        if(self.atBottom() and !self.editing and !self.hasSelection()){self.following=true;self.latest.as(gtk.Widget).setVisible(0);}
    }
    fn selectionChanged(_: *gobject.Object,_:*gobject.ParamSpec,self:*Panel) callconv(.c) void {
        if(!self.updating){if(self.hasSelection()){self.following=false;self.latest.as(gtk.Widget).setVisible(1);}self.reportInteraction();}
    }
    fn followLatest(_: *gtk.Button,self:*Panel) callconv(.c) void {self.following=true;self.editing=false;self.editor.setEditable(0);self.applyFollowing();self.reportInteraction();}
    fn applyFollowing(self:*Panel) void {
        if(self.updating)return;
        if(self.following and !self.editing){
            self.updating=true;const adjustment=self.scroll.getVadjustment();adjustment.setValue(@max(0,adjustment.getUpper()-adjustment.getPageSize()));self.updating=false;
        }
        self.latest.as(gtk.Widget).setVisible(@intFromBool(!self.following));
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
            self.following=false;self.latest.as(gtk.Widget).setVisible(1);
            self.editing=true;self.reportInteraction();self.editor.setEditable(1);self.editor.as(gtk.Widget).setFocusable(1);
            if(self.popover) |popup| popup.as(gtk.Widget).setFocusable(1);
            _=self.editor.as(gtk.Widget).grabFocus();return;
        }
        if(std.mem.eql(u8,kind,"copy")) {
            var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;self.editor.getBuffer().getBounds(&first,&last);
            const live=self.editor.getBuffer().getText(&first,&last,1);defer glib.free(live);
            self.owner.input(.{.type="copy_preview",.operation_id=dictate.string(self.owner.panelValue(),"operation_id"),.text=std.mem.span(live)});return;
        }
        self.owner.panelAction(kind);
    }
    pub fn present(self:*Panel) void {self.refresh();if(self.window) |window| window.present();}
    pub fn refresh(self:*Panel) void {
        const panel=self.owner.panelValue();
        const id=dictate.string(panel,"operation_id");
        if(!std.mem.eql(u8,id,self.current_id)){const next=a.dupe(u8,id) catch return;a.free(self.current_id);self.current_id=next;self.following=true;self.editing=false;self.editor.setEditable(0);self.reported_interaction=false;self.reportInteraction();}
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
        const body=dictate.string(panel,"text");
        const text=a.dupeZ(u8,if(body.len==0 and std.mem.eql(u8,dictate.string(panel,"phase"),"failed"))dictate.string(panel,"status") else body) catch return;defer a.free(text);
        const buffer=self.editor.getBuffer();var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;buffer.getBounds(&first,&last);
        const existing=buffer.getText(&first,&last,1);defer glib.free(existing);
        if(!std.mem.eql(u8,std.mem.span(existing),text)){
            const adjustment=self.scroll.getVadjustment();const position=adjustment.getValue();
            var selected_start:gtk.TextIter=undefined;var selected_end:gtk.TextIter=undefined;
            const selected=buffer.getSelectionBounds(&selected_start,&selected_end)!=0;
            const from=if(selected)selected_start.getOffset() else 0;const to=if(selected)selected_end.getOffset() else 0;
            self.updating=true;
            if(std.mem.startsWith(u8,text,std.mem.span(existing))) buffer.insert(&last,text.ptr+std.mem.len(existing),@intCast(text.len-std.mem.len(existing)))
            else buffer.setText(text,@intCast(text.len));
            if(!self.following){
                if(selected){_=buffer.getIterAtOffset(&selected_start,@min(from,buffer.getCharCount()));_=buffer.getIterAtOffset(&selected_end,@min(to,buffer.getCharCount()));buffer.selectRange(&selected_start,&selected_end);}
                adjustment.setValue(position);
            }
            self.updating=false;
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
        self.actions.as(gtk.Widget).setVisible(@intFromBool(!std.mem.eql(u8,dictate.string(panel,"phase"),"failed") or dictate.boolean(panel,"can_copy")));
        self.failure_actions.as(gtk.Widget).setVisible(@intFromBool(dictate.boolean(panel,"can_history")));
        self.scroll.as(gtk.Widget).setVisible(@intFromBool(text.len>0 or self.editing));
        for(items,0..) |item,i| {
            if(direct(item.kind)) {
                const projected=dictate.field(panel,"actions");if(std.posix.getenv("POLAR_STATION_DEBUG_PREVIEW")!=null)std.log.info("preview action metadata kind={s} array={} count={d}",.{item.kind,projected==.array,if(projected==.array)projected.array.items.len else @as(usize,0)});if(projected!=.array)continue;
                for(projected.array.items) |action| if(std.mem.eql(u8,dictate.string(action,"id"),item.kind)) {
                    const label=a.dupeZ(u8,dictate.string(action,"label")) catch continue;defer a.free(label);
                    var props=[_]gtk.AccessibleProperty{.label};var values=[_]gobject.Value{std.mem.zeroes(gobject.Value)};
                    gtk.AccessibleProperty.initValue(.label,&values[0]);defer values[0].unset();values[0].setString(label);
                    self.buttons[i].as(gtk.Accessible).updatePropertyValue(1,&props,&values);
                    const tooltip=if(dictate.boolean(action,"enabled")) a.dupeZ(u8,label) catch continue else std.fmt.allocPrintSentinel(a,"{s}: {s}",.{label,dictate.string(action,"disabled_reason")},0) catch continue;defer a.free(tooltip);
                    self.buttons[i].as(gtk.Widget).setTooltipText(tooltip);self.buttons[i].as(gtk.Widget).setSensitive(@intFromBool(dictate.boolean(action,"enabled")));self.buttons[i].as(gtk.Widget).setVisible(1);
                };
            } else {
                const label=a.dupeZ(u8,dictate.string(panel,item.label)) catch continue;defer a.free(label);
                self.buttons[i].setLabel(label);self.buttons[i].as(gtk.Widget).setVisible(@intFromBool(item.key.len==0 or dictate.boolean(panel,item.key)));
            }
        }
        if(!dictate.boolean(panel,"can_edit")){self.editing=false;self.editor.setEditable(0);self.editor.as(gtk.Widget).setFocusable(0);}
        self.applyFollowing();
        if(self.popover) |popup| if(popup.as(gtk.Widget).getMapped()==0) {
            var previous:?*gtk.Widget=null;
            if(self.anchor) |anchor| if(anchor.getRoot()) |root| if(gobject.ext.cast(gtk.Window,root.as(gobject.Object))) |window| {previous=window.getFocus();};
            popup.popup();
            if(!self.editing) if(previous) |focused| {_=focused.grabFocus();};
        };
    }
};
