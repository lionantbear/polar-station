extends RefCounted

## Read-only preview, not polar-ideacave validation, CAS admission, or runtime
## reference resolution. These checks prevent unsafe preview traversal only.
const MAX_VIEW_NODES := 4096
const MAX_RELATIONS := 65536
const MAX_COMPOSITION_DEPTH := 64
const MAX_BYTES := 8 * 1024 * 1024
const MAX_PROJECTION_SOURCE_BYTES := 32 * 1024 * 1024
const I_TREE_SCHEMA := "polar.ideacave.itree.v1"
var records: Dictionary = {}
var ordered_ids: Array[String] = []
var children: Dictionary = {}
var parents: Dictionary = {}
var neighbors: Dictionary = {}
var relation_edges: Array[Dictionary] = []
var source_roots: Array[String] = []
var root_key := ""
var initial_selection := ""
var source_label := ""
var is_projection := false
var projection_actual_nodes := 0
var error := ""
var compatibility_warnings: Array[String] = []
var _composition_by_node: Dictionary = {}
var _source_nodes: Dictionary = {}
var _multi_file_preview := false
# Godot's ResourceCache holds weak entries; models own the strong leases.
static var _snapshot_lock := Mutex.new()
static var _graph_lock := Mutex.new()
var _snapshot_leases: Array[Resource] = []
var _shared_graph: Resource
const SHARED_FIELDS = ["records", "ordered_ids", "children", "parents", "neighbors", "relation_edges", "source_roots", "root_key", "initial_selection", "source_label", "is_projection", "projection_actual_nodes", "compatibility_warnings", "_composition_by_node", "_source_nodes", "_multi_file_preview", "_snapshot_leases"]

static func _freeze(value: Variant) -> void:
	if value is Dictionary:
		for child in value.values():_freeze(child)
		value.make_read_only()
	elif value is Array:
		for child in value:_freeze(child)
		value.make_read_only()


func load_input() -> bool:
	if not OS.get_environment("POLAR_LAB_GRAPH").is_empty():
		return _load_projection(OS.get_environment("POLAR_LAB_GRAPH"))
	var path := ""
	for arg in OS.get_cmdline_user_args():
		if arg.begins_with("--itree="):
			path = arg.substr(8)
	if path.is_empty():
		error = "Open an Idea Tree file to continue."
		return false
	return _load_itree(path)

func _reset_state() -> void:
	_shared_graph = null
	_snapshot_leases = []
	records = {}
	ordered_ids = []
	children = {}
	parents = {}
	neighbors = {}
	relation_edges = []
	source_roots = []
	_composition_by_node = {}
	_source_nodes = {}
	compatibility_warnings = []
	root_key = ""
	initial_selection = ""
	source_label = ""
	is_projection = false
	_multi_file_preview = false
	projection_actual_nodes = 0
	error = ""

func _load_error(message: String) -> bool:
	# Never expose half of a failed load to the browser.
	_reset_state()
	error = message
	push_error(message)
	return false

func identity(id: String) -> String:
	if not records.has(id):
		return ""
	var record: Dictionary = records[id]
	# UUID is logical identity, not an exact source version. JSON quoting also
	# prevents delimiter collisions between a source path and local node key.
	return "preview:" + JSON.stringify([record.source, record.source_sha256, record.key])

func resolve(identity_value: String) -> String:
	if not identity_value.begins_with("preview:"):
		return ""
	for id in ordered_ids:
		if identity(id) == identity_value:
			return id
	return ""

func reference_descriptor(id: String) -> Dictionary:
	if not records.has(id):
		return {}
	var record: Dictionary = records[id]
	var descriptor := {
		"kind": "local-document-preview", "source": record.source,
		"source_sha256": record.source_sha256, "node": record.key,
		"admitted": false, "storage_status": "local-document"
	}
	if not str(record.uuid).is_empty():
		descriptor["expected_uuid"] = record.uuid
	if bool(record.get("external", false)):
		descriptor["kind"] = "unresolved-preview"
		descriptor["storage_status"] = "not-loaded"
		descriptor["reference"] = record.get("reference", {}).duplicate(true)
		descriptor["reason"] = record.get("reason", "Reference has not been loaded")
	return descriptor

func composition_edges(id: String) -> Array:
	# Include unloaded refs; callers expand only resolved, loaded destinations.
	return _composition_by_node.get(id, []).duplicate()

static func content(properties: Dictionary) -> String:
	var parts: Array[String] = []
	for key in ["statement", "text", "summary", "description", "goal"]:
		if properties.has(key) and not str(properties[key]).is_empty() and str(properties[key]) not in parts:
			parts.append(str(properties[key]))
	return "\n\n".join(parts)

func _read_snapshot(path: String, byte_budget: int = -1) -> Dictionary:
	var file := FileAccess.open(path, FileAccess.READ)
	if file == null:
		_load_error("Cannot open I-tree preview input: " + path)
		return {}
	# Capture length once: a later grow must not turn the bounded read into
	# an unbounded second get_length()/get_buffer() allocation.
	var length := file.get_length()
	var limit := length if byte_budget < 0 else byte_budget
	if length > limit:
		_load_error("I-tree preview input exceeds its byte budget (%d bytes): %s" % [limit, path])
		return {}
	var bytes := file.get_buffer(length)
	if bytes.size() != length:
		_load_error("I-tree preview input changed or could not be read completely: " + path)
		return {}
	var digest := HashingContext.new()
	digest.start(HashingContext.HASH_SHA256)
	digest.update(bytes)
	var fingerprint := digest.finish().hex_encode()
	var key := "res://.polar-preview/source/" + fingerprint
	_snapshot_lock.lock()
	var resource: Resource = ResourceLoader.get_cached_ref(key)
	if resource == null:
		var parsed = JSON.parse_string(bytes.get_string_from_utf8())
		if typeof(parsed) != TYPE_DICTIONARY:
			_snapshot_lock.unlock()
			_load_error("Invalid source JSON: " + path)
			return {}
		var data := {"document":parsed,"sha256":fingerprint,"size_bytes":bytes.size()}
		_freeze(data)
		resource = Resource.new();resource.set_meta("snapshot",data);resource.take_over_path(key)
	_snapshot_lock.unlock()
	_snapshot_leases.append(resource)
	return resource.get_meta("snapshot")

func _validate_document(document: Dictionary, source: String, legacy_defaults: bool = false) -> bool:
	if document.get("schema", "") != I_TREE_SCHEMA:
		return _load_error("Expected an actual " + I_TREE_SCHEMA + " document: " + source)
	var nodes = document.get("nodes", {})
	var root := str(document.get("root", ""))
	if typeof(nodes) != TYPE_DICTIONARY or nodes.is_empty() or not nodes.has(root):
		return _load_error("I-tree nodes/root missing: " + source)
	if typeof(nodes[root]) != TYPE_DICTIONARY or typeof(nodes[root].get("uuid")) != TYPE_STRING or str(nodes[root].uuid).is_empty():
		return _load_error("I-tree root must have its persistent UUID: " + source)
	for key in nodes:
		var node = nodes[key]
		if typeof(node) != TYPE_DICTIONARY or typeof(node.get("type")) != TYPE_STRING:
			return _load_error("Invalid I-tree node structure: " + str(key))
		if node.has("uuid") and typeof(node.uuid) != TYPE_STRING:
			return _load_error("Invalid I-tree UUID value on " + str(key))
		for field in ["properties", "relations"]:
			if not node.has(field) and legacy_defaults:
				var warning := "Legacy missing empty maps are displayed as empty; source is not runtime-schema-validated."
				if warning not in compatibility_warnings:
					compatibility_warnings.append(warning)
			elif typeof(node.get(field)) != TYPE_DICTIONARY:
				return _load_error("Invalid I-tree " + field + " on " + str(key))
		for slot in node.get("relations", {}):
			var relation = node.relations[slot]
			if typeof(relation) != TYPE_DICTIONARY or typeof(relation.get("target")) != TYPE_DICTIONARY or typeof(relation.get("relation")) != TYPE_STRING:
				return _load_error("Invalid relation on " + str(key) + ":" + str(slot))
			if typeof(relation.get("properties", {} if legacy_defaults else null)) != TYPE_DICTIONARY:
				return _load_error("Invalid relation properties on " + str(key) + ":" + str(slot))
			var target: Dictionary = relation.target
			var kind := str(target.get("kind", ""))
			if kind not in ["local", "object", "tree", "content"]:
				return _load_error("Unknown reference kind on " + str(key) + ":" + str(slot))
			if kind == "local" and (typeof(target.get("node")) != TYPE_STRING or not nodes.has(target.node)):
				return _load_error("Missing local I-tree target on " + str(key) + ":" + str(slot))
	return true

func _add_record(id: String, key: String, source: String, fingerprint: String, original: Dictionary, is_root: bool, byte_count:int=-1) -> void:
	var properties: Dictionary = original.get("properties", {})
	var name_text := str(properties.get("name", properties.get("title", properties.get("text", key))))
	if name_text.strip_edges().is_empty(): name_text = key
	records[id] = {
		"id": id, "key": key, "source": source, "source_sha256": fingerprint, "source_size_bytes":byte_count,
		"parent": null, "title": name_text, "subtitle": "NODE · " + key,
		"type": str(original.type), "uuid": original.get("uuid", ""),
		"description": content(properties), "properties": properties,
		"relations": original.get("relations", {}), "external": false,
		"width": 5.5, "height": 2.05, "initial": is_root, "position": [0.0, 0.0, 0.0]
	}
	ordered_ids.append(id)
	children[id] = []
	parents[id] = []
	neighbors[id] = []
	_composition_by_node[id] = []
	if not _source_nodes.has(source):
		_source_nodes[source] = {}
	_source_nodes[source][key] = id
	if is_root:
		source_roots.append(id)

func _load_itree(path: String) -> bool:
	_reset_state()
	var snapshot := _read_snapshot(path)
	if snapshot.is_empty() or not _validate_document(snapshot.document, path):
		return false
	var document: Dictionary = snapshot.document
	var native_root := str(document.root)
	var keys: Array = document.nodes.keys()
	keys.sort()
	keys.erase(native_root)
	keys.push_front(native_root)
	for key in keys:
		_add_record(str(key), str(key), path, snapshot.sha256, document.nodes[key], str(key) == native_root, int(snapshot.size_bytes))
	root_key = native_root
	initial_selection = root_key
	source_label = path
	is_projection = true
	projection_actual_nodes = ordered_ids.size()
	return _build_relations()

## Direct multi-file preview reuses the same record and relation projection as
## the legacy verified projection input; no intermediate export is required.
func load_files(paths: PackedStringArray) -> bool:
	_reset_state()
	var snapshots: Dictionary = {}
	var versions: Array = []
	for path in paths:
		var source := path.simplify_path()
		if snapshots.has(source):continue
		var snapshot := _read_snapshot(source)
		if snapshot.is_empty():return false
		snapshots[source] = snapshot
		versions.append([source,snapshot.sha256])
	var key := "res://.polar-preview/graph/" + JSON.stringify(versions).sha256_text()
	_graph_lock.lock()
	var resource: Resource = ResourceLoader.get_cached_ref(key)
	if resource != null:
		var data: Dictionary = resource.get_meta("graph")
		for field in SHARED_FIELDS:set(field,data[field])
		_shared_graph=resource
		_graph_lock.unlock()
		return true
	if not _load_files_uncached(paths,snapshots):
		_graph_lock.unlock();return false
	var data: Dictionary = {}
	for field in SHARED_FIELDS:data[field]=get(field)
	_freeze(data)
	resource=Resource.new();resource.set_meta("graph",data);resource.take_over_path(key)
	_shared_graph=resource
	_graph_lock.unlock()
	return true

func _load_files_uncached(paths: PackedStringArray, snapshots: Dictionary) -> bool:
	if paths.is_empty():
		return _load_error("Choose one or more Idea Tree files")
	var source_bytes := 0
	for path in paths:
		var source := path.simplify_path()
		if _source_nodes.has(source):
			continue
		var snapshot: Dictionary = snapshots[source]
		if not _validate_document(snapshot.document, source, true):
			return false
		source_bytes += int(snapshot.size_bytes)
		var keys: Array = snapshot.document.nodes.keys()
		keys.sort()
		keys.erase(snapshot.document.root)
		keys.push_front(snapshot.document.root)
		for key in keys:
			var id := "g%d" % ordered_ids.size()
			_add_record(id, str(key), source, snapshot.sha256, snapshot.document.nodes[key], key == snapshot.document.root, int(snapshot.size_bytes))
	root_key = source_roots[0]
	initial_selection = root_key
	is_projection = true
	_multi_file_preview = true
	projection_actual_nodes = ordered_ids.size()
	source_label = "%d files · %d nodes · Local preview" % [source_roots.size(), projection_actual_nodes]
	return _build_relations()

func _load_projection(path: String, source_byte_budget: int = MAX_PROJECTION_SOURCE_BYTES, source_node_budget: int = MAX_VIEW_NODES) -> bool:
	_reset_state()
	var input := _read_snapshot(path)
	if input.is_empty():
		return false
	var projection: Dictionary = input.document
	if projection.get("version", 0) != 2 or typeof(projection.get("nodes")) != TYPE_ARRAY or typeof(projection.get("sources")) != TYPE_ARRAY:
		return _load_error("Invalid real I-tree projection")
	var entries: Array = projection.nodes
	if entries.is_empty() or entries.size() > MAX_VIEW_NODES:
		return _load_error("Real I-tree projection exceeds the 4096-node preview limit")
	# Every source needs at least its own root entry. Reject impossible source
	# lists before opening their files, rather than only after retaining them.
	if projection.sources.is_empty() or projection.sources.size() > entries.size():
		return _load_error("Projection source count exceeds its available root entries")
	# Optional budgets let small regression fixtures exercise aggregate bounds;
	# callers may lower, but never raise, the production preview limits.
	var byte_limit := clampi(source_byte_budget, 0, MAX_PROJECTION_SOURCE_BYTES)
	var node_limit := clampi(source_node_budget, 0, MAX_VIEW_NODES)
	var source_bytes := 0
	var source_nodes := 0
	var source_root := OS.get_environment("POLAR_LAB_SOURCE_ROOT")
	var documents: Dictionary = {}
	for source in projection.sources:
		if typeof(source) != TYPE_DICTIONARY or typeof(source.get("path")) != TYPE_STRING or typeof(source.get("sha256")) != TYPE_STRING:
			return _load_error("Invalid projection source descriptor")
		var source_name := str(source.path)
		if documents.has(source_name):
			return _load_error("Duplicate projection source: " + source_name)
		var file_path := source_root.path_join(source_name)
		var snapshot := _read_snapshot(file_path, byte_limit - source_bytes)
		if snapshot.is_empty():
			return false
		if snapshot.sha256 != source.sha256:
			return _load_error("I-tree changed during loading; relaunch: " + file_path)
		if not _validate_document(snapshot.document, file_path, true):
			return false
		source_nodes += snapshot.document.nodes.size()
		if source_nodes > node_limit:
			return _load_error("Projection sources exceed the aggregate node budget (%d nodes)" % node_limit)
		source_bytes += int(snapshot.size_bytes)
		documents[source_name] = snapshot
	for entry in entries:
		if typeof(entry) != TYPE_DICTIONARY or not entry.has("id"):
			return _load_error("Invalid projection node entry")
		if typeof(entry.id) not in [TYPE_INT, TYPE_FLOAT] or not is_finite(float(entry.id)) or float(entry.id) < 0 or float(entry.id) != floor(float(entry.id)):
			return _load_error("Projection node ID must be a nonnegative integer")
		var id := "g%d" % int(entry.id)
		if records.has(id):
			return _load_error("Duplicate projection node ID: " + id)
		var key := str(entry.get("key", ""))
		var source := str(entry.get("source", ""))
		if bool(entry.get("external", false)):
			if _source_nodes.get(path, {}).has(key):
				return _load_error("Duplicate unresolved projection reference: " + key)
			_add_record(id, key, path, input.sha256, {"type": "external", "uuid": key, "properties": {"name": str(entry.get("title", "Unloaded reference"))}, "relations": {}}, false)
			records[id]["external"] = true
			records[id]["subtitle"] = "Unloaded reference"
			records[id]["reference"] = {"kind": "object", "uuid": key}
			records[id]["reason"] = "Logical object reference has no runtime-selected version."
		else:
			if not documents.has(source) or not documents[source].document.nodes.has(key):
				return _load_error("Projection node missing from its exact source: " + source + ":" + key)
			if _source_nodes.get(source, {}).has(key):
				return _load_error("Duplicate projected source node: " + source + ":" + key)
			var snapshot: Dictionary = documents[source]
			_add_record(id, key, source, snapshot.sha256, snapshot.document.nodes[key], key == snapshot.document.root, int(snapshot.size_bytes))
			projection_actual_nodes += 1
	if source_roots.size() != documents.size() or source_roots.is_empty():
		return _load_error("Projection source roots are incomplete")
	for source in documents:
		for key in documents[source].document.nodes:
			if not _source_nodes.get(source, {}).has(key):
				return _load_error("Projection omits original source nodes; rebuild the projection: " + str(source))
	root_key = source_roots[0]
	initial_selection = root_key
	is_projection = true
	source_label = "%d real I-tree files · %d nodes · %d unloaded placeholders" % [documents.size(), projection_actual_nodes, entries.size() - projection_actual_nodes]
	_multi_file_preview = true
	# Recover typed relations from original slots, not the lossy layout edge list.
	# A legacy projector's UUID candidate does not establish a selected version.
	if not _build_relations():
		return false
	print("MINDMAP_SOURCES files=%d actual=%d placeholders=%d relations=%d" % [documents.size(), projection_actual_nodes, entries.size() - projection_actual_nodes, relation_edges.size()])
	return true

func _build_relations() -> bool:
	var uuid_candidates: Dictionary = {}
	var placeholders: Dictionary = {}
	if _multi_file_preview:
		for node_id in ordered_ids:
			var uuid := str(records[node_id].uuid)
			if uuid.is_empty(): continue
			if records[node_id].external: placeholders[uuid] = node_id
			else:
				if not uuid_candidates.has(uuid): uuid_candidates[uuid] = []
				uuid_candidates[uuid].append(node_id)
	for id in ordered_ids:
		var record: Dictionary = records[id]
		var slots: Array = record.relations.keys()
		slots.sort()
		for slot in slots:
			var relation: Dictionary = record.relations[slot]
			var reference: Dictionary = relation.target.duplicate(true)
			var kind := str(reference.kind)
			var target := ""
			var reason := ""
			match kind:
				"local":
					target = str(_source_nodes.get(record.source, {}).get(reference.node, ""))
					if target.is_empty():
						reason = "Local target is not included in the loaded preview."
				"object":
					reason = "Logical object reference has no runtime-selected version."
					if _multi_file_preview:
						var candidates: Array = uuid_candidates.get(str(reference.uuid),[])
						if candidates.size() == 1:
							target = candidates[0]
							reason = "Unique fingerprint-verified local preview; not runtime version selection."
						elif candidates.is_empty(): target = str(placeholders.get(str(reference.uuid),""))
				"tree":
					reason = "Exact external I-tree document is not loaded; content and UUID assertions are preserved."
					if reference.get("format", "") == "etree":
						reason = "E-tree reference is not an I-tree composition child; open through its runtime reader."
					elif reference.get("format", "itree") == "itree":
						var digest := str(reference.get("content", {}).get("id", "")).trim_prefix("sha256:")
						var matches: Array[String] = []
						for source in _source_nodes:
							var candidate := str(_source_nodes[source].get(str(reference.get("node", "")), ""))
							if candidate.is_empty() or str(records[candidate].source_sha256) != digest: continue
							if reference.get("content",{}).has("size_bytes") and int(reference.content.size_bytes)!=int(records[candidate].get("source_size_bytes",-1)):continue
							if reference.has("uuid") and str(reference.uuid) != str(records[candidate].uuid): continue
							matches.append(candidate)
						if matches.size() == 1:
							target = matches[0]
							reason = "Exact fingerprint-verified local preview only; not CAS admission or runtime version selection."
				"content":
					reason = "Content attachment bytes are not loaded and are not I-tree children."
			var edge := {
				"from": id, "to": target, "slot": str(slot), "type": str(relation.relation),
				"target_kind": kind, "reference": reference,
				"properties": relation.get("properties", {}).duplicate(true),
				"composition": str(relation.relation) in ["polar.contains.v1", "polar.fs.entry.v1"],
				"resolved": not target.is_empty() and not records[target].external, "reason": reason
			}
			relation_edges.append(edge)
			if bool(edge.composition):
				_composition_by_node[id].append(edge)
				if bool(edge.resolved):
					if target not in children[id]:
						children[id].append(target)
					if id not in parents[target]:
						parents[target].append(id)
			if bool(edge.resolved) and target != id:
				if target not in neighbors[id]:
					neighbors[id].append(target)
				if id not in neighbors[target]:
					neighbors[target].append(id)
	for id in ordered_ids:
		_composition_by_node[id].sort_custom(_edge_less)
		# Retain the legacy endpoint API, ordered consistently with occurrences.
		children[id].clear()
		for edge in _composition_by_node[id]:
			if bool(edge.resolved) and edge.to not in children[id]:
				children[id].append(edge.to)
	return _check_composition_safety()

func _sequence(edge: Dictionary):
	var sequence = edge.properties.get("sequence")
	if sequence == null and records.has(str(edge.to)):
		sequence = records[str(edge.to)].properties.get("sequence")
	if typeof(sequence) not in [TYPE_INT, TYPE_FLOAT] or not is_finite(float(sequence)) or floor(float(sequence)) != float(sequence):
		return null
	return float(sequence)

func _edge_less(a: Dictionary, b: Dictionary) -> bool:
	var left = _sequence(a)
	var right = _sequence(b)
	# Tuple ordering remains transitive when only some siblings have sequence.
	if (left != null) != (right != null):
		return left != null
	if left != null and left != right:
		return left < right
	return str(a.slot) < str(b.slot)

func _check_composition_safety() -> bool:
	# Only resolved composition links matter. Semantic cycles are valid; parallel
	# relation slots are distinct edges even if their endpoints are equal.
	var incoming := {}
	var depths := {}
	var queue: Array[String] = []
	for id in ordered_ids:
		incoming[id] = 0
		depths[id] = 0
	for edge in relation_edges:
		if bool(edge.composition) and bool(edge.resolved):
			incoming[edge.to] += 1
	for id in ordered_ids:
		if incoming[id] == 0:
			queue.append(id)
	var cursor := 0
	while cursor < queue.size():
		var id: String = queue[cursor]
		cursor += 1
		for edge in _composition_by_node[id]:
			if not bool(edge.resolved):
				continue
			var target := str(edge.to)
			depths[target] = maxi(int(depths[target]), int(depths[id]) + 1)
			if int(depths[target]) > MAX_COMPOSITION_DEPTH:
				return _load_error("I-tree preview exceeds the 64-level composition depth limit")
			incoming[target] -= 1
			if incoming[target] == 0:
				queue.append(target)
	if cursor != ordered_ids.size():
		return _load_error("I-tree composition cycle detected; semantic backlinks remain permitted")
	return true


