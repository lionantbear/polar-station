extends RefCounted
## Generic Graphviz coordinates and edge routing. Keys and metadata are opaque;
## source interpretation, occurrence expansion and navigation belong to callers.
const PIXELS_PER_INCH := 72.0
static var _route_lock := Mutex.new()
static var executions := 0

static func _geometry() -> Dictionary:
	return {"positions": {}, "routes": [], "error": ""}

static func executable() -> String:
	var explicit: String = OS.get_environment("POLAR_LAB_DOT_BIN")
	if not explicit.is_empty():
		return explicit if FileAccess.file_exists(explicit) else ""
	var packaged := ProjectSettings.globalize_path("res://../graphviz/bin/dot")
	if FileAccess.file_exists(packaged): return packaged
	var name: String = "dot.exe" if OS.get_name() == "Windows" else "dot"
	var separator: String = ";" if OS.get_name() == "Windows" else ":"
	for directory in OS.get_environment("PATH").split(separator, false):
		var candidate: String = directory.path_join(name)
		if FileAccess.file_exists(candidate):
			return candidate
	return ""


static func route(keys: Array, edges: Array, sizes: Dictionary, engine: String = "dot",
		direction: String = "LR", fixed: Dictionary = {}, root: String = "", routing_margin: float = -1.0, routing_style: String = "spline") -> Dictionary:
	# Cache exact inputs, including edge metadata; callers mutate their own copy.
	var digest := HashingContext.new();digest.start(HashingContext.HASH_SHA256)
	digest.update(var_to_bytes([keys,edges,sizes,engine,direction,fixed,root,routing_margin,routing_style,executable()]))
	var key := "res://.polar-graphics/routes/" + digest.finish().hex_encode()
	_route_lock.lock()
	var resource: Resource = ResourceLoader.get_cached_ref(key)
	if resource == null:
		var geometry := _route_uncached(keys,edges,sizes,engine,direction,fixed,root,routing_margin,routing_style)
		if not str(geometry.error).is_empty():
			_route_lock.unlock();return geometry
		resource=Resource.new();resource.set_meta("geometry",geometry);resource.take_over_path(key)
	_route_lock.unlock()
	var result: Dictionary = resource.get_meta("geometry").duplicate(true)
	result["_geometry_resource"]=resource
	return result

static func _route_uncached(keys: Array, edges: Array, sizes: Dictionary, engine: String,
		direction: String, fixed: Dictionary, root: String, routing_margin: float, routing_style: String) -> Dictionary:
	var result := _geometry()
	if keys.is_empty():
		return result
	if keys.size() == 1 and edges.is_empty():
		result.positions[keys[0]] = fixed.get(keys[0], Vector2.ZERO)
		return result
	var executable := executable()
	if executable.is_empty():
		result.error = "Graphviz is required for precise node and edge layout; dot was not found."
		return result
	var aliases: Dictionary = {}
	var reverse: Dictionary = {}
	for index in keys.size():
		var alias := "n%d" % index
		aliases[keys[index]] = alias
		reverse[alias] = keys[index]
	var lines: Array[String] = ["digraph PreciseTree {",
		"graph [rankdir=%s, ordering=out, nodesep=0.22, ranksep=0.65, splines=%s, margin=0, outputorder=edgesfirst];" % [direction,routing_style],
		"node [shape=box, fixedsize=true, label=\"\", margin=0];",
		"edge [arrowhead=none, headclip=true, tailclip=true];"]
	if routing_style in ["ortho","line"]:lines.append("graph [nodesep=0.4, ranksep=1.8];")
	if routing_margin >= 0.0:
		# Opt-in obstacle clearance for callers supplying full card bounds.
		# Omitted margins retain Graphviz's existing default behavior.
		lines.append("graph [esep=\"+%.5f\"];" % routing_margin)
	if engine == "twopi":
		lines.append("graph [root=%s, overlap=prism, ranksep=2.4];" % aliases.get(root, aliases[keys[0]]))
	for key in keys:
		var size: Vector2 = sizes[key]
		var position := ""
		if not fixed.is_empty():
			var point: Vector2 = fixed[key]
			position = ", pos=\"%.8f,%.8f!\", pin=true" % [point.x, -point.y]
		lines.append("%s [width=%.8f, height=%.8f%s];" % [aliases[key], size.x / PIXELS_PER_INCH, size.y / PIXELS_PER_INCH, position])
	var edge_queues: Dictionary = {}
	for edge in edges:
		if not aliases.has(edge.from) or not aliases.has(edge.to):
			result.error = "A routed edge has a missing occurrence endpoint."
			return result
		var pair := str(aliases[edge.from]) + ">" + str(aliases[edge.to])
		if not edge_queues.has(pair):
			edge_queues[pair] = []
		edge_queues[pair].append(edge)
		lines.append("%s -> %s;" % [aliases[edge.from], aliases[edge.to]])
	lines.append("}")
	var temporary := FileAccess.create_temp(FileAccess.WRITE, "polar-graphics-routed", "dot")
	if temporary == null:
		result.error = "Cannot create temporary Graphviz layout input."
		return result
	temporary.store_string("\n".join(lines))
	temporary.flush()
	var args := PackedStringArray(["-K" + engine, "-Tplain"])
	if not fixed.is_empty():
		args.append("-n2")
	args.append(temporary.get_path_absolute())
	var output: Array = []
	executions += 1
	var status := OS.execute(executable, args, output, true)
	temporary = null
	if status != 0:
		result.error = "Graphviz %s routing exited %d." % [engine, status]
		return result
	if routing_margin >= 0.0 and "falling back to straight line edges" in "\n".join(output):
		result.error = "Graphviz obstacle routing fell back to straight lines; card clearance is insufficient."
		return result
	var graph_height := 0.0
	var pending_routes: Array = []
	for line in "\n".join(output).split("\n", false):
		var fields := line.split(" ", false)
		if fields.size() >= 4 and fields[0] == "graph":
			graph_height = fields[3].to_float() * PIXELS_PER_INCH
		elif fields.size() >= 6 and fields[0] == "node" and reverse.has(fields[1]):
			var point := Vector2(fields[2].to_float() * PIXELS_PER_INCH, graph_height - fields[3].to_float() * PIXELS_PER_INCH)
			if point.is_finite():
				result.positions[reverse[fields[1]]] = point
		elif fields.size() >= 6 and fields[0] == "edge":
			var pair := str(fields[1]) + ">" + str(fields[2])
			var count := fields[3].to_int()
			if count < 2 or fields.size() < 4 + count * 2 or not edge_queues.has(pair) or edge_queues[pair].is_empty():
				result.error = "Graphviz emitted an incomplete or unmatched edge route."
				return result
			var control_points := PackedVector2Array()
			for index in count:
				var point := Vector2(fields[4 + index * 2].to_float() * PIXELS_PER_INCH,
					graph_height - fields[5 + index * 2].to_float() * PIXELS_PER_INCH)
				if not point.is_finite():
					result.error = "Graphviz emitted a non-finite edge coordinate."
					return result
				control_points.append(point)
			var original: Dictionary = edge_queues[pair].pop_front()
			pending_routes.append({"from": original.from, "to": original.to,
				"points": control_points if routing_style == "ortho" else _flatten_spline(control_points), "relation": original.get("relation", {}).duplicate(true), "kind": "tree"})
	if result.positions.size() != keys.size() or pending_routes.size() != edges.size():
		result.error = "Incomplete Graphviz node/edge layout."
		return result
	result.routes = pending_routes
	if not fixed.is_empty():
		# neato -n2 can translate its fixed input into positive output bounds.
		# Undo exactly that translation; selecting semantic lines cannot move a
		# node or the user's camera. Verify every center before using the routes.
		_translate(result, Vector2(fixed[keys[0]]) - Vector2(result.positions[keys[0]]))
		var coordinate_scale := 1.0
		for value in fixed.values():
			coordinate_scale = maxf(coordinate_scale, Vector2(value).abs().length())
		# Plain output uses limited significant digits. Allow only its expected
		# rounding error at very large layouts, then restore the exact centers.
		var rounding_tolerance := 0.15 + coordinate_scale * 0.00003
		for key in keys:
			if Vector2(result.positions[key]).distance_to(Vector2(fixed[key])) > rounding_tolerance:
				result.error = "Fixed-position routing unexpectedly moved a node."
				result.routes.clear()
				return result
		result.positions = fixed.duplicate()
	for route in result.routes:
		var points: PackedVector2Array = route.points
		# Orthogonal callers trim padded ports after simplifying the straight
		# legs. Radial box clipping would introduce diagonal entry segments.
		if routing_style != "ortho":
			points[0] = _clip_to_box(result.positions[route.from], sizes[route.from], points[0])
			points[points.size() - 1] = _clip_to_box(result.positions[route.to], sizes[route.to], points[points.size() - 1])
		route.points = points
	return result


static func _clip_to_box(center: Vector2, size: Vector2, toward: Vector2) -> Vector2:
	var delta := toward - center
	if delta.length_squared() < 0.000001:
		return center + Vector2(size.x * 0.5, 0.0)
	var ratio := maxf(absf(delta.x) / (size.x * 0.5), absf(delta.y) / (size.y * 0.5))
	return center + delta / ratio


static func _flatten_spline(points: PackedVector2Array) -> PackedVector2Array:
	if points.size() < 4 or (points.size() - 1) % 3 != 0:
		return points
	var curve:=Curve2D.new()
	curve.add_point(points[0])
	for start in range(0,points.size()-1,3):
		curve.set_point_out(curve.point_count-1,points[start+1]-points[start])
		curve.add_point(points[start+3],points[start+2]-points[start+3],Vector2.ZERO)
	var sampled:=curve.tessellate(5,2.0)
	if sampled.size()<5:
		# Preserve the existing sampled-route contract even for a straight cubic.
		var length:=curve.get_baked_length()
		sampled=PackedVector2Array()
		for step in 5:sampled.append(curve.sample_baked(length*float(step)/4.0))
	return sampled


static func _translate(geometry: Dictionary, offset: Vector2) -> void:
	for key in geometry.positions:
		geometry.positions[key] += offset
	for route in geometry.routes:
		var points: PackedVector2Array = route.points
		for index in points.size():
			points[index] += offset
		route.points = points




## Graphviz owns orthogonal obstacle routing. Curve2D rounds only its bends;
## straight corridors and box-side entry/exit segments remain straight.
static func smooth_obstacle_routes(keys: Array, edges: Array, sizes: Dictionary, fixed: Dictionary) -> Dictionary:
	var obstacles: Dictionary={}
	# 11 units per side reserve the rounded corner plus the visible stroke.
	for key in keys:obstacles[key]=sizes[key]+Vector2(22,22)
	var result:=route(keys,edges,obstacles,"neato","LR",fixed,"",0.0,"ortho")
	if not str(result.error).is_empty():return result
	for edge in result.routes:
		var stroke_obstacles:Array[Rect2]=[]
		for key in keys:
			if key!=edge.from and key!=edge.to:stroke_obstacles.append(Rect2(fixed[key]-sizes[key]*.5,sizes[key]).grow(5.0))
		var points: PackedVector2Array=edge.points
		# Discard repeated cubic control samples along the same straight leg.
		var corners:=PackedVector2Array()
		for point in points:
			if not corners.is_empty() and point.distance_to(corners[-1])<.02:continue
			while corners.size()>1:
				var incoming: Vector2=corners[-1]-corners[-2]
				var outgoing: Vector2=point-corners[-1]
				if absf(incoming.normalized().cross(outgoing.normalized()))>.001 or incoming.dot(outgoing)<=0:break
				corners.remove_at(corners.size()-1)
			corners.append(point)
		_attach_orthogonal_port(corners,fixed[edge.from],sizes[edge.from],true)
		_attach_orthogonal_port(corners,fixed[edge.to],sizes[edge.to],false)
		var curve:=Curve2D.new()
		curve.add_point(corners[0])
		for i in range(1,corners.size()-1):
			var incoming: Vector2=corners[i]-corners[i-1]
			var outgoing: Vector2=corners[i+1]-corners[i]
			# Same nominal radius everywhere; shorten only in a tight corridor.
			# Leave a straight section even between two consecutive bends.
			var radius:=minf(40.0,minf(incoming.length(),outgoing.length())*.4)
			# An ortho corridor clears padded boxes, but a large inner fillet
			# can cut their corners. Limit only that bend using native geometry;
			# this is curve shaping, never another routing solve or frame task.
			while radius>.25 and not _rounded_turn_clear(corners[i],incoming.normalized(),outgoing.normalized(),radius,stroke_obstacles):radius*=.5
			var a:=corners[i]-incoming.normalized()*radius
			var b:=corners[i]+outgoing.normalized()*radius
			curve.add_point(a,Vector2.ZERO,incoming.normalized()*radius*.55228475)
			curve.add_point(b,-outgoing.normalized()*radius*.55228475,Vector2.ZERO)
		curve.add_point(corners[-1])
		# Native adaptive tessellation: straight legs cost one segment.
		edge.points=curve.tessellate(5,2.0)
	return result


static func _rounded_turn_clear(corner:Vector2,incoming:Vector2,outgoing:Vector2,radius:float,obstacles:Array[Rect2])->bool:
	var a:=corner-incoming*radius;var b:=corner+outgoing*radius
	var envelope:=Rect2(corner,Vector2.ZERO).expand(a).expand(b)
	var arc:PackedVector2Array=PackedVector2Array()
	for obstacle in obstacles:
		if not envelope.intersects(obstacle):continue
		if arc.is_empty():
			var curve:=Curve2D.new();curve.add_point(a,Vector2.ZERO,incoming*radius*.55228475);curve.add_point(b,-outgoing*radius*.55228475,Vector2.ZERO)
			arc=curve.tessellate(5,2.0)
		var polygon:=PackedVector2Array([obstacle.position,Vector2(obstacle.end.x,obstacle.position.y),obstacle.end,Vector2(obstacle.position.x,obstacle.end.y)])
		if not Geometry2D.intersect_polyline_with_polygon(arc,polygon).is_empty():return false
	return true


static func _attach_orthogonal_port(points: PackedVector2Array, center: Vector2, size: Vector2, first: bool) -> void:
	var i:=0 if first else points.size()-1
	var adjacent:=1 if first else points.size()-2
	var leg:=points[adjacent]-points[i]
	var box:=Rect2(center-size*.5,size)
	var original:=points[i]
	var port:Vector2=original.clamp(box.position,box.end)
	var junction:=port
	if absf(leg.x)>absf(leg.y):
		port.x=center.x+signf(original.x-center.x)*size.x*.5
		junction=Vector2(port.x,original.y)
	else:
		port.y=center.y+signf(original.y-center.y)*size.y*.5
		junction=Vector2(original.x,port.y)
	# A padded-box corner port can lie beyond the real card. Extend that
	# existing corridor with a short orthogonal stub; never move its long leg.
	points[i]=junction
	if not junction.is_equal_approx(port):
		if first:points.insert(0,port)
		else:points.append(port)
