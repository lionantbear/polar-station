extends "res://itree_spatial_view.gd"
## Experimental forest adapter: the inherited host/world/camera/dirty buffers
## and projected picking remain intact. Model records and relation slots are
## never copied into a second domain runtime.
const PlaqueMesh=preload("res://addons/polar_graphics/depth/spatial_plaque_mesh.gd")
const TreeProjection=preload("res://itree_tree_projection.gd")
const Geometry=preload("res://depth_forest/forest_geometry.gd")
var prepared_geometry
const GraphProjection=preload("res://itree_graph_projection.gd")
const SpaceBridge=preload("res://addons/polar_graphics/space_bridge.gd")
var projected_endpoints: Dictionary={}
var projected_node_count:=0
signal forest_selected(id: String)
signal forest_activated(id: String)
signal endpoint_requested(id: String)
var input_model
var source_order: Array=[]
# Optional data-only partition supplied by the shared perspective owner.
var group_members: Dictionary={}
var group_assignment:Dictionary={}
var wheel_region:=Rect2()
var geometry_leases:Dictionary={}
var geometry_revision:=""
var definition_projection:Dictionary={}
var measured_card_sizes:Dictionary={}
var fitted_wheel_titles:Dictionary={}
var label_measurements:=0
var measured_font_id:=0
var last_build_timing:Dictionary={}
var emphasis_dimming:Dictionary={}
var reading_direction:=Vector3(0,0,1)
var source_offsets: Dictionary={}
var source_bounds: Dictionary={}
var source_by_node: Dictionary={}
var plaques_3d: Dictionary={}
var _retained_cards: Dictionary={}
# One native node per visited definition, shared across perspectives of this
# exact source generation. Inactive nodes stay attached and hidden.
var _definition_cards:Dictionary={}
var _reference_group:=""
var reference_affordances:Dictionary={}
const ActionButton=preload("res://node_action_button.gd")
signal reference_read_requested(id:String)
var _animated_cards: Dictionary={}
var source_plaques: Dictionary={}
var direct_ids: Dictionary={}
var selected_edges: Array=[]
var full_relations: Array=[]
var source_index:=1
var depth_value:=1.0
var depth_clearance:=0.0
var selector_depth:=-1.0
var forest_pan:=Vector2.ZERO
var forest_zoom:=1.0
var reduced_motion:=true
var overview_mode:=false
var camera_changed_revision:=0
var _edge_context:Array=[]
var _hover:=""
var _press:=""
var _last_pointer:=Vector2.ZERO
var studio_fill: OmniLight3D
var _world_ready:=false
var visible_nodes: Dictionary={}
var reading_keys: Array=[]
var cached_source_layouts: Dictionary={}
var reading_rect:=Rect2()
var star_atlas
var source_reading_keys: Dictionary={}
var _route_cache: Dictionary={}
var _bridge_curves:Dictionary={}
var detail_route_layouts: Dictionary={}
var route_build_count:=0
const CARD_SIZE:=Vector2(4.2,1.36)
var out_slots: Dictionary={}
var in_slots: Dictionary={}
var source_labels: Dictionary={}
var _source_label_size:=Vector2.ZERO
var boundary_hints: Array=[]
var boundary_labels: Array[Button]=[] # Compatibility observation: never populated.
var port_groups:Dictionary={}
var port_handles:Dictionary={}
var port_popup:PanelContainer
var port_popup_cards:Array[Button]=[]
var hover_port:=""
var _port_pointer:=Vector2.INF
var _port_selection:=""
var cross_segments:Dictionary={}
var cross_parts:Dictionary={}
var cross_segment_alpha:Dictionary={}
var _cross_alpha:Dictionary={}
var last_route_error:=""
var source_full_bounds: Dictionary={}
var relation_mode:=false
var _focus_pixel_world:=0.0
var reading_surface_material:StandardMaterial3D
var detail_active:=false
var relation_annotations: Array=[]
var relation_overlay: Control
var relation_owner:=""
var view_direction:=Vector3(0,0,1)
const SOURCE_STEP:=Vector3(16,0,-40)

func _ready() -> void:
	super._ready()
	var route_material:=ShaderMaterial.new()
	route_material.shader=preload("res://addons/polar_graphics/depth/spatial_edge_clip.gdshader")
	edge_mesh.material_override=route_material
	mouse_filter=Control.MOUSE_FILTER_PASS
	var environment: Environment=world.get_child(0).environment
	environment.background_color=Color("091923")
	environment.ambient_light_energy=0.38
	var sky:=Sky.new();sky.radiance_size=Sky.RADIANCE_SIZE_64
	var studio:=ProceduralSkyMaterial.new()
	studio.sky_top_color=Color("172939");studio.sky_horizon_color=Color("9eafb6")
	studio.ground_horizon_color=Color("5e727b");studio.ground_bottom_color=Color("0b1520")
	studio.sky_curve=.18;studio.ground_curve=.12;sky.sky_material=studio
	environment.sky=sky;environment.reflected_light_source=Environment.REFLECTION_SOURCE_SKY
	world.get_child(1).light_energy=1.8
	world.get_child(1).rotation_degrees=Vector3(-32,-24,0)
	var fill:=OmniLight3D.new();studio_fill=fill;fill.light_color=Color("94d6e0");fill.light_energy=.85;fill.omni_range=50
	fill.position=Vector3(-6,9,14);world.add_child(fill)
	_build_grid()
	_build_relation_overlay()
	_build_port_popup()

func _build_grid() -> void:
	# The former decorative grid is replaced after actual node layout exists.
	pass

func set_forest(model, sources: Array) -> void:
	var started:=Time.get_ticks_usec()
	var same_input:bool=not group_members.is_empty() and geometry_revision==model.presentation_revision
	if not same_input:
		for card in _definition_cards.values():card.free()
		_definition_cards.clear()
	var phase:=started
	# Occurrence cards are local consumers, not retained canonical definitions.
	for key in plaques_3d.keys():
		if record_for(str(key)).has("group_reference"):
			plaques_3d[key].free();plaques_3d.erase(key);labels.erase(key);meshes.erase(key)
	_reference_group=""
	last_build_timing={}
	if group_members.is_empty():geometry_leases.clear();measured_card_sizes.clear();fitted_wheel_titles.clear()
	elif geometry_revision!=model.presentation_revision:
		geometry_leases.clear();measured_card_sizes.clear();fitted_wheel_titles.clear();geometry_revision=model.presentation_revision
	clear_navigation_trace();trace_surface_material=null
	# Retain native nodes while clearing the active projection. Identity uses
	# physical source and local key; model keys include a changing byte digest.
	_retained_cards=_definition_cards
	_definition_cards={}
	var keep_attached:Dictionary={}
	if same_input:
		for card in _retained_cards.values():keep_attached[card.get_instance_id()]=true
	for key in plaques_3d:
		var card=plaques_3d[key]
		var old:Dictionary=record_for(key)
		var identity:=JSON.stringify([old.get("source",""),old.get("key",old.get("uuid",key))])
		if same_input:
			keep_attached[card.get_instance_id()]=true
			card.hide()
		else:node_layer.remove_child(card)
		_retained_cards[identity]=card
	for collection in [source_labels,source_plaques]:
		for item in collection.values():world.remove_child(item);item.queue_free()
		collection.clear()
	if star_atlas!=null:world.remove_child(star_atlas);star_atlas.queue_free();star_atlas=null
	source_full_bounds.clear();direct_ids.clear();selected_edges.clear();_source_label_size=Vector2.ZERO
	_clear_ports()
	input_model=model;source_order=sources.duplicate();_clear_nodes(keep_attached)
	plaques_3d.clear();source_plaques.clear();source_offsets.clear();source_bounds.clear();source_by_node.clear()
	visible_nodes.clear();reading_keys.clear();cached_source_layouts.clear();source_reading_keys.clear();_route_cache.clear();_bridge_curves.clear();detail_route_layouts.clear();cross_segments.clear();cross_parts.clear();cross_segment_alpha.clear();_cross_alpha.clear()
	# Reuse the existing definition projection for unresolved endpoints. Local
	# file loading deliberately does not insert these into the domain records.
	last_build_timing["retire_ms"]=(Time.get_ticks_usec()-phase)/1000.0;phase=Time.get_ticks_usec()
	var prepared=prepared_geometry
	prepared_geometry=null
	if prepared==null:
		prepared=prepare_geometry(model,sources,group_members,group_assignment)
	if not prepared.last_route_error.is_empty():last_route_error=prepared.last_route_error;push_error(last_route_error);return
	for field in Geometry.FIELDS:
		if field!="geometry_leases":set(field,prepared.get(field).duplicate(true))
	full_relations=prepared.full_relations;projected_node_count=prepared.projected_node_count
	reading_keys=prepared.reading_keys.duplicate()
	if not geometry_leases.has(prepared.get_instance_id()):route_build_count+=prepared.route_build_count
	geometry_leases[prepared.get_instance_id()]=prepared
	edge_geometry_context=str(get_instance_id())+"/"+prepared.resource_path
	edge_geometry_leases=geometry_leases
	last_build_timing["layouts_ms"]=(Time.get_ticks_usec()-phase)/1000.0;phase=Time.get_ticks_usec()
	for id in records:
		if record_for(str(id)).has("group_reference"):continue
		_make_card(str(id),records[id]);visible_nodes[id]=true
	_sync_reference_cards()
	for source in sources:
		var root:=str(source.id);var group:Dictionary=cached_source_layouts[root]
		var complete_bounds:Rect2=source_full_bounds[root]
		var marker:=Label3D.new();marker.font=ThemeDB.fallback_font;marker.font_size=48;marker.fixed_size=true
		marker.text=str(source.title).replace("\n"," ")+"  /  "+str(group.ids.size())+" nodes"
		marker.position=source_offsets[root]+_world_point(Vector2(complete_bounds.get_center().x,complete_bounds.position.y-120),0)
		marker.modulate=source.accent;marker.outline_size=0;marker.no_depth_test=true;marker.render_priority=3;marker.billboard=BaseMaterial3D.BILLBOARD_ENABLED
		world.add_child(marker);source_labels[root]=marker
	last_build_timing["nodes_ms"]=(Time.get_ticks_usec()-phase)/1000.0;phase=Time.get_ticks_usec()
	for ordinal in full_relations.size():
		var relation: Dictionary=full_relations[ordinal];var a:=str(relation.from);var b:=str(relation.to)
		if not points.has(a) or not points.has(b):continue
		var key:=_slot_key(relation)
		if not _route_cache.has(key):_route_cache[key]=_relation_path(a,b,ordinal)
		var path: PackedVector3Array=_route_cache[key]
		var edge:Dictionary={"from":a,"to":b,"points":path,"kind":"tree" if bool(relation.composition) or bool(relation.get("display_reference",false)) else "semantic","relations":[relation]}
		if source_by_node.get(a)!=source_by_node.get(b):
			edge["bridge_alpha"]=_cross_alpha[key] if _cross_alpha.has(key) else SpaceBridge.alpha(path)
			edge["visual"]={"arrow":false}
		edge_paths.append(edge)
	last_build_timing["routes_ms"]=(Time.get_ticks_usec()-phase)/1000.0;phase=Time.get_ticks_usec()
	for i in sources.size():
		var root:=str(sources[i].id);var card=PlaqueMesh.new();card.dimensions=Vector2(2.1,.82);card.continuous_context_materials=true
		card.title=str(sources[i].title);card.glyph_kind=int(sources[i].glyph);card.accent=sources[i].accent
		world.add_child(card);_track_card(card);source_plaques[root]=card
		card.title_label.pixel_size=.0041
		card.title_label.width=330
		var title_key:=JSON.stringify([str(sources[i].title),card.title_label.width,card.title_label.font_size,card.title_label.pixel_size,card.dimensions,ThemeDB.fallback_font.get_instance_id()])
		if group_members.is_empty():card.title_label.text=_fit_card_title(str(sources[i].title),card.title_label,card.dimensions)
		else:
			if not fitted_wheel_titles.has(title_key):
				fitted_wheel_titles[title_key]=_fit_card_title(str(sources[i].title),card.title_label,card.dimensions);label_measurements+=1
			card.title_label.text=str(fitted_wheel_titles[title_key])
	last_build_timing["wheel_nodes_ms"]=(Time.get_ticks_usec()-phase)/1000.0;phase=Time.get_ticks_usec()
	star_atlas=load("res://depth_forest/spatial_star_atlas.gd").new();world.add_child(star_atlas)
	var canonical_points:Dictionary={}
	for id in points:
		if not record_for(str(id)).has("group_reference"):canonical_points[id]=points[id]
	star_atlas.configure(canonical_points,source_by_node)
	last_build_timing["atlas_ms"]=(Time.get_ticks_usec()-phase)/1000.0;phase=Time.get_ticks_usec()
	if same_input:_definition_cards=_retained_cards
	else:
		for card in _retained_cards.values():card.free()
	_retained_cards={}
	last_build_timing["cached_definition_nodes"]=_definition_cards.size()+plaques_3d.size()
	last_build_timing["unused_nodes_ms"]=(Time.get_ticks_usec()-phase)/1000.0;phase=Time.get_ticks_usec()
	_world_ready=true;set_selection(str(sources[source_index].id))
	last_build_timing["selection_ms"]=(Time.get_ticks_usec()-phase)/1000.0;phase=Time.get_ticks_usec()
	apply_camera()
	last_build_timing["camera_ms"]=(Time.get_ticks_usec()-phase)/1000.0
	last_build_timing["wheel_atlas_camera_ms"]=(Time.get_ticks_usec()-phase)/1000.0
	last_build_timing["total_ms"]=(Time.get_ticks_usec()-started)/1000.0

func _physical_layer(id:String)->String:
	return str(source_by_node.get(id,source_order[-1].id))

func prepare_geometry(model,sources:Array,members:Dictionary,assignment:Dictionary):
	var graph:Dictionary=definition_projection if not definition_projection.is_empty() else GraphProjection.build(model)
	return Geometry.prepare(model,sources,members,assignment,measure_geometry(model,members,graph),graph)

func measure_geometry(model,members:Dictionary,graph:Dictionary)->Dictionary:
	var sizes:Dictionary={}
	var ids:Array=[]
	if members.is_empty():ids.assign(model.ordered_ids)
	else:
		for group in members.values():ids.append_array(group)
	for id in ids:
		if bool(model.records[id].external):continue
		var extent:Vector2=_measured_card_size(str(id),str(model.records[id].title))
		sizes[id]=extent
	for item in graph.nodes:
		if bool(item.get("external",false)) or not model.records.has(str(item.id)):
			var uuid:String=str(item.get("uuid",item.get("reference",{}).get("uuid",item.get("reference",{}).get("node",""))))
			if model.records.has(str(item.id)):uuid=str(model.records[str(item.id)].uuid)
			var title:="Unloaded\n"+uuid.left(8)+"…"+uuid.right(4)
			sizes[str(item.id)]=_measured_card_size(str(item.id),title)
	return sizes

func prepare_planar_routes(ids:Array,positions:Dictionary,extents:Dictionary,relations:Array)->Dictionary:
	var geometry=Geometry.new()
	var result:Dictionary=geometry.prepare_planar_routes(ids,positions,extents,relations)
	route_build_count+=geometry.route_build_count;last_route_error=geometry.last_route_error
	return result

func _slot_key(relation: Dictionary) -> String:
	return JSON.stringify([relation.from,relation.slot])

func _measured_card_size(id:String,title:String)->Vector2:
	if group_members.is_empty():return PlaqueMesh.dimensions_for_title(title)
	var font_id:int=ThemeDB.fallback_font.get_instance_id()
	if measured_font_id!=font_id:measured_card_sizes.clear();fitted_wheel_titles.clear();measured_font_id=font_id
	if not measured_card_sizes.has(id):
		measured_card_sizes[id]=PlaqueMesh.dimensions_for_title(title);label_measurements+=1
	return measured_card_sizes[id]

func _make_card(key: String, item: Dictionary) -> void:
	var record:Dictionary=record_for(key)
	var identity:=JSON.stringify([record.get("source",""),record.get("key",record.get("uuid",key))])
	var card=_retained_cards.get(identity)
	var accent:Color=source_order[int(item.branch)%source_order.size()].accent
	var reference:Dictionary=record.get("group_reference",{})
	if not reference.is_empty():
		for source:Dictionary in source_order:
			if str(source.id)==str(reference.target_group):accent=source.accent;break
	if card!=null:
		_retained_cards.erase(identity)
		if card.title!=str(item.title) or card.dimensions!=bounds[key]:
			card.free();card=null
		elif card.accent!=accent or card.glyph_kind!=int(item.branch)%4:
			if group_members.is_empty():card.free();card=null
			else:
				card.accent=accent;card.glyph_kind=int(item.branch)%4
				card.glyph_label.set_kind(card.glyph_kind);card.glyph_label.set_color(accent)
	if card==null:
		card=PlaqueMesh.new()
		card.dimensions=bounds[key];card.title=str(item.title)
		card.accent=accent;card.glyph_kind=int(item.branch)%4
	card.position=points[key]
	if card.get_parent()!=node_layer:node_layer.add_child(card)
	card.show()
	_track_card(card)
	plaques_3d[key]=card
	card.title_label.text=str(item.title)
	labels[key]=card.title_label
	meshes[key]=card
	if not reference.is_empty():
		card.surface_tint_strength=.40;card._apply_context_tone()
		# Use the normal plaque bounds. The compact icon rail has its own hit area.
		_add_reference_affordance(key,reference,accent)

func _sync_reference_cards()->void:
	if source_order.is_empty():return
	var group:String=str(source_order[source_index].id)
	if group==_reference_group:return
	_reference_group=group
	for entry in reference_affordances.values():
		entry.layer.free()
		# A jump can change this group inside the Button's pressed emission.
		# Remove its hit immediately and let Godot finish dispatch before deletion.
		entry.jump.hide();entry.jump.queue_free()
		entry.read.hide();entry.read.queue_free()
	reference_affordances.clear()
	for key in plaques_3d.keys():
		if record_for(str(key)).has("group_reference"):
			plaques_3d[key].free();plaques_3d.erase(key);labels.erase(key);meshes.erase(key);visible_nodes.erase(key)
	for id in cached_source_layouts[group].ids:
		if record_for(str(id)).has("group_reference"):
			_make_card(str(id),records[id]);visible_nodes[id]=true
	_edge_dirty=true;_label_dirty=true

func _add_reference_affordance(id:String,reference:Dictionary,accent:Color)->void:
	var title:String=str(input_model.records[str(reference.target_group)].title)
	var jump:=ActionButton.create("jump","Go to "+title+" (Enter / double-click)")
	var read:=ActionButton.create("read","Details (Space)")
	jump.pressed.connect(func():forest_activated.emit(id))
	read.pressed.connect(func():reference_read_requested.emit(id))
	add_child(jump);add_child(read)
	# Curve baking/native segment geometry happens once, in normalized space.
	# Camera and wheel placement subsequently change only this node's transform.
	var curve:=ResourceLoader.get_cached_ref("res://.polar-graphics/group-tether-curve-v1") as Curve3D
	if curve==null:
		curve=Curve3D.new();curve.bake_interval=.045
		curve.add_point(Vector3.ZERO,Vector3.ZERO,Vector3(.33,-.055,.22))
		curve.add_point(Vector3.RIGHT,Vector3(-.33,-.055,.22),Vector3.ZERO)
		curve.take_over_path("res://.polar-graphics/group-tether-curve-v1")
	geometry_leases[curve.get_instance_id()]=curve
	var layer=preload("res://addons/polar_graphics/native_edge_layer.gd").new()
	layer.name="GroupTether";world.add_child(layer)
	var material=preload("res://addons/polar_graphics/material_resources.gd").standard({"shading_mode":BaseMaterial3D.SHADING_MODE_UNSHADED,"vertex_color_use_as_albedo":true,"transparency":BaseMaterial3D.TRANSPARENCY_ALPHA,"cull_mode":BaseMaterial3D.CULL_DISABLED})
	var routes:Array=[{"key":"group-tether","points":curve.get_baked_points(),"radius":.0018,"color":Color(accent,.6),"style":"solid","group":"tether","arrow":false,"relations":[]}]
	layer.sync(routes,material,"group-tether-v1",geometry_leases)
	reference_affordances[id]={"jump":jump,"read":read,"layer":layer,"target":str(reference.target_group),"routes":routes,"material":material,"highlighted":true}

func _place_reference_affordances()->void:
	for id in reference_affordances:
		var entry:Dictionary=reference_affordances[id]
		var card=plaques_3d.get(id)
		var target_card=source_plaques.get(entry.target)
		var show_actions:bool=card!=null and card.visible and not detail_active and not overview_mode and not relation_mode
		var rectangle:Rect2=_card_screen_rect(id) if show_actions else Rect2()
		show_actions=show_actions and Rect2(Vector2.ZERO,size).encloses(rectangle)
		entry.jump.visible=show_actions;entry.read.visible=show_actions
		if show_actions:
			entry.jump.position=Vector2(rectangle.end.x+5,rectangle.get_center().y-31)
			entry.read.position=Vector2(rectangle.end.x+5,rectangle.get_center().y+1)
		var highlighted:bool=id==selected_id or id==_hover
		if highlighted!=entry.highlighted:
			entry.highlighted=highlighted;entry.routes[0].color.a=.85 if highlighted else .32
			entry.layer.sync(entry.routes,entry.material,"group-tether-v1",geometry_leases)
		var show_line:bool=show_actions and target_card!=null and target_card.visible
		entry.layer.visible=show_line
		if not show_line:continue
		var start:Vector3=card.skin.to_global(Vector3(-card.dimensions.x*.5,0,PlaqueMesh.FRONT_Z))
		var end:Vector3=target_card.skin.to_global(Vector3(target_card.dimensions.x*.5,0,PlaqueMesh.FRONT_Z))
		var delta:=end-start;var length:=delta.length()
		entry.layer.transform=Transform3D(Basis(delta,camera.global_basis.y*length,-camera.global_basis.z*length*.7),start)

func _track_card(card)->void:
	var id:int=card.get_instance_id()
	if not card.has_meta("animation_tracked"):
		card.set_meta("animation_tracked",true)
		card.animation_changed.connect(func(active):
			if active:_animated_cards[id]=true
			else:_animated_cards.erase(id)
			# The final tween pose and immediate reduced-motion pose need one draw,
			# not a camera/geometry invalidation or a frame-count afterglow.
			viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE)
		card.tree_exiting.connect(func():_animated_cards.erase(id))
	if card.animating():_animated_cards[id]=true

func _port(id: String,relation: Dictionary,outgoing: bool,side: float) -> Vector3:
	if detail_active and id==get_parent().detail_sheet.focus_id:
		return get_parent().detail_sheet.reader_port(relation,outgoing)
	var slots: Array=out_slots.get(id,[]) if outgoing else in_slots.get(id,[])
	var index:=slots.find(_slot_key(relation))
	var y: float=((float(index)+1)/float(slots.size()+1)-.5)*bounds[id].y*.70
	var half: Vector2=bounds[id]*.5
	var x:=side*(half.x-minf(half.y*.4,.16)*absf(y)/half.y)
	return plaques_3d[id].skin.to_global(Vector3(x,y,PlaqueMesh.FRONT_Z+.012))

func _relation_path(a: String,b: String,ordinal: int) -> PackedVector3Array:
	route_build_count+=1
	var relation: Dictionary=full_relations[ordinal] if ordinal<full_relations.size() else {}
	if relation.is_empty():return PackedVector3Array([points[a],points[b]])
	var side:=1.0 if points[b].x>=points[a].x else -1.0
	var start:=_port(a,relation,true,side);var end:=_port(b,relation,false,-side)
	if detail_active:
		var detour: PackedVector3Array=get_parent().detail_sheet.avoid_reader(start,end)
		if not detour.is_empty():return detour
	if source_by_node.get(a)!=source_by_node.get(b):
		var bridge:=SpaceBridge.build(start,end,side,minf(bounds[a].x,bounds[b].x))
		bridge.get_baked_length() # Godot caches camera-travel samples once here.
		_bridge_curves[_slot_key(relation)]=bridge
		return bridge.tessellate(5,2.0)
	var offset:=1.2+float(ordinal%7)*.12
	var clearance:=-1.0 if source_by_node.get(a)!=source_by_node.get(b) else .7
	# The former four control points were drawn as three straight segments,
	# producing the conspicuous angular spikes. Godot bakes the actual cubic
	# while preserving ports, direction and distinct original slot lanes.
	var curve:=Curve3D.new();curve.bake_interval=.25
	var handle:=minf(2.0,maxf(.02,absf(end.x-start.x)*.45)) if detail_active else 2.0
	if detail_active:offset=0;clearance=0
	curve.add_point(start,Vector3.ZERO,Vector3(side*handle,offset,clearance))
	curve.add_point(end,Vector3(-side*handle,offset,clearance),Vector3.ZERO)
	return curve.get_baked_points()

func bridge_between(a:String,b:String)->Dictionary:
	for edge in edge_paths:
		if (str(edge.from)==a and str(edge.to)==b) or (str(edge.from)==b and str(edge.to)==a):
			var key:=_slot_key(edge.relations[0])
			if _bridge_curves.has(key):return {"curve":_bridge_curves[key],"reverse":str(edge.to)==a}
	return {}

func _card_screen_rect(id: String) -> Rect2:
	var rectangle:=Rect2()
	for i in 4:
		var local:=Vector3((-.5 if i%2==0 else .5)*plaques_3d[id].dimensions.x,(-.5 if i<2 else .5)*plaques_3d[id].dimensions.y,PlaqueMesh.FRONT_Z+.012)
		var point:=camera.unproject_position(plaques_3d[id].skin.to_global(local))
		rectangle=Rect2(point,Vector2.ZERO) if i==0 else rectangle.expand(point)
	return rectangle

func _screen_to_layer(point: Vector2,z: float) -> Vector3:
	var hit=Plane(Vector3(0,0,1),z).intersects_ray(camera.project_ray_origin(point),camera.project_ray_normal(point))
	return hit if hit is Vector3 else Vector3.ZERO

func set_selection(node_id: String, occurrence: String="") -> void:
	# Retain the actually drawn selected-face shader until its replacement
	# exists. History no longer owns face emission in the revised hierarchy.
	if plaques_3d.has(selected_id) and plaques_3d[selected_id].reading_surface:
		trace_surface_material=plaques_3d[selected_id].face_material
	_retain_reading_surface()
	selected_id=node_id;selected_occurrence=node_id
	direct_ids={node_id:true};selected_edges.clear()
	for relation in full_relations:
		if str(relation.from)==node_id or str(relation.to)==node_id:
			selected_edges.append(relation)
			direct_ids[str(relation.from)]=true
			if not str(relation.to).is_empty():direct_ids[str(relation.to)]=true
	for id in direct_ids:
		if plaques_3d.has(id):plaques_3d[id].visible=true;visible_nodes[id]=true
	_sync_ports(node_id)
	for key in plaques_3d:
		var card=plaques_3d[key]
		var dim:=maxf(1.0 if (relation_mode and not direct_ids.has(key)) or (not overview_mode and source_by_node.get(key)!=str(source_order[source_index].id)) else 0.0,float(emphasis_dimming.get(key,0.0)))
		card.set_visual_state(key==node_id,not reduced_motion,dim)
		card.set_reading_surface(detail_active or _physical_layer(key)==str(source_order[source_index].id))
		card.pose(key==_hover,false,Vector2.ZERO,reduced_motion)
		card.title_label.modulate=card.theme_text(direct_ids.has(key) or source_by_node.get(key)==str(source_order[source_index].id))
	_place_reference_affordances()
	_edge_dirty=true;_label_dirty=true

func _retain_reading_surface()->void:
	# Keep one actually used opaque stencil-writer through a layer handoff.
	# Otherwise clearing the old layer before marking the new one drops its
	# last native shader reference and recompiles the same variant repeatedly.
	for card in plaques_3d.values():
		if card.reading_surface and not card.chosen:
			reading_surface_material=card.face_material;return

func _key_is_selected(key: String) -> bool:
	return key==selected_id

func _rebuild_edge_geometry(_selected_only: bool=false) -> void:
	if _selected_only:return # This variant uses one filtered typed-edge layer.
	if edge_mesh and size.x>0 and size.y>0:
		edge_mesh.material_override.set_shader_parameter("canvas_region",Vector4((24.0 if detail_active and get_parent().detail_sheet.compact_wheel else float(get_parent().wheel_width)+56)/size.x,90.0/size.y,1.0,(size.y-48)/size.y))
	# Keep full edge membership. Non-selected semantic edges remain in the
	# topology but are not rendered as an unreadable permanent hairball.
	var complete: Array=edge_paths
	for edge in complete:
		var active: bool=str(edge.from)==selected_id or str(edge.to)==selected_id
		edge["clip_frustum"]=true
		var bridge:bool=edge.has("bridge_alpha") and not detail_active
		edge["width_px"]=(3.0 if active else 1.4) if bridge else (3.6 if detail_active else 4.0) if active else (1.6 if detail_active else 2.6)
		if relation_mode and active:edge["radius_world"]=_focus_pixel_world*edge.width_px*.5
		else:edge.erase("radius_world")
		if bridge:edge["point_alpha"]=edge.bridge_alpha
		else:edge.erase("point_alpha")
		var current: bool=source_by_node.get(str(edge.from))==str(source_order[source_index].id)
		edge["opacity"]=(.9 if active else .18) if bridge else 1.0 if active else (.14 if detail_active else .10 if relation_mode else (.34 if current else .23))
	edge_paths=complete.filter(func(edge):return visible_nodes.has(str(edge.from)) and visible_nodes.has(str(edge.to)) and (not detail_active or ((str(edge.from)==selected_id or plaques_3d[str(edge.from)].visible) and (str(edge.to)==selected_id or plaques_3d[str(edge.to)].visible))) and (str(edge.from)==selected_id or str(edge.to)==selected_id or str(edge.kind)=="tree") and not _has_local_reference(edge))
	edge_paths=_bundle_draw_edges(edge_paths)
	var draw_started:=Time.get_ticks_usec()
	var normal_context:=edge_geometry_context
	if detail_active or relation_mode:edge_geometry_context=""
	super._rebuild_edge_geometry()
	edge_geometry_context=normal_context
	var draw_done:=Time.get_ticks_usec()
	edge_paths=complete
	_label_dirty=true # Endpoint annotations update once with the label pass.
	var hints_done:=Time.get_ticks_usec()
	_rebuild_navigation_trace()
	if OS.get_environment("POLAR_LAB_PROFILE")=="1":print("FOREST_EDGE_PARTS ",draw_done-draw_started," ",hints_done-draw_done," ",Time.get_ticks_usec()-hints_done)

func _has_local_reference(edge:Dictionary)->bool:
	if detail_active or overview_mode or relation_mode or not edge.has("bridge_alpha"):return false
	var membership=input_model.get("display_membership")
	return membership is Dictionary and membership.has(str(edge.from)) and membership.has(str(edge.to))

func _bundle_draw_edges(edges:Array)->Array:
	if detail_active:return edges
	var draw:Array=[];var shared:Dictionary={}
	for edge in edges:
		var parts:Array=cross_parts.get(_slot_key(edge.relations[0]),[])
		if parts.is_empty():draw.append(edge);continue
		for key in parts:
			if not cross_segments.has(key):continue
			if not shared.has(key):
				var part:Dictionary=edge.duplicate()
				part["route_key"]="cross-part/"+key;part.points=cross_segments[key];part.relations=[]
				part["visual"]={"arrow":false}
				part["point_alpha"]=cross_segment_alpha[key];shared[key]=part;draw.append(part)
			if float(edge.opacity)>float(shared[key].opacity):
				shared[key].opacity=edge.opacity;shared[key].width_px=edge.width_px
				shared[key].from=edge.from;shared[key].to=edge.to
				if edge.has("radius_world"):shared[key].radius_world=edge.radius_world
				else:shared[key].erase("radius_world")
			for relation in edge.relations:
				if not relation in shared[key].relations:shared[key].relations.append(relation)
	return draw

func _clear_ports()->void:
	_port_selection="";hover_port=""
	for handle in port_handles.values():
		handle.marker.hide();handle.hit.hide();handle.marker.queue_free();handle.hit.queue_free()
	port_handles.clear();port_groups.clear()
	if port_popup:port_popup.hide()

func _sync_ports(node_id:String)->void:
	if detail_active:
		if not _port_selection.is_empty():_clear_ports()
		return
	if _port_selection==node_id:return
	_clear_ports();_port_selection=node_id
	for edge in selected_edges:
		var outgoing:=str(edge.from)==node_id
		var other:=str(edge.to) if outgoing else str(edge.from)
		if not points.has(other) or _physical_layer(node_id)==_physical_layer(other):continue
		var membership=input_model.get("display_membership")
		if membership is Dictionary and membership.has(node_id) and membership.has(other):continue
		var root_id:=str(source_by_node.get(other,"unloaded"))
		var path:PackedVector3Array=_route_cache.get(_slot_key(edge),PackedVector3Array())
		if path.is_empty():continue
		var anchor:Vector3=path[0] if outgoing else path[-1]
		var key:=JSON.stringify([node_id,outgoing,anchor])
		if not port_groups.has(key):port_groups[key]={"node":node_id,"root":root_id,"outgoing":outgoing,"targets":[],"relations":[],"position":Vector3.ZERO}
		var group:Dictionary=port_groups[key]
		if not other in group.targets:group.targets.append(other)
		group.relations.append(edge)
		group.position=anchor
	var tree_order:Array=source_order.map(func(source):return str(source.id))
	for group in port_groups.values():
		group.targets.sort_custom(func(a,b):
			var ar:=str(source_by_node.get(a,""));var br:=str(source_by_node.get(b,""))
			var ai:=tree_order.find(ar);var bi:=tree_order.find(br)
			if ai<0:ai=tree_order.size()
			if bi<0:bi=tree_order.size()
			return ai<bi if ai!=bi else str(record_for(a).title)<str(record_for(b).title))
		group.root=str(source_by_node.get(group.targets[0],"unloaded"))
	for key in port_groups:
		var group:Dictionary=port_groups[key]
		var marker=preload("res://addons/polar_graphics/connection_port.gd").new()
		marker.configure(plaques_3d[node_id].accent);plaques_3d[node_id].skin.add_child(marker)
		marker.position=plaques_3d[node_id].skin.to_local(group.position)+Vector3(0,0,.07)
		var hit:=Button.new();hit.flat=true;hit.focus_mode=Control.FOCUS_ALL
		for state in ["normal","hover","pressed","focus"]:hit.add_theme_stylebox_override(state,StyleBoxEmpty.new())
		hit.mouse_default_cursor_shape=Control.CURSOR_POINTING_HAND;hit.custom_minimum_size=Vector2(24,24);hit.size=Vector2(24,24)
		hit.mouse_entered.connect(_show_port_targets.bind(key));hit.focus_entered.connect(_show_port_targets.bind(key))
		hit.mouse_exited.connect(func():call_deferred("_dismiss_port_popup_if_outside"))
		hit.pressed.connect(_show_port_targets.bind(key))
		add_child(hit);port_handles[key]={"marker":marker,"hit":hit}

func _build_port_popup()->void:
	port_popup=PanelContainer.new();port_popup.hide();port_popup.z_index=20
	var style:=StyleBoxFlat.new();style.bg_color=Color("10212cf5");style.border_color=Color("536c77");style.set_border_width_all(1);style.set_corner_radius_all(9);style.set_content_margin_all(8);style.shadow_color=Color(0,0,0,.4);style.shadow_size=12
	port_popup.add_theme_stylebox_override("panel",style);add_child(port_popup)
	port_popup.mouse_exited.connect(func():call_deferred("_dismiss_port_popup_if_outside"))

func _show_port_targets(key:String)->void:
	if not port_groups.has(key):return
	if hover_port!=key:
		for child in port_popup.get_children():child.free()
		port_popup_cards.clear();hover_port=key
		var scroll:=ScrollContainer.new();scroll.horizontal_scroll_mode=ScrollContainer.SCROLL_MODE_DISABLED
		var column:=VBoxContainer.new();column.add_theme_constant_override("separation",7);column.size_flags_horizontal=Control.SIZE_EXPAND_FILL
		scroll.add_child(column);port_popup.add_child(scroll)
		var group:Dictionary=port_groups[key]
		for id in group.targets:
			var button:=Button.new();button.custom_minimum_size=Vector2(300,68);button.alignment=HORIZONTAL_ALIGNMENT_LEFT
			button.mouse_default_cursor_shape=Control.CURSOR_POINTING_HAND
			var accent:Color=plaques_3d[id].accent
			for state in ["normal","hover","pressed","focus"]:
				# The popup reuses the actual node's recessed glass texture;
				# native nine-patch drawing keeps scrolling/clipping and crisp text.
				var style:=StyleBoxTexture.new();style.texture=plaques_3d[id].face_material.albedo_texture
				for side in [SIDE_LEFT,SIDE_TOP,SIDE_RIGHT,SIDE_BOTTOM]:style.set_texture_margin(side,12)
				style.set_content_margin_all(10)
				style.modulate_color=Color.WHITE if state=="normal" else Color.WHITE.lerp(accent,.16)*1.2
				style.modulate_color.a=1.0
				button.add_theme_stylebox_override(state,style)
			var text:=VBoxContainer.new();text.mouse_filter=Control.MOUSE_FILTER_IGNORE
			var title:=Label.new();title.text="Unloaded reference" if bool(record_for(id).external) else str(record_for(id).title).replace("\n"," ");title.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART;title.add_theme_font_size_override("font_size",16);title.add_theme_color_override("font_color",Color("e5f1f5"));title.custom_minimum_size.x=280;title.mouse_filter=Control.MOUSE_FILTER_IGNORE
			text.add_child(title)
			var source:=Label.new();source.text=str(record_for(id).uuid) if bool(record_for(id).external) else _source_title(str(source_by_node.get(id,"unloaded")));source.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS;source.custom_minimum_size.x=280;source.add_theme_font_size_override("font_size",13);source.modulate=accent;source.mouse_filter=Control.MOUSE_FILTER_IGNORE;text.add_child(source)
			button.add_child(text);text.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT);text.offset_left=12;text.offset_top=8;text.offset_right=-10;text.offset_bottom=-8
			var measured:=ThemeDB.fallback_font.get_multiline_string_size(title.text,HORIZONTAL_ALIGNMENT_LEFT,280,16,-1,TextServer.BREAK_MANDATORY|TextServer.BREAK_WORD_BOUND|TextServer.BREAK_ADAPTIVE)
			button.custom_minimum_size.y=maxf(68,measured.y+42)
			button.set_meta("target",id)
			button.pressed.connect(func():_hide_port_popup();endpoint_requested.emit(id))
			column.add_child(button);port_popup_cards.append(button)
		scroll.custom_minimum_size=Vector2(320,minf(maxf(90,size.y-170),column.get_combined_minimum_size().y+6))
	port_popup.reset_size();port_popup.show();_place_port_popup()
	call_deferred("_place_port_popup")

func _source_title(root_id:String)->String:
	for source in source_order:
		if str(source.id)==root_id:return str(source.title)
	return "Unloaded reference"

func _hide_port_popup()->void:
	port_popup.hide();hover_port=""

func _dismiss_port_popup_if_outside()->void:
	if not port_popup.visible:return
	var pointer:=_port_pointer if _port_pointer.is_finite() else get_global_mouse_position()
	if port_popup.get_global_rect().grow(8).has_point(pointer):return
	if port_handles.has(hover_port) and port_handles[hover_port].hit.get_global_rect().grow(8).has_point(pointer):return
	_hide_port_popup()

func port_pointer_moved(position:Vector2)->void:
	_port_pointer=position
	if port_popup.visible:call_deferred("_dismiss_port_popup_if_outside")

func _place_port_popup()->void:
	if hover_port.is_empty() or not port_handles.has(hover_port):return
	var point:Vector2=port_handles[hover_port].hit.get_rect().get_center()
	var right:=point.x+10
	if right+port_popup.size.x>size.x-8:right=point.x-port_popup.size.x-10
	port_popup.position=Vector2(clampf(right,8,maxf(8,size.x-port_popup.size.x-8)),clampf(point.y-18,98,maxf(98,size.y-port_popup.size.y-50)))

func _update_boundary_hints() -> void:
	# Only native port dots remain at rest. Titles belong to hovered targets.
	boundary_hints.clear()
	for label in boundary_labels:label.hide()
	for key in port_handles:
		var handle:Dictionary=port_handles[key]
		var visible:bool=not detail_active and plaques_3d[_port_selection].visible and not camera.is_position_behind(handle.marker.global_position)
		handle.marker.visible=visible;handle.hit.visible=visible
		if visible:handle.hit.position=camera.unproject_position(handle.marker.global_position)-handle.hit.size*.5
	if detail_active:_hide_port_popup()
	elif port_popup.visible:_place_port_popup()

func _update_labels() -> void:
	for key in plaques_3d:
		var card=plaques_3d[key]
		var pixels: float=float(card.title_label.font_size)*float(card.title_label.pixel_size)/_pixel_world(points[key])
		if not detail_active:
			var reading_source: bool=source_by_node.get(key)==str(source_order[source_index].id) and depth_clearance<.001
			card.visible=not camera.is_position_behind(points[key]) and (overview_mode or reading_source or pixels<=38.0)
		card.title_label.visible=pixels>=12.0 and not camera.is_position_behind(points[key]) and (detail_active or overview_mode or direct_ids.has(key) or bool(record_for(key).external) or source_by_node.get(key)==str(source_order[wrapi(roundi(depth_value),0,source_order.size())].id))
		card.glyph_label.visible=not detail_active and pixels>=10.0
	_update_boundary_hints()
	var heading_resize:bool=_source_label_size!=size
	var heading_scale:=0.0
	if heading_resize:
		_source_label_size=size
		heading_scale=camera.project_position(Vector2.ZERO,1.0).distance_to(camera.project_position(Vector2(0,14),1.0))
	for root_id in source_labels:
		var marker: Label3D=source_labels[root_id]
		if heading_resize:marker.pixel_size=heading_scale/marker.font_size
		marker.visible=not detail_active and not relation_mode and not camera.is_position_behind(marker.position) and (overview_mode or str(root_id)==str(source_order[source_index].id))
	if relation_overlay:relation_overlay.queue_redraw()
	if detail_active:get_parent().detail_sheet.update_annotations()
	viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE

func apply_camera() -> void:
	if not _world_ready:return
	_sync_reference_cards()
	overview_mode=false;relation_mode=false;focus_callout.hide();view_direction=reading_direction
	if relation_overlay:relation_overlay.hide()
	for id in plaques_3d:
		var strength:=clampf(absf(wrapf(depth_value-float(records[id].branch),-source_order.size()*.5,source_order.size()*.5)),0,1) if source_by_node.has(id) else 1.0
		plaques_3d[id].set_context_strength(maxf(strength,float(emphasis_dimming.get(id,0.0))))
	var index:=fposmod(depth_value,source_order.size())
	var low:=int(floor(index));var high:=(low+1)%source_order.size()
	var from_anchor: Vector3=source_offsets[str(source_order[low].id)]
	var to_anchor: Vector3=source_offsets[str(source_order[high].id)]
	var anchor: Vector3=from_anchor.lerp(to_anchor,index-low)
	# Pull back during travel so a cyclic return never flies through plaques.
	depth_clearance=sin((index-low)*PI)*absf(to_anchor.z-from_anchor.z)*1.15
	# Reserve the narrow selector, then put the root near the left reading
	# margin so its children can use the canvas instead of wasting half of it.
	# Convert the reference experiment's 2x pixel margins to logical units.
	# Node world sizes and the detail reader's logical fonts stay unchanged.
	var reading_left:=maxf(120,float(get_parent().wheel_width)+46)
	var root_width: float=bounds[str(source_order[source_index].id)].x
	var reading_offset: float=maxf(0,(size.x*.5-reading_left)*19.0/maxf(1,size.y)-root_width*.5)
	target=anchor+Vector3(reading_offset,0,0)+Vector3(-forest_pan.x,forest_pan.y,0)*.012
	view_size=19.0/forest_zoom
	_update_camera()
	camera_changed_revision+=1
	_place_wheel()
	var edge_context:Array=[source_index,detail_active,relation_mode]
	if edge_context!=_edge_context:
		_edge_context=edge_context;_edge_dirty=true
		_retain_reading_surface()
		for id in plaques_3d:plaques_3d[id].set_reading_surface(detail_active or _physical_layer(id)==str(source_order[source_index].id))
	_label_dirty=true

func _update_camera() -> void:
	if camera==null or not camera.is_inside_tree():return
	camera.projection=Camera3D.PROJECTION_PERSPECTIVE
	camera.fov=43.0
	camera.keep_aspect=Camera3D.KEEP_HEIGHT
	if not overview_mode:orbit_distance=view_size/(2.0*tan(deg_to_rad(camera.fov)*.5))+depth_clearance
	camera.position=target+view_direction*orbit_distance
	camera.look_at(target,Vector3.UP)
	if studio_fill:studio_fill.position=camera.to_global(Vector3(-5,5,-6))
	if star_atlas:star_atlas.refresh(camera,size,str(source_order[source_index].id),Rect2(float(get_parent().wheel_width)+76,145,maxf(100,size.x-float(get_parent().wheel_width)-116),maxf(100,size.y-210)),reduced_motion)
	viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE
	_label_dirty=true

func _edge_uses_perspective() -> bool:
	return camera.projection!=Camera3D.PROJECTION_ORTHOGONAL

func _pixel_world(point: Vector3) -> float:
	# The base Canvas role assumes orthographic size. This fixed-angle forest
	# uses its existing perspective Camera3D branch, including depth-correct
	# line widths and source-control projection sizes.
	var height:=maxf(1,size.y)
	if not _edge_uses_perspective():return camera.size/height
	var depth:=maxf(.1,-camera.to_local(point).z)
	return 2.0*depth*tan(deg_to_rad(camera.fov)*.5)/height

func _place_wheel() -> void:
	var region:=wheel_region if wheel_region.has_area() else Rect2(13,0,float(get_parent().wheel_width),size.y)
	place_plaque_wheel(source_order,source_plaques,source_index,depth_value,region)
	_place_reference_affordances()

func place_plaque_wheel(order:Array, plaques:Dictionary, selected:int, value:float, region:Rect2, spacing:float=10.0, padding:float=48.0)->void:
	if camera==null or size.y<1:return
	# A shared cylinder arc in camera space: one world size for every plaque.
	# Perspective provides the falloff; do not compensate each card's depth.
	var front_distance:=orbit_distance*.72
	var world_per_pixel:=2.0*front_distance*tan(deg_to_rad(camera.fov)*.5)/size.y
	var wheel_width: float=region.size.x
	var center_x:=region.get_center().x
	# Keep all settled labels on the reading face when the Pane has room.
	# The old TAU/count mapping hid half the sources even in a tall Pane.
	var count:=order.size()
	var face_limit:=.85
	var radius:=0.0
	var plaque_height:float=wheel_width*(plaques[str(order[0].id)].dimensions.y if not order.is_empty() else .82)/2.1
	var slots:=maxi(1,count)
	var half_span:=float(slots-1)*.5
	while slots>1:
		var step:=face_limit/half_span
		var smallest_gap:=sin(face_limit)-sin(face_limit-step)
		radius=(plaque_height+spacing)/smallest_gap
		if 2.0*radius*sin(face_limit)+plaque_height+padding<=region.size.y:break
		slots-=1;half_span=float(slots-1)*.5
	if slots==1:radius=0.0
	for i in order.size():
		var root:=str(order[i].id)
		var card=plaques[root]
		var distance:=float(i)-value
		# Half-step offset for even slot counts centers the reading group.
		# Wrapping and overflow travel behind the arc; +/-PI have identical
		# pose, so recycling a hidden plaque never causes a visible teleport.
		var offset:=.5 if slots%2==0 else 0.0
		distance=wrapf(distance+offset,-float(count)*.5,float(count)*.5)
		var angle:=distance*face_limit/maxf(half_span,.5)
		if absf(distance)>half_span:
			angle=signf(distance)*lerpf(face_limit,PI,minf(1.0,(absf(distance)-half_span)*2.0))
		var recession:=front_distance*(1.0-cos(angle))*.24
		var screen_y: float=region.get_center().y-size.y*.5+radius*sin(angle)
		var z: float=-front_distance-recession
		var x: float=(center_x-5.0*(1.0-cos(angle))-size.x*.5)*world_per_pixel
		var y: float=-screen_y*world_per_pixel*(-z/front_distance)
		# Closed wheel: rear plaques pass behind the front reading sector.
		# Their controls are hidden too; there is no visible endpoint teleport.
		card.position=camera.to_global(Vector3(x*(-z/front_distance),y,z))
		card.basis=camera.global_basis
		card.scale=Vector3.ONE*world_per_pixel*wheel_width/2.1
		card.visible=cos(angle)>-.08 and not (detail_active and get_parent().detail_sheet.compact_wheel)
		card.set_context_strength(clampf((1.0-cos(angle))*.25,0,1))
		if card.chosen!=(i==selected) or card.motion!=not reduced_motion:
			card.set_selected(i==selected,not reduced_motion)
			card.pose(false,false,Vector2.ZERO,reduced_motion)

func source_rect(index: int) -> Rect2:
	if index<0 or index>=source_order.size():return Rect2()
	var card=source_plaques[str(source_order[index].id)]
	var rectangle:=Rect2()
	for i in 4:
		var corner:=Vector3((-.5 if i%2==0 else .5)*card.dimensions.x,(-.5 if i<2 else .5)*card.dimensions.y,card.title_label.position.z)
		var point:=camera.unproject_position(card.skin.to_global(corner))
		rectangle=Rect2(point,Vector2.ZERO) if i==0 else rectangle.expand(point)
	return rectangle

func _process(delta: float) -> void:
	if not is_visible_in_tree():return
	super._process(delta)
	if not reduced_motion:viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE
	if not _animated_cards.is_empty():viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE

func needs_frames() -> bool:
	if not is_visible_in_tree():return false
	if _edge_dirty or _selection_edge_dirty or _label_dirty:return true
	if not reduced_motion:return true
	return not _animated_cards.is_empty()

func _gui_input(event: InputEvent) -> void:
	# The assembly owns the already-tested wheel/pan/zoom semantics. This
	# subclass contributes only 3D node picking/feedback, never free orbit.
	if event is InputEventMouseMotion:
		var key:=pick_node(event.position)
		if key!=_hover:
			if plaques_3d.has(_hover):plaques_3d[_hover].pose(false,false,Vector2.ZERO,reduced_motion)
			_hover=key
			_place_reference_affordances()
			if plaques_3d.has(key):plaques_3d[key].pose(true,false,Vector2.ZERO,reduced_motion)
		if plaques_3d.has(key) and event.position.distance_to(_last_pointer)>3:
			var rectangle:=_card_screen_rect(key)
			var pointer: Vector2=((event.position-rectangle.get_center())/(rectangle.size*.5)).clamp(Vector2(-1,-1),Vector2(1,1))
			plaques_3d[key].pose(true,false,pointer,reduced_motion);_last_pointer=event.position
	if event is InputEventMouseButton and event.button_index==MOUSE_BUTTON_LEFT:
		var key:=pick_node(event.position)
		if event.pressed:
			_press=key
			if plaques_3d.has(key):plaques_3d[key].pose(true,true,Vector2.ZERO,reduced_motion)
		else:
			if plaques_3d.has(_press):plaques_3d[_press].pose(key==_press,false,Vector2.ZERO,reduced_motion)
			if not key.is_empty() and key==_press:forest_selected.emit(key)
			_press=""
		if event.pressed and event.double_click and not key.is_empty():forest_activated.emit(key)

func focus_overview() -> void:
	overview_mode=true;relation_mode=false;relation_overlay.hide()
	for key in plaques_3d:plaques_3d[key].set_context_strength(float(emphasis_dimming.get(key,0.0)))
	# Reuse SpatialView's real perspective corner/radius fit, with its 43deg
	# field of view and the same viewing direction as this forest camera.
	view_direction=Vector3(.65,.18,1).normalized()
	yaw=atan2(view_direction.x,view_direction.z);pitch=asin(view_direction.y)
	_fit_region(points.keys(),Rect2(250,130,maxf(200,size.x-355),maxf(200,size.y-200)))
	_place_wheel();_edge_dirty=true;_label_dirty=true

func focus_related() -> void:
	overview_mode=true;relation_mode=true
	view_direction=Vector3(.65,.18,1).normalized()
	yaw=atan2(view_direction.x,view_direction.z);pitch=asin(view_direction.y)
	var keys: Array=[]
	for id in direct_ids:
		if points.has(id):keys.append(id)
	var region:=Rect2(250,150,maxf(220,size.x-300),maxf(200,size.y-220))
	if size.x<1080 or size.y<660:region=Rect2(24,120,maxf(10,size.x-48),maxf(10,size.y-170))
	_fit_region(keys,region)
	# Native camera conversion once per focus composition. Keep the resulting
	# world geometry through camera motion; do not project/rebuild every frame.
	var far_depth:=camera.near
	for key in keys:far_depth=maxf(far_depth,-camera.to_local(points[key]).z)
	_focus_pixel_world=camera.project_position(Vector2.ZERO,far_depth).distance_to(camera.project_position(Vector2(1,0),far_depth))
	_build_endpoint_annotations()
	for id in plaques_3d:plaques_3d[id].set_context_strength(maxf(1.0 if not direct_ids.has(id) else 0.0,float(emphasis_dimming.get(id,0.0))))
	_place_wheel();_edge_dirty=true;_label_dirty=true

func focus_reading_context() -> void:
	# Reading changes the camera only. Deep plaques retain visible geometry.
	forest_pan=Vector2.ZERO;forest_zoom=1.0;apply_camera()

func reading_font_pixels(id: String) -> float:
	var label: Label3D=plaques_3d[id].title_label
	return float(label.font_size)*label.pixel_size/_pixel_world(points[id])

func visual_level(id: String) -> Array:
	var source: String=source_by_node.get(id,"")
	var positions: Dictionary=get_parent().detail_sheet.base_points if detail_active else points
	var result: Array=[]
	if source.is_empty() or not points.has(id):return result
	for candidate in cached_source_layouts[source].ids:
		if absf(positions[candidate].x-positions[id].x)<.05:result.append(candidate)
	result.sort_custom(func(a,b):
		return positions[a].y>positions[b].y if not is_equal_approx(positions[a].y,positions[b].y) else str(a)<str(b))
	return result

func visual_children(parent_id: String) -> Array[Dictionary]:
	# Preserve original occurrence slots; the controller owns path selection.
	var result: Array[Dictionary]=[]
	for edge in input_model.composition_edges(parent_id):
		if bool(edge.resolved) and points.has(str(edge.to)):result.append(edge)
	result.sort_custom(func(a: Dictionary,b: Dictionary):
		var positions: Dictionary=get_parent().detail_sheet.base_points if detail_active else points
		var pa: Vector3=positions[str(a.to)];var pb: Vector3=positions[str(b.to)]
		return pa.y>pb.y if not is_equal_approx(pa.y,pb.y) else pa.x<pb.x)
	return result

func ensure_reading_visible(ids: Array) -> Vector2:
	# Return only a delta to existing controller pan. Keep zoom/orientation;
	# an already visible selection never causes a camera jump.
	if overview_mode or ids.is_empty():return Vector2.ZERO
	var rectangle:=Rect2();var first_id:=""
	for value in ids:
		var id:=str(value)
		if not points.has(id):continue
		var box:=_card_screen_rect(id)
		if record_for(id).has("group_reference"):box.size.x+=40
		rectangle=box if first_id.is_empty() else rectangle.merge(box)
		if first_id.is_empty():first_id=id
	if first_id.is_empty():return Vector2.ZERO
	var left:=float(get_parent().wheel_width)+38.0
	var safe:=Rect2(left,72.5,maxf(100,size.x-left-20),maxf(100,size.y-105))
	var map=get_parent().get("independent_map")
	if map is Control and map.visible:
		var overlay:=Rect2(map.get_global_rect().position-global_position,map.size).grow(10)
		var selected_box:=_card_screen_rect(first_id)
		if selected_box.intersects(overlay):
			var above:float=overlay.position.y-safe.position.y
			var beside:float=overlay.position.x-safe.position.x
			if above>=selected_box.size.y:safe.size.y=minf(safe.size.y,above)
			elif beside>=selected_box.size.x:safe.size.x=minf(safe.size.x,beside)
	# A whole long branch cannot fit at reading scale. Reveal the requested
	# first card rather than zooming the entire tree down to unreadable text.
	if rectangle.size.x>safe.size.x or rectangle.size.y>safe.size.y:rectangle=_card_screen_rect(first_id)
	var shift:=Vector2.ZERO
	if rectangle.position.x<safe.position.x:shift.x=safe.position.x-rectangle.position.x
	elif rectangle.end.x>safe.end.x:shift.x=safe.end.x-rectangle.end.x
	if rectangle.position.y<safe.position.y:shift.y=safe.position.y-rectangle.position.y
	elif rectangle.end.y>safe.end.y:shift.y=safe.end.y-rectangle.end.y
	if shift.is_zero_approx():return Vector2.ZERO
	var center:=rectangle.get_center()
	var world_delta:=_screen_to_layer(center,points[first_id].z)-_screen_to_layer(center+shift,points[first_id].z)
	return Vector2(-world_delta.x,world_delta.y)/.012

func _color(record: Dictionary) -> Color:
	var index:=int(record.get("branch",0))
	return source_order[index].accent if index>=0 and index<source_order.size() else Color("9bb8bc")

func pick_node(screen: Vector2) -> String:
	var begin := camera.project_position(screen,camera.near)
	var end := camera.project_position(screen,camera.far)
	var closest := INF
	var selected := ""
	for id in plaques_3d:
		var card = plaques_3d[id]
		if not card.is_visible_in_tree():continue
		var inverse: Transform3D = card.shell.global_transform.affine_inverse()
		var local_begin: Vector3 = inverse*begin
		var local_end: Vector3 = inverse*end
		if not card.shell.get_aabb().intersects_segment(local_begin,local_end):continue
		var hit: Dictionary = card.shell.mesh.generate_triangle_mesh().intersect_segment(local_begin,local_end)
		if hit.is_empty():continue
		var distance: float = begin.distance_to(card.shell.to_global(hit.position))
		if distance < closest:closest=distance;selected=str(id)
	return selected

func _build_relation_overlay() -> void:
	relation_overlay=Control.new();relation_overlay.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	relation_overlay.mouse_filter=Control.MOUSE_FILTER_IGNORE;relation_overlay.z_index=20
	add_child(relation_overlay)
	relation_overlay.hide()

func _build_endpoint_annotations() -> void:
	relation_annotations.clear();relation_owner=selected_id
	# Real node titles and hover cards replace the old permanent side rails.
	relation_overlay.hide()

func _place_endpoint_annotations() -> void:
	relation_overlay.hide()

func _projected_card_bounds(keys: Array) -> Rect2:
	var rectangle:=Rect2();var first:=true
	for id in keys:
		var next:=_card_screen_rect(str(id))
		rectangle=next if first else rectangle.merge(next);first=false
	return rectangle

func _fit_region(keys: Array, region: Rect2) -> void:
	if keys.is_empty() or size.x<2 or size.y<2:return
	if not region.position.is_finite() or not region.size.is_finite() or region.size.x<=1 or region.size.y<=1:
		region=Rect2(Vector2.ONE,Vector2(maxf(1,size.x-2),maxf(1,size.y-2)))
	region=region.intersection(Rect2(Vector2.ZERO,size))
	if region.size.x<=1 or region.size.y<=1:region=Rect2(Vector2.ONE,Vector2(maxf(1,size.x-2),maxf(1,size.y-2)))
	# Extend the inherited fixed-angle perspective corner fit to an asymmetric
	# usable rectangle. Camera-space point inequalities avoid iterative screen
	# recentering across a large depth span (which can cross the near plane).
	fit_keys(keys,false)
	var base_target:=target
	var right:=camera.global_basis.x;var up:=camera.global_basis.y;var toward:=camera.global_basis.z
	var pixels:=size.y/(2.0*tan(deg_to_rad(camera.fov)*.5))
	var limits:=Vector4((region.position.x-size.x*.5)/pixels,(region.end.x-size.x*.5)/pixels,(size.y*.5-region.end.y)/pixels,(size.y*.5-region.position.y)/pixels)
	var corners: Array[Vector3]=[];var nearest:=1.0
	for id in keys:
		for offset in [Vector3(-2.1,-.68,0),Vector3(-2.1,.68,0),Vector3(2.1,-.68,0),Vector3(2.1,.68,0)]:
			var p: Vector3=points[id]+offset-base_target
			var local:=Vector3(p.dot(right),p.dot(up),p.dot(toward))
			corners.append(local);nearest=maxf(nearest,local.z+1.0)
	# The extrema separate from distance, giving an exact lower bound in
	# one pass: max(px + right*z) - min(px + left*z) <= (right-left)*D.
	var extrema:=Vector4(-INF,INF,-INF,INF)
	for point in corners:
		extrema.x=maxf(extrema.x,point.x+limits.y*point.z)
		extrema.y=minf(extrema.y,point.x+limits.x*point.z)
		extrema.z=maxf(extrema.z,point.y+limits.w*point.z)
		extrema.w=minf(extrema.w,point.y+limits.z*point.z)
	orbit_distance=maxf(nearest,maxf((extrema.x-extrema.y)/(limits.y-limits.x),(extrema.z-extrema.w)/(limits.w-limits.z)))*1.025
	var interval:=_region_camera_interval(corners,limits,orbit_distance)
	target=base_target+right*(interval.x+interval.y)*.5+up*(interval.z+interval.w)*.5
	_update_camera()

func _region_camera_interval(corners: Array[Vector3], limits: Vector4, distance: float) -> Vector4:
	var interval:=Vector4(-INF,INF,-INF,INF)
	for point in corners:
		var depth:=distance-point.z
		interval.x=maxf(interval.x,point.x-limits.y*depth)
		interval.y=minf(interval.y,point.x-limits.x*depth)
		interval.z=maxf(interval.z,point.y-limits.w*depth)
		interval.w=minf(interval.w,point.y-limits.z*depth)
	return interval

# Last keyboard operation is presentation state, never a model relationship.
var trace_from:=""
var trace_to:=""
var trace_semantic:=false
var trace_mesh: MultiMeshInstance3D
var trace_material: ShaderMaterial
var trace_lights: Array[OmniLight3D]=[]
# Keep the actually-used native emission shader alive between trace handoffs.
# Dropping its last material before constructing the next trace recompiles it.
var trace_surface_material:StandardMaterial3D

func clear_navigation_trace() -> void:
	for id in [trace_from,trace_to]:
		if plaques_3d.has(id):plaques_3d[id].set_navigation_role(0,false)
	trace_from="";trace_to=""
	if trace_mesh:trace_mesh.hide()
	for light in trace_lights:light.hide()
	if viewport_3d:viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE

func show_navigation_trace(from_id: String,to_id: String) -> void:
	if not bool(motion_preferences.get("path_feedback",true)):clear_navigation_trace();return
	clear_navigation_trace()
	if from_id==to_id or not points.has(from_id) or not points.has(to_id):return
	trace_from=from_id;trace_to=to_id
	if trace_mesh==null:
		trace_mesh=preload("res://addons/polar_graphics/native_path_strip.gd").new();world.add_child(trace_mesh)
		trace_material=ShaderMaterial.new();trace_material.shader=preload("res://addons/polar_graphics/depth/spatial_navigation_flow.gdshader")
		trace_mesh.material_override=trace_material
		for tint in [Color("f2b35d"),Color("65ddeb")]:
			var light:=OmniLight3D.new();light.light_color=tint;light.light_energy=.7;light.omni_range=6
			light.shadow_enabled=false;world.add_child(light);trace_lights.append(light)
	plaques_3d[from_id].set_navigation_role(-1,not reduced_motion)
	plaques_3d[to_id].set_navigation_role(1,not reduced_motion)
	trace_surface_material=plaques_3d[to_id].face_material
	trace_mesh.show()
	_rebuild_navigation_trace()

func _rebuild_navigation_trace() -> void:
	if trace_from.is_empty() or not trace_mesh:return
	if detail_active and (not visible_nodes.has(trace_from) or not visible_nodes.has(trace_to) or (trace_from!=selected_id and not plaques_3d[trace_from].visible) or (trace_to!=selected_id and not plaques_3d[trace_to].visible)):
		trace_mesh.hide()
		for light in trace_lights:light.hide()
		return
	trace_mesh.show()
	var path:=PackedVector3Array();trace_semantic=false
	for edge in edge_paths:
		if str(edge.from)==trace_from and str(edge.to)==trace_to:
			path=edge.points.duplicate();trace_semantic=true;break
		if str(edge.to)==trace_from and str(edge.from)==trace_to:
			path=edge.points.duplicate();path.reverse();trace_semantic=true;break
	if path.is_empty():
		# A separate dashed gutter arc communicates a UI move, not ancestry.
		var a: Vector3=points[trace_from]+Vector3(-bounds[trace_from].x*.5,0,PlaqueMesh.FRONT_Z+.035)
		var b: Vector3=points[trace_to]+Vector3(-bounds[trace_to].x*.5,0,PlaqueMesh.FRONT_Z+.035)
		if detail_active and absf(points[trace_from].x-points[trace_to].x)<.1:
			var direction:=signf(points[trace_to].y-points[trace_from].y)
			a=points[trace_from]+Vector3(0,direction*bounds[trace_from].y*.5,PlaqueMesh.FRONT_Z+.035)
			b=points[trace_to]+Vector3(0,-direction*bounds[trace_to].y*.5,PlaqueMesh.FRONT_Z+.035)
		var gutter:=minf(a.x,b.x)-.75
		var c:=Vector3(gutter,a.y,a.z);var d:=Vector3(gutter,b.y,b.z)
		var curve:=Curve3D.new()
		curve.add_point(a,Vector3.ZERO,c-a);curve.add_point(b,d-b,Vector3.ZERO)
		path=curve.get_baked_points()
	trace_mesh.sync(path,.32)
	trace_material.set_shader_parameter("animate_flow",not reduced_motion and bool(motion_preferences.get("connection_flow",true)))
	trace_material.set_shader_parameter("semantic_route",trace_semantic)
	trace_material.set_shader_parameter("canvas_region",Vector4((24.0 if detail_active and get_parent().detail_sheet.compact_wheel else float(get_parent().wheel_width)+56)/maxf(1,size.x),125.0/maxf(1,size.y),1.0,(size.y-48)/maxf(1,size.y)))
	trace_lights[0].position=points[trace_from]+Vector3(-2,1,2);trace_lights[0].show()
	trace_lights[1].position=points[trace_to]+Vector3(2,1,2);trace_lights[1].show()
	viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE


func record_for(id: String) -> Dictionary:
	return input_model.records[id] if input_model.records.has(id) else projected_endpoints.get(id,{})

func refresh_context_tones()->void:
	for id in plaques_3d:
		var strength:=0.0
		if relation_mode:strength=1.0 if not direct_ids.has(id) else 0.0
		elif not overview_mode and source_by_node.has(id):strength=clampf(absf(wrapf(depth_value-float(records[id].branch),-source_order.size()*.5,source_order.size()*.5)),0,1)
		plaques_3d[id].set_context_strength(maxf(strength,float(emphasis_dimming.get(id,0.0))))
	viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE

func _exit_tree()->void:
	for card in _definition_cards.values():
		if is_instance_valid(card):card.free()
	_definition_cards.clear()
	super._exit_tree()
