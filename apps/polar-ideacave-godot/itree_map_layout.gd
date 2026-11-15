extends RefCounted

## Presentation projection only. Runtime I-tree and reference authority stay
## in Polar's Rust model. Graphviz supplies card coordinates; Godot GraphEdit
## owns all drawing, input and the explicitly reported native fallback.

const CARD_SIZE := Vector2(260.0, 128.0)
const DEFAULT_BUDGET := 240
const MAX_BUDGET := 1200
const POINTS_PER_INCH := 72.0

static func root_occurrence(model, id: String) -> String:
	return "o_" + str(model.identity(id)).sha256_text()

static func child_occurrence(parent: String, edge: Dictionary) -> String:
	return parent + "/" + str(edge.get("slot", "")).uri_encode()

static func _result() -> Dictionary:
	return {"items": {}, "edges": [], "positions": {}, "sizes": {}, "roots": [],
		"hidden_count": 0, "unresolved_count": 0, "backend": "godot-native", "error": "", "warning": ""}

static func _eligible(model, edge: Dictionary) -> bool:
	var target: String = str(edge.get("to", ""))
	return bool(edge.get("resolved", false)) and model.records.has(target) and not bool(model.records[target].get("external", false))

static func _item(model, node_id: String, key: String, parent: String, slot: String,
	path: Array, root_id: String, depth: int, branch: int, expanded: bool, relation: Dictionary = {}) -> Dictionary:
	var record: Dictionary = model.records[node_id]
	var title: String = str(record.get("title", node_id))
	var properties: Dictionary = relation.get("properties", {})
	if str(relation.get("type", "")) == "polar.fs.entry.v1" and properties.get("name") is String and not str(properties.name).is_empty():
		title = str(properties.name)
	var unresolved: int = 0
	var child_edges: Array = model.composition_edges(node_id)
	for edge in child_edges:
		if not _eligible(model, edge):
			unresolved += 1
	return {"occurrence": key, "node_id": node_id, "parent": parent, "slot": slot,
		"path": path.duplicate(), "root_id": root_id, "depth": depth, "branch": branch,
		"title": title, "child_count": child_edges.size(), "expanded": expanded,
		"hidden_child_count": 0, "unresolved_child_count": unresolved, "role": "root" if depth == 0 else "branch"}

static func build(model, roots: Array, expanded: Dictionary, arrangement: String = "balanced", budget: int = DEFAULT_BUDGET, priority_occurrence: String = "") -> Dictionary:
	var result: Dictionary = _result()
	var cap: int = clampi(budget, 1, MAX_BUDGET)
	var reservation: Dictionary = _priority_chain(model, roots, priority_occurrence)
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
		var id: String = str(value)
		if not model.records.has(id):
			result.error = "Unknown display root: " + id
			return result
		var key: String = root_occurrence(model, id)
		if result.items.has(key):
			continue
		if not required.has(key) and result.items.size() >= cap - required.size():
			result.hidden_count += 1
			continue
		required.erase(key)
		result.items[key] = _item(model, id, key, "", "", [], id, 0, -1, bool(expanded.get(key, false)))
		result.sizes[key] = CARD_SIZE
		result.roots.append(key)
		queue.append(key)
		ancestry[key] = [id]
	var cursor: int = 0
	while cursor < queue.size():
		var parent_key: String = queue[cursor]
		cursor += 1
		var parent: Dictionary = result.items[parent_key]
		result.unresolved_count += int(parent.unresolved_child_count)
		if not parent.expanded:
			continue
		var ordinal: int = 0
		for relation in model.composition_edges(str(parent.node_id)):
			if not _eligible(model, relation):
				continue
			var child_id: String = str(relation.to)
			if child_id in ancestry[parent_key]:
				result.error = "Composition cycle at slot %s of %s" % [str(relation.slot), str(parent.node_id)]
				return result
			var branch: int = ordinal if int(parent.depth) == 0 else int(parent.branch)
			ordinal += 1
			var key: String = child_occurrence(parent_key, relation)
			if not required.has(key) and result.items.size() >= cap - required.size():
				parent.hidden_child_count += 1
				result.hidden_count += 1
				continue
			required.erase(key)
			var path: Array = parent.path.duplicate()
			path.append(str(relation.slot))
			result.items[key] = _item(model, child_id, key, parent_key, str(relation.slot), path,
				str(parent.root_id), int(parent.depth) + 1, branch, bool(expanded.get(key, false)), relation)
			result.sizes[key] = CARD_SIZE
			result.edges.append({"from": parent_key, "to": key, "relation": relation.duplicate(true)})
			var trail: Array = ancestry[parent_key].duplicate()
			trail.append(child_id)
			ancestry[key] = trail
			queue.append(key)
	if not required.is_empty():
		result.error = "The requested occurrence path has a collapsed ancestor; expand its actual relation slots first"
		return result
	_apply_map_positions(result, arrangement)
	return result

static func _priority_chain(model, roots: Array, requested: String) -> Dictionary:
	var result: Dictionary = {"keys": {}, "error": ""}
	if requested.is_empty():
		return result
	var parts: PackedStringArray = requested.split("/", true)
	var current_id: String = ""
	for value in roots:
		if model.records.has(str(value)) and root_occurrence(model, str(value)) == parts[0]:
			current_id = str(value)
			break
	if current_id.is_empty():
		result.error = "Requested occurrence does not belong to the displayed roots"
		return result
	var current_key: String = parts[0]
	result.keys[current_key] = true
	for index in range(1, parts.size()):
		var slot: String = parts[index].uri_decode()
		var found: bool = false
		for edge in model.composition_edges(current_id):
			if str(edge.slot) == slot and _eligible(model, edge):
				current_key = child_occurrence(current_key, edge)
				current_id = str(edge.to)
				result.keys[current_key] = true
				found = true
				break
		if not found:
			result.error = "Requested occurrence contains an unresolved or absent relation slot: " + slot
			return result
	return result

static func path_to(model, id: String, preferred_root: String = "") -> Dictionary:
	if not model.records.has(id):
		return {"root": "", "path": [], "error": "Unknown node: " + id}
	var roots: Array[String] = []
	if not preferred_root.is_empty() and model.records.has(preferred_root):
		roots.append(preferred_root)
	for value in model.source_roots:
		if str(value) not in roots:
			roots.append(str(value))
	# A bounded BFS chooses a deterministic shortest display path, not a
	# canonical data parent. The browser retains a previously entered path.
	for candidate in roots:
		var queue: Array[Dictionary] = [{"id": candidate, "path": []}]
		var visited: Dictionary = {candidate: true}
		var cursor: int = 0
		while cursor < queue.size():
			var current: Dictionary = queue[cursor]
			cursor += 1
			if str(current.id) == id:
				return {"root": candidate, "root_id": candidate, "path": current.path, "error": ""}
			for relation in model.composition_edges(str(current.id)):
				if not _eligible(model, relation) or visited.has(str(relation.to)):
					continue
				var path: Array = current.path.duplicate()
				path.append(str(relation.slot))
				visited[str(relation.to)] = true
				queue.append({"id": str(relation.to), "path": path})
	return {"root": id, "root_id": id, "path": [], "detached": true, "error": ""}

static func _key_at_path(model, root_id: String, path: Array) -> String:
	var key: String = root_occurrence(model, root_id)
	for slot in path:
		key = child_occurrence(key, {"slot": str(slot)})
	return key

static func _node_at_path(model, root_id: String, path: Array) -> String:
	var current: String = root_id
	for slot in path:
		var next: String = ""
		for relation in model.composition_edges(current):
			if str(relation.slot) == str(slot) and _eligible(model, relation):
				next = str(relation.to)
				break
		if next.is_empty():
			return ""
		current = next
	return current

static func build_focus(model, id: String, origin_item: Dictionary = {}, _budget: int = 0, include_siblings: bool = true) -> Dictionary:
	# Extend the existing local focus projection: every real direct relation is
	# retained, independent of occurrence/view culling. No 120-neighbor budget.
	var result:Dictionary=_result()
	result["groups"]={};result["anchor_id"]=id
	if not model.records.has(id):result.error="Unknown focus node: "+id;return result
	var anchor:=str(origin_item.get("occurrence",root_occurrence(model,id)))
	var center:=_item(model,id,anchor,"","",[],id,0,-1,false)
	center.role="focus";result.items[anchor]=center;result.sizes[anchor]=CARD_SIZE;result.roots=[anchor]
	var parents:Array=[]
	for edge:Dictionary in model.relation_edges:
		if not bool(edge.get("composition",false)) or str(edge.get("to",""))!=id:continue
		var parent:=str(edge.from)
		var key:=_focus_endpoint(model,result,parent,"parent",parent,edge)
		if parent not in parents:parents.append(parent)
		result.edges.append({"from":key,"to":anchor,"relation":edge.duplicate(true)})
	for edge:Dictionary in model.relation_edges:
		var from_id:=str(edge.get("from",""));var to_id:=str(edge.get("to",""))
		if from_id!=id and to_id!=id:continue
		if bool(edge.get("composition",false)) and to_id==id:continue
		var outgoing:=from_id==id;var other:=to_id if outgoing else from_id
		var role:="child" if outgoing and bool(edge.get("composition",false)) else "related"
		var key:=anchor if other==id else _focus_endpoint(model,result,other,role,"",edge)
		result.edges.append({"from":anchor if outgoing else key,"to":key if outgoing else anchor,"relation":edge.duplicate(true)})
	if include_siblings:
		for parent:String in parents:
			for edge:Dictionary in model.composition_edges(parent):
				if str(edge.get("to",""))==id:continue
				var key:=_focus_endpoint(model,result,str(edge.get("to","")),"sibling",parent,edge)
				var parent_key:="focus:parent:"+parent.uri_encode()+":"+parent.uri_encode()
				# These are the actual parent→sibling slots, never fake sibling edges.
				result.edges.append({"from":parent_key,"to":key,"relation":edge.duplicate(true)})
	result["distinct_count"]={}
	for item:Dictionary in result.items.values():result.distinct_count[item.node_id]=true
	result["distinct_count"]=result.distinct_count.size()
	_apply_focus_positions(result,[])
	return result

static func _focus_endpoint(model,result:Dictionary,id:String,role:String,group:String,edge:Dictionary)->String:
	var resolved:bool=model.records.has(id) and not bool(model.records[id].get("external",false))
	var identity:=str(model.identity(id)) if resolved else JSON.stringify(edge.get("reference",{}))
	var key:="focus:"+role+":"+group.uri_encode()+":"+id.uri_encode() if resolved else "focus:"+role+":reference:"+identity.sha256_text()
	if result.items.has(key):return key
	var item:Dictionary
	if resolved:item=_item(model,id,key,"","",[],id,0,-1,false,edge)
	else:
		item={"occurrence":key,"node_id":"unresolved:"+identity.sha256_text(),"title":"Reference not loaded","external":true,"reference":edge.get("reference",{}).duplicate(true),"type":"Unresolved target","shape":"placeholder","icon":"?","status":"Not loaded","source_index":-1,"child_count":0,"unresolved_child_count":0,"root_id":"","depth":0,"branch":-1,"parent":"","path":[],"slot":""}
		result.unresolved_count+=1
	item["role"]=role;item["group"]=group
	result.items[key]=item;result.sizes[key]=CARD_SIZE
	return key

static func _position_bounds(positions: Dictionary) -> Rect2:
	var bounds := Rect2()
	var first: bool = true
	for position in positions.values():
		var rectangle := Rect2(position, CARD_SIZE)
		bounds = rectangle if first else bounds.merge(rectangle)
		first = false
	return bounds

static func _apply_focus_positions(result: Dictionary, _related: Array[String]) -> void:
	# Existing local focus layout, now role-constrained. World/source topology
	# is unchanged; all role occurrences get a readable card coordinate.
	var groups:Dictionary={"parent":[],"child":[],"related":[],"sibling":[]}
	for key in result.items:
		var role:=str(result.items[key].role)
		if role=="focus":result.positions[key]=Vector2.ZERO
		elif groups.has(role):groups[role].append(key)
	for role in ["parent","child"]:
		var keys:Array=groups[role];var columns:=mini(3,maxi(1,keys.size()));var rows:=ceili(float(keys.size())/columns)
		for i in keys.size():
			var y:=(-float(rows-i/columns)*140.0-15.0) if role=="parent" else (float(i/columns)*140.0+155.0)
			result.positions[keys[i]]=Vector2((float(i%columns)-float(columns-1)*0.5)*270.0,y)
	for role in ["sibling","related"]:
		var keys:Array=groups[role];var sign_value:float=-1 if role=="sibling" else 1
		var rows:Array=[];var row:Array=[];var previous_group:=""
		for key in keys:
			var group:=str(result.items[key].get("group",""))
			if not row.is_empty() and (group!=previous_group or row.size()==4):rows.append(row);row=[]
			row.append(key);previous_group=group
		if not row.is_empty():rows.append(row)
		for r in rows.size():
			for column in rows[r].size():
				result.positions[rows[r][column]]=Vector2(sign_value*(250+column*290),(float(r)-float(rows.size()-1)*0.5)*140)
	result.backend="focus-roles";result["groups"]=groups

static func _apply_map_positions(result: Dictionary, arrangement: String) -> void:
	if result.items.is_empty():
		return
	if arrangement != "balanced":
		_accept_coordinates(result, _dot_positions(result.items.keys(), result.edges))
		return
	var positions: Dictionary = {}
	var forest_y: float = 0.0
	for root_key in result.roots:
		var local: Dictionary = {root_key: Vector2.ZERO}
		for side in [1, -1]:
			var keys: Array = [root_key]
			for key in result.items:
				var item: Dictionary = result.items[key]
				if key == root_key or item.root_id != result.items[root_key].root_id:
					continue
				var item_side: int = 1 if int(item.branch) % 2 == 0 else -1
				if item_side == side:
					keys.append(key)
			if keys.size() < 2:
				continue
			var subset: Array = []
			for edge in result.edges:
				if edge.from in keys and edge.to in keys:
					subset.append(edge)
			var coordinates: Dictionary = _dot_positions(keys, subset)
			if coordinates.positions.is_empty():
				result.warning = coordinates.warning
				return
			var anchor: Vector2 = coordinates.positions[root_key]
			for key in coordinates.positions:
				var relative: Vector2 = coordinates.positions[key] - anchor
				local[key] = Vector2(relative.x * float(side), relative.y)
		var min_y: float = 0.0
		var max_y: float = CARD_SIZE.y
		for value in local.values():
			min_y = minf(min_y, value.y)
			max_y = maxf(max_y, value.y + CARD_SIZE.y)
		for key in local:
			positions[key] = local[key] + Vector2(0.0, forest_y - min_y)
		forest_y += max_y - min_y + 96.0
	result.positions = positions
	result.backend = "graphviz-dot" if result.items.size() > result.roots.size() else "trivial"

static func _accept_coordinates(result: Dictionary, coordinates: Dictionary) -> void:
	result.positions = coordinates.positions
	result.warning = coordinates.get("warning", "")
	if not result.positions.is_empty():
		result.backend = "graphviz-dot"

static func _dot_executable() -> String:
	return preload("res://addons/polar_graphics/graphviz.gd").executable()

static func _dot_positions(keys: Array, edges: Array, same_rank: Array = [], direction: String = "LR", rank_groups: Array = []) -> Dictionary:
	var result: Dictionary = {"positions": {}, "warning": ""}
	var executable: String = _dot_executable()
	if executable.is_empty():
		result.warning = "Graphviz dot unavailable; using Godot GraphEdit.arrange_nodes()"
		return result
	var aliases: Dictionary = {}
	var reverse: Dictionary = {}
	var rank_direction: String = "TB" if direction == "TB" else "LR"
	var lines: Array[String] = ["digraph Display {",
		"graph [rankdir=%s, ordering=out, nodesep=0.40, ranksep=0.95, splines=false, margin=0];" % rank_direction,
		"node [shape=box, fixedsize=true, label=\"\", width=%.8f, height=%.8f];" % [CARD_SIZE.x / POINTS_PER_INCH, CARD_SIZE.y / POINTS_PER_INCH]]
	for i in keys.size():
		var alias: String = "n%d" % i
		aliases[keys[i]] = alias
		reverse[alias] = keys[i]
		lines.append(alias + ";")
	for edge in edges:
		if not aliases.has(edge.from) or not aliases.has(edge.to):
			continue
		var constraint: String = "true" if bool(edge.relation.get("composition", false)) else "false"
		lines.append("%s -> %s [constraint=%s];" % [aliases[edge.from], aliases[edge.to], constraint])
	var groups: Array = rank_groups.duplicate()
	if same_rank.size() > 1:
		groups.append(same_rank)
	for group in groups:
		var rank_nodes: Array[String] = []
		for key in group:
			if aliases.has(key):
				rank_nodes.append(str(aliases[key]))
		lines.append("{ rank=same; " + "; ".join(rank_nodes) + "; }")
		for i in range(1, rank_nodes.size()):
			lines.append("%s -> %s [style=invis, constraint=false, weight=100];" % [rank_nodes[i - 1], rank_nodes[i]])
	lines.append("}")
	# Source names and labels never enter DOT. Only generated ordinal IDs and
	# known geometry are passed; no shell or user text is executed.
	var temporary: FileAccess = FileAccess.create_temp(FileAccess.WRITE, "polar-itree-layout", "dot")
	if temporary == null:
		result.warning = "Cannot create Graphviz layout input; using Godot-native layout"
		return result
	temporary.store_string("\n".join(lines))
	temporary.flush()
	var output: Array = []
	var exit_code: int = OS.execute(executable, PackedStringArray(["-Tplain", temporary.get_path_absolute()]), output, true)
	# The temporary FileAccess owns cleanup. It is never an I-tree source file.
	temporary = null
	if exit_code != 0:
		result.warning = "Graphviz exited %d; using Godot-native layout" % exit_code
		return result
	var height: float = 0.0
	for line in "\n".join(output).split("\n", false):
		var fields: PackedStringArray = line.split(" ", false)
		if fields.size() >= 4 and fields[0] == "graph":
			height = fields[3].to_float() * POINTS_PER_INCH
		elif fields.size() >= 6 and fields[0] == "node" and reverse.has(fields[1]):
			var center: Vector2 = Vector2(fields[2].to_float() * POINTS_PER_INCH, height - fields[3].to_float() * POINTS_PER_INCH)
			if center.is_finite():
				result.positions[reverse[fields[1]]] = center - CARD_SIZE * 0.5
	if result.positions.size() != keys.size():
		result.positions.clear()
		result.warning = "Incomplete Graphviz output; using Godot-native layout"
	return result
