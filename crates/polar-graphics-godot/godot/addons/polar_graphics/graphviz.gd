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
		direction: String = "LR", fixed: Dictionary = {}, root: String = "", routing_margin: float = -1.0) -> Dictionary:
	# Cache exact inputs, including edge metadata; callers mutate their own copy.
	var digest := HashingContext.new();digest.start(HashingContext.HASH_SHA256)
	digest.update(var_to_bytes([keys,edges,sizes,engine,direction,fixed,root,routing_margin,executable()]))
	var key := "res://.polar-graphics/routes/" + digest.finish().hex_encode()
	_route_lock.lock()
	var resource: Resource = ResourceLoader.get_cached_ref(key)
	if resource == null:
		var geometry := _route_uncached(keys,edges,sizes,engine,direction,fixed,root,routing_margin)
		if not str(geometry.error).is_empty():
			_route_lock.unlock();return geometry
		resource=Resource.new();resource.set_meta("geometry",geometry);resource.take_over_path(key)
	_route_lock.unlock()
	var result: Dictionary = resource.get_meta("geometry").duplicate(true)
	result["_geometry_resource"]=resource
	return result

static func _route_uncached(keys: Array, edges: Array, sizes: Dictionary, engine: String,
		direction: String, fixed: Dictionary, root: String, routing_margin: float) -> Dictionary:
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
		"graph [rankdir=%s, ordering=out, nodesep=0.22, ranksep=0.65, splines=true, margin=0, outputorder=edgesfirst];" % direction,
		"node [shape=box, fixedsize=true, label=\"\", margin=0];",
		"edge [arrowhead=none, headclip=true, tailclip=true];"]
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
				"points": _flatten_spline(control_points), "relation": original.get("relation", {}).duplicate(true), "kind": "tree"})
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
	var flattened := PackedVector2Array([points[0]])
	for start in range(0, points.size() - 1, 3):
		var a := points[start]
		var b := points[start + 1]
		var c := points[start + 2]
		var d := points[start + 3]
		var length := a.distance_to(b) + b.distance_to(c) + c.distance_to(d)
		var steps := clampi(ceili(length / 12.0), 4, 80)
		for step in range(1, steps + 1):
			var t := float(step) / float(steps)
			var u := 1.0 - t
			flattened.append(u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d)
	return flattened


static func _translate(geometry: Dictionary, offset: Vector2) -> void:
	for key in geometry.positions:
		geometry.positions[key] += offset
	for route in geometry.routes:
		var points: PackedVector2Array = route.points
		for index in points.size():
			points[index] += offset
		route.points = points


