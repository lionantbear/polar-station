extends RefCounted
## Pane composition adapter. Call after initial view construction and after
## newly created geometry/reader content, never on each frame or camera move.
const VisualTheme = preload("res://addons/polar_graphics/visual_theme.gd")
const Trim = preload("res://addons/polar_graphics/themes/seasonal_trim.gd")
const STATE := "polar_visual_theme_state"
const ORIGINAL := "polar_visual_theme_original"

static func apply(workspace: Control, theme_id: String, resource_path: String = "") -> bool:
	# Load and validate the entire packaged input before touching live visuals.
	var visual = VisualTheme.load_theme(theme_id,resource_path)
	if visual == null or not is_instance_valid(workspace):return false
	workspace.set_meta(STATE, visual)
	_apply_node(workspace, visual)
	var trim := workspace.get_node_or_null("PolarSeasonalTrim")
	if trim == null:
		trim = Trim.new();trim.name = "PolarSeasonalTrim";trim.z_index = 30
		workspace.add_child(trim)
	trim.apply_visual_theme(visual)
	# One redraw of existing render targets; no relayout or animation preference.
	for field in ["canvas", "tree_view", "graph_view"]:
		var view = _property(workspace, field)
		if view != null:_redraw_view(view)
	var depth = _property(workspace, "depth_browser")
	if depth != null:
		if depth.has_method("apply_visual_theme"):depth.apply_visual_theme(visual)
		var forest = _property(depth, "forest")
		if forest != null:_redraw_view(forest)
		var detail = _property(depth, "detail_sheet")
		if detail != null:
			var viewport = _property(detail, "viewport")
			if viewport is SubViewport and workspace.is_visible_in_tree() and depth.is_visible_in_tree() and detail.visible:viewport.render_target_update_mode = SubViewport.UPDATE_ONCE
	workspace.queue_redraw()
	return true

static func _property(object: Object, name: String):
	for entry in object.get_property_list():
		if entry.name == name:return object.get(name)
	return null

static func _original(object: Object, key: String, value):
	var originals: Dictionary = object.get_meta(ORIGINAL, {})
	if not originals.has(key):originals[key] = value;object.set_meta(ORIGINAL, originals)
	return originals[key]

static func _set_token(object: Object, property: String, value, is_default: bool) -> void:
	var baseline = _original(object, property, object.get(property))
	object.set(property, baseline if is_default else value)

static func _apply_node(node: Node, visual: Resource) -> void:
	var is_default: bool = visual.id == "default"
	if node.has_method("apply_visual_theme"):node.apply_visual_theme(visual)
	if node is Control:_control(node, visual)
	if node is ColorRect and node.color.get_luminance() < .3:
		_set_token(node, "color", Color(visual.background, node.color.a), is_default)
	if node is WorldEnvironment and node.environment != null:
		# SpatialView and DepthForest construct Environment/Sky per view.
		# Reuse those owned resources: replacing an unrendered Sky leaves an
		# orphan in Godot 4.7.2 GLES3's dirty_sky_list (two radiance textures).
		var environment: Environment = node.environment
		_set_token(environment, "background_color", visual.background, is_default)
		_set_token(environment, "ambient_light_color", visual.ambient, is_default)
		if environment.sky != null and environment.sky.sky_material is ProceduralSkyMaterial:
			_set_token(environment.sky.sky_material, "sky_top_color", visual.sky_top, is_default)
			_set_token(environment.sky.sky_material, "sky_horizon_color", visual.sky_horizon, is_default)
	if node is Light3D:_set_token(node, "light_color", visual.key_light if node is DirectionalLight3D else visual.fill_light, is_default)
	if node is MeshInstance3D:_mesh(node, visual)
	if node is MultiMeshInstance3D and node.material_override is StandardMaterial3D:
		# Keep per-instance source colors intact; only frost the existing solid.
		var material: StandardMaterial3D = _owned_material(node)
		_set_token(material, "roughness", visual.surface_roughness, is_default)
	for child in node.get_children():_apply_node(child, visual)

static func _control(control: Control, visual: Resource) -> void:
	var is_default: bool = visual.id == "default"
	if control.theme != null or control.has_meta(STATE):
		var baseline = _original(control, "theme", control.theme)
		control.theme = baseline if is_default else visual.controls(baseline)
	for property in control.get_property_list():
		var name: String = property.name
		if name.begins_with("theme_override_styles/"):
			var style = control.get(name)
			if style is StyleBoxFlat:
				var baseline = _original(control, name, style)
				control.set(name, baseline if is_default else visual.panel_style(baseline))
		elif name.begins_with("theme_override_colors/"):
			var color = control.get(name)
			if color is Color and color.s < .25:
				var baseline: Color = _original(control, name, color)
				control.set(name, baseline if is_default else (visual.text if baseline.get_luminance() > .65 else visual.muted))
	control.queue_redraw()

static func _mesh(node: MeshInstance3D, visual: Resource) -> void:
	var material = node.material_override
	if not material is ShaderMaterial or material.shader == null:return
	if material.shader.resource_path != "res://addons/polar_graphics/card.gdshader":return
	material = _owned_material(node)
	# The original renderer mutates selected/context/accent in this same material;
	# only the two visual tokens are touched, including when returning to default.
	var is_default: bool = visual.id == "default"
	for name in ["base_color", "selection_outline"]:
		var baseline = _original(material, name, material.get_shader_parameter(name))
		material.set_shader_parameter(name, baseline if is_default else (visual.card if name == "base_color" else visual.selection))

static func _owned_material(node: GeometryInstance3D) -> Material:
	var material: Material = node.material_override
	if not node.has_meta("polar_visual_theme_material") or node.get_meta("polar_visual_theme_material") != material:
		# Share the immutable Shader, own each parameter dictionary per Pane.
		material = material.duplicate()
		node.material_override = material
		node.set_meta("polar_visual_theme_material", material)
	return material

static func _redraw_view(view: Control) -> void:
	var viewport = _property(view, "viewport_3d")
	if viewport is SubViewport and view.is_visible_in_tree():viewport.render_target_update_mode = SubViewport.UPDATE_ONCE
	view.queue_redraw()
