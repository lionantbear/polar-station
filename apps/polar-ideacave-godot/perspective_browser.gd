extends "res://depth_forest/browser.gd"
## Experimental composition over the production Godot scene. Business roots,
## directed browsing parents and partitions are supplied by shared Rust.
signal perspective_requested(index:int)
signal granularity_requested(level:int)
var projection:Dictionary={}
var full_graph_projection:Dictionary={}
var perspectives:Array=[]
var perspective_index:=1
var perspective_value:=1.0
var perspective_cards:Dictionary={}
var perspective_hits:Array[Button]=[]
var perspective_animation:Tween
var independent_map:Control
var perspective_label:Label
var group_label:Label
var grain_label:Label
var _axis:=""
var _gesture_region:=""
var _gesture_amount:=0.0
var _horizontal_amount:=0.0
var _drag_region:=""
var _drag_distance:=Vector2.ZERO
var _axis_deadline
var _filter_matches:Dictionary={}
var _filter_active:=false
var _revealed:=""
var requested_granularity:=0
var display_cursor:=""

func _ready()->void:
	focus_mode=Control.FOCUS_ALL
	requested_granularity=int(projection.level)
	super._ready()
	_place_perspectives()

func _read_sources()->bool:
	model=prepared_model;navigation=prepared_navigation;sources.clear()
	sources.assign(sources_for(model,projection))
	active_index=0
	for i in sources.size():
		if str(projection.membership.get(navigation.selected_id,""))==str(sources[i].id):active_index=i
	wheel_value=float(active_index);depth_target=wheel_value
	return not sources.is_empty()

static func sources_for(value,next:Dictionary)->Array:
	var result:Array=[]
	for i in next.groups.size():
		var group:Dictionary=next.groups[i]
		result.append({"id":str(group.id),"title":str(value.records[str(group.id)].title),"accent":[Color("e8b96b"),Color("58dcd0"),Color("b59aff"),Color("f38d83"),Color("80b8ff"),Color("c0df77"),Color("ed91cd"),Color("e7df88")][i%8],"glyph":i%4})
	return result

func _build_chrome()->void:
	super._build_chrome()
	_axis_deadline=preload("res://addons/polar_graphics/deadline.gd").new(_end_axis,.18)
	perspective_label=_label("PERSPECTIVE",14,Color("a6c8d7"))
	group_label=_label("GROUP",14,Color("a6c8d7"))
	grain_label=_label("",13,Color("87aebf"))
	_build_perspective_controls()

func set_perspectives(values:Array)->void:
	if perspectives==values:return
	if perspective_animation and perspective_animation.is_valid():perspective_animation.kill()
	for card in perspective_cards.values():card.free()
	for button in perspective_hits:button.free()
	perspective_cards.clear();perspective_hits.clear()
	perspectives=values
	_build_perspective_controls()

func _build_perspective_controls()->void:
	for i in perspectives.size():
		var item:Dictionary=perspectives[i]
		var plaque=load("res://addons/polar_graphics/depth/spatial_plaque_mesh.gd").new()
		plaque.dimensions=Vector2(2.1,.50);plaque.continuous_context_materials=true
		plaque.title=str(item.title);plaque.glyph_kind=i%4;plaque.accent=Color("78d8df")
		forest.world.add_child(plaque);forest._track_card(plaque)
		plaque.title_label.pixel_size=.0041;plaque.title_label.font_size=60;plaque.title_label.width=463;plaque.title_label.position.x=0;plaque.glyph_label.hide()
		perspective_cards[str(item.id)]=plaque
		var button:=Button.new();button.focus_mode=Control.FOCUS_ALL
		for state in ["normal","hover","pressed","focus"]:button.add_theme_stylebox_override(state,StyleBoxEmpty.new())
		button.mouse_default_cursor_shape=Control.CURSOR_POINTING_HAND
		button.pressed.connect(func():perspective_requested.emit(i))
		add_child(button);perspective_hits.append(button)
	perspective_value=float(perspective_index)

func _build_sources()->void:
	forest.definition_projection=full_graph_projection
	forest.group_assignment=model.display_membership
	forest.group_members.clear()
	for group in model.display_groups:forest.group_members[str(group)]=model.display_groups[group]
	# set_forest retains/reuses node meshes; geometry cache is keyed by the
	# exact source revision and measured group geometry, never by camera zoom.
	super._build_sources()
	for button in plaques:button.focus_mode=Control.FOCUS_ALL

func _upper_region()->Rect2:
	return Rect2(13,42,wheel_width,maxf(140,size.y*.37-42))
func _lower_region()->Rect2:
	var top:=maxf(192,size.y*.41)
	return Rect2(13,top+45,wheel_width,maxf(80,size.y-top-70))
func _pointer_region(position_value:Vector2)->String:
	if position_value.x<0 or position_value.x>wheel_width+55:return ""
	return "perspective" if position_value.y<_lower_region().position.y-25 else "group"
func _sync_source_controls()->void:
	super._sync_source_controls()
	_place_perspectives()
func _place_sources()->void:
	if forest!=null:forest.wheel_region=_lower_region()
	super._place_sources();_place_perspectives()
func _place_tree()->void:
	super._place_tree();_place_perspectives()
func select_source(index:int,animate:bool=true,requested_depth:float=NAN,duration:float=.62,preserve_selection:bool=false,refresh_selection:bool=true)->void:
	display_cursor=""
	if not sources.is_empty():
		var group_id:String=str(sources[wrapi(index,0,sources.size())].id)
		var memory:Dictionary=per_source_views.get(group_id,{})
		if not memory.is_empty() and str(projection.membership.get(str(memory.get("selection","")),""))!=group_id:
			memory=memory.duplicate();memory["selection"]=group_id;per_source_views[group_id]=memory
	super.select_source(index,animate,requested_depth,duration,preserve_selection,refresh_selection)
func _resize()->void:
	if forest!=null:forest.wheel_region=_lower_region()
	super._resize();_place_perspectives()
func _place_perspectives()->void:
	if forest==null or perspective_cards.is_empty() or forest.camera==null:return
	forest.place_plaque_wheel(perspectives,perspective_cards,perspective_index,perspective_value,_upper_region(),5.0,20.0)
	for i in perspectives.size():
		var card=perspective_cards[str(perspectives[i].id)]
		var rectangle:=Rect2()
		for corner in [Vector3(-1.05,-.41,0),Vector3(1.05,.41,0)]:
			var point:Vector2=forest.camera.unproject_position(card.skin.to_global(corner))
			rectangle=Rect2(point,Vector2.ZERO) if rectangle==Rect2() else rectangle.expand(point)
		perspective_hits[i].position=rectangle.position;perspective_hits[i].size=rectangle.size
		perspective_hits[i].visible=card.visible
	perspective_label.position=Vector2(13,14)
	group_label.position=Vector2(13,_lower_region().position.y-41)
	grain_label.position=Vector2(13,_lower_region().position.y-20)
	grain_label.text="Coarse ‹ %d › Fine" % int(projection.level)

func update_projection(value,controller,next:Dictionary)->void:
	display_cursor=""
	var focus:=get_viewport().gui_get_focus_owner()
	var group_focused:bool=focus is Button and focus in plaques
	# A Tab transfer changes the navigation owner, not the partition. Let the
	# shared browser retain its atlas/meshes when both data revision and layout
	# are unchanged; its replace_context checks the source fingerprint too.
	_forest_built=_forest_built and projection==next
	projection=next
	requested_granularity=int(next.level)
	forest.group_assignment=value.display_membership
	forest.group_members.clear()
	for group in value.display_groups:forest.group_members[str(group)]=value.display_groups[group]
	var previous_width:=wheel_width
	replace_context(value,controller)
	wheel_width=previous_width;_place_sources()
	_place_perspectives();_apply_filter()
	if group_focused and not plaques.is_empty():plaques[active_index].grab_focus()
func select_perspective(index:int,animate:bool=true)->void:
	perspective_index=wrapi(index,0,perspectives.size())
	var goal:=perspective_value+wrapf(float(perspective_index)-fposmod(perspective_value,perspectives.size()),-perspectives.size()*.5,perspectives.size()*.5)
	if perspective_animation and perspective_animation.is_valid():perspective_animation.kill()
	if animate:
		perspective_animation=create_tween().set_trans(Tween.TRANS_SINE).set_ease(Tween.EASE_OUT)
		perspective_animation.tween_method(func(value:float):perspective_value=value;_place_perspectives(),perspective_value,goal,.24)
	else:perspective_value=goal;_place_perspectives()

func _refresh_selection()->void:
	if forest==null or not _forest_built:return
	var id:String=_cursor()
	forest.set_selection(id)
	if detail_sheet and detail_sheet.active:detail_sheet.present(id)
	_apply_filter();_trace_state()

func _cursor()->String:
	return display_cursor if not display_cursor.is_empty() and model.records.has(display_cursor) and model.canonical(display_cursor)==navigation.selected_id else navigation.selected_id

func displayed_selection()->String:return _cursor()

func show_details()->void:
	detail_sheet.present(_cursor());_trace_state()

func _activate_node(id:String)->void:
	if not model.group_reference_for(id).is_empty():
		# Close the current reader before selecting another occurrence; otherwise
		# a double-click would prepare an unseen reader layout only to close it.
		if detail_sheet.active:_close_details();detail_sheet._settle()
		_select_node(id);_follow_reference()
	else:_select_node(id);show_details()

func sync_selection()->void:
	if not initialized:return
	if not model.group_reference_for(_cursor()).is_empty():_refresh_selection();return
	display_cursor="";super.sync_selection()

func _select_node(id:String)->void:
	if not forest.plaques_3d.has(id):return
	var reference:Dictionary=model.group_reference_for(id)
	if reference.is_empty():display_cursor="";super._select_node(id);return
	display_cursor=id
	navigation.select_node(model.canonical(id))
	inspected_reference="";_refresh_selection();state_changed.emit()

func _keyboard_node(code:Key)->void:
	_settle_source_transition()
	if not detail_sheet.active:detail_sheet._settle()
	if forest.overview_mode or forest.relation_mode:
		forest.overview_mode=false;forest.relation_mode=false;forest.relation_overlay.hide();_place_tree()
	var id:String=_cursor();var target:=""
	if code==KEY_SPACE:
		if model.group_reference_for(id).is_empty():navigation.navigate_visible("toggle")
		elif not detail_sheet.active:show_details()
		return
	if code==KEY_LEFT:
		var parents:Array=model.parents.get(id,[])
		if not parents.is_empty():target=str(parents[0])
	elif code==KEY_RIGHT:
		var children:Array=forest.visual_children(id)
		if not children.is_empty():target=str(children[0].to)
	else:
		var peers:Array=detail_sheet.band_ids.level if detail_sheet.active else forest.visual_level(id)
		var at:int=peers.find(id);var next:int=at+(-1 if code==KEY_UP else 1)
		if at>=0 and next>=0 and next<peers.size():target=str(peers[next])
	if target.is_empty() or str(model.display_membership.get(target,""))!=str(sources[active_index].id):return
	_select_node(target)
	if not detail_sheet.active:
		tree_pan+=forest.ensure_reading_visible([target,id]);_place_tree()
	forest.show_navigation_trace(id,target)

func _follow_reference()->bool:
	var reference:Dictionary=model.group_reference_for(_cursor())
	if reference.is_empty():return false
	for i in sources.size():
		if str(sources[i].id)!=str(reference.target_group):continue
		var node:String=str(reference.node);display_cursor=""
		select_source(i,true,NAN,.22,true,false)
		navigation.select_node(node);_refresh_selection()
		return true
	return false

func execute_action(action:String)->bool:
	if action=="read" and not detail_sheet.active and _follow_reference():return true
	return super.execute_action(action)

func browser_state()->Dictionary:
	var value:Dictionary=super.browser_state()
	value["selected_group_reference"]=initialized and not model.group_reference_for(_cursor()).is_empty()
	return value

func debug_state()->Dictionary:
	var value:Dictionary=super.debug_state()
	var rectangle:Rect2=forest._card_screen_rect(_cursor()) if forest.points.has(_cursor()) else Rect2()
	value["selection_rect"]=[rectangle.position.x,rectangle.position.y,rectangle.size.x,rectangle.size.y]
	if independent_map!=null:
		var map_rect:Rect2=independent_map.get_rect()
		value["map_rect"]=[map_rect.position.x,map_rect.position.y,map_rect.size.x,map_rect.size.y]
	value["display_cursor"]=_cursor()
	value["reference_target_group"]=str(model.group_reference_for(_cursor()).get("target_group",""))
	value["reference_card_count"]=forest.plaques_3d.keys().filter(func(id):return not model.group_reference_for(str(id)).is_empty()).size()
	value["canonical_card_count"]=forest.plaques_3d.size()-int(value.reference_card_count)
	value["prepared_route_builds"]=forest.route_build_count
	value["reference_actions"]={}
	for id in forest.reference_affordances:
		var entry:Dictionary=forest.reference_affordances[id]
		if entry.jump.visible:
			value.reference_actions[id]={}
			for action in ["jump","read"]:
				var rect:Rect2=entry[action].get_rect()
				value.reference_actions[id][action]=[rect.position.x,rect.position.y,rect.size.x,rect.size.y]
	value["detail_actions"]={}
	if detail_sheet.active:
		for item in [["edit",detail_sheet.instruction_button],["close",detail_sheet.close_button]]:
			var rect:Rect2=item[1].get_rect()
			value.detail_actions[item[0]]=[rect.position.x,rect.position.y,rect.size.x,rect.size.y]
	return value

func snapshot()->Dictionary:
	var value:Dictionary=super.snapshot();value["display_cursor"]=_cursor();return value

func restore_view(value:Dictionary)->void:
	var cursor:String=str(value.get("display_cursor",""))
	var reference:Dictionary=model.group_reference_for(cursor)
	if not reference.is_empty() and model.canonical(cursor)==navigation.selected_id and str(value.get("source_id",""))==str(reference.group):
		for i in sources.size():
			if str(sources[i].id)==str(reference.group):select_source(i,false,NAN,.22,true,false);break
	display_cursor=cursor;super.restore_view(value)

func set_filter(matches:Array,active:bool,reveal:String="")->void:
	_filter_matches.clear()
	for id in matches:_filter_matches[str(id)]=true
	_filter_active=active;_revealed=reveal;_apply_filter()
func _apply_filter()->void:
	if forest==null:return
	var context:Dictionary={}
	if _filter_active:
		for id in _filter_matches:
			var parent_id:String=id
			while projection.parents.has(parent_id):
				parent_id=str(projection.parents[parent_id].from);context[parent_id]=true
	var dimming:Dictionary={}
	if _filter_active:
		for id in forest.plaques_3d:
			if model.group_reference_for(str(id)).is_empty() and not _filter_matches.has(id) and not context.has(id) and id!=navigation.selected_id and id!=_revealed:dimming[id]=.86
	forest.emphasis_dimming=dimming
	forest.refresh_context_tones()

func _end_axis()->void:
	_axis="";_gesture_region="";_gesture_amount=0;_horizontal_amount=0
func _step_granularity(delta:int)->void:
	requested_granularity=clampi(requested_granularity+delta,0,int(projection.max_level))
	granularity_requested.emit(requested_granularity)
func _input(event:InputEvent)->void:
	if event is InputEventMouse or event is InputEventGesture:
		event=event.duplicate();event.position-=global_position
	if not initialized or not is_visible_in_tree():return
	if event is InputEventKey and event.pressed and event.keycode in [KEY_ENTER,KEY_KP_ENTER] and not detail_sheet.active:
		var focused:=get_viewport().gui_get_focus_owner()
		if (focused==null or focused==self or focused==forest) and _follow_reference():
			get_viewport().set_input_as_handled();return
	if event is InputEventMouse or event is InputEventGesture:
		if independent_map!=null and _gesture_region.is_empty() and _drag_region.is_empty() and not dragging_canvas:
			if independent_map.dragging or independent_map.get_global_rect().has_point(event.position+global_position):return
	var focus:=get_viewport().gui_get_focus_owner()
	if focus is LineEdit or focus is TextEdit or (focus!=null and focus!=self and not is_ancestor_of(focus)):return
	if event is InputEventKey and event.pressed and focus is Button and focus in perspective_hits:
		if event.keycode in [KEY_UP,KEY_DOWN]:perspective_requested.emit(perspective_index+(-1 if event.keycode==KEY_UP else 1));get_viewport().set_input_as_handled();return
	if event is InputEventKey and event.pressed and focus is Button and focus in plaques:
		if event.keycode in [KEY_UP,KEY_DOWN,KEY_LEFT,KEY_RIGHT]:
			if event.keycode in [KEY_UP,KEY_DOWN]:step_source(-1 if event.keycode==KEY_UP else 1)
			else:_step_granularity(-1 if event.keycode==KEY_LEFT else 1)
			get_viewport().set_input_as_handled();return
	if event is InputEventPanGesture:
		if bool(event.get_meta("scroll_started",false)):_finish_touch_depth();_end_axis()
		var region:=_pointer_region(event.position)
		if _gesture_region.is_empty() and _touch_owner.is_empty() and not region.is_empty():_gesture_region=region
		if not _gesture_region.is_empty():
			_axis_deadline.start()
			if _axis.is_empty():_axis="horizontal" if absf(event.delta.x)>absf(event.delta.y) and _gesture_region=="group" else "vertical"
			if _axis=="horizontal":
				_horizontal_amount+=event.delta.x
				if absf(_horizontal_amount)>=6.0:
					_step_granularity(1 if _horizontal_amount>0 else -1);_horizontal_amount=0
			elif _gesture_region=="perspective":
				_gesture_amount+=event.delta.y
				if absf(_gesture_amount)>=6.0:perspective_requested.emit(perspective_index+(1 if _gesture_amount>0 else -1));_gesture_amount=0
			else:super._handle_touch_pan(event)
			get_viewport().set_input_as_handled();return
	if event is InputEventMouseButton:
		var region:=_pointer_region(event.position)
		if event.pressed and event.button_index in [MOUSE_BUTTON_WHEEL_UP,MOUSE_BUTTON_WHEEL_DOWN] and not region.is_empty():
			if region=="perspective":perspective_requested.emit(perspective_index+(-1 if event.button_index==MOUSE_BUTTON_WHEEL_UP else 1))
			else:wheel_step(-1 if event.button_index==MOUSE_BUTTON_WHEEL_UP else 1,event.factor)
			get_viewport().set_input_as_handled();return
		if event.button_index==MOUSE_BUTTON_LEFT:
			if event.pressed and region=="group":_drag_region=region;_drag_distance=Vector2.ZERO
			elif not event.pressed:_drag_region=""
	if event is InputEventMouseMotion and _drag_region=="group" and event.button_mask&MOUSE_BUTTON_MASK_LEFT:
		_drag_distance+=event.relative
		if absf(_drag_distance.x)>48 and absf(_drag_distance.x)>absf(_drag_distance.y):
			_step_granularity(1 if _drag_distance.x>0 else -1);_drag_distance=Vector2.ZERO
		get_viewport().set_input_as_handled();return
	super._input(event)

func _navigation_parents(id:String)->Array:
	if detail_sheet.active:return super._navigation_parents(id)
	return [str(projection.parents[id].from)] if projection.parents.has(id) else []
