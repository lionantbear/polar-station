extends Control

var space_pressed := false
var motion_preferences: Dictionary = {}
var canvas_mode := "two_d"
var layer_progress := 0.0
var _layering_enabled := false
var _mode_tween: Tween
var _layout_tween:Tween
var _source_planes: Dictionary = {}
var active_layer:=0
var _tree_wheel_amount:=0.0
var _base_geometry_key:=""
var browse_origin_key:=""
var _trail_points:=PackedVector3Array()
var _trail_due_ms:=0
var _trail_phase:=0.0
const SOURCE_LAYER_GAP := 3.0

## Three rendering roles, one Godot-native geometry/input implementation.
## Layouts are immutable input. Opaque display objects only; no product identity or topology inference.
signal tree_step_requested(amount:float)
signal presentation_updated
signal node_dragged(key:String, position:Vector3, phase:String)
signal node_selected(node_id: String, occurrence: String)
signal node_activated(node_id: String, occurrence: String)
signal edge_selected(relations: Array)
signal node_control_requested(occurrence: String, action: String)
signal camera_changed(state: Dictionary)
signal map_position_requested(point: Vector3)
signal map_pan_requested(delta: Vector3)
signal overview_zoom_requested(factor: float)
signal overview_pan_requested(delta: Vector2)
var dragging_region := false
var label_update_time := 0

const SCALE := 0.01
const PresentationTheme = preload("res://addons/polar_graphics/presentation_theme.gd")
const PALETTE = PresentationTheme.SOURCES
const NodeArrow=preload("res://addons/polar_graphics/node_arrow.gd")
const TypeGeometry = preload("res://addons/polar_graphics/type_geometry.gd")
const CARD_SHADER = preload("res://addons/polar_graphics/card.gdshader")
var role := "canvas"
var viewport_3d: SubViewport
var world: Node3D
var node_layer: Node3D
var edge_mesh: MeshInstance3D
var _selection_edges:MeshInstance3D
var _selection_edge_dirty:=true
var camera: Camera3D
var selected_id := ""
var selected_occurrence := ""
var hovered_key := ""
var points: Dictionary = {}
var bounds: Dictionary = {}
var records: Dictionary = {}
var labels: Dictionary = {}
var card_controls: Dictionary = {}
var meshes: Dictionary = {}
var edge_paths: Array = []
var edge_screen: Array = []
var source_layout: Dictionary = {}
var target := Vector3.ZERO
var view_size := 10.0
var content_inset_right:=0.0
var occlusion_rects:Array[Rect2]=[]
var orbit_distance := 22.0
var yaw := 0.28
var pitch := 0.28
var tilted := true
var dragging := false
var drag_button := 0
var drag_orbit := false
var drag_distance := 0.0
var drag_origin := Vector2.ZERO
var drag_last := Vector2.ZERO
var _suppress_click_release := false
var _last_click_key := ""
var _last_click_position := Vector2.INF
var _last_click_ms := -1000
var graph_multimesh: MultiMeshInstance3D
var graph_instances: Dictionary = {}
var graph_groups:Dictionary = {}
var graph_outline:MeshInstance3D
var visible_markers:Array = []
var focus_region: Array = []
var focus_mesh: MeshInstance3D
var focus_callout: Label
var _edge_dirty := true
var _label_dirty := true
var _settle_frames := 0
var _camera_tween: Tween
var _inertia_tween:Tween
var _drag_key := ""
var _drag_active := false
var _drag_plane:Plane
var _drag_offset:=Vector3.ZERO
var _drag_velocity:=Vector2.ZERO
var _drag_tick:=0
var _render_active := true
var _trail_tween:Tween
var _trail_mesh:MeshInstance3D

func _ready() -> void:
	if role=="canvas":pitch=-0.35
	mouse_filter = Control.MOUSE_FILTER_STOP
	focus_mode = Control.FOCUS_ALL
	custom_minimum_size = Vector2(140, 110) if role == "canvas" else Vector2.ZERO
	viewport_3d = SubViewport.new()
	viewport_3d.name = "SpatialViewport"
	viewport_3d.own_world_3d = true
	viewport_3d.transparent_bg = role in ["tree", "graph"]
	viewport_3d.msaa_3d = Viewport.MSAA_4X
	viewport_3d.render_target_update_mode = SubViewport.UPDATE_ONCE
	add_child(viewport_3d)
	world = Node3D.new()
	world.name = "GraphWorld3D"
	viewport_3d.add_child(world)
	var environment_node := WorldEnvironment.new()
	var environment := Environment.new()
	environment.background_mode = Environment.BG_COLOR
	environment.background_color = PresentationTheme.BACKGROUND
	environment.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	environment.ambient_light_color = Color("aec7d7")
	environment.ambient_light_energy = 0.65
	environment_node.environment = environment
	world.add_child(environment_node)
	var light := DirectionalLight3D.new()
	light.rotation_degrees = Vector3(-35, -25, 0)
	light.light_energy = 0.8
	world.add_child(light)
	node_layer = Node3D.new()
	node_layer.name = "Nodes"
	world.add_child(node_layer)
	edge_mesh = MeshInstance3D.new()
	edge_mesh.name = "RoutedEdges"
	var material := StandardMaterial3D.new()
	material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	material.vertex_color_use_as_albedo = true
	material.transparency = BaseMaterial3D.TRANSPARENCY_ALPHA
	material.cull_mode = BaseMaterial3D.CULL_DISABLED
	material.no_depth_test = false
	edge_mesh.material_override = material
	world.add_child(edge_mesh)
	_selection_edges=MeshInstance3D.new();_selection_edges.material_override=material;world.add_child(_selection_edges)
	focus_mesh = MeshInstance3D.new()
	focus_mesh.material_override = material
	world.add_child(focus_mesh)
	camera = Camera3D.new()
	camera.name = "ViewCamera3D"
	camera.near = 0.05
	camera.far = 10000.0
	camera.current = true
	world.add_child(camera)
	focus_callout = Label.new()
	focus_callout.mouse_filter = Control.MOUSE_FILTER_IGNORE
	focus_callout.add_theme_font_size_override("font_size", 12)
	focus_callout.add_theme_color_override("font_color", Color("e8f4f5"))
	focus_callout.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	var callout_style := StyleBoxFlat.new()
	callout_style.bg_color = Color("172c39")
	callout_style.set_border_width_all(1)
	callout_style.border_color = Color("6ebba8")
	callout_style.set_corner_radius_all(3)
	callout_style.set_content_margin_all(5)
	focus_callout.add_theme_stylebox_override("normal", callout_style)
	focus_callout.visible = false
	add_child(focus_callout)
	resized.connect(_resize)
	get_viewport().size_changed.connect(_resize)
	_sync_render_target()
	mouse_exited.connect(func():
		hovered_key = ""
		tooltip_text = ""
		_label_dirty = true)
	_update_camera()
	_settle_frames = 3

func _notification(what:int)->void:
	if what==NOTIFICATION_VISIBILITY_CHANGED:_sync_render_target()
	if what in [NOTIFICATION_VISIBILITY_CHANGED, NOTIFICATION_WM_WINDOW_FOCUS_OUT]:
		if what==NOTIFICATION_WM_WINDOW_FOCUS_OUT or not is_visible_in_tree():
			_stop_tween()
			if _mode_tween!=null:_mode_tween.kill()
			if _trail_tween!=null:_trail_tween.kill()
			if _trail_mesh!=null:_trail_mesh.hide()
			if _drag_active:node_dragged.emit(_drag_key,points.get(_drag_key,Vector3.ZERO),"cancel")
			_drag_active=false;dragging=false;drag_button=0

func _resize() -> void:
	_sync_render_target()
	if role == "tree" and camera != null: fit_all(false)
	_edge_dirty = true
	_label_dirty = true
	_settle_frames = 3

func set_render_active(value: bool) -> void:
	_render_active=value
	_sync_render_target()

func _sync_render_target() -> void:
	if viewport_3d == null or not is_inside_tree(): return
	# A stretching SubViewportContainer sizes its texture in logical pixels.
	# Draw the same GPU texture directly so backing allocation and logical
	# Camera3D projection/picking can use Godot's existing viewport override.
	var backing_scale := (get_viewport().get_final_transform() * get_global_transform_with_canvas()).get_scale().abs()
	var logical := Vector2i(size.round()).max(Vector2i.ONE)
	var drawing := _render_active and is_visible_in_tree()
	viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE if drawing else SubViewport.UPDATE_DISABLED
	viewport_3d.size = Vector2i((Vector2(logical) * backing_scale).round()).max(Vector2i.ONE) if drawing else Vector2i(2,2)
	viewport_3d.size_2d_override = logical
	viewport_3d.size_2d_override_stretch = true
	queue_redraw()

func _draw() -> void:
	if viewport_3d != null:
		draw_texture_rect(viewport_3d.get_texture(), Rect2(Vector2.ZERO, size), false)

func _process(_delta: float) -> void:
	if trail_frame_due():_draw_browse_path(true)
	if camera == null:
		return
	if _settle_frames > 0:
		_settle_frames -= 1
		_edge_dirty = true
		_label_dirty = true
	if _edge_dirty or _selection_edge_dirty or _label_dirty:
		viewport_3d.render_target_update_mode = SubViewport.UPDATE_ONCE
	if _edge_dirty:
		var started := Time.get_ticks_usec()
		_rebuild_edge_geometry()
		if OS.get_environment("POLAR_LAB_PROFILE") == "1": print("VIEW_EDGE_US ",role," ",Time.get_ticks_usec()-started)
		_edge_dirty = false
		_selection_edge_dirty=true
	if _selection_edge_dirty:
		_rebuild_edge_geometry(true);_selection_edge_dirty=false
	if _label_dirty:
		label_update_time = Time.get_ticks_msec()
		var started := Time.get_ticks_usec()
		_update_labels()
		if OS.get_environment("POLAR_LAB_PROFILE") == "1": print("VIEW_LABEL_US ",role," ",Time.get_ticks_usec()-started)
		_label_dirty = false

func _clear_nodes() -> void:
	_source_planes.clear()
	for controls in card_controls.values():
		for control in controls:
			remove_child(control)
			control.queue_free()
	card_controls.clear()
	for child in node_layer.get_children():
		node_layer.remove_child(child)
		child.queue_free()
	points.clear()
	bounds.clear()
	records.clear()
	labels.clear()
	meshes.clear()
	graph_instances.clear()
	graph_groups.clear()
	graph_outline = null
	graph_multimesh = null
	edge_paths.clear()
	edge_screen.clear()

func _color(record: Dictionary) -> Color:
	var ordinal:=int(record.get("source_index",record.get("branch",0)))
	return PresentationTheme.source_color(ordinal) if ordinal>=0 else PresentationTheme.MUTED

func _world_point(value: Vector2, z_value: float) -> Vector3:
	return Vector3(value.x * SCALE, -value.y * SCALE, z_value)

func _remove_card(key: String) -> void:
	if meshes.has(key):
		var anchor: Node = meshes[key].get_parent()
		node_layer.remove_child(anchor)
		anchor.queue_free()
	for button in card_controls.get(key, []):
		remove_child(button)
		button.queue_free()
	for values in [points, bounds, records, labels, meshes, card_controls]: values.erase(key)

func set_tree(layout: Dictionary, semantic: Array = []) -> void:
	if _layout_tween!=null:_layout_tween.kill()
	var previous_points:=points.duplicate()
	var identity_positions:Dictionary={}
	for key in records:identity_positions[str(records[key].get("node_id",key))]=points[key]
	var next_geometry_key:=str(layout.get("geometry_key",""))
	var base_changed:=next_geometry_key.is_empty() or next_geometry_key!=_base_geometry_key
	_base_geometry_key=next_geometry_key
	source_layout = layout
	if graph_multimesh != null: _clear_nodes()
	for key in points.keys():
		if not layout.get("items", {}).has(key): _remove_card(str(key))
	edge_paths.clear()
	edge_screen.clear()
	for key_value in layout.get("items", {}):
		var key := str(key_value)
		var item: Dictionary = layout.items[key]
		var position_2d: Vector2 = layout.positions.get(key, Vector2.ZERO)
		var size_2d: Vector2 = layout.sizes.get(key, Vector2(180, 54))
		var size_world := size_2d * SCALE
		# The existing card dictionaries are the resource cache. Expansion changes
		# placement and membership, not the title geometry of surviving cards.
		if records.has(key) and (str(records[key].get("title", "")) != str(item.get("title", "")) or bounds[key] != size_world or records[key].has("controls") != item.has("controls")):
			_remove_card(key)
		var retained := meshes.has(key)
		if not previous_points.has(key) and identity_positions.has(str(item.node_id)):previous_points[key]=identity_positions[str(item.node_id)]
		var z_value := -float(item.get("layer_index",item.get("source_index", 0))) * SOURCE_LAYER_GAP * layer_progress if _layering_enabled else -minf(float(item.get("depth", 0)) * 0.11, 0.7)
		if bool(layout.get("flat_layout",false)):z_value=0.0
		points[key] = _world_point(position_2d, z_value)
		if bool(item.get("temporary",false)) and not bool(layout.get("flat_layout",false)):
			var along:Vector3=camera.global_basis.z
			points[key]+=Vector3(along.x,along.y,0)*(z_value+float(active_layer)*SOURCE_LAYER_GAP*layer_progress)/maxf(0.01,along.z)
		bounds[key] = size_world
		records[key] = item
		if retained:
			meshes[key].get_parent().position = points[key]
			meshes[key].material_override.set_shader_parameter("accent", _color(item))
			var controls: Array = card_controls.get(key, [])
			for i in controls.size():
				controls[i].text = str(item.controls[i].text)
				controls[i].tooltip_text = str(item.controls[i].hint)
				controls[i].disabled = not bool(item.controls[i].enabled)
				controls[i].expansion_state=str(item.controls[i].get("state","collapsed"))
				controls[i].queue_redraw()
		else:
			_make_card(key, item)
	for route in layout.get("routes", []): _append_tree_route(route)
	for route in semantic: _append_tree_route(route)
	if role == "canvas":
		_update_source_planes()
		if base_changed and not previous_points.is_empty() and bool(motion_preferences.get("navigation_animation",true)) and not bool(motion_preferences.get("reduce_motion",false)):
			_animate_layout(previous_points)
	for key in meshes:
		var dim:=bool(records[key].get("context_dim",false))
		var silhouette:bool=int(records[key].get("layer_distance",0))>0 and not bool(records[key].get("direct_neighbor",false)) and not bool(layout.get("flat_layout",false))
		meshes[key].material_override.set_shader_parameter("silhouette",silhouette)
		meshes[key].material_override.set_shader_parameter("context_strength",0.45 if silhouette else (0.3 if dim else 1.0))
		labels[key].modulate.a=0.32 if dim else 1.0
	set_selection(selected_id, selected_occurrence)
	if role == "tree": fit_all(false)
	_edge_dirty = _edge_dirty or base_changed
	_selection_edge_dirty=true
	_label_dirty = true
	_settle_frames = 3

func _animate_layout(previous:Dictionary)->void:
	var final_points:=points.duplicate()
	_layout_tween=create_tween().set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_IN_OUT)
	_layout_tween.tween_method(func(progress:float):
		for key in final_points:
			var parent:=str(records[key].get("parent",""))
			var origin:Vector3=previous.get(key,previous.get(parent,final_points[key]))
			points[key]=origin.lerp(final_points[key],progress)
			if _layering_enabled:points[key].z=0.0 if bool(source_layout.get("flat_layout",false)) else -float(records[key].get("layer_index",records[key].get("source_index",0)))*SOURCE_LAYER_GAP*layer_progress
			meshes[key].get_parent().position=points[key]
		_refresh_routed_points()
		_edge_dirty=true;_label_dirty=true,0.0,1.0,PresentationTheme.TRANSITION_SECONDS)

func set_semantic_routes(routes: Array) -> void:
	edge_paths = edge_paths.filter(func(e: Dictionary): return e.kind in ["tree","unresolved"])
	for route in routes:
		_append_tree_route(route)
	_selection_edge_dirty = true

func _append_tree_route(route: Dictionary) -> void:
	var from_key := str(route.get("from", ""))
	var to_key := str(route.get("to", ""))
	if not points.has(from_key) or not points.has(to_key):
		return
	var route_points = route.get("points", PackedVector2Array())
	var spatial := PackedVector3Array()
	for i in route_points.size():
		var t := float(i) / maxf(1.0, float(route_points.size() - 1))
		var z_value := lerpf(points[from_key].z, points[to_key].z, t) - 0.025
		if role=="canvas" and not is_equal_approx(points[from_key].z,points[to_key].z):z_value+=sin(t*PI)*0.5
		var start:Vector3=points[from_key]-_world_point(source_layout.positions.get(from_key,Vector2.ZERO),points[from_key].z)
		var end:Vector3=points[to_key]-_world_point(source_layout.positions.get(to_key,Vector2.ZERO),points[to_key].z)
		spatial.append(_world_point(route_points[i], z_value)+start.lerp(end,t))
	if spatial.size() >= 2:
		var route_kind:=str(route.get("kind","tree"))
		if route_kind!="semantic" and not bool(route.get("relation",{}).get("resolved",true)):route_kind="unresolved"
		edge_paths.append({"from": from_key, "to": to_key, "points": spatial, "layout_points":route_points, "visual":route.get("visual",{}), "kind":route_kind, "relations": [route.get("relation", {})]})

func set_canvas_mode(mode: String, animate: bool = false) -> void:
	if role != "canvas" or mode not in ["two_d", "layered"]: return
	var was_enabled := _layering_enabled
	_layering_enabled = true
	var goal := 1.0 if mode == "layered" else 0.0
	if was_enabled and canvas_mode == mode and is_equal_approx(layer_progress, goal): return
	canvas_mode = mode
	tilted = mode == "layered"
	if _mode_tween != null: _mode_tween.kill()
	var anchor := screen_point(selected_occurrence) if points.has(selected_occurrence) else size * 0.5
	if animate and bool(motion_preferences.get("navigation_animation",true)) and not bool(motion_preferences.get("reduce_motion",false)):
		_mode_tween = create_tween().set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_IN_OUT)
		_mode_tween.tween_method(func(value: float): _set_layer_progress(value,anchor),layer_progress,goal,PresentationTheme.TRANSITION_SECONDS)
	else: _set_layer_progress(goal,anchor)

func _set_layer_progress(value: float, screen_anchor: Vector2) -> void:
	var previous: Vector3 = points.get(selected_occurrence,Vector3.ZERO)
	layer_progress = value
	for key in points:
		var point: Vector3 = points[key]
		point.z = 0.0 if bool(source_layout.get("flat_layout",false)) else -float(records[key].get("layer_index",records[key].get("source_index",0))) * SOURCE_LAYER_GAP * value
		points[key] = point
		if meshes.has(key): meshes[key].get_parent().position = point
	if points.has(selected_occurrence): target.z += points[selected_occurrence].z - previous.z
	_update_camera()
	for key in points:
		if not bool(records[key].get("temporary",false)) or bool(source_layout.get("flat_layout",false)):continue
		var base:Vector3=_world_point(source_layout.positions.get(key,Vector2.ZERO),points[key].z)
		var along:Vector3=camera.global_basis.z
		base+=Vector3(along.x,along.y,0)*(points[key].z+float(active_layer)*SOURCE_LAYER_GAP*value)/maxf(0.01,along.z)
		points[key]=base;meshes[key].get_parent().position=base
	if points.has(selected_occurrence):
		var point: Vector3 = points[selected_occurrence]
		var plane := Plane(Vector3.FORWARD,Vector3.FORWARD.dot(point))
		var hit = plane.intersects_ray(camera.project_ray_origin(screen_anchor),camera.project_ray_normal(screen_anchor))
		if hit is Vector3:
			target += point - hit
			_update_camera()
	_refresh_routed_points()
	for ordinal in _source_planes:
		_source_planes[ordinal].visible=value>0.001 and not bool(source_layout.get("flat_layout",false))
		_source_planes[ordinal].position.z = -float(ordinal)*SOURCE_LAYER_GAP*value-0.035
	_edge_dirty=true;_label_dirty=true
	camera_changed.emit(get_camera_state())

func _refresh_routed_points()->void:
	for edge in edge_paths:
		if not edge.has("layout_points"): continue
		var routed: PackedVector2Array = edge.layout_points
		var projected := PackedVector3Array()
		for i in routed.size():
			var t := float(i)/maxf(1,routed.size()-1)
			var start:Vector3=points[edge.from]-_world_point(source_layout.positions.get(edge.from,Vector2.ZERO),points[edge.from].z)
			var end:Vector3=points[edge.to]-_world_point(source_layout.positions.get(edge.to,Vector2.ZERO),points[edge.to].z)
			var height:=sin(t*PI)*0.5 if not is_equal_approx(points[edge.from].z,points[edge.to].z) else 0.0
			projected.append(_world_point(routed[i],lerpf(points[edge.from].z,points[edge.to].z,t)-0.025+height)+start.lerp(end,t))
		edge.points = projected

func _update_source_planes() -> void:
	var rectangles: Dictionary = {}
	var plane_colors:Dictionary={}
	for key in points:
		if bool(records[key].get("temporary",false)) or bool(records[key].get("external",false)):continue
		var ordinal := int(records[key].get("layer_index",records[key].get("source_index",0)))
		if ordinal<0:continue
		plane_colors[ordinal]=_color(records[key])
		var rect := Rect2(Vector2(points[key].x,points[key].y)-bounds[key]*0.5,bounds[key]).grow(0.25)
		rectangles[ordinal] = rectangles[ordinal].merge(rect) if rectangles.has(ordinal) else rect
	for ordinal in _source_planes.keys():
		if not rectangles.has(ordinal): _source_planes[ordinal].queue_free();_source_planes.erase(ordinal)
	for ordinal in rectangles:
		var plane: MeshInstance3D = _source_planes.get(ordinal)
		if plane == null:
			plane=MeshInstance3D.new();plane.name="SourcePlane_"+str(ordinal)
			var material:=StandardMaterial3D.new();material.shading_mode=BaseMaterial3D.SHADING_MODE_UNSHADED
			material.transparency=BaseMaterial3D.TRANSPARENCY_ALPHA;material.cull_mode=BaseMaterial3D.CULL_DISABLED
			material.albedo_color=Color(plane_colors.get(ordinal,PresentationTheme.MUTED),0.065)
			plane.material_override=material;node_layer.add_child(plane);_source_planes[ordinal]=plane
		var rect:Rect2=rectangles[ordinal]
		var quad:=QuadMesh.new();quad.size=rect.size;plane.mesh=quad
		plane.visible=layer_progress>0.001 and not bool(source_layout.get("flat_layout",false))
		plane.position=Vector3(rect.get_center().x,rect.get_center().y,-ordinal*SOURCE_LAYER_GAP*layer_progress-0.035)

func _make_card(key: String, item: Dictionary) -> void:
	var size_world: Vector2 = bounds[key]
	var anchor := Node3D.new()
	anchor.name = "Occurrence_" + key.sha256_text().substr(0, 14)
	anchor.position = points[key]
	node_layer.add_child(anchor)
	var card := MeshInstance3D.new()
	var quad := QuadMesh.new()
	quad.size = size_world
	card.mesh = quad
	var material := ShaderMaterial.new()
	material.shader = CARD_SHADER
	material.set_shader_parameter("rect_size", size_world)
	material.set_shader_parameter("accent", _color(item))
	material.set_shader_parameter("base_color",PresentationTheme.CARD)
	material.set_shader_parameter("selection_outline",PresentationTheme.SELECTED)
	material.set_shader_parameter("selected", false)
	material.set_shader_parameter("compact", role == "tree")
	card.material_override = material
	anchor.add_child(card)
	meshes[key] = card
	var label := Label3D.new()
	label.name = "Title"
	label.font_size = 42
	# Layout measures a 14px font. Higher-resolution glyphs preserve exactly
	# that world-space metric rather than inflating labels to 23 layout pixels.
	label.pixel_size = 0.01 / 3.0
	label.outline_size = 0
	label.modulate = PresentationTheme.TEXT
	var has_controls := role == "canvas" and item.has("controls")
	label.position = Vector3(0, 0, 0.014)
	label.offset = Vector2(-0.16 / label.pixel_size if has_controls else 0, 0)
	label.width = maxf(20.0, (size_world.x - 0.25 - (0.32 if has_controls else 0.0)) / label.pixel_size)
	label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	label.text = _fit_card_title(str(item.get("title", item.get("node_id", key))), label, size_world)
	label.no_depth_test = false
	label.billboard = BaseMaterial3D.BILLBOARD_ENABLED
	label.render_priority = 1
	anchor.add_child(label)
	labels[key] = label
	if role=="canvas":
		var metadata:=Label3D.new();metadata.font_size=28;metadata.pixel_size=label.pixel_size
		metadata.text=str(item.get("icon","○"))+ ("  ▣" if item.get("container",false) else "")+ ("  "+str(item.get("status","")) if not str(item.get("status","")).is_empty() else "")
		metadata.billboard=BaseMaterial3D.BILLBOARD_ENABLED;metadata.outline_size=0;metadata.modulate=_color(item)
		metadata.offset=Vector2(-0.1/metadata.pixel_size,(size_world.y*0.5-0.065)/metadata.pixel_size)
		anchor.add_child(metadata)
	if role == "canvas" and item.has("controls"):
		var controls: Array[Button] = []
		for spec in item.controls:
			var button := NodeArrow.new()
			button.direction=str(spec.action)
			button.expansion_state=str(spec.get("state","collapsed"))
			button.text = str(spec.text)
			button.tooltip_text = str(spec.hint)
			button.disabled = not bool(spec.enabled)
			button.focus_mode = Control.FOCUS_NONE
			button.add_theme_font_size_override("font_size", 11)
			button.custom_minimum_size = Vector2(22, 16)
			for state in ["normal", "hover", "pressed", "disabled"]:
				var style := StyleBoxFlat.new()
				style.bg_color = Color(0.28, 0.48, 0.58, 0.4) if state in ["hover", "pressed"] else Color(0.1, 0.2, 0.27, 0.2)
				style.set_content_margin_all(0)
				style.set_corner_radius_all(3)
				button.add_theme_stylebox_override(state, style)
			var action := str(spec.action)
			button.pressed.connect(func():
				grab_focus()
				node_control_requested.emit(key, action))
			add_child(button)
			controls.append(button)
		card_controls[key] = controls

func _short_title(value: String, limit: int) -> String:
	var text := value.replace("\n", " ").strip_edges()
	return text if text.length() <= limit else text.substr(0, limit - 1) + "…"

func _font(label: Label3D) -> Font:
	return label.font if label.font != null else ThemeDB.fallback_font

func _fit_card_title(value: String, label: Label3D, card_size: Vector2) -> String:
	var font := _font(label)
	var full := value.replace("\n", " ").strip_edges().left(160)
	var limit := minf(font.get_height(label.font_size) * 2.0 + 1.0,
		(card_size.y - 0.13) / label.pixel_size)
	var text := full
	while text.length() > 1 and font.get_multiline_string_size(text, HORIZONTAL_ALIGNMENT_CENTER,
		label.width, label.font_size).y > limit:
		full = full.left(full.length() - 1)
		text = full + "…"
	return text

func _fit_graph_title(value: String) -> String:
	var text := value.replace("\n", " ").strip_edges().left(120)
	var font: Font = ThemeDB.fallback_font
	var width_limit := minf(190.0, maxf(110.0, size.x - 40.0))
	var full := text
	while text.length() > 1 and font.get_string_size(text, HORIZONTAL_ALIGNMENT_LEFT, -1, 15).x > width_limit:
		full = full.left(full.length() - 1)
		text = full + "…"
	return text

func set_graph(graph_data: Dictionary, coordinates: Dictionary) -> void:
	_clear_nodes()
	var groups:Dictionary={}
	for item:Dictionary in graph_data.get("nodes",[]):
		var shape:=str(item.get("shape","circle"))
		if not groups.has(shape):groups[shape]=[]
		groups[shape].append(item)
	for shape:String in groups:
		var multi:=MultiMesh.new();multi.transform_format=MultiMesh.TRANSFORM_3D;multi.use_colors=true
		multi.mesh=TypeGeometry.mesh(shape);multi.instance_count=groups[shape].size()
		var instance:=MultiMeshInstance3D.new();instance.name="Type_"+shape;instance.multimesh=multi
		var material:=StandardMaterial3D.new();material.vertex_color_use_as_albedo=true;material.roughness=0.85
		instance.material_override=material;node_layer.add_child(instance);graph_groups[shape]=instance
		if graph_multimesh==null:graph_multimesh=instance
		for i in groups[shape].size():
			var item:Dictionary=groups[shape][i];var id:=str(item.id)
			var coord:Vector3=coordinates.get(id,Vector3.ZERO);var radius:=0.25
			points[id]=coord;bounds[id]=Vector2.ONE*radius*2;records[id]=item
			graph_instances[id]={"shape":shape,"index":i}
			multi.set_instance_transform(i,Transform3D(Basis.IDENTITY.scaled(Vector3.ONE*radius),coord))
			multi.set_instance_color(i,_color(item))
			var label:=Label3D.new();label.text=("▣ " if bool(item.get("container",false)) else "")+_fit_graph_title(str(item.get("title",id)))
			if not str(item.get("status","")).is_empty():label.text+="\n["+str(item.status)+"]"
			label.font_size=40;label.pixel_size=0.006;label.billboard=BaseMaterial3D.BILLBOARD_ENABLED
			label.outline_size=3;label.outline_modulate=PresentationTheme.BACKGROUND;label.modulate=PresentationTheme.TEXT
			label.position=coord+Vector3(0,radius+0.2,0);node_layer.add_child(label);labels[id]=label
	var lane_counts:Dictionary={}
	for link:Dictionary in graph_data.get("links",[]):
		var pair:Array=[str(link.source),str(link.target)];pair.sort()
		var key:=JSON.stringify(pair);lane_counts[key]=int(lane_counts.get(key,0))+1
	var lanes:Dictionary={}
	for link:Dictionary in graph_data.get("links",[]):
		var a:=str(link.source);var b:=str(link.target)
		if not points.has(a) or not points.has(b):continue
		var pair:Array=[a,b];pair.sort();var key:=JSON.stringify(pair)
		var lane:=int(lanes.get(key,0));lanes[key]=lane+1
		edge_paths.append({"from":a,"to":b,"relations":[link.get("relation",{})],"visual":link.get("visual",{}),"kind":"graph","lane":lane-(int(lane_counts[key])-1)*0.5})
	graph_outline=MeshInstance3D.new();graph_outline.name="SelectionOutline"
	var outline_material:=StandardMaterial3D.new();outline_material.shading_mode=BaseMaterial3D.SHADING_MODE_UNSHADED
	outline_material.albedo_color=PresentationTheme.SELECTED;outline_material.cull_mode=BaseMaterial3D.CULL_DISABLED
	graph_outline.material_override=outline_material;node_layer.add_child(graph_outline)
	update_graph_coordinates(coordinates)
	set_selection(selected_id,selected_occurrence)
	_settle_frames=3

func update_graph_coordinates(coordinates:Dictionary)->void:
	for id:String in coordinates:
		if not graph_instances.has(id):continue
		var p:Vector3=coordinates[id];points[id]=p
		var spec:Dictionary=graph_instances[id]
		graph_groups[spec.shape].multimesh.set_instance_transform(int(spec.index),Transform3D(Basis.IDENTITY.scaled(Vector3.ONE*0.25),p))
		labels[id].position=p+Vector3(0,0.45,0)
	for edge:Dictionary in edge_paths:
		var a:Vector3=points[edge.from];var b:Vector3=points[edge.to];var route:=PackedVector3Array()
		var lane:=float(edge.get("lane",0))
		if edge.from==edge.to:
			for n in 25:
				var angle:=TAU*n/24.0;route.append(a+Vector3(0.35+cos(angle)*0.4,sin(angle)*0.4,0.07*lane))
		else:
			var direction:Vector3=(b-a).normalized();a+=direction*0.27;b-=direction*0.27
			var side:Vector3=direction.cross(Vector3.UP).normalized()
			if side.length_squared()<0.001:side=Vector3.RIGHT
			for n in 13:
				var t:=n/12.0;route.append(a.lerp(b,t)+side*sin(t*PI)*lane*0.24)
		edge.points=route
	_update_graph_outline();_edge_dirty=true;_label_dirty=true

func _update_graph_outline()->void:
	if graph_outline==null:return
	graph_outline.visible=graph_instances.has(selected_id)
	if graph_outline.visible:
		graph_outline.mesh=TypeGeometry.selection(str(graph_instances[selected_id].shape))
		graph_outline.position=points[selected_id];graph_outline.scale=Vector3.ONE*0.285

func key_for_node(node_id:String,preferred:String="")->String:
	if points.has(preferred):return preferred
	for key in records:
		if str(records[key].get("node_id",key))==node_id:return str(key)
	return ""

func set_selection(node_id: String, occurrence: String = "") -> void:
	var previous_id:=selected_id;var previous_occurrence:=selected_occurrence
	if role!="graph":occurrence=key_for_node(node_id,occurrence)
	selected_id = node_id
	selected_occurrence = occurrence
	for key in meshes:
		var was_selected:bool=key==previous_occurrence if not previous_occurrence.is_empty() else str(records[key].get("node_id",""))==previous_id
		if bool(source_layout.get("flat_layout",false)):was_selected=str(records[key].get("node_id",""))==previous_id
		if not was_selected and not _key_is_selected(key):continue
		var active:bool=_key_is_selected(key)
		meshes[key].material_override.set_shader_parameter("selected", active)
	_update_graph_outline()
	_selection_edge_dirty = true
	_label_dirty = true

func _key_is_selected(key: String) -> bool:
	if role == "graph":
		return key == selected_id
	if role=="canvas" and bool(source_layout.get("flat_layout",false)):return str(records.get(key,{}).get("node_id",""))==selected_id
	return key == selected_occurrence or (selected_occurrence.is_empty() and str(records.get(key, {}).get("node_id", "")) == selected_id)

func _update_camera() -> void:
	if camera == null:
		return
	if role == "graph":
		camera.projection = Camera3D.PROJECTION_PERSPECTIVE
		camera.keep_aspect = Camera3D.KEEP_HEIGHT
		camera.fov = 43.0
		var vector := Vector3(sin(yaw) * cos(pitch), sin(pitch), cos(yaw) * cos(pitch))
		camera.position = target + vector * orbit_distance
	else:
		camera.projection = Camera3D.PROJECTION_ORTHOGONAL
		camera.keep_aspect = Camera3D.KEEP_HEIGHT
		camera.size = view_size
		var vector := Vector3(0.08, 0.19, 1.0).normalized() if tilted else Vector3(0, 0, 1)
		if role == "canvas" and _layering_enabled:
			vector = Vector3(0,0,1).lerp(Vector3(0.12,0.24,1).normalized(),0.0 if bool(source_layout.get("flat_layout",false)) else layer_progress).normalized()
		if role == "tree" and tilted:
			vector = Vector3(0.025, 0.08, 1.0).normalized()
		camera.position = target + vector * 70.0
	camera.look_at(target, Vector3.UP)
	_edge_dirty = true
	_label_dirty = true
	if role == "tree": set_focus_region(focus_region)

func get_camera_state() -> Dictionary:
	return {"target": [target.x, target.y, target.z], "size": view_size, "distance": orbit_distance, "yaw": yaw, "pitch": pitch, "tilted": tilted}

func restore_camera(value: Dictionary, animate:bool=false) -> void:
	if role == "tree":
		fit_all(false)
		return
	if value.is_empty():
		return
	_stop_tween()
	var v = value.get("target", [0.0, 0.0, 0.0])
	if not v is Array or v.size() != 3:
		return
	var restored_target:=Vector3(float(v[0]), float(v[1]), float(v[2]))
	var restored_size:=clampf(float(value.get("size", 10)), 1.0, 2000.0)
	var restored_distance:=clampf(float(value.get("distance", 22)), 1.0, 2000.0)
	yaw = float(value.get("yaw", 0.28))
	pitch = clampf(float(value.get("pitch", 0.28)), -1.35, 1.35)
	tilted = bool(value.get("tilted", true))
	_move_camera(restored_target,restored_size,restored_distance,animate)

func _stop_tween() -> void:
	if _inertia_tween!=null:_inertia_tween.kill()
	if _camera_tween != null and _camera_tween.is_running():
		_camera_tween.kill()

func fit_all(animate: bool = false) -> void:
	fit_keys(points.keys(), animate)

func fit_keys(keys: Array, animate: bool = false, readable: bool = false) -> void:
	if keys.is_empty() or size.x < 1 or size.y < 1:
		return
	var lo := Vector3(INF, INF, INF)
	var hi := Vector3(-INF, -INF, -INF)
	for key in keys:
		if not points.has(key):
			continue
		var p: Vector3 = points[key]
		if role!="graph":p=camera.global_basis.inverse()*p
		var b: Vector2 = bounds.get(key, Vector2.ONE)
		lo = lo.min(p - Vector3(b.x, b.y, 0.3) * 0.5)
		hi = hi.max(p + Vector3(b.x, b.y, 0.3) * 0.5)
	if not lo.is_finite():
		return
	var new_target := (lo + hi) * 0.5
	if role!="graph":new_target=camera.global_basis*new_target
	var span := hi - lo
	var aspect := maxf(0.1, (size.x-content_inset_right) / maxf(size.y, 1))
	var new_size := maxf(2.5, maxf(span.y, span.x / aspect) * 1.22)
	if readable:
		new_size = minf(new_size, 6.2)
		if points.has(selected_occurrence):
			new_target = points[selected_occurrence]
		# Keep the initial left/right title columns inside the working viewport;
		# vertical overflow is deliberate readable focus, covered by the navigator.
		var focus_local:=camera.global_basis.inverse()*new_target
		var width_from_focus := 2.0 * maxf(absf(lo.x - focus_local.x), absf(hi.x - focus_local.x))
		new_size = maxf(new_size, width_from_focus / aspect * 1.08)
	var half_fov := deg_to_rad(43.0) * 0.5
	var direction := Vector3(sin(yaw) * cos(pitch), sin(pitch), cos(yaw) * cos(pitch))
	var right := Vector3.UP.cross(direction).normalized()
	var up := direction.cross(right)
	var distance := 4.0
	# Fit actual projected points, not the diagonal of an enclosing sphere.
	# The latter shrinks a useful overview into a tiny ball, especially wide.
	for key in keys:
		if not points.has(key):
			continue
		var relative: Vector3 = points[key] - new_target
		var radius := float(bounds.get(key, Vector2.ONE).x) * 0.5
		var z := relative.dot(direction)
		distance = maxf(distance, z + (absf(relative.dot(right)) + radius) / (tan(half_fov) * aspect))
		distance = maxf(distance, z + (absf(relative.dot(up)) + radius) / tan(half_fov))
	distance *= 1.12
	if role=="canvas":new_target+=camera.global_basis.x*new_size*content_inset_right/maxf(1,size.y)*0.5
	_move_camera(new_target, new_size, distance, animate)

func center_key(key: String, animate: bool = true) -> void:
	if not points.has(key):
		return
	var next_size:=minf(view_size,minf(6.2,maxf(1,0.14*size.y/13.0))) if role=="canvas" else view_size
	var next_target:Vector3=points[key]+camera.global_basis.x*next_size*content_inset_right/maxf(1,size.y)*0.5
	_move_camera(next_target,next_size,orbit_distance,animate)

func _move_camera(new_target: Vector3, new_size: float, distance: float, animate: bool) -> void:
	animate=animate and bool(motion_preferences.get("navigation_animation",true)) and not bool(motion_preferences.get("reduce_motion",false))
	_stop_tween()
	if animate and OS.get_environment("POLAR_LAB_NO_ANIMATION") != "1":
		_camera_tween = create_tween().set_parallel(true).set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
		_camera_tween.tween_property(self, "target", new_target, 0.24)
		_camera_tween.tween_property(self, "view_size", new_size, 0.24)
		_camera_tween.tween_property(self, "orbit_distance", distance, 0.24)
		_camera_tween.tween_method(func(_t: float): _update_camera(), 0.0, 1.0, 0.24)
		_camera_tween.chain().tween_callback(func(): camera_changed.emit(get_camera_state()))
	else:
		target = new_target
		view_size = new_size
		orbit_distance = distance
		_update_camera()
		camera_changed.emit(get_camera_state())

func toggle_tilt() -> void:
	tilted = not tilted
	_update_camera()
	camera_changed.emit(get_camera_state())

func _plane_point(screen: Vector2) -> Vector3:
	var origin := camera.project_ray_origin(screen)
	var direction := camera.project_ray_normal(screen)
	var normal := camera.global_basis.z if role == "graph" else Vector3(0, 0, 1)
	var plane := Plane(normal, normal.dot(target))
	var point = plane.intersects_ray(origin, direction)
	return point if point is Vector3 else target

func _gui_input(event: InputEvent) -> void:
	if event is InputEventMouseButton and not OS.get_environment("POLAR_STATION_TRACE_DIR").is_empty():
		print("GRAPHICS_POINTER ", role, " ", event.position, " button=", event.button_index, " pressed=", event.pressed)
	if camera == null:
		return
	if event is InputEventMagnifyGesture:
		if not OS.get_environment("POLAR_STATION_TRACE_DIR").is_empty(): print("GRAPHICS_GESTURE magnify ",role," ",event.position," factor=",event.factor)
		if role == "tree": overview_zoom_requested.emit(event.factor)
		else: zoom_at(event.factor, event.position)
		accept_event()
		return
	if event is InputEventMouseButton:
		if event.button_index in [MOUSE_BUTTON_WHEEL_UP, MOUSE_BUTTON_WHEEL_DOWN] and event.pressed:
			var factor := 1.14 if event.button_index == MOUSE_BUTTON_WHEEL_UP else 1.0 / 1.14
			if role == "tree": overview_zoom_requested.emit(factor)
			elif role=="canvas" and not event.button_mask & MOUSE_BUTTON_MASK_RIGHT:
				tree_step_requested.emit((1 if event.button_index==MOUSE_BUTTON_WHEEL_DOWN else -1)*event.factor)
			else:
				if role=="canvas":_suppress_click_release=true
				zoom_at(pow(factor,event.factor), event.position)
			accept_event()
			return
		if event.pressed:
			_stop_tween()
			if drag_button != 0: return
			grab_focus()
			_suppress_click_release = false
			drag_distance = 0
			drag_button = event.button_index
			dragging_region = false
			if role == "tree" and event.button_index == MOUSE_BUTTON_LEFT and focus_region.size() == 4:
				var polygon := PackedVector2Array()
				for corner in focus_region: polygon.append(camera.unproject_position(corner))
				dragging_region = Geometry2D.is_point_in_polygon(event.position, polygon)
			elif role=="tree" and event.button_index==MOUSE_BUTTON_LEFT and not visible_markers.is_empty():dragging_region=false
			if role == "tree" and not dragging_region:
				drag_button = 0
				accept_event()
				return
			drag_origin = event.position
			drag_last = event.position
			_drag_key=pick_node(event.position) if event.button_index==MOUSE_BUTTON_LEFT and role!="tree" else ""
			_drag_active=false;_drag_velocity=Vector2.ZERO;_drag_tick=Time.get_ticks_usec()
			if not _drag_key.is_empty():
				var normal:Vector3=camera.global_basis.z if role=="graph" else Vector3.BACK
				_drag_plane=Plane(normal,normal.dot(points[_drag_key]))
				var hit=_drag_plane.intersects_ray(camera.project_ray_origin(event.position),camera.project_ray_normal(event.position))
				_drag_offset=points[_drag_key]-hit if hit is Vector3 else Vector3.ZERO
			drag_orbit = (role == "graph") and event.button_index == MOUSE_BUTTON_LEFT and not space_pressed
			dragging = event.button_index in [MOUSE_BUTTON_MIDDLE, MOUSE_BUTTON_LEFT, MOUSE_BUTTON_RIGHT]
		else:
			if event.button_index != drag_button: return
			drag_button = 0
			var was_click := drag_distance < 6.0
			dragging = false
			if _drag_active:
				node_dragged.emit(_drag_key,points.get(_drag_key,Vector3.ZERO),"end")
				_drag_active=false;_drag_key="";_suppress_click_release=true
			elif not was_click and _drag_key.is_empty() and role!="tree":_start_inertia()
			_drag_key=""
			if role == "tree":
				accept_event()
				return
			if was_click and not _suppress_click_release and event.button_index == MOUSE_BUTTON_LEFT:
				var key := pick_node(event.position)
				if not key.is_empty():
					var id := str(records[key].get("node_id", key))
					_last_click_key = key
					_last_click_position = event.position
					_last_click_ms = Time.get_ticks_msec()
					node_selected.emit(id, "" if role == "graph" else key)
				elif role == "tree":
					map_position_requested.emit(_plane_point(event.position))
			elif was_click and not _suppress_click_release and event.button_index == MOUSE_BUTTON_RIGHT:
				var relations := pick_edge(event.position)
				if not relations.is_empty():
					edge_selected.emit(relations)
		if role != "tree" and event.pressed and event.double_click and event.button_index == MOUSE_BUTTON_LEFT:
			var key := pick_node(event.position)
			if Time.get_ticks_msec() - _last_click_ms < 500 and event.position.distance_to(_last_click_position) < 8 and records.has(_last_click_key):
				key = _last_click_key
			if not key.is_empty():
				_suppress_click_release = true
				dragging = false
				node_activated.emit(str(records[key].get("node_id", key)), "" if role == "graph" else key)
		accept_event()
	elif event is InputEventMouseMotion:
		if dragging:
			_stop_tween()
			drag_distance = maxf(drag_distance,event.position.distance_to(drag_origin))
			if drag_distance < 6.0:
				accept_event()
				return
			var delta: Vector2 = event.position - drag_last
			var now:=Time.get_ticks_usec();var elapsed:=maxf(0.001,(now-_drag_tick)/1000000.0)
			_drag_velocity=delta/elapsed;_drag_tick=now
			if not _drag_key.is_empty():
				if not _drag_active:_drag_active=true;node_dragged.emit(_drag_key,points[_drag_key],"begin")
				var hit=_drag_plane.intersects_ray(camera.project_ray_origin(event.position),camera.project_ray_normal(event.position))
				if hit is Vector3:node_dragged.emit(_drag_key,hit+_drag_offset,"move")
				drag_last=event.position;accept_event();return
			if dragging_region:
				map_pan_requested.emit(_plane_point(event.position) - _plane_point(drag_last))
				drag_last = event.position
				accept_event()
				return
			if drag_orbit:
				yaw -= delta.x * 0.008
				pitch = clampf(pitch + delta.y * 0.008, -1.35, 1.35)
			else:
				target += _plane_point(drag_last) - _plane_point(event.position)
			drag_last = event.position
			_update_camera()
			camera_changed.emit(get_camera_state())
		else:
			var key := pick_node(event.position)
			if key != hovered_key:
				hovered_key = key
				tooltip_text = str(records.get(key, {}).get("title", ""))+"\n"+str(records.get(key,{}).get("type",""))+" · "+str(records.get(key,{}).get("status",""))
				_label_dirty = true
		accept_event()
	elif event is InputEventPanGesture:
		if not OS.get_environment("POLAR_STATION_TRACE_DIR").is_empty(): print("GRAPHICS_GESTURE pan ",role," ",event.position," delta=",event.delta)
		if role == "tree": overview_pan_requested.emit(preload("res://addons/polar_graphics/gesture_motion.gd").content_pixels(event.delta))
		elif role=="canvas" and event.alt_pressed:tree_step_requested.emit(event.delta.y*0.16)
		else: pan_by(preload("res://addons/polar_graphics/gesture_motion.gd").content_pixels(event.delta))
		accept_event()

func _start_inertia()->void:
	if not bool(motion_preferences.get("camera_inertia",true)) or bool(motion_preferences.get("reduce_motion",false)):return
	if Time.get_ticks_usec()-_drag_tick>100000 or _drag_velocity.length()<60:return
	var velocity:=_drag_velocity.limit_length(1800);var previous:Array=[0.0]
	_inertia_tween=create_tween().set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
	_inertia_tween.tween_method(func(progress:float):
		var delta:=velocity*(progress-float(previous[0]))*0.16;previous[0]=progress
		if drag_orbit:yaw-=delta.x*0.008;pitch=clampf(pitch+delta.y*0.008,-1.35,1.35)
		else:target+=_plane_point(size*0.5)-_plane_point(size*0.5+delta)
		_update_camera();camera_changed.emit(get_camera_state()),0.0,1.0,0.45)

func update_canvas_point(key:String,position_value:Vector3)->void:
	if not points.has(key):return
	var before:Vector3=points[key];position_value.z=before.z
	points[key]=position_value;meshes[key].get_parent().position=position_value
	if source_layout.has("positions"):source_layout.positions[key]=Vector2(position_value.x,-position_value.y)/SCALE
	for edge:Dictionary in edge_paths:
		if edge.from!=key and edge.to!=key:continue
		var route:PackedVector3Array=edge.points
		for i in route.size():
			var t:=float(i)/maxf(1,route.size()-1)
			route[i]+=(position_value-before)*(1.0-t if edge.from==key else t)
		edge.points=route
		var layout_points:=PackedVector2Array()
		for p in route:layout_points.append(Vector2(p.x,-p.y)/SCALE)
		edge.layout_points=layout_points
	_edge_dirty=true;_label_dirty=true

func show_browse_path(keys:Array)->void:
	clear_browse_path()
	for i in range(1,keys.size()):
		for edge:Dictionary in edge_paths:
			if (edge.from==keys[i-1] and edge.to==keys[i]) or (edge.to==keys[i-1] and edge.from==keys[i]):
				var route:PackedVector3Array=edge.points.duplicate()
				if edge.to==keys[i-1]:route.reverse()
				_trail_points.append_array(route);break
	if _trail_points.size()<2:return
	if _trail_mesh==null:
		_trail_mesh=MeshInstance3D.new();world.add_child(_trail_mesh)
		_trail_mesh.material_override=edge_mesh.material_override
	_trail_mesh.show();_draw_browse_path(false)

func clear_browse_path()->void:
	_trail_points.clear()
	if _trail_tween!=null:_trail_tween.kill()
	if _trail_mesh!=null:_trail_mesh.hide()

func trail_frame_due()->bool:
	return not _trail_points.is_empty() and is_visible_in_tree() and bool(motion_preferences.get("connection_flow",true)) and not bool(motion_preferences.get("reduce_motion",false)) and Time.get_ticks_msec()>=_trail_due_ms

func _draw_browse_path(advance:bool)->void:
	if _trail_points.size()<2 or _trail_mesh==null:return
	if advance:_trail_phase=fposmod(_trail_phase+0.07,1.0)
	_trail_due_ms=Time.get_ticks_msec()+67
	var vertices:=PackedVector3Array();var colors:=PackedColorArray()
	for i in range(1,_trail_points.size()):
		var a:=_trail_points[i-1];var b:=_trail_points[i];var normal:Vector3=(b-a).cross(camera.global_basis.z).normalized()*_pixel_world(b)*1.4
		var phase:=float(i)/_trail_points.size();var color:=PresentationTheme.SELECTED
		color.a=1.0 if absf(phase-_trail_phase)<0.15 else 0.35
		for point in [a-normal,a+normal,b+normal,a-normal,b+normal,b-normal]:vertices.append(point+camera.global_basis.z*0.04);colors.append(color)
	var arrays:Array=[];arrays.resize(Mesh.ARRAY_MAX);arrays[Mesh.ARRAY_VERTEX]=vertices;arrays[Mesh.ARRAY_COLOR]=colors
	var mesh:=ArrayMesh.new();mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES,arrays);_trail_mesh.mesh=mesh
	viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE

func reveal_readable(key:String,policy:String="reading_priority")->void:
	if not points.has(key):return
	var rectangle:=screen_card_rect(key)
	var safe:=Rect2(Vector2(24,24),Vector2(maxf(60,size.x-48),maxf(60,size.y-48)))
	var covered:=occlusion_rects.any(func(rect):return rect.intersects(rectangle))
	var readable:=0.14/maxf(0.00001,_pixel_world(points[key]))>=13.0
	if not covered and safe.encloses(rectangle) and (readable or policy=="fixed_zoom"):return
	if policy=="both_endpoints" and points.has(browse_origin_key) and browse_origin_key!=key:
		fit_keys([browse_origin_key,key],true,false);return
	var next_size:=view_size
	if not readable and policy!="fixed_zoom":next_size=minf(view_size,0.14*size.y/13.0)
	var next_target:=target
	if not is_equal_approx(next_size,view_size):
		next_target=points[key]+camera.global_basis.x*next_size*content_inset_right/maxf(1,size.y)*0.5
	else:
		var shift:=Vector2.ZERO
		if covered:shift.y=size.y*0.5-rectangle.get_center().y
		if rectangle.position.x<safe.position.x:shift.x=safe.position.x-rectangle.position.x
		elif rectangle.end.x>safe.end.x:shift.x=safe.end.x-rectangle.end.x
		if rectangle.position.y<safe.position.y:shift.y=safe.position.y-rectangle.position.y
		elif rectangle.end.y>safe.end.y:shift.y=safe.end.y-rectangle.end.y
		next_target-=camera.global_basis.x*shift.x*_pixel_world(points[key])-camera.global_basis.y*shift.y*_pixel_world(points[key])
	_move_camera(next_target,next_size,orbit_distance,true)

func zoom_at(factor: float, position_value: Vector2) -> void:
	if not is_finite(factor) or factor <= 0: return
	_stop_tween()
	var before := _plane_point(position_value)
	if role == "graph": orbit_distance = clampf(orbit_distance / factor, 1.0, 2000.0)
	else: view_size = clampf(view_size / factor, 1.0, 2000.0)
	_update_camera()
	target += before - _plane_point(position_value)
	_update_camera()
	camera_changed.emit(get_camera_state())

func pan_by(delta: Vector2) -> void:
	_stop_tween()
	target += _plane_point(size * 0.5) - _plane_point(size * 0.5 + delta)
	_update_camera()
	camera_changed.emit(get_camera_state())

func pick_node(screen: Vector2) -> String:
	var origin := camera.project_ray_origin(screen)
	var direction := camera.project_ray_normal(screen)
	var nearest := INF
	var result := ""
	for key in points:
		if role=="canvas" and int(records[key].get("layer_distance",0))>0 and not bool(records[key].get("direct_neighbor",false)):continue
		var point: Vector3 = points[key]
		if role == "graph":
			if camera.is_position_behind(point):
				continue
			var projected := camera.unproject_position(point)
			var radius := maxf(7.0, float(bounds[key].x) * 0.5 / _pixel_world(point))
			var depth := origin.distance_to(point)
			if projected.distance_to(screen) <= radius and depth < nearest:
				nearest = depth
				result = key
		else:
			var normal:Vector3=camera.global_basis.z
			var denominator:=direction.dot(normal)
			if absf(denominator) < 0.000001:
				continue
			var distance := (point-origin).dot(normal)/denominator
			if distance <= 0 or distance >= nearest:
				continue
			var hit := origin + direction * distance
			var b: Vector2 = bounds[key]
			var local_hit:Vector3=camera.global_basis.inverse()*(hit-point)
			if absf(local_hit.x) <= b.x * 0.5 and absf(local_hit.y) <= b.y * 0.5:
				nearest = distance
				result = key
	return result

func screen_point(key: String) -> Vector2:
	return camera.unproject_position(points[key]) if points.has(key) else Vector2(-10000, -10000)

func screen_card_rect(key: String) -> Rect2:
	if not points.has(key): return Rect2()
	var point: Vector3 = points[key]
	var half: Vector2 = bounds[key] * 0.5
	var rectangle := Rect2(camera.unproject_position(point + camera.global_basis*Vector3(-half.x, half.y, 0)), Vector2.ZERO)
	for corner in [Vector3(half.x, half.y, 0), Vector3(half.x, -half.y, 0), Vector3(-half.x, -half.y, 0)]:
		rectangle = rectangle.expand(camera.unproject_position(point + camera.global_basis*corner))
	return rectangle


func pick_edge(screen: Vector2) -> Array:
	var best := 7.0
	var result: Array = []
	for edge in edge_screen:
		for i in range(1, edge.points.size()):
			if not bool(edge.visible[i - 1]) or not bool(edge.visible[i]):
				continue
			var point := Geometry2D.get_closest_point_to_segment(screen, edge.points[i - 1], edge.points[i])
			var distance := point.distance_to(screen)
			if distance < best:
				best = distance
				result = edge.relations
	return result

func _pixel_world(point: Vector3) -> float:
	var height := maxf(1.0, size.y)
	if role != "graph":
		return camera.size / height
	var depth := maxf(0.1, -camera.to_local(point).z)
	return 2.0 * depth * tan(deg_to_rad(camera.fov) * 0.5) / height

func _rebuild_edge_geometry(selected_only:bool=false) -> void:
	var vertices := PackedVector3Array()
	var colors := PackedColorArray()
	if not selected_only:edge_screen.clear()
	else:edge_screen=edge_screen.filter(func(edge):return not bool(edge.get("semantic",false)))
	for edge in edge_paths:
		var incident:=_key_is_selected(edge.from) or _key_is_selected(edge.to)
		if selected_only and not incident and str(edge.kind)!="semantic":continue
		if not selected_only and role=="canvas" and str(edge.kind)=="semantic":continue
		var path_points: PackedVector3Array = edge.points
		var active := selected_only and incident
		var semantic := str(edge.kind) == "semantic"
		if role != "graph" and not semantic and _key_is_selected(edge.from) and int(records.get(edge.from, {}).get("depth", 0)) == 0:
			active = _key_is_selected(edge.to)
		var color := _color(records.get(edge.from, {})).lerp(PresentationTheme.MUTED,0.25)
		color.a = 0.9 if active else (0.5 if semantic else 0.43)
		var width := 1.8 if role != "graph" else 1.0
		if role == "graph":
			color = Color("f0d6a5") if active else Color(0.42, 0.61, 0.71, 0.18)
			width = 1.8 if active else 0.85
		# Optional per-edge presentation only. Existing roles, typed line
		# styles and the independent selection layer retain their defaults.
		width = float(edge.get("width_px", width))
		color.a *= float(edge.get("opacity", 1.0))
		var clip_frustum := bool(edge.get("clip_frustum", false))
		var track_pick := not selected_only or (semantic and role == "canvas")
		var screens := PackedVector2Array()
		var visible := PackedByteArray()
		if not clip_frustum:
			for p in path_points:
				screens.append(camera.unproject_position(p))
				visible.append(0 if camera.is_position_behind(p) else 1)
			if track_pick:edge_screen.append({"points": screens, "visible": visible, "relations": edge.relations,"semantic":semantic})
		var style:=str(edge.get("visual",{}).get("line_style","solid"))
		var travelled:=0.0
		for i in range(1,path_points.size()):
			var a:=path_points[i-1];var b:=path_points[i]
			if clip_frustum:
				# Clip actual world segments before projection. Each surviving
				# segment owns its pick interval, with no bridge across a culled gap.
				var original_a:=a;var original_b:=b
				if not camera.is_position_in_frustum(a):
					var entry:=Geometry3D.segment_intersects_convex(original_a,original_b,camera.get_frustum())
					if entry.is_empty():continue
					a=entry[0]
				if not camera.is_position_in_frustum(b):
					var entry:=Geometry3D.segment_intersects_convex(original_b,original_a,camera.get_frustum())
					if entry.is_empty():continue
					b=entry[0]
			var direction:=b-a
			if direction.length_squared()<0.00000001:continue
			if clip_frustum and track_pick:
				edge_screen.append({"points":PackedVector2Array([camera.unproject_position(a),camera.unproject_position(b)]),"visible":PackedByteArray([1,1]),"relations":edge.relations,"semantic":semantic})
			var middle:Vector3=(a+b)*0.5
			var unit:=_pixel_world(middle)
			var normal:=direction.cross(camera.global_basis.z).normalized()
			if normal.length_squared()<0.00001:normal=camera.global_basis.x
			var length_pixels:=direction.length()/unit
			var start:=0.0
			while start<length_pixels:
				var step:=(length_pixels-start) if style in ["solid","double"] else minf(2.0,length_pixels-start)
				var phase:=fposmod(travelled+start,24.0)
				var draw_segment:=style in ["solid","double"] or (style=="long_dash" and phase<15) or (style=="dotted" and fposmod(phase,8)<2.6) or (style=="dash_dot" and (phase<12 or (phase>17 and phase<20)))
				if draw_segment:
					var offsets:Array=[0.0] if style!="double" else [-1.8,1.8]
					for offset:float in offsets:
						var p:Vector3=a+direction*(start/length_pixels)+normal*unit*offset
						var q:Vector3=a+direction*((start+step)/length_pixels)+normal*unit*offset
						var half:Vector3=normal*unit*width*0.5
						for vertex in [p-half,p+half,q+half,p-half,q+half,q-half]:vertices.append(vertex);colors.append(color)
				start+=step
			travelled+=length_pixels
		if bool(edge.get("visual",{}).get("arrow",true)) and path_points.size()>=2 and (not clip_frustum or camera.is_position_in_frustum(path_points[-1])):
			var b:=path_points[-1];var direction:Vector3=(b-path_points[-2]).normalized()
			var normal:=direction.cross(camera.global_basis.z).normalized();var unit:=_pixel_world(b)
			for p in [b,b-direction*unit*7+normal*unit*3,b-direction*unit*7-normal*unit*3]:vertices.append(p);colors.append(color)
	var mesh := ArrayMesh.new()
	if not vertices.is_empty():
		var arrays: Array = []
		arrays.resize(Mesh.ARRAY_MAX)
		arrays[Mesh.ARRAY_VERTEX] = vertices
		arrays[Mesh.ARRAY_COLOR] = colors
		mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, arrays)
	if selected_only:_selection_edges.mesh=mesh
	else:edge_mesh.mesh = mesh

func _update_labels() -> void:
	var occupied: Array[Rect2] = []
	var ordered: Array = labels.keys()
	ordered.sort_custom(func(a, b):
		var a_priority := 100000.0 if _key_is_selected(a) else (50000.0 if a == hovered_key else float(records[a].get("degree", 0)))
		var b_priority := 100000.0 if _key_is_selected(b) else (50000.0 if b == hovered_key else float(records[b].get("degree", 0)))
		return a_priority > b_priority)
	var visible_count := 0
	var label_budget := (5 if size.x < 500 else 30) if role == "graph" else (6 if role == "tree" and size.x < 500 else labels.size())
	for key in ordered:
		var label: Label3D = labels[key]
		var point: Vector3 = points[key]
		var active: bool = _key_is_selected(key) or key == hovered_key
		var screen := camera.unproject_position(point)
		var on_screen := not camera.is_position_behind(point) and Rect2(Vector2(-40, -40), size + Vector2(80, 80)).has_point(screen)
		if role == "canvas":
			# A card can enter the viewport before its center does. Its cached
			# title belongs to the visible card, not a center-point culling test.
			var card_rect := screen_card_rect(key)
			on_screen = not camera.is_position_behind(point) and Rect2(Vector2.ZERO, size).intersects(card_rect)
		var allowed := on_screen and visible_count < label_budget
		if role=="canvas" and not bool(source_layout.get("flat_layout",false)) and int(records[key].get("layer_distance",0))>0 and not bool(records[key].get("direct_neighbor",false)) and not active:allowed=false
		var screen_height := float(bounds[key].y) / _pixel_world(point)
		if role != "graph":
			# Card geometry and text share fixed world units. Camera zoom must
			# never reshape every title to maintain a tiny screen-space font.
			# Hide unreadable labels; the selected/hovered callout remains legible.
			var text_pixels := label.pixel_size * float(label.font_size) / _pixel_world(point)
			allowed = allowed and text_pixels >= 11.0 and screen_height >= 14.0 and float(bounds[key].x) / _pixel_world(point) >= 42.0
		else:
			allowed = allowed and (active or orbit_distance < 25)
		if not allowed:
			label.visible = false
			if role=="canvas":
				for child in label.get_parent().get_children():
					if child is Label3D:child.visible=false
			continue
		if role == "graph":
			# Label3D's existing billboard material keeps Node3D scale (Godot
			# 4.7.2 get_material_for_2d). Transform cached glyphs on the GPU;
			# pixel_size would queue a fresh local glyph mesh on every zoom.
			var desired := _pixel_world(label.global_position)*15.0/(float(label.font_size)*label.pixel_size)
			if not is_equal_approx(label.scale.x,desired):label.scale=Vector3.ONE*desired
		if role=="graph":
			label.position=points[key]+Vector3(0,0.45,0)
			var caption:=_label_screen_rect(str(key));var shift:=Vector2.ZERO
			shift.x=maxf(4-caption.position.x,0)+minf(size.x-4-caption.end.x,0)
			shift.y=maxf(4-caption.position.y,0)+minf(size.y-4-caption.end.y,0)
			label.position+=(camera.global_basis.x*shift.x-camera.global_basis.y*shift.y)*_pixel_world(label.global_position)
		var rectangle := _label_screen_rect(str(key)).grow(4)
		if allowed and not active and role != "canvas":
			for existing in occupied:
				if existing.intersects(rectangle):
					allowed = false
					break
		label.visible = allowed
		if role=="canvas":
			for child in label.get_parent().get_children():
				if child is Label3D and child!=label:child.visible=allowed
		if allowed:
			visible_count += 1
			occupied.append(rectangle)
	_update_card_controls()
	_update_callout()
	presentation_updated.emit()

func _update_card_controls() -> void:
	for key in card_controls:
		var controls: Array = card_controls[key]
		var point: Vector3 = points[key]
		var pixel := _pixel_world(point)
		var width := float(bounds[key].x) / pixel
		var height := float(bounds[key].y) / pixel
		var center := camera.unproject_position(point)
		var half: Vector2 = bounds[key] * 0.5
		var top_right := camera.unproject_position(point + camera.global_basis*Vector3(half.x, half.y, 0))
		var bottom_right := camera.unproject_position(point + camera.global_basis*Vector3(half.x, -half.y, 0))
		var edge_height := absf(bottom_right.y - top_right.y)
		var visible := not (role=="canvas" and int(records[key].get("layer_distance",0))>0 and not bool(records[key].get("direct_neighbor",false))) and not camera.is_position_behind(point) and width >= (70 if _key_is_selected(key) else 100) and edge_height >= (24 if _key_is_selected(key) else 34) and Rect2(Vector2.ZERO, size).intersects(Rect2(center - Vector2(width,height)*0.5,Vector2(width,height)))
		var button_height := minf(20, (edge_height - 2) / 2)
		var top := (top_right.y + bottom_right.y) * 0.5 - (button_height * 2 + 2) * 0.5
		for i in controls.size():
			var button: Button = controls[i]
			button.visible = visible
			button.size = Vector2(22, maxf(16, button_height))
			button.position = Vector2(minf(top_right.x, bottom_right.x) - 25, top + i * (button_height + 2))

func set_visible_markers(keys:Array)->void:
	visible_markers=keys.duplicate()
	set_focus_region([])
	var vertices:=PackedVector3Array();var colors:=PackedColorArray()
	for key in visible_markers:
		if not points.has(key):continue
		var p:Vector3=points[key];p.z=0.4
		var unit:=_pixel_world(p)*3
		for offset in [Vector3(-unit,0,0),Vector3(0,unit,0),Vector3(unit,0,0),Vector3(-unit,0,0),Vector3(unit,0,0),Vector3(0,-unit,0)]:
			vertices.append(p+offset);colors.append(PresentationTheme.SELECTED)
	var mesh:=ArrayMesh.new()
	if not vertices.is_empty():
		var arrays:Array=[];arrays.resize(Mesh.ARRAY_MAX);arrays[Mesh.ARRAY_VERTEX]=vertices;arrays[Mesh.ARRAY_COLOR]=colors
		mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES,arrays)
	focus_mesh.mesh=mesh;viewport_3d.render_target_update_mode=SubViewport.UPDATE_ONCE

func set_focus_region(corners: Array) -> void:
	focus_region = corners.duplicate()
	var vertices := PackedVector3Array()
	var colors := PackedColorArray()
	if role == "tree" and focus_region.size() == 4:
		for index in 4:
			var a: Vector3 = focus_region[index]
			var b: Vector3 = focus_region[(index + 1) % 4]
			a.z = 0.4
			b.z = 0.4
			var normal := (b - a).cross(camera.global_basis.z).normalized() * _pixel_world((a + b) * 0.5) * 0.65
			for p in [a - normal, a + normal, b + normal, a - normal, b + normal, b - normal]:
				vertices.append(p)
				colors.append(Color(0.70, 0.88, 0.92, 0.65))
	var mesh := ArrayMesh.new()
	if not vertices.is_empty():
		var arrays: Array = []
		arrays.resize(Mesh.ARRAY_MAX)
		arrays[Mesh.ARRAY_VERTEX] = vertices
		arrays[Mesh.ARRAY_COLOR] = colors
		mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, arrays)
	focus_mesh.mesh = mesh
	viewport_3d.render_target_update_mode = SubViewport.UPDATE_ONCE

func _update_callout() -> void:
	focus_callout.hide()
	if role == "tree": return
	if role == "graph":
		return
	var key := hovered_key if not hovered_key.is_empty() else selected_occurrence
	if not points.has(key) or camera.is_position_behind(points[key]):
		return
	var screen := screen_point(key)
	if not Rect2(Vector2.ZERO, size).has_point(screen):
		return
	# At overview scale use one readable anchor, not hundreds of enlarged
	# labels. The world geometry and picking are still genuine 3D.
	if bool(labels[key].visible) and float(bounds[key].x) / _pixel_world(points[key]) >= 130:
		return
	focus_callout.text = str(records[key].get("title", key)).replace("\n", " ")
	focus_callout.custom_minimum_size = Vector2.ZERO
	focus_callout.size = Vector2(minf(240, maxf(80, size.x - 16)), 28)
	focus_callout.position = Vector2(clampf(screen.x - focus_callout.size.x * 0.5, 4, maxf(4, size.x - focus_callout.size.x - 4)),
		clampf(screen.y + 14, 4, maxf(4, size.y - 32)))
	focus_callout.show()

func _label_screen_rect(key: String) -> Rect2:
	if not labels.has(key):
		return Rect2()
	var label: Label3D = labels[key]
	var center := camera.unproject_position(label.global_position)
	if role == "graph":
		# Billboard text is centered at the LABEL, not the sphere. Use font
		# metrics, including CJK glyph widths, rather than character counts.
		var metrics := _font(label).get_multiline_string_size(label.text, HORIZONTAL_ALIGNMENT_LEFT, -1, label.font_size)
		metrics *= label.pixel_size*label.scale.x / _pixel_world(label.global_position)
		return Rect2(center - metrics * 0.5, metrics)
	var local_bounds := label.get_aabb()
	var result := Rect2()
	var first := true
	for corner in 8:
		var screen := camera.unproject_position(label.to_global(local_bounds.get_endpoint(corner)))
		if first:
			result = Rect2(screen, Vector2.ZERO)
			first = false
		else:
			result = result.expand(screen)
	return result

func debug_state() -> Dictionary:
	return {"role": role, "node_count": points.size(), "edge_count": edge_paths.size(), "projection": camera.projection, "true_3d": world is Node3D and camera is Camera3D, "visible_labels": labels.values().filter(func(label): return label.visible).size(), "camera": get_camera_state()}
