extends SceneTree
var failures: Array[String] = []
func check(condition: bool, message: String) -> void:
	if not condition: failures.append(message)
func _initialize() -> void:
	call_deferred("run")
func run() -> void:
	var component = load("res://addons/polar_graphics/spatial_view.gd")
	if component == null:
		quit(2)
		return
	var first = component.new()
	var second = component.new()
	first.role = "canvas"
	second.role = "graph"
	root.add_child(first)
	root.add_child(second)
	await process_frame
	var items = {"album-a": {"title": "Example Album", "node_id": "album-a", "branch": 0}}
	first.set_tree({"items": items, "positions": {"album-a": Vector2.ZERO}, "sizes": {"album-a": Vector2(200,64)}, "routes": []})
	check(first.records.size() == 1, "generic card is visible without Idea Tree model")
	check(second.records.is_empty(), "views own independent records")
	var old_target: Vector3 = second.target
	first.target += Vector3(3, 2, 0)
	check(second.target == old_target, "views own independent cameras")
	var label = first.labels.get("album-a")
	check(label != null, "generic card label exists")
	if label != null:
		var text = label.text
		var width = label.width
		first.view_size *= 0.7
		first._update_camera()
		check(label.text == text and label.width == width, "zoom retains shaped text")
	var geometry = preload("res://addons/polar_graphics/graphviz.gd").route(
		["album-a", "album-b"], [{"from": "album-a", "to": "album-b"}],
		{"album-a": Vector2(200,64), "album-b": Vector2(200,64)})
	check(str(geometry.error).is_empty(), "generic album layout: " + str(geometry.error))
	check(geometry.positions.size() == 2 and geometry.routes.size() == 1, "generic Graphviz routing needs no Idea Tree metadata")
	first.queue_free()
	second.queue_free()
	await process_frame
	print("GRAPHICS_REUSE_RESULT ", JSON.stringify(failures))
	quit(0 if failures.is_empty() else 2)
