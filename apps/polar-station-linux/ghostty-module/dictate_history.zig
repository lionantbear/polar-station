//! Stable native reader for the shared Dictate history projection.
const std = @import("std");
const gtk = @import("gtk");
const gio=@import("gio");
const gdk=@import("gdk");
const glib = @import("glib");
const gobject = @import("gobject");
const dictate = @import("dictate.zig");
const Value = std.json.Value;
const a = std.heap.c_allocator;
const field = dictate.field;
const string = dictate.string;
const boolean = dictate.boolean;

pub const History = struct {
    controller: *dictate.Controller,
    input_editor: *gtk.TextView,
    insert_source: c_uint = 0,
    popup: *gtk.Popover,
    list: *gtk.Box,
    scroll: *gtk.ScrolledWindow,
    detail: *gtk.Box,
    detail_viewport: *gtk.ScrolledWindow,
    content: *gtk.TextView,
    detail_heading:*gtk.Label,
    recovery_notice:*gtk.Label,
    text_scroll:*gtk.ScrolledWindow,
    record_actions:*gtk.MenuButton,
    error_label: *gtk.Label,
    retry_open_button: *gtk.Button,
    empty: *gtk.Label,
    original_button: *gtk.Button,
    refined_button: *gtk.Button,
    copy_button: *gtk.Button,
    insert_button: *gtk.Button,
    retry_button: *gtk.Button,
    dismiss_button: *gtk.Button,
    settings_button: *gtk.Button,
    cleanup_button:*gtk.Button,
    delete_button:*gtk.Button,
    pin_button:*gtk.Button,
    delete_confirmation:@import("dictate_cleanup.zig").Confirmation=.{},
    pending_delete_id:?[]u8=null,
    cleanup_requested:bool=false,
    cleanup_confirmation:@import("dictate_cleanup.zig").Confirmation=.{},
    more_button: *gtk.Button,
    input_button:*gtk.Button,
    input_controls:*gtk.Box,
    input_stage:*gtk.DropDown,
    input_request:*gtk.DropDown,
    sources_button:*gtk.Button,
    request_button:*gtk.Button,
    input_status:*gtk.Label,
    updating_inputs:bool=false,
    opening_inputs:bool=false,
    original: bool = false,
    selected_id: ?[]u8 = null,
    rows: std.ArrayListUnmanaged(Row) = .empty,
    const Row = struct { id: []u8, button: *gtk.Button, preview: *gtk.Label, metadata: *gtk.Label };
    const Action = struct { reader: *History, bytes: []u8 };

    pub fn create(controller: *dictate.Controller, input_editor: *gtk.TextView) !*History {
        const self = try a.create(History);
        const popup = gtk.Popover.new(); popup.setPosition(.top);
        const root = gtk.Box.new(.vertical, 8);
        root.as(gtk.Widget).setMarginTop(10); root.as(gtk.Widget).setMarginBottom(10);
        root.as(gtk.Widget).setMarginStart(10); root.as(gtk.Widget).setMarginEnd(10);
        root.as(gtk.Widget).setSizeRequest(440, -1);
        const heading=gtk.Box.new(.horizontal,8);const title=gtk.Label.new("Dictate History");title.setXalign(0);title.as(gtk.Widget).setHexpand(1);title.as(gtk.Widget).addCssClass("heading");heading.append(title.as(gtk.Widget));
        const settings=gtk.Button.newWithLabel("");heading.append(settings.as(gtk.Widget));root.append(heading.as(gtk.Widget));
        const errors = gtk.Label.new(""); errors.setWrap(1); errors.setXalign(0); root.append(errors.as(gtk.Widget));
        const retry_open=gtk.Button.newWithLabel(""); root.append(retry_open.as(gtk.Widget));
        const list = gtk.Box.new(.vertical, 4);
        const scroll = gtk.ScrolledWindow.new(); scroll.setPolicy(.never,.automatic);
        scroll.setMinContentHeight(180); scroll.setMaxContentHeight(180); scroll.setPropagateNaturalHeight(0);
        scroll.setChild(list.as(gtk.Widget)); root.append(scroll.as(gtk.Widget));
        const empty = gtk.Label.new(""); _=empty.as(gobject.Object).refSink(); list.append(empty.as(gtk.Widget));
        const more = gtk.Button.newWithLabel(""); root.append(more.as(gtk.Widget));
        const detail = gtk.Box.new(.vertical, 6);
        const detail_header=gtk.Box.new(.horizontal,6);const detail_heading=gtk.Label.new("");detail_heading.setXalign(0);detail_heading.setEllipsize(.end);detail_heading.as(gtk.Widget).setHexpand(1);detail_header.append(detail_heading.as(gtk.Widget));
        const record_actions=gtk.MenuButton.new();record_actions.setLabel("⋯");const record_menu=gtk.Popover.new();const maintenance=gtk.Box.new(.vertical,6);record_menu.setChild(maintenance.as(gtk.Widget));record_actions.setPopover(record_menu);detail_header.append(record_actions.as(gtk.Widget));detail.append(detail_header.as(gtk.Widget));
        const recovery_notice=gtk.Label.new("");recovery_notice.setXalign(0);recovery_notice.setWrap(1);recovery_notice.setMaxWidthChars(55);recovery_notice.as(gtk.Widget).addCssClass("warning");detail.append(recovery_notice.as(gtk.Widget));
        const variants = gtk.Box.new(.horizontal, 6);
        const original = gtk.Button.newWithLabel(""); const refined = gtk.Button.newWithLabel("");
        variants.append(original.as(gtk.Widget)); variants.append(refined.as(gtk.Widget)); detail.append(variants.as(gtk.Widget));
        const input_button=gtk.Button.newWithLabel("Inputs");variants.append(input_button.as(gtk.Widget));
        const input_controls=gtk.Box.new(.vertical,4);detail.append(input_controls.as(gtk.Widget));
        const input_stage=gtk.DropDown.new(null,null);const input_request=gtk.DropDown.new(null,null);
        input_request.as(gtk.Widget).setSizeRequest(340,-1);
        input_controls.append(input_stage.as(gtk.Widget));input_controls.append(input_request.as(gtk.Widget));
        const input_views=gtk.Box.new(.horizontal,4);const sources_button=gtk.Button.newWithLabel("Sources");const request_button=gtk.Button.newWithLabel("Request text");
        input_views.append(sources_button.as(gtk.Widget));input_views.append(request_button.as(gtk.Widget));input_controls.append(input_views.as(gtk.Widget));
        const input_status=gtk.Label.new("");input_status.setWrap(1);input_status.setMaxWidthChars(44);input_status.setXalign(0);input_controls.append(input_status.as(gtk.Widget));
        const text = gtk.TextView.new(); text.setEditable(0); text.setCursorVisible(0); text.setWrapMode(.word_char);
        const text_scroll = gtk.ScrolledWindow.new(); text_scroll.setPolicy(.never,.automatic);
        text_scroll.setMinContentHeight(140); text_scroll.setMaxContentHeight(140);
        text_scroll.setChild(text.as(gtk.Widget)); detail.append(text_scroll.as(gtk.Widget));
        const actions = gtk.Box.new(.horizontal,6);
        const copy = gtk.Button.newWithLabel(""); const insert = gtk.Button.newWithLabel(""); const retry = gtk.Button.newWithLabel("");
        actions.append(copy.as(gtk.Widget)); actions.append(insert.as(gtk.Widget)); actions.append(retry.as(gtk.Widget));
        const dismiss=gtk.Button.newWithLabel("");maintenance.append(dismiss.as(gtk.Widget));
        detail.append(actions.as(gtk.Widget));
        const pin=gtk.Button.newWithLabel("");const delete=gtk.Button.newWithLabel("");maintenance.append(pin.as(gtk.Widget));maintenance.append(delete.as(gtk.Widget));
        const detail_viewport=gtk.ScrolledWindow.new();detail_viewport.setPolicy(.never,.automatic);
        detail_viewport.setMinContentHeight(300);detail_viewport.setMaxContentHeight(300);detail_viewport.setPropagateNaturalHeight(0);
        detail.as(gtk.Widget).setValign(.start);detail_viewport.setChild(detail.as(gtk.Widget));root.append(detail_viewport.as(gtk.Widget));
        root.append(gtk.Separator.new(.horizontal).as(gtk.Widget));
        const cleanup=gtk.Button.newWithLabel("");root.append(cleanup.as(gtk.Widget));
        popup.setChild(root.as(gtk.Widget));
        self.* = .{ .input_button=input_button,.input_controls=input_controls,.input_stage=input_stage,.input_request=input_request,.sources_button=sources_button,.request_button=request_button,.input_status=input_status,.controller=controller,.input_editor=input_editor,.popup=popup,.list=list,.scroll=scroll,.detail=detail,.detail_viewport=detail_viewport,.content=text,.detail_heading=detail_heading,.recovery_notice=recovery_notice,.text_scroll=text_scroll,.record_actions=record_actions,.error_label=errors,.retry_open_button=retry_open,.empty=empty,
            .original_button=original,.refined_button=refined,.copy_button=copy,.insert_button=insert,.retry_button=retry,.dismiss_button=dismiss,.settings_button=settings,.cleanup_button=cleanup,.delete_button=delete,.pin_button=pin,.more_button=more };
        root.as(gtk.Widget).setFocusable(1);
        const keys=gtk.EventControllerKey.new();keys.as(gtk.EventController).setPropagationPhase(.capture);
        _=gtk.EventControllerKey.signals.key_pressed.connect(keys,*History,historyKeys,self,.{});
        root.as(gtk.Widget).addController(keys.as(gtk.EventController));
        _ = gtk.Widget.signals.map.connect(popup.as(gtk.Widget),*History,opened,self,.{});
        _=gtk.Button.signals.clicked.connect(input_button,*History,showInputs,self,.{});
        _=gtk.Button.signals.clicked.connect(sources_button,*History,showSources,self,.{});
        _=gtk.Button.signals.clicked.connect(request_button,*History,showRequest,self,.{});
        _=gobject.Object.signals.notify.connect(input_stage,*History,stageChanged,self,.{.detail="selected"});
        _=gobject.Object.signals.notify.connect(input_request,*History,requestChanged,self,.{.detail="selected"});
        _ = gtk.Button.signals.clicked.connect(original,*History,showOriginal,self,.{});
        _ = gtk.Button.signals.clicked.connect(refined,*History,showRefined,self,.{});
        _ = gtk.Button.signals.clicked.connect(copy,*History,copyText,self,.{});
        _ = gtk.Button.signals.clicked.connect(insert,*History,insertText,self,.{});
        _ = gtk.Button.signals.clicked.connect(retry,*History,retryRecording,self,.{});
        _ = gtk.Button.signals.clicked.connect(dismiss,*History,dismissDraft,self,.{});
        _ = gtk.Button.signals.clicked.connect(settings,*History,settingsClicked,self,.{});
        _=gtk.Button.signals.clicked.connect(cleanup,*History,cleanupClicked,self,.{});
        _=gtk.Button.signals.clicked.connect(delete,*History,deleteClicked,self,.{});
        _=gtk.Button.signals.clicked.connect(pin,*History,pinClicked,self,.{});
        _ = gtk.Button.signals.clicked.connect(more,*History,loadOlder,self,.{});
        _ = gtk.Button.signals.clicked.connect(retry_open,*History,retryOpen,self,.{});
        popup.as(gobject.Object).weakRef(freed,self);
        self.refresh();
        return self;
    }
    fn freed(data:?*anyopaque,_:*gobject.Object) callconv(.c) void {
        const self:*History=@ptrCast(@alignCast(data orelse return));
        self.cleanup_confirmation.close();self.delete_confirmation.close();if(self.pending_delete_id)|id|a.free(id);
        if(self.insert_source!=0)_=glib.Source.remove(self.insert_source);
        for(self.rows.items)|row|a.free(row.id);self.rows.deinit(a);
        if(self.selected_id)|id|a.free(id);self.empty.as(gobject.Object).unref();a.destroy(self);
    }
    pub fn bindInput(self:*History,editor:*gtk.TextView) void {
        if(self.input_editor==editor) return;
        if(self.insert_source!=0){_=glib.Source.remove(self.insert_source);self.insert_source=0;}
        self.input_editor=editor;
    }
    fn label(button:*gtk.Button, text:[]const u8) void { const z=a.dupeZ(u8,text) catch return;defer a.free(z);button.setLabel(z); }
    fn setLabel(widget:*gtk.Label,text:[]const u8) void {const z=a.dupeZ(u8,text) catch return;defer a.free(z);widget.setText(z);}
    fn rowSummary(button:*gtk.Button,text:[]const u8) void {
        const z=a.dupeZ(u8,text) catch return;defer a.free(z);
        var properties=[_]gtk.AccessibleProperty{.label};
        var values=[_]gobject.Value{std.mem.zeroes(gobject.Value)};
        gtk.AccessibleProperty.initValue(.label,&values[0]);defer values[0].unset();
        values[0].setString(z);
        button.as(gtk.Accessible).updatePropertyValue(1,&properties,&values);
        button.as(gtk.Widget).setTooltipText(z);
    }
    fn number(value:Value,key:[]const u8) i64 {const v=field(value,key);return if(v==.integer) v.integer else 0;}
    fn opened(_: *gtk.Widget,self:*History) callconv(.c) void {
        if(!self.opening_inputs)self.controller.refreshHistory();self.refresh();
        if(self.popup.getChild())|root|{_=root.grabFocus();}
    }
    fn historyKeys(_: *gtk.EventControllerKey,key:c_uint,_:c_uint,mods:gdk.ModifierType,self:*History) callconv(.c) c_int {
        if(key!=gdk.KEY_Up and key!=gdk.KEY_Down)return 0;
        if(mods.control_mask or mods.alt_mask or mods.shift_mask or mods.super_mask)return 0;
        // Text selection and request dropdown navigation belong to the native control.
        if(self.popup.as(gtk.Widget).getRoot())|root|if(root.getFocus())|focus| {
            for([_]*gtk.Widget{self.content.as(gtk.Widget),self.input_stage.as(gtk.Widget),self.input_request.as(gtk.Widget)})|control| {
                if(focus==control or focus.isAncestor(control)!=0)return 0;
            }
        };
        self.controller.input(.{.type="history_move",.direction=if(key==gdk.KEY_Down) @as([]const u8,"down") else "up"});
        return 1;
    }
    fn retryOpen(_: *gtk.Button,self:*History) callconv(.c) void {self.controller.input(.{.type="retry_open"});}
    fn loadOlder(_: *gtk.Button,self:*History) callconv(.c) void {self.controller.input(.{.type="history_more"});}
    fn deleteClicked(_: *gtk.Button,self:*History) callconv(.c) void {
        const id=self.selected_id orelse return;if(self.pending_delete_id)|old|a.free(old);self.pending_delete_id=a.dupe(u8,id) catch return;
        const root=self.input_editor.as(gtk.Widget).getRoot();const parent=if(root)|r|gobject.ext.cast(gtk.Window,r.as(gobject.Object)) else null;
        self.delete_confirmation.show(parent,field(self.controller.view(),"delete_ui"),self,deleteAccepted);
    }
    fn deleteAccepted(context:?*anyopaque) void {const self:*History=@ptrCast(@alignCast(context orelse return));const id=self.pending_delete_id orelse return;self.controller.input(.{.type="history_delete",.operation_id=id,.confirmation=true});a.free(id);self.pending_delete_id=null;}
    fn pinClicked(_: *gtk.Button,self:*History) callconv(.c) void {const id=self.selected_id orelse return;self.controller.input(.{.type="history_pin",.operation_id=id,.pinned=!boolean(field(self.controller.view(),"history_detail"),"pinned")});}
    fn cleanupClicked(_: *gtk.Button,self:*History) callconv(.c) void {self.cleanup_requested=true;self.controller.input(.{.type="cleanup_preview"});}
    fn cleanupAccepted(context:?*anyopaque) void {const self:*History=@ptrCast(@alignCast(context orelse return));self.controller.input(.{.type="cleanup_apply",.confirmation=true});}
    fn settingsClicked(_: *gtk.Button,self:*History) callconv(.c) void {
        self.popup.popdown();if(self.controller.show_settings)|show|show(self.controller.settings_context);
    }
    fn showOriginal(_: *gtk.Button,self:*History) callconv(.c) void {self.original=true;self.controller.input(.{.type="input_close"});self.refreshText();}
    fn showRefined(_: *gtk.Button,self:*History) callconv(.c) void {self.original=false;self.controller.input(.{.type="input_close"});self.refreshText();}
    fn showInputs(_: *gtk.Button,self:*History) callconv(.c) void {self.controller.input(.{.type="history_inputs",.operation_id=self.selected_id orelse return});}
    fn showSources(_: *gtk.Button,self:*History) callconv(.c) void {self.controller.input(.{.type="input_view",.sources=true});}
    fn showRequest(_: *gtk.Button,self:*History) callconv(.c) void {self.controller.input(.{.type="input_view",.sources=false});}
    fn stageChanged(_: *gtk.DropDown,_:*gobject.ParamSpec,self:*History) callconv(.c) void {
        if(self.updating_inputs)return;
        const choices=field(field(self.controller.view(),"inputs"),"stages");const i=self.input_stage.getSelected();
        if(choices==.array and i<choices.array.items.len)self.controller.input(.{.type="input_stage",.stage=string(choices.array.items[i],"id")});
    }
    fn requestChanged(_: *gtk.DropDown,_:*gobject.ParamSpec,self:*History) callconv(.c) void {
        if(self.updating_inputs)return;
        const choices=field(field(self.controller.view(),"inputs"),"requests");const i=self.input_request.getSelected();
        if(choices==.array and i<choices.array.items.len)self.controller.input(.{.type="input_request",.key=string(choices.array.items[i],"key")});
    }
    fn setChoices(dropdown:*gtk.DropDown,values:Value,key:[]const u8,selected:[]const u8) void {
        const entries=if(values==.array)values.array.items else &.{};
        const model=dropdown.getModel();const current=if(model)|m|gobject.ext.cast(gtk.StringList,m.as(gobject.Object)) else null;
        var same=if(model)|m|m.getNItems()==entries.len else false;
        var selection:c_uint=0;
        for(entries,0..)|item,i|{
            if(std.mem.eql(u8,string(item,key),selected))selection=@intCast(i);
            if(same){const previous=if(current)|list|list.getString(@intCast(i)) else null;same=if(previous)|text|std.mem.eql(u8,std.mem.span(text),string(item,"label")) else false;}
        }
        if(!same){
            const names=gtk.StringList.new(null);defer names.unref();
            for(entries)|item|{const z=a.dupeZ(u8,string(item,"label")) catch continue;defer a.free(z);names.append(z);}
            dropdown.setModel(names.as(gio.ListModel));
        }
        if(dropdown.getSelected()!=selection)dropdown.setSelected(selection);
    }
    fn copyText(_: *gtk.Button,self:*History) callconv(.c) void {
        const inputs=field(self.controller.view(),"inputs");
        if(boolean(inputs,"visible")){
            if(!boolean(inputs,"can_copy"))return;
            const z=a.dupeZ(u8,string(inputs,"text")) catch return;defer a.free(z);
            if(gdk.Display.getDefault())|display|display.getClipboard().setText(z);return;
        }
        const detail=field(self.controller.view(),"history_detail");
        self.controller.input(.{.type="history_copy",.operation_id=string(detail,"operation_id"),.original=self.original});
    }
    fn insertText(_: *gtk.Button,self:*History) callconv(.c) void {
        self.popup.popdown();
        if(self.insert_source==0)self.insert_source=glib.idleAdd(insertReady,self);
    }
    fn insertReady(data:?*anyopaque) callconv(.c) c_int {
        const self:*History=@ptrCast(@alignCast(data orelse return 0));self.insert_source=0;
        const id=self.selected_id orelse return 0;
        _=self.input_editor.as(gtk.Widget).grabFocus();
        self.controller.historyAction("history_insert",id);return 0;
    }
    fn dismissDraft(_: *gtk.Button,self:*History) callconv(.c) void {
        self.controller.input(.{.type="history_dismiss",.operation_id=self.selected_id orelse return});
    }
    fn retryRecording(_: *gtk.Button,self:*History) callconv(.c) void {
        const id=self.selected_id orelse return;self.popup.popdown();self.controller.historyAction("history_retry",id);
    }
    fn actionFreed(data:?*anyopaque,_:*gobject.Object) callconv(.c) void {const action:*Action=@ptrCast(@alignCast(data orelse return));a.free(action.bytes);a.destroy(action);}
    fn selectRow(_: *gtk.Button, action:*Action) callconv(.c) void {
        var parsed=std.json.parseFromSlice(Value,a,action.bytes,.{}) catch return;defer parsed.deinit();action.reader.controller.input(parsed.value);
    }
    fn createRow(self:*History, id:[]const u8) !Row {
        const owned=try a.dupe(u8,id);errdefer a.free(owned);
        const bytes=try std.json.Stringify.valueAlloc(a,.{.type="history_select",.operation_id=id},.{});errdefer a.free(bytes);
        const action=try a.create(Action);action.*=.{.reader=self,.bytes=bytes};
        const button=gtk.Button.new();button.as(gtk.Widget).setFocusOnClick(0);
        const column=gtk.Box.new(.vertical,2);
        const preview=gtk.Label.new("");const metadata=gtk.Label.new("");
        for([_]*gtk.Label{preview,metadata})|text|{
            text.setWrap(0);text.setSingleLineMode(1);text.setEllipsize(.end);text.setMaxWidthChars(48);text.setXalign(0);
            text.as(gtk.Widget).setHexpand(1);column.append(text.as(gtk.Widget));
        }
        metadata.as(gtk.Widget).addCssClass("caption");button.setChild(column.as(gtk.Widget));
        button.as(gobject.Object).weakRef(actionFreed,action);
        _=gtk.Button.signals.clicked.connect(button,*Action,selectRow,action,.{});
        return .{.id=owned,.button=button,.preview=preview,.metadata=metadata};
    }
    fn reconcileRows(self:*History, items:[]const Value) void {
        var next:std.ArrayListUnmanaged(Row)=.empty;
        next.ensureTotalCapacity(a,items.len) catch return;
        var previous:?*gtk.Widget=null;
        for(items)|item|{
            const id=string(item,"operation_id");
            const existing: ?Row=for(self.rows.items)|row|{
                if(std.mem.eql(u8,id,row.id))break row;
            }else null;
            const row=existing orelse (self.createRow(id) catch continue);
            const widget=row.button.as(gtk.Widget);
            if(existing==null)self.list.insertChildAfter(widget,previous)
            else if(widget.getPrevSibling()!=previous)self.list.reorderChildAfter(widget,previous);
            next.appendAssumeCapacity(row);previous=widget;
        }
        for(self.rows.items)|row|{
            const retained=for(next.items)|current|{
                if(std.mem.eql(u8,row.id,current.id))break true;
            }else false;
            if(!retained){self.list.remove(row.button.as(gtk.Widget));a.free(row.id);}
        }
        self.rows.deinit(a);self.rows=next;
    }
    pub fn refresh(self:*History) void {
        const completed=string(self.controller.view(),"copy_completed");
        const current=string(field(self.controller.view(),"history_detail"),"operation_id");
        if(completed.len>0 and std.mem.eql(u8,completed,current))self.popup.popdown();
        const view=self.controller.view();const ui=field(view,"history_ui");
        const inputs=field(view,"inputs");const inspecting=boolean(inputs,"visible");
        self.input_controls.as(gtk.Widget).setVisible(@intFromBool(inspecting));
        label(self.input_button,string(inputs,"title"));label(self.sources_button,string(inputs,"sources_label"));label(self.request_button,string(inputs,"request_text_label"));setLabel(self.input_status,string(inputs,"status"));
        self.updating_inputs=true;
        setChoices(self.input_stage,field(inputs,"stages"),"id",string(inputs,"stage"));
        setChoices(self.input_request,field(inputs,"requests"),"key",string(inputs,"selected_key"));
        self.updating_inputs=false;
        if(boolean(inputs,"open_requested") and !self.opening_inputs) {
            if(self.input_editor.as(gtk.Widget).getRoot())|root|if(gobject.ext.cast(gtk.Window,root.as(gobject.Object)))|window| {
                if(window.getApplication())|app|if(app.getActiveWindow()==window and self.input_editor.as(gtk.Widget).getMapped()!=0){
                    self.opening_inputs=true;window.present();self.popup.popup();self.opening_inputs=false;
                };
            };
        }

        label(self.cleanup_button,string(ui,"cleanup"));
        const cleanup=field(view,"cleanup_preview");
        if(self.cleanup_requested and cleanup==.object) {
            self.cleanup_requested=false;
            const root=self.input_editor.as(gtk.Widget).getRoot();
            const parent=if(root)|r| gobject.ext.cast(gtk.Window,r.as(gobject.Object)) else null;
            self.cleanup_confirmation.show(parent,field(cleanup,"presentation"),self,cleanupAccepted);
        }
        const detail_label=a.dupeZ(u8,string(ui,"detail")) catch return;defer a.free(detail_label);self.content.as(gtk.Widget).setTooltipText(detail_label);
        for([_]*gtk.Button{self.original_button,self.refined_button,self.copy_button,self.insert_button,self.retry_button,self.dismiss_button,self.settings_button,self.more_button},[_][]const u8{"original","refined","copy","insert","retry","dismiss","settings","more"})|button,key|label(button,string(ui,key));
        setLabel(self.error_label,string(view,"history_error"));self.error_label.as(gtk.Widget).setVisible(@intFromBool(string(view,"history_error").len>0));
        label(self.retry_open_button,string(ui,"retry_open"));self.retry_open_button.as(gtk.Widget).setVisible(@intFromBool(boolean(view,"history_retry_open")));
        const rows=field(view,"history_rows");const items=if(rows==.array)rows.array.items else &.{};
        var same=items.len==self.rows.items.len;
        if(same)for(items,self.rows.items)|item,row|{if(!std.mem.eql(u8,string(item,"operation_id"),row.id)){same=false;break;}};
        const position=self.scroll.getVadjustment().getValue();
        if(!same)self.reconcileRows(items);
        self.empty.as(gtk.Widget).setVisible(@intFromBool(items.len==0));setLabel(self.empty,string(ui,"empty"));
        for(items,0..)|item,i|{
            if(i>=self.rows.items.len)break;
            const date=if(field(item,"created_at_ms")==.integer)glib.DateTime.newFromUnixLocal(@divTrunc(number(item,"created_at_ms"),1000)) else null;defer if(date)|v|v.unref();
            const stamp=if(date)|v|v.format("%Y-%m-%d %H:%M") else null;defer if(stamp)|v|glib.free(v);
            const duration: u64 = @intCast(@max(0, number(item,"duration_ms")));
            const kind=if(std.mem.eql(u8,string(item,"kind"),"text")) a.dupe(u8,string(ui,"text")) catch continue
                else std.fmt.allocPrint(a,"{d}:{d:0>2}",.{@divTrunc(duration,60000),@mod(@divTrunc(duration,1000),60)}) catch continue;
            defer a.free(kind);
            const metadata=std.fmt.allocPrint(a,"{s} · {s} · {s} · {s}",.{if(stamp)|v|std.mem.span(v) else string(ui,"earlier"),kind,string(item,"mode_label"),string(item,"status")}) catch continue;defer a.free(metadata);
            const row=self.rows.items[i];const preview=string(item,"preview");
            setLabel(row.preview,preview);setLabel(row.metadata,metadata);
            const summary=std.fmt.allocPrint(a,"{s}\n{s}",.{preview,metadata}) catch continue;defer a.free(summary);rowSummary(row.button,summary);
        }
        self.more_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(view,"history_more")));
        const detail=field(view,"history_detail");const id=string(detail,"operation_id");
        const selection_changed=!std.mem.eql(u8,id,self.selected_id orelse "");
        if(selection_changed) {if(self.selected_id)|v|a.free(v);self.selected_id=a.dupe(u8,id) catch null;self.original = !boolean(detail,"can_refined");}
        self.detail.as(gtk.Widget).setVisible(@intFromBool(detail==.object));
        const seconds=@divTrunc(@max(0,number(detail,"duration_ms")),1000);
        const date=if(field(detail,"created_at_ms")==.integer)glib.DateTime.newFromUnixLocal(@divTrunc(number(detail,"created_at_ms"),1000)) else null;defer if(date)|d|d.unref();
        const stamp=if(date)|d|d.format("%Y-%m-%d %H:%M") else null;defer if(stamp)|v|glib.free(v);
        const heading=std.fmt.allocPrint(a,"{s} · {d}:{d:0>2}",.{if(stamp)|v|std.mem.span(v) else string(ui,"earlier"),@divTrunc(seconds,60),@mod(seconds,60)}) catch return;defer a.free(heading);setLabel(self.detail_heading,heading);
        const notice=std.fmt.allocPrint(a,"{s}\n{s}\n{s}",.{string(detail,"notice_title"),string(detail,"notice"),string(detail,"error")}) catch return;defer a.free(notice);setLabel(self.recovery_notice,std.mem.trim(u8,notice,"\n"));
        self.recovery_notice.as(gtk.Widget).setVisible(@intFromBool(!inspecting and std.mem.trim(u8,notice,"\n").len>0));
        self.text_scroll.as(gtk.Widget).setVisible(@intFromBool(inspecting or boolean(detail,"has_transcript")));
        self.copy_button.as(gtk.Widget).setVisible(@intFromBool(inspecting or boolean(detail,"has_transcript")));
        const secondary=a.dupeZ(u8,string(ui,"secondary")) catch return;defer a.free(secondary);self.record_actions.as(gtk.Widget).setTooltipText(secondary);
        for(self.rows.items)|row|{if(std.mem.eql(u8,row.id,id))row.button.as(gtk.Widget).addCssClass("suggested-action") else row.button.as(gtk.Widget).removeCssClass("suggested-action");}
        self.original_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(detail,"can_original")));
        self.refined_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(detail,"can_refined")));
        self.copy_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(detail,"can_copy")));
        self.insert_button.as(gtk.Widget).setVisible(@intFromBool(boolean(detail,"can_recover") or boolean(detail,"can_insert")));
        label(self.retry_button,string(detail,"retry_label"));
        self.retry_button.as(gtk.Widget).setVisible(@intFromBool(boolean(detail,"can_retry")));
        self.dismiss_button.as(gtk.Widget).setVisible(@intFromBool(boolean(detail,"can_dismiss")));
        label(self.delete_button,string(ui,"delete"));self.delete_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(detail,"can_delete")));
        label(self.pin_button,string(detail,"pin_label"));self.pin_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(detail,"can_pin")));
        if(inspecting){for([_]*gtk.Button{self.insert_button,self.retry_button,self.dismiss_button})|b|b.as(gtk.Widget).setVisible(0);}
        self.refreshText();self.scroll.getVadjustment().setValue(position);
        if(selection_changed) {
            self.detail_viewport.getVadjustment().setValue(0);
            for(self.rows.items)|row|if(std.mem.eql(u8,row.id,id)) {
                var x:f64=0;var y:f64=0;
                if(row.button.as(gtk.Widget).translateCoordinates(self.list.as(gtk.Widget),0,0,&x,&y)!=0)
                    self.scroll.getVadjustment().clampPage(y,y+@as(f64,@floatFromInt(row.button.as(gtk.Widget).getHeight())));
                break;
            };
        }
    }
    fn refreshText(self:*History) void {
        const inputs=field(self.controller.view(),"inputs");const inspecting=boolean(inputs,"visible");
        const detail=field(self.controller.view(),"history_detail");const value=if(inspecting)string(inputs,"text") else string(detail,if(self.original) "raw" else "text");
        self.copy_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(detail,if(self.original) "can_copy_original" else "can_copy")));
        if(inspecting){self.copy_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(inputs,"can_copy")));label(self.copy_button,string(inputs,"copy_label"));}
        const buffer=self.content.getBuffer();var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;buffer.getBounds(&first,&last);
        const old=buffer.getText(&first,&last,1);defer glib.free(old);
        if(!std.mem.eql(u8,std.mem.span(old),value)){const z=a.dupeZ(u8,value) catch return;defer a.free(z);buffer.setText(z,@intCast(value.len));}
    }
};
