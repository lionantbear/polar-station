extends Control
## Minimal browser contract. Authors override drawing/layout, keep common intents.
signal state_changed
signal instruction_requested
var initialized:=false
var context
var preferences:Dictionary={}
var _reading:=false
var _instruction:=false
func attach_context(value)->void:
	if context!=null:
		if context.selection_changed.is_connected(sync_selection):context.selection_changed.disconnect(sync_selection)
		if context.snapshot_changed.is_connected(refresh_snapshot):context.snapshot_changed.disconnect(refresh_snapshot)
	context=value
	context.selection_changed.connect(sync_selection)
	context.snapshot_changed.connect(refresh_snapshot)
func _ready()->void:
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	initialized=context!=null
func browser_state()->Dictionary:
	return {"reader_open":_reading,"relationship_focus":false,"canvas_mode":"two_d","tilted":false}
func reader_rect()->Rect2:return Rect2()
func editor_rect()->Rect2:
	if not _reading or not _instruction:return Rect2()
	var rect:=reader_rect()
	var width:=clampf(size.x-rect.end.x-24,240,360)
	if rect.end.x+width+24<=size.x:return Rect2(rect.end.x+12,rect.position.y,width,rect.size.y)
	return Rect2(rect.position.x,rect.end.y+12,rect.size.x,maxf(100,size.y-rect.end.y-24))
func set_instruction_visible(value:bool)->void:
	_instruction=value
	if value:show_details()
	state_changed.emit()
func execute_action(action:String)->bool:
	match action:
		"read":_reading=not _reading
		"close_reader":_reading=false;_instruction=false
		_:return false
	sync_selection();return true
func resize_view()->void:queue_redraw();state_changed.emit()
func debug_state()->Dictionary:return {}
func set_active(value:bool)->void:
	visible=value;process_mode=Node.PROCESS_MODE_INHERIT if value else Node.PROCESS_MODE_DISABLED
func set_preferences(value:Dictionary)->void:preferences=value.duplicate(true);queue_redraw()
func snapshot()->Dictionary:return {"version":1,"reader_open":_reading}
func restore_view(value:Dictionary)->void:
	_reading=bool(value.get("reader_open",false));_instruction=false
	sync_selection()
func sync_selection()->void:queue_redraw();state_changed.emit()
func show_details()->void:_reading=true;sync_selection()
func needs_continuous_frames()->bool:return false
func _unhandled_key_input(event:InputEvent)->void:
	if not visible or not event is InputEventKey or not event.pressed:return
	match event.keycode:
		KEY_ENTER:execute_action("read")
		KEY_ESCAPE:execute_action("close_reader")
		KEY_UP:context.navigate("up")
		KEY_DOWN:context.navigate("down")
		KEY_LEFT:context.navigate("left")
		KEY_RIGHT:context.navigate("right")
		KEY_SPACE:context.navigate("toggle")
		_:return
	get_viewport().set_input_as_handled()

func refresh_snapshot()->void:
	sync_selection();queue_redraw()

# Packages that retain copied model records must explicitly implement their
# refresh before opting into reuse by another Pane.
func supports_context_rebinding()->bool:return false
