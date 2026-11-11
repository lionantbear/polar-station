//! Stable native reader for the shared Dictate history projection.
const std = @import("std");
const gtk = @import("gtk");
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
    content: *gtk.TextView,
    local_shortcut_label:*gtk.Label,
    global_shortcut_label:*gtk.Label,
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
        root.as(gtk.Widget).setSizeRequest(360, -1);
        root.append(gtk.Label.new("Dictate history").as(gtk.Widget));
        const local_shortcut=gtk.Label.new("");local_shortcut.setWrap(1);local_shortcut.setMaxWidthChars(42);local_shortcut.setXalign(0);root.append(local_shortcut.as(gtk.Widget));
        const global_shortcut=gtk.Label.new("");global_shortcut.setWrap(1);global_shortcut.setMaxWidthChars(42);global_shortcut.setXalign(0);root.append(global_shortcut.as(gtk.Widget));
        const errors = gtk.Label.new(""); errors.setWrap(1); errors.setXalign(0); root.append(errors.as(gtk.Widget));
        const retry_open=gtk.Button.newWithLabel(""); root.append(retry_open.as(gtk.Widget));
        const list = gtk.Box.new(.vertical, 4);
        const scroll = gtk.ScrolledWindow.new(); scroll.setPolicy(.never,.automatic);
        scroll.setMinContentHeight(100); scroll.setMaxContentHeight(160); scroll.setPropagateNaturalHeight(1);
        scroll.setChild(list.as(gtk.Widget)); root.append(scroll.as(gtk.Widget));
        const empty = gtk.Label.new(""); _=empty.as(gobject.Object).refSink(); list.append(empty.as(gtk.Widget));
        const more = gtk.Button.newWithLabel(""); root.append(more.as(gtk.Widget));
        const detail = gtk.Box.new(.vertical, 6);
        const variants = gtk.Box.new(.horizontal, 6);
        const original = gtk.Button.newWithLabel(""); const refined = gtk.Button.newWithLabel("");
        variants.append(original.as(gtk.Widget)); variants.append(refined.as(gtk.Widget)); detail.append(variants.as(gtk.Widget));
        const text = gtk.TextView.new(); text.setEditable(0); text.setCursorVisible(0); text.setWrapMode(.word_char);
        const text_scroll = gtk.ScrolledWindow.new(); text_scroll.setPolicy(.never,.automatic);
        text_scroll.setMinContentHeight(140); text_scroll.setMaxContentHeight(140);
        text_scroll.setChild(text.as(gtk.Widget)); detail.append(text_scroll.as(gtk.Widget));
        const actions = gtk.Box.new(.horizontal,6);
        const copy = gtk.Button.newWithLabel(""); const insert = gtk.Button.newWithLabel(""); const retry = gtk.Button.newWithLabel("");
        actions.append(copy.as(gtk.Widget)); actions.append(insert.as(gtk.Widget)); actions.append(retry.as(gtk.Widget));
        const dismiss=gtk.Button.newWithLabel("");actions.append(dismiss.as(gtk.Widget));
        detail.append(actions.as(gtk.Widget));
        const maintenance=gtk.Box.new(.horizontal,6);const pin=gtk.Button.newWithLabel("");const delete=gtk.Button.newWithLabel("");maintenance.append(pin.as(gtk.Widget));maintenance.append(delete.as(gtk.Widget));detail.append(maintenance.as(gtk.Widget));root.append(detail.as(gtk.Widget));
        root.append(gtk.Separator.new(.horizontal).as(gtk.Widget));
        const cleanup=gtk.Button.newWithLabel("");root.append(cleanup.as(gtk.Widget));
        const settings = gtk.Button.newWithLabel(""); root.append(settings.as(gtk.Widget)); popup.setChild(root.as(gtk.Widget));
        self.* = .{ .controller=controller,.input_editor=input_editor,.popup=popup,.list=list,.scroll=scroll,.detail=detail,.content=text,.local_shortcut_label=local_shortcut,.global_shortcut_label=global_shortcut,.error_label=errors,.retry_open_button=retry_open,.empty=empty,
            .original_button=original,.refined_button=refined,.copy_button=copy,.insert_button=insert,.retry_button=retry,.dismiss_button=dismiss,.settings_button=settings,.cleanup_button=cleanup,.delete_button=delete,.pin_button=pin,.more_button=more };
        _ = gtk.Widget.signals.map.connect(popup.as(gtk.Widget),*History,opened,self,.{});
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
    fn opened(_: *gtk.Widget,self:*History) callconv(.c) void { self.controller.refreshHistory();self.refresh(); }
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
    fn showOriginal(_: *gtk.Button,self:*History) callconv(.c) void {self.original=true;self.refreshText();}
    fn showRefined(_: *gtk.Button,self:*History) callconv(.c) void {self.original=false;self.refreshText();}
    fn copyText(_: *gtk.Button,self:*History) callconv(.c) void {
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
        setLabel(self.local_shortcut_label,"Inside Polar Station: Ctrl+Win — tap to keep recording; hold and release or press again to stop");
        label(self.cleanup_button,string(ui,"cleanup"));
        const cleanup=field(view,"cleanup_preview");
        if(self.cleanup_requested and cleanup==.object) {
            self.cleanup_requested=false;
            const root=self.input_editor.as(gtk.Widget).getRoot();
            const parent=if(root)|r| gobject.ext.cast(gtk.Window,r.as(gobject.Object)) else null;
            self.cleanup_confirmation.show(parent,field(cleanup,"presentation"),self,cleanupAccepted);
        }
        const presentation=self.controller.presentation();
        const global=std.fmt.allocPrint(a,"Global: {s} · {s}",.{string(presentation,"shortcut_hint"),string(presentation,"shortcut_status")}) catch return;defer a.free(global);
        setLabel(self.global_shortcut_label,global);
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
        self.more_button.as(gtk.Widget).setVisible(@intFromBool(boolean(view,"history_more")));
        const detail=field(view,"history_detail");const id=string(detail,"operation_id");
        if(!std.mem.eql(u8,id,self.selected_id orelse "")) {if(self.selected_id)|v|a.free(v);self.selected_id=a.dupe(u8,id) catch null;self.original = !boolean(detail,"can_refined");}
        self.detail.as(gtk.Widget).setVisible(@intFromBool(detail==.object));
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
        self.refreshText();self.scroll.getVadjustment().setValue(position);
    }
    fn refreshText(self:*History) void {
        const detail=field(self.controller.view(),"history_detail");const value=string(detail,if(self.original) "raw" else "text");
        self.copy_button.as(gtk.Widget).setSensitive(@intFromBool(boolean(detail,if(self.original) "can_copy_original" else "can_copy")));
        const buffer=self.content.getBuffer();var first:gtk.TextIter=undefined;var last:gtk.TextIter=undefined;buffer.getBounds(&first,&last);
        const old=buffer.getText(&first,&last,1);defer glib.free(old);
        if(!std.mem.eql(u8,std.mem.span(old),value)){const z=a.dupeZ(u8,value) catch return;defer a.free(z);buffer.setText(z,@intCast(value.len));}
    }
};
