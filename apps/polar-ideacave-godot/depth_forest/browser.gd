extends "res://depth_forest/spatial_wheel_3d.gd"
## Scene-only adapter. The owning Pane supplies its exact existing model and
## navigation controller; this view never initializes a second history.
signal state_changed
signal instruction_requested
signal reference_requested(reference:Dictionary)
var motion_settings: Dictionary = {}
var _last_host_state: Array = []

func _ready() -> void:
	super._ready()
	if not initialized:return
	# Native composer owns actions; the wheel and spatial content fill the Pane.
	for control in get_children():
		if control is Button and control not in plaques:control.hide()
	for control in [heading,context_label,footer]:control.hide()
	width_grip.hide()
	sync_selection()

func sync_selection() -> void:
	if not initialized or not forest.points.has(navigation.selected_id):return
	var source: String=forest.source_by_node.get(navigation.selected_id,"")
	for i in sources.size():
		if sources[i].id==source and i!=active_index:
			if transition and transition.is_valid():transition.kill()
			active_index=i;depth_target=float(i);wheel_value=float(i);forest.selector_depth=float(i)
			var memory: Dictionary=per_source_views.get(source,{})
			tree_pan=memory.get("pan",Vector2.ZERO);tree_zoom=float(memory.get("zoom",1.0))
			_place_sources()
	if forest.selected_id!=navigation.selected_id:_refresh_selection()
	_trace_state()

func set_active(value: bool) -> void:
	visible=value
	process_mode=Node.PROCESS_MODE_INHERIT if value else Node.PROCESS_MODE_DISABLED
	set_process_input(value)
	if forest!=null:
		forest.viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE if value else SubViewport.UPDATE_DISABLED
	if not value and initialized:
		_settle_source_transition()
		if detail_sheet.active:detail_sheet._settle()
		dragging_canvas=false
		touch_quiet.stop()
	if value and initialized:
		_resize();sync_selection()

func set_preferences(value: Dictionary) -> void:
	motion_settings=value.duplicate()
	if forest==null:return
	forest.reduced_motion=bool(value.get("reduce_motion",false)) or not bool(value.get("navigation_animation",true))
	forest.motion_preferences=value.duplicate()
	forest.update_connection_labels()
	if forest.reduced_motion:
		_settle_source_transition()
		if detail_sheet.active:detail_sheet._settle()

func snapshot() -> Dictionary:
	if not initialized:return {}
	return {"version":1,"fingerprint":navigation.snapshot_fingerprint,"per_source_views":per_source_views.duplicate(true),"pan":tree_pan,"zoom":tree_zoom,
		"source_id":sources[active_index].id,"reader_open":detail_sheet.active,
		"density":detail_sheet.context_density,"font_delta":detail_sheet.font_delta}

func restore_view(value: Dictionary) -> void:
	if not initialized or int(value.get("version",1))!=1 or value.get("fingerprint",navigation.snapshot_fingerprint)!=navigation.snapshot_fingerprint:return
	if value.get("per_source_views") is Dictionary:per_source_views=value.per_source_views.duplicate(true)
	# Selection from the shared controller wins over a stale variant snapshot.
	sync_selection()
	if value.get("source_id")==sources[active_index].id:
		if value.get("pan") is Vector2:tree_pan=value.pan
		tree_zoom=clampf(float(value.get("zoom",1)),.25,4)
		detail_sheet.context_density=clampf(float(value.get("density",1)),.75,1.5)
		detail_sheet.font_delta=clampi(int(value.get("font_delta",0)),-6,12)
		detail_sheet.change_font(0)
		_place_tree()

func _trace_state() -> void:
	super._trace_state()
	if not initialized:return
	var next: Array=[navigation.selected_occurrence,detail_sheet.active,forest.relation_mode,forest.overview_mode,detail_sheet.instruction_open,detail_sheet.instruction_editor_rect,detail_sheet._reader_rect() if detail_sheet.active else Rect2()]
	if next!=_last_host_state:
		_last_host_state=next
		state_changed.emit()

func needs_continuous_frames() -> bool:
	return visible and process_mode!=Node.PROCESS_MODE_DISABLED and super.needs_continuous_frames()

func attach(value, controller) -> void:
	prepared_model=value;prepared_navigation=controller

func browser_state() -> Dictionary:
	return {"reader_open":detail_sheet.active,"relationship_focus":forest.relation_mode,"canvas_mode":"layered","tilted":true}

func reader_rect() -> Rect2:
	return detail_sheet._reader_rect() if detail_sheet.active else Rect2()

func editor_rect() -> Rect2:
	return detail_sheet.instruction_editor_rect if detail_sheet.active and detail_sheet.instruction_open else Rect2()

func set_instruction_visible(value:bool) -> void:
	if value and not detail_sheet.active:show_details()
	detail_sheet.set_instruction_open(value)
	if value:detail_sheet._settle()

func resize_view() -> void:
	_resize()

func execute_action(action:String) -> bool:
	match action:
		"canvas_two_d","canvas_layered","tilt":return true
		"read":
			if detail_sheet.active:_close_details()
			else:show_details()
		"close_reader":_close_details()
		"locate":forest.focus_reading_context()
		"fit":_toggle_overview()
		"relationship_focus","focus_this_node","focus_branch":_open_relationships()
		_:return false
	return true

func debug_state()->Dictionary:
	return {"pan":[tree_pan.x,tree_pan.y]}

func _locate_endpoint(id:String)->void:
	var record:Dictionary=forest.record_for(id)
	if bool(record.get("external",false)) and record.get("reference") is Dictionary:
		reference_requested.emit(record.reference)
	super._locate_endpoint(id)
