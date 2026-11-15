extends Control
## Pane-local source display. Identity/order and selection remain workspace-owned.
## Cylinder projection adapted from GoWheelPicker, gohud54bbdd2912526a94f3cf7faa48d8e5f405bcbd94
## Copyright (c)2026 JaeHo Song, MIT: res://licenses/gohud-wheel-MIT.txt.
## Only sin/cos projection is reused; text is upright, selection immediate and hits use final Controls.
signal source_selected(key:String)
signal reordered(key:String,before:String)
signal width_changed(value:float)
signal geometry_changed
const ROW_HEIGHT:=56.0
const MIN_WIDTH:=104.0
var entries:Array=[]
var current_key:=""
var rows:Dictionary={}
var grip:Control
var drag_active:=false
var _resizing:=false
var _resize_start:=Vector2.ZERO
var _resize_width:=120.0
var _accumulator:=0.0
var _display_position:=0.0
var _snap:Tween
var _hover_key:=""
var _labels:Dictionary={}
var _spacing:=80.0
var _radius:=1.0
var _axis_y:=0.0
const MAX_TURN:=1.05
var motion_enabled:=true
var visual_theme

func apply_visual_theme(value) -> void:
	visual_theme=value
	queue_redraw()
	for row in rows.values():
		for child in row.button.get_children():
			if child is Label:child.add_theme_color_override("font_color",value.text)

func _theme_color(token: String, baseline: Color) -> Color:
	return visual_theme.get(token) if visual_theme!=null and visual_theme.id!="default" else baseline


class InlineGrip extends Button:
	func _draw()->void:
		for x in [size.x-5.0,size.x-2.0]:
			for y in [size.y*.5-4.0,size.y*.5,size.y*.5+4.0]:draw_circle(Vector2(x,y),0.8,Color("a9bbc8"))

func _ready()->void:
	custom_minimum_size.x=120;size_flags_vertical=Control.SIZE_EXPAND_FILL;clip_contents=true
	grip=Control.new();grip.mouse_default_cursor_shape=Control.CURSOR_HSIZE;add_child(grip);grip.gui_input.connect(_resize_input)
	gui_input.connect(_scroll_input);mouse_exited.connect(_sync_hover)
	resized.connect(_arrange);set_process(false);_arrange()

func _get_minimum_size()->Vector2:return Vector2(MIN_WIDTH,0)

func _flat(button:Button)->void:
	button.flat=true
	for state in ["normal","hover","pressed","focus","disabled"]:button.add_theme_stylebox_override(state,StyleBoxEmpty.new())

func set_sources(value:Array,selected:String)->void:
	stop_motion()
	for row:Dictionary in rows.values():row.button.queue_free()
	entries=value;current_key=selected;rows.clear();_labels=_display_labels(entries)
	for entry:Dictionary in entries:
		var key:=str(entry.key);var button:=Button.new();_flat(button);add_child(button)
		button.mouse_default_cursor_shape=Control.CURSOR_POINTING_HAND;button.tooltip_text=str(entry.title)+"\n"+str(entry.source)
		button.pressed.connect(func():source_selected.emit(key))
		button.gui_input.connect(_scroll_input);button.mouse_entered.connect(_sync_hover);button.mouse_exited.connect(_sync_hover)
		button.focus_entered.connect(_sync_hover);button.focus_exited.connect(_sync_hover)
		var title:=Label.new();title.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART;title.max_lines_visible=2;title.vertical_alignment=VERTICAL_ALIGNMENT_CENTER;title.mouse_filter=Control.MOUSE_FILTER_IGNORE;button.add_child(title)
		var handle:=InlineGrip.new();_flat(handle);handle.focus_mode=Control.FOCUS_NONE;handle.tooltip_text="Drag to reorder trees";handle.mouse_default_cursor_shape=Control.CURSOR_DRAG;button.add_child(handle)
		handle.pressed.connect(func():source_selected.emit(key));handle.gui_input.connect(_scroll_input);handle.mouse_entered.connect(_sync_hover);handle.mouse_exited.connect(_sync_hover)
		handle.set_drag_forwarding(func(_p):
			var preview:=Label.new();preview.text=str(_labels[key]);handle.set_drag_preview(preview)
			return {"tree_wheel":get_instance_id(),"key":key},
			func(_p,data):return data is Dictionary and data.get("tree_wheel")==get_instance_id(),
			func(_p,data):reordered.emit(str(data.key),key))
		button.set_drag_forwarding(Callable(),func(_p,data):return data is Dictionary and data.get("tree_wheel")==get_instance_id(),func(_p,data):reordered.emit(str(data.key),key))
		rows[key]={"row":button,"button":button,"title":title,"handle":handle,"font_size":0,"text_width":-1,"scale":1.0,"alpha":1.0,"x_offset":0.0}
	_display_position=float(_selected_index());_arrange();grip.move_to_front()

func select(key:String)->void:
	if not rows.has(key):return
	current_key=key;_retarget(float(_selected_index()))

func _selected_index()->int:
	for i in entries.size():
		if str(entries[i].key)==current_key:return i
	return 0

static func _display_labels(values:Array)->Dictionary:
	var counts:Dictionary={};var candidates:Dictionary={};var aliases:Dictionary={}
	for entry:Dictionary in values:
		var text:=str(entry.title).strip_edges();var first:=text.get_slice(" ",0)
		if text.contains(" "):counts[first]=int(counts.get(first,0))+1
	for entry:Dictionary in values:
		var key:=str(entry.key);var text:=str(entry.title).strip_edges()
		var alias=entry.get("alias","")
		if alias is String and not alias.strip_edges().is_empty():text=alias.strip_edges();aliases[key]=true
		else:
			var first:=text.get_slice(" ",0)
			if text.contains(" ") and int(counts.get(first,0))>1:text=text.trim_prefix(first+" ").strip_edges()
			for separator in ["：",":"]:
				if text.contains(separator) and not text.get_slice(separator,0).strip_edges().is_empty():text=text.get_slice(separator,0).strip_edges();break
		candidates[key]=text
	var duplicates:Dictionary={}
	for text in candidates.values():duplicates[text]=int(duplicates.get(text,0))+1
	for entry:Dictionary in values:
		var key:=str(entry.key)
		if int(duplicates[candidates[key]])>1 and not aliases.has(key):candidates[key]=str(entry.title)
	return candidates

func _fits(text:String,font:Font,font_size:int,width:float)->bool:
	var measured:=font.get_multiline_string_size(text,HORIZONTAL_ALIGNMENT_LEFT,width,font_size)
	return measured.x<=width+0.5 and measured.y<=font.get_height(font_size)*2.0+2.0

func _middle_fit(text:String,font:Font,font_size:int,width:float)->String:
	if _fits(text,font,font_size,width):return text
	var breaks:=TextServerManager.get_primary_interface().string_get_character_breaks(text)
	if breaks.is_empty() or breaks[0]!=0:breaks.insert(0,0)
	if breaks[-1]!=text.length():breaks.append(text.length())
	var clusters:Array[String]=[]
	for i in breaks.size()-1:clusters.append(text.substr(breaks[i],breaks[i+1]-breaks[i]))
	for kept in range(clusters.size()-1,1,-1):
		var left:=ceili(kept*.5);var right:=kept-left
		var candidate:="".join(clusters.slice(0,left))+"…"+"".join(clusters.slice(clusters.size()-right))
		if _fits(candidate,font,font_size,width):return candidate
	return text

func _center_index()->float:
	if entries.is_empty():return 0.0
	if entries.size()<=4:return _display_position
	var reach:float=MAX_TURN*_radius/_spacing
	if float(entries.size()-1)<=reach*2:return float(entries.size()-1)*.5
	return clampf(_display_position,reach,float(entries.size()-1)-reach)

func _arrange()->void:
	if grip==null:return
	grip.position=Vector2(size.x-5,0);grip.size=Vector2(5,size.y)
	# GoWheelPicker's cylindrical mapping, bounded for two upright text lines.
	# A short group uses the same cylinder without stretching across the full rail.
	_radius=maxf(ROW_HEIGHT,(size.y-ROW_HEIGHT-16.0)/(2.0*sin(MAX_TURN)))
	if entries.size()<=4:_radius=_spacing*3.0/MAX_TURN
	var center:=_center_index();var step:=_spacing/_radius;var width:=maxf(1,size.x-5)
	_axis_y=size.y*.5
	if entries.size()<=4:_axis_y-=_radius*(sin(-center*step)+sin((entries.size()-1-center)*step))*.5
	for i in entries.size():
		var key:=str(entries[i].key);var row:Dictionary=rows[key];var turn:float=(float(i)-center)*step
		var face:float=cos(turn);var depth:=clampf((1.0-face)/(1.0-cos(MAX_TURN)),0,1)
		var offset:=22.0*depth;var row_height:=maxf(46,ROW_HEIGHT*face)
		row.scale=face;row.x_offset=offset;row.turn=turn
		row.alpha=1.0 if key==current_key else lerpf(.94,.20,depth)
		row.button.position=Vector2(roundf(offset),roundf(_axis_y+sin(turn)*_radius-row_height*.5))
		row.button.size=Vector2(maxf(1,width-roundf(offset)-roundf(3*depth)),roundf(row_height))
		row.button.scale=Vector2.ONE;row.button.modulate.a=row.alpha;row.button.z_index=roundi(face*100)
		row.title.position=Vector2(6,1);row.title.size=Vector2(row.button.size.x-12,row.button.size.y-2)
		var font_size:=16 if key==current_key else clampi(roundi(14+2*maxf(0,face)*maxf(0,face)),14,16)
		var text_width:=int(row.title.size.x)
		if font_size!=row.font_size or text_width!=row.text_width:
			row.title.add_theme_font_size_override("font_size",font_size);row.title.text=_middle_fit(str(_labels[key]),row.title.get_theme_font("font"),font_size,text_width);row.font_size=font_size;row.text_width=text_width
		row.handle.position=Vector2(row.button.size.x-24,(row.button.size.y-18)*.5);row.handle.size=Vector2(24,18)
		row.button.visible=absf(turn)<=MAX_TURN+.00001 and row.button.position.y+row.button.size.y>=0 and row.button.position.y<=size.y
	_sync_hover();queue_redraw();geometry_changed.emit()

func _sync_hover()->void:
	_hover_key="";var point:=get_local_mouse_position()
	for key in rows:
		var row:Dictionary=rows[key]
		if row.button.visible and row.button.get_rect().has_point(point):_hover_key=str(key)
	for key in rows:rows[key].handle.visible=key==_hover_key or rows[key].button.has_focus()
	queue_redraw()


func _draw()->void:
	draw_rect(Rect2(Vector2.ZERO,size),_theme_color("background",Color("1c222c")))
	if entries.is_empty():return
	# A continuous cylinder silhouette; glyphs remain independent front-facing Controls.
	var rail:=PackedVector2Array()
	for sample in 49:
		var y:float=size.y*sample/48.0;var turn:=asin(clampf((y-_axis_y)/_radius,-sin(MAX_TURN),sin(MAX_TURN)))
		var inset:float=22*(1-cos(turn))/(1-cos(MAX_TURN))
		rail.append(Vector2(2+inset,y))
	draw_polyline(rail,Color(_theme_color("border",Color("70a3b0")),.3),1.0,true)
	for key in rows:
		var row:Dictionary=rows[key]
		if not row.button.visible:continue
		var r:Rect2=row.button.get_rect();var bevel:float=4*(1-row.scale)
		var surface:=PackedVector2Array([r.position+Vector2(bevel,0),Vector2(r.end.x-bevel,r.position.y),r.end-Vector2(0,bevel),Vector2(r.position.x,r.end.y-bevel)])
		draw_colored_polygon(surface,Color(.36,.52,.59,.10*row.alpha))
		draw_line(r.position+Vector2(bevel,0),Vector2(r.end.x-bevel,r.position.y),Color(.54,.70,.74,.16*row.alpha),1.0)
	if rows.has(current_key):
		var band:Rect2=rows[current_key].button.get_rect()
		draw_rect(band,_theme_color("card",Color("29403e")));draw_line(band.position,Vector2(band.end.x,band.position.y),Color(_theme_color("selection",Color("4de0cc")),.5),1)
		draw_line(Vector2(band.position.x,band.end.y),band.end,Color(_theme_color("selection",Color("4de0cc")),.5),1)
		draw_rect(Rect2(band.position,Vector2(3,band.size.y)),_theme_color("selection",Color("4de0cc")))

func _retarget(value:float)->void:
	if _snap!=null:_snap.kill()
	if not motion_enabled or is_equal_approx(_display_position,value):_display_position=value;_arrange();return
	_snap=create_tween().set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
	_snap.tween_method(func(position:float):_display_position=position;_arrange(),_display_position,value,0.16)

func is_animating()->bool:return _snap!=null and _snap.is_running()
func stop_motion()->void:
	if _snap!=null:_snap.kill()
func settle_motion()->void:
	stop_motion();_display_position=float(_selected_index());_arrange()

func _scroll_input(event:InputEvent)->void:
	if event is InputEventMouseButton and event.pressed and event.button_index in [MOUSE_BUTTON_WHEEL_UP,MOUSE_BUTTON_WHEEL_DOWN]:
		_step((1 if event.button_index==MOUSE_BUTTON_WHEEL_DOWN else -1)*event.factor);accept_event()
	elif event is InputEventPanGesture:
		stop_motion();_display_position=clampf(_display_position+event.delta.y*10.0/_spacing,0,maxi(0,entries.size()-1));_arrange()
		var index:=clampi(roundi(_display_position),0,entries.size()-1)
		if not entries.is_empty() and str(entries[index].key)!=current_key:source_selected.emit(str(entries[index].key))
		_retarget(float(_selected_index()));accept_event()

func _step(amount:float)->void:
	_accumulator+=amount;var steps:=int(_accumulator)
	if steps==0 or entries.is_empty():return
	_accumulator-=steps;var next:=clampi(_selected_index()+steps,0,entries.size()-1)
	if str(entries[next].key)!=current_key:source_selected.emit(str(entries[next].key))

func _resize_input(event:InputEvent)->void:
	if event is InputEventMouseButton and event.button_index==MOUSE_BUTTON_LEFT:
		_resizing=event.pressed;_resize_start=get_global_mouse_position();_resize_width=custom_minimum_size.x
	elif event is InputEventMouseMotion and _resizing:
		var width:=clampf(_resize_width+get_global_mouse_position().x-_resize_start.x,MIN_WIDTH,320)
		custom_minimum_size.x=width;width_changed.emit(width)

func _notification(what:int)->void:
	if what==NOTIFICATION_DRAG_BEGIN:
		var data=get_viewport().gui_get_drag_data();drag_active=data is Dictionary and data.get("tree_wheel")==get_instance_id();set_process(drag_active)
	elif what==NOTIFICATION_DRAG_END:
		drag_active=false;set_process(false);_retarget(float(_selected_index()))
func _process(delta:float)->void:
	var point:=get_local_mouse_position()
	if point.x<0 or point.x>size.x:return
	var amount:float=-delta*240.0/_spacing if point.y<40 else (delta*240.0/_spacing if point.y>size.y-40 else 0.0)
	if not is_zero_approx(amount):stop_motion();_display_position=clampf(_display_position+amount,0,maxi(0,entries.size()-1));_arrange()

func review_state()->Dictionary:
	var result:Dictionary={"selected":current_key,"selected_index":_selected_index(),"display_position":_display_position,"center_index":_center_index(),"spacing":_spacing,"row_height":ROW_HEIGHT,"width":size.x,"radius":_radius,"axis_y":_axis_y,"max_turn":MAX_TURN,"animating":is_animating(),"rows":[]}
	for entry:Dictionary in entries:
		var row:Dictionary=rows[str(entry.key)];var r:Rect2=row.title.get_global_rect();var h:Rect2=row.handle.get_global_rect()
		result.rows.append({"key":entry.key,"title":entry.title,"display_text":row.title.text,"title_rect":[r.position.x,r.position.y,r.size.x,r.size.y],"font_size":row.font_size,"scale":row.scale,"alpha":row.alpha,"x_offset":row.x_offset,"handle_visible":row.handle.visible,"handle_rect":[h.position.x,h.position.y,h.size.x,h.size.y],"visible":row.button.visible,"turn":row.turn,"row_rect":[row.button.position.x,row.button.position.y,row.button.size.x,row.button.size.y]})
	return result
