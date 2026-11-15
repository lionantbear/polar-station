extends "res://depth_forest/spatial_wheel_3d.gd"
## Scene-only adapter. The owning Pane supplies its exact existing model and
## navigation controller; this view never initializes a second history.
signal state_changed
signal instruction_requested
signal reference_requested(reference:Dictionary)
var motion_settings: Dictionary = {}
var _last_host_state: Array = []
var _reader_viewport_size:=Vector2i.ZERO
var _active_size:=Vector2.ZERO

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
		forest.set_render_active(value)
	if not value and initialized:
		_settle_source_transition()
		if detail_sheet.active:detail_sheet._settle()
		dragging_canvas=false
		touch_quiet.stop()
		if _reader_viewport_size==Vector2i.ZERO:_reader_viewport_size=detail_sheet.viewport.size
		detail_sheet.viewport.render_target_update_mode=SubViewport.UPDATE_DISABLED
		detail_sheet.viewport.size=Vector2i(2,2)
	if value and initialized:
		if _reader_viewport_size!=Vector2i.ZERO:
			detail_sheet.viewport.size=_reader_viewport_size;_reader_viewport_size=Vector2i.ZERO
			detail_sheet.viewport.render_target_update_mode=SubViewport.UPDATE_ONCE if detail_sheet.active else SubViewport.UPDATE_DISABLED
		if _active_size!=size:_resize();_active_size=size
		sync_selection()

func set_preferences(value: Dictionary) -> void:
	motion_settings=value.duplicate()
	if forest==null:return
	forest.reduced_motion=bool(value.get("reduce_motion",false)) or not bool(value.get("navigation_animation",true))
	forest.motion_preferences=value.duplicate()
	if forest.reduced_motion:
		_settle_source_transition()
		if detail_sheet.active:detail_sheet._settle()

func snapshot() -> Dictionary:
	if not initialized:return {}
	return {"version":1,"fingerprint":navigation.snapshot_fingerprint,"per_source_views":per_source_views.duplicate(true),"pan":tree_pan,"zoom":tree_zoom,
		"source_id":sources[active_index].id,"reader_open":detail_sheet.active,
		"density":detail_sheet.context_density,"font_delta":detail_sheet.font_delta,
		"detail_offsets":detail_sheet.offsets.duplicate(),"instruction_open":detail_sheet.instruction_open,
		"relationship_focus":forest.relation_mode,"overview":forest.overview_mode,
		"wheel_width":wheel_width,"camera":forest.get_camera_state(),"view_direction":forest.view_direction}

func restore_view(value: Dictionary) -> void:
	if not initialized or int(value.get("version",1))!=1 or value.get("fingerprint",navigation.snapshot_fingerprint)!=navigation.snapshot_fingerprint:return
	wheel_width=clampf(float(value.get("wheel_width",wheel_width)),min_wheel_width,260)
	if value.get("per_source_views") is Dictionary:per_source_views=value.per_source_views.duplicate(true)
	# Selection from the shared controller wins over a stale variant snapshot.
	sync_selection()
	if value.get("source_id")==sources[active_index].id:
		if value.get("pan") is Vector2:tree_pan=value.pan
		tree_zoom=clampf(float(value.get("zoom",1)),.25,4)
		detail_sheet.context_density=clampf(float(value.get("density",1)),.75,1.5)
		detail_sheet.font_delta=clampi(int(value.get("font_delta",0)),-6,12)
		if detail_sheet.active:_close_details()
		_place_tree()
		if bool(value.get("reader_open",false)):
			if value.get("detail_offsets") is Dictionary:detail_sheet.offsets=value.detail_offsets.duplicate()
			show_details();detail_sheet._settle();detail_sheet.change_font(0)
			set_instruction_visible(bool(value.get("instruction_open",false)))
		elif bool(value.get("relationship_focus",false)):_open_relationships()
		elif bool(value.get("overview",false)):forest.focus_overview()
		if forest.overview_mode and value.get("camera") is Dictionary:
			if value.get("view_direction") is Vector3 and value.view_direction.is_finite():forest.view_direction=value.view_direction.normalized()
			forest.restore_camera(value.camera,false)
			_mode_control_pan=tree_pan;_mode_control_zoom=tree_zoom
			forest._place_wheel();_sync_source_controls()
	_trace_state()

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

func replace_context(value,controller)->void:
	var same_document:bool=_forest_built and navigation.snapshot_fingerprint==controller.snapshot_fingerprint and model.ordered_ids==value.ordered_ids and model.source_roots==value.source_roots
	# A source refresh preserves this Pane's view; a new controller owner must
	# start from its own snapshot rather than inheriting the previous Tab.
	if navigation!=controller:
		per_source_views.clear();tree_pan=Vector2.ZERO;tree_zoom=1.0;wheel_width=158
		_mode_control_pan=Vector2.ZERO;_mode_control_zoom=1.0
		detail_sheet.offsets.clear();detail_sheet.font_delta=0;detail_sheet.context_density=1.0
		detail_sheet.instruction_open=false
		forest.relation_mode=false;forest.overview_mode=false
	_settle_source_transition()
	if detail_sheet.active:detail_sheet.close();detail_sheet._settle()
	detail_sheet.context_ids.clear();detail_sheet.focus_id=""
	prepared_model=value;prepared_navigation=controller
	if not _read_sources():return
	if not same_document:
		layouts.clear();layout={};_forest_built=false
	else:
		# Retain immutable geometry/layout and the atlas; only ownership changes.
		forest.input_model=model
		forest.clear_navigation_trace()
	_selection_initialized=false
	inspected_reference=""
	_build_sources();sync_selection();_refresh_selection()
	# Rebinding has already aligned the source to the controller. The next
	# wheel action is a user selection, not the initial scene setup.
	_selection_initialized=true

func supports_context_rebinding()->bool:return true
