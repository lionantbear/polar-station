extends "res://perspective_runtime.gd"
## Thin Station host port. No file loading, composer, focus handler or second
## navigation history: source/model, selection, editing and refresh stay native.
signal state_changed
signal instruction_requested
signal reference_requested(reference:Dictionary)
var initialized:=false
var _host_model
var _host_navigation
var _active:=true
var _initial_state:Dictionary={}
var _pending_commit:Array=[]
var _preferences:Dictionary={}
var _visual_theme:Resource
var _map_source
var _map_fingerprint:=""
var _map_ids:Array=[]
var _pending_restore:Dictionary={}
var _pending_reader:=false
var _pending_editor:=-1
var _configuration_revision:=-1
var forest:
	get:return browser.forest if browser!=null else null
var detail_sheet:
	get:return browser.detail_sheet if browser!=null else null

func attach(value,controller)->void:
	_host_model=value;_host_navigation=controller
func prepare_view_state(value:Dictionary)->void:_initial_state=value.duplicate(true)
func _ready()->void:
	native_completion=true
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	column=VBoxContainer.new();column.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT);add_child(column)
	_configure_host()
func _configuration()->String:
	var configuration=preload("res://browsing_configuration.gd").shared(get_tree())
	_configuration_revision=int(configuration.current().revision)
	return configuration.content()
func refresh_configuration()->void:
	var configuration=preload("res://browsing_configuration.gd").shared(get_tree())
	if not _active or int(configuration.current().revision)==_configuration_revision:return
	var state:Dictionary=snapshot()
	_initial_state=state.duplicate(true)
	_configure_host()
	restore_view(state)
func _configure_host()->void:
	_request_generation+=1;_pending_geometry={};_pending_commit=[];_desired_view={}
	models.clear();_geometry_cache.clear();_memories.clear();levels.clear();_filter_by_perspective.clear()
	current_navigation=null
	if not configure_view(_host_model,_configuration(),_host_navigation):initialized=false;return
	var desired:String=str(_initial_state.get("perspective_id","perspective-development"))
	var index:=0
	for i in profiles.size():
		if str(profiles[i].id)==desired:index=i;break
	levels[str(profiles[index].id)]=int(_initial_state.get("group_level",1))
	current_perspective=index
	# This is the already selected Pane view, not an explicit user switch.
	# Present its saved perspective before any selection fallback is applied.
	initialized=true
	switch_perspective(index)
	if browser!=null:_wire_browser()
func _wire_browser()->void:
	if not browser.instruction_requested.is_connected(_instruction):browser.instruction_requested.connect(_instruction)
	if not browser.reference_requested.is_connected(_reference):browser.reference_requested.connect(_reference)
	browser.set_preferences(_preferences)
func _instruction()->void:instruction_requested.emit()
func _reference(value:Dictionary)->void:reference_requested.emit(value)
func _selection_changed()->void:
	super._selection_changed()
	if initialized:state_changed.emit()
func _commit_projection(index:int,next:Dictionary,prepared,started:int)->bool:
	if not _active:_pending_commit=[index,next,prepared,started];return true
	return super._commit_projection(index,next,prepared,started)
func _after_projection_commit()->void:
	_wire_browser()
	if _map_fingerprint!=shared_navigation.snapshot_fingerprint or _map_ids!=original.ordered_ids:
		_build_map();map_label.hide()
		_map_fingerprint=shared_navigation.snapshot_fingerprint;_map_ids=original.ordered_ids.duplicate()
	_map_source=original
	if not _pending_restore.is_empty():
		var state:=_pending_restore;_pending_restore={};browser.restore_view(state)
	if _pending_reader:browser.show_details();_pending_reader=false
	if _pending_editor>=0:browser.set_instruction_visible(_pending_editor==1);_pending_editor=-1
	if _visual_theme!=null:preload("res://itree_theme_adapter.gd")._apply_node(browser,_visual_theme)
	if map_view!=null:map_view.set_selection(current_navigation.selected_id)
	state_changed.emit()
func replace_context(value,controller)->void:
	_host_model=value;_host_navigation=controller
	_configure_host()
func supports_context_rebinding()->bool:return true
func perspective_catalog()->Array:return profiles
func sync_selection()->void:
	if not initialized or browser==null:return
	browser.sync_selection();_selection_changed()
func browser_state()->Dictionary:
	return browser.browser_state() if browser!=null else {"reader_open":false,"relationship_focus":false,"canvas_mode":"layered","tilted":true}
func reader_rect()->Rect2:return browser.reader_rect() if browser!=null else Rect2()
func editor_rect()->Rect2:return browser.editor_rect() if browser!=null else Rect2()
func set_instruction_visible(value:bool)->void:
	if current_navigation!=shared_navigation or current_model==null or current_model.original!=original:_pending_editor=1 if value else 0;return
	if browser!=null:browser.set_instruction_visible(value)
func show_details()->void:
	if current_navigation!=shared_navigation or current_model==null or current_model.original!=original:_pending_reader=true;return
	if browser!=null and browser.forest.points.has(current_navigation.selected_id):browser.show_details()
func resize_view()->void:
	if browser!=null:browser.resize_view();_place_map()
func execute_action(action:String)->bool:
	if browser==null:return false
	if action in ["canvas_two_d","canvas_layered","tilt"]:
		browser.forest.reading_direction=Vector3(0,0,1) if action=="canvas_two_d" else Vector3(.12,.08,1).normalized()
		browser._place_sources();state_changed.emit();return true
	return browser.execute_action(action)
func set_preferences(value:Dictionary)->void:
	_preferences=value.duplicate()
	if browser!=null:browser.set_preferences(value)
	if map_view!=null:map_view.motion_preferences=value.duplicate()
func apply_visual_theme(value:Resource)->void:_visual_theme=value
func set_active(value:bool)->void:
	_active=value;visible=value
	if value and initialized:refresh_configuration()
	if value and not _pending_commit.is_empty():
		var pending:=_pending_commit;_pending_commit=[]
		_commit_projection(pending[0],pending[1],pending[2],pending[3])
	if browser!=null:browser.set_active(value)
	if map_view!=null:map_view.set_render_active(value)
func needs_continuous_frames()->bool:
	return _active and browser!=null and (browser.needs_continuous_frames() or (map_view!=null and map_view.needs_frames()))
func snapshot()->Dictionary:
	if browser==null:return {}
	var value:Dictionary=browser.snapshot()
	value["perspective_id"]=str(profiles[current_perspective].id)
	value["group_level"]=int(projection.level)
	value["perspective_version"]=1
	return value
func restore_view(value:Dictionary)->void:
	if not initialized or browser==null or int(value.get("perspective_version",0))!=1:return
	var id:String=str(value.get("perspective_id",""))
	if not profiles.any(func(profile):return str(profile.id)==id):return
	if current_navigation!=shared_navigation or current_model==null or current_model.original!=original:_pending_restore=value.duplicate(true);return
	_initial_state=value.duplicate(true)
	for i in profiles.size():
		if str(profiles[i].id)!=id:continue
		levels[id]=int(value.get("group_level",1))
		if i!=current_perspective or int(projection.level)!=levels[id]:
			_pending_restore=value.duplicate(true);switch_perspective(i);return
		break
	browser.restore_view(value)
func debug_state()->Dictionary:
	var value:Dictionary=browser.debug_state() if browser!=null else {}
	value["perspective_id"]=str(profiles[current_perspective].id) if not profiles.is_empty() else ""
	value["group_level"]=int(projection.get("level",0))
	value["configuration_revision"]=_configuration_revision
	value["filter_id"]=filter_id
	value["perspective_count"]=profiles.size()
	value["projection_ready"]=current_model!=null and current_model.original==original and current_navigation==shared_navigation
	value["group_count"]=projection.get("groups",[]).size()
	value["group_id"]=str(browser.sources[browser.active_index].id) if browser!=null else ""
	return value
func _update_status()->void:pass
