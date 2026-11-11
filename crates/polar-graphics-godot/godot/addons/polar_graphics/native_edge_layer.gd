extends Node3D
## Persistent native primitives. Routes are world-space presentation data;
## projection and clipping belong exclusively to the renderer.
var groups: Dictionary = {}
var _batches: Dictionary = {}
var _route_geometry: Dictionary = {}
var build_count := 0
var geometry_build_count := 0
var pick_segments: Array = []
var _snapshot: Array = []
var _cylinder: CylinderMesh
var _cone: CylinderMesh
var _picker: TriangleMesh
var _cache_context:=""
var _cache_resource:Resource
var _material: Material

func _init() -> void:
	_cylinder = ResourceLoader.get_cached_ref("res://.polar-graphics/edge-cylinder-v1") as CylinderMesh
	if _cylinder == null:
		_cylinder = CylinderMesh.new()
		_cylinder.top_radius = 1.0;_cylinder.bottom_radius = 1.0;_cylinder.height = 1.0
		_cylinder.radial_segments = 8;_cylinder.rings = 0
		_cylinder.take_over_path("res://.polar-graphics/edge-cylinder-v1")
	_cone = ResourceLoader.get_cached_ref("res://.polar-graphics/edge-cone-v1") as CylinderMesh
	if _cone == null:
		_cone = CylinderMesh.new()
		_cone.top_radius = 0.0;_cone.bottom_radius = 1.0;_cone.height = 1.0
		_cone.radial_segments = 8;_cone.rings = 0
		_cone.take_over_path("res://.polar-graphics/edge-cone-v1")
	_picker = _cylinder.generate_triangle_mesh()

func segment_transform(a: Vector3, b: Vector3, radius: float) -> Transform3D:
	var direction := b - a
	var basis := Basis(Quaternion(Vector3.UP, direction.normalized()))
	return Transform3D(basis.scaled_local(Vector3(radius, direction.length(), radius)), (a + b) * 0.5)

func sync(routes: Array, material: Material, cache_context:String="", leases:Dictionary={}) -> void:
	var context_changed:=cache_context!=_cache_context
	if context_changed:
		_cache_context=cache_context
		if cache_context.is_empty():
			_cache_resource=null;_route_geometry={}
		else:
			var path:="res://.polar-graphics/native-edge-cache/"+cache_context.sha256_text()
			_cache_resource=ResourceLoader.get_cached_ref(path)
			if _cache_resource==null:
				_cache_resource=Resource.new();_cache_resource.set_meta("routes",{});_cache_resource.take_over_path(path)
			_route_geometry=_cache_resource.get_meta("routes")
			leases[_cache_resource.get_instance_id()]=_cache_resource
	if material != _material:
		_material = material
		for group in groups.values():group.material_override = material
	if not context_changed and routes == _snapshot:return
	for route in routes:
		var alpha:PackedFloat32Array=route.get("point_alpha",PackedFloat32Array())
		if not alpha.is_empty() and alpha.size()!=route.points.size():
			push_error("Native edge point_alpha must match the retained path points.")
			return
	_snapshot = routes.duplicate(true)
	build_count += 1
	pick_segments.clear()
	var batches: Dictionary = {}
	var retained: Dictionary = {}
	for index in routes.size():
		var route: Dictionary = routes[index]
		var signature: Array = [route.points,route.radius,route.style,route.group,route.arrow,route.relations]
		var route_key = route.get("key",index)
		retained[route_key] = true
		var cached = _route_geometry.get(route_key)
		if cached == null or cached.signature != signature:
			geometry_build_count += 1
			var route_batches: Dictionary = {}
			var route_picks: Array = []
			var path: PackedVector3Array = route.points
			var radius: float = route.radius
			var style: String = route.style
			var phase := 0.0
			for i in range(1, path.size()):
				var a := path[i - 1]
				var b := path[i]
				var length := a.distance_to(b)
				if length < 0.00001:continue
				var transform := segment_transform(a, b, radius)
				route_picks.append({"transform": transform, "relations": route.relations})
				var offsets: Array = [-2.0, 2.0] if style == "double" else [0.0]
				for offset: float in offsets:
					var shift: Vector3 = transform.basis.x * offset
					if style in ["solid", "double"]:
						_append(route_batches, route.group, false, segment_transform(a + shift, b + shift, radius), route.color, i-1)
					else:
						# World-space dash instances are prepared only when routes change.
						var period := 0.48 if style != "dotted" else 0.16
						var on := 0.30 if style == "long_dash" else 0.052 if style == "dotted" else 0.24
						var cursor := -fposmod(phase, period)
						while cursor < length:
							_emit_run(route_batches, route, a + shift, b + shift, maxf(0, cursor), minf(length, cursor + on), length, i-1)
							if style == "dash_dot":_emit_run(route_batches, route, a + shift, b + shift, maxf(0, cursor + .34), minf(length, cursor + .40), length, i-1)
							cursor += period
				phase += length
			if route.arrow and path.size() > 1:
				var end := path[-1]
				var direction := end - path[-2]
				if direction.length_squared() > 0.000001:
					var head_length:=minf(radius*4,minf(phase*.18,direction.length()*.7))
					if head_length>radius*2:
						var head_radius:=minf(radius*1.6,head_length*.4)
						_append(route_batches, route.group, true, segment_transform(end-direction.normalized()*head_length,end,head_radius), route.color, path.size()-1, 0.0)
			cached = {"signature":signature.duplicate(true),"batches":route_batches,"picks":route_picks,"color":route.color,"point_alpha":PackedFloat32Array()}
			_route_geometry[route_key] = cached
		var point_alpha:PackedFloat32Array=route.get("point_alpha",PackedFloat32Array())
		if cached.color != route.color or cached.point_alpha != point_alpha:
			for batch in cached.batches.values():
				var colored: PackedFloat32Array = batch.items
				for i in range(0,colored.size(),16):
					colored[i+12]=route.color.r;colored[i+13]=route.color.g
					var weight:=1.0
					if not point_alpha.is_empty():
						var sample:Vector2=batch.samples[int(i/16)]
						var from:=int(sample.x);var to:=mini(from+1,point_alpha.size()-1)
						weight=lerpf(point_alpha[from],point_alpha[to],sample.y)
					colored[i+14]=route.color.b;colored[i+15]=route.color.a*weight
				batch.items = colored
			cached.color = route.color
			cached.point_alpha=point_alpha
		pick_segments.append_array(cached.picks)
		for key in cached.batches:
			var contribution: Dictionary = cached.batches[key]
			if not batches.has(key):batches[key] = {"arrow":contribution.arrow,"items":PackedFloat32Array()}
			batches[key].items.append_array(contribution.items)
	if cache_context.is_empty():
		for key in _route_geometry.keys():
			if not retained.has(key):_route_geometry.erase(key)
	for key in groups.keys():
		if not batches.has(key):
			remove_child(groups[key]);groups[key].queue_free();groups.erase(key);_batches.erase(key)
	for key in batches:
		var batch: Dictionary = batches[key]
		if _batches.get(key) == batch.items:continue
		var node: MultiMeshInstance3D = groups.get(key)
		if node == null:
			node = MultiMeshInstance3D.new();add_child(node);groups[key] = node
			node.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
			var instances := MultiMesh.new()
			instances.transform_format = MultiMesh.TRANSFORM_3D
			instances.use_colors = true
			instances.mesh = _cone if batch.arrow else _cylinder
			node.multimesh = instances
		node.material_override = material
		var count: int = batch.items.size()/16
		if node.multimesh.instance_count != count:node.multimesh.instance_count = count
		node.multimesh.buffer = batch.items
		_batches[key] = batch.items

func _emit_run(batches: Dictionary, route: Dictionary, a: Vector3, b: Vector3, start: float, finish: float, length: float, point_index: int) -> void:
	if finish <= start:return
	_append(batches, route.group, false, segment_transform(a.lerp(b, start / length), a.lerp(b, finish / length), route.radius), route.color, point_index, (start+finish)*.5/length)

func _append(batches: Dictionary, group: String, arrow: bool, transform: Transform3D, color: Color, point_index: int, point_t: float=.5) -> void:
	var key := group + ("/arrow" if arrow else "/line")
	if not batches.has(key):batches[key] = {"arrow": arrow, "items": PackedFloat32Array(), "samples":PackedVector2Array()}
	batches[key].samples.append(Vector2(point_index,point_t))
	# Godot TRANSFORM_3D + color native buffer: three matrix rows, then RGBA.
	var b:=transform.basis;var o:=transform.origin
	batches[key].items.append_array(PackedFloat32Array([
		b.x.x,b.y.x,b.z.x,o.x,b.x.y,b.y.y,b.z.y,o.y,b.x.z,b.y.z,b.z.z,o.z,
		color.r,color.g,color.b,color.a]))

func pick(camera: Camera3D, screen: Vector2) -> Array:
	var begin := camera.project_position(screen, camera.near)
	var end := camera.project_position(screen, camera.far)
	var nearest := INF
	var result: Array = []
	for segment in pick_segments:
		var transform: Transform3D = segment.transform
		# Native camera conversion preserves the existing thin-line hit target.
		var depth:=maxf(camera.near,-camera.to_local(transform.origin).z)
		var radius:=maxf(transform.basis.x.length(),camera.project_position(screen+Vector2(7,0),depth).distance_to(camera.project_position(screen,depth)))
		transform.basis.x=transform.basis.x.normalized()*radius
		transform.basis.z=transform.basis.z.normalized()*radius
		var inverse := transform.affine_inverse()
		var local_begin:=inverse*begin;var local_end:=inverse*end
		if not _cylinder.get_aabb().intersects_segment(local_begin,local_end):continue
		var hit := _picker.intersect_segment(local_begin,local_end)
		if hit.is_empty():continue
		var distance: float = begin.distance_to(transform * hit.position)
		if distance < nearest:
			nearest = distance;result = segment.relations
	return result
