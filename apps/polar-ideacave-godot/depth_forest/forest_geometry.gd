extends Resource
## The existing forest Graphviz/Curve3D preparation, separated from live scene
## objects. Numeric inputs are immutable; callers measure fonts on the UI thread.
const TreeProjection=preload("res://itree_tree_projection.gd")
const PlaqueMesh=preload("res://addons/polar_graphics/depth/spatial_plaque_mesh.gd")
const SpaceBridge=preload("res://addons/polar_graphics/space_bridge.gd")
const SCALE:=.01
const CARD_SIZE:=Vector2(4.2,1.36)
static var _prepare_lock:=Mutex.new()
const FIELDS=["projected_endpoints", "out_slots", "in_slots", "source_by_node", "source_full_bounds", "source_offsets", "source_bounds", "source_reading_keys", "cached_source_layouts", "points", "bounds", "records", "_route_cache", "_cross_alpha", "cross_segments", "cross_parts", "cross_segment_alpha", "_bridge_curves", "geometry_leases"]
var projected_node_count:=0
var full_relations:Array=[]
var reading_keys:Array=[]
var source_order:Array=[]
var group_members:Dictionary={}
var group_assignment:Dictionary={}
var route_build_count:=0
var last_route_error:=""
var _source_records:Dictionary={}
var projected_endpoints:Dictionary={}
var out_slots:Dictionary={}
var in_slots:Dictionary={}
var source_by_node:Dictionary={}
var source_full_bounds:Dictionary={}
var source_offsets:Dictionary={}
var source_bounds:Dictionary={}
var source_reading_keys:Dictionary={}
var cached_source_layouts:Dictionary={}
var points:Dictionary={}
var bounds:Dictionary={}
var records:Dictionary={}
var _route_cache:Dictionary={}
var _cross_alpha:Dictionary={}
var cross_segments:Dictionary={}
var cross_parts:Dictionary={}
var cross_segment_alpha:Dictionary={}
var _bridge_curves:Dictionary={}
var geometry_leases:Dictionary={}

static func cache_key(model,sources:Array,members:Dictionary,assignment:Dictionary,sizes:Dictionary)->String:
	var revision=model.get("presentation_revision")
	if revision==null:revision=JSON.stringify(model.source_roots.map(func(id):return model.identity(id))).sha256_text()
	# Array[Dictionary] and Array serialize differently despite equal values.
	# Cache geometry inputs, never container annotations, colours or UI fields.
	var source_spec:Array=[]
	for source in sources:source_spec.append(str(source.id))
	return "res://.polar-graphics/forest-geometry/"+var_to_bytes([revision,model.get("organization_revision"),source_spec,members,assignment,sizes]).hex_encode().sha256_text()

static func prepare(model,sources:Array,members:Dictionary,assignment:Dictionary,sizes:Dictionary,graph:Dictionary):
	var key:=cache_key(model,sources,members,assignment,sizes)
	_prepare_lock.lock()
	var cached=ResourceLoader.get_cached_ref(key)
	if cached==null:
		cached=load("res://depth_forest/forest_geometry.gd").new()
		cached.source_order=sources.duplicate(true);cached.group_members=members;cached.group_assignment=assignment
		cached._build(model,sizes,graph)
		if cached.last_route_error.is_empty():cached.take_over_path(key)
	_prepare_lock.unlock()
	return cached

func record_for(id:String)->Dictionary:return _source_records.get(id,projected_endpoints.get(id,{}))
func _world_point(value:Vector2,z_value:float)->Vector3:return Vector3(value.x*SCALE,-value.y*SCALE,z_value)
func _build(model,measured_sizes:Dictionary,graph_data:Dictionary)->void:
	_source_records=model.records
	var sources:Array=source_order
	var graph:Dictionary=graph_data
	projected_node_count=graph.nodes.size();projected_endpoints.clear()
	for item in graph.nodes:
		if not model.records.has(str(item.id)):
			var display: Dictionary=item.duplicate(true)
			display["key"]=str(item.id);display["uuid"]=str(item.reference.get("uuid",item.reference.get("node","")))
			display["source_sha256"]="";display["description"]=""
			display["properties"]={"reference":item.reference}
			projected_endpoints[str(item.id)]=display
	full_relations=graph.links.map(func(edge):return edge.relation)
	var references=model.get("reference_relations")
	if references is Array:full_relations=full_relations+references
	out_slots.clear();in_slots.clear()
	for relation in full_relations:
		var a:=str(relation.from);var b:=str(relation.to)
		if not out_slots.has(a):out_slots[a]=[]
		if not in_slots.has(b):in_slots[b]=[]
		out_slots[a].append(_slot_key(relation));in_slots[b].append(_slot_key(relation))
	var grouped_edges:Dictionary={}
	if not group_assignment.is_empty():
		for root in group_members:grouped_edges[root]=[]
		for relation in full_relations:
			var root:String=str(group_assignment.get(str(relation.from),""))
			if not root.is_empty() and root==str(group_assignment.get(str(relation.to),"")):
				grouped_edges[root].append({"from":relation.from,"to":relation.to,"relation":relation})
	var groups: Array=[]
	for i in sources.size():
		var root:=str(sources[i].id);var path:=str(model.records[root].source)
		var ids: Array=[];var sizes: Dictionary={}
		var members:Array=group_members.get(root,model.ordered_ids)
		for id in members:
			if not bool(model.records[id].external) and (group_members.has(root) or str(model.records[id].source)==path):
				ids.append(id);sizes[id]=measured_sizes[id]/SCALE;source_by_node[id]=root
		var edges:Array=grouped_edges.get(root,[])
		if group_assignment.is_empty():
			for relation in full_relations:
				if str(relation.from) in ids and str(relation.to) in ids:edges.append({"from":relation.from,"to":relation.to,"relation":relation})
		# Composition owns tree geometry. Semantic relationships remain original
		# slots and routes, but cannot pull a branch out of its hierarchy.
		var hierarchy: Array=edges.filter(func(edge):return bool(edge.relation.composition))
		if group_members.has(root):
			hierarchy=[]
			for parent in ids:
				for edge:Dictionary in model.composition_edges(parent):
					if str(group_assignment.get(str(edge.to),""))==root:hierarchy.append({"from":edge.from,"to":edge.to,"relation":edge})
		# Dev has only semantic local links; retain those genuine endpoints as
		# its local display graph without inventing composition relationships.
		if hierarchy.is_empty():hierarchy=edges
		var full:Dictionary=TreeProjection.GraphicsLayout.route(ids,hierarchy,sizes,"dot","LR",{},"",-1.0,"line")
		# Reuse Graphviz's existing ResourceCache; retain only strong leases
		# for visited partitions of this exact input generation.
		if not group_members.is_empty() and full.get("_geometry_resource") is Resource:
			var resource:Resource=full._geometry_resource
			geometry_leases[resource.get_instance_id()]=resource
		var shown: Array=[root]
		for relation in model.composition_edges(root):
			if bool(relation.resolved) and str(relation.to) in ids and str(relation.to) not in shown:shown.append(str(relation.to))
		# The dev source's local children use typed semantic slots rather than
		# contains; show their genuine directly linked endpoints as reading tier.
		if shown.size()==1:
			for edge in edges:
				if str(edge.from)==root and str(edge.to) not in shown:shown.append(str(edge.to))
		if not str(full.error).is_empty():last_route_error=str(full.error);return
		var rectangle: Rect2=TreeProjection._bounds(full.positions,sizes,full.routes)
		groups.append({"root":root,"ids":ids,"index":i,"shown":shown,"full":full,"rectangle":rectangle,"path":path,"sizes":sizes})
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
			if id in group.shown:reading_keys.append(id)
		for route in group.full.routes:
			var path_points:=PackedVector3Array()
			for p in route.points:path_points.append(_world_point(p+offset,0)+layer_offset+Vector3(0,0,PlaqueMesh.FRONT_Z+.012))
			_route_cache[_slot_key(route.relation)]=path_points

	var external_top:=5.4
	var max_x:=0.0
	for rectangle in source_bounds.values():max_x=maxf(max_x,rectangle.end.x)
	for id in model.ordered_ids+projected_endpoints.keys():
		if bool(record_for(id).external):
			bounds[id]=CARD_SIZE;records[id]={"node_id":id,"title":"Unloaded\n"+str(record_for(id).uuid).left(8)+"…"+str(record_for(id).uuid).right(4),"root_id":"external","branch":3,"depth":1,"source":"unloaded"}
			bounds[id]=measured_sizes[id]
			points[id]=source_offsets[str(sources[-1].id)]+Vector3(source_full_bounds[str(sources[-1].id)].end.x*SCALE+5,external_top-bounds[id].y*.5,0)
			external_top-=bounds[id].y+.30

	_prepare_layer_routes()
	for relation in full_relations:
		if points.has(str(relation.from)) and points.has(str(relation.to)) and not _route_cache.has(_slot_key(relation)):
			last_route_error="Prepared geometry omitted a visible relation";return
	_source_records={}
	group_members={};group_assignment={}

func _physical_layer(id:String)->String:
	return str(source_by_node.get(id,source_order[-1].id))

func _prepare_layer_routes()->void:
	var layers:Dictionary={};var bundles:Dictionary={}
	for source in source_order:
		layers[str(source.id)]={"ids":[],"positions":{},"sizes":{},"relations":[]}
	for id in points:
		var layer:Dictionary=layers[_physical_layer(id)]
		layer.ids.append(id);layer.positions[id]=points[id];layer.sizes[id]=bounds[id]
	for relation in full_relations:
		var a:=str(relation.from);var b:=str(relation.to)
		if not points.has(a) or not points.has(b):continue
		var sa:=_physical_layer(a);var sb:=_physical_layer(b)
		if sa==sb:
			layers[sa].relations.append(relation);continue
		var key:=JSON.stringify([sa,sb])
		if not bundles.has(key):bundles[key]={"from":sa,"to":sb,"relations":[],"starts":{},"ends":{}}
		var bundle:Dictionary=bundles[key]
		bundle.relations.append(relation)
		var depth:float=points[b].z-points[a].z
		var source_key:=_prepare_takeoff(a,true,depth)
		var target_key:=_prepare_takeoff(b,false,-depth)
		bundle.starts[source_key]=cross_segments[source_key][-1]
		bundle.ends[target_key]=cross_segments[target_key][0]
		cross_parts[_slot_key(relation)]=[source_key,JSON.stringify([key,a,"departure"]),key,JSON.stringify([key,b,"arrival"]),target_key]
	# The same-layer solve sees no remote gates or long cross-layer detours.
	for layer in layers.values():
		_route_cache.merge(prepare_planar_routes(layer.ids,layer.positions,layer.sizes,layer.relations),true)
	for key in bundles:
		var bundle:Dictionary=bundles[key]
		var start:=Vector3.ZERO;var end:=Vector3.ZERO
		for point in bundle.starts.values():start+=point
		for point in bundle.ends.values():end+=point
		start/=bundle.starts.size();end/=bundle.ends.size()
		var source_z:float=source_offsets[bundle.from].z+PlaqueMesh.FRONT_Z+.012
		var target_z:float=source_offsets[bundle.to].z+PlaqueMesh.FRONT_Z+.012
		start.z=lerpf(source_z,target_z,.22);end.z=lerpf(source_z,target_z,.78)
		_store_depth_segment(key,start,end)
		for relation in bundle.relations:
			var slot:=_slot_key(relation);var parts:Array=cross_parts[slot]
			_store_depth_segment(parts[1],cross_segments[parts[0]][-1],start)
			_store_depth_segment(parts[3],end,cross_segments[parts[4]][0])
			var path:=PackedVector3Array();var alpha:=PackedFloat32Array()
			for part in parts:
				path.append_array(cross_segments[part]);alpha.append_array(cross_segment_alpha[part])
			_route_cache[slot]=path;_cross_alpha[slot]=alpha
			var curve:=Curve3D.new();curve.bake_interval=.25
			for point in path:
				if curve.point_count==0 or not curve.get_point_position(curve.point_count-1).is_equal_approx(point):curve.add_point(point)
			curve.get_baked_length();_bridge_curves[slot]=curve

func _prepare_takeoff(id:String,outgoing:bool,depth:float)->String:
	var key:=JSON.stringify([id,outgoing,signf(depth)])
	if cross_segments.has(key):return key
	var direction:=Vector3(1 if outgoing else -1,0,0)
	var port:Vector3=points[id]+Vector3(direction.x*bounds[id].x*.5,signf(depth)*bounds[id].y*.15,PlaqueMesh.FRONT_Z+.012)
	var run:=.55
	var begin:=Vector2(port.x,port.y);var finish:=begin+Vector2(direction.x,0)*run
	for obstacle in points:
		if obstacle==id or _physical_layer(obstacle)!=_physical_layer(id):continue
		var box:=Rect2(Vector2(points[obstacle].x,points[obstacle].y)-bounds[obstacle]*.5,bounds[obstacle]).grow(.06)
		if not box.intersects(Rect2(begin,Vector2.ZERO).expand(finish).grow(.001)):continue
		var polygon:=PackedVector2Array([box.position,Vector2(box.end.x,box.position.y),box.end,Vector2(box.position.x,box.end.y)])
		for clipped in Geometry2D.intersect_polyline_with_polygon(PackedVector2Array([begin,finish]),polygon):
			for point in clipped:run=minf(run,maxf(0,begin.distance_to(point)-.06))
	var path:=SpaceBridge.takeoff(port,direction,signf(depth)*minf(4,absf(depth)*.15),run).tessellate(5,2.0)
	var alpha:=SpaceBridge.departure_alpha(path)
	if not outgoing:path.reverse();alpha.reverse()
	cross_segments[key]=path;cross_segment_alpha[key]=alpha
	return key

func _store_depth_segment(key:String,start:Vector3,end:Vector3)->void:
	if cross_segments.has(key):return
	var path:=SpaceBridge.depth_join(start,end).tessellate(5,2.0)
	var alpha:=PackedFloat32Array();alpha.resize(path.size());alpha.fill(.22)
	cross_segments[key]=path;cross_segment_alpha[key]=alpha

func prepare_planar_routes(ids: Array, positions: Dictionary, extents: Dictionary, relations: Array) -> Dictionary:
	var fixed: Dictionary={};var sizes: Dictionary={};var edges: Array=[]
	for id in ids:
		fixed[id]=Vector2(positions[id].x,-positions[id].y)/SCALE
		sizes[id]=extents[id]/SCALE
	for relation in relations:
		if str(relation.from) in ids and str(relation.to) in ids:
			edges.append({"from":str(relation.from),"to":str(relation.to),"relation":relation})
	if edges.is_empty():return {}
	route_build_count+=1
	var result: Dictionary=preload("res://addons/polar_graphics/graphviz.gd").smooth_obstacle_routes(ids,edges,sizes,fixed)
	if not str(result.error).is_empty():
		last_route_error=str(result.error);push_error(last_route_error);return {}
	var prepared: Dictionary={}
	for route in result.routes:
		var path:=PackedVector3Array()
		var a: Vector3=positions[route.from];var b: Vector3=positions[route.to]
		for i in route.points.size():
			var p: Vector2=route.points[i]
			path.append(Vector3(p.x*SCALE,-p.y*SCALE,lerpf(a.z,b.z,float(i)/maxf(1,route.points.size()-1))+PlaqueMesh.FRONT_Z+.012))
		prepared[_slot_key(route.relation)]=path
	return prepared

func _slot_key(relation: Dictionary) -> String:
	return JSON.stringify([relation.from,relation.slot])
