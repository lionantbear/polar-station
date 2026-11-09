extends RefCounted

## Definition-level 3D overview. Exact versions stay distinct; only occurrences
## of the same definition collapse. The original typed relations are retained.
const WORLD_SCALE := 0.045
const Visuals = preload("res://itree_visual_semantics.gd")

static func build(model, scope: Dictionary = {}, filter_kind: String = "all") -> Dictionary:
	var included: Dictionary = {}
	var source_indices := Visuals.sources(model)
	var queue: Array = []
	var cursor := 0
	if str(scope.get("mode", "all")) == "branch":
		var root_id := str(model.resolve(str(scope.get("identity", ""))))
		if not root_id.is_empty() and bool(model.records[root_id].get("external", false)):
			root_id = ""
		queue = [root_id] if not root_id.is_empty() else []
		cursor = 0
		while cursor < queue.size():
			var id := str(queue[cursor])
			cursor += 1
			if included.has(id):
				continue
			included[id] = true
			for edge in model.composition_edges(id):
				if bool(edge.get("resolved", false)):
					queue.append(str(edge.to))
	else:
		for id in model.ordered_ids:
			included[id] = true
	var nodes: Array = []
	var links: Array = []
	var degrees: Dictionary = {}
	var unresolved := 0
	var placeholders: Dictionary = {}
	for edge in model.relation_edges:
		var structural := bool(edge.get("composition", false))
		if filter_kind == "tree" and not structural or filter_kind == "semantic" and structural:
			continue
		if not bool(edge.get("resolved", false)):
			if not included.has(str(edge.from)): continue
			unresolved += 1
			var identity := "unresolved:" + JSON.stringify(edge.get("reference", {})).sha256_text()
			var target := str(edge.to) if model.records.has(str(edge.to)) else identity
			if not model.records.has(target):
				placeholders[target] = {"id":target, "identity":identity, "title":"Not loaded · " + str(edge.get("reference",{}).get("node", edge.get("reference",{}).get("uuid", edge.target_kind))),
					"source":str(edge.get("reference",{}).get("content",{}).get("id","Unresolved source")), "source_index":-1,
					"type":"Unresolved target", "shape":"placeholder", "icon":"?", "status":"Not loaded", "external":true, "reference":edge.get("reference",{}).duplicate(true), "degree":0}
			else: included[target] = true
			var display_edge: Dictionary = edge.duplicate(true)
			display_edge.to = target
			links.append({"source":str(edge.from), "target":target, "relation":display_edge, "visual":Visuals.relation(display_edge)})
			continue
		if not included.has(str(edge.from)) or not included.has(str(edge.to)):
			continue
		links.append({"source": str(edge.from), "target": str(edge.to), "relation": edge.duplicate(true), "visual":Visuals.relation(edge)})
		degrees[str(edge.from)] = int(degrees.get(str(edge.from), 0)) + 1
		degrees[str(edge.to)] = int(degrees.get(str(edge.to), 0)) + 1
	# Link forces are sequential. Stable exact-source/slot order is part of
	# determinism, just as stable node order is; transient gNN IDs are not.
	links.sort_custom(func(a, b):
		var source_a := str(model.identity(str(a.source)))
		var source_b := str(model.identity(str(b.source)))
		return str(a.relation.get("slot", "")) < str(b.relation.get("slot", "")) if source_a == source_b else source_a < source_b)
	var ids: Array = included.keys()
	ids.sort_custom(func(a, b): return str(model.identity(str(a))) < str(model.identity(str(b))))
	for id in ids:
		var node := {"id":str(id), "identity":str(model.identity(str(id))), "title":str(model.records[id].title), "degree":int(degrees.get(id,0))}
		node.merge(Visuals.node(model.records[id],source_indices,id in model.source_roots),true)
		if bool(node.external):
			node.merge({"identity":"unresolved:"+JSON.stringify({"kind":"object","uuid":str(model.records[id].uuid)}).sha256_text(),"shape":"placeholder","icon":"?","source_index":-1,"status":"Not loaded","reference":{"kind":"object","uuid":str(model.records[id].uuid)}},true)
		nodes.append(node)
	for key in placeholders: nodes.append(placeholders[key])
	nodes.sort_custom(func(a:Dictionary,b:Dictionary):return str(a.identity)<str(b.identity))
	return {"nodes":nodes, "links":links, "unresolved":unresolved, "hidden_count":model.ordered_ids.size()-included.size(), "placeholder_count":placeholders.size()}

static func solver_input(graph: Dictionary, pins: Dictionary = {}) -> Dictionary:
	var nodes: Array = []
	var links: Array = []
	for item in graph.nodes:
		var node := {"id": str(item.id)}
		var pin = pins.get(str(item.identity))
		if pin is Vector3 and pin.is_finite():
			pin = [pin.x, pin.y, pin.z]
		if pin is Array and pin.size() == 3 and pin.all(func(value):
			return (value is float or value is int) and is_finite(float(value))):
			for axis in 3:
				node[["fx", "fy", "fz"][axis]] = float(pin[axis]) / WORLD_SCALE
		nodes.append(node)
	for edge in graph.links:
		links.append({"source": str(edge.source), "target": str(edge.target)})
	return {"nodes": nodes, "links": links, "config": {"seed": 42, "charge_strength": -65.0,
		"link_distance": 38.0, "alpha_decay": 0.0228, "velocity_decay": 0.4}}

static func coordinates(result: Dictionary) -> Dictionary:
	var output: Dictionary = {}
	for value in result.get("positions", []):
		output[str(value.id)] = Vector3(float(value.x), float(value.y), float(value.z)) * WORLD_SCALE
	return output
