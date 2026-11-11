extends Control

## Application assembly only: identity, navigation, layout and geometry each
## have independent modules. Views emit intentions, never call other views.
signal host_state_changed(state: Dictionary)
signal host_open_source(path:String)
signal host_focus_search

const ViewModel = preload("res://itree_view_model.gd")
const Navigation = preload("res://itree_navigation_state.gd")
const TreeProjection = preload("res://itree_tree_projection.gd")
const GraphProjection = preload("res://itree_graph_projection.gd")
const SpatialView = preload("res://itree_spatial_view.gd")
const PresentationTheme = preload("res://addons/polar_graphics/presentation_theme.gd")
const DEFAULT_PREFERENCES := {"canvas_mode": "two_d", "navigation_animation": true,
	"frame_endpoints": true, "path_feedback": true, "camera_inertia": true,
	"node_elasticity": true, "reduce_motion": false, "framing_policy":"reading_priority", "connection_flow":true}
var preferences: Dictionary = DEFAULT_PREFERENCES.duplicate()

var browser_id := "mind_map" # Initial assembly; restored/default browser is applied before ready.
var _instruction_open:=false
var _instruction_classic_rect:=Rect2()
var instruction_request_id := 0
var theme_id := "default"
var depth_browser
static var _window_presenters:Dictionary={} # Weak workspace owners; scene lifetime stays in SceneTree.
var _content_column:VBoxContainer
var _classic_roots:Array[Node]=[]
var _active_package_browser := ""
var view_registry=preload("res://view_packages/registry.gd").new()
var browsing_configuration
var browser_views: Dictionary = {}
var _variant_switching := false
var model = ViewModel.new()
var tree_strip
var tree_entries:Array=[]
var tree_order:Array=[]
var tree_views:Dictionary={}
var active_tree:=""
var _switching_tree:=false
var _tree_scroll:=0.0
var focus_anchor:=""
var _focus_return:Dictionary={}
var _focus_layout:Dictionary={}
var _source_base_layout:Dictionary={}
var _pending_browse_path:Array=[]
var _focus_headers:Dictionary={}
var prepared_model
var prepared_layout: Dictionary = {}
var source_paths := PackedStringArray()
var source_entry_paths := PackedStringArray()
var source_loader

var _source_presented_count := 0
var _source_events: Node
var _source_quiet
var _initializing:=true
var _events=ClassDB.instantiate("PolarGraphicsEvents")
var _semantic_event:int
var source_refresh_count := 0
var _source_reload_failed := false
var file_dialog: FileDialog
const SOURCE_STATE := "user://idea-tree-sources.cfg"
var navigation = Navigation.new()
var saved_view_state := ConfigFile.new()

func _view_state_path() -> String:
	return "user://idea-tree-view-" + str(get_meta("view_id", "standalone")).sha256_text() + ".cfg"

func _save_view_state() -> void:
	if not initialized: return
	_remember_tree()
	if depth_browser!=null:browser_views[_active_package_browser]=depth_browser.snapshot()
	saved_view_state.set_value("view","browser_views",browser_views)
	saved_view_state.set_value("view","browser_version",1)
	saved_view_state.set_value("view","theme_version",1)
	saved_view_state.set_value("view","tree_order",tree_order)
	saved_view_state.set_value("view","tree_views",tree_views)
	saved_view_state.set_value("view","active_tree",active_tree)
	saved_view_state.set_value("view","tree_width",tree_strip.custom_minimum_size.x if tree_strip!=null else 120.0)
	saved_view_state.set_value("view", "paths", source_entry_paths if not source_entry_paths.is_empty() else source_paths)
	saved_view_state.set_value("view", "loaded_paths", source_paths)
	saved_view_state.set_value("view", "navigation", navigation.snapshot())
	saved_view_state.set_value("view", "map_sizes", map_sizes)
	saved_view_state.set_value("view", "preferences", preferences)
	saved_view_state.save(_view_state_path())

func _restore_preferences() -> void:
	preferences = DEFAULT_PREFERENCES.duplicate()
	var saved = saved_view_state.get_value("view", "preferences", {})
	if saved is Dictionary and not saved.is_empty():
		for key in DEFAULT_PREFERENCES:
			if key == "canvas_mode":
				if saved.get(key) in ["two_d", "layered"]: preferences[key] = saved[key]
			elif key=="framing_policy":
				if saved.get(key) in ["reading_priority","both_endpoints","fixed_zoom"]:preferences[key]=saved[key]
			elif saved.get(key) is bool: preferences[key] = saved[key]
	else:
		var previous = saved_view_state.get_value("view", "navigation", {})
		if previous is Dictionary:
			var camera_state = previous.get("camera_states", {}).get("canvas", {})
			if camera_state.has("tilted"): preferences.canvas_mode = "layered" if camera_state.tilted else "two_d"
	_apply_preferences()

func _apply_preferences(animate_mode: bool = false) -> void:
	if depth_browser!=null:depth_browser.set_preferences(preferences)
	if tree_strip!=null:
		tree_strip.motion_enabled=bool(preferences.navigation_animation) and not bool(preferences.reduce_motion)
		if not tree_strip.motion_enabled:tree_strip.settle_motion()
	for view in [canvas, tree_view, graph_view]:
		if view==null:continue
		view.motion_preferences = preferences.duplicate()
		if bool(preferences.reduce_motion) or not bool(preferences.navigation_animation):
			for tween in [view._camera_tween,view._mode_tween,view._layout_tween]:
				if tween!=null and tween.is_running():tween.custom_step(1000);tween.kill()
		if bool(preferences.reduce_motion) or not bool(preferences.camera_inertia):
			if view._inertia_tween!=null:view._inertia_tween.kill()
		if not bool(preferences.path_feedback):
			if view._trail_tween!=null:view._trail_tween.kill()
			view.clear_browse_path()
	if bool(preferences.reduce_motion) or not bool(preferences.navigation_animation):
		if _reader_tween!=null and _reader_tween.is_running():_reader_tween.custom_step(1000);_reader_tween.kill()
	if _interaction_active and (bool(preferences.reduce_motion) or not bool(preferences.node_elasticity)):
		_interaction_active=false;_interaction_dragging=false
		if force_bridge!=null:force_bridge.cancel()
		_save_graph_constraints()
	if canvas==null:return
	canvas.tilted = preferences.canvas_mode == "layered"
	if initialized:
		_source_base_layout=_canvas_scope(tree_layout)
		_refresh_relationship_projection()
		_refresh_semantic_routes()
		if canvas._layout_tween!=null:canvas._layout_tween.kill()
	canvas.set_canvas_mode(str(preferences.canvas_mode),animate_mode)
	tree_view.tilted = false
	graph_view.tilted = true
	for view in [canvas, tree_view, graph_view]: view._update_camera()

var canvas
var tree_view
var graph_view
var tree_layout: Dictionary = {}
var overview_layout: Dictionary = {}
var canvas_layout: Dictionary = {}
var graph_data: Dictionary = {}
var graph_coordinates: Dictionary = {}
var force_bridge
var graph_ready := false
var graph_error := ""
var initialized := false
var _host_visible:=true
var _pending_source_update:Array=[]
var error := ""
var layout_revision := 0
var _navigation_timing: Dictionary = {}
var _input_serial := 0
var applied_generation := -1
var _interaction_generation:=1000000
var _interaction_active:=false
var _interaction_dragging:=false
var _interaction_settled:=false
var _interaction_command:Dictionary={}
var _interaction_id:=""
var _last_browse_occurrence:=""
var _job_fingerprint := ""
var _job_key := ""
static var _graph_cache: Dictionary = {}
static var _graph_cache_order: Array[String] = []
var _restoring := false
var _last_intent := ""
var _graph_fit_pending := false
var _reader_requested := true
var _compact := false
var _compact_reader_open := false
var _show_relations := true

var map_panels: Array[PanelContainer] = []
var map_grips: Array[Control] = []
var map_controls:Array[HBoxContainer]=[]
var _graph_legend:PanelContainer
var map_sizes := [Vector2(240, 160), Vector2(240, 160)]
var semantic_thread: Thread
var semantic_pending: Dictionary = {}
var _semantic_cache_key := ""
var _overview_semantics_ready:=false
var _overview_semantics:Array=[]
var semantic_serial := 0
var canvas_actions: HFlowContainer
var brand: Label
var toolbar: HFlowContainer
var body: HBoxContainer
var navigation_panel: PanelContainer
var canvas_panel: PanelContainer
var reader_panel: PanelContainer
var _reader_tween:Tween
var _reader_progress:=0.0
var _reader_target_open:=false
var _reader_origin:=Rect2()
var _reader_ghost:PanelContainer
var _reader_ghost_title:Label
var _reader_identity:=""
var _reader_custom:=false
var _reader_source_path:=""
var _reader_source_button:Button
var content_box: VBoxContainer
var nav_tabs: TabContainer
var search: LineEdit
var search_popup: PanelContainer
var search_results: ItemList
var reader_source: RichTextLabel
var reader_technical: RichTextLabel
var reader_close_button: Button
var _reader_reading_scroll := 0
var reader_scroll: ScrollContainer
var reader_title: Label
var reader_meta: Label
var reader_raw_button: Button
var _reader_prose := ""
var _reader_raw := ""
var reader_content: RichTextLabel
var relation_list: VBoxContainer
var breadcrumb: Label
var status: Label
var graph_status: Label
var back_button: Button
var forward_button: Button
var locate_button: Button
var reader_button: Button
var tilt_button: Button
var relation_button: Button

func host_state() -> Dictionary:
	var result := {"ready": initialized, "has_selection": not navigation.selected_id.is_empty(),
		"can_back": navigation.can_travel(false), "can_forward": navigation.can_travel(true),
		"reader_open": reader_panel != null and reader_panel.visible,
		"tilted": canvas != null and canvas.tilted, "relations": _show_relations,
		"selection": str(model.records.get(navigation.selected_id, {}).get("title", "")),
		"source_paths": source_paths, "source_refresh_count": source_refresh_count,
		"selected_reference": _selected_reference(),
		"instruction_request_id": instruction_request_id, "instruction_anchor": _instruction_anchor(),
		"instruction_editor_rect": _instruction_editor_rect(), "instruction_style": _instruction_style(),
		"status": status.text if status != null else "Loading…", "relationship_focus":not focus_anchor.is_empty()}
	result["related_sources"]=[]
	result["loading_paused"]=false
	if source_loader!=null:
		result.loading_paused=source_loader.paused
		for entry in source_loader.entries:
			result.related_sources.append({"path":entry.path,"label":entry.label,"status":entry.status,"reason":entry.reason})
	result.merge(preferences, true)
	result.browser_id=browser_id;result.theme_id=theme_id
	result.browsers=view_registry.choices("browsers")
	result.themes=view_registry.choices("themes")
	result.view_packages=view_registry.package_choices()
	if browsing_configuration!=null:result.browsing_configuration=browsing_configuration.current()
	result.canvas_modes=view_registry.browsers.get(browser_id,{}).get("modes",[])
	if browser_id!="mind_map" and depth_browser!=null:
		result.canvas_mode="layered";result.tilted=true
		result.merge(depth_browser.browser_state(),true)
	return result

func _selected_reference() -> Variant:
	if not model.records.has(navigation.selected_id): return null
	var record: Dictionary = model.records[navigation.selected_id]
	if bool(record.get("external", false)): return null
	return {"source_path":str(record.source), "source_sha256":str(record.source_sha256),
		"node_key":str(record.key), "node_uuid":str(record.uuid), "title":str(record.title),
		"details_open": (depth_browser.browser_state().reader_open if browser_id!="mind_map" and depth_browser!=null else reader_panel!=null and reader_panel.visible)}

func _instruction_anchor() -> Variant:
	var rect := Rect2()
	if browser_id!="mind_map" and depth_browser!=null and depth_browser.browser_state().reader_open:
		rect=depth_browser.reader_rect()
	elif reader_panel!=null and reader_panel.visible:
		rect=reader_panel.get_rect()
	if rect.size.x<=0 or rect.size.y<=0:return null
	return {"x":rect.position.x,"y":rect.position.y,"width":rect.size.x,"height":rect.size.y}

func _rect_value(rect:Rect2)->Variant:
	if rect.size.x<=0 or rect.size.y<=0:return null
	return {"x":rect.position.x,"y":rect.position.y,"width":rect.size.x,"height":rect.size.y}

func _instruction_editor_rect()->Variant:
	if browser_id!="mind_map" and depth_browser!=null:
		return _rect_value(depth_browser.editor_rect())
	return _rect_value(_instruction_classic_rect) if _instruction_open and reader_panel!=null and reader_panel.visible else null

func _instruction_style()->Dictionary:
	var visual=get_meta("polar_visual_theme_state") if has_meta("polar_visual_theme_state") else null
	if visual==null:visual=preload("res://addons/polar_graphics/visual_theme.gd").load_theme(theme_id)
	var result:Dictionary={}
	for key in ["card","text","muted","selection","border"]:
		var color:Color=visual.get(key)
		if key=="card":color.a*=.96
		result[key]="#"+color.to_html(true)
	return result

func _close_instruction()->void:
	_instruction_open=false;_instruction_classic_rect=Rect2()
	if browser_id!="mind_map" and depth_browser!=null:depth_browser.set_instruction_visible(false)
	else:_fit_reader()
	_publish_host_state()

func _request_instruction() -> void:
	if _selected_reference()==null:return
	if browser_id!="mind_map" and depth_browser!=null:
		depth_browser.set_instruction_visible(true)
	elif reader_panel!=null and not reader_panel.visible:
		_open_reader()
	_instruction_open=true
	if browser_id=="mind_map":_fit_reader()
	instruction_request_id+=1
	_publish_host_state()

func _publish_host_state() -> void:
	if bool(get_meta("native_controls", false)): host_state_changed.emit(host_state())
	call_deferred("_trace_view_state")

func _browsing_configuration_changed()->void:
	if not initialized:return
	if _host_visible and depth_browser!=null and depth_browser.has_method("refresh_configuration"):
		depth_browser.refresh_configuration()
	_publish_host_state()

func host_command(action: String, query: String = "") -> void:
	if action=="load_browsing_configuration":browsing_configuration.select_file(query);return
	if action=="use_default_browsing_configuration":browsing_configuration.use_default();return
	if action=="open_browsing_configuration_source":
		var path:String=str(browsing_configuration.current().path)
		if not path.is_empty():
			if bool(get_meta("native_controls",false)):host_open_source.emit(path)
			else:OS.shell_open(path)
		return
	if action=="bind_window":
		var previous:String=str(get_meta("window_id",""))
		if previous==query:return
		if _window_presenters.has(previous) and _window_presenters[previous].get_ref()==self:_window_presenters.erase(previous)
		set_meta("window_id",query)
		if initialized:
			var owner=_window_presenters.get(query)
			if owner!=null and owner.get_ref()!=null and owner.get_ref()!=self:
				_save_view_state()
				if depth_browser!=null:
					view_registry.unbind_browser(depth_browser)
					remove_child(depth_browser);depth_browser.queue_free();depth_browser=null;_active_package_browser=""
				if _host_visible:_switch_browser(browser_id)
			else:_register_window_presenter()
		return
	if action=="select_browser":_select_global_browser(query);return
	if action=="select_theme":_select_global_theme(query);return
	if action in ["install_package","export_package","uninstall_view_package"]:
		if action=="uninstall_view_package":
			if browser_id.begins_with(query+"/"):_switch_browser("depth_forest")
			if theme_id.begins_with(query+"/"):_apply_visual_theme("default")
		status.text=view_registry.command(action,query)
		_save_view_state();_publish_host_state();return
	if action=="load_related" and source_loader!=null:source_loader.request(query);return
	if action=="pause_loading" and source_loader!=null:source_loader.toggle_pause();return
	if action=="close_instruction":_close_instruction();return
	if action == "locate_node":
		var matches: Array=[]
		for id in model.ordered_ids:
			var record: Dictionary=model.records[id]
			if not bool(record.get("external",false)) and str(record.get("uuid",""))==query:matches.append(id)
		if matches.size()==1:
			navigation.select_node(matches[0]);host_command("locate")
		else:
			status.text="Referenced node is missing or ambiguous.";_publish_host_state()
		return
	if action == "edit_instruction":
		_request_instruction()
		return
	if action == "load_files":
		var paths = JSON.parse_string(query)
		if paths is Array and paths.all(func(path): return path is String): _open_files(PackedStringArray(paths))
		return
	if action == "locate_reference":
		var ref = JSON.parse_string(query)
		if ref is Dictionary:
			for id in model.ordered_ids:
				var record: Dictionary = model.records[id]
				if str(record.source)==str(ref.get("source_path", "")) and ((not str(ref.get("node_uuid", "")).is_empty() and str(record.uuid)==str(ref.node_uuid)) or (str(ref.get("node_uuid", "")).is_empty() and str(record.key)==str(ref.get("node_key", "")))):
					navigation.select_node(id)
					host_command("locate")
					return
		return
	if action == "open_files":
		_choose_files()
		call_deferred("_trace_view_state")
		return
	if not initialized: return
	if action in ["browser_mind_map","browser_depth_forest"]:
		_select_global_browser("depth_forest" if action=="browser_depth_forest" else "mind_map")
		return
	if action in ["theme_default","theme_christmas"]:
		_select_global_theme("christmas" if action=="theme_christmas" else "default")
		_save_view_state();_publish_host_state();return
	if browser_id!="mind_map" and depth_browser!=null and depth_browser.execute_action(action):
		_publish_host_state();return
	if action in ["canvas_two_d", "canvas_layered", "tilt"]:
		preferences.canvas_mode = ("two_d" if preferences.canvas_mode == "layered" else "layered") if action == "tilt" else ("two_d" if action == "canvas_two_d" else "layered")
		_apply_preferences(true)
		_save_view_state()
		_publish_host_state()
		return
	if action.begins_with("framing_"):
		preferences.framing_policy={"framing_reading":"reading_priority","framing_both":"both_endpoints","framing_fixed":"fixed_zoom"}.get(action,"reading_priority")
		_apply_preferences();_save_view_state();_publish_host_state();return
	if action in ["navigation_animation", "frame_endpoints", "path_feedback", "camera_inertia", "node_elasticity", "reduce_motion","connection_flow"]:
		preferences[action] = not bool(preferences[action])
		_apply_preferences()
		_save_view_state()
		_publish_host_state()
		return
	match action:
		"back": navigation.travel(false)
		"forward": navigation.travel(true)
		"search":
			search.text = query
			_search_changed(query)
		"locate": locate_selected()
		"read": _toggle_reader()
		"open_source": _open_source(true)
		"fit": _fit_current_tree()
		"relationship_focus": _toggle_relationship_focus()
		"focus_this_node": _enter_relationship_focus(true)
		"focus_branch": focus_selected()
		"relations":
			relation_button.set_pressed_no_signal(not _show_relations)
			_toggle_relations()
	_publish_host_state()

func _ready() -> void:
	browsing_configuration=preload("res://browsing_configuration.gd").shared(get_tree())
	browsing_configuration.changed.connect(_browsing_configuration_changed)
	view_registry.refresh()
	view_registry.changed.connect(_view_catalog_changed)
	view_registry.selection_changed.connect(_sync_global_presentation)
	view_registry.reference_requested.connect(_request_related_reference)
	TranslationServer.set_locale("en")
	OS.low_processor_usage_mode = false if DisplayServer.get_name() == "gtk_surface" else OS.low_processor_usage_mode
	# Standalone windows use real logical pixels. Never force an embedded
	# Station host's viewport into the lab's original 1460px design resolution.
	if get_parent() == get_tree().root:
		get_window().content_scale_size = Vector2i.ZERO
		get_window().content_scale_mode = Window.CONTENT_SCALE_MODE_DISABLED
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	browser_id=str(view_registry.current_selection().browser)
	_build_ui()
	for view in [canvas,tree_view,graph_view]:
		if view!=null:view.set_render_active(false)
	if not OS.get_environment("POLAR_STATION_TRACE_DIR").is_empty():
		RenderingServer.frame_post_draw.connect(_record_navigation_frame)
	resized.connect(_responsive)
	toolbar.resized.connect(_responsive)
	_responsive()
	saved_view_state.load(_view_state_path())
	_restore_preferences()
	var saved_sizes = saved_view_state.get_value("view", "map_sizes", map_sizes)
	if saved_sizes is Array and saved_sizes.size() == 2:
		for i in 2:
			if saved_sizes[i] is Vector2 and saved_sizes[i].is_finite(): map_sizes[i] = saved_sizes[i].clamp(Vector2(120,80),Vector2(480,320))
	var loaded := false
	if prepared_model != null:
		model = prepared_model
		loaded = true
	else:
		for arg in OS.get_cmdline_user_args():
			if arg.begins_with("--itree="): source_paths.append(arg.substr(8))
		if source_paths.is_empty():
			source_paths = PackedStringArray(saved_view_state.get_value("view", "paths", []))
			source_entry_paths=source_paths.duplicate()
			for loaded_path in saved_view_state.get_value("view", "loaded_paths", []):
				if loaded_path not in source_paths:source_paths.append(loaded_path)
		if source_paths.is_empty():
			var saved := ConfigFile.new()
			if saved.load(SOURCE_STATE) == OK:
				source_paths = PackedStringArray(saved.get_value("preview", "paths", []))
		if not source_paths.is_empty(): loaded = model.load_files(source_paths)
		elif not OS.get_environment("POLAR_LAB_GRAPH").is_empty(): loaded = model.load_input()
	if not loaded:
		error = str(model.error)
		status.text = error if not error.is_empty() else "Open files to browse one or more Idea Trees."
		back_button.disabled = true
		forward_button.disabled = true
		locate_button.disabled = true
		_initializing=false
		_publish_host_state()
		return
	var initial_browser: String=str(view_registry.current_selection().browser)
	var prepare_classic: bool = _host_visible and (initial_browser=="mind_map" or not view_registry.browsers.has(initial_browser))
	navigation.initialize(model)
	navigation.changed.connect(_on_navigation_changed)
	_initialize_tree_navigation()
	_rebuild_tree(prepare_classic)
	if prepare_classic:
		_refresh_selection()
		_start_graph()
	_responsive()
	await get_tree().process_frame
	await get_tree().process_frame
	# Frame the selected source tree at reading scale; the navigator keeps
	# the complete forest. Unrelated roots must not shrink this first view.
	if canvas!=null:
		var initial_root := str(tree_layout.items.get(navigation.selected_occurrence, {}).get("root_id", ""))
		var initial_keys:Array=canvas.points.keys().filter(func(key):return str(canvas.records[key].get("source",""))==_active_source())
		canvas.fit_keys(initial_keys, false, true)
		tree_view.fit_all(false)
	_variant_switching=true
	if saved_view_state.has_section_key("view", "navigation"):
		navigation.restore(saved_view_state.get_value("view", "navigation"))
	_apply_preferences()
	_restore_tree_camera()
	_variant_switching=false
	_initializing=false
	initialized = error.is_empty()
	browser_views=saved_view_state.get_value("view","browser_views",{})
	var saved_browser: String=str(view_registry.current_selection().browser)
	var saved_theme: String=str(view_registry.current_selection().theme)
	var saved_reader: bool=bool(browser_views.get("depth_forest",{}).get("reader_open",false))
	if saved_browser!="mind_map" and view_registry.browsers.has(saved_browser):
		_switch_browser(saved_browser)
		if depth_browser!=null and saved_reader:depth_browser.show_details()
	elif saved_browser!="mind_map":status.text="Saved browser unavailable; showing Mind Map."
	_apply_visual_theme(saved_theme)
	for view in [canvas,tree_view,graph_view]:
		if view!=null:view.set_render_active(browser_id=="mind_map")
	_publish_host_state()
	_trace_view_state()
	_start_source_events()
	_start_related_loading()
	print("NAVIGATOR_READY nodes=%d occurrences=%d true_2_5d=true" % [model.ordered_ids.size(), tree_layout.get("items", {}).size()])

func _choose_files() -> void:
	if not DisplayServer.has_feature(DisplayServer.FEATURE_NATIVE_DIALOG_FILE):
		status.text = "Native file chooser is unavailable in this host."
		_publish_host_state()
		return
	if not source_paths.is_empty(): file_dialog.current_dir = source_paths[0].get_base_dir()
	file_dialog.popup_centered_ratio(0.8)

func _open_files(paths: PackedStringArray) -> void:
	if paths.is_empty(): return
	var candidate = ViewModel.new()
	if not candidate.load_files(paths):
		status.text = "Could not open files; current view retained: " + str(candidate.error)
		_publish_host_state()
		return
	var candidate_navigation = Navigation.new()
	candidate_navigation.initialize(candidate)
	var migrated_ids: Dictionary = candidate_navigation.migrate_from(navigation) if navigation.model != null else {}
	var candidate_layout := TreeProjection.build(candidate, candidate_navigation.roots, candidate_navigation.expanded,
		candidate_navigation.budget, "balanced", candidate_navigation.selected_occurrence)
	if not str(candidate_layout.get("error", "")).is_empty():
		status.text = "Layout failed; current view retained: " + str(candidate_layout.error)
		_publish_host_state()
		return
	if _source_quiet != null:
		_source_quiet.stop();_source_quiet = null
	if _source_events != null:
		remove_child(_source_events);_source_events.queue_free();_source_events = null
	source_paths = paths
	source_entry_paths=paths.duplicate()
	_apply_source_candidate(candidate, candidate_navigation, candidate_layout, migrated_ids)
	_start_source_events()
	_start_related_loading()
	var saved := ConfigFile.new()
	saved.set_value("preview", "paths", paths)
	var save_result := saved.save(SOURCE_STATE)
	if save_result != OK: push_warning("Cannot remember Idea Tree paths: %d" % save_result)

func _theme() -> Theme:
	var value:=PresentationTheme.controls()
	value.default_font=ThemeDB.fallback_font
	return value

func _panel() -> PanelContainer:
	var panel:=PanelContainer.new()
	panel.add_theme_stylebox_override("panel",PresentationTheme.panel())
	return panel

func _label(text: String, font_size: int = 16, color: Color = PresentationTheme.TEXT) -> Label:
	var label := Label.new()
	label.text = text
	label.add_theme_font_size_override("font_size", font_size)
	label.add_theme_color_override("font_color", color)
	return label

func _button(parent: Control, text: String, action: Callable, hint: String = "") -> Button:
	var button := Button.new()
	button.text = text
	button.tooltip_text = hint
	button.pressed.connect(action)
	parent.add_child(button)
	return button

func _build_ui() -> void:
	theme = _theme()
	var background := ColorRect.new()
	background.color = PresentationTheme.BACKGROUND
	background.mouse_filter = Control.MOUSE_FILTER_IGNORE
	background.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	add_child(background)
	var margin := MarginContainer.new()
	margin.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	for side in ["left", "right", "top", "bottom"]:
		margin.add_theme_constant_override("margin_" + side, 0 if get_meta("native_controls", false) else 12)
	add_child(margin)
	var column := VBoxContainer.new()
	column.add_theme_constant_override("separation", 0 if get_meta("native_controls", false) else 10)
	margin.add_child(column)
	toolbar = HFlowContainer.new()
	toolbar.add_theme_constant_override("h_separation", 7)
	toolbar.add_theme_constant_override("v_separation", 7)
	column.add_child(toolbar)
	brand = _label("IDEA TREE", 20, Color("8be0ca"))
	toolbar.add_child(brand)
	_button(toolbar, "Open files", _choose_files, "Select multiple Idea Trees; failed loads preserve the current view")
	file_dialog = FileDialog.new()
	file_dialog.access = FileDialog.ACCESS_FILESYSTEM
	file_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILES
	file_dialog.use_native_dialog = true
	file_dialog.filters = PackedStringArray(["*.itree.json ; Idea Tree", "*.json ; JSON"])
	file_dialog.title = "Open Idea Tree"
	file_dialog.files_selected.connect(_open_files)
	add_child(file_dialog)
	back_button = _button(toolbar, "←", func(): navigation.travel(false), "Back · Alt+Left")
	forward_button = _button(toolbar, "→", func(): navigation.travel(true), "Forward · Alt+Right")
	search = LineEdit.new()
	search.placeholder_text = "Search nodes or content…  Ctrl+F"
	search.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	search.custom_minimum_size.x = 180
	toolbar.add_child(search)
	search.text_changed.connect(_search_changed)
	search.text_submitted.connect(func(_value):
		var selected := search_results.get_selected_items()
		_open_search_result(selected[0] if not selected.is_empty() else 0))
	search.gui_input.connect(_search_key)
	locate_button = _button(toolbar, "Locate selected", locate_selected, "Locate the selected node in the canvas")
	reader_button = _button(toolbar, "Read", _toggle_reader, "Read node content, source and relationships")
	reader_button.toggle_mode = true
	reader_button.button_pressed = false
	status = _label("Loading…", 12, Color("8ba2b6"))
	status.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	column.add_child(status)
	search_popup = _panel()
	search_popup.z_index = 100
	search_popup.visible = false
	add_child(search_popup)
	search_results = ItemList.new()
	search_results.custom_minimum_size = Vector2(540, 280)
	search_results.item_activated.connect(_open_search_result)
	search_popup.add_child(search_results)
	_content_column=column
	if browser_id=="mind_map" and _host_visible:_build_classic_ui()

func _build_classic_ui()->void:
	if canvas!=null:return
	var previous_children:=get_children()
	var column:=_content_column
	body = HBoxContainer.new()
	body.size_flags_vertical = Control.SIZE_EXPAND_FILL
	body.add_theme_constant_override("separation", 10)
	column.add_child(body)
	navigation_panel = _panel()
	navigation_panel.custom_minimum_size.x = 300
	body.add_child(navigation_panel)
	tree_strip=load("res://tree_wheel.gd").new();body.add_child(tree_strip)
	tree_strip.source_selected.connect(func(key:String):_switch_tree(key);canvas.grab_focus())
	tree_strip.reordered.connect(_reorder_tree)
	tree_strip.width_changed.connect(func(_v):_save_view_state())
	var nav_column := VBoxContainer.new()
	nav_column.add_theme_constant_override("separation", 8)
	navigation_panel.add_child(nav_column)
	var nav_heading := HBoxContainer.new()
	nav_column.add_child(nav_heading)
	var nav_title := _label("NAVIGATE", 12, Color("8ca3b8"))
	nav_title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	nav_heading.add_child(nav_title)
	nav_tabs = TabContainer.new()
	nav_tabs.size_flags_vertical = Control.SIZE_EXPAND_FILL
	nav_tabs.add_theme_stylebox_override("panel", StyleBoxEmpty.new())
	nav_column.add_child(nav_tabs)
	var tree_box := VBoxContainer.new()
	tree_box.name = "Tree"
	nav_tabs.add_child(tree_box)
	tree_view = SpatialView.new()
	tree_view.role = "tree"
	tree_view.size_flags_vertical = Control.SIZE_EXPAND_FILL
	tree_box.add_child(tree_view)
	var graph_box := VBoxContainer.new()
	graph_box.name = "3D overview"
	nav_tabs.add_child(graph_box)
	graph_view = SpatialView.new()
	graph_view.role = "graph"
	graph_view.size_flags_vertical = Control.SIZE_EXPAND_FILL
	graph_box.add_child(graph_view)
	graph_status = _label("Computing 3D layout…", 12, Color("8ca3b8"))
	graph_status.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	graph_box.add_child(graph_status)
	nav_tabs.tab_changed.connect(_nav_tab_changed)
	canvas_panel = _panel()
	if get_meta("native_controls", false): canvas_panel.add_theme_stylebox_override("panel", StyleBoxEmpty.new())
	canvas_panel.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	canvas_panel.size_flags_vertical = Control.SIZE_EXPAND_FILL
	body.add_child(canvas_panel)
	content_box = VBoxContainer.new()
	content_box.add_theme_constant_override("separation", 0 if get_meta("native_controls", false) else 8)
	canvas_panel.add_child(content_box)
	canvas_actions = HFlowContainer.new()
	content_box.add_child(canvas_actions)
	canvas_actions.add_child(_label("CANVAS", 12, Color("8ca3b8")))
	_button(canvas_actions, "Fit", func(): canvas.fit_all(true))
	_button(canvas_actions, "Focus branch", focus_selected)
	tilt_button = _button(canvas_actions, "2D / 2.5D", func():
		host_command("tilt")
		tilt_button.text = "2.5D" if canvas.tilted else "Front")
	relation_button = _button(canvas_actions, "Relations", _toggle_relations, "Show relationships for the selected node")
	relation_button.toggle_mode = true
	relation_button.button_pressed = true
	canvas = SpatialView.new()
	canvas.role = "canvas"
	canvas.size_flags_vertical = Control.SIZE_EXPAND_FILL
	canvas.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	content_box.add_child(canvas)
	breadcrumb = _label("", 12, Color("98adbf"))
	breadcrumb.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	content_box.add_child(breadcrumb)
	reader_panel = _panel()
	reader_panel.custom_minimum_size.x = 290
	add_child(reader_panel)
	reader_panel.z_index = 50
	reader_panel.visible = false
	_reader_ghost=PanelContainer.new();_reader_ghost.mouse_filter=Control.MOUSE_FILTER_IGNORE;_reader_ghost.z_index=51
	_reader_ghost.add_theme_stylebox_override("panel",PresentationTheme.panel(PresentationTheme.SELECTED))
	_reader_ghost_title=_label("",20);_reader_ghost_title.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART
	_reader_ghost.add_child(_reader_ghost_title);add_child(_reader_ghost);_reader_ghost.hide()
	var reader_shell := VBoxContainer.new()
	reader_shell.add_theme_constant_override("separation", 10)
	reader_panel.add_child(reader_shell)
	var reader_actions := HFlowContainer.new()
	reader_shell.add_child(reader_actions)
	reader_close_button = _button(reader_actions, "Close ×", _close_reader)
	_reader_source_button=_button(reader_actions,"Open file",_open_source)
	_button(reader_actions, "Edit instruction", _request_instruction)
	_button(reader_actions, "Copy content", func(): DisplayServer.clipboard_set(_reader_prose))
	_button(reader_actions, "Copy source", func(): DisplayServer.clipboard_set(reader_source.text))
	reader_raw_button = _button(reader_actions, "Technical ▾", func():
		if reader_raw_button.button_pressed:
			_reader_reading_scroll = reader_scroll.scroll_vertical
			reader_technical.show()
			reader_raw_button.text = "Technical ▴"
			reader_scroll.call_deferred("ensure_control_visible", reader_technical)
		else:
			reader_technical.hide()
			reader_raw_button.text = "Technical ▾"
			reader_scroll.set_deferred("scroll_vertical", _reader_reading_scroll))
	reader_raw_button.toggle_mode = true
	reader_scroll = ScrollContainer.new()
	reader_scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	reader_scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	reader_shell.add_child(reader_scroll)
	var reader_column := VBoxContainer.new()
	reader_column.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	reader_column.add_theme_constant_override("separation", 14)
	reader_scroll.add_child(reader_column)
	reader_title = _label("", 24)
	reader_title.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	reader_column.add_child(reader_title)
	reader_meta = _label("", 12, PresentationTheme.MUTED)
	reader_meta.autowrap_mode = TextServer.AUTOWRAP_ARBITRARY
	reader_column.add_child(reader_meta)
	reader_content = RichTextLabel.new()
	reader_content.bbcode_enabled = false
	reader_content.selection_enabled = true
	reader_content.fit_content = true
	reader_content.scroll_active = false
	reader_content.add_theme_font_size_override("normal_font_size", 18)
	reader_content.add_theme_constant_override("line_separation", 5)
	reader_column.add_child(reader_content)
	reader_column.add_child(_label("Source", 12, PresentationTheme.MUTED))
	reader_source = RichTextLabel.new()
	reader_source.selection_enabled = true
	reader_source.fit_content = true
	reader_source.scroll_active = false
	reader_source.autowrap_mode = TextServer.AUTOWRAP_ARBITRARY
	reader_source.add_theme_font_size_override("normal_font_size", 13)
	reader_column.add_child(reader_source)
	reader_technical = RichTextLabel.new()
	reader_technical.selection_enabled = true
	reader_technical.fit_content = true
	reader_technical.scroll_active = false
	reader_technical.autowrap_mode = TextServer.AUTOWRAP_ARBITRARY
	reader_technical.add_theme_font_size_override("normal_font_size", 12)
	reader_technical.hide()
	reader_column.add_child(reader_technical)
	# Retained for explicit semantic-edge reading only; no hierarchy list.
	relation_list = VBoxContainer.new()
	relation_list.hide()
	reader_column.add_child(relation_list)
	reader_panel.minimum_size_changed.connect(func(): call_deferred("_fit_reader"))
	for index in 2:
		var map_panel := PanelContainer.new()
		var backing := StyleBoxFlat.new()
		backing.bg_color = Color(PresentationTheme.BACKGROUND,0.15)
		map_panel.add_theme_stylebox_override("panel", backing)
		add_child(map_panel)
		map_panels.append(map_panel)
		var tools:=HBoxContainer.new();tools.add_theme_constant_override("separation",2);tools.mouse_filter=Control.MOUSE_FILTER_IGNORE
		add_child(tools);map_controls.append(tools)
		if index==1:
			_button(tools,"⌂",func():graph_view.fit_all(true),"Fit complete relationship network")
			_button(tools,"↟",func():graph_view.yaw=0;graph_view.pitch=0;graph_view.fit_all(true),"Front direction")
			_button(tools,"ⓘ",_toggle_graph_legend,"Sources, node types and relationship styles")
		var view = tree_view if index == 0 else graph_view
		view.reparent(map_panel)
		var grip := Control.new()
		grip.mouse_default_cursor_shape = Control.CURSOR_BDIAGSIZE if index == 0 else Control.CURSOR_FDIAGSIZE
		grip.tooltip_text = "Resize map"
		add_child(grip)
		map_grips.append(grip)
		grip.size = Vector2(24,24)
		grip.draw.connect(func():
			if grip.get_rect().has_point(get_local_mouse_position()):
				grip.draw_line(Vector2(3,3),Vector2(13,13),Color(0.7,0.8,0.9,0.8),2))
		grip.mouse_entered.connect(grip.queue_redraw)
		grip.mouse_exited.connect(grip.queue_redraw)
		grip.gui_input.connect(func(event):
			if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT:
				if event.pressed:
					# Begin at the displayed extent, not a preference clipped by a small Pane.
					map_sizes[index] = map_panel.size
					map_sizes[1-index].y = map_panels[1-index].size.y
				else: _save_view_state()
				grip.accept_event()
			elif event is InputEventMouseMotion and event.button_mask & MOUSE_BUTTON_MASK_LEFT:
				var available := maxf(2, canvas.size.y - 36)
				var minimum_height := minf(80, available * 0.5)
				map_sizes[index] = (map_sizes[index] + Vector2(-event.relative.x,event.relative.y if index == 0 else -event.relative.y)).clamp(Vector2(120,minimum_height),Vector2(480,minf(320,available-minimum_height)))
				map_sizes[1-index].y = minf(map_sizes[1-index].y, available-map_sizes[index].y)
				_place_maps()
				grip.accept_event())

	navigation_panel.hide()
	navigation_panel.custom_minimum_size = Vector2.ZERO
	graph_status.hide()
	_classic_roots=[body]
	for child in get_children():
		if child not in previous_children:_classic_roots.append(child)
	_connect_view(canvas, "canvas")
	_connect_view(tree_view, "tree")
	_connect_view(graph_view, "graph")
	tree_view.map_position_requested.connect(func(point):
		canvas._move_camera(point, canvas.view_size, canvas.orbit_distance, false))
	tree_view.map_pan_requested.connect(func(delta):
		canvas._move_camera(canvas.target + delta, canvas.view_size, canvas.orbit_distance, false))
	tree_view.overview_zoom_requested.connect(func(factor): canvas.zoom_at(factor, canvas.size * 0.5))
	tree_view.overview_pan_requested.connect(func(delta): canvas.pan_by(delta))


func _connect_view(view, role: String) -> void:
	if not OS.get_environment("POLAR_STATION_TRACE_DIR").is_empty():
		view.presentation_updated.connect(func():call_deferred("_trace_view_state"))
	view.node_selected.connect(func(id:String,occurrence:String):
		var key:String=view.key_for_node(id,occurrence)
		var display_record:Dictionary=view.records.get(key,{})
		if bool(display_record.get("external",false)):_show_unresolved(display_record);return
		if not focus_anchor.is_empty() or not tree_layout.items.has(occurrence):navigation.select_node(id)
		else:navigation.select_occurrence(occurrence,tree_layout))
	view.node_activated.connect(func(id: String, occurrence: String):
		var display_record:Dictionary=view.records.get(id if occurrence.is_empty() else occurrence,{})
		if bool(display_record.get("external",false)):_show_unresolved(display_record);return
		if occurrence.is_empty(): navigation.select_node(id)
		elif tree_layout.items.has(occurrence): navigation.select_occurrence(occurrence, tree_layout)
		else:navigation.select_node(id)
		_open_reader())
	view.node_control_requested.connect(func(key: String, action: String):
		navigation.toggle_neighbors(key, action, tree_layout))
	view.node_dragged.connect(func(key:String,position_value:Vector3,phase:String):_node_drag(role,key,position_value,phase))
	view.edge_selected.connect(_show_edge)
	view.camera_changed.connect(func(value: Dictionary):
		if not _restoring:
			navigation.set_camera(role, value)
			if role=="canvas":_remember_tree()
		if role == "canvas":
			_sync_canvas_region())
	if role == "canvas":
		view.clip_contents=true
		view.presentation_updated.connect(_place_focus_headers)
		view.tree_step_requested.connect(_step_tree)
		view.resized.connect(func():
			call_deferred("_sync_canvas_region")
			call_deferred("_place_maps")
			call_deferred("_reveal_selected_after_resize"))

func _reveal_selected_after_resize() -> void:
	if not initialized or browser_id != "mind_map" or preferences.framing_policy == "fixed_zoom": return
	_reveal_selected()

func _sync_canvas_region() -> void:
	if canvas == null or canvas.camera == null or tree_view == null:
		return
	var aligned:=true
	for key in canvas.points:
		if not tree_view.points.has(key) or Vector2(canvas.points[key].x,canvas.points[key].y).distance_to(Vector2(tree_view.points.get(key,Vector3.ZERO).x,tree_view.points.get(key,Vector3.ZERO).y))>0.001:
			aligned=false;break
	if preferences.canvas_mode=="layered" or not aligned:
		var visible:Dictionary={}
		for key in canvas.points:
			if not canvas.camera.is_position_behind(canvas.points[key]) and Rect2(Vector2.ZERO,canvas.size).intersects(canvas.screen_card_rect(key)):
				var marker:String=tree_view.key_for_node(str(canvas.records[key].node_id),str(key))
				if not marker.is_empty():visible[marker]=true
		tree_view.set_visible_markers(visible.keys())
		return
	tree_view.visible_markers=[]
	var corners: Array = []
	for point in [Vector2.ZERO, Vector2(canvas.size.x, 0), canvas.size, Vector2(0, canvas.size.y)]:
		corners.append(canvas._plane_point(point))
	tree_view.set_focus_region(corners)

func _on_navigation_changed(intent: String) -> void:
	if _variant_switching:return
	if not _host_visible or (depth_browser==null and canvas==null):
		_publish_host_state()
		return
	if browser_id!="mind_map" and depth_browser!=null:
		depth_browser.call_deferred("sync_selection")
		_publish_host_state()
		return
	if _switching_tree:return
	if not _navigation_timing.is_empty() and not _navigation_timing.has("navigation_us"):
		_navigation_timing["navigation_us"] = Time.get_ticks_usec() - int(_navigation_timing.received_us)
		_navigation_timing["intent"] = intent
	_last_intent = intent
	if intent.begins_with("camera:") or intent.begins_with("pins:"):
		return
	if intent in ["selection", "browse_selection"]:
		_refresh_selection()
		if focus_anchor.is_empty() or intent=="browse_selection":_reveal_selected()
		_remember_tree()
		return
	if intent in ["initialize", "expansion", "focus", "locate", "history", "restore", "browse"]:
		_rebuild_tree()
	if intent in ["history", "restore"]:
		_restoring = true
		for role in ["canvas", "tree", "graph"]:
			_view(role).restore_camera(navigation.camera_states.get(role, {}))
		_restoring = false
		navigation.filter_kind = "all"
		_show_relations = bool(navigation.camera_states.get("presentation", {}).get("relations", _show_relations))
	_refresh_selection()
	_sync_canvas_region()
	if intent in ["locate", "focus"]:
		_responsive()
		canvas.center_key(navigation.selected_occurrence)
	elif intent == "browse":
		_reveal_selected()
	# A navigation action invalidates the controller generation. Cached snapshots
	# can be applied without reheating; an in-flight result is never accepted late.
	var graph_started := Time.get_ticks_usec()
	_start_graph(intent in ["focus", "filter"])
	if not _navigation_timing.is_empty(): _navigation_timing["graph_update_us"] = Time.get_ticks_usec() - graph_started

func _reveal_selected() -> void:
	var key:String=canvas.key_for_node(navigation.selected_id,navigation.selected_occurrence)
	canvas.reveal_readable(key,str(preferences.framing_policy))


func _view(role: String):
	return {"canvas": canvas, "tree": tree_view, "graph": graph_view}[role]

func _rebuild_tree(materialize: bool = true) -> void:
	var layout_started := Time.get_ticks_usec()
	if not prepared_layout.is_empty():
		tree_layout = prepared_layout
		prepared_layout = {}
	else:
		tree_layout = TreeProjection.build(model, navigation.roots, navigation.expanded,
			navigation.budget, "balanced", navigation.selected_occurrence)
	if not _navigation_timing.is_empty(): _navigation_timing["layout_solver_us"] = Time.get_ticks_usec() - layout_started
	layout_revision += 1
	if not str(tree_layout.get("error", "")).is_empty():
		error = str(tree_layout.error)
		status.text = error
		return
	error = ""
	if not materialize:return
	var controls_started := Time.get_ticks_usec()
	for key in tree_layout.items:
		var item: Dictionary = tree_layout.items[key]
		var controls: Array = []
		for direction in ["parents", "children"]:
			var state: Dictionary = navigation.neighbor_state(str(key), direction, tree_layout)
			var mark: String = {"none": "·", "collapsed": "+", "partial": "±", "full": "−"}[str(state.state)]
			controls.append({"action": direction, "state":str(state.state), "text": ("↑" if direction == "parents" else "↓") + mark,
				"hint": "%s: %s (%d)%s" % [direction.capitalize(), str(state.state), int(state.count), " · %d not loaded" % int(item.unresolved_child_count) if direction == "children" and int(item.unresolved_child_count) > 0 else ""], "enabled": int(state.count) > 0})
		item["controls"] = controls
	if not _navigation_timing.is_empty(): _navigation_timing["controls_projection_us"] = Time.get_ticks_usec() - controls_started
	_source_base_layout=_canvas_scope(tree_layout)
	if overview_layout.is_empty():
		overview_layout=TreeProjection.complete_overview(model)
	if tree_view.points.is_empty():tree_view.set_tree(overview_layout)
	var geometry_started := Time.get_ticks_usec()
	_refresh_relationship_projection()
	for key in navigation.pinned_positions.get("canvas",{}):
		var p=navigation.pinned_positions.canvas[key]
		if p is Array and p.size()==3:canvas.update_canvas_point(str(key),Vector3(float(p[0]),float(p[1]),float(p[2])))
	if not _navigation_timing.is_empty(): _navigation_timing["tree_geometry_us"] = Time.get_ticks_usec() - geometry_started
	if not _navigation_timing.is_empty(): _navigation_timing["tree_layout_and_geometry_us"] = Time.get_ticks_usec() - layout_started
	if theme_id!="default":_apply_visual_theme(theme_id)

func _legacy_canvas_scope(layout: Dictionary) -> Dictionary:
	if str(navigation.scope.get("mode", "all")) != "branch":
		return layout
	var root_occurrence := str(navigation.scope.get("occurrence", ""))
	var result := layout.duplicate(true)
	for key in result.items.keys():
		if key != root_occurrence and not str(key).begins_with(root_occurrence + "/"):
			result.items.erase(key)
			result.positions.erase(key)
			result.sizes.erase(key)
	result.routes = result.routes.filter(func(route): return result.items.has(route.from) and result.items.has(route.to))
	result.edges = result.edges.filter(func(edge): return result.items.has(edge.from) and result.items.has(edge.to))
	return result

func _browse_path(from_key:String,to_key:String)->Array:
	var left:Array=[];var right:Array=[];var key:=from_key
	while tree_layout.items.has(key):left.append(key);key=str(tree_layout.items[key].get("parent",""))
	key=to_key
	while tree_layout.items.has(key):right.append(key);key=str(tree_layout.items[key].get("parent",""))
	for i in left.size():
		var match_index:=right.find(left[i])
		if match_index>=0:
			var result:Array=left.slice(0,i+1);var tail:Array=right.slice(0,match_index);tail.reverse();result.append_array(tail);return result
	return []

func _refresh_selection() -> void:
	_refresh_relationship_projection()
	var previous:=_last_browse_occurrence
	_last_browse_occurrence=navigation.selected_occurrence
	if previous!=navigation.selected_occurrence and not previous.is_empty():
		var route:Dictionary=navigation.route_for_occurrence(previous)
		canvas.browse_origin_key=canvas.key_for_node(str(route.get("node_id","")),previous)
		_pending_browse_path=[]
		for key in _browse_path(previous,navigation.selected_occurrence):
			var part:Dictionary=navigation.route_for_occurrence(str(key))
			var actual:String=canvas.key_for_node(str(part.get("node_id","")),str(key))
			if not actual.is_empty():_pending_browse_path.append(actual)
		if _pending_browse_path.is_empty():
			for edge:Dictionary in model.relation_edges:
				if (str(edge.from)==str(route.get("node_id","")) and str(edge.to)==navigation.selected_id) or (str(edge.to)==str(route.get("node_id","")) and str(edge.from)==navigation.selected_id):
					_pending_browse_path=[canvas.browse_origin_key,canvas.key_for_node(navigation.selected_id,navigation.selected_occurrence)];break
		if bool(preferences.path_feedback):canvas.show_browse_path(_pending_browse_path)
	var selection_started := Time.get_ticks_usec()
	for view in [canvas, tree_view, graph_view]:
		view.set_selection(navigation.selected_id, navigation.selected_occurrence)
	_refresh_semantic_routes()
	_reader_custom=false
	if reader_panel.visible:
		if not _reader_identity.is_empty() and model.resolve(_reader_identity).is_empty():_close_reader()
		else:_refresh_reader()
	back_button.disabled = not navigation.can_travel(false)
	forward_button.disabled = not navigation.can_travel(true)
	var record: Dictionary = model.records.get(navigation.selected_id, {})
	var path: Array = navigation.last_route.get("path", [])
	breadcrumb.text = "Path  / " + " / ".join(path) if not path.is_empty() else "Root  · " + str(record.get("title", ""))
	if bool(navigation.last_route.get("ambiguous", false)):
		breadcrumb.text += "  ·  %d reachable paths; current occurrence retained" % int(navigation.last_route.candidate_count)
	breadcrumb.tooltip_text = breadcrumb.text
	_update_status()
	if not _navigation_timing.is_empty(): _navigation_timing["selection_projection_us"] = Time.get_ticks_usec() - selection_started

func _refresh_semantic_routes() -> void:
	var key:String=str(layout_revision)+":"+str(_show_relations)+":"+(navigation.selected_identity if focus_anchor.is_empty() else focus_anchor)+":"+active_tree+":"+focus_anchor
	if key==_semantic_cache_key:return
	_semantic_cache_key=key
	semantic_serial += 1
	if _overview_semantics_ready:tree_view.set_semantic_routes(_overview_semantics if _show_relations else [])
	semantic_pending = {"serial":semantic_serial,"layout":canvas_layout.duplicate(true),"occurrence":navigation.selected_occurrence,"show":_show_relations,
		"compute_overview":not _overview_semantics_ready and _show_relations,"overview":overview_layout.duplicate(true) if not _overview_semantics_ready and _show_relations else {}}
	_advance_semantic_routes()

func _semantic_completed()->void:
	if semantic_thread != null:
		var result: Dictionary = semantic_thread.wait_to_finish()
		semantic_thread = null
		if bool(result.get("overview_computed",false)):
			_overview_semantics=result.overview_routes;_overview_semantics_ready=true
			tree_view.set_semantic_routes(_overview_semantics if _show_relations else [])
		if int(result.serial) == semantic_serial:
			canvas.set_semantic_routes(result.routes)
			if bool(preferences.path_feedback) and not _pending_browse_path.is_empty():canvas.show_browse_path(_pending_browse_path)
			if not str(result.error).is_empty(): status.text = result.error
	_advance_semantic_routes()

func _advance_semantic_routes()->void:
	if semantic_thread == null and not semantic_pending.is_empty():
		var request := semantic_pending
		semantic_pending = {}
		if _semantic_event==0:_semantic_event=_events.subscribe(_semantic_completed)
		var notify:Callable=_events.waker(_semantic_event)
		semantic_thread = Thread.new()
		semantic_thread.start(func():
			var routes:Array=[]
			if request.show:
				routes=TreeProjection.fixed_routes(request.layout,request.layout.get("temporary_edges",[])) if request.layout.has("temporary_edges") else TreeProjection.semantic_routes(model,request.layout,request.occurrence,true)
			var overview:Array=[]
			if request.compute_overview:overview=TreeProjection.fixed_routes(request.overview,request.overview.get("complete_semantic_edges",[]))
			notify.call()
			return {"serial":request.serial,"routes":routes,"error":request.layout.get("semantic_error",""),"overview_computed":request.compute_overview,"overview_routes":overview})

func _refresh_reader() -> void:
	if _reader_target_open and not model.records.has(navigation.selected_id):_close_reader()
	var record: Dictionary = model.records.get(navigation.selected_id, {})
	reader_title.text = str(record.get("title", ""))
	reader_title.tooltip_text = reader_title.text
	reader_meta.text = str(record.get("type", "")) + " · " + str(record.get("source", "")).get_file()
	reader_source.text = str(record.get("source", ""))
	_reader_source_path=reader_source.text
	_reader_source_button.disabled=_reader_source_path.is_empty()
	var original := str(record.get("description", ""))
	_reader_prose = original if not original.is_empty() else "No content provided for this node."
	_reader_raw = JSON.stringify({"identity": model.reference_descriptor(navigation.selected_id), "properties": record.get("properties", {}), "relations": record.get("relations", {})}, "  ")
	reader_raw_button.set_pressed_no_signal(false)
	reader_raw_button.text = "Technical ▾"
	reader_technical.hide()
	reader_technical.text = _reader_raw
	reader_content.text = _reader_prose
	reader_scroll.scroll_vertical = 0

func _open_relation(edge: Dictionary) -> void:
	if not bool(edge.get("resolved", false)):
		return
	var outgoing: bool = str(edge.from) == navigation.selected_id
	var target_id := str(edge.to) if outgoing else str(edge.from)
	var preferred := ""
	if bool(edge.get("composition", false)):
		if outgoing:
			# A clicked slot is an occurrence instruction, not merely a request
			# for an equivalent definition remembered through another parent.
			preferred = TreeProjection.child_occurrence(navigation.selected_occurrence, edge)
		else:
			var delimiter := str(navigation.selected_occurrence).rfind("/")
			if delimiter > 0:
				var parent_path := str(navigation.selected_occurrence).left(delimiter)
				var route: Dictionary = navigation.route_for_occurrence(parent_path)
				if str(route.get("node_id", "")) == target_id and str(navigation.last_route.get("path", []).back()) == str(edge.get("slot", "")):
					preferred = parent_path
	navigation.locate_node(target_id, preferred)

func _node_drag(role:String,key:String,position_value:Vector3,phase:String)->void:
	if not position_value.is_finite():return
	if role=="graph" and not graph_ready:return
	if phase=="cancel" and role=="graph":
		_interaction_active=false;_interaction_dragging=false
		if force_bridge!=null:force_bridge.cancel()
		_save_graph_constraints();return
	if role=="canvas":
		if phase=="begin":semantic_serial+=1;semantic_pending={};_semantic_cache_key=""
		if phase=="move":canvas.update_canvas_point(key,position_value);_sync_canvas_region()
		if phase in ["end","cancel"] and canvas.points.has(key):
			var pins:Dictionary=navigation.pinned_positions.get("canvas",{}).duplicate(true)
			var point:Vector3=canvas.points[key];pins[key]=[point.x,point.y,point.z]
			navigation.set_pinned_positions("canvas",pins);_save_view_state();_refresh_semantic_routes()
		return
	if role!="graph" or not graph_ready:return
	if phase=="begin":
		_interaction_generation+=1;_interaction_id=key;_interaction_dragging=true;_interaction_settled=false
		_interaction_command={}
		if bool(preferences.node_elasticity) and not bool(preferences.reduce_motion) and force_bridge!=null and force_bridge.has_method("begin_interaction"):
			var input:=GraphProjection.solver_input(graph_data)
			var neighbors:Dictionary={}
			for edge:Dictionary in graph_data.links:
				if str(edge.source)==key:neighbors[str(edge.target)]=true
				if str(edge.target)==key:neighbors[str(edge.source)]=true
			for node:Dictionary in input.nodes:
				var id:=str(node.id);var p:Vector3=graph_coordinates[id]/GraphProjection.WORLD_SCALE
				for i in 3:node[["x","y","z"][i]]=p[i]
				if not neighbors.has(id):
					for i in 3:node[["fx","fy","fz"][i]]=p[i]
			input.config.alpha=0.2;input.config.alpha_decay=0.06;input.config.forces=["link","charge"]
			_interaction_active=force_bridge.begin_interaction(JSON.stringify(input),_interaction_generation)
	elif phase=="move":
		_interaction_settled=false
		graph_coordinates[key]=position_value;graph_view.update_graph_coordinates({key:position_value})
		if _interaction_active:
			var p:Vector3=position_value/GraphProjection.WORLD_SCALE
			_interaction_command={"id":key,"position":[p.x,p.y,p.z]}
			if not force_bridge.is_running():_layout_completed()
	elif phase=="end":
		_interaction_dragging=false
		if not _interaction_active or _interaction_settled:
			_interaction_active=false;_save_graph_constraints()

func _save_graph_constraints()->void:
	var pins:Dictionary=navigation.pinned_positions.get("graph",{}).duplicate(true)
	# Interaction results are current-Pane constraints, never source edits.
	for id in graph_coordinates:
		var identity:String=str(graph_view.records.get(id,{}).get("identity",model.identity(str(id))))
		if not identity.is_empty():
			var p:Vector3=graph_coordinates[id];pins[identity]=[p.x,p.y,p.z]
	navigation.set_pinned_positions("graph",pins)
	_job_key=JSON.stringify(GraphProjection.solver_input(graph_data,pins)).sha256_text()
	_graph_cache[_job_key]=graph_coordinates.duplicate()
	_graph_cache_order.erase(_job_key);_graph_cache_order.append(_job_key)
	while _graph_cache_order.size()>16:_graph_cache.erase(_graph_cache_order.pop_front())
	_save_view_state()

func _finish_interaction_result(result:Dictionary)->void:
	if int(result.get("generation",-1))!=_interaction_generation or not _interaction_active:return
	if not bool(result.get("ok",false)):
		_interaction_active=false;_interaction_dragging=false;_interaction_settled=true
		force_bridge.cancel();status.text="3D interaction stopped: "+str(result.get("error","Invalid worker output"));_publish_host_state();return
	_interaction_settled=bool(result.get("settled",false))
	var coordinates:=GraphProjection.coordinates(result)
	# A newer pointer position is authoritative over a completed worker snapshot.
	if _interaction_dragging and graph_coordinates.has(_interaction_id):coordinates[_interaction_id]=graph_coordinates[_interaction_id]
	graph_coordinates=coordinates;graph_view.update_graph_coordinates(coordinates)
	if bool(result.get("settled",false)) and not _interaction_dragging and _interaction_command.is_empty():
		_interaction_active=false;_save_graph_constraints()

func _notification(what:int)->void:
	if what in [NOTIFICATION_VISIBILITY_CHANGED,NOTIFICATION_WM_WINDOW_FOCUS_OUT] and initialized:
		if what==NOTIFICATION_WM_WINDOW_FOCUS_OUT or not is_visible_in_tree():
			_interaction_active=false;_interaction_dragging=false
			if force_bridge!=null:force_bridge.cancel()

func _start_graph(fit: bool = true) -> void:
	graph_data = GraphProjection.build(model, navigation.scope, navigation.filter_kind)
	var input := GraphProjection.solver_input(graph_data, navigation.pinned_positions.get("graph", {}))
	var serialized := JSON.stringify(input)
	var next_key:String = serialized.sha256_text()
	if graph_ready and _job_key == next_key: return
	_interaction_active=false;_interaction_dragging=false
	_job_key = next_key
	_job_fingerprint = navigation.snapshot_fingerprint
	var generation: int = navigation.begin_layout({"view": "graph", "key": _job_key})
	if force_bridge != null:
		force_bridge.cancel()
	if _graph_cache.has(_job_key):
		_graph_cache_order.erase(_job_key)
		_graph_cache_order.append(_job_key)
		_apply_graph(_graph_cache[_job_key], generation, fit and not graph_ready)
		return
	graph_ready = false
	_graph_fit_pending = fit
	graph_error = ""
	if not ClassDB.class_exists("PolarForceLayout"):
		GDExtensionManager.load_extension("res://native/polar_force_layout.gdextension")
	if not ClassDB.class_exists("PolarForceLayout"):
		graph_error = "3D layout component missing. Reinstall the graphics runtime."
		graph_status.text = graph_error
		return
	if force_bridge == null:
		force_bridge = ClassDB.instantiate("PolarForceLayout")
		force_bridge.on_completed(_layout_completed)
	if not force_bridge.submit(serialized, 300, generation):
		graph_error = "Could not start layout task"
		graph_status.text = graph_error
		return
	graph_status.text = "3D force layout · Computing…"

func _layout_completed() -> void:
	if browser_id!="mind_map":return
	if not _host_visible or not is_visible_in_tree():return
	if _interaction_active and force_bridge!=null and not force_bridge.is_running():
		# Consume a completed snapshot before requesting the next bounded batch.
		var ready:=str(force_bridge.poll())
		if not ready.is_empty():
			var completed=JSON.parse_string(ready)
			if completed is Dictionary:_finish_interaction_result(completed)
		if _interaction_active and (not _interaction_settled or not _interaction_command.is_empty()):
			_interaction_settled=false
			force_bridge.interaction_tick(JSON.stringify(_interaction_command),_interaction_generation)
			_interaction_command={}
		return
	if force_bridge == null:
		return
	var message := str(force_bridge.poll())
	if message.is_empty():
		return
	var result = JSON.parse_string(message)
	if result is Dictionary and bool(result.get("interactive",false)):
		_finish_interaction_result(result);return
	if not result is Dictionary or not bool(result.get("ok", false)):
		graph_error = str(result.get("error", "Invalid layout output")) if result is Dictionary else "Invalid layout output"
		graph_status.text = graph_error
		return
	var generation := int(result.get("generation", -1))
	if not navigation.accept_layout(generation, _job_fingerprint):
		return
	var coordinates := GraphProjection.coordinates(result)
	_graph_cache[_job_key] = coordinates
	_graph_cache_order.erase(_job_key)
	_graph_cache_order.append(_job_key)
	while _graph_cache_order.size() > 16:
		_graph_cache.erase(_graph_cache_order.pop_front())
	_apply_graph(coordinates, generation, _graph_fit_pending)

func _apply_graph(coordinates: Dictionary, generation: int, fit: bool) -> void:
	if not navigation.accept_layout(generation, _job_fingerprint):
		return
	var geometry_started := Time.get_ticks_usec()
	graph_coordinates = coordinates
	graph_view.set_graph(graph_data, coordinates)
	graph_view.set_selection(navigation.selected_id)
	if not _navigation_timing.is_empty(): _navigation_timing["graph_geometry_us"] = Time.get_ticks_usec() - geometry_started
	graph_ready = true
	applied_generation = generation
	graph_status.text = "%d definitions · %d relations\nRust · d3-force-3d / 300 ticks" % [graph_data.nodes.size(), graph_data.links.size()]
	if fit:
		graph_view.fit_all(false)
	_update_status()
	print("NAVIGATOR_GRAPH_READY generation=%d nodes=%d links=%d" % [generation, graph_data.nodes.size(), graph_data.links.size()])

func _update_status() -> void:
	var semantic_error := str(canvas_layout.get("semantic_error", ""))
	status.text = "%d definitions  ·  %d visible paths  ·  %s  ·  Read-only local snapshot" % [model.ordered_ids.size(), tree_layout.get("items", {}).size(), tree_layout.get("backend", "")]
	if not focus_anchor.is_empty():status.text="Relationship focus · %d exact nodes · %d role cards · %d references" % [_focus_layout.get("distinct_count",0),_focus_layout.get("items",{}).size(),_focus_layout.get("unresolved_count",0)]
	elif canvas_layout.has("distinct_count"):status.text+=" · %d direct neighborhood nodes" % int(canvas_layout.distinct_count)
	if int(tree_layout.get("hidden_count", 0)) > 0:
		status.text += "  ·  Expand or search hidden nodes"
	if not error.is_empty() or not semantic_error.is_empty():
		status.text = error if not error.is_empty() else "Relationship routing failed: " + semantic_error

	_publish_host_state()

func locate_selected() -> void:
	navigation.locate_node(navigation.selected_id, navigation.selected_occurrence)

func toggle_selected() -> void:
	if tree_layout.get("items", {}).has(navigation.selected_occurrence):
		navigation.toggle_occurrence(navigation.selected_occurrence, tree_layout)
	else:
		locate_selected()

func focus_selected() -> void:
	if tree_layout.get("items", {}).has(navigation.selected_occurrence):
		navigation.focus_occurrence(navigation.selected_occurrence, tree_layout)
	else:
		locate_selected()

func _toggle_graph_pin() -> void:
	if not graph_coordinates.has(navigation.selected_id):
		return
	var pins: Dictionary = navigation.pinned_positions.get("graph", {}).duplicate(true)
	if pins.has(navigation.selected_identity):
		pins.erase(navigation.selected_identity)
	else:
		var position_value: Vector3 = graph_coordinates[navigation.selected_id]
		pins[navigation.selected_identity] = [position_value.x, position_value.y, position_value.z]
	navigation.set_pinned_positions("graph", pins)

func _toggle_relations() -> void:
	_show_relations = relation_button.button_pressed
	navigation.set_camera("presentation", {"relations": _show_relations})
	_refresh_semantic_routes()

func _open_source(force_selection:bool=false)->void:
	var path:=_reader_source_path if reader_panel.visible and not force_selection else str(model.records.get(navigation.selected_id,{}).get("source",""))
	if path.is_empty():return
	path=ProjectSettings.globalize_path(path)
	if bool(get_meta("native_controls",false)):host_open_source.emit(path)
	else:OS.shell_open(path)

func _close_reader()->void:
	_set_reader_open(false)

func _open_reader()->void:
	canvas._stop_tween()
	_set_reader_open(true)

func _set_reader_open(open_value:bool)->void:
	if _reader_tween!=null:_reader_tween.kill()
	_reader_target_open=open_value
	if not open_value:_instruction_open=false;_instruction_classic_rect=Rect2()
	if open_value and _reader_progress<=0.001:
		if not _reader_custom:_refresh_reader()
		_reader_origin=canvas.screen_card_rect(canvas.key_for_node(navigation.selected_id,navigation.selected_occurrence))
		_reader_origin.position+=canvas.global_position-global_position
		if _reader_origin.size==Vector2.ZERO:_reader_origin=Rect2(size*0.5,Vector2.ONE)
		_reader_identity=navigation.selected_identity
		_reader_ghost_title.text=reader_title.text
	_responsive()
	reader_panel.show();_reader_ghost.show()
	reader_button.set_pressed_no_signal(open_value)
	var target_progress:=1.0 if open_value else 0.0
	if bool(preferences.reduce_motion) or not bool(preferences.navigation_animation):
		_animate_reader(target_progress);_finish_reader_transition();return
	_reader_tween=create_tween().set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_IN_OUT)
	_reader_tween.tween_method(_animate_reader,_reader_progress,target_progress,PresentationTheme.TRANSITION_SECONDS*absf(target_progress-_reader_progress))
	_reader_tween.tween_callback(_finish_reader_transition)
	_publish_host_state()

func _animate_reader(progress:float)->void:
	_reader_progress=progress
	var target_rect:=Rect2(reader_panel.position,reader_panel.size)
	_reader_ghost.position=_reader_origin.position.lerp(target_rect.position,progress)
	_reader_ghost.size=_reader_origin.size.lerp(target_rect.size,progress)
	_reader_ghost.modulate.a=1.0-progress
	reader_panel.modulate.a=progress

func _finish_reader_transition()->void:
	_reader_ghost.hide()
	reader_panel.visible=_reader_target_open
	if _reader_target_open:reader_close_button.grab_focus()
	else:canvas.grab_focus()
	_publish_host_state()

func _toggle_reader()->void:
	_set_reader_open(not _reader_target_open)

func _responsive() -> void:
	if size.x < 96 or size.y < 96: return
	if browser_id!="mind_map" or canvas==null:
		if depth_browser!=null:depth_browser.resize_view()
		call_deferred("_publish_host_state")
		return
	_compact = size.x < 1120
	navigation_panel.hide()
	canvas_panel.show()
	for control in [brand, back_button, forward_button, locate_button, reader_button, canvas_actions]:
		control.visible = size.y >= 420
	if get_meta("native_controls", false):
		for control in [toolbar, canvas_actions, breadcrumb, status]: control.hide()
	call_deferred("_place_maps")
	_fit_reader()
	if _instruction_open:call_deferred("_publish_host_state")
	locate_button.text = "Locate" if _compact else "Locate selected"

func _place_maps() -> void:
	if browser_id!="mind_map":return
	if not is_inside_tree() or canvas == null: return
	var area := Rect2(canvas.global_position - global_position, canvas.size)
	var height_scale := minf(0.78 if area.size.x<500 else 1.0, maxf(2,area.size.y-36) / (map_sizes[0].y+map_sizes[1].y))
	for index in map_panels.size():
		var panel = map_panels[index]
		var extent := Vector2(minf(map_sizes[index].x,maxf(1,area.size.x - 24)), map_sizes[index].y * height_scale)
		panel.size = extent
		canvas.content_inset_right=minf(extent.x+24,maxf(0,area.size.x-280))
		panel.position = area.position + Vector2(area.size.x - extent.x - 12, 12 if index == 0 else area.size.y - extent.y - 12)
		map_controls[index].position=panel.position+Vector2(maxf(0,extent.x-108),4)
		map_grips[index].position = panel.position + Vector2(0, extent.y - 24 if index == 0 else 0)
		tree_view.fit_all(false)

	canvas.occlusion_rects.clear()
	for panel in map_panels:canvas.occlusion_rects.append(Rect2(panel.global_position-canvas.global_position,panel.size))
	_sync_canvas_region()
	_trace_view_state()

func _toggle_graph_legend()->void:
	if _graph_legend!=null:
		_graph_legend.queue_free();_graph_legend=null;return
	_graph_legend=_panel();_graph_legend.z_index=25;add_child(_graph_legend)
	var column:=VBoxContainer.new();_graph_legend.add_child(column)
	column.add_child(_label("Relationship explorer",16))
	var sources:Dictionary=load("res://itree_visual_semantics.gd").sources(model)
	for source:String in sources:column.add_child(_label("●  "+source.get_file(),12,PresentationTheme.source_color(int(sources[source]))))
	column.add_child(_label("▰ Request   ◇ Decision   ▢ Task
⬡ Design   ▤ Rule   ▣ Root/group",12))
	column.add_child(_label("Color = source · outline = selected
Badge = original status · ? = unresolved",12))
	column.add_child(_label("→ Contains / Implements
– – → Uses     ···→ Context
══→ Defined-by     –·–→ Derived-from",12))
	column.add_child(_label("%d definitions · %d unresolved targets
%d explicitly hidden by scope/filter" % [model.ordered_ids.size(),int(graph_data.get("placeholder_count",0)),int(graph_data.get("hidden_count",0))],12))
	_button(column,"Close",_toggle_graph_legend)
	_graph_legend.position=Vector2(maxf(8,size.x-320),maxf(8,size.y-360))
	_graph_legend.size=Vector2(minf(300,size.x-16),0)

func _fit_reader() -> void:
	if _instruction_open:
		var pair:Dictionary=preload("res://node_instruction_layout.gd").compose(size,Vector2(minf(680,size.x-32),size.y-32))
		reader_panel.custom_minimum_size=Vector2.ZERO
		reader_panel.size=pair.detail.size;reader_panel.position=pair.detail.position
		_instruction_classic_rect=pair.editor
		return
	reader_panel.custom_minimum_size = Vector2.ZERO
	reader_panel.size = Vector2(maxf(1, minf(680, size.x - 32)), maxf(1, size.y - 32))
	reader_panel.position = Vector2(maxf(0, (size.x - reader_panel.size.x) * 0.5), 16)

func host_visibility_changed(visible_value:bool)->void:
	_host_visible=visible_value
	if visible_value:
		_sync_global_presentation()
		if not _pending_source_update.is_empty():
			var update:=_pending_source_update;_pending_source_update=[]
			_apply_source_candidate(update[0],update[1],update[2],update[3])
	for view in [canvas,tree_view,graph_view]:
		if view!=null:view.set_render_active(visible_value and browser_id=="mind_map")
	if depth_browser!=null:depth_browser.set_active(visible_value and browser_id!="mind_map")
	if not visible_value:
		for view in [canvas,tree_view,graph_view]:
			if view==null:continue
			if view._drag_active:_node_drag(view.role,view._drag_key,view.points.get(view._drag_key,Vector3.ZERO),"cancel")
		_interaction_active=false;_interaction_dragging=false
		if force_bridge!=null:force_bridge.cancel()
		if _reader_tween!=null:_reader_tween.kill()
		if _reader_ghost!=null:_reader_ghost.hide()
		if reader_panel!=null:
			_reader_progress=1.0 if _reader_target_open else 0.0
			reader_panel.modulate.a=_reader_progress;reader_panel.visible=_reader_target_open
		for view in [canvas,tree_view,graph_view]:
			if view==null:continue
			view._stop_tween()
			if view._mode_tween!=null:view._mode_tween.kill()
			if view._layout_tween!=null:view._layout_tween.kill()
			if view._trail_tween!=null:view._trail_tween.kill()
			view.clear_browse_path()
			view.dragging=false;view.drag_button=0;view._drag_active=false
		if has_meta("window_id"):
			_release_classic_ui()
			if depth_browser!=null and (not depth_browser.has_method("supports_context_rebinding") or not depth_browser.supports_context_rebinding()):
				_save_view_state();view_registry.unbind_browser(depth_browser)
				remove_child(depth_browser);depth_browser.queue_free();depth_browser=null;_active_package_browser=""
	else:
		_apply_preferences()
		for view in [canvas,tree_view,graph_view]:
			if view!=null:view._resize()
		if initialized and browser_id=="mind_map" and not graph_ready:_start_graph(false)
	_trace_view_state()

func needs_continuous_frames() -> bool:
	if not _host_visible:return false
	if _initializing or source_refresh_count != _source_presented_count:return true
	if browser_id!="mind_map" and depth_browser!=null:return depth_browser.needs_continuous_frames()
	if tree_strip!=null and (tree_strip.drag_active or tree_strip.is_animating()):return _host_visible
	if not initialized or not _host_visible: return false
	if (_interaction_active and not _interaction_settled) or (_reader_tween!=null and _reader_tween.is_running()):return true
	for view in [canvas, tree_view, graph_view]:
		if not view.is_visible_in_tree():continue
		if view.trail_frame_due():return true
		if view._edge_dirty or view._selection_edge_dirty or view._label_dirty: return true
		for tween in [view._camera_tween,view._mode_tween,view._layout_tween,view._inertia_tween,view._trail_tween]:
			if tween!=null and tween.is_running():return true
		for controls:Array in view.card_controls.values():
			for button in controls:
				if button._hover_tween!=null and button._hover_tween.is_running():return true
	return false

func _nav_tab_changed(index: int) -> void:
	# Each tab retains its camera. A newly displayed viewport only needs edge
	# widths/labels refreshed for its actual pixel extent.
	tree_view.set_process(true)
	graph_view.set_process(true)
	tree_view.viewport_3d.render_target_update_mode = SubViewport.UPDATE_ONCE
	graph_view.viewport_3d.render_target_update_mode = SubViewport.UPDATE_ONCE
	_view("tree" if index == 0 else "graph")._resize()
	if index == 1 and graph_ready and not navigation.camera_states.has("graph"):
		graph_view.call_deferred("fit_all", false)

func _search_changed(value: String) -> void:
	search_results.clear()
	var query := value.strip_edges().to_lower()
	if query.is_empty():
		search_popup.hide()
		return
	for id in model.ordered_ids:
		var record: Dictionary = model.records[id]
		if bool(record.get("external", false)):
			continue
		var haystack := (str(record.title) + " " + str(record.key) + " " + str(record.description)).to_lower()
		if haystack.contains(query):
			var index := search_results.add_item(str(record.title) + "  ·  " + str(record.key))
			search_results.set_item_metadata(index, str(id))
			if search_results.item_count >= 100:
				break
	if search_results.item_count > 0:
		search_results.select(0)
		search_popup.position = search.get_global_position() + Vector2(0, search.size.y + 4)
		search_popup.size = Vector2(minf(560, size.x - search_popup.position.x - 12), 300)
		if get_meta("native_controls", false):
			search_results.custom_minimum_size = Vector2(minf(540, size.x - 32), minf(280, size.y - 32))
			search_popup.size = search_results.custom_minimum_size + Vector2(20, 20)
			search_popup.position = Vector2(6, maxf(6, size.y - search_popup.size.y - 6))
		search_popup.show()
	else:
		search_popup.hide()

func _open_search_result(index: int) -> void:
	if index < 0 or index >= search_results.item_count:
		return
	var id := str(search_results.get_item_metadata(index))
	search_popup.hide()
	navigation.locate_node(id)
	canvas.grab_focus()

func _search_key(event: InputEvent) -> void:
	if not event is InputEventKey or not event.pressed:
		return
	if event.keycode == KEY_ESCAPE:
		search_popup.hide()
		canvas.grab_focus()
		search.accept_event()
	elif event.keycode in [KEY_DOWN, KEY_UP] and search_results.item_count > 0:
		var selected := search_results.get_selected_items()
		var index := selected[0] if not selected.is_empty() else 0
		index = clampi(index + (1 if event.keycode == KEY_DOWN else -1), 0, search_results.item_count - 1)
		search_results.select(index)
		search_results.ensure_current_is_visible()
		search.accept_event()

func _show_unresolved(record:Dictionary)->void:
	if record.get("reference") is Dictionary:_request_related_reference(record.reference)
	_reader_custom=true;_reader_source_path="";_reader_source_button.disabled=true
	reader_title.text=str(record.get("title","Not loaded"));reader_meta.text="Unresolved target · reference preserved"
	_reader_prose="This exact target is not available in the loaded local snapshot. Its original reference is preserved below."
	reader_content.text=_reader_prose;reader_source.text=JSON.stringify(record.get("reference",{}),"  ")
	reader_technical.hide();reader_raw_button.set_pressed_no_signal(false)
	reader_technical.text=reader_source.text;_open_reader()

func _show_edge(relations: Array) -> void:
	_reader_custom=true;_reader_source_path="";_reader_source_button.disabled=true
	reader_title.text = "Relationships · %d" % relations.size()
	reader_meta.text = "Direction · Type · Related node"
	var descriptions: Array[String] = []
	for edge in relations:
		var from_title := str(model.records.get(str(edge.get("from", "")), {}).get("title", edge.get("from", "")))
		var to_title := str(model.records.get(str(edge.get("to", "")), {}).get("title", "Unloaded target"))
		descriptions.append(from_title + " → " + to_title + "\n" + str(edge.get("type", "")) + " · " + str(edge.get("slot", "")))
	_reader_prose = "\n\n".join(descriptions)
	_reader_raw = JSON.stringify(relations, "  ")
	reader_raw_button.button_pressed = false
	reader_content.text = _reader_prose
	reader_source.text = "Explicit relationship selection"
	reader_technical.text = _reader_raw
	reader_technical.hide()
	_open_reader()

func _input(event: InputEvent) -> void:
	if browser_id!="mind_map":
		if event is InputEventKey and event.pressed and (event.ctrl_pressed or event.meta_pressed) and event.keycode==KEY_F:
			host_focus_search.emit();accept_event()
		return
	if event is InputEventKey and event.pressed and not event.echo and not OS.get_environment("POLAR_STATION_TRACE_DIR").is_empty() and get_viewport().gui_get_focus_owner() == canvas and event.keycode in [KEY_UP, KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_SPACE, KEY_ENTER]:
		_input_serial += 1
		_navigation_timing = {"serial": _input_serial, "key": OS.get_keycode_string(event.keycode), "received_us": Time.get_ticks_usec(), "tree_layout_and_geometry_us": 0, "graph_update_us": 0}
	if event is InputEventKey and event.keycode == KEY_SPACE:
		# Preserve the spatial overview's held-Space pan modifier without
		# stealing text input or the canvas's new discrete expansion action.
		if not event.pressed: graph_view.space_pressed = false
		elif get_viewport().gui_get_focus_owner() == graph_view and not reader_panel.visible:
			graph_view.space_pressed = true
	if event is InputEventGesture and not OS.get_environment("POLAR_STATION_TRACE_DIR").is_empty():
		set_meta("trace_event_position", [event.position.x,event.position.y])
		print("GRAPHICS_GESTURE_ROOT ",get_meta("view_id", "standalone")," ",event," hover=",get_viewport().gui_get_hovered_control())
		call_deferred("_trace_view_state")
	if event is InputEventMouseButton and event.pressed and reader_panel.visible:
		if not reader_panel.get_global_rect().has_point(get_global_mouse_position()): _close_reader()
	if event is InputEventKey and event.pressed and event.keycode==KEY_ESCAPE and get_viewport().gui_get_focus_owner() in [canvas,graph_view]:
		var cancelled:=false
		for view in [canvas,graph_view]:
			if view._drag_active:
				_node_drag(view.role,view._drag_key,view.points.get(view._drag_key,Vector3.ZERO),"cancel")
				view._drag_active=false;view.dragging=false;view.drag_button=0;cancelled=true
		if cancelled:accept_event();return
	if event is InputEventKey and event.pressed and event.keycode==KEY_ENTER and reader_panel.visible and not _reader_target_open:
		_open_reader();accept_event();return
	if event is InputEventKey and event.pressed and event.keycode == KEY_ESCAPE and reader_panel.visible:
		_close_reader()
		_responsive()
		accept_event()
		return
	if not event is InputEventKey or not event.pressed:
		return
	if (event.ctrl_pressed or event.meta_pressed) and event.keycode == KEY_F:
		if get_meta("native_controls", false):
			host_focus_search.emit()
			accept_event()
			return
		search.grab_focus()
		search.select_all()
		accept_event()
		return
	if reader_panel.visible or file_dialog.visible or search_popup.visible or get_viewport().gui_get_focus_owner() != canvas:
		return
	if event.echo and event.keycode not in [KEY_UP,KEY_DOWN,KEY_LEFT,KEY_RIGHT]:return
	# Pinyin/fullwidth punctuation is delivered as Unicode even while this
	# non-text Canvas owns focus. Normalize only the scoped command; editors
	# above still receive the untouched event and original Unicode.
	var tree_direction:=-1 if event.keycode in [KEY_BRACKETLEFT,0x3010,0xff3b] or event.unicode in [0x3010,0xff3b] else (1 if event.keycode in [KEY_BRACKETRIGHT,0x3011,0xff3d] or event.unicode in [0x3011,0xff3d] else 0)
	if tree_direction!=0 and not event.alt_pressed and not event.ctrl_pressed and not event.meta_pressed:
		_step_tree(tree_direction);accept_event();return
	if event.keycode==KEY_F and not event.alt_pressed and not event.ctrl_pressed and not event.meta_pressed:
		if event.shift_pressed:_fit_current_tree()
		else:_reveal_selected()
		accept_event();return
	if event.keycode==KEY_SPACE and event.shift_pressed:
		_toggle_relationship_focus();accept_event();return
	if event.keycode==KEY_ESCAPE and not focus_anchor.is_empty():
		_exit_relationship_focus();accept_event();return
	if event.keycode in [KEY_UP,KEY_DOWN,KEY_LEFT,KEY_RIGHT,KEY_SPACE,KEY_ENTER]:
		canvas._stop_tween()
		for view in [canvas,graph_view]:
			if view._drag_active:
				_node_drag(view.role,view._drag_key,view.points.get(view._drag_key,Vector3.ZERO),"cancel")
				view._drag_active=false;view.dragging=false;view.drag_button=0
	if event.alt_pressed and event.keycode in [KEY_LEFT, KEY_RIGHT]:
		navigation.travel(event.keycode == KEY_RIGHT)
	elif event.keycode in [KEY_UP, KEY_DOWN, KEY_LEFT, KEY_RIGHT]:
		navigation.navigate({KEY_UP: "up", KEY_DOWN: "down", KEY_LEFT: "left", KEY_RIGHT: "right"}[event.keycode], tree_layout)
	elif event.keycode == KEY_SPACE:
		navigation.toggle_neighbors(navigation.selected_occurrence, "children", tree_layout)
	elif event.keycode == KEY_ENTER:
		_open_reader()
	elif event.keycode == KEY_E:
		toggle_selected()
	elif event.keycode == KEY_HOME:
		canvas.fit_all(true)
	elif event.keycode == KEY_ESCAPE and str(navigation.scope.get("mode", "all")) != "all":
		locate_selected()
	else:
		return
	accept_event()

func _exit_tree() -> void:
	var window_id:String=str(get_meta("window_id",""))
	if _window_presenters.has(window_id) and _window_presenters[window_id].get_ref()==self:_window_presenters.erase(window_id)
	if _semantic_event:_events.unsubscribe(_semantic_event)
	if _source_quiet!=null:_source_quiet.stop();_source_quiet=null
	_save_view_state()
	if semantic_thread != null:
		semantic_thread.wait_to_finish()
		semantic_thread = null
	if force_bridge != null:
		force_bridge.cancel()

func _record_navigation_frame() -> void:
	if _navigation_timing.is_empty() or _navigation_timing.has("visible_frame_us"): return
	_navigation_timing["visible_frame_us"] = Time.get_ticks_usec() - int(_navigation_timing.received_us)
	_navigation_timing["layout_revision"] = layout_revision
	_trace_view_state()

# Development instrumentation, enabled only by the existing native trace directory.
func _trace_view_state() -> void:
	if not is_inside_tree(): return
	var directory := OS.get_environment("POLAR_STATION_TRACE_DIR")
	if directory.is_empty(): return
	DirAccess.make_dir_recursive_absolute(directory)
	var identity := str(get_meta("view_id", "standalone"))
	var data := {"engine_debug_build":OS.is_debug_build(),"tree_navigation":{"active":active_tree,"order":tree_order,"entries":tree_entries,"width":tree_strip.size.x if tree_strip!=null else 0,"anchor":focus_anchor,"role_counts":_focus_layout.get("groups",{}),"all_neighbor_count":canvas_layout.get("distinct_count",0)},"view_id":identity,"size":[size.x,size.y],"views":{},"maps":[],"grips":[],"event_position":get_meta("trace_event_position", []),"host":host_state(),"native_controls":get_meta("native_controls", false),"file_dialog_visible":file_dialog.visible,"search_visible":search_popup.visible}
	data["presentation"]={"window_id":str(get_meta("window_id","")),"browser_instance":depth_browser.get_instance_id() if depth_browser!=null else 0,"visible":_host_visible}
	if depth_browser!=null:data["browser"]=depth_browser.debug_state()
	if canvas==null:
		data["depth_pan"]=depth_browser.debug_state().get("pan",[]) if depth_browser!=null else []
		data["selection"]={"id":navigation.selected_id,"occurrence":navigation.selected_occurrence}
		data["motion"]={"continuous_frames":needs_continuous_frames()}
		_write_trace(directory,identity,data)
		return
	for view in [canvas,tree_view,graph_view]:
		data.views[view.role] = {"rect":[view.global_position.x,view.global_position.y,view.size.x,view.size.y],"camera":view.get_camera_state(),"node_count":view.points.size(),"edge_count":view.edge_paths.size()}
	data.tree_navigation.entries=[]
	if tree_strip!=null:data.tree_navigation["wheel"]=tree_strip.review_state()
	if tree_strip!=null:
		for entry:Dictionary in tree_entries:
			var item:Dictionary=entry.duplicate();var rect:Rect2=tree_strip.rows[str(entry.key)].row.get_global_rect()
			item["rect"]=[rect.position.x,rect.position.y,rect.size.x,rect.size.y];data.tree_navigation.entries.append(item)
	if depth_browser!=null:data["depth_pan"]=depth_browser.debug_state().get("pan",[])
	data["relationship_nodes"]=[]
	for id in graph_view.points:
		var item:Dictionary=graph_view.records[id];var p:Vector3=graph_view.points[id];var screen:Vector2=graph_view.screen_point(str(id))+graph_view.global_position
		data.relationship_nodes.append({"id":id,"identity":item.get("identity",""),"type":item.get("type",""),"shape":item.get("shape",""),"status":item.get("status",""),"source":item.get("source",""),"external":item.get("external",false),"point":[screen.x,screen.y],"world":[p.x,p.y,p.z]})
	data["relationship_edges"]=graph_data.get("links",[])
	data["motion"]={"path_points":canvas._trail_points.size(),"interaction_active":_interaction_active,"dragging":_interaction_dragging,"settled":_interaction_settled,"continuous_frames":needs_continuous_frames()}
	data["navigation_timing"] = _navigation_timing.duplicate(true)
	data["selection"] = {"id": navigation.selected_id, "occurrence": navigation.selected_occurrence}
	data["reader"] = {"visible": reader_panel.visible, "rect": [reader_panel.position.x,reader_panel.position.y,reader_panel.size.x,reader_panel.size.y], "technical": reader_technical.visible, "progress":_reader_progress, "opening":_reader_target_open}
	data["nodes"] = []
	for key in canvas.points:
		var point: Vector2 = canvas.screen_point(str(key)) + canvas.global_position
		var controls: Array = []
		for button in canvas.card_controls.get(key, []):
			controls.append({"text": button.text, "hint": button.tooltip_text, "visible": button.visible, "rect": [button.global_position.x,button.global_position.y,button.size.x,button.size.y]})
		data.nodes.append({"key": key, "id": canvas.records[key].node_id, "title": canvas.records[key].title,
			"point": [point.x,point.y], "role":canvas.records[key].get("role",""),"temporary":canvas.records[key].get("temporary",false),"source":canvas.records[key].get("source",""),"label_visible": canvas.labels[key].visible, "controls": controls})
	for panel in map_panels: data.maps.append([panel.position.x,panel.position.y,panel.size.x,panel.size.y])
	for grip in map_grips: data.grips.append([grip.position.x,grip.position.y,grip.size.x,grip.size.y])
	_write_trace(directory,identity,data)

func _write_trace(directory:String,identity:String,data:Dictionary)->void:
	var path:=directory.path_join("idea-view-"+identity.sha256_text()+".json")
	var file:=FileAccess.open(path+".tmp",FileAccess.WRITE)
	if file!=null:
		file.store_string(JSON.stringify(data));file.close()
		DirAccess.rename_absolute(path+".tmp",path)

func _initialize_tree_navigation()->void:
	var identities:Dictionary={}
	for root_id:String in model.source_roots:
		var uuid:=str(model.records[root_id].get("uuid",""));identities[uuid]=int(identities.get(uuid,0))+1
	var available:Dictionary={}
	for root_id:String in model.source_roots:
		var record:Dictionary=model.records[root_id];var uuid:=str(record.get("uuid",""))
		var key:="uuid:"+uuid if not uuid.is_empty() and identities[uuid]==1 else "path:"+str(record.source)
		available[key]={"key":key,"root":root_id,"source":str(record.source),"title":str(record.title)}
	tree_order=[]
	for key in saved_view_state.get_value("view","tree_order",[]):
		if available.has(str(key)) and key not in tree_order:tree_order.append(str(key))
	for key in available:
		if key not in tree_order:tree_order.append(key)
	tree_entries=[]
	for key in tree_order:tree_entries.append(available[key])
	tree_views=saved_view_state.get_value("view","tree_views",{})
	active_tree=str(saved_view_state.get_value("view","active_tree",tree_order[0] if not tree_order.is_empty() else ""))
	if active_tree not in tree_order:active_tree=str(tree_order[0])
	if tree_strip!=null:tree_strip.custom_minimum_size.x=clampf(float(saved_view_state.get_value("view","tree_width",120)),104,320)
	if tree_strip!=null:tree_strip.set_sources(tree_entries,active_tree)
	_switching_tree=true
	navigation.select_node(_tree_node(tree_entries[tree_order.find(active_tree)],tree_views.get(active_tree,{})))
	_switching_tree=false

func _active_source()->String:
	for entry:Dictionary in tree_entries:
		if str(entry.key)==active_tree:return str(entry.source)
	return ""

func _remember_tree()->void:
	if not initialized or _switching_tree or active_tree.is_empty() or canvas==null or not focus_anchor.is_empty():return
	var state:Dictionary=tree_views.get(active_tree,{})
	state["camera"]=canvas.get_camera_state()
	if str(model.records.get(navigation.selected_id,{}).get("source",""))==_active_source():
		state["identity"]=navigation.selected_identity;state["occurrence"]=navigation.selected_occurrence
		var record:Dictionary=model.records[navigation.selected_id]
		state["node_key"]=record.key;state["node_uuid"]=record.uuid;state["fingerprint"]=record.source_sha256;state["size_bytes"]=record.source_size_bytes
	tree_views[active_tree]=state

func _restore_tree_camera(animate:bool=false)->void:
	if canvas==null:return
	var state:Dictionary=tree_views.get(active_tree,{})
	if state.has("camera"):
		var camera_state:Dictionary=state.camera.duplicate(true)
		var old_progress:=1.0 if camera_state.get("tilted",false) else 0.0
		camera_state.target[2]-=float(tree_order.find(active_tree))*canvas.SOURCE_LAYER_GAP*(canvas.layer_progress-old_progress)
		camera_state.tilted=preferences.canvas_mode=="layered"
		canvas.restore_camera(camera_state,animate)
	else:
		var key:String=canvas.key_for_node(navigation.selected_id,navigation.selected_occurrence)
		canvas.center_key(key,true)

func _switch_tree(key:String)->void:
	if key==active_tree or key not in tree_order:return
	if not focus_anchor.is_empty():_exit_relationship_focus()
	_remember_tree();active_tree=key;_switching_tree=true
	var entry:Dictionary=tree_entries[tree_order.find(key)]
	var remembered:Dictionary=tree_views.get(key,{})
	var id:String=_tree_node(entry,remembered)
	navigation.select_node(id)
	_source_base_layout=_canvas_scope(tree_layout);_refresh_selection();_restore_tree_camera(true);_switching_tree=false;_remember_tree()
	tree_strip.select(active_tree);_save_view_state();_publish_host_state()

func _step_tree(amount:float)->void:
	if not focus_anchor.is_empty():return
	_tree_scroll+=amount
	var steps:=int(_tree_scroll)
	if steps==0:return
	_tree_scroll-=steps
	var index:=tree_order.find(active_tree)
	var next:=clampi(index+steps,0,tree_order.size()-1)
	if next!=index:_switch_tree(str(tree_order[next]))

func _tree_node(entry:Dictionary,remembered:Dictionary)->String:
	var id:String=str(model.resolve(str(remembered.get("identity",""))))
	if not id.is_empty() and str(model.records[id].source)==str(entry.source):return id
	# A renamed exact local source keeps byte fingerprint, node key/UUID and
	# byte length. This is the existing local preview identity check, not a
	# logical UUID or runtime version selection fallback.
	for candidate:String in model.ordered_ids:
		var record:Dictionary=model.records[candidate]
		if str(record.source)==str(entry.source) and str(record.source_sha256)==str(remembered.get("fingerprint","")) and int(record.source_size_bytes)==int(remembered.get("size_bytes",-2)) and str(record.key)==str(remembered.get("node_key","")) and str(record.uuid)==str(remembered.get("node_uuid","")):return candidate
	return str(entry.root)

func _reorder_tree(key:String,before:String)->void:
	if key==before or key not in tree_order or before not in tree_order:return
	_remember_tree();var old_order:Array=tree_order.duplicate();var old_point:Vector3=canvas.points.get(canvas.key_for_node(navigation.selected_id),Vector3.ZERO)
	tree_order.erase(key);tree_order.insert(tree_order.find(before),key)
	for source_key in tree_views:
		var state:Dictionary=tree_views[source_key]
		if not state.has("camera"):continue
		var delta:=tree_order.find(source_key)-old_order.find(source_key);var target:Array=state.camera.get("target",[0,0,0])
		target[0]+=float(delta)*0.42;target[2]-=float(delta)*canvas.SOURCE_LAYER_GAP*(1.0 if state.camera.get("tilted",false) else 0.0)
		state.camera.target=target
	var entries:Dictionary={}
	for entry:Dictionary in tree_entries:entries[str(entry.key)]=entry
	tree_entries=[]
	for id in tree_order:tree_entries.append(entries[id])
	if tree_strip!=null:tree_strip.set_sources(tree_entries,active_tree)
	_switching_tree=true;_source_base_layout=_canvas_scope(tree_layout);_refresh_selection()
	var shift:Vector3=canvas.points.get(canvas.key_for_node(navigation.selected_id),old_point)-old_point
	canvas._move_camera(canvas.target+shift,canvas.view_size,canvas.orbit_distance,false);_switching_tree=false;_remember_tree();_save_view_state()

func _canvas_scope(layout:Dictionary)->Dictionary:
	var result:Dictionary=layout.duplicate(true)
	var origins:Dictionary={};var indices:Dictionary={}
	for i in tree_entries.size():
		var entry:Dictionary=tree_entries[i];indices[entry.source]=i
		var root_key:=TreeProjection.root_occurrence(model,str(entry.root))
		origins[entry.source]=layout.positions.get(root_key,Vector2.ZERO)
	var offsets:Dictionary={}
	for key in result.items.keys():
		var item:Dictionary=result.items[key];var source:=str(model.records.get(str(item.node_id),{}).get("source",item.get("source","")))
		if preferences.canvas_mode=="two_d" and source!=_active_source():
			result.items.erase(key);result.positions.erase(key);result.sizes.erase(key);continue
		var index:=int(indices.get(source,tree_order.find(active_tree)))
		item["layer_index"]=index;item["source"]=source;item["layer_distance"]=abs(index-tree_order.find(active_tree))
		var offset:Vector2=-Vector2(origins.get(source,Vector2.ZERO))+Vector2(float(index)*42.0,0)
		offsets[key]=offset;result.positions[key]+=offset
	result.edges=result.edges.filter(func(e):return result.items.has(e.from) and result.items.has(e.to))
	var routes:Array=[]
	for route:Dictionary in result.get("routes",[]):
		if not result.items.has(route.from) or not result.items.has(route.to):continue
		var points:PackedVector2Array=route.points.duplicate()
		for i in points.size():points[i]+=Vector2(offsets[route.from]).lerp(Vector2(offsets[route.to]),float(i)/maxf(1,points.size()-1))
		route.points=points;routes.append(route)
	result.routes=routes;result["geometry_key"]=str(layout_revision)+":"+active_tree+":"+str(preferences.canvas_mode)+":"+JSON.stringify(tree_order)
	canvas.active_layer=tree_order.find(active_tree)
	return result

func _refresh_relationship_projection()->void:
	if tree_layout.is_empty() or active_tree.is_empty():return
	if not focus_anchor.is_empty():
		if canvas_layout.get("focus_anchor","")==focus_anchor:return
		_focus_layout=TreeProjection.relationship_focus(model,focus_anchor,tree_layout.items.get(navigation.selected_occurrence,{}))
		_focus_layout["temporary_edges"]=_focus_layout.edges.duplicate(true)
		_focus_layout["focus_anchor"]=focus_anchor;_focus_layout["geometry_key"]="focus:"+focus_anchor
		canvas_layout=_focus_layout;canvas.set_tree(canvas_layout);_semantic_cache_key="";return
	var selected_source:=str(model.records.get(navigation.selected_id,{}).get("source",""))
	if not _switching_tree and not selected_source.is_empty() and selected_source!=_active_source():
		_remember_tree()
		for entry:Dictionary in tree_entries:
			if str(entry.source)==selected_source:active_tree=str(entry.key);tree_strip.select(active_tree);_source_base_layout=_canvas_scope(tree_layout);break
	var result:Dictionary=_source_base_layout.duplicate(true)
	var neighborhood:Dictionary=TreeProjection.relationship_focus(model,navigation.selected_id,tree_layout.items.get(navigation.selected_occurrence,{}),false)
	var visible:Dictionary={}
	for key in result.items:
		var id:=str(result.items[key].node_id)
		if not visible.has(id) or key==navigation.selected_occurrence:visible[id]=key
	var center:Vector2=result.positions.get(visible.get(navigation.selected_id,""),Vector2.ZERO)
	var remap:Dictionary={}
	for key in neighborhood.items:
		var item:Dictionary=neighborhood.items[key];var id:=str(item.node_id)
		var foreign:=str(item.get("source",""))!=_active_source() and not bool(item.get("external",false))
		var existing:=str(visible.get(id,key))
		if foreign:existing="related:"+navigation.selected_identity.sha256_text()+":"+id
		remap[key]=existing
		if not result.items.has(existing):
			item["layer_index"]=_source_index(str(item.get("source","")));item["temporary"]=true
			if int(item.layer_index)<0:item.layer_index=tree_order.find(active_tree)
			result.items[existing]=item
			var local:Vector2=neighborhood.positions[key]
			if str(item.role)=="related":local.y+=180.0
			result.positions[existing]=center+local*0.75;result.sizes[existing]=neighborhood.sizes[key]
		visible[id]=existing
	var related:Dictionary={}
	for item:Dictionary in neighborhood.items.values():related[str(item.node_id)]=true
	for item:Dictionary in result.items.values():
		item["direct_neighbor"]=related.has(str(item.node_id)) and (str(item.get("source",""))==_active_source() or bool(item.get("temporary",false)))
		item["context_dim"]=not bool(item.direct_neighbor)
	result["temporary_edges"]=[]
	for edge:Dictionary in neighborhood.edges:
		var e:=edge.duplicate(true);e.from=remap[edge.from];e.to=remap[edge.to]
		if not result.edges.any(func(base):return base.from==e.from and base.to==e.to and str(base.relation.get("slot",""))==str(e.relation.get("slot",""))):result.temporary_edges.append(e)
	result["distinct_count"]=neighborhood.distinct_count
	canvas_layout=result;canvas.set_tree(canvas_layout);_semantic_cache_key=""

func _source_index(source:String)->int:
	for i in tree_entries.size():
		if str(tree_entries[i].source)==source:return i
	return -1

func _fit_current_tree()->void:
	var keys:Array=canvas.records.keys().filter(func(key):return str(canvas.records[key].get("source",""))==_active_source())
	canvas.fit_keys(keys,true,false)

func _toggle_relationship_focus()->void:
	if focus_anchor.is_empty():_enter_relationship_focus()
	else:_exit_relationship_focus()

func _enter_relationship_focus(reanchor:bool=false)->void:
	if navigation.selected_id.is_empty():return
	if focus_anchor.is_empty():
		_remember_tree();_focus_return={"navigation":navigation.snapshot(),"tree":active_tree,"views":tree_views.duplicate(true)}
	elif not reanchor:return
	focus_anchor=navigation.selected_id;canvas_layout={};_refresh_selection()
	canvas.center_key(canvas.key_for_node(focus_anchor),true);_publish_host_state()

func _exit_relationship_focus()->void:
	if focus_anchor.is_empty():return
	var previous:Dictionary=_focus_return;focus_anchor="";_focus_layout={};_focus_return={}
	active_tree=str(previous.tree);tree_views=previous.views;_switching_tree=true
	navigation.restore(previous.navigation)
	_rebuild_tree()
	_source_base_layout=_canvas_scope(tree_layout);_refresh_selection();_restore_tree_camera();_switching_tree=false;_remember_tree();tree_strip.select(active_tree);_publish_host_state()

func _place_focus_headers()->void:
	if focus_anchor.is_empty():
		for header:Label in _focus_headers.values():header.hide()
		return
	var regions:Dictionary={}
	for key in canvas.records:
		var item:Dictionary=canvas.records[key];var role:=str(item.get("role",""))
		if role not in ["parent","child","sibling","related","focus"]:continue
		var group:=str(item.get("group","")) if role=="sibling" else ""
		var token:=role+":"+group
		var rect:Rect2=canvas.screen_card_rect(str(key))
		if not regions.has(token):regions[token]={"rect":rect,"count":0,"role":role,"group":group}
		else:regions[token].rect=regions[token].rect.merge(rect)
		regions[token].count+=1
	for token in regions:
		var region:Dictionary=regions[token]
		if not _focus_headers.has(token):
			var label:=_label("",14);label.mouse_filter=Control.MOUSE_FILTER_IGNORE;label.z_index=0;canvas.add_child(label);_focus_headers[token]=label
		var header:Label=_focus_headers[token]
		header.text={"parent":"Parents","child":"Children","related":"Other relations","sibling":"Siblings","focus":"Anchor"}.get(region.role,"Relations")+" · "+str(region.count)
		if region.role=="sibling":header.text+=" / "+str(model.records.get(str(region.group),{}).get("title",""))
		header.position=region.rect.position-Vector2(0,25)
		header.visible=Rect2(Vector2.ZERO,canvas.size).intersects(Rect2(header.position,header.size))


func _switch_browser(next: String) -> void:
	if not view_registry.browsers.has(next):return
	if not _host_visible:
		browser_id=next
		return
	if next==browser_id and ((next=="mind_map" and canvas!=null) or (next!="mind_map" and depth_browser!=null)):return
	var restoring_classic:bool=next=="mind_map" and browser_id==next and canvas==null
	var was_reading: bool=_reader_target_open if restoring_classic else host_state().reader_open
	var was_instruction:bool=_instruction_open if restoring_classic else false
	_instruction_open=false;_instruction_classic_rect=Rect2()
	_variant_switching=true
	var candidate=depth_browser
	if next!="mind_map" and next!=_active_package_browser:
		candidate=_borrow_window_browser(next)
		if candidate==null:candidate=view_registry.create_browser(next,model,navigation,browser_views.get(next,{}))
		if candidate==null:
			_variant_switching=false;status.text=view_registry.error;_publish_host_state();return
		if candidate.get_parent()==null:add_child(candidate)
		if not candidate.initialized:
			candidate.queue_free();_variant_switching=false;status.text="Browser could not load; current view retained.";_publish_host_state();return
		candidate.state_changed.connect(_publish_host_state)
		candidate.instruction_requested.connect(_request_instruction)
		candidate.set_preferences(preferences)
		candidate.restore_view(browser_views.get(next,{}))

	if browser_id=="mind_map":
		_remember_tree()
		if was_reading and reader_panel!=null:_set_reader_open(false)
		if force_bridge!=null:force_bridge.cancel()
	elif depth_browser!=null:
		browser_views[_active_package_browser]=depth_browser.snapshot()
		if depth_browser.browser_state().reader_open:depth_browser.execute_action("close_reader")
	if next!="mind_map" and candidate!=depth_browser:
		if depth_browser!=null:remove_child(depth_browser);depth_browser.queue_free()
		depth_browser=candidate;_active_package_browser=next
	if next=="mind_map":
		if depth_browser!=null:
			remove_child(depth_browser);depth_browser.queue_free();depth_browser=null;_active_package_browser=""
		_build_classic_ui()
	else:_release_classic_ui()
	search_popup.z_index=50
	browser_id=next
	var classic: bool=next=="mind_map"
	if body!=null:body.visible=classic
	for panel in map_panels:panel.visible=classic
	for grip in map_grips:grip.visible=classic
	for controls in map_controls:controls.visible=classic
	if _graph_legend!=null:_graph_legend.visible=classic
	for view in [canvas,tree_view,graph_view]:
		if view==null:continue
		view.process_mode=Node.PROCESS_MODE_INHERIT if classic else Node.PROCESS_MODE_DISABLED
		view.set_render_active(classic and _host_visible)
	if depth_browser!=null:depth_browser.set_active(not classic and _host_visible)
	_variant_switching=false
	if classic:
		_initialize_tree_navigation()
		# Match the selected source without selecting its remembered node.
		for entry in tree_entries:
			if entry.source==model.records[navigation.selected_id].source:active_tree=str(entry.key);break
		tree_strip.select(active_tree)
		_rebuild_tree();_apply_preferences();_refresh_selection();_restore_tree_camera();_responsive()
		if not graph_ready:_start_graph(false)
		canvas.reveal_readable(canvas.key_for_node(navigation.selected_id,navigation.selected_occurrence),"reading_priority")
		if was_reading:_open_reader()
		if was_instruction:_instruction_open=true;_responsive()
	else:
		depth_browser.sync_selection()
		if was_reading:depth_browser.show_details()
	_apply_visual_theme(theme_id)
	_register_window_presenter()
	_save_view_state();_publish_host_state()

func _apply_visual_theme(value: String) -> void:
	if not view_registry.themes.has(value):value="default"
	if not view_registry.activate_entry(view_registry.themes[value]):return
	if ResourceLoader.exists("res://itree_theme_adapter.gd"):
		var adapter=load("res://itree_theme_adapter.gd")
		if adapter.apply(self,value,str(view_registry.themes[value].entry)):theme_id=value
		else:status.text="Theme unavailable; current appearance retained."


func _source_frame_presented() -> void:
	_source_presented_count = source_refresh_count

func _start_source_events() -> void:
	if source_paths.is_empty(): return
	if not RenderingServer.frame_post_draw.is_connected(_source_frame_presented):
		RenderingServer.frame_post_draw.connect(_source_frame_presented)
	if not ClassDB.class_exists("PolarFileEvents"):
		status.text = "Automatic refresh unavailable: file event adapter is missing."
		_publish_host_state();return
	_source_quiet = preload("res://addons/polar_graphics/deadline.gd").new(_reload_source_files,.12)
	_source_events = ClassDB.instantiate("PolarFileEvents")
	_source_events.files_changed.connect(func(): _source_quiet.start())
	_source_events.watch_failed.connect(func(message):
		status.text = "File watch failed: " + str(message);_publish_host_state())
	add_child(_source_events)
	var paths := PackedStringArray()
	for path in source_paths: paths.append(ProjectSettings.globalize_path(path).simplify_path())
	var failure: String = _source_events.watch_files(paths)
	if not failure.is_empty():
		status.text = "Automatic refresh unavailable: " + failure;_publish_host_state()

func _reload_source_files() -> void:
	if not initialized or source_paths.is_empty(): return
	var candidate = ViewModel.new()
	if not candidate.load_files(source_paths):
		_source_reload_failed = true
		status.text = "File changed but cannot be loaded; current view retained: " + str(candidate.error)
		_publish_host_state();return
	var next_navigation = Navigation.new();next_navigation.initialize(candidate)
	if next_navigation.snapshot_fingerprint == navigation.snapshot_fingerprint:
		if _source_reload_failed:
			_source_reload_failed = false;status.text = "Source file is valid again.";_publish_host_state()
		return
	var migrated_ids: Dictionary = next_navigation.migrate_from(navigation)
	var next_layout: Dictionary = TreeProjection.build(candidate, next_navigation.roots, next_navigation.expanded,
		next_navigation.budget, "balanced", next_navigation.selected_occurrence)
	if not str(next_layout.get("error", "")).is_empty():
		status.text = "File changed but layout failed; current view retained: " + str(next_layout.error)
		_publish_host_state();return
	_apply_source_candidate(candidate, next_navigation, next_layout, migrated_ids)
	_start_related_loading()

func _apply_source_candidate(candidate, next_navigation, next_layout: Dictionary, migrated_ids: Dictionary) -> void:
	if not _host_visible:
		_pending_source_update=[candidate,next_navigation,next_layout,migrated_ids]
		return
	var first_load := not initialized
	var cameras: Dictionary = {}
	for role in ["canvas", "tree", "graph"]:
		if _view(role)!=null:cameras[role] = _view(role).get_camera_state()
	var browser_state: Dictionary = depth_browser.snapshot() if depth_browser != null else {}
	var selected_browser := browser_id
	var reading: bool = host_state().reader_open
	_save_view_state()
	_variant_switching = true
	if force_bridge != null: force_bridge.cancel()
	if navigation.changed.is_connected(_on_navigation_changed): navigation.changed.disconnect(_on_navigation_changed)
	model = candidate;navigation = next_navigation
	error = "";initialized = true
	navigation.changed.connect(_on_navigation_changed)
	var selected_route: Dictionary = navigation.last_route.duplicate(true)
	_initialize_tree_navigation()
	navigation._commit_selection(selected_route)
	prepared_layout = next_layout;overview_layout = {};focus_anchor = "";_focus_return = {};_focus_layout = {}
	graph_ready = false;applied_generation = -1
	browser_id = selected_browser
	_variant_switching = false
	_rebuild_tree(selected_browser=="mind_map")
	if selected_browser=="mind_map":
		_refresh_selection();_start_graph(first_load)
	if first_load:
		_apply_preferences()
		if canvas!=null:
			canvas.fit_keys(canvas.points.keys().filter(func(key): return str(canvas.records[key].get("source",""))==_active_source()),false,true)
			tree_view.fit_all(false)
	else:
		for role in cameras: _view(role).restore_camera(cameras[role], false)
	if selected_browser != "mind_map":
		# This is an explicit validated version migration, not exact restore.
		browser_state["fingerprint"] = navigation.snapshot_fingerprint
		browser_state["source_id"] = migrated_ids.get(str(browser_state.get("source_id", "")), "")
		var source_views: Dictionary = {}
		for id in browser_state.get("per_source_views", {}):
			if migrated_ids.has(id): source_views[migrated_ids[id]] = browser_state.per_source_views[id]
		browser_state["per_source_views"] = source_views
		browser_views[selected_browser] = browser_state
		if depth_browser!=null:
			view_registry.update_browser(depth_browser,model,navigation,browser_state)
			depth_browser.restore_view(browser_state)
		else:_switch_browser(selected_browser)
		if reading: depth_browser.show_details()
	elif reading: _open_reader()
	for view in [canvas,tree_view,graph_view]:
		if view!=null:view.set_render_active(_host_visible and browser_id=="mind_map")
	_source_reload_failed = false
	source_refresh_count += 1
	status.text = "Updated from file."
	_save_view_state();_publish_host_state();_trace_view_state()

func _start_related_loading()->void:
	if source_paths.is_empty():return
	if source_entry_paths.is_empty():source_entry_paths=source_paths.duplicate()
	if source_loader==null:
		source_loader=preload("res://itree_source_loader.gd").new();add_child(source_loader)
		source_loader.changed.connect(_publish_host_state)
		source_loader.apply_candidate=_related_model_ready
	source_loader.configure(model,source_entry_paths,source_paths)

func _related_model_ready(candidate,paths:PackedStringArray)->bool:
	var next_navigation=Navigation.new();next_navigation.initialize(candidate)
	var migrated:Dictionary=next_navigation.migrate_from(navigation)
	var layout:Dictionary=TreeProjection.build(candidate,next_navigation.roots,next_navigation.expanded,next_navigation.budget,"balanced",next_navigation.selected_occurrence)
	if not str(layout.get("error","")).is_empty():
		status.text="Related tree layout failed; current view retained: "+str(layout.error);_publish_host_state();return false
	source_paths=paths
	_apply_source_candidate(candidate,next_navigation,layout,migrated)
	if _source_events!=null:_source_events.watch_files(source_paths)
	return true

func _view_catalog_changed()->void:
	var current:Dictionary=view_registry.current_selection()
	var browser:String=str(current.browser) if view_registry.browsers.has(str(current.browser)) else "depth_forest"
	var theme:String=str(current.theme) if view_registry.themes.has(str(current.theme)) else "default"
	if browser!=str(current.browser) or theme!=str(current.theme):view_registry.select_presentation(browser,theme)
	_publish_host_state()

func _request_related_reference(reference:Dictionary)->void:
	if source_loader==null or not source_loader.request_reference(reference):
		status.text="Choose the related source in View; no unique file hint is available."
	else:status.text="Loading referenced tree…"
	_publish_host_state()

func _select_global_browser(value:String)->void:
	var current:Dictionary=view_registry.current_selection()
	if not view_registry.select_presentation(value,str(current.theme)):status.text="Browser unavailable; current view retained."

func _select_global_theme(value:String)->void:
	var current:Dictionary=view_registry.current_selection()
	if not view_registry.select_presentation(str(current.browser),value):status.text="Theme unavailable; current appearance retained."

func _sync_global_presentation()->void:
	var current:Dictionary=view_registry.current_selection()
	if not _host_visible:
		if str(current.browser)!=browser_id or str(current.theme)!=theme_id:
			_save_view_state();_release_classic_ui()
			if depth_browser!=null:remove_child(depth_browser);depth_browser.queue_free();depth_browser=null;_active_package_browser=""
		browser_id=str(current.browser);theme_id=str(current.theme)
		_publish_host_state()
		return
	if initialized:_switch_browser(str(current.browser))
	_apply_visual_theme(str(current.theme))
	_publish_host_state()

func _release_classic_ui()->void:
	if canvas==null:return
	_remember_tree()
	if force_bridge!=null:force_bridge.cancel();force_bridge=null
	if semantic_thread!=null:semantic_thread.wait_to_finish();semantic_thread=null
	semantic_pending={};graph_ready=false
	if _reader_tween!=null:_reader_tween.kill();_reader_tween=null
	for node in _classic_roots:
		if is_instance_valid(node):node.get_parent().remove_child(node);node.queue_free()
	_classic_roots.clear()
	if _graph_legend!=null:_graph_legend.queue_free();_graph_legend=null
	for field in ["canvas","tree_view","graph_view","tree_strip","body","navigation_panel","nav_tabs","graph_status","canvas_panel","content_box","canvas_actions","breadcrumb","reader_panel","_reader_ghost","_reader_ghost_title","reader_close_button","_reader_source_button","reader_raw_button","reader_scroll","reader_title","reader_meta","reader_content","reader_source","reader_technical","relation_list","tilt_button","relation_button"]:set(field,null)
	map_panels.clear();map_grips.clear();map_controls.clear();_focus_headers.clear()

func _register_window_presenter()->void:
	var window_id:String=str(get_meta("window_id",""))
	if not window_id.is_empty() and (depth_browser!=null or canvas!=null):_window_presenters[window_id]=weakref(self)

func _borrow_window_browser(next:String):
	var window_id:String=str(get_meta("window_id",""))
	if window_id.is_empty():return null
	var previous=_window_presenters.get(window_id)
	var owner=previous.get_ref() if previous!=null else null
	if owner==null or owner==self:return null
	if owner._host_visible:return null # Host must hide the previous Tab before switching.
	if owner._active_package_browser!=next or owner.depth_browser==null:return null
	var browser=owner.depth_browser
	owner._save_view_state()
	browser.set_active(false)
	browser.state_changed.disconnect(owner._publish_host_state)
	browser.instruction_requested.disconnect(owner._request_instruction)
	owner.view_registry.unbind_browser(browser)
	owner.depth_browser=null;owner._active_package_browser=""
	owner._trace_view_state()
	browser.reparent(self,false)
	view_registry.bind_browser(browser,model,navigation,browser_views.get(next,{}))
	return browser
