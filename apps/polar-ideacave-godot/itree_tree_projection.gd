extends RefCounted

## Read-only occurrence projection for the precise tree and content canvas.
## All geometry is x-right/y-down layout pixels. Positions are CENTERS, not
## GraphEdit offsets. The renderer alone converts pixels to world coordinates.
## Graphviz owns both node placement and edge routing. No source text enters
## DOT: only generated aliases, measured bounds and numeric coordinates do.
const Occurrences = preload("res://itree_map_layout.gd")
const GraphicsLayout = preload("res://addons/polar_graphics/graphviz.gd")
const Visuals = preload("res://itree_visual_semantics.gd")
const MIN_SIZE := Vector2(160.0, 46.0)
const MAX_SIZE := Vector2(240.0, 68.0)
const ROOT_GAP := 84.0


static func root_occurrence(model, id: String) -> String:
	return Occurrences.root_occurrence(model, id)


static func child_occurrence(parent: String, relation: Dictionary) -> String:
	return Occurrences.child_occurrence(parent, relation)


static func path_to(model, id: String, preferred_root: String = "") -> Dictionary:
	return Occurrences.path_to(model, id, preferred_root)


static func node_size(title: String) -> Vector2:
	# Use the same real font metrics for ASCII, CJK and mixed titles. Compact
	# nodes reserve two title lines at most; original content belongs in Canvas.
	var measured := 0.0
	var font: Font = ThemeDB.fallback_font
	var sample := title.left(160)
	if font != null:
		measured = font.get_string_size(sample, HORIZONTAL_ALIGNMENT_LEFT, -1, 14).x
	else:
		measured = float(sample.length()) * 9.0
	var width := clampf(ceilf(measured + 40.0), MIN_SIZE.x, MAX_SIZE.x)
	var height := MIN_SIZE.y if measured <= width - 32.0 else MAX_SIZE.y
	return Vector2(width, height)


static func build(model, roots: Array, expanded: Dictionary, budget: int = 240,
		arrangement: String = "balanced", priority_occurrence: String = "") -> Dictionary:
	var result := _collect(model, roots, expanded, budget, priority_occurrence)
	result["routes"] = []
	result["bounds"] = Rect2()
	result["backend"] = "graphviz-routed"
	result["semantic_error"] = ""
	if not str(result.error).is_empty() or result.items.is_empty():
		return result
	if arrangement not in ["balanced", "horizontal", "vertical", "radial"]:
		result.error = "Unknown tree arrangement: " + arrangement
		return result
	_add_unresolved(model,result,budget)
	var source_indices := Visuals.sources(model)
	for key in result.items:
		var item: Dictionary = result.items[key]
		if model.records.has(item.node_id):item.merge(Visuals.node(model.records[item.node_id], source_indices, int(item.depth)==0 or item.node_id in model.source_roots), true)
		result.sizes[key] = node_size(str(result.items[key].title))
	var groups: Array[Dictionary] = []
	for root_key in result.roots:
		var keys: Array = []
		for key in result.items:
			if str(result.items[key].root_id) == str(result.items[root_key].root_id):
				keys.append(key)
		var edges: Array = []
		for edge in result.edges:
			if str(result.items[edge.from].root_id) == str(result.items[root_key].root_id):
				edges.append(edge)
		var geometry := _balanced(root_key, keys, edges, result) if arrangement == "balanced" else _graphviz(
			keys, edges, result.sizes, "twopi" if arrangement == "radial" else "dot",
			"TB" if arrangement == "vertical" else "LR", {}, root_key)
		if not str(geometry.error).is_empty():
			result.error = geometry.error
			result.positions.clear()
			result.routes.clear()
			return result
		groups.append(geometry)
	# Independent source roots remain independent. Pack their already-routed
	# layouts; never invent a semantic parent merely to position a forest.
	var columns := maxi(1, ceili(sqrt(float(groups.size()))))
	var x := 0.0
	var y := 0.0
	var row_height := 0.0
	for index in groups.size():
		var group: Dictionary = groups[index]
		var bounds := _bounds(group.positions, result.sizes, group.routes)
		var offset := Vector2(x, y) - bounds.position
		_translate(group, offset)
		result.positions.merge(group.positions)
		result.routes.append_array(group.routes)
		x += bounds.size.x + ROOT_GAP
		row_height = maxf(row_height, bounds.size.y)
		if (index + 1) % columns == 0:
			x = 0.0
			y += row_height + ROOT_GAP
			row_height = 0.0
	for route in result.routes:route["visual"]=Visuals.relation(route.get("relation",{}))
	result.bounds = _bounds(result.positions, result.sizes, result.routes)
	result.backend = "graphviz-twopi-routed" if arrangement == "radial" else "graphviz-dot-routed"
	if result.edges.is_empty():
		result.backend = "trivial"
	return result


static func _add_unresolved(model,layout:Dictionary,budget:int)->void:
	var visible:Dictionary={}
	for key in layout.items:
		var id:=str(layout.items[key].node_id)
		if not visible.has(id):visible[id]=key
	var ordinal:=0
	for edge:Dictionary in model.relation_edges:
		if bool(edge.get("resolved",false)) or not visible.has(str(edge.from)):continue
		if layout.items.size()>=clampi(budget,1,Occurrences.MAX_BUDGET):
			layout.hidden_count+=1;continue
		var owner:=str(visible[str(edge.from)]);var key:=owner+"/unresolved:"+str(edge.slot)
		var item:Dictionary=layout.items[owner].duplicate(true)
		var title:="Reference not loaded"
		item.merge({"occurrence":key,"node_id":"unresolved:"+JSON.stringify(edge.reference).sha256_text(),"title":title,"parent":"","external":true,"type":"Unresolved target","shape":"placeholder","icon":"?","status":"Not loaded","container":false,"child_count":0,"unresolved_child_count":0,"reference":edge.reference.duplicate(true),"source":str(model.records.get(str(layout.items[owner].node_id),{}).get("source","")),"source_index":-1,"depth":int(item.depth)+1,"branch":ordinal if int(item.depth)==0 else int(item.branch)},true)
		ordinal+=1;layout.items[key]=item
		# Graphviz receives the original typed edge. This is a display endpoint,
		# never a fabricated composition child or admitted model record.
		layout.edges.append({"from":owner,"to":key,"relation":edge.duplicate(true)})
		layout["placeholder_count"]=int(layout.get("placeholder_count",0))+1

static func _collect(model, roots: Array, expanded: Dictionary, budget: int, priority: String) -> Dictionary:
	# Reuse the existing occurrence identity/item/path contracts without calling
	# its GraphEdit-specific layout, avoiding two Graphviz passes per tree.
	var result: Dictionary = Occurrences._result()
	var cap := clampi(budget, 1, Occurrences.MAX_BUDGET)
	var reservation: Dictionary = Occurrences._priority_chain(model, roots, priority)
	if not str(reservation.error).is_empty():
		result.error = reservation.error
		return result
	var required: Dictionary = reservation.keys.duplicate()
	if required.size() > cap:
		result.error = "The requested path needs %d visible occurrences; budget is %d" % [required.size(), cap]
		return result
	var queue: Array[String] = []
	var ancestry: Dictionary = {}
	for value in roots:
		var id := str(value)
		if not model.records.has(id):
			result.error = "Unknown display root: " + id
			return result
		var key := root_occurrence(model, id)
		if result.items.has(key):
			continue
		if not required.has(key) and result.items.size() >= cap - required.size():
			result.hidden_count += 1
			continue
		required.erase(key)
		result.items[key] = Occurrences._item(model, id, key, "", "", [], id, 0, -1, bool(expanded.get(key, false)))
		result.roots.append(key)
		queue.append(key)
		ancestry[key] = [id]
	var cursor := 0
	while cursor < queue.size():
		var parent_key := queue[cursor]
		cursor += 1
		var parent: Dictionary = result.items[parent_key]
		result.unresolved_count += int(parent.unresolved_child_count)
		if not parent.expanded:
			continue
		var ordinal := 0
		for relation in model.composition_edges(str(parent.node_id)):
			if not Occurrences._eligible(model, relation):
				continue
			var child_id := str(relation.to)
			if child_id in ancestry[parent_key]:
				result.error = "Composition cycle at slot %s of %s" % [str(relation.slot), str(parent.node_id)]
				return result
			var branch := ordinal if int(parent.depth) == 0 else int(parent.branch)
			ordinal += 1
			var key := child_occurrence(parent_key, relation)
			if not required.has(key) and result.items.size() >= cap - required.size():
				parent.hidden_child_count += 1
				result.hidden_count += 1
				continue
			required.erase(key)
			var path: Array = parent.path.duplicate()
			path.append(str(relation.slot))
			result.items[key] = Occurrences._item(model, child_id, key, parent_key, str(relation.slot), path,
				str(parent.root_id), int(parent.depth) + 1, branch, bool(expanded.get(key, false)), relation)
			result.edges.append({"from": parent_key, "to": key, "relation": relation.duplicate(true)})
			var trail: Array = ancestry[parent_key].duplicate()
			trail.append(child_id)
			ancestry[key] = trail
			queue.append(key)
	if not required.is_empty():
		result.error = "The requested occurrence path has a collapsed ancestor; expand its actual relation slots first"
	return result


static func _geometry() -> Dictionary:
	return {"positions": {}, "routes": [], "error": ""}


static func _balanced(root: String, keys: Array, edges: Array, layout: Dictionary) -> Dictionary:
	var result := _geometry()
	result.positions[root] = Vector2.ZERO
	for side in [1.0, -1.0]:
		var subset: Array = [root]
		for key in keys:
			if key == root:
				continue
			var branch_side := 1.0 if int(layout.items[key].branch) % 2 == 0 else -1.0
			if branch_side == side:
				subset.append(key)
		if subset.size() < 2:
			continue
		var selected_edges: Array = []
		for edge in edges:
			if edge.from in subset and edge.to in subset:
				selected_edges.append(edge)
		var geometry := _graphviz(subset, selected_edges, layout.sizes)
		if not str(geometry.error).is_empty():
			return geometry
		var anchor: Vector2 = geometry.positions[root]
		for key in geometry.positions:
			var relative: Vector2 = geometry.positions[key] - anchor
			result.positions[key] = Vector2(relative.x * side, relative.y)
		for route in geometry.routes:
			var points := PackedVector2Array()
			for point in route.points:
				var relative: Vector2 = point - anchor
				points.append(Vector2(relative.x * side, relative.y))
			route.points = points
			result.routes.append(route)
	return result


static func semantic_routes(model, layout: Dictionary, selected_occurrence: String, all: bool = false) -> Array:
	layout["semantic_error"] = ""
	if not str(layout.get("error", "")).is_empty() or layout.get("positions", {}).is_empty():
		return []
	var selected_id := str(layout.items.get(selected_occurrence, {}).get("node_id", ""))
	var visible: Dictionary = {}
	for key in layout.items:
		var id := str(layout.items[key].node_id)
		if not visible.has(id):
			visible[id] = key
	if not selected_id.is_empty():
		visible[selected_id] = selected_occurrence
	var edges: Array = []
	for relation in model.relation_edges:
		if bool(relation.composition) or not bool(relation.resolved):
			continue
		if not all and str(relation.from) != selected_id and str(relation.to) != selected_id:
			continue
		if not visible.has(str(relation.from)) or not visible.has(str(relation.to)):
			continue
		edges.append({"from": visible[str(relation.from)], "to": visible[str(relation.to)],
			"relation": relation.duplicate(true)})
	if edges.is_empty():
		return []
	var geometry := _graphviz(layout.items.keys(), edges, layout.sizes, "neato", "LR", layout.positions)
	if not str(geometry.error).is_empty():
		layout.semantic_error = geometry.error
		return []
	for route in geometry.routes:
		route.kind = "semantic"
		route["visual"]=Visuals.relation(route.get("relation",{}))
	return geometry.routes


static func _graphviz(keys: Array, edges: Array, sizes: Dictionary, engine: String = "dot",
		direction: String = "LR", fixed: Dictionary = {}, root: String = "", routing_margin: float = -1.0) -> Dictionary:
	return GraphicsLayout.route(keys, edges, sizes, engine, direction, fixed, root, routing_margin)

static func _translate(geometry: Dictionary, offset: Vector2) -> void:
	GraphicsLayout._translate(geometry, offset)

static func _bounds(positions: Dictionary, sizes: Dictionary, routes: Array) -> Rect2:
	var bounds := Rect2()
	var first := true
	for key in positions:
		var rectangle := Rect2(Vector2(positions[key]) - Vector2(sizes[key]) * 0.5, sizes[key])
		bounds = rectangle if first else bounds.merge(rectangle)
		first = false
	for route in routes:
		for point in route.points:
			bounds = bounds.expand(point)
	return bounds

static func relationship_focus(model,id:String,origin:Dictionary={},siblings:bool=true)->Dictionary:
	var result:Dictionary=Occurrences.build_focus(model,id,origin,0,siblings)
	var indices:=Visuals.sources(model)
	for key in result.items:
		var item:Dictionary=result.items[key]
		if model.records.has(str(item.node_id)):item.merge(Visuals.node(model.records[item.node_id],indices,str(item.node_id) in model.source_roots),true)
		result.sizes[key]=node_size(str(item.title))
	result["routes"]=[];result["bounds"]=_bounds(result.positions,result.sizes,[]);result["flat_layout"]=siblings
	return result

static func fixed_routes(layout:Dictionary,edges:Array)->Array:
	var geometry:=_graphviz(layout.items.keys(),edges,layout.sizes,"neato","LR",layout.positions)
	layout["semantic_error"]=geometry.error
	for route:Dictionary in geometry.routes:
		route["kind"]="semantic";route["visual"]=Visuals.relation(route.get("relation",{}))
	return geometry.routes

static func complete_overview(model)->Dictionary:
	# Reuse the already-complete exact-version graph for overview membership.
	# Occurrence expansion budgets may limit a reading tree, never this map.
	var graph:Dictionary=preload("res://itree_graph_projection.gd").build(model)
	var result:Dictionary=Occurrences._result();result["routes"]=[];result["complete_semantic_edges"]=[]
	var keys:Dictionary={}
	for node:Dictionary in graph.nodes:
		var id:=str(node.id)
		var key:=root_occurrence(model,id) if model.records.has(id) else "overview:"+str(node.identity).sha256_text()
		keys[id]=key
		var item:Dictionary=node.duplicate(true)
		item.merge({"occurrence":key,"node_id":id,"parent":"","root_id":id,"depth":0 if id in model.source_roots else 1,"path":[],"slot":"","role":"root" if id in model.source_roots else "branch","child_count":0,"unresolved_child_count":0,"expanded":false},true)
		result.items[key]=item;result.sizes[key]=node_size(str(item.title));result.roots.append(key)
	for link:Dictionary in graph.links:
		var edge:Dictionary={"from":keys[str(link.source)],"to":keys[str(link.target)],"relation":link.relation.duplicate(true)}
		if bool(link.relation.get("composition",false)):result.edges.append(edge)
		else:result.complete_semantic_edges.append(edge)
	var geometry:=_graphviz(result.items.keys(),result.edges,result.sizes)
	result.positions=geometry.positions;result.routes=geometry.routes;result.error=geometry.error
	for route:Dictionary in result.routes:route["visual"]=Visuals.relation(route.get("relation",{}))
	result["bounds"]=_bounds(result.positions,result.sizes,result.routes);result.backend="complete-graphviz-forest"
	return result
