extends "res://depth_forest/spatial_wheel.gd"
const ForestView=preload("res://depth_forest/spatial_wheel_forest.gd")
var forest
var detail_sheet
var per_source_views: Dictionary={}
var _selection_initialized:=false
var capture_stage:=""
var _forest_built:=false
var _placing_camera:=false
var overview_button: Button
var inspected_reference:=""
var _mode_control_pan:=Vector2.ZERO
var _mode_control_zoom:=1.0
var wheel_marks: Control
var touch_quiet
var _touch_owner:=""
var depth_target:=1.0
var touch_previewing:=false
var touch_start_depth:=1.0
var touch_start_pan:=Vector2.ZERO
var touch_start_zoom:=1.0
const TOUCH_DEPTH_UNITS:=18.0

func _ready() -> void:
	# The approved Retina experiment used 232 backing pixels (116 points).
	# Keep that reading footprint at every display scale, not only on Mac.
	wheel_width=116.0
	min_wheel_width=116.0
	touch_quiet=preload("res://addons/polar_graphics/deadline.gd").new(_finish_touch_depth,.18)
	if OS.get_name()=="macOS":
		# macOS 26's reserved PingFangUI system fallback is not loadable by
		# this Godot/FreeType build. Load the installed CJK face directly,
		# before both title measurement and Label3D construction.
		var cjk:=FontFile.new()
		cjk.allow_system_fallback=false
		var loaded:=cjk.load_dynamic_font("/System/Library/Fonts/STHeiti Medium.ttc")
		if loaded==OK and cjk.has_char("树".unicode_at(0)):
			ThemeDB.fallback_font=cjk
			theme=Theme.new();theme.default_font=cjk
			print("SPATIAL_FONT direct Heiti CJK glyphs verified")
		else:push_error("Cannot load the installed CJK font; tree titles would be incomplete")
	super._ready()

func _build_chrome() -> void:
	forest=ForestView.new()
	forest.role="depth_forest"
	forest.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	forest.reduced_motion=OS.get_environment("POLAR_LAB_REDUCE_MOTION")=="1"
	add_child(forest)
	forest.forest_selected.connect(_select_node)
	forest.reference_read_requested.connect(func(id):_select_node(id);show_details())
	forest.endpoint_requested.connect(_locate_endpoint)
	forest.forest_activated.connect(_activate_node)
	wheel_marks=Control.new();wheel_marks.mouse_filter=Control.MOUSE_FILTER_IGNORE
	wheel_marks.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	wheel_marks.draw.connect(_draw_wheel_marks);add_child(wheel_marks)
	super._build_chrome()
	heading.add_theme_font_size_override("font_size",15)
	context_label.add_theme_font_size_override("font_size",16)
	footer.add_theme_font_size_override("font_size",14)
	reader_text.add_theme_font_size_override("normal_font_size",18)
	heading.text="IDEA TREE   /   DEPTH FOREST"
	footer.text="Alt/Option + ↑↓ Depth   ·   ↑↓ Same level   ·   ← Parent   → Child   ·   Enter Details"
	overview_button=Button.new();overview_button.text="Forest overview"
	overview_button.focus_mode=Control.FOCUS_NONE
	overview_button.position=Vector2(400,18);overview_button.pressed.connect(_toggle_overview)
	add_child(overview_button)
	var related:=Button.new();related.text="Selected relationships"
	related.focus_mode=Control.FOCUS_NONE
	related.position=Vector2(555,18);related.pressed.connect(_open_relationships)
	add_child(related)
	for button in [overview_button,related]:
		button.add_theme_font_size_override("font_size",16)
		button.add_theme_color_override("font_color",Color("b9d2da"))
		for state in ["normal","hover","pressed"]:
			var style:=StyleBoxFlat.new()
			style.bg_color=Color("142b36") if state=="hover" else Color("0b202b")
			style.bg_color.a=.88 if state=="hover" else .65
			style.border_color=Color("406572") if state=="hover" else Color("243f4b")
			style.set_border_width_all(1);style.set_corner_radius_all(5)
			style.content_margin_left=12;style.content_margin_right=12
			style.content_margin_top=7;style.content_margin_bottom=7
			button.add_theme_stylebox_override(state,style)
	related.position.x=580
	detail_sheet=load("res://depth_forest/spatial_node_detail.gd").new();forest.world.add_child(detail_sheet);detail_sheet.setup(self)

func _settle_source_transition() -> void:
	if touch_previewing or (transition and transition.is_valid() and transition.is_running()):
		if transition and transition.is_valid():transition.kill()
		touch_previewing=false;touch_quiet.stop();pan_accumulator=0;_touch_owner=""
		wheel_value=depth_target;forest.selector_depth=wheel_value
		var memory: Dictionary=per_source_views.get(str(sources[active_index].id),{})
		tree_pan=memory.get("pan",tree_pan);tree_zoom=float(memory.get("zoom",tree_zoom))
		_place_sources()

func _open_relationships() -> void:
	_settle_source_transition();forest.focus_related()
	_mode_control_pan=tree_pan;_mode_control_zoom=tree_zoom
	overview_button.text="Reading view";_sync_source_controls()
	# Container sorting and wrapped-label minima settle before exposing hit
	# rectangles to the native diagnostic driver. The GUI itself is unchanged.
	await get_tree().process_frame
	await get_tree().process_frame
	_trace_state()

func _toggle_overview() -> void:
	_settle_source_transition()
	if forest.overview_mode:forest.focus_reading_context()
	else:forest.focus_overview()
	_mode_control_pan=tree_pan;_mode_control_zoom=tree_zoom
	overview_button.text="Reading view" if forest.overview_mode else "Forest overview"
	_sync_source_controls()
	_trace_state()

func _sync_source_controls() -> void:
	wheel_marks.queue_redraw()
	for i in plaques.size():
		var rectangle: Rect2=forest.source_rect(i)
		plaques[i].position=rectangle.position;plaques[i].size=rectangle.size
		plaques[i].visible=forest.source_plaques[str(sources[i].id)].visible

func _build_sources() -> void:
	for card in plaques:card.queue_free()
	plaques.clear()
	if not _forest_built:
		forest.source_index=active_index
		forest.set_forest(model,sources)
		_forest_built=true
	else:
		forest.source_order=sources.duplicate()
		# M1c keeps stable source positions; M2 owns actual order remapping.
	for i in sources.size():
		var source: Dictionary=sources[i]
		var button:=Button.new()
		button.focus_mode=Control.FOCUS_NONE
		for state in ["normal","hover","pressed","focus"]:button.add_theme_stylebox_override(state,StyleBoxEmpty.new())
		button.mouse_default_cursor_shape=Control.CURSOR_POINTING_HAND
		add_child(button)
		button.pressed.connect(func():select_source(sources.find(source)))
		button.mouse_entered.connect(func():forest.source_plaques[str(source.id)].pose(true,false,Vector2.ZERO,forest.reduced_motion))
		button.mouse_exited.connect(func():forest.source_plaques[str(source.id)].pose(false,false,Vector2.ZERO,forest.reduced_motion))
		plaques.append(button)
	_place_sources()

func _place_sources() -> void:
	if forest==null or not _forest_built:return
	if detail_sheet and detail_sheet.active:
		forest.source_index=active_index;forest.depth_value=wheel_value;forest._place_wheel();_sync_source_controls();return
	if not _placing_camera:
		_placing_camera=true
		forest.depth_value=wheel_value;forest.source_index=active_index
		forest.forest_pan=tree_pan;forest.forest_zoom=tree_zoom
		if forest.overview_mode:forest._place_wheel()
		else:forest.apply_camera()
		_placing_camera=false
	for i in plaques.size():
		var rectangle: Rect2=forest.source_rect(i)
		plaques[i].position=rectangle.position;plaques[i].size=rectangle.size
		plaques[i].visible=forest.source_plaques[str(sources[i].id)].visible
	width_grip.position=Vector2(wheel_width+51,90);width_grip.size=Vector2(8,size.y-145)
	if wheel_marks:wheel_marks.queue_redraw()
	_trace_state()

func select_source(index: int, animate: bool=true, requested_depth: float=NAN, duration: float=.62, preserve_selection:bool=false, refresh_selection:bool=true) -> void:
	if sources.is_empty():return
	forest.clear_navigation_trace()
	var goal_depth:=requested_depth
	if is_nan(goal_depth):goal_depth=float(index) if not animate else depth_target+wrapf(float(index)-fposmod(depth_target,sources.size()),-sources.size()*.5,sources.size()*.5)
	if not inspected_reference.is_empty():reader.hide()
	inspected_reference=""
	if forest and forest.focus_callout:forest.focus_callout.hide()
	if overview_button:overview_button.text="Forest overview"
	if _selection_initialized and _forest_built and active_index>=0 and active_index<sources.size() and not touch_previewing and not (transition and transition.is_valid() and transition.is_running()):
		per_source_views[str(sources[active_index].id)]={"pan":tree_pan,"zoom":tree_zoom,"selection":navigation.selected_id}
	index=wrapi(index,0,sources.size())
	active_index=index;depth_target=goal_depth;touch_previewing=false
	forest.overview_mode=false;forest.relation_mode=false;forest.relation_overlay.hide()
	var root:=str(sources[index].id)
	var memory: Dictionary=per_source_views.get(root,{"pan":Vector2.ZERO,"zoom":1.0,"selection":root})
	per_source_views[root]=memory.duplicate()
	var start_pan:=tree_pan;var start_zoom:=tree_zoom;var start_depth:=wheel_value
	var goal_pan: Vector2=memory.pan;var goal_zoom:=float(memory.zoom)
	if _selection_initialized and not preserve_selection:navigation.select_node(str(memory.selection))
	_selection_initialized=true
	if transition and transition.is_valid():transition.kill()
	if animate and not forest.reduced_motion:
		transition=create_tween().set_trans(Tween.TRANS_SINE).set_ease(Tween.EASE_IN_OUT)
		transition.tween_method(func(value: float):
			wheel_value=lerpf(start_depth,goal_depth,value)
			forest.selector_depth=wheel_value
			tree_pan=start_pan.lerp(goal_pan,value);tree_zoom=lerpf(start_zoom,goal_zoom,value)
			_place_sources(),0.0,1.0,duration)
		transition.finished.connect(_trace_state)
	else:wheel_value=goal_depth;forest.selector_depth=goal_depth;tree_pan=goal_pan;tree_zoom=goal_zoom
	if refresh_selection:_rebuild_tree()
	_place_sources()
	print("SPATIAL_WHEEL_SELECT index=%d forest_persistent=true" % index)

func step_source(delta: int) -> void:
	if sources.is_empty():return
	select_source(wrapi(active_index+delta,0,sources.size()),true,depth_target+delta)

func wheel_step(delta: int, factor: float=1.0) -> void:
	var amount:=float(delta)*minf(absf(factor),1.0)
	if wheel_accumulator*amount<0:wheel_accumulator=0.0
	wheel_accumulator+=amount
	if absf(wheel_accumulator)>=.999:
		step_source(1 if wheel_accumulator>0 else -1);wheel_accumulator=0

func _rebuild_tree() -> void:
	if not _forest_built:return
	_refresh_selection()

func _refresh_selection() -> void:
	if forest==null or not _forest_built:return
	forest.set_selection(navigation.selected_id)
	var root:=str(sources[active_index].id)
	context_label.text="%s   ·   %d nodes   ·   %d direct relationships" % [str(sources[active_index].title),forest.cached_source_layouts[root].ids.size(),forest.selected_edges.size()]
	if detail_sheet and detail_sheet.active:detail_sheet.present(navigation.selected_id)
	_trace_state()

func _place_tree() -> void:
	if forest==null or not _forest_built:return
	if detail_sheet and detail_sheet.active:return
	if forest.overview_mode:
		# Apply the existing SpatialView camera deltas to the current mode
		# camera. Only an explicit source selection returns to its reading anchor.
		var delta: Vector2=tree_pan-_mode_control_pan
		forest.target+=Vector3(-delta.x,delta.y,0)*.012
		forest.orbit_distance/=maxf(.001,tree_zoom/_mode_control_zoom)
		_mode_control_pan=tree_pan;_mode_control_zoom=tree_zoom
		forest._update_camera();forest._place_wheel()
	else:
		forest.forest_pan=tree_pan;forest.forest_zoom=tree_zoom
		forest.apply_camera()
	for i in plaques.size():
		var rectangle: Rect2=forest.source_rect(i)
		plaques[i].position=rectangle.position;plaques[i].size=rectangle.size
		plaques[i].visible=forest.source_plaques[str(sources[i].id)].visible
	_trace_state()

func _resize() -> void:
	if forest==null or forest.camera==null or not forest.camera.is_inside_tree():return
	var preserve_relation: bool=forest!=null and _forest_built and forest.relation_mode
	var preserve_overview: bool=forest!=null and _forest_built and forest.overview_mode
	super._resize()
	if detail_sheet:
		reader.position=Vector2.ZERO;reader.size=Vector2(detail_sheet.viewport.size)
		if detail_sheet.active:detail_sheet.present(detail_sheet.focus_id,false)
	if preserve_relation:forest.focus_related()
	elif preserve_overview:forest.focus_overview()
	if preserve_overview:
		_mode_control_pan=tree_pan;_mode_control_zoom=tree_zoom;_sync_source_controls();_trace_state()

func _select_node(key: String) -> void:
	if not forest.points.has(key):return
	var previous_detail: String=detail_sheet.focus_id if detail_sheet and detail_sheet.active else ""
	forest.clear_navigation_trace()
	if bool(forest.record_for(key).external):_locate_endpoint(key);return
	if not detail_sheet.active and forest.source_by_node.get(key)!=str(sources[active_index].id):
		_locate_endpoint(key);return
	_activate_node_source(key)
	if not inspected_reference.is_empty():reader.hide()
	inspected_reference="";forest.focus_callout.hide()
	navigation.select_node(key)
	sources[active_index]["selection"]=key
	_refresh_selection()
	if not previous_detail.is_empty() and previous_detail!=key:forest.show_navigation_trace(previous_detail,key)

func _locate_endpoint(id: String) -> void:
	if not forest.points.has(id):return
	_settle_source_transition()
	var previous:String=navigation.selected_id
	var before:Dictionary=_bridge_camera_pose()
	forest.clear_navigation_trace()
	inspected_reference="";reader.hide();forest.focus_callout.hide()
	var focused:=get_viewport().gui_get_focus_owner()
	if focused:focused.release_focus()
	if bool(forest.record_for(id).external):
		# The existing Navigation model deliberately refuses unloaded identities.
		# Inspect the genuine reference geometry without manufacturing a loaded node.
		inspected_reference=id
		forest.relation_mode=false;forest.relation_overlay.hide();forest.overview_mode=true
		forest.fit_keys([id],false);forest.orbit_distance=19.0/(2.0*tan(deg_to_rad(43.0)*.5));forest._update_camera()
		forest.focus_callout.text="Unloaded reference · "+str(forest.record_for(id).uuid)
		forest.focus_callout.position=Vector2(size.x*.5-250,size.y*.5+60);forest.focus_callout.size=Vector2(500,36);forest.focus_callout.add_theme_font_size_override("font_size",15);forest.focus_callout.show()
		forest._place_wheel();_sync_source_controls();_trace_state();return
	var root_id: String=forest.source_by_node.get(id,"")
	for i in sources.size():
		if str(sources[i].id)==root_id:
			if i!=active_index:select_source(i,false,NAN,.62,true,false)
			break
	navigation.locate_node(id)
	var anchor: Vector3=forest.source_offsets[root_id]+Vector3(1.3,0,0)
	var desired: Vector3=forest.points[id]+Vector3(1.3,0,0)
	tree_pan=Vector2((anchor.x-desired.x)/.012,(desired.y-anchor.y)/.012);tree_zoom=1.0
	_place_tree();_refresh_selection()
	forest.show_navigation_trace(previous,id)
	_animate_bridge_navigation(previous,before)

func _bridge_camera_pose()->Dictionary:
	return {"target":forest.target,"size":forest.view_size,"direction":forest.view_direction,"depth":wheel_value}

func _animate_bridge_navigation(previous:String,before:Dictionary)->void:
	# Review-only override: assess the new camera effect separately from lines.
	if OS.get_environment("POLAR_BRIDGE_CAMERA_REVIEW")=="0":return
	if detail_sheet.active or forest.reduced_motion or not bool(forest.motion_preferences.get("navigation_animation",true)):return
	var bridge:Dictionary=forest.bridge_between(previous,navigation.selected_id)
	if bridge.is_empty():return
	var curve:Curve3D=bridge.curve
	var length:=curve.get_baked_length()
	var start:=curve.sample_baked(length if bridge.reverse else 0.0)
	var end:=curve.sample_baked(0.0 if bridge.reverse else length)
	var after:Dictionary=_bridge_camera_pose()
	per_source_views[str(sources[active_index].id)]={"pan":tree_pan,"zoom":tree_zoom,"selection":navigation.selected_id}
	if transition and transition.is_valid():transition.kill()
	transition=create_tween().set_trans(Tween.TRANS_SINE).set_ease(Tween.EASE_IN_OUT)
	transition.tween_method(func(value:float):
		var point:=curve.sample_baked(length*(1.0-value if bridge.reverse else value))
		forest.target=point+(Vector3(before.target)-start).lerp(Vector3(after.target)-end,value)
		forest.view_size=lerpf(before.size,after.size,value)
		forest.view_direction=Vector3(before.direction).slerp(after.direction,value)
		wheel_value=lerpf(before.depth,after.depth,value);forest.depth_value=wheel_value;forest.selector_depth=wheel_value
		forest._update_camera();forest._place_wheel();_sync_source_controls(),0.0,1.0,.38)
	transition.finished.connect(func():wheel_value=depth_target;forest.selector_depth=wheel_value;_place_sources())

func show_details() -> void:
	_settle_source_transition()
	var id: String=inspected_reference if not inspected_reference.is_empty() else navigation.selected_id
	detail_sheet.present(id);_trace_state()

func _activate_node(id:String)->void:
	_select_node(id);show_details()

func _close_details() -> void:
	detail_sheet.close();_trace_state()

func displayed_selection()->String:return navigation.selected_id

func _input(event: InputEvent) -> void:
	if not initialized or not is_visible_in_tree():return
	if event is InputEventMouseMotion:forest.port_pointer_moved(event.position)
	var owner := get_viewport().gui_get_focus_owner()
	if owner is LineEdit or owner is TextEdit or (owner!=null and owner!=self and not is_ancestor_of(owner)):return
	if detail_sheet and detail_sheet.handle_pointer(event):
		get_viewport().set_input_as_handled();_trace_state();return
	if detail_sheet and detail_sheet.active:
		# Detail gestures belong to the reader/group under the pointer, not to
		# the obscured browsing canvas or its remembered pan/depth state.
		if event is InputEventPanGesture or event is InputEventMagnifyGesture or (event is InputEventMouseButton and event.button_index in [MOUSE_BUTTON_RIGHT,MOUSE_BUTTON_WHEEL_UP,MOUSE_BUTTON_WHEEL_DOWN]):
			get_viewport().set_input_as_handled();return
	if event is InputEventPanGesture:
		_handle_touch_pan(event);get_viewport().set_input_as_handled();return
	var focused:=get_viewport().gui_get_focus_owner()
	if event is InputEventKey and event.pressed and event.keycode in [KEY_ENTER,KEY_KP_ENTER,KEY_ESCAPE] and reader.visible:
		if not event.echo:
			if event.keycode==KEY_ESCAPE:_close_details()
			elif has_signal("instruction_requested"):emit_signal("instruction_requested")
		get_viewport().set_input_as_handled();_trace_state();return
	if event is InputEventKey and event.pressed and not event.echo and event.keycode in [KEY_ENTER,KEY_SPACE] and focused is Button and (forest.port_popup.is_ancestor_of(focused) or forest.port_handles.values().any(func(handle):return handle.hit==focused)):
		focused.pressed.emit();get_viewport().set_input_as_handled();return
	if event is InputEventKey and event.pressed and event.keycode==KEY_ESCAPE:
		if forest.port_popup.visible:
			forest._hide_port_popup();get_viewport().set_input_as_handled();return
		forest.clear_navigation_trace()
	if event is InputEventKey and event.pressed and event.alt_pressed and event.keycode in [KEY_UP,KEY_DOWN]:
		if not (focused is LineEdit or focused is TextEdit):
			step_source(-1 if event.keycode==KEY_UP else 1)
		get_viewport().set_input_as_handled();return
	# Consume navigation once, including native key repeat. GUI focus traversal
	# must not independently reinterpret the same arrows after node selection.
	if event is InputEventKey and event.pressed and event.keycode in [KEY_UP,KEY_DOWN,KEY_LEFT,KEY_RIGHT,KEY_SPACE]:
		if event.keycode!=KEY_SPACE or not event.echo:_keyboard_node(event.keycode)
		get_viewport().set_input_as_handled();_trace_state();return
	super._input(event)
	if event is InputEventKey:_trace_state()

func _handle_touch_pan(event: InputEventPanGesture) -> void:
	if bool(event.get_meta("scroll_started",false)):_finish_touch_depth()
	var owner:="canvas" if event.position.x>=wheel_width+65 and not event.alt_pressed else "depth"
	if not _touch_owner.is_empty():owner=_touch_owner
	if owner=="depth" and (bool(event.get_meta("scroll_momentum",false)) or bool(event.get_meta("scroll_ended",false))):
		if touch_previewing:_finish_touch_depth()
		# Retain the existing owner through the native momentum tail, until its
		# quiet boundary or the next native begin. No velocity model or polling.
		_touch_owner="depth";touch_quiet.start();return
	if _touch_owner.is_empty():
		if owner=="canvas":_settle_source_transition()
		else:
			if not touch_previewing and not (transition and transition.is_valid() and transition.is_running()):
				per_source_views[str(sources[active_index].id)]={"pan":tree_pan,"zoom":tree_zoom,"selection":navigation.selected_id}
			touch_start_depth=wheel_value;touch_start_pan=tree_pan;touch_start_zoom=tree_zoom
			if transition and transition.is_valid():transition.kill()
			touch_previewing=true
		_touch_owner=owner
	touch_quiet.start()
	if _touch_owner=="canvas":
		tree_pan+=preload("res://addons/polar_graphics/gesture_motion.gd").content_pixels(event.delta);_place_tree();return
	pan_accumulator+=event.delta.y
	var preview:=touch_start_depth+pan_accumulator/TOUCH_DEPTH_UNITS
	var nearest:=roundf(preview)
	if wrapi(roundi(preview),0,sources.size())!=active_index:
		# Commit identities only when crossing the next layer centre; keep
		# the fractional pose under the fingers instead of starting a Tween.
		select_source(wrapi(roundi(preview),0,sources.size()),false,nearest)
		touch_previewing=true
	depth_target=nearest
	wheel_value=nearest if forest.reduced_motion else preview
	forest.selector_depth=wheel_value
	var low:=int(floor(preview));var fraction:=preview-floorf(preview)
	var a:Dictionary=per_source_views.get(str(sources[wrapi(low,0,sources.size())].id),{"pan":Vector2.ZERO,"zoom":1.0})
	var b:Dictionary=per_source_views.get(str(sources[wrapi(low+1,0,sources.size())].id),{"pan":Vector2.ZERO,"zoom":1.0})
	tree_pan=Vector2(a.pan).lerp(b.pan,fraction);tree_zoom=lerpf(float(a.zoom),float(b.zoom),fraction)
	_place_sources()

func _finish_touch_depth() -> void:
	var settle:=_touch_owner=="depth" and touch_previewing
	_touch_owner="";pan_accumulator=0;touch_quiet.stop()
	if settle:select_source(active_index,true,depth_target,.22)

func _navigation_parents(_id:String)->Array:
	return detail_sheet.band_ids.parents.filter(func(candidate):return candidate in detail_sheet.roles.parents) if detail_sheet.active else model.parents.get(_id,[])

func _keyboard_node(keycode: Key) -> void:
	_settle_source_transition()
	var id: String=navigation.selected_id
	var before:Dictionary=_bridge_camera_pose()
	var direction:String={KEY_SPACE:"toggle",KEY_LEFT:"left",KEY_RIGHT:"right",KEY_UP:"up",KEY_DOWN:"down"}.get(keycode,"")
	var parents:Array=_navigation_parents(id)
	var peers:Array=detail_sheet.band_ids.level if detail_sheet.active else forest.visual_level(id)
	if not navigation.navigate_visible(direction,peers,parents,forest.visual_children(id)):return
	if direction=="toggle":return
	_activate_node_source(navigation.selected_id)
	inspected_reference="";forest.focus_callout.hide()
	sources[active_index]["selection"]=navigation.selected_id
	_refresh_selection()
	# Keep the current reading scale. Only pan when the destination leaves the
	# safe canvas; never refit a whole tree (or switch DAG parent) on every key.
	if forest.overview_mode:
		forest.overview_mode=false;forest.relation_mode=false
		forest.relation_overlay.hide();overview_button.text="Forest overview"
		_place_tree()
	if not detail_sheet.active:
		var same_layer:bool=forest.source_by_node.get(id)==forest.source_by_node.get(navigation.selected_id)
		tree_pan+=forest.ensure_reading_visible([navigation.selected_id,id] if same_layer else [navigation.selected_id])
	_place_tree()
	forest.show_navigation_trace(id,navigation.selected_id)
	_animate_bridge_navigation(id,before)

func _activate_node_source(id: String) -> void:
	var source: String=forest.source_by_node.get(id,"")
	for index in sources.size():
		if str(sources[index].id)==source:
			# The caller submits the final node selection after changing sources.
			if index!=active_index:select_source(index,false,NAN,.62,true,false)
			return

func _zoom_at(point: Vector2, factor: float) -> void:
	var before: Vector3=forest._plane_point(point)
	tree_zoom=clampf(tree_zoom*factor,.25,4.0)
	_place_tree()
	var after: Vector3=forest._plane_point(point)
	var difference:=before-after
	tree_pan+=Vector2(-difference.x,difference.y)/.012
	_place_tree()

func needs_continuous_frames() -> bool:
	return super.needs_continuous_frames() or (detail_sheet != null and detail_sheet.animating()) or (forest != null and forest.needs_frames())

func _draw() -> void:
	pass

func _draw_wheel_marks() -> void:
	if forest==null or not _forest_built:return
	if detail_sheet and detail_sheet.active and detail_sheet.compact_wheel:return
	# Small depth graduations connect the projected source plaques visually;
	# no opaque sidebar, enclosing rectangle or false graph relationship.
	var center:=Vector2(14,size.y*.5)
	for tick in range(-18,19):
		var angle:=float(tick)/18.0
		var point:=center+Vector2(7*(1.0-angle*angle),angle*size.y*.40)
		var length:=7.0 if tick%3==0 else 3.0
		wheel_marks.draw_line(point,point+Vector2(length,0),Color(.40,.65,.72,.22 if tick%3==0 else .10),1.0,true)
	var rectangle: Rect2=forest.source_rect(active_index)
	var color: Color=sources[active_index].accent
	var marker:=Vector2(17,rectangle.get_center().y)
	wheel_marks.draw_circle(marker,3.0,Color(color,.85))
	wheel_marks.draw_line(Vector2(rectangle.position.x+8,rectangle.end.y+7),Vector2(rectangle.end.x-8,rectangle.end.y+7),Color(color,.42),1.0,true)

func _trace_state() -> void:
	if forest==null or not _forest_built or sources.is_empty():return
	var path:=OS.get_environment("POLAR_LAB_STATE_PATH")
	if path.is_empty():return
	var state: Dictionary={"stage":capture_stage,"ready":initialized,"active_index":active_index,"wheel_value":wheel_value,"active_source":sources[active_index].id,"selected_id":navigation.selected_id,"zoom":tree_zoom,"pan":[tree_pan.x,tree_pan.y],"size":[size.x,size.y],"continuous_frames":needs_continuous_frames(),"sources":[],"nodes":[],"reader_visible":reader.visible,"reader_text":reader_text.text if reader.visible else "","controls":{},"forest_membership":forest.points.size(),"model_membership":model.records.size(),"projected_membership":forest.projected_node_count,"typed_slots":model.relation_edges.size(),"rendered_relation_membership":forest.edge_paths.size(),"visible_membership":forest.visible_nodes.size(),"camera_projection":forest.camera.projection,"camera_position":[forest.camera.position.x,forest.camera.position.y,forest.camera.position.z],"in_frustum_membership":0,"readable_membership":0,"camera_size":forest.camera.size,"direct_relations":forest.selected_edges.size(),"overview_mode":forest.overview_mode,"boundary_hints":forest.boundary_hints,"last_route_error":forest.last_route_error,"relation_mode":forest.relation_mode,"relation_annotations":[],"inspected_reference":inspected_reference}
	for child in get_children():
		if child is Button and child.text=="Selected relationships":
			var point: Vector2=child.get_global_rect().get_center();state.controls["relationships"]=[point.x,point.y]
	if detail_sheet and detail_sheet.active:
		var box: Rect2=detail_sheet._reader_rect()
		state["detail_context"]={"roles":detail_sheet.roles,"peers":detail_sheet.peers,"shown":detail_sheet.shown,"offsets":detail_sheet.offsets,"focus":detail_sheet.focus_id,"bands":detail_sheet.band_ids,"zoom":detail_sheet.detail_zoom,"reader_rect":[box.position.x,box.position.y,box.size.x,box.size.y]}
	for annotation in forest.relation_annotations:
		var button: Button=annotation.button
		state.relation_annotations.append({"target":annotation.target,"external":bool(forest.record_for(annotation.target).external),"from":annotation.relation.from,"to":annotation.relation.to,"slot":annotation.relation.slot,"font_size":annotation.font_size,"point":[forest.screen_point(annotation.target).x,forest.screen_point(annotation.target).y],"label_rect":[button.global_position.x,button.global_position.y,button.size.x,button.size.y],"visible":button.is_visible_in_tree()})
	for i in sources.size():
		var rectangle: Rect2=forest.source_rect(i)
		state.sources.append({"index":i,"id":sources[i].id,"point":[rectangle.get_center().x,rectangle.get_center().y],"visible":plaques[i].visible if plaques.size()>i else true})
	for id in forest.points:
		var point: Vector2=forest.screen_point(id)
		var p: Vector3=forest.points[id]
		var in_frustum: bool=forest.camera.is_position_in_frustum(p)
		if in_frustum:state.in_frustum_membership+=1
		if in_frustum and forest.labels[id].visible:state.readable_membership+=1
		state.nodes.append({"in_frustum":in_frustum,"label_visible":forest.labels[id].visible,"id":id,"key":forest.record_for(id).key,"point":[point.x,point.y],"world":[p.x,p.y,p.z],"source":forest.record_for(id).source,"sha256":forest.record_for(id).source_sha256,"visible":forest.plaques_3d[id].visible,"font_px":forest.reading_font_pixels(id),"scale":[forest.plaques_3d[id].scale.x,forest.plaques_3d[id].scale.y,forest.plaques_3d[id].scale.z]})
	if forest.plaques_3d.has(navigation.selected_id):
		var skin: Node3D=forest.plaques_3d[navigation.selected_id].skin
		state["selected_skin_pose"]={"position":[skin.position.x,skin.position.y,skin.position.z],"rotation":[skin.rotation.x,skin.rotation.y,skin.rotation.z]}
	var temporary:=path+".tmp"
	var file:=FileAccess.open(temporary,FileAccess.WRITE)
	if file:
		file.store_string(JSON.stringify(state));file.close()
		DirAccess.rename_absolute(temporary,path)
