extends "res://view_packages/sdk.gd"
## Example independent scheme. Model is copied; selection uses the intent port.
var records:Dictionary={}
var ids:Array=[]
var visual
func _ready()->void:
	super._ready()
	var data:Dictionary=context.snapshot();records=data.records
	for id in data.ordered_ids:
		if not bool(records[id].external):ids.append(id)
func reader_rect()->Rect2:
	return Rect2(24,48,minf(540,size.x-48),minf(320,size.y*.55)) if _reading else Rect2()
func apply_visual_theme(value:Resource)->void:visual=value;queue_redraw()
func _draw()->void:
	if not initialized:return
	var font:=ThemeDB.fallback_font
	var accent:Color=visual.selection if visual!=null else Color("ad85ff")
	var selected:String=context.selection().id
	draw_string(font,Vector2(24,30),"Aurora · "+str(records.size())+" nodes",HORIZONTAL_ALIGNMENT_LEFT,-1,20,accent)
	if _reading:
		var rect:=reader_rect()
		draw_style_box(_panel(accent),rect)
		draw_multiline_string(font,rect.position+Vector2(18,34),str(records.get(selected,{}).get("title","")),HORIZONTAL_ALIGNMENT_LEFT,rect.size.x-36,24)
	var y:float=reader_rect().end.y+32 if _reading else 70
	for id in ids:
		if y>size.y-20:break
		var box:=Rect2(24,y, minf(540,size.x-48),36)
		if id==selected:draw_style_box(_panel(accent),box)
		draw_string(font,box.position+Vector2(12,25),str(records[id].title),HORIZONTAL_ALIGNMENT_LEFT,box.size.x-24,18,Color.WHITE if id==selected else Color("bdafcc"))
		y+=42
func _panel(accent:Color)->StyleBoxFlat:
	var style:=StyleBoxFlat.new();style.bg_color=Color("20162e");style.border_color=accent;style.set_border_width_all(1);style.set_corner_radius_all(10);return style
func _gui_input(event:InputEvent)->void:
	if event is InputEventMouseButton and event.pressed and event.button_index==MOUSE_BUTTON_LEFT:
		var start:float=reader_rect().end.y+32 if _reading else 70
		var at:=floori((event.position.y-start)/42)
		if at>=0 and at<ids.size():context.select_node(ids[at]);accept_event()
