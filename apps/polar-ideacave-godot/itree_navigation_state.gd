extends RefCounted

## Shared navigation state, not an I-tree mutation API or a rendering object.
## Passive selection is NOT a navigation request: it never changes cameras, scope,
## expansion, history, or the running layout generation. Views consume intents
## from this controller rather than dispatching each other's selection signals.
signal changed(intent: String)

const MapLayout = preload("res://itree_map_layout.gd")
const SNAPSHOT_VERSION := 1
const HISTORY_LIMIT := 100
const PATH_COUNT_LIMIT := 64

var model
var selected_id := ""
var selected_identity := ""
var selected_occurrence := ""
var roots: Array[String] = []
var expanded: Dictionary = {}
var budget := MapLayout.DEFAULT_BUDGET
var scope: Dictionary = {"mode": "all"}
var filter_kind := "all"
var camera_states: Dictionary = {}
var pinned_positions: Dictionary = {}
var snapshot_fingerprint := ""
var last_route: Dictionary = {}
var error := ""

var _last_paths: Dictionary = {}
var _root_nodes: Dictionary = {}
var _path_counts: Dictionary = {}
var _past: Array[Dictionary] = []
var _future: Array[Dictionary] = []
var _layout_generation := 0
var _layout_scope: Variant = null


func initialize(input_model) -> void:
	model = input_model
	selected_id = ""
	selected_identity = ""
	selected_occurrence = ""
	roots.clear()
	expanded.clear()
	budget = MapLayout.DEFAULT_BUDGET
	scope = {"mode": "all"}
	filter_kind = "all"
	camera_states.clear()
	pinned_positions.clear()
	_last_paths.clear()
	_root_nodes.clear()
	_path_counts.clear()
	_past.clear()
	_future.clear()
	last_route.clear()
	error = ""
	var identities: Array[String] = []
	for value in model.ordered_ids:
		var id := str(value)
		identities.append(str(model.identity(id)))
		if not bool(model.records[id].get("external", false)):
			_root_nodes[MapLayout.root_occurrence(model, id)] = id
	identities.sort()
	snapshot_fingerprint = JSON.stringify(identities).sha256_text()
	for value in model.source_roots:
		var id := str(value)
		if _is_loaded(id) and id not in roots:
			roots.append(id)
	if roots.is_empty() and _is_loaded(str(model.root_key)):
		roots.append(str(model.root_key))
	for id in roots:
		expanded[MapLayout.root_occurrence(model, id)] = true
	_count_paths()
	var initial := str(model.initial_selection)
	if not _is_loaded(initial):
		initial = roots[0] if not roots.is_empty() else ""
	if not initial.is_empty():
		_commit_selection(_choose_route(initial))
	_invalidate_layout()
	changed.emit("initialize")


func _is_loaded(id: String) -> bool:
	return model != null and model.records.has(id) and not bool(model.records[id].get("external", false))


func _eligible(edge: Dictionary) -> bool:
	return bool(edge.get("resolved", false)) and _is_loaded(str(edge.get("to", "")))


func _failure(message: String) -> Dictionary:
	error = message
	return {"error": message, "node_id": "", "identity": "", "occurrence": ""}


func _route(root_id: String, path: Array, choice: String = "") -> Dictionary:
	if not _is_loaded(root_id) or path.size() > model.MAX_COMPOSITION_DEPTH:
		return {"error": "Unknown root or occurrence path beyond the preview depth limit"}
	var id := str(MapLayout._node_at_path(model, root_id, path))
	if not _is_loaded(id):
		return {"error": "Occurrence path contains an absent or unresolved composition slot"}
	var count := int(_path_counts.get(id, 0))
	return {
		"error": "", "node_id": id, "identity": str(model.identity(id)),
		"root_id": root_id, "root_identity": str(model.identity(root_id)),
		"path": path.duplicate(), "occurrence": MapLayout._key_at_path(model, root_id, path),
		"choice": choice, "ambiguous": count > 1, "candidate_count": maxi(1, count),
		"candidate_count_capped": count >= PATH_COUNT_LIMIT,
		"detached": count == 0
	}


func route_for_occurrence(key: String) -> Dictionary:
	var pieces := key.split("/", true)
	if pieces.is_empty() or not _root_nodes.has(pieces[0]):
		return {"error": "Occurrence belongs to an unknown exact source/root"}
	var path: Array = []
	for i in range(1, pieces.size()):
		path.append(pieces[i].uri_decode())
	var route := _route(str(_root_nodes[pieces[0]]), path)
	if str(route.get("error", "")).is_empty() and str(route.occurrence) != key:
		return {"error": "Occurrence path does not use canonical slot encoding"}
	return route


func _route_from_item(key: String, layout: Dictionary) -> Dictionary:
	var items: Dictionary = layout.get("items", {})
	if not items.has(key) or typeof(items[key]) != TYPE_DICTIONARY:
		return {"error": "Occurrence is not present in the supplied view"}
	var route := route_for_occurrence(key)
	if str(route.get("error", "")).is_empty() and str(route.node_id) != str(items[key].get("node_id", "")):
		return {"error": "Displayed occurrence does not match its exact source node"}
	return route


func _choose_route(id: String, preferred_occurrence: String = "") -> Dictionary:
	if not _is_loaded(id):
		return {"error": "Node is not a loaded exact source version: " + id}
	# Explicit hints are never trusted to change the requested node identity.
	for key in [preferred_occurrence, selected_occurrence]:
		if str(key).is_empty():
			continue
		var candidate := route_for_occurrence(str(key))
		if str(candidate.get("error", "")).is_empty() and str(candidate.node_id) == id:
			candidate.choice = "preferred" if key == preferred_occurrence else "current"
			return candidate
	var identity := str(model.identity(id))
	if _last_paths.has(identity):
		var remembered: Dictionary = _last_paths[identity]
		var root_id := str(model.resolve(str(remembered.get("root_identity", ""))))
		var candidate := _route(root_id, remembered.get("path", []), "remembered")
		if str(candidate.get("error", "")).is_empty() and str(candidate.node_id) == id:
			return candidate
	# Use the existing bounded shortest-path helper. Prefer the node's exact
	# source root, never its logical UUID. The result is a display route, NOT
	# a newly asserted canonical data parent; ambiguity stays in the descriptor.
	var ordered_roots: Array[String] = []
	for value in model.source_roots:
		if _is_loaded(str(value)):
			ordered_roots.append(str(value))
	ordered_roots.sort_custom(func(a: String, b: String) -> bool: return str(model.identity(a)) < str(model.identity(b)))
	var preferred_root := "" if ordered_roots.is_empty() else ordered_roots[0]
	for root_id in ordered_roots:
		if str(model.records[root_id].source) == str(model.records[id].source):
			preferred_root = root_id
			break
	var fallback: Dictionary = MapLayout.path_to(model, id, preferred_root)
	if str(fallback.get("error", "")).is_empty() and not bool(fallback.get("detached", false)):
		return _route(str(fallback.root), fallback.path, "deterministic")
	return _route(id, [], "detached")


func _commit_selection(route: Dictionary) -> void:
	selected_id = str(route.node_id)
	selected_identity = str(route.identity)
	selected_occurrence = str(route.occurrence)
	last_route = route.duplicate(true)
	_last_paths[selected_identity] = {
		"root_identity": route.root_identity, "path": route.path.duplicate()
	}
	error = ""


func select_occurrence(key: String, layout: Dictionary) -> bool:
	var route := _route_from_item(key, layout)
	if not str(route.get("error", "")).is_empty():
		error = str(route.error)
		return false
	route.choice = "exact-occurrence"
	_commit_selection(route)
	changed.emit("selection")
	return true


func select_node(id: String) -> Dictionary:
	var route := _choose_route(id)
	if not str(route.get("error", "")).is_empty():
		return _failure(str(route.error))
	_commit_selection(route)
	changed.emit("selection")
	return route.duplicate(true)


# Explicit browsing intents reuse the exact-occurrence expansion/history model.
# Passive select_node/select_occurrence remain available to linked views.
func _children(id: String) -> Array:
	return model.composition_edges(id).filter(func(edge): return _eligible(edge))


func _parent_routes(id: String) -> Array[Dictionary]:
	var routes: Array[Dictionary] = []
	for parent in model.parents.get(id, []):
		var route := _choose_route(str(parent))
		if str(route.get("error", "")).is_empty(): routes.append(route)
	return routes


func neighbor_state(key: String, direction: String, layout: Dictionary) -> Dictionary:
	var route := route_for_occurrence(key)
	if not str(route.get("error", "")).is_empty(): return {"count": 0, "state": "none"}
	var total := 0
	var shown := 0
	if direction == "parents":
		for parent in _parent_routes(str(route.node_id)):
			total += 1
			if layout.get("items", {}).has(parent.occurrence) and bool(expanded.get(parent.occurrence, false)): shown += 1
	else:
		var children := _children(str(route.node_id))
		total = model.composition_edges(str(route.node_id)).size()
		for edge in children:
			if layout.get("items", {}).has(MapLayout.child_occurrence(key, edge)): shown += 1
	var state := "none" if total == 0 else ("collapsed" if shown == 0 else ("full" if shown == total else "partial"))
	return {"count": total, "shown": shown, "state": state}


func _include_route(route: Dictionary) -> void:
	if str(scope.get("mode", "all")) == "branch":
		var key := str(scope.get("occurrence", ""))
		if str(route.occurrence) != key and not str(route.occurrence).begins_with(key + "/"):
			scope = {"mode": "all"}


func _expand_neighbors(route: Dictionary, parents: bool, children: bool) -> void:
	_include_route(route)
	_open_route(route)
	if children: expanded[route.occurrence] = true
	if parents:
		for parent in _parent_routes(str(route.node_id)):
			_include_route(parent)
			_open_route(parent)
			expanded[parent.occurrence] = true
	# The explicit complete layer takes precedence over the default budget.
	# Existing MAX_BUDGET remains the safety boundary, with partial state shown.
	budget = mini(MapLayout.MAX_BUDGET, maxi(budget, expanded.size() + model.ordered_ids.size()))


func _expansion_state() -> Array:
	return [roots.duplicate(), expanded.duplicate(), scope.duplicate(true), budget]


func _finish_browse(route: Dictionary, before: Array, layout: Dictionary = {}) -> void:
	_commit_selection(route)
	var changed_layout: bool = before != _expansion_state() or (not layout.is_empty() and not layout.get("items", {}).has(selected_occurrence))
	if changed_layout: _invalidate_layout()
	changed.emit("browse" if changed_layout else "browse_selection")


func browse_occurrence(key: String, layout: Dictionary) -> bool:
	var route := _route_from_item(key, layout)
	if not str(route.get("error", "")).is_empty(): return false
	_push_history()
	var before := _expansion_state()
	_expand_neighbors(route, true, true)
	_finish_browse(route, before, layout)
	return true


func toggle_neighbors(key: String, direction: String, layout: Dictionary) -> bool:
	var route := _route_from_item(key, layout)
	if not str(route.get("error", "")).is_empty(): return false
	var state := neighbor_state(key, direction, layout)
	if int(state.count) == 0: return true
	_push_history()
	var before := _expansion_state()
	if direction == "parents":
		if str(state.state) == "full":
			for parent in _parent_routes(str(route.node_id)): expanded[parent.occurrence] = false
			# Retain the selected data node as a detached display occurrence;
			# collapsing an incoming path must never delete or hide its content.
			var id := str(route.node_id)
			if id not in roots: roots.append(id)
			route = _route(id, [], "parents-collapsed")
			_include_route(route)
		else:
			_expand_neighbors(route, true, false)
	else:
		if bool(expanded.get(key, false)) and int(state.shown) == _children(str(route.node_id)).size(): expanded[key] = false
		else: _expand_neighbors(route, false, true)
	_finish_browse(route, before, layout)
	return true


func navigate_visible(direction:String,peers:Array=[],parents:Array=[],children:Array=[])->bool:
	var occurrence:String=selected_occurrence
	var layout:Dictionary={"items":{occurrence:{"node_id":selected_id}}}
	if direction=="toggle":return toggle_occurrence(occurrence,layout)
	var target:=occurrence
	if direction=="left" and not parents.is_empty():
		var route:Dictionary=_choose_route(str(parents[0]))
		if not str(route.get("error","")).is_empty():return false
		target=str(route.occurrence)
	elif direction in ["up","down"] and not peers.is_empty():
		var at:=peers.find(selected_id)
		var next:=at+(-1 if direction=="up" else 1)
		if at<0 or next<0 or next>=peers.size():return false
		var route:Dictionary=_choose_route(str(peers[next]))
		if not str(route.get("error","")).is_empty():return false
		target=str(route.occurrence)
	elif direction=="right" and not children.is_empty():
		expand_occurrence(occurrence,layout)
		target=MapLayout.child_occurrence(occurrence,children[0])
	else:return navigate(direction,layout)
	if target==occurrence:return false
	var route:Dictionary=route_for_occurrence(target)
	if not str(route.get("error","")).is_empty():return false
	return select_occurrence(target,{"items":{target:{"node_id":str(route.node_id)}}})

func navigate(direction: String, layout: Dictionary = {}) -> bool:
	if selected_id.is_empty(): return false
	var route := route_for_occurrence(selected_occurrence)
	var target: Dictionary = {}
	if direction == "left":
		var parents := _parent_routes(selected_id)
		if parents.is_empty(): return false
		target = parents[0]
	elif direction == "right":
		var children := _children(selected_id)
		if children.is_empty(): return false
		target = route_for_occurrence(MapLayout.child_occurrence(selected_occurrence, children[0]))
	else:
		var parent_key := selected_occurrence.get_slice("/", 0)
		var split := selected_occurrence.rfind("/")
		var siblings: Array[Dictionary] = []
		if split > 0:
			parent_key = selected_occurrence.left(split)
			var parent := route_for_occurrence(parent_key)
			for edge in _children(str(parent.node_id)):
				siblings.append(route_for_occurrence(MapLayout.child_occurrence(parent_key, edge)))
		else:
			var parents := _parent_routes(selected_id)
			if not parents.is_empty():
				parent_key = str(parents[0].occurrence)
				for edge in _children(str(parents[0].node_id)):
					siblings.append(route_for_occurrence(MapLayout.child_occurrence(parent_key, edge)))
			else:
				for id in roots: siblings.append(_route(id, []))
		var index := -1
		for i in siblings.size():
			if str(siblings[i].occurrence) == selected_occurrence or (index < 0 and str(siblings[i].node_id) == selected_id): index = i
		var next := index + (-1 if direction == "up" else 1)
		if index < 0 or next < 0 or next >= siblings.size(): return false
		target = siblings[next]
	if not str(target.get("error", "")).is_empty(): return false
	_push_history()
	var before := _expansion_state()
	if direction == "right": _expand_neighbors(route, false, true)
	_include_route(target)
	_open_route(target)
	_finish_browse(target, before, layout)
	return true


func locate_node(id: String, preferred_occurrence: String = "") -> Dictionary:
	var route := _choose_route(id, preferred_occurrence)
	if not str(route.get("error", "")).is_empty():
		return _failure(str(route.error))
	_push_history()
	_open_route(route)
	_commit_selection(route)
	scope = {"mode": "all"}
	_invalidate_layout()
	changed.emit("locate")
	return route.duplicate(true)


func _open_route(route: Dictionary) -> void:
	var root_id := str(route.root_id)
	if root_id not in roots:
		roots.append(root_id)
	var key := str(MapLayout.root_occurrence(model, root_id))
	for slot in route.path:
		expanded[key] = true
		key = MapLayout.child_occurrence(key, {"slot": str(slot)})
	budget = clampi(maxi(budget, route.path.size() + 1), 1, MapLayout.MAX_BUDGET)


func expand_occurrence(key: String, layout: Dictionary) -> bool:
	return _set_expanded(key, layout, true)


func collapse_occurrence(key: String, layout: Dictionary) -> bool:
	return _set_expanded(key, layout, false)


func toggle_occurrence(key: String, layout: Dictionary) -> bool:
	return _set_expanded(key, layout, not bool(expanded.get(key, false)))


func _set_expanded(key: String, layout: Dictionary, value: bool) -> bool:
	var route := _route_from_item(key, layout)
	if not str(route.get("error", "")).is_empty():
		error = str(route.error)
		return false
	if bool(expanded.get(key, false)) == value:
		return true
	_push_history()
	expanded[key] = value
	_invalidate_layout()
	changed.emit("expansion")
	return true


func focus_occurrence(key: String, layout: Dictionary) -> bool:
	var route := _route_from_item(key, layout)
	if not str(route.get("error", "")).is_empty():
		error = str(route.error)
		return false
	_push_history()
	_open_route(route)
	expanded[key] = true
	_commit_selection(route)
	# Preserve the original occurrence path when zooming into a branch. A view
	# may filter by this scope but must not manufacture a new identity/root.
	scope = {"mode": "branch", "identity": route.identity, "occurrence": key,
		"root_identity": route.root_identity, "path": route.path.duplicate()}
	_invalidate_layout()
	changed.emit("focus")
	return true


func set_camera(view: String, data: Dictionary) -> void:
	if view.is_empty() or camera_states.get(view, {}) == data:
		return
	camera_states[view] = data.duplicate(true)
	# Camera intents never invalidate simulation or add navigation history.
	changed.emit("camera:" + view)


func set_pinned_positions(view: String, positions: Dictionary) -> void:
	if view.is_empty():
		return
	# Keys are exact identities (graph) or occurrence keys (tree/canvas), not
	# transient gNN projection indexes. Position values are owned by each view.
	pinned_positions[view] = positions.duplicate(true)
	# Canvas constraints do not change the independent definition-force graph.
	if view == "graph": _invalidate_layout()
	changed.emit("pins:" + view)


func set_filter(kind: String) -> void:
	if kind == filter_kind:
		return
	_push_history()
	filter_kind = kind
	_invalidate_layout()
	changed.emit("filter")


func snapshot() -> Dictionary:
	var root_identities: Array[String] = []
	for id in roots:
		root_identities.append(str(model.identity(id)))
	return {"version": SNAPSHOT_VERSION, "snapshot_fingerprint": snapshot_fingerprint,
		"selected_identity": selected_identity, "selected_occurrence": selected_occurrence,
		"root_identities": root_identities, "expanded": expanded.duplicate(true),
		"budget": budget, "scope": scope.duplicate(true), "filter_kind": filter_kind,
		"camera_states": camera_states.duplicate(true), "pinned_positions": pinned_positions.duplicate(true),
		"last_paths": _last_paths.duplicate(true)}


func restore(state: Dictionary) -> bool:
	if not _restore(state):
		return false
	# External restore establishes a fresh history baseline. travel() uses the
	# private method so that normal back/forward keeps its two stacks intact.
	_past.clear()
	_future.clear()
	changed.emit("restore")
	return true


func _restore(state: Dictionary) -> bool:
	if int(state.get("version", -1)) != SNAPSHOT_VERSION or str(state.get("snapshot_fingerprint", "")) != snapshot_fingerprint:
		error = "Navigation snapshot belongs to another exact source snapshot"
		return false
	for field in ["expanded", "scope", "camera_states", "pinned_positions", "last_paths"]:
		if typeof(state.get(field)) != TYPE_DICTIONARY:
			error = "Invalid navigation snapshot field: " + field
			return false
	if typeof(state.get("root_identities")) != TYPE_ARRAY:
		error = "Navigation snapshot roots must be exact source identities"
		return false
	var restored_roots: Array[String] = []
	for identity in state.root_identities:
		var id := str(model.resolve(str(identity)))
		if not _is_loaded(id):
			error = "Navigation snapshot root is not loaded"
			return false
		if id not in restored_roots:
			restored_roots.append(id)
	var route := route_for_occurrence(str(state.get("selected_occurrence", "")))
	if not str(route.get("error", "")).is_empty() or str(route.get("identity", "")) != str(state.get("selected_identity", "")):
		error = "Navigation snapshot selection does not match its exact occurrence"
		return false
	for key in state.expanded:
		if typeof(state.expanded[key]) != TYPE_BOOL or not str(route_for_occurrence(str(key)).get("error", "")).is_empty():
			error = "Navigation snapshot contains an invalid expanded occurrence"
			return false
	var restored_scope: Dictionary = state.scope
	if str(restored_scope.get("mode", "")) == "branch":
		var focus_route := route_for_occurrence(str(restored_scope.get("occurrence", "")))
		var scope_matches := str(focus_route.get("error", "")).is_empty()
		scope_matches = scope_matches and str(focus_route.get("identity", "")) == str(restored_scope.get("identity", ""))
		scope_matches = scope_matches and str(focus_route.get("root_identity", "")) == str(restored_scope.get("root_identity", ""))
		scope_matches = scope_matches and focus_route.get("path", []) == restored_scope.get("path", [])
		if not scope_matches:
			error = "Navigation snapshot contains an invalid branch scope"
			return false
	elif str(restored_scope.get("mode", "")) != "all":
		error = "Navigation snapshot contains an unknown scope mode"
		return false
	for identity in state.last_paths:
		var remembered = state.last_paths[identity]
		if typeof(remembered) != TYPE_DICTIONARY or typeof(remembered.get("path")) != TYPE_ARRAY:
			error = "Navigation snapshot contains an invalid remembered path"
			return false
		var remembered_root := str(model.resolve(str(remembered.get("root_identity", ""))))
		var remembered_route := _route(remembered_root, remembered.path)
		if not str(remembered_route.get("error", "")).is_empty() or str(remembered_route.get("identity", "")) != str(identity):
			error = "Navigation snapshot remembered path resolves to another source node"
			return false
	# All validation precedes assignment: a failed restore cannot partially
	# replace cameras or open branches from another source version.
	roots = restored_roots
	expanded = state.expanded.duplicate(true)
	budget = clampi(int(state.get("budget", MapLayout.DEFAULT_BUDGET)), 1, MapLayout.MAX_BUDGET)
	scope = restored_scope.duplicate(true)
	filter_kind = str(state.get("filter_kind", "all"))
	camera_states = state.camera_states.duplicate(true)
	pinned_positions = state.pinned_positions.duplicate(true)
	_last_paths = state.last_paths.duplicate(true)
	route.choice = "restored"
	_commit_selection(route)
	_invalidate_layout()
	return true


func _push_history() -> void:
	var current := snapshot()
	if _past.is_empty() or _past.back() != current:
		_past.append(current)
		if _past.size() > HISTORY_LIMIT:
			_past.pop_front()
	_future.clear()


func can_travel(ahead: bool) -> bool:
	return not (_future if ahead else _past).is_empty()


func travel(ahead: bool) -> bool:
	var source: Array[Dictionary] = _future if ahead else _past
	var target: Array[Dictionary] = _past if ahead else _future
	if source.is_empty():
		return false
	var destination: Dictionary = source.back()
	var current := snapshot()
	if not _restore(destination):
		return false
	source.pop_back()
	target.append(current)
	changed.emit("history")
	return true


func _invalidate_layout() -> void:
	_layout_generation += 1
	_layout_scope = null


func begin_layout(scope_value: Variant) -> int:
	_layout_generation += 1
	_layout_scope = scope_value.duplicate(true) if scope_value is Dictionary or scope_value is Array else scope_value
	return _layout_generation


func accept_layout(generation: int, fingerprint: String) -> bool:
	return generation == _layout_generation and fingerprint == snapshot_fingerprint


func _count_paths() -> void:
	# Count composition occurrences with a saturated DAG dynamic program, not
	# exponential path enumeration. Parallel slots count separately. Semantic
	# cycles and unresolved refs are intentionally excluded.
	var indegree: Dictionary = {}
	for value in model.ordered_ids:
		var id := str(value)
		if _is_loaded(id):
			indegree[id] = 0
			_path_counts[id] = 0
	for id in indegree:
		for edge in model.composition_edges(str(id)):
			if _eligible(edge):
				indegree[str(edge.to)] += 1
	for value in model.source_roots:
		var id := str(value)
		if indegree.has(id):
			_path_counts[id] = mini(PATH_COUNT_LIMIT, int(_path_counts[id]) + 1)
	var queue: Array[String] = []
	for id in indegree:
		if int(indegree[id]) == 0:
			queue.append(str(id))
	var cursor := 0
	while cursor < queue.size():
		var id := queue[cursor]
		cursor += 1
		for edge in model.composition_edges(id):
			if not _eligible(edge):
				continue
			var target := str(edge.to)
			_path_counts[target] = mini(PATH_COUNT_LIMIT, int(_path_counts[target]) + int(_path_counts[id]))
			indegree[target] -= 1
			if int(indegree[target]) == 0:
				queue.append(target)


## Explicit presentation migration after a validated local-file revision change.
## Does not permit restore() to accept snapshots from a different version.
func migrate_from(previous) -> Dictionary:
	var by_identity: Dictionary = {}
	for id in model.ordered_ids:
		var record: Dictionary = model.records[id]
		var identity := _logical_source_identity(record)
		if not by_identity.has(identity): by_identity[identity] = []
		by_identity[identity].append(id)
	var ids: Dictionary = {}
	var prefixes: Dictionary = {}
	for old_id in previous.model.ordered_ids:
		var matches: Array = by_identity.get(_logical_source_identity(previous.model.records[old_id]), [])
		if matches.size() != 1: continue
		ids[old_id] = str(matches[0])
		prefixes[MapLayout.root_occurrence(previous.model, old_id)] = MapLayout.root_occurrence(model, str(matches[0]))
	var migrated: Dictionary = {}
	for old_key in previous.expanded:
		var key := _migrate_occurrence(str(old_key), prefixes)
		if not key.is_empty() and str(route_for_occurrence(key).get("error", "")).is_empty():
			migrated[key] = previous.expanded[old_key]
	for key in migrated: expanded[key] = migrated[key]
	if ids.has(previous.selected_id):
		var key := _migrate_occurrence(previous.selected_occurrence, prefixes)
		var route := route_for_occurrence(key)
		if str(route.get("error", "")).is_empty() and str(route.node_id) == ids[previous.selected_id]:
			_commit_selection(route)
		else: select_node(ids[previous.selected_id])
	budget = previous.budget
	filter_kind = previous.filter_kind
	camera_states = previous.camera_states.duplicate(true)
	# Deleted selections retain initialize()'s valid default; history stays reset.
	_invalidate_layout()
	return ids

static func _logical_source_identity(record: Dictionary) -> String:
	var uuid := str(record.get("uuid", ""))
	return JSON.stringify([record.get("source", ""), "uuid" if not uuid.is_empty() else "key", uuid if not uuid.is_empty() else str(record.get("key", ""))])

static func _migrate_occurrence(value: String, prefixes: Dictionary) -> String:
	var parts := value.split("/", false, 1)
	if parts.is_empty() or not prefixes.has(parts[0]): return ""
	return str(prefixes[parts[0]]) + ("/" + parts[1] if parts.size() > 1 else "")
