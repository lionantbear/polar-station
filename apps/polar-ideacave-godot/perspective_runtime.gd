extends Control
## Shared Perspective/Group scene runtime. Hosts supply immutable loaded data
## and the navigation owner; native and standalone entrypoints share geometry.
const Model=preload("res://itree_view_model.gd")
const Navigation=preload("res://itree_navigation_state.gd")
const Adapter=preload("res://perspective_model.gd")
const Browser=preload("res://perspective_browser.gd")
const Geometry=preload("res://depth_forest/forest_geometry.gd")
signal perspective_committed
signal perspective_failed(message:String)
var native_completion:=false
var _completion_events
var _completion_token:=0
var _geometry_thread:Thread
var _request_generation:=0
var _pending_geometry:Dictionary={}
var _geometry_cache:Dictionary={}
var _desired_view:Dictionary={}
const Graph=preload("res://itree_graph_projection.gd")
var original=Model.new()
var backend
var browser
var map_view
var projection:Dictionary={}
var profiles:Array=[]
var current_perspective:=1
var levels:Dictionary={}
var shared_navigation
var models:Dictionary={}
var current_model
var current_navigation
var _memories:Dictionary={}
var status:Label
var column:VBoxContainer
var search_window:PopupPanel
var search_input:LineEdit
var search_results:ItemList
var _results:Array=[]
var filter_menu:PopupMenu
var filter_button:Button
var filter_id:="filter-clear"
var matches:Array=[]
var search_reveal:=""
var input:LineEdit
var graph_data:Dictionary={}
var graph_coordinates:Dictionary={}
var map_built:=false
var map_label:Label
var last_error:=""
var ready_for_review:=false
var timing:Dictionary={}
var last_switch_timing:Dictionary={}
var _filter_by_perspective:Dictionary={}

func configure_view(value,config_text:String,controller=null)->bool:
	original=value;shared_navigation=controller
	graph_data=Graph.build(original)
	backend=ClassDB.instantiate("PolarIdeaPerspective")
	if backend==null:_fail("Perspective extension missing");return false
	var result=JSON.parse_string(backend.configure(config_text,JSON.stringify({"records":original.records,"relations":original.relation_edges,"roots":original.source_roots})))
	if not result is Dictionary or not bool(result.get("ok",false)):_fail(str(result));return false
	profiles=JSON.parse_string(backend.perspectives()).filter(func(profile):return bool(profile.get("available",true)))
	if profiles.is_empty():_fail("No exact perspective root is loaded");return false
	return true

func _fail(message:String)->void:
	last_error=message
	if status:status.text=message;status.show()
	push_error(message)
	perspective_failed.emit(message)

func switch_perspective(index:int)->bool:
	if profiles.is_empty():return false
	index=wrapi(index,0,profiles.size())
	return _request_projection(index,int(levels.get(str(profiles[index].id),1)))


func change_granularity(level:int)->bool:
	var bounded:=clampi(level,0,int(projection.max_level))
	return _request_projection(current_perspective,bounded)


func _request_projection(index:int,level:int)->bool:
	var started:=Time.get_ticks_usec()
	var target:Dictionary={"index":index,"level":level}
	if _desired_view==target:return false
	_request_generation+=1;_pending_geometry={};_desired_view=target
	if browser!=null and current_model!=null and current_model.original==original and current_navigation==shared_navigation and index==current_perspective and level==int(projection.level):
		browser.select_perspective(index);return false
	var id:String=str(profiles[index].id)
	var next=JSON.parse_string(backend.projection(id,level))
	if not next is Dictionary or not bool(next.get("ok",false)):_fail(str(next));return false
	if browser==null:return _commit_projection(index,next,null,started)
	var model_key:=id+"/"+str(next.level)
	if not models.has(model_key):
		var adapter=Adapter.new();adapter.configure(original,next);models[model_key]=adapter
	var model=models[model_key]
	var sources:Array=Browser.sources_for(model,next)
	var members:Dictionary={}
	for group in model.display_groups:members[str(group)]=model.display_groups[group]
	var sizes:Dictionary=browser.forest.measure_geometry(model,members,graph_data)
	var key:String=Geometry.cache_key(model,sources,members,model.display_membership,sizes)
	var cached=ResourceLoader.get_cached_ref(key)
	if cached!=null:return _commit_projection(index,next,cached,started)
	_pending_geometry={"index":index,"next":next,"model":model,"sources":sources,"members":members,"sizes":sizes,"key":key,"generation":_request_generation,"started":started}
	browser.select_perspective(index)
	_start_geometry()
	return true


func _start_geometry()->void:
	if _geometry_thread!=null or _pending_geometry.is_empty():return
	var job:=_pending_geometry;_pending_geometry={}
	_geometry_thread=Thread.new()
	var notify:=Callable()
	if native_completion:
		if _completion_events==null:_completion_events=ClassDB.instantiate("PolarGraphicsEvents")
		if _completion_token==0:_completion_token=_completion_events.subscribe(_geometry_ready)
		notify=_completion_events.waker(_completion_token)
	var error:=_geometry_thread.start(_prepare_geometry.bind(job,graph_data,notify))
	if error!=OK:_geometry_thread=null;_desired_view={};_fail("Could not start geometry preparation")


func _prepare_geometry(job:Dictionary,graph:Dictionary,notify:Callable)->Dictionary:
	var prepared=Geometry.prepare(job.model,job.sources,job.members,job.model.display_membership,job.sizes,graph)
	# Godot owns this thread-safe deferred delivery; no frame polling. The
	# worker touches only its numeric preparation Resource and immutable input.
	if notify.is_valid():notify.call()
	else:_geometry_ready.call_deferred()
	return {"job":job,"geometry":prepared}


func _geometry_ready()->void:
	if _geometry_thread==null:return
	var result:Dictionary=_geometry_thread.wait_to_finish();_geometry_thread=null
	var job:Dictionary=result.job
	var prepared=result.geometry
	if prepared.last_route_error.is_empty() and job.model.original==original:_geometry_cache[job.key]=prepared
	if job.generation==_request_generation:
		if prepared.last_route_error.is_empty():_commit_projection(job.index,job.next,prepared,job.started)
		else:_desired_view={};_fail(prepared.last_route_error);browser.select_perspective(current_perspective)
	_start_geometry()


func _commit_projection(index:int,next:Dictionary,prepared,started:int)->bool:
	var changed:bool=browser!=null and index!=current_perspective
	if browser!=null and current_navigation!=null:
		var previous:String=str(profiles[current_perspective].id)
		_memories[previous]=browser.snapshot();_filter_by_perspective[previous]=filter_id
	var commit_started:=Time.get_ticks_usec()
	var id:String=str(profiles[index].id)
	var selected:String=current_navigation.selected_id if current_navigation!=null else ""
	if browser!=null:browser.forest.prepared_geometry=prepared
	if not _install_projection(next,id,selected):return false
	current_perspective=index
	last_switch_timing={"perspective":id,"prepare_and_wait_ms":(commit_started-started)/1000.0,"scene_ms":(Time.get_ticks_usec()-commit_started)/1000.0,"forest":browser.forest.last_build_timing.duplicate()}
	browser.select_perspective(index)
	filter_id=str(_filter_by_perspective.get(id,profiles[index].get("default_filter","filter-clear")))
	apply_filter(filter_id)
	if changed and _memories.has(id):browser.restore_view(_memories[id])
	last_switch_timing["total_ms"]=(Time.get_ticks_usec()-started)/1000.0
	last_switch_timing["commit_ms"]=(Time.get_ticks_usec()-commit_started)/1000.0
	_update_status();_after_projection_commit();perspective_committed.emit();return true

func _after_projection_commit()->void:pass


func _exit_tree()->void:
	_request_generation+=1;_pending_geometry={}
	if _geometry_thread!=null:_geometry_thread.wait_to_finish();_geometry_thread=null
	_geometry_cache.clear()
	if _completion_events!=null and _completion_token!=0:_completion_events.unsubscribe(_completion_token);_completion_token=0


func _install_projection(next:Dictionary,id:String,selected:String)->bool:
	projection=next;levels[id]=int(next.level)
	var model_key:=id+"/"+str(next.level)
	if not models.has(model_key):
		var adapter=Adapter.new();adapter.configure(original,next);models[model_key]=adapter
	current_model=models[model_key]
	if shared_navigation==null:
		shared_navigation=Navigation.new();shared_navigation.initialize(original)
	current_navigation=shared_navigation
	if next.membership.has(selected):current_navigation.select_node(selected)
	elif not next.membership.has(current_navigation.selected_id):current_navigation.select_node(str(next.root))
	if browser==null:
		browser=Browser.new();browser.set_meta("native_controls",true)
		browser.perspectives=profiles;browser.perspective_index=current_perspective;browser.projection=projection;browser.full_graph_projection=graph_data
		browser.prepared_model=current_model;browser.prepared_navigation=current_navigation
		browser.size_flags_vertical=Control.SIZE_EXPAND_FILL
		column.add_child(browser);column.move_child(browser,mini(1,column.get_child_count()-1))
		browser.perspective_requested.connect(switch_perspective);browser.granularity_requested.connect(change_granularity)
		browser.state_changed.connect(_selection_changed)
		if not browser.initialized:_fail(browser.error);return false
	else:
		browser.set_perspectives(profiles)
		browser.full_graph_projection=graph_data
		browser.update_projection(current_model,current_navigation,next)
	return true

func _selection_changed()->void:
	if map_view!=null and current_navigation!=null:
		map_view.set_selection(current_navigation.selected_id)



func apply_filter(id:String)->void:
	var result=JSON.parse_string(backend.matches(id))
	if not result is Array:_fail(str(result));return
	filter_id=id;matches=result
	if browser:browser.set_filter(matches,id!="filter-clear",search_reveal)
	if filter_button:filter_button.text="More •" if id!="filter-clear" else "More"
	_update_status()

func locate_node(id:String)->void:
	search_reveal=id
	if projection.membership.has(id):
		browser._select_node(id);browser.set_filter(matches,filter_id!="filter-clear",id)
		if status:status.text="Search result revealed · Filter unchanged"
	else:
		if map_view!=null:map_view.set_selection(id);map_view.fit_keys([id],true)
		if status:status.text="Outside current perspective · located in full Graph; select a Perspective to browse it."
	if search_window!=null:search_window.hide()


func _build_map()->void:
	var start:=Time.get_ticks_usec()
	var solver=ClassDB.instantiate("PolarForceLayout")
	var layout_started:=Time.get_ticks_usec()
	var result=JSON.parse_string(solver.layout(JSON.stringify(Graph.solver_input(graph_data)),300))
	timing["full_graph_solver_ms"]=(Time.get_ticks_usec()-layout_started)/1000.0
	var geometry_started:=Time.get_ticks_usec()
	if not result is Dictionary or not bool(result.get("ok",false)):_fail("Full Graph layout failed: "+str(result));return
	graph_coordinates=Graph.coordinates(result)
	if map_view==null:
		map_view=load("res://itree_spatial_view.gd").new();map_view.role="graph";map_view.graph_label_policy="selected";map_view.size=Vector2(300,220)
		browser.add_child(map_view)
		browser.independent_map=map_view
		map_view.node_selected.connect(func(id:String,_occurrence:String):locate_node(id))
		browser.resized.connect(_place_map)
		map_label=Label.new();map_label.add_theme_font_size_override("font_size",13);map_label.mouse_filter=Control.MOUSE_FILTER_IGNORE;browser.add_child(map_label)
	map_view.set_graph(graph_data,graph_coordinates);map_view.fit_all(false)
	for label in map_view.labels.values():label.hide()
	map_label.text="3D relationships · %d nodes" % graph_data.nodes.size();_place_map()
	map_built=true;timing["full_graph_geometry_ms"]=(Time.get_ticks_usec()-geometry_started)/1000.0
	timing["full_graph_map_ms"]=(Time.get_ticks_usec()-start)/1000.0

func _update_status()->void:
	if projection.is_empty() or status==null:return
	status.text="%s  ·  %d groups  ·  Level %d/%d  ·  %d nodes in view / %d loaded  ·  read-only experiment" % [str(profiles[current_perspective].title),projection.groups.size(),int(projection.level),int(projection.max_level),projection.nodes.size(),original.records.size()]

func debug_state()->Dictionary:
	return {"ready":ready_for_review,"perspective":str(profiles[current_perspective].id) if not profiles.is_empty() else "","projection":projection,"backend":JSON.parse_string(backend.stats()) if backend!=null else {},"map_nodes":graph_data.get("nodes",[]).size(),"timing":timing,"error":last_error}

func _place_map()->void:
	if map_view==null:return
	map_view.position=Vector2(maxf(190,browser.size.x-315),maxf(20,browser.size.y-235))
	if map_label!=null:map_label.position=map_view.position-Vector2(0,22)
