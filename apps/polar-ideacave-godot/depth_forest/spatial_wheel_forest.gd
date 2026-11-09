extends "res://itree_spatial_view.gd"
## Experimental forest adapter: the inherited host/world/camera/dirty buffers
## and projected picking remain intact. Model records and relation slots are
## never copied into a second domain runtime.
const PlaqueMesh=preload("res://addons/polar_graphics/depth/spatial_plaque_mesh.gd")
const TreeProjection=preload("res://itree_tree_projection.gd")
const GraphProjection=preload("res://itree_graph_projection.gd")
var projected_endpoints: Dictionary={}
var projected_node_count:=0
signal forest_selected(id: String)
signal forest_activated(id: String)
signal endpoint_requested(id: String)
var input_model
var source_order: Array=[]
var source_offsets: Dictionary={}
var source_bounds: Dictionary={}
var source_by_node: Dictionary={}
var plaques_3d: Dictionary={}
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
const CARD_SIZE:=Vector2(4.2,1.36)
var out_slots: Dictionary={}
var in_slots: Dictionary={}
var routes_pending:=true
var route_settle_frames:=10
var routing_size:=Vector2.ZERO
var route_due_msec:=0
var source_labels: Dictionary={}
var boundary_hints: Array=[]
var boundary_labels: Array[Button]=[]
var last_route_error:=""
var source_full_bounds: Dictionary={}
var relation_mode:=false
var detail_active:=false
var relation_annotations: Array=[]
var relation_overlay: Control
var relation_rails: Array[VBoxContainer]=[]
var relation_heading: Label
var relation_owner:=""
var view_direction:=Vector3(.035,.025,1).normalized()
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

func _build_grid() -> void:
	# The former decorative grid is replaced after actual node layout exists.
	pass

func set_forest(model, sources: Array) -> void:
	input_model=model;source_order=sources.duplicate();_clear_nodes()
	plaques_3d.clear();source_plaques.clear();source_offsets.clear();source_bounds.clear();source_by_node.clear()
	visible_nodes.clear();reading_keys.clear();cached_source_layouts.clear();source_reading_keys.clear();_route_cache.clear()
	# Reuse the existing definition projection for unresolved endpoints. Local
	# file loading deliberately does not insert these into the domain records.
	var graph: Dictionary=GraphProjection.build(model)
	projected_node_count=graph.nodes.size();projected_endpoints.clear()
	for item in graph.nodes:
		if not model.records.has(str(item.id)):
			var display: Dictionary=item.duplicate(true)
			display["key"]=str(item.id);display["uuid"]=str(item.reference.get("uuid",item.reference.get("node","")))
			display["source_sha256"]="";display["description"]=""
			display["properties"]={"reference":item.reference}
			projected_endpoints[str(item.id)]=display
	full_relations=graph.links.map(func(edge):return edge.relation)
	out_slots.clear();in_slots.clear()
	for relation in full_relations:
		var a:=str(relation.from);var b:=str(relation.to)
		if not out_slots.has(a):out_slots[a]=[]
		if not in_slots.has(b):in_slots[b]=[]
		out_slots[a].append(_slot_key(relation));in_slots[b].append(_slot_key(relation))
	var groups: Array=[]
	for i in sources.size():
		var root:=str(sources[i].id);var path:=str(model.records[root].source)
		var ids: Array=[];var sizes: Dictionary={}
		for id in model.ordered_ids:
			if not bool(model.records[id].external) and str(model.records[id].source)==path:
				ids.append(id);sizes[id]=PlaqueMesh.dimensions_for_title(str(model.records[id].title))/SCALE;source_by_node[id]=root
		var edges: Array=[]
		for relation in full_relations:
			if str(relation.from) in ids and str(relation.to) in ids:edges.append({"from":relation.from,"to":relation.to,"relation":relation})
		# Composition owns tree geometry. Semantic relationships remain original
		# slots and routes, but cannot pull a branch out of its hierarchy.
		var hierarchy: Array=edges.filter(func(edge):return bool(edge.relation.composition))
		# Dev has only semantic local links; retain those genuine endpoints as
		# its local display graph without inventing composition relationships.
		if hierarchy.is_empty():hierarchy=edges
		var full: Dictionary=TreeProjection._graphviz(ids,hierarchy,sizes,"dot","LR")
		var shown: Array=[root]
		for relation in model.composition_edges(root):
			if bool(relation.resolved) and str(relation.to) in ids and str(relation.to) not in shown:shown.append(str(relation.to))
		# The dev source's local children use typed semantic slots rather than
		# contains; show their genuine directly linked endpoints as reading tier.
		if shown.size()==1:
			for edge in edges:
				if str(edge.from)==root and str(edge.to) not in shown:shown.append(str(edge.to))
		if not str(full.error).is_empty():push_error(str(full.error));return
		var rectangle: Rect2=TreeProjection._bounds(full.positions,sizes,full.routes)
		groups.append({"root":root,"ids":ids,"index":i,"shown":shown,"full":full,"rectangle":rectangle,"path":path,"sizes":sizes})
	# One immutable, complete LR tree per source. Reading changes only camera.
	var source_cursor:=0.0
	for group in groups:
		var root: String=group.root;var index: int=group.index
		var offset: Vector2=-group.full.positions[root]
		var full_sizes: Dictionary=group.sizes
		var local_positions: Dictionary={}
		for id in group.ids:local_positions[id]=group.full.positions[id]+offset
		var complete_bounds: Rect2=TreeProjection._bounds(local_positions,full_sizes,[])
		# Reuse the projection's complete-bounds sequential placement, now along
		# a depth track. A source reserves its complete geometry, not six cards.
		var z: float=-source_cursor*2.0
		var layer_offset:=Vector3(source_cursor-complete_bounds.position.x*SCALE+z*.12,float(index)*8.0,z)
		source_cursor+=complete_bounds.size.x*SCALE+12.0
		source_full_bounds[root]=complete_bounds
		source_offsets[root]=layer_offset
		source_bounds[root]=Rect2(group.rectangle.position+offset,group.rectangle.size)
		source_reading_keys[root]=group.shown.duplicate();cached_source_layouts[root]=group
		for id in group.ids:
			var p: Vector2=local_positions[id]
			points[id]=_world_point(p,0)+layer_offset;bounds[id]=full_sizes[id]*SCALE
			records[id]={"node_id":id,"title":str(model.records[id].title),"root_id":root,"branch":index,"depth":0 if id==root else 1,"source":group.path}
			_make_card(id,records[id]);visible_nodes[id]=true
			if id in group.shown:reading_keys.append(id)
		for route in group.full.routes:
			var path_points:=PackedVector3Array()
			for p in route.points:path_points.append(_world_point(p+offset,0)+layer_offset+Vector3(0,0,PlaqueMesh.FRONT_Z+.012))
			_route_cache[_slot_key(route.relation)]=path_points
		var marker:=Label3D.new();marker.font=ThemeDB.fallback_font;marker.font_size=48;marker.pixel_size=.009
		marker.text=str(sources[index].title).replace("\n"," ")+"  /  "+str(group.ids.size())+" nodes"
		marker.position=layer_offset+_world_point(Vector2(complete_bounds.get_center().x,complete_bounds.position.y-120),0)
		marker.modulate=sources[index].accent;marker.outline_size=0;marker.no_depth_test=true;marker.render_priority=3;marker.billboard=BaseMaterial3D.BILLBOARD_ENABLED
		world.add_child(marker);source_labels[root]=marker
	var external_top:=5.4
	var max_x:=0.0
	for rectangle in source_bounds.values():max_x=maxf(max_x,rectangle.end.x)
	for id in model.ordered_ids+projected_endpoints.keys():
		if bool(record_for(id).external):
			bounds[id]=CARD_SIZE;records[id]={"node_id":id,"title":"Unloaded\n"+str(record_for(id).uuid).left(8)+"…"+str(record_for(id).uuid).right(4),"root_id":"external","branch":3,"depth":1,"source":"unloaded"}
			bounds[id]=PlaqueMesh.dimensions_for_title(str(records[id].title))
			points[id]=source_offsets[str(sources[-1].id)]+Vector3(source_full_bounds[str(sources[-1].id)].end.x*SCALE+5,external_top-bounds[id].y*.5,0)
			external_top-=bounds[id].y+.30
			_make_card(id,records[id]);visible_nodes[id]=true
	for ordinal in full_relations.size():
		var relation: Dictionary=full_relations[ordinal];var a:=str(relation.from);var b:=str(relation.to)
		if not points.has(a) or not points.has(b):continue
		var path: PackedVector3Array=_route_cache.get(_slot_key(relation),_relation_path(a,b,ordinal))
		edge_paths.append({"from":a,"to":b,"points":path,"kind":"tree" if bool(relation.composition) else "semantic","relations":[relation]})
	for i in sources.size():
		var root:=str(sources[i].id);var card=PlaqueMesh.new();card.dimensions=Vector2(2.1,.82)
		card.title=str(sources[i].title);card.glyph_kind=int(sources[i].glyph);card.accent=sources[i].accent
		world.add_child(card);source_plaques[root]=card
		card.title_label.pixel_size=.0041
		card.title_label.width=330
		card.title_label.text=_fit_card_title(str(sources[i].title),card.title_label,card.dimensions)
	star_atlas=load("res://depth_forest/spatial_star_atlas.gd").new();world.add_child(star_atlas)
	star_atlas.configure(points,source_by_node)
	_world_ready=true;set_selection(str(sources[source_index].id));apply_camera();routes_pending=true;route_settle_frames=10

func _slot_key(relation: Dictionary) -> String:
	return JSON.stringify([relation.from,relation.slot])

func _make_card(key: String, item: Dictionary) -> void:
	var card=PlaqueMesh.new()
	card.dimensions=bounds[key];card.title=str(item.title)
	card.accent=source_order[int(item.branch)%source_order.size()].accent
	card.glyph_kind=int(item.branch)%4
	card.position=points[key];node_layer.add_child(card)
	plaques_3d[key]=card
	card.title_label.text=str(item.title)
	labels[key]=card.title_label
	meshes[key]=card

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
	var relation: Dictionary=full_relations[ordinal] if ordinal<full_relations.size() else {}
	if relation.is_empty():return PackedVector3Array([points[a],points[b]])
	var side:=1.0 if points[b].x>=points[a].x else -1.0
	var start:=_port(a,relation,true,side);var end:=_port(b,relation,false,-side)
	if detail_active:
		var detour: PackedVector3Array=get_parent().detail_sheet.avoid_reader(start,end)
		if not detour.is_empty():return detour
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

func _card_screen_rect(id: String) -> Rect2:
	var rectangle:=Rect2()
	for i in 4:
		var local:=Vector3((-.5 if i%2==0 else .5)*bounds[id].x,(-.5 if i<2 else .5)*bounds[id].y,PlaqueMesh.FRONT_Z+.012)
		var point:=camera.unproject_position(plaques_3d[id].skin.to_global(local))
		rectangle=Rect2(point,Vector2.ZERO) if i==0 else rectangle.expand(point)
	return rectangle

func _screen_to_layer(point: Vector2,z: float) -> Vector3:
	var hit=Plane(Vector3(0,0,1),z).intersects_ray(camera.project_ray_origin(point),camera.project_ray_normal(point))
	return hit if hit is Vector3 else Vector3.ZERO

func _route_visible_semantics() -> void:
	if size.x<100 or size.y<100:return
	routes_pending=false;last_route_error=""
	# Each reroute starts from valid immutable world paths, so a failed screen
	# obstacle solve cannot leave an older camera's partial routing in place.
	for edge in edge_paths:
		if str(edge.from)!=selected_id and str(edge.to)!=selected_id:continue
		var relation: Dictionary=edge.relations[0]
		var ordinal:=full_relations.find(relation)
		edge.points=_route_cache.get(_slot_key(relation),_relation_path(str(edge.from),str(edge.to),ordinal))
	if reading_font_pixels(selected_id)<12:return
	var fixed: Dictionary={};var sizes: Dictionary={};var routed_edges: Array=[]
	var port_data: Dictionary={}
	var relevant:=Rect2();var have_relevant:=false
	for id in direct_ids:
		if not points.has(id) or camera.is_position_behind(points[id]):continue
		var rectangle:=_card_screen_rect(id)
		relevant=rectangle if not have_relevant else relevant.merge(rectangle);have_relevant=true
	relevant=relevant.grow(30)
	# Temporary aliases are geometry-only port stubs, not nodes or relations in
	# the ViewModel. Neato's existing fixed-position obstacle router owns the
	# clearance around real card rectangles, including other depth layers.
	for id in visible_nodes:
		if camera.is_position_behind(points[id]):continue
		# Distinct depth planes can overlap in projection. They cannot be
		# planar neato obstacles; their original world-space curves remain.
		if not overview_mode and source_by_node.get(id)!=str(source_order[source_index].id):continue
		var rectangle:=_card_screen_rect(id)
		if rectangle.size.x<8 or (not camera.is_position_in_frustum(points[id]) and not direct_ids.has(id)):continue
		if not direct_ids.has(id) and (reading_font_pixels(id)<12 or not relevant.intersects(rectangle)):continue
		fixed[id]=rectangle.get_center();sizes[id]=rectangle.size
	for i in full_relations.size():
		var relation: Dictionary=full_relations[i];var a:=str(relation.from);var b:=str(relation.to)
		if not fixed.has(a) or not fixed.has(b):continue
		if a!=selected_id and b!=selected_id:continue
		if bool(relation.composition) and _route_cache.has(_slot_key(relation)):continue
		var side:=1.0 if fixed[b].x>=fixed[a].x else -1.0
		var start:=_port(a,relation,true,side);var end:=_port(b,relation,false,-side)
		routed_edges.append({"from":a,"to":b,"relation":relation,"tail_port":"e" if side>0 else "w","head_port":"w" if side>0 else "e"})
		port_data[_slot_key(relation)]={"start":start,"end":end}
	if routed_edges.is_empty():return
	var result: Dictionary=TreeProjection._graphviz(fixed.keys(),routed_edges,sizes,"neato","LR",fixed,"",0.0)
	if not str(result.error).is_empty():last_route_error=str(result.error);return # Keep valid immutable world paths; expose diagnostic, no retry loop.
	for route in result.routes:
		var key:=_slot_key(route.relation);var ports: Dictionary=port_data[key]
		var world_points:=PackedVector3Array([ports.start])
		for i in route.points.size():
			var t:=float(i)/maxf(1,route.points.size()-1)
			world_points.append(_screen_to_layer(route.points[i],lerpf(ports.start.z,ports.end.z,t)))
		world_points.append(ports.end)
		for edge in edge_paths:
			if _slot_key(edge.relations[0])==key:edge.points=world_points;break
	routes_pending=false;routing_size=size
	_edge_dirty=true

func set_selection(node_id: String, occurrence: String="") -> void:
	selected_id=node_id;selected_occurrence=node_id
	direct_ids={node_id:true};selected_edges.clear()
	for relation in full_relations:
		if str(relation.from)==node_id or str(relation.to)==node_id:
			selected_edges.append(relation)
			direct_ids[str(relation.from)]=true
			if not str(relation.to).is_empty():direct_ids[str(relation.to)]=true
	for id in direct_ids:
		if plaques_3d.has(id):plaques_3d[id].visible=true;visible_nodes[id]=true
	for key in plaques_3d:
		var card=plaques_3d[key]
		card.set_selected(key==node_id,not reduced_motion)
		card.set_context_dim((relation_mode and not direct_ids.has(key)) or (not overview_mode and source_by_node.get(key)!=str(source_order[source_index].id)))
		card.pose(key==_hover,false,Vector2.ZERO,reduced_motion)
		card.title_label.modulate=card.theme_text(direct_ids.has(key) or source_by_node.get(key)==str(source_order[source_index].id))
	_invalidate_routes()
	_edge_dirty=true;_label_dirty=true;_settle_frames=2

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
		edge["width_px"]=(3.6 if detail_active else 4.0) if active else (1.6 if detail_active else 2.6)
		var current: bool=source_by_node.get(str(edge.from))==str(source_order[source_index].id)
		edge["opacity"]=1.0 if active else (.14 if detail_active else .10 if relation_mode else (.34 if current else .23))
	edge_paths=complete.filter(func(edge):return visible_nodes.has(str(edge.from)) and visible_nodes.has(str(edge.to)) and (not detail_active or ((str(edge.from)==selected_id or plaques_3d[str(edge.from)].visible) and (str(edge.to)==selected_id or plaques_3d[str(edge.to)].visible))) and (str(edge.from)==selected_id or str(edge.to)==selected_id or (str(edge.kind)=="tree" and not camera.is_position_behind(points[str(edge.from)]) and not camera.is_position_behind(points[str(edge.to)]))))
	super._rebuild_edge_geometry()
	edge_paths=complete
	_update_boundary_hints()
	_rebuild_navigation_trace()

func _update_boundary_hints() -> void:
	boundary_hints.clear()
	var occupied: Array[Rect2]=[]
	for label in boundary_labels:label.hide()
	if detail_active:return
	for edge in edge_paths:
		if str(edge.from)!=selected_id and str(edge.to)!=selected_id:continue
		var other: String=str(edge.to) if str(edge.from)==selected_id else str(edge.from)
		if camera.is_position_in_frustum(points[other]):continue
		var path: PackedVector3Array=edge.points.duplicate()
		if str(edge.to)==other:path.reverse()
		var boundary:=Vector3.INF
		for i in range(1,path.size()):
			var hit:=Geometry3D.segment_intersects_convex(path[i-1],path[i],camera.get_frustum())
			if not hit.is_empty():boundary=hit[0];break
		if not boundary.is_finite():continue
		var source_name:=str(record_for(other).key)
		for source in source_order:
			if str(source.id)==str(source_by_node.get(other,"")):source_name=str(source.title);break
		var ordinal:=boundary_hints.size()
		if ordinal>=boundary_labels.size():
			var label:=Button.new();label.add_theme_font_size_override("font_size",14);label.modulate=Color("d4b071");label.add_theme_stylebox_override("normal",StyleBoxEmpty.new());label.mouse_default_cursor_shape=Control.CURSOR_POINTING_HAND;label.pressed.connect(func():endpoint_requested.emit(str(label.get_meta("target"))));add_child(label);boundary_labels.append(label)
		var label:=boundary_labels[ordinal]
		label.text=("← " if str(edge.to)==selected_id else "→ ")+source_name+" · "+str(record_for(other).title).replace("\n"," ")
		label.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS
		label.tooltip_text=label.text+" (offscreen)"
		var point:=camera.unproject_position(boundary)
		label.set_meta("target",other);label.size=Vector2(300,30)
		label.position=Vector2(clampf(point.x+8,float(get_parent().wheel_width)+70,size.x-label.size.x-8),clampf(point.y+8,100,size.y-80))
		# Reuse the label renderer's occupied-rectangle convention to keep
		# distinct offscreen endpoints from printing over one another.
		for attempt in 16:
			var collision:=false
			for box in occupied:
				if box.intersects(label.get_rect()):collision=true;break
			if not collision:break
			label.position.y+=-34 if point.y>size.y*.5 else 34
		occupied.append(label.get_rect());label.show()
		boundary_hints.append({"from":edge.from,"to":edge.to,"slot":edge.relations[0].slot,"screen":[point.x,point.y],"behind":camera.is_position_behind(points[other])})

func _update_labels() -> void:
	for key in plaques_3d:
		var card=plaques_3d[key]
		var pixels: float=float(card.title_label.font_size)*float(card.title_label.pixel_size)/_pixel_world(points[key])
		if not detail_active:
			var reading_source: bool=source_by_node.get(key)==str(source_order[source_index].id) and depth_clearance<.001
			card.visible=not camera.is_position_behind(points[key]) and (overview_mode or reading_source or pixels<=38.0)
		card.title_label.visible=pixels>=12.0 and not camera.is_position_behind(points[key]) and (detail_active or overview_mode or bool(record_for(key).external) or source_by_node.get(key)==str(source_order[wrapi(roundi(depth_value),0,source_order.size())].id))
		card.glyph_label.visible=not detail_active and pixels>=10.0
	if relation_mode:_place_endpoint_annotations()
	for root_id in source_labels:
		var marker: Label3D=source_labels[root_id]
		marker.visible=not detail_active and not relation_mode and not camera.is_position_behind(marker.position) and (overview_mode or str(root_id)==str(source_order[source_index].id))
		marker.pixel_size=maxf(.009,_pixel_world(marker.position)*14.0/marker.font_size)
	if relation_overlay:relation_overlay.queue_redraw()
	if detail_active:get_parent().detail_sheet.update_annotations()
	viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE

func _invalidate_routes() -> void:
	routes_pending=true;route_due_msec=Time.get_ticks_msec()+180

func apply_camera() -> void:
	if not _world_ready:return
	overview_mode=false;relation_mode=false;focus_callout.hide();view_direction=Vector3(.035,.025,1).normalized()
	if relation_overlay:relation_overlay.hide()
	for id in plaques_3d:
		var strength:=clampf(absf(wrapf(depth_value-float(records[id].branch),-source_order.size()*.5,source_order.size()*.5)),0,1) if source_by_node.has(id) else 1.0
		plaques_3d[id].set_context_strength(strength)
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
	_invalidate_routes()
	_edge_dirty=true;_label_dirty=true;_settle_frames=2

func _update_camera() -> void:
	if camera==null:return
	camera.projection=Camera3D.PROJECTION_PERSPECTIVE
	camera.fov=43.0
	camera.keep_aspect=Camera3D.KEEP_HEIGHT
	if not overview_mode:orbit_distance=view_size/(2.0*tan(deg_to_rad(camera.fov)*.5))+depth_clearance
	camera.position=target+view_direction*orbit_distance
	camera.look_at(target,Vector3.UP)
	if studio_fill:studio_fill.position=camera.to_global(Vector3(-5,5,-6))
	if star_atlas:star_atlas.refresh(camera,size,str(source_order[source_index].id),Rect2(float(get_parent().wheel_width)+76,145,maxf(100,size.x-float(get_parent().wheel_width)-116),maxf(100,size.y-210)),reduced_motion)
	_edge_dirty=true;_label_dirty=true

func _pixel_world(point: Vector3) -> float:
	# The base Canvas role assumes orthographic size. This fixed-angle forest
	# uses its existing perspective Camera3D branch, including depth-correct
	# line widths and source-control projection sizes.
	var height:=maxf(1,size.y)
	if camera.projection==Camera3D.PROJECTION_ORTHOGONAL:return camera.size/height
	var depth:=maxf(.1,-camera.to_local(point).z)
	return 2.0*depth*tan(deg_to_rad(camera.fov)*.5)/height

func _place_wheel() -> void:
	if camera==null or size.y<1:return
	# A shared cylinder arc in camera space: one world size for every plaque.
	# Perspective provides the falloff; do not compensate each card's depth.
	var front_distance:=orbit_distance*.72
	var world_per_pixel:=2.0*front_distance*tan(deg_to_rad(camera.fov)*.5)/size.y
	var wheel_width: float=float(get_parent().wheel_width)
	var center_x:=13.0+wheel_width*.5
	# Keep all settled labels on the reading face when the Pane has room.
	# The old TAU/count mapping hid half the sources even in a tall Pane.
	var count:=source_order.size()
	var face_limit:=.85
	var radius:=0.0
	var plaque_height:=wheel_width*.82/2.1
	var slots:=maxi(1,count)
	var half_span:=float(slots-1)*.5
	while slots>1:
		var step:=face_limit/half_span
		var smallest_gap:=sin(face_limit)-sin(face_limit-step)
		radius=(plaque_height+10.0)/smallest_gap
		if 2.0*radius*sin(face_limit)+plaque_height+48.0<=size.y:break
		slots-=1;half_span=float(slots-1)*.5
	if slots==1:radius=0.0
	for i in source_order.size():
		var root:=str(source_order[i].id)
		var card=source_plaques[root]
		var distance:=float(i)-depth_value
		# Half-step offset for even slot counts centers the reading group.
		# Wrapping and overflow travel behind the arc; +/-PI have identical
		# pose, so recycling a hidden plaque never causes a visible teleport.
		var offset:=.5 if slots%2==0 else 0.0
		distance=wrapf(distance+offset,-float(count)*.5,float(count)*.5)
		var angle:=distance*face_limit/maxf(half_span,.5)
		if absf(distance)>half_span:
			angle=signf(distance)*lerpf(face_limit,PI,minf(1.0,(absf(distance)-half_span)*2.0))
		var recession:=front_distance*(1.0-cos(angle))*.24
		var screen_y: float=radius*sin(angle)
		var z: float=-front_distance-recession
		var x: float=(center_x-5.0*(1.0-cos(angle))-size.x*.5)*world_per_pixel
		var y: float=-screen_y*world_per_pixel*(-z/front_distance)
		# Closed wheel: rear plaques pass behind the front reading sector.
		# Their controls are hidden too; there is no visible endpoint teleport.
		card.position=camera.to_global(Vector3(x*(-z/front_distance),y,z))
		card.basis=camera.global_basis
		card.rotate_object_local(Vector3.UP,-.14-sin(angle)*.12)
		card.rotate_object_local(Vector3.RIGHT,sin(angle)*.10)
		card.scale=Vector3.ONE*world_per_pixel*wheel_width/2.1
		card.visible=cos(angle)>-.08 and not (detail_active and get_parent().detail_sheet.compact_wheel)
		card.set_context_strength(clampf((1.0-cos(angle))*.25,0,1))
		if card.chosen!=(i==source_index) or card.motion!=not reduced_motion:
			card.set_selected(i==source_index,not reduced_motion)
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

func _resize() -> void:
	super._resize()
	routes_pending=true;route_settle_frames=10

func _process(delta: float) -> void:
	if not is_visible_in_tree():return
	super._process(delta)
	if _world_ready and routes_pending and Time.get_ticks_msec()>=route_due_msec:_route_visible_semantics()
	if not reduced_motion:viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE
	for card in plaques_3d.values()+source_plaques.values():
		if card.animating():viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE

func needs_frames() -> bool:
	if not is_visible_in_tree():return false
	if _edge_dirty or _label_dirty or _settle_frames>0 or routes_pending:return true
	if not reduced_motion:return true
	for card in plaques_3d.values()+source_plaques.values():
		if card.animating():return true
	return false

func _gui_input(event: InputEvent) -> void:
	# The assembly owns the already-tested wheel/pan/zoom semantics. This
	# subclass contributes only 3D node picking/feedback, never free orbit.
	if event is InputEventMouseMotion:
		var key:=pick_node(event.position)
		if key!=_hover:
			if plaques_3d.has(_hover):plaques_3d[_hover].pose(false,false,Vector2.ZERO,reduced_motion)
			_hover=key
			if plaques_3d.has(key):plaques_3d[key].pose(true,false,Vector2.ZERO,reduced_motion)
			_settle_frames=12
		if plaques_3d.has(key) and event.position.distance_to(_last_pointer)>3:
			var rectangle:=_card_screen_rect(key)
			var pointer: Vector2=((event.position-rectangle.get_center())/(rectangle.size*.5)).clamp(Vector2(-1,-1),Vector2(1,1))
			plaques_3d[key].pose(true,false,pointer,reduced_motion);_last_pointer=event.position;_settle_frames=12
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
		_settle_frames=12

func focus_overview() -> void:
	overview_mode=true;relation_mode=false;relation_overlay.hide()
	for card in plaques_3d.values():card.set_context_dim(false)
	# Reuse SpatialView's real perspective corner/radius fit, with its 43deg
	# field of view and the same viewing direction as this forest camera.
	view_direction=Vector3(.65,.18,1).normalized()
	yaw=atan2(view_direction.x,view_direction.z);pitch=asin(view_direction.y)
	_fit_region(points.keys(),Rect2(250,130,maxf(200,size.x-355),maxf(200,size.y-200)))
	_place_wheel();_invalidate_routes();_edge_dirty=true;_label_dirty=true

func focus_related() -> void:
	overview_mode=true;relation_mode=true
	view_direction=Vector3(.65,.18,1).normalized()
	yaw=atan2(view_direction.x,view_direction.z);pitch=asin(view_direction.y)
	var keys: Array=[]
	for id in direct_ids:
		if points.has(id):keys.append(id)
	var region:=Rect2(520,215,maxf(220,size.x-820),maxf(200,size.y-350))
	if size.x<1080 or size.y<660:region=Rect2(24,120,maxf(10,size.x-48),maxf(10,size.y-170))
	_fit_region(keys,region)
	_build_endpoint_annotations()
	for id in plaques_3d:plaques_3d[id].set_context_dim(not direct_ids.has(id))
	_place_wheel();_invalidate_routes();_edge_dirty=true;_label_dirty=true

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
		rectangle=box if first_id.is_empty() else rectangle.merge(box)
		if first_id.is_empty():first_id=id
	if first_id.is_empty():return Vector2.ZERO
	var left:=float(get_parent().wheel_width)+38.0
	var safe:=Rect2(left,72.5,maxf(100,size.x-left-20),maxf(100,size.y-105))
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
	# Same native camera ray/plane picking as SpatialView, constrained to the
	# existing visible presentation and the plaque's actual hover/press pose.
	var origin:=camera.project_ray_origin(screen);var direction:=camera.project_ray_normal(screen)
	var closest:=INF;var selected:=""
	for id in plaques_3d:
		var card=plaques_3d[id]
		if not card.is_visible_in_tree():continue
		var inverse: Transform3D=card.skin.global_transform.affine_inverse()
		var hit=Plane(Vector3(0,0,1),PlaqueMesh.FRONT_Z+.012).intersects_ray(inverse*origin,inverse.basis*direction)
		if not hit is Vector3:continue
		if not Geometry2D.is_point_in_polygon(Vector2(hit.x,hit.y),PlaqueMesh.shape(card.dimensions)):continue
		var point: Vector3=card.skin.to_global(hit);var distance:=origin.distance_to(point)
		if distance<closest:closest=distance;selected=str(id)
	return selected

func _build_relation_overlay() -> void:
	relation_overlay=Control.new();relation_overlay.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	relation_overlay.mouse_filter=Control.MOUSE_FILTER_IGNORE;relation_overlay.z_index=20
	add_child(relation_overlay);relation_overlay.draw.connect(_draw_annotation_leaders)
	for i in 2:
		var rail:=VBoxContainer.new();rail.add_theme_constant_override("separation",12)
		rail.mouse_filter=Control.MOUSE_FILTER_IGNORE;relation_overlay.add_child(rail);relation_rails.append(rail)
	relation_heading=Label.new();relation_heading.add_theme_font_size_override("font_size",16)
	relation_heading.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART;relation_heading.horizontal_alignment=HORIZONTAL_ALIGNMENT_CENTER
	relation_heading.mouse_filter=Control.MOUSE_FILTER_IGNORE;relation_overlay.add_child(relation_heading)
	relation_overlay.hide()

func _build_endpoint_annotations() -> void:
	for rail in relation_rails:
		for child in rail.get_children():rail.remove_child(child);child.queue_free()
	relation_annotations.clear();relation_owner=selected_id
	var entries: Array=[]
	for edge in selected_edges:
		var incoming: bool=str(edge.to)==selected_id
		var id: String=str(edge.from) if incoming else str(edge.to)
		entries.append({"target":id,"relation":edge,"direction":"IN" if incoming else "OUT"})
	for i in entries.size():
		var entry: Dictionary=entries[i];var id: String=entry.target
		var button:=Button.new();button.custom_minimum_size=Vector2(250,64)
		button.mouse_default_cursor_shape=Control.CURSOR_POINTING_HAND;button.focus_mode=Control.FOCUS_ALL
		button.pressed.connect(func():endpoint_requested.emit(id))
		button.mouse_entered.connect(func():
			if plaques_3d.has(id):plaques_3d[id].pose(true,false,Vector2.ZERO,reduced_motion)
			relation_overlay.queue_redraw();_settle_frames=10)
		button.mouse_exited.connect(func():
			if plaques_3d.has(id):plaques_3d[id].pose(false,false,Vector2.ZERO,reduced_motion)
			relation_overlay.queue_redraw();_settle_frames=10)
		var color:=_color(records[id]);var style:=StyleBoxFlat.new()
		style.bg_color=Color(0,0,0,0);style.border_color=Color(color,.55);style.border_width_bottom=1
		button.add_theme_stylebox_override("normal",style)
		var hover: StyleBoxFlat=style.duplicate();hover.bg_color=Color(.04,.10,.14,.55);hover.border_color=color
		button.add_theme_stylebox_override("hover",hover);button.add_theme_stylebox_override("focus",hover);button.add_theme_stylebox_override("pressed",hover)
		var column:=VBoxContainer.new();column.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
		column.mouse_filter=Control.MOUSE_FILTER_IGNORE;column.offset_left=6;column.offset_right=-6;button.add_child(column)
		var meta:=Label.new();meta.add_theme_font_size_override("font_size",14);meta.modulate=color;meta.mouse_filter=Control.MOUSE_FILTER_IGNORE
		var source_name:="Unloaded reference"
		for source in source_order:
			if str(source.id)==str(source_by_node.get(id,"")):source_name=str(source.title);break
		var slot: String=str(entry.relation.slot)
		var names: Dictionary={"rust-graph-layout-dependency-review":"Layout review","delivery-solution":"Delivery solution","knowledge-solution":"Knowledge solution","prr":"PRR"}
		var readable_slot: String=names.get(slot,slot.replace("-"," ").capitalize())
		meta.text=entry.direction+" · "+readable_slot+(" / unloaded" if bool(record_for(id).external) else "")
		meta.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS;column.add_child(meta)
		var title:=Label.new();title.add_theme_font_size_override("font_size",15)
		title.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART;title.max_lines_visible=2;title.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS;title.mouse_filter=Control.MOUSE_FILTER_IGNORE
		title.text=str(record_for(id).title).replace("\n"," ")
		if bool(record_for(id).external):title.text="Reference · "+str(record_for(id).uuid).left(8)+"…"+str(record_for(id).uuid).right(4)
		column.add_child(title)
		button.tooltip_text=entry.direction+" slot: "+str(entry.relation.slot)+"\nSource: "+source_name+"\n"+str(record_for(id).title)+("\nUUID: "+str(record_for(id).uuid) if bool(record_for(id).external) else "")+"\nClick or Tab / Enter to locate this actual endpoint"
		relation_rails[i%2].add_child(button)
		entry["button"]=button;entry["font_size"]=15;entry["color"]=color;entry["source_name"]=source_name;entry["column"]=column;entry["title_label"]=title;entry["meta_label"]=meta
		relation_annotations.append(entry)
	relation_heading.text="Selected · "+str(record_for(selected_id).title).replace("\n"," ")+"\n"+str(selected_edges.size())+" direct slots · Tab / Enter locates real endpoints"
	relation_overlay.show();update_connection_labels();_place_endpoint_annotations()

func _place_endpoint_annotations() -> void:
	if not relation_mode:return
	if relation_owner!=selected_id:_build_endpoint_annotations();return
	if size.x<1080 or size.y<660:
		for rail in relation_rails:rail.hide()
		relation_heading.position=Vector2(16,90);relation_heading.size=Vector2(maxf(1,size.x-32),90)
		relation_heading.text="Readable relationship annotations need a wider window (1080 × 660).\nResize or return to Reading view. The real endpoints remain in the scene."
		relation_overlay.queue_redraw();return
	for rail in relation_rails:rail.show()
	var width:=clampf((size.x-620.0)*.5,120.0,250.0)
	for i in 2:
		relation_rails[i].position=Vector2(235 if i==0 else size.x-width-22,170)
		relation_rails[i].size=Vector2(width,0)
		for button in relation_rails[i].get_children():button.custom_minimum_size.x=width
	for entry in relation_annotations:
		var button: Button=entry.button
		button.custom_minimum_size.y=maxf(62,entry.column.get_combined_minimum_size().y+8)
	relation_heading.position=Vector2(525,90);relation_heading.size=Vector2(maxf(280,size.x-815),80)
	relation_overlay.queue_redraw()

func _draw_annotation_leaders() -> void:
	if not relation_mode or size.x<1080 or size.y<660:return
	for entry in relation_annotations:
		var button: Button=entry.button;var id: String=entry.target
		if camera.is_position_behind(points[id]):continue
		var endpoint:=screen_point(id)
		var rectangle:=button.get_global_rect();rectangle.position-=global_position
		var anchor:=Vector2(rectangle.end.x,rectangle.get_center().y) if button.get_parent()==relation_rails[0] else Vector2(rectangle.position.x,rectangle.get_center().y)
		var color: Color=entry.color;color.a=.8 if button.is_hovered() or button.has_focus() else .28
		relation_overlay.draw_line(anchor,endpoint,color,1.0,true)
		relation_overlay.draw_circle(endpoint,3.0,color)
	var selected_point:=screen_point(selected_id)
	relation_overlay.draw_line(relation_heading.position+Vector2(relation_heading.size.x*.5,relation_heading.size.y),selected_point,Color(.35,.85,.80,.5),1.0,true)

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
var trace_mesh: MeshInstance3D
var trace_material: ShaderMaterial
var trace_caption: Label
var trace_lights: Array[OmniLight3D]=[]

func clear_navigation_trace() -> void:
	for id in [trace_from,trace_to]:
		if plaques_3d.has(id):plaques_3d[id].set_navigation_role(0,false)
	trace_from="";trace_to=""
	if trace_mesh:trace_mesh.hide()
	if trace_caption:trace_caption.hide()
	for light in trace_lights:light.hide()
	if viewport_3d:viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE

func show_navigation_trace(from_id: String,to_id: String) -> void:
	if not bool(motion_preferences.get("path_feedback",true)):clear_navigation_trace();return
	clear_navigation_trace()
	if from_id==to_id or not points.has(from_id) or not points.has(to_id):return
	trace_from=from_id;trace_to=to_id
	if trace_mesh==null:
		trace_mesh=MeshInstance3D.new();world.add_child(trace_mesh)
		trace_material=ShaderMaterial.new();trace_material.shader=preload("res://addons/polar_graphics/depth/spatial_navigation_flow.gdshader")
		trace_mesh.material_override=trace_material
		trace_caption=Label.new();trace_caption.mouse_filter=Control.MOUSE_FILTER_IGNORE
		trace_caption.add_theme_font_size_override("font_size",14);trace_caption.modulate=Color("90bcc4");add_child(trace_caption)
		for tint in [Color("f2b35d"),Color("65ddeb")]:
			var light:=OmniLight3D.new();light.light_color=tint;light.light_energy=.7;light.omni_range=6
			light.shadow_enabled=false;world.add_child(light);trace_lights.append(light)
	plaques_3d[from_id].set_navigation_role(-1,not reduced_motion)
	plaques_3d[to_id].set_navigation_role(1,not reduced_motion)
	trace_mesh.show();trace_caption.visible=bool(motion_preferences.get("connection_labels",true))
	_rebuild_navigation_trace()

func _rebuild_navigation_trace() -> void:
	if trace_from.is_empty() or not trace_mesh:return
	if detail_active and (not visible_nodes.has(trace_from) or not visible_nodes.has(trace_to) or (trace_from!=selected_id and not plaques_3d[trace_from].visible) or (trace_to!=selected_id and not plaques_3d[trace_to].visible)):
		trace_mesh.hide();trace_caption.hide()
		for light in trace_lights:light.hide()
		return
	trace_mesh.show();trace_caption.visible=bool(motion_preferences.get("connection_labels",true))
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
		for i in 41:
			var t:=float(i)/40;var u:=1.0-t
			path.append(u*u*u*a+3*u*u*t*c+3*u*t*t*d+t*t*t*b)
	var total:=0.0
	for i in range(1,path.size()):total+=path[i].distance_to(path[i-1])
	var vertices:=PackedVector3Array();var uv:=PackedVector2Array();var progress:=PackedVector2Array();var traveled:=0.0
	for i in range(1,path.size()):
		var a:=path[i-1];var b:=path[i];var length:=a.distance_to(b)
		if length<.00001:continue
		var normal: Vector3=(b-a).cross(camera.global_basis.z).normalized()*_pixel_world((a+b)*.5)*8.0
		var positions: Array=[a-normal,a+normal,b+normal,a-normal,b+normal,b-normal]
		var distances: Array=[traveled,traveled,traveled+length,traveled,traveled+length,traveled+length]
		for j in 6:
			vertices.append(positions[j]);uv.append(Vector2(distances[j],[-1,1,1,-1,1,-1][j]));progress.append(Vector2(distances[j]/maxf(total,.001),0))
		traveled+=length
	var mesh:=ArrayMesh.new()
	if not vertices.is_empty():
		var arrays: Array=[];arrays.resize(Mesh.ARRAY_MAX)
		arrays[Mesh.ARRAY_VERTEX]=vertices;arrays[Mesh.ARRAY_TEX_UV]=uv;arrays[Mesh.ARRAY_TEX_UV2]=progress
		mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES,arrays)
	trace_mesh.mesh=mesh
	trace_material.set_shader_parameter("animate_flow",not reduced_motion and bool(motion_preferences.get("connection_flow",true)))
	trace_material.set_shader_parameter("semantic_route",trace_semantic)
	trace_material.set_shader_parameter("canvas_region",Vector4((24.0 if detail_active and get_parent().detail_sheet.compact_wheel else float(get_parent().wheel_width)+56)/maxf(1,size.x),125.0/maxf(1,size.y),1.0,(size.y-48)/maxf(1,size.y)))
	trace_caption.text="Previous move · amber departure → cyan arrival"+("" if trace_semantic else " · dashed navigation trail")
	trace_caption.position=Vector2(24 if detail_active and get_parent().detail_sheet.compact_wheel else float(get_parent().wheel_width)+76,90 if detail_active else 100)
	trace_lights[0].position=points[trace_from]+Vector3(-2,1,2);trace_lights[0].show()
	trace_lights[1].position=points[trace_to]+Vector3(2,1,2);trace_lights[1].show()
	viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE


func record_for(id: String) -> Dictionary:
	return input_model.records[id] if input_model.records.has(id) else projected_endpoints.get(id,{})

func update_connection_labels() -> void:
	var enabled:=bool(motion_preferences.get("connection_labels",true))
	for entry in relation_annotations:
		entry.meta_label.visible=enabled
	if trace_caption!=null:
		trace_caption.visible=enabled and trace_mesh!=null and trace_mesh.visible
